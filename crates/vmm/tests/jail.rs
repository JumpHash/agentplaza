// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// The VMM jail's probes, as an integration test — its own process, separate
// from the unit-test binary. Two reasons:
//
//  * the filter's refusal is `SECCOMP_RET_KILL_PROCESS`, so a probe that works
//    kills the process it runs in; and
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
//    this thread's filter (`Seccomp: 2`).
//
// A platform that cannot install a filter at all (qemu-user answers
// `PR_SET_SECCOMP` with `EINVAL`) makes the probes report nothing and pass,
// the way the crate's other capability probes do — never a false "the jail
// refused it".
#![cfg(target_os = "linux")]

use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};

/// The environment variable the child reads to learn which syscall to make.
const PROBE_ENV: &str = "VMM_JAIL_PROBE";

/// The (ignored) test the probes re-execute as a child.
const PROBE_TEST: &str = "the_probe_child";

/// The child's exit status when this kernel cannot install a filter at all.
const SKIP: i32 = 77;

/// Re-executes this test binary as a probe child and returns how it ended, or
/// `None` when this platform cannot install a filter (the probe reports
/// nothing rather than a false verdict).
fn probe(what: &str) -> Option<ExitStatus> {
    let exe = std::env::current_exe().expect("the test binary's path");
    let status = Command::new(exe)
        .args([
            "--ignored",
            "--exact",
            PROBE_TEST,
            "--test-threads=1",
            "--nocapture",
        ])
        .env(PROBE_ENV, what)
        // The probe must arm the jail, whatever the surrounding run's setting.
        .env_remove("APZ_VMM_NO_JAIL")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("spawning the probe child");
    if status.code() == Some(SKIP) {
        eprintln!("soft-skip: this kernel cannot install a seccomp filter");
        return None;
    }
    Some(status)
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

/// The probe child: what [`probe`] re-executes this binary to run. It is
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
    match vmm::jail::Jail::arm() {
        Ok(()) => {}
        // No filter can exist on this kernel: tell the parent to report
        // nothing rather than a false verdict.
        Err(vmm::jail::JailError::Unsupported(_)) => std::process::exit(SKIP),
        Err(err) => panic!("the probe child must arm the filter: {err}"),
    }
    match what.as_str() {
        "getcwd" => {
            // getcwd(2): not on the allowlist → SIGSYS before this returns.
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
            use std::io::Read;
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
        _ => std::process::exit(72),
    }
    // The syscall was not refused: leave a status the parent reports as
    // "the jail did not refuse it".
    std::process::exit(0);
}
