// Copyright 2024 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/devices/virtio/iov_deque.rs (trimmed:
// the `memfd` crate becomes raw `libc` `memfd_create`/`fcntl` calls, so no new
// dependency is pulled in; `displaydoc` becomes explicit `thiserror` messages).

// Syscall + guest-memory boundary (see the allowlist in `lib.rs`):
// `memfd_create`/`ftruncate`/`fcntl`/`mmap`/`munmap` and the volatile access to
// the double-mapped iovec ring the guest and the device both see.
#![allow(unsafe_code)]

#[cfg(target_os = "linux")]
use libc::{c_int, c_void, off_t, size_t};

use crate::pal::iovec;
#[cfg(target_os = "linux")]
use crate::vstate::memory::host_page_size;

#[derive(Debug, thiserror::Error)]
pub enum IovDequeError {
    /// Error creating or sealing the memfd: {0}.
    #[cfg(target_os = "linux")]
    #[error("error creating or sealing the memfd: {0}")]
    Memfd(std::io::Error),
    /// Error while resizing the memfd: {0}.
    #[cfg(target_os = "linux")]
    #[error("error while resizing the memfd: {0}")]
    MemfdResize(std::io::Error),
    /// Error calling mmap: {0}.
    #[cfg(target_os = "linux")]
    #[error("error calling mmap: {0}")]
    Mmap(std::io::Error),
}

/// ['IovDeque'] is a ring buffer tailored for `struct iovec` objects.
///
/// From the point of view of API, [`IovDeque`] is a typical ring buffer that allows us to push
/// `struct iovec` objects at the end of the buffer and pop them from its beginning.
///
/// It is tailored to store `struct iovec` objects that described memory that was passed to us from
/// the guest via a VirtIO queue. This allows us to assume the maximum size of a ring buffer (the
/// negotiated size of the queue).
// An important feature of the data structure is that it can give us a slice of all `struct iovec`
// objects in the queue, so that we can use this `&mut [iovec]` to perform operations such as
// `readv`. A typical implementation of a ring buffer allows for entries to wrap around the end of
// the underlying buffer. For example, a ring buffer with a capacity of 10 elements which
// currently holds 4 elements can look like this:
//
//                      tail                        head
//                       |                           |
//                       v                           v
//                 +---+---+---+---+---+---+---+---+---+---+
// ring buffer:    | C | D |   |   |   |   |   |   | A | B |
//                 +---+---+---+---+---+---+---+---+---+---+
//
// When getting a slice for this data we should get something like that: &[A, B, C, D], which
// would require copies in order to make the elements continuous in memory.
//
// In order to avoid that and make the operation of getting a slice more efficient, we implement
// the optimization described in the "Optimization" section of the "Circular buffer" wikipedia
// entry: https://en.wikipedia.org/wiki/Circular_buffer. The optimization consists of allocating
// double the size of the virtual memory required for the buffer and map both parts on the same
// physical address. Looking at the same example as before, we should get, this picture:
//
//                                    head   |    tail
//                                     |     |     |
//                                     v     |     v
//   +---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
//   | C | D |   |   |   |   |   |   | A | B | C | D |   |   |   |   |   |   | A | B |
//   +---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
//            First virtual page             |       Second virtual page
//                                           |
//                                           |
//
//                                     Virtual memory
// ---------------------------------------------------------------------------------------
//                                    Physical memory
//
//                      +---+---+---+---+---+---+---+---+---+---+
//                      | C | D |   |   |   |   |   |   | A | B |
//                      +---+---+---+---+---+---+---+---+---+---+
//
// Like that, the elements stored in the buffer are always laid out in contiguous virtual memory,
// so making a slice out of them does not require any copies.
//
// The `L` const generic determines the maximum number of `iovec` elements the queue should hold
// at any point in time. The actual capacity of the queue may differ and will depend on the host
// page size.
//
// ```Rust
// pub struct iovec {
//    pub iov_base: *mut ::c_void,
//    pub iov_len: ::size_t,
// }
// ```

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct IovDeque<const L: u16> {
    pub iov: *mut iovec,
    pub start: u16,
    pub len: u16,
    pub capacity: u16,
}

// SAFETY: This is `Send`. We hold sole ownership of the underlying buffer.
#[cfg(target_os = "linux")]
unsafe impl<const L: u16> Send for IovDeque<L> {}

