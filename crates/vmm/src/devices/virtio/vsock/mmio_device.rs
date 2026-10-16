// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the mmio transport drives a `Box<dyn VirtioDevice>`
// whose queues are `QueueConfig` (ring *addresses* for a kernel-side backend),
// while the userspace vsock device ([`Vsock`]) owns real [`Queue`] rings. This
// adapter is the bridge: it is the `VirtioDevice` the transport sees, and it
// programs the driver's queue registers and feature acknowledgements into the
// shared [`Vsock`] at activation time.

//! The virtio-mmio face of the userspace vsock device.

use std::fmt::Debug;
use std::num::Wrapping;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};

use crate::pal::eventfd::EventFd;

use super::device::{DeviceError, Vsock};
use super::{VsockBackend, VsockError};
use crate::devices::virtio::VirtioError;
use crate::devices::virtio::ack_feature_page;
use crate::devices::virtio::mmio::{DeviceSnapshot, QueueConfig, QueuePosition, VirtioDevice};
use crate::devices::virtio::queue::Queue;
use crate::vstate::memory::{GuestAddress, GuestMemoryMmap};

/// `VIRTIO_ID_VSOCK`: the device type id of a virtio-vsock device.
const VIRTIO_ID_VSOCK: u32 = 19;

/// How many queues a virtio-vsock device has: receive, transmit, events.
const VSOCK_QUEUE_COUNT: usize = 3;

/// The ring size offered to the driver, matching the queues
/// [`Vsock::new`] creates (`FIRECRACKER_MAX_QUEUE_SIZE`).
const VSOCK_QUEUE_MAX_SIZE: u32 = 256;

/// The virtio-mmio device that hands the userspace vsock device to the
/// transport.
///
/// The device itself lives behind the shared `Arc<Mutex<..>>` so the VM's
/// vsock event thread can drive its ring processing concurrently with the
/// vCPU thread's register accesses; this adapter is only the transport's
/// register-level view of it.
#[derive(Debug)]
pub struct VsockMmioDevice<B> {
    /// The device, shared with the event thread.
    device: Arc<Mutex<Vsock<B>>>,
    /// The driver-visible queues (ring addresses), programmed into the device
    /// on activation.
    queues: [QueueConfig; VSOCK_QUEUE_COUNT],
}

