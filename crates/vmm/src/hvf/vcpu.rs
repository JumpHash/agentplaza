// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. The exception handling, the halted-vCPU timer trick
// and the virtual-timer bookkeeping follow the decisions of the reference
// Hypervisor.framework VMM (QEMU's `target/arm/hvf/hvf.c`, GPL-2.0, read for
// its protocol - no code copied); the timer arithmetic comes from the Arm
// architecture's description of CNTV_CTL_EL0/CNTV_CVAL_EL0.

//! The vCPUs of an HVF VM: one thread each, their run loop, and the exits
//! they hand to the devices.
//!
//! # The run loop
//!
//! A vCPU thread creates its vCPU, applies the boot state, and then loops:
//! mirror the interrupt controller's line for this vCPU into
//! `hv_vcpu_set_pending_interrupt`, `hv_vcpu_run`, and act on the exit. Exits
//! are a canceled run (the VM was stopped or another vCPU was kicked), a
//! timer activation, or a guest exception - which [`crate::hvf::exit`]
//! decodes and this module answers for (device access, `MCC_*` system
//! register, `WFI`, PSCI call).
//!
//! # Idling and the virtual timer
//!
//! Hypervisor.framework reports a fired virtual timer only from
//! `hv_vcpu_run`, so a vCPU that has executed `WFI` cannot wait for it
//! passively: the loop computes the timer's deadline from `CNTV_CTL_EL0` and
//! `CNTV_CVAL_EL0`, sleeps on a condition variable for no longer than that,
//! and runs the vCPU again - which is where the framework reports the timer
//! and the timer interrupt is raised into the controller. Without this the
//! guest hangs the first time it goes idle.
//!
//! # Lock order
//!
//! A halted vCPU takes its own slot lock and then the interrupt controller's
//! lock, so the controller's lock is never held while a slot lock is taken: a
//! path that changes the controller goes through [`SharedVm::with_gic`], which
//! releases it before waking anybody. `crate::hvf::wait` holds the other end
//! of that rule.

// FFI boundary (see the allowlist in `lib.rs`): the vCPU backend drives
// Hypervisor.framework directly — `hv_vcpu_create`/`run`/`destroy`, the
// register accessors, the vtimer, `mach_absolute_time`, and the read of the
// exit structure `hv_vcpu_run` filled in.
#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::hvf::exit::{Exception, MmioAccess, Wait};
use crate::hvf::gic::{self, Gic, GicMmio};
use crate::hvf::psci::{self, Action, Machine};
use crate::hvf::sys::{self, HvfError};
use crate::hvf::uart::SerialDevice;
use crate::hvf::vm::HvfVm;
use crate::hvf::wait::{self, Start, WaitSlot, wake_asserted};
use crate::spawner::VmRuntime;
use crate::vstate::bus::BusDevice;
use crate::vstate::memory::GuestMemoryMmap;

/// PSTATE for EL1h with D, A, I and F set: the state the Linux boot protocol
/// hands the kernel in.
const PSTATE_EL1H_MASKED: u64 = 0x3c5;

/// `SCTLR_EL1` for a Linux guest: the Apple Silicon boot value with `SPAN`
/// set, so that `PSTATE.PAN` is not changed on exception entry (what the
/// reference HVF backend boots with, from the Asahi Linux `m1n1` value).
const SCTLR_EL1_BOOT: u64 = 0x3010_0180 | 0x0080_0000;

/// The `GIC` field of `ID_AA64PFR0_EL1` (bits 27:24): 1 advertises the GICv3
/// system-register interface, which is what the userspace controller
/// implements.
const ID_AA64PFR0_GIC_SHIFT: u64 = 24;

/// The offset in the exception vector table of a synchronous exception taken
/// from the current exception level using that level's stack pointer.
const VECTOR_SYNCHRONOUS_SPX: u64 = 0x200;

/// `ESR_ELx.EC` for an undefined instruction (what the backend injects when
/// the guest executes something it cannot emulate).
const EC_UNKNOWN: u64 = 0x00;
/// `ESR_ELx.EC` for a data abort taken without a change of exception level.
const EC_DATAABORT_SAME_EL: u64 = 0x25;
/// `ESR_ELx.ISS.DFSC` for a translation fault at level 0: what a guest sees
/// when it touches an address the VM has no memory at.
const DFSC_TRANSLATION_FAULT_0: u64 = 0b000100;
/// `ESR_ELx.IL`: the instruction is 32 bits.
const ESR_IL: u64 = 1 << 25;
/// `ESR_ELx.ISS.WnR`: the faulting access was a write.
const DABORT_WNR: u64 = 1 << 6;

/// The most vCPUs this backend runs: enough for any microVM, and small enough
/// that the per-VM bookkeeping stays a fixed-size array. `boot` refuses a
/// larger count rather than silently running fewer.
pub const MAX_VCPUS: usize = 64;

/// The console's MMIO window: the address the device tree gives the guest,
/// from the aarch64 layout the KVM backend uses as well.
const SERIAL_BASE: u64 = crate::arch::aarch64::layout::SERIAL_MEM_START;
/// The length of a device's MMIO window.
const SERIAL_LEN: u64 = crate::arch::aarch64::layout::MMIO_LEN;

/// The address the guest starts executing at, and the device tree it is given
/// in `X0`.
#[derive(Debug, Clone, Copy)]
pub struct BootParams {
    /// The kernel entry point.
    pub entry: u64,
    /// The address of the device tree blob.
    pub fdt: u64,
}

/// A vCPU handle.
///
/// Hypervisor.framework requires a vCPU to be created, run, have every
/// register accessed and be destroyed on one thread, so this type is created
/// by [`run_vcpu`] on the vCPU's own thread and never leaves it. The exit
/// structure is read through a raw pointer that stays valid until the vCPU is
/// destroyed.
#[derive(Debug)]
pub struct HvfVcpu {
    id: sys::hv_vcpu_t,
    exit: *mut sys::hv_vcpu_exit_t,
    /// Whether the framework currently has this vCPU's virtual timer masked
    /// (it masks it when it reports `HV_EXIT_REASON_VTIMER_ACTIVATED`, and the
    /// backend unmasks it once the guest has cleared the timer).
    vtimer_masked: bool,
}

