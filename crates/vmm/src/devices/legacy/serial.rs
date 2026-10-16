// Copyright 2021 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/devices/legacy/serial.rs (trimmed:
// the event-manager input plumbing (stdin/epoll), the rate limiter, the
// metrics counters and the stdout/file sinks are dropped; the queue events
// of `SerialEventsWrapper` become no-ops; a `Shared` sink appends the guest
// serial output to an in-memory buffer shared with the VM handle; the
// console input the event manager used to read from stdin is instead queued
// through `SerialWrapper::queue_input`, which calls the same
// `RawIOHandler::raw_input` it did).

//! Implements a wrapper over an UART serial device.
use std::fmt::Debug;
use std::io;
use std::sync::{Arc, Barrier, Condvar, Mutex};

use crate::pal::eventfd::EventFd;
use vm_superio::Serial;
use vm_superio::Trigger;
use vm_superio::serial::{Error as SerialError, SerialEvents};

use crate::devices::legacy::EventFdTrigger;
use crate::vstate::bus::BusDevice;

#[derive(Debug, thiserror::Error)]
pub enum RawIOError {
    /// Serial error: {0:?}
    #[error("Serial error: {0:?}")]
    Serial(SerialError<io::Error>),
}

pub trait RawIOHandler {
    /// Send raw input to this emulated device.
    fn raw_input(&mut self, _data: &[u8]) -> Result<(), RawIOError>;
}

impl<EV: SerialEvents + Debug, W: std::io::Write + Debug> RawIOHandler
    for Serial<EventFdTrigger, EV, W>
{
    // Upstream calls this a dummy implementation; here it is the console
    // input path (`SerialWrapper::queue_input`).
    fn raw_input(&mut self, data: &[u8]) -> Result<(), RawIOError> {
        // Fail fast if the serial is serviced with more data than it can buffer.
        if data.len() > self.fifo_capacity() {
            return Err(RawIOError::Serial(SerialError::FullFifo));
        }

        // Before enqueuing bytes we first check if there is enough free space
        // in the FIFO.
        if self.fifo_capacity() >= data.len() {
            self.enqueue_raw_bytes(data).map_err(RawIOError::Serial)?;
        }
        Ok(())
    }
}

/// Wrapper over available events (i.e metrics, buffer ready etc).
///
/// With the event manager and metrics gone, the queue events are no-ops;
/// the `buffer_ready_event_fd` is kept for state fidelity with upstream.
#[derive(Debug)]
pub struct SerialEventsWrapper {
    /// Buffer ready event.
    pub buffer_ready_event_fd: Option<EventFdTrigger>,
}

impl SerialEvents for SerialEventsWrapper {
    fn buffer_read(&self) {
        // No-op (upstream: metric increment).
    }
    fn out_byte(&self) {
        // No-op (upstream: metric increment).
    }
    fn tx_lost_byte(&self) {
        // No-op (upstream: metric increment).
    }
    fn in_buffer_empty(&self) {
        // No-op (upstream: signal the buffer-ready event fd for the event
        // manager's input loop, which this port does not have).
    }
}

/// The underlying output destination.
#[derive(Debug)]
pub enum SerialOutInner {
    /// Discards every byte (the upstream default sink).
    Sink,
    /// Appends every byte to a shared in-memory buffer (the serial capture
    /// shared between the serial device and the VM handle).
    Shared(Arc<Mutex<Vec<u8>>>, Arc<Condvar>),
}

/// Upper bound on the in-memory serial capture: guest console output is
/// guest-controlled, so it must not grow the VMM's memory without bound.
/// Output past the bound is dropped; the head (which holds the boot log) is
/// kept.
const MAX_SERIAL_CAPTURE: usize = 4 << 20;

/// The UART's IIR "no interrupt pending" bit (16550A).
const IIR_NONE_BIT: u8 = 0b0000_0001;

/// Output sink for the serial device.
#[derive(Debug)]
pub struct SerialOut {
    inner: SerialOutInner,
}

impl SerialOut {
    /// Create a new `SerialOut` with the given inner sink.
    pub fn new(inner: SerialOutInner) -> Self {
        Self { inner }
    }

    /// Create the shared serial capture sink.
    pub fn shared(buffer: Arc<Mutex<Vec<u8>>>, ready: Arc<Condvar>) -> Self {
        Self {
            inner: SerialOutInner::Shared(buffer, ready),
        }
    }
}

