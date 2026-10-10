// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/devices/virtio/test_utils.rs (trimmed:
// the event-manager `VirtioTestHelper` and `SomeplaceInMemory` machinery are
// dropped; what remains is a small `VirtQueue` that lays a desc table / avail
// ring / used ring out in guest memory and drives a [`Queue`] from host-only
// tests, plus a single-region guest memory factory).

#![cfg(test)]

use crate::devices::virtio::queue::{Descriptor, Queue, UsedElement};
use crate::vstate::memory::{Address, Bytes, GuestAddress, GuestMemoryMmap};

/// A single-region 64 KiB guest memory, enough for a few test virtqueues.
pub fn default_mem() -> GuestMemoryMmap {
    crate::vstate::memory::tests::single_region_mem(0x10000)
}

/// A guest-side virtqueue laid out in `mem` at `start`, for driving a
/// [`Queue`] from host-runnable tests without a hypervisor.
pub struct VirtQueue<'a> {
    mem: &'a GuestMemoryMmap,
    size: u16,
    dtable_start: GuestAddress,
    avail_start: GuestAddress,
    used_start: GuestAddress,
}

impl<'a> VirtQueue<'a> {
    /// Bytes of one descriptor-table entry.
    const DESC_SIZE: u64 = 16;

    pub fn new(start: GuestAddress, mem: &'a GuestMemoryMmap, size: u16) -> Self {
        assert!(size.is_power_of_two());
        let dtable_start = start;
        let avail_start = GuestAddress(start.0 + u64::from(size) * Self::DESC_SIZE);
        // avail ring: flags u16 + idx u16 + ring[size] u16 + used_event u16.
        let used_start_raw = avail_start.0 + 4 + u64::from(size) * 2;
        // Used ring needs 4-byte alignment.
        let used_start = GuestAddress((used_start_raw + 3) & !3u64);
        Self {
            mem,
            size,
            dtable_start,
            avail_start,
            used_start,
        }
    }

    /// Guest address of avail-ring `idx`.
    fn avail_idx_addr(&self) -> GuestAddress {
        self.avail_start.checked_add(2).unwrap()
    }

    fn write_desc(&self, index: u16, desc: Descriptor) {
        let addr = self
            .dtable_start
            .checked_add(u64::from(index) * Self::DESC_SIZE)
            .unwrap();
        self.mem.write_obj(desc, addr).unwrap();
    }

    /// Set descriptor `index` to `(addr, len, flags, next)`.
    pub fn set_desc(&self, index: u16, addr: u64, len: u32, flags: u16, next: u16) {
        self.write_desc(index, Descriptor { addr, len, flags, next });
    }

    pub fn desc_addr(&self, index: u16) -> u64 {
        self.mem
            .read_obj::<Descriptor>(
                self.dtable_start
                    .checked_add(u64::from(index) * Self::DESC_SIZE)
                    .unwrap(),
            )
            .unwrap()
            .addr
    }

    /// Publish descriptor `index` to the avail ring (append to its `idx`).
    pub fn publish(&self, index: u16) {
        let idx: u16 = self.mem.read_obj(self.avail_idx_addr()).unwrap();
        let ring_addr = self.avail_start.checked_add(4 + u64::from(idx) * 2).unwrap();
        self.mem.write_obj::<u16>(index, ring_addr).unwrap();
        self.mem.write_obj::<u16>(idx.wrapping_add(1), self.avail_idx_addr()).unwrap();
    }

    /// Set the avail-ring `idx` directly (used to simulate a malicious driver).
    pub fn set_avail_idx(&self, idx: u16) {
        self.mem.write_obj::<u16>(idx, self.avail_idx_addr()).unwrap();
    }

    /// Read the avail-ring `idx`.
    pub fn avail_idx(&self) -> u16 {
        self.mem.read_obj(self.avail_idx_addr()).unwrap()
    }

    /// Read the used-ring `idx`.
    pub fn used_idx(&self) -> u16 {
        self.mem.read_obj(self.used_start.checked_add(2).unwrap()).unwrap()
    }

    /// Read used-ring element `i` as `(id, len)`.
    pub fn used_elem(&self, i: u16) -> (u32, u32) {
        let addr = self.used_start.checked_add(4 + u64::from(i) * 8).unwrap();
        let e: UsedElement = self.mem.read_obj(addr).unwrap();
        (e.id, e.len)
    }

    /// A device-side [`Queue`] over this virtqueue, already initialized.
    pub fn create_queue(&self) -> Queue {
        let mut q = Queue::new(self.size);
        q.size = self.size;
        q.ready = true;
        q.desc_table_address = self.dtable_start;
        q.avail_ring_address = self.avail_start;
        q.used_ring_address = self.used_start;
        q.initialize(self.mem).unwrap();
        q
    }
}
