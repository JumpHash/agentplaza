//! Snapshot and restore of a real guest, end to end.
//!
//! Skipped unless `APZ_VMM_SMOKE_KERNEL` names a bootable x86_64 kernel and
//! `APZ_VMM_CONSOLE_INITRD` the guest initramfs from `scripts/mk-initrd.sh` (its `PID 1`
//! heartbeats and serves commands on the serial console):
//!
//! ```text
//! APZ_VMM_SMOKE_KERNEL=target/guest/x86_64/vmlinux \
//! APZ_VMM_CONSOLE_INITRD=target/guest/x86_64/initrd.gz \
//!     cargo test -p vmm --test snapshot_restore -- --nocapture
//! ```
//!
//! The guest boots with a vsock device (its harness in the keepalive loop, so the driver is up) and a
//! workspace volume, answers a console command, is snapshotted and killed; a VM restored from the
//! snapshot under another CID must go on heartbeating and answer the console; a second, diff
//! snapshot of the restored VM must restore too.

#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use vmm::spawner::{SnapshotKind, VmHandle, VmSpec, VsockConfig, spawn};

fn guest() -> Option<(PathBuf, PathBuf)> {
    let kernel = env::var_os("APZ_VMM_SMOKE_KERNEL").filter(|v| !v.is_empty())?;
    let initrd = env::var_os("APZ_VMM_CONSOLE_INITRD").filter(|v| !v.is_empty())?;
    Some((PathBuf::from(kernel), PathBuf::from(initrd)))
}

/// A scratch directory with a short path (the vsock sockets are bound in it).
fn scratch(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!("apz-snap-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A 64 MiB ext4 image for the workspace volume.
fn workspace_image(dir: &Path) -> PathBuf {
    let image = dir.join("ws.ext4");
    let file = std::fs::File::create(&image).expect("image");
    file.set_len(64 << 20).expect("size");
    let status = Command::new("mkfs.ext4")
        .args(["-q", "-F"])
        .arg(&image)
        .status()
        .expect("mkfs.ext4");
    assert!(status.success(), "mkfs.ext4 failed");
    image
}

/// Waits until the VM's serial capture, from byte `from` on, contains `needle`.
fn wait_for(vm: &VmHandle, from: usize, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let serial = vm.serial();
        if serial.get(from..).is_some_and(|tail| tail.contains(needle)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn spec(kernel: &Path, initrd: &Path, cid: u64, uds: PathBuf, workspace: &Path) -> VmSpec {
    VmSpec {
        kernel: kernel.to_path_buf(),
        initrd: Some(initrd.to_path_buf()),
        cmdline: "console=ttyS0 plaza.keepalive=1 plaza.vm=0".into(),
        vcpus: 2,
        memory_mib: 256,
        vsock: Some(VsockConfig {
            guest_cid: cid,
            uds_path: uds,
        }),
        workspace: Some(workspace.to_path_buf()),
        ..VmSpec::default()
    }
}

#[test]
fn a_guest_runs_on_after_a_restore_and_a_restore_of_its_diff() {
    let Some((kernel, initrd)) = guest() else {
        eprintln!("APZ_VMM_SMOKE_KERNEL / APZ_VMM_CONSOLE_INITRD not set: skipping");
        return;
    };
    let dir = scratch("e2e");
    let workspace = workspace_image(&dir);

    // Boot, and let the guest come up: its first heartbeat, then a console round trip.
    let booted_at = Instant::now();
    let mut first = spawn(spec(&kernel, &initrd, 41, dir.join("a"), &workspace)).expect("boot");
    assert!(
        wait_for(&first, 0, "heartbeat 1", Duration::from_secs(30)),
        "the guest never beat:\n{}",
        first.serial()
    );
    let boot_ms = booted_at.elapsed().as_millis();
    first
        .serial_input(b"echo before-snapshot\n")
        .expect("input");
    assert!(wait_for(
        &first,
        0,
        "before-snapshot",
        Duration::from_secs(10)
    ));

    // A full snapshot of the booted guest, then the guest is gone.
    let snap = dir.join("snap1");
    let info = first.snapshot(&snap, SnapshotKind::Full).expect("snapshot");
    assert_eq!(info.kind, SnapshotKind::Full);
    assert!(info.memory_written > 0);
    eprintln!(
        "boot to first heartbeat {boot_ms} ms; full snapshot wrote {} KiB, paused {} ms",
        info.memory_written / 1024,
        info.paused_ms
    );
    first.kill().expect("kill");

    // Restored under another CID and socket: it goes on beating and answers the console.
    let mut restore_spec = spec(&kernel, &initrd, 42, dir.join("b"), &workspace);
    restore_spec.restore = Some(snap.clone());
    let restored_at = Instant::now();
    let second = spawn(restore_spec).expect("restore");
    let restore_ms = restored_at.elapsed().as_millis();
    second.serial_input(b"echo after-restore\n").expect("input");
    assert!(
        wait_for(&second, 0, "after-restore", Duration::from_secs(15)),
        "the restored guest did not answer:\n{}",
        second.serial()
    );
    let answered_ms = restored_at.elapsed().as_millis();
    assert!(
        wait_for(&second, 0, "heartbeat", Duration::from_secs(12)),
        "the restored guest stopped beating:\n{}",
        second.serial()
    );
    eprintln!("restore returned in {restore_ms} ms; console answered after {answered_ms} ms");

    // A diff snapshot of the restored guest (over a copy of the first memory file), restored again.
    let snap2 = dir.join("snap2");
    let mut second = second;
    let info2 = second
        .snapshot(&snap2, SnapshotKind::Diff)
        .expect("diff snapshot");
    assert_eq!(info2.kind, SnapshotKind::Diff);
    eprintln!(
        "diff snapshot wrote {} KiB, paused {} ms",
        info2.memory_written / 1024,
        info2.paused_ms
    );
    second.kill().expect("kill");
    let mut third_spec = spec(&kernel, &initrd, 43, dir.join("c"), &workspace);
    third_spec.restore = Some(snap2);
    let mut third = spawn(third_spec).expect("restore of the diff");
    third.serial_input(b"echo after-diff\n").expect("input");
    assert!(
        wait_for(&third, 0, "after-diff", Duration::from_secs(15)),
        "the guest restored from the diff did not answer:\n{}",
        third.serial()
    );
    third
        .serial_input(b"wspace snapshot-test\n")
        .expect("input");
    assert!(
        wait_for(&third, 0, "snapshot-test", Duration::from_secs(10)),
        "the workspace volume did not answer after the restore:\n{}",
        third.serial()
    );
    third.kill().expect("kill");
    let _ = std::fs::remove_dir_all(&dir);
}
