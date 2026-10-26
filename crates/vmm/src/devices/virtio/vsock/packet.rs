// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/devices/virtio/vsock/packet.rs
// (trimmed: the test module is replaced with host-runnable round-trips that
// drive real guest memory instead of the event-manager test harness).

//! `VsockPacket` provides a thin wrapper over the buffers exchanged via virtio queues.
//! There are two components to a vsock packet, each using its own descriptor in a
//! virtio queue:
//! - the packet header; and
//! - the packet data/buffer.
//!
//! There is a 1:1 relation between descriptor chains and packets: the first (chain head) holds
//! the header, and an optional second descriptor holds the data. The second descriptor is only
//! present for data packets (VSOCK_OP_RW).
//!
//! `VsockPacket` wraps these two buffers and provides direct access to the data stored
//! in guest memory. This is done to avoid unnecessarily copying data from guest memory
//! to temporary buffers, before passing it on to the vsock backend.

// Guest-memory boundary (see the allowlist in `lib.rs`): the packet header is a
// `ByteValued` POD type (so it can be copied to and from guest memory), and
// `parse` loads the guest's descriptor chain through the `unsafe` chained-load
// entry points of `iovec`.
#![allow(unsafe_code)]

use std::fmt::Debug;

use vm_memory::volatile_memory::Error;
use vm_memory::{GuestMemoryError, ReadVolatile, WriteVolatile};

use super::{VsockError, defs};
use crate::devices::virtio::iovec::{IoVecBuffer, IoVecBufferMut};
use crate::devices::virtio::queue::DescriptorChain;
use crate::vstate::memory::{ByteValued, GuestMemoryMmap};

// The vsock packet header is defined by the C struct:
//
// ```C
//     le64 src_cid;
//     le64 dst_cid;
//     le32 src_port;
//     le32 dst_port;
//     le32 len;
//     le16 type;
//     le16 op;
//     le32 flags;
//     le32 buf_alloc;
//     le32 fwd_cnt;
// } __attribute__((packed));
// ```
// We create a rust structure that mirrors it.
// The mirroring struct is only used privately by `VsockPacket`, that offers getter and setter
// methods, for each struct field, that will also handle the correct endianess.

#[repr(C, packed)]
#[derive(Copy, Clone, Debug, Default)]
pub struct VsockPacketHeader {
    // Source CID.
    src_cid: u64,
    // Destination CID.
    dst_cid: u64,
    // Source port.
    src_port: u32,
    // Destination port.
    dst_port: u32,
    // Data length (in bytes) - may be 0, if there is no data buffer.
    len: u32,
    // Socket type. Currently, only connection-oriented streams are defined by the vsock protocol.
    type_: u16,
    // Operation ID - one of the VSOCK_OP_* values; e.g.
    // - VSOCK_OP_RW: a data packet;
    // - VSOCK_OP_REQUEST: connection request;
    // - VSOCK_OP_RST: forcefull connection termination;
    // etc (see `super::defs::uapi` for the full list).
    op: u16,
    // Additional options (flags) associated with the current operation (`op`).
    // Currently, only used with shutdown requests (VSOCK_OP_SHUTDOWN).
    flags: u32,
    // Size (in bytes) of the packet sender receive buffer (for the connection to which this packet
    // belongs).
    buf_alloc: u32,
    // Number of bytes the sender has received and consumed (for the connection to which this
    // packet belongs). For instance, for our Unix backend, this counter would be the total
    // number of bytes we have successfully written to a backing Unix socket.
    fwd_cnt: u32,
}

impl VsockPacketHeader {
    pub fn src_cid(&self) -> u64 {
        u64::from_le(self.src_cid)
    }

    pub fn set_src_cid(&mut self, cid: u64) -> &mut Self {
        self.src_cid = cid.to_le();
        self
    }

    pub fn dst_cid(&self) -> u64 {
        u64::from_le(self.dst_cid)
    }

    pub fn set_dst_cid(&mut self, cid: u64) -> &mut Self {
        self.dst_cid = cid.to_le();
        self
    }

    pub fn src_port(&self) -> u32 {
        u32::from_le(self.src_port)
    }

