//! Multi-microVM monitor for AgentPlaza's node daemon, derived from Firecracker
//! (Apache-2.0; see `NOTICE`).
//!
//! Boots N isolated microVMs per the plaza's machine model
//! (`docs/orchestrator-draft/schema.sql`: x86_64 servers on KVM, arm64 on KVM —
//! Raspberry Pi 4/5 with a GICv2 — and Apple Silicon on Hypervisor.framework),
//! each with vCPUs, guest memory, a kernel image and a serial console, and no
//! network device by design.
//!
//! Module map (mirrors Firecracker's `src/vmm` crate, trimmed):
//! - [`vstate`] — hypervisor core: `kvm`, `vm`, `vcpu`, `memory`, `bus`,
//!   `resources` (Linux/KVM)
//! - [`arch`] — per-architecture boot, register setup, GIC (v2/v3) and FDT
//! - [`devices`] — `BusDevice`, the legacy serial console and the virtio
//!   devices: the MMIO transport and, behind it, the vsock device that carries
//!   the guest's only channel to the host (there is no network device)
//! - [`builder`] — single-VM build and boot orchestration
//! - [`spawner`] — the public multi-VM API: [`spawner::spawn`] /
//!   [`spawner::spawn_n`] return [`spawner::VmHandle`]s with `wait`, `kill`
//!   and serial capture
//! - [`hvf`] — Apple Silicon Hypervisor.framework backend (macOS; written for
//!   this port, absent upstream)

#[cfg(target_os = "linux")]
pub mod builder;
pub mod devices;
pub mod spawner;

/// ACPI table generation (`x86_64` guests only; aarch64 uses FDT).
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub mod acpi;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod arch;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod vstate;
/// The `hvf` module declares itself on every target; its own `mod.rs` gates the
/// platform halves: the pure decode/PSCI/GIC model (and its unit tests)
/// compiles and runs on any target, the FFI/vm/vcpu backend only on
/// macOS/Apple Silicon.
pub mod hvf;
