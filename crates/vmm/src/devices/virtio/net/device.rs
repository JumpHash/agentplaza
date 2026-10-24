// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port (PLAN 22.6, W58). The virtio-net queues are the split-ring queues of virtio
// 1.0 §5.1.6: the receive ring takes device-writable chains that the device fills with a frame behind
// the header, the transmit ring takes driver-readable chains whose bytes are a frame behind the
// header. Every guest buffer is read or written through guest memory, never through a pointer the
// guest chose, and a chain that does not fit or is not of the right direction is consumed with a zero
// length, as the block device does for a request it cannot parse.

//! The net device's queues: transmit and receive over guest memory, through the switch's port.
//!
//! [`VirtioNet`] owns the port the device sends through and the two rings. It is driven from two places
//! that never run at once (the transport serializes them): the transport's queue notifications, which
//! call [`VirtioNet::process_tx`] and [`VirtioNet::flush_rx`], and the device's receive thread (see
//! [`super::mmio_device`]), which hands each frame the switch delivered to [`VirtioNet::receive`].
//! A frame that arrives before the driver has posted a receive buffer waits in a bounded queue; a
//! frame that arrives while the queue is full is dropped, as a real NIC drops under overrun.

use std::collections::VecDeque;

use crate::devices::virtio::mmio::QueueConfig;
use crate::devices::virtio::queue::{DescriptorChain, Queue};
use crate::pal::eventfd::EventFd;
use crate::vstate::memory::{Bytes, GuestAddress, GuestMemoryMmap};

use super::switch::Port;
use super::{MAX_FRAME_LEN, Mac, NET_HDR_LEN, NET_QUEUE_SIZE, NetError, header_with_buffers};

/// Frames the receive path holds while the driver has no buffer for them.
pub const MAX_PENDING_RX: usize = 256;

/// The most bytes one transmit chain may carry: a frame and its header.
const MAX_TX_BYTES: usize = MAX_FRAME_LEN + NET_HDR_LEN;

/// The device's queues, its port and the guest memory they live in, once the driver activates it.
#[derive(Debug)]
pub struct VirtioNet {
    mac: Mac,
    port: Port,
    rx: Queue,
    tx: Queue,
    mem: Option<GuestMemoryMmap>,
    irq: Option<EventFd>,
    pending: VecDeque<Vec<u8>>,
}

/// One descriptor of a chain, as the device reads it: guest address, length, and whether the driver
/// marked it device-writable.
type Buffer = (u64, u32, bool);

/// The descriptors of `head`'s chain, in order.
fn buffers(head: &DescriptorChain) -> Vec<Buffer> {
    let mut out = Vec::new();
    let mut current = Some(*head);
    while let Some(desc) = current {
        out.push((desc.addr.0, desc.len, desc.is_write_only()));
        current = desc.next_descriptor();
    }
    out
}

/// A transmit chain's frame: the bytes behind the header, or `None` when the chain is not one the
/// device can send (a writable buffer, too many bytes, or a buffer outside guest memory).
fn gather(head: &DescriptorChain, mem: &GuestMemoryMmap) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    for (addr, len, write_only) in buffers(head) {
        if write_only {
            return None;
        }
        let len = usize::try_from(len).ok()?;
        if bytes.len().checked_add(len)? > MAX_TX_BYTES {
            return None;
        }
        let start = bytes.len();
        bytes.resize(start + len, 0);
        mem.read_slice(&mut bytes[start..], GuestAddress(addr))
            .ok()?;
    }
    if bytes.len() <= NET_HDR_LEN {
        return None;
    }
    Some(bytes.split_off(NET_HDR_LEN))
}

/// Writes a received frame into a receive chain behind a zeroed header with `num_buffers` 1, and
/// returns the number of bytes the device wrote. `None` when the chain cannot hold the frame or is
/// not device-writable: the chain is then given back empty.
fn scatter(head: &DescriptorChain, mem: &GuestMemoryMmap, frame: &[u8]) -> Option<u32> {
    let total = NET_HDR_LEN + frame.len();
    let layout = buffers(head);
    if layout.iter().any(|(_, _, write_only)| !write_only) {
        return None;
    }
    let capacity: usize = layout
        .iter()
        .map(|(_, len, _)| usize::try_from(*len).unwrap_or(usize::MAX))
        .fold(0usize, usize::saturating_add);
    if capacity < total {
        return None;
    }
    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(&header_with_buffers(1));
    bytes.extend_from_slice(frame);
    let mut offset = 0;
    for (addr, len, _) in layout {
        if offset == total {
            break;
        }
        let n = usize::try_from(len).ok()?.min(total - offset);
        mem.write_slice(&bytes[offset..offset + n], GuestAddress(addr))
            .ok()?;
        offset += n;
    }
    u32::try_from(total).ok()
}

