// Copyright 2020 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/devices/virtio/block/virtio/request.rs
// (upstream commit fd9fc7a9f164b73002d22065d17c150d8c262d91). The parsers
// (`RequestHeader::read_from`, `Request::parse`, `Request::validate_discard_ranges`)
// and the status-byte protocol (`Status`, `PendingRequest`, `FinishedRequest`)
// are upstream's, unchanged but for the adaptations this port needs:
//
//   * the request header and the discard range are read as little-endian
//     `zerocopy` fields instead of upstream's platform-endian `ByteValued`
//     PODs, so no `unsafe impl` is needed (the port's safe-Rust policy) and a
//     big-endian host cannot mis-parse the guest's buffers;
//   * the metrics, the rate limiter, the async engine and the
//     `ProcessingResult`/`PendingRequest` submission split are dropped: this
//     device executes every request synchronously, so a request is executed
//     and finished in one call;
//   * `DiskProperties` (the blocking file) replaces the `FileEngine`;
//   * `error!`/`block_metrics.*` become `eprintln!`/nothing.
//
// Everything that decides whether a request is legal is untouched: the
// descriptor-direction checks, the sector-length and discaer-range bounds, the
// `checked_add`/`checked_mul` guards and the exact error variants each
// malformed input produces.

use std::convert::From;
use std::io;
use std::mem::size_of;

use zerocopy::little_endian::{U32, U64};
use zerocopy::{FromBytes, Immutable, IntoBytes};

use super::VirtioBlockError;
use super::device::DiskProperties;
use super::{MAX_DISCARD_SECTORS, SECTOR_SHIFT, SECTOR_SIZE};
use crate::devices::virtio::queue::DescriptorChain;
use crate::vstate::memory::{Bytes, GuestAddress, GuestMemoryMmap};

/// `VIRTIO_BLK_T_IN`: read the volume's sectors into the guest's buffer.
pub const VIRTIO_BLK_T_IN: u32 = 0;
/// `VIRTIO_BLK_T_OUT`: write the guest's buffer into the volume's sectors.
pub const VIRTIO_BLK_T_OUT: u32 = 1;
/// `VIRTIO_BLK_T_FLUSH`: flush the volume's write cache.
pub const VIRTIO_BLK_T_FLUSH: u32 = 4;
/// `VIRTIO_BLK_T_GET_ID`: read the device id into the guest's buffer.
pub const VIRTIO_BLK_T_GET_ID: u32 = 8;
/// `VIRTIO_BLK_T_DISCARD`: discard a range of sectors.
pub const VIRTIO_BLK_T_DISCARD: u32 = 11;
/// `VIRTIO_BLK_T_WRITE_ZEROES`: write zeroes to a range of sectors. The device
/// does not serve it, but a read-only volume refuses it as a write.
pub const VIRTIO_BLK_T_WRITE_ZEROES: u32 = 13;

/// `VIRTIO_BLK_S_OK`: the request was executed.
pub const VIRTIO_BLK_S_OK: u8 = 0;
/// `VIRTIO_BLK_S_IOERR`: the request failed.
pub const VIRTIO_BLK_S_IOERR: u8 = 1;
/// `VIRTIO_BLK_S_UNSUPP`: the request type is not supported by this device.
pub const VIRTIO_BLK_S_UNSUPP: u8 = 2;

/// `VIRTIO_BLK_ID_BYTES`: the length of the device id `GET_ID` returns.
pub const VIRTIO_BLK_ID_BYTES: u32 = 20;

