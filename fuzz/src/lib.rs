// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. Upstream Firecracker's `fuzz/` directory has a
// `vsock_packet_fuzzer`; this harness is written against this port's parsers
// (the descriptor-chain layout and the packet API differ), so no upstream code
// is copied here.

//! The harness shared by the vsock fuzz targets.
//!
//! A guest controls three things that reach the vsock parsers: the shape and
//! lengths of its descriptor chains, the 44-byte packet header, and the payload.
//! [`PacketInput`] turns an arbitrary byte slice into all three deterministically,
//! so a fuzzer explores the space by mutating bytes; [`build_packets`] lays one
//! RX (write-only) and one TX (read-only) chain over the shared buffers and
//! parses a packet view over each, returning whichever parses succeeded.
//!
//! The harness deliberately never builds a cyclic descriptor chain (the data
//! descriptor's `NEXT` bit is masked off): chain cycles belong to the virtqueue
//! itself, not to the packet parser. Everything else — descriptor lengths that
//! overrun the chain, headers shorter than 44 bytes, advertised lengths past
//! `MAX_PKT_BUF_SIZE`, either queue direction — is reachable.
//!
//! Nothing here may panic: every failure is an `Option` the target handles.

use vm_memory::{Bytes, GuestAddress};
use vmm::devices::virtio::block::VirtioBlockError;
use vmm::devices::virtio::block::{
    MAX_DISCARD_SECTORS,
    request::{
        Request, RequestType, VIRTIO_BLK_T_DISCARD, VIRTIO_BLK_T_FLUSH, VIRTIO_BLK_T_GET_ID,
        VIRTIO_BLK_T_IN, VIRTIO_BLK_T_OUT,
    },
};
use vmm::devices::virtio::queue::{
    Descriptor, DescriptorChain, Queue, VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE,
};
use vmm::devices::virtio::vsock::packet::{
    VSOCK_PKT_HDR_SIZE, VsockPacketHeader, VsockPacketRx, VsockPacketTx,
};
use vmm::vstate::memory::{ByteValued, GuestMemoryMmap, guest_memory_from_ranges};

/// Guest memory: one 64 KiB region (two virtqueues and two packet buffers).
pub const MEM_SIZE: usize = 0x10000;
/// Guest address of the packet header buffer (44 bytes).
pub const HDR_ADDR: u64 = 0x8000;
/// Guest address of the packet payload buffer.
pub const DATA_ADDR: u64 = 0x9000;
/// Bytes of payload space behind `DATA_ADDR`.
pub const DATA_LEN: u32 = (MEM_SIZE as u64 - DATA_ADDR) as u32;

/// Bytes of one descriptor-table entry.
const DESC_SIZE: u64 = 16;
/// Virtqueue size (descriptor-table entries).
const QUEUE_SIZE: u16 = 8;
/// Guest address of the RX (device-to-guest) descriptor table.
const RX_DTABLE: u64 = 0x0000;
/// Guest address of the TX (guest-to-device) descriptor table.
const TX_DTABLE: u64 = 0x0100;

/// The guest-controlled knobs of one fuzz iteration.
#[derive(Debug, Clone, Copy)]
pub struct PacketInput<'a> {
    /// The 44 header bytes as laid down in guest memory (the length field is
    /// then overwritten with `pkt_len`).
    pub hdr_bytes: [u8; VSOCK_PKT_HDR_SIZE as usize],
    /// The payload bytes written behind the header.
    pub payload: &'a [u8],
    /// Flags of the RX chain's header descriptor.
    pub rx_hdr_flags: u16,
    /// Flags of the RX chain's payload descriptor.
    pub rx_data_flags: u16,
    /// Requested length of the RX chain's header descriptor.
    pub rx_hdr_len: u32,
    /// Requested length of the RX chain's payload descriptor.
    pub rx_data_len: u32,
    /// Flags of the TX chain's header descriptor.
    pub tx_hdr_flags: u16,
    /// Flags of the TX chain's payload descriptor.
    pub tx_data_flags: u16,
    /// Requested length of the TX chain's header descriptor.
    pub tx_hdr_len: u32,
    /// Requested length of the TX chain's payload descriptor.
    pub tx_data_len: u32,
    /// The `len` field of the packet header.
    pub pkt_len: u32,
    /// Offset for the read/write round-trip.
    pub offset: u32,
    /// Byte count for the read/write round-trip.
    pub count: u32,
}

