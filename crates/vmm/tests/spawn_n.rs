//! N-VM boot smoke test.
//!
//! Skipped unless `APZ_VMM_SMOKE_KERNEL` names a bootable x86_64 kernel
//! (uncompressed ELF `vmlinux` or `bzImage`), because the test needs
//! `/dev/kvm` and a guest kernel:
//!
//! ```text
//! APZ_VMM_SMOKE_KERNEL=/tmp/vmlinux-test cargo test -p vmm --test spawn_n
//! ```
//!
//! Two more tests use the guest userspace built by
//! `scripts/mk-initrd.sh` and skip unless their initramfs is named:
//!
//! * `kill_stops_a_halted_vm` needs `APZ_VMM_HALT_INITRD` (a guest whose
//!   `PID 1` parks forever, `plaza.halt=1`), and
//! * `serial_console_input_reaches_the_guest` needs `APZ_VMM_CONSOLE_INITRD`
//!   (a guest whose `PID 1` serves commands on the serial console).
//!
//! ```text
//! APZ_VMM_SMOKE_KERNEL=/boot/vmlinuz-linux \
//! APZ_VMM_HALT_INITRD=target/guest/initrd.x86_64.gz \
//! APZ_VMM_CONSOLE_INITRD=target/guest/initrd.x86_64.gz \
//!     cargo test -p vmm --test spawn_n
//! ```

use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use vmm::spawner::{SpawnError, VmExit, VmHandle, VmSpec, spawn_n};

/// The kernel named by `APZ_VMM_SMOKE_KERNEL`, if the variable is set.
fn smoke_kernel() -> Option<PathBuf> {
    match env::var_os("APZ_VMM_SMOKE_KERNEL") {
        Some(path) if !path.is_empty() => Some(resolve(PathBuf::from(path))),
        _ => {
            eprintln!("APZ_VMM_SMOKE_KERNEL is not set: skipping the N-VM boot smoke test");
            None
        }
    }
}

/// The image named by the `name` environment variable, if it is set.
fn image_from_env(name: &str) -> Option<PathBuf> {
    match env::var_os(name) {
        Some(path) if !path.is_empty() => Some(resolve(PathBuf::from(path))),
        _ => {
            eprintln!("{name} is not set: skipping the test that needs it");
            None
        }
    }
}

/// Resolves an image path an operator set in the environment.
///
/// `cargo test` gives the test binary the *crate* directory as its working
/// directory, while the paths an operator sets have been written from the
/// workspace root (`scripts/mk-initrd.sh` builds
/// `target/guest/initrd.x86_64.gz` there), so a relative path is tried from
/// both. A path that exists in neither is passed on as it was given, so the
/// failure names the file the operator asked for.
///
/// The answer is canonicalised: the jailer mirrors every source at its own path
/// and refuses one that names a parent directory (`..`), so the repository-root
/// candidate this helper builds must be resolved before it reaches a boot —
/// otherwise the boot is refused for a path the operator never wrote.
fn resolve(path: PathBuf) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(&path) {
        return real;
    }
    let from_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(&path);
    std::fs::canonicalize(&from_root).unwrap_or(path)
}