#[cfg(target_os = "linux")]
impl<const L: u16> IovDeque<L> {
    /// Create a sealed memfd of `pages_bytes` bytes, double-mapped below.
    ///
    /// Uses raw `libc` so the `memfd` crate is not pulled in. Sealing keeps the
    /// backing file from being resized underneath the `MAP_SHARED` mappings.
    fn create_memfd(pages_bytes: usize) -> Result<libc::c_int, IovDequeError> {
        // SAFETY: the name is a NUL-terminated string literal; flags are valid.
        let mfd = unsafe {
            libc::memfd_create(
                c"iov_deque".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if mfd < 0 {
            return Err(IovDequeError::Memfd(std::io::Error::last_os_error()));
        }

        // Resize to system page size.
        // SAFETY: `mfd` is a valid open file descriptor.
        if unsafe { libc::ftruncate(mfd, pages_bytes.try_into().unwrap()) } < 0 {
            return Err(IovDequeError::MemfdResize(std::io::Error::last_os_error()));
        }

        // Add seals to prevent further resizing.
        // SAFETY: `mfd` is a valid open file descriptor.
        if unsafe {
            libc::fcntl(
                mfd,
                libc::F_ADD_SEALS,
                libc::F_SEAL_SHRINK | libc::F_SEAL_GROW,
            )
        } < 0
        {
            return Err(IovDequeError::Memfd(std::io::Error::last_os_error()));
        }

        // Prevent further sealing changes.
        // SAFETY: `mfd` is a valid open file descriptor.
        if unsafe { libc::fcntl(mfd, libc::F_ADD_SEALS, libc::F_SEAL_SEAL) } < 0 {
            return Err(IovDequeError::Memfd(std::io::Error::last_os_error()));
        }

        Ok(mfd)
    }

    /// A safe wrapper on top of libc's `mmap` system call
    ///
    /// # Safety: Callers need to make sure that the arguments to `mmap` are valid
    unsafe fn mmap(
        addr: *mut c_void,
        len: size_t,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: off_t,
    ) -> Result<*mut c_void, IovDequeError> {
        // SAFETY: caller should ensure the parameters are valid
        let ptr = unsafe { libc::mmap(addr, len, prot, flags, fd, offset) };
        if ptr == libc::MAP_FAILED {
            return Err(IovDequeError::Mmap(std::io::Error::last_os_error()));
        }

        Ok(ptr)
    }

    /// Allocate memory for our ring buffer
    ///
    /// This will allocate 2 * `pages_bytes` bytes of virtual memory.
    fn allocate_ring_buffer_memory(pages_bytes: usize) -> Result<*mut c_void, IovDequeError> {
        // SAFETY: We are calling the system call with valid arguments
        unsafe {
            Self::mmap(
                std::ptr::null_mut(),
                pages_bytes * 2,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        }
    }

    /// Calculate a number of bytes in full pages required for
    /// the type to operate.
    fn pages_bytes() -> usize {
        let host_page_size = host_page_size();
        let bytes = L as usize * std::mem::size_of::<iovec>();
        let num_host_pages = bytes.div_ceil(host_page_size);
        num_host_pages * host_page_size
    }

    /// Create a new [`IovDeque`] that can hold memory described by a single VirtIO queue.
    pub fn new() -> Result<Self, IovDequeError> {
        let pages_bytes = Self::pages_bytes();
        let capacity = pages_bytes / std::mem::size_of::<iovec>();
        let capacity: u16 = capacity.try_into().unwrap();
        assert!(
            L <= capacity,
            "Actual capacity {} is smaller than requested capacity {}",
            capacity,
            L
        );

        let memfd = Self::create_memfd(pages_bytes)?;
        let raw_memfd = memfd;
        let buffer = Self::allocate_ring_buffer_memory(pages_bytes)?;

        // Map the first page of virtual memory to the physical page described by the memfd object
        // SAFETY: We are calling the system call with valid arguments
        let _ = unsafe {
            Self::mmap(
                buffer,
                pages_bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_FIXED,
                raw_memfd,
                0,
            )
        }?;

        // Map the second page of virtual memory to the physical page described by the memfd object
        //
        // SAFETY: This is safe because:
        // * Both `buffer` and the result of `buffer.add(pages_bytes)` are within bounds of the
        //   allocation we got from `Self::allocate_ring_buffer_memory`.
        // * The resulting pointer is the beginning of the second page of our allocation, so it
        //   doesn't wrap around the address space.
        let next_page = unsafe { buffer.add(pages_bytes) };

        // SAFETY: We are calling the system call with valid arguments
        let _ = unsafe {
            Self::mmap(
                next_page,
                pages_bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_FIXED,
                raw_memfd,
                0,
            )
        }?;

        // The `MAP_SHARED` mappings hold the pages alive; the file descriptor is
        // no longer needed once both halves are mapped.
        // SAFETY: `raw_memfd` is a valid open file descriptor.
        unsafe { libc::close(raw_memfd) };

        Ok(Self {
            iov: buffer.cast(),
            start: 0,
            len: 0,
            capacity,
        })
    }

    /// Returns the number of `iovec` objects currently in the [`IovDeque`]
    #[inline(always)]
    pub fn len(&self) -> u16 {
        self.len
    }

    /// Returns `true` if the [`IovDeque`] is full, `false` otherwise
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.len() == L
    }

    /// Resets the queue, dropping all its elements.
    #[inline(always)]
    pub fn clear(&mut self) {
        self.start = 0;
        self.len = 0;
    }

    /// Adds an `iovec` in the ring buffer.
    ///
    /// Returns an `IovDequeError::Full` error if the buffer is full.
    pub fn push_back(&mut self, iov: iovec) {
        // This should NEVER happen, since our ring buffer is as big as the maximum queue size.
        // We also check for the sanity of the VirtIO queues, in queue.rs, which means that if we
        // ever try to add something in a full ring buffer, there is an internal bug in the device
        // emulation logic. Panic here because the device is hopelessly broken.
        assert!(
            !self.is_full(),
            "The number of `iovec` objects is bigger than the available space"
        );

        // SAFETY: self.iov is a valid pointer and `self.start + self.len` is within range (we
        // asserted before that the buffer is not full).
        unsafe {
            self.iov
                .add((self.start + self.len) as usize)
                .write_volatile(iov)
        };
        self.len += 1;
    }

    /// Pops the first `nr_iovecs` iovecs from the front of the buffer.
    ///
    /// This will panic if we are asked
    /// to pop more iovecs than what is currently available in the buffer.
    pub fn pop_front(&mut self, nr_iovecs: u16) {
        assert!(
            self.len() >= nr_iovecs,
            "Internal bug! Trying to drop more iovec objects than what is available"
        );

        self.start += nr_iovecs;
        self.len -= nr_iovecs;
        if self.capacity <= self.start {
            self.start -= self.capacity;
        }
    }

    /// Pops the first `nr_iovecs` iovecs from the back of the buffer.
    ///
    /// This will panic if we are asked
    /// to pop more iovecs than what is currently available in the buffer.
    pub fn pop_back(&mut self, nr_iovecs: u16) {
        assert!(
            self.len() >= nr_iovecs,
            "Internal bug! Trying to drop more iovec objects than what is available"
        );

        self.len -= nr_iovecs;
    }

    /// Get a slice of the iovec objects currently in the buffer.
    pub fn as_slice(&self) -> &[iovec] {
        // SAFETY: Here we create a slice out of the existing elements in the buffer (not the whole
        // allocated memory). That means that we can:
        // * We can read `self.len * mem::size_of::<iovec>()` bytes out of the memory range we are
        //   returning.
        // * `self.iov.add(self.start.into())` is a non-null pointer and aligned.
        // * The underlying memory comes from a single allocation.
        // * The returning pointer points to `self.len` consecutive initialized `iovec` objects.
        // * We are only accessing the underlying memory through the returned slice. Since we are
        //   returning a slice of only the existing pushed elements the slice does not contain any
        //   aliasing references.
        // * The slice can be up to 1 page long which is smaller than `isize::MAX`.
        unsafe {
            let slice_start = self.iov.add(self.start.into());
            std::slice::from_raw_parts(slice_start, self.len.into())
        }
    }

    /// Get a mutable slice of the iovec objects currently in the buffer.
    pub fn as_mut_slice(&mut self) -> &mut [iovec] {
        // SAFETY: Here we create a slice out of the existing elements in the buffer (not the whole
        // allocated memory). That means that we can:
        // * We can read/write `self.len * mem::size_of::<iovec>()` bytes out of the memory range we
        //   are returning.
        // * The underlying memory comes from a single allocation.
        // * `self.iov.add(self.start.into())` is a non-null pointer and aligned
        // * The returning pointer points to `self.len` consecutive initialized `iovec` objects.
        // * We are only accessing the underlying memory through the returned slice. Since we are
        //   returning a slice of only the existing pushed elements the slice does not contain any
        //   aliasing references.
        // * The slice can be up to 1 page long which is smaller than `isize::MAX`.
        unsafe {
            let slice_start = self.iov.add(self.start.into());
            std::slice::from_raw_parts_mut(slice_start, self.len.into())
        }
    }
}

#[cfg(target_os = "linux")]
impl<const L: u16> Drop for IovDeque<L> {
    fn drop(&mut self) {
        let pages_bytes = Self::pages_bytes();
        // SAFETY: We are passing an address that we got from a previous allocation of `2 *
        // pages_bytes` by calling mmap
        let _ = unsafe { libc::munmap(self.iov.cast(), 2 * pages_bytes) };
    }
}

/// Windows has no `memfd_create`, and no way to map the same pages twice at adjacent addresses
/// without a section object. The ring is therefore a `Vec` of `2 * L` iovecs in which the live
/// elements are `buf[start..start + len]`. `start` is kept below `L`, so the live range always
/// fits in `buf` and is contiguous; when `start` would reach `L`, the live elements are moved back
/// to the front (at most `L` copies per wrap).
#[cfg(windows)]
#[derive(Debug)]
pub struct IovDeque<const L: u16> {
    buf: Vec<iovec>,
    start: u16,
    len: u16,
}

// SAFETY: This is `Send`. We hold sole ownership of the underlying buffer.
#[cfg(windows)]
unsafe impl<const L: u16> Send for IovDeque<L> {}

#[cfg(windows)]
impl<const L: u16> IovDeque<L> {
    /// Create a new [`IovDeque`] that can hold memory described by a single VirtIO queue.
    pub fn new() -> Result<Self, IovDequeError> {
        Ok(Self {
            buf: vec![
                iovec {
                    iov_base: std::ptr::null_mut(),
                    iov_len: 0,
                };
                2 * usize::from(L)
            ],
            start: 0,
            len: 0,
        })
    }

    /// Returns the number of `iovec` objects currently in the [`IovDeque`]
    #[inline(always)]
    pub fn len(&self) -> u16 {
        self.len
    }

    /// Returns `true` if the [`IovDeque`] is full, `false` otherwise
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.len() == L
    }

    /// Resets the queue, dropping all its elements.
    #[inline(always)]
    pub fn clear(&mut self) {
        self.start = 0;
        self.len = 0;
    }

    /// The live elements, as a range of indices into `buf`.
    #[inline(always)]
    fn live(&self) -> std::ops::Range<usize> {
        let start = usize::from(self.start);
        start..start + usize::from(self.len)
    }

    /// Adds an `iovec` in the ring buffer.
    ///
    /// Panics if the buffer is full.
    pub fn push_back(&mut self, iov: iovec) {
        // Same contract as the Linux implementation: pushing into a full ring is an internal bug.
        assert!(
            !self.is_full(),
            "The number of `iovec` objects is bigger than the available space"
        );

        // `start < L` and `len < L` here, so the index is below `2 * L`.
        self.buf[usize::from(self.start) + usize::from(self.len)] = iov;
        self.len += 1;
    }

    /// Pops the first `nr_iovecs` iovecs from the front of the buffer.
    ///
    /// This will panic if we are asked
    /// to pop more iovecs than what is currently available in the buffer.
    pub fn pop_front(&mut self, nr_iovecs: u16) {
        assert!(
            self.len() >= nr_iovecs,
            "Internal bug! Trying to drop more iovec objects than what is available"
        );

        self.start += nr_iovecs;
        self.len -= nr_iovecs;
        if self.start >= L {
            // Move the live elements back to the front so that `start < L` again.
            let live = self.live();
            self.buf.copy_within(live, 0);
            self.start = 0;
        }
    }

    /// Pops the first `nr_iovecs` iovecs from the back of the buffer.
    ///
    /// This will panic if we are asked
    /// to pop more iovecs than what is currently available in the buffer.
    pub fn pop_back(&mut self, nr_iovecs: u16) {
        assert!(
            self.len() >= nr_iovecs,
            "Internal bug! Trying to drop more iovec objects than what is available"
        );

        self.len -= nr_iovecs;
    }

    /// Get a slice of the iovec objects currently in the buffer.
    pub fn as_slice(&self) -> &[iovec] {
        &self.buf[self.live()]
    }

    /// Get a mutable slice of the iovec objects currently in the buffer.
    pub fn as_mut_slice(&mut self) -> &mut [iovec] {
        let live = self.live();
        &mut self.buf[live]
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::ffi::c_void;

    use crate::pal::iovec;

    type IovDeque = super::IovDeque<256>;

    fn make_iovec(id: u16, len: usize) -> iovec {
        iovec {
            iov_base: id as *mut c_void,
            iov_len: len,
        }
    }

    fn assert_iov_eq(actual: &iovec, expected: &iovec) {
        assert_eq!(actual.iov_len, expected.iov_len);
        assert_eq!(actual.iov_base, expected.iov_base);
    }

    #[test]
    fn new_is_empty() {
        let deque = IovDeque::new().unwrap();
        assert_eq!(deque.len(), 0);
        assert!(!deque.is_full());
    }

    #[test]
    fn push_pop_preserves_order() {
        let mut deque = IovDeque::new().unwrap();
        let iovs: Vec<iovec> = (0u16..4).map(|i| make_iovec(i, i as usize)).collect();
        for iov in &iovs {
            deque.push_back(*iov);
        }
        assert_eq!(deque.len(), 4);
        deque
            .as_slice()
            .iter()
            .zip(iovs.iter())
            .for_each(|(actual, expected)| assert_iov_eq(actual, expected));

        deque.pop_front(2);
        assert_eq!(deque.len(), 2);
        deque
            .as_slice()
            .iter()
            .zip(iovs[2..].iter())
            .for_each(|(actual, expected)| assert_iov_eq(actual, expected));
    }

    #[test]
    fn pop_back_drops_from_tail() {
        let mut deque = IovDeque::new().unwrap();
        for i in 0u16..4 {
            deque.push_back(make_iovec(i, 1));
        }
        deque.pop_back(1);
        assert_eq!(deque.len(), 3);
        assert_eq!(deque.as_slice()[0].iov_base as usize, 0);
        assert_eq!(deque.as_slice()[2].iov_base as usize, 2);
    }

    #[test]
    fn fill_and_drain_to_empty() {
        let mut deque = IovDeque::new().unwrap();
        for i in 0u16..256 {
            deque.push_back(make_iovec(i, 1));
        }
        assert!(deque.is_full());
        for _ in 0..256 {
            deque.pop_front(1);
        }
        assert!(deque.as_slice().is_empty());
    }

    #[test]
    fn as_mut_slice_round_trip() {
        let mut deque = IovDeque::new().unwrap();
        deque.push_back(make_iovec(0, 100));
        deque.push_back(make_iovec(1, 200));
        for iov in deque.as_mut_slice() {
            iov.iov_len *= 2;
        }
        assert_eq!(deque.as_slice()[0].iov_len, 200);
        assert_eq!(deque.as_slice()[1].iov_len, 400);
    }

    #[test]
    #[should_panic]
    fn push_back_past_capacity_panics() {
        let mut deque = IovDeque::new().unwrap();
        for i in 0u16..256 {
            deque.push_back(make_iovec(i, 1));
        }
        deque.push_back(make_iovec(0, 1));
    }

    #[test]
    fn wrap_around_keeps_order() {
        fn assert_contents(deque: &IovDeque, expected: &VecDeque<u16>) {
            assert_eq!(usize::from(deque.len()), expected.len());
            let ids: Vec<u16> = deque
                .as_slice()
                .iter()
                .map(|iov| iov.iov_base as usize as u16)
                .collect();
            assert_eq!(ids, expected.iter().copied().collect::<Vec<u16>>());
        }

        let mut deque = IovDeque::new().unwrap();
        let mut expected = VecDeque::new();
        let mut next_id: u16 = 0;
        // Uneven push and pop batches move `start` across the end of the buffer many times; far
        // more than `2 * L` elements pass through the queue in total.
        for step in 0..2000usize {
            let to_push = (step * 7 % 97).min(256 - expected.len());
            for _ in 0..to_push {
                deque.push_back(make_iovec(next_id, 1));
                expected.push_back(next_id);
                next_id = next_id.wrapping_add(1);
            }
            assert_contents(&deque, &expected);

            let to_pop = (step * 13 % 89).min(expected.len());
            deque.pop_front(u16::try_from(to_pop).unwrap());
            for _ in 0..to_pop {
                expected.pop_front();
            }
            assert_contents(&deque, &expected);
        }
    }
}
