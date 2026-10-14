// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/devices/virtio/vsock/device.rs
// (trimmed: this port has no `event_manager`, no `logger`, no `VirtioInterrupt`
// and no snapshot support, so the device owns its guest memory and interrupt
// eventfd directly instead of an `ActiveState`, `signal_used_queue` writes the
// transport's interrupt eventfd, `kick`/`prepare_save` are dropped, and the
// `VirtioDevice` trait impl becomes inherent methods — the device lives
// outside the mmio transport's trait and is hand-wired by the next slice).

//! This is the vsock device frontend. It handles the virtio-level device logic:
//! feature negotiation, device configuration, and device activation.
//!
//! We aim to conform to the VirtIO v1.1 spec:
//! https://docs.oasis-open.org/virtio/virtio/v1.1/virtio-v1.1.html
//!
//! The vsock device has two input parameters: a CID to identify the device, and a
//! `VsockBackend` to use for offloading vsock traffic.

use std::fmt::Debug;

use crate::pal::eventfd::EventFd;

use super::metrics::{IncMetric, METRICS};
use super::packet::{VSOCK_PKT_HDR_SIZE, VsockPacketRx, VsockPacketTx};
use super::{VsockBackend, VsockError, defs};
use crate::devices::virtio::queue::{InvalidAvailIdx, Queue as VirtQueue, QueueError};
use crate::vstate::memory::{ByteValued, Bytes, GuestMemoryMmap};

pub(crate) const RXQ_INDEX: usize = 0;
pub(crate) const TXQ_INDEX: usize = 1;
pub(crate) const EVQ_INDEX: usize = 2;

pub(crate) const VIRTIO_VSOCK_EVENT_TRANSPORT_RESET: u32 = 0;

/// `VIRTIO_F_VERSION_1` (feature bit 32).
const VIRTIO_F_VERSION_1: u64 = 1 << 32;
/// `VIRTIO_F_IN_ORDER` (feature bit 35): used buffers are returned in avail order.
const VIRTIO_F_IN_ORDER: u64 = 1 << 35;
/// `VIRTIO_RING_F_EVENT_IDX` (feature bit 29): notification suppression.
const VIRTIO_RING_F_EVENT_IDX: u64 = 1 << 29;

/// The virtio features supported by our vsock device:
/// - VIRTIO_F_VERSION_1: the device conforms to at least version 1.0 of the VirtIO spec.
/// - VIRTIO_F_IN_ORDER: the device returns used buffers in the same order that the driver makes
///   them available.
/// - VIRTIO_RING_F_EVENT_IDX: the device supports used_event/avail_event notification
///   suppression.
pub(crate) const AVAIL_FEATURES: u64 =
    VIRTIO_F_VERSION_1 | VIRTIO_F_IN_ORDER | VIRTIO_RING_F_EVENT_IDX;

