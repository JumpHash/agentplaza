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
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::pal::eventfd::EventFd;

use super::device::Vsock;
use super::VsockBackend;
use crate::devices::virtio::mmio::{QueueConfig, VirtioDevice};
use crate::devices::virtio::ack_feature_page;
use crate::devices::virtio::VirtioError;
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
        self.device
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
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
        let acked = ack_feature_page(device.avail_features(), device.acked_features(), page, value)?;
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
            *byte = config.get(start.saturating_add(index)).copied().unwrap_or(0);
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
        let irq = irq
            .try_clone()
            .map_err(super::ActivateError::EventFd)?;
        device.activate(mem.clone(), irq).map_err(VirtioError::from)
    }

    fn reset(&mut self) {
        self.lock().reset();
        for queue in &mut self.queues {
            queue.reset();
        }
    }
}
