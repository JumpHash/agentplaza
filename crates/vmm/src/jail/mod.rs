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
//! on the allowlist meets `SECCOMP_RET_KILL_PROCESS`: the whole process is
//! killed by `SIGSYS`, which is the honest choice for a deployment — a VMM that
//! reaches for a syscall its allowlist does not cover is either compromised or
//! buggy, and the supervisor (the node daemon or fleet supervisor that owns the
//! VM handle) tears the VM down when its threads disappear. There is no
//! `SECCOMP_RET_ERRNO` middle ground and no `SIGSYS` handler to limp along
//! through: the jail is a boundary, not a warning.
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
//! **The escape hatch.** `Jail::arm` is a no-op when the process setting says
//! the jail is off. The setting is read once, lazily, from the environment
//! variable `APZ_VMM_NO_JAIL`: set it to anything but `"0"`/`"false"`/`"no"`/
//! `"off"` (or empty) and every VMM thread skips the install. **Absent — the
//! default — the jail is armed.** [`Jail::set_enabled`] flips the same switch
//! programmatically, which is how a CLI (`vmctl --no-jail`) or an acceptance
//! suite compares jailed against unjailed runs. (The flag itself belongs to the
//! binary that owns the CLI; this is the library half of it.)
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
/// `SECCOMP_RET_KILL_PROCESS`: kill the whole process with `SIGSYS`.
const RET_KILL_PROCESS: u32 = 0x8000_0000;

/// What to do with a syscall the allowlist does not name:
/// `SECCOMP_RET_KILL_PROCESS` in every build that ships.
///
/// The discovery procedure behind [`ALLOWLIST`] — a temporary switch to
/// `SECCOMP_RET_LOG` (which allows *and* logs) plus an empty list, and then
/// removing entries and re-running until the boot and session stay clean —
/// lives in the repository's task record, not in a shipped knob. (The kernel
/// rate-limits those audit records — `kauditd_printk_skb: N callbacks
/// suppressed` — so the iterative `KILL` probe is what pinned the last two
/// entries; `LOG` mode is only a first pass.)
const RET_DENY: u32 = RET_KILL_PROCESS;

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
/// here, grouped by what needs it; everything else is killed.
///
/// It was built and then cut down, not guessed: a first pass had roughly twenty
/// more entries (the `readv`/`writev` and `sendmsg`/`recvmsg` families, the
/// `timerfd` pair, `pipe2`, `mprotect`, `dup`/`dup3`, `pread64`/`pwrite64`,
/// `statx`/`newfstatat`/`fstat`, `ftruncate`, `socketpair`, and the `ppoll`/
/// `epoll_pwait` pair on `x86_64`), and each was removed with the boot and the
/// live vsock session re-run clean. Two more (`poll`, `unlink`) were *added*
/// after an iterative `SIGSYS` probe found them. The result is verified by a
/// boot, a live vsock session and a two-VM node-daemon run with this exact list
/// armed and no `SIGSYS` anywhere; it is not claimed to be minimal entry by
/// entry — the entries left are the ones a path in those scenarios exercises or
/// reaches. See the port's task record for the transcripts.
///
/// Firecracker's `vmm` and `vcpu` thread filters are the reference the list
/// started from ([`FC_REFERENCE`]); `every_firecracker_reference_entry_is_accounted_for`
/// records both what this port dropped and what it added.
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
              threads share, and the per-thread setup a thread inherits this filter into \
              would make at its start — the three threads today arm themselves, so a \
              future fourth would make these calls *under* the inherited filter.",
        syscalls: &[
            libc::SYS_clone,
            libc::SYS_clone3,
            libc::SYS_futex,
            libc::SYS_set_robust_list,
            libc::SYS_rseq,
            libc::SYS_set_tid_address,
            libc::SYS_prlimit64,
            libc::SYS_getrandom,
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
        why: "Guest memory, the heap, and the allocator's use of madvise/brk — buffers \
              the VMM allocates while the VM runs.",
        syscalls: &[
            libc::SYS_mmap,
            libc::SYS_munmap,
            libc::SYS_madvise,
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
              socket this VMM ever makes (there is no network device). Its reads and writes \
              go through read(2)/write(2), not sendmsg/recvmsg. socket(2) is also \
              argument-filtered to AF_UNIX.",
        syscalls: &[
            libc::SYS_socket,
            libc::SYS_bind,
            libc::SYS_listen,
            libc::SYS_accept4,
            libc::SYS_connect,
        ],
    },
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
    (
        "mprotect",
        "the guest regions keep their mapping protection after arming",
    ),
    ("readv", "the vsock muxer reads with read(2)"),
    ("writev", "the vsock muxer writes with write(2)"),
    ("recvfrom", "no datagram socket in the port"),
    ("recvmsg", "the muxer reads with read(2)"),
    ("sendto", "no datagram socket in the port"),
    ("sendmsg", "the muxer writes with write(2)"),
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
        "tgkill",
        "the vCPU kick is tgkill(2); Firecracker lists tkill(2)",
    ),
];