/// Polls `condition` until it holds or `timeout` elapses.
fn wait_until(condition: impl Fn() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Waits for `needle` to appear in the VM's serial capture.
fn wait_for_serial(vm: &VmHandle, needle: &str, timeout: Duration) -> bool {
    wait_until(|| vm.serial().contains(needle), timeout)
}

/// The tail of a VM's serial capture, for an assertion message.
fn serial_tail(vm: &VmHandle) -> String {
    let serial = vm.serial();
    let skip = serial.chars().count().saturating_sub(2000);
    serial.chars().skip(skip).collect()
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
        ..VmSpec::default()
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
        assert!(
            matches!(exit, VmExit::GuestExited { .. }),
            "VM {} did not stop on its own (it reported {exit:?}); serial:\n{}",
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
        vm.wait_timeout(Duration::from_millis(500))
            .unwrap()
            .is_none(),
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

/// How long killing a halted guest is given: the spawner's own kill timeout is
/// 10 s per attempt and it retries once, so a vCPU left running behind the
/// join shows up as a `KillTimeout` well inside this bound.
const KILL_BOUND: Duration = Duration::from_secs(30);

/// A guest that halted itself still stops when the host kills it.
///
/// The halt initramfs's `PID 1` parks in `pause()` forever (`plaza.halt=1`),
/// so its vCPU sits idle *inside* `KVM_RUN` (`kvm_vcpu_block`) and no guest
/// exit is ever coming to wake anything up: `VmHandle::kill` has to break the
/// vCPU out of the hypervisor call, observe the stop request and join the VM.
/// A stop request carried only by the event channel - which the vCPU reads
/// when `KVM_RUN` is interrupted - would leave that vCPU running, the exit
/// watcher's join blocked forever, `wait()` hanging and the VM's guest memory
/// never released.
#[test]
fn kill_stops_a_halted_vm() {
    let (Some(kernel), Some(initrd)) = (smoke_kernel(), image_from_env("APZ_VMM_HALT_INITRD"))
    else {
        return;
    };
    let spec = VmSpec {
        kernel,
        cmdline: "console=ttyS0 plaza.halt=1".to_string(),
        initrd: Some(initrd),
        ..VmSpec::default()
    };
    let mut vm = spawn_n(spec, 1).expect("spawning a halted VM").remove(0);

    // The guest's own message (not the kernel's echo of `plaza.halt=1` on its
    // command line) is what says it reached its parking loop.
    assert!(
        wait_for_serial(&vm, "plaza guest: parking forever", BOOT_TIMEOUT),
        "the guest must reach its parking loop; serial tail:\n{}",
        serial_tail(&vm)
    );
    // Nothing is about to stop this guest by itself: it is up, idle, and stays
    // up until it is killed.
    assert!(
        vm.wait_timeout(Duration::from_millis(500))
            .unwrap()
            .is_none(),
        "a guest parked in pause() must keep the VM running"
    );

    let started = Instant::now();
    vm.kill().expect("killing the halted VM");
    let elapsed = started.elapsed();
    assert_eq!(
        vm.wait().expect("waiting for the killed VM"),
        VmExit::Killed,
        "a killed VM must not report a guest exit"
    );
    assert!(
        elapsed < KILL_BOUND,
        "killing a halted guest must not strand its vCPU (took {elapsed:?})"
    );

    // The console is gone with the VM: input for a stopped guest is refused
    // rather than queued into a console nobody owns any more.
    let refused = vm.serial_input(b"echo nobody-reads-this\n");
    assert!(
        matches!(refused, Err(SpawnError::SerialInput { .. })),
        "input for a stopped VM must be refused: {refused:?}"
    );
    eprintln!("killing a halted guest took {elapsed:?}");
}

/// Host console input reaches the guest, and the guest still stops itself.
///
/// The console initramfs's `PID 1` reads commands from `ttyS0` - the kernel
/// gives it that line as stdin, since `console=ttyS0` names it as the console -
/// and answers them on the same line. A marker coming back proves the host's
/// bytes crossed into guest memory, raised the console's interrupt and were
/// processed by the guest's terminal and command loop; `poweroff` then proves
/// the VM still stops because the *guest* exited.
#[test]
fn serial_console_input_reaches_the_guest() {
    let (Some(kernel), Some(initrd)) = (smoke_kernel(), image_from_env("APZ_VMM_CONSOLE_INITRD"))
    else {
        return;
    };
    let spec = VmSpec {
        kernel,
        cmdline: "console=ttyS0".to_string(),
        initrd: Some(initrd),
        ..VmSpec::default()
    };
    let mut vm = spawn_n(spec, 1).expect("spawning a console VM").remove(0);

    assert!(
        wait_for_serial(&vm, "plaza guest alive: heartbeat", BOOT_TIMEOUT),
        "the guest must reach its command loop; serial tail:\n{}",
        serial_tail(&vm)
    );

    vm.serial_input(b"echo plaza-marker-7\n")
        .expect("sending console input");

    // The guest's *answer* is a line that is nothing but the marker: the
    // terminal echo of the command itself is the line `echo plaza-marker-7`,
    // so only the guest running the command produces a bare marker line.
    const MARKER: &str = "plaza-marker-7";
    let answered = wait_until(
        || {
            vm.serial()
                .lines()
                .any(|line| line.trim_end_matches('\r') == MARKER)
        },
        Duration::from_secs(60),
    );
    assert!(
        answered,
        "the guest must answer the command it was given; serial tail:\n{}",
        serial_tail(&vm)
    );

    // `poweroff` is the guest's own shutdown (mapped to a reboot, because this
    // port has no ACPI S5 path): the VM has to stop because the guest exited,
    // not because the host killed it.
    vm.serial_input(b"poweroff\n").expect("sending poweroff");
    let exit = vm
        .wait_timeout(Duration::from_secs(60))
        .expect("waiting for the VM")
        .expect("the guest must exit on its own after poweroff");
    assert!(
        matches!(exit, VmExit::GuestExited { .. }),
        "poweroff must be a guest exit, not a kill: {exit:?}"
    );
}
