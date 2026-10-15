// Copyright 2020 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE-BSD-3-Clause file.
//
// Derived from Firecracker src/vmm/src/vstate/memory.rs (trimmed:
// hotpluggable/unplugged slots, dirty-page tracking, memfd and snapshot-file
// backings, hugetlb, and balloon discard; one plugged KVM slot per DRAM
// region) and src/utils/mod.rs + src/arch/mod.rs (numeric helpers,
// `host_page_size` and the `align_up!`/`align_down!` macros folded in).

// Syscall boundary (see the allowlist in `lib.rs`): `mmap`/`munmap`/`sysconf`
// and the raw-pointer guest-region builder have no safe-Rust equivalent, and
// the region owns its mapping, which is what the `Send`/`Sync` impls assert.
// On Windows the guest region is vm-memory's own `VirtualAlloc` mapping, so there is no
// unsafe code of ours on that path.
#![allow(unsafe_code)]

#[cfg(unix)]
use std::ffi::c_void;
use std::ops::Deref;

#[cfg(target_os = "linux")]
use kvm_bindings::kvm_userspace_memory_region;
pub use vm_memory::{
    Address, ByteValued, Bytes, FileOffset, GuestAddress, GuestMemoryRegion, GuestUsize,
    MemoryRegionAddress, VolatileSlice,
};
use vm_memory::{GuestMemoryRegionBytes, GuestRegionCollection};

/// Type of GuestMemoryMmap.
pub type GuestMemoryMmap = GuestRegionCollection<GuestRegionMmapExt>;

/// How many bits to left-shift by to convert MiB to bytes
const MIB_TO_BYTES_SHIFT: usize = 20;

/// Safely converts a u64 value to a usize value.
/// This bypasses the Clippy lint check because we only support 64-bit platforms.
#[cfg(target_pointer_width = "64")]
#[inline]
#[allow(clippy::cast_possible_truncation)]
pub const fn u64_to_usize(num: u64) -> usize {
    num as usize
}

/// Safely converts a usize value to a u64 value.
/// This bypasses the Clippy lint check because we only support 64-bit platforms.
#[cfg(target_pointer_width = "64")]
#[inline]
#[allow(clippy::cast_possible_truncation)]
pub const fn usize_to_u64(num: usize) -> u64 {
    num as u64
}

/// Converts MiB to Bytes
pub const fn mib_to_bytes(mib: usize) -> usize {
    mib << MIB_TO_BYTES_SHIFT
}

/// Get the size of the host page size.
#[cfg(unix)]
pub fn host_page_size() -> usize {
    /// Default page size for the host OS.
    static PAGE_SIZE: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
        // # Safety: Value always valid
        let r = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        usize::try_from(r).unwrap_or(4096)
    });

    *PAGE_SIZE
}

/// Get the size of the host page size (Windows: 4 KiB, the page size of the supported targets).
#[cfg(windows)]
pub fn host_page_size() -> usize {
    4096
}

/// Align address up to the alignment.
///
/// Works with any integer type (`u64`, `usize`, etc.).
/// `$align` must be a power of two.
#[macro_export]
macro_rules! align_up {
    ($addr:expr, $align:expr) => {{
        assert!($align.is_power_of_two());
        ($addr.wrapping_add($align - 1)) & !($align - 1)
    }};
}

/// Align address down to the alignment.
///
/// Works with any integer type (`u64`, `usize`, etc.).
/// `$align` must be a power of two.
#[macro_export]
macro_rules! align_down {
    ($addr:expr, $align:expr) => {{
        assert!($align.is_power_of_two());
        $addr & !($align - 1)
    }};
}

/// The alignment used to allocate guest memory.
/// Chosen to enable optimizations on host kernel, e.g. allow huge pages at the beginning of the memory space.
#[cfg(unix)]
const GUEST_MEMORY_ALIGNMENT: usize = mib_to_bytes(2);