/// Why executing a request failed.
///
/// These are the errors the device reports to the driver as
/// `VIRTIO_BLK_S_IOERR` (or `VIRTIO_BLK_S_UNSUPP`, for a request type it does
/// not serve); none of them is a VMM failure.
#[derive(Debug, thiserror::Error)]
pub enum IoErr {
    /// Writing the device id into the guest's buffer failed: {0}
    #[error("failed to write the device id: {0}")]
    GetId(#[from] vm_memory::GuestMemoryError),
    /// The driver asked for a request type this device does not serve: {0}
    #[error("unsupported request type {0}")]
    UnsupportedRequest(u32),
    /// The volume is read-only (`VIRTIO_BLK_F_RO`) and the request would write to it.
    #[error("the volume is read-only")]
    ReadOnly,
    /// The guest's buffer is not in guest memory: {0}
    #[error("the driver's buffer is not in guest memory: {0}")]
    GuestMemory(vm_memory::GuestMemoryError),
    /// The backing file could not be read, written or flushed: {0}
    #[error("the backing file could not be served: {0}")]
    File(#[from] io::Error),
    /// Only part of the requested data was transferred.
    #[error("only {completed} of {expected} bytes were transferred")]
    PartialTransfer {
        /// Bytes actually transferred.
        completed: u32,
        /// Bytes the request asked for.
        expected: u32,
    },
}

/// The request types of the virtio-blk protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestType {
    /// A read (`VIRTIO_BLK_T_IN`).
    In,
    /// A write (`VIRTIO_BLK_T_OUT`).
    Out,
    /// A discard (`VIRTIO_BLK_T_DISCARD`).
    Discard,
    /// A cache flush (`VIRTIO_BLK_T_FLUSH`).
    Flush,
    /// A device-id read (`VIRTIO_BLK_T_GET_ID`).
    GetDeviceID,
    /// Anything else, as the driver's number.
    Unsupported(u32),
}

impl From<u32> for RequestType {
    fn from(value: u32) -> Self {
        match value {
            VIRTIO_BLK_T_IN => RequestType::In,
            VIRTIO_BLK_T_OUT => RequestType::Out,
            VIRTIO_BLK_T_DISCARD => RequestType::Discard,
            VIRTIO_BLK_T_FLUSH => RequestType::Flush,
            VIRTIO_BLK_T_GET_ID => RequestType::GetDeviceID,
            t => RequestType::Unsupported(t),
        }
    }
}

/// What the device must return to the driver once a request is done: how many
/// bytes of the used-ring entry and how much data was written back.
#[derive(Debug)]
pub struct FinishedRequest {
    /// The `len` of the used-ring entry: the bytes the device wrote into the
    /// guest's buffers (the status byte plus, for a read, the data), or 0 when
    /// even the status byte could not be written.
    pub num_bytes_to_mem: u32,
    /// The head descriptor index the used-ring entry refers to.
    pub desc_idx: u16,
}

/// The outcome of executing a request, before the status byte is written.
#[derive(Debug)]
enum Status {
    Ok { num_bytes_to_mem: u32 },
    IoErr { num_bytes_to_mem: u32, err: IoErr },
    Unsupported { op: u32 },
}

impl Status {
    fn from_data(data_len: u32, transferred_data_len: u32, data_to_mem: bool) -> Status {
        let num_bytes_to_mem = match data_to_mem {
            true => transferred_data_len,
            false => 0,
        };

        match transferred_data_len == data_len {
            true => Status::Ok { num_bytes_to_mem },
            false => Status::IoErr {
                num_bytes_to_mem,
                err: IoErr::PartialTransfer {
                    completed: transferred_data_len,
                    expected: data_len,
                },
            },
        }
    }
}

/// A request that has been parsed and executed but whose status byte is not
/// written yet.
#[derive(Debug)]
pub struct PendingRequest {
    r#type: RequestType,
    data_len: u32,
    status_addr: GuestAddress,
    desc_idx: u16,
}

impl PendingRequest {
    /// Writes the status byte at the chain's status descriptor and returns the
    /// used-ring entry the device must publish.
    ///
    /// A status byte that cannot be written (the driver pointed the descriptor
    /// outside guest memory) is not a VMM failure: the entry is published with
    /// length 0, which is how upstream discards such a descriptor.
    fn write_status_and_finish(self, status: &Status, mem: &GuestMemoryMmap) -> FinishedRequest {
        let (num_bytes_to_mem, status_code) = match status {
            Status::Ok { num_bytes_to_mem } => (*num_bytes_to_mem, VIRTIO_BLK_S_OK),
            Status::IoErr {
                num_bytes_to_mem,
                err,
            } => {
                eprintln!(
                    "warning: virtio-block: a {} request failed: {err}",
                    self.kind()
                );
                (*num_bytes_to_mem, VIRTIO_BLK_S_IOERR)
            }
            Status::Unsupported { op } => {
                eprintln!("warning: virtio-block: unsupported request type {op}");
                (0, VIRTIO_BLK_S_UNSUPP)
            }
        };

        let num_bytes_to_mem = mem
            .write_obj(status_code, self.status_addr)
            .map(|()| {
                // Account for the status byte.
                num_bytes_to_mem + 1
            })
            .unwrap_or_else(|err| {
                eprintln!("warning: virtio-block: cannot write the request status: {err}");
                // If the status cannot be written, discard the descriptor.
                0
            });

        FinishedRequest {
            num_bytes_to_mem,
            desc_idx: self.desc_idx,
        }
    }

