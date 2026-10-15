// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. The wrapper shape - the bounded in-memory capture,
// the `BusDevice` adaptation and the byte-only register handling - mirrors
// `devices/legacy/serial.rs` (itself derived from Firecracker's
// `src/vmm/src/devices/legacy/serial.rs`); what differs is the interrupt
// trigger: this path cannot use the KVM path's `EventFd`, which macOS does not
// have, so it points the trigger at the userspace interrupt controller
// instead. The device itself is the `vm-superio` `Serial` both paths share.

//! The serial console of an aarch64 VM on Hypervisor.framework: a 16550 UART
//! at the address the aarch64 layout gives it (`SERIAL_MEM_START`), whose
//! output is
//! appended to the VM's capture and whose interrupt is an SPI of the
//! userspace interrupt controller.
//!
//! The device is reached through the [`BusDevice`] trait, so it is the same
//! device model the KVM path registers - only how its interrupt line is
//! raised differs.

use std::io::{self, Write};
use std::sync::{Arc, Barrier, Mutex, MutexGuard, PoisonError, Weak};

use vm_superio::Trigger;
use vm_superio::serial::{NoEvents, Serial};

use crate::vstate::bus::BusDevice;

/// Upper bound on the in-memory console capture: the guest's output is guest
/// controlled, so it must not grow the VMM's memory without bound. Output past
/// the bound is dropped; the head (the boot log) is kept. The KVM path's
/// wrapper keeps the same bound.
const MAX_CAPTURE: usize = 4 << 20;

/// The interrupt line of the UART, seen from the device: raising it asserts
/// the console's SPI in the VM's interrupt controller and wakes the vCPU that
/// has to take it.
///
/// The device is written from the vCPU thread that faulted on it, and the
/// interrupt may have to reach another vCPU, so the line is a trait rather
/// than a direct reference to the VM.
pub trait ConsoleLine: Send + Sync + std::fmt::Debug {
    /// Asserts the console's interrupt.
    fn raise(&self) -> io::Result<()>;
}

/// The `vm-superio` trigger over a [`ConsoleLine`].
///
/// The line is held **weakly**: in this backend the line *is* the VM, and the
/// device lives in the VM's own state, so a strong reference would make the
/// device and the VM keep each other alive - the VM, its vCPU threads and its
/// guest memory would never be released after it stopped.
#[derive(Debug)]
pub struct GicTrigger {
    line: Weak<dyn ConsoleLine>,
}

impl Trigger for GicTrigger {
    type E = io::Error;

    fn trigger(&self) -> io::Result<()> {
        match self.line.upgrade() {
            Some(line) => line.raise(),
            // The VM is gone, so there is no interrupt controller left to
            // raise the console's interrupt in and nothing to report.
            None => Ok(()),
        }
    }
}

/// Writes the guest's console output into the capture shared with the VM
/// handle.
#[derive(Debug)]
pub struct SharedSink {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl SharedSink {
    /// A sink appending to `buffer`, which the caller shares with whoever
    /// reads the console.
    pub fn new(buffer: Arc<Mutex<Vec<u8>>>) -> Self {
        Self { buffer }
    }

    /// The capture, for a caller that wants to read it without going through
    /// the VM handle.
    pub fn buffer(&self) -> MutexGuard<'_, Vec<u8>> {
        self.buffer.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Write for SharedSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut capture = self.buffer();
        let room = MAX_CAPTURE.saturating_sub(capture.len());
        capture.extend_from_slice(&buf[..buf.len().min(room)]);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The console device: the `vm-superio` UART with the VM's capture sink and
/// interrupt line.
#[derive(Debug)]
pub struct SerialDevice {
    uart: Serial<GicTrigger, NoEvents, SharedSink>,
}

impl SerialDevice {
    /// Creates the console: `capture` receives the guest's output and `line`
    /// is asserted when the UART has an interrupt to raise.
    ///
    /// Only a weak reference to `line` is kept (see [`GicTrigger`]): the
    /// caller keeps ownership of the VM, and the device must not extend its
    /// life.
    pub fn new(capture: Arc<Mutex<Vec<u8>>>, line: &Arc<dyn ConsoleLine>) -> Self {
        let trigger = GicTrigger {
            line: Arc::downgrade(line),
        };
        Self {
            uart: Serial::with_events(trigger, NoEvents, SharedSink::new(capture)),
        }
    }

    /// The UART's registers, as a snapshot keeps them.
    pub fn registers(&self) -> crate::vstate::snapshot::SerialRegisters {
        let state = self.uart.state();
        crate::vstate::snapshot::SerialRegisters {
            baud_divisor_low: state.baud_divisor_low,
            baud_divisor_high: state.baud_divisor_high,
            interrupt_enable: state.interrupt_enable,
            interrupt_identification: state.interrupt_identification,
            line_control: state.line_control,
            line_status: state.line_status,
            modem_control: state.modem_control,
            modem_status: state.modem_status,
            scratch: state.scratch,
            in_buffer: state.in_buffer,
        }
    }