/// Errors from [`Jail::arm`].
#[derive(Debug, thiserror::Error)]
pub enum JailError {
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

/// Whether to arm. `0` = not decided yet (read the environment), `1` = forced
/// on, `2` = forced off.
static ENABLED: AtomicU8 = AtomicU8::new(0);

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

/// The environment variable that turns the jail **off** when set to a
/// non-empty, non-false value. Its absence means the jail is armed.
pub const NO_JAIL_ENV: &str = "APZ_VMM_NO_JAIL";

/// Forces the jail on (`true`) or off (`false`), overriding the environment for
/// the rest of the process. The library half of a `--no-jail` flag.
pub fn set_enabled(on: bool) {
    ENABLED.store(if on { 1 } else { 2 }, Ordering::SeqCst);
}

/// Whether the jail is armed: forced if [`set_enabled`] was called, otherwise
/// off only when `APZ_VMM_NO_JAIL` is set to a truthy value.
pub fn is_enabled() -> bool {
    match ENABLED.load(Ordering::SeqCst) {
        1 => true,
        2 => false,
        _ => !env_says_disabled(),
    }
}

/// Reads [`NO_JAIL_ENV`]; a set, non-empty value that is not an explicit false
/// turns the jail off.
fn env_says_disabled() -> bool {
    match std::env::var(NO_JAIL_ENV) {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ),
        Err(_) => false,
    }
}

/// The VMM's jail: one BPF allowlist filter, installed per VMM thread.
pub struct Jail;

