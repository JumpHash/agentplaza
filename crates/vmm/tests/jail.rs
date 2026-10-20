// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// The VMM jail's probes, as an integration test — its own process, separate
// from the unit-test binary. Two reasons:
//
//  * the `kill` arm's refusal is `SECCOMP_RET_KILL_PROCESS`, so a probe that
//    works kills the process it runs in; and
//  * the probe child is a `fork`-free re-execution of this binary, so it
//    inherits no file descriptors of a sibling test's (a `fork`-based probe in
//    the shared unit-test process made an unrelated `vhost` test — which holds
//    a `/dev/vhost-vsock` guest CID until the last reference to its fd goes
//    away — fail).
//
// The child uses only `std`, so this test crate needs no `unsafe`:
//
//  * `std::env::current_dir()` makes `getcwd(2)`, which is not on the
//    allowlist;
//  * `std::net::TcpStream::connect(..)` makes `socket(AF_INET, ..)` — the
//    syscall *is* allowlisted, for `AF_UNIX` only, so this exercises the
//    address-family argument filter;
//  * `std::process::id()` makes `getpid(2)`, which is allowlisted;
//  * a `read(2)` of `/proc/thread-self/status` reads back the kernel's view of
//    this thread's filter (`Seccomp: 2`);
//  * a `UnixStream::read`/`write` is the C library's `recv`/`send`, which is
//    what the muxer's `read_local_stream_port`/`send_bytes_raw` use — the two
//    calls the live fleet was killed on; and
//  * `std::thread::spawn` from an armed thread makes a thread that *inherits*
//    the filter and runs its whole libc and Rust bootstrap under it, which is
//    where the allocator's `mprotect` is.
//
// Every probe runs twice where it matters: once against the allowlist this port
// shipped in `6ce364b` (`PRE_FIX_ALLOWLIST`, the list the live fleet died
// under) and once against the shipped one. The first is the reproduction, the
// second is the fix. The mode switch is probed the same way, through the
// environment of the child, so what is tested is the wiring and not the parser
// (which the crate's unit tests cover).
//
// Every probe that arms a filter is judged from what the child *reports* about
// itself, never from the platform's name. After it arms, the child reads its
// own `/proc/thread-self/status` and reports the `Seccomp:` mode the kernel
// gives it, and the parent combines that with how the probe ended:
//
// | arm result                        | probe result        | verdict   |
// |-----------------------------------|---------------------|-----------|
// | refused (`EINVAL`), `Seccomp: 0`  | either              | SKIP      |
// | accepted, but `Seccomp` is not 2  | either              | FAIL      |
// | accepted, `Seccomp: 2`            | syscall killed      | PASS      |
// | accepted, `Seccomp: 2`            | syscall admitted    | PASS/SKIP |
//
// The last two rows are the point of the report: a kernel that enforces the
// filter kills the refused syscall (`PASS` when that is what the probe
// expects, a failure otherwise), while a platform that installs the filter and
// does not apply it — which is what a user-mode emulator does to a filter
// built for the guest ABI — lets it through, and the probe is a documented
// `SKIP` with the child's own evidence. A refused arm is `SKIP` too: qemu-user
// answers `PR_SET_SECCOMP` with `EINVAL`, so there is no filter to enforce and
// the jail is *unavailable*, not misconfigured. An arm the kernel *accepted*
// but that left `Seccomp` at 0 is the one case that must never be skipped —
// that is a real arming regression.
//
// A `SKIP` is written to the run's stderr directly (see `common::note`),
// because `libtest` hides a passing test's captured output and a reader has to
// be able to tell a skip from a pass in an ordinary `cargo test`. The probe
// child itself is launched through cargo's cross-target runner
// (`common::command`), without which a re-executed aarch64 child dies in the
// dynamic loader under `qemu-user` before it can arm anything.
#![cfg(target_os = "linux")]

mod common;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Stdio};

use common::note;
use vmm::jail::{Jail, JailError, Mode, OnMiss};

/// The environment variable the child reads to learn which syscall to make.
const PROBE_ENV: &str = "VMM_JAIL_PROBE";

