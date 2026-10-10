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
//! - [`devices`] — `BusDevice` and the legacy serial console
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod arch;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod vstate;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod hvf;