    /// The console with the registers a snapshot kept (see [`SerialDevice::new`] for `capture` and
    /// `line`).
    pub fn from_registers(
        registers: &crate::vstate::snapshot::SerialRegisters,
        capture: Arc<Mutex<Vec<u8>>>,
        line: &Arc<dyn ConsoleLine>,
    ) -> Result<Self, String> {
        let state = vm_superio::serial::SerialState {
            baud_divisor_low: registers.baud_divisor_low,
            baud_divisor_high: registers.baud_divisor_high,
            interrupt_enable: registers.interrupt_enable,
            interrupt_identification: registers.interrupt_identification,
            line_control: registers.line_control,
            line_status: registers.line_status,
            modem_control: registers.modem_control,
            modem_status: registers.modem_status,
            scratch: registers.scratch,
            in_buffer: registers.in_buffer.clone(),
        };
        let trigger = GicTrigger {
            line: Arc::downgrade(line),
        };
        let uart = Serial::from_state(&state, trigger, NoEvents, SharedSink::new(capture))
            .map_err(|err| format!("the serial console: {err:?}"))?;
        Ok(Self { uart })
    }

    /// Queues `data` as console input for the guest to read from the UART's
    /// data register, raising the console's received-data interrupt.
    ///
    /// This is the `ttyS0`/`ttyAMA0` counterpart of the guest's own writes:
    /// the same contract as the KVM path's
    /// `devices::legacy::SerialWrapper::queue_input` (everything or nothing,
    /// with a reason the caller can show), and the same `vm-superio` queue
    /// underneath. The reason is the text a caller sees in
    /// `crate::spawner::SpawnError::SerialInput`.
    pub fn queue_input(&mut self, data: &[u8]) -> Result<(), String> {
        let room = self.uart.fifo_capacity();
        if data.len() > room {
            return Err(format!(
                "the console's input queue takes {room} more bytes and was given {}",
                data.len()
            ));
        }
        self.uart
            .enqueue_raw_bytes(data)
            .map(|_| ())
            .map_err(|err| format!("the console's input queue rejected the data: {err}"))
    }
}

impl BusDevice for SerialDevice {
    fn read(&mut self, _base: u64, offset: u64, data: &mut [u8]) {
        if let (Ok(offset), 1) = (u8::try_from(offset), data.len()) {
            data[0] = self.uart.read(offset);
        }
        // Wider or unaligned reads are dropped, as in the KVM path's wrapper:
        // the ns16550a driver only ever does byte accesses to a UART.
    }

