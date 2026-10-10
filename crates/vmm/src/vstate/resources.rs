// Copyright 2023 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/vstate/resources.rs (trimmed: the
// `vm_allocator` and `bitvec` dependencies are replaced with local
// equivalents — a first-fit bump `AddressAllocator` without free-list
// support and a `Vec<bool>`-backed `IdAllocator`; the snapshot `Persist`
// implementation is dropped).

// Upstream re-exports the per-arch `layout` constants through `crate::arch`
// (`pub use crate::arch::x86_64::{layout::*, ...}` in its `arch/mod.rs`); this
// tree's `arch/mod.rs` declares the per-arch modules only, so select the
// current arch's `layout` module here.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use crate::arch::x86_64::layout as arch_layout;
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
use crate::arch::aarch64::layout as arch_layout;
use crate::align_up;

/// Errors associated with the resource allocators (mirroring the variants of
/// the upstream `vm_allocator::Error` that this port can produce).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The provided range is invalid: base {0:#x}, size {1:#x}
    #[error("The provided range is invalid: base {0:#x}, size {1:#x}")]
    InvalidRange(u64, u64),
    /// The operation caused an overflow
    #[error("The operation caused an overflow")]
    Overflow,
    /// The requested resource is not available
    #[error("The requested resource is not available")]
    ResourceNotAvailable,
    /// The provided id {0} is out of range
    #[error("The provided id {0} is out of range")]
    OutOfRange(u32),
    /// The id {0} was never allocated
    #[error("The id {0} was never allocated")]
    NeverAllocated(u32),
}

/// Helper function to allocate many ids from an id allocator
fn allocate_many_ids(id_allocator: &mut IdAllocator, count: u32) -> Result<Vec<u32>, Error> {
    let mut ids = Vec::with_capacity(count as usize);

    for _ in 0..count {
        match id_allocator.allocate_id() {
            Ok(id) => ids.push(id),
            Err(err) => {
                // It is ok to unwrap here, we just allocated the GSI
                ids.into_iter().for_each(|id| {
                    id_allocator.free_id(id).unwrap();
                });
                return Err(err);
            }
        }
    }

    Ok(ids)
}

/// A resource manager for (de)allocating interrupt lines (GSIs) and guest memory
///
/// At the moment, we support:
///
/// * GSIs for legacy x86_64 devices
/// * GSIs for MMIO devicecs
/// * Memory allocations in the MMIO address space
#[derive(Debug, Clone)]
pub struct ResourceAllocator {
    /// Allocator for legacy device interrupt lines
    pub gsi_legacy_allocator: IdAllocator,
    /// Allocator for PCI device GSIs
    pub gsi_msi_allocator: IdAllocator,
    /// Allocator for memory in the 32-bit MMIO address space
    pub mmio32_memory: AddressAllocator,
    /// Allocator for memory in the 64-bit MMIO address space
    pub mmio64_memory: AddressAllocator,
    /// Allocator for memory after the 64-bit MMIO address space
    pub past_mmio64_memory: AddressAllocator,
    /// Memory allocator for system data
    pub system_memory: AddressAllocator,
}

impl Default for ResourceAllocator {
    fn default() -> Self {
        ResourceAllocator::new()
    }
}

impl ResourceAllocator {
    /// Create a new resource allocator for Firecracker devices
    pub fn new() -> Self {
        // It is fine for us to unwrap the following since we know we are passing valid ranges for
        // all allocators
        Self {
            gsi_legacy_allocator: IdAllocator::new(
                arch_layout::GSI_LEGACY_START,
                arch_layout::GSI_LEGACY_END,
            )
            .unwrap(),
            gsi_msi_allocator: IdAllocator::new(
                arch_layout::GSI_MSI_START,
                arch_layout::GSI_MSI_END,
            )
            .unwrap(),
            mmio32_memory: AddressAllocator::new(
                arch_layout::MEM_32BIT_DEVICES_START,
                arch_layout::MEM_32BIT_DEVICES_SIZE,
            )
            .unwrap(),
            mmio64_memory: AddressAllocator::new(
                arch_layout::MEM_64BIT_DEVICES_START,
                arch_layout::MEM_64BIT_DEVICES_SIZE,
            )
            .unwrap(),
            past_mmio64_memory: AddressAllocator::new(
                arch_layout::FIRST_ADDR_PAST_64BITS_MMIO,
                arch_layout::PAST_64BITS_MMIO_SIZE,
            )
            .unwrap(),
            system_memory: AddressAllocator::new(
                arch_layout::SYSTEM_MEM_START,
                arch_layout::SYSTEM_MEM_SIZE,
            )
            .unwrap(),
        }
    }

    /// Allocate a number of legacy GSIs
    ///
    /// # Arguments
    ///
    /// * `gsi_count` - The number of legacy GSIs to allocate
    pub fn allocate_gsi_legacy(&mut self, gsi_count: u32) -> Result<Vec<u32>, Error> {
        allocate_many_ids(&mut self.gsi_legacy_allocator, gsi_count)
    }

