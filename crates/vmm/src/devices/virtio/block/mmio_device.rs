// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the MMIO transport drives a `Box<dyn VirtioDevice>`
// whose queues are `QueueConfig` (the register values a driver programmed),
// while the block device owns a real `Queue` ring. This adapter is the bridge
// — the same shape as `vsock::mmio_device`, minus its `Arc<Mutex<..>>`: the
// block device is only ever touched through the transport, which the MMIO bus
// already serializes, so it needs no lock of its own and no event thread (it
// serves its queue synchronously, on the vCPU thread that notifies it).

//! The virtio-mmio face of the block device.

use vmm_sys_util::eventfd::EventFd;

use super::device::VirtioBlock;
use super::{BLOCK_QUEUE_NUM, BLOCK_QUEUE_SIZE, VIRTIO_ID_BLOCK, VirtioBlockError};
use crate::devices::virtio::VirtioError;
use crate::devices::virtio::ack_feature_page;
use crate::devices::virtio::mmio::{QueueConfig, VirtioDevice};
use crate::vstate::memory::{GuestAddress, GuestMemoryMmap};

/// The virtio-mmio device that hands the block device to the transport.
///
/// Unlike the vsock device's adapter there is no shared state here: the device
/// processes its queue inside [`VirtioDevice::notify_queue`], which the
/// transport calls while it holds this adapter, so the transport's own
/// serialization is the device's.
#[derive(Debug)]
pub struct BlockMmioDevice {
    /// The device itself.
    device: VirtioBlock,
    /// The driver-visible queue (its size and ring addresses), programmed into
    /// the device on activation.
    queues: [QueueConfig; BLOCK_QUEUE_NUM],
}

impl BlockMmioDevice {
    /// Wraps `device` as the `VirtioDevice` the transport drives.
    pub fn new(device: VirtioBlock) -> Self {
        Self {
            device,
            queues: [QueueConfig::new(u32::from(BLOCK_QUEUE_SIZE)); BLOCK_QUEUE_NUM],
        }
    }

    /// The device behind the transport, for a caller that needs to inspect it
    /// (the builder logs the volume it opened).
    pub fn device(&self) -> &VirtioBlock {
        &self.device
    }
}

impl VirtioDevice for BlockMmioDevice {
    fn device_type(&self) -> u32 {
        VIRTIO_ID_BLOCK
    }

    fn features(&self) -> u64 {
        self.device.avail_features()
    }

    fn ack_features_by_page(&mut self, page: u32, value: u32) -> Result<(), VirtioError> {
        let acked = ack_feature_page(
            self.device.avail_features(),
            self.device.acked_features(),
            page,
            value,
        )?;
        self.device.set_acked_features(acked);
        Ok(())
    }

    fn queues(&self) -> &[QueueConfig] {
        &self.queues
    }

    fn queues_mut(&mut self) -> &mut [QueueConfig] {
        &mut self.queues
    }

    fn read_config(&mut self, offset: u64, data: &mut [u8]) {
        let config = self.device.config_as_bytes();
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        for (index, byte) in data.iter_mut().enumerate() {
            *byte = config
                .get(start.saturating_add(index))
                .copied()
                .unwrap_or(0);
        }
    }

    fn notify_queue(&mut self, index: u32) -> Result<(), VirtioError> {
        // A driver can write any number to `QueueNotify`; the device has one
        // queue, so a notification for another index is dropped (the register
        // write is guest-controlled and must not fault the VMM).
        if index != 0 {
            return Ok(());
        }
        self.device
            .process_queue()
            .map_err(VirtioBlockError::from)
            .map_err(VirtioError::from)
    }

    fn activate(&mut self, mem: &GuestMemoryMmap, irq: &EventFd) -> Result<(), VirtioError> {
        let queue = self.device.queue_mut();
        // `config.size` is already validated to be a non-zero power of two no
        // larger than `max_size` (256), so it fits in a `u16`.
        queue.size = self.queues[0].size as u16;
        queue.ready = self.queues[0].ready;
        queue.desc_table_address = GuestAddress(self.queues[0].desc);
        queue.avail_ring_address = GuestAddress(self.queues[0].avail);
        queue.used_ring_address = GuestAddress(self.queues[0].used);

        let irq = irq.try_clone().map_err(super::ActivateError::EventFd)?;
        self.device
            .activate(mem.clone(), irq)
            .map_err(VirtioError::from)
    }

