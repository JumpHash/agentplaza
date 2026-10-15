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
#[cfg(any(target_os = "linux", all(windows, target_arch = "x86_64")))]
pub mod resources;
/// VM snapshots on disk: the format, the state they carry, and the files.
// Every platform: the format and the metadata are what a daemon reads; the state parts are gated
// inside to the KVM backends that take snapshots.
pub mod snapshot;
/// Guest memory in snapshots: writing the touched (or changed) pages, and mapping a memory file
/// back as a restored guest's memory.
#[cfg(target_os = "linux")]
pub mod snapshot_memory;
/// Module with Vcpu implementation.
#[cfg(target_os = "linux")]
pub mod vcpu;
/// Module with Vm implementation.
#[cfg(target_os = "linux")]
pub mod vm;
