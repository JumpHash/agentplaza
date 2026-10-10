// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. Firecracker's console wrapper
// (`devices/legacy/serial.rs`) is Linux-only here because its interrupt
// trigger rides on an `EventFd`, which macOS does not have; this is the same
// device with the trigger pointed at the userspace interrupt controller
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
use std::sync::{Arc, Barrier, Mutex, MutexGuard, PoisonError};

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
#[derive(Debug)]
pub struct GicTrigger {
    line: Arc<dyn ConsoleLine>,
}

impl Trigger for GicTrigger {
    type E = io::Error;

    fn trigger(&self) -> io::Result<()> {
        self.line.raise()
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
    pub fn new(capture: Arc<Mutex<Vec<u8>>>, line: Arc<dyn ConsoleLine>) -> Self {
        let trigger = GicTrigger { line };
        Self {
            uart: Serial::with_events(trigger, NoEvents, SharedSink::new(capture)),
        }
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

    /// A console with a fresh capture and a line that counts raises.
    fn console() -> (SerialDevice, Arc<Mutex<Vec<u8>>>, Arc<AtomicUsize>) {
        let capture = Arc::new(Mutex::new(Vec::new()));
        let raises = Arc::new(AtomicUsize::new(0));
        let line: Arc<dyn ConsoleLine> = Arc::new(CountingLine::new(Arc::clone(&raises)));
        (
            SerialDevice::new(Arc::clone(&capture), line),
            capture,
            raises,
        )
    }

    #[test]
    fn the_console_appends_what_the_guest_writes() {
        let (mut device, capture, raises) = console();

        // IER: enable the "transmitter holding register empty" interrupt, then
        // transmit a byte through the THR, as the 8250 driver does.
        device.write(0, 1, &[0x02]);
        device.write(0, 0, b"h");

        assert_eq!(&capture.lock().unwrap()[..], b"h");
        assert_eq!(
            raises.load(Ordering::SeqCst),
            1,
            "the first byte raises the console's interrupt"
        );

        // The line stays asserted until the driver acknowledges it, so the
        // next transmitted byte raises nothing new ...
        device.write(0, 0, b"i");
        assert_eq!(raises.load(Ordering::SeqCst), 1);

        // ... and once the driver has read the interrupt identification
        // register, the next byte does.
        let mut identification = [0u8; 1];
        device.read(0, 2, &mut identification);
        assert_eq!(identification[0] & 1, 0, "an interrupt was pending");
        device.write(0, 0, b"!");
        assert_eq!(raises.load(Ordering::SeqCst), 2);

        assert_eq!(&capture.lock().unwrap()[..], b"hi!");
    }

    #[test]
    fn a_line_register_can_be_read_and_wide_accesses_are_dropped() {
        let (mut device, _, raises) = console();

        let mut line_status = [0u8; 1];
        device.read(0, 5, &mut line_status);

        // A wider access is dropped, as in the KVM path's wrapper: the guest's
        // UART driver only ever does byte accesses.
        let mut wide = [0u8; 2];
        device.read(0, 5, &mut wide);
        assert_eq!(wide, [0, 0]);
        assert_eq!(raises.load(Ordering::SeqCst), 0, "a read raises nothing");
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
