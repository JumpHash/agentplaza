// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// The per-VM process, end to end: `SpawnMode::Process` boots a real guest in a
// real `apz-vmm` child, the guest's own agent harness completes a keepalive
// session over the muxer socket the *child* bound, and the child is provably
// inside the jailer's root — read from the kernel's `/proc`, never from
// anything the daemon says about itself.
//
// Skipped unless `APZ_VMM_SMOKE_KERNEL` names a bootable x86_64 kernel, because
// the test needs `/dev/kvm` and a guest (the same gate `tests/spawn_n.rs`
// uses). The guest userspace is the initramfs built by
// `scripts/mk-initrd-x86_64.sh` (it carries `/agent-harness`), named by
// `APZ_VMM_HARNESS_INITRD` or taken from the repository's `target/guest/`.
//
// ```text
// APZ_VMM_SMOKE_KERNEL=/boot/vmlinuz-linux \
// APZ_VMM_HARNESS_INITRD=target/guest/initrd.x86_64.gz \
//     cargo test -p vmm --test per_vm_process -- --nocapture
// ```
//
// Every test in this file drives the same process-wide mode switch
// (`spawner::set_spawn_mode`), so they take [`MODE_LOCK`] and run one at a
// time: the switch is the deployment's, not a per-call argument (see the
// mode's documentation in `spawner.rs`), and serialising is how a test binary
// uses it honestly.
#![cfg(target_os = "linux")]

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use vmm::jail::jailer;
use vmm::spawner::{
    SpawnMode, VmExit, VmHandle, VmSpec, VsockConfig, create_uds_dir, set_spawn_mode, spawn,
};

/// The process-wide mode switch is shared by every test here: the tests
/// serialise on it instead of racing each other's `set_spawn_mode`.
static MODE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn mode_lock() -> MutexGuard<'static, ()> {
    MODE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The kernel named by `APZ_VMM_SMOKE_KERNEL`, if the variable is set.
fn smoke_kernel() -> Option<PathBuf> {
    std::env::var_os("APZ_VMM_SMOKE_KERNEL").map(PathBuf::from)
}

/// The guest initramfs: `APZ_VMM_HARNESS_INITRD` when it is set, else the
/// repository's `target/guest/initrd.x86_64.gz` when that exists.
fn harness_initrd() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("APZ_VMM_HARNESS_INITRD") {
        return Some(resolve(PathBuf::from(path)));
    }
    let default = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/guest/initrd.x86_64.gz");
    default.is_file().then(|| resolve(default))
}

/// Resolves an image path an operator set in the environment: a relative path
/// is tried from the crate directory (a test binary's working directory) and
/// then from the repository root (where `scripts/mk-initrd-x86_64.sh` writes),
/// and the answer is canonicalised — a path with a `..` in it is refused by the
/// jailer (its mirrors must stay inside the root), so the test must hand it a
/// real path, exactly as a deployment would.
fn resolve(path: PathBuf) -> PathBuf {
    if path.is_absolute() && path.exists() {
        return fs::canonicalize(&path).unwrap_or(path);
    }
    let from_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(&path);
    for candidate in [PathBuf::from(&path), from_root] {
        if let Ok(real) = fs::canonicalize(&candidate) {
            return real;
        }
    }
    path
}

/// A private work directory for one test, `0700` like every socket directory
/// this port makes.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("apz-per-vm-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a work directory");
    create_uds_dir(&dir).expect("a private work directory");
    dir
}

/// A spec for one guest of this test's own CID, with a vsock device whose
/// socket lives in `dir`.
fn spec(dir: &Path, cid: u64, cmdline: &str) -> VmSpec {
    VmSpec {
        kernel: smoke_kernel().expect("a kernel"),
        cmdline: cmdline.to_string(),
        vcpus: 1,
        memory_mib: 256,
        initrd: harness_initrd(),
        gic: None,
        vsock: Some(VsockConfig {
            guest_cid: cid,
            uds_path: dir.join(format!("vm{cid}")),
        }),
        workspace: None,
    }
}

/// Polls `condition` until it holds or `timeout` elapses.
fn wait_until(condition: impl Fn() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    condition()
}

/// Waits for `needle` to appear in the VM's serial capture.
fn wait_for_serial(vm: &VmHandle, needle: &str, timeout: Duration) -> bool {
    wait_until(|| vm.serial().contains(needle), timeout)
}