/// The environment variable the child reads to learn which filter to arm.
const ARM_ENV: &str = "VMM_JAIL_PROBE_ARM";

/// The (ignored) test the probes re-execute as a child.
const PROBE_TEST: &str = "the_probe_child";

/// The child's exit status when this platform cannot install a filter at all:
/// its arm was refused. The parent reports the child's evidence and skips, the
/// way the crate's other capability probes do.
const SKIP: i32 = 77;

/// The child's exit status when the arm was *accepted* but the kernel is not
/// enforcing it (`/proc/thread-self/status` does not say `Seccomp: 2`). This is
/// a real arming regression, and the parent fails on it — it is never skipped.
const NOT_IN_EFFECT: i32 = 70;

/// The child's exit statuses for the modes it was told to be in, so the parent
/// can assert that the child's *process* read the environment (and not only
/// that this crate can parse a string).
const MODE_KILL: i32 = 60;
const MODE_LOG: i32 = 61;
const MODE_OFF: i32 = 62;
const MODE_INVALID: i32 = 63;

/// The child's exit status when arming was refused: a misconfigured jail must
/// stop the thread, never fall back to a mode.
const REFUSED: i32 = 64;

/// How long one probe child is given. Every probe is a handful of syscalls on
/// a process that is already running; a child that outlives this is blocking on
/// something the test cannot wait for.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The allowlist this port shipped in `6ce364b` — the list the live `noded`
/// fleet was running when its vCPU and muxer threads were killed — frozen here
/// so the regression probes can show what it refused. It is the shipped list
/// minus what the trace added ([`vmm::jail::TRACE_ADDED`]): `recvfrom`,
/// `sendto`, `mprotect`, `sched_getaffinity` and `prctl`. The arch-specific
/// spellings are separate below, exactly as the shipped list keeps them.
const PRE_FIX_ALLOWLIST: &[&str] = &[
    "exit",
    "exit_group",
    "sched_yield",
    "getpid",
    "gettid",
    "clone",
    "clone3",
    "futex",
    "set_robust_list",
    "rseq",
    "set_tid_address",
    "prlimit64",
    "getrandom",
    "rt_sigaction",
    "rt_sigprocmask",
    "rt_sigreturn",
    "sigaltstack",
    "tgkill",
    "clock_gettime",
    "clock_nanosleep",
    "mmap",
    "munmap",
    "madvise",
    "brk",
    "openat",
    "close",
    "read",
    "write",
    "lseek",
    "fcntl",
    "fsync",
    "eventfd2",
    "epoll_create1",
    "epoll_ctl",
    "ioctl",
    "socket",
    "bind",
    "listen",
    "accept4",
    "connect",
];

/// `x86_64` spells the epoll wait, the poll and the unlink `unlink`/
/// `epoll_wait`/`poll`; `aarch64` has none of those three and glibc uses
/// `unlinkat`/`epoll_pwait`/`ppoll`.
#[cfg(target_arch = "x86_64")]
const PRE_FIX_ALLOWLIST_ARCH: &[&str] = &["unlink", "unlinkat", "epoll_wait", "poll"];
#[cfg(not(target_arch = "x86_64"))]
const PRE_FIX_ALLOWLIST_ARCH: &[&str] = &["unlinkat", "epoll_pwait", "ppoll"];

/// The pre-fix list with this kernel's spellings.
fn pre_fix_allowlist() -> Vec<&'static str> {
    PRE_FIX_ALLOWLIST
        .iter()
        .chain(PRE_FIX_ALLOWLIST_ARCH)
        .copied()
        .collect()
}

/// What a working jail does to a probe's syscall.
#[derive(Clone, Copy)]
enum Expect {
    /// The allowlist does not name the syscall: the armed thread is killed.
    Killed,
    /// The allowlist names it, or the logged arm admits it: the thread runs.
    Allowed,
}

