// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/devices/virtio/queue.rs (trimmed: the
// kani proofs are dropped; `GuestMemorySlice` is folded in locally instead of
// living in `vstate::memory`; `crate::logger::error`/`crate::utils` become a
// plain `eprintln!` and `vstate::memory::u64_to_usize`).

use std::num::Wrapping;
use std::sync::atomic::{Ordering, fence};

use vm_memory::bitmap::Bitmap;
use vm_memory::{GuestMemoryBackend, GuestMemoryError};

use crate::vstate::memory::{u64_to_usize, ByteValued, GuestAddress};

/// A resolved guest-memory range with volatile accesses bounded by its original length.
///
/// Reusing the resolved pointer avoids a region lookup on each access. The slice does not keep
/// its mapping alive; devices retain it until reset, and the mapping outlives it by design.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GuestMemorySlice {
    base: *mut u8,
    len: usize,
}

// SAFETY: `base` points into guest memory, which is shared by design and is only reached through
// the bounds-checked volatile accessors below. A slice owns nothing and holds no thread-affine
// state, so moving one to a device's worker thread is sound.
unsafe impl Send for GuestMemorySlice {}

impl GuestMemorySlice {
    /// An unresolved range used before queue initialization.
    pub(crate) const UNRESOLVED: Self = Self {
        base: std::ptr::null_mut(),
        len: 0,
    };

    /// Resolves `addr..addr + len`, which must lie within a single region.
    pub(crate) fn new<M: GuestMemoryBackend>(
        mem: &M,
        addr: GuestAddress,
        len: usize,
    ) -> Result<Self, GuestMemoryError> {
        let slice = mem.get_slice(addr, len)?;
        // This port's guest memory has no dirty-page bitmap (`B = ()`), so this is a no-op.
        slice.bitmap().mark_dirty(0, len);
        Ok(Self {
            base: slice.ptr_guard_mut().as_ptr(),
            len,
        })
    }

    #[inline(always)]
    fn ptr_at<T>(&self, offset: usize) -> Option<*mut T> {
        if offset
            .checked_add(std::mem::size_of::<T>())
            .is_none_or(|end| end > self.len)
        {
            return None;
        }
        // SAFETY: callers keep the mapping alive. The check above keeps
        // `[offset, offset + size_of::<T>())` within the range `new` validated.
        Some(unsafe { self.base.add(offset) }.cast())
    }

    /// Reads the `T` at `offset` bytes into the range, or `None` if it does not fit.
    ///
    /// # Safety
    ///
    /// In-bounds addresses must be aligned for `T` and valid for reads.
    #[inline(always)]
    pub(crate) unsafe fn read_obj<T: ByteValued>(&self, offset: usize) -> Option<T> {
        let ptr = self.ptr_at::<T>(offset)?;
        // SAFETY: `ptr_at` bounds the access. The caller guarantees alignment and a live mapping.
        Some(unsafe { ptr.read_volatile() })
    }

    /// Reads the final `T`, or `None` if the range is too short.
    ///
    /// # Safety
    ///
    /// The final `T`, if it fits, must be aligned and valid for reads.
    #[inline(always)]
    pub(crate) unsafe fn read_last<T: ByteValued>(&self) -> Option<T> {
        // SAFETY: the caller upholds `read_obj`'s requirements at the tail offset.
        unsafe { self.read_obj(self.last_offset::<T>()?) }
    }

    /// Writes `val` at `offset` bytes into the range, or returns `None` if it does not fit.
    ///
    /// # Safety
    ///
    /// In-bounds addresses must be aligned for `T` and valid for writes.
    #[inline(always)]
    #[must_use]
    pub(crate) unsafe fn write_obj<T: ByteValued>(&mut self, val: T, offset: usize) -> Option<()> {
        let ptr = self.ptr_at::<T>(offset)?;
        // SAFETY: as in `read_obj`.
        unsafe { ptr.write_volatile(val) };
        Some(())
    }

    /// Writes `val` into the final `T`-sized slot, or returns `None` if it does not fit.
    ///
    /// # Safety
    ///
    /// The final `T`, if it fits, must be aligned and valid for writes.
    #[inline(always)]
    #[must_use]
    pub(crate) unsafe fn write_last<T: ByteValued>(&mut self, val: T) -> Option<()> {
        // SAFETY: the caller upholds `write_obj`'s requirements at the tail offset.
        unsafe { self.write_obj(val, self.last_offset::<T>()?) }
    }

