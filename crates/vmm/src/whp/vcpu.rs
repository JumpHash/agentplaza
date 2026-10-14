// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the WHP counterpart of the KVM vCPU loop
// (`vstate::vcpu`) and of `hvf::vcpu`. The exit handling follows the WHP
// documentation and QEMU's WHPX accelerator (`target/i386/whpx`), the
// reference user of the WHP instruction emulator.

//! The VM's shared state and its vCPU threads.
//!
//! Each vCPU thread runs its processor with `WHvRunVirtualProcessor`. A port
//! or MMIO exit is handed to the WHP instruction emulator, which decodes the
//! instruction and calls back into [`io_port_callback`] /
//! [`memory_callback`]; those dispatch to the same `vstate::bus` the KVM
//! backend uses (the serial console, the i8042 and PM1 ports, the I/O APIC
//! and the virtio-mmio transports). Interrupts reach the guest through the
//! userspace I/O APIC ([`Interrupts`]) and `WHvRequestInterrupt`; the local
//! APICs are the hypervisor's.

// FFI boundary (see the allowlist in `lib.rs`): the emulator callbacks
// (`extern "system"` functions handed raw pointers by WinHvEmulation.dll),
// the emulator handle, and the reads of the exit-context union.
#![allow(unsafe_code)]
// The exit reasons are matched by their SDK names.
#![allow(non_upper_case_globals)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use vm_memory::{Bytes, GuestAddress};
use windows_sys::Win32::System::Hypervisor::*;
use windows_sys::core::HRESULT;

use super::ioapic::{IOAPIC_BASE, IoApic};
use super::sys::{Api, WhpError, call};
use super::vm::Partition;
use crate::pal::eventfd::EventFd;
use crate::spawner::VmRuntime;
use crate::vstate::bus::{Bus, BusDevice};
use crate::vstate::memory::GuestMemoryMmap;

/// The most vCPUs this backend boots (the MPTable's limit).
pub const MAX_VCPUS: u8 = 254;

/// How long a halted vCPU sleeps before it runs again when no interrupt
/// request wakes it first. The hypervisor's own local APIC (its timer, IPIs)
/// can make an interrupt pending without this process knowing, so a halt is
/// never waited out indefinitely.
const HALT_POLL: Duration = Duration::from_millis(1);

const S_OK: HRESULT = 0;
const E_FAIL: HRESULT = 0x8000_4005_u32 as HRESULT;

/// The userspace I/O APIC and the partition its deliveries go to.
pub struct Interrupts {
    ioapic: Mutex<IoApic>,
    partition: Arc<Partition>,
    /// Wakes halted vCPUs when an interrupt is requested.
    wake: (Mutex<u64>, Condvar),
}

impl Interrupts {
    /// The I/O APIC (with the MPTable's I/O APIC id) for `partition`.
    pub fn new(partition: Arc<Partition>, ioapic_id: u8) -> Self {
        Self {
            ioapic: Mutex::new(IoApic::new(ioapic_id)),
            partition,
            wake: (Mutex::new(0), Condvar::new()),
        }
    }

    fn deliver(&self, interrupts: impl IntoIterator<Item = super::ioapic::Interrupt>) {
        let mut any = false;
        for interrupt in interrupts {
            any = true;
            if let Err(err) = self.partition.request_interrupt(interrupt) {
                eprintln!(
                    "whp: interrupt vector {:#x} not delivered: {err}",
                    interrupt.vector
                );
            }
        }
        if any {
            let (generation, condvar) = &self.wake;
            *generation.lock().unwrap_or_else(PoisonError::into_inner) += 1;
            condvar.notify_all();
        }
    }

    /// An edge on I/O APIC input `pin` (a device's interrupt event fired).
    pub fn pulse(&self, pin: usize) {
        let delivery = self
            .ioapic
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pulse_irq(pin);
        self.deliver(delivery);
    }

    /// The guest's EOI for a level-triggered `vector`.
    fn end_of_interrupt(&self, vector: u8) {
        let redeliver = self
            .ioapic
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .end_of_interrupt(vector);
        self.deliver(redeliver);
    }