    pub fn set_src_port(&mut self, port: u32) -> &mut Self {
        self.src_port = port.to_le();
        self
    }

    pub fn dst_port(&self) -> u32 {
        u32::from_le(self.dst_port)
    }

    pub fn set_dst_port(&mut self, port: u32) -> &mut Self {
        self.dst_port = port.to_le();
        self
    }

    pub fn len(&self) -> u32 {
        u32::from_le(self.len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn set_len(&mut self, len: u32) -> &mut Self {
        self.len = len.to_le();
        self
    }

    pub fn type_(&self) -> u16 {
        u16::from_le(self.type_)
    }

    pub fn set_type(&mut self, type_: u16) -> &mut Self {
        self.type_ = type_.to_le();
        self
    }

    pub fn op(&self) -> u16 {
        u16::from_le(self.op)
    }

    pub fn set_op(&mut self, op: u16) -> &mut Self {
        self.op = op.to_le();
        self
    }

    pub fn flags(&self) -> u32 {
        u32::from_le(self.flags)
    }

    pub fn set_flags(&mut self, flags: u32) -> &mut Self {
        self.flags = flags.to_le();
        self
    }

    pub fn set_flag(&mut self, flag: u32) -> &mut Self {
        self.set_flags(self.flags() | flag);
        self
    }

    pub fn buf_alloc(&self) -> u32 {
        u32::from_le(self.buf_alloc)
    }

    pub fn set_buf_alloc(&mut self, buf_alloc: u32) -> &mut Self {
        self.buf_alloc = buf_alloc.to_le();
        self
    }

    pub fn fwd_cnt(&self) -> u32 {
        u32::from_le(self.fwd_cnt)
    }

    pub fn set_fwd_cnt(&mut self, fwd_cnt: u32) -> &mut Self {
        self.fwd_cnt = fwd_cnt.to_le();
        self
    }
}
/// The vsock packet header struct size (the struct is packed).
pub const VSOCK_PKT_HDR_SIZE: u32 = 44;

/// Where `count` payload bytes at `offset` start in a packet buffer of `len` bytes (the header first), or
/// `GuestMemoryBounds` when they do not fit. An offset past the payload is refused even with no bytes to copy, so the
/// sum below cannot overflow.
fn payload_at(len: u32, offset: u32, count: u32) -> Result<usize, VsockError> {
    let room = len.saturating_sub(VSOCK_PKT_HDR_SIZE);
    if offset > room || count > room - offset {
        return Err(VsockError::GuestMemoryBounds);
    }
    Ok((offset + VSOCK_PKT_HDR_SIZE) as usize)
}

// SAFETY: `VsockPacketHeader` is a POD and contains no padding.
unsafe impl ByteValued for VsockPacketHeader {}

// /// Struct describing a single vsock packet.
// ///
// /// Encapsulates the virtio descriptor chain containing the packet through the `IoVecBuffer[Mut]`
// /// abstractions.
#[derive(Debug, Default)]
pub struct VsockPacketTx {
    /// A copy of the vsock packet's 44-byte header, held in hypervisor memory
    /// to minimize the number of accesses to guest memory. Can be written back
    /// to geust memory using [`VsockPacket::commit_hdr`] (only for RX buffers).
    pub hdr: VsockPacketHeader,
    /// The raw buffer, as it is contained in guest memory (containing both
    /// header and payload)
    buffer: IoVecBuffer,
}

impl VsockPacketTx {
    /// Clear the packet buffer.
    pub fn clear(&mut self) {
        self.hdr = Default::default();
        self.buffer.clear();
    }

    /// Create the packet wrapper from a TX virtq chain head.
    ///
    /// ## Errors
    /// Returns
    /// - [`VsockError::UnreadableDescriptor`] if the provided descriptor chain contains any
    ///   descriptor not marked as writable.
    /// - [`VsockError::DescChainTooShortForHeader`] if the descriptor chain's total buffer length
    ///   is insufficient to hold the 44 byte vsock header
    /// - [`VsockError::InvalidPktLen`] if the contained vsock header describes a vsock packet whose
    ///   length would exceed [`defs::MAX_PKT_BUR_SIZE`].
    /// - [`VsockError::DescChainTooShortForPacket`] if the contained vsock header describes a vsock
    ///   packet whose length exceeds the descriptor chain's actual total buffer length.
    pub fn parse(
        &mut self,
        mem: &GuestMemoryMmap,
        chain: DescriptorChain,
    ) -> Result<(), VsockError> {
        // SAFETY: This descriptor chain is only loaded once
        // virtio requests are handled sequentially so no two IoVecBuffers
        // are live at the same time, meaning this has exclusive ownership over the memory
        unsafe { self.buffer.load_descriptor_chain(mem, chain)? };

        let mut hdr = VsockPacketHeader::default();
        match self.buffer.read_exact_volatile_at(hdr.as_mut_slice(), 0) {
            Ok(()) => (),
            Err(Error::PartialBuffer { completed, .. }) => {
                return Err(VsockError::DescChainTooShortForHeader(completed));
            }
            Err(err) => return Err(VsockError::GuestMemoryMmap(err.into())),
        }

        // Read the length through the accessor: it converts from the packet's
        // little-endian wire order, which the raw field does not.
        if hdr.len() > defs::MAX_PKT_BUF_SIZE {
            return Err(VsockError::InvalidPktLen(hdr.len()));
        }

        if hdr.len() > self.buffer.len() - VSOCK_PKT_HDR_SIZE {
            return Err(VsockError::DescChainTooShortForPacket(
                self.buffer.len(),
                hdr.len(),
            ));
        }
        self.hdr = hdr;
        Ok(())
    }

    pub fn write_from_offset_to<T: WriteVolatile + Debug>(
        &self,
        dst: &mut T,
        offset: u32,
        count: u32,
    ) -> Result<u32, VsockError> {
        let at = payload_at(self.buffer.len(), offset, count)?;
        self.buffer
            .read_volatile_at(dst, at, count as usize)
            .map_err(|err| VsockError::GuestMemoryMmap(GuestMemoryError::from(err)))
            .and_then(|read| read.try_into().map_err(|_| VsockError::DescChainOverflow))
    }

    /// Returns the total length of this [`VsockPacket`]'s buffer (e.g. the amount of data bytes
    /// contained in this packet).
    ///
    /// Return value will equal the total length of the underlying descriptor chain's buffers,
    /// minus the length of the vsock header.
    pub fn buf_size(&self) -> u32 {
        self.buffer.len() - VSOCK_PKT_HDR_SIZE
    }
}
/// Struct describing a single vsock packet.
///
/// Encapsulates the virtio descriptor chain containing the packet through the `IoVecBuffer[Mut]`
/// abstractions.
#[derive(Debug)]
pub struct VsockPacketRx {
    /// A copy of the vsock packet's 44-byte header, held in hypervisor memory
    /// to minimize the number of accesses to guest memory. Can be written back
    /// to geust memory using [`VsockPacket::commit_hdr`] (only for RX buffers).
    pub hdr: VsockPacketHeader,
    /// The raw buffer, as it is contained in guest memory (containing both
    /// header and payload)
    buffer: IoVecBufferMut,
}

impl VsockPacketRx {
    /// Clear the packet buffer.
    pub fn clear(&mut self) {
        self.hdr = Default::default();
        self.buffer.clear();
    }

    /// Creates new VsockPacketRx.
    pub fn new() -> Result<Self, VsockError> {
        let buffer = IoVecBufferMut::new().map_err(VsockError::IovDeque)?;
        Ok(Self {
            hdr: Default::default(),
            buffer,
        })
    }

    /// Create the packet wrapper from an RX virtq chain head.
    ///
    /// ## Errors
    /// Returns [`VsockError::DescChainTooShortForHeader`] if the descriptor chain's total buffer
    /// length is insufficient to hold the 44 byte vsock header
    pub fn parse(
        &mut self,
        mem: &GuestMemoryMmap,
        chain: DescriptorChain,
    ) -> Result<(), VsockError> {
        // SAFETY: This descriptor chain is only loaded once
        // virtio requests are handled sequentially so no two IoVecBuffers
        // are live at the same time, meaning this has exclusive ownership over the memory
        unsafe { self.buffer.load_descriptor_chain(mem, chain)? };
        if self.buffer.len() < VSOCK_PKT_HDR_SIZE {
            return Err(VsockError::DescChainTooShortForHeader(
                self.buffer.len() as usize
            ));
        }
        self.hdr = VsockPacketHeader::default();
        Ok(())
    }

    /// Writes the local copy of the packet header to the guest memory.
    ///
    /// ## Errors
    /// The function returns [`VsockError::UnwritableDescriptor`] if this [`VsockPacket`]
    /// contains a guest-to-host (TX) packet. It returned [`VsockError::InvalidPktLen`] if the
    /// packet's payload as described by this [`VsockPacket`] would exceed
    /// [`defs::MAX_PKT_BUF_SIZE`].
    pub fn commit_hdr(&mut self) -> Result<(), VsockError> {
        if self.hdr.len() > defs::MAX_PKT_BUF_SIZE {
            return Err(VsockError::InvalidPktLen(self.hdr.len()));
        }
        self.buffer
            .write_all_volatile_at(self.hdr.as_slice(), 0)
            .map_err(GuestMemoryError::from)
            .map_err(VsockError::GuestMemoryMmap)
    }

    /// Returns the total length of this [`VsockPacket`]'s buffer (e.g. the amount of data bytes
    /// contained in this packet).
    ///
    /// Return value will equal the total length of the underlying descriptor chain's buffers,
    /// minus the length of the vsock header.
    pub fn buf_size(&self) -> u32 {
        self.buffer.len() - VSOCK_PKT_HDR_SIZE
    }

    pub fn read_at_offset_from<T: ReadVolatile + Debug>(
        &mut self,
        src: &mut T,
        offset: u32,
        count: u32,
    ) -> Result<u32, VsockError> {
        let at = payload_at(self.buffer.len(), offset, count)?;
        self.buffer
            .write_volatile_at(src, at, count as usize)
            .map_err(|err| VsockError::GuestMemoryMmap(GuestMemoryError::from(err)))
            .and_then(|read| read.try_into().map_err(|_| VsockError::DescChainOverflow))
    }
}
#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{VSOCK_PKT_HDR_SIZE, VsockError, VsockPacketHeader, VsockPacketRx, VsockPacketTx};
    use crate::devices::virtio::queue::{VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE};
    use crate::devices::virtio::test_utils::{VirtQueue, default_mem};
    use crate::devices::virtio::vsock::defs;
    use crate::vstate::memory::{ByteValued, Bytes, GuestAddress};

    const HDR_ADDR: u64 = 0x8000;
    const DATA_ADDR: u64 = 0x9000;
    /// The data-descriptor size used by the assembly/offset tests.
    const PKT_BUF_SIZE: u32 = 4096;

    #[test]
    fn packet_header_size_is_44() {
        assert_eq!(
            VSOCK_PKT_HDR_SIZE as usize,
            std::mem::size_of::<VsockPacketHeader>()
        );
    }

    #[test]
    fn tx_packet_parses_header_and_payload() {
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);

        let mut hdr = VsockPacketHeader::default();
        hdr.set_src_cid(3)
            .set_dst_cid(defs::uapi::VSOCK_HOST_CID)
            .set_src_port(1000)
            .set_dst_port(2000)
            .set_len(6)
            .set_type(defs::uapi::VSOCK_TYPE_STREAM)
            .set_op(defs::uapi::VSOCK_OP_RW);
        mem.write_slice(hdr.as_slice(), GuestAddress(HDR_ADDR))
            .unwrap();
        mem.write_slice(b"hello!", GuestAddress(DATA_ADDR)).unwrap();

        // Header descriptor -> data descriptor, both read-only.
        vq.set_desc(0, HDR_ADDR, VSOCK_PKT_HDR_SIZE, VIRTQ_DESC_F_NEXT, 1);
        vq.set_desc(1, DATA_ADDR, 6, 0, 0);
        let mut q = vq.create_queue();
        vq.publish(0);
        let head = q.pop().unwrap().unwrap();

        let mut pkt = VsockPacketTx::default();
        pkt.parse(&mem, head).unwrap();
        assert_eq!(pkt.hdr.src_cid(), 3);
        assert_eq!(pkt.hdr.dst_cid(), defs::uapi::VSOCK_HOST_CID);
        assert_eq!(pkt.hdr.len(), 6);
        assert_eq!(pkt.buf_size(), 6);

        let mut payload = [0u8; 6];
        let mut cursor = Cursor::new(payload.as_mut_slice());
        let n = pkt.write_from_offset_to(&mut cursor, 0, 6).unwrap();
        assert_eq!(n, 6);
        assert_eq!(cursor.into_inner(), b"hello!");
    }