    /// The request type's name, for the device's diagnostics.
    fn kind(&self) -> &'static str {
        match self.r#type {
            RequestType::In => "read",
            RequestType::Out => "write",
            RequestType::Discard => "discard",
            RequestType::Flush => "flush",
            RequestType::GetDeviceID => "device-id",
            RequestType::Unsupported(_) => "unsupported",
        }
    }

    /// Turns the result of the file operation into a status byte and the
    /// used-ring entry.
    pub fn finish(self, mem: &GuestMemoryMmap, res: Result<u32, IoErr>) -> FinishedRequest {
        let status = match (res, self.r#type) {
            (Ok(transferred_data_len), RequestType::In) => {
                Status::from_data(self.data_len, transferred_data_len, true)
            }
            (Ok(transferred_data_len), RequestType::Out) => {
                Status::from_data(self.data_len, transferred_data_len, false)
            }
            (Ok(_), RequestType::Discard) => Status::Ok {
                num_bytes_to_mem: 0,
            },
            (Ok(_), RequestType::Flush) => Status::Ok {
                num_bytes_to_mem: 0,
            },
            (Ok(transferred_data_len), RequestType::GetDeviceID) => {
                Status::from_data(self.data_len, transferred_data_len, true)
            }
            (Err(IoErr::UnsupportedRequest(op)), _) | (_, RequestType::Unsupported(op)) => {
                Status::Unsupported { op }
            }
            (Err(err), _) => Status::IoErr {
                num_bytes_to_mem: 0,
                err,
            },
        };

        self.write_status_and_finish(&status, mem)
    }
}

/// The request header represents the mandatory fields of each block device request.
///
/// A request header contains the following fields:
///   * request_type: an u32 value mapping to a read, write or flush operation.
///   * reserved: 32 bits are reserved for future extensions of the Virtio Spec.
///   * sector: an u64 value representing the offset where a read/write is to occur.
///
/// The header simplifies reading the request from memory as all requests follow
/// the same memory layout.
#[derive(Debug, Copy, Clone, Default, Immutable, FromBytes, IntoBytes)]
#[repr(C)]
pub struct RequestHeader {
    request_type: U32,
    _reserved: U32,
    sector: U64,
}

// The header is 16 bytes of little-endian fields with no padding: the layout
// the driver writes (`struct virtio_blk_outhdr` plus the reserved word).
const _: () = assert!(size_of::<RequestHeader>() == 16);

impl RequestHeader {
    /// A header for `request_type` at `sector`.
    pub fn new(request_type: u32, sector: u64) -> RequestHeader {
        RequestHeader {
            request_type: request_type.into(),
            _reserved: 0.into(),
            sector: sector.into(),
        }
    }

    /// The request type the driver asked for.
    pub fn request_type(&self) -> u32 {
        self.request_type.get()
    }

    /// The sector the request starts at.
    pub fn sector(&self) -> u64 {
        self.sector.get()
    }

    /// Reads the request header from guest memory starting at `addr`.
    ///
    /// Virtio 1.0 specifies that the data is transmitted by the driver in
    /// little-endian format. The header's fields are little-endian
    /// (`zerocopy::little_endian`), so the read is correct on any host
    /// endianness; the 16 bytes are copied out of guest memory and validated
    /// as a whole, so an unaligned header address is not a problem (the driver
    /// is only required to align the descriptor table, not the header).
    fn read_from(memory: &GuestMemoryMmap, addr: GuestAddress) -> Result<Self, VirtioBlockError> {
        let mut bytes = [0u8; size_of::<RequestHeader>()];
        memory.read_slice(&mut bytes, addr)?;
        // The buffer is exactly `size_of::<RequestHeader>()` bytes, so this
        // cannot fail; a size mismatch would be a compile-time error.
        RequestHeader::read_from_bytes(&bytes).map_err(|_| VirtioBlockError::InvalidDataLength)
    }
}

