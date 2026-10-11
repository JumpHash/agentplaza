// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the VMM's confinement layer v0 — a seccomp-bpf
// allowlist plus `no_new_privs`, installed per VMM thread. Firecracker's
// equivalent is `src/vmm/src/seccomp.rs` plus the compiled filters in
// `resources/seccomp/*.json`; this port builds the BPF program by hand from a
// stated allowlist instead of vendoring the seccompiler/libseccomp (see
// `sys.rs` for the install path and its provenance).

//! The VMM's confinement layer, v0: a stated seccomp-bpf allowlist.
//!
//! **What this is.** Every VMM thread installs one BPF filter — built here from
//! [`ALLOWLIST`] — over the `prctl(2)` pair in [`sys::install`]: `no_new_privs`
//! first, then `PR_SET_SECCOMP` + `SECCOMP_MODE_FILTER`. A syscall that is not
//! on the allowlist meets the filter's *default action*, which is a deployment
//! choice ([`Mode`]):
//!
//! * [`Mode::Kill`] — `SECCOMP_RET_KILL_PROCESS`, the shipped default. The
//!   whole process is killed by `SIGSYS`, which is the honest choice for a
//!   deployment: a VMM that reaches for a syscall its allowlist does not cover
//!   is either compromised or buggy, and the supervisor that owns the VM handle
//!   tears the VM down when its threads disappear. There is no
//!   `SECCOMP_RET_ERRNO` middle ground in this mode and no `SIGSYS` handler to
//!   limp along through: it is a boundary, not a warning.
//! * [`Mode::Log`] — `SECCOMP_RET_LOG`, the *field-diagnosis* mode. The kernel
//!   audits the call (`audit: type=1326 ... syscall=N comm="apz_vm0_vsock"`)
//!   and **allows** it, so the daemon lives and an operator reads the missing
//!   entry out of the kernel log instead of watching a daemon vanish. It
//!   confines nothing: use it to derive a list, not to run a fleet.
//!
//! **How the allowlist was derived — from a trace, not from memory.** Every
//! entry in [`ALLOWLIST`] is either reached by a VMM thread in the *derivation
//! trace* recorded in the port's task record, or carried over from
//! Firecracker's reference filters and accounted for in [`FC_ADDED`]; the trace
//! is re-derived the same way CI can re-check it. The trace is a real 3-VM
//! fleet (`noded --agents <3 roles>`, a `vsockd` hub, three real keepalive
//! guests) under `strace -f -e trace=all`, driven through a full vsock session
//! (hello/welcome are the guests' own, `tool_call` via the hub's admin surface,
//! `bye` via the hub's stop view), a workspace write over the guest's serial
//! console (virtio-blk), and peer disconnects that RST the muxer's host socket.
//! Its per-thread syscall sets — the ones *after* each thread's
//! `PR_SET_SECCOMP` — are frozen as [`TRACE_OBSERVED`] and asserted against the
//! allowlist by `every_syscall_the_trace_observed_is_allowlisted`, so the list
//! cannot drift below what was measured.
//!
//! That trace, and the kernel's own audit records of the live fleet's deaths
//! (`journalctl -k | grep type=1326`), found the syscalls this list was
//! missing. Three of them had been killing the daemon on an armed thread:
//!
//! * `recvfrom` — the C library's `recv(2)` (x86_64 has no `recv` syscall;
//!   glibc's `recv` *is* `recvfrom`), which `std::io::Read for UnixStream`
//!   issues for `read_local_stream_port`'s `read_exact` of the muxer's
//!   `connect <port>` command. Live record:
//!   `comm="apz_vm0_vsock" ... syscall=45 ... code=0x80000000`.
//! * `mprotect` — reached from *inside* glibc: the allocator's arena setup
//!   (`malloc/arena.c: new_heap()`, `mmap(PROT_NONE)` + `munmap` + `mprotect`)
//!   runs on whichever thread makes the allocation that needs a new arena,
//!   which can be a VMM thread long after it armed. Live records:
//!   `comm="apz_vcpu_0" ... syscall=10 ... code=0x80000000`, six times during
//!   the incident — and the deterministic reproduction is `tests/jail.rs`'s
//!   thread probe, whose kernel audit record is the same `syscall=10`. `sendto`
//!   is the write half of the same stream (`send_bytes_raw`'s
//!   `std::io::Write`) and is covered with it: [`TRACE_ADDED`] names all of them
//!   with their evidence.
//!
//! **What the trace did *not* show, and is deliberately not on the list.**
//! `shutdown`, `getsockname`/`getpeername`, `recvmsg`/`sendmsg`, `readv`/
//! `writev` and `statx`/`fstat` do not appear on any armed thread at all: the
//! muxer drops a stream instead of calling `shutdown(2)`, reads and writes bulk
//! data with `read`/`write` through `vm-memory`'s volatile accessors, and sizes
//! its images before the threads arm. Everything the armed threads *do* reach —
//! including the libc/Rust thread bootstrap every one of them makes before it
//! arms — is allowed, so no observed call is left as a "known finding".
//! `Mode::Log` is the answer to the ones a trace cannot predict: it turns the
//! next unknown into a kernel log line instead of a silent death.
//!
//! **On a kernel that cannot install a filter.** [`Jail::arm_thread`] (what the
//! threads call) returns an error for every failure except
//! [`JailError::Unsupported`] — a kernel without `CONFIG_SECCOMP_FILTER`, or a
//! user-mode emulator such as qemu-user, which answers `PR_SET_SECCOMP` with
//! `EINVAL`. There is no filter to install on such a platform, so the jail is
//! *unavailable* rather than misconfigured: the VMM reports it once and runs.
//! The four hard invariants (no network device, no credentials, a read-only
//! guest, and this filter) do not depend on it, and there is nothing to weaken
//! where the mechanism does not exist.
//!
//! **Where it arms — and why per thread, not once per process.** This port's
//! "VMM process" is really a *thread set* inside the node daemon: `noded`,
//! `vmctl` and the tests all call [`crate::spawner`] in-process and keep
//! running host-side code on the calling thread afterwards. A single
//! process-wide arm — a `TSYNC` install, or one in `boot`'s thread — would
//! confine the daemon and its request loop along with the VM, which is not what
//! the boundary is for (and, with `KILL_PROCESS`, would take the daemon down
//! with a single VM's violation — exactly the wrong blast radius). So
//! [`Jail::arm_thread`] is called once per **VMM thread**, at the top of each thread's
//! body: after the caller has finished every setup syscall it needs (device
//! creation, the muxer's socket, the guest kernel image) and after the thread
//! itself exists, and before the thread enters its own loop. That is "just
//! before entering the run loop", per thread. The three threads are:
//!
//! * `apz_vcpu_{n}` — [`crate::vstate::vcpu`], armed before its first
//!   `KVM_RUN`;
//! * `apz_vm{id}_vsock` — [`crate::devices::virtio::vsock::event_thread`], armed
//!   before its epoll loop;
//! * `apz_vm{id}_exit` — [`crate::builder`], armed before it waits for the exit
//!   event.
//!
//! Arming a thread that is *already* inside `KVM_RUN` (the alternative the
//! design note allowed) is neither needed nor used here: each thread arms
//! itself before its first guest-facing syscall, and a boot with an empty
//! allowlist showed `poll(2)` — the exit watcher's wait — as the only syscall
//! on that thread's path, so no thread is armed late.
//!
//! **The mode switch.** The mode is read from the environment, once, when a
//! thread asks to arm:
//!
//! * `APZ_VMM_JAIL` — exactly `kill`, `log` or `off` (ASCII case-insensitive,
//!   surrounding whitespace ignored). **Anything else is refused**, with
//!   [`JailError::Config`]: a typo must stop the VM at startup, not silently
//!   pick a mode. This variable is the deployment's knob.
//! * `APZ_VMM_NO_JAIL` — the older off-switch, still honoured: any value except
//!   `""`/`0`/`false`/`no`/`off` means [`Mode::Off`]. Absent (and with
//!   `APZ_VMM_JAIL` unset) the jail is **armed with the `kill` default**, which
//!   is what a deployment gets unless it asks otherwise. A set `APZ_VMM_JAIL`
//!   wins over `APZ_VMM_NO_JAIL`, and an invalid `APZ_VMM_JAIL` is refused even
//!   when `APZ_VMM_NO_JAIL` is set alongside it.
//! * [`Jail::set_mode`] and [`Jail::set_enabled`] set the same switch
//!   programmatically and override the environment for the rest of the
//!   process — the library half of a CLI (`vmctl --no-jail`) or of a test that
//!   compares a jailed run against an unjailed one.
//!
//! [`Jail::arm_logged`] is the mode-independent arm: it installs the logged
//! filter whatever the environment says, which is what a diagnosis run wants.
//!
//! **What this v0 does not do.** No namespaces, no chroot/pivot_root, no
//! cgroup, no uid/capability drop, no `Landlock`. It is the seccomp +
//! `no_new_privs` slice of the confinement layer, applied to the VMM's threads
//! inside a daemon that still holds the host's privileges. The remaining pieces
//! are the node daemon's, and are a stated open item.