    /// A TX packet whose header does not fit its descriptor chain, or whose
    /// declared length is over the maximum, is rejected by `parse`.
    #[test]
    fn tx_packet_rejects_malformed_headers() {
        // A descriptor shorter than the header is rejected as too short.
        {
            let mem = default_mem();
            let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
            vq.set_desc(0, HDR_ADDR, 10, 0, 0);
            let mut q = vq.create_queue();
            vq.publish(0);
            let head = q.pop().unwrap().unwrap();

            let mut pkt = VsockPacketTx::default();
            assert!(matches!(
                pkt.parse(&mem, head),
                Err(VsockError::DescChainTooShortForHeader(_))
            ));
        }

        // A header whose declared length exceeds the maximum is rejected.
        {
            let mem = default_mem();
            let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
            let mut hdr = VsockPacketHeader::default();
            hdr.set_len(defs::MAX_PKT_BUF_SIZE + 1);
            mem.write_slice(hdr.as_slice(), GuestAddress(HDR_ADDR))
                .unwrap();
            vq.set_desc(0, HDR_ADDR, VSOCK_PKT_HDR_SIZE, 0, 0);
            let mut q = vq.create_queue();
            vq.publish(0);
            let head = q.pop().unwrap().unwrap();

            let mut pkt = VsockPacketTx::default();
            assert!(matches!(
                pkt.parse(&mem, head),
                Err(VsockError::InvalidPktLen(_))
            ));
        }
    }