/// Errors associated with the guest memory implementation.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    /// Cannot create mmap region: {0}
    #[error("Cannot create mmap region: {0}")]
    MmapRegionError(vm_memory::mmap::MmapRegionError),
    /// Cannot create guest memory
    #[error("Cannot create guest memory")]
    VmMemoryError,
    /// Total sum of memory regions exceeds largest possible file offset
    #[error("Total sum of memory regions exceeds largest possible file offset")]
    OffsetTooLarge,
    /// Memory region has zero size
    #[error("Memory region has zero size")]
    ZeroSize,
    /// A file-backed memory region's offset or length is not a multiple of the host page size
    #[error("File-backed memory region at offset {offset} of length {len} is not page aligned")]
    UnalignedFileRegion { offset: u64, len: u64 },
    /// The backing file is shorter than the regions laid out in it
    #[error("Backing file of {file_len} bytes is shorter than the {needed} bytes its regions need")]
    FileTooShort { file_len: u64, needed: u64 },
    /// Cannot read the length of the backing file
    #[error("Cannot read the backing file's length: {0}")]
    FileLength(std::io::Error),
    /// Failed to allocate guest memory with VirtualAlloc
    #[cfg(windows)]
    #[error("failed to allocate guest memory: {0}")]
    Alloc(std::io::Error),
}

/// A region of guest memory. Unmapped on drop.
/// This struct doesn't provide any facility to access the memory or read its properties, see [GuestRegionMmap].
#[cfg(unix)]
#[derive(Debug)]
struct RawGuestRegionMmap {
    mmap_address: *mut u8,
    mmap_size: usize,
}