impl<'a> PacketInput<'a> {
    /// Splits `input` into knobs (its first 32 bytes, zero-extended) and
    /// payload (the rest). The descriptor lengths are derived so that both the
    /// well-formed and the malformed shapes are reachable, including a payload
    /// descriptor that runs past the end of guest memory.
    pub fn from_bytes(input: &'a [u8]) -> Self {
        const KNOBS: usize = 32;
        let (knobs, payload) = input.split_at(input.len().min(KNOBS));
        let byte = |i: usize| knobs.get(i).copied().unwrap_or(0);
        let u32_at =
            |i: usize| u32::from_le_bytes([byte(i), byte(i + 1), byte(i + 2), byte(i + 3)]);

        // 40..=47: straddles the 44-byte header, so the "header descriptor is
        // too short" path is reached often.
        let hdr_len = |i: usize| {
            let base = i32::try_from(VSOCK_PKT_HDR_SIZE).unwrap_or(0);
            let delta = i32::from(byte(i) % 8) - 4;
            u32::try_from((base + delta).max(0)).unwrap_or(VSOCK_PKT_HDR_SIZE)
        };
        // Up to 8160 normally, and up to `DATA_LEN + 64` (past the end of the
        // region) for the largest knob values.
        let data_len = |i: usize| (u32::from(byte(i)) * 128).min(DATA_LEN + 64);

        let mut hdr_bytes = [0u8; VSOCK_PKT_HDR_SIZE as usize];
        for (dst, src) in hdr_bytes.iter_mut().zip(payload) {
            *dst = *src;
        }

        // The header's `len` field: biased so that in-range lengths (and the
        // resulting successful parse, which is where the payload arithmetic
        // lives) are common, while the out-of-range cases stay reachable.
        let pkt_len = match byte(8) % 4 {
            0 => u32::from(byte(9)),
            1 => u32::from(byte(9)) * 256,
            2 => u32_at(12),
            _ => u32::from(byte(9)) % (VSOCK_PKT_HDR_SIZE * 2),
        };

        Self {
            hdr_bytes,
            payload: payload.get(VSOCK_PKT_HDR_SIZE as usize..).unwrap_or(&[]),
            rx_hdr_flags: u16::from(byte(0)),
            rx_data_flags: u16::from(byte(1)),
            rx_hdr_len: hdr_len(2),
            rx_data_len: data_len(3),
            tx_hdr_flags: u16::from(byte(4)),
            tx_data_flags: u16::from(byte(5)),
            tx_hdr_len: hdr_len(6),
            tx_data_len: data_len(7),
            pkt_len,
            offset: u32_at(16),
            count: u32_at(20),
        }
    }

    /// Builds the guest memory for this input: the header (with `pkt_len`
    /// written into its length field) and the payload.
    pub fn memory(&self) -> Option<GuestMemoryMmap> {
        let mem = guest_memory_from_ranges(&[(GuestAddress(0), MEM_SIZE)]).ok()?;

        let mut hdr = VsockPacketHeader::default();
        hdr.as_mut_slice().copy_from_slice(&self.hdr_bytes);
        hdr.set_len(self.pkt_len);
        mem.write_slice(hdr.as_slice(), GuestAddress(HDR_ADDR)).ok()?;

        let payload_len = self.payload.len().min(DATA_LEN as usize);
        mem.write_slice(&self.payload[..payload_len], GuestAddress(DATA_ADDR))
            .ok()?;

        Some(mem)
    }
}

/// One parsed packet view, from either direction.
pub enum Parsed {
    /// The guest-to-host (read-only) view.
    Tx(VsockPacketTx),
    /// The host-to-guest (write-only) view.
    Rx(VsockPacketRx),
}

/// Lays `input`'s RX and TX chains out, publishes them, and parses both views.
/// The successfully parsed views are returned (RX then TX); a chain the parser
/// rejects simply does not appear.
pub fn build_packets(input: &PacketInput<'_>) -> Option<(GuestMemoryMmap, Vec<Parsed>)> {
    let mem = input.memory()?;
    let mut parsed = Vec::with_capacity(2);

    let rx_head = publish_chain(
        &mem,
        RX_DTABLE,
        input.rx_hdr_flags | VIRTQ_DESC_F_WRITE,
        input.rx_hdr_len,
        input.rx_data_flags | VIRTQ_DESC_F_WRITE,
        input.rx_data_len,
    )?;
    if let Ok(mut pkt) = VsockPacketRx::new() {
        if pkt.parse(&mem, rx_head).is_ok() {
            parsed.push(Parsed::Rx(pkt));
        }
    }

    let tx_head = publish_chain(
        &mem,
        TX_DTABLE,
        input.tx_hdr_flags & !VIRTQ_DESC_F_WRITE,
        input.tx_hdr_len,
        input.tx_data_flags & !VIRTQ_DESC_F_WRITE,
        input.tx_data_len,
    )?;
    let mut pkt = VsockPacketTx::default();
    if pkt.parse(&mem, tx_head).is_ok() {
        parsed.push(Parsed::Tx(pkt));
    }

    Some((mem, parsed))
}