impl Jail {
    /// Builds and installs the filter on the calling thread, unless the process
    /// setting has the jail off.
    ///
    /// Fails closed: an error means the filter is **not** installed. A kernel
    /// without `CONFIG_SECCOMP_FILTER` — or a user-mode emulator such as
    /// qemu-user, which answers `PR_SET_SECCOMP` with `EINVAL` — is reported as
    /// [`JailError::Unsupported`]; there is no filter to install there, so
    /// [`Jail::arm_thread`] (what the VMM threads call) reports it and carries
    /// on, while a strict caller can tell the difference. Every other failure
    /// is fatal to the thread that asked.
    pub fn arm() -> Result<(), JailError> {
        if !is_enabled() {
            return Ok(());
        }
        Self::arm_forced()
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

    /// Arms the filter regardless of the process setting. The library half of
    /// "the flag's absence = armed"; [`Jail::arm`] is what the threads call.
    fn arm_forced() -> Result<(), JailError> {
        let program = program()?;
        sys::install(&program).map_err(|err| {
            // `EINVAL` from either `prctl(2)` here is "this kernel cannot
            // install a filter" (the program itself is validated by the
            // caller's tests and by the length check): a valid
            // `PR_SET_NO_NEW_PRIVS`/`PR_SET_SECCOMP` pair cannot fail that way
            // on a kernel that implements them.
            if err.raw_os_error() == Some(libc::EINVAL) {
                JailError::Unsupported(err)
            } else {
                JailError::Install(err)
            }
        })
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

/// The BPF program: an architecture check, then one allow decision per
/// allowlisted syscall (`socket` after a family check), then the deny. All
/// jumps are short constants, so the layout is verifiable by reading it (see
/// the tests).
fn program() -> Result<Vec<u64>, JailError> {
    let numbers: Vec<libc::c_long> = ALLOWLIST
        .iter()
        .flat_map(|group| group.syscalls.iter().copied())
        .collect();

    // 4 instructions of overhead (arch check, deny, load nr) plus 2 per plain
    // syscall and 5 per family-filtered one.
    let body: usize = numbers
        .iter()
        .map(|&nr| if is_family_filtered(nr) { 5 } else { 2 })
        .sum();
    let mut program = Vec::with_capacity(4 + body);

    // 0: load the caller's architecture.
    program.push(stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_ARCH));
    // 1: if it matches, skip the deny at 2; else fall through to it.
    program.push(jump(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH, 1, 0));
    // 2: a non-native syscall ABI is not on the allowlist.
    program.push(stmt(BPF_RET | BPF_K, RET_DENY));
    // 3: load the syscall number.
    program.push(stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_NR));

    for &nr in &numbers {
        let nr = nr_u32(nr);
        if is_family_filtered(nr as libc::c_long) {
            // `nr == socket?` — if not, jump past this block (4 instructions
            // ahead: `LD`, `JEQ`, `RET ALLOW`, `RET DENY`) to the next check.
            program.push(jump(BPF_JMP | BPF_JEQ | BPF_K, nr, 0, 4));
            // Load arg0 (the address family) and allow only AF_UNIX.
            program.push(stmt(BPF_LD | BPF_W | BPF_ABS, OFFSET_ARG0));
            program.push(jump(BPF_JMP | BPF_JEQ | BPF_K, AF_UNIX_ALLOWED, 0, 1));
            program.push(stmt(BPF_RET | BPF_K, RET_ALLOW));
            program.push(stmt(BPF_RET | BPF_K, RET_DENY));
        } else {
            // `nr == this?` — else fall through to the next check.
            program.push(jump(BPF_JMP | BPF_JEQ | BPF_K, nr, 0, 1));
            program.push(stmt(BPF_RET | BPF_K, RET_ALLOW));
        }
    }

    // The terminal deny: a number that matched nothing above.
    program.push(stmt(BPF_RET | BPF_K, RET_DENY));

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

/// The name of a syscall number, for diagnostics and tests.
fn syscall_name(nr: libc::c_long) -> &'static str {
    match nr {
        libc::SYS_exit => "exit",
        libc::SYS_exit_group => "exit_group",
        libc::SYS_sched_yield => "sched_yield",
        libc::SYS_getpid => "getpid",
        libc::SYS_gettid => "gettid",
        libc::SYS_clone => "clone",
        libc::SYS_clone3 => "clone3",
        libc::SYS_futex => "futex",
        libc::SYS_set_robust_list => "set_robust_list",
        libc::SYS_rseq => "rseq",
        libc::SYS_set_tid_address => "set_tid_address",
        libc::SYS_prlimit64 => "prlimit64",
        libc::SYS_getrandom => "getrandom",
        libc::SYS_rt_sigaction => "rt_sigaction",
        libc::SYS_rt_sigprocmask => "rt_sigprocmask",
        libc::SYS_rt_sigreturn => "rt_sigreturn",
        libc::SYS_sigaltstack => "sigaltstack",
        libc::SYS_tgkill => "tgkill",
        libc::SYS_clock_gettime => "clock_gettime",
        libc::SYS_clock_nanosleep => "clock_nanosleep",
        libc::SYS_mmap => "mmap",
        libc::SYS_munmap => "munmap",
        libc::SYS_madvise => "madvise",
        libc::SYS_brk => "brk",
        libc::SYS_openat => "openat",
        libc::SYS_close => "close",
        libc::SYS_read => "read",
        libc::SYS_write => "write",
        libc::SYS_lseek => "lseek",
        libc::SYS_fcntl => "fcntl",
        libc::SYS_fsync => "fsync",
        #[cfg(target_arch = "x86_64")]
        libc::SYS_unlink => "unlink",
        libc::SYS_unlinkat => "unlinkat",
        libc::SYS_eventfd2 => "eventfd2",
        libc::SYS_epoll_create1 => "epoll_create1",
        libc::SYS_epoll_ctl => "epoll_ctl",
        #[cfg(target_arch = "x86_64")]
        libc::SYS_epoll_wait => "epoll_wait",
        #[cfg(not(target_arch = "x86_64"))]
        libc::SYS_epoll_pwait => "epoll_pwait",
        #[cfg(target_arch = "x86_64")]
        libc::SYS_poll => "poll",
        #[cfg(not(target_arch = "x86_64"))]
        libc::SYS_ppoll => "ppoll",
        libc::SYS_ioctl => "ioctl",
        libc::SYS_socket => "socket",
        libc::SYS_bind => "bind",
        libc::SYS_listen => "listen",
        libc::SYS_accept4 => "accept4",
        libc::SYS_connect => "connect",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The program builds, and stays inside the kernel's limits: no more than
    /// `BPF_MAX_LEN` instructions, and every jump lands inside the program.
    #[test]
    fn the_filter_builds_with_valid_offsets() {
        let program = program().expect("the allowlist must build a program");
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

        // The program ends with a deny, so a number matching nothing is killed
        // rather than falling off the end (which the kernel would reject).
        let last = *program.last().expect("non-empty");
        assert_eq!((last & 0xffff) as u16, BPF_RET | BPF_K);
        assert_eq!((last >> 32) as u32, RET_DENY);
    }

    /// The allowlist is non-trivial and has no duplicate syscall numbers.
    #[test]
    fn the_allowlist_is_deduplicated() {
        let mut numbers: Vec<libc::c_long> = ALLOWLIST
            .iter()
            .flat_map(|group| group.syscalls.iter().copied())
            .collect();
        let count = numbers.len();
        assert!(count > 30, "the allowlist should be the VMM's real set");
        numbers.sort_unstable();
        numbers.dedup();
        assert_eq!(numbers.len(), count, "a syscall is listed twice");
    }

    /// Every allowlisted number has a name, so diagnostics and the reference
    /// comparison can rely on `syscall_name`.
    #[test]
    fn every_allowlisted_syscall_has_a_name() {
        for (name, nr, _) in Jail::allowlist() {
            assert_ne!(name, "unknown", "syscall {nr} needs a name in syscall_name");
        }
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

    /// The default is armed; `APZ_VMM_NO_JAIL` and `set_enabled` are the ways
    /// off.
    #[test]
    fn the_jail_is_armed_unless_turned_off() {
        // The environment is process-global and other tests may set it; use the
        // forced switch, which overrides the environment deterministically.
        set_enabled(true);
        assert!(is_enabled());
        set_enabled(false);
        assert!(!is_enabled());
        // Restore the environment-derived default for any later test.
        ENABLED.store(0, Ordering::SeqCst);
    }
}