    /// Sleeps a halted vCPU until an interrupt is requested or [`HALT_POLL`]
    /// passes.
    fn wait_for_interrupt(&self) {
        let (generation, condvar) = &self.wake;
        let guard = generation.lock().unwrap_or_else(PoisonError::into_inner);
        let seen = *guard;
        let _ = condvar
            .wait_timeout_while(guard, HALT_POLL, |now| *now == seen)
            .unwrap_or_else(PoisonError::into_inner);
    }

    fn wake_all(&self) {
        let (generation, condvar) = &self.wake;
        *generation.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        condvar.notify_all();
    }
}

/// The I/O APIC's register window on the MMIO bus.
pub struct IoApicMmio(pub Arc<Interrupts>);

impl BusDevice for IoApicMmio {
    fn read(&mut self, _base: u64, offset: u64, data: &mut [u8]) {
        self.0
            .ioapic
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .mmio_read(offset, data);
    }

    fn write(&mut self, _base: u64, offset: u64, data: &[u8]) -> Option<Arc<std::sync::Barrier>> {
        let deliveries = self
            .0
            .ioapic
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .mmio_write(offset, data);
        self.0.deliver(deliveries);
        None
    }
}

/// How the VM ends: the shared exit event and flags the legacy devices, the
/// vsock event thread and the exit watcher agree on (the KVM backend's).
pub struct ExitSignals {
    /// Written by every exit path; the watcher and the vsock event thread
    /// poll it.
    pub evt: EventFd,
    /// Set before `evt` is written.
    pub signalled: Arc<AtomicBool>,
    /// Set by the PM1 reset register (and by a triple fault): the guest asked
    /// for a reset rather than a power-off.
    pub guest_reset: Arc<AtomicBool>,
    /// Set by the PM1 device when the vCPUs should finish.
    pub finish_requested: Arc<AtomicBool>,
    /// Set by the stop control: the host killed the VM.
    pub killed: AtomicBool,
}

impl ExitSignals {
    /// Fresh, unsignalled exit state.
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            evt: EventFd::new(crate::pal::eventfd::EFD_NONBLOCK)?,
            signalled: Arc::new(AtomicBool::new(false)),
            guest_reset: Arc::new(AtomicBool::new(false)),
            finish_requested: Arc::new(AtomicBool::new(false)),
            killed: AtomicBool::new(false),
        })
    }

    /// Signals the exit (idempotent).
    pub fn signal(&self) {
        self.signalled.store(true, Ordering::SeqCst);
        if let Err(err) = self.evt.write(1) {
            eprintln!("whp: cannot signal the VM's exit event: {err}");
        }
    }
}

/// Everything the vCPU threads share. Owns the partition (through `Arc`) and
/// the guest memory it maps: both outlive every vCPU thread because each
/// thread holds an `Arc<WhpVm>`.
///
/// Field order is drop order: the partition (and the I/O APIC, which holds
/// the partition's other reference) goes before the guest memory it maps, so
/// RAM is never freed while a partition still has it mapped.
pub struct WhpVm {
    pub partition: Arc<Partition>,
    pub interrupts: Arc<Interrupts>,
    pub memory: GuestMemoryMmap,
    pub pio: Bus,
    pub mmio: Bus,
    pub vcpu_count: u32,
    pub exit: ExitSignals,
    /// The vCPU loops stop at their next exit (set by [`WhpVm::stop`]).
    stopping: AtomicBool,
}

impl WhpVm {
    /// Assembles the shared state.
    pub fn new(
        partition: Arc<Partition>,
        memory: GuestMemoryMmap,
        interrupts: Arc<Interrupts>,
        vcpu_count: u32,
        exit: ExitSignals,
    ) -> Self {
        Self {
            partition,
            interrupts,
            memory,
            pio: Bus::new(),
            mmio: Bus::new(),
            vcpu_count,
            exit,
            stopping: AtomicBool::new(false),
        }
    }

    /// Stops every vCPU: each loop ends at its next exit, and running ones are
    /// cancelled out of `WHvRunVirtualProcessor`.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.interrupts.wake_all();
        for index in 0..self.vcpu_count {
            // A processor that is not running returns at once from its next
            // run; an error here (a vCPU that never started) changes nothing.
            let _ = self.partition.cancel_vcpu(index);
        }
    }

    fn should_stop(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
            || self.exit.signalled.load(Ordering::SeqCst)
            || self.exit.finish_requested.load(Ordering::SeqCst)
    }

    /// A guest-initiated end of the VM (a triple fault, a dead halt, or an
    /// unrecoverable exit).
    fn guest_stopped(&self, reset: bool) {
        if reset {
            self.exit.guest_reset.store(true, Ordering::SeqCst);
        }
        self.exit.signal();
    }
}