/// Writes a two-descriptor chain (header, then payload) into the descriptor
/// table at `dtable`, publishes it, and returns the popped head.
fn publish_chain(
    mem: &GuestMemoryMmap,
    dtable: u64,
    hdr_flags: u16,
    hdr_len: u32,
    data_flags: u16,
    data_len: u32,
) -> Option<DescriptorChain> {
    let dtable = GuestAddress(dtable);
    let avail = GuestAddress(dtable.0 + u64::from(QUEUE_SIZE) * DESC_SIZE);
    let avail_idx = GuestAddress(avail.0 + 2);
    let used_raw = avail.0 + 4 + u64::from(QUEUE_SIZE) * 2;
    let used = GuestAddress((used_raw + 3) & !3u64);

    let descs = [
        Descriptor {
            addr: HDR_ADDR,
            len: hdr_len,
            flags: hdr_flags | VIRTQ_DESC_F_NEXT,
            next: 1,
        },
        Descriptor {
            addr: DATA_ADDR,
            len: data_len,
            flags: data_flags & !VIRTQ_DESC_F_NEXT,
            next: 0,
        },
    ];
    for (i, desc) in descs.iter().enumerate() {
        let offset = (i as u64).checked_mul(DESC_SIZE)?;
        let addr = GuestAddress(dtable.0.checked_add(offset)?);
        mem.write_obj(*desc, addr).ok()?;
    }
    // avail: flags 0, idx 1, ring[0] = 0 (the head descriptor) — the ring is
    // zeroed by the fresh mapping, so publishing one chain only has to advance
    // the index from 0 to 1.
    mem.write_obj::<u16>(1, avail_idx).ok()?;

    let mut queue = Queue::new(QUEUE_SIZE);
    queue.size = QUEUE_SIZE;
    queue.ready = true;
    queue.desc_table_address = dtable;
    queue.avail_ring_address = avail;
    queue.used_ring_address = used;
    queue.initialize(mem).ok()?;
    queue.pop().ok()?
}

/// Drives the packet parser over one input: lays the chains out, parses both
/// views, reads every header field and walks the payload accessors at an
/// input-chosen offset. This is the body of the `vsock_packet` target; it is in
/// the library so the smoke test can drive it over many inputs, with or without
/// LibFuzzer.
pub fn drive_packet(data: &[u8]) {
    let input = PacketInput::from_bytes(data);
    let Some((_mem, parsed)) = build_packets(&input) else {
        return;
    };

    for pkt in parsed {
        match pkt {
            Parsed::Tx(pkt) => {
                let _ = pkt.buf_size();
                let _ = pkt.hdr.src_cid();
                let _ = pkt.hdr.dst_cid();
                let _ = pkt.hdr.src_port();
                let _ = pkt.hdr.dst_port();
                let _ = pkt.hdr.len();
                let _ = pkt.hdr.type_();
                let _ = pkt.hdr.op();
                let _ = pkt.hdr.flags();
                let _ = pkt.hdr.buf_alloc();
                let _ = pkt.hdr.fwd_cnt();

                // The payload accessors must fail closed, never index past the
                // chain.
                let mut sink = Vec::new();
                let _ = pkt.write_from_offset_to(&mut sink, input.offset, input.count);
            }
            Parsed::Rx(mut pkt) => {
                let _ = pkt.buf_size();
                // Commit a header built from the input's knobs, then take the
                // same offset path in the write direction.
                pkt.hdr
                    .set_src_cid(u64::from(input.pkt_len))
                    .set_dst_cid(u64::from(input.offset))
                    .set_len(input.count);
                let _ = pkt.commit_hdr();

                let data = vec![0u8; input.count.min(4096) as usize];
                let _ = pkt.read_at_offset_from(&mut data.as_slice(), input.offset, input.count);
            }
        }
    }
}