/// Re-executes this test binary as a probe child — through cargo's cross-target
/// runner, so a child under `qemu-user` runs as this test does — and returns how
/// it ended together with the child's own report (its stderr).
///
/// `arm` selects the child's filter (`"shipped"`, `"pre-fix"` or `"logged"`)
/// and `env` adds variables to the child's environment — which is how the mode
/// switch is probed.
///
/// The wait is bounded: a probe child that blocks on the socket it is probing
/// (which is what a read on a stream whose peer sends nothing does) fails the
/// test with the timeout rather than wedging the suite.
fn probe_child(what: &str, arm: &str, env: &[(&str, &str)]) -> (ExitStatus, String) {
    let exe = std::env::current_exe().expect("the test binary's path");
    let mut command = common::command(&exe);
    command
        .args([
            "--ignored",
            "--exact",
            PROBE_TEST,
            "--test-threads=1",
            "--nocapture",
        ])
        .env(PROBE_ENV, what)
        .env(ARM_ENV, arm)
        // The probe must arm what it was asked for, whatever the surrounding
        // run's setting says.
        .env_remove("APZ_VMM_JAIL")
        .env_remove("APZ_VMM_NO_JAIL");
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning the probe child");
    let deadline = std::time::Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the {what:?} probe child did not finish within {PROBE_TIMEOUT:?}");
            }
            Err(err) => panic!("waiting for the {what:?} probe child: {err}"),
        }
    };
    // The child is dead, so its stderr is at EOF: read the one-line report it
    // left (the `vmm-jail-probe:` notes the child wrote).
    let mut report = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut report);
    }
    (status, report)
}

/// The child's report of a platform that cannot *enforce* the filter.
///
/// `Some(message)` — to print and treat as a skip — when the child's arm was
/// refused and it left its evidence. Panics when the arm was *accepted* but is
/// not in effect: a real arming regression must fail, never skip. `None` when
/// the child reached a real verdict for the caller to judge.
fn unenforced(what: &str, status: &ExitStatus, report: &str) -> Option<String> {
    match status.code() {
        Some(SKIP) => Some(format!(
            "soft-skip: the {what:?} jail probe is not enforced here — this platform cannot install \
             a seccomp filter; the child reports: {}",
            report.trim()
        )),
        Some(NOT_IN_EFFECT) => panic!(
            "the {what:?} jail probe: the arm was accepted but the filter is not in effect; the \
             child reports: {}",
            report.trim()
        ),
        _ => None,
    }
}

/// Runs one probe and asserts the jail did what `expect` says.
///
/// Returns `Some(())` when the platform enforced the jail as expected. Returns
/// `None` — after printing the child's own evidence to the run's stderr — when
/// it could not: the arm was refused, or the arm was accepted with `Seccomp: 2`
/// and the forbidden syscall lived anyway. The caller returns early, so a
/// platform that cannot enforce the filter is a documented skip and never a
/// false pass or a false failure.
fn probe_expect(what: &str, arm: &str, env: &[(&str, &str)], expect: Expect) -> Option<()> {
    let (status, report) = probe_child(what, arm, env);
    if let Some(skip) = unenforced(what, &status, &report) {
        note(skip);
        return None;
    }
    match expect {
        Expect::Killed if status.signal() == Some(libc::SIGSYS) => {
            note(format!(
                "vmm-jail probe: {what:?} killed by SIGSYS — the jail refused it"
            ));
            Some(())
        }
        Expect::Allowed if status.code() == Some(0) => Some(()),
        // `Seccomp: 2` and the syscall lived. Ask the kernel whether it
        // enforces *any* filter at all — an empty allowlist kills the child on
        // its next syscall — so a platform that only installs a filter is told
        // apart from a real regression in the filter under test.
        Expect::Killed => {
            if kernel_enforces() {
                panic!(
                    "the {what:?} jail probe must be killed by the jail, yet the syscall lived \
                     while an empty allowlist does kill: the filter is in effect but does not \
                     refuse it; the child reports: {}",
                    report.trim()
                );
            }
            note(format!(
                "soft-skip: the {what:?} jail probe is not enforced here — this platform installs \
                 the seccomp filter but does not apply it; the child reports: {}",
                report.trim()
            ));
            None
        }
        Expect::Allowed => panic!(
            "the {what:?} jail probe must be admitted by the jail, but ended {status:?}; the child \
             reports: {}",
            report.trim()
        ),
    }
}

