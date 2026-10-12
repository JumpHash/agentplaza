// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. Firecracker has no Windows backend (it drives KVM
// only), so this backend is new; like the Hypervisor.framework one it is split
// into a pure half - compiled and unit-tested on any target - and an FFI half
// gated to Windows.

//! The Windows backend: the Windows Hypervisor Platform (WHP) instead of KVM.
//!
//! Windows has no KVM; the same x86_64 microVM is built out of
//! `WinHvPlatform.dll` (partitions, vCPUs, GPA mappings) and
//! `WinHvEmulation.dll` (the instruction emulator that turns an MMIO or port
//! exit into a plain read or write):
//!
//! | what | KVM backend | this backend |
//! |---|---|---|
//! | VM | `KVM_CREATE_VM` | `WHvCreatePartition` + `WHvSetupPartition` |
//! | memory | a KVM memory slot per region | `VirtualAlloc` + `WHvMapGpaRange` |
//! | vCPUs | `KVM_RUN` on a vCPU fd | `WHvRunVirtualProcessor`, one thread each |
//! | MMIO / port I/O | `KVM_EXIT_MMIO` / `KVM_EXIT_IO` | exits decoded by the WHP emulator |
//! | local APIC | in-kernel irqchip | WHP's xAPIC emulation |
//! | I/O APIC | in-kernel irqchip | [`ioapic`]: a userspace 82093AA |
//! | boot state | `KVM_SET_SREGS`/`KVM_SET_REGS` | [`boot_state`] as WHP registers |
//! | kernel loader | `linux-loader` | [`kernel`] (ELF `vmlinux`, `bzImage`) |
//! | zero page | `linux-loader`'s `boot_params` | [`zero_page`] |
//!
//! The guest sees the machine the KVM x86_64 backend builds - the same memory
//! map, MPTable (I/O APIC and ISA routing), hardware-reduced ACPI tables,
//! COM1, the PM1 control/reset ports and virtio-mmio transports announced on
//! the command line - so one guest kernel and one initrd boot on both.
//!
//! # What is verified where
//!
//! The pure half - [`ioapic`], [`boot_state`], [`zero_page`], [`kernel`] - has
//! no FFI and no `crate::arch` dependency, so it compiles everywhere and its
//! tests run on any host. The FFI half (`sys`, `vm`, `vcpu`, `boot`) is gated
//! to Windows; CI compiles it for `x86_64-pc-windows-msvc` and
//! `aarch64-pc-windows-msvc` and a guest boot needs a Windows machine with the
//! "Windows Hypervisor Platform" feature enabled (see `docs/windows-whp.md`).
//!
//! # Loading the platform
//!
//! `WinHvPlatform.dll` exists only where the optional feature is installed, so
//! the backend loads it at run time (`LoadLibraryExW` from System32) instead
//! of importing it: a binary that links this crate - `noded`, `vmctl` - still
//! starts on a machine without the feature and reports the missing feature
//! when a VM is booted.

pub mod boot_state;
pub mod ioapic;
pub mod kernel;
pub mod zero_page;

#[cfg(windows)]
pub(crate) mod sys;
#[cfg(windows)]
mod vm;