impl HvfVcpu {
    /// Creates the vCPU for the calling thread.
    fn create() -> Result<Self, HvfError> {
        let mut id: sys::hv_vcpu_t = 0;
        let mut exit: *mut sys::hv_vcpu_exit_t = std::ptr::null_mut();
        sys::hv_try!(hv_vcpu_create(&mut id, &mut exit, sys::NO_VCPU_CONFIG))?;
        Ok(Self {
            id,
            exit,
            vtimer_masked: false,
        })
    }

    /// The handle, which other threads may pass to `hv_vcpus_exit`.
    fn id(&self) -> sys::hv_vcpu_t {
        self.id
    }

    /// This vCPU's general-purpose register `reg`.
    fn get_reg(&self, reg: sys::hv_reg_t) -> Result<u64, HvfError> {
        let mut value = 0u64;
        sys::hv_try!(hv_vcpu_get_reg(self.id, reg, &mut value))?;
        Ok(value)
    }

    /// Sets this vCPU's general-purpose register `reg`.
    fn set_reg(&self, reg: sys::hv_reg_t, value: u64) -> Result<(), HvfError> {
        sys::hv_try!(hv_vcpu_set_reg(self.id, reg, value))
    }

    /// This vCPU's system register `reg`.
    fn get_sys_reg(&self, reg: sys::hv_sys_reg_t) -> Result<u64, HvfError> {
        let mut value = 0u64;
        sys::hv_try!(hv_vcpu_get_sys_reg(self.id, reg, &mut value))?;
        Ok(value)
    }

    /// Sets this vCPU's system register `reg`.
    fn set_sys_reg(&self, reg: sys::hv_sys_reg_t, value: u64) -> Result<(), HvfError> {
        sys::hv_try!(hv_vcpu_set_sys_reg(self.id, reg, value))
    }

    /// Asserts or de-asserts this vCPU's interrupt line.
    fn set_irq_line(&self, asserted: bool) -> Result<(), HvfError> {
        sys::hv_try!(hv_vcpu_set_pending_interrupt(
            self.id,
            sys::HV_INTERRUPT_TYPE_IRQ,
            asserted
        ))
    }

    /// Runs the vCPU until its next exit.
    fn run(&self) -> Result<(), HvfError> {
        sys::hv_try!(hv_vcpu_run(self.id))
    }

    /// Why the last run returned.
    fn exit_reason(&self) -> u32 {
        // SAFETY: `self.exit` is the pointer `hv_vcpu_create` filled in for
        // this vCPU; it stays valid until this thread destroys the vCPU, and
        // it is only ever read on this thread.
        unsafe { (*self.exit).reason }
    }

    /// The exception information of the last exit: the syndrome, the faulting
    /// *virtual* address (what the guest's `FAR_EL1` has to hold) and the
    /// faulting intermediate physical address (what this backend routes to its
    /// devices).
    fn exception(&self) -> (u64, u64, u64) {
        // SAFETY: as in `exit_reason`: the structure belongs to this vCPU and
        // this thread, and only describes the run that just returned.
        unsafe {
            let exception = (*self.exit).exception;
            (
                exception.syndrome,
                exception.virtual_address,
                exception.physical_address,
            )
        }
    }

    /// Applies the state a vCPU needs before its first run: its identity in
    /// `MPIDR_EL1` (the affinity the device tree and the redistributor both
    /// report as this vCPU's index), its start state in `PSTATE`, and the
    /// identification registers the guest uses to decide how to boot.
    fn configure(&self, index: usize, vtimer_offset: u64) -> Result<(), HvfError> {
        self.set_sys_reg(sys::HV_SYS_REG_MPIDR_EL1, index as u64)?;
        self.set_sys_reg(sys::HV_SYS_REG_SCTLR_EL1, SCTLR_EL1_BOOT)?;
        self.set_reg(sys::HV_REG_CPSR, PSTATE_EL1H_MASKED)?;
        self.apply_id_registers()?;
        sys::hv_try!(hv_vcpu_set_vtimer_offset(self.id, vtimer_offset))?;
        Ok(())
    }

    /// Reads the identification registers the host reports and hands the
    /// guest the ones this backend implements.
    ///
    /// Two adjustments matter. `ID_AA64PFR0_EL1`'s `GIC` field is set to
    /// advertise the GICv3 system-register interface: the guest only takes
    /// that path - and therefore only makes the `ICC_*` accesses this
    /// controller answers - when it is told the interface exists.
    /// `ID_AA64MMFR0_EL1`'s physical address range is clamped to the VM's
    /// intermediate physical address size, so the guest never programs page
    /// tables the VM cannot translate.
    fn apply_id_registers(&self) -> Result<(), HvfError> {
        let (host_pfr0, host_mmfr0) = host_id_registers()?;

        // `ID_AA64PFR0_EL1`: advertise the GICv3 system-register interface.
        let id_aa64pfr0 = host_pfr0 | 1 << ID_AA64PFR0_GIC_SHIFT;
        self.set_sys_reg(sys::HV_SYS_REG_ID_AA64PFR0_EL1, id_aa64pfr0)?;

        // `ID_AA64MMFR0_EL1`: clamp the physical address range to the VM's
        // intermediate physical address size.
        let mut ipa_size = 0u32;
        sys::hv_try!(hv_vm_config_get_default_ipa_size(&mut ipa_size))?;
        let id_aa64mmfr0 = (host_mmfr0 & !0xf) | u64::from(parange_for_ipa_size(ipa_size));
        self.set_sys_reg(sys::HV_SYS_REG_ID_AA64MMFR0_EL1, id_aa64mmfr0)?;
        Ok(())
    }

    /// Steps over the instruction that caused the current exit.
    fn advance_pc(&self) -> Result<(), HvfError> {
        let pc = self.get_reg(sys::HV_REG_PC)?;
        self.set_reg(sys::HV_REG_PC, pc.wrapping_add(4))
    }

