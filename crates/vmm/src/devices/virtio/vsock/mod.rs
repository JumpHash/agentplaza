// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/devices/virtio/vsock/mod.rs (trimmed:
// the `csm`/`unix` muxer backend and `event_handler` land with the next slice;
// `persist`/snapshot support is dropped, so `VsockError` loses its
// `VirtioState`/`VsockUdsBackend` variants, to be re-added with the backend;
// `displaydoc` becomes explicit `thiserror` messages). The traits below are
// the frozen interface the muxer backend implements.

//! The Firecracker vsock device aims to provide full virtio-vsock support to
//! software running inside the guest VM, while bypassing vhost kernel code on the
//! host. To that end, Firecracker implements the virtio-vsock device model, and
//! mediates communication between AF_UNIX sockets (on the host end) and AF_VSOCK
//! sockets (on the guest end).

mod device;
pub mod metrics;
mod packet;

use std::os::unix::io::AsRawFd;

use vm_memory::GuestMemoryError;
use vmm_sys_util::epoll::EventSet;

pub use self::defs::VSOCK_DEV_ID;
pub use self::device::{ActivateError, Vsock};
use self::packet::{VsockPacketRx, VsockPacketTx};
use super::iov_deque::IovDequeError;
use crate::devices::virtio::iovec::IoVecError;

/// The vhost-backed vsock device (the boot path's MMIO device), re-exported
/// here so the existing `vsock::VsockDevice` import keeps resolving while the
/// two device flavours coexist. The next slice wires in [`Vsock`] and removes
/// this re-export together with the vhost device.
pub use crate::devices::virtio::vhost_device::VsockDevice;

mod defs {
    use crate::devices::virtio::queue::FIRECRACKER_MAX_QUEUE_SIZE;

    /// Device ID used in MMIO device identification.
    /// Because Vsock is unique per-vm, this ID can be hardcoded.
    pub const VSOCK_DEV_ID: &str = "vsock";

    /// Number of virtio queues.
    pub const VSOCK_NUM_QUEUES: usize = 3;

    /// Virtio queue sizes, in number of descriptor chain heads.
    /// There are 3 queues for a virtio device (in this order): RX, TX, Event
    pub const VSOCK_QUEUE_SIZES: [u16; VSOCK_NUM_QUEUES] = [
        FIRECRACKER_MAX_QUEUE_SIZE,
        FIRECRACKER_MAX_QUEUE_SIZE,
        FIRECRACKER_MAX_QUEUE_SIZE,
    ];

    /// Max vsock packet data/buffer size.
    pub const MAX_PKT_BUF_SIZE: u32 = 64 * 1024;

    /// Vsock packet op/flag/type constants from `/include/uapi/linux/virtio_vsock.h`.
    /// Only a few are read by the device frontend today; the muxer backend uses
    /// the rest.
    #[allow(dead_code)]
    pub mod uapi {
        /// Vsock packet operation IDs.
        /// Defined in `/include/uapi/linux/virtio_vsock.h`.
        ///
        /// Connection request.
        pub const VSOCK_OP_REQUEST: u16 = 1;
        /// Connection response.
        pub const VSOCK_OP_RESPONSE: u16 = 2;
        /// Connection reset.
        pub const VSOCK_OP_RST: u16 = 3;
        /// Connection clean shutdown.
        pub const VSOCK_OP_SHUTDOWN: u16 = 4;
        /// Connection data (read/write).
        pub const VSOCK_OP_RW: u16 = 5;
        /// Flow control credit update.
        pub const VSOCK_OP_CREDIT_UPDATE: u16 = 6;
        /// Flow control credit update request.
        pub const VSOCK_OP_CREDIT_REQUEST: u16 = 7;

        /// Vsock packet flags.
        /// Defined in `/include/uapi/linux/virtio_vsock.h`.
        ///
        /// Valid with a VSOCK_OP_SHUTDOWN packet: the packet sender will receive no more data.
        pub const VSOCK_FLAGS_SHUTDOWN_RCV: u32 = 1;
        /// Valid with a VSOCK_OP_SHUTDOWN packet: the packet sender will send no more data.
        pub const VSOCK_FLAGS_SHUTDOWN_SEND: u32 = 2;

        /// Vsock packet type.
        /// Defined in `/include/uapi/linux/virtio_vsock.h`.
        ///
        /// Stream / connection-oriented packet (the only currently valid type).
        pub const VSOCK_TYPE_STREAM: u16 = 1;

        pub const VSOCK_HOST_CID: u64 = 2;
    }
}

