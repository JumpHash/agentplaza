//! Per-architecture dispatch, mirroring Firecracker's `arch/mod.rs`: the
//! re-export surface is defined by the per-arch `mod.rs` files, selected by
//! `cfg`. `aarch64` is visible on any OS so the Hypervisor.framework backend
//! can reuse its pure parts (layout, FDT); its KVM-dependent submodules are
//! gated to Linux inside `aarch64/mod.rs`.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub mod x86_64;

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