    /// Allocate a number of GSIs for MSI
    ///
    /// # Arguments
    ///
    /// * `gsi_count` - The number of GSIs to allocate
    pub fn allocate_gsi_msi(&mut self, gsi_count: u32) -> Result<Vec<u32>, Error> {
        allocate_many_ids(&mut self.gsi_msi_allocator, gsi_count)
    }
}

/// An unique ID allocator that allows management of IDs in a given interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdAllocator {
    // Beginning of the range of IDs that we want to manage.
    range_base: u32,
    // One flag per id in the managed range. A `true` entry means the
    // corresponding id is currently allocated (a `BitVec` upstream).
    allocated: Vec<bool>,
}

impl IdAllocator {
    /// Create a new IdAllocator with IDs in [`range_base`, `range_end`].
    pub fn new(range_base: u32, range_end: u32) -> Result<Self, Error> {
        if range_end < range_base {
            return Err(Error::InvalidRange(
                u64::from(range_base),
                u64::from(range_end),
            ));
        }

        let num_ids = u64::from(range_end) - u64::from(range_base) + 1;
        let num_ids = usize::try_from(num_ids).map_err(|_| Error::Overflow)?;

        Ok(IdAllocator {
            range_base,
            allocated: vec![false; num_ids],
        })
    }

    /// Map `id` to its corresponding index in the bitmap.
    fn index_for(&self, id: u32) -> Result<usize, Error> {
        let offset = id
            .checked_sub(self.range_base)
            .ok_or(Error::OutOfRange(id))?;
        let index = offset as usize;
        self.allocated
            .get(index)
            .map(|_| index)
            .ok_or(Error::OutOfRange(id))
    }

    // Given `index` into the bitmap, return the `id` in that position
    // Returns `Error::Overflow` if the index would map to an overflowed id.
    fn id_for_index(&self, index: usize) -> Result<u32, Error> {
        let offset = u32::try_from(index).map_err(|_| Error::Overflow)?;
        self.range_base
            .checked_add(offset)
            .ok_or(Error::Overflow)
    }

    /// Allocate an ID from the managed range.Returns the first available id
    /// from the managed range, or `ResourceNotAvailable` if every id has been
    /// handed out.
    pub fn allocate_id(&mut self) -> Result<u32, Error> {
        if let Some(index) = self.allocated.iter().position(|allocated| !allocated) {
            self.allocated[index] = true;
            let id = self.id_for_index(index)?;
            Ok(id)
        } else {
            Err(Error::ResourceNotAvailable)
        }
    }

    /// Allocate the specified ID from the managed range.
    ///
    /// Returns `id` on success, `OutOfRange` if `id` is outside the managed
    /// range, or `ResourceNotAvailable` if `id` has already been allocated.
    pub fn allocate_id_at(&mut self, id: u32) -> Result<u32, Error> {
        let index = self.index_for(id)?;
        if !self.allocated[index] {
            self.allocated[index] = true;
            Ok(id)
        } else {
            Err(Error::ResourceNotAvailable)
        }
    }

    /// Returns `true` if `id` is currently allocated from the managed range.
    ///
    /// Ids outside the managed range are considered never allocated, so return false.
    pub fn is_allocated(&self, id: u32) -> bool {
        match self.index_for(id) {
            Ok(index) => self.allocated[index],
            Err(_) => false,
        }
    }

    /// Frees an id from the managed range.
    pub fn free_id(&mut self, id: u32) -> Result<u32, Error> {
        let index = self.index_for(id)?;
        if self.allocated[index] {
            self.allocated[index] = false;
            Ok(id)
        } else {
            Err(Error::NeverAllocated(id))
        }
    }
}

/// Allocation policy for [`AddressAllocator`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocPolicy {
    /// Allocate the first matching free range.
    FirstMatch,
}

/// A first-fit bump allocator over a contiguous address range.
///
/// Local equivalent of the upstream `vm_allocator::AddressAllocator`; unlike
/// upstream it cannot free ranges (nothing in this port frees allocations),
/// and only supports [`AllocPolicy::FirstMatch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressAllocator {
    /// Base address of the managed range.
    base: u64,
    /// Size in bytes of the managed range.
    size: u64,
    /// First free address (allocations only ever move this forward).
    next: u64,
}

impl AddressAllocator {
    /// Create a new allocator managing `[base, base + size)`.
    pub fn new(base: u64, size: u64) -> Result<Self, Error> {
        let end = base.checked_add(size).ok_or(Error::Overflow)?;
        if size == 0 || end == 0 {
            return Err(Error::InvalidRange(base, size));
        }
        Ok(AddressAllocator { base, size, next: base })
    }