/// Drives the connection state machine over one input. This is the body of the
/// `vsock_csm` target; the socket pair is created per call, and the peer's CID
/// and buffer allocation come from the input.
pub fn drive_csm(data: &[u8]) {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use vmm::devices::virtio::vsock::VsockChannel;
    use vmm::devices::virtio::vsock::VsockEpollListener;
    use vmm::devices::virtio::vsock::csm::VsockConnection;
    use vmm_sys_util::epoll::EventSet;

    /// The host CID; the ports are fixed by the harness.
    const HOST_CID: u64 = 2;
    const LOCAL_PORT: u32 = 1002;
    const PEER_PORT: u32 = 1003;

    let input = PacketInput::from_bytes(data);
    let Some((_mem, parsed)) = build_packets(&input) else {
        return;
    };

    // A real, non-blocking stream pair: the harness owns both ends, so a write
    // can never hang the target and the read path has real data to consume.
    let Ok((host, mut peer)) = UnixStream::pair() else {
        return;
    };
    let _ = host.set_nonblocking(true);
    let _ = peer.set_nonblocking(true);
    let seed = input.payload.len().min(1024);
    let _ = peer.write_all(&input.payload[..seed]);

    let mut conn = VsockConnection::<UnixStream>::new_peer_init(
        host,
        HOST_CID,
        u64::from(input.pkt_len),
        LOCAL_PORT,
        PEER_PORT,
        input.count,
    );

    for pkt in parsed {
        match pkt {
            Parsed::Tx(pkt) => {
                // Guest-to-host: the whole CSM input surface.
                conn.send_pkt(&pkt);
            }
            Parsed::Rx(mut pkt) => {
                // Host-to-guest: fill the packet from the stream.
                let _ = conn.recv_pkt(&mut pkt);
            }
        }
        let _ = conn.has_pending_rx();
        let _ = conn.get_polled_evset();
        let _ = conn.will_expire();
        let _ = conn.has_expired();
        let _ = conn.state();
    }

    // The event notifications the muxer delivers, and the kill path that
    // depends on them.
    conn.notify(EventSet::IN);
    conn.notify(EventSet::OUT);
    conn.notify(EventSet::empty());
    conn.kill();
    let _ = conn.has_pending_rx();
}

// ---------------------------------------------------------------------------
// virtio-block request parser
// ---------------------------------------------------------------------------
//
// The guest's descriptor chain is the whole input surface of `Request::parse`:
// the head descriptor's address and direction, the middle descriptor's
// direction, length and `next` index, the status descriptor's direction and
// length, and the sixteen header bytes at the head's address (the request type
// and the sector). [`BlockInput`] turns an arbitrary byte slice into all of
// them deterministically, so a fuzzer explores the space by mutating bytes, and
// [`build_block_chain`] lays one three-descriptor chain over the buffers and
// returns the popped head, exactly as the driver publishes it.
//
// The harness never builds a chain the queue cannot resolve, and nothing here
// may panic: every failure is an `Option` or a typed `VirtioBlockError` the
// target turns into a coverage bucket.

/// Guest memory for the block harness: one 64 KiB region holding the
/// descriptor table, the request header, the data buffer and the status byte.
pub const BLOCK_MEM_SIZE: usize = 0x10000;
/// Guest address of the 16-byte request header buffer.
const BLOCK_HDR_ADDR: u64 = 0x1000;
/// Guest address of the data buffer (where a discard's range is read from).
const BLOCK_DATA_ADDR: u64 = 0x2000;
/// Guest address of the status byte.
const BLOCK_STATUS_ADDR: u64 = 0x3000;
/// Guest address of the descriptor table.
const BLOCK_DTABLE: u64 = 0x0000;
/// Virtqueue size for the block harness's single chain.
const BLOCK_QUEUE_SIZE: u16 = 8;
/// Bytes of one discard range (`struct virtio_blk_discard_write_zeroes`).
const BLOCK_DISCARD_RANGE_SIZE: u32 = 16;
/// Bytes of one request header (`struct virtio_blk_outhdr` plus `reserved`).
const BLOCK_HDR_SIZE: usize = 16;

/// The guest-controlled knobs of one block-request fuzz iteration.
#[derive(Debug, Clone, Copy)]
pub struct BlockInput {
    /// The header's `request_type` field, verbatim (every [`RequestType`] plus
    /// numbers the parser has never heard of).
    pub request_type: u32,
    /// The header's `sector` field.
    pub sector: u64,
    /// Head descriptor: the write-only bit (an illegal head when set).
    pub head_write_only: bool,
    /// Head descriptor buffer address (the header is read from here).
    pub head_addr: GuestAddress,
    /// Whether the head chains to a further descriptor.
    pub head_has_next: bool,
    /// Middle descriptor: the write-only bit (the read/write direction rule).
    pub mid_write_only: bool,
    /// Middle descriptor buffer length (the 512-multiple and id-length rules).
    pub mid_len: u32,
    /// Middle descriptor buffer address (the discard range is read from here).
    pub mid_addr: GuestAddress,
    /// Whether the middle descriptor chains to the status descriptor.
    pub mid_has_next: bool,
    /// The middle descriptor's `next` index (may point outside the ring).
    pub mid_next: u16,
    /// Status descriptor: the write-only bit (MUST be set).
    pub status_write_only: bool,
    /// Status descriptor buffer length (MUST be at least one byte).
    pub status_len: u32,
    /// Status descriptor buffer address.
    pub status_addr: GuestAddress,
    /// The discard range's `sector` field.
    pub discard_sector: u64,
    /// The discard range's `num_sectors` field.
    pub discard_num_sectors: u32,
    /// The discard range's `flags` field.
    pub discard_flags: u32,
    /// The volume size the parser bounds a request against.
    pub num_disk_sectors: u64,
}

