// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/devices/virtio/block/virtio/{mod.rs,
// device.rs, request.rs, test_utils.rs} (upstream commit
// fd9fc7a9f164b73002d22065d17c150d8c262d91). The virtio-block device model and
// its request parsers are upstream's; trimmed here: the async io_uring engine
// (only synchronous `pread`/`pwrite`-equivalent transfers remain), the rate
// limiter, the device metrics, the snapshot/persist support, the event-manager
// subscriber (the device processes its queue on the vCPU thread, in the
// transport's notification), DISCARD/WRITE_ZEROES support, the `GET_ID` file
// metadata (this port returns zeroes: no host data reaches the guest), the
// PCI transport and the `displaydoc` derives. Upstream's `ByteValued` PODs
// become `zerocopy` little-endian structs, as in `crates/acpi-tables` and
// `acpi.rs` of this port.

//! A virtio-block device on the MMIO transport: the guest's workspace volume.
//!
//! One request queue ([`BLOCK_QUEUE_NUM`] is 1), a configuration space in
//! which the driver reads the volume's capacity, and the request protocol of
//! virtio 1.0 §5.2: the driver publishes a descriptor chain whose head is a
//! [`request::RequestHeader`], whose middle descriptor carries the data and
//! whose last descriptor is a single status byte the device writes back.
//!
//! The parser is [`request::Request::parse`], ported from upstream and the
//! security-critical part of the device: every field comes from guest memory
//! (an unaligned `u16`/`u32`/`u64` header, a discard range table), every
//! descriptor's direction and length is checked against the request type, and
//! every sector address is checked against the volume's size before any I/O.
//! A chain that does not validate is answered with a status byte and an I/O
//! error, never with a panic and never with an out-of-range access.
//!
//! The device is synchronous: [`device::VirtioBlock::process_queue`] runs on
//! the vCPU thread, inside the transport's `QueueNotify` handling, and returns
//! once the queue is drained. The volume is a host file the VMM opens
//! read-write — the only writable queue in this port (kernel and root
//! filesystem images are read-only, and there is no network device at all).

pub mod device;
pub mod mmio_device;
pub mod request;

#[cfg(test)]
pub mod test_utils;

use crate::devices::virtio::queue::{InvalidAvailIdx, QueueError};

/// Whether the device has a request queue (there is exactly one).
pub const BLOCK_QUEUE_NUM: usize = 1;

/// `VIRTIO_ID_BLOCK`: the virtio type id of a block device.
pub const VIRTIO_ID_BLOCK: u32 = 2;

/// Sector shift of the block device: the protocol addresses 512-byte sectors.
pub const SECTOR_SHIFT: u8 = 9;
/// Size of one sector, the unit every request's offset and length is given in.
pub const SECTOR_SIZE: u32 = 1 << SECTOR_SHIFT;

/// The ring size the request queue is offered with. It matches the userspace
/// virtqueue substrate's limit (`queue::FIRECRACKER_MAX_QUEUE_SIZE`); a request
/// spans two or three descriptors, so a 256-entry ring carries ≥ 85 requests.
pub const BLOCK_QUEUE_SIZE: u16 = 256;

/// The largest discard range [`request::Request::parse`] accepts, in sectors
/// (128 MiB, upstream's bound). The device does not offer
/// `VIRTIO_BLK_F_DISCARD`, so an honest driver never sends one and a discard
/// request is refused; the bound keeps the parser's validation of a hostile
/// range identical to upstream's.
pub const MAX_DISCARD_SECTORS: u32 = (128 << 20) / SECTOR_SIZE;

/// Errors of the block device.
///
/// Every one of these is reported, never panicked on: the paths that produce
/// them (a descriptor chain, a file, an unaligned header) are guest-reachable
/// or host I/O, and neither may fault the VMM.
#[derive(Debug, thiserror::Error)]
pub enum VirtioBlockError {
    /// The backing file could not be opened or its size read: {0}: {1}
    #[error("the workspace image {1} could not be opened: {0}")]
    BackingFile(#[source] std::io::Error, String),
    /// Guest gave us too few descriptors in a descriptor chain.
    #[error("the driver's descriptor chain is too short")]
    DescriptorChainTooShort,
    /// Guest gave us a descriptor that was too short to use.
    #[error("the driver's descriptor is too short")]
    DescriptorLengthTooSmall,
    /// Guest gave us bad memory addresses: {0}
    #[error("the driver's buffer is not in guest memory: {0}")]
    GuestMemory(#[from] vm_memory::GuestMemoryError),
    /// The data length is invalid for the request type.
    #[error("the request's data length is invalid")]
    InvalidDataLength,
    /// The request would reach past the end of the volume.
    #[error("the request reaches past the end of the volume")]
    InvalidOffset,
    /// Guest sent us a read-only descriptor where the protocol says to write.
    #[error("the driver sent a read-only descriptor where the protocol writes")]
    UnexpectedReadOnlyDescriptor,
    /// Guest sent us a write-only descriptor where the protocol says to read.
    #[error("the driver sent a write-only descriptor where the protocol reads")]
    UnexpectedWriteOnlyDescriptor,
    /// The virtqueue is not in a state this device can use: {0}
    #[error("the block device's virtqueue is unusable: {0}")]
    Queue(#[from] QueueError),
    /// The driver asked for more descriptors than the ring can hold: {0}
    #[error(transparent)]
    InvalidAvailIdx(#[from] InvalidAvailIdx),
}

/// Errors of [`device::VirtioBlock::activate`], which the transport maps into
/// its own [`crate::devices::virtio::VirtioError`].
#[derive(Debug, thiserror::Error)]
pub enum ActivateError {
    /// The queue the driver programmed could not be resolved in guest memory: {0}
    #[error("failed to initialize the block device's virtqueue: {0}")]
    QueueMemoryError(#[from] QueueError),
    /// The device's interrupt event fd could not be cloned: {0}
    #[error("failed to prepare the block device's interrupt: {0}")]
    EventFd(#[from] std::io::Error),
}