    /// Injects an exception into the guest: the fault registers, the return
    /// address and the vector the guest's handler is entered at.
    ///
    /// This is how the backend fails closed on something it cannot emulate -
    /// a reserved system register, or a device access the VM has no device
    /// for - instead of pretending the access succeeded.
    fn inject(
        &self,
        class: u64,
        iss: u64,
        far: Option<u64>,
        return_address: u64,
    ) -> Result<(), HvfError> {
        let vector_base = self.get_sys_reg(sys::HV_SYS_REG_VBAR_EL1)?;
        let pstate = self.get_reg(sys::HV_REG_CPSR)?;
        if let Some(far) = far {
            self.set_sys_reg(sys::HV_SYS_REG_FAR_EL1, far)?;
        }
        self.set_sys_reg(
            sys::HV_SYS_REG_ESR_EL1,
            (class << 26) | ESR_IL | (iss & 0x01ff_ffff),
        )?;
        self.set_sys_reg(sys::HV_SYS_REG_ELR_EL1, return_address)?;
        self.set_sys_reg(sys::HV_SYS_REG_SPSR_EL1, pstate)?;
        self.set_reg(sys::HV_REG_CPSR, PSTATE_EL1H_MASKED)?;
        self.set_reg(
            sys::HV_REG_PC,
            vector_base.wrapping_add(VECTOR_SYNCHRONOUS_SPX),
        )
    }

    /// Injects an undefined instruction: what a guest gets for an `SMC` when
    /// the device tree declares the `hvc` conduit, and for a system register
    /// the backend does not implement.
    fn inject_undef(&self, return_address: u64) -> Result<(), HvfError> {
        self.inject(EC_UNKNOWN, 0, None, return_address)
    }

    /// Injects the data abort a guest gets for an address the VM has no
    /// memory or device at.
    ///
    /// `far` is the guest's *virtual* faulting address, which is what
    /// `FAR_EL1` has to report to a data-abort handler; the intermediate
    /// physical address is this backend's business (device routing) and must
    /// not be handed to the guest in its place.
    fn inject_data_abort(
        &self,
        far: u64,
        write: bool,
        return_address: u64,
    ) -> Result<(), HvfError> {
        let iss = DFSC_TRANSLATION_FAULT_0 | if write { DABORT_WNR } else { 0 };
        self.inject(EC_DATAABORT_SAME_EL, iss, Some(far), return_address)
    }

    /// Mirrors the guest's virtual timer into its interrupt line.
    ///
    /// Hypervisor.framework masks the virtual timer when it reports it, and
    /// stops reporting it until the backend unmasks it again; the guest clears
    /// the timer's condition by programming `CNTV_CVAL_EL0`/`CNTV_CTL_EL0` in
    /// its handler, which the framework applies without exiting. So the line
    /// has to be re-checked after every exception, and the timer unmasked once
    /// the guest has cleared the condition - otherwise the guest never gets a
    /// timer interrupt again.
    fn sync_vtimer(&mut self, index: usize, shared: &SharedVm) -> Result<(), HvfError> {
        if !self.vtimer_masked {
            return Ok(());
        }
        let control = self.get_sys_reg(sys::HV_SYS_REG_CNTV_CTL_EL0)?;
        let armed = wait::CNTV_CTL_ENABLE | wait::CNTV_CTL_ISTATUS;
        let masked =
            control & (wait::CNTV_CTL_ENABLE | wait::CNTV_CTL_IMASK | wait::CNTV_CTL_ISTATUS);
        if masked == armed {
            shared.with_interrupts(|gic| gic.raise_virtual_timer(index));
        } else {
            shared.with_interrupts(|gic| gic.lower_virtual_timer(index));
            sys::hv_try!(hv_vcpu_set_vtimer_mask(self.id, false))?;
            self.vtimer_masked = false;
        }
        Ok(())
    }

    /// How long a halted vCPU may sleep before the guest's virtual timer is
    /// due: `None` when no timer is programmed, or when a timer that has fired
    /// cannot reach the guest yet (see [`wait::idle_deadline`]).
    fn vtimer_sleep(&self, index: usize, shared: &SharedVm) -> Result<Option<Duration>, HvfError> {
        let control = self.get_sys_reg(sys::HV_SYS_REG_CNTV_CTL_EL0)?;
        let compare = self.get_sys_reg(sys::HV_SYS_REG_CNTV_CVAL_EL0)?;
        let now = guest_counter(shared.vtimer_offset);
        let deliverable = shared
            .gic()
            .delivers_private(index, gic::VIRTUAL_TIMER_INTID);
        Ok(wait::idle_deadline(
            control,
            compare,
            now,
            deliverable,
            counter_frequency,
        ))
    }
}

impl Drop for HvfVcpu {
    fn drop(&mut self) {
        // SAFETY: the vCPU was created by this thread (which is the thread
        // dropping it, since nothing else can reach this value).
        let ret = unsafe { sys::hv_vcpu_destroy(self.id) };
        if ret != sys::HV_SUCCESS {
            eprintln!("warning: hv_vcpu_destroy failed: {ret:#010x}");
        }
    }
}

/// Reads the host's `ID_AA64PFR0_EL1` and `ID_AA64MMFR0_EL1` through one vCPU
/// configuration object.
///
/// The object `hv_vcpu_config_create` returns is owned by the caller, so it is
/// released here on every path, including the failures: a VMM that started
/// many vCPUs would otherwise leak one object per vCPU.
fn host_id_registers() -> Result<(u64, u64), HvfError> {
    // SAFETY: `hv_vcpu_config_create` returns an object owned by the caller;
    // it is only read from before it is destroyed below.
    let config = unsafe { sys::hv_vcpu_config_create() };
    let registers =
        feature_reg(config, sys::HV_FEATURE_REG_ID_AA64PFR0_EL1).and_then(|id_aa64pfr0| {
            feature_reg(config, sys::HV_FEATURE_REG_ID_AA64MMFR0_EL1)
                .map(|id_aa64mmfr0| (id_aa64pfr0, id_aa64mmfr0))
        });
    // SAFETY: `config` was created above, this process owns its one reference,
    // and it is released exactly once (an OS object: `os_release`).
    unsafe { sys::os_release(config) };
    registers
}

/// Reads a host feature register through a vCPU configuration object.
fn feature_reg(config: sys::hv_vcpu_config_t, reg: sys::hv_feature_reg_t) -> Result<u64, HvfError> {
    let mut value = 0u64;
    // SAFETY: `config` was returned by `hv_vcpu_config_create` (never a null
    // pointer, which the framework's own API contract makes non-null) and is
    // live until its owner destroys it; the pointer to `value` is to a live
    // local for the duration of the call.
    sys::check("hv_vcpu_config_get_feature_reg", unsafe {
        sys::hv_vcpu_config_get_feature_reg(config, reg, &mut value)
    })?;
    Ok(value)
}