/// The tail of a VM's serial capture, for an assertion message.
fn serial_tail(vm: &VmHandle) -> String {
    let serial = vm.serial();
    serial[serial.len().saturating_sub(600)..].to_string()
}

/// The tail of a VM's per-VM process log, for an assertion message.
fn log_tail(vm: &VmHandle) -> String {
    let log = vm.process_log();
    log[log.len().saturating_sub(20)..].join("\n")
}

/// How long a boot, a session or an exit is given.
const SETTLE: Duration = Duration::from_secs(90);

// ---------------------------------------------------------------------------
// The host end of the guest's keepalive session: a stub orchestrator.
// ---------------------------------------------------------------------------

/// The harness's default orchestrator port (`plaza.port=`, unset here): the
/// guest connects to CID 2 port 5000, and the VM's muxer turns that into a
/// host connection to `{uds_path}_5000`.
const HARNESS_PORT: u32 = 5000;

/// Accepts the muxer's connection, which arrives once the guest's harness
/// connects, or fails the test after `timeout`.
fn accept(listener: &UnixListener, timeout: Duration) -> UnixStream {
    listener
        .set_nonblocking(true)
        .expect("the stub listener is non-blocking");
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .expect("the session is blocking");
                stream
                    .set_read_timeout(Some(Duration::from_secs(30)))
                    .expect("a read timeout");
                return stream;
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "the guest's harness never connected to {HARNESS_PORT}"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(err) => panic!("the stub listener failed: {err}"),
        }
    }
}

/// Reads one protocol line, failing the test if the connection ends or the
/// wait for `what` runs long.
fn read_line(reader: &mut BufReader<UnixStream>, what: &str) -> String {
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .unwrap_or_else(|err| panic!("waiting for {what}: {err}"));
    assert!(
        read > 0,
        "the connection closed while waiting for {what}; the guest's transcript so far is in the \
         serial capture"
    );
    line
}