/// Error type for [`Vsock::activate`], mirroring upstream's `ActivateError`.
#[derive(Debug, thiserror::Error)]
pub enum ActivateError {
    /// Error activating the queues: {0}.
    #[error("error activating the queues: {0}")]
    QueueMemoryError(#[from] QueueError),
    /// The number of queues is not as expected: expected {expected}, got {got}.
    #[error("the number of queues is not as expected: expected {expected}, got {got}")]
    QueueMismatch {
        /// The number of queues the device expects.
        expected: usize,
        /// The number of queues the driver configured.
        got: usize,
    },
    /// Vsock backend activation error: {0}.
    #[error("vsock backend activation error: {0}")]
    VsockBackend(#[from] VsockError),
    /// Failed to write the activation event: {0}.
    #[error("failed to write the activation event: {0}")]
    EventFd(#[from] std::io::Error),
}

/// Error type for device operations that signal the guest or drain the event
/// queue, mirroring upstream's `DeviceError`.
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    /// Failed to signal the used queue: {0}.
    #[error("failed to signal the used queue: {0}")]
    FailedSignalingIrq(#[from] std::io::Error),
    /// Virtio queue error: {0}.
    #[error("virtio queue error: {0}")]
    QueueError(#[from] InvalidAvailIdx),
    /// Vsock error: {0}.
    #[error("vsock error: {0}")]
    VsockError(#[from] VsockError),
}

/// Structure representing the vsock device.
#[derive(Debug)]
pub struct Vsock<B> {
    cid: u64,
    pub(crate) queues: Vec<VirtQueue>,
    pub(crate) queue_events: Vec<EventFd>,
    pub(crate) backend: B,
    pub(crate) avail_features: u64,
    pub(crate) acked_features: u64,
    // This EventFd is written by `activate` so the device's event thread can complete the
    // activation (registering backend listeners) off the vCPU thread. It is one of the fds the
    // event thread polls.
    pub(crate) activate_evt: EventFd,
    activated: bool,
    mem: Option<GuestMemoryMmap>,
    irq: Option<EventFd>,
    pub rx_packet: VsockPacketRx,
    pub tx_packet: VsockPacketTx,

    /// Gates RX delivery while a TRANSPORT_RESET is awaiting guest ack.
    pub(crate) pending_event_ack: bool,
}

impl<B> Vsock<B>
where
    B: VsockBackend + Debug,
{
    /// Auxiliary function for creating a new virtio-vsock device with the given VM CID, vsock
    /// backend and empty virtio queues.
    pub fn with_queues(
        cid: u64,
        backend: B,
        queues: Vec<VirtQueue>,
    ) -> Result<Vsock<B>, VsockError> {
        let mut queue_events = Vec::new();
        for _ in 0..queues.len() {
            queue_events.push(
                EventFd::new(crate::pal::eventfd::EFD_NONBLOCK).map_err(VsockError::EventFd)?,
            );
        }

        Ok(Vsock {
            cid,
            queues,
            queue_events,
            backend,
            avail_features: AVAIL_FEATURES,
            acked_features: 0,
            activate_evt: EventFd::new(crate::pal::eventfd::EFD_NONBLOCK)
                .map_err(VsockError::EventFd)?,
            activated: false,
            mem: None,
            irq: None,
            rx_packet: VsockPacketRx::new()?,
            tx_packet: VsockPacketTx::default(),
            pending_event_ack: false,
        })
    }

    /// Create a new virtio-vsock device with the given VM CID and vsock backend.
    pub fn new(cid: u64, backend: B) -> Result<Vsock<B>, VsockError> {
        let queues: Vec<VirtQueue> = defs::VSOCK_QUEUE_SIZES
            .iter()
            .map(|&max_size| VirtQueue::new(max_size))
            .collect();
        Self::with_queues(cid, backend, queues)
    }

    /// Retrieve the cid associated with this vsock device.
    pub fn cid(&self) -> u64 {
        self.cid
    }

    /// Access the backend behind the device.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Mutable access to the backend behind the device.
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// The features the device offers the driver.
    pub fn avail_features(&self) -> u64 {
        self.avail_features
    }

    /// The features the driver has acknowledged so far.
    pub fn acked_features(&self) -> u64 {
        self.acked_features
    }

    /// Record the features the driver acknowledged.
    pub fn set_acked_features(&mut self, acked_features: u64) {
        self.acked_features = acked_features;
    }

    /// The device's virtio queues, in driver order (RX, TX, EV).
    pub fn queues(&self) -> &[VirtQueue] {
        &self.queues
    }

    /// Mutable access to the device's virtio queues.
    pub fn queues_mut(&mut self) -> &mut [VirtQueue] {
        &mut self.queues
    }

    /// The per-queue event fds the device's event thread polls.
    pub fn queue_events(&self) -> &[EventFd] {
        &self.queue_events
    }

    /// The activation event fd the device's event thread polls.
    pub fn activate_evt(&self) -> &EventFd {
        &self.activate_evt
    }

    /// Whether the driver acknowledged `feature`.
    fn has_feature(&self, feature: u64) -> bool {
        self.acked_features & feature != 0
    }

    /// Signal the guest driver that we've used some virtio buffers that it had previously made
    /// available.
    ///
    /// The mmio transport reports `VIRTIO_MMIO_INT_VRING` unconditionally (its register set has no
    /// per-queue interrupt status), so every queue shares the one interrupt event fd: the queue
    /// index is deliberately unused.
    pub fn signal_used_queue(&self, _qidx: usize) -> Result<(), DeviceError> {
        let irq = self
            .irq
            .as_ref()
            .expect("vsock: signal_used_queue on an inactive device");
        irq.write(1).map_err(DeviceError::FailedSignalingIrq)
    }

    /// Walk the driver-provided RX queue buffers and attempt to fill them up with any data that we
    /// have pending. Return `true` if the guest needs to be notified (respecting notification
    /// suppression).
    pub fn process_rx(&mut self) -> Result<bool, InvalidAvailIdx> {
        if self.pending_event_ack {
            return Ok(false);
        }

        // `mem` is `Some` exactly while the device is activated (`activate` and
        // `reset` maintain the two together); the event thread re-checks the
        // live `is_activated()` while holding this device's lock, immediately
        // before it dispatches an event to here, so a reset cannot land between
        // that check and this call.
        let mem = self
            .mem
            .as_ref()
            .expect("vsock: process_rx on an inactive device");

        let queue = &mut self.queues[RXQ_INDEX];
        let mut have_used = false;

        while let Some(head) = queue.pop_or_enable_notification()? {
            let index = head.index;
            let used_len = match self.rx_packet.parse(mem, head) {
                Ok(()) => match self.backend.recv_pkt(&mut self.rx_packet) {
                    Ok(()) => match self.rx_packet.commit_hdr() {
                        // This addition cannot overflow, because packet length
                        // is previously validated against `MAX_PKT_BUF_SIZE`
                        // bound as part of `commit_hdr()`.
                        Ok(()) => VSOCK_PKT_HDR_SIZE + self.rx_packet.hdr.len(),
                        Err(err) => {
                            eprintln!(
                                "vsock: error writing packet header to guest memory: \
                                 {err:?}. Discarding the packet."
                            );
                            0
                        }
                    },
                    Err(VsockError::NoData) => {
                        // No more data to deliver. Return the unused buffer to the queue.
                        queue.undo_pop();
                        break;
                    }
                    Err(err) => {
                        eprintln!("vsock: error receiving packet from backend: {err:?}");
                        queue.undo_pop();
                        break;
                    }
                },
                Err(err) => {
                    eprintln!("vsock: RX queue error: {err:?}. Discarding the packet.");
                    0
                }
            };

            have_used = true;
            queue.add_used(index, used_len).unwrap_or_else(|err| {
                eprintln!("error: failed to add available descriptor {index}: {err}");
            });
        }
        queue.advance_used_ring_idx();

        Ok(have_used && queue.prepare_kick())
    }

    /// Walk the driver-provided TX queue buffers, package them up as vsock packets, and send them
    /// to the backend for processing. Return `true` if the guest needs to be notified (respecting
    /// notification suppression).
    pub fn process_tx(&mut self) -> Result<bool, InvalidAvailIdx> {
        // `mem` is `Some` exactly while the device is activated (`activate` and
        // `reset` maintain the two together); the event thread re-checks the
        // live `is_activated()` while holding this device's lock, immediately
        // before it dispatches an event to here, so a reset cannot land between
        // that check and this call.
        let mem = self
            .mem
            .as_ref()
            .expect("vsock: process_tx on an inactive device");

        let queue = &mut self.queues[TXQ_INDEX];
        let mut have_used = false;

        while let Some(head) = queue.pop_or_enable_notification()? {
            let index = head.index;
            match self.tx_packet.parse(mem, head) {
                Ok(()) => (),
                Err(err) => {
                    eprintln!("vsock: error reading TX packet: {err:?}");
                    have_used = true;
                    queue.add_used(index, 0).unwrap_or_else(|err| {
                        eprintln!("error: failed to add available descriptor {index}: {err}");
                    });
                    continue;
                }
            };

            self.backend.send_pkt(&self.tx_packet);

            have_used = true;
            queue.add_used(index, 0).unwrap_or_else(|err| {
                eprintln!("error: failed to add available descriptor {index}: {err}");
            });
        }
        queue.advance_used_ring_idx();

        Ok(have_used && queue.prepare_kick())
    }

    // Send TRANSPORT_RESET_EVENT to driver. According to specs, the driver shuts down established
    // connections and the guest_cid configuration field is fetched again. Existing listen sockets
    // remain but their CID is updated to reflect the current guest_cid.
    pub fn send_transport_reset_event(&mut self) -> Result<(), DeviceError> {
        // Requires an active device: `mem` is `Some` exactly while the device is
        // activated, so a caller must hold the device and observe
        // `is_activated()` first (`activate` and `reset` maintain the two
        // together).
        let mem = self
            .mem
            .as_ref()
            .expect("vsock: send_transport_reset_event on an inactive device");

        let queue = &mut self.queues[EVQ_INDEX];
        let head = queue.pop()?.ok_or_else(|| {
            METRICS.ev_queue_event_fails.inc();
            DeviceError::VsockError(VsockError::EmptyQueue)
        })?;

        mem.write_obj::<u32>(VIRTIO_VSOCK_EVENT_TRANSPORT_RESET, head.addr)
            .unwrap_or_else(|err| {
                eprintln!("error: failed to write virtio vsock reset event: {err:?}")
            });

        queue.add_used(head.index, head.len).unwrap_or_else(|err| {
            eprintln!("error: failed to add used descriptor {}: {err}", head.index);
        });
        queue.advance_used_ring_idx();

        // The evq is only popped here, not via a drain loop, so
        // `avail_event` is not advanced by `pop_or_enable_notification`.
        // Arm it so the driver's refill of the consumed head is not
        // suppressed by EVENT_IDX.
        queue.enable_notification();

        self.pending_event_ack = true;

        self.signal_used_queue(EVQ_INDEX)?;

        Ok(())
    }

    /// The driver notified `index` (a guest `QueueNotify` write, decoded on the vCPU thread):
    /// wake the device's event thread by writing that queue's event fd and return immediately.
    /// All ring processing happens on the event thread, never on the vCPU thread.
    pub fn notify_queue(&mut self, index: usize) -> Result<(), VsockError> {
        // An out-of-range index is dropped: nothing to wake, and failing here would let a
        // guest-controlled register write fault the VMM.
        if let Some(evt) = self.queue_events.get(index) {
            evt.write(1).map_err(VsockError::EventFd)?;
        }
        Ok(())
    }

    /// Activate the device against `mem`, raising its interrupt through `irq`.
    pub fn activate(&mut self, mem: GuestMemoryMmap, irq: EventFd) -> Result<(), ActivateError> {
        assert!(!self.activated);

        for q in self.queues.iter_mut() {
            q.initialize(&mem)
                .map_err(ActivateError::QueueMemoryError)?;
        }

        if self.queues.len() != defs::VSOCK_NUM_QUEUES {
            METRICS.activate_fails.inc();
            return Err(ActivateError::QueueMismatch {
                expected: defs::VSOCK_NUM_QUEUES,
                got: self.queues.len(),
            });
        }

        if self.has_feature(VIRTIO_RING_F_EVENT_IDX) {
            for queue in &mut self.queues {
                queue.enable_notif_suppression();
            }
        }

        self.backend.activate().map_err(|err| {
            METRICS.activate_fails.inc();
            ActivateError::VsockBackend(err)
        })?;

        if self.activate_evt.write(1).is_err() {
            METRICS.activate_fails.inc();
            return Err(ActivateError::EventFd(std::io::Error::last_os_error()));
        }

        self.mem = Some(mem);
        self.irq = Some(irq);
        self.activated = true;

        Ok(())
    }

    /// Whether the device is activated.
    pub fn is_activated(&self) -> bool {
        self.activated
    }

    /// Reset the device: drop the guest memory/interrupt handles, reset the backend, and clear
    /// in-flight packet state.
    pub fn reset(&mut self) {
        self.backend.reset();
        self.rx_packet.clear();
        self.tx_packet.clear();
        self.pending_event_ack = false;
        self.activated = false;
        self.mem = None;
        self.irq = None;
    }

    /// The device configuration space: the guest CID, little-endian.
    pub fn config_as_bytes(&self) -> &[u8] {
        // ByteValued::as_slice() gives native-endian bytes. Firecracker only
        // targets little-endian platforms, matching virtio's LE config space;
        // the static assert below makes a big-endian target a compile error
        // rather than a silent mis-serialization.
        const _: () = assert!(
            cfg!(target_endian = "little"),
            "virtio config requires a little-endian target"
        );
        self.cid.as_slice()
    }

    /// The vsock device has no writable configuration space: a guest write is logged and dropped.
    pub fn write_config(&mut self, offset: u64, data: &[u8]) {
        METRICS.cfg_fails.inc();
        eprintln!(
            "warning: vsock: guest driver attempted to write device config (offset={offset:#x}, len={:#x})",
            data.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::pal::{AsRawFd, RawFd};

    use super::*;
    use crate::devices::virtio::queue::{VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE};
    use crate::devices::virtio::test_utils::{VirtQueue, default_mem};
    use crate::devices::virtio::vsock::packet::VsockPacketHeader;
    use crate::devices::virtio::vsock::{VsockChannel, VsockEpollListener};
    use crate::vstate::memory::{GuestAddress, GuestMemoryMmap};

    /// A minimal in-memory backend that answers `recv_pkt` from a queue of
    /// pre-staged headers and records every TX packet's header+payload it is
    /// handed. This is the shape the real muxer backend will fill in.
    #[derive(Debug, Default)]
    struct FakeBackend {
        pending_rx: bool,
        rx_len: u32,
        rx_op: u16,
        received_tx: Vec<(u16, u32)>,
    }

    impl VsockChannel for FakeBackend {
        fn recv_pkt(&mut self, pkt: &mut VsockPacketRx) -> Result<(), VsockError> {
            if !self.pending_rx {
                return Err(VsockError::NoData);
            }
            self.pending_rx = false;
            pkt.hdr
                .set_src_cid(defs::uapi::VSOCK_HOST_CID)
                .set_dst_cid(3)
                .set_src_port(2000)
                .set_dst_port(1000)
                .set_len(self.rx_len)
                .set_type(defs::uapi::VSOCK_TYPE_STREAM)
                .set_op(self.rx_op);
            Ok(())
        }

        fn send_pkt(&mut self, pkt: &VsockPacketTx) {
            self.received_tx.push((pkt.hdr.op(), pkt.hdr.len()));
        }

        fn has_pending_rx(&self) -> bool {
            self.pending_rx
        }
    }

    impl VsockEpollListener for FakeBackend {
        fn get_polled_evset(&self) -> crate::pal::epoll::EventSet {
            crate::pal::epoll::EventSet::IN
        }

        fn notify(&mut self, _evset: crate::pal::epoll::EventSet) {}
    }

    impl VsockBackend for FakeBackend {
        fn activate(&mut self) -> Result<(), VsockError> {
            Ok(())
        }

        fn reset(&mut self) {}
    }

    // The real backend is a host socket; the fake has no fd, so it reports one
    // that is never polled by these tests.
    impl AsRawFd for FakeBackend {
        fn as_raw_fd(&self) -> RawFd {
            0
        }
    }

    /// Lay three virtqueues out in `mem` (RX at 0, TX at 0x2000, EV at 0x4000)
    /// and return a device activated against a clone of `mem` together with the
    /// guest-side virtqueues, which borrow `mem` from the caller.
    fn activated_ctx<'a>(mem: &'a GuestMemoryMmap) -> (Vsock<FakeBackend>, Vec<VirtQueue<'a>>) {
        let vqs = vec![
            VirtQueue::new(GuestAddress(0), mem, 16),
            VirtQueue::new(GuestAddress(0x2000), mem, 16),
            VirtQueue::new(GuestAddress(0x4000), mem, 16),
        ];
        let queues = vqs.iter().map(|vq| vq.create_queue()).collect();
        let mut device = Vsock::with_queues(3, FakeBackend::default(), queues).unwrap();
        let irq = EventFd::new(crate::pal::eventfd::EFD_NONBLOCK).unwrap();
        device.activate((*mem).clone(), irq).unwrap();
        (device, vqs)
    }

    #[test]
    fn activation_reports_cid_and_features() {
        let device = Vsock::new(3, FakeBackend::default()).unwrap();
        assert_eq!(device.cid(), 3);
        assert_eq!(device.avail_features(), AVAIL_FEATURES);
        assert_eq!(device.queues().len(), defs::VSOCK_NUM_QUEUES);
        assert_eq!(device.config_as_bytes(), 3u64.as_slice());
    }

    #[test]
    fn process_tx_delivers_guest_packets_to_backend() {
        let mem = default_mem();
        let (mut device, vqs) = activated_ctx(&mem);
        let vq = &vqs[TXQ_INDEX];

        // A TX packet: header (op=RW, len=4) + payload.
        let mut hdr = VsockPacketHeader::default();
        hdr.set_src_cid(3)
            .set_dst_cid(defs::uapi::VSOCK_HOST_CID)
            .set_len(4)
            .set_type(defs::uapi::VSOCK_TYPE_STREAM)
            .set_op(defs::uapi::VSOCK_OP_RW);
        mem.write_slice(hdr.as_slice(), GuestAddress(0x8000))
            .unwrap();
        vq.set_desc(0, 0x8000, VSOCK_PKT_HDR_SIZE, VIRTQ_DESC_F_NEXT, 1);
        vq.set_desc(1, 0x9000, 4, 0, 0);
        // The driver published the chain.
        vq.publish(0);
        // The guest notified the TX queue.
        device.notify_queue(TXQ_INDEX).unwrap();

        assert!(device.process_tx().unwrap());
        assert_eq!(
            device.backend().received_tx,
            vec![(defs::uapi::VSOCK_OP_RW, 4)]
        );
        assert_eq!(vq.used_idx(), 1);
        assert_eq!(vq.used_elem(0), (0, 0));
    }

    #[test]
    fn process_rx_delivers_host_packets_to_guest() {
        let mem = default_mem();
        let (mut device, vqs) = activated_ctx(&mem);
        let vq = &vqs[RXQ_INDEX];

        // An RX buffer: write-only header + write-only payload.
        vq.set_desc(
            0,
            0x8000,
            VSOCK_PKT_HDR_SIZE,
            VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT,
            1,
        );
        vq.set_desc(1, 0x9000, 4, VIRTQ_DESC_F_WRITE, 0);
        vq.publish(0);

        // Backend has a 4-byte RW packet pending.
        device.backend_mut().pending_rx = true;
        device.backend_mut().rx_len = 4;
        device.backend_mut().rx_op = defs::uapi::VSOCK_OP_RW;

        assert!(device.process_rx().unwrap());

        let read_back: VsockPacketHeader = mem.read_obj(GuestAddress(0x8000)).unwrap();
        assert_eq!(read_back.src_cid(), defs::uapi::VSOCK_HOST_CID);
        assert_eq!(read_back.dst_cid(), 3);
        assert_eq!(read_back.len(), 4);
        assert_eq!(vq.used_idx(), 1);
    }

    #[test]
    fn send_transport_reset_event_publishes_and_arms_gate() {
        let mem = default_mem();
        let (mut device, vqs) = activated_ctx(&mem);
        let vq = &vqs[EVQ_INDEX];

        vq.set_desc(0, 0x8000, 4, VIRTQ_DESC_F_WRITE, 0);
        vq.publish(0);

        assert!(!device.pending_event_ack);
        device.send_transport_reset_event().unwrap();
        assert!(device.pending_event_ack);

        let payload: u32 = mem.read_obj(GuestAddress(0x8000)).unwrap();
        assert_eq!(payload, VIRTIO_VSOCK_EVENT_TRANSPORT_RESET);
        assert_eq!(vq.used_idx(), 1);
    }

    #[test]
    fn reset_clears_backend_and_state() {
        let mem = default_mem();
        let (mut device, _vqs) = activated_ctx(&mem);
        assert!(device.is_activated());
        device.reset();
        assert!(!device.is_activated());
        assert!(!device.pending_event_ack);
    }
}