/// The `ID_AA64MMFR0_EL1.PARange` value for an intermediate physical address
/// size: the largest implemented physical address range that fits it.
///
/// `PARange` counts 32, 36, 40, 42, 44, 48 and 52 bits of physical address;
/// the guest is told the range that its intermediate addresses cover, so it
/// never forms an address the VM cannot translate.
fn parange_for_ipa_size(ipa_size: u32) -> u8 {
    const RANGES: [u32; 7] = [32, 36, 40, 42, 44, 48, 52];
    let mut parange = 0u8;
    for (index, bits) in RANGES.iter().enumerate() {
        if *bits <= ipa_size {
            parange = index as u8;
        }
    }
    parange
}

/// The host's view of the clock the guest's virtual timer runs on.
///
/// `libc`'s Mach time functions are marked deprecated in favour of the
/// `mach2` crate, which this port does not depend on; the deprecation is
/// acknowledged in this one place rather than at every call site.
mod host_clock {
    /// The host's virtual counter.
    ///
    /// On Apple Silicon `mach_absolute_time` *is* the architected counter, and
    /// this is the value `hv_vcpu_set_vtimer_offset` shifts the guest's view
    /// of `CNTVCT_EL0` by.
    #[allow(deprecated)]
    pub(super) fn now() -> u64 {
        // SAFETY: `mach_absolute_time` takes no arguments and only reads the
        // host's time base.
        unsafe { libc::mach_absolute_time() }
    }

    /// The counter's frequency, in Hz, from the host's time base.
    #[allow(deprecated)]
    pub(super) fn frequency() -> u64 {
        let mut info = libc::mach_timebase_info { numer: 0, denom: 0 };
        // SAFETY: both fields are plain 32-bit integers; the call fills them
        // in, and its status is checked before they are read.
        let status = unsafe { libc::mach_timebase_info(&mut info) };
        if status != 0 || info.numer == 0 {
            return 24_000_000;
        }
        u64::from(info.denom) * 1_000_000_000 / u64::from(info.numer)
    }
}

/// The host's virtual counter, in the VM's time base.
///
/// `hv_vcpu_set_vtimer_offset` shifts the guest's view of `CNTVCT_EL0` by the
/// offset the VM was created with, and `mach_absolute_time` is that same
/// counter, so the guest's counter is the host's minus the offset. This is the
/// arithmetic the reference HVF backend uses to compute how long a halted vCPU
/// may sleep.
fn guest_counter(vtimer_offset: u64) -> u64 {
    host_clock::now().saturating_sub(vtimer_offset)
}

/// The counter's frequency, in Hz: the guest's `CNTFRQ_EL0` on Apple Silicon
/// is 24 MHz, but deriving it from the host's time base keeps this correct on
/// any host Apple ships.
fn counter_frequency() -> u64 {
    host_clock::frequency()
}

/// One vCPU's slot: what other threads may touch.
#[derive(Debug)]
struct VcpuSlot {
    /// The framework handle, published by the owning thread once the vCPU
    /// exists; zero before that.
    id: AtomicU64,
    /// The wake protocol: whether the vCPU is powered on, and how it is woken
    /// (see [`crate::hvf::wait`]).
    wait: WaitSlot,
}

/// What the vCPUs of one VM share, and what the devices and the spawner
/// reach them through.
pub struct SharedVm {
    /// The framework VM. It is destroyed before the memory is freed, and its
    /// mappings refer to that memory, so the two are kept together for as long
    /// as any vCPU can run. Neither is read after the vCPUs start: they are
    /// here for their lifetime, and the field order below is what orders their
    /// destruction.
    #[allow(dead_code)]
    vm: HvfVm,
    /// The guest memory mapped into the VM.
    #[allow(dead_code)]
    memory: GuestMemoryMmap,
    /// The spawner's per-VM state: the console capture, the VM's id and the
    /// guest-exit flag.
    runtime: Arc<VmRuntime>,
    /// The userspace interrupt controller.
    interrupts: Mutex<Gic>,
    /// The serial console. It is installed after construction because its
    /// interrupt line points back at this VM (which has to exist first); no
    /// vCPU runs before boot installs it.
    serial: Mutex<Option<SerialDevice>>,
    /// Where the guest's view of the virtual counter starts.
    vtimer_offset: u64,
    /// The vCPUs.
    slots: Vec<Arc<VcpuSlot>>,
    /// Set when the VM should stop.
    stop: AtomicBool,
    /// The first fatal backend error, for the report the spawner's caller
    /// gets after the VM has stopped.
    failure: Mutex<Option<String>>,
    /// Whether at least one vCPU thread has created its vCPU.
    started: Mutex<bool>,
    /// Taken and notified when a vCPU is created or the VM fails to start.
    started_wake: Condvar,
    /// The address the guest starts at.
    boot: BootParams,
}

