// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/devices/virtio/mod.rs (trimmed: the
// PCI transport, the userspace vsock muxer (`csm`/`unix`) and the device
// metrics are dropped or replaced by a local shim; there is no network device
// by design). The userspace vsock core below is ported so the VMM can mediate
// vsock itself instead of handing the rings to the kernel's vhost-vsock; that
// vhost device is retired and is not on the boot path.

//! Virtio devices on the MMIO transport.
//!
//! * [`mmio`] — the transport: the register set a driver programs, the device
//!   status state machine, and the [`mmio::VirtioDevice`] trait the transport
//!   drives.
//! * [`queue`], [`iovec`], [`iov_deque`] — the userspace virtqueue substrate:
//!   descriptor-chain walking and scatter/gather buffers over guest memory.
//! * [`vsock`] — the userspace virtio-vsock device: the
//!   [`vsock::VsockBackend`] interface, the packet model, the connection state
//!   machine ([`vsock::csm`]), the Unix-socket muxer ([`vsock::unix`]), the
//!   [`vsock::Vsock`] device frontend and its [`vsock::event_thread`]. This is
//!   the device the boot path uses: the VMM, not the host kernel, decides
//!   which host sockets a guest may reach.
//! * [`vhost`], [`vhost_device`] — the retired `/dev/vhost-vsock` backend and
//!   its MMIO wrapper. They are **not on the boot path** (nothing constructs
//!   `VsockDevice` or opens `/dev/vhost-vsock`): they survive only for their
//!   tests and for the `ack_feature_page`/`read_guest_cid_config` helpers the
//!   userspace device's [`vsock::mmio_device`] adapter reuses.
//!
//! Splitting the transport from the backend is what makes the device testable
//! without a hypervisor: the transport's tests drive a mock
//! [`mmio::VirtioDevice`], and the muxer's tests need only host Unix sockets
//! (see `FACTS.md` next to this module for what was measured on a real host).

// The balloon is Linux only (it releases pages with madvise(2)); the block
// device builds everywhere the virtio devices do (the workspace and data
// volumes of the KVM, WHP and Hypervisor.framework backends).
#[cfg(target_os = "linux")]
pub mod balloon;
pub mod block;
pub mod iov_deque;
pub mod iovec;
pub mod mmio;
pub mod queue;
#[cfg(target_os = "linux")]
pub mod vhost;
#[cfg(target_os = "linux")]
pub mod vhost_device;
pub mod vsock;

/// Test helpers (guest-side virtqueues over real guest memory), test-only.
#[cfg(test)]
pub mod test_utils;

/// When the driver initializes the device, it lets the device know about the
/// completed stages using the Device Status Field.
///
/// These following consts are defined in the order in which the bits would
/// typically be set by the driver. INIT -> ACKNOWLEDGE -> DRIVER and so on.
///
/// This module is a 1:1 mapping for the Device Status Field in the virtio 1.0
/// specification, section 2.1.
pub(crate) mod device_status {
    /// The device's reset state (status 0).
    pub const INIT: u32 = 0;
    /// The driver has acknowledged the device.
    pub const ACKNOWLEDGE: u32 = 1;
    /// The driver knows how to drive the device.
    pub const DRIVER: u32 = 2;
    /// The driver has accepted the device's features.
    pub const FEATURES_OK: u32 = 8;
    /// The driver is done and the device may be used.
    pub const DRIVER_OK: u32 = 4;
    /// The driver gave up on the device.
    pub const FAILED: u32 = 128;
    /// The device wants to be reset and re-initialized.
    pub const DEVICE_NEEDS_RESET: u32 = 64;
}

