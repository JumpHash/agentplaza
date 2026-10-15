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
// `scripts/mk-initrd.sh` (it carries `/agent-harness`), named by
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

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use vmm::jail::jailer;
use vmm::spawner::{
    CHILD_BINARY_ENV, CHILD_EXIT_USAGE, CHILD_PDEATHSIG, CHILD_READY, SpawnMode, VmExit, VmHandle,
    VmSpec, VsockConfig, create_uds_dir, set_spawn_mode, spawn,
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
/// then from the repository root (where `scripts/mk-initrd.sh` writes),
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
        balloon: None,
        restore: None,
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

// ---------------------------------------------------------------------------
// The parent-death backstop: `PR_SET_PDEATHSIG(SIGKILL)` in the child.
// ---------------------------------------------------------------------------

/// The environment variable that tells a re-executed copy of this binary it is
/// the *parent* of a per-VM process rather than an ordinary test run. Its
/// value is the scratch directory the parent works in.
const HELPER_ENV: &str = "APZ_VMM_PARENT_HELPER";

/// The ignored test [`the_per_vm_process_dies_with_its_parent`] re-executes
/// this binary as.
const HELPER_TEST: &str = "the_parent_of_a_per_vm_process";

/// The helper's guest CID and command line: a guest that parks forever
/// (`plaza.halt`), so *nothing* but the host can end its VM — a guest that
/// could exit on its own would make "the child is gone" ambiguous.
const HELPER_CID: u64 = 415;
const HELPER_CMDLINE: &str = "console=ttyS0 panic=-1 plaza.halt=1";

/// How long the child of a `SIGKILL`ed parent is given to be gone. The
/// mechanism is the kernel's, so delivery is immediate; this is slack for the
/// reaping (and for a loaded machine), not a timeout the mechanism needs.
const ORPHAN_DEADLINE: Duration = Duration::from_secs(10);

/// How long a VM spawned from a short-lived caller thread is watched after that
/// thread is gone. The defect it pins killed the child within milliseconds of
/// the spawning thread's exit, so a window this wide is far longer than the
/// failure and far shorter than the parked guest's own life.
const SURVIVES_CALLER_SETTLE: Duration = Duration::from_secs(3);

/// The `apz-vmm` binary these tests run directly: [`CHILD_BINARY_ENV`] when an
/// operator set it (the spawner's own override), else the one cargo built for
/// this test.
fn child_binary() -> PathBuf {
    match std::env::var_os(CHILD_BINARY_ENV) {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(env!("CARGO_BIN_EXE_apz-vmm")),
    }
}

/// The `/proc/<pid>/stat` state of the process at `proc`: `None` when there is
/// no such process (or no readable stat), else the one-letter state.
fn proc_state(proc: &Path) -> Option<char> {
    let stat = fs::read_to_string(proc.join("stat")).ok()?;
    // `pid (comm) state ...`: `comm` can hold spaces and parentheses, so the
    // state is the first field after the *last* `)`.
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().next()?.chars().next()
}

/// Whether the process `/proc/<pid>` names has **ended**: its directory is
/// gone (reaped), or its state is a dead one. "Ended" is the claim; whether
/// the host's PID 1 has collected the corpse is not — a zombie counts, so this
/// does not assume the reaper is prompt.
fn is_dead(proc: &Path) -> bool {
    match proc_state(proc) {
        None | Some('Z' | 'X' | 'x') => true,
        Some(_) => false,
    }
}

/// The tail of the parent helper's own stderr, for an assertion message.
fn helper_tail(dir: &Path) -> String {
    let Ok(text) = fs::read_to_string(dir.join("helper.err")) else {
        return "no helper log".to_string();
    };
    text[text.len().saturating_sub(600)..].to_string()
}

/// (g) The child's parent is the spawner's own thread, not the caller's: a VM
/// spawned from a thread that **exits** — the shape of an HTTP request thread,
/// which is how `noded` boots a run's VMs — keeps running.
///
/// This is the live defect the acceptance suite found (`tests/acceptance/
/// suite_jail.py`, "an API-created run in process mode ... still up after the
/// request"): `apz-vmm` arms `PR_SET_PDEATHSIG(SIGKILL)` against the *thread*
/// that created it, and a request thread exits when its response is written, so
/// the VM was `SIGKILL`ed milliseconds after the run reached `running`. The fix
/// is that [`spawn`] hands the command to one long-lived supervisor thread
/// (`spawner::process`), so the child's parent thread outlives the caller. The
/// child here is a guest that parks forever ([`HELPER_CMDLINE`]), so nothing
/// but the host can end it: if it is still alive once the caller thread is
/// gone, the caller's death did not reach it.
#[test]
fn a_short_lived_caller_thread_does_not_take_the_guest_down() {
    let _guard = mode_lock();
    if smoke_kernel().is_none() {
        eprintln!("skipping: set APZ_VMM_SMOKE_KERNEL (needs a hypervisor and a guest kernel)");
        return;
    }
    assert!(harness_initrd().is_some(), "a guest initramfs is required");

    let dir = scratch("short-lived-caller");
    let cid = 416;
    set_spawn_mode(SpawnMode::Process);

    // The caller: a thread that boots the VM, returns the handle, and exits —
    // exactly what an HTTP request thread does once it has answered.
    let booted = spec(&dir, cid, HELPER_CMDLINE);
    let mut vm = std::thread::Builder::new()
        .name("short_lived_caller".to_string())
        .spawn(move || spawn(booted))
        .expect("the caller thread starts")
        .join()
        .expect("the caller thread ends")
        .expect("the per-VM process boots the guest");
    let pid = vm.process_id().expect("the VM runs in a per-VM process");
    let proc = PathBuf::from(format!("/proc/{pid}"));

    // The guest booted: its kernel reached the parent's capture.
    assert!(
        wait_for_serial(&vm, "Linux version", SETTLE),
        "the guest's kernel did not reach the parent's capture: {}",
        serial_tail(&vm)
    );

    // The caller thread is gone. The child must outlive it: the parent-death
    // signal watches the thread that created the child, so a child spawned on a
    // soon-to-exit thread is `SIGKILL`ed within milliseconds of it — long
    // before this settle elapses.
    std::thread::sleep(SURVIVES_CALLER_SETTLE);
    assert!(
        !is_dead(&proc),
        "the per-VM process {pid} ended although the guest was parked (state {:?}): {}",
        proc_state(&proc),
        log_tail(&vm)
    );

    // And it armed the backstop it still carries: the daemon's death will take
    // it, a caller's will not.
    assert!(
        vm.process_log().iter().any(|line| line == CHILD_PDEATHSIG),
        "the child never armed its parent-death signal: {}",
        log_tail(&vm)
    );

    vm.kill().expect("the kill takes the per-VM process down");
    assert!(
        wait_until(|| is_dead(&proc), Duration::from_secs(10)),
        "the per-VM process {pid} was not reaped after the kill"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// (e) The per-VM process **dies with its parent**: a parent that is `SIGKILL`ed
/// — its stop path never runs, so neither the graceful `SIGTERM` nor the stdin
/// close happens — leaves no live `apz-vmm` behind, because the child armed
/// `PR_SET_PDEATHSIG(SIGKILL)` and the kernel delivers it.
///
/// The parent cannot be this test process, so it is a re-executed copy of this
/// binary (`the_parent_of_a_per_vm_process`): the same `spawner::spawn` call a
/// daemon makes, the same pipes, and the child a real jailed `apz-vmm` with a
/// guest parked forever. What the test asserts is the *outcome* — a live child
/// before, and no live child within [`ORPHAN_DEADLINE`] after — and the
/// mechanism is the signal: measured, with the backstop absent, a child in
/// exactly this shape outlived its `SIGKILL`ed parent for minutes with its
/// guest still parked (the stdin EOF only ends console-input forwarding; see
/// `arm_parent_death_signal` in `src/bin/apz-vmm/linux.rs`).
#[test]
fn the_per_vm_process_dies_with_its_parent() {
    if smoke_kernel().is_none() {
        eprintln!("skipping: set APZ_VMM_SMOKE_KERNEL (needs a hypervisor and a guest kernel)");
        return;
    }
    assert!(harness_initrd().is_some(), "a guest initramfs is required");

    let dir = scratch("orphan");
    let pidfile = dir.join("child.pid");
    let logfile = dir.join("child.log");
    let helper_err = fs::File::create(dir.join("helper.err")).expect("the helper's log file");

    // The parent: a copy of this binary, in its own process, holding a real
    // per-VM process's stdin pipe.
    let mut helper = common::command(&std::env::current_exe().expect("the test binary's path"))
        .args([
            "--ignored",
            "--exact",
            HELPER_TEST,
            "--test-threads=1",
            "--nocapture",
        ])
        .env(HELPER_ENV, &dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(helper_err))
        .spawn()
        .expect("the parent helper starts");

    // It reports its child (the real per-VM process) once that child's VM is up.
    assert!(
        wait_until(|| pidfile.is_file(), SETTLE),
        "the parent helper never booted a per-VM process: {}",
        helper_tail(&dir)
    );
    let pid: i32 = fs::read_to_string(&pidfile)
        .expect("the child's pid file")
        .trim()
        .parse()
        .expect("a pid");
    let proc = PathBuf::from(format!("/proc/{pid}"));

    // The child is a live VM process — the kernel's own name for it, never the
    // helper's claim — and it armed the backstop before it did anything else.
    assert!(
        wait_until(
            || fs::read_to_string(proc.join("status")).is_ok_and(|status| status
                .lines()
                .any(|line| line.starts_with("Name:") && line.contains("apz-vmm"))),
            SETTLE
        ),
        "/proc/{pid} is not a live per-VM process: {}",
        helper_tail(&dir)
    );
    let log = fs::read_to_string(&logfile).expect("the helper reported the child's log");
    assert!(
        log.contains(CHILD_PDEATHSIG),
        "the child never armed its parent-death signal: {log}"
    );
    assert!(
        log.contains(CHILD_READY),
        "the child never reported its VM up: {log}"
    );
    assert!(
        !is_dead(&proc),
        "the child {pid} ended before the parent was killed — the test proves nothing"
    );

    // The parent dies *without asking*: `Child::kill` is `SIGKILL`, so no stop
    // path (no `SIGTERM`, and the stdin pipe is closed only by death) runs.
    let killed_at = Instant::now();
    helper.kill().expect("the parent helper is SIGKILLed");
    let _ = helper.wait();

    assert!(
        wait_until(|| is_dead(&proc), ORPHAN_DEADLINE),
        "the per-VM process {pid} outlived its SIGKILLed parent by {:?} (state {:?}, log {log:?}): \
         the parent-death signal did not take",
        killed_at.elapsed(),
        proc_state(&proc)
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The parent helper: what the test above re-executes this binary to run. It
/// is the parent of a real per-VM process and then does nothing but stay alive
/// until the test `SIGKILL`s it.
///
/// `#[ignore]`d so an ordinary `cargo test` never starts it (it boots a guest
/// and sleeps); run without [`HELPER_ENV`] — an operator asking for ignored
/// tests — it is a no-op.
#[test]
#[ignore = "re-executed by the parent-death test as the parent of a real apz-vmm"]
fn the_parent_of_a_per_vm_process() {
    let Ok(dir) = std::env::var(HELPER_ENV) else {
        return;
    };
    let dir = PathBuf::from(dir);

    set_spawn_mode(SpawnMode::Process);
    let vm = spawn(spec(&dir, HELPER_CID, HELPER_CMDLINE)).expect("the parent helper boots its VM");
    let pid = vm.process_id().expect("a per-VM process");
    fs::write(dir.join("child.pid"), pid.to_string()).expect("the child's pid is reported");
    // The child's log as it stands once the VM is up, so the test can assert
    // what the child said about itself: the child is a different process, and
    // the helper — which holds the pipe the log came down — is the one that
    // gets `SIGKILL`ed.
    fs::write(dir.join("child.log"), vm.process_log().join("\n"))
        .expect("the child's log is reported");

    // Stay the parent until the test kills us. Bounded, so an interrupted test
    // cannot leave this process behind forever.
    for _ in 0..600 {
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// (f) The child **arms the parent-death signal first** — before it parses its
/// command line, let alone boots a VM. A command line it must refuse is the
/// smallest run that shows the ordering, and it needs no hypervisor, no guest
/// kernel and no VM; `--help` shows the same line on the success path.
///
/// This is the only cheap observable there is for the setting: the child logs
/// the kernel's answer (the `prctl(2)` return code, which gates the line), and
/// no `/proc` field reports it — `/proc/<pid>/status` carries `SigPnd`/
/// `SigCgt` (pending signals and *dispositions*) and no parent-death field,
/// while `PR_GET_PDEATHSIG` only answers about the calling thread itself. The
/// *behaviour* is the test above.
#[test]
fn the_child_arms_its_parent_death_signal_first() {
    let binary = child_binary();

    let refused = common::command(&binary)
        .arg("--nonsense")
        .output()
        .expect("the per-VM process runs");
    assert_eq!(
        refused.status.code(),
        Some(CHILD_EXIT_USAGE),
        "a command line with no VM in it is refused: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(
        stderr.lines().next(),
        Some(CHILD_PDEATHSIG),
        "the child's first line must be the arming, before it reads the command line: {stderr:?}"
    );

    let help = common::command(&binary)
        .arg("--help")
        .output()
        .expect("the per-VM process runs");
    assert!(help.status.success(), "--help exits 0");
    assert_eq!(
        String::from_utf8_lossy(&help.stderr).lines().next(),
        Some(CHILD_PDEATHSIG),
        "the arming line is on stderr, and the usage is on stdout"
    );
    assert!(
        String::from_utf8_lossy(&help.stdout).starts_with("usage: apz-vmm"),
        "--help still prints the usage"
    );
}