/// One discard range, as the driver lays it out in the data descriptor
/// (`struct virtio_blk_discard_write_zeroes`, without the legacy padding).
#[derive(Debug, Copy, Clone, Default, Immutable, FromBytes, IntoBytes)]
#[repr(C)]
pub struct DiscardWriteZeroes {
    sector: U64,
    num_sectors: U32,
    flags: U32,
}

// SAFETY/LAYOUT: plain data, no padding: the size assertion is the one
// upstream makes (`DISCARD_RANGE_SIZE` is 16, as the virtio specification
// fixes it).
const DISCARD_RANGE_SIZE: u32 = 16;
const _: () = assert!(size_of::<DiscardWriteZeroes>() == DISCARD_RANGE_SIZE as usize);

/// A parsed block request: what the device has to do, with every field the
/// driver supplied already checked.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    /// The request type.
    pub r#type: RequestType,
    /// Length of the data descriptor's buffer.
    pub data_len: u32,
    /// Address of the status descriptor's byte.
    pub status_addr: GuestAddress,
    /// The sector the request starts at (byte offset 512 × this).
    sector: u64,
    /// Address of the data descriptor's buffer.
    data_addr: GuestAddress,
}

impl Request {
    /// Parses and validates one request from the driver's descriptor chain.
    ///
    /// The chain is: the header descriptor (read-only, the driver wrote the
    /// [`RequestHeader`]), an optional data descriptor (write-only for a read
    /// or a device-id request, read-only for a write or a discard; absent for
    /// a flush), and the status descriptor (write-only, at least one byte).
    /// Anything else is a [`VirtioBlockError`] and the chain is answered with
    /// an error status; nothing here can panic.
    pub fn parse(
        avail_desc: &DescriptorChain,
        mem: &GuestMemoryMmap,
        num_disk_sectors: u64,
    ) -> Result<Request, VirtioBlockError> {
        // The head contains the request type which MUST be readable.
        if avail_desc.is_write_only() {
            return Err(VirtioBlockError::UnexpectedWriteOnlyDescriptor);
        }

        let request_header = RequestHeader::read_from(mem, avail_desc.addr)?;
        let mut req = Request {
            r#type: RequestType::from(request_header.request_type()),
            sector: request_header.sector(),
            data_addr: GuestAddress(0),
            data_len: 0,
            status_addr: GuestAddress(0),
        };

        let data_desc;
        let status_desc;
        let desc = avail_desc
            .next_descriptor()
            .ok_or(VirtioBlockError::DescriptorChainTooShort)?;

        if !desc.has_next() {
            status_desc = desc;
            // Only flush requests are allowed to skip the data descriptor.
            if req.r#type != RequestType::Flush {
                return Err(VirtioBlockError::DescriptorChainTooShort);
            }
        } else {
            data_desc = desc;
            status_desc = data_desc
                .next_descriptor()
                .ok_or(VirtioBlockError::DescriptorChainTooShort)?;

            if data_desc.is_write_only()
                && (req.r#type == RequestType::Out || req.r#type == RequestType::Discard)
            {
                return Err(VirtioBlockError::UnexpectedWriteOnlyDescriptor);
            }
            if !data_desc.is_write_only()
                && (req.r#type == RequestType::In || req.r#type == RequestType::GetDeviceID)
            {
                return Err(VirtioBlockError::UnexpectedReadOnlyDescriptor);
            }

            req.data_addr = data_desc.addr;
            req.data_len = data_desc.len;
        }

        // Check request validity.
        match req.r#type {
            RequestType::In | RequestType::Out => {
                // Check that the data length is a multiple of 512 as specified
                // in the virtio standard.
                if !req.data_len.is_multiple_of(SECTOR_SIZE) {
                    return Err(VirtioBlockError::InvalidDataLength);
                }
                let top_sector = req
                    .sector
                    .checked_add(u64::from(req.data_len) >> SECTOR_SHIFT)
                    .ok_or(VirtioBlockError::InvalidOffset)?;
                if top_sector > num_disk_sectors {
                    return Err(VirtioBlockError::InvalidOffset);
                }
            }
            RequestType::GetDeviceID if req.data_len < VIRTIO_BLK_ID_BYTES => {
                return Err(VirtioBlockError::InvalidDataLength);
            }
            RequestType::Discard => {
                req.validate_discard_ranges(mem, num_disk_sectors)?;
            }
            _ => {}
        }

        // The status MUST always be writable.
        if !status_desc.is_write_only() {
            return Err(VirtioBlockError::UnexpectedReadOnlyDescriptor);
        }

        if status_desc.len < 1 {
            return Err(VirtioBlockError::DescriptorLengthTooSmall);
        }

        req.status_addr = status_desc.addr;

        Ok(req)
    }

