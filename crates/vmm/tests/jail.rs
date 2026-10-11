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
// A platform that cannot install a filter at all (qemu-user answers
// `PR_SET_SECCOMP` with `EINVAL`) makes the probes report nothing and pass,
// the way the crate's other capability probes do — never a false "the jail
// refused it".
#![cfg(target_os = "linux")]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};

use vmm::jail::{Jail, JailError, Mode, OnMiss};

/// The environment variable the child reads to learn which syscall to make.
const PROBE_ENV: &str = "VMM_JAIL_PROBE";

/// The environment variable the child reads to learn which filter to arm.
const ARM_ENV: &str = "VMM_JAIL_PROBE_ARM";

/// The (ignored) test the probes re-execute as a child.
const PROBE_TEST: &str = "the_probe_child";

/// The child's exit status when this kernel cannot install a filter at all.
const SKIP: i32 = 77;

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

/// Re-executes this test binary as a probe child and returns how it ended, or
/// `None` when this platform cannot install a filter (the probe reports
/// nothing rather than a false verdict).
///
/// `arm` selects the child's filter (`"shipped"`, `"pre-fix"` or `"logged"`)
/// and `env` adds variables to the child's environment — which is how the mode
/// switch is probed.
///
/// The wait is bounded: a probe child that blocks on the socket it is probing
/// (which is what a read on a stream whose peer sends nothing does) fails the
/// test with the timeout rather than wedging the suite.
fn probe_with(what: &str, arm: &str, env: &[(&str, &str)]) -> Option<ExitStatus> {
    let exe = std::env::current_exe().expect("the test binary's path");
    let mut command = Command::new(exe);
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
        .stderr(Stdio::null())
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
    if status.code() == Some(SKIP) {
        eprintln!("soft-skip: this kernel cannot install a seccomp filter");
        return None;
    }
    Some(status)
}

/// [`probe_with`] with the shipped allowlist and no extra environment.
fn probe(what: &str) -> Option<ExitStatus> {
    probe_with(what, "shipped", &[])
}

/// A syscall the allowlist does not name is refused: the child dies by
/// `SIGSYS`. This is the red test: with the filter not installed (or with the
/// deny swapped for an allow) the child exits 0 and this fails.
#[test]
fn a_syscall_outside_the_allowlist_is_killed() {
    let Some(status) = probe("getcwd") else {
        return;
    };
    assert_eq!(
        status.signal(),
        Some(libc::SIGSYS),
        "getcwd(2) is not on the allowlist and must be killed"
    );
}

/// `socket(2)` is allowlisted, but only for `AF_UNIX`: an `AF_INET` socket is
/// refused by the argument filter, not the number filter.
#[test]
fn an_inet_socket_is_killed() {
    let Some(status) = probe("inet") else {
        return;
    };
    assert_eq!(
        status.signal(),
        Some(libc::SIGSYS),
        "socket(AF_INET) must be killed even though socket(2) is allowlisted"
    );
}

/// The filter is not a blanket deny: an allowlisted syscall runs and the child
/// exits cleanly.
#[test]
fn an_allowlisted_syscall_runs() {
    let Some(status) = probe("getpid") else {
        return;
    };
    assert_eq!(
        status.code(),
        Some(0),
        "getpid(2) is allowlisted: {status:?}"
    );
}

/// The filter really is installed: after arming, the kernel reports the
/// calling thread in filter mode (`Seccomp: 2`).
#[test]
fn the_jail_installs_seccomp_filter_mode() {
    let Some(status) = probe("witness") else {
        return;
    };
    assert_eq!(
        status.code(),
        Some(0),
        "the child should observe Seccomp: 2 after arming: {status:?}"
    );
}

