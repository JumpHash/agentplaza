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
// Safe Rust only: nothing here needs an `unsafe` block.
#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
#[path = "apz-vmm/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    linux::main()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("apz-vmm: the per-VM process is Linux-only; there is no VM to boot here");
    std::process::ExitCode::from(2)
}