    fn reset(&mut self) {
        self.device.reset();
        self.queues[0].reset();
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::FileExt;

    use vmm_sys_util::tempfile::TempFile;

    use super::*;
    use crate::devices::virtio::block::request::{RequestHeader, VIRTIO_BLK_S_OK, VIRTIO_BLK_T_IN};
    use crate::devices::virtio::block::test_utils::RequestDescriptorChain;
    use crate::devices::virtio::mmio::VirtioMmioTransport;
    use crate::devices::virtio::test_utils::{VirtQueue, default_mem};
    use crate::vstate::bus::BusDevice;

    // The virtio-mmio register offsets this test programs (virtio 1.0 §4.2.2).
    const MAGIC_VALUE: u64 = 0x00;
    const VERSION: u64 = 0x04;
    const DEVICE_ID: u64 = 0x08;
    const DEVICE_FEATURES: u64 = 0x10;
    const DEVICE_FEATURES_SEL: u64 = 0x14;
    const DRIVER_FEATURES: u64 = 0x20;
    const DRIVER_FEATURES_SEL: u64 = 0x24;
    const QUEUE_SEL: u64 = 0x30;
    const QUEUE_NUM_MAX: u64 = 0x34;
    const QUEUE_NUM: u64 = 0x38;
    const QUEUE_READY: u64 = 0x44;
    const QUEUE_NOTIFY: u64 = 0x50;
    const INTERRUPT_STATUS: u64 = 0x60;
    const STATUS: u64 = 0x70;
    const QUEUE_DESC_LOW: u64 = 0x80;
    const QUEUE_DESC_HIGH: u64 = 0x84;
    const QUEUE_DRIVER_LOW: u64 = 0x90;
    const QUEUE_DRIVER_HIGH: u64 = 0x94;
    const QUEUE_DEVICE_LOW: u64 = 0xa0;
    const QUEUE_DEVICE_HIGH: u64 = 0xa4;
    const CONFIG: u64 = 0x100;

    // The device status bits, as a driver sets them during initialization.
    const ACKNOWLEDGE: u32 = 1;
    const DRIVER: u32 = 2;
    const DRIVER_OK: u32 = 4;
    const FEATURES_OK: u32 = 8;

    /// The `VIRTIO_MMIO_INT_VRING` bit the transport reports.
    const INT_VRING: u32 = 0x01;

    fn write_u32(transport: &mut VirtioMmioTransport, offset: u64, value: u32) {
        let _ = transport.write(0, offset, &value.to_le_bytes());
    }

    fn read_u32(transport: &mut VirtioMmioTransport, offset: u64) -> u32 {
        let mut bytes = [0u8; 4];
        transport.read(0, offset, &mut bytes);
        u32::from_le_bytes(bytes)
    }

    fn read_u64(transport: &mut VirtioMmioTransport, offset: u64) -> u64 {
        let mut bytes = [0u8; 8];
        transport.read(0, offset, &mut bytes);
        u64::from_le_bytes(bytes)
    }

    /// The whole path a driver takes through the transport: the register
    /// protocol identifies the device and its capacity, brings it up (features,
    /// queue, status), and a request the driver published in the ring is served
    /// and acknowledged with an interrupt.
    ///
    /// This is the unit-test counterpart of the live boot: it exercises the
    /// transport's register decode, the adapter's queue programming and the
    /// device's request serving together, without a hypervisor.
    #[test]
    fn the_transport_serves_a_read_request_through_the_block_device() {
        let payload: Vec<u8> = (0..512u32).map(|byte| (byte % 241) as u8).collect();
        let file = TempFile::new().expect("a temporary file");
        file.as_file().set_len(4096).expect("a sized volume");
        file.as_file()
            .write_all_at(&payload, 0)
            .expect("the volume's contents");

        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        let chain = RequestDescriptorChain::new(&vq);
        chain.set_header(RequestHeader::new(VIRTIO_BLK_T_IN, 0));
        chain.set_data_len(512);
        chain.publish();

        let device = VirtioBlock::new(file.as_path()).expect("a block device");
        let irq = EventFd::new(libc::EFD_NONBLOCK).expect("an interrupt event fd");
        let irq_probe = irq.try_clone().expect("a probe of the interrupt event fd");
        let mut transport =
            VirtioMmioTransport::new(Box::new(BlockMmioDevice::new(device)), mem.clone(), irq);

        // The transport identifies itself as a virtio 1.0 block device.
        assert_eq!(read_u32(&mut transport, MAGIC_VALUE), 0x7472_6976);
        assert_eq!(read_u32(&mut transport, VERSION), 2);
        assert_eq!(read_u32(&mut transport, DEVICE_ID), VIRTIO_ID_BLOCK);

        // The driver acknowledges the device, then reads the offered features.
        write_u32(&mut transport, STATUS, ACKNOWLEDGE);
        write_u32(&mut transport, STATUS, ACKNOWLEDGE | DRIVER);
        let offered_low = read_u32(&mut transport, DEVICE_FEATURES);
        write_u32(&mut transport, DEVICE_FEATURES_SEL, 1);
        let offered_high = read_u32(&mut transport, DEVICE_FEATURES);
        write_u32(&mut transport, DRIVER_FEATURES_SEL, 0);
        write_u32(&mut transport, DRIVER_FEATURES, offered_low);
        write_u32(&mut transport, DRIVER_FEATURES_SEL, 1);
        write_u32(&mut transport, DRIVER_FEATURES, offered_high);
        write_u32(&mut transport, STATUS, ACKNOWLEDGE | DRIVER | FEATURES_OK);

        // The configuration space reports the volume's capacity: 4096 / 512.
        assert_eq!(read_u64(&mut transport, CONFIG), 8);

        // Program queue 0 with the rings the driver laid out.
        write_u32(&mut transport, QUEUE_SEL, 0);
        assert!(
            read_u32(&mut transport, QUEUE_NUM_MAX) >= 16,
            "the device offers room for the driver's ring"
        );
        write_u32(&mut transport, QUEUE_NUM, 16);
        let desc = vq.desc_table_addr().0;
        let avail = vq.avail_ring_addr().0;
        let used = vq.used_ring_addr().0;
        write_u32(&mut transport, QUEUE_DESC_LOW, desc as u32);
        write_u32(&mut transport, QUEUE_DESC_HIGH, (desc >> 32) as u32);
        write_u32(&mut transport, QUEUE_DRIVER_LOW, avail as u32);
        write_u32(&mut transport, QUEUE_DRIVER_HIGH, (avail >> 32) as u32);
        write_u32(&mut transport, QUEUE_DEVICE_LOW, used as u32);
        write_u32(&mut transport, QUEUE_DEVICE_HIGH, (used >> 32) as u32);
        write_u32(&mut transport, QUEUE_READY, 1);

        // DRIVER_OK activates the device against the guest memory.
        write_u32(
            &mut transport,
            STATUS,
            ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK,
        );

        // The driver notifies the queue; the device serves the request.
        write_u32(&mut transport, QUEUE_NOTIFY, 0);

        assert_eq!(chain.status(), VIRTIO_BLK_S_OK);
        assert_eq!(chain.read_data(512), payload);
        assert_eq!(vq.used_idx(), 1);
        assert_eq!(vq.used_elem(0), (0, 512 + 1));
        assert_eq!(
            read_u32(&mut transport, INTERRUPT_STATUS),
            INT_VRING,
            "the transport tells the driver a used ring has entries"
        );
        assert_eq!(
            irq_probe.read().expect("the interrupt was raised"),
            1,
            "the device raised its interrupt through the transport's irq fd"
        );
    }

    /// A notification for a queue the device does not have is dropped, and a
    /// reset returns the device (and the transport's queue registers) to their
    /// pre-activation state.
    #[test]
    fn a_reset_returns_the_device_to_its_initial_state() {
        let file = TempFile::new().expect("a temporary file");
        file.as_file().set_len(4096).expect("a sized volume");
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        let device = VirtioBlock::new(file.as_path()).expect("a block device");
        let irq = EventFd::new(libc::EFD_NONBLOCK).expect("an interrupt event fd");
        let mut transport =
            VirtioMmioTransport::new(Box::new(BlockMmioDevice::new(device)), mem.clone(), irq);

        // A notification before activation is dropped without touching a ring.
        write_u32(&mut transport, QUEUE_NOTIFY, 0);
        // And so is a notification for a queue that does not exist.
        write_u32(&mut transport, QUEUE_NOTIFY, 7);

        // Bring the device up, then reset it with a status write of 0.
        write_u32(&mut transport, STATUS, ACKNOWLEDGE);
        write_u32(&mut transport, STATUS, ACKNOWLEDGE | DRIVER);
        let offered = read_u32(&mut transport, DEVICE_FEATURES);
        write_u32(&mut transport, DRIVER_FEATURES, offered);
        write_u32(&mut transport, STATUS, ACKNOWLEDGE | DRIVER | FEATURES_OK);
        write_u32(&mut transport, QUEUE_SEL, 0);
        write_u32(&mut transport, QUEUE_NUM, 16);
        write_u32(
            &mut transport,
            QUEUE_DESC_LOW,
            vq.desc_table_addr().0 as u32,
        );
        write_u32(
            &mut transport,
            QUEUE_DRIVER_LOW,
            vq.avail_ring_addr().0 as u32,
        );
        write_u32(
            &mut transport,
            QUEUE_DEVICE_LOW,
            vq.used_ring_addr().0 as u32,
        );
        write_u32(&mut transport, QUEUE_READY, 1);
        write_u32(
            &mut transport,
            STATUS,
            ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK,
        );

        write_u32(&mut transport, QUEUE_SEL, 0);
        assert_eq!(read_u32(&mut transport, QUEUE_READY), 1);
        write_u32(&mut transport, STATUS, 0);
        assert_eq!(read_u32(&mut transport, STATUS), 0, "the status is reset");
        assert_eq!(
            read_u32(&mut transport, QUEUE_READY),
            0,
            "the driver's queue configuration is dropped"
        );
    }
}
