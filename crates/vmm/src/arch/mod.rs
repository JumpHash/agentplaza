//! Per-architecture dispatch, mirroring Firecracker's `arch/mod.rs`: the
//! re-export surface is defined by the per-arch `mod.rs` files, selected by
//! `cfg`. `aarch64` is visible on any OS so the Hypervisor.framework backend
//! can reuse its pure parts (layout, FDT); its KVM-dependent submodules are
//! gated to Linux inside `aarch64/mod.rs`.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub mod x86_64;

/// The x86_64 machine description the Windows Hypervisor Platform backend
/// shares with KVM: the memory layout and the MPTable (I/O APIC and ISA IRQ
/// routing). The rest of `x86_64` drives KVM and `linux-loader` and is not
/// built on Windows; the WHP backend has its own boot state and loader
/// (`crate::whp`).
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod x86_64 {
    #[allow(missing_docs)]
    pub mod generated;
    /// Layout for the x86_64 system.
    pub mod layout;
    pub(crate) mod mptable;
}

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