impl BlockInput {
    /// Splits `input`'s bytes into knobs. An all-zero input maps to the
    /// well-formed read request (type `IN`, sector 0, a 512-byte write-only
    /// data descriptor, a one-byte write-only status descriptor), so the fixed
    /// input the non-fuzzing `main` uses reaches a real parse; the asymmetric
    /// selectors keep every other branch — the malformed shapes and the
    /// extreme sector/length values — reachable from a mutated input.
    pub fn from_bytes(input: &[u8]) -> Self {
        let byte = |i: usize| input.get(i).copied().unwrap_or(0);
        let u16_at = |i: usize| u16::from_le_bytes([byte(i), byte(i + 1)]);
        let u32_at = |i: usize| u32::from_le_bytes([byte(i), byte(i + 1), byte(i + 2), byte(i + 3)]);
        let u64_at = |i: usize| {
            u64::from_le_bytes([
                byte(i),
                byte(i + 1),
                byte(i + 2),
                byte(i + 3),
                byte(i + 4),
                byte(i + 5),
                byte(i + 6),
                byte(i + 7),
            ])
        };

        // The volume size the top-sector and discard bounds are checked against.
        let num_disk_sectors = match byte(2) % 3 {
            0 => 1024,
            1 => u64_at(52),
            _ => u64::MAX,
        };
        // The request's start sector: arbitrary, `u64::MAX` (the `checked_add`
        // overflow) and a value guaranteed to be inside the volume.
        let sector = match byte(1) % 4 {
            0 => u64_at(8),
            1 => u64::MAX,
            2 => u64::from(u32_at(8)) % num_disk_sectors.saturating_add(1),
            _ => u64_at(8) & 0x0000_0000_ffff_ffff,
        };
        // The head address: the header buffer, an unaligned offset inside the
        // region, or a buffer that runs off the end of guest memory.
        let head_addr = match byte(4) % 4 {
            0 => GuestAddress(BLOCK_HDR_ADDR),
            1 => GuestAddress(BLOCK_HDR_ADDR + u64::from(u16_at(5)) % 0x800),
            2 => GuestAddress(BLOCK_HDR_ADDR + u64::from(u32_at(5))),
            _ => GuestAddress((BLOCK_MEM_SIZE as u64).wrapping_sub(8) + u64::from(u16_at(5))),
        };
        // The middle (data/status) address: usually the data buffer, sometimes
        // off the end of the region (a discard range that cannot be read).
        let mid_addr = match byte(16) % 8 {
            0 => GuestAddress(BLOCK_DATA_ADDR),
            1..=3 => GuestAddress(BLOCK_DATA_ADDR + u64::from(u32_at(17) & 0xfff)),
            _ => GuestAddress(BLOCK_DATA_ADDR + u64::from(u32_at(17))),
        };
        // The middle length: the boundary values of the length rules — 512 and
        // 1024 (valid), 16 (a discard range), 0 (too short), arbitrary, and a
        // silent round down to a 512 multiple.
        let mid_len = match byte(11) % 6 {
            0 => 512,
            1 => 1024,
            2 => BLOCK_DISCARD_RANGE_SIZE,
            3 => 0,
            4 => u32_at(12),
            _ => u32_at(12) & !511,
        };
        // The middle descriptor's next index: a valid one, a self/head cycle,
        // or a value outside the ring (an invalid chain).
        let mid_next = match byte(22) % 4 {
            0 => 2,
            1 => 0,
            2 => 1,
            _ => u16_at(23),
        };
        // The status address is stored, not read, by the parser, so an
        // out-of-range one is harmless and still worth exploring.
        let status_addr = match byte(31) % 4 {
            0 => GuestAddress(BLOCK_STATUS_ADDR),
            _ => GuestAddress(BLOCK_STATUS_ADDR + u64::from(u32_at(32))),
        };
        // The status length: the one valid minimum, arbitrary, zero (too
        // short), and the device-id length.
        let status_len = match byte(26) % 4 {
            0 => 1,
            1 => u32_at(27),
            2 => 0,
            _ => 20,
        };
        // The discard range's sector count: arbitrary, the two zero cases, the
        // bound itself and just past it, and the wide-open maximum.
        let discard_num_sectors = match byte(44) % 6 {
            0 => u32_at(45),
            1 => 0,
            2 => 1,
            3 => MAX_DISCARD_SECTORS,
            4 => MAX_DISCARD_SECTORS + 1,
            _ => u32::MAX,
        };
        // The discard range's flags: the only legal value, arbitrary, a set
        // bit, and all bits.
        let discard_flags = match byte(48) % 4 {
            0 => 0,
            1 => u32_at(49),
            2 => 1,
            _ => u32::MAX,
        };

        Self {
            request_type: match byte(0) % 7 {
                0 => VIRTIO_BLK_T_IN,
                1 => VIRTIO_BLK_T_OUT,
                2 => VIRTIO_BLK_T_FLUSH,
                3 => VIRTIO_BLK_T_GET_ID,
                4 => VIRTIO_BLK_T_DISCARD,
                5 => u32_at(4),
                _ => u32::MAX,
            },
            sector,
            head_write_only: byte(3) % 8 == 7,
            head_addr,
            head_has_next: byte(9) % 8 != 7,
            mid_write_only: byte(10) % 4 != 3,
            mid_len,
            mid_addr,
            mid_has_next: byte(21) % 4 != 3,
            mid_next,
            status_write_only: byte(25) % 4 != 3,
            status_len,
            status_addr,
            discard_sector: u64_at(36),
            discard_num_sectors,
            discard_flags,
            num_disk_sectors,
        }
    }

