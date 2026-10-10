// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/devices/virtio/mod.rs (trimmed: this
// port has exactly one virtio device family — the vhost-vsock device behind
// the MMIO transport — so the PCI transport, the userspace vsock muxer
// (`csm`/`unix`), the queue/iovec/event-manager machinery (every virtqueue is
// processed by the kernel's vhost backend, never by this process), the device
// metrics and every other device family are dropped; there is no network
// device by design).

//! Virtio devices on the MMIO transport.
//!
//! Three pieces, in the order a guest sees them:
//!
//! * [`mmio`] — the transport: the register set a driver programs, the device
//!   status state machine, and the [`mmio::VirtioDevice`] trait the transport
//!   drives.
//! * [`vsock`] — the device: the virtio-vsock identity (device id 19, its
//!   queues and features) and the translation from the rings the driver
//!   programmed to the addresses the backend is given.
//! * [`vhost`] — the backend binding: `/dev/vhost-vsock`, the ioctl sequence
//!   that hands the virtqueues to the host kernel, and the CIDs.
//!
//! Splitting the transport from the backend is what makes the device testable
//! without a hypervisor: the transport's tests drive a mock
//! [`mmio::VirtioDevice`], and the backend is the only part that needs the
//! kernel interface (see `FACTS.md` next to this module for what was measured
//! on a real host).

pub mod mmio;
pub mod vhost;
pub mod vsock;

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
