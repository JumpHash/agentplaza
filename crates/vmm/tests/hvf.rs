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
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use common::{Watchdog, note};
use vmm::spawner::{SnapshotKind, VmExit, VmHandle, VmSpec, VsockConfig, spawn};

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
    let Some(guest) = guest_files() else {
        return;
    };
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

/// The arm64 guest in `target/guest/aarch64`, or `None` after saying which of its files is missing.
fn guest_files() -> Option<PathBuf> {
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
        return None;
    }
    Some(guest)
}

/// Whether `tool` runs from PATH.
fn on_path(tool: &str) -> bool {
    Command::new(tool)
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Makes a sparse ext4 image of `mib` MiB with the host's e2fsprogs: `mkfs.ext4`, or `mke2fs -t ext4`
/// where only that name exists (the Homebrew and Android build of e2fsprogs ships it so). `None`
/// when neither runs, and the caller says so.
fn workspace_image(dir: &Path, mib: u64) -> Option<PathBuf> {
    let (tool, extra): (&str, &[&str]) = if on_path("mkfs.ext4") {
        ("mkfs.ext4", &[])
    } else if on_path("mke2fs") {
        ("mke2fs", &["-t", "ext4"])
    } else {
        return None;
    };
    let image = dir.join("workspace.ext4");
    fs::File::create(&image)
        .and_then(|file| file.set_len(mib << 20))
        .expect("the workspace image is created");
    let status = Command::new(tool)
        .args(["-q", "-F"])
        .args(extra)
        .arg(&image)
        .status()
        .expect("the e2fsprogs tool runs");
    assert!(status.success(), "{tool} made the workspace image");
    Some(image)
}

/// Accepts the guest's connection on `listener`, or `None` when none comes within `limit`.
fn accept_within(listener: &UnixListener, limit: Duration) -> Option<UnixStream> {
    listener
        .set_nonblocking(true)
        .expect("the listener is nonblocking");
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .expect("the connection blocks");
                return Some(stream);
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(err) => panic!("accept on the vsock listener failed: {err}"),
        }
    }
    None
}

#[test]
fn the_guest_reaches_a_host_listener_over_vsock_and_writes_its_workspace() {
    if !entitled() {
        return;
    }
    let Some(guest) = guest_files() else {
        return;
    };
    let _one = one_vm();
    let _watchdog = Watchdog::new("the guest's vsock and workspace", GUEST_WATCHDOG);
    let dir = scratch("vsock-workspace");
    let Some(workspace) = workspace_image(&dir, 64) else {
        note("skipped: no mkfs.ext4 or mke2fs on PATH to make the workspace image");
        return;
    };

    // The guest's connect to CID 2, port 5000, reaches `{uds_path}_5000`: the test is the host
    // listener there, the orchestrator the harness says hello to.
    let listener = UnixListener::bind(dir.join("vm3_5000")).expect("the host listener binds");
    let mut vm = spawn(VmSpec {
        kernel: guest.join("vmlinux"),
        initrd: Some(guest.join("initrd.gz")),
        cmdline: "console=ttyS0 panic=-1 plaza.keepalive=1 plaza.vm=0".to_string(),
        memory_mib: 256,
        vsock: Some(VsockConfig {
            guest_cid: 3,
            uds_path: dir.join("vm3"),
        }),
        workspace: Some(workspace.clone()),
        ..VmSpec::default()
    })
    .expect("the guest boots with its vsock and workspace devices");
    let Some(mut stream) = accept_within(&listener, GUEST_LIMIT) else {
        let _ = vm.kill();
        panic!(
            "the guest did not connect to the host listener; serial tail: {:?}",
            serial_tail(&vm.serial())
        );
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("the read timeout is set");
    let mut lines = BufReader::new(stream.try_clone().expect("the stream clones"));
    let mut hello = String::new();
    lines.read_line(&mut hello).expect("the guest says hello");
    assert!(
        hello.contains(r#""type":"hello""#) && hello.contains(r#""guest_cid":3"#),
        "the harness says hello with its CID: {hello:?}"
    );
    stream
        .write_all(b"{\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\",\"keepalive\":true}\n")
        .expect("the welcome is sent");
    let mut heartbeat = String::new();
    lines
        .read_line(&mut heartbeat)
        .expect("the guest keeps its session: a heartbeat arrives");
    assert!(
        heartbeat.contains(r#""type":"heartbeat""#),
        "a heartbeat over vsock: {heartbeat:?}"
    );

    // The workspace: the guest writes a line to the volume, prints it back, and syncs it to the image
    // before the guest powers off.
    vm.serial_input(b"wspace hello-from-hvf\n")
        .expect("the console takes the command");
    assert!(
        wait_for_serial(&vm, "hello-from-hvf", GUEST_LIMIT),
        "the workspace answers; serial tail: {:?}",
        serial_tail(&vm.serial())
    );
    vm.serial_input(b"sync\npoweroff\n")
        .expect("the console takes the power-off");
    let exit = vm.wait().expect("the guest powers off");
    assert_eq!(
        exit,
        VmExit::GuestExited { reset: false },
        "the guest powers off through PSCI SYSTEM_OFF; serial tail: {:?}",
        serial_tail(&vm.serial())
    );

    // The bytes are in the image: the volume is a file the host reads after the VM is gone.
    let image = fs::read(&workspace).expect("the workspace image is read");
    assert!(
        image
            .windows(b"hello-from-hvf".len())
            .any(|window| window == b"hello-from-hvf"),
        "the line the guest wrote is in the workspace image"
    );
    let _ = fs::remove_dir_all(&dir);
}