mod sys;

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// `SECCOMP_RET_ALLOW`: let the syscall through.
const RET_ALLOW: u32 = 0x7fff_0000;
/// `SECCOMP_RET_LOG`: audit the syscall and let it through.
const RET_LOG: u32 = 0x7ffc_0000;
/// `SECCOMP_RET_KILL_PROCESS`: kill the whole process with `SIGSYS`.
const RET_KILL_PROCESS: u32 = 0x8000_0000;

/// What the filter does with a syscall the allowlist does not name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnMiss {
    /// `SECCOMP_RET_KILL_PROCESS` — the shipped default.
    Kill,
    /// `SECCOMP_RET_LOG` — audit it and allow it (see [`Mode::Log`]).
    Log,
}

impl OnMiss {
    /// The BPF return value for this action.
    fn ret(self) -> u32 {
        match self {
            Self::Kill => RET_KILL_PROCESS,
            Self::Log => RET_LOG,
        }
    }
}

/// What a VMM thread does when it asks to arm ([`Jail::arm`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Install the filter with `SECCOMP_RET_KILL_PROCESS` as its default
    /// action. The shipped default.
    Kill,
    /// Install the filter with `SECCOMP_RET_LOG` as its default action: the
    /// kernel audits and *allows* a syscall the allowlist does not name, so a
    /// missing entry costs a kernel log line, not the daemon.
    Log,
    /// Do not install a filter at all.
    Off,
}

impl Mode {
    /// The default action this mode installs, or `None` when it installs no
    /// filter.
    fn on_miss(self) -> Option<OnMiss> {
        match self {
            Self::Kill => Some(OnMiss::Kill),
            Self::Log => Some(OnMiss::Log),
            Self::Off => None,
        }
    }
}

/// `offsetof(struct seccomp_data, nr)` — `linux/seccomp.h`. The layout is
/// `{ int nr; __u32 arch; __u64 instruction_pointer; __u64 args[6]; }`.
const OFFSET_NR: u32 = 0;
/// `offsetof(struct seccomp_data, arch)`.
const OFFSET_ARCH: u32 = 4;
/// `offsetof(struct seccomp_data, args[0])`.
const OFFSET_ARG0: u32 = 16;

/// `AUDIT_ARCH_X86_64` (`linux/audit.h`).
#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xc000_003e;
/// `AUDIT_ARCH_AARCH64` (`linux/audit.h`).
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xc000_00b7;

// ---------------------------------------------------------------------------
// BPF encoding (`linux/filter.h`, `linux/bpf_common.h`).
// ---------------------------------------------------------------------------

/// `BPF_LD` (`0x00`).
const BPF_LD: u16 = 0x00;
/// `BPF_W`.
const BPF_W: u16 = 0x00;
/// `BPF_ABS`.
const BPF_ABS: u16 = 0x20;
/// `BPF_JMP` (`0x05`).
const BPF_JMP: u16 = 0x05;
/// `BPF_JEQ`.
const BPF_JEQ: u16 = 0x10;
/// `BPF_K` (`0x00`).
const BPF_K: u16 = 0x00;
/// `BPF_RET` (`0x06`).
const BPF_RET: u16 = 0x06;

/// Encodes `BPF_STMT(code, k)`: a statement with an immediate operand.
fn stmt(code: u16, k: u32) -> u64 {
    u64::from(code) | (u64::from(k) << 32)
}

/// Encodes `BPF_JUMP(code, k, jt, jf)`: a conditional jump by `jt`/`jf`
/// instructions from the one after this one.
fn jump(code: u16, k: u32, jt: u8, jf: u8) -> u64 {
    u64::from(code) | (u64::from(jt) << 16) | (u64::from(jf) << 24) | (u64::from(k) << 32)
}

// ---------------------------------------------------------------------------
// The allowlist.
// ---------------------------------------------------------------------------

/// The `unlink` family: `std::fs::remove_file` (the muxer clearing its socket
/// file on drop) goes through `unlink(2)` on `x86_64` and `unlinkat(2)` on
/// `aarch64`, where the kernel has no `unlink` at all. Both are allowed where
/// they exist.
#[cfg(target_arch = "x86_64")]
const UNLINK: &[libc::c_long] = &[libc::SYS_unlink, libc::SYS_unlinkat];
#[cfg(not(target_arch = "x86_64"))]
const UNLINK: &[libc::c_long] = &[libc::SYS_unlinkat];

/// The `poll` the exit watcher and the builder wait on. `x86_64`'s glibc calls
/// `poll(2)`; `aarch64`'s kernel has no `poll` at all and glibc's `poll()` is
/// `ppoll(2)`. Verified by the aarch64 cross-build, where `libc::SYS_poll` does
/// not exist.
#[cfg(target_arch = "x86_64")]
const POLL: &[libc::c_long] = &[libc::SYS_poll];
#[cfg(not(target_arch = "x86_64"))]
const POLL: &[libc::c_long] = &[libc::SYS_ppoll];

/// The epoll wait of `vmm-sys-util`'s `Epoll::wait`: `epoll_wait(2)` on
/// `x86_64`, `epoll_pwait(2)` on `aarch64`, which has no `epoll_wait`.
#[cfg(target_arch = "x86_64")]
const EPOLL_WAIT: &[libc::c_long] = &[libc::SYS_epoll_wait];
#[cfg(not(target_arch = "x86_64"))]
const EPOLL_WAIT: &[libc::c_long] = &[libc::SYS_epoll_pwait];

/// One group of allowlisted syscalls, with the reason they are needed.
struct Group {
    /// Why this group is on the list: what in the VMM makes these calls.
    why: &'static str,
    /// The syscall numbers (`__NR_*`).
    syscalls: &'static [libc::c_long],
}