    #[inline(always)]
    fn last_offset<T>(&self) -> Option<usize> {
        self.len.checked_sub(std::mem::size_of::<T>())
    }
}

pub const VIRTQ_DESC_F_NEXT: u16 = 0x1;
pub const VIRTQ_DESC_F_WRITE: u16 = 0x2;

/// Byte size of the `flags` and `idx` header both the available and the used ring begin with,
/// i.e. the offset of their first ring element.
const RING_HEADER_SIZE: usize = 2 * std::mem::size_of::<u16>();

/// Max size of virtio queues offered by firecracker's virtio devices.
pub(super) const FIRECRACKER_MAX_QUEUE_SIZE: u16 = 256;

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    /// Descriptor index out of bounds: {0}.
    #[error("descriptor index out of bounds: {0}")]
    DescIndexOutOfBounds(u16),
    /// Failed to write value into the virtio queue used ring: {0}.
    #[error("failed to write value into the virtio queue used ring: {0}")]
    MemoryError(#[from] vm_memory::GuestMemoryError),
    /// Pointer is not aligned properly: {0:#x} not {1}-byte aligned.
    #[error("pointer is not aligned properly: {0:#x} not {1}-byte aligned")]
    PointerNotAligned(usize, usize),
    /// Attempt to use virtio queue that is not marked ready.
    #[error("attempt to use virtio queue that is not marked ready")]
    NotReady,
    /// Virtio queue with invalid size: {0}.
    #[error("virtio queue with invalid size: {0}")]
    InvalidSize(u16),
}

/// Error type indicating the guest configured a virtio queue such that the avail_idx field would
/// indicate there are more descriptors to process than the queue actually has space for.
///
/// Should this error bubble up to the event loop, we exit Firecracker, since this could be a
/// potential malicious driver scenario. This way we also eliminate the risk of repeatedly
/// logging and potentially clogging the microVM through the log system.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "The number of available virtio descriptors {reported_len} is greater than queue size: \
     {queue_size}!"
)]
pub struct InvalidAvailIdx {
    queue_size: u16,
    reported_len: u16,
}

