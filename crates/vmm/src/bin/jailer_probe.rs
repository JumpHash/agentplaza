// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// The jailer's probe: a *separate binary*, because the jailer has a
// precondition that no test harness can meet. `unshare(CLONE_NEWUSER)` — the
// only way an unprivileged process gets `CAP_SYS_ADMIN` for a mount namespace —
// is refused by a multi-threaded process, and `libtest` runs every test on a
// spawned thread (`Threads: 2` in a child re-executed with `--test-threads 1`,
// measured on this host). A probe of the *inside* of the jail therefore cannot
// be a `#[test]`; it has to be a plain `main`, which is single-threaded until
// it says otherwise.
//
// This file is the dispatcher: the jailer, and so the probe, exists only on
// Linux (`vmm::jail` is Linux-gated, and the crate has to *build* for
// `aarch64-apple-darwin` — this port's five-gate matrix). The probe itself is
// `jailer_probe/linux.rs`; `tests/jailer.rs` drives it and asserts its exit
// code and its one-line verdict, never this file's text.

// Safe Rust only: the jailer's three syscalls live behind the crate's
// enumerated unsafe boundary (`vmm::jail::jailer_sys`), and nothing here needs
// one.
#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
#[path = "jailer_probe/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    linux::main()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("jailer_probe: the jailer is Linux-only; there is nothing to probe here");
    std::process::ExitCode::from(2)
}
