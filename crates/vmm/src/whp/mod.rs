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
pub mod snapshot_state;
pub mod zero_page;

// The platform layer (`sys`, `vm`) builds for both Windows architectures; only
// x86_64 guests are booted so far. WHP on arm64 has its own register set and
// an in-hypervisor GICv3, which the arm64 guest path (the HVF backend's FDT
// and memory map, on WHP) would build on - until then the layer has no
// caller there.
#[cfg(all(windows, target_arch = "x86_64"))]
mod boot;
#[cfg(all(windows, target_arch = "x86_64"))]
mod snapshot;
#[cfg(windows)]
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub(crate) mod sys;
#[cfg(all(windows, target_arch = "x86_64"))]
mod vcpu;
#[cfg(windows)]
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
mod vm;

#[cfg(all(windows, target_arch = "x86_64"))]
pub use boot::boot;

/// A virtio-mmio transport as the guest's command line announces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmioTransport {
    /// Base of the register window.
    pub base: u64,
    /// Size of the register window.
    pub len: u64,
    /// The legacy IRQ (I/O APIC pin) it raises.
    pub irq: u32,
}

/// Kernel command line handed to the guest: the caller's, plus the defaults
/// this backend needs. The KVM x86_64 backend appends the same first four
/// (see `builder::guest_cmdline` for why each is there):
///
/// * `pci=off`, `loglevel=7` and `reboot=acpi`, each unless the caller set
///   that option;
/// * one `virtio_mmio.device=<len>@<base>:<irq>` per transport, unless the
///   caller registered transports itself;
/// * `tsc_early_khz=<khz>` and `tsc=reliable` when the host's TSC frequency
///   is known (`WHvCapabilityCodeProcessorClockFrequency`) and the caller set
///   no `tsc` option. The KVM guest learns the TSC rate from kvmclock; under
///   WHP there is no kvmclock, and this machine has no PIT or HPET for the
///   kernel to calibrate against (the ACPI tables are hardware-reduced), so
///   the rate is given and the clocksource watchdog - which would have
///   nothing to compare the TSC with - is told to trust it.
///
/// The appended values are this backend's layout constants and the host's
/// clock rate - never a path, token or environment variable.
pub fn guest_cmdline(
    requested: &str,
    transports: &[MmioTransport],
    tsc_khz: Option<u64>,
) -> String {
    const LOGLEVEL_ARGS: [&str; 4] = ["loglevel=", "ignore_loglevel", "quiet", "debug"];

    let requested = requested.trim();
    let args: Vec<&str> = requested.split_whitespace().collect();
    let sets = |prefixes: &[&str]| {
        args.iter()
            .any(|arg| prefixes.iter().any(|prefix| arg.starts_with(prefix)))
    };

    let mut defaults: Vec<String> = Vec::new();
    if !sets(&["pci="]) {
        defaults.push("pci=off".to_string());
    }
    if !sets(&LOGLEVEL_ARGS) {
        defaults.push("loglevel=7".to_string());
    }
    if !sets(&["reboot="]) {
        defaults.push("reboot=acpi".to_string());
    }
    if !sets(&["virtio_mmio.device"]) {
        for t in transports {
            defaults.push(format!(
                "virtio_mmio.device={:#x}@{:#x}:{}",
                t.len, t.base, t.irq
            ));
        }
    }
    if let Some(khz) = tsc_khz
        && khz > 0
        && !sets(&["tsc=", "tsc_early_khz="])
    {
        defaults.push(format!("tsc_early_khz={khz}"));
        defaults.push("tsc=reliable".to_string());
    }

    if defaults.is_empty() {
        requested.to_string()
    } else if requested.is_empty() {
        defaults.join(" ")
    } else {
        format!("{requested} {}", defaults.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::{MmioTransport, guest_cmdline};

    const VSOCK: MmioTransport = MmioTransport {
        base: 0xc000_1000,
        len: 0x1000,
        irq: 5,
    };

    #[test]
    fn defaults_are_appended() {
        assert_eq!(
            guest_cmdline("console=ttyS0", &[VSOCK], Some(2_995_200)),
            "console=ttyS0 pci=off loglevel=7 reboot=acpi \
             virtio_mmio.device=0x1000@0xc0001000:5 tsc_early_khz=2995200 tsc=reliable"
        );
        assert_eq!(
            guest_cmdline("", &[], None),
            "pci=off loglevel=7 reboot=acpi"
        );
    }

    #[test]
    fn callers_settings_are_kept() {
        assert_eq!(
            guest_cmdline("quiet pci=nocrs reboot=k tsc=unstable", &[], Some(1000)),
            "quiet pci=nocrs reboot=k tsc=unstable"
        );
        assert_eq!(
            guest_cmdline("virtio_mmio.device=0x200@0x1000:6", &[VSOCK], None),
            "virtio_mmio.device=0x200@0x1000:6 pci=off loglevel=7 reboot=acpi"
        );
    }

    #[test]
    fn an_unknown_or_zero_clock_adds_nothing() {
        assert_eq!(
            guest_cmdline("a", &[], Some(0)),
            "a pci=off loglevel=7 reboot=acpi"
        );
        assert_eq!(
            guest_cmdline("a", &[], None),
            "a pci=off loglevel=7 reboot=acpi"
        );
    }
}