    /// Validates a discard request's range and rewrites the request's
    /// `sector`/`data_len` to the byte offset and length it covers.
    fn validate_discard_ranges(
        &mut self,
        mem: &GuestMemoryMmap,
        num_disk_sectors: u64,
    ) -> Result<(), VirtioBlockError> {
        if self.data_len != DISCARD_RANGE_SIZE {
            return Err(VirtioBlockError::InvalidDataLength);
        }

        let mut bytes = [0u8; DISCARD_RANGE_SIZE as usize];
        mem.read_slice(&mut bytes, self.data_addr)?;
        let range: DiscardWriteZeroes = DiscardWriteZeroes::read_from_bytes(&bytes)
            .map_err(|_| VirtioBlockError::InvalidDataLength)?;

        let num_sectors = range.num_sectors.get();
        if num_sectors == 0 || num_sectors > MAX_DISCARD_SECTORS || range.flags.get() != 0 {
            return Err(VirtioBlockError::InvalidDataLength);
        }

        let top_sector = range
            .sector
            .get()
            .checked_add(u64::from(num_sectors))
            .ok_or(VirtioBlockError::InvalidOffset)?;

        if top_sector > num_disk_sectors {
            return Err(VirtioBlockError::InvalidOffset);
        }

        let byte_offset = range
            .sector
            .get()
            .checked_mul(u64::from(SECTOR_SIZE))
            .ok_or(VirtioBlockError::InvalidOffset)?;
        let byte_len = u64::from(num_sectors)
            .checked_mul(u64::from(SECTOR_SIZE))
            .ok_or(VirtioBlockError::InvalidDataLength)?;
        let byte_len = u32::try_from(byte_len).map_err(|_| VirtioBlockError::InvalidDataLength)?;

        self.sector = byte_offset;
        self.data_len = byte_len;

        Ok(())
    }