/// The context the emulator callbacks receive: the VM and the vCPU index.
struct VcpuContext<'a> {
    vm: &'a WhpVm,
    index: u32,
}

/// A WHP instruction emulator, destroyed on drop.
struct Emulator {
    api: &'static Api,
    handle: *mut c_void,
}

impl Emulator {
    fn new(api: &'static Api) -> Result<Self, WhpError> {
        let callbacks = WHV_EMULATOR_CALLBACKS {
            Size: std::mem::size_of::<WHV_EMULATOR_CALLBACKS>() as u32,
            Reserved: 0,
            WHvEmulatorIoPortCallback: Some(io_port_callback),
            WHvEmulatorMemoryCallback: Some(memory_callback),
            WHvEmulatorGetVirtualProcessorRegisters: Some(get_registers_callback),
            WHvEmulatorSetVirtualProcessorRegisters: Some(set_registers_callback),
            WHvEmulatorTranslateGvaPage: Some(translate_gva_callback),
        };
        let mut handle = std::ptr::null_mut();
        // SAFETY: `callbacks` is a complete callback table (the emulator copies
        // it) and `handle` a writable out-parameter.
        unsafe { call!(api, WHvEmulatorCreateEmulator(&callbacks, &mut handle))? };
        Ok(Self { api, handle })
    }
}

impl Drop for Emulator {
    fn drop(&mut self) {
        // SAFETY: `handle` is the emulator created in `new`, used by this
        // thread only and no longer in a call.
        let _ = unsafe { call!(self.api, WHvEmulatorDestroyEmulator(self.handle)) };
    }
}

/// Recovers the callback context.
///
/// # Safety
///
/// `context` must be the `&VcpuContext` the vCPU loop passed to the
/// `WHvEmulatorTry*Emulation` call that is calling back.
unsafe fn context<'a>(context: *const c_void) -> &'a VcpuContext<'a> {
    // SAFETY: guaranteed by the caller.
    unsafe { &*context.cast::<VcpuContext<'a>>() }
}

/// Port I/O on behalf of the emulator: `Direction` 0 is `in`, 1 is `out`.
unsafe extern "system" fn io_port_callback(
    ctx: *const c_void,
    access: *mut WHV_EMULATOR_IO_ACCESS_INFO,
) -> HRESULT {
    // SAFETY: the emulator passes back our context and a valid access record.
    let (ctx, access) = unsafe { (context(ctx), &mut *access) };
    let size = usize::from(access.AccessSize).min(4);
    let port = u64::from(access.Port);
    if access.Direction == 0 {
        let mut bytes = [0xffu8; 4];
        // A port with no device reads as all ones, like an empty ISA bus.
        let _ = ctx.vm.pio.read(port, &mut bytes[..size]);
        access.Data = u32::from_le_bytes(bytes);
    } else {
        let bytes = access.Data.to_le_bytes();
        let _ = ctx.vm.pio.write(port, &bytes[..size]);
    }
    S_OK
}

/// Memory access on behalf of the emulator: a device register on the MMIO
/// bus, or guest RAM (the other operand of a string instruction, say).
unsafe extern "system" fn memory_callback(
    ctx: *const c_void,
    access: *mut WHV_EMULATOR_MEMORY_ACCESS_INFO,
) -> HRESULT {
    // SAFETY: the emulator passes back our context and a valid access record.
    let (ctx, access) = unsafe { (context(ctx), &mut *access) };
    let size = usize::from(access.AccessSize).min(8);
    let gpa = access.GpaAddress;
    let vm = ctx.vm;
    if access.Direction == 0 {
        let data = &mut access.Data[..size];
        if vm.mmio.read(gpa, data).is_err()
            && vm.memory.read_slice(data, GuestAddress(gpa)).is_err()
        {
            data.fill(0xff);
        }
    } else {
        let data = &access.Data[..size];
        if vm.mmio.write(gpa, data).is_err() {
            let _ = vm.memory.write_slice(data, GuestAddress(gpa));
        }
    }
    S_OK
}

