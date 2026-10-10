// Copyright 2020 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/vstate/mod.rs (trimmed: the `interrupts`
// module belongs to the virtio/MSI plumbing, which is not ported).

/// Module with the implementation of a Bus that can hold devices.
pub mod bus;
/// Module with Kvm implementation.
pub mod kvm;
/// Module with GuestMemory implementation.
pub mod memory;
/// Resource manager for devices.
pub mod resources;
/// Module with Vcpu implementation.
pub mod vcpu;
/// Module with Vm implementation.
pub mod vm;