/// The VMM's allowlist. Every syscall a VMM thread makes while a VM runs is
/// here, grouped by what needs it; everything else meets the filter's default
/// action ([`OnMiss`]).
///
/// **How it is kept honest.** [`TRACE_OBSERVED`] freezes every syscall the
/// derivation trace saw on any *armed* VMM thread, and
/// `every_syscall_the_trace_observed_is_allowlisted` asserts each one is on
/// this list. That is the check the incident's list failed: it had been pruned
/// by removing entries until a short run looked clean, which is how `recvfrom`
/// and `mprotect` came to be missing from a list two real paths used.
/// [`TRACE_ADDED`] records the entries this derivation added, with the
/// evidence. Firecracker's `vmm` and `vcpu` thread filters are the reference
/// the list started from ([`FC_REFERENCE`]);
/// `every_firecracker_reference_entry_is_accounted_for` records both what this
/// port dropped, what it added, and why.
///
/// `socket(2)` is additionally argument-filtered to `AF_UNIX`
/// ([`AF_UNIX_ALLOWED`]): the vsock muxer's only socket is a host Unix-domain
/// socket, and there is no network device anywhere in this port, so the jail
/// refuses even to *create* a socket in another family (`socketpair(2)` is not
/// on the list at all — nothing in the port pairs sockets).
static ALLOWLIST: &[Group] = &[
    Group {
        why: "Thread and process lifecycle: a VMM thread leaves through exit/exit_group, \
              and the runtime yields while another thread holds a lock.",
        syscalls: &[
            libc::SYS_exit,
            libc::SYS_exit_group,
            libc::SYS_sched_yield,
            libc::SYS_getpid,
            libc::SYS_gettid,
        ],
    },
    Group {
        why: "Thread creation and synchronization: the futex-backed Mutex/Arc the VMM \
              threads share, and the setup a Rust thread makes on the way to its body. \
              Most of it (`sched_getaffinity`, `set_robust_list`, `rseq`, \
              `set_tid_address`, `prctl(PR_SET_NAME)`, `prlimit64`) runs *before* the \
              thread arms itself, but a thread created by a thread that has already \
              armed inherits this filter, so those calls are the difference between a \
              future fourth thread living and taking the daemon down with it.",
        syscalls: &[
            libc::SYS_clone,
            libc::SYS_clone3,
            libc::SYS_futex,
            libc::SYS_set_robust_list,
            libc::SYS_rseq,
            libc::SYS_set_tid_address,
            libc::SYS_prlimit64,
            libc::SYS_getrandom,
            libc::SYS_sched_getaffinity,
            libc::SYS_prctl,
        ],
    },
    Group {
        why: "Signals: the vCPU kick (tgkill) that breaks a vCPU out of KVM_RUN, the \
              kick handler's mask/action calls, and Rust's stack-overflow guard \
              (sigaltstack).",
        syscalls: &[
            libc::SYS_rt_sigaction,
            libc::SYS_rt_sigprocmask,
            libc::SYS_rt_sigreturn,
            libc::SYS_sigaltstack,
            libc::SYS_tgkill,
        ],
    },
    Group {
        why: "Time: Rust's std::thread::sleep (clock_nanosleep) and clock_gettime.",
        syscalls: &[libc::SYS_clock_gettime, libc::SYS_clock_nanosleep],
    },
    Group {
        why: "Guest memory, the heap, and the allocator: the buffers a VMM thread allocates \
              while the VM runs (`mmap`/`munmap`/`madvise`/`brk`), and the `mprotect` glibc's \
              allocator issues *from inside libc* when an allocation needs a new arena \
              (`malloc/arena.c: new_heap()`). That is not a startup call: the live fleet's \
              armed vCPU threads reached it (`syscall=10`, six audit records with \
              `code=0x80000000`), and tests/jail.rs reproduces it — a thread spawned by an \
              armed thread dies on the pre-fix allowlist's mprotect and runs on this one.",
        syscalls: &[
            libc::SYS_mmap,
            libc::SYS_munmap,
            libc::SYS_madvise,
            libc::SYS_mprotect,
            libc::SYS_brk,
        ],
    },
    Group {
        why: "File descriptors and I/O: the writable workspace image (the only writable \
              device; the VMM reads and writes it with seek+read/write), the non-blocking \
              flag the muxer sets on accepted sockets (fcntl), the fsync behind a guest \
              flush, and the unlink of the muxer's socket file when it is dropped.",
        syscalls: &[
            libc::SYS_openat,
            libc::SYS_close,
            libc::SYS_read,
            libc::SYS_write,
            libc::SYS_lseek,
            libc::SYS_fcntl,
            libc::SYS_fsync,
        ],
    },
    Group {
        why: "The unlink family (see [`UNLINK`]).",
        syscalls: UNLINK,
    },
    Group {
        why: "The event loop: eventfd (queue kicks and the VM exit) and the epoll the vsock \
              event thread creates and controls.",
        syscalls: &[
            libc::SYS_eventfd2,
            libc::SYS_epoll_create1,
            libc::SYS_epoll_ctl,
        ],
    },
    Group {
        why: "The event loop's epoll wait (see [`EPOLL_WAIT`]).",
        syscalls: EPOLL_WAIT,
    },
    Group {
        why: "The exit watcher's and the builder's wait (see [`POLL`]).",
        syscalls: POLL,
    },
    Group {
        why: "The KVM device: every VM and vCPU operation (KVM_RUN included) is an ioctl on \
              a file descriptor the VMM already holds, so only the ioctl syscall itself is \
              needed — there is no open of /dev/kvm after arming.",
        syscalls: &[libc::SYS_ioctl],
    },
    Group {
        why: "Host Unix-domain sockets: the per-VM vsock muxer's only channel, and the only \
              socket this VMM ever makes (there is no network device). Its bulk data goes \
              through read(2)/write(2) — `vm-memory`'s ReadVolatile/WriteVolatile on the \
              stream — but std's UnixStream I/O is the C library's recv/send under the hood, \
              so `recvfrom` (glibc's `recv`; x86_64 has no recv syscall) is what \
              `read_local_stream_port` parses a host-initiated connection's `connect <port>` \
              command with, and `sendto` (glibc's `send`) is the write half of that stream, \
              used by `send_bytes_raw` for the local-init ack. socket(2) is also \
              argument-filtered to AF_UNIX.",
        syscalls: &[
            libc::SYS_socket,
            libc::SYS_bind,
            libc::SYS_listen,
            libc::SYS_accept4,
            libc::SYS_connect,
            libc::SYS_recvfrom,
            libc::SYS_sendto,
        ],
    },
];

/// Every syscall the derivation trace observed on an *armed* VMM thread — the
/// union over the vCPU (`apz_vcpu_{n}`), vsock-muxer (`apz_vm{id}_vsock`) and
/// exit-watcher (`apz_vm{id}_exit`) thread kinds of a traced 3-VM
/// `noded --agents` fleet, on both sides of each thread's `PR_SET_SECCOMP` —
/// and therefore every syscall the shipped filter must admit.
/// `every_syscall_the_trace_observed_is_allowlisted` asserts exactly that.
///
/// Both sides of the arming are included because a thread's *bootstrap* (the
/// `mmap`/`mprotect`/`sched_getaffinity`/`prctl(PR_SET_NAME)` libc and Rust make
/// on the way to the thread body) runs before it arms: it is allowed here so
/// that a thread created by a thread that has already armed — and so inherits
/// this filter — survives its own start. `.0`'s body is `ioctl` (KVM_RUN) plus
/// the workspace block device's `read`/`write`/`lseek`/`fsync`/`fcntl`; `.2`'s
/// is its wait; the muxer's is the socket set (`socket`→`connect` to the hub,
/// `accept4` for a host-initiated connection, `recvfrom` for that connection's
/// `connect <port>` command) over `epoll`.
pub const TRACE_OBSERVED: &[&str] = &[
    "accept4",
    "close",
    "connect",
    "epoll_create1",
    "epoll_ctl",
    "fcntl",
    "fsync",
    "futex",
    "ioctl",
    "lseek",
    "mmap",
    "mprotect",
    "munmap",
    "openat",
    "prctl",
    "read",
    "recvfrom",
    "rseq",
    "rt_sigprocmask",
    "sched_getaffinity",
    "set_robust_list",
    "sigaltstack",
    "socket",
    "write",
];

