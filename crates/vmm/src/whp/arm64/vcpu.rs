// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//! The arm64 VM's shared state and its vCPU run loop: a thread per vCPU runs the partition, and each exit
//! is decoded by [`abi::decode_exit`] and completed here: a device access goes to the MMIO bus (a read
//! fills the transfer register, a write takes the register's value), a reset ends the VM, a cancel is
//! the stop flag, and anything else stops the VM with its reason.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::abi::{self, ArmExit, Mmio, ResetOutcome};
use super::vm::Partition;
use crate::pal::eventfd::EventFd;
use crate::spawner::VmRuntime;
use crate::vstate::bus::{Bus, BusDevice};
use crate::vstate::memory::GuestMemoryMmap;

/// The virtio-mmio `InterruptACK` register: a write clears the bits the driver acknowledges, and the
/// level-triggered line drops once none are left (the design doc's "deassert when the guest acks").
const VIRTIO_MMIO_INTERRUPT_ACK: u64 = 0x064;

/// The exit context, 16-byte aligned, as the hypervisor fills it.
#[repr(C, align(16))]
struct ExitContext([u8; abi::EXIT_CONTEXT_SIZE]);

/// One transport's interrupt line: its window on the bus and its SPI.
#[derive(Debug, Clone, Copy)]
pub struct IrqLine {
    pub base: u64,
    pub len: u64,
    pub spi: u32,
}

/// The VM the vCPU threads, the interrupt relay and the stop control share.
pub struct Vm {
    /// Declared first, so it is deleted before the memory it maps is freed.
    pub partition: Partition,
    /// The guest memory the partition maps; the devices read and write it through their own clones.
    pub memory: GuestMemoryMmap,
    /// The MMIO bus: the virtio transports, and nothing else (an address no device claims reads as zero).
    pub bus: Bus,
    /// The vCPUs' count; vCPU `i` is partition processor `i`.
    pub vcpus: u32,
    /// The transports' interrupt lines, for the acknowledge path.
    pub lines: Mutex<Vec<IrqLine>>,
    /// Set when the VM stops; the vCPUs, the relay and the device threads watch it.
    pub stopping: Arc<AtomicBool>,
    /// Written when the VM stops, so a relay or device thread blocked in a poll wakes up.
    pub exit_evt: EventFd,
    /// The VM's handle in the spawner: the exit reason and the thread list.
    pub runtime: Arc<VmRuntime>,
}

impl Vm {
    /// Asks every vCPU to leave its run and the relay to end. Idempotent.
    pub fn stop(&self) {
        if self.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        // A write that fails here leaves the flag set: the threads still see it on their next check.
        let _ = self.exit_evt.write(1);
        for index in 0..self.vcpus {
            let _ = self.partition.cancel(index);
        }
    }

    /// Raises SPI `spi`: the line is asserted and stays so until the driver acknowledges it.
    pub fn raise_spi(&self, spi: u32) -> Result<(), String> {
        let intid = crate::hvf::gic::SPI_BASE + spi;
        self.partition
            .request_interrupt(&abi::interrupt_control(intid, true))
            .map_err(|err| err.to_string())
    }

    /// Records a transport's interrupt line.
    pub fn add_line(&self, line: IrqLine) {
        self.lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(line);
    }

    /// Reports why the VM stopped and stops it.
    fn fail(&self, index: u32, reason: &str) {
        eprintln!("vmm: VM {} vCPU {index} stopped: {reason}", self.runtime.id);
        self.stop();
    }