/// A virtio descriptor constraints with C representative.
/// Taken from Virtio spec:
/// https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-430008
/// 2.6.5 The Virtqueue Descriptor Table
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Descriptor {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

// SAFETY: `Descriptor` is a POD and contains no padding.
unsafe impl ByteValued for Descriptor {}

/// A virtio used element in the used ring.
/// Taken from Virtio spec:
/// https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-430008
/// 2.6.8 The Virtqueue Used Ring
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct UsedElement {
    pub id: u32,
    pub len: u32,
}

// SAFETY: `UsedElement` is a POD and contains no padding.
unsafe impl ByteValued for UsedElement {}

/// A virtio descriptor chain.
#[derive(Debug, Copy, Clone)]
pub struct DescriptorChain {
    desc_table: GuestMemorySlice,

    queue_size: u16,
    ttl: u16, // used to prevent infinite chain cycles

    /// Index into the descriptor table
    pub index: u16,

    /// Guest physical address of device specific data
    pub addr: GuestAddress,

    /// Length of device specific data
    pub len: u32,

    /// Includes next, write, and indirect bits
    pub flags: u16,

    /// Index into the descriptor table of the next descriptor if flags has
    /// the next bit set
    pub next: u16,
}

impl DescriptorChain {
    /// Creates a new `DescriptorChain` from the given descriptor table.
    ///
    /// Note that the desc_table and queue_size are assumed to be validated by the caller.
    fn checked_new(desc_table: GuestMemorySlice, queue_size: u16, index: u16) -> Option<Self> {
        if queue_size <= index {
            return None;
        }

        // SAFETY: initialization aligns the table, and descriptor-sized offsets preserve
        // alignment. The caller keeps the mapping alive.
        let desc = unsafe {
            desc_table
                .read_obj::<Descriptor>(usize::from(index) * std::mem::size_of::<Descriptor>())
        }
        .expect("virtio ring access outside the range validated at activation");
        let chain = DescriptorChain {
            desc_table,
            queue_size,
            ttl: queue_size,
            index,
            addr: GuestAddress(desc.addr),
            len: desc.len,
            flags: desc.flags,
            next: desc.next,
        };

        if chain.is_valid() { Some(chain) } else { None }
    }

    fn is_valid(&self) -> bool {
        !self.has_next() || self.next < self.queue_size
    }

    /// Gets if this descriptor chain has another descriptor chain linked after it.
    pub fn has_next(&self) -> bool {
        self.flags & VIRTQ_DESC_F_NEXT != 0 && self.ttl > 1
    }

    /// If the driver designated this as a write only descriptor.
    ///
    /// If this is false, this descriptor is read only.
    /// Write only means the emulated device can write and the driver can read.
    pub fn is_write_only(&self) -> bool {
        self.flags & VIRTQ_DESC_F_WRITE != 0
    }

    /// Gets the next descriptor in this descriptor chain, if there is one.
    ///
    /// Note that this is distinct from the next descriptor chain returned by [`Queue::pop`], which
    /// is the head of the next _available_ descriptor chain.
    #[inline(always)]
    pub fn next_descriptor(&self) -> Option<Self> {
        if self.has_next() {
            DescriptorChain::checked_new(self.desc_table, self.queue_size, self.next).map(
                |mut c| {
                    c.ttl = self.ttl - 1;
                    c
                },
            )
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub struct DescriptorIterator(Option<DescriptorChain>);

impl IntoIterator for DescriptorChain {
    type Item = DescriptorChain;
    type IntoIter = DescriptorIterator;

    fn into_iter(self) -> Self::IntoIter {
        DescriptorIterator(Some(self))
    }
}

impl Iterator for DescriptorIterator {
    type Item = DescriptorChain;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.take().inspect(|desc| {
            self.0 = desc.next_descriptor();
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// A virtio queue's parameters.
pub struct Queue {
    /// The maximal size in elements offered by the device
    pub max_size: u16,

    /// The queue size in elements the driver selected
    pub size: u16,

    /// Indicates if the queue is finished with configuration
    pub ready: bool,

    /// Guest physical address of the descriptor table
    pub desc_table_address: GuestAddress,

    /// Guest physical address of the available ring
    pub avail_ring_address: GuestAddress,

    /// Guest physical address of the used ring
    pub used_ring_address: GuestAddress,

    /// Descriptor table in guest memory, resolved by `initialize`.
    /// struct DescriptorTable = [Descriptor; <queue_size>]
    pub(crate) desc_table: GuestMemorySlice,

    /// Available ring in guest memory, resolved by `initialize`.
    /// struct AvailRing {
    ///     flags: u16,
    ///     idx: u16,
    ///     ring: [u16; <queue size>],
    ///     used_event: u16,
    /// }
    pub(crate) avail_ring: GuestMemorySlice,

    /// Used ring in guest memory, resolved by `initialize`.
    /// struct UsedRing {
    ///     flags: u16,
    ///     idx: u16,
    ///     ring: [UsedElement; <queue size>],
    ///     avail_event: u16,
    /// }
    pub(crate) used_ring: GuestMemorySlice,

    pub next_avail: Wrapping<u16>,
    pub next_used: Wrapping<u16>,

    /// VIRTIO_F_RING_EVENT_IDX negotiated (notification suppression enabled)
    pub uses_notif_suppression: bool,
    /// The number of added used buffers since last guest kick
    pub num_added: Wrapping<u16>,
}

#[allow(clippy::len_without_is_empty)]
impl Queue {
    /// Constructs an empty virtio queue with the given `max_size`.
    pub fn new(max_size: u16) -> Queue {
        Queue {
            max_size,
            size: max_size,
            ready: false,
            desc_table_address: GuestAddress(0),
            avail_ring_address: GuestAddress(0),
            used_ring_address: GuestAddress(0),

            desc_table: GuestMemorySlice::UNRESOLVED,
            avail_ring: GuestMemorySlice::UNRESOLVED,
            used_ring: GuestMemorySlice::UNRESOLVED,

            next_avail: Wrapping(0),
            next_used: Wrapping(0),
            uses_notif_suppression: false,
            num_added: Wrapping(0),
        }
    }

    fn desc_table_size(&self) -> usize {
        std::mem::size_of::<Descriptor>() * usize::from(self.size)
    }

    fn avail_ring_size(&self) -> usize {
        std::mem::size_of::<u16>()
            + std::mem::size_of::<u16>()
            + std::mem::size_of::<u16>() * usize::from(self.size)
            + std::mem::size_of::<u16>()
    }

    fn used_ring_size(&self) -> usize {
        std::mem::size_of::<u16>()
            + std::mem::size_of::<u16>()
            + std::mem::size_of::<UsedElement>() * usize::from(self.size)
            + std::mem::size_of::<u16>()
    }

    /// Position of a ring index within the ring.
    ///
    /// `initialize` requires a power-of-two `size`, so this is `index % size` without the divide.
    /// The mask is `usize` to keep the load of `index` 16 bits wide. A wider load overlaps the
    /// store that advanced it and loses store forwarding.
    #[inline(always)]
    fn ring_index(&self, index: u16) -> usize {
        usize::from(index) & (usize::from(self.size) - 1)
    }

    fn check_alignment(addr: GuestAddress, alignment: u64) -> Result<(), QueueError> {
        // Guest memory base address is page aligned, so as long as alignment divides page size,
        // it suffices to check that the GPA is properly aligned (e.g. we don't need to recheck
        // the HVA).
        if addr.0 & (alignment - 1) != 0 {
            return Err(QueueError::PointerNotAligned(
                u64_to_usize(addr.0),
                u64_to_usize(alignment),
            ));
        }
        Ok(())
    }

    /// Resolve the queue objects in the guest memory and mark them dirty.
    pub fn initialize<M: GuestMemoryBackend>(&mut self, mem: &M) -> Result<(), QueueError> {
        if !self.ready {
            return Err(QueueError::NotReady);
        }

        if self.size > self.max_size || self.size == 0 || (self.size & (self.size - 1)) != 0 {
            return Err(QueueError::InvalidSize(self.size));
        }

        // The ring accessors use aligned volatile accesses, so the addresses below must be
        // aligned. A broken or fuzzed snapshot can restore a misaligned one.
        //
        // Specification of the alignments:
        // https://docs.oasis-open.org/virtio/virtio/v1.2/csd01/virtio-v1.2-csd01.html#x1-350007
        // > ================ ==========
        // > Virtqueue Part    Alignment
        // > ================ ==========
        // > Descriptor Table 16
        // > Available Ring   2
        // > Used Ring        4
        // > ================ ==========
        Self::check_alignment(self.desc_table_address, 16)?;
        Self::check_alignment(self.avail_ring_address, 2)?;
        Self::check_alignment(self.used_ring_address, 4)?;

        self.desc_table =
            GuestMemorySlice::new(mem, self.desc_table_address, self.desc_table_size())?;
        self.avail_ring =
            GuestMemorySlice::new(mem, self.avail_ring_address, self.avail_ring_size())?;
        self.used_ring = GuestMemorySlice::new(mem, self.used_ring_address, self.used_ring_size())?;

        Ok(())
    }

    // The transports prevent active queue reconfiguration. An out-of-range access indicates
    // invalid device state, so abort as for `InvalidAvailIdx`.

    /// Get AvailRing.idx
    #[inline(always)]
    pub fn avail_ring_idx_get(&self) -> u16 {
        // SAFETY: initialization aligns the ring for u16, the offset is even, and the device
        // keeps the mapping alive.
        unsafe { self.avail_ring.read_obj(std::mem::size_of::<u16>()) }
            .expect("virtio ring access outside the range validated at activation")
    }

    /// Get element from AvailRing.ring at index
    #[inline(always)]
    fn avail_ring_ring_get(&self, index: usize) -> u16 {
        // SAFETY: the ring base, header and stride are u16-aligned, and the device keeps
        // the mapping alive.
        unsafe {
            self.avail_ring
                .read_obj(RING_HEADER_SIZE + std::mem::size_of::<u16>() * index)
        }
        .expect("virtio ring access outside the range validated at activation")
    }

    /// Get AvailRing.used_event, the last `u16` of the ring.
    #[inline(always)]
    pub fn avail_ring_used_event_get(&self) -> u16 {
        // SAFETY: the ring base and retained length are u16-aligned, and the device keeps
        // the mapping alive.
        unsafe { self.avail_ring.read_last() }
            .expect("virtio ring access outside the range validated at activation")
    }

    /// Set UsedRing.idx
    #[inline(always)]
    pub fn used_ring_idx_set(&mut self, val: u16) {
        // SAFETY: initialization aligns the ring for u16, the offset is even, and the device
        // keeps the mapping alive.
        unsafe { self.used_ring.write_obj(val, std::mem::size_of::<u16>()) }
            .expect("virtio ring access outside the range validated at activation")
    }

    /// Set element in UsedRing.ring at index
    #[inline(always)]
    fn used_ring_ring_set(&mut self, index: usize, val: UsedElement) {
        // SAFETY: initialization aligns the ring for UsedElement. The header and stride
        // preserve alignment, and the device keeps the mapping alive.
        unsafe {
            self.used_ring.write_obj(
                val,
                RING_HEADER_SIZE + std::mem::size_of::<UsedElement>() * index,
            )
        }
        .expect("virtio ring access outside the range validated at activation")
    }

    /// Get UsedRing.avail_event, the last `u16` of the ring.
    #[cfg(test)]
    #[inline(always)]
    pub fn used_ring_avail_event_get(&mut self) -> u16 {
        // SAFETY: the ring base and retained length are u16-aligned, and the device keeps
        // the mapping alive.
        unsafe { self.used_ring.read_last() }
            .expect("virtio ring access outside the range validated at activation")
    }

    /// Set UsedRing.avail_event, the last `u16` of the ring.
    #[inline(always)]
    pub fn used_ring_avail_event_set(&mut self, val: u16) {
        // SAFETY: the ring base and retained length are u16-aligned, and the device keeps
        // the mapping alive.
        unsafe { self.used_ring.write_last(val) }
            .expect("virtio ring access outside the range validated at activation")
    }

    /// Returns the number of yet-to-be-popped descriptor chains in the avail ring.
    pub fn len(&self) -> u16 {
        (Wrapping(self.avail_ring_idx_get()) - self.next_avail).0
    }

    /// Checks if the driver has made any descriptor chains available in the avail ring.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Pop the first available descriptor chain from the avail ring.
    ///
    /// If this function returns an error at runtime, then the guest has requested Firecracker
    /// to process more virtio descriptors than there can possibly be given the queue's size.
    /// This can be a malicious guest driver scenario, and hence a DoS attempt. If encountered
    /// and runtime, correct handling is to panic!
    ///
    /// This function however is also called on paths that can (and should) just report
    /// the error to the user (e.g. loading a corrupt snapshot file), and hence cannot panic on its
    /// own.
    pub fn pop(&mut self) -> Result<Option<DescriptorChain>, InvalidAvailIdx> {
        let len = self.len();
        // The number of descriptor chain heads to process should always
        // be smaller or equal to the queue size, as the driver should
        // never ask the VMM to process a available ring entry more than
        // once. Checking and reporting such incorrect driver behavior
        // can prevent potential hanging and Denial-of-Service from
        // happening on the VMM side.
        if self.size < len {
            return Err(InvalidAvailIdx {
                queue_size: self.size,
                reported_len: len,
            });
        }

        if len == 0 {
            return Ok(None);
        }

        Ok(self.pop_unchecked())
    }

    /// Try to pop the first available descriptor chain from the avail ring.
    /// If no descriptor is available, enable notifications.
    ///
    /// If this function returns an error at runtime, then the guest has requested Firecracker
    /// to process more virtio descriptors than there can possibly be given the queue's size.
    /// This can be a malicious guest driver scenario, and hence a DoS attempt. If encountered
    /// and runtime, correct handling is to panic!
    ///
    /// This function however is also called on paths that can (and should) just report
    /// the error to the user (e.g. loading a corrupt snapshot file), and hence cannot panic on its
    /// own.
    pub fn pop_or_enable_notification(
        &mut self,
    ) -> Result<Option<DescriptorChain>, InvalidAvailIdx> {
        if !self.uses_notif_suppression {
            return self.pop();
        }

        if self.try_enable_notification()? {
            return Ok(None);
        }

        Ok(self.pop_unchecked())
    }

    /// Pop the first available descriptor chain from the avail ring.
    ///
    /// # Important
    /// This is an internal method that ASSUMES THAT THERE ARE AVAILABLE DESCRIPTORS. Otherwise it
    /// will retrieve a descriptor that contains garbage data (obsolete/empty).
    fn pop_unchecked(&mut self) -> Option<DescriptorChain> {
        // This fence ensures all subsequent reads see the updated driver writes.
        fence(Ordering::Acquire);

        // We'll need to find the first available descriptor, that we haven't yet popped.
        // In a naive notation, that would be:
        // `descriptor_table[avail_ring[next_avail]]`.
        //
        // We use `self.next_avail` to store the position, in `ring`, of the next available
        // descriptor index, with a twist: we always only increment `self.next_avail`, so the
        // actual position will be `self.next_avail % self.size`.
        let desc_index = self.avail_ring_ring_get(self.ring_index(self.next_avail.0));

        DescriptorChain::checked_new(self.desc_table, self.size, desc_index).inspect(|_| {
            self.next_avail += Wrapping(1);
        })
    }

    /// Undo the effects of the last `self.pop()` call.
    /// The caller can use this, if it was unable to consume the last popped descriptor chain.
    pub fn undo_pop(&mut self) {
        self.next_avail -= Wrapping(1);
    }

    /// Write used element into used_ring ring.
    /// - [`ring_index_offset`] is an offset added to the current [`self.next_used`] to obtain
    ///   actual index into used_ring.
    pub fn write_used_element(
        &mut self,
        ring_index_offset: u16,
        desc_index: u16,
        len: u32,
    ) -> Result<(), QueueError> {
        if self.size <= desc_index {
            eprintln!(
                "error: attempted to add out of bounds descriptor to used ring: {}",
                desc_index
            );
            return Err(QueueError::DescIndexOutOfBounds(desc_index));
        }

        let next_used = self.ring_index((self.next_used + Wrapping(ring_index_offset)).0);
        let used_element = UsedElement {
            id: u32::from(desc_index),
            len,
        };
        self.used_ring_ring_set(next_used, used_element);
        Ok(())
    }

    /// Advance queue and used ring by `n` elements.
    pub fn advance_next_used(&mut self, n: u16) {
        self.num_added += Wrapping(n);
        self.next_used += Wrapping(n);
    }

    /// Set the used ring index to the current `next_used` value.
    /// Should be called once after number of `add_used` calls.
    pub fn advance_used_ring_idx(&mut self) {
        // This fence ensures all descriptor writes are visible before the index update is.
        fence(Ordering::Release);
        self.used_ring_idx_set(self.next_used.0);
    }

    /// Puts an available descriptor head into the used ring for use by the guest.
    pub fn add_used(&mut self, desc_index: u16, len: u32) -> Result<(), QueueError> {
        self.write_used_element(0, desc_index, len)?;
        self.advance_next_used(1);
        Ok(())
    }

    /// Try to enable notification events from the guest driver. Returns true if notifications were
    /// successfully enabled. Otherwise it means that one or more descriptors can still be consumed
    /// from the available ring and we can't guarantee that there will be a notification. In this
    /// case the caller might want to consume the mentioned descriptors and call this method again.
    fn try_enable_notification(&mut self) -> Result<bool, InvalidAvailIdx> {
        // If the device doesn't use notification suppression, we'll continue to get notifications
        // no matter what.
        if !self.uses_notif_suppression {
            return Ok(true);
        }

        let len = self.len();
        if len != 0 {
            // The number of descriptor chain heads to process should always
            // be smaller or equal to the queue size.
            if len > self.size {
                return Err(InvalidAvailIdx {
                    queue_size: self.size,
                    reported_len: len,
                });
            }
            return Ok(false);
        }

        // Set the next expected avail_idx as avail_event.
        self.used_ring_avail_event_set(self.next_avail.0);

        // Make sure all subsequent reads are performed after we set avail_event.
        fence(Ordering::SeqCst);

        // If the actual avail_idx is different than next_avail one or more descriptors can still
        // be consumed from the available ring.
        Ok(self.next_avail.0 == self.avail_ring_idx_get())
    }

    /// Enable notification suppression.
    pub fn enable_notif_suppression(&mut self) {
        self.uses_notif_suppression = true;
    }

    /// Arm `avail_event` at the current `avail.idx` so the driver's next
    /// publish produces a notification. Unlike [`Self::try_enable_notification`],
    /// does not require the queue to be drained and does not recheck
    /// `avail.idx`; only correct when the driver does not add to the avail
    /// ring until it has observed our used-ring update.
    pub fn enable_notification(&mut self) {
        if !self.uses_notif_suppression {
            return;
        }

        self.used_ring_avail_event_set(self.avail_ring_idx_get());
        fence(Ordering::Release);
    }

    /// Check if we need to kick the guest.
    ///
    /// Please note this method has side effects: once it returns `true`, it considers the
    /// driver will actually be notified, and won't return `true` again until the driver
    /// updates `used_event` and/or the notification conditions hold once more.
    ///
    /// This is similar to the `vring_need_event()` method implemented by the Linux kernel.
    pub fn prepare_kick(&mut self) -> bool {
        // If the device doesn't use notification suppression, always return true
        if !self.uses_notif_suppression {
            return true;
        }

        // We need to expose used array entries before checking the used_event.
        fence(Ordering::SeqCst);

        let new = self.next_used;
        let old = self.next_used - self.num_added;
        let used_event = Wrapping(self.avail_ring_used_event_get());

        self.num_added = Wrapping(0);

        new - used_event - Wrapping(1) < new - old
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::virtio::test_utils::{default_mem, VirtQueue};
    use crate::vstate::memory::GuestAddress;

    #[test]
    fn descriptor_chain_walks_next_descriptors() {
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        vq.set_desc(0, 0x1000, 0x100, VIRTQ_DESC_F_NEXT, 1);
        vq.set_desc(1, 0x2000, 0x200, 0, 0);
        let mut q = vq.create_queue();
        vq.publish(0);

        let head = q.pop().unwrap().unwrap();
        assert_eq!(head.index, 0);
        assert_eq!(head.addr.0, 0x1000);
        assert_eq!(head.flags, VIRTQ_DESC_F_NEXT);

        let next = head.next_descriptor().unwrap();
        assert_eq!(next.index, 1);
        assert_eq!(next.addr.0, 0x2000);
        assert!(next.next_descriptor().is_none());
    }

    #[test]
    fn descriptor_chain_rejects_out_of_bounds_next() {
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        vq.set_desc(0, 0x1000, 0x100, VIRTQ_DESC_F_NEXT, 16);
        let q = vq.create_queue();
        assert!(DescriptorChain::checked_new(q.desc_table, 16, 0).is_none());
    }

    #[test]
    fn queue_pop_and_add_used_round_trip() {
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        vq.set_desc(0, 0x1000, 0x10, 0, 0);
        vq.set_desc(1, 0x2000, 0x20, 0, 0);
        let mut q = vq.create_queue();
        vq.publish(0);
        vq.publish(1);

        let head = q.pop().unwrap().unwrap();
        assert_eq!(head.index, 0);
        q.add_used(head.index, 0x10).unwrap();

        let head = q.pop().unwrap().unwrap();
        assert_eq!(head.index, 1);
        q.add_used(head.index, 0x20).unwrap();

        q.advance_used_ring_idx();
        assert_eq!(vq.used_idx(), 2);
        assert_eq!(vq.used_elem(0), (0, 0x10));
        assert_eq!(vq.used_elem(1), (1, 0x20));
    }

    #[test]
    fn queue_rejects_avail_idx_larger_than_queue() {
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        let mut q = vq.create_queue();
        // A malicious driver publishes more descriptors than the queue holds.
        vq.set_avail_idx(17);
        assert!(q.pop().is_err());
    }

    #[test]
    fn queue_initialization_validates_size_and_alignment() {
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        let mut q = vq.create_queue();

        q.size = 11;
        assert!(matches!(q.initialize(&mem).unwrap_err(), QueueError::InvalidSize(_)));
        q.size = 16;

        q.desc_table_address = GuestAddress(0x1001);
        assert!(matches!(
            q.initialize(&mem).unwrap_err(),
            QueueError::PointerNotAligned(_, _)
        ));
    }

    #[test]
    fn pop_or_enable_notification_drains_then_arms() {
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        vq.set_desc(0, 0x1000, 0x10, 0, 0);
        let mut q = vq.create_queue();
        q.enable_notif_suppression();

        vq.publish(0);
        assert!(q.pop_or_enable_notification().unwrap().is_some());

        // Drained: the next call arms avail_event and reports no work.
        assert!(q.pop_or_enable_notification().unwrap().is_none());
    }
}