impl<B> VsockMmioDevice<B>
where
    B: VsockBackend + Debug,
{
    /// Wraps `device` as the `VirtioDevice` the transport drives.
    pub fn new(device: Arc<Mutex<Vsock<B>>>) -> Self {
        Self {
            device,
            queues: [QueueConfig::new(VSOCK_QUEUE_MAX_SIZE); VSOCK_QUEUE_COUNT],
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vsock<B>> {
        self.device.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<B> VirtioDevice for VsockMmioDevice<B>
where
    B: VsockBackend + Debug,
{
    fn device_type(&self) -> u32 {
        VIRTIO_ID_VSOCK
    }

    fn features(&self) -> u64 {
        self.lock().avail_features()
    }

    fn ack_features_by_page(&mut self, page: u32, value: u32) -> Result<(), VirtioError> {
        let mut device = self.lock();
        let acked = ack_feature_page(
            device.avail_features(),
            device.acked_features(),
            page,
            value,
        )?;
        device.set_acked_features(acked);
        Ok(())
    }

    fn queues(&self) -> &[QueueConfig] {
        &self.queues
    }

    fn queues_mut(&mut self) -> &mut [QueueConfig] {
        &mut self.queues
    }

    fn read_config(&mut self, offset: u64, data: &mut [u8]) {
        let device = self.lock();
        let config = device.config_as_bytes();
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        for (index, byte) in data.iter_mut().enumerate() {
            *byte = config
                .get(start.saturating_add(index))
                .copied()
                .unwrap_or(0);
        }
    }

    fn notify_queue(&mut self, index: u32) -> Result<(), VirtioError> {
        // Out-of-range indices are dropped inside `Vsock::notify_queue`; the
        // index comes from a guest register write and must not fault the VMM.
        let Ok(index) = usize::try_from(index) else {
            return Ok(());
        };
        self.lock().notify_queue(index).map_err(VirtioError::from)
    }

    fn activate(&mut self, mem: &GuestMemoryMmap, irq: &EventFd) -> Result<(), VirtioError> {
        let mut device = self.lock();
        for (config, queue) in self.queues.iter().zip(device.queues_mut()) {
            // `config.size` is already validated to be a non-zero power of two
            // no larger than `max_size` (256), so it fits in a `u16`.
            queue.size = config.size as u16;
            queue.ready = config.ready;
            queue.desc_table_address = GuestAddress(config.desc);
            queue.avail_ring_address = GuestAddress(config.avail);
            queue.used_ring_address = GuestAddress(config.used);
        }
        let irq = irq.try_clone().map_err(super::ActivateError::EventFd)?;
        device.activate(mem.clone(), irq).map_err(VirtioError::from)
    }

    fn acked_features(&self) -> u64 {
        self.lock().acked_features()
    }

    fn save_device_state(&self) -> DeviceSnapshot {
        let device = self.lock();
        if !device.is_activated() {
            return DeviceSnapshot::default();
        }
        DeviceSnapshot {
            queues: queue_positions(device.queues()),
            // The RX gate: a `TRANSPORT_RESET` the guest has not acknowledged holds
            // RX delivery, so the restored device must hold it as well.
            extra: json!({ "pending_event_ack": device.pending_event_ack }),
        }
    }

    fn restore_device_state(&mut self, state: &DeviceSnapshot) -> Result<(), VirtioError> {
        let mut device = self.lock();
        let expected = if device.is_activated() {
            device.queues().len()
        } else {
            0
        };
        crate::devices::virtio::mmio::check_snapshot_queue_count(expected, state.queues.len())?;
        restore_queue_positions(device.queues_mut(), &state.queues);
        if let Some(armed) = state
            .extra
            .get("pending_event_ack")
            .and_then(Value::as_bool)
        {
            device.pending_event_ack = armed;
        }
        Ok(())
    }

    fn on_restored(&mut self) -> Result<(), VirtioError> {
        // A device the driver never activated has no event queue to publish on.
        let mut device = self.lock();
        if !device.is_activated() {
            return Ok(());
        }
        // The restored VM has a fresh muxer, so every connection the guest holds
        // is gone: the reset makes the guest drop them and re-read its CID. It
        // also arms the RX gate until the guest acknowledges the event.
        device.send_transport_reset_event().map_err(|err| {
            VirtioError::Vsock(match err {
                DeviceError::VsockError(err) => err,
                DeviceError::FailedSignalingIrq(err) => VsockError::EventFd(err),
                // The event queue's avail index is past its ring, so there is no
                // buffer the reset could be written into.
                DeviceError::QueueError(_) => VsockError::EmptyQueue,
            })
        })
    }

    fn reset(&mut self) {
        self.lock().reset();
        for queue in &mut self.queues {
            queue.reset();
        }
    }
}

/// The positions of `queues`, in queue order, as a snapshot carries them.
fn queue_positions(queues: &[Queue]) -> Vec<QueuePosition> {
    queues
        .iter()
        .map(|queue| QueuePosition {
            next_avail: queue.next_avail.0,
            next_used: queue.next_used.0,
        })
        .collect()
}

/// Sets each queue's position from the snapshot, in queue order.
fn restore_queue_positions(queues: &mut [Queue], positions: &[QueuePosition]) {
    for (queue, position) in queues.iter_mut().zip(positions) {
        queue.next_avail = Wrapping(position.next_avail);
        queue.next_used = Wrapping(position.next_used);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::virtio::queue::VIRTQ_DESC_F_WRITE;
    use crate::devices::virtio::test_utils::{VirtQueue, default_mem};
    use crate::devices::virtio::vsock::device::VIRTIO_VSOCK_EVENT_TRANSPORT_RESET;
    use crate::devices::virtio::vsock::device::tests::FakeBackend;
    use crate::vstate::memory::Bytes;

    /// Guest address of the event buffer the driver posts on the event queue.
    const EVENT_BUFFER: u64 = 0x8000;

    /// The three queues, laid out in `mem` as the driver would.
    fn guest_queues(mem: &GuestMemoryMmap) -> Vec<VirtQueue<'_>> {
        vec![
            VirtQueue::new(GuestAddress(0), mem, 16),
            VirtQueue::new(GuestAddress(0x2000), mem, 16),
            VirtQueue::new(GuestAddress(0x4000), mem, 16),
        ]
    }

    /// An adapter over a vsock device that the driver has not activated.
    fn unactivated() -> VsockMmioDevice<FakeBackend> {
        let device = Vsock::new(3, FakeBackend::default()).expect("a vsock device");
        VsockMmioDevice::new(Arc::new(Mutex::new(device)))
    }

    /// Programs the driver's registers for `vqs` and activates `device` over `mem`.
    fn activate_over(
        device: &mut VsockMmioDevice<FakeBackend>,
        mem: &GuestMemoryMmap,
        vqs: &[VirtQueue],
    ) {
        for (regs, vq) in device.queues_mut().iter_mut().zip(vqs) {
            regs.size = 16;
            regs.ready = true;
            regs.desc = vq.desc_table_addr().0;
            regs.avail = vq.avail_ring_addr().0;
            regs.used = vq.used_ring_addr().0;
        }
        let irq = EventFd::new(crate::pal::eventfd::EFD_NONBLOCK).expect("an interrupt event fd");
        VirtioDevice::activate(device, mem, &irq).expect("the device activates");
    }

    #[test]
    fn a_device_saves_no_queue_positions_before_activation() {
        let device = unactivated();
        assert_eq!(device.save_device_state(), DeviceSnapshot::default());
    }

    #[test]
    fn a_snapshot_restores_queue_positions_and_the_rx_gate() {
        let mem = default_mem();
        let vqs = guest_queues(&mem);

        let mut source = unactivated();
        activate_over(&mut source, &mem, &vqs);
        let features = source.features();
        source
            .ack_features_by_page(0, features as u32)
            .expect("the offered features acknowledge");
        source
            .ack_features_by_page(1, (features >> 32) as u32)
            .expect("the offered features acknowledge");
        assert_eq!(source.acked_features(), features);
        source.lock().queues_mut()[0].next_avail = Wrapping(5);
        source.lock().queues_mut()[1].next_used = Wrapping(2);
        source.lock().pending_event_ack = true;
        let saved = source.save_device_state();
        assert_eq!(saved.queues.len(), 3);

        let mut restored = unactivated();
        activate_over(&mut restored, &mem, &vqs);
        restored
            .restore_device_state(&saved)
            .expect("the snapshot restores onto the activated device");
        assert_eq!(restored.save_device_state(), saved);
        assert!(restored.lock().pending_event_ack);
    }

    #[test]
    fn restore_refuses_a_snapshot_with_the_wrong_queue_count() {
        let mem = default_mem();
        let vqs = guest_queues(&mem);
        let mut device = unactivated();
        activate_over(&mut device, &mem, &vqs);
        let mut state = device.save_device_state();
        state.queues.pop();
        assert!(device.restore_device_state(&state).is_err());
    }

    /// `on_restored` publishes a transport reset on the event queue once the
    /// device is activated, and does nothing before activation.
    #[test]
    fn on_restored_publishes_reset_only_when_activated() {
        // The reset is published on the event queue after activation.
        {
            let mem = default_mem();
            let vqs = guest_queues(&mem);
            let mut device = unactivated();
            activate_over(&mut device, &mem, &vqs);

            // The driver has posted one event buffer.
            vqs[2].set_desc(0, EVENT_BUFFER, 4, VIRTQ_DESC_F_WRITE, 0);
            vqs[2].publish(0);
            assert_eq!(vqs[2].used_idx(), 0);

            device
                .on_restored()
                .expect("the reset is published on the event queue");
            assert_eq!(vqs[2].used_idx(), 1);
            assert_eq!(
                mem.read_obj::<u32>(GuestAddress(EVENT_BUFFER))
                    .expect("the event buffer is in guest memory"),
                VIRTIO_VSOCK_EVENT_TRANSPORT_RESET
            );
            assert!(device.lock().pending_event_ack);
        }

        // An unactivated device has no event queue, so on_restored is a no-op.
        {
            let mut device = unactivated();
            device
                .on_restored()
                .expect("an unactivated device has no event queue to reset");
            assert!(!device.lock().pending_event_ack);
        }
    }
}