/// Errors of the virtio device model.
///
/// Everything a driver can do to a device — including a driver that
/// acknowledges features that were never offered or activates with queues it
/// never configured — is reported through this type, never by panicking: the
/// register decode path is guest-controlled.
#[derive(Debug, thiserror::Error)]
pub enum VirtioError {
    /// The driver acknowledged features the device does not offer: {flags:#x}
    #[error("the driver acknowledged features the device does not offer: {flags:#x}")]
    UnsupportedFeatures {
        /// The offending feature set.
        flags: u64,
    },
    /// The driver did not acknowledge {feature}, which this device requires
    #[error("the driver did not acknowledge the required feature {feature}")]
    RequiredFeatureNotAcked {
        /// Name of the feature the device needs.
        feature: &'static str,
    },
    /// The device has no queue {0}
    #[error("the device has no queue {0}")]
    NoSuchQueue(u32),
    /// A snapshot of a device of another type cannot restore into this one
    #[error("the snapshot is of virtio device type {found}, not {expected}")]
    SnapshotDeviceType {
        /// The type of the device being restored into.
        expected: u32,
        /// The type recorded in the snapshot.
        found: u32,
    },
    /// A snapshot with another number of queues cannot restore into this device
    #[error("the snapshot has {found} virtqueues, but the device has {expected}")]
    SnapshotQueueCount {
        /// The number of queues the device has.
        expected: usize,
        /// The number of queues recorded in the snapshot.
        found: usize,
    },
    /// A snapshot field no sequence of driver writes could have left: refused, not clamped
    #[error("the snapshot of virtio device type {device_type} has an invalid {field}: {value:#x}")]
    SnapshotField {
        /// The virtio device type the snapshot is of.
        device_type: u32,
        /// The register, and the queue it is for, that the snapshot recorded.
        field: String,
        /// The value the snapshot recorded.
        value: u64,
    },
    /// The vhost backend: {0}
    #[error("vhost: {0}")]
    #[cfg(target_os = "linux")]
    Vhost(#[from] vhost::VhostError),
    /// The userspace vsock device: {0}
    #[error("vsock device: {0}")]
    Vsock(#[from] crate::devices::virtio::vsock::VsockError),
    /// Failed to activate the userspace vsock device: {0}
    #[error("failed to activate the userspace vsock device: {0}")]
    VsockActivate(#[from] crate::devices::virtio::vsock::ActivateError),
    /// The block device: {0}
    #[error("block device: {0}")]
    Block(#[from] crate::devices::virtio::block::VirtioBlockError),
    /// Failed to activate the block device: {0}
    #[error("failed to activate the block device: {0}")]
    BlockActivate(#[from] crate::devices::virtio::block::ActivateError),
    /// The balloon device: {0}
    #[error("balloon device: {0}")]
    #[cfg(target_os = "linux")]
    Balloon(#[from] crate::devices::virtio::balloon::BalloonError),
}

/// Records the features the driver acknowledged for one page.
///
/// `offered` is the device's full feature set, `acked` what the driver has
/// acknowledged so far, and `page`/`value` what it just wrote to the feature
/// registers. Bits that were not offered are an error: a driver and a device
/// that disagree about the features in use are undefined, so the device is not
/// allowed to accept them and carry on.
///
/// The page is whatever the driver selected, so it is not trusted here: a
/// 64-bit feature set has two pages, and a driver asking about any other one
/// reads no features at all, which makes acknowledging zero there a no-op and
/// acknowledging anything there an error (this is also what keeps the shifts
/// below in range for a page a guest chooses arbitrarily).
pub(crate) fn ack_feature_page(
    offered: u64,
    acked: u64,
    page: u32,
    value: u32,
) -> Result<u64, VirtioError> {
    if page > 1 {
        return if value == 0 {
            Ok(acked)
        } else {
            Err(VirtioError::UnsupportedFeatures {
                flags: u64::from(value),
            })
        };
    }
    let offered_page = if page == 0 {
        offered as u32
    } else {
        (offered >> 32) as u32
    };
    if value & !offered_page != 0 {
        return Err(VirtioError::UnsupportedFeatures {
            flags: u64::from(value) << (32 * page),
        });
    }
    let mask = 0xffff_ffff_u64 << (32 * page);
    Ok((acked & !mask) | (u64::from(value) << (32 * page)))
}