    #[test]
    fn rx_packet_commits_header_and_payload() {
        let mem = default_mem();
        let vq = VirtQueue::new(GuestAddress(0), &mem, 16);
        // RX chain: write-only header descriptor -> write-only data descriptor.
        vq.set_desc(
            0,
            HDR_ADDR,
            VSOCK_PKT_HDR_SIZE,
            VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
            1,
        );
        vq.set_desc(1, DATA_ADDR, 6, VIRTQ_DESC_F_WRITE, 0);
        let mut q = vq.create_queue();
        vq.publish(0);
        let head = q.pop().unwrap().unwrap();

        let mut pkt = VsockPacketRx::new().unwrap();
        pkt.parse(&mem, head).unwrap();
        pkt.hdr
            .set_src_cid(defs::uapi::VSOCK_HOST_CID)
            .set_dst_cid(3)
            .set_src_port(2000)
            .set_dst_port(1000)
            .set_len(6)
            .set_type(defs::uapi::VSOCK_TYPE_STREAM)
            .set_op(defs::uapi::VSOCK_OP_RW);
        pkt.commit_hdr().unwrap();

        let mut src = Cursor::new(b"hello!" as &[u8]);
        let n = pkt.read_at_offset_from(&mut src, 0, 6).unwrap();
        assert_eq!(n, 6);

        // The guest memory now holds the committed header and payload.
        let read_back: VsockPacketHeader = mem.read_obj(GuestAddress(HDR_ADDR)).unwrap();
        assert_eq!(read_back.src_cid(), defs::uapi::VSOCK_HOST_CID);
        assert_eq!(read_back.dst_cid(), 3);
        let mut payload = [0u8; 6];
        mem.read_slice(&mut payload, GuestAddress(DATA_ADDR))
            .unwrap();
        assert_eq!(&payload, b"hello!");
    }