    /// Allocate `size` bytes aligned to `alignment` (a power of two) from the
    /// managed range, returning the allocated range on success.
    pub fn allocate(
        &mut self,
        size: u64,
        alignment: u64,
        policy: AllocPolicy,
    ) -> Result<std::ops::RangeInclusive<u64>, Error> {
        // This port only ever allocates first-match (the local equivalent of
        // the upstream `AllocPolicy::FirstMatch`); keep the policy parameter
        // so callers stay in the upstream shape.
        let _ = policy;
        assert!(alignment.is_power_of_two(), "alignment must be a power of two");
        if size == 0 {
            return Err(Error::InvalidRange(self.base, 0));
        }
        let alignment = alignment.max(1);

        let size = align_up!(size, alignment);
        let start = align_up!(self.next, alignment);
        let end = start
            .checked_add(size)
            .filter(|&end| end <= self.base + self.size)
            .ok_or(Error::ResourceNotAvailable)?;

        self.next = end;
        Ok(start..=end - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The GSI bounds differ per architecture; anchor the allocator tests on
    // whatever the current arch exposes.
    #[test]
    fn test_allocate_many_ids() {
        let mut id_allocator = IdAllocator::new(5, 23).unwrap();
        let ids = allocate_many_ids(&mut id_allocator, 3).unwrap();
        assert_eq!(ids, vec![5, 6, 7]);

        // Freeing is wired through `free_id`.
        for id in ids {
            id_allocator.free_id(id).unwrap();
        }

        // The freed ids are handed out again, in order.
        let ids = allocate_many_ids(&mut id_allocator, 3).unwrap();
        assert_eq!(ids, vec![5, 6, 7]);
    }

    #[test]
    fn test_allocate_many_ids_failure_rolls_back() {
        let mut id_allocator = IdAllocator::new(5, 7).unwrap();
        // Requesting more ids than the range holds must fail and roll back.
        let err = allocate_many_ids(&mut id_allocator, 4).unwrap_err();
        assert_eq!(err, Error::ResourceNotAvailable);
        // The allocator is untouched: all three ids are still available.
        assert_eq!(id_allocator.allocate_id().unwrap(), 5);
        assert_eq!(id_allocator.allocate_id().unwrap(), 6);
        assert_eq!(id_allocator.allocate_id().unwrap(), 7);
    }

    #[test]
    fn test_id_allocator() {
        let mut id_allocator = IdAllocator::new(5, 7).unwrap();

        // Invalid range: end before base.
        assert_eq!(
            IdAllocator::new(9, 5).unwrap_err(),
            Error::InvalidRange(9, 5)
        );

        // Ids are handed out in order and track allocation state.
        assert_eq!(id_allocator.allocate_id().unwrap(), 5);
        assert!(id_allocator.is_allocated(5));
        assert!(!id_allocator.is_allocated(6));
        assert_eq!(
            id_allocator.allocate_id_at(5).unwrap_err(),
            Error::ResourceNotAvailable
        );
        assert_eq!(
            id_allocator.allocate_id_at(6).unwrap(),
            6
        );
        assert_eq!(
            id_allocator.allocate_id_at(8).unwrap_err(),
            Error::OutOfRange(8)
        );

        // The managed range is [5, 7]: the last id goes out, then the
        // allocator is exhausted.
        assert_eq!(id_allocator.allocate_id().unwrap(), 7);
        assert!(id_allocator.is_allocated(7));
        assert_eq!(
            id_allocator.allocate_id().unwrap_err(),
            Error::ResourceNotAvailable
        );

        // Freeing an unallocated id fails; freeing an allocated one works and
        // makes it allocatable again.
        id_allocator.free_id(7).unwrap();
        assert_eq!(id_allocator.free_id(7).unwrap_err(), Error::NeverAllocated(7));
        assert_eq!(id_allocator.free_id(5).unwrap(), 5);
        assert_eq!(id_allocator.allocate_id().unwrap(), 5);

        // Ids outside the managed range report as never allocated.
        assert!(!id_allocator.is_allocated(4));
        assert!(!id_allocator.is_allocated(8));
    }

    #[test]
    fn test_address_allocator() {
        let mut allocator = AddressAllocator::new(0x9fc00, 0x24400).unwrap();

        // Zero-sized ranges are invalid.
        assert_eq!(
            AddressAllocator::new(0x1000, 0).unwrap_err(),
            Error::InvalidRange(0x1000, 0)
        );

        // First allocation starts at the base and honors alignment.
        let range = allocator.allocate(1024, 1024, AllocPolicy::FirstMatch).unwrap();
        assert_eq!(*range.start(), 0x9fc00);
        assert_eq!(*range.end(), 0x9fc00 + 1023);

        // The next allocation starts right after (aligned up).
        let range = allocator.allocate(1, 1, AllocPolicy::FirstMatch).unwrap();
        assert_eq!(*range.start(), 0xa0000);

        // Exhausting the range fails.
        let mut tiny = AddressAllocator::new(0x100, 0x10).unwrap();
        assert_eq!(
            tiny.allocate(0x11, 1, AllocPolicy::FirstMatch).unwrap_err(),
            Error::ResourceNotAvailable
        );
    }

    #[test]
    fn test_resource_allocator_gsis() {
        let mut resource_allocator = ResourceAllocator::new();
        // The legacy GSI range on x86_64 is [5, 23] and the MSI range follows
        // it; only the legacy range is exercised here.
        let gsis = resource_allocator.allocate_gsi_legacy(2).unwrap();
        assert_eq!(gsis.len(), 2);
        assert!(gsis[0] >= arch_layout::GSI_LEGACY_START);
    }
}
