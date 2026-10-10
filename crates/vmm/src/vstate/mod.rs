// Copyright 2020 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/vstate/mod.rs (trimmed: the `interrupts`
// module belongs to the virtio/MSI plumbing, which is not ported).

/// Module with the implementation of a Bus that can hold devices.
pub mod bus;
/// Module with Kvm implementation.
#[cfg(target_os = "linux")]
pub mod kvm;
/// Module with GuestMemory implementation.
pub mod memory;
/// Resource manager for devices.
#[cfg(target_os = "linux")]
pub mod resources;
/// Module with Vcpu implementation.
#[cfg(target_os = "linux")]
pub mod vcpu;
/// Module with Vm implementation.
#[cfg(target_os = "linux")]
pub mod vm;