    /// Whether the request writes to the volume: a write, a discard or a
    /// write-zeroes.
    fn is_write_type(&self) -> bool {
        matches!(self.r#type, RequestType::Out | RequestType::Discard)
            || self.r#type == RequestType::Unsupported(VIRTIO_BLK_T_WRITE_ZEROES)
    }

    /// The byte offset of the request in the volume.
    fn offset(&self) -> u64 {
        self.sector << SECTOR_SHIFT
    }

    fn to_pending_request(&self, desc_idx: u16) -> PendingRequest {
        PendingRequest {
            r#type: self.r#type,
            data_len: self.data_len,
            status_addr: self.status_addr,
            desc_idx,
        }
    }

    /// Executes the request against `disk` and writes its status byte back
    /// into the guest's chain.
    ///
    /// `desc_idx` is the head descriptor's index, which the used-ring entry
    /// must repeat. The device serves no discard (`VIRTIO_BLK_F_DISCARD` is not
    /// offered), which is reported as `VIRTIO_BLK_S_UNSUPP`, exactly as
    /// upstream does for a device that does not support discard.
    pub(crate) fn process(
        self,
        disk: &mut DiskProperties,
        desc_idx: u16,
        mem: &GuestMemoryMmap,
    ) -> FinishedRequest {
        let pending = self.to_pending_request(desc_idx);
        // A read-only volume refuses every write-type request before the file
        // is touched. The status is written directly, so a write the device
        // does not serve (`WRITE_ZEROES`) is an I/O error, not "unsupported".
        if disk.is_read_only() && self.is_write_type() {
            let status = Status::IoErr {
                num_bytes_to_mem: 0,
                err: IoErr::ReadOnly,
            };
            return pending.write_status_and_finish(&status, mem);
        }
        let res = match self.r#type {
            RequestType::In => disk.read(self.offset(), mem, self.data_addr, self.data_len),
            RequestType::Out => disk.write(self.offset(), mem, self.data_addr, self.data_len),
            RequestType::Discard => Err(IoErr::UnsupportedRequest(VIRTIO_BLK_T_DISCARD)),
            RequestType::Flush => disk.flush(),
            RequestType::GetDeviceID => mem
                .write_slice(disk.image_id(), self.data_addr)
                .map(|()| VIRTIO_BLK_ID_BYTES)
                .map_err(IoErr::GetId),
            RequestType::Unsupported(_) => {
                return pending.finish(mem, Ok(0));
            }
        };

        pending.finish(mem, res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::virtio::block::test_utils::RequestDescriptorChain;
    use crate::devices::virtio::queue::{VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE};
    use crate::devices::virtio::test_utils::{VirtQueue, default_mem};
    use crate::vstate::memory::tests::single_region_mem;
    use vm_memory::GuestMemoryBackend;

    const NUM_DISK_SECTORS: u64 = 1024;

    #[test]
    fn test_read_request_header() {
        let mem = single_region_mem(0x1000);
        let addr = GuestAddress(0);
        let sector = 123_454_321;

        // Test that all supported request types are read correctly from memory.
        let supported_request_types = [
            VIRTIO_BLK_T_IN,
            VIRTIO_BLK_T_OUT,
            VIRTIO_BLK_T_DISCARD,
            VIRTIO_BLK_T_FLUSH,
            VIRTIO_BLK_T_GET_ID,
        ];

        for request_type in supported_request_types {
            let expected_header = RequestHeader::new(request_type, sector);
            mem.write_slice(expected_header.as_bytes(), addr).unwrap();

            let actual_header = RequestHeader::read_from(&mem, addr).unwrap();
            assert_eq!(actual_header.request_type(), expected_header.request_type());
            assert_eq!(actual_header.sector(), expected_header.sector());
        }

        // Test that trying to read a request header that goes outside of the
        // memory boundary fails.
        RequestHeader::read_from(&mem, GuestAddress(0x1000)).unwrap_err();
    }

    /// A request header is read from whatever the driver wrote, in little
    /// endianness, wherever in the descriptor's buffer it sits (the driver is
    /// not required to align the header, and this port must not assume it).
    #[test]
    fn a_header_is_read_from_an_unaligned_address() {
        let mem = default_mem();
        let addr = GuestAddress(0x1234);
        let header = RequestHeader::new(VIRTIO_BLK_T_OUT, 0x1122_3344_5566_7788);
        // Write the header's bytes at a deliberately 4-byte-misaligned address.
        mem.write_slice(header.as_bytes(), addr).unwrap();

        let read = RequestHeader::read_from(&mem, addr).unwrap();
        assert_eq!(read.request_type(), VIRTIO_BLK_T_OUT);
        assert_eq!(read.sector(), 0x1122_3344_5566_7788);
    }

    #[test]
    fn test_request_type_from() {
        assert_eq!(RequestType::from(VIRTIO_BLK_T_IN), RequestType::In);
        assert_eq!(RequestType::from(VIRTIO_BLK_T_OUT), RequestType::Out);
        assert_eq!(
            RequestType::from(VIRTIO_BLK_T_DISCARD),
            RequestType::Discard
        );
        assert_eq!(RequestType::from(VIRTIO_BLK_T_FLUSH), RequestType::Flush);
        assert_eq!(
            RequestType::from(VIRTIO_BLK_T_GET_ID),
            RequestType::GetDeviceID
        );
        assert_eq!(RequestType::from(42), RequestType::Unsupported(42));
    }

    impl RequestDescriptorChain<'_, '_> {
        fn check_parse_err(&self, _e: VirtioBlockError) {
            let mut q = self.create_queue();
            let memory = self.memory();

            assert!(matches!(
                Request::parse(&q.pop().unwrap().unwrap(), memory, NUM_DISK_SECTORS),
                Err(_e)
            ));
        }

        fn check_parse(&self, check_data: bool) {
            let mut q = self.create_queue();
            let memory = self.memory();
            let request =
                Request::parse(&q.pop().unwrap().unwrap(), memory, NUM_DISK_SECTORS).unwrap();
            let expected_header = self.header();

            assert_eq!(
                request.r#type,
                RequestType::from(expected_header.request_type())
            );
            if request.r#type == RequestType::Discard {
                let mut bytes = [0u8; DISCARD_RANGE_SIZE as usize];
                memory
                    .read_slice(&mut bytes, self.data_desc_addr())
                    .unwrap();
                let range: DiscardWriteZeroes =
                    DiscardWriteZeroes::read_from_bytes(&bytes).unwrap();
                assert_eq!(request.sector, range.sector.get() * u64::from(SECTOR_SIZE));
                assert_eq!(request.data_len, range.num_sectors.get() * SECTOR_SIZE);
            } else {
                assert_eq!(request.sector, expected_header.sector());
            }

            if check_data && request.r#type != RequestType::Discard {
                assert_eq!(request.data_addr.0, self.data_desc_addr().0);
                assert_eq!(request.data_len, self.data_desc_len());
            }

            assert_eq!(request.status_addr.0, self.status_desc_addr().0);
        }
    }

