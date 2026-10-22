// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// The Hypervisor.framework backend end to end, on an Apple Silicon Mac. A payload that needs no
// kernel is booted through `spawn`, snapshotted while it runs, and restored into a fresh VM, and
// the arm64 guest kernel is booted with its serial console captured and powered off.
//
// The VM tests need the `com.apple.security.hypervisor` entitlement, so they run under the runner
// (`scripts/hvf-run.sh`):
//
//     CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER="$PWD/scripts/hvf-run.sh" cargo test -p vmm --test hvf
//
// Without the runner they say so on stderr and pass. The guest test needs the guest built by
// `scripts/mk-guest.sh aarch64` in `target/guest/aarch64`; without it the test skips and says which
// file is missing.
//
// One VM per process is the framework's rule, so every test here takes `ONE_VM` and the tests run
// one at a time.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use common::{Watchdog, note};
use vmm::spawner::{SnapshotKind, VmExit, VmHandle, VmSpec, spawn};

/// The process's one VM at a time: the framework allows one VM per process.
static ONE_VM: Mutex<()> = Mutex::new(());

fn one_vm() -> MutexGuard<'static, ()> {
    ONE_VM.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How long a payload may take to reach its next marker. A payload runs in well under a second;
/// the limit is for a loaded machine.
const PAYLOAD_LIMIT: Duration = Duration::from_secs(60);

/// How long the guest kernel may take to print its banner, its first heartbeat, or to power off.
const GUEST_LIMIT: Duration = Duration::from_secs(180);

/// The guest test's watchdog: its three waits, one after another, and some margin.
const GUEST_WATCHDOG: Duration = Duration::from_secs(600);

/// The payload's instructions, from the start of its kernel image. The assembly, with the marks it
/// writes to the console (`A` before it waits for a byte from the host, then `B`, or `X` when the
/// sentinel did not come back):
///
/// ```text
/// movz x1, #0x2000             ; x1 = 0x4000_2000, the console
/// movk x1, #0x4000, lsl #16
/// movz x2, #0x8080, lsl #16    ; x2 = 0x8080_0000, a word of RAM
/// movz w3, #0x5a5a
/// str  w3, [x2]                ; the sentinel
/// movz w4, #0x41               ; 'A': the mark the host waits for before it snapshots
/// strb w4, [x1]
/// wait: ldrb w5, [x1, #5]      ; the console's line status
/// tbz  w5, #0, wait            ; until the host has sent a byte
/// ldrb w5, [x1]                ; take the byte
/// ldr  w6, [x2]                ; the sentinel must be there after a restore
/// movz w4, #0x42               ; 'B'
/// cmp  w6, w3
/// b.eq done
/// movz w4, #0x58               ; 'X'
/// done: strb w4, [x1]
/// movz x0, #0x8400, lsl #16    ; x0 = 0x8400_0008: PSCI SYSTEM_OFF
/// movk x0, #0x8
/// hvc  #0
/// ```
const PAYLOAD: [u32; 19] = [
    0xd284_0001,
    0xf2a8_0001,
    0xd2b0_1002,
    0x528b_4b43,
    0xb900_0043,
    0x5280_0824,
    0x3900_0024,
    0x3940_1425,
    0x3607_ffe5,
    0x3940_0025,
    0xb940_0046,
    0x5280_0844,
    0x6b03_00df,
    0x5400_0040,
    0x5280_0b04,
    0x3900_0024,
    0xd2b0_8000,
    0xf280_0100,
    0xd400_0002,
];

/// The arm64 `Image` of the payload: the 64-byte header the loader reads (`ARM\x64` at 56, the image
/// size at 16, and a branch over the header in its first word), then the code. Loaded at the
/// kernel's address with a zero `text_offset`, so the payload starts at the kernel entry.
fn payload_image() -> Vec<u8> {
    let size = 64 + PAYLOAD.len() * 4;
    let mut image = Vec::with_capacity(size);
    image.extend_from_slice(&0x1400_0010u32.to_le_bytes()); // code0: b +64, over the header
    image.extend_from_slice(&0u32.to_le_bytes()); // code1
    image.extend_from_slice(&0u64.to_le_bytes()); // text_offset
    image.extend_from_slice(&(size as u64).to_le_bytes()); // image_size
    image.extend_from_slice(&[0u8; 32]); // flags, res2, res3, res4
    image.extend_from_slice(&0x644d_5241u32.to_le_bytes()); // magic: "ARM\x64"
    image.extend_from_slice(&0u32.to_le_bytes()); // res5
    for word in PAYLOAD {
        image.extend_from_slice(&word.to_le_bytes());
    }
    image
}