    /// Completes one device access: the bus, then the transfer register and the PC past the instruction.
    fn complete_mmio(&self, index: u32, mmio: &Mmio) -> Result<(), String> {
        let width = usize::from(mmio.access.len);
        let next_pc = mmio.pc + 4;
        let rt = u32::from(mmio.access.rt);
        if mmio.access.read {
            let mut bytes = [0u8; 8];
            // An address no device claims reads as zero, as the bus does for any unmapped range.
            let _ = self.bus.read(mmio.gpa, &mut bytes[..width]);
            let value = abi::read_result(&bytes, mmio.access, mmio.is_64bit);
            let mut registers = vec![(abi::REGISTER_PC, next_pc)];
            if rt != 31 {
                registers.push((abi::REGISTER_X0 + rt, value));
            }
            return self
                .partition
                .set_registers(index, &registers)
                .map_err(|err| err.to_string());
        }
        let value = if rt == 31 {
            0
        } else {
            self.partition
                .get_register(index, abi::REGISTER_X0 + rt)
                .map_err(|err| err.to_string())?
        };
        let bytes = abi::write_bytes(value, mmio.access);
        // A write to a device that fails (its own error) is the device's report; the guest carries on.
        let _ = self.bus.write_and_wait(mmio.gpa, &bytes[..width]);
        self.acknowledge(mmio.gpa, &bytes[..width]);
        self.partition
            .set_registers(index, &[(abi::REGISTER_PC, next_pc)])
            .map_err(|err| err.to_string())
    }

    /// A write to a transport's `InterruptACK` drops its line.
    fn acknowledge(&self, gpa: u64, _data: &[u8]) {
        let lines = self
            .lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for line in lines {
            if gpa >= line.base
                && gpa < line.base + line.len
                && gpa - line.base == VIRTIO_MMIO_INTERRUPT_ACK
            {
                let intid = crate::hvf::gic::SPI_BASE + line.spi;
                let _ = self
                    .partition
                    .request_interrupt(&abi::interrupt_control(intid, false));
            }
        }
    }
}

/// Runs vCPU `index` until the VM stops.
pub fn run_vcpu(vm: &Vm, index: u32) {
    let mut context = ExitContext([0; abi::EXIT_CONTEXT_SIZE]);
    while !vm.stopping.load(Ordering::SeqCst) {
        if let Err(err) = vm.partition.run(index, &mut context.0) {
            vm.fail(index, &format!("the run failed: {err}"));
            return;
        }
        match abi::decode_exit(&context.0) {
            ArmExit::Canceled => {}
            ArmExit::Mmio(mmio) => {
                if let Err(err) = vm.complete_mmio(index, &mmio) {
                    vm.fail(
                        index,
                        &format!("a device access at {:#x} failed: {err}", mmio.gpa),
                    );
                    return;
                }
            }
            ArmExit::Reset { kind, code } => {
                match abi::reset_stops_vm(kind) {
                    Some(ResetOutcome::Off) => vm.runtime.mark_guest_exited(),
                    Some(ResetOutcome::Reset) => vm.runtime.mark_guest_reset(),
                    None => vm.fail(
                        index,
                        &format!("reset kind {kind} (code {code}) is not one the backend serves"),
                    ),
                }
                vm.stop();
                return;
            }
            ArmExit::Unsupported { reason } => {
                vm.fail(index, &reason);
                return;
            }
        }
    }
}

/// The interrupt relay's view of the VM: held weakly, upgraded only to raise an SPI or to stop it.
pub struct WeakVm(pub Weak<Vm>);

impl crate::hvf::relay::SpiSink for WeakVm {
    fn raise(&self, spi: u32) -> bool {
        match self.0.upgrade() {
            Some(vm) => vm.raise_spi(spi).is_ok(),
            None => false,
        }
    }

    fn alive(&self) -> bool {
        self.0.strong_count() > 0
    }

    fn stop(&self) {
        if let Some(vm) = self.0.upgrade() {
            vm.stop();
        }
    }
}

/// Puts a device on the bus at `base`, length one transport window.
pub fn attach(
    bus: &Bus,
    device: Arc<Mutex<dyn BusDevice>>,
    base: u64,
    len: u64,
) -> Result<(), crate::vstate::bus::BusError> {
    bus.insert(device, base, len)
}
