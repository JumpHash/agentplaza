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
// vsock itself instead of handing the rings to the kernel's vhost-vsock; the
// vhost device stays on the boot path until the next slice cuts it over.

//! Virtio devices on the MMIO transport.
//!
//! * [`mmio`] — the transport: the register set a driver programs, the device
//!   status state machine, and the [`mmio::VirtioDevice`] trait the transport
//!   drives.
//! * [`queue`], [`iovec`], [`iov_deque`] — the userspace virtqueue substrate:
//!   descriptor-chain walking and scatter/gather buffers over guest memory
//!   (unused by the vhost path, which lets the kernel walk the rings).
//! * [`vhost`] — the `/dev/vhost-vsock` ioctl backend; [`vhost_device`] — the
//!   MMIO wrapper around it (`VsockDevice`). This is the vsock device the
//!   boot path uses today.
//! * [`vsock`] — the userspace virtio-vsock device core: the
//!   [`vsock::VsockBackend`] interface, the packet model, and the
//!   [`vsock::Vsock`] device frontend. It lives outside the [`mmio::VirtioDevice`]
//!   trait (which stays `QueueConfig`-shaped for the vhost device); the next
//!   slice hand-wires it through the transport and replaces the vhost device.
//!
//! Splitting the transport from the backend is what makes the device testable
//! without a hypervisor: the transport's tests drive a mock
//! [`mmio::VirtioDevice`], and the backend is the only part that needs the
//! kernel interface (see `FACTS.md` next to this module for what was measured
//! on a real host).

pub mod iov_deque;
pub mod iovec;
pub mod mmio;
pub mod queue;
pub mod vhost;
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
    /// The vhost backend: {0}
    #[error("vhost: {0}")]
    Vhost(#[from] vhost::VhostError),
}