impl std::fmt::Debug for SharedVm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedVm")
            .field("vcpus", &self.slots.len())
            .field("vtimer_offset", &self.vtimer_offset)
            .field("stop", &self.stop.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl SharedVm {
    /// Creates the shared state of a VM with `vcpu_count` vCPUs, none of
    /// which are running yet.
    ///
    /// `vm` and `memory` are moved in and kept alive for the lifetime of the
    /// VM: the framework's mappings refer to `memory`, and `vm` has to be
    /// destroyed before it is freed. The declaration order of the fields is
    /// what makes that happen.
    pub fn new(
        vm: HvfVm,
        memory: GuestMemoryMmap,
        runtime: Arc<VmRuntime>,
        vcpu_count: usize,
        boot: BootParams,
    ) -> Arc<Self> {
        // The guest's counter starts where the VM does; the framework
        // subtracts this offset from the host counter it reports as
        // `CNTVCT_EL0`, so the guest sees a small counter after boot instead
        // of one that has been running since the host started.
        let vtimer_offset = host_clock::now();
        let slots = (0..vcpu_count)
            .map(|index| {
                Arc::new(VcpuSlot {
                    id: AtomicU64::new(0),
                    // Only the boot CPU starts running; the others wait for a
                    // PSCI CPU_ON.
                    wait: WaitSlot::new(index == 0),
                })
            })
            .collect();
        Arc::new(Self {
            vm,
            memory,
            runtime,
            interrupts: Mutex::new(Gic::new(vcpu_count)),
            serial: Mutex::new(None),
            vtimer_offset,
            slots,
            stop: AtomicBool::new(false),
            failure: Mutex::new(None),
            started: Mutex::new(false),
            started_wake: Condvar::new(),
            boot,
        })
    }

    /// Installs the console. Called once, before any vCPU runs.
    pub fn install_serial(&self, serial: SerialDevice) {
        *lock(&self.serial) = Some(serial);
    }

    /// Queues host bytes as console input: the guest reads them from the UART
    /// and its received-data interrupt is raised (see
    /// `crate::spawner::VmHandle::serial_input`).
    ///
    /// The reason is the text the caller sees in
    /// `crate::spawner::SpawnError::SerialInput`; nothing is queued when it is
    /// returned.
    pub fn serial_input(&self, data: &[u8]) -> Result<(), String> {
        let mut serial = lock(&self.serial);
        match serial.as_mut() {
            Some(serial) => serial.queue_input(data),
            // The console is installed before the vCPUs start, so it is gone
            // only once the VM has stopped (or never came up).
            None => Err("the VM has stopped".to_string()),
        }
    }

    /// The number of vCPUs.
    pub fn vcpu_count(&self) -> usize {
        self.slots.len()
    }

    /// The interrupt controller.
    fn gic(&self) -> MutexGuard<'_, Gic> {
        lock(&self.interrupts)
    }

    /// Changes the interrupt controller under its lock, and then wakes the
    /// vCPUs whose interrupt line the change asserted.
    ///
    /// Every path that can change the controller goes through here: the
    /// console's trigger, the virtual timer, and the guest's own
    /// system-register and MMIO writes. A change that asserts another vCPU's
    /// line has to wake that vCPU - it may be halted, and nothing else would
    /// tell it that an interrupt is now pending for it.
    ///
    /// The controller's lock is released before any vCPU is woken: a halted
    /// vCPU takes its own slot's lock and then the controller's, so waking
    /// while holding the controller would deadlock.
    fn with_gic<R>(&self, change: impl FnOnce(&mut Gic) -> R) -> R {
        let (result, asserted) = {
            let mut gic = self.gic();
            let result = change(&mut gic);
            let mut asserted = 0u64;
            for index in 0..self.slots.len().min(MAX_VCPUS) {
                if gic.irq_line(index) {
                    asserted |= 1 << index;
                }
            }
            (result, asserted)
        };
        wake_asserted(
            self.slots.iter().map(|slot| &slot.wait),
            asserted,
            |index| self.kick(index),
        );
        result
    }

    /// Changes the interrupt controller and wakes the vCPUs whose line the
    /// change asserted.
    fn with_interrupts(&self, change: impl FnOnce(&mut Gic)) {
        self.with_gic(change);
    }

    /// Wakes a vCPU: any thread waiting on its slot, and the vCPU itself if it
    /// is inside `hv_vcpu_run`.
    fn wake(&self, index: usize) {
        let Some(slot) = self.slots.get(index) else {
            return;
        };
        slot.wait.wake();
        self.kick(index);
    }

    /// Asks a vCPU that is inside `hv_vcpu_run` to leave it.
    fn kick(&self, index: usize) {
        let Some(slot) = self.slots.get(index) else {
            return;
        };
        let id = slot.id.load(Ordering::SeqCst);
        if id != 0 {
            // SAFETY: `id` is a handle this process published after
            // `hv_vcpu_create`; `hv_vcpus_exit` is the one call the framework
            // allows from another thread, and it only asks the vCPU to leave
            // `hv_vcpu_run`.
            let ret = unsafe { sys::hv_vcpus_exit(&id, 1) };
            if ret != sys::HV_SUCCESS {
                eprintln!("warning: hv_vcpus_exit failed for vCPU {index}: {ret:#010x}");
            }
        }
    }

    /// Publishes a vCPU's handle so other threads can kick it, and reports
    /// that the VM has started.
    fn publish(&self, index: usize, id: sys::hv_vcpu_t) {
        if let Some(slot) = self.slots.get(index) {
            slot.id.store(id, Ordering::SeqCst);
        }
        *lock(&self.started) = true;
        self.started_wake.notify_all();
    }

    /// Waits until a vCPU has created itself, and reports a failure that
    /// stopped the VM instead.
    ///
    /// `boot` calls this so that a VM whose vCPUs cannot be created is
    /// reported to the caller of `spawn`, rather than looking like a running
    /// VM that produces no output.
    pub fn wait_until_started(&self, timeout: Duration) -> Option<String> {
        let deadline = Instant::now() + timeout;
        let mut started = lock(&self.started);
        loop {
            if *started {
                return None;
            }
            if let Some(failure) = lock(&self.failure).clone() {
                return Some(failure);
            }
            let now = Instant::now();
            if now >= deadline {
                return Some("the vCPUs did not start in time".to_string());
            }
            let (guard, _) = self
                .started_wake
                .wait_timeout(started, deadline - now)
                .unwrap_or_else(PoisonError::into_inner);
            started = guard;
        }
    }

    /// Asks the VM to stop: every vCPU wakes and leaves its run loop, after
    /// which the spawner's threads finish and the VM's memory and framework
    /// objects are released.
    pub fn stop(&self) {
        self.request_stop(StopCause::Requested);
    }

    /// Whether the VM has been asked to stop.
    fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Stops the VM: every vCPU is woken and leaves its run loop.
    ///
    /// The cause distinguishes the guest stopping the VM itself (a shutdown or
    /// a reset) from the spawner asking for it, which is what the VM handle
    /// reports afterwards.
    fn request_stop(&self, cause: StopCause) {
        match cause {
            StopCause::Requested => {}
            StopCause::GuestShutdown => self.runtime.mark_guest_exited(),
            // A reset is still the guest ending the VM: this backend has no
            // re-boot path, but the caller is told which of the two it was.
            StopCause::GuestReset => self.runtime.mark_guest_reset(),
        }
        self.stop.store(true, Ordering::SeqCst);
        for index in 0..self.slots.len() {
            self.wake(index);
        }
    }

    /// Records the first fatal error of a vCPU and stops the VM.
    fn fail(&self, index: usize, error: &HvfError) {
        let message = format!("vCPU {index}: {error}");
        {
            // The `started` lock is held across the failure write and the
            // notification: `wait_until_started` checks the failure under that
            // lock, so a report that lands between its check and its wait is
            // still seen (rather than making `boot` wait out its timeout).
            let _started = lock(&self.started);
            let mut failure = lock(&self.failure);
            if failure.is_none() {
                eprintln!("VM {}: {message}", self.runtime.id);
                *failure = Some(message);
            }
            self.started_wake.notify_all();
        }
        self.request_stop(StopCause::Requested);
    }

    /// Starts a vCPU at `entry` with `context_id` in `X0`, for a PSCI
    /// `CPU_ON`.
    ///
    /// Returns whether this call is what started it. The check and the state
    /// write happen together under the slot's lock, so two concurrent
    /// `CPU_ON`s of the same vCPU cannot both start it, the second silently
    /// overwriting the first's entry point; the caller answers PSCI
    /// `ALREADY_ON` when this returns `false`.
    fn start_vcpu(&self, index: usize, entry: u64, context_id: u64) -> bool {
        let Some(slot) = self.slots.get(index) else {
            return false;
        };
        // A powered-off vCPU waits on its slot (never inside `hv_vcpu_run`),
        // so starting it wakes it without a kick.
        slot.wait.start(entry, context_id)
    }

    /// Whether a vCPU is powered on, as PSCI sees it.
    fn is_running(&self, index: usize) -> bool {
        self.slots
            .get(index)
            .is_some_and(|slot| slot.wait.is_running())
    }

    /// Marks a vCPU as powered off (a PSCI `CPU_OFF`).
    fn park(&self, index: usize) {
        if let Some(slot) = self.slots.get(index) {
            slot.wait.park();
        }
    }

    /// Reads `len` bytes of a device register at guest physical address `ipa`
    /// into `bytes`. Returns `false` when no device answers there, which the
    /// caller turns into a fault in the guest.
    fn read_mmio(&self, ipa: u64, bytes: &mut [u8]) -> bool {
        let size = bytes.len() as u8;
        // The interrupt controller's lock is released before the console's:
        // the console's trigger takes the controller's lock, so holding it
        // here would deadlock the thread that is writing to the console.
        let register = {
            let gic = self.gic();
            match gic.decode_address(ipa) {
                Some(GicMmio::Distributor { offset }) => Some(gic.dist_read(offset, size)),
                Some(GicMmio::Redistributor { cpu, offset }) => {
                    Some(gic.redist_read(cpu, offset, size))
                }
                None => None,
            }
        };
        if let Some(value) = register {
            bytes.copy_from_slice(&value.to_le_bytes()[..bytes.len()]);
            return true;
        }
        if let Some(range) = offset_in(ipa, SERIAL_BASE, SERIAL_LEN) {
            let mut serial = lock(&self.serial);
            if let Some(serial) = serial.as_mut() {
                serial.read(range.base, range.offset, bytes);
                return true;
            }
        }
        false
    }

    /// Writes `bytes` to a device register at guest physical address `ipa`.
    /// Returns `false` when no device answers there.
    fn write_mmio(&self, ipa: u64, bytes: &[u8]) -> bool {
        let size = bytes.len() as u8;
        let value = to_u64(bytes);
        // A guest write to the controller goes through `with_gic`, so that a
        // write which asserts another vCPU's line (an `ISPENDR`, or an
        // `ISENABLER` of a pending interrupt) wakes it. The address is decoded
        // first: a write to a device must not pay for that scan, and the
        // controller's lock has to be released before the wake.
        match self.gic().decode_address(ipa) {
            Some(GicMmio::Distributor { offset }) => {
                self.with_gic(|gic| gic.dist_write(offset, size, value));
                return true;
            }
            Some(GicMmio::Redistributor { cpu, offset }) => {
                self.with_gic(|gic| gic.redist_write(cpu, offset, size, value));
                return true;
            }
            None => {}
        }
        if let Some(range) = offset_in(ipa, SERIAL_BASE, SERIAL_LEN) {
            let mut serial = lock(&self.serial);
            if let Some(serial) = serial.as_mut() {
                serial.write(range.base, range.offset, bytes);
                return true;
            }
        }
        false
    }
}