/// Raises the device's interrupt when the driver has not suppressed it.
fn signal(irq: Option<&EventFd>) {
    if let Some(irq) = irq {
        irq.write(1).ok();
    }
}

impl VirtioNet {
    /// A device with `mac`, sending and receiving through `port`, with no queues yet.
    pub fn new(mac: Mac, port: Port) -> Self {
        Self {
            mac,
            port,
            rx: Queue::new(NET_QUEUE_SIZE),
            tx: Queue::new(NET_QUEUE_SIZE),
            mem: None,
            irq: None,
            pending: VecDeque::new(),
        }
    }

    /// The MAC the device's configuration space reports.
    pub fn mac(&self) -> Mac {
        self.mac
    }

    /// Whether the driver has activated the device (and it has not been reset since).
    pub fn is_active(&self) -> bool {
        self.mem.is_some()
    }

    /// Activates the device: the receive queue (index 0) and transmit queue (index 1) take the ring
    /// addresses the driver programmed in `configs`, in guest memory `mem`, and interrupt through `irq`.
    pub fn activate(
        &mut self,
        mem: GuestMemoryMmap,
        irq: EventFd,
        configs: &[QueueConfig],
    ) -> Result<(), NetError> {
        let [rx, tx] = configs else {
            return Err(NetError::Activate("the device has two queues".into()));
        };
        for (queue, config) in [(&mut self.rx, rx), (&mut self.tx, tx)] {
            queue.size = u16::try_from(config.size)
                .map_err(|_| NetError::Activate(format!("a queue of {} entries", config.size)))?;
            queue.ready = config.ready;
            queue.desc_table_address = GuestAddress(config.desc);
            queue.avail_ring_address = GuestAddress(config.avail);
            queue.used_ring_address = GuestAddress(config.used);
            queue.initialize(&mem)?;
        }
        self.mem = Some(mem);
        self.irq = Some(irq);
        Ok(())
    }

    /// Returns the device to its state before activation: no memory, no queues, no pending frames.
    pub fn reset(&mut self) {
        self.mem = None;
        self.irq = None;
        self.pending.clear();
        self.rx = Queue::new(NET_QUEUE_SIZE);
        self.tx = Queue::new(NET_QUEUE_SIZE);
    }

    /// Sends every frame the driver has queued on the transmit ring through the port, and completes
    /// each chain. A chain that is not a frame is completed with zero length and sends nothing.
    pub fn process_tx(&mut self) -> Result<(), NetError> {
        let VirtioNet {
            port, tx, mem, irq, ..
        } = self;
        let Some(mem) = mem.as_ref() else {
            return Ok(());
        };
        let mut used_any = false;
        while let Some(head) = tx.pop_or_enable_notification()? {
            if let Some(frame) = gather(&head, mem) {
                // A frame the switch refuses (too short, too large) is dropped with the chain.
                port.send(&frame).ok();
            }
            tx.add_used(head.index, 0)?;
            used_any = true;
        }
        tx.advance_used_ring_idx();
        if used_any && tx.prepare_kick() {
            signal(irq.as_ref());
        }
        Ok(())
    }

    /// Takes a frame the switch delivered to this device. It waits for a receive buffer if the driver
    /// has none posted; a frame that finds the wait queue full is dropped. An inactive device drops it.
    pub fn receive(&mut self, frame: Vec<u8>) -> Result<(), NetError> {
        if !self.is_active() || self.pending.len() >= MAX_PENDING_RX {
            return Ok(());
        }
        self.pending.push_back(frame);
        self.flush_rx()
    }

    /// Delivers the waiting frames into the receive buffers the driver has posted, and completes each
    /// buffer with the bytes it took. Called when the driver posts buffers and when a frame arrives.
    pub fn flush_rx(&mut self) -> Result<(), NetError> {
        let VirtioNet {
            rx,
            mem,
            irq,
            pending,
            ..
        } = self;
        let Some(mem) = mem.as_ref() else {
            return Ok(());
        };
        let mut used_any = false;
        while let Some(frame) = pending.pop_front() {
            let Some(head) = rx.pop()? else {
                pending.push_front(frame);
                break;
            };
            let written = scatter(&head, mem, &frame).unwrap_or(0);
            rx.add_used(head.index, written)?;
            used_any = true;
        }
        rx.advance_used_ring_idx();
        if used_any && rx.prepare_kick() {
            signal(irq.as_ref());
        }
        Ok(())
    }