unsafe extern "system" fn get_registers_callback(
    ctx: *const c_void,
    names: *const WHV_REGISTER_NAME,
    count: u32,
    values: *mut WHV_REGISTER_VALUE,
) -> HRESULT {
    // SAFETY: the emulator passes our context and two arrays of `count`
    // elements.
    let (ctx, names, values) = unsafe {
        (
            context(ctx),
            std::slice::from_raw_parts(names, count as usize),
            std::slice::from_raw_parts_mut(values, count as usize),
        )
    };
    match ctx.vm.partition.get_registers(ctx.index, names, values) {
        Ok(()) => S_OK,
        Err(_) => E_FAIL,
    }
}

unsafe extern "system" fn set_registers_callback(
    ctx: *const c_void,
    names: *const WHV_REGISTER_NAME,
    count: u32,
    values: *const WHV_REGISTER_VALUE,
) -> HRESULT {
    // SAFETY: the emulator passes our context and two arrays of `count`
    // elements.
    let (ctx, names, values) = unsafe {
        (
            context(ctx),
            std::slice::from_raw_parts(names, count as usize),
            std::slice::from_raw_parts(values, count as usize),
        )
    };
    match ctx.vm.partition.set_registers(ctx.index, names, values) {
        Ok(()) => S_OK,
        Err(_) => E_FAIL,
    }
}

unsafe extern "system" fn translate_gva_callback(
    ctx: *const c_void,
    gva: u64,
    flags: WHV_TRANSLATE_GVA_FLAGS,
    result: *mut WHV_TRANSLATE_GVA_RESULT_CODE,
    gpa: *mut u64,
) -> HRESULT {
    // SAFETY: the emulator passes our context and two writable out-parameters.
    let ctx = unsafe { context(ctx) };
    match ctx.vm.partition.translate_gva(ctx.index, gva, flags) {
        Ok((code, address)) => {
            // SAFETY: as above.
            unsafe {
                *result = code;
                *gpa = address;
            }
            S_OK
        }
        Err(_) => E_FAIL,
    }
}

/// `WHV_EMULATOR_STATUS.EmulationSuccessful`.
fn emulation_succeeded(status: WHV_EMULATOR_STATUS) -> bool {
    // SAFETY: every bit pattern of the status union is a valid `u32`.
    unsafe { status.AsUINT32 & 1 != 0 }
}

/// Runs vCPU `index` until the VM stops. Errors end the VM: they are reported
/// on stderr (the node's log) and turned into a guest exit, so the spawner's
/// `wait` returns instead of hanging on a dead processor.
pub fn run_vcpu(vm: Arc<WhpVm>, runtime: Arc<VmRuntime>, index: u32) {
    if let Err(err) = vcpu_loop(&vm, index) {
        eprintln!("whp: VM {} vCPU {index}: {err}", runtime.id);
        vm.guest_stopped(false);
    }
}

