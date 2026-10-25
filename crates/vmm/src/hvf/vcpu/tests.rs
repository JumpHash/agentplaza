// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. The payload is four arm64 instructions that the test writes into guest
// memory itself, so no kernel is needed. The tests drive this module's own vCPU calls: each exit is
// seen as Hypervisor.framework reports it and decoded by `hvf::exit`.
//
// They need the `com.apple.security.hypervisor` entitlement, so they run under the runner
// (`scripts/hvf-run.sh`, see the `hvf` module's documentation). Without it they say why on stderr
// and pass without running anything.

//! The vCPU's payload tests: a store to an address the VMM does not map, the PSCI call that ends the
//! payload, and a saved vCPU and its memory resumed in a fresh VM.
//!
//! Hypervisor.framework allows one VM per process, so the tests take [`vm_lock`]: a test that
//! creates a VM never overlaps another. Every test is under a [`Watchdog`], so a vCPU that never
//! returns from `hv_vcpu_run` ends the test binary rather than the machine's afternoon.

use std::io::Write as _;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use vm_memory::Bytes;

use super::*;
use crate::arch::aarch64::layout::DRAM_MEM_START;
use crate::hvf::boot;
use crate::hvf::exit::{Exception, MmioAccess};
use crate::hvf::psci::{self, Action, FN_SYSTEM_OFF, Machine};
use crate::vstate::memory::GuestAddress;

/// The guest memory the payload runs in: two MiB at the start of DRAM.
const MEMORY_MIB: usize = 2;

/// The address the payload stores to: the first byte of a 256 MiB window that the VMM maps nothing
/// in (the interrupt controller is below it, the console and the virtio devices above).
const UNMAPPED: u64 = 0x1000_0000;

/// The payload, from the start of DRAM:
///
/// ```text
/// 0x00  movz x1, #0x1000, lsl #16   ; x1 = 0x1000_0000
/// 0x04  movz w2, #0xbeef            ; w2 = 0xbeef
/// 0x08  str  w2, [x1]               ; a 32-bit store the VMM does not map: a data abort exit
/// 0x0c  movz x0, #0x8400, lsl #16   ; x0 = 0x8400_0000 (PSCI, SMC32 range)
/// 0x10  movk x0, #0x8               ; x0 = 0x8400_0008: PSCI SYSTEM_OFF
/// 0x14  hvc  #0                     ; the PSCI conduit: an HVC exit
/// ```
const PAYLOAD: [u32; 6] = [
    0xd2a2_0001,
    0x5297_dde2,
    0xb900_0022,
    0xd2b0_8000,
    0xf280_0100,
    0xd400_0002,
];

/// The store the payload makes, at the start of DRAM plus eight bytes.
const STORE_PC: u64 = DRAM_MEM_START + 0x08;

/// The HVC the payload ends with, at the start of DRAM plus twenty bytes.
const HVC_PC: u64 = DRAM_MEM_START + 0x14;

/// The value the payload stores.
const STORED: u64 = 0xbeef;

/// How long a payload test may run before the watchdog ends the test binary. A payload finishes in
/// milliseconds; the limit only has to outlast a loaded machine.
const WATCHDOG: Duration = Duration::from_secs(60);

/// The process's one VM, serialised: a second `hv_vm_create` in the same process fails with
/// `HV_BUSY`, so two tests that each create a VM would race on it.
static VM_LOCK: Mutex<()> = Mutex::new(());

fn vm_lock() -> MutexGuard<'static, ()> {
    VM_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Ends the test binary if the test it guards is still running after its limit. Dropping the
/// guard, which the test does when it finishes, cancels the watchdog.
struct Watchdog {
    _cancel: mpsc::Sender<()>,
}

impl Watchdog {
    fn new(name: &'static str, limit: Duration) -> Self {
        let (done, waiting) = mpsc::channel::<()>();
        thread::spawn(move || {
            if let Err(RecvTimeoutError::Timeout) = waiting.recv_timeout(limit) {
                let _ = writeln!(
                    std::io::stderr(),
                    "hvf watchdog: {name} is still running after {limit:?}; ending the test binary"
                );
                std::process::exit(124);
            }
        });
        Self { _cancel: done }
    }
}

/// Whether the tests may create a VM: only under the runner, whose entitlement `hv_vm_create` checks.
/// Otherwise the reason is written to stderr, where it shows even on a passing test.
fn entitled() -> bool {
    if apz_os::runner::runner().is_empty() {
        let _ = writeln!(
            std::io::stderr(),
            "skipped: the HVF payload tests need the hypervisor entitlement; run them with \
             CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER=$PWD/scripts/hvf-run.sh cargo test -p vmm"
        );
        return false;
    }
    true
}

/// Writes the payload at the start of DRAM, where the vCPU starts.
fn load_payload(memory: &GuestMemoryMmap) {
    let bytes: Vec<u8> = PAYLOAD.iter().flat_map(|word| word.to_le_bytes()).collect();
    memory
        .write_slice(&bytes, GuestAddress(DRAM_MEM_START))
        .expect("the payload fits in DRAM");
}

/// Asserts that the last run ended in an exception and decodes it.
fn exception_of_last_run(vcpu: &HvfVcpu) -> Exception {
    assert_eq!(
        vcpu.exit_reason(),
        sys::HV_EXIT_REASON_EXCEPTION,
        "the run ended for a reason other than an exception"
    );
    let (syndrome, _, _) = vcpu.exception();
    Exception::decode(syndrome)
}