#[cfg(unix)]
impl RawGuestRegionMmap {
    /// Allocates a `PROT_NONE` region of memory, aligned to [`GUEST_MEMORY_ALIGNMENT`] (2 MiB).
    ///
    /// To actually use the memory, callers must either re-map it (e.g. with `MAP_FIXED`) or use
    /// [`Self::allocate`] which does this automatically.
    ///
    /// # Size rounding
    ///
    /// The returned `mmap_size` may be **larger** than `requested_size`: it is rounded up to the
    /// nearest multiple of the page size. This is because `mmap`/`munmap` operate at page
    /// granularity — the kernel cannot map or protect a partial page. For example, requesting
    /// 5000 bytes on a 4 KiB-page host yields an `mmap_size` of 8192 bytes (2 pages).
    ///
    /// The invariant is: `mmap_size == align_up!(requested_size, page_size)`.
    fn allocate_protected(requested_size: usize) -> Result<Self, MemoryError> {
        // Strategy: over-allocate by up to `GUEST_MEMORY_ALIGNMENT - page_size`, then trim
        // the unaligned head and tail via munmap, leaving a 2 MiB-aligned region of
        // `size_page_multiple` bytes.
        //
        //         ptr                          ptr + alloc_size
        //          |                                  |
        //          v                                  v
        //          +------+------------------+--------+
        //          | head |   guest memory   |  tail  |
        //          +------+------------------+--------+
        //                 ^                  ^
        //                 |                  |
        //           aligned_ptr       aligned_ptr + size_page_multiple
        //          (2 MiB-aligned)
        //
        //  1. Round requested_size up to page size -> `size_page_multiple`.
        //  2. mmap(`size_page_multiple + GUEST_MEMORY_ALIGNMENT - page_size`) -> `ptr`.
        //     `ptr` is page-aligned, so the worst-case distance to the next 2 MiB boundary
        //     is `GUEST_MEMORY_ALIGNMENT - page_size` bytes. This guarantees the allocation
        //     contains at least one 2 MiB-aligned region of `size_page_multiple` bytes.
        //  3. Compute `aligned_ptr` = first 2 MiB boundary ≥ `ptr`.
        //  4. munmap the head (`ptr..aligned_ptr`) and tail (`aligned_ptr + size_page_multiple..end`).
        //  5. Return the 2 MiB-aligned region of `size_page_multiple` bytes.
        if requested_size == 0 {
            return Err(MemoryError::ZeroSize);
        }
        let page_size = host_page_size();
        let size_page_multiple = align_up!(requested_size, page_size);

        // Over-allocate to guarantee we can find a 2 MiB-aligned sub-region of the desired size.
        // The over-allocation is `GUEST_MEMORY_ALIGNMENT - page_size` because the mmap return
        // address is page-aligned, so we need at most that many extra bytes to reach the next
        // 2 MiB boundary.
        let alloc_size = size_page_multiple + GUEST_MEMORY_ALIGNMENT - page_size;

        // SAFETY: anonymous private mapping with no fd; does not alias existing memory.
        // The returned region is PROT_NONE (inaccessible) until the caller re-maps it.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                alloc_size,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_NORESERVE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(MemoryError::MmapRegionError(
                vm_memory::mmap::MmapRegionError::Mmap(std::io::Error::last_os_error()),
            ));
        }

        // Find the first 2 MiB-aligned address within the allocation.
        let aligned_ptr = align_up!(ptr as usize, GUEST_MEMORY_ALIGNMENT);

        // Compute head/tail sizes to trim. Both are guaranteed to be page-aligned because
        // `ptr` is page-aligned (from mmap), `aligned_ptr` is 2 MiB-aligned (a multiple of
        // page size), and `size_page_multiple` is a page multiple by construction.
        let head_size = aligned_ptr - ptr as usize;
        let tail_size = alloc_size - head_size - size_page_multiple;

        // Sanity checks: the three parts must exactly partition the allocation.
        assert_eq!(
            head_size + size_page_multiple + tail_size,
            alloc_size,
            "Allocated memory is not fully partitioned"
        );
        assert_eq!(
            ptr as usize + head_size,
            aligned_ptr,
            "Head and aligned portion aren't adjacent"
        );
        assert_eq!(
            aligned_ptr + size_page_multiple,
            ptr as usize + alloc_size - tail_size,
            "Tail and aligned portion aren't adjacent"
        );
        assert_eq!(
            head_size % page_size,
            0,
            "head size is not a multiple of page-size"
        );
        assert_eq!(
            tail_size % page_size,
            0,
            "tail size is not a multiple of page-size"
        );

        // Unmap the head and tail to release them back to the kernel.
        // Failures are non-critical (at worst we leak a `PROT_NONE` VMA slot), so we only log.
        if head_size > 0 {
            // SAFETY: `ptr` is page-aligned and `head_size` is a page multiple within our mapping.
            let ret = unsafe { libc::munmap(ptr, head_size) };
            if ret != 0 {
                log_munmap_failure(head_size);
            }
        }
        if tail_size > 0 {
            // SAFETY: address is page-aligned and `tail_size` is a page multiple within our mapping.
            let ret = unsafe {
                libc::munmap((aligned_ptr + size_page_multiple) as *mut c_void, tail_size)
            };
            if ret != 0 {
                log_munmap_failure(tail_size);
            }
        }

        Ok(Self {
            mmap_address: aligned_ptr as *mut u8,
            mmap_size: size_page_multiple,
        })
    }

    /// Allocates a region of memory of the given size, aligned with [GUEST_MEMORY_ALIGNMENT].
    ///
    /// Note: the returned region of memory might be bigger than the requested size, as it is aligned to page-size.
    fn allocate(size: usize, prot: libc::c_int, flags: libc::c_int) -> Result<Self, MemoryError> {
        // This function works by doing a mmap fixed over a GuestMmapRegion allocated with PROT_NONE.
        let ret = Self::allocate_protected(size)?;

        // SAFETY: this mmap is performed over the same memory region we just allocated, so it's
        // guarantee it's not in use elsewhere.
        let addr = unsafe {
            libc::mmap(
                ret.mmap_address.cast::<c_void>(),
                ret.mmap_size,
                prot,
                flags | libc::MAP_FIXED,
                -1,
                0,
            )
        };
        if addr == libc::MAP_FAILED {
            return Err(MemoryError::MmapRegionError(
                vm_memory::mmap::MmapRegionError::Mmap(std::io::Error::last_os_error()),
            ));
        }
        assert_eq!(
            addr.cast::<u8>(),
            ret.mmap_address,
            "mmap fixed returned a different address"
        );

        Ok(ret)
    }
}

