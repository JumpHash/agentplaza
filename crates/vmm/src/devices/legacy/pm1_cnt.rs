// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port (not derived from Firecracker, which has no ACPI S5
// support): a small PIO stub emulating the ACPI reset register (RESET_REG) and
// the ACPI 5.0+ extended sleep control/status registers (SLEEP_CONTROL_REG /
// SLEEP_STATUS_REG) of a HW-reduced FADT.
//
// * A write of RESET_VALUE to the reset register stops the VM as a *reset*
//   (the builder's exit watcher reports reset=true). This gives a guest reboot
//   (RB_AUTOBOOT) a clean ACPI reset path instead of the BIOS real-mode jump,
//   which double-faults on this KVM port.
// * A write of SLP_EN with SLP_TYP=5 (S5) to the sleep control register stops
//   the VM as a *shutdown* (reset=false).
//
// The SLP_EN/SLP_TYP bit layout follows the ACPI spec ("Values for
// sleep_status and sleep_control registers"; also Linux's
// include/acpi/actbl.h), and both the shutdown condition and the reset-value
// dispatch mirror Cloud Hypervisor's AcpiShutdownDevice (Apache-2.0, Copyright
// 2019 Intel Corporation).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};

use crate::pal::eventfd::EventFd;

use crate::vstate::bus::BusDevice;

/// Port of the sleep control register (SLEEP_CONTROL_REG).
pub const SLEEP_CONTROL_PORT: u64 = 0x604;
/// Port of the sleep status register (SLEEP_STATUS_REG).
pub const SLEEP_STATUS_PORT: u64 = 0x605;
/// Port of the reset register (RESET_REG).
pub const RESET_PORT: u64 = 0x606;
/// Size of the window this device owns (three byte-wide ports).
pub const PM1_CNT_SIZE: u64 = 0x3;
/// Value written to the reset register to reset the system.
pub const RESET_VALUE: u8 = 1;

/// SLP_EN: bit 5 of the sleep control register.
const SLP_EN_BIT: u8 = 5;
/// SLP_TYP: bits 4:2 of the sleep control register.
const SLP_TYP_SHIFT: u8 = 2;
/// SLP_TYP value for S5 (soft off).
const S5_SLEEP_TYPE: u8 = 5;

/// Offset of the sleep control register within this device's window.
const OFS_SLEEP_CONTROL: u64 = 0;
/// Offset of the sleep status register within this device's window.
const OFS_SLEEP_STATUS: u64 = 1;
/// Offset of the reset register within this device's window.
const OFS_RESET: u64 = 2;

/// A PIO device for the reset/sleep-control/sleep-status ports that turns a
/// guest reset or S5 request into the VM's exit event, distinguishing the two
/// by the `guest_reset` flag the exit watcher reads.
#[derive(Debug)]
pub struct Pm1CntDevice {
    shutdown_evt: EventFd,
    guest_reset: Arc<AtomicBool>,
    /// The VM's "exit signalled" flag, published before `shutdown_evt` is
    /// written (see `VmCommon::exit_signalled`), so a second observer of the
    /// exit event fd cannot lose the exit to the exit watcher's read.
    exit_signalled: Arc<AtomicBool>,
    /// Set by the VM's exit watcher (through `KvmVm::shutdown_vcpus`) once the
    /// vCPUs are being stopped. The reset path spins on it so the guest cannot
    /// re-enter its reboot sequence and double-fault in the BIOS real-mode
    /// jump before the stop lands.
    finish_requested: Arc<AtomicBool>,
}

impl Pm1CntDevice {
    /// Constructs a device that signals `shutdown_evt` when the guest requests
    /// a reset or S5, setting `guest_reset` first for a reset so the exit
    /// watcher reports it as `reset=true`, and publishing `exit_signalled`
    /// before the write so a shared wakeup cannot be lost.
    pub fn new(
        shutdown_evt: EventFd,
        guest_reset: Arc<AtomicBool>,
        exit_signalled: Arc<AtomicBool>,
        finish_requested: Arc<AtomicBool>,
    ) -> Self {
        Self {
            shutdown_evt,
            guest_reset,
            exit_signalled,
            finish_requested,
        }
    }

    fn signal_exit(&self) {
        self.exit_signalled.store(true, Ordering::SeqCst);
        if let Err(err) = self.shutdown_evt.write(1) {
            eprintln!("warning: failed to trigger ACPI exit event: {err:?}");
        }
    }