impl crate::hvf::uart::ConsoleLine for SharedVm {
    fn raise(&self) -> std::io::Result<()> {
        self.with_interrupts(|gic| gic.raise_spi(gic::UART_SPI));
        Ok(())
    }
}

/// A device address inside `[base, base + len)`.
struct DeviceAddress {
    base: u64,
    offset: u64,
}

/// Splits `addr` into the device's base address and the offset into it.
fn offset_in(addr: u64, base: u64, len: u64) -> Option<DeviceAddress> {
    if !gic::in_window(addr, base, len) {
        return None;
    }
    Some(DeviceAddress {
        base,
        offset: addr - base,
    })
}

/// A little-endian word from a device access of up to eight bytes.
fn to_u64(bytes: &[u8]) -> u64 {
    let mut value = [0u8; 8];
    let len = bytes.len().min(8);
    value[..len].copy_from_slice(&bytes[..len]);
    u64::from_le_bytes(value)
}

/// Locks a mutex, recovering from a peer thread's panic the way the rest of
/// this tree does: one panicking thread must not take the whole VM down.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the run loop does after an exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Run the vCPU again.
    Again,
    /// Idle the vCPU until an interrupt, the virtual timer, or a stop.
    Idle,
    /// Park the vCPU: it is powered off until a `CPU_ON` starts it.
    Park,
    /// Stop the whole VM, for the reason PSCI reported.
    Stop(StopCause),
}

/// Why a VM is being stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopCause {
    /// The spawner (or a backend failure) asked the VM to stop.
    Requested,
    /// The guest shut itself down (PSCI `SYSTEM_OFF`).
    GuestShutdown,
    /// The guest asked to reset (PSCI `SYSTEM_RESET`). This backend has no
    /// re-boot path, so the VM stops either way; the caller is told which of
    /// the two the guest asked for.
    GuestReset,
}

/// Runs one vCPU until the VM stops.
///
/// This is the body of a vCPU thread: it must be called on the thread that
/// owns the vCPU (see [`HvfVcpu`]). A fatal error is recorded on the VM, which
/// is stopped, and reported through [`SharedVm::failure`].
pub fn run_vcpu(shared: &Arc<SharedVm>, index: usize) {
    let result = vcpu_loop(shared, index);
    if let Err(error) = result {
        shared.fail(index, &error);
    }
}