    /// Builds the guest memory for this input: the request header (type,
    /// reserved, sector — little-endian, as the driver writes them) at
    /// [`BLOCK_HDR_ADDR`], and the discard range at [`BLOCK_DATA_ADDR`].
    pub fn memory(&self) -> Option<GuestMemoryMmap> {
        let mem = guest_memory_from_ranges(&[(GuestAddress(0), BLOCK_MEM_SIZE)]).ok()?;

        let mut hdr = [0u8; BLOCK_HDR_SIZE];
        hdr[0..4].copy_from_slice(&self.request_type.to_le_bytes());
        // bytes 4..8 are the reserved word: zero, as a conforming driver writes.
        hdr[8..16].copy_from_slice(&self.sector.to_le_bytes());
        mem.write_slice(&hdr, GuestAddress(BLOCK_HDR_ADDR)).ok()?;

        let mut range = [0u8; BLOCK_DISCARD_RANGE_SIZE as usize];
        range[0..8].copy_from_slice(&self.discard_sector.to_le_bytes());
        range[8..12].copy_from_slice(&self.discard_num_sectors.to_le_bytes());
        range[12..16].copy_from_slice(&self.discard_flags.to_le_bytes());
        mem.write_slice(&range, GuestAddress(BLOCK_DATA_ADDR)).ok()?;

        Some(mem)
    }
}

/// Which check rejected a chain. The parser's decisions are a fixed sequence of
/// gates; naming the one that fired is how a smoke run reports which parts of
/// the parser the corpus actually drove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RejectPath {
    /// The chain ended before the descriptor the request type needs.
    ChainTooShort,
    /// The head descriptor was write-only.
    HeadWriteOnly,
    /// The data descriptor was write-only for a write or a discard.
    DataWriteOnly,
    /// The data descriptor was read-only for a read or a device-id request.
    DataReadOnly,
    /// The status descriptor was read-only.
    StatusReadOnly,
    /// The request header could not be read out of guest memory.
    HeaderUnreadable,
    /// A discard's range could not be read out of guest memory.
    DiscardUnreadable,
    /// The data length (or discard range) was not valid for the type.
    InvalidDataLength,
    /// The request reached past the end of the volume.
    InvalidOffset,
    /// The status descriptor was shorter than one byte.
    StatusTooSmall,
    /// A rejection the harness does not attribute to a named gate (the
    /// backing-file/queue variants, unreachable from `parse` on a valid queue).
    Other,
}

/// The number of [`RejectPath`] variants.
const REJECT_PATHS: usize = 11;

/// The request-type buckets of the coverage report, in discriminant order:
/// `In`, `Out`, `Discard`, `Flush`, `GetDeviceID`, `Unsupported`.
const TYPE_COUNT: usize = 6;
/// Labels for [`TYPE_COUNT`], in the same order.
const TYPE_LABELS: [&str; TYPE_COUNT] = ["in", "out", "discard", "flush", "get-id", "unsupported"];

/// The coverage bucket of a request type.
fn type_index(request_type: RequestType) -> usize {
    match request_type {
        RequestType::In => 0,
        RequestType::Out => 1,
        RequestType::Discard => 2,
        RequestType::Flush => 3,
        RequestType::GetDeviceID => 4,
        RequestType::Unsupported(_) => 5,
    }
}

impl RejectPath {
    /// Every path, in discriminant order, for reporting.
    const ALL: [RejectPath; REJECT_PATHS] = [
        RejectPath::ChainTooShort,
        RejectPath::HeadWriteOnly,
        RejectPath::DataWriteOnly,
        RejectPath::DataReadOnly,
        RejectPath::StatusReadOnly,
        RejectPath::HeaderUnreadable,
        RejectPath::DiscardUnreadable,
        RejectPath::InvalidDataLength,
        RejectPath::InvalidOffset,
        RejectPath::StatusTooSmall,
        RejectPath::Other,
    ];