/// Sends one line to the guest.
fn send(stream: &mut UnixStream, line: &str) {
    stream
        .write_all(format!("{line}\n").as_bytes())
        .expect("the session takes the line");
    stream.flush().expect("the line is sent");
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// (a) The child boots a real guest and the guest's whole session runs
/// through it: the guest's harness connects to the muxer socket **the child
/// bound**, and the hello → welcome → heartbeat → tool_call/tool_result → stop
/// → bye transcript is on the wire.
///
/// The stub here is the host end: it is what answers the guest, exactly as the
/// hub does, so what is proven is the transport (the child's muxer, the guest's
/// virtio-vsock and the guest's own protocol code) and not this test's opinion
/// of it.
#[test]
fn the_per_vm_process_boots_a_guest_and_carries_its_session() {
    let _guard = mode_lock();
    if smoke_kernel().is_none() {
        eprintln!("skipping: set APZ_VMM_SMOKE_KERNEL (needs a hypervisor and a guest kernel)");
        return;
    }
    assert!(harness_initrd().is_some(), "a guest initramfs is required");

    let dir = scratch("session");
    let cid = 410;
    let uds = dir.join(format!("vm{cid}"));

    // The host end of the guest's session, bound before the boot: the guest
    // connects as soon as its harness starts, and the muxer connects to this
    // path.
    let listener = UnixListener::bind(format!("{}_5000", uds.display()))
        .expect("the stub orchestrator binds the muxer's up-call path");

    set_spawn_mode(SpawnMode::Process);
    let mut vm = spawn(spec(
        &dir,
        cid,
        "console=ttyS0 panic=-1 plaza.keepalive=1 plaza.role=probe",
    ))
    .expect("the per-VM process boots the guest");
    let pid = vm.process_id().expect("the VM runs in a per-VM process");

    // The guest's own console arrives through the child's stdout, into the same
    // capture the in-process shape fills.
    assert!(
        wait_for_serial(&vm, "Linux version", SETTLE),
        "the guest's kernel did not reach the parent's capture: {}",
        serial_tail(&vm)
    );

    let mut stream = accept(&listener, SETTLE);
    let mut reader = BufReader::new(stream.try_clone().expect("the session reader"));

    // hello: the guest names itself, its CID (the device's, not the command
    // line's) and the keepalive session.
    let hello = read_line(&mut reader, "hello");
    for needle in [
        "\"type\":\"hello\"",
        &format!("\"guest_cid\":{cid}"),
        "\"role\":\"probe\"",
        "\"keepalive\":true",
    ] {
        assert!(hello.contains(needle), "hello {hello:?} lacks {needle}");
    }

    // welcome: the hub's acceptance, which the harness requires to be a v2
    // keepalive welcome.
    send(
        &mut stream,
        "{\"type\":\"welcome\",\"echo\":\"plaza-orchestrator\",\"role\":\"probe\",\"keepalive\":true}",
    );

    // heartbeat: the long-lived session's liveness line (the first beat is one
    // 5 s interval after the welcome).
    let beat = read_line(&mut reader, "the first heartbeat");
    assert!(
        beat.contains("\"type\":\"heartbeat\"") && beat.contains("\"seq\":1"),
        "the first line after the welcome was {beat:?}"
    );

    // tool_call → tool_result: the hub asks, the agent serves.
    send(
        &mut stream,
        "{\"type\":\"tool_call\",\"call\":1,\"name\":\"echo\",\"args\":{\"text\":\"plaza-via-child\"}}",
    );
    let result = read_line(&mut reader, "the tool result");
    assert!(
        result.contains("\"type\":\"tool_result\"")
            && result.contains("\"call\":1")
            && result.contains("\"ok\":true")
            && result.contains("plaza-via-child"),
        "the echo tool answered {result:?}"
    );

    // stop → bye: the hub ends the session and the agent answers, then closes.
    send(
        &mut stream,
        "{\"type\":\"stop\",\"reason\":\"the probe is done\"}",
    );
    let bye = read_line(&mut reader, "the bye");
    assert!(
        bye.contains("\"type\":\"bye\"") && bye.contains("\"calls\":1"),
        "the session ended with {bye:?}"
    );

    // The guest's own transcript of the same session is in the parent's
    // capture: the harness prints every step to its console.
    for needle in [
        "agent-harness: role probe accepted",
        "agent-harness: heartbeat 1",
        "agent-harness: the hub stopped the agent",
    ] {
        assert!(
            wait_for_serial(&vm, needle, SETTLE),
            "the guest console never showed {needle:?}: {}",
            serial_tail(&vm)
        );
    }

    // The console input path is proxied too: the host's bytes go to the child's
    // stdin, the child hands them to its UART, and the guest's serial command
    // loop (this initramfs' PID 1) answers on the same line.
    vm.serial_input(b"echo proxied-from-the-parent\n")
        .expect("the console takes the bytes");
    assert!(
        wait_for_serial(&vm, "proxied-from-the-parent", SETTLE),
        "the guest never answered the console input: {}",
        serial_tail(&vm)
    );

    // The guest is a keepalive one: the session ending does not stop the VM.
    // The kill is the host's, and it takes.
    assert!(
        Path::new(&format!("/proc/{pid}")).exists(),
        "the VM must still be up after the session ended"
    );
    vm.kill().expect("the kill takes the per-VM process down");
    let _ = fs::remove_dir_all(&dir);
}

/// (b) The VM is **inside the jailer**: the child's root is the per-VM root
/// (the `pivot_root` the kernel reports), the outside is unreachable through
/// it, its `/` listing holds only the jail's own entries, and every VMM thread
/// of the child runs with the seccomp filter installed (`Seccomp: 2`).
///
/// Every fact here is read from `/proc/<child pid>` — the kernel's view of the
/// process — never from the child's own report. The child's log is used only to
/// tell "this host cannot create a user namespace" (where the jailer says so
/// and the probe reports nothing) apart from a real failure.
#[test]
fn the_per_vm_process_is_inside_the_jail() {
    let _guard = mode_lock();
    let Some(kernel) = smoke_kernel() else {
        eprintln!("skipping: set APZ_VMM_SMOKE_KERNEL (needs a hypervisor and a guest kernel)");
        return;
    };
    assert!(harness_initrd().is_some(), "a guest initramfs is required");

    let dir = scratch("jail");
    let cid = 411;
    let uds = dir.join(format!("vm{cid}"));

    set_spawn_mode(SpawnMode::Process);
    let mut vm = spawn(spec(&dir, cid, "console=ttyS0 panic=-1 plaza.halt=1"))
        .expect("the per-VM process boots the guest");
    let pid = vm.process_id().expect("the VM runs in a per-VM process");
    let proc = PathBuf::from(format!("/proc/{pid}"));

    // The jailer reports on the child's stderr, which the handle carries.
    let log = log_tail(&vm);
    if log.contains("cannot be jailed") {
        eprintln!("skipping: this host cannot create a user namespace; the jailer said so: {log}");
        vm.kill().expect("the kill takes");
        return;
    }
    assert!(
        wait_until(
            || log_tail(&vm).contains("jailed in"),
            Duration::from_secs(10)
        ),
        "the jailer never reported a jail: {}",
        log_tail(&vm)
    );

    // 1. The process's `/` *is* the per-VM root. `/proc/<pid>/root` resolves to
    //    `/` inside the process's own mount namespace (that is what a pivot
    //    looks like from outside), so the kernel's mount table is what names
    //    the root: the root filesystem's `root=` field is the jail directory
    //    under the jail base, and its mount point is `/`.
    let mountinfo = fs::read_to_string(proc.join("mountinfo")).expect("the child's mount table");
    let root_mount = mountinfo
        .lines()
        .find_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            (fields.len() > 4 && fields[4] == "/").then(|| fields[3])
        })
        .expect("the child has a root mount");
    let base = jailer::default_base(Some(&dir));
    let base_name = base
        .file_name()
        .expect("the jail base has a name")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        Path::new(root_mount)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
        Some(format!("{pid}-0-0")),
        "the child's root filesystem is not the run's jail directory: {mountinfo}"
    );
    assert_eq!(
        Path::new(root_mount)
            .parent()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned()),
        Some(base_name.clone()),
        "the child's jail is not under this VM's jail base {base:?}: {mountinfo}"
    );

    // 2. The outside is unreachable through that root, while it is still there
    //    for this test.
    assert!(
        !proc.join("root/etc/hostname").exists(),
        "the child can still see the host's /etc — it is not jailed"
    );
    assert!(
        Path::new("/etc/hostname").exists(),
        "this test's own premise: the host has an /etc/hostname the jail does not"
    );

    // 3. The child's own `/` listing holds only the jail's entries: the mirrors
    //    of the sources this VM was given (each at its own path, so at its
    //    first component) plus the `/dev` nodes and the `/run` tmpfs. Nothing
    //    else of the host's tree is there.
    let mut expected: BTreeSet<String> = ["dev".to_string(), "run".to_string()].into();
    for source in [&kernel, &uds] {
        let component = source
            .components()
            .find_map(|component| match component {
                std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
                _ => None,
            })
            .expect("a source with a path component");
        expected.insert(component);
    }
    if let Some(initrd) = harness_initrd()
        && let Some(component) = initrd.components().find_map(|component| match component {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
    {
        expected.insert(component);
    }
    let listed: BTreeSet<String> = fs::read_dir(proc.join("root"))
        .expect("the child's root directory")
        .map(|entry| {
            entry
                .expect("a readable entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        listed, expected,
        "the child's / holds more (or less) than the jail's own entries"
    );

    // 4. Every VMM thread of the child runs under the seccomp filter: the
    //    kernel's per-thread `Seccomp` field is `2` (filter mode) for the three
    //    kinds the VMM makes. The child's own console threads and its main
    //    thread are *not* in this set: they are the parent-facing plumbing, not
    //    guest-facing VMM threads, and the filter's allowlist is derived for
    //    the latter.
    let tasks = proc.join("task");
    let statuses = |tasks: &Path| -> Vec<(String, String)> {
        let Ok(entries) = fs::read_dir(tasks) else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| fs::read_to_string(entry.path().join("status")).ok())
            .map(|status| {
                let field = |name: &str| {
                    status
                        .lines()
                        .find_map(|line| line.strip_prefix(name))
                        .unwrap_or("")
                        .trim()
                        .to_string()
                };
                (field("Name:"), field("Seccomp:"))
            })
            .collect()
    };
    let armed = wait_until(
        || {
            let seen = statuses(&tasks);
            ["apz_vcpu_0", "apz_vm0_vsock", "apz_vm0_exit"]
                .iter()
                .all(|wanted| {
                    seen.iter()
                        .any(|(name, seccomp)| name == wanted && seccomp == "2")
                })
        },
        Duration::from_secs(30),
    );
    assert!(
        armed,
        "not every VMM thread of the child is armed: {:?}",
        statuses(&tasks)
    );
    // And none of the VMM threads is unarmed: the set above is exact, not
    // "at least one of each".
    for (name, seccomp) in statuses(&tasks) {
        if name.starts_with("apz_vcpu")
            || (name.starts_with("apz_vm") && !name.starts_with("apz_vmm"))
        {
            assert_eq!(seccomp, "2", "VMM thread {name} is not armed");
        }
    }

    vm.kill().expect("the kill takes the jailed VM down");
    let _ = fs::remove_dir_all(&dir);
}

/// (d) The kill path: the child *is* the VM, so killing it stops the guest and
/// the handle reports the kill — not a guest exit it did not observe — and the
/// process is reaped rather than left as a zombie.
#[test]
fn killing_the_per_vm_process_stops_the_guest() {
    let _guard = mode_lock();
    if smoke_kernel().is_none() {
        eprintln!("skipping: set APZ_VMM_SMOKE_KERNEL (needs a hypervisor and a guest kernel)");
        return;
    }
    assert!(harness_initrd().is_some(), "a guest initramfs is required");

    let dir = scratch("kill");
    let cid = 412;
    set_spawn_mode(SpawnMode::Process);
    // The guest parks forever (`plaza.halt`): no guest exit is coming, so only
    // the host's stop can end this VM — the same case `tests/spawn_n.rs`'s
    // halted-guest test covers in-process, here across the process boundary.
    let mut vm = spawn(spec(&dir, cid, "console=ttyS0 panic=-1 plaza.halt=1"))
        .expect("the per-VM process boots the guest");
    let pid = vm.process_id().expect("the VM runs in a per-VM process");
    assert!(
        wait_for_serial(&vm, "Linux version", SETTLE),
        "the guest boots"
    );
    assert!(
        Path::new(&format!("/proc/{pid}")).exists(),
        "the per-VM process must be live before the kill"
    );

    vm.kill().expect("the kill takes the per-VM process down");
    assert_eq!(
        vm.wait().expect("the VM reports how it stopped"),
        VmExit::Killed,
        "a killed VM is a kill, not a guest exit"
    );
    assert!(
        wait_until(
            || !Path::new(&format!("/proc/{pid}")).exists(),
            Duration::from_secs(10)
        ),
        "the per-VM process was not reaped: /proc/{pid} is still there"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// (c) Both modes boot the same VM to the same end: a guest that shuts itself
/// down (`plaza.beats=1` reboots it after one heartbeat) reports
/// [`VmExit::GuestExited`] with `reset: true` whether the VMM was this
/// process's threads or a child process, and the child's exit status is what
/// carries that fact.
#[test]
fn the_guest_exit_reads_the_same_in_both_modes() {
    let _guard = mode_lock();
    if smoke_kernel().is_none() {
        eprintln!("skipping: set APZ_VMM_SMOKE_KERNEL (needs a hypervisor and a guest kernel)");
        return;
    }

    let dir = scratch("modes");

    // In this process: the v0 shape, unchanged.
    set_spawn_mode(SpawnMode::InProcess);
    let mut in_process = spawn(spec(&dir, 413, "console=ttyS0 panic=-1 plaza.beats=1"))
        .expect("the in-process VM boots");
    assert_eq!(
        in_process.process_id(),
        None,
        "an in-process VM has no per-VM process"
    );
    let in_process_exit = in_process.wait().expect("the guest stops the VM itself");

    // In a per-VM process: the same guest, the same spec but for the CID.
    set_spawn_mode(SpawnMode::Process);
    let mut child = spawn(spec(&dir, 414, "console=ttyS0 panic=-1 plaza.beats=1"))
        .expect("the per-VM process boots the same guest");
    assert!(
        child.process_id().is_some(),
        "a per-VM process VM reports its pid"
    );
    let child_exit = child.wait().expect("the guest stops the VM itself");

    assert_eq!(
        in_process_exit,
        VmExit::GuestExited { reset: true },
        "the guest's reboot is a reset: {}",
        serial_tail(&in_process)
    );
    assert_eq!(
        child_exit,
        in_process_exit,
        "the two modes must report the same guest exit: {}",
        serial_tail(&child)
    );
    let _ = fs::remove_dir_all(&dir);
}