/// The arch-specific halves of [`TRACE_OBSERVED`], the same split the
/// Firecracker diff uses: `x86_64`'s epoll wait is `epoll_wait` and the exit
/// watcher waits with `poll`; `aarch64` has neither syscall and glibc uses
/// `epoll_pwait`/`ppoll` there.
#[cfg(target_arch = "x86_64")]
pub const TRACE_OBSERVED_ARCH: &[&str] = &["epoll_wait", "poll"];
#[cfg(not(target_arch = "x86_64"))]
pub const TRACE_OBSERVED_ARCH: &[&str] = &["epoll_pwait", "ppoll"];

/// The entries the trace derivation — and the live kernel audit records it was
/// checked against — *added* to the list this port shipped in `6ce364b`, each
/// with the evidence that put it there. `every_trace_added_entry_is_on_the_allowlist`
/// keeps this table and [`ALLOWLIST`] in step.
pub const TRACE_ADDED: &[(&str, &str)] = &[
    (
        "recvfrom",
        "the muxer's host-initiated read: std's UnixStream read is glibc's recv, which x86_64 \
         implements as recvfrom. Observed on the armed apz_vm2_vsock thread in the trace; live \
         `comm=\"apz_vm0_vsock\" syscall=45 code=0x80000000`.",
    ),
    (
        "sendto",
        "the same stream's write half: `send_bytes_raw`'s std UnixStream write is glibc's send, \
         which x86_64 implements as sendto. Not reached in the trace (no guest in the port \
         listens, so a local-init connection never establishes) — refused by the pre-fix \
         allowlist and admitted by this one, both proven in tests/jail.rs.",
    ),
    (
        "mprotect",
        "glibc's allocator issuing `new_heap()` for a thread's first allocation that needs a \
         new arena — reachable long after arming. Live `comm=\"apz_vcpu_0\" syscall=10 \
         code=0x80000000`, six records during the incident; reproduced deterministically by \
         tests/jail.rs's thread probe.",
    ),
    (
        "sched_getaffinity",
        "libc's thread bootstrap; observed before arming on all nine armed threads of the trace, \
         and needed under the filter by a thread created by an armed thread.",
    ),
    (
        "prctl",
        "`thread::Builder::name`'s PR_SET_NAME (plus the PR_SET_NO_NEW_PRIVS/PR_SET_SECCOMP a \
         self-arming thread under an inherited filter repeats); observed before arming on all \
         nine armed threads of the trace.",
    ),
];

/// The one address family [`ALLOWLIST`]'s `socket(2)` may be called with
/// (`AF_UNIX`).
const AF_UNIX_ALLOWED: u32 = libc::AF_UNIX as u32;

/// Whether `nr` is the family-filtered socket syscall.
fn is_family_filtered(nr: libc::c_long) -> bool {
    nr == libc::SYS_socket
}

/// Firecracker's `vmm` + `vcpu` thread filters, unioned (from
/// `resources/seccomp/x86_64-unknown-linux-musl.json`), as the reference this
/// port's [`ALLOWLIST`] is compared against. Kept as data so the difference is
/// tested, not just described.
#[cfg(test)]
const FC_REFERENCE: &[&str] = &[
    "accept4",
    "brk",
    "clock_gettime",
    "close",
    "connect",
    "epoll_ctl",
    "epoll_pwait",
    "eventfd2",
    "exit",
    "exit_group",
    "fallocate",
    "fcntl",
    "fstat",
    "fsync",
    "ftruncate",
    "futex",
    "getrandom",
    "gettid",
    "io_uring_enter",
    "io_uring_register",
    "io_uring_setup",
    "ioctl",
    "lseek",
    "madvise",
    "memfd_create",
    "mincore",
    "mmap",
    "mprotect",
    "mremap",
    "msync",
    "munmap",
    "open",
    "read",
    "readv",
    "recvfrom",
    "recvmsg",
    "restart_syscall",
    "rt_sigaction",
    "rt_sigprocmask",
    "rt_sigreturn",
    "sched_yield",
    "sendmsg",
    "sendto",
    "sigaltstack",
    "socket",
    "stat",
    "timerfd_create",
    "timerfd_settime",
    "tkill",
    "write",
    "writev",
];

/// The arch-specific halves of [`FC_DROPPED`] and [`FC_ADDED`]: the poll and
/// epoll-wait families differ between the two kernels this port targets.
/// `x86_64` has the bare `poll(2)`/`epoll_wait(2)`; `aarch64` has neither and
/// glibc uses `ppoll(2)`/`epoll_pwait(2)` there, and `std::fs::remove_file` is
/// `unlink(2)` on `x86_64` but `unlinkat(2)` on `aarch64`.
#[cfg(all(test, target_arch = "x86_64"))]
const FC_DROPPED_ARCH: &[(&str, &str)] = &[(
    "epoll_pwait",
    "x86_64's epoll wait is epoll_wait(2), which the allowlist has instead",
)];
#[cfg(all(test, not(target_arch = "x86_64")))]
const FC_DROPPED_ARCH: &[(&str, &str)] = &[];

#[cfg(all(test, target_arch = "x86_64"))]
const FC_ADDED_ARCH: &[(&str, &str)] = &[
    (
        "poll",
        "the exit watcher and the builder wait with poll(2), not epoll",
    ),
    (
        "epoll_wait",
        "vmm-sys-util's Epoll::wait uses epoll_wait(2), not epoll_pwait(2)",
    ),
    (
        "unlink",
        "std::fs::remove_file on x86_64 (the muxer's socket file)",
    ),
];
#[cfg(all(test, not(target_arch = "x86_64")))]
const FC_ADDED_ARCH: &[(&str, &str)] = &[(
    "ppoll",
    "aarch64 has no poll(2); glibc's poll() is ppoll(2)",
)];

/// The syscalls Firecracker's filters have that this port's allowlist does not,
/// with the reason. Frozen so the difference cannot drift silently.
#[cfg(test)]
const FC_DROPPED: &[(&str, &str)] = &[
    ("open", "this port opens its images with openat(2)"),
    (
        "stat",
        "no stat after arming; File::metadata runs while building the VM",
    ),
    (
        "fstat",
        "as `stat`: the block device sizes its image before the threads arm",
    ),
    (
        "restart_syscall",
        "only follows a syscall the allowlist does not name",
    ),
    (
        "tkill",
        "the vCPU kick is tgkill(2), which the allowlist has instead",
    ),
    (
        "mremap",
        "the guest-memory regions are mapped once; nothing remaps",
    ),
    ("msync", "nothing flushes guest memory to a file"),
    ("mincore", "nothing asks for page residency"),
    (
        "memfd_create",
        "the guest memory is anonymous; no memfd backing",
    ),
    ("fallocate", "no snapshotting v0, so no hole punching"),
    (
        "ftruncate",
        "the block image is sized before arming; only tests resize it",
    ),
    ("readv", "the vsock muxer reads with read(2)"),
    ("writev", "the vsock muxer writes with write(2)"),
    (
        "recvmsg",
        "the muxer's host stream is read with read(2) (vm-memory's ReadVolatile); the \
         std UnixStream read on it is glibc's recv, which the allowlist has as recvfrom",
    ),
    (
        "sendmsg",
        "as `recvmsg`: the std UnixStream write is glibc's send, which the allowlist has \
         as sendto",
    ),
    ("timerfd_create", "no timerfd in the port"),
    ("timerfd_settime", "no timerfd in the port"),
    ("io_uring_setup", "the port does not use io_uring"),
    ("io_uring_enter", "the port does not use io_uring"),
    ("io_uring_register", "the port does not use io_uring"),
];

