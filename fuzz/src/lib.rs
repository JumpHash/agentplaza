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