    /// A short label for reports.
    pub fn label(self) -> &'static str {
        match self {
            RejectPath::ChainTooShort => "chain-too-short",
            RejectPath::HeadWriteOnly => "head-write-only",
            RejectPath::DataWriteOnly => "data-write-only",
            RejectPath::DataReadOnly => "data-read-only",
            RejectPath::StatusReadOnly => "status-read-only",
            RejectPath::HeaderUnreadable => "header-unreadable",
            RejectPath::DiscardUnreadable => "discard-unreadable",
            RejectPath::InvalidDataLength => "invalid-data-length",
            RejectPath::InvalidOffset => "invalid-offset",
            RejectPath::StatusTooSmall => "status-too-small",
            RejectPath::Other => "other",
        }
    }
}

/// What one block-harness iteration reached, and the request type it was for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockOutcome {
    /// The harness could not lay the chain out, so `Request::parse` never ran.
    Unbuilt,
    /// The parser ran and rejected the chain.
    Rejected {
        /// The gate that rejected the chain.
        path: RejectPath,
        /// The request type the header declared.
        request_type: RequestType,
    },
    /// The parser ran and accepted the request.
    Parsed(RequestType),
}

/// Per-path and per-type counts over a corpus: the harness's coverage
/// measurement, so a smoke run reports how much of the parser the inputs
/// reached, not merely that nothing panicked.
#[derive(Debug, Default, Clone, Copy)]
pub struct BlockCoverage {
    unbuilt: u64,
    parsed: u64,
    rejected: [u64; REJECT_PATHS],
    by_type: [u64; TYPE_COUNT],
}

impl BlockCoverage {
    /// An empty tally.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one harness outcome.
    pub fn record(&mut self, outcome: BlockOutcome) {
        match outcome {
            BlockOutcome::Unbuilt => self.unbuilt += 1,
            BlockOutcome::Rejected { path, request_type } => {
                self.rejected[path as usize] += 1;
                self.by_type[type_index(request_type)] += 1;
            }
            BlockOutcome::Parsed(request_type) => {
                self.parsed += 1;
                self.by_type[type_index(request_type)] += 1;
            }
        }
    }

    /// Inputs the parser accepted (the "reached a real parse" number).
    pub fn parsed(&self) -> u64 {
        self.parsed
    }

    /// Inputs that never reached the parser.
    pub fn unbuilt(&self) -> u64 {
        self.unbuilt
    }

    /// Inputs rejected by `path`.
    pub fn rejected(&self, path: RejectPath) -> u64 {
        self.rejected[path as usize]
    }

    /// Inputs whose header declared a request type in `request_type`'s bucket.
    pub fn requests(&self, request_type: RequestType) -> u64 {
        self.by_type[type_index(request_type)]
    }

    /// Total inputs recorded.
    pub fn total(&self) -> u64 {
        self.unbuilt + self.parsed + self.rejected.iter().sum::<u64>()
    }

