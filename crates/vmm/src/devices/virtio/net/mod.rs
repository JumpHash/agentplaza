// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port (PLAN 22.6, W58). Upstream Firecracker has no network device in its
// userspace model that a multi-VM topology could use: its virtio-net needs a tap device on the
// host, and this port has no root and no tap. What lives here instead is the device model (the
// virtio 1.0 `virtio-net` queues and header), a userspace switch that forwards frames within a
// private segment, and the overlay that carries a segment's frames between two switches.

//! A virtio-net device for lab topologies: one private segment per topology, shared by its VMs.
//!
//! * [`device`] — the device's queues: the receive and transmit rings, descriptor chains
//!   walked and filled over guest memory, and the `virtio_net_hdr` every frame carries.
//! * [`mmio_device`] — the device on the MMIO transport, with the thread that hands frames the
//!   switch delivers to the guest's receive ring.
//! * [`switch`] — the host side: a userspace switch whose segments forward frames by destination
//!   MAC within the segment, flood broadcasts and unknown unicast inside it, and reach the outside
//!   only through the segment's gateway, which accepts the egress proxy's connections and nothing
//!   else.
//! * [`overlay`] — a segment's frames carried between two switches over a byte stream (the node
//!   tunnel or the mesh between nodes, PLAN 17.4), split-horizon so no frame loops.
//! * [`plan`] — a segment's addresses and the guest commands that bring its interface up.
//!
//! The minimal feature set is `VIRTIO_NET_F_MAC` (the MAC in the configuration space) and
//! `VIRTIO_F_VERSION_1` (the modern transport, and with it the 12-byte header that carries
//! `num_buffers`). The device offers no checksum, segmentation or control-queue features: the
//! guest sends and receives whole Ethernet frames, and the header is zeros.
//!
//! Only lab topologies carry a net device. The builder refuses one on any session VM
//! (`VmKind::Session`, a fork of a session included), see [`crate::spawner`].

pub mod device;
pub mod mmio_device;
pub mod overlay;
pub mod plan;
pub mod switch;

use crate::devices::virtio::queue::{InvalidAvailIdx, QueueError};

/// `VIRTIO_ID_NET`: the virtio type id of a network device.
pub const VIRTIO_ID_NET: u32 = 1;

/// `VIRTIO_NET_F_MAC`: the device has a MAC address in its configuration space.
pub const VIRTIO_NET_F_MAC: u64 = 1 << 5;
/// `VIRTIO_F_VERSION_1`: the modern (virtio 1.0) interface, and the header with `num_buffers`.
pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;

/// The receive queue's index (the guest's frames come in here).
pub const RX_QUEUE: u32 = 0;
/// The transmit queue's index (the guest's frames go out here).
pub const TX_QUEUE: u32 = 1;
/// How many queues the device has: receive and transmit, no control queue.
pub const NET_QUEUE_NUM: usize = 2;
/// The ring size both queues are offered with.
pub const NET_QUEUE_SIZE: u16 = 256;

/// Bytes of the `virtio_net_hdr` every frame is preceded by (`VIRTIO_F_VERSION_1` layout:
/// flags, gso type, header length, gso size, checksum start and offset, and `num_buffers`).
pub const NET_HDR_LEN: usize = 12;
/// Offset of `num_buffers` inside the header (a little-endian `u16`).
const NUM_BUFFERS_OFFSET: usize = 10;

/// A MAC address.
pub type Mac = [u8; 6];

/// Bytes of an Ethernet II header: destination, source and EtherType.
pub const ETH_HDR_LEN: usize = 14;
/// The largest frame the device or the overlay carries, header included: a jumbo frame, with
/// room over it for a VLAN tag. Larger frames are refused rather than split.
pub const MAX_FRAME_LEN: usize = 9216 + 4;

/// Errors of the net device, the switch and the overlay.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    /// The segment {0:?} does not exist: a port attaches to a segment the switch has created
    #[error("the segment {0:?} does not exist on this switch")]
    UnknownSegment(String),
    /// A segment has one gateway: the egress proxy's port
    #[error("the segment {0:?} already has a gateway")]
    GatewayTaken(String),
    /// The MAC {mac} is already attached to segment {segment:?}
    #[error("the MAC {mac} is already attached to the segment {segment:?}")]
    MacInUse {
        /// The MAC that was offered twice.
        mac: String,
        /// The segment both ports are on.
        segment: String,
    },
    /// A frame's source address is not the port's own MAC: the frame is refused, not forwarded
    #[error("a frame from a port with MAC {mac} names another source")]
    SpoofedSource {
        /// The port's MAC.
        mac: String,
    },
    /// The driver's frame is larger than the device carries
    #[error("a frame of {0} bytes is larger than the device carries")]
    FrameTooLarge(usize),
    /// The frame is shorter than an Ethernet header
    #[error("a frame of {0} bytes is shorter than an Ethernet header")]
    FrameTooShort(usize),
    /// The overlay's stream broke or sent a frame it cannot have: {0}
    #[error("the overlay stream: {0}")]
    Overlay(String),
    /// The virtqueue is not in a state the device can use: {0}
    #[error("the net device's virtqueue is unusable: {0}")]
    Queue(#[from] QueueError),
    /// The driver asked for more descriptors than the ring can hold: {0}
    #[error(transparent)]
    AvailRing(#[from] InvalidAvailIdx),
    /// The device's guest memory access failed: {0}
    #[error("the net device could not access guest memory: {0}")]
    GuestMemory(String),
    /// The device could not be activated: {0}
    #[error("the net device could not be activated: {0}")]
    Activate(String),
}

/// Attaches a guest interface with `mac` to `segment` on `switch`, and returns the device a transport
/// drives. The port is detached when the device is dropped.
pub fn attach(
    switch: &std::sync::Arc<switch::Switch>,
    segment: &str,
    mac: Mac,
) -> Result<mmio_device::NetMmioDevice, NetError> {
    let (port, frames) = switch.attach(segment, switch::Endpoint::Guest { mac })?;
    Ok(mmio_device::NetMmioDevice::new(mac, port, frames))
}

/// Formats a MAC as the colon-separated hex the guest's tools print.
pub fn mac_text(mac: &Mac) -> String {
    mac.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Sets `num_buffers` in a zeroed header, which the receive path writes as one.
pub(crate) fn header_with_buffers(num_buffers: u16) -> [u8; NET_HDR_LEN] {
    let mut header = [0u8; NET_HDR_LEN];
    header[NUM_BUFFERS_OFFSET..NET_HDR_LEN].copy_from_slice(&num_buffers.to_le_bytes());
    header
}