/// The syscalls this port's allowlist has that Firecracker's reference does not,
/// with the reason. Firecracker arms its threads only after they exist (so its
/// filters need no thread-creation or wait syscalls), while this port's threads
/// arm themselves.
#[cfg(test)]
const FC_ADDED: &[(&str, &str)] = &[
    (
        "epoll_create1",
        "the vsock event thread creates its epoll after arming",
    ),
    ("clock_nanosleep", "Rust's std::thread::sleep"),
    ("getpid", "the C library's process identity"),
    ("openat", "the port opens with openat(2), not open(2)"),
    (
        "unlinkat",
        "std::fs::remove_file on aarch64, where unlink(2) does not exist",
    ),
    (
        "bind",
        "the vsock muxer binds its Unix-domain socket after arming",
    ),
    ("listen", "the vsock muxer listens on it"),
    (
        "clone",
        "a thread that arms itself is created by clone/clone3 if added later",
    ),
    ("clone3", "as `clone`"),
    (
        "set_robust_list",
        "per-thread setup of a thread created under this filter",
    ),
    ("set_tid_address", "as `set_robust_list`"),
    ("rseq", "as `set_robust_list`"),
    ("prlimit64", "the C library's thread/stack setup"),
    (
        "sched_getaffinity",
        "libc's thread bootstrap, observed on every armed thread of the trace before it armed",
    ),
    (
        "prctl",
        "`thread::Builder::name`'s PR_SET_NAME and a self-arming thread's own \
         PR_SET_NO_NEW_PRIVS/PR_SET_SECCOMP, observed on every armed thread of the trace",
    ),
    (
        "tgkill",
        "the vCPU kick is tgkill(2); Firecracker lists tkill(2)",
    ),
];

/// Errors from [`Jail::arm`].
#[derive(Debug, thiserror::Error)]
pub enum JailError {
    /// The jail's configuration is invalid, so nothing is armed: {0}. A
    /// misconfigured jail is refused rather than defaulted — a typo in
    /// `APZ_VMM_JAIL` must stop the VM at startup.
    #[error("the jail is misconfigured: {0}")]
    Config(String),
    /// The seccomp program is malformed or too long: {0}
    #[error("the seccomp program is malformed: {0}")]
    Program(String),
    /// Installing the filter failed: {0}
    #[error("cannot install the seccomp filter: {0}")]
    Install(std::io::Error),
    /// This kernel cannot install a seccomp-bpf filter at all (no
    /// `CONFIG_SECCOMP_FILTER`), or the process runs under a user-mode
    /// emulator (qemu-user answers `PR_SET_SECCOMP` with `EINVAL`). There is
    /// no filter to install, so this is not a failure to confine — see
    /// [`Jail::arm`]: {0}
    #[error("this kernel cannot install a seccomp-bpf filter: {0}")]
    Unsupported(std::io::Error),
}

/// The mode the process is set to. `0` = not decided yet (read the
/// environment), then one of [`Mode`]'s values as a `u8`; `INVALID` = the
/// environment named a mode that does not exist, which is refused on every
/// read until something sets it.
static MODE: AtomicU8 = AtomicU8::new(0);

/// [`MODE`]'s encoding.
const MODE_FROM_ENV: u8 = 0;
const MODE_KILL: u8 = 1;
const MODE_LOG: u8 = 2;
const MODE_OFF: u8 = 3;
/// The environment named a mode that does not exist: refused, never defaulted.
const MODE_INVALID: u8 = 4;

/// Whether the "this kernel cannot install the filter" warning has been printed
/// (once per process, not once per VMM thread).
static WARNED_UNSUPPORTED: AtomicBool = AtomicBool::new(false);

/// Reports, once, that the platform cannot install a seccomp filter.
fn warn_unsupported_once(err: &std::io::Error) {
    if !WARNED_UNSUPPORTED.swap(true, Ordering::SeqCst) {
        eprintln!(
            "vmm: this kernel cannot install a seccomp-bpf filter ({err}); \
             the VMM runs without its jail"
        );
    }
}

/// The environment variable that selects the jail's mode: exactly `kill`,
/// `log` or `off`. Anything else is refused.
pub const JAIL_ENV: &str = "APZ_VMM_JAIL";

/// The older switch, kept working: a set, non-empty value that is not an
/// explicit false means [`Mode::Off`].
pub const NO_JAIL_ENV: &str = "APZ_VMM_NO_JAIL";

/// Sets the mode programmatically, overriding the environment for the rest of
/// the process. The library half of a `--jail MODE` flag; [`set_enabled`] is
/// the two-valued shorthand.
pub fn set_mode(mode: Mode) {
    MODE.store(
        match mode {
            Mode::Kill => MODE_KILL,
            Mode::Log => MODE_LOG,
            Mode::Off => MODE_OFF,
        },
        Ordering::SeqCst,
    );
}

/// Forces the jail on ([`Mode::Kill`], `true`) or off ([`Mode::Off`],
/// `false`), overriding the environment for the rest of the process.
pub fn set_enabled(on: bool) {
    set_mode(if on { Mode::Kill } else { Mode::Off });
}

/// The mode this process is set to: programmatic if [`set_mode`] or
/// [`set_enabled`] was called, otherwise the environment's, otherwise
/// [`Mode::Kill`]. An `APZ_VMM_JAIL` value that is not `kill`/`log`/`off` is
/// [`JailError::Config`] — on *every* call, until something sets the mode.
pub fn mode() -> Result<Mode, JailError> {
    let decided = MODE.load(Ordering::SeqCst);
    if decided == MODE_FROM_ENV {
        return mode_from_env();
    }
    match decided {
        MODE_KILL => Ok(Mode::Kill),
        MODE_LOG => Ok(Mode::Log),
        MODE_OFF => Ok(Mode::Off),
        // Only `MODE_INVALID` is left: `set_mode`, `set_enabled` and
        // `mode_from_env` store these five values and nothing else.
        _ => Err(invalid_jail_env()),
    }
}

/// Whether the jail is armed at all (either mode). A process whose jail is
/// misconfigured reads as armed, so a caller that only checks this still asks
/// [`Jail::arm`] and meets the error there.
pub fn is_enabled() -> bool {
    !matches!(mode(), Ok(Mode::Off))
}

/// The refusal an `APZ_VMM_JAIL` value that names no mode produces.
fn invalid_jail_env() -> JailError {
    let value = std::env::var(JAIL_ENV).unwrap_or_default();
    parse_jail_env(&value).expect_err("only called when the value names no mode")
}

/// The mode an [`JAIL_ENV`] value names: `kill`, `log` or `off`, ASCII
/// case-insensitive and trimmed. Anything else — an empty string included — is
/// [`JailError::Config`].
///
/// A pure function on purpose: `std::env::set_var` is `unsafe` in edition 2024
/// and this crate denies `unsafe`, so the unit tests exercise the parsing
/// directly and `tests/jail.rs` is what proves the variable is read, by setting
/// it on a child process.
fn parse_jail_env(value: &str) -> Result<Mode, JailError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "kill" => Ok(Mode::Kill),
        "log" => Ok(Mode::Log),
        "off" => Ok(Mode::Off),
        _ => Err(JailError::Config(format!(
            "{JAIL_ENV}={value:?} is not a mode: expected \"kill\", \"log\" or \"off\""
        ))),
    }
}