    #[test]
    fn test_parse_generic() {
        let mem = default_mem();
        let queue = VirtQueue::new(GuestAddress(0), &mem, 16);
        let chain = RequestDescriptorChain::new(&queue);
        let request_header = RequestHeader::new(100, 114);
        chain.set_header(request_header);

        // Write only request type descriptor.
        chain.set_header_flags(VIRTQ_DESC_F_WRITE);
        chain.check_parse_err(VirtioBlockError::UnexpectedWriteOnlyDescriptor);

        // Chain too short: no DATA_DESCRIPTOR.
        chain.set_header_flags(0);
        chain.check_parse_err(VirtioBlockError::DescriptorChainTooShort);

        // Chain too short: no status descriptor.
        chain.set_header_flags(VIRTQ_DESC_F_NEXT);
        chain.set_data_flags(0);
        chain.check_parse_err(VirtioBlockError::DescriptorChainTooShort);

        // Status descriptor not writable.
        chain.set_data_flags(VIRTQ_DESC_F_NEXT);
        chain.set_status_flags(0);
        chain.check_parse_err(VirtioBlockError::UnexpectedReadOnlyDescriptor);

        // Status descriptor too small.
        chain.set_status_flags(VIRTQ_DESC_F_WRITE);
        chain.set_status_len(0);
        chain.check_parse_err(VirtioBlockError::DescriptorLengthTooSmall);

        // Fix status descriptor length.
        chain.set_status_len(0x1000);

        // Invalid guest address for the status descriptor. Parsing will still succeed
        // as the operation that will fail happens when executing the request.
        chain.set_status_addr(mem.last_addr().0);
        chain.check_parse(true);

        // Fix status descriptor addr.
        chain.set_status_addr(0x3000);

        // Invalid guest address for the data descriptor. Parsing will still succeed
        // as the operation that will fail happens when executing the request.
        chain.set_data_addr(mem.last_addr().0);
        chain.check_parse(true);

        // Fix data descriptor addr.
        chain.set_data_addr(0x2000);
        chain.check_parse(true);
    }

    #[test]
    fn test_parse_in() {
        let mem = default_mem();
        let queue = VirtQueue::new(GuestAddress(0), &mem, 16);
        let chain = RequestDescriptorChain::new(&queue);

        let mut request_header = RequestHeader::new(VIRTIO_BLK_T_IN, 99);
        chain.set_header(request_header);

        // Read only data descriptor for IN.
        chain.set_data_flags(VIRTQ_DESC_F_NEXT);
        chain.check_parse_err(VirtioBlockError::UnexpectedReadOnlyDescriptor);

        // data_len is not multiple of 512 for IN.
        chain.set_data_flags(VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE);
        chain.set_data_len(513);
        chain.check_parse_err(VirtioBlockError::InvalidDataLength);

        // sector is too big.
        request_header = RequestHeader::new(VIRTIO_BLK_T_IN, NUM_DISK_SECTORS);
        chain.set_data_len(512);
        chain.set_header(request_header);
        chain.check_parse_err(VirtioBlockError::InvalidOffset);

        // Fix data descriptor.
        request_header = RequestHeader::new(VIRTIO_BLK_T_IN, NUM_DISK_SECTORS - 1);
        chain.set_header(request_header);
        chain.check_parse(true);
    }