    /// Lays a two-descriptor chain — header at `HDR_ADDR`, data at `DATA_ADDR` —
    /// with the given descriptor lengths and flags, publishes it, and returns
    /// the popped head. The packet header's `len` field is written to guest
    /// memory first, so `parse` sees it.
    fn chain(
        mem: &crate::vstate::memory::GuestMemoryMmap,
        queue_start: GuestAddress,
        hdr_len: u32,
        hdr_flags: u16,
        data_len: u32,
        data_flags: u16,
        pkt_len: u32,
    ) -> crate::devices::virtio::queue::DescriptorChain {
        let mut hdr = VsockPacketHeader::default();
        hdr.set_len(pkt_len);
        mem.write_slice(hdr.as_slice(), GuestAddress(HDR_ADDR))
            .unwrap();

        let vq = VirtQueue::new(queue_start, mem, 16);
        vq.set_desc(0, HDR_ADDR, hdr_len, hdr_flags, 1);
        vq.set_desc(1, DATA_ADDR, data_len, data_flags, 0);
        let mut q = vq.create_queue();
        vq.publish(0);
        q.pop().unwrap().unwrap()
    }

    /// The standard RX/TX pair over the same buffers, for the offset tests.
    fn packet_pair(mem: &crate::vstate::memory::GuestMemoryMmap) -> (VsockPacketRx, VsockPacketTx) {
        let rx = chain(
            mem,
            GuestAddress(0),
            VSOCK_PKT_HDR_SIZE,
            VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
            PKT_BUF_SIZE,
            VIRTQ_DESC_F_WRITE,
            0,
        );
        let tx = chain(
            mem,
            GuestAddress(0x1000),
            VSOCK_PKT_HDR_SIZE,
            VIRTQ_DESC_F_NEXT,
            PKT_BUF_SIZE,
            0,
            0,
        );
        let mut pkt = VsockPacketRx::new().unwrap();
        pkt.parse(mem, rx).unwrap();
        let mut pkt2 = VsockPacketTx::default();
        pkt2.parse(mem, tx).unwrap();
        (pkt, pkt2)
    }

