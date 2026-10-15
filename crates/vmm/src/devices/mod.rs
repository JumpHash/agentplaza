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
// userspace virtio-vsock one, because there is no network device by design).

//! Emulates virtual and hardware devices.

pub mod legacy;
/// Virtio devices over the MMIO transport, and the **userspace** vsock device
/// behind this port's vsock transport.
///
/// Built on Linux, Windows and macOS (the KVM, WHP and Hypervisor.framework
/// backends): the device's backend is the VMM's own muxer, which mediates the
/// guest's vsock traffic over host Unix-domain sockets — there is no
/// `/dev/vhost-vsock` on the boot path. The event primitives come from
/// [`crate::pal`].
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod virtio;
