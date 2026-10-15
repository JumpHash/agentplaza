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
//! - [`whp`] — Windows Hypervisor Platform backend (Windows x64; written for
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
//   * `host_vsock` — the host end of a vhost-vsock guest (Linux): `socket(2)`,
//     `bind(2)`, `listen(2)`, `accept4(2)`, `setsockopt(2)`, `shutdown(2)`,
//     `fcntl(2)` and the `read`/`write` of an accepted `AF_VSOCK` stream, for a
//     QEMU workstation VM's relay (noded `ws_relay`).
//   * `vstate::memory` — `mmap`/`munmap`/`sysconf` and the raw-pointer guest
//     region builder (`unsafe impl Send/Sync` for the owned mapping).
//   * `vstate::vcpu` — `dup(2)` of the KVM fd and `create_vcpu_from_rawfd`.
//   * `vstate::vm` — `KVM_SET_USER_MEMORY_REGION` (a raw-pointer ioctl).
//   * `arch::x86_64::xstate` — `arch_prctl(2)` for dynamic XSAVE features.
//   * `arch::x86_64::vcpu` — `KVM_GET_XSAVE2`/`KVM_SET_XSAVE` (raw-pointer ioctls in `kvm-ioctls`)
//     and the XSAVE area's flexible-array header, for the vCPU's snapshot state.
//   * `arch::x86_64::mptable` — the upstream `ByteValued` impls for the `mpspec`
//     tables (upstream-copied, unchanged).
//   * `arch::aarch64::gic::regs` — `KVM_GET_DEVICE_ATTR`, which `kvm-ioctls`
//     exposes as an `unsafe fn` because the caller owns the out-pointer.
//   * `devices::virtio::iov_deque` — `memfd_create`/`ftruncate`/`fcntl`/`mmap`
//     and the volatile double-mapped ring access (Linux; elsewhere the ring is a
//     plain `Vec`).
//   * `devices::virtio::queue` — volatile guest-memory access, the
//     `ByteValued` descriptor types and the `Send` impls.
//   * `devices::virtio::iovec` — guest descriptor-chain walking (volatile
//     access, `iovec` construction, `Send` impls).
//   * `devices::virtio::vsock::packet` — the `ByteValued` packet header and the
//     descriptor-chain loads into it.
//   * `devices::virtio::vhost` — the `/dev/vhost-vsock` ioctl family.
//   * `whp::{sys,vm,vcpu,snapshot}` and `pal::windows` (Windows only) — the WHP and
//     WinSock FFI, the guest memory allocation, the exit-context reads, and a
//     snapshot's register unions and guest-memory slices.
//   * `pal::posix` (macOS; compiled into Linux test builds for its tests) —
//     `pipe(2)`, `fcntl(2)`, `poll(2)` and the non-blocking
//     `socket(2)`/`connect(2)` pair, the macOS counterparts of `vmm-sys-util`'s
//     eventfd and epoll and of `sys`.
//   * `hvf::{sys,vm,vcpu,snapshot}` (macOS only) — the Hypervisor.framework FFI
//     declarations, the framework calls, the exit-structure reads, the call of
//     `hv_vcpu_set_simd_fp_reg` (a NEON vector argument, by inline assembly),
//     and a snapshot's guest-memory slices.
//   * `jail::sys` — the seccomp-bpf install path: the two `prctl(2)` calls that
//     set `no_new_privs` and hand the kernel the BPF program. See [`jail`] for
//     the allowlist, where it is armed, and the `APZ_VMM_JAIL=kill|log|off`
//     mode switch (`log` audits a missing entry instead of killing the daemon);
//     its probes are an integration test (`tests/jail.rs`), in a process of
//     their own.
//   * `jail::jailer_sys` — the jailer's raw-syscall boundary: `unshare(2)`,
//     `mount(2)`, `pivot_root(2)`, `chdir(2)`, `mkdir(2)`, `rmdir(2)`,
//     `umount2(2)` and the `setresgid(2)`/`setresuid(2)` drop. See
//     [`jail::jailer`] and its probe (`tests/jailer.rs`).
#![deny(unsafe_code)]

#[cfg(target_os = "linux")]
pub mod builder;
// The crate's raw-syscall boundary (`poll(2)`, `uname(2)`, `socket(2)`/`connect(2)`).
#[cfg(target_os = "linux")]
mod sys;

/// cgroup v2 limits for the per-VM processes of [`spawner::SpawnMode::Process`]:
/// one cgroup per VM, sized from its shape. Set with [`spawner::set_cgroup_root`].
pub mod cgroup;
/// Copies of disk and memory images that share blocks where the filesystem can (`FICLONE`).
pub mod clonefile;
/// Pausing, resuming and snapshotting a running in-process VM (the builder installs it).
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
pub mod control;
pub mod devices;
/// The host end of a vhost-vsock guest's connections (Linux): the `AF_VSOCK` listeners a QEMU workstation VM's relay
/// serves. See the allowlist above.
#[cfg(target_os = "linux")]
pub mod host_vsock;
/// The VMM's confinement layer (v0): the seccomp-bpf allowlist installed per
/// VMM thread, with `no_new_privs`. Linux/KVM only for now — the
/// Hypervisor.framework path has its own confinement story.
#[cfg(target_os = "linux")]
pub mod jail;
pub mod spawner;

/// ACPI table generation (`x86_64` guests only; aarch64 uses FDT).
#[cfg(all(any(target_os = "linux", windows), target_arch = "x86_64"))]
pub mod acpi;

// On Windows only x86_64 guests are built (the WHP backend); arm64 Windows
// hosts run the hub, the apps and the node daemons but no VM yet.
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    all(windows, target_arch = "x86_64")
))]
pub mod arch;

/// The `hvf` module declares itself on every target; its own `mod.rs` gates the
/// platform halves: the pure decode/PSCI/GIC model (and its unit tests)
/// compiles and runs on any target, the FFI/vm/vcpu backend only on
/// macOS/Apple Silicon.
pub mod hvf;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod vstate;
/// The Windows Hypervisor Platform backend. Like [`hvf`], its pure half (the
/// I/O APIC model, the x86_64 boot state, the zero page and the kernel loader)
/// compiles and is tested on any target; the FFI half only on Windows.
pub mod whp;

/// The event primitives the virtio devices are built on (`EventFd`, `Epoll`),
/// as one API on every platform: `vmm-sys-util`'s on Linux, a WinSock
/// (`WSAPoll`) implementation with the same surface on Windows, and a pipe and
/// `poll(2)` implementation on macOS.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub mod pal;