/// [`probe_expect`] with the shipped allowlist and no extra environment.
fn probe(what: &str, expect: Expect) -> Option<()> {
    probe_expect(what, "shipped", &[], expect)
}

/// The child's exit status, for the probes that assert a *wiring* code (the mode
/// switch, a refused allowlist) rather than a jail verdict. A platform that
/// cannot install a filter is still reported and skipped rather than failed.
fn probe_code(what: &str, arm: &str, env: &[(&str, &str)]) -> Option<ExitStatus> {
    let (status, report) = probe_child(what, arm, env);
    if let Some(skip) = unenforced(what, &status, &report) {
        note(skip);
        return None;
    }
    Some(status)
}

/// Whether this platform *applies* a seccomp filter at all, asked independently
/// of the allowlist under test: an **empty** allowlist's terminal rule kills the
/// child on its very next syscall, so the child dies by `SIGSYS` on any kernel
/// that enforces a filter and survives on a platform that only installs one.
///
/// This is what tells a platform that cannot enforce (skip) from a real
/// regression (fail) when an arm was accepted and reported `Seccomp: 2`, yet the
/// probe's forbidden syscall lived.
fn kernel_enforces() -> bool {
    probe_child("canary", "canary", &[]).0.signal() == Some(libc::SIGSYS)
}

/// A syscall the allowlist does not name is refused: the child dies by `SIGSYS`.
/// `getcwd(2)` is not on the allowlist at all; `socket(AF_INET)` is allowlisted for
/// `AF_UNIX` only, so the argument filter refuses it. These are the red tests. A
/// platform that cannot install a filter, or cannot apply one it installed, skips
/// instead (the enforcement canary and the child's own `Seccomp:` report tell a real
/// regression from a platform that only pretends). The filter is not a blanket deny
/// either: `the_fixed_allowlist_admits_what_the_vmm_does_after_arming` runs the calls
/// it names.
#[test]
fn the_shipped_allowlist_kills_what_it_does_not_name() {
    for what in ["getcwd", "inet"] {
        probe(what, Expect::Killed);
    }
}

/// The shipped allowlist admits what the VMM does after arming, each probed in its own
/// child: `getpid(2)` (an allowlisted call runs and the child exits cleanly), the
/// muxer's `recv` and `send` (the two calls the live fleet was killed on), a buffer
/// growing past glibc's 128 KiB mmap threshold (`mremap`, the serial capture's shape),
/// and a thread spawned after arming, whose whole libc and Rust bootstrap runs under the
/// filter (`clone`, `sched_getaffinity`, `set_robust_list`, `rseq`, `sigaltstack`, the
/// allocator's `mprotect`, `prctl(PR_SET_NAME)` and the join). `witness` is the read-back
/// alone: every armed child also checks `Seccomp: 2` before it probes.
#[test]
fn the_fixed_allowlist_admits_what_the_vmm_does_after_arming() {
    for what in ["witness", "getpid", "recv", "send", "grow", "thread"] {
        probe(what, Expect::Allowed);
    }
}

/// **The deaths the live fleet died**, each probed under the pre-fix list the port
/// shipped in `6ce364b` so the kill is shown. The fixed-allowlist test above admits the
/// same calls.
///
/// * `recv`: the muxer's `read_local_stream_port` reads a host-initiated `connect <port>`
///   command with std's `UnixStream` read, which is glibc's `recv` → `recvfrom`. The
///   armed `apz_vm0_vsock` thread was killed by `SIGSYS`, taking the node daemon with it.
/// * `send`: `send_bytes_raw`'s write is glibc's `send` → `sendto`.
/// * `grow`: a buffer growing past 128 KiB is reallocated with `mremap` (syscall 25); the
///   VMM's serial capture is such a buffer. The armed `apz_vcpu_0` thread died, and the
///   acceptance matrix saw a run lose both its VMs (`sig=31 code=0x80000000`).
/// * `thread`: a thread created after arming runs its bootstrap under the filter. The
///   pre-fix list names neither `mprotect`, `sched_getaffinity` nor `prctl`, so the new
///   thread's startup kills the process, where the live fleet's vCPU thread died.
#[test]
fn the_pre_fix_allowlist_killed_the_fleet_paths() {
    for what in ["recv", "send", "grow", "thread"] {
        probe_expect(what, "pre-fix", &[], Expect::Killed);
    }
}