fn vcpu_loop(shared: &Arc<SharedVm>, index: usize) -> Result<(), HvfError> {
    let mut vcpu = HvfVcpu::create()?;
    shared.publish(index, vcpu.id());
    vcpu.configure(index, shared.vtimer_offset)?;
    if index == 0 {
        // The boot CPU starts at the kernel entry point with the device tree
        // in X0, as the arm64 boot protocol specifies.
        vcpu.set_reg(sys::HV_REG_PC, shared.boot.entry)?;
        vcpu.set_reg(sys::HV_REG_X0, shared.boot.fdt)?;
    }

    let slot = match shared.slots.get(index) {
        Some(slot) => Arc::clone(slot),
        // `boot` only spawns a thread per slot it created.
        None => {
            return Err(HvfError::Call {
                call: "run_vcpu",
                code: sys::HV_BAD_ARGUMENT,
                hint: "there is no vCPU with this index",
            });
        }
    };
    loop {
        if shared.stop_requested() {
            return Ok(());
        }

        // A vCPU that is powered off (a secondary CPU at boot, or one that
        // executed CPU_OFF) waits here for a PSCI CPU_ON. Register state is
        // applied on this thread, because Hypervisor.framework only accepts
        // register access from the vCPU's own thread.
        match slot.wait.wait_for_start(|| shared.stop_requested()) {
            Start::Stopped => return Ok(()),
            Start::Running => {}
            Start::At { entry, context_id } => {
                // A freshly powered-on core presents reset register state, as
                // the PSCI specification requires: the guest must not observe
                // whatever a previous powering of this vCPU left in its
                // registers.
                for reg in 1..=30 {
                    vcpu.set_reg(sys::HV_REG_X0 + reg, 0)?;
                }
                vcpu.set_reg(sys::HV_REG_PC, entry)?;
                vcpu.set_reg(sys::HV_REG_X0, context_id)?;
                vcpu.set_reg(sys::HV_REG_CPSR, PSTATE_EL1H_MASKED)?;
            }
        }

        // The virtual timer can only be unmasked on the vCPU's own thread, and
        // the framework reports it only from `hv_vcpu_run`: re-checking the
        // timer's condition right before each run is what lets a guest that
        // cleared the timer go on receiving timer interrupts.
        vcpu.sync_vtimer(index, shared)?;

        let line = shared.gic().irq_line(index);
        vcpu.set_irq_line(line)?;
        vcpu.run()?;

        let outcome = match vcpu.exit_reason() {
            sys::HV_EXIT_REASON_CANCELED => Outcome::Again,
            sys::HV_EXIT_REASON_VTIMER_ACTIVATED => {
                // The framework masked the timer as it reported it; raise the
                // timer's interrupt and let `sync_vtimer` unmask it once the
                // guest has dealt with it.
                vcpu.vtimer_masked = true;
                shared.with_interrupts(|gic| gic.raise_virtual_timer(index));
                Outcome::Again
            }
            sys::HV_EXIT_REASON_EXCEPTION => handle_exception(&mut vcpu, index, shared)?,
            reason => {
                return Err(HvfError::Call {
                    call: "hv_vcpu_run",
                    code: sys::HV_ERROR,
                    hint: match reason {
                        sys::HV_EXIT_REASON_UNKNOWN => {
                            "the guest left the vCPU for a reason this backend does not know"
                        }
                        _ => "the guest left the vCPU for an unknown reason",
                    },
                });
            }
        };

        match outcome {
            Outcome::Again => {}
            Outcome::Idle => {
                let sleep = vcpu.vtimer_sleep(index, shared)?;
                // `halt` re-checks the predicate under the slot's lock, so an
                // interrupt raised, or a stop requested, while this vCPU is
                // deciding to sleep is not missed.
                slot.wait.halt(sleep, || {
                    shared.stop_requested() || shared.gic().irq_line(index)
                });
            }
            Outcome::Park => {
                shared.park(index);
            }
            Outcome::Stop(cause) => {
                shared.request_stop(cause);
                return Ok(());
            }
        }
    }
}

/// Answers one guest exception.
fn handle_exception(
    vcpu: &mut HvfVcpu,
    index: usize,
    shared: &Arc<SharedVm>,
) -> Result<Outcome, HvfError> {
    let (syndrome, virtual_address, ipa) = vcpu.exception();
    let exception = Exception::decode(syndrome);
    match exception {
        Exception::Wait(Wait { event: true }) => {
            // WFE waits for an event, which this backend does not model; it
            // steps over the instruction, as the reference backend does.
            vcpu.advance_pc()?;
            Ok(Outcome::Again)
        }
        Exception::Wait(Wait { event: false }) => {
            vcpu.advance_pc()?;
            // Work already pending means idling would be pointless.
            if shared.gic().irq_line(index) {
                Ok(Outcome::Again)
            } else {
                Ok(Outcome::Idle)
            }
        }
        Exception::Mmio(access) => handle_mmio(vcpu, shared, access, ipa, virtual_address),
        Exception::SysReg(reg) => handle_sys_reg(vcpu, index, shared, reg),
        Exception::Hvc { .. } => handle_psci(vcpu, shared),
        Exception::Smc { .. } => {
            // The device tree declares the `hvc` conduit, so an SMC is not a
            // PSCI call: fault it back into the guest.
            let pc = vcpu.get_reg(sys::HV_REG_PC)?;
            vcpu.inject_undef(pc)?;
            Ok(Outcome::Again)
        }
        Exception::Unhandled { class } => Err(HvfError::Call {
            call: "handle_exception",
            code: sys::HV_ERROR,
            hint: match class {
                0x20 => {
                    "the guest took an instruction abort (it fetched from an address the VM \
                         has no memory at)"
                }
                0x21 => "the guest took a PC alignment fault",
                _ => "the guest took an exception class this backend does not emulate",
            },
        }),
    }
}

