// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// The node on Apple Silicon, end to end: a run of one VM of the arm64 guest, started through the
// registry, which boots it in its own `apz-vmm` process (Hypervisor.framework runs one VM per
// process, so the node's spawner is in process mode there). The guest's first heartbeat on the serial
// console is the proof that it booted.
//
// The VM's process needs the hypervisor entitlement, so the test runs under the runner, with the
// `apz-vmm` binary built by the same cargo command (`cargo test -p vmm -p noded` builds it):
//
//     CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="$PWD/scripts/hvf-run.sh" cargo test -p vmm -p noded
//
// The test binary itself creates no VM, so it does not need the entitlement; it signs `apz-vmm` with
// `scripts/hvf-run.sh --sign` before the node starts it. Without the runner, or without the guest in
// `target/guest/aarch64`, the test says why on stderr and passes without running anything.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use noded::events::EventBus;
use noded::registry::{CreateRun, Detail, Registry, VmImages};
use vmm::spawner::SpawnMode;

/// How long the guest may take to print its first heartbeat, on a loaded machine.
const BOOT_LIMIT: Duration = Duration::from_secs(120);

/// The test's watchdog: a VM whose guest never beats must not hold the machine.
const WATCHDOG: Duration = Duration::from_secs(300);

/// The serial line the guest's init prints after its first heartbeat.
const FIRST_HEARTBEAT: &str = "plaza guest alive: heartbeat 1";

/// Writes `message` to stderr, where it shows even on a passing test.
fn note(message: impl std::fmt::Display) {
    let _ = writeln!(std::io::stderr(), "{message}");
}

/// Ends the test binary when the test is still running after `limit`.
struct Watchdog {
    _cancel: std::sync::mpsc::Sender<()>,
}

impl Watchdog {
    fn new(limit: Duration) -> Self {
        let (cancel, waiting) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = waiting.recv_timeout(limit) {
                note(format_args!(
                    "watchdog: the node's VM test is still running after {limit:?}; ending the test binary"
                ));
                std::process::exit(124);
            }
        });
        Self { _cancel: cancel }
    }
}

/// Runs `scripts/hvf-run.sh --sign` on `binary`: the VM's process is started by the node, not by
/// cargo's runner, so it has to carry the entitlement already.
fn sign(binary: &Path) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/hvf-run.sh");
    let status = Command::new(&script)
        .arg("--sign")
        .arg(binary)
        .status()
        .expect("scripts/hvf-run.sh runs");
    assert!(
        status.success(),
        "the apz-vmm binary is signed with the hypervisor entitlement"
    );
}

/// A scratch directory with a short path: a Unix socket's path has a length limit on macOS.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from("/tmp").join(format!("apz-noded-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

#[test]
fn a_node_boots_a_vm_of_the_guest_in_its_own_process_and_the_guest_beats() {
    if apz_os::runner::runner().is_empty() {
        note(
            "skipped: the node's VM test needs the hypervisor entitlement; run it with \
             CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER=$PWD/scripts/hvf-run.sh cargo test -p vmm -p noded",
        );
        return;
    }
    let guest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/guest/aarch64");
    let missing: Vec<&str> = ["vmlinux", "initrd.gz"]
        .into_iter()
        .filter(|file| !guest.join(file).is_file())
        .collect();
    if !missing.is_empty() {
        note(format_args!(
            "skipped: the arm64 guest is not built ({} missing in {}); build it with \
             scripts/mk-guest.sh aarch64",
            missing.join(", "),
            guest.display()
        ));
        return;
    }
    // `apz-vmm` sits beside `deps/`, where cargo puts the test binary's own siblings.
    let child = std::env::current_exe()
        .expect("the test binary's path")
        .parent()
        .and_then(Path::parent)
        .expect("the test binary is under target/debug/deps")
        .join("apz-vmm");
    if !child.is_file() {
        note(format_args!(
            "skipped: {} is not built; cargo test -p vmm -p noded builds it",
            child.display()
        ));
        return;
    }
    sign(&child);
    vmm::spawner::set_child_binary(&child);
    vmm::spawner::set_spawn_mode(SpawnMode::Process);
    let _watchdog = Watchdog::new(WATCHDOG);

    let dir = scratch("hvf-node");
    vmm::spawner::create_uds_dir(&dir.join("uds")).expect("the socket directory");
    let registry = Registry::new(
        VmImages {
            kernel: guest.join("vmlinux"),
            initrd: guest.join("initrd.gz"),
            workspace: None,
        },
        dir.join("uds"),
        EventBus::new(),
    );
    let request: CreateRun = serde_json::from_value(serde_json::json!({
        "n": 1,
        "vcpus": 1,
        "memory_mib": 256,
    }))
    .expect("a create request");
    let run = registry
        .create_run(&request)
        .expect("the node starts the run");

    let deadline = Instant::now() + BOOT_LIMIT;
    let (beat, serial) = loop {
        let view = run.view(Detail::Full);
        let serial = view
            .vms
            .first()
            .and_then(|vm| vm.serial.clone())
            .unwrap_or_default();
        if serial.contains(FIRST_HEARTBEAT) {
            break (true, serial);
        }
        if Instant::now() >= deadline {
            break (false, serial);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    run.kill();
    let _ = fs::remove_dir_all(&dir);
    assert!(
        beat,
        "the guest did not print its first heartbeat; serial tail: {:?}",
        serial.get(serial.len().saturating_sub(2000)..)
    );
}
