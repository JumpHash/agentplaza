// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. Firecracker has no Hypervisor.framework backend (its
// aarch64 port drives the kernel's KVM), so this backend is new; its pure half
// - the exception decoder, the PSCI dispatcher, the userspace GICv3 and the
// console device - compiles and is unit-tested on any target, while the FFI
// and the vCPU threads are gated to `aarch64` macOS.

//! The Apple Silicon backend: Hypervisor.framework instead of KVM.
//!
//! macOS has no KVM, so the same microVM is built out of
//! `Hypervisor/hv_*.h`:
//!
//! | what | KVM backend | this backend |
//! |---|---|---|
//! | vCPUs | `KVM_RUN` on a vCPU fd | `hv_vcpu_run` on a vCPU per thread |
//! | memory | a KVM memory slot per region | `hv_vm_map` of this process's pages |
//! | devices | in-kernel irqchip + userspace MMIO | unmapped IPA ranges + userspace MMIO (GIC, UART, virtio-mmio) |
//! | interrupt controller | in-kernel vGIC (or none on x86) | [`gic`]: a userspace GICv3 |
//! | device interrupts | KVM irqfds | an interrupt relay thread raising [`gic`] SPIs from the devices' `EventFd`s |
//! | console | `vm-superio` UART + `EventFd` | [`uart`]: the same UART, no event fd |
//! | power management | PSCI in the kernel | [`psci`]: PSCI in the VMM |
//! | boot | `KVM_*` register ioctls | `vm` + `vcpu` register calls |
//!
//! The pure half - [`exit`] (ESR/ISS decoding), [`psci`], [`gic`], [`uart`]
//! and [`wait`] (the wake protocol of a halted or powered-off vCPU) - is
//! target-independent: it has no FFI and no `crate::arch` dependency, so it
//! compiles everywhere and its tests run on the host. Everything that talks to
//! Hypervisor.framework is gated to `aarch64` macOS: `sys` (the FFI), `vm`,
//! `vcpu` and `boot`.
//!
//! The device tree, the kernel image layout and the memory map are shared with
//! the aarch64 KVM backend through `crate::arch::aarch64`: this backend
//! implements the FDT module's `GicFdt` for its userspace controller rather
//! than forking a device tree generator.
//!
//! # What is verified where
//!
//! The decoding, the PSCI state machine and the interrupt controller model are
//! unit-tested on the host. The FFI, the vCPU threads and the device tree
//! written for a real guest cannot be executed here (no Apple Silicon host),
//! so they are checked by type and by review, and their protocol decisions are
//! documented against the reference implementation (QEMU's HVF backend) and
//! Apple's headers. Booting a guest on this backend needs an Apple Silicon
//! machine and a kernel image for it. The virtio devices (vsock, workspace, data
//! volumes) are the shared `devices::virtio` ones over the [`crate::pal`] POSIX
//! event layer, whose tests run on Linux; their wiring here has not run on a Mac
//! either.
//!
//! # One VM per process
//!
//! `hv_vm_create` creates *the* VM of the calling process: unlike the KVM
//! backends, Hypervisor.framework has no per-process VM table, so a node
//! daemon that wants several microVMs on Apple Silicon has to run one VMM
//! process per VM. Here, a second VM in the same process fails at creation
//! with an error that names `HV_BUSY`; `spawner::spawn_n` therefore boots one
//! VM on this platform, not `n`.

pub mod exit;
pub mod gic;
pub mod psci;
// The interrupt relay needs only the event primitives, so it builds (and is tested) wherever they do.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod relay;
pub mod snapshot_state;
// The serial device is built on `vstate::bus`, which exists on Linux and macOS only.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod uart;
pub mod wait;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod boot;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod snapshot;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod sys;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod vcpu;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod vm;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use boot::boot;

/// Kernel command line handed to the guest: the caller's, plus the default
/// this backend needs.
///
/// `loglevel=7` is appended when the caller has not set a console level of its
/// own: the guest's only console is the serial capture, and distribution
/// kernels commonly ship `CONFIG_CONSOLE_LOGLEVEL_DEFAULT=4`, which hides
/// everything below a warning - boot banner included. Callers that set the
/// level themselves (`loglevel=`, `quiet`, `debug`, `ignore_loglevel`) are
/// left alone.
///
/// The x86_64 backend appends `pci=off` as well; there is no PCI window in
/// this backend's device tree, so there is nothing for the guest to probe and
/// nothing to switch off.
///
/// The appended strings are constants, never host state: no environment
/// variable, token or path can reach the guest this way.
pub fn guest_cmdline(requested: &str) -> String {
    const LOGLEVEL_ARGS: [&str; 4] = ["loglevel=", "ignore_loglevel", "quiet", "debug"];

    let requested = requested.trim();
    let sets_loglevel = requested
        .split_whitespace()
        .any(|arg| LOGLEVEL_ARGS.iter().any(|prefix| arg.starts_with(prefix)));
    if sets_loglevel {
        return requested.to_string();
    }
    if requested.is_empty() {
        "loglevel=7".to_string()
    } else {
        format!("{requested} loglevel=7")
    }
}

#[cfg(test)]
mod tests {
    use super::guest_cmdline;

    #[test]
    fn guest_cmdline_appends_the_console_level() {
        assert_eq!(
            guest_cmdline("console=ttyS0 panic=-1"),
            "console=ttyS0 panic=-1 loglevel=7"
        );
        assert_eq!(guest_cmdline(""), "loglevel=7");
        assert_eq!(guest_cmdline("  panic=-1  "), "panic=-1 loglevel=7");
    }

    #[test]
    fn guest_cmdline_respects_the_callers_console_level() {
        assert_eq!(guest_cmdline("quiet"), "quiet");
        assert_eq!(guest_cmdline("debug"), "debug");
        assert_eq!(guest_cmdline("loglevel=3"), "loglevel=3");
        assert_eq!(
            guest_cmdline("console=ttyS0 ignore_loglevel"),
            "console=ttyS0 ignore_loglevel"
        );
    }
}