/// Reports a munmap failure during region trimming (non-fatal: at worst a
/// `PROT_NONE` VMA is leaked).
#[cfg(unix)]
fn log_munmap_failure(len: usize) {
    // Local equivalent of the upstream `warn!` logger macro (trimmed: no logger).
    eprintln!("warning: munmap of {len} bytes failed: {}", {
        // SAFETY: `std::io::Error::last_os_error` has no invariants.
        std::io::Error::last_os_error()
    });
}

#[cfg(unix)]
impl Drop for RawGuestRegionMmap {
    fn drop(&mut self) {
        // SAFETY: All memory access is done via GuestRegionMmap, and at this point there are no references to that.
        let ret = unsafe { libc::munmap(self.mmap_address.cast::<c_void>(), self.mmap_size) };
        // Panicking if unmapping failed: otherwise the guest memory might still be accessible after drop!
        assert_eq!(
            ret,
            0,
            "munmap of guest region failed: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// SAFETY: Send and Sync aren't automatically inherited for the raw address pointer.
/// RawGuestRegionMmap exclusively owns the memory region and doesn't expose the raw pointer for concurrent mutation.
#[cfg(unix)]
unsafe impl Send for RawGuestRegionMmap {}
/// SAFETY: See comment above.
#[cfg(unix)]
unsafe impl Sync for RawGuestRegionMmap {}

/// A region of guest memory. Unmapped on drop.
/// Note: this implements automatic defer for vm_memory::GuestRegionMmap, so it can be used in its place.
#[derive(Debug)]
pub struct GuestRegionMmap {
    /// Held for its `Drop` impl which unmaps the guest memory region.
    #[cfg(unix)]
    _region: RawGuestRegionMmap,
    proxy: vm_memory::GuestRegionMmap,
}

impl Deref for GuestRegionMmap {
    type Target = vm_memory::GuestRegionMmap;

    fn deref(&self) -> &Self::Target {
        &self.proxy
    }
}

impl GuestRegionMmap {
    /// Creates a new [GuestRegionMmap], by actually allocating the guest memory with the given properties.
    #[cfg(unix)]
    fn allocate(guest_base: GuestAddress, size: usize) -> Result<Self, MemoryError> {
        let region = RawGuestRegionMmap::allocate(
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
        )?;

        let mut builder = vm_memory::mmap::MmapRegionBuilder::new(size)
            .with_mmap_prot(libc::PROT_READ | libc::PROT_WRITE)
            .with_mmap_flags(libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE);

        // SAFETY: the memory mapping is valid, and has the same flags passed to builder.
        unsafe {
            builder = builder.with_raw_mmap_pointer(region.mmap_address);
        }

        let mmap_region = builder.build().map_err(MemoryError::MmapRegionError)?;
        // The memory cannot be double-owned: OwnedRegionMmap should be responsible for deallocation
        assert!(!mmap_region.owned());
        // `region` and `mapping` need to point to the same address.
        assert_eq!(region.mmap_address, mmap_region.as_ptr());
        // Size might differ, as region.mmap_size is page-aligned. But it for sure cannot be smaller.
        assert!(region.mmap_size >= mmap_region.size());

        Ok(GuestRegionMmap {
            #[cfg(unix)]
            _region: region,
            proxy: vm_memory::GuestRegionMmap::new(mmap_region, guest_base)
                .ok_or(MemoryError::VmMemoryError)?,
        })
    }

    /// Creates a new [GuestRegionMmap], by allocating the guest memory with vm-memory's own
    /// Windows mapping (committed, read-write `VirtualAlloc`, freed when the region is dropped).
    #[cfg(windows)]
    fn allocate(guest_base: GuestAddress, size: usize) -> Result<Self, MemoryError> {
        if size == 0 {
            return Err(MemoryError::ZeroSize);
        }
        let mmap_region = vm_memory::MmapRegion::new(size).map_err(MemoryError::Alloc)?;
        Ok(GuestRegionMmap {
            proxy: vm_memory::GuestRegionMmap::new(mmap_region, guest_base)
                .ok_or(MemoryError::VmMemoryError)?,
        })
    }
}

/// An extension to GuestMemoryRegion that stores the KVM slot number this DRAM
/// region is registered under.
///
/// Upstream splits regions into multiple KVM slots of the same slot size and
/// tracks plugged/unplugged state for memory hotplug; this port registers each
/// DRAM region as a single always-plugged slot.
#[derive(Debug)]
pub struct GuestRegionMmapExt {
    /// the wrapped GuestRegionMmap
    pub inner: GuestRegionMmap,
    /// the KVM slot number assigned to this region
    pub slot: u32,
}

#[cfg(target_os = "linux")]
impl From<&GuestRegionMmapExt> for kvm_userspace_memory_region {
    fn from(region: &GuestRegionMmapExt) -> Self {
        kvm_userspace_memory_region {
            // No dirty page tracking (trimmed along with snapshot support).
            flags: 0,
            slot: region.slot,
            guest_phys_addr: region.start_addr().raw_value(),
            memory_size: region.len(),
            userspace_addr: region.as_ptr() as u64,
        }
    }
}

impl GuestRegionMmapExt {
    /// Adds a DRAM region which only contains a single plugged slot
    pub(crate) fn dram_from_mmap_region(region: GuestRegionMmap, slot: u32) -> Self {
        GuestRegionMmapExt {
            inner: region,
            slot,
        }
    }
}

impl Deref for GuestRegionMmapExt {
    type Target = vm_memory::MmapRegion<()>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl GuestMemoryRegionBytes for GuestRegionMmapExt {}

#[allow(clippy::cast_possible_wrap)]
#[allow(clippy::cast_possible_truncation)]
impl vm_memory::GuestMemoryRegion for GuestRegionMmapExt {
    type B = ();

    fn len(&self) -> GuestUsize {
        self.inner.len()
    }

    fn start_addr(&self) -> GuestAddress {
        self.inner.start_addr()
    }

    fn bitmap(&self) -> vm_memory::bitmap::BS<'_, Self::B> {
        self.inner.bitmap()
    }

    fn get_host_address(
        &self,
        addr: MemoryRegionAddress,
    ) -> vm_memory::guest_memory::Result<*mut u8> {
        self.inner.get_host_address(addr)
    }

    fn file_offset(&self) -> Option<&FileOffset> {
        self.inner.file_offset()
    }

    fn get_slice(
        &self,
        offset: MemoryRegionAddress,
        count: usize,
    ) -> vm_memory::guest_memory::Result<VolatileSlice<'_, vm_memory::bitmap::BS<'_, Self::B>>>
    {
        self.inner.get_slice(offset, count)
    }
}

/// Creates a single `GuestRegionMmap`.
fn memory_region_from_range(
    start: GuestAddress,
    size: usize,
) -> Result<GuestRegionMmap, MemoryError> {
    GuestRegionMmap::allocate(start, size)
}

/// Creates a `Vec` of anonymous `GuestRegionMmap` with the given configuration.
pub fn memory_regions_from_ranges(
    regions: &[(GuestAddress, usize)],
) -> Result<Vec<GuestRegionMmap>, MemoryError> {
    regions
        .iter()
        .map(|&(start, size)| memory_region_from_range(start, size))
        .collect()
}

/// Creates a GuestMemoryMmap from raw regions.
pub fn anonymous(regions: &[(GuestAddress, usize)]) -> Result<Vec<GuestRegionMmap>, MemoryError> {
    // Upstream additionally madvise()s the regions (MADV_NORMAL for the
    // default configuration, i.e. a no-op) and can track dirty pages or use
    // hugetlb; both trimmed here.
    memory_regions_from_ranges(regions)
}

/// Creates a [`GuestMemoryMmap`] from anonymous regions, each registered as a
/// single plugged slot.
///
/// Unlike [`anonymous`], the result is usable as guest memory. This is the
/// safe constructor for callers that need a guest memory object outside a
/// booting VM: the in-crate unit tests, and the out-of-crate fuzz harnesses.
pub fn guest_memory_from_ranges(
    regions: &[(GuestAddress, usize)],
) -> Result<GuestMemoryMmap, MemoryError> {
    GuestRegionCollection::from_regions(
        anonymous(regions)?
            .into_iter()
            .map(|region| GuestRegionMmapExt::dram_from_mmap_region(region, 0))
            .collect(),
    )
    .map_err(|_| MemoryError::VmMemoryError)
}

/// Maps `file` as guest memory laid out by `regions` (a snapshot's memory file): each region a
/// private (`MAP_PRIVATE | MAP_NORESERVE`) mapping of the file at its offset, so the guest reads the
/// file's pages lazily and its writes stay in its own anonymous memory. Refuses a file shorter than
/// the layout. (Restore; see `crate::vstate::snapshot`.)
#[cfg(target_os = "linux")]
pub fn map_private(
    file: &std::fs::File,
    regions: &[crate::vstate::snapshot::RegionMeta],
) -> Result<Vec<GuestRegionMmap>, MemoryError> {
    let page = usize_to_u64(host_page_size());
    let file_len = file.metadata().map_err(MemoryError::FileLength)?.len();
    regions
        .iter()
        .map(|region| {
            // `mmap` takes page-aligned offsets and maps whole pages, so a region that is not
            // page-aligned could only be mapped with its tail silently rounded into the next
            // region. Refuse it instead.
            if region.len == 0 {
                return Err(MemoryError::ZeroSize);
            }
            if region.len % page != 0 || region.offset % page != 0 {
                return Err(MemoryError::UnalignedFileRegion {
                    offset: region.offset,
                    len: region.len,
                });
            }
            let needed = region
                .offset
                .checked_add(region.len)
                .ok_or(MemoryError::OffsetTooLarge)?;
            if needed > file_len {
                return Err(MemoryError::FileTooShort { file_len, needed });
            }
            map_file_region(
                GuestAddress(region.guest_addr),
                file,
                region.offset,
                region.len,
            )
        })
        .collect()
}

/// Maps `len` bytes of `file` from `offset` privately (copy-on-write) at a host address of its own,
/// wrapped the way [`GuestRegionMmap::allocate`] wraps an anonymous mapping. The caller has checked
/// that `offset` and `len` are page multiples and that the file covers them.
#[cfg(target_os = "linux")]
fn map_file_region(
    guest_base: GuestAddress,
    file: &std::fs::File,
    offset: u64,
    len: u64,
) -> Result<GuestRegionMmap, MemoryError> {
    use std::os::fd::AsRawFd;

    let size = u64_to_usize(len);
    let file_offset = libc::off_t::try_from(offset).map_err(|_| MemoryError::OffsetTooLarge)?;
    // SAFETY: a new mapping at a kernel-chosen address (no fixed address, so it overlaps nothing)
    // of `size` bytes of `file` from `file_offset`. The file covers the range, which the caller
    // checked, so no access beyond its end can fault with SIGBUS. `MAP_PRIVATE` makes the guest's
    // writes copy-on-write, so the file itself is never modified. The returned address is owned by
    // `RawGuestRegionMmap`, which unmaps it on drop.
    let addr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_NORESERVE,
            file.as_raw_fd(),
            file_offset,
        )
    };
    if addr == libc::MAP_FAILED {
        return Err(MemoryError::MmapRegionError(
            vm_memory::mmap::MmapRegionError::Mmap(std::io::Error::last_os_error()),
        ));
    }
    // From here on `region` owns the mapping: if the wrapping below fails, dropping it unmaps.
    let region = RawGuestRegionMmap {
        mmap_address: addr.cast::<u8>(),
        mmap_size: size,
    };

    let mut builder = vm_memory::mmap::MmapRegionBuilder::new(size)
        .with_mmap_prot(libc::PROT_READ | libc::PROT_WRITE)
        .with_mmap_flags(libc::MAP_PRIVATE | libc::MAP_NORESERVE);

    // SAFETY: `region.mmap_address` is the start of the `size`-byte mapping made above, which
    // `region` owns and outlives the `MmapRegion` built from it, and the flags passed to the
    // builder are the ones it was mapped with.
    unsafe {
        builder = builder.with_raw_mmap_pointer(region.mmap_address);
    }

    let mmap_region = builder.build().map_err(MemoryError::MmapRegionError)?;
    // Same checks as `GuestRegionMmap::allocate`: the mapping is not owned by the `MmapRegion`,
    // and both name the same address.
    assert!(!mmap_region.owned());
    assert_eq!(region.mmap_address, mmap_region.as_ptr());

    Ok(GuestRegionMmap {
        _region: region,
        proxy: vm_memory::GuestRegionMmap::new(mmap_region, guest_base)
            .ok_or(MemoryError::VmMemoryError)?,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Creates a [`GuestMemoryMmap`] with a single region of the given size starting at guest
    /// physical address 0 and without dirty tracking.
    #[allow(dead_code, reason = "used by the KVM backends' tests; HVF has its own")]
    pub(crate) fn single_region_mem(region_size: usize) -> GuestMemoryMmap {
        single_region_mem_at(0, region_size)
    }

    /// Creates a [`GuestMemoryMmap`] with a single region of the given size starting at the given
    /// guest physical address `at` and without dirty tracking.
    #[allow(dead_code, reason = "used by the KVM backends' tests; HVF has its own")]
    pub(crate) fn single_region_mem_at(at: u64, size: usize) -> GuestMemoryMmap {
        multi_region_mem(&[(GuestAddress(at), size)])
    }

    /// Creates a [`GuestMemoryMmap`] with multiple regions and without dirty page tracking.
    pub(crate) fn multi_region_mem(regions: &[(GuestAddress, usize)]) -> GuestMemoryMmap {
        guest_memory_from_ranges(regions).expect("Cannot initialize memory")
    }

    // Reads /proc/self/maps: Linux only.
    #[cfg(target_os = "linux")]
    /// Returns the (start, end) of the VMA containing `addr` in `/proc/self/maps`, if any.
    fn find_vma_containing(addr: usize) -> Option<(usize, usize)> {
        use std::io::BufRead;
        let maps = std::fs::File::open("/proc/self/maps").unwrap();
        for line in std::io::BufReader::new(maps).lines() {
            let line = line.unwrap();
            let range = line.split_whitespace().next().unwrap_or("");
            let mut parts = range.split('-');
            let start = usize::from_str_radix(parts.next().unwrap_or("0"), 16).unwrap_or(0);
            let end = usize::from_str_radix(parts.next().unwrap_or("0"), 16).unwrap_or(0);
            if addr >= start && addr < end {
                return Some((start, end));
            }
        }
        None
    }

    /// Creates a [`GuestMemoryMmap`] of the given size with the contained regions laid out in
    /// accordance with the requirements of the architecture on which the tests are being run
    /// (upstream: `test_utils::arch_mem`).
    // Only where a backend lays out memory for the host architecture (not Intel macOS).
    #[cfg(any(
        all(any(target_os = "linux", windows), target_arch = "x86_64"),
        all(not(windows), target_arch = "aarch64")
    ))]
    pub(crate) fn arch_mem(mem_size_bytes: usize) -> GuestMemoryMmap {
        // Upstream resolves this through `crate::arch::arch_memory_regions`; the
        // per-arch module is selected directly here.
        #[cfg(target_arch = "x86_64")]
        let regions = crate::arch::x86_64::arch_memory_regions(mem_size_bytes);
        #[cfg(target_arch = "aarch64")]
        let regions = crate::arch::aarch64::arch_memory_regions(mem_size_bytes);
        multi_region_mem(&regions)
    }

    // Reads /proc/self/maps: Linux only.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_guest_region_drop_reclaims_memory() {
        let size = mib_to_bytes(4);
        let region = RawGuestRegionMmap::allocate_protected(size).unwrap();
        let addr = region.mmap_address as usize;

        // The region should be mapped before drop.
        assert!(find_vma_containing(addr).is_some());

        // Drop the region.
        drop(region);

        // After drop, the memory should no longer be mapped.
        assert!(
            find_vma_containing(addr).is_none(),
            "Memory was not reclaimed after drop"
        );
    }
}