/// Whether a value of [`NO_JAIL_ENV`] means the jail is off: set and non-empty,
/// and not one of the explicit false spellings.
fn parse_no_jail_env(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

/// Reads [`JAIL_ENV`] and [`NO_JAIL_ENV`] and decides the process mode, caching
/// the decision in [`MODE`] — an invalid value included, which it caches as
/// [`MODE_INVALID`] so it is never silently re-read as a default.
///
/// [`JAIL_ENV`] wins when it is set; then [`NO_JAIL_ENV`]; with neither set the
/// mode is [`Mode::Kill`], so a deployment gets an armed jail unless it asks
/// otherwise.
fn mode_from_env() -> Result<Mode, JailError> {
    if let Ok(value) = std::env::var(JAIL_ENV) {
        let mode = parse_jail_env(&value);
        if mode.is_err() {
            MODE.store(MODE_INVALID, Ordering::SeqCst);
        }
        return mode;
    }
    if env_says_disabled() {
        return Ok(Mode::Off);
    }
    Ok(Mode::Kill)
}

/// Reads [`NO_JAIL_ENV`] through [`parse_no_jail_env`]; absent means the jail is
/// not disabled by it.
fn env_says_disabled() -> bool {
    std::env::var(NO_JAIL_ENV).is_ok_and(|value| parse_no_jail_env(&value))
}

/// The VMM's jail: one BPF allowlist filter, installed per VMM thread.
pub struct Jail;

impl Jail {
    /// Builds and installs the filter on the calling thread, as the process
    /// mode says ([`mode`]): [`Mode::Kill`] or [`Mode::Log`] install, and
    /// [`Mode::Off`] is a no-op.
    ///
    /// Fails closed: an error means the filter is **not** installed — including
    /// [`JailError::Config`], which is what a process whose `APZ_VMM_JAIL`
    /// names no mode gets, on every thread, so a misconfigured jail stops the
    /// VM rather than silently confining it a different way. A kernel without
    /// `CONFIG_SECCOMP_FILTER` — or a user-mode emulator such as qemu-user,
    /// which answers `PR_SET_SECCOMP` with `EINVAL` — is reported as
    /// [`JailError::Unsupported`]; there is no filter to install there, so
    /// [`Jail::arm_thread`] (what the VMM threads call) reports it and carries
    /// on, while a strict caller can tell the difference. Every other failure
    /// is fatal to the thread that asked.
    pub fn arm() -> Result<(), JailError> {
        match mode()?.on_miss() {
            Some(on_miss) => Self::arm_with(on_miss),
            None => Ok(()),
        }
    }

    /// Arms the *logged* filter — `SECCOMP_RET_LOG` as the default action —
    /// whatever the process mode says: a syscall the allowlist does not name is
    /// audited by the kernel and allowed, so the daemon lives and the missing
    /// entry is read out of the kernel log (`journalctl -k | grep type=1326`,
    /// or `dmesg`). This confines nothing; it is the field-diagnosis arm, and
    /// the mode switch (`APZ_VMM_JAIL=log`) is the deployment-level half of it.
    pub fn arm_logged() -> Result<(), JailError> {
        Self::arm_with(OnMiss::Log)
    }

    /// [`Jail::arm`] for a VMM thread: a real install failure is returned (the
    /// thread must not run unconfined), but a platform that *cannot* install a
    /// filter at all is reported once and tolerated — there is no confinement
    /// to weaken on such a kernel (the four hard invariants do not depend on
    /// this filter).
    pub fn arm_thread() -> Result<(), JailError> {
        match Self::arm() {
            Err(JailError::Unsupported(err)) => {
                warn_unsupported_once(&err);
                Ok(())
            }
            other => other,
        }
    }

    /// Arms the shipped allowlist with `on_miss` as the filter's default
    /// action.
    fn arm_with(on_miss: OnMiss) -> Result<(), JailError> {
        let numbers = allowlist_numbers();
        Self::install(&program(&numbers, on_miss)?)
    }

    /// Installs a built program, mapping the one error that means "this kernel
    /// cannot install a filter at all" — qemu-user answers `PR_SET_SECCOMP`
    /// with `EINVAL` — so both arms agree on what [`JailError::Unsupported`]
    /// means. A kernel that implements `PR_SET_SECCOMP` cannot fail that way
    /// for a valid `no_new_privs` + filter pair.
    fn install(program: &[u64]) -> Result<(), JailError> {
        sys::install(program).map_err(|err| {
            if err.raw_os_error() == Some(libc::EINVAL) {
                JailError::Unsupported(err)
            } else {
                JailError::Install(err)
            }
        })
    }

    /// Installs a filter built from an explicit list of syscall *names* with
    /// `on_miss` as its default action, on the calling thread. **The
    /// confinement tests' seam**: `tests/jail.rs` arms the allowlist this port
    /// shipped in `6ce364b` (a frozen name table there) to show the calls it
    /// refused, and then this one to show they run. No VMM thread calls it —
    /// they call [`Jail::arm_thread`] — and a name this port does not know is
    /// [`JailError::Config`] rather than a silently dropped entry.
    #[doc(hidden)]
    pub fn arm_named(names: &[&str], on_miss: OnMiss) -> Result<(), JailError> {
        let mut numbers = Vec::with_capacity(names.len());
        for name in names {
            match syscall_number(name) {
                Some(nr) => numbers.push(nr),
                None => {
                    return Err(JailError::Config(format!(
                        "{name:?} is not a syscall this port knows"
                    )));
                }
            }
        }
        let program = program(&numbers, on_miss)?;
        Self::install(&program)
    }

    /// The allowlist, as `(name, syscall number, group reason)` rows — for the
    /// tests and for an operator who wants to see the boundary.
    pub fn allowlist() -> impl Iterator<Item = (&'static str, libc::c_long, &'static str)> {
        ALLOWLIST.iter().flat_map(|group| {
            group
                .syscalls
                .iter()
                .map(move |&nr| (syscall_name(nr), nr, group.why))
        })
    }
}

/// The flattened allowlist: every group's syscalls, in order.
fn allowlist_numbers() -> Vec<libc::c_long> {
    ALLOWLIST
        .iter()
        .flat_map(|group| group.syscalls.iter().copied())
        .collect()
}

/// The BPF program: an architecture check, then one allow decision per
/// allowlisted syscall (`socket` after a family check), then the default action
/// for everything else. All jumps are short constants, so the layout is
/// verifiable by reading it (and by the interpreter in the tests).
fn program(numbers: &[libc::c_long], on_miss: OnMiss) -> Result<Vec<u64>, JailError> {
    // 4 instructions of overhead (arch check, default action, load nr) plus 2
    // per plain syscall and 5 per family-filtered one.
    let body: usize = numbers
        .iter()
        .map(|&nr| if is_family_filtered(nr) { 5 } else { 2 })
        .sum();
    let mut program = Vec::with_capacity(4 + body);

    // 0: load the caller's architecture.
    program.push(stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_ARCH));
    // 1: if it matches, skip the arch refusal at 2; else fall through to it.
    program.push(jump(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH, 1, 0));
    // 2: a non-native syscall ABI is never on the allowlist.
    program.push(stmt(BPF_RET | BPF_K, on_miss.ret()));
    // 3: load the syscall number.
    program.push(stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_NR));

    for &nr in numbers {
        let nr = nr_u32(nr);
        if is_family_filtered(nr as libc::c_long) {
            // `nr == socket?` — if not, jump past this block (4 instructions
            // ahead: `LD`, `JEQ`, `RET ALLOW`, `RET deny`) to the next check.
            program.push(jump(BPF_JMP | BPF_JEQ | BPF_K, nr, 0, 4));
            // Load arg0 (the address family) and allow only AF_UNIX.
            program.push(stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_ARG0));
            program.push(jump(BPF_JMP | BPF_JEQ | BPF_K, AF_UNIX_ALLOWED, 0, 1));
            program.push(stmt(BPF_RET | BPF_K, RET_ALLOW));
            program.push(stmt(BPF_RET | BPF_K, on_miss.ret()));
        } else {
            // `nr == this?` — else fall through to the next check.
            program.push(jump(BPF_JMP | BPF_JEQ | BPF_K, nr, 0, 1));
            program.push(stmt(BPF_RET | BPF_K, RET_ALLOW));
        }
    }

    // The terminal action: a number that matched nothing above.
    program.push(stmt(BPF_RET | BPF_K, on_miss.ret()));

    if program.len() > sys::BPF_MAX_LEN {
        return Err(JailError::Program(format!(
            "{} instructions, over the kernel's maximum of {}",
            program.len(),
            sys::BPF_MAX_LEN
        )));
    }
    Ok(program)
}

