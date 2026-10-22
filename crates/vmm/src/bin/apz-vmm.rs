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
// job is one VM can. It also exists on Apple Silicon, where Hypervisor.framework
// allows one VM per process: a caller that wants several microVMs runs one of
// these per VM. This binary is that process; the parent that runs it and
// proxies its console, its exit and its stop is `vmm::spawner::process`
// (`SpawnMode::Process`).
//
// This file is the dispatcher: the body is `apz-vmm/child.rs`, which is the
// same on Linux and macOS (the VM is built by `vmm::spawner::spawn`, whose
// backend is KVM or Hypervisor.framework); only its jail and its parent-death
// watch are per platform. Windows has no `apz-vmm` VM to boot (its WHP VMs are
// in-process), so the binary there only says so.
//
// Safe Rust, with one stated exception. The crate's library is
// `deny(unsafe_code)` with an enumerated module allowlist (`lib.rs`); this
// binary's own entry on that discipline is `apz-vmm/child.rs`, whose only
// `unsafe` is the call that arms the parent-death backstop (`prctl(2)` on
// Linux, `kqueue(2)` and `kevent(2)` on macOS) — calls no safe interface
// exposes, and that have to run in this process because it is this process's
// *parent* that dies. So this root is `deny` rather than `forbid` and that
// module carries the explicit `#![allow(unsafe_code)]` and the `// SAFETY:`
// notes, exactly as the library's syscall modules do.
#![deny(unsafe_code)]

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[path = "apz-vmm/child.rs"]
mod child;

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn main() -> std::process::ExitCode {
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("apz-vmm {}", apz_os::version());
        return std::process::ExitCode::SUCCESS;
    }
    child::main()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn main() -> std::process::ExitCode {
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("apz-vmm {}", apz_os::version());
        return std::process::ExitCode::SUCCESS;
    }
    eprintln!("apz-vmm: the per-VM process runs on Linux and macOS; there is no VM to boot here");
    std::process::ExitCode::from(2)
}