/// **The death the live fleet died.** The muxer's `read_local_stream_port`
/// reads a host-initiated connection's `connect <port>` command with std's
/// `UnixStream` read — the C library's `recv`, which is the `recvfrom` syscall
/// on x86_64 — and the allowlist this port shipped in `6ce364b` did not name
/// it: the armed `apz_vm0_vsock` thread was killed by `SIGSYS`, taking the
/// whole node daemon with it. The pre-fix list is armed deliberately here to
/// show the kill, and the next test shows the fix.
#[test]
fn the_pre_fix_allowlist_killed_the_muxers_read() {
    let Some(status) = probe_with("recv", "pre-fix", &[]) else {
        return;
    };
    assert_eq!(
        status.signal(),
        Some(libc::SIGSYS),
        "a UnixStream read (recvfrom) must be killed by the pre-fix allowlist: {status:?}"
    );
}

/// The same read under the shipped allowlist: the syscall is named, so the
/// thread runs. This is the green half of the pair above.
#[test]
fn the_muxers_read_runs_under_the_fixed_allowlist() {
    let Some(status) = probe_with("recv", "shipped", &[]) else {
        return;
    };
    assert_eq!(
        status.code(),
        Some(0),
        "the muxer's read must be admitted now: {status:?}"
    );
}

/// The write half of the same stream: `send_bytes_raw`'s std `UnixStream`
/// write is the C library's `send`, which is `sendto` on x86_64.
#[test]
fn the_pre_fix_allowlist_killed_the_muxers_write() {
    let Some(status) = probe_with("send", "pre-fix", &[]) else {
        return;
    };
    assert_eq!(
        status.signal(),
        Some(libc::SIGSYS),
        "a UnixStream write (sendto) must be killed by the pre-fix allowlist: {status:?}"
    );
}

/// The same write under the shipped allowlist.
#[test]
fn the_muxers_write_runs_under_the_fixed_allowlist() {
    let Some(status) = probe_with("send", "shipped", &[]) else {
        return;
    };
    assert_eq!(
        status.code(),
        Some(0),
        "the muxer's write must be admitted now: {status:?}"
    );
}

/// **The whole bootstrap of a thread created by an armed thread** — where the
/// allocator's `mprotect` is, and where the live fleet's vCPU thread was killed
/// (the first allocation a thread makes can create a glibc arena, long after it
/// armed). The pre-fix list names neither `mprotect` nor `sched_getaffinity`
/// nor `prctl`, so the new thread's own startup kills the process.
#[test]
fn the_pre_fix_allowlist_killed_a_thread_spawned_after_arming() {
    let Some(status) = probe_with("thread", "pre-fix", &[]) else {
        return;
    };
    assert_eq!(
        status.signal(),
        Some(libc::SIGSYS),
        "a thread's bootstrap under the pre-fix allowlist must be killed: {status:?}"
    );
}

/// The same thread under the shipped allowlist: `clone`, the libc bootstrap
/// (`sched_getaffinity`, `set_robust_list`, `rseq`, `sigaltstack`), the
/// allocator's `mprotect`, Rust's `prctl(PR_SET_NAME)` and the join all run.
#[test]
fn a_thread_spawned_after_arming_survives_the_fixed_allowlist() {
    let Some(status) = probe_with("thread", "shipped", &[]) else {
        return;
    };
    assert_eq!(
        status.code(),
        Some(0),
        "a thread created by an armed thread must survive its bootstrap: {status:?}"
    );
}

/// The logged arm: `SECCOMP_RET_LOG` audits a syscall the allowlist does not
/// name and **allows** it, so the child that would be killed under the shipped
/// `kill` arm lives. (The kernel's audit record itself is a privileged
/// reader's: `journalctl -k | grep type=1326` shows `code=0x7ffc0000` for it.)
#[test]
fn the_logged_arm_audits_an_unlisted_call_and_the_child_lives() {
    // What the kill arm does to the same call, for contrast.
    let Some(killed) = probe_with("getcwd", "shipped", &[]) else {
        return;
    };
    assert_eq!(killed.signal(), Some(libc::SIGSYS));

    let Some(kept) = probe_with("getcwd", "logged", &[]) else {
        return;
    };
    assert_eq!(
        kept.code(),
        Some(0),
        "the logged arm must let an unlisted syscall through: {kept:?}"
    );
}