    fn write(&mut self, _base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        if let (Ok(offset), 1) = (u8::try_from(offset), data.len())
            && let Err(err) = self.uart.write(offset, data[0])
        {
            eprintln!("warning: failed the write to serial: {err:?}");
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A console line that counts how often the device asked for an interrupt.
    #[derive(Debug, Default)]
    struct CountingLine {
        raises: Arc<AtomicUsize>,
    }

    impl CountingLine {
        fn new(raises: Arc<AtomicUsize>) -> Self {
            Self { raises }
        }
    }

    impl ConsoleLine for CountingLine {
        fn raise(&self) -> io::Result<()> {
            self.raises.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// A console with a fresh capture, a line that counts raises, and the line
    /// itself - which the test keeps alive exactly as the VM keeps itself
    /// alive, because the device only holds a weak reference to it.
    struct Console {
        device: SerialDevice,
        capture: Arc<Mutex<Vec<u8>>>,
        raises: Arc<AtomicUsize>,
        /// Held for the test's duration: the device holds only a weak
        /// reference, so dropping this is what makes the line go away.
        _line: Arc<dyn ConsoleLine>,
    }

    fn console() -> Console {
        let capture = Arc::new(Mutex::new(Vec::new()));
        let raises = Arc::new(AtomicUsize::new(0));
        let line: Arc<dyn ConsoleLine> = Arc::new(CountingLine::new(Arc::clone(&raises)));
        let device = SerialDevice::new(Arc::clone(&capture), &line);
        Console {
            device,
            capture,
            raises,
            _line: line,
        }
    }

    #[test]
    fn the_console_appends_what_the_guest_writes() {
        let mut console = console();

        // IER: enable the "transmitter holding register empty" interrupt, then
        // transmit a byte through the THR, as the 8250 driver does.
        console.device.write(0, 1, &[0x02]);
        console.device.write(0, 0, b"h");

        assert_eq!(&console.capture.lock().unwrap()[..], b"h");
        assert_eq!(
            console.raises.load(Ordering::SeqCst),
            1,
            "the first byte raises the console's interrupt"
        );

        // The line stays asserted until the driver acknowledges it, so the
        // next transmitted byte raises nothing new ...
        console.device.write(0, 0, b"i");
        assert_eq!(console.raises.load(Ordering::SeqCst), 1);

        // ... and once the driver has read the interrupt identification
        // register, the next byte does.
        let mut identification = [0u8; 1];
        console.device.read(0, 2, &mut identification);
        assert_eq!(identification[0] & 1, 0, "an interrupt was pending");
        console.device.write(0, 0, b"!");
        assert_eq!(console.raises.load(Ordering::SeqCst), 2);

        assert_eq!(&console.capture.lock().unwrap()[..], b"hi!");
    }

    #[test]
    fn the_console_does_not_keep_the_vm_alive() {
        // The line is the VM in this backend, and the device lives in the
        // VM's own state: a device holding a *strong* reference to its line
        // makes the two keep each other alive, so a stopped VM never releases
        // its vCPU threads, its guest memory or its hypervisor VM.
        let capture = Arc::new(Mutex::new(Vec::new()));
        let raises = Arc::new(AtomicUsize::new(0));
        let line: Arc<dyn ConsoleLine> = Arc::new(CountingLine::new(Arc::clone(&raises)));
        let mut device = SerialDevice::new(Arc::clone(&capture), &line);

        device.write(0, 1, &[0x02]);
        device.write(0, 0, b"a");
        assert_eq!(
            raises.load(Ordering::SeqCst),
            1,
            "the line raises while the VM is alive"
        );

        // The VM goes away; the device must not have been holding it.
        drop(line);

        // Acknowledge the pending interrupt, so that another byte would raise
        // the line again if the VM were still there.
        let mut identification = [0u8; 1];
        device.read(0, 2, &mut identification);
        device.write(0, 0, b"b");
        assert_eq!(
            raises.load(Ordering::SeqCst),
            1,
            "a dropped VM's line can no longer be raised"
        );
        // ... and the console still captures the guest's output.
        assert_eq!(&capture.lock().unwrap()[..], b"ab");
    }

    #[test]
    fn console_input_is_readable_from_the_data_register() {
        let mut console = console();

        // The driver enables the received-data interrupt (IER bit 0), as the
        // ns16550a driver does before it can be told about input.
        console.device.write(0, 1, &[0x01]);
        console
            .device
            .queue_input(b"help\n")
            .expect("queueing console input");

        let mut byte = [0u8; 1];
        for expected in b"help\n" {
            console.device.read(0, 0, &mut byte);
            assert_eq!(byte[0], *expected);
        }
        assert_eq!(
            console.raises.load(Ordering::SeqCst),
            1,
            "queued input raises the console's interrupt"
        );

        // Drained: a read with nothing queued returns the register default.
        console.device.read(0, 0, &mut byte);
        assert_eq!(byte[0], 0);
    }

    #[test]
    fn console_input_refuses_a_chunk_larger_than_the_queue() {
        let mut console = console();

        // Everything or nothing, as on the KVM path: a refused chunk leaves
        // the queue empty, so a caller that retries it does not duplicate
        // bytes.
        let oversized = vec![b'a'; 1024];
        let reason = console.device.queue_input(&oversized).unwrap_err();
        assert!(
            reason.contains("1024"),
            "the reason must say what was refused: {reason}"
        );

        let mut byte = [0u8; 1];
        console.device.read(0, 0, &mut byte);
        assert_eq!(byte[0], 0, "a refused chunk must not queue anything");

        console
            .device
            .queue_input(b"x")
            .expect("queueing a fitting chunk");
        console.device.read(0, 0, &mut byte);
        assert_eq!(byte[0], b'x');
    }

    #[test]
    fn a_line_register_can_be_read_and_wide_accesses_are_dropped() {
        let mut console = console();

        let mut line_status = [0u8; 1];
        console.device.read(0, 5, &mut line_status);

        // A wider access is dropped, as in the KVM path's wrapper: the guest's
        // UART driver only ever does byte accesses.
        let mut wide = [0u8; 2];
        console.device.read(0, 5, &mut wide);
        assert_eq!(wide, [0, 0]);
        assert_eq!(
            console.raises.load(Ordering::SeqCst),
            0,
            "a read raises nothing"
        );
    }

    #[test]
    fn the_capture_does_not_grow_without_bound() {
        let capture = Arc::new(Mutex::new(Vec::new()));
        let mut sink = SharedSink::new(Arc::clone(&capture));
        let written = sink.write(&vec![b'x'; MAX_CAPTURE + 16]).unwrap();
        assert_eq!(written, MAX_CAPTURE + 16, "the device is told it all went");
        assert_eq!(capture.lock().unwrap().len(), MAX_CAPTURE);
    }
}