/// The logged arm (`SECCOMP_RET_LOG`) audits a call the allowlist does not name and
/// **allows** it, so a child the kill arm kills lives. Two cases: `getcwd` under the
/// shipped list (the kill arm's refusal of it is checked above), and the muxer's `recv`
/// under the *pre-fix* list, the field-diagnosis arm. A deployment that has not yet found
/// its missing entry runs the pre-fix list with LOG, so the daemon survives while the
/// operator reads the entry from the kernel log (`journalctl -k | grep type=1326` shows
/// `code=0x7ffc0000`; the audit record itself is a privileged reader's).
#[test]
fn the_logged_arm_admits_the_calls_the_kill_arm_refuses() {
    probe_expect("getcwd", "logged", &[], Expect::Allowed);
    probe_expect("recv", "pre-fix-logged", &[], Expect::Allowed);
}

/// An allowlist the port cannot resolve is refused, not silently dropped from
/// the filter it is building — `Jail::arm_named` fails closed on a name it does
/// not know, the same way the environment refuses a mode it does not know.
#[test]
fn an_unknown_syscall_name_is_refused_rather_than_dropped() {
    let Some(status) = probe_code("arm", "bad-name", &[]) else {
        return;
    };
    assert_eq!(
        status.code(),
        Some(REFUSED),
        "an unknown syscall name must refuse to arm: {status:?}"
    );
}

/// The mode switch, end to end: the child's *process* mode is what its
/// environment says, and a value that names no mode is refused rather than
/// defaulted. `APZ_VMM_JAIL` is the switch; `APZ_VMM_NO_JAIL` is kept working.
#[test]
fn the_mode_switch_reaches_the_child_and_a_typo_is_refused() {
    for (value, want) in [
        ("kill", MODE_KILL),
        ("log", MODE_LOG),
        ("off", MODE_OFF),
        (" LOG ", MODE_LOG),
        ("loud", MODE_INVALID),
        ("", MODE_INVALID),
    ] {
        let Some(status) = probe_code("mode", "shipped", &[("APZ_VMM_JAIL", value)]) else {
            return;
        };
        assert_eq!(
            status.code(),
            Some(want),
            "APZ_VMM_JAIL={value:?} must reach the child as {want}, got {status:?}"
        );
    }

    // The old name still turns the jail off, and its false spellings do not.
    let Some(status) = probe_code("mode", "shipped", &[("APZ_VMM_NO_JAIL", "1")]) else {
        return;
    };
    assert_eq!(status.code(), Some(MODE_OFF));
    let Some(status) = probe_code("mode", "shipped", &[("APZ_VMM_NO_JAIL", "0")]) else {
        return;
    };
    assert_eq!(
        status.code(),
        Some(MODE_KILL),
        "APZ_VMM_NO_JAIL=0 leaves the jail armed"
    );

    // An invalid APZ_VMM_JAIL is refused *and nothing is armed*: the child
    // reports REFUSED from `Jail::arm`, not a mode it guessed from the typo.
    let Some(status) = probe_code("arm", "shipped", &[("APZ_VMM_JAIL", "loud")]) else {
        return;
    };
    assert_eq!(
        status.code(),
        Some(REFUSED),
        "a misconfigured jail must refuse to arm: {status:?}"
    );
}

