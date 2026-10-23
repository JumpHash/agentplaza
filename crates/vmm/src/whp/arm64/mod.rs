// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//! The Windows arm64 backend: agents' VMs on the Windows Hypervisor Platform on Windows-on-Arm.
//!
//! The platform-independent half is [`abi`]: the register names, the exit decoding, the interrupt and
//! GIC structures. The Windows half (`vm`, `vcpu`, `boot`) is compiled only for
//! `windows` + `aarch64`, boots the same aarch64 guest as the Hypervisor.framework and KVM arm64
//! backends (the `arch::aarch64` layout, FDT and kernel loader), and runs its GIC inside the
//! hypervisor. The design and what is verified where: `docs/windows-whp-arm64.md`.

pub mod abi;

#[cfg(all(windows, target_arch = "aarch64"))]
mod boot;
#[cfg(all(windows, target_arch = "aarch64"))]
mod vcpu;
#[cfg(all(windows, target_arch = "aarch64"))]
mod vm;

#[cfg(all(windows, target_arch = "aarch64"))]
pub use boot::boot;