#[test]
fn an_unmapped_store_exits_with_its_address_width_and_value_then_psci_system_off() {
    if !entitled() {
        return;
    }
    let _one_vm = vm_lock();
    let _watchdog = Watchdog::new("the store and PSCI payload", WATCHDOG);

    let memory = boot::guest_memory(MEMORY_MIB).expect("guest memory");
    load_payload(&memory);
    let mut vm = HvfVm::create().expect("a VM");
    vm.map_guest_memory(&memory).expect("guest memory mapped");
    let vcpu = HvfVcpu::create().expect("a vCPU");
    vcpu.configure(0, 0).expect("the vCPU is configured");
    vcpu.set_reg(sys::HV_REG_PC, DRAM_MEM_START)
        .expect("the PC is set");

    vcpu.run().expect("the payload runs to its store");
    let access = match exception_of_last_run(&vcpu) {
        Exception::Mmio(access) => access,
        other => panic!("the store exited as {other:?}, not as a device access"),
    };
    assert_eq!(
        access,
        MmioAccess {
            read: false,
            rt: 2,
            len: 4,
            sign_extend: false,
            valid: true,
            cache_maintenance: false,
        },
        "the store decodes to a 32-bit write from X2"
    );
    let (_, virtual_address, physical_address) = vcpu.exception();
    assert_eq!(
        physical_address, UNMAPPED,
        "the faulting IPA is the store's address"
    );
    assert_eq!(
        virtual_address, UNMAPPED,
        "with the MMU off, the VA is the address too"
    );
    assert_eq!(
        vcpu.get_reg(sys::HV_REG_X0 + 2).expect("X2"),
        STORED,
        "the stored value is in the register the syndrome names"
    );
    assert_eq!(
        vcpu.get_reg(sys::HV_REG_PC).expect("the PC"),
        STORE_PC,
        "the exit reports the store's own instruction"
    );

    // What the run loop does for a completed access: step over the store, then run on.
    vcpu.advance_pc().expect("the PC is advanced");
    vcpu.run().expect("the payload runs to its HVC");
    let function = match exception_of_last_run(&vcpu) {
        Exception::Hvc { imm: 0 } => vcpu.get_reg(sys::HV_REG_X0).expect("X0"),
        other => panic!("the payload's HVC exited as {other:?}"),
    };
    assert_eq!(function, FN_SYSTEM_OFF, "the payload calls PSCI SYSTEM_OFF");
    assert_eq!(
        psci::handle(function, 0, 0, 0, &Machine { running: &[true] }),
        Action::SystemOff { reset: false },
        "the PSCI dispatcher ends the VM"
    );
    assert_eq!(
        vcpu.get_reg(sys::HV_REG_PC).expect("the PC"),
        HVC_PC + 4,
        "the HVC's return address is the instruction after it"
    );
}

#[test]
fn a_saved_vcpu_and_its_memory_resume_in_a_fresh_vm() {
    if !entitled() {
        return;
    }
    let _one_vm = vm_lock();
    let _watchdog = Watchdog::new("the save and resume payload", WATCHDOG);

    // The first VM runs to the store; the store is completed as the run loop would complete it, and
    // the vCPU and its memory are saved there.
    let (state, memory_bytes) = {
        let memory = boot::guest_memory(MEMORY_MIB).expect("guest memory");
        load_payload(&memory);
        let mut vm = HvfVm::create().expect("a VM");
        vm.map_guest_memory(&memory).expect("guest memory mapped");
        let vcpu = HvfVcpu::create().expect("a vCPU");
        vcpu.configure(0, 0).expect("the vCPU is configured");
        vcpu.set_reg(sys::HV_REG_PC, DRAM_MEM_START)
            .expect("the PC is set");
        vcpu.run().expect("the payload runs to its store");
        assert!(
            matches!(exception_of_last_run(&vcpu), Exception::Mmio(_)),
            "the first VM stops at the store"
        );
        vcpu.advance_pc().expect("the PC is advanced");
        let state = vcpu.save_state().expect("the vCPU is saved");
        let mut memory_bytes = vec![0u8; MEMORY_MIB << 20];
        memory
            .read_slice(&mut memory_bytes, GuestAddress(DRAM_MEM_START))
            .expect("the memory is read");
        (state, memory_bytes)
        // The vCPU, then the VM, then the memory are dropped here, so the first VM is gone before
        // the second is created.
    };

    let memory = boot::guest_memory(MEMORY_MIB).expect("guest memory");
    memory
        .write_slice(&memory_bytes, GuestAddress(DRAM_MEM_START))
        .expect("the saved memory is written back");
    let mut vm = HvfVm::create().expect("a second VM, after the first was destroyed");
    vm.map_guest_memory(&memory).expect("guest memory mapped");
    let mut vcpu = HvfVcpu::create().expect("a vCPU");
    vcpu.configure(0, 0).expect("the vCPU is configured");
    vcpu.restore_state(&state)
        .expect("the saved state is restored");
    assert_eq!(
        vcpu.get_reg(sys::HV_REG_X0 + 2).expect("X2"),
        STORED,
        "the saved register comes back"
    );

    vcpu.run().expect("the restored payload runs to its HVC");
    match exception_of_last_run(&vcpu) {
        Exception::Hvc { imm: 0 } => {}
        other => panic!("the restored payload exited as {other:?}, not as its HVC"),
    }
    assert_eq!(
        vcpu.get_reg(sys::HV_REG_X0).expect("X0"),
        FN_SYSTEM_OFF,
        "the restored payload reaches PSCI SYSTEM_OFF, which the saved memory holds"
    );
}