/// A syscall number as the `u32` the BPF program compares against.
fn nr_u32(nr: libc::c_long) -> u32 {
    // Syscall numbers are small positive integers; the truncation is exact.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let nr = nr as u32;
    nr
}

/// Every syscall this port names, as `(name, number)`. [`syscall_name`] and
/// [`syscall_number`] are both lookups into this one table, so they cannot
/// disagree; `every_named_syscall_round_trips` asserts it (and that every
/// allowlisted number is in it). The two names that differ between the kernels
/// are gated the way the allowlist's are.
const NAMED: &[(&str, libc::c_long)] = &[
    ("exit", libc::SYS_exit),
    ("exit_group", libc::SYS_exit_group),
    ("sched_yield", libc::SYS_sched_yield),
    ("getpid", libc::SYS_getpid),
    ("gettid", libc::SYS_gettid),
    ("clone", libc::SYS_clone),
    ("clone3", libc::SYS_clone3),
    ("futex", libc::SYS_futex),
    ("set_robust_list", libc::SYS_set_robust_list),
    ("rseq", libc::SYS_rseq),
    ("set_tid_address", libc::SYS_set_tid_address),
    ("prlimit64", libc::SYS_prlimit64),
    ("getrandom", libc::SYS_getrandom),
    ("sched_getaffinity", libc::SYS_sched_getaffinity),
    ("prctl", libc::SYS_prctl),
    ("rt_sigaction", libc::SYS_rt_sigaction),
    ("rt_sigprocmask", libc::SYS_rt_sigprocmask),
    ("rt_sigreturn", libc::SYS_rt_sigreturn),
    ("sigaltstack", libc::SYS_sigaltstack),
    ("tgkill", libc::SYS_tgkill),
    ("clock_gettime", libc::SYS_clock_gettime),
    ("clock_nanosleep", libc::SYS_clock_nanosleep),
    ("mmap", libc::SYS_mmap),
    ("munmap", libc::SYS_munmap),
    ("madvise", libc::SYS_madvise),
    ("mprotect", libc::SYS_mprotect),
    ("brk", libc::SYS_brk),
    ("openat", libc::SYS_openat),
    ("close", libc::SYS_close),
    ("read", libc::SYS_read),
    ("write", libc::SYS_write),
    ("lseek", libc::SYS_lseek),
    ("fcntl", libc::SYS_fcntl),
    ("fsync", libc::SYS_fsync),
    #[cfg(target_arch = "x86_64")]
    ("unlink", libc::SYS_unlink),
    ("unlinkat", libc::SYS_unlinkat),
    ("eventfd2", libc::SYS_eventfd2),
    ("epoll_create1", libc::SYS_epoll_create1),
    ("epoll_ctl", libc::SYS_epoll_ctl),
    #[cfg(target_arch = "x86_64")]
    ("epoll_wait", libc::SYS_epoll_wait),
    #[cfg(not(target_arch = "x86_64"))]
    ("epoll_pwait", libc::SYS_epoll_pwait),
    #[cfg(target_arch = "x86_64")]
    ("poll", libc::SYS_poll),
    #[cfg(not(target_arch = "x86_64"))]
    ("ppoll", libc::SYS_ppoll),
    ("ioctl", libc::SYS_ioctl),
    ("socket", libc::SYS_socket),
    ("bind", libc::SYS_bind),
    ("listen", libc::SYS_listen),
    ("accept4", libc::SYS_accept4),
    ("connect", libc::SYS_connect),
    ("recvfrom", libc::SYS_recvfrom),
    ("sendto", libc::SYS_sendto),
];

/// The name of a syscall number, for diagnostics and tests.
fn syscall_name(nr: libc::c_long) -> &'static str {
    NAMED
        .iter()
        .find(|(_, number)| *number == nr)
        .map_or("unknown", |(name, _)| name)
}