/// **The field-diagnosis arm, over the list that was killing the fleet.** A
/// deployment that has not yet found its missing entry can run the *pre-fix*
/// allowlist with `SECCOMP_RET_LOG`: the muxer's `recvfrom` — the syscall that
/// killed the live `noded` — is audited and allowed, so the daemon lives while
/// the operator reads the entry out of the kernel log.
#[test]
fn the_logged_arm_keeps_a_daemon_alive_on_the_pre_fix_allowlist() {
    let Some(killed) = probe_with("recv", "pre-fix", &[]) else {
        return;
    };
    assert_eq!(killed.signal(), Some(libc::SIGSYS));
    let Some(kept) = probe_with("recv", "pre-fix-logged", &[]) else {
        return;
    };
    assert_eq!(
        kept.code(),
        Some(0),
        "the logged arm must let the pre-fix list's missing recvfrom through: {kept:?}"
    );
}

/// An allowlist the port cannot resolve is refused, not silently dropped from
/// the filter it is building — `Jail::arm_named` fails closed on a name it does
/// not know, the same way the environment refuses a mode it does not know.
#[test]
fn an_unknown_syscall_name_is_refused_rather_than_dropped() {
    let Some(status) = probe_with("arm", "bad-name", &[]) else {
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
        let Some(status) = probe_with("mode", "shipped", &[("APZ_VMM_JAIL", value)]) else {
            return;
        };
        assert_eq!(
            status.code(),
            Some(want),
            "APZ_VMM_JAIL={value:?} must reach the child as {want}, got {status:?}"
        );
    }

    // The old name still turns the jail off, and its false spellings do not.
    let Some(status) = probe_with("mode", "shipped", &[("APZ_VMM_NO_JAIL", "1")]) else {
        return;
    };
    assert_eq!(status.code(), Some(MODE_OFF));
    let Some(status) = probe_with("mode", "shipped", &[("APZ_VMM_NO_JAIL", "0")]) else {
        return;
    };
    assert_eq!(
        status.code(),
        Some(MODE_KILL),
        "APZ_VMM_NO_JAIL=0 leaves the jail armed"
    );

    // An invalid APZ_VMM_JAIL is refused *and nothing is armed*: the child
    // reports REFUSED from `Jail::arm`, not a mode it guessed from the typo.
    let Some(status) = probe_with("arm", "shipped", &[("APZ_VMM_JAIL", "loud")]) else {
        return;
    };
    assert_eq!(
        status.code(),
        Some(REFUSED),
        "a misconfigured jail must refuse to arm: {status:?}"
    );
}

/// The probe child: what [`probe_with`] re-executes this binary to run. It is
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
        _ => Jail::arm(),
    };
    match outcome {
        Ok(()) => {}
        // No filter can exist on this kernel: tell the parent to report
        // nothing rather than a false verdict.
        Err(JailError::Unsupported(_)) => std::process::exit(SKIP),
        // A misconfigured jail is refused: nothing was armed, and the child
        // says so rather than running under a guessed mode.
        Err(err) => {
            eprintln!("the probe child refused to arm: {err}");
            std::process::exit(REFUSED);
        }
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
        "witness" => {
            // Installed? `/proc/thread-self/status` is *this* thread's status,
            // which is what a per-thread filter changes. Read it with a plain
            // `read(2)` on a fixed buffer: `read_to_string` would stat the file
            // first (`fstat(2)`), which the VMM never calls after arming and so
            // is not on the allowlist.
            let mut buf = [0u8; 4096];
            let n = std::fs::File::open("/proc/thread-self/status")
                .and_then(|mut file| file.read(&mut buf))
                .unwrap_or(0);
            let status = String::from_utf8_lossy(&buf[..n]);
            std::process::exit(if status.contains("Seccomp:\t2") {
                0
            } else {
                71
            });
        }
        "arm" => {
            // Armed (or refused) above and nothing else to do: reaching this
            // means the arm succeeded.
        }
        _ => std::process::exit(72),
    }
    // The syscall was not refused: leave a status the parent reports as
    // "the jail did not refuse it".
    std::process::exit(0);
}
