// Copyright 2020 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/devices/virtio/iovec.rs (trimmed: the
// snapshot-only `Serialize`/`Deserialize` on `ParsedDescriptorChain` and the
// kani proofs are dropped; `displaydoc` becomes explicit `thiserror` messages).

// Guest-memory boundary (see the allowlist in `lib.rs`): walking the guest's
// descriptor chains means building `iovec`s over guest addresses, taking
// `VolatileSlice`s from them, and asserting `Send` for the buffers a device
// thread reads while the vCPU writes.
#![allow(unsafe_code)]

use std::io::ErrorKind;

use std::ffi::c_void;

use crate::pal::iovec;
#[allow(non_camel_case_types)]
type size_t = usize;
use vm_memory::bitmap::Bitmap;
use vm_memory::{
    GuestMemoryBackend, GuestMemoryError, ReadVolatile, VolatileMemoryError, VolatileSlice,
    WriteVolatile,
};

use super::iov_deque::{IovDeque, IovDequeError};
use super::queue::FIRECRACKER_MAX_QUEUE_SIZE;
use crate::devices::virtio::queue::DescriptorChain;
use crate::vstate::memory::GuestMemoryMmap;

#[derive(Debug, thiserror::Error)]
pub enum IoVecError {
    /// Tried to create an `IoVec` from a write-only descriptor chain.
    #[error("tried to create an `IoVec` from a write-only descriptor chain")]
    WriteOnlyDescriptor,
    /// Tried to create an `IoVecMut` from a read-only descriptor chain.
    #[error("tried to create an `IoVecMut` from a read-only descriptor chain")]
    ReadOnlyDescriptor,
    /// Tried to create an `IoVec` or `IoVecMut` from a descriptor chain that was too large.
    #[error("tried to create an `IoVec` or `IoVecMut` from a descriptor chain that was too large")]
    OverflowedDescriptor,
    /// Tried to push to full IovDeque.
    #[error("tried to push to a full iov deque")]
    IovDequeOverflow,
    /// Guest memory error: {0}.
    #[error("guest memory error: {0}")]
    GuestMemory(#[from] GuestMemoryError),
    /// Error with underlying `IovDeque`: {0}.
    #[error("underlying iov deque error: {0}")]
    IovDeque(#[from] IovDequeError),
}

/// This is essentially a wrapper of a `Vec<libc::iovec>` which can be passed to `libc::writev`.
///
/// It describes a buffer passed to us by the guest that is scattered across multiple
/// memory regions. Additionally, this wrapper provides methods that allow reading arbitrary ranges
/// of data from that buffer.
#[derive(Debug, Default)]
pub struct IoVecBuffer {
    // container of the memory regions included in this IO vector
    vecs: Vec<iovec>,
    // Total length of the IoVecBuffer
    len: u32,
}

// SAFETY: `IoVecBuffer` doesn't allow for interior mutability and no shared ownership is possible
// as it doesn't implement clone
unsafe impl Send for IoVecBuffer {}

impl IoVecBuffer {
    /// Create an `IoVecBuffer` from a `DescriptorChain`
    ///
    /// # Safety
    ///
    /// The descriptor chain cannot be referencing the same memory location as another chain
    pub unsafe fn load_descriptor_chain(
        &mut self,
        mem: &GuestMemoryMmap,
        head: DescriptorChain,
    ) -> Result<(), IoVecError> {
        self.clear();

        let mut next_descriptor = Some(head);
        while let Some(desc) = next_descriptor {
            if desc.is_write_only() {
                return Err(IoVecError::WriteOnlyDescriptor);
            }

            // We use get_slice instead of `get_host_address` here in order to have the whole
            // range of the descriptor chain checked, i.e. [addr, addr + len) is a valid memory
            // region in the GuestMemoryMmap.
            let iov_base = mem
                .get_slice(desc.addr, desc.len as usize)?
                .ptr_guard_mut()
                .as_ptr()
                .cast::<c_void>();
            self.vecs.push(iovec {
                iov_base,
                iov_len: desc.len as size_t,
            });
            self.len = self
                .len
                .checked_add(desc.len)
                .ok_or(IoVecError::OverflowedDescriptor)?;

            next_descriptor = desc.next_descriptor();
        }

        Ok(())
    }

    /// Create an `IoVecBuffer` from a `DescriptorChain`
    ///
    /// # Safety
    ///
    /// The descriptor chain cannot be referencing the same memory location as another chain
    pub unsafe fn from_descriptor_chain(
        mem: &GuestMemoryMmap,
        head: DescriptorChain,
    ) -> Result<Self, IoVecError> {
        let mut new_buffer = Self::default();
        // SAFETY: descriptor chain cannot be referencing the same memory location as another chain
        unsafe {
            new_buffer.load_descriptor_chain(mem, head)?;
        }
        Ok(new_buffer)
    }

    /// Get the total length of the memory regions covered by this `IoVecBuffer`
    pub(crate) fn len(&self) -> u32 {
        self.len
    }

    /// Returns a pointer to the memory keeping the `iovec` structs
    pub fn as_iovec_ptr(&self) -> *const iovec {
        self.vecs.as_ptr()
    }

    /// Returns the length of the `iovec` array.
    pub fn iovec_count(&self) -> usize {
        self.vecs.len()
    }

    /// Clears the `iovec` array
    pub fn clear(&mut self) {
        self.vecs.clear();
        self.len = 0u32;
    }

    /// Reads a number of bytes from the `IoVecBuffer` starting at a given offset.
    ///
    /// This will try to fill `buf` reading bytes from the `IoVecBuffer` starting from
    /// the given offset.
    ///
    /// # Returns
    ///
    /// `Ok(())` if `buf` was filled by reading from this [`IoVecBuffer`],
    /// `Err(VolatileMemoryError::PartialBuffer)` if only part of `buf` could not be filled, and
    /// `Err(VolatileMemoryError::OutOfBounds)` if `offset >= self.len()`.
    pub fn read_exact_volatile_at(
        &self,
        mut buf: &mut [u8],
        offset: usize,
    ) -> Result<(), VolatileMemoryError> {
        if offset < self.len() as usize {
            let expected = buf.len();
            let bytes_read = self.read_volatile_at(&mut buf, offset, expected)?;

            if bytes_read != expected {
                return Err(VolatileMemoryError::PartialBuffer {
                    expected,
                    completed: bytes_read,
                });
            }

            Ok(())
        } else {
            // If `offset` is past size, there's nothing to read.
            Err(VolatileMemoryError::OutOfBounds { addr: offset })
        }
    }

    /// Reads up to `len` bytes from the `IoVecBuffer` starting at the given offset.
    ///
    /// This will try to write to the given [`WriteVolatile`].
    pub fn read_volatile_at<W: WriteVolatile>(
        &self,
        dst: &mut W,
        mut offset: usize,
        mut len: usize,
    ) -> Result<usize, VolatileMemoryError> {
        let mut total_bytes_read = 0;

        for iov in &self.vecs {
            if len == 0 {
                break;
            }

            if offset >= iov.iov_len {
                offset -= iov.iov_len;
                continue;
            }

            let mut slice =
                // SAFETY: the constructor IoVecBufferMut::from_descriptor_chain ensures that
                // all iovecs contained point towards valid ranges of guest memory
                unsafe { VolatileSlice::new(iov.iov_base.cast(), iov.iov_len).offset(offset)? };
            offset = 0;

            if slice.len() > len {
                slice = slice.subslice(0, len)?;
            }

            match loop {
                match dst.write_volatile(&slice) {
                    Err(VolatileMemoryError::IOError(err))
                        if err.kind() == ErrorKind::Interrupted => {}
                    result => break result,
                }
            } {
                Ok(bytes_read) => {
                    total_bytes_read += bytes_read;

                    if bytes_read < slice.len() {
                        break;
                    }
                    len -= bytes_read;
                }
                // exit successfully if we previously managed to write some bytes
                Err(_) if total_bytes_read > 0 => break,
                Err(err) => return Err(err),
            }
        }

        Ok(total_bytes_read)
    }
}

#[derive(Debug, Clone)]
pub struct ParsedDescriptorChain {
    pub head_index: u16,
    pub length: u32,
    pub nr_iovecs: u16,
}

/// This is essentially a wrapper of a `Vec<libc::iovec>` which can be passed to `libc::readv`.
///
/// It describes a write-only buffer passed to us by the guest that is scattered across multiple
/// memory regions. Additionally, this wrapper provides methods that allow reading arbitrary ranges
/// of data from that buffer.
/// `L` const generic value must be a multiple of 256 as required by the `IovDeque` requirements.
#[derive(Debug)]
pub struct IoVecBufferMut<const L: u16 = FIRECRACKER_MAX_QUEUE_SIZE> {
    // container of the memory regions included in this IO vector
    pub vecs: IovDeque<L>,
    // Total length of the IoVecBufferMut
    // We use `u32` here because we use this type in devices which
    // should not give us huge buffers. In any case this
    // value will not overflow as we explicitly check for this case.
    pub len: u32,
}

// SAFETY: `IoVecBufferMut` doesn't allow for interior mutability and no shared ownership is
// possible as it doesn't implement clone
unsafe impl<const L: u16> Send for IoVecBufferMut<L> {}

impl<const L: u16> IoVecBufferMut<L> {
    /// Append a `DescriptorChain` in this `IoVecBufferMut`
    ///
    /// # Safety
    ///
    /// The descriptor chain cannot be referencing the same memory location as another chain
    pub unsafe fn append_descriptor_chain(
        &mut self,
        mem: &GuestMemoryMmap,
        head: DescriptorChain,
    ) -> Result<ParsedDescriptorChain, IoVecError> {
        let head_index = head.index;
        let mut next_descriptor = Some(head);
        let mut length = 0u32;
        let mut nr_iovecs = 0u16;
        while let Some(desc) = next_descriptor {
            if !desc.is_write_only() {
                self.vecs.pop_back(nr_iovecs);
                return Err(IoVecError::ReadOnlyDescriptor);
            }

            // We use get_slice instead of `get_host_address` here in order to have the whole
            // range of the descriptor chain checked, i.e. [addr, addr + len) is a valid memory
            // region in the GuestMemoryMmap.
            let slice = mem
                .get_slice(desc.addr, desc.len as usize)
                .inspect_err(|_| {
                    self.vecs.pop_back(nr_iovecs);
                })?;
            // We need to mark the area of guest memory that will be mutated through this
            // IoVecBufferMut as dirty ahead of time, as we loose access to all
            // vm-memory related information after converting down to iovecs.
            slice.bitmap().mark_dirty(0, desc.len as usize);
            let iov_base = slice.ptr_guard_mut().as_ptr().cast::<c_void>();

            if self.vecs.is_full() {
                self.vecs.pop_back(nr_iovecs);
                return Err(IoVecError::IovDequeOverflow);
            }

            self.vecs.push_back(iovec {
                iov_base,
                iov_len: desc.len as size_t,
            });

            nr_iovecs += 1;
            length = length
                .checked_add(desc.len)
                .ok_or(IoVecError::OverflowedDescriptor)
                .inspect_err(|_| {
                    self.vecs.pop_back(nr_iovecs);
                })?;

            next_descriptor = desc.next_descriptor();
        }

        self.len = self.len.checked_add(length).ok_or_else(|| {
            self.vecs.pop_back(nr_iovecs);
            IoVecError::OverflowedDescriptor
        })?;

        Ok(ParsedDescriptorChain {
            head_index,
            length,
            nr_iovecs,
        })
    }

    /// Create an empty `IoVecBufferMut`.
    pub fn new() -> Result<Self, IovDequeError> {
        let vecs = IovDeque::new()?;
        Ok(Self { vecs, len: 0 })
    }

    /// Create an `IoVecBufferMut` from a `DescriptorChain`
    ///
    /// This will clear any previous `iovec` objects in the buffer and load the new
    /// [`DescriptorChain`].
    ///
    /// # Safety
    ///
    /// The descriptor chain cannot be referencing the same memory location as another chain
    pub unsafe fn load_descriptor_chain(
        &mut self,
        mem: &GuestMemoryMmap,
        head: DescriptorChain,
    ) -> Result<(), IoVecError> {
        self.clear();
        // SAFETY: descriptor chain cannot be referencing the same memory location as another chain
        let _ = unsafe { self.append_descriptor_chain(mem, head)? };
        Ok(())
    }

    /// Drop descriptor chain from the `IoVecBufferMut` front
    ///
    /// This will drop memory described by the `IoVecBufferMut` from the beginning.
    pub fn drop_chain_front(&mut self, parse_descriptor: &ParsedDescriptorChain) {
        self.vecs.pop_front(parse_descriptor.nr_iovecs);
        self.len -= parse_descriptor.length;
    }

    /// Drop descriptor chain from the `IoVecBufferMut` back
    ///
    /// This will drop memory described by the `IoVecBufferMut` from the beginning.
    pub fn drop_chain_back(&mut self, parse_descriptor: &ParsedDescriptorChain) {
        self.vecs.pop_back(parse_descriptor.nr_iovecs);
        self.len -= parse_descriptor.length;
    }

    /// Create an `IoVecBuffer` from a `DescriptorChain`
    ///
    /// # Safety
    ///
    /// The descriptor chain cannot be referencing the same memory location as another chain
    pub unsafe fn from_descriptor_chain(
        mem: &GuestMemoryMmap,
        head: DescriptorChain,
    ) -> Result<Self, IoVecError> {
        let mut new_buffer = Self::new()?;
        // SAFETY: descriptor chain cannot be referencing the same memory location as another chain
        unsafe {
            new_buffer.load_descriptor_chain(mem, head)?;
        }
        Ok(new_buffer)
    }

    /// Get the total length of the memory regions covered by this `IoVecBuffer`
    #[inline(always)]
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Returns true if buffer is empty.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns a pointer to the memory keeping the `iovec` structs
    pub fn as_iovec_mut_slice(&mut self) -> &mut [iovec] {
        self.vecs.as_mut_slice()
    }

    /// Clears the `iovec` array
    pub fn clear(&mut self) {
        self.vecs.clear();
        self.len = 0;
    }

    /// Writes a number of bytes into the `IoVecBufferMut` starting at a given offset.
    ///
    /// This will try to fill `IoVecBufferMut` writing bytes from the `buf` starting from
    /// the given offset. It will write as many bytes from `buf` as they fit inside the
    /// `IoVecBufferMut` starting from `offset`.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the entire contents of `buf` could be written to this [`IoVecBufferMut`],
    /// `Err(VolatileMemoryError::PartialBuffer)` if only part of `buf` could be transferred, and
    /// `Err(VolatileMemoryError::OutOfBounds)` if `offset >= self.len()`.
    pub fn write_all_volatile_at(
        &mut self,
        mut buf: &[u8],
        offset: usize,
    ) -> Result<(), VolatileMemoryError> {
        if offset < self.len() as usize {
            let expected = buf.len();
            let bytes_written = self.write_volatile_at(&mut buf, offset, expected)?;

            if bytes_written != expected {
                return Err(VolatileMemoryError::PartialBuffer {
                    expected,
                    completed: bytes_written,
                });
            }

            Ok(())
        } else {
            // We cannot write past the end of the `IoVecBufferMut`.
            Err(VolatileMemoryError::OutOfBounds { addr: offset })
        }
    }

    /// Writes up to `len` bytes into the `IoVecBuffer` starting at the given offset.
    ///
    /// This will try to write to the given [`WriteVolatile`].
    pub fn write_volatile_at<W: ReadVolatile>(
        &mut self,
        src: &mut W,
        mut offset: usize,
        mut len: usize,
    ) -> Result<usize, VolatileMemoryError> {
        let mut total_bytes_read = 0;

        for iov in self.vecs.as_slice() {
            if len == 0 {
                break;
            }

            if offset >= iov.iov_len {
                offset -= iov.iov_len;
                continue;
            }

            let mut slice =
                // SAFETY: the constructor IoVecBufferMut::from_descriptor_chain ensures that
                // all iovecs contained point towards valid ranges of guest memory
                unsafe { VolatileSlice::new(iov.iov_base.cast(), iov.iov_len).offset(offset)? };
            offset = 0;

            if slice.len() > len {
                slice = slice.subslice(0, len)?;
            }

            match loop {
                match src.read_volatile(&mut slice) {
                    Err(VolatileMemoryError::IOError(err))
                        if err.kind() == ErrorKind::Interrupted => {}
                    result => break result,
                }
            } {
                Ok(bytes_read) => {
                    total_bytes_read += bytes_read;

                    if bytes_read < slice.len() {
                        break;
                    }
                    len -= bytes_read;
                }
                // exit successfully if we previously managed to read some bytes
                Err(_) if total_bytes_read > 0 => break,
                Err(err) => return Err(err),
            }
        }

        Ok(total_bytes_read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::virtio::queue::{FIRECRACKER_MAX_QUEUE_SIZE, VIRTQ_DESC_F_WRITE};
    use crate::devices::virtio::test_utils::{VirtQueue, default_mem};
    use crate::vstate::memory::{Bytes, GuestAddress};

    #[test]
    fn iovec_buffer_rejects_write_only_descriptor() {
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        vq.set_desc(0, 0x8000, 0x100, VIRTQ_DESC_F_WRITE, 0);
        let mut q = vq.create_queue();
        vq.publish(0);
        let head = q.pop().unwrap().unwrap();

        // SAFETY: the descriptor chain is not aliased.
        let result = unsafe { IoVecBuffer::from_descriptor_chain(&mem, head) };
        assert!(matches!(result, Err(IoVecError::WriteOnlyDescriptor)));
    }

    #[test]
    fn iovec_buffer_read_exact_round_trip() {
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        let data = b"hello vsock";
        mem.write_slice(data, GuestAddress(0x8000)).unwrap();
        vq.set_desc(0, 0x8000, data.len() as u32, 0, 0);
        let mut q = vq.create_queue();
        vq.publish(0);
        let head = q.pop().unwrap().unwrap();

        // SAFETY: exclusive ownership of the guest range.
        let buffer = unsafe { IoVecBuffer::from_descriptor_chain(&mem, head) }.unwrap();
        assert_eq!(buffer.len(), data.len() as u32);
        let mut out = [0u8; 11];
        buffer.read_exact_volatile_at(&mut out, 0).unwrap();
        assert_eq!(&out, data);
    }

    #[test]
    fn iovec_buffer_mut_writes_then_reads_back() {
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        vq.set_desc(0, 0x8000, 0x100, VIRTQ_DESC_F_WRITE, 0);
        let mut q = vq.create_queue();
        vq.publish(0);
        let head = q.pop().unwrap().unwrap();

        // SAFETY: exclusive ownership of the guest range.
        let mut buffer = unsafe {
            IoVecBufferMut::<FIRECRACKER_MAX_QUEUE_SIZE>::from_descriptor_chain(&mem, head)
        }
        .unwrap();
        assert_eq!(buffer.len(), 0x100);
        let src = [0xabu8; 16];
        buffer.write_all_volatile_at(&src, 0).unwrap();
        let mut out = [0u8; 16];
        mem.read_slice(&mut out, GuestAddress(0x8000)).unwrap();
        assert_eq!(out, src);
    }
}