/// Emulates a load or store to a device.
///
/// `ipa` is the guest physical address the access has to be routed to, and
/// `virtual_address` the faulting virtual address, which is what an injected
/// data abort reports to the guest in `FAR_EL1`.
fn handle_mmio(
    vcpu: &mut HvfVcpu,
    shared: &Arc<SharedVm>,
    access: MmioAccess,
    ipa: u64,
    virtual_address: u64,
) -> Result<Outcome, HvfError> {
    // A cache maintenance instruction that faulted is skipped by the
    // architecture's own rule.
    if access.cache_maintenance {
        vcpu.advance_pc()?;
        return Ok(Outcome::Again);
    }

    let pc = vcpu.get_reg(sys::HV_REG_PC)?;
    // The syndrome only carries a decode for the plain `LDR`/`STR` family;
    // for anything else (SIMD, SVE, or an instruction the framework could not
    // decode) the access cannot be emulated and the guest is faulted.
    if !access.valid || access.len > 8 || access.len == 0 {
        vcpu.inject_data_abort(virtual_address, !access.read, pc)?;
        return Ok(Outcome::Again);
    }

    let len = usize::from(access.len);
    // Register 31 is XZR: it reads as zero and discards writes. A guest that
    // faults an access naming it must not be able to steer the emulation into
    // one of the vCPU's real registers (the PC among them).
    let rt = sys::HV_REG_X0 + sys::hv_reg_t::from(access.rt);
    if access.read {
        let mut bytes = [0u8; 8];
        if !shared.read_mmio(ipa, &mut bytes[..len]) {
            vcpu.inject_data_abort(virtual_address, false, pc)?;
            return Ok(Outcome::Again);
        }
        if access.rt != 31 {
            let mut value = u64::from_le_bytes(bytes);
            if access.sign_extend {
                value = sign_extend(value, access.len);
            }
            vcpu.set_reg(rt, value)?;
        }
    } else {
        let value = if access.rt == 31 {
            0
        } else {
            vcpu.get_reg(rt)?
        };
        if !shared.write_mmio(ipa, &value.to_le_bytes()[..len]) {
            vcpu.inject_data_abort(virtual_address, true, pc)?;
            return Ok(Outcome::Again);
        }
    }
    vcpu.advance_pc()?;
    Ok(Outcome::Again)
}

/// Sign-extends the low `len` bytes of `value` to 64 bits, for an `LDRSW` or
/// a sign-extending load.
fn sign_extend(value: u64, len: u8) -> u64 {
    let bits = u32::from(len) * 8;
    if bits >= 64 {
        return value;
    }
    let shift = 64 - bits;
    ((value << shift) as i64 >> shift) as u64
}

/// Answers an `MRS`/`MSR` the framework left to the backend: the interrupt
/// controller's `ICC_*` registers.
fn handle_sys_reg(
    vcpu: &mut HvfVcpu,
    index: usize,
    shared: &Arc<SharedVm>,
    reg: crate::hvf::exit::SysReg,
) -> Result<Outcome, HvfError> {
    use crate::hvf::gic as icc;

    if reg.read {
        let value = match reg.reg {
            // Acknowledging has a side effect on the interrupt state, so it
            // is not a plain register read.
            icc::ICC_IAR0_EL1 | icc::ICC_IAR1_EL1 => {
                u64::from(shared.with_gic(|gic| gic.ack(index)))
            }
            icc::ICC_HPPIR0_EL1 | icc::ICC_HPPIR1_EL1 => shared.gic().hppir(index),
            other => match shared.gic().sysreg_read(index, other) {
                Some(value) => value,
                None => {
                    // A system register this backend does not own: the guest
                    // is told the instruction is undefined, which is how a
                    // guest probes for features.
                    let pc = vcpu.get_reg(sys::HV_REG_PC)?;
                    vcpu.inject_undef(pc)?;
                    return Ok(Outcome::Again);
                }
            },
        };
        // Register 31 is XZR: a read into it is discarded, a write from it
        // transfers zero.
        if reg.rt != 31 {
            let rt = sys::HV_REG_X0 + sys::hv_reg_t::from(reg.rt);
            vcpu.set_reg(rt, value)?;
        }
    } else {
        let value = if reg.rt == 31 {
            0
        } else {
            vcpu.get_reg(sys::HV_REG_X0 + sys::hv_reg_t::from(reg.rt))?
        };
        // A guest write goes through `with_gic`: it can assert a line - of
        // this vCPU or of another one (an SGI, an enable, a priority-mask
        // change) - and the vCPUs whose line it asserts have to be woken.
        let handled = match reg.reg {
            icc::ICC_IAR0_EL1 | icc::ICC_IAR1_EL1 => {
                shared.with_gic(|gic| gic.ack(index));
                true
            }
            icc::ICC_HPPIR0_EL1 | icc::ICC_HPPIR1_EL1 => true,
            other => shared.with_gic(|gic| gic.sysreg_write(index, other, value)),
        };
        if !handled {
            let pc = vcpu.get_reg(sys::HV_REG_PC)?;
            vcpu.inject_undef(pc)?;
            return Ok(Outcome::Again);
        }
    }
    vcpu.advance_pc()?;
    Ok(Outcome::Again)
}

/// Answers a PSCI call.
fn handle_psci(vcpu: &mut HvfVcpu, shared: &Arc<SharedVm>) -> Result<Outcome, HvfError> {
    let function = vcpu.get_reg(sys::HV_REG_X0)?;
    let arguments = [
        vcpu.get_reg(sys::HV_REG_X0 + 1)?,
        vcpu.get_reg(sys::HV_REG_X0 + 2)?,
        vcpu.get_reg(sys::HV_REG_X0 + 3)?,
    ];

    // The view of the machine a PSCI call gets: which vCPUs are powered on.
    let count = shared.vcpu_count().min(MAX_VCPUS);
    let mut running = [false; MAX_VCPUS];
    for (index, flag) in running[..count].iter_mut().enumerate() {
        *flag = shared.is_running(index);
    }
    let machine = Machine {
        running: &running[..count],
    };

    match psci::handle(function, arguments[0], arguments[1], arguments[2], &machine) {
        Action::Return(value) => {
            vcpu.set_reg(sys::HV_REG_X0, value)?;
            Ok(Outcome::Again)
        }
        Action::CpuOn {
            mpidr,
            entry,
            context_id,
        } => {
            // The running check above raced whatever else touched this vCPU's
            // state; `start_vcpu` is the authoritative one, and reports
            // whether it is what started it.
            let returned = if shared.start_vcpu(psci::affinity0(mpidr) as usize, entry, context_id)
            {
                psci::RET_SUCCESS
            } else {
                psci::RET_ALREADY_ON
            };
            vcpu.set_reg(sys::HV_REG_X0, returned)?;
            Ok(Outcome::Again)
        }
        Action::CpuOff => Ok(Outcome::Park),
        Action::SystemOff { reset } => Ok(Outcome::Stop(if reset {
            StopCause::GuestReset
        } else {
            StopCause::GuestShutdown
        })),
        Action::Suspend => {
            vcpu.set_reg(sys::HV_REG_X0, psci::RET_SUCCESS)?;
            Ok(Outcome::Idle)
        }
    }
}