fn vcpu_loop(vm: &WhpVm, index: u32) -> Result<(), WhpError> {
    let emulator = Emulator::new(vm.partition.api())?;
    let ctx = VcpuContext { vm, index };
    let ctx_ptr = (&ctx as *const VcpuContext<'_>).cast::<c_void>();
    let mut exit = WHV_RUN_VP_EXIT_CONTEXT::default();
    let api = vm.partition.api();

    while !vm.should_stop() {
        vm.partition.run_vcpu(index, &mut exit)?;
        match exit.ExitReason {
            WHvRunVpExitReasonMemoryAccess => {
                let mut status = WHV_EMULATOR_STATUS::default();
                // SAFETY: the exit reason says the union holds the memory
                // access context; `ctx_ptr` outlives the call, and the
                // callbacks only read through it.
                unsafe {
                    call!(
                        api,
                        WHvEmulatorTryMmioEmulation(
                            emulator.handle,
                            ctx_ptr,
                            &exit.VpContext,
                            &exit.Anonymous.MemoryAccess,
                            &mut status,
                        )
                    )?;
                }
                if !emulation_succeeded(status) {
                    // SAFETY: as above.
                    let gpa = unsafe { exit.Anonymous.MemoryAccess.Gpa };
                    eprintln!("whp: vCPU {index}: MMIO at {gpa:#x} could not be emulated");
                    // SAFETY: every bit pattern is a valid `u32`.
                    return Err(WhpError::Emulation {
                        what: "MMIO",
                        status: unsafe { status.AsUINT32 },
                    });
                }
            }
            WHvRunVpExitReasonX64IoPortAccess => {
                let mut status = WHV_EMULATOR_STATUS::default();
                // SAFETY: the exit reason says the union holds the port access
                // context; see the MMIO arm.
                unsafe {
                    call!(
                        api,
                        WHvEmulatorTryIoEmulation(
                            emulator.handle,
                            ctx_ptr,
                            &exit.VpContext,
                            &exit.Anonymous.IoPortAccess,
                            &mut status,
                        )
                    )?;
                }
                if !emulation_succeeded(status) {
                    // SAFETY: every bit pattern is a valid `u32`.
                    return Err(WhpError::Emulation {
                        what: "port I/O",
                        status: unsafe { status.AsUINT32 },
                    });
                }
            }
            WHvRunVpExitReasonX64ApicEoi => {
                // SAFETY: the exit reason says the union holds the EOI context.
                let vector = unsafe { exit.Anonymous.ApicEoi.InterruptVector };
                vm.interrupts.end_of_interrupt(vector as u8);
            }
            WHvRunVpExitReasonX64Cpuid => {
                // SAFETY: the exit reason says the union holds the CPUID context.
                let access = unsafe { exit.Anonymous.CpuidAccess };
                let length = u64::from(exit.VpContext._bitfield & 0xf).max(2);
                let (rax, rbx, rcx, rdx) = cpuid(access);
                let reg64 = |value: u64| WHV_REGISTER_VALUE { Reg64: value };
                vm.partition.set_registers(
                    index,
                    &[
                        WHvX64RegisterRax,
                        WHvX64RegisterRbx,
                        WHvX64RegisterRcx,
                        WHvX64RegisterRdx,
                        WHvX64RegisterRip,
                    ],
                    &[
                        reg64(rax),
                        reg64(rbx),
                        reg64(rcx),
                        reg64(rdx),
                        reg64(exit.VpContext.Rip + length),
                    ],
                )?;
            }
            WHvRunVpExitReasonX64Halt => {
                // A halt with interrupts disabled never ends: the guest
                // stopped itself (a kernel that powered off without ACPI, or a
                // panic with `panic=0`).
                if exit.VpContext.Rflags & (1 << 9) == 0 {
                    vm.guest_stopped(false);
                    return Ok(());
                }
                vm.interrupts.wait_for_interrupt();
            }
            WHvRunVpExitReasonCanceled => {}
            WHvRunVpExitReasonUnrecoverableException => {
                // A triple fault: a PC resets.
                eprintln!(
                    "whp: vCPU {index}: triple fault at rip {:#x}; stopping the VM",
                    exit.VpContext.Rip
                );
                vm.guest_stopped(true);
                return Ok(());
            }
            other => {
                eprintln!(
                    "whp: vCPU {index}: unhandled exit reason {other} at rip {:#x}; stopping the VM",
                    exit.VpContext.Rip
                );
                vm.guest_stopped(false);
                return Ok(());
            }
        }
    }
    Ok(())
}

/// CPUID.1:ECX.x2APIC.
const CPUID_1_ECX_X2APIC: u64 = 1 << 21;

/// The answer to a CPUID that exited (leaf 1 only, see `vm::Partition::create`):
/// the hypervisor's default result, with x2APIC hidden. WHP emulates the
/// local APIC in xAPIC mode here, so a guest that switched to x2APIC would
/// write MSRs nothing implements.
fn cpuid(access: WHV_X64_CPUID_ACCESS_CONTEXT) -> (u64, u64, u64, u64) {
    let mut rcx = access.DefaultResultRcx;
    if access.Rax as u32 == 1 {
        rcx &= !CPUID_1_ECX_X2APIC;
    }
    (
        access.DefaultResultRax,
        access.DefaultResultRbx,
        rcx,
        access.DefaultResultRdx,
    )
}

/// The I/O APIC's window, for the MMIO bus.
pub const IOAPIC_WINDOW: (u64, u64) = (IOAPIC_BASE, super::ioapic::IOAPIC_SIZE);