    /// Blocks the calling vCPU thread until the exit watcher has started
    /// stopping the vCPUs. Without this, the guest would return from the reset
    /// register write and carry on down its reboot sequence (keyboard
    /// controller, EFI, BIOS real-mode jump), which double-faults on this KVM
    /// port. The watcher sets `finish_requested` early in `shutdown_vcpus`, so
    /// this spins only until the stop is already in flight.
    fn spin_until_stopped(&self) {
        while !self.finish_requested.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

impl BusDevice for Pm1CntDevice {
    fn read(&mut self, _base: u64, _offset: u64, data: &mut [u8]) {
        // All registers are byte-wide; a wrong-sized read is ignored (the
        // dispatcher has already zeroed the buffer).
        if data.len() != 1 {
            return;
        }
        // Reading any register returns "no sleep/reset in progress".
        data[0] = 0;
    }

    fn write(&mut self, _base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        // All registers are byte-wide; a wrong-sized write is ignored.
        if data.len() != 1 {
            return None;
        }

        match offset {
            OFS_SLEEP_CONTROL => {
                // Enter S5 only: SLP_EN set with SLP_TYP=5. Any other sleep
                // state is unsupported and is ignored rather than misread as a
                // shutdown (fail closed).
                let value = data[0];
                let slp_typ = (value >> SLP_TYP_SHIFT) & 0x7;
                let slp_en = (value >> SLP_EN_BIT) & 0x1;
                if slp_en == 1 && slp_typ == S5_SLEEP_TYPE {
                    self.signal_exit();
                }
            }
            OFS_RESET => {
                // A guest reboot writes RESET_VALUE to the reset register.
                // This is a reset, not a shutdown: publish that before firing
                // the exit event so the watcher reports reset=true.
                if data[0] == RESET_VALUE {
                    self.guest_reset.store(true, Ordering::SeqCst);
                    self.signal_exit();
                    // Do not return to the guest: hold this vCPU until the
                    // watcher has begun stopping the vCPUs, so the guest
                    // cannot continue its reboot sequence into the BIOS
                    // real-mode jump (which double-faults on KVM).
                    self.spin_until_stopped();
                }
            }
            // SLEEP_STATUS: the guest clears/writes WAK_STS here, but this VM
            // never sleeps — it stops on S5 instead.
            OFS_SLEEP_STATUS => {}
            // Any other offset is outside the device's window.
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use crate::pal::eventfd::EFD_NONBLOCK;
    use crate::pal::eventfd::EventFd;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;

    fn device() -> (Pm1CntDevice, EventFd, Arc<AtomicBool>) {
        let evt = EventFd::new(EFD_NONBLOCK).unwrap();
        let reset = Arc::new(AtomicBool::new(false));
        // Pre-set finish_requested so the reset path's stop-spin returns
        // immediately: a unit test has no exit watcher to set it.
        let finish = Arc::new(AtomicBool::new(true));
        let exit_signalled = Arc::new(AtomicBool::new(false));
        (
            Pm1CntDevice::new(
                evt.try_clone().unwrap(),
                Arc::clone(&reset),
                exit_signalled,
                finish,
            ),
            evt,
            reset,
        )
    }

    /// The canonical S5 write: SLP_EN (bit 5) with SLP_TYP=5 (bits 4:2). It
    /// fires the exit event and is a shutdown, so guest_reset stays false.
    #[test]
    fn slp_en_with_s5_type_fires_shutdown() {
        let (mut dev, evt, reset) = device();
        dev.write(0, OFS_SLEEP_CONTROL, &[0x34]); // (5 << 2) | (1 << 5)
        assert_eq!(evt.read().unwrap(), 1);
        assert!(!reset.load(Ordering::SeqCst));
    }

    /// SLP_EN without SLP_TYP=5 (here SLP_TYP=0) is not a shutdown: the VM
    /// does not support the other sleep states, so it must not stop.
    #[test]
    fn slp_en_without_s5_type_is_ignored() {
        let (mut dev, evt, _) = device();
        dev.write(0, OFS_SLEEP_CONTROL, &[0x20]); // SLP_EN set, SLP_TYP=0
        // The eventfd is non-blocking: an empty counter reads as `WouldBlock`,
        // not `0`, so a "did not fire" check asserts the read errors.
        assert!(evt.read().is_err());
    }

    /// A write to the sleep status register (offset 1) never shuts the VM down.
    #[test]
    fn sleep_status_write_is_ignored() {
        let (mut dev, evt, _) = device();
        dev.write(0, OFS_SLEEP_STATUS, &[0x80]); // WAK_STS
        assert!(evt.read().is_err());
    }

    /// A write of RESET_VALUE to the reset register is a reset: the event fires
    /// and guest_reset is set so the exit watcher reports reset=true.
    #[test]
    fn reset_register_write_fires_reset() {
        let (mut dev, evt, reset) = device();
        dev.write(0, OFS_RESET, &[RESET_VALUE]);
        assert_eq!(evt.read().unwrap(), 1);
        assert!(reset.load(Ordering::SeqCst));
    }

    /// A write of any other value to the reset register is not a reset.
    #[test]
    fn reset_register_other_value_is_ignored() {
        let (mut dev, evt, reset) = device();
        dev.write(0, OFS_RESET, &[0]);
        assert!(evt.read().is_err());
        assert!(!reset.load(Ordering::SeqCst));
    }

    /// Wrong-sized writes are ignored (all registers are byte-wide).
    #[test]
    fn wrong_sized_write_is_ignored() {
        let (mut dev, evt, _) = device();
        dev.write(0, OFS_SLEEP_CONTROL, &[0x34, 0x00]);
        assert!(evt.read().is_err());
    }

    /// Reads return zero.
    #[test]
    fn reads_return_zero() {
        let (mut dev, _evt, _) = device();
        let mut buf = [0xffu8; 1];
        dev.read(0, OFS_SLEEP_CONTROL, &mut buf);
        assert_eq!(buf[0], 0);
    }
}