    #[test]
    fn test_parse_out() {
        let mem = default_mem();
        let queue = VirtQueue::new(GuestAddress(0), &mem, 16);
        let chain = RequestDescriptorChain::new(&queue);

        let mut request_header = RequestHeader::new(VIRTIO_BLK_T_OUT, 100);
        chain.set_header(request_header);

        // Write only data descriptor for OUT.
        chain.set_data_flags(VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE);
        chain.check_parse_err(VirtioBlockError::UnexpectedWriteOnlyDescriptor);

        // data_len is not a multiple of 512 for OUT.
        chain.set_data_flags(VIRTQ_DESC_F_NEXT);
        chain.set_data_len(1000);
        chain.check_parse_err(VirtioBlockError::InvalidDataLength);

        // sector is too big.
        request_header = RequestHeader::new(VIRTIO_BLK_T_OUT, NUM_DISK_SECTORS - 1);
        chain.set_data_len(1024);
        chain.set_header(request_header);
        chain.check_parse_err(VirtioBlockError::InvalidOffset);

        // Fix header descriptor.
        request_header = RequestHeader::new(VIRTIO_BLK_T_OUT, NUM_DISK_SECTORS - 2);
        chain.set_header(request_header);
        chain.check_parse(true);
    }

    #[test]
    fn test_parse_flush() {
        let mem = default_mem();
        let queue = VirtQueue::new(GuestAddress(0), &mem, 16);
        let chain = RequestDescriptorChain::new(&queue);

        // Flush request with a data descriptor.
        chain.set_header(RequestHeader::new(VIRTIO_BLK_T_FLUSH, 50));
        chain.check_parse(true);

        // Flush request without a data descriptor: the status descriptor
        // follows the header directly.
        chain.set_header_next(2);
        chain.check_parse(false);
    }

    #[test]
    fn test_parse_discard() {
        let mem = default_mem();
        let queue = VirtQueue::new(GuestAddress(0), &mem, 16);
        let chain = RequestDescriptorChain::new(&queue);

        chain.set_header(RequestHeader::new(VIRTIO_BLK_T_DISCARD, 0));

        // Write only data descriptor for Discard.
        chain.set_data_flags(VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE);
        chain.check_parse_err(VirtioBlockError::UnexpectedWriteOnlyDescriptor);

        chain.set_data_flags(VIRTQ_DESC_F_NEXT);
        chain.set_data_len(15);
        chain.check_parse_err(VirtioBlockError::InvalidDataLength);

        let write_range = |range: DiscardWriteZeroes| {
            mem.write_slice(range.as_bytes(), chain.data_desc_addr())
                .unwrap();
        };

        chain.set_data_len(DISCARD_RANGE_SIZE);
        write_range(DiscardWriteZeroes {
            sector: 0.into(),
            num_sectors: 0.into(),
            flags: 0.into(),
        });
        chain.check_parse_err(VirtioBlockError::InvalidDataLength);

        write_range(DiscardWriteZeroes {
            sector: 0.into(),
            num_sectors: 1.into(),
            flags: 1.into(),
        });
        chain.check_parse_err(VirtioBlockError::InvalidDataLength);

        write_range(DiscardWriteZeroes {
            sector: 0.into(),
            num_sectors: (MAX_DISCARD_SECTORS + 1).into(),
            flags: 0.into(),
        });
        chain.check_parse_err(VirtioBlockError::InvalidDataLength);

        write_range(DiscardWriteZeroes {
            sector: (NUM_DISK_SECTORS - 1).into(),
            num_sectors: 2.into(),
            flags: 0.into(),
        });
        chain.check_parse_err(VirtioBlockError::InvalidOffset);

        write_range(DiscardWriteZeroes {
            sector: (NUM_DISK_SECTORS - 1).into(),
            num_sectors: 1.into(),
            flags: 0.into(),
        });
        chain.check_parse(true);
    }

    #[test]
    fn test_parse_get_id() {
        let mem = default_mem();
        let queue = VirtQueue::new(GuestAddress(0), &mem, 16);
        let chain = RequestDescriptorChain::new(&queue);

        chain.set_header(RequestHeader::new(VIRTIO_BLK_T_GET_ID, 15));

        // Read only data descriptor for GetDeviceId.
        chain.set_data_flags(VIRTQ_DESC_F_NEXT);
        chain.check_parse_err(VirtioBlockError::UnexpectedReadOnlyDescriptor);

        // data_len is < VIRTIO_BLK_ID_BYTES for GetDeviceID.
        chain.set_data_flags(VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE);
        chain.set_data_len(VIRTIO_BLK_ID_BYTES - 1);
        chain.check_parse_err(VirtioBlockError::InvalidDataLength);

        chain.set_data_len(VIRTIO_BLK_ID_BYTES);
        chain.check_parse(true);
    }
}
