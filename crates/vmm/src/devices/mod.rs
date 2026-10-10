// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/devices/mod.rs (trimmed: the virtio,
// pci, pseudo and acpi device modules and the net/vsock error plumbing are
// dropped; `legacy` is the only device family in this port — there is no
// network device by design).

//! Emulates virtual and hardware devices.

pub mod legacy;