/// This thread's seccomp mode, read back from the kernel — the observable the
/// probes are judged by.
///
/// `/proc/thread-self/status` is *this* thread's status, which is what a
/// per-thread filter changes. It is read with a plain `read(2)` on a fixed
/// buffer: `read_to_string` would `fstat(2)` the file first, and `fstat` is not
/// on the allowlist (the VMM never calls it after arming). `None` when the file
/// cannot be read or carries no `Seccomp:` line, which the caller treats as
/// "not installed".
fn seccomp_mode() -> Option<u32> {
    let mut buf = [0u8; 4096];
    let n = std::fs::File::open("/proc/thread-self/status")
        .and_then(|mut file| file.read(&mut buf))
        .unwrap_or(0);
    String::from_utf8_lossy(&buf[..n])
        .lines()
        .find_map(|line| line.strip_prefix("Seccomp:")?.trim().parse().ok())
}

/// [`seccomp_mode`] as the child prints it: `2`, `0`, or `absent`.
fn seccomp_report() -> String {
    seccomp_mode().map_or_else(|| "absent".to_string(), |mode| mode.to_string())
}

/// The probe child: what [`probe_child`] re-executes this binary to run. It is
/// `#[ignore]`d so an ordinary `cargo test` never runs it — arming the filter
/// here and then making a refused syscall would kill the test process.
#[test]
#[ignore = "re-executed by the probes above; it arms the jail and is killed"]
fn the_probe_child() {
    let Ok(what) = std::env::var(PROBE_ENV) else {
        // Run without the variable (an operator asked for ignored tests): a
        // no-op, since there is no action to take.
        return;
    };

    // "mode" reports what mode the *process* resolves to, without arming — so
    // the parent asserts the environment wiring, not this crate's parser.
    if what == "mode" {
        std::process::exit(match vmm::jail::mode() {
            Ok(Mode::Kill) => MODE_KILL,
            Ok(Mode::Log) => MODE_LOG,
            Ok(Mode::Off) => MODE_OFF,
            Err(_) => MODE_INVALID,
        });
    }

    // Everything a probe needs that must not be a syscall *after* arming is
    // built here: a `UnixStream::pair` is `socketpair(2)`, which no version of
    // the allowlist names (the muxer's socket is made before its thread arms,
    // which is what this mirrors), and the muxer sets its streams non-blocking
    // for the same reason a probe must: a read with no data must return, not
    // park the thread.
    let mut pair = if what == "recv" || what == "send" {
        let (mine, peer) = UnixStream::pair().expect("a socket pair before arming");
        mine.set_nonblocking(true).expect("non-blocking");
        Some((mine, peer))
    } else {
        None
    };

    let arm = std::env::var(ARM_ENV).unwrap_or_else(|_| "shipped".to_string());
    let names = pre_fix_allowlist();
    let outcome = match arm.as_str() {
        "pre-fix" => Jail::arm_named(&names, OnMiss::Kill),
        // The field-diagnosis arm over the *pre-fix* list: the deployment that
        // is being killed by a missing entry and has not found it yet.
        "pre-fix-logged" => Jail::arm_named(&names, OnMiss::Log),
        "logged" => Jail::arm_logged(),
        // An allowlist the port cannot resolve: refused, never silently
        // dropped from the filter it is building.
        "bad-name" => Jail::arm_named(&["no_such_syscall"], OnMiss::Kill),
        // The enforcement canary: an *empty* allowlist, whose terminal rule
        // kills the very next syscall. It answers "does this kernel apply a
        // filter at all?" without depending on the allowlist under test.
        "canary" => Jail::arm_named(&[], OnMiss::Kill),
        _ => Jail::arm(),
    };
    match outcome {
        Ok(()) => {}
        // No filter can exist on this platform: print the state the kernel
        // gives us and let the parent skip, rather than invent a verdict.
        Err(JailError::Unsupported(err)) => {
            note(format!(
                "vmm-jail-probe: {what} arm=refused (PR_SET_SECCOMP: {err}); \
                 /proc/thread-self/status says Seccomp: {} — nothing is enforced \
                 (a user-mode emulator such as qemu-user answers EINVAL)",
                seccomp_report()
            ));
            std::process::exit(SKIP);
        }
        // A misconfigured jail is refused: nothing was armed, and the child
        // says so rather than running under a guessed mode.
        Err(err) => {
            note(format!(
                "vmm-jail-probe: {what} arm=refused-bad-config: {err}"
            ));
            std::process::exit(REFUSED);
        }
    }

    // The arm was accepted. Read back what the kernel did with it *before* the
    // probe: a filter the kernel accepted but that left this thread unfiltered
    // is a real arming regression, and the parent fails on it — never skips.
    if seccomp_mode() != Some(2) {
        note(format!(
            "vmm-jail-probe: {what} arm=ok but /proc/thread-self/status says Seccomp: {} — \
             the filter is not in effect",
            seccomp_report()
        ));
        std::process::exit(NOT_IN_EFFECT);
    }

    match what.as_str() {
        "recv" => {
            let (mut mine, _peer) = pair.take().expect("the pair");
            // The muxer's `read_local_stream_port` shape, byte for byte: a
            // `read_exact` of the ten bytes `b"connect 0\n"` needs, which is
            // glibc's `recv` → `recvfrom` per call. The peer sends nothing, so
            // the first one returns `WouldBlock` on the non-blocking stream.
            let mut buf = [0u8; 10];
            let _ = mine.read_exact(&mut buf);
        }
        "send" => {
            let (mut mine, _peer) = pair.take().expect("the pair");
            // `send_bytes_raw`'s shape: a `Write` on a UnixStream, which is
            // glibc's `send` → `sendto`.
            let _ = mine.write_all(b"connect 5000\n");
        }
        "thread" => {
            // A thread spawned by an armed thread inherits the filter, so its
            // whole bootstrap runs under it. The name is what makes the
            // `prctl(PR_SET_NAME)` part of that bootstrap.
            let handle = std::thread::Builder::new()
                .name("apz_probe_child".to_string())
                .spawn(|| 42)
                .expect("spawning inside an armed thread");
            let answer = handle.join().expect("the thread must run to completion");
            assert_eq!(answer, 42);
        }
        "getcwd" => {
            // getcwd(2): not on the allowlist → SIGSYS before this returns
            // (or an audit line and a return, under the logged arm).
            let _ = std::env::current_dir();
        }
        "inet" => {
            // socket(AF_INET, ..): allowlisted only for AF_UNIX → SIGSYS.
            let _ = std::net::TcpStream::connect("127.0.0.1:9");
        }
        "getpid" => {
            // getpid(2): allowlisted → this returns.
            let _ = std::process::id();
        }
        "grow" => {
            // A buffer that grows past glibc's 128 KiB mmap threshold — the
            // shape of the VMM's serial capture, which starts small and keeps
            // the guest's console. Past the threshold `realloc` remaps
            // (`mremap`) instead of copying, so the armed thread that feeds the
            // capture dies on an allowlist that does not name it. Eight-KiB
            // steps so the growth passes through the boundary the way the
            // capture does.
            let mut buf: Vec<u8> = Vec::with_capacity(4 * 1024);
            for _ in 0..64 {
                buf.extend(std::iter::repeat_n(0xa5u8, 8 * 1024));
            }
            assert_eq!(buf.len(), 512 * 1024);
        }
        "witness" | "canary" | "arm" => {
            // Nothing else to probe: the read-back above already asserted the
            // kernel reports this thread in filter mode (`Seccomp: 2`). For
            // `canary` this arm is only reached where the *empty* allowlist did
            // not kill the child — the platform does not apply a filter — which
            // the parent reads from the absence of a signal.
        }
        _ => exit_jailed(72),
    }
    // The probe syscall was admitted: report the state the parent prints when it
    // explains a skip, and leave the status it reads as "not refused".
    note(format!(
        "vmm-jail-probe: {what} arm={arm} Seccomp: 2 — the probe syscall was admitted"
    ));
    exit_jailed(0);
}

/// Ends the probe from inside the jail without the process's exit handlers:
/// under coverage one of them writes the profile, with file syscalls the
/// allowlist does not name, and the jail would kill a probe it had admitted.
fn exit_jailed(code: i32) -> ! {
    // SAFETY: `_exit` takes no pointers and does not return; stderr is
    // unbuffered, so the notes above are already written.
    unsafe { libc::_exit(code) }
}
