// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// The per-VM process: `apz-vmm`, one microVM in one process.
//
// It exists because the jailer ([`vmm::jail::jailer`]) needs a process that is
// single-threaded when the VM is built: `unshare(CLONE_NEWUSER)` is refused by
// a process with a sibling thread, and that namespace is the only road an
// unprivileged VMM has to `CAP_SYS_ADMIN` over a mount namespace — so a
// serving daemon's in-process VMs cannot be jailed, and a process whose whole
// job is one VM can. This binary is that process; the parent that runs it and
// proxies its console, its exit and its stop is
// `vmm::spawner::process` (`SpawnMode::Process`).
//
// This file is the dispatcher: the VM boot is Linux/KVM
// (`vmm::builder::boot`), and the crate has to *build* for
// `aarch64-apple-darwin` (this port's five-gate matrix), so the body lives in
// `apz-vmm/linux.rs` behind this gate — the same shape `jailer_probe.rs` uses
// for the same reason.
//
// Safe Rust, with one stated exception. The crate's library is
// `deny(unsafe_code)` with an enumerated module allowlist (`lib.rs`); this
// binary's own entry on that discipline is `apz-vmm/linux.rs`, whose only
// `unsafe` is the `prctl(2)` that arms `PR_SET_PDEATHSIG` — a call no safe
// interface exposes, and one that has to run in this process because it is
// this process's *parent* that dies. So this root is `deny` rather than
// `forbid` and that module carries the explicit `#![allow(unsafe_code)]` and
// the `// SAFETY:` note, exactly as the library's syscall modules do.
#![deny(unsafe_code)]

#[cfg(target_os = "linux")]
#[path = "apz-vmm/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    if std::env::args().skip(1).any(|arg| arg == "--version" || arg == "-V") {
        println!("apz-vmm {}", apz_os::version());
        return std::process::ExitCode::SUCCESS;
    }
    linux::main()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    if std::env::args().skip(1).any(|arg| arg == "--version" || arg == "-V") {
        println!("apz-vmm {}", apz_os::version());
        return std::process::ExitCode::SUCCESS;
    }
    eprintln!("apz-vmm: the per-VM process is Linux-only; there is no VM to boot here");
    std::process::ExitCode::from(2)
}
