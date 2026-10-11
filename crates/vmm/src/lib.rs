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

// This crate is safe Rust only. `deny(unsafe_code)` makes any `unsafe` in a
// module that is not on the allowlist below a compile error, so every
// avoidable `unsafe` is eliminated and the irreducible syscall/FFI boundary is
// confined to the small, enumerated, SAFETY-documented modules that follow.
// This is Firecracker's posture — the same ioctl/`mmap`/FFI calls cannot be
// expressed in safe Rust — made compiler-enforced.
//
// The allowlist (each module's `#![allow(unsafe_code)]` carries the reason
// next to the code, and every `unsafe` block has its own `// SAFETY:` note):
//   * `sys` — `poll(2)`, `uname(2)` and the non-blocking `socket(2)`/`connect(2)`
//     pair behind the vsock muxer's guest-initiated connects.
//   * `vstate::memory` — `mmap`/`munmap`/`sysconf` and the raw-pointer guest
//     region builder (`unsafe impl Send/Sync` for the owned mapping).
//   * `vstate::vcpu` — `dup(2)` of the KVM fd and `create_vcpu_from_rawfd`.
//   * `vstate::vm` — `KVM_SET_USER_MEMORY_REGION` (a raw-pointer ioctl).
//   * `arch::x86_64::xstate` — `arch_prctl(2)` for dynamic XSAVE features.
//   * `arch::x86_64::mptable` — the upstream `ByteValued` impls for the `mpspec`
//     tables (upstream-copied, unchanged).
//   * `arch::aarch64::gic::regs` — `KVM_GET_DEVICE_ATTR`, which `kvm-ioctls`
//     exposes as an `unsafe fn` because the caller owns the out-pointer.
//   * `devices::virtio::iov_deque` — `memfd_create`/`ftruncate`/`fcntl`/`mmap`
//     and the volatile double-mapped ring access.
//   * `devices::virtio::queue` — volatile guest-memory access, the
//     `ByteValued` descriptor types and the `Send` impls.
//   * `devices::virtio::iovec` — guest descriptor-chain walking (volatile
//     access, `iovec` construction, `Send` impls).
//   * `devices::virtio::vsock::packet` — the `ByteValued` packet header and the
//     descriptor-chain loads into it.
//   * `devices::virtio::vhost` — the `/dev/vhost-vsock` ioctl family.
//   * `hvf::{sys,vm,vcpu}` (macOS only) — the Hypervisor.framework FFI
//     declarations, the framework calls, and the exit-structure reads.
//   * `jail::sys` — the seccomp-bpf install path: the two `prctl(2)` calls that
//     set `no_new_privs` and hand the kernel the BPF program. See [`jail`] for
//     the allowlist, where it is armed, and the `APZ_VMM_JAIL=kill|log|off`
//     mode switch (`log` audits a missing entry instead of killing the daemon);
//     its probes are an integration test (`tests/jail.rs`), in a process of
//     their own.
#![deny(unsafe_code)]

#[cfg(target_os = "linux")]
pub mod builder;
// The crate's raw-syscall boundary (`poll(2)`, `uname(2)`, `socket(2)`/`connect(2)`).
#[cfg(target_os = "linux")]
mod sys;

/// The VMM's confinement layer (v0): the seccomp-bpf allowlist installed per
/// VMM thread, with `no_new_privs`. Linux/KVM only for now — the
/// Hypervisor.framework path has its own confinement story.
#[cfg(target_os = "linux")]
pub mod jail;
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