    /// Every `(label, count)` pair, for a report.
    pub fn report(&self) -> impl Iterator<Item = (&'static str, u64)> + '_ {
        std::iter::once(("parsed", self.parsed))
            .chain(std::iter::once(("chain-unbuilt", self.unbuilt)))
            .chain(
                RejectPath::ALL
                    .into_iter()
                    .map(|path| (path.label(), self.rejected(path))),
            )
    }

    /// The `(request-type label, count)` pairs, for a report.
    pub fn type_report(&self) -> impl Iterator<Item = (&'static str, u64)> + '_ {
        TYPE_LABELS
            .iter()
            .copied()
            .zip(self.by_type.iter().copied())
    }
}

/// Lays a three-descriptor chain over `input`'s buffers, publishes it as the
/// single available chain, and returns the popped head.
///
/// `None` means the harness itself could not publish the chain (it never does
/// on this fixed layout, but the target must not panic if it ever did).
fn build_block_chain(input: &BlockInput, mem: &GuestMemoryMmap) -> Option<DescriptorChain> {
    let head_flags = if input.head_has_next {
        VIRTQ_DESC_F_NEXT
    } else {
        0
    } | if input.head_write_only {
        VIRTQ_DESC_F_WRITE
    } else {
        0
    };
    let mid_flags = if input.mid_has_next {
        VIRTQ_DESC_F_NEXT
    } else {
        0
    } | if input.mid_write_only {
        VIRTQ_DESC_F_WRITE
    } else {
        0
    };
    let status_flags = if input.status_write_only {
        VIRTQ_DESC_F_WRITE
    } else {
        0
    };

    let descs = [
        Descriptor {
            addr: input.head_addr.0,
            len: BLOCK_HDR_SIZE as u32,
            flags: head_flags,
            next: 1,
        },
        Descriptor {
            addr: input.mid_addr.0,
            len: input.mid_len,
            flags: mid_flags,
            next: input.mid_next,
        },
        Descriptor {
            addr: input.status_addr.0,
            len: input.status_len,
            flags: status_flags,
            next: 0,
        },
    ];

    let dtable = GuestAddress(BLOCK_DTABLE);
    let avail = GuestAddress(dtable.0 + u64::from(BLOCK_QUEUE_SIZE) * DESC_SIZE);
    let avail_idx = GuestAddress(avail.0 + 2);
    let used_raw = avail.0 + 4 + u64::from(BLOCK_QUEUE_SIZE) * 2;
    let used = GuestAddress((used_raw + 3) & !3u64);

    for (i, desc) in descs.iter().enumerate() {
        let offset = (i as u64).checked_mul(DESC_SIZE)?;
        let addr = GuestAddress(dtable.0.checked_add(offset)?);
        mem.write_obj(*desc, addr).ok()?;
    }
    // avail: flags 0, idx 1, ring[0] = 0 (the head descriptor) — the ring is
    // zeroed by the fresh mapping, so publishing one chain only has to advance
    // the index from 0 to 1.
    mem.write_obj::<u16>(1, avail_idx).ok()?;

    let mut queue = Queue::new(BLOCK_QUEUE_SIZE);
    queue.size = BLOCK_QUEUE_SIZE;
    queue.ready = true;
    queue.desc_table_address = dtable;
    queue.avail_ring_address = avail;
    queue.used_ring_address = used;
    queue.initialize(mem).ok()?;
    queue.pop().ok()?
}

/// The parser's rejection, attributed to the gate that fired (the two
/// write-only and the two read-only gates share an error variant, so the input
/// is needed to tell them apart).
fn reject_path(input: &BlockInput, err: &VirtioBlockError) -> RejectPath {
    match err {
        VirtioBlockError::DescriptorChainTooShort => RejectPath::ChainTooShort,
        VirtioBlockError::UnexpectedWriteOnlyDescriptor => {
            if input.head_write_only {
                RejectPath::HeadWriteOnly
            } else {
                RejectPath::DataWriteOnly
            }
        }
        VirtioBlockError::UnexpectedReadOnlyDescriptor => {
            let needs_write = matches!(
                RequestType::from(input.request_type),
                RequestType::In | RequestType::GetDeviceID
            );
            if needs_write && input.mid_has_next && !input.mid_write_only {
                RejectPath::DataReadOnly
            } else {
                RejectPath::StatusReadOnly
            }
        }
        VirtioBlockError::GuestMemory(_) => {
            if RequestType::from(input.request_type) == RequestType::Discard
                && input.mid_len == BLOCK_DISCARD_RANGE_SIZE
            {
                RejectPath::DiscardUnreadable
            } else {
                RejectPath::HeaderUnreadable
            }
        }
        VirtioBlockError::DescriptorLengthTooSmall => RejectPath::StatusTooSmall,
        VirtioBlockError::InvalidDataLength => RejectPath::InvalidDataLength,
        VirtioBlockError::InvalidOffset => RejectPath::InvalidOffset,
        _ => RejectPath::Other,
    }
}

/// Drives the block request parser over one laid-out input: builds the chain,
/// calls [`Request::parse`] and touches the parsed request's public surface.
fn drive_block_request_input(input: &BlockInput) -> BlockOutcome {
    let Some(mem) = input.memory() else {
        return BlockOutcome::Unbuilt;
    };
    let Some(head) = build_block_chain(input, &mem) else {
        return BlockOutcome::Unbuilt;
    };

    match Request::parse(&head, &mem, input.num_disk_sectors) {
        Ok(req) => {
            // Every field the parser exposes, so the accepted side is walked too.
            let _ = req.r#type;
            let _ = req.data_len;
            let _ = req.status_addr;
            BlockOutcome::Parsed(req.r#type)
        }
        Err(err) => BlockOutcome::Rejected {
            path: reject_path(input, &err),
            request_type: RequestType::from(input.request_type),
        },
    }
}

/// Drives the request parser over one input: turns the bytes into knobs, lays
/// the chain out, and parses. This is the body of the `block_request` target;
/// it is in the library so the smoke test can drive it over many inputs, with
/// or without LibFuzzer.
pub fn drive_block_request(data: &[u8]) -> BlockOutcome {
    drive_block_request_input(&BlockInput::from_bytes(data))
}