    /// Derived from Firecracker's `test_tx_packet_assembly` / `test_rx_packet_assembly`
    /// (the descriptor-shape error matrix), adapted to drive the chains directly.
    #[test]
    fn test_tx_packet_assembly_errors() {
        let mem = default_mem();

        // A write-only header descriptor is not device-readable.
        let head = chain(
            &mem,
            GuestAddress(0),
            VSOCK_PKT_HDR_SIZE,
            VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
            PKT_BUF_SIZE,
            0,
            0,
        );
        assert!(matches!(
            VsockPacketTx::default().parse(&mem, head),
            Err(VsockError::UnreadableDescriptor)
        ));

        // A write-only data descriptor is not device-readable either.
        let head = chain(
            &mem,
            GuestAddress(0),
            VSOCK_PKT_HDR_SIZE,
            VIRTQ_DESC_F_NEXT,
            PKT_BUF_SIZE,
            VIRTQ_DESC_F_WRITE,
            0,
        );
        assert!(matches!(
            VsockPacketTx::default().parse(&mem, head),
            Err(VsockError::UnreadableDescriptor)
        ));

        // The chain cannot hold the 44-byte header.
        let head = chain(
            &mem,
            GuestAddress(0),
            VSOCK_PKT_HDR_SIZE - 1,
            VIRTQ_DESC_F_NEXT,
            0,
            0,
            0,
        );
        assert!(matches!(
            VsockPacketTx::default().parse(&mem, head),
            Err(VsockError::DescChainTooShortForHeader(_))
        ));

        // A zero-length packet is valid.
        let head = chain(
            &mem,
            GuestAddress(0),
            VSOCK_PKT_HDR_SIZE,
            VIRTQ_DESC_F_NEXT,
            0,
            0,
            0,
        );
        VsockPacketTx::default().parse(&mem, head).unwrap();

        // The header's length may not exceed the maximum packet buffer size.
        let head = chain(
            &mem,
            GuestAddress(0),
            VSOCK_PKT_HDR_SIZE,
            VIRTQ_DESC_F_NEXT,
            PKT_BUF_SIZE,
            0,
            defs::MAX_PKT_BUF_SIZE + 1,
        );
        assert!(matches!(
            VsockPacketTx::default().parse(&mem, head),
            Err(VsockError::InvalidPktLen(_))
        ));

        // A data length with no data descriptor behind it is rejected, and the
        // error reports the chain's real size against the header's claim.
        let head = chain(
            &mem,
            GuestAddress(0),
            VSOCK_PKT_HDR_SIZE,
            0,
            PKT_BUF_SIZE,
            0,
            1024,
        );
        assert!(matches!(
            VsockPacketTx::default().parse(&mem, head),
            Err(VsockError::DescChainTooShortForPacket(44, 1024))
        ));

        // A data descriptor that cannot fit the header's advertised length.
        let head = chain(
            &mem,
            GuestAddress(0),
            VSOCK_PKT_HDR_SIZE,
            VIRTQ_DESC_F_NEXT,
            PKT_BUF_SIZE,
            0,
            8 * 1024,
        );
        assert!(matches!(
            VsockPacketTx::default().parse(&mem, head),
            Err(VsockError::DescChainTooShortForPacket(4140, 8192))
        ));
    }