/// A scratch directory with a short path: a Unix socket's path has a length limit on macOS.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from("/tmp").join(format!("apz-hvf-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// The spec that boots the payload: one vCPU and 16 MiB, which holds the payload and the device tree.
fn payload_spec(dir: &Path) -> VmSpec {
    let kernel = dir.join("payload.Image");
    fs::write(&kernel, payload_image()).expect("the payload image is written");
    VmSpec {
        kernel,
        cmdline: String::new(),
        memory_mib: 16,
        ..VmSpec::default()
    }
}

/// Whether the tests may create VMs here: only under the runner, whose entitlement the framework
/// checks. Otherwise the reason is printed and the test passes without running.
fn entitled() -> bool {
    if apz_os::runner::runner().is_empty() {
        note(
            "skipped: the HVF tests need the hypervisor entitlement; run them with \
             CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER=$PWD/scripts/hvf-run.sh cargo test -p vmm --test hvf",
        );
        return false;
    }
    true
}

/// Waits until the VM's serial capture contains `needle`; `false` when `limit` passes first.
fn wait_for_serial(vm: &VmHandle, needle: &str, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if vm.serial().contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn a_payload_is_snapshotted_mid_run_and_resumes_in_a_fresh_vm() {
    if !entitled() {
        return;
    }
    let _one = one_vm();
    let _watchdog = Watchdog::new("the snapshot payload", PAYLOAD_LIMIT);
    let dir = scratch("in-process");
    let spec = payload_spec(&dir);

    let mut first = spawn(spec.clone()).expect("the payload boots");
    assert!(
        wait_for_serial(&first, "A", PAYLOAD_LIMIT),
        "the payload marks the console before it waits; serial so far: {:?}",
        first.serial()
    );
    let snapshot = dir.join("snapshot");
    first
        .snapshot(&snapshot, SnapshotKind::Full)
        .expect("the VM is snapshotted while the payload waits for the console");
    first.kill().expect("the first VM stops");
    assert_eq!(
        first.wait().expect("the first VM has stopped"),
        VmExit::Killed,
        "the host stopped the first VM: the payload did not run on past its snapshot"
    );

    let mut restored = spawn(VmSpec {
        restore: Some(snapshot),
        ..spec
    })
    .expect("the snapshot restores into a fresh VM");
    restored
        .serial_input(b"g")
        .expect("the restored console takes the byte the payload waits for");
    let exit = restored.wait().expect("the restored VM stops");
    assert_eq!(
        exit,
        VmExit::GuestExited { reset: false },
        "the restored payload reaches PSCI SYSTEM_OFF, which its own `hvc` makes"
    );
    assert_eq!(
        restored.serial(),
        "B",
        "the restored payload resumes after its wait, finds its sentinel in the restored RAM"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_guest_kernel_boots_prints_its_heartbeat_and_powers_off_on_the_console() {
    if !entitled() {
        return;
    }
    let guest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/guest/aarch64");
    let missing: Vec<&str> = ["vmlinux", "initrd.gz", "guest.json"]
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
    let _one = one_vm();
    // The three waits below (banner, heartbeat, power-off) each have their own limit: the watchdog
    // covers all of them.
    let _watchdog = Watchdog::new("the guest boot", GUEST_WATCHDOG);

    let mut vm = spawn(VmSpec {
        kernel: guest.join("vmlinux"),
        initrd: Some(guest.join("initrd.gz")),
        cmdline: "console=ttyS0 panic=-1".to_string(),
        ..VmSpec::default()
    })
    .expect("the guest boots");
    let banner = wait_for_serial(&vm, "Linux version", GUEST_LIMIT);
    let heartbeat = banner && wait_for_serial(&vm, "plaza guest alive: heartbeat 1", GUEST_LIMIT);
    if !heartbeat {
        let _ = vm.kill();
        panic!(
            "the guest did not print {} (serial tail: {:?})",
            if banner {
                "its first heartbeat"
            } else {
                "its banner"
            },
            serial_tail(&vm.serial())
        );
    }
    vm.serial_input(b"poweroff\n")
        .expect("the console takes the poweroff command");
    let exit = vm.wait().expect("the guest powers off");
    assert_eq!(
        exit,
        VmExit::GuestExited { reset: false },
        "the guest powers off through PSCI SYSTEM_OFF; serial tail: {:?}",
        serial_tail(&vm.serial())
    );
}

/// The last 2000 bytes of a console capture, for a failure message.
fn serial_tail(serial: &str) -> &str {
    let mut start = serial.len().saturating_sub(2000);
    while !serial.is_char_boundary(start) {
        start += 1;
    }
    &serial[start..]
}

/// Restores the in-process spawn mode when dropped (the per-VM process test switches it).
struct InProcessAgain;

impl Drop for InProcessAgain {
    fn drop(&mut self) {
        vmm::spawner::set_spawn_mode(vmm::spawner::SpawnMode::InProcess);
    }
}

/// Runs `scripts/hvf-run.sh --sign` on a binary this test starts itself: `apz-vmm` is started by the
/// kernel (the per-VM process), not by cargo's runner, so it has to carry the entitlement already.
fn sign(binary: &Path) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/hvf-run.sh");
    let status = std::process::Command::new(&script)
        .arg("--sign")
        .arg(binary)
        .status()
        .expect("scripts/hvf-run.sh runs");
    assert!(
        status.success(),
        "the child binary is signed with the hypervisor entitlement"
    );
}

#[test]
fn a_payload_is_snapshotted_in_a_per_vm_process_and_restored_in_another() {
    if !entitled() {
        return;
    }
    let _one = one_vm();
    let _watchdog = Watchdog::new("the per-VM process payload", PAYLOAD_LIMIT);
    let child = Path::new(env!("CARGO_BIN_EXE_apz-vmm"));
    sign(child);
    vmm::spawner::set_child_binary(child);
    vmm::spawner::set_spawn_mode(vmm::spawner::SpawnMode::Process);
    // The mode is process-wide: whatever happens below, the next test boots in-process again.
    let _in_process_again = InProcessAgain;

    let dir = scratch("per-vm-process");
    let spec = VmSpec {
        vsock: Some(vmm::spawner::VsockConfig {
            guest_cid: 3,
            uds_path: dir.join("vm3"),
        }),
        ..payload_spec(&dir)
    };
    let mut first = spawn(spec.clone()).expect("the per-VM process boots the payload");
    assert!(
        wait_for_serial(&first, "A", PAYLOAD_LIMIT),
        "the child's console reaches the handle; serial so far: {:?}",
        first.serial()
    );
    let snapshot = dir.join("snapshot");
    first
        .snapshot(&snapshot, SnapshotKind::Full)
        .expect("the child is snapshotted through its control socket");
    first.kill().expect("the first child stops");

    let mut restored = spawn(VmSpec {
        restore: Some(snapshot),
        vsock: Some(vmm::spawner::VsockConfig {
            guest_cid: 4,
            uds_path: dir.join("vm4"),
        }),
        ..spec
    })
    .expect("a second child restores the snapshot");
    restored
        .serial_input(b"g")
        .expect("the restored child's console takes the byte");
    let exit = restored.wait().expect("the restored child stops");
    assert_eq!(
        exit,
        VmExit::GuestExited { reset: false },
        "the restored child's payload powers its guest off; serial: {:?}",
        restored.serial()
    );
    assert_eq!(
        restored.serial(),
        "B",
        "the restored payload resumes in its own process"
    );
    let _ = fs::remove_dir_all(&dir);
}