    /// Frames waiting for a receive buffer.
    pub fn pending_frames(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::virtio::net::switch::{Endpoint, Port, Switch};
    use crate::devices::virtio::queue::{VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE};
    use crate::devices::virtio::test_utils::{VirtQueue, default_mem};
    use crate::pal::eventfd::EFD_NONBLOCK;
    use crate::vstate::memory::GuestMemoryMmap as Memory;
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    const DEV: Mac = [0x02, 0x41, 0x50, 0x5a, 0x00, 0x0a];
    const PEER: Mac = [0x02, 0x41, 0x50, 0x5a, 0x00, 0x0b];
    const SIZE: u16 = 16;

    /// A frame of EtherType 0x88b5 from `src` to `dst` carrying `payload`.
    fn frame(dst: Mac, src: Mac, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&dst);
        out.extend_from_slice(&src);
        out.extend_from_slice(&[0x88, 0xb5]);
        out.extend_from_slice(payload);
        out
    }

    /// The fake guest: the device under test on one segment with a peer, the device's rings laid
    /// out in guest memory as a driver would (a receive ring at 0, a transmit ring at 0x2000).
    struct Lab<'a> {
        dev: VirtioNet,
        peer: Port,
        dev_rx: Receiver<Vec<u8>>,
        peer_rx: Receiver<Vec<u8>>,
        rx: VirtQueue<'a>,
        tx: VirtQueue<'a>,
    }

    impl<'a> Lab<'a> {
        /// An activated device whose queues are both configured in `mem`.
        fn activated(mem: &'a Memory) -> Self {
            let switch = Switch::new();
            switch.create_segment("s");
            let (port, dev_rx) = switch.attach("s", Endpoint::Guest { mac: DEV }).unwrap();
            let (peer, peer_rx) = switch.attach("s", Endpoint::Guest { mac: PEER }).unwrap();
            let rx = VirtQueue::new(GuestAddress(0), mem, SIZE);
            let tx = VirtQueue::new(GuestAddress(0x2000), mem, SIZE);
            let mut dev = VirtioNet::new(DEV, port);
            let configs = [&rx, &tx].map(|vq| {
                let mut config = QueueConfig::new(u32::from(NET_QUEUE_SIZE));
                config.size = u32::from(SIZE);
                config.ready = true;
                config.desc = vq.desc_table_addr().0;
                config.avail = vq.avail_ring_addr().0;
                config.used = vq.used_ring_addr().0;
                config
            });
            dev.activate(mem.clone(), EventFd::new(EFD_NONBLOCK).unwrap(), &configs)
                .unwrap();
            Self {
                dev,
                peer,
                dev_rx,
                peer_rx,
                rx,
                tx,
            }
        }
    }

    fn got(rx: &Receiver<Vec<u8>>) -> Option<Vec<u8>> {
        rx.recv_timeout(Duration::from_millis(100)).ok()
    }

    #[test]
    fn a_transmitted_frame_leaves_by_the_port_with_its_header_stripped() {
        let mem = default_mem();
        let mut lab = Lab::activated(&mem);
        // A two-entry chain: the header (zeros) in one buffer, the frame in the next.
        let f = frame(PEER, DEV, b"out");
        mem.write_slice(&[0u8; NET_HDR_LEN], GuestAddress(0x4000))
            .unwrap();
        mem.write_slice(&f, GuestAddress(0x4100)).unwrap();
        lab.tx
            .set_desc(0, 0x4000, NET_HDR_LEN as u32, VIRTQ_DESC_F_NEXT, 1);
        lab.tx.set_desc(1, 0x4100, f.len() as u32, 0, 0);
        lab.tx.publish(0);

        lab.dev.process_tx().unwrap();

        assert_eq!(
            got(&lab.peer_rx),
            Some(f),
            "the peer gets exactly the frame"
        );
        assert_eq!(lab.tx.used_idx(), 1, "the chain is completed");
        assert_eq!(
            lab.tx.used_elem(0),
            (0, 0),
            "a transmit completes with zero length"
        );
    }

    #[test]
    fn a_transmit_chain_with_a_writable_buffer_is_completed_and_sends_nothing() {
        let mem = default_mem();
        let mut lab = Lab::activated(&mem);
        let f = frame(PEER, DEV, b"nope");
        mem.write_slice(&[0u8; NET_HDR_LEN], GuestAddress(0x4000))
            .unwrap();
        mem.write_slice(&f, GuestAddress(0x4100)).unwrap();
        lab.tx
            .set_desc(0, 0x4000, NET_HDR_LEN as u32, VIRTQ_DESC_F_NEXT, 1);
        lab.tx
            .set_desc(1, 0x4100, f.len() as u32, VIRTQ_DESC_F_WRITE, 0);
        lab.tx.publish(0);

        lab.dev.process_tx().unwrap();

        assert_eq!(lab.tx.used_elem(0), (0, 0), "completed with nothing sent");
        assert_eq!(
            got(&lab.peer_rx),
            None,
            "a writable transmit buffer is not a frame"
        );
    }

    #[test]
    fn a_received_frame_waits_for_a_buffer_then_lands_behind_a_zeroed_header() {
        let mem = default_mem();
        let mut lab = Lab::activated(&mem);
        let f = frame(DEV, PEER, b"hello device");
        lab.peer_send_to_device(&f);

        // No buffer posted yet: the frame waits in the device.
        lab.dev.receive(got(&lab.dev_rx).unwrap()).unwrap();
        assert_eq!(lab.dev.pending_frames(), 1);
        assert_eq!(lab.rx.used_idx(), 0);

        // The driver posts a writable buffer: the frame lands behind a header with num_buffers 1.
        lab.rx.set_desc(0, 0x6000, 2048, VIRTQ_DESC_F_WRITE, 0);
        lab.rx.publish(0);
        lab.dev.flush_rx().unwrap();
        assert_eq!(lab.dev.pending_frames(), 0);
        assert_eq!(lab.rx.used_idx(), 1);
        assert_eq!(lab.rx.used_elem(0), (0, (NET_HDR_LEN + f.len()) as u32));
        let mut header = [0u8; NET_HDR_LEN];
        mem.read_slice(&mut header, GuestAddress(0x6000)).unwrap();
        assert_eq!(
            header[..10],
            [0u8; 10],
            "flags, gso and checksum fields are zero"
        );
        assert_eq!(header[10..], [1, 0], "num_buffers is 1");
        let mut body = vec![0u8; f.len()];
        mem.read_slice(&mut body, GuestAddress(0x6000 + NET_HDR_LEN as u64))
            .unwrap();
        assert_eq!(body, f);
    }

    #[test]
    fn a_frame_too_big_for_its_buffer_completes_the_buffer_empty() {
        let mem = default_mem();
        let mut lab = Lab::activated(&mem);
        lab.rx.set_desc(0, 0x6000, 8, VIRTQ_DESC_F_WRITE, 0);
        lab.rx.publish(0);
        lab.peer_send_to_device(&frame(DEV, PEER, b"too long for eight bytes"));

        lab.dev.receive(got(&lab.dev_rx).unwrap()).unwrap();
        assert_eq!(lab.rx.used_idx(), 1, "the buffer is given back");
        assert_eq!(lab.rx.used_elem(0), (0, 0), "with nothing written");
        assert_eq!(
            lab.dev.pending_frames(),
            0,
            "the frame that did not fit is gone"
        );
    }

    #[test]
    fn a_read_only_receive_buffer_is_completed_empty() {
        let mem = default_mem();
        let mut lab = Lab::activated(&mem);
        lab.rx.set_desc(0, 0x6000, 2048, 0, 0);
        lab.rx.publish(0);
        lab.peer_send_to_device(&frame(DEV, PEER, b"into a read-only buffer"));

        lab.dev.receive(got(&lab.dev_rx).unwrap()).unwrap();
        assert_eq!(lab.rx.used_elem(0), (0, 0));
    }

    #[test]
    fn an_inactive_device_sends_nothing_and_queues_nothing() {
        let switch = Switch::new();
        switch.create_segment("s");
        let (port, dev_rx) = switch.attach("s", Endpoint::Guest { mac: DEV }).unwrap();
        let (peer, _peer_rx) = switch.attach("s", Endpoint::Guest { mac: PEER }).unwrap();
        let mut dev = VirtioNet::new(DEV, port);
        assert!(!dev.is_active());
        dev.process_tx().unwrap();
        peer.send(&frame(DEV, PEER, b"early")).unwrap();
        dev.receive(got(&dev_rx).unwrap()).unwrap();
        assert_eq!(
            dev.pending_frames(),
            0,
            "a frame for a device that is down is dropped"
        );
    }

    #[test]
    fn the_wait_queue_is_bounded_and_a_reset_clears_it() {
        let mem = default_mem();
        let mut lab = Lab::activated(&mem);
        for _ in 0..(MAX_PENDING_RX + 10) {
            lab.dev.receive(frame(DEV, PEER, b"x")).unwrap();
        }
        assert_eq!(lab.dev.pending_frames(), MAX_PENDING_RX);
        lab.dev.reset();
        assert_eq!(lab.dev.pending_frames(), 0);
        assert!(!lab.dev.is_active());
    }

    impl Lab<'_> {
        /// The peer sends `f` (addressed to the device) through the switch.
        fn peer_send_to_device(&self, f: &[u8]) {
            self.peer.send(f).unwrap();
        }
    }
}
