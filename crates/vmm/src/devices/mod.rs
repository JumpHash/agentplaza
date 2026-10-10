// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/devices/mod.rs (trimmed: the pci,
// pseudo and acpi device modules are dropped; `legacy` and `virtio` are the
// only device families in this port — and the virtio family is one device, the
// vhost-vsock one, because there is no network device by design).

//! Emulates virtual and hardware devices.

pub mod legacy;
/// Virtio devices over the MMIO transport, and the vhost-vsock backend behind
/// this port's vsock device.
///
/// Linux-only: the device's backend is `/dev/vhost-vsock`, an interface only
/// Linux has (the Hypervisor.framework backend on Apple Silicon has no vsock
/// device at all).
#[cfg(target_os = "linux")]
pub mod virtio;
