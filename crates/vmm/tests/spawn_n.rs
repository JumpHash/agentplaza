//! N-VM boot smoke test.
//!
//! Skipped unless `APZ_VMM_SMOKE_KERNEL` names a bootable x86_64 kernel
//! (uncompressed ELF `vmlinux` or `bzImage`), because the test needs
//! `/dev/kvm` and a guest kernel:
//!
//! ```text
//! APZ_VMM_SMOKE_KERNEL=/tmp/vmlinux-test cargo test -p vmm --test spawn_n
//! ```

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use vmm::spawner::{VmExit, VmHandle, VmSpec, spawn_n};

/// The kernel named by `APZ_VMM_SMOKE_KERNEL`, if the variable is set.
fn smoke_kernel() -> Option<PathBuf> {
    match env::var_os("APZ_VMM_SMOKE_KERNEL") {
        Some(path) if !path.is_empty() => Some(PathBuf::from(path)),
        _ => {
            eprintln!(
                "APZ_VMM_SMOKE_KERNEL is not set: skipping the N-VM boot smoke test"
            );
            None
        }
    }
}

/// How long a single VM is given to boot and shut down.
const BOOT_TIMEOUT: Duration = Duration::from_secs(120);

/// Two VMs boot independently, each printing a Linux banner to its own serial
/// console, and each shuts the VM down on its own (`panic=-1` plus the lack of
/// an init makes the kernel panic out, which stops the VM).
#[test]
fn spawn_two_vms_boot_linux() {
    let Some(kernel) = smoke_kernel() else {
        return;
    };

    let spec = VmSpec {
        kernel,
        cmdline: "console=ttyS0 panic=-1".to_string(),
        vcpus: 1,
        memory_mib: 512,
        initrd: None,
        gic: None,
    };

    let mut vms = spawn_n(spec, 2).expect("spawning two VMs");
    assert_eq!(vms.len(), 2);
    assert_eq!(
        vms.iter().map(VmHandle::id).collect::<Vec<_>>(),
        vec![0, 1],
        "spawn_n must hand out ids 0..n"
    );

    for vm in &mut vms {
        let exit = vm
            .wait_timeout(BOOT_TIMEOUT)
            .expect("waiting for the VM")
            .unwrap_or_else(|| {
                panic!(
                    "VM {} did not stop within {BOOT_TIMEOUT:?}; serial so far:\n{}",
                    vm.id(),
                    vm.serial()
                )
            });
        assert_eq!(
            exit,
            VmExit::GuestExited,
            "VM {} serial:\n{}",
            vm.id(),
            vm.serial()
        );
        assert!(
            vm.serial().contains("Linux version"),
            "VM {} printed no Linux banner; serial:\n{}",
            vm.id(),
            vm.serial()
        );
    }
}

/// A VM whose guest hangs (a kernel panic with `panic=0` never reboots, and
/// there is no init to keep it going) stays up until it is killed, and the
/// kill path stops and joins every thread.
#[test]
fn kill_stops_a_running_vm() {
    let Some(kernel) = smoke_kernel() else {
        return;
    };

    let spec = VmSpec {
        // `panic=0` (the default) makes the kernel wait after panicking
        // instead of rebooting: the VM keeps running.
        cmdline: "console=ttyS0 panic=0".to_string(),
        ..VmSpec {
            kernel,
            ..VmSpec::default()
        }
    };

    let started = std::time::Instant::now();
    let mut vm = spawn_n(spec, 1).expect("spawning a VM").remove(0);

    // Wait until the guest is actually running (it reaches the panic), then
    // make sure it is still there: the kill below is what stops it.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !vm.serial().contains("Kernel panic") && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        vm.wait_timeout(Duration::from_millis(500)).unwrap().is_none(),
        "a guest panicked with panic=0 must keep the VM running"
    );

    vm.kill().expect("killing the VM");
    assert_eq!(
        vm.wait().expect("waiting for the killed VM"),
        VmExit::Killed,
        "a killed VM must not report a guest exit"
    );
    eprintln!(
        "kill took {:?}; serial tail:\n{}",
        started.elapsed(),
        vm.serial()
    );
}

/// Spawning without a kernel must fail with an error rather than panic.
#[test]
fn spawn_without_kernel_fails() {
    // Only meaningful where a backend is compiled in; on other targets the
    // spawner reports `NoBackend`, which is also an error.
    let err = spawn_n(VmSpec::default(), 1).unwrap_err();
    eprintln!("expected boot failure: {err}");
}