impl std::io::Write for SerialOut {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match &mut self.inner {
            SerialOutInner::Sink => Ok(buf.len()),
            SerialOutInner::Shared(buffer, ready) => {
                {
                    let mut buffer = buffer
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    // Keep at most `MAX_SERIAL_CAPTURE` bytes: the boot log is at
                    // the head, and a chatty guest must not grow VMM memory
                    // without bound.
                    let room = MAX_SERIAL_CAPTURE.saturating_sub(buffer.len());
                    let taken = room.min(buf.len());
                    buffer.extend_from_slice(&buf[..taken]);
                }
                ready.notify_all();
                Ok(buf.len())
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Wrapper over the imported serial device.
///
/// Upstream is generic over a serial input source (`I: Read + AsRawFd +
/// Send`) wired to the event manager, which is what fed the guest's console
/// from stdin. This port has no event manager: the wrapper owns only the
/// serial device itself, and the host's console input is pushed into it by
/// [`SerialWrapper::queue_input`] (the stdin path upstream reached through
/// `RawIOHandler::raw_input`, which is what that method calls).
#[derive(Debug)]
pub struct SerialWrapper {
    /// Serial device object.
    pub serial: Serial<EventFdTrigger, SerialEventsWrapper, SerialOut>,
}

/// Type for representing a serial device.
pub type SerialDevice = SerialWrapper;

impl SerialDevice {
    pub fn new(serial_out: SerialOut) -> Result<Self, std::io::Error> {
        let interrupt_evt = EventFdTrigger::new(EventFd::new(crate::pal::eventfd::EFD_NONBLOCK)?);
        let buffer_read_event_fd =
            EventFdTrigger::new(EventFd::new(crate::pal::eventfd::EFD_NONBLOCK)?);
        let events = SerialEventsWrapper {
            buffer_ready_event_fd: Some(buffer_read_event_fd),
        };

        let serial = Serial::with_events(interrupt_evt, events, serial_out);

        Ok(SerialDevice { serial })
    }

    /// The UART's registers, as a snapshot keeps them.
    pub fn registers(&self) -> crate::vstate::snapshot::SerialRegisters {
        let state = self.serial.state();
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

    /// A UART with the registers a snapshot kept, writing to `serial_out`, with new interrupt events.
    pub fn from_registers(
        registers: &crate::vstate::snapshot::SerialRegisters,
        serial_out: SerialOut,
    ) -> Result<Self, String> {
        let fresh = || {
            EventFd::new(crate::pal::eventfd::EFD_NONBLOCK)
                .map(EventFdTrigger::new)
                .map_err(|err| format!("the serial console: {err}"))
        };
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
        let events = SerialEventsWrapper {
            buffer_ready_event_fd: Some(fresh()?),
        };
        let serial = Serial::from_state(&state, fresh()?, events, serial_out)
            .map_err(|err| format!("the serial console: {err:?}"))?;
        Ok(SerialDevice { serial })
    }

    /// Raises the UART's interrupt again when its saved registers hold one the guest has not taken.
    ///
    /// The model raises an interrupt only while its IIR bit is clear, and the line is a KVM irqfd
    /// pulse. A snapshot can catch the bit set while the pulse never reached the saved interrupt
    /// controller state, so the restored guest would wait for an interrupt that never comes, and every
    /// console write would block until something else read the IIR. A spurious pulse is harmless: the
    /// driver reads IIR "none" and returns.
    pub fn raise_pending_interrupt(&self) -> std::io::Result<()> {
        if self.serial.state().interrupt_identification & IIR_NONE_BIT == 0 {
            self.serial.interrupt_evt().trigger()?;
        }
        Ok(())
    }

    /// Queues `data` as console input for the guest to read from the UART's
    /// data register, raising the console's received-data interrupt.
    ///
    /// Everything or nothing: a slice that does not fit in the input queue is
    /// refused (upstream's `RawIOHandler::raw_input`, which the event manager
    /// fed at most `fifo_capacity()` bytes at a time) and nothing is queued, so
    /// a caller can retry the same slice once the guest has drained what is
    /// there. The reason is the text a caller sees in
    /// `crate::spawner::SpawnError::SerialInput`.
    pub fn queue_input(&mut self, data: &[u8]) -> Result<(), String> {
        self.serial.raw_input(data).map_err(|err| {
            format!(
                "the console's input queue takes {} more bytes and was given {}: {err}",
                self.serial.fifo_capacity(),
                data.len()
            )
        })
    }
}

impl BusDevice for SerialWrapper {
    fn read(&mut self, _base: u64, offset: u64, data: &mut [u8]) {
        if let (Ok(offset), 1) = (u8::try_from(offset), data.len()) {
            data[0] = self.serial.read(offset);
        }
        // Wider or unaligned reads are dropped (upstream counts a missed-read
        // metric here).
    }

    fn write(&mut self, _base: u64, offset: u64, data: &[u8]) -> Option<Arc<Barrier>> {
        if let (Ok(offset), 1) = (u8::try_from(offset), data.len())
            && let Err(err) = self.serial.write(offset, data[0])
        {
            eprintln!("warning: failed the write to serial: {err:?}");
        }
        // Wider or unaligned writes are dropped (upstream counts a
        // missed-write metric here).
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a `SerialOut` with `Sink` inner for tests.
    fn test_serial_out_sink() -> SerialOut {
        SerialOut::new(SerialOutInner::Sink)
    }

    #[test]
    fn a_restored_console_raises_the_interrupt_the_guest_had_not_taken() {
        // The registers a snapshot kept: THRE raised and not yet read (IIR 0x02), or nothing pending
        // (IIR 0x01). Only the first raises the line again.
        for (iir, raised) in [(0x02u8, true), (IIR_NONE_BIT, false)] {
            let mut registers = SerialDevice::new(test_serial_out_sink())
                .unwrap()
                .registers();
            registers.interrupt_enable = 0x02;
            registers.interrupt_identification = iir;
            let serial = SerialDevice::from_registers(&registers, test_serial_out_sink()).unwrap();
            serial.raise_pending_interrupt().unwrap();
            assert_eq!(
                serial.serial.interrupt_evt().read().is_ok(),
                raised,
                "IIR {iir:#04x}"
            );
        }
    }

    #[test]
    fn test_serial_bus_read() {
        let mut serial = SerialDevice::new(test_serial_out_sink()).unwrap();

        serial.serial.raw_input(b"abc").unwrap();

        // A wider read is dropped and leaves the FIFO untouched.
        let mut v = [0x00; 2];
        serial.read(0x0, 0u64, &mut v);
        assert_eq!(v, [0x00; 2]);

        let mut v = [0x00; 1];
        serial.read(0x0, 0u64, &mut v);
        assert_eq!(v[0], b'a');

        // The wider read did not consume a byte: the next one is 'b'.
        serial.read(0x0, 0u64, &mut v);
        assert_eq!(v[0], b'b');
    }

    #[test]
    fn test_serial_bus_write_to_shared_sink() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let mut serial = SerialDevice::new(SerialOut::shared(
            Arc::clone(&buffer),
            Arc::new(std::sync::Condvar::new()),
        ))
        .unwrap();

        // The kernel writes bytes one at a time to the UART data register
        // (offset 0).
        for byte in b"Linux version\n" {
            serial.write(0x3f8, 0, &[*byte]);
        }

        let captured = buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(captured.as_slice(), b"Linux version\n".as_slice());
    }

    #[test]
    fn test_queue_input_is_readable_from_the_data_register() {
        let mut serial = SerialDevice::new(test_serial_out_sink()).unwrap();

        // Console input comes from the host, not from a bus write: the bytes
        // become readable through the data register, which is what the guest
        // driver's read of ttyS0 does (and the interrupt that tells it to
        // read is raised by the queue).
        serial.queue_input(b"uname\n").expect("queueing input");

        let mut byte = [0u8; 1];
        for expected in b"uname\n" {
            serial.read(0x3f8, 0, &mut byte);
            assert_eq!(byte[0], *expected);
        }

        // The queue is drained: a read with nothing queued returns the
        // register's default instead of a stale byte.
        serial.read(0x3f8, 0, &mut byte);
        assert_eq!(byte[0], 0);
    }

    #[test]
    fn test_queue_input_refuses_a_chunk_larger_than_the_queue() {
        let mut serial = SerialDevice::new(test_serial_out_sink()).unwrap();

        // Everything or nothing: a chunk the queue cannot hold whole is
        // refused *and queues nothing*, so a caller that retries it does not
        // duplicate the bytes that were already taken.
        let oversized = vec![b'a'; 1024];
        let reason = serial.queue_input(&oversized).unwrap_err();
        assert!(
            reason.contains("1024"),
            "the reason must say what was refused: {reason}"
        );

        let mut byte = [0u8; 1];
        serial.read(0x3f8, 0, &mut byte);
        assert_eq!(byte[0], 0, "a refused chunk must not queue anything");

        // The queue still works afterwards.
        serial.queue_input(b"x").expect("queueing a fitting chunk");
        serial.read(0x3f8, 0, &mut byte);
        assert_eq!(byte[0], b'x');
    }
}