    /// Derived from Firecracker's `test_rx_packet_assembly` (the RX half of the
    /// matrix).
    #[test]
    fn test_rx_packet_assembly_errors() {
        let mem = default_mem();

        // A read-only header descriptor is not device-writable.
        let head = chain(
            &mem,
            GuestAddress(0),
            VSOCK_PKT_HDR_SIZE,
            VIRTQ_DESC_F_NEXT,
            PKT_BUF_SIZE,
            0,
            0,
        );
        assert!(matches!(
            VsockPacketRx::new().unwrap().parse(&mem, head),
            Err(VsockError::UnwritableDescriptor)
        ));

        // The chain cannot hold the 44-byte header.
        let head = chain(
            &mem,
            GuestAddress(0),
            VSOCK_PKT_HDR_SIZE - 1,
            VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
            0,
            VIRTQ_DESC_F_WRITE,
            0,
        );
        assert!(matches!(
            VsockPacketRx::new().unwrap().parse(&mem, head),
            Err(VsockError::DescChainTooShortForHeader(_))
        ));
    }

    /// Derived from Firecracker's `test_packet_buf`: data read/written at every
    /// offset of the packet buffer round-trips, and a read or write that would
    /// leave the buffer fails closed with `GuestMemoryBounds`.
    #[test]
    fn test_packet_buf_offsets_and_bounds() {
        let mem = default_mem();
        let (mut pkt, pkt2) = packet_pair(&mem);
        let buf_size = pkt.buf_size();
        assert_eq!(buf_size, PKT_BUF_SIZE);

        let data: Vec<u8> = (0..buf_size).map(|i| (i & 0xff) as u8).collect();
        pkt.read_at_offset_from(&mut data.as_slice(), 0, buf_size)
            .unwrap();

        for offset in 0..buf_size {
            let mut buf = vec![0; (buf_size - offset) as usize];
            let written = pkt2
                .write_from_offset_to(&mut buf.as_mut_slice(), offset, buf_size - offset)
                .unwrap();
            assert_eq!(written, buf_size - offset);
            assert_eq!(buf, data[offset as usize..]);
        }

        // Out-of-bounds offset/count pairs are rejected by both views, an offset past the payload even with nothing to
        // copy (the fuzzer's u32::MAX offset with a count of 0 overflowed the sum).
        let oob_cases = [
            (1, buf_size),
            (buf_size, 1),
            (u32::MAX, 1),
            (1, u32::MAX),
            (u32::MAX, 0),
            (buf_size + 1, 0),
        ];
        for (offset, count) in oob_cases {
            let mut buf = vec![0; buf_size as usize];
            assert!(matches!(
                pkt2.write_from_offset_to(&mut buf.as_mut_slice(), offset, count),
                Err(VsockError::GuestMemoryBounds)
            ));
            let mut src = data.as_slice();
            assert!(matches!(
                pkt.read_at_offset_from(&mut src, offset, count),
                Err(VsockError::GuestMemoryBounds)
            ));
        }
    }
}
