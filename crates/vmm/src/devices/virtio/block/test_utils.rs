// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/devices/virtio/block/virtio/test_utils.rs
// (upstream commit fd9fc7a9f164b73002d22065d17c150d8c262d91): the
// `RequestDescriptorChain` helper and `read_blk_req_descriptors` layout that
// let a host-only test drive a real request chain into the device. Adapted
// because this port's `VirtQueue` exposes `set_desc` rather than upstream's
// per-descriptor cells: the chain keeps its own copy of the three descriptors
// (in `Cell`s, so a test can corrupt a field exactly as upstream's does) and
// publishes them into the queue's descriptor table before the device reads
// them, with descriptor 0 as the only available chain head — exactly the
// layout upstream's `read_blk_req_descriptors` produces.

use std::cell::Cell;

use zerocopy::IntoBytes;

use crate::devices::virtio::block::request::RequestHeader;
use crate::devices::virtio::queue::{Queue, VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE};
use crate::devices::virtio::test_utils::VirtQueue;
use crate::vstate::memory::{Bytes, GuestAddress, GuestMemoryMmap};

/// Where the chain's three descriptors point, as upstream's helper lays them
/// out: the header at 0x1000, the data at 0x2000, the status at 0x3000, each
/// with a 0x1000-byte buffer.
const HEADER_ADDR: u64 = 0x1000;
const DATA_ADDR: u64 = 0x2000;
const STATUS_ADDR: u64 = 0x3000;
const DESC_LEN: u32 = 0x1000;

const HEADER_DESC: u16 = 0;
const DATA_DESC: u16 = 1;
const STATUS_DESC: u16 = 2;

/// One descriptor of a test request chain, as the driver published it.
#[derive(Debug, Default)]
struct Desc {
    addr: Cell<u64>,
    len: Cell<u32>,
    flags: Cell<u16>,
    next: Cell<u16>,
}

impl Desc {
    fn new(addr: u64, flags: u16, next: u16) -> Self {
        Self {
            addr: Cell::new(addr),
            len: Cell::new(DESC_LEN),
            flags: Cell::new(flags),
            next: Cell::new(next),
        }
    }
}

/// The three descriptors of one block request, in the [`VirtQueue`] the device
/// under test reads from.
///
/// The header descriptor is read-only (the driver wrote the header there), the
/// data descriptor starts out write-only (a read request; a write request clears
/// `VIRTQ_DESC_F_WRITE`), and the status descriptor is write-only and one byte
/// long, as the protocol requires.
pub struct RequestDescriptorChain<'a, 'b> {
    driver_queue: &'b VirtQueue<'a>,
    header: Desc,
    data: Desc,
    status: Desc,
}

impl<'a, 'b> RequestDescriptorChain<'a, 'b> {
    /// Lays a three-descriptor request chain out in `driver_queue`, with the
    /// header descriptor as the head of the available ring.
    pub fn new(driver_queue: &'b VirtQueue<'a>) -> Self {
        Self {
            driver_queue,
            header: Desc::new(HEADER_ADDR, VIRTQ_DESC_F_NEXT, DATA_DESC),
            data: Desc::new(DATA_ADDR, VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE, STATUS_DESC),
            status: Desc::new(STATUS_ADDR, VIRTQ_DESC_F_WRITE, 0),
        }
    }

    /// The guest memory the chain lives in.
    pub fn memory(&self) -> &'a GuestMemoryMmap {
        self.driver_queue.memory()
    }

    /// A device-side [`Queue`] over this chain, with the descriptors published
    /// and the header descriptor available.
    pub fn create_queue(&self) -> Queue {
        self.publish();
        self.driver_queue.create_queue()
    }

    /// Publishes the chain's descriptors into the queue and makes the header
    /// descriptor the only available chain head.
    ///
    /// Idempotent: a test that changes a field calls this again before the
    /// device reads the chain.
    pub fn publish(&self) {
        self.driver_queue.set_desc(
            HEADER_DESC,
            self.header.addr.get(),
            self.header.len.get(),
            self.header.flags.get(),
            self.header.next.get(),
        );
        self.driver_queue.set_desc(
            DATA_DESC,
            self.data.addr.get(),
            self.data.len.get(),
            self.data.flags.get(),
            self.data.next.get(),
        );
        self.driver_queue.set_desc(
            STATUS_DESC,
            self.status.addr.get(),
            self.status.len.get(),
            self.status.flags.get(),
            self.status.next.get(),
        );
        // Only descriptor 0 is available, as upstream's helper leaves the ring.
        self.driver_queue.set_avail_idx(0);
        self.driver_queue.publish(HEADER_DESC);
    }

    /// The header the driver wrote into the header descriptor's buffer.
    pub fn header(&self) -> RequestHeader {
        let mut bytes = [0u8; std::mem::size_of::<RequestHeader>()];
        self.memory()
            .read_slice(&mut bytes, GuestAddress(self.header.addr.get()))
            .unwrap();
        zerocopy::FromBytes::read_from_bytes(&bytes).unwrap()
    }

    /// Writes `header` into the header descriptor's buffer.
    pub fn set_header(&self, header: RequestHeader) {
        self.memory()
            .write_slice(header.as_bytes(), GuestAddress(self.header.addr.get()))
            .unwrap();
    }

    /// Writes `bytes` into the data descriptor's buffer.
    pub fn write_data(&self, bytes: &[u8]) {
        self.memory()
            .write_slice(bytes, GuestAddress(self.data.addr.get()))
            .unwrap();
    }

    /// Reads the data descriptor's buffer.
    pub fn read_data(&self, len: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; len];
        self.memory()
            .read_slice(&mut bytes, GuestAddress(self.data.addr.get()))
            .unwrap();
        bytes
    }

    /// The status byte the device wrote back.
    pub fn status(&self) -> u8 {
        self.memory()
            .read_obj(GuestAddress(self.status.addr.get()))
            .unwrap()
    }

    pub fn data_desc_addr(&self) -> GuestAddress {
        GuestAddress(self.data.addr.get())
    }

    pub fn data_desc_len(&self) -> u32 {
        self.data.len.get()
    }

    pub fn status_desc_addr(&self) -> GuestAddress {
        GuestAddress(self.status.addr.get())
    }

    pub fn set_header_flags(&self, flags: u16) {
        self.header.flags.set(flags);
    }

    pub fn set_header_next(&self, next: u16) {
        self.header.next.set(next);
    }

    pub fn set_data_flags(&self, flags: u16) {
        self.data.flags.set(flags);
    }

    pub fn set_data_addr(&self, addr: u64) {
        self.data.addr.set(addr);
    }

    pub fn set_data_len(&self, len: u32) {
        self.data.len.set(len);
    }

    pub fn set_status_flags(&self, flags: u16) {
        self.status.flags.set(flags);
    }

    pub fn set_status_addr(&self, addr: u64) {
        self.status.addr.set(addr);
    }

    pub fn set_status_len(&self, len: u32) {
        self.status.len.set(len);
    }
}