/// The inverse of [`syscall_name`]: the number a name refers to, for
/// [`Jail::arm_named`]. `None` for a name this port does not know; the caller
/// refuses it rather than dropping it from the filter it is building.
fn syscall_number(name: &str) -> Option<libc::c_long> {
    NAMED
        .iter()
        .find(|(named, _)| *named == name)
        .map(|(_, number)| *number)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `program` the way the kernel's BPF interpreter would, for one
    /// `seccomp_data`: `(arch, nr, arg0)` → the BPF return value. The program is
    /// straight-line `LD|W|ABS` / `JEQ` / `RET` code, so this is the whole
    /// instruction set it can contain.
    fn run(program: &[u64], arch: u32, nr: u32, arg0: u32) -> u32 {
        let mut acc: u32 = 0;
        let mut pc: usize = 0;
        loop {
            let insn = program[pc];
            let code = (insn & 0xffff) as u16;
            let jt = ((insn >> 16) & 0xff) as usize;
            let jf = ((insn >> 24) & 0xff) as usize;
            let k = (insn >> 32) as u32;
            if code == BPF_LD | BPF_W | BPF_ABS {
                acc = match k {
                    OFFSET_ARCH => arch,
                    OFFSET_NR => nr,
                    OFFSET_ARG0 => arg0,
                    other => panic!("instruction {pc} loads offset {other}, which is unknown"),
                };
                pc += 1;
            } else if code == BPF_JMP | BPF_JEQ | BPF_K {
                pc += if acc == k { 1 + jt } else { 1 + jf };
            } else if code == BPF_RET | BPF_K {
                return k;
            } else {
                panic!("instruction {pc} has an unexpected code {code:#x}");
            }
        }
    }

    /// The program builds for both default actions, and stays inside the
    /// kernel's limits: no more than `BPF_MAX_LEN` instructions, and every jump
    /// lands inside the program.
    #[test]
    fn the_filter_builds_with_valid_offsets() {
        for on_miss in [OnMiss::Kill, OnMiss::Log] {
            let program = program(&allowlist_numbers(), on_miss).expect("must build a program");
            assert!(!program.is_empty());
            assert!(
                program.len() <= sys::BPF_MAX_LEN,
                "{} instructions exceeds the kernel's {}",
                program.len(),
                sys::BPF_MAX_LEN
            );

            // Decode each instruction and check every jump stays in bounds.
            for (index, insn) in program.iter().enumerate() {
                let code = (*insn & 0xffff) as u16;
                let jt = ((*insn >> 16) & 0xff) as usize;
                let jf = ((*insn >> 24) & 0xff) as usize;
                let is_jump = code & 0x07 == BPF_JMP && code & 0xf0 == BPF_JEQ;
                if is_jump {
                    assert!(
                        index + 1 + jt < program.len(),
                        "jump jt at {index} leaves the program"
                    );
                    assert!(
                        index + 1 + jf < program.len(),
                        "jump jf at {index} leaves the program"
                    );
                }
            }

            // The program ends with the default action, so a number matching
            // nothing gets it rather than falling off the end (which the kernel
            // would reject).
            let last = *program.last().expect("non-empty");
            assert_eq!((last & 0xffff) as u16, BPF_RET | BPF_K);
            assert_eq!((last >> 32) as u32, on_miss.ret());
        }
    }

    /// What the filter decides, run through the interpreter: every allowlisted
    /// syscall is admitted, `socket` is admitted only for `AF_UNIX`, and an
    /// unnamed syscall — or any syscall on a foreign syscall ABI — gets the
    /// default action, `KILL_PROCESS` or `LOG` as asked.
    #[test]
    fn the_program_admits_the_allowlist_and_refuses_everything_else() {
        for on_miss in [OnMiss::Kill, OnMiss::Log] {
            let program = program(&allowlist_numbers(), on_miss).expect("must build");
            for nr in allowlist_numbers() {
                let arg0 = if is_family_filtered(nr) {
                    AF_UNIX_ALLOWED
                } else {
                    0
                };
                assert_eq!(
                    run(&program, AUDIT_ARCH, nr_u32(nr), arg0),
                    RET_ALLOW,
                    "{} must be admitted",
                    syscall_name(nr)
                );
            }
            assert_eq!(
                run(
                    &program,
                    AUDIT_ARCH,
                    nr_u32(libc::SYS_socket),
                    libc::AF_INET as u32
                ),
                on_miss.ret(),
                "socket(AF_INET) is refused by the argument filter"
            );
            // 0x1234 is outside every syscall table this port targets.
            assert_eq!(run(&program, AUDIT_ARCH, 0x1234, 0), on_miss.ret());
            assert_eq!(
                run(&program, 0xdead_beef, nr_u32(libc::SYS_read), 0),
                on_miss.ret(),
                "another syscall ABI is refused whatever the number names"
            );
            // The log arm's action is the kernel's `SECCOMP_RET_LOG`, not a
            // value that only looks like it in this file.
            assert_eq!(OnMiss::Log.ret(), 0x7ffc_0000);
        }
    }

    /// The allowlist is non-trivial and has no duplicate syscall numbers.
    #[test]
    fn the_allowlist_is_deduplicated() {
        let mut numbers = allowlist_numbers();
        let count = numbers.len();
        assert!(count > 30, "the allowlist should be the VMM's real set");
        numbers.sort_unstable();
        numbers.dedup();
        assert_eq!(numbers.len(), count, "a syscall is listed twice");
    }

    /// Every entry of the trace's observed set — the syscalls the derivation
    /// trace saw on an *armed* VMM thread — is on the allowlist. This is the
    /// check the incident's list failed: `recvfrom` and `mprotect` were missing
    /// from it while two real paths used them.
    #[test]
    fn every_syscall_the_trace_observed_is_allowlisted() {
        let ours: Vec<&str> = Jail::allowlist().map(|(name, _, _)| name).collect();
        for name in TRACE_OBSERVED.iter().chain(TRACE_OBSERVED_ARCH) {
            assert!(
                ours.contains(name),
                "{name} was observed on an armed thread and must be allowlisted"
            );
        }
    }

    /// Every entry the trace derivation added is on the allowlist, is a syscall
    /// this port names, and carries the evidence that put it there.
    #[test]
    fn every_trace_added_entry_is_on_the_allowlist() {
        let ours: Vec<&str> = Jail::allowlist().map(|(name, _, _)| name).collect();
        for (name, evidence) in TRACE_ADDED {
            assert!(
                ours.contains(name),
                "{name} is in TRACE_ADDED but not the list"
            );
            assert_ne!(syscall_number(name), None, "{name} needs a number");
            assert!(!evidence.is_empty(), "{name} needs its evidence");
        }
        for (name, _) in TRACE_ADDED {
            assert_eq!(
                TRACE_ADDED.iter().filter(|(n, _)| n == name).count(),
                1,
                "{name} is documented twice"
            );
        }
    }

    /// Every allowlisted number has a name, every name maps back to its number,
    /// and no name or number is in the table twice — so diagnostics, the
    /// Firecracker comparison and `Jail::arm_named` all read one table.
    #[test]
    fn every_named_syscall_round_trips() {
        for (name, nr, _) in Jail::allowlist() {
            assert_ne!(name, "unknown", "syscall {nr} needs a name in NAMED");
        }
        for (name, nr) in NAMED {
            assert_eq!(syscall_name(*nr), *name);
            assert_eq!(syscall_number(name), Some(*nr));
        }
        let mut names: Vec<&str> = NAMED.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "a syscall name is in NAMED twice");
        assert_eq!(syscall_number("no_such_syscall"), None);
        assert_eq!(syscall_name(-1), "unknown");
    }

    /// Both directions of the comparison with Firecracker's reference are
    /// exactly the documented sets — what this port dropped, and what it added
    /// — each with a reason.
    #[test]
    fn every_firecracker_reference_entry_is_accounted_for() {
        let ours: Vec<&str> = Jail::allowlist().map(|(name, _, _)| name).collect();

        let mut dropped: Vec<&str> = FC_REFERENCE
            .iter()
            .copied()
            .filter(|name| !ours.contains(name))
            .collect();
        let mut documented_dropped: Vec<&str> = FC_DROPPED
            .iter()
            .chain(FC_DROPPED_ARCH)
            .map(|(name, _)| *name)
            .collect();
        dropped.sort_unstable();
        documented_dropped.sort_unstable();
        assert_eq!(
            dropped, documented_dropped,
            "the syscalls dropped from Firecracker's reference must be the documented set"
        );

        let mut added: Vec<&str> = ours
            .iter()
            .copied()
            .filter(|name| !FC_REFERENCE.contains(name))
            .collect();
        let mut documented_added: Vec<&str> = FC_ADDED
            .iter()
            .chain(FC_ADDED_ARCH)
            .map(|(name, _)| *name)
            .collect();
        added.sort_unstable();
        documented_added.sort_unstable();
        assert_eq!(
            added, documented_added,
            "the syscalls added to Firecracker's reference must be the documented set"
        );

        for (_, why) in FC_DROPPED
            .iter()
            .chain(FC_ADDED)
            .chain(FC_DROPPED_ARCH)
            .chain(FC_ADDED_ARCH)
        {
            assert!(!why.is_empty(), "every difference needs a reason");
        }
    }

    /// The environment parsing: exactly three mode names, case-insensitive and
    /// trimmed, and *any* other value refused rather than defaulted.
    #[test]
    fn a_jail_env_value_that_names_no_mode_is_refused() {
        assert_eq!(parse_jail_env("kill").expect("kill"), Mode::Kill);
        assert_eq!(parse_jail_env(" LOG ").expect("log"), Mode::Log);
        assert_eq!(parse_jail_env("Off").expect("off"), Mode::Off);
        // Surrounding whitespace is ignored, so a value a shell appended a
        // newline to still names its mode.
        assert_eq!(parse_jail_env("kill\n").expect("kill"), Mode::Kill);
        for typo in ["", " ", "loud", "none", "0", "false", "offr", "kill log"] {
            assert!(
                matches!(parse_jail_env(typo), Err(JailError::Config(_))),
                "{typo:?} names no mode and must be refused"
            );
        }
    }

    /// The older switch: a set, non-empty value that is not an explicit false
    /// disables the jail; its false spellings and its absence do not.
    #[test]
    fn the_old_switch_disables_the_jail_only_when_it_says_so() {
        for on in ["1", "true", "yes", "TRUE", "anything"] {
            assert!(parse_no_jail_env(on), "{on:?} disables the jail");
        }
        for off in ["", " ", "0", "false", "no", "off", "OFF"] {
            assert!(!parse_no_jail_env(off), "{off:?} must not disable the jail");
        }
    }

    /// The three modes are what a caller sets, and the programmatic switch
    /// beats the environment. The tests here never read the environment except
    /// after setting the mode, so the two are independent even though the
    /// environment is process-global.
    #[test]
    fn the_mode_is_set_programmatically_or_defaults_to_armed() {
        for want in [Mode::Kill, Mode::Log, Mode::Off] {
            set_mode(want);
            assert_eq!(mode().expect("a set mode is never an error"), want);
            assert_eq!(is_enabled(), want != Mode::Off, "{want:?}");
        }
        // `set_enabled` is the two-valued shorthand for the same switch.
        set_enabled(true);
        assert_eq!(mode().expect("kill"), Mode::Kill);
        set_enabled(false);
        assert_eq!(mode().expect("off"), Mode::Off);
        assert!(!is_enabled());
        // Back to "read the environment" for any later test.
        MODE.store(MODE_FROM_ENV, Ordering::SeqCst);
    }
}