/// Vsock device related errors.
#[derive(Debug, thiserror::Error)]
#[rustfmt::skip]
pub enum VsockError {
    /// The total length of the descriptor chain ({0}) is too short to hold a packet of length {1} + header.
    #[error("the total length of the descriptor chain ({0}) is too short to hold a packet of length {1} + header")]
    DescChainTooShortForPacket(u32, u32),
    /// Empty queue.
    #[error("empty queue")]
    EmptyQueue,
    /// EventFd error: {0}.
    #[error("eventfd error: {0}")]
    EventFd(std::io::Error),
    /// Chained GuestMemoryMmap error: {0}.
    #[error("guest memory error: {0}")]
    GuestMemoryMmap(GuestMemoryError),
    /// Bounds check failed on guest memory pointer.
    #[error("bounds check failed on guest memory pointer")]
    GuestMemoryBounds,
    /// The total length of the descriptor chain ({0}) is less than the number of bytes required to hold a vsock packet header.
    #[error("the total length of the descriptor chain ({0}) is less than the number of bytes required to hold a vsock packet header")]
    DescChainTooShortForHeader(usize),
    /// The descriptor chain length was greater than the max ([u32::MAX]).
    #[error("the descriptor chain length was greater than the max (u32::MAX)")]
    DescChainOverflow,
    /// The vsock header `len` field holds an invalid value: {0}.
    #[error("the vsock header `len` field holds an invalid value: {0}")]
    InvalidPktLen(u32),
    /// A data fetch was attempted when no data was available.
    #[error("a data fetch was attempted when no data was available")]
    NoData,
    /// A data buffer was expected for the provided packet, but it is missing.
    #[error("a data buffer was expected for the provided packet, but it is missing")]
    PktBufMissing,
    /// Encountered an unexpected write-only virtio descriptor.
    #[error("encountered an unexpected write-only virtio descriptor")]
    UnreadableDescriptor,
    /// Encountered an unexpected read-only virtio descriptor.
    #[error("encountered an unexpected read-only virtio descriptor")]
    UnwritableDescriptor,
    /// Underlying IovDeque error: {0}.
    #[error("underlying iov deque error: {0}")]
    IovDeque(IovDequeError),
    /// Tried to push to full IovDeque.
    #[error("tried to push to a full iov deque")]
    IovDequeOverflow,
}

impl From<IoVecError> for VsockError {
    fn from(value: IoVecError) -> Self {
        match value {
            IoVecError::WriteOnlyDescriptor => VsockError::UnreadableDescriptor,
            IoVecError::ReadOnlyDescriptor => VsockError::UnwritableDescriptor,
            IoVecError::GuestMemory(err) => VsockError::GuestMemoryMmap(err),
            IoVecError::OverflowedDescriptor => VsockError::DescChainOverflow,
            IoVecError::IovDeque(err) => VsockError::IovDeque(err),
            IoVecError::IovDequeOverflow => VsockError::IovDequeOverflow,
        }
    }
}

/// A passive, event-driven object, that needs to be notified whenever an epoll-able event occurs.
/// An event-polling control loop will use `as_raw_fd()` and `get_polled_evset()` to query
/// the listener for the file descriptor and the set of events it's interested in. When such an
/// event occurs, the control loop will route the event to the listener via `notify()`.
pub trait VsockEpollListener: AsRawFd {
    /// Get the set of events for which the listener wants to be notified.
    fn get_polled_evset(&self) -> EventSet;

    /// Notify the listener that one ore more events have occurred.
    fn notify(&mut self, evset: EventSet);
}

/// Any channel that handles vsock packet traffic: sending and receiving packets. Since we're
/// implementing the device model here, our responsibility is to always process the sending of
/// packets (i.e. the TX queue). So, any locally generated data, addressed to the driver (e.g.
/// a connection response or RST), will have to be queued, until we get to processing the RX queue.
///
/// Note: `recv_pkt()` and `send_pkt()` are named analogous to `Read::read()` and `Write::write()`,
///       respectively. I.e.
///       - `recv_pkt(&mut pkt)` will read data from the channel, and place it into `pkt`; and
///       - `send_pkt(&pkt)` will fetch data from `pkt`, and place it into the channel.
pub trait VsockChannel {
    /// Read/receive an incoming packet from the channel.
    fn recv_pkt(&mut self, pkt: &mut VsockPacketRx) -> Result<(), VsockError>;

    /// Write/send a packet through the channel.
    ///
    /// The packet is always consumed (its virtio TX buffers can be returned to the guest):
    /// host-side back-pressure is absorbed by buffering, and unrecoverable errors terminate
    /// the offending connection rather than halting queue processing.
    fn send_pkt(&mut self, pkt: &VsockPacketTx);

    /// Checks whether there is pending incoming data inside the channel, meaning that a subsequent
    /// call to `recv_pkt()` won't fail.
    fn has_pending_rx(&self) -> bool;
}

/// The vsock backend, which is basically an epoll-event-driven vsock channel.
/// Currently, the only implementation we have is `crate::devices::virtio::unix::muxer::VsockMuxer`,
/// which translates guest-side vsock connections to host-side Unix domain socket connections.
pub trait VsockBackend: VsockChannel + VsockEpollListener + Send {
    /// Activate the backend, adding its listeners to the poll set.
    fn activate(&mut self) -> Result<(), VsockError>;

    /// Reset the backend, dropping all active connections and removing its listeners
    /// from the poll set.
    fn reset(&mut self);
}
