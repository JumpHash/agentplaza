// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the supervisor half of the per-VM process mode.
// Firecracker's jailer forks and `exec`s the VMM ("src/jailer/src/main.rs"
// runs the VMM as a child of the jailed process), so there the VMM *is* a
// process; this is that shape for this port, with `apz-vmm`
// (`src/bin/apz-vmm.rs`) as the child and this module as the parent. No code
// is copied from Firecracker's jailer: the protocol below — a readiness line
// on stderr, exit status as the exit reason, the muxer socket bound by the
// child, the console proxied over two pipes — is this port's own.

//! The per-VM process mode: one `apz-vmm` child per VM.
//!
//! **Why.** The jailer ([`crate::jail::jailer`]) needs a process that is
//! single-threaded when the VM is built — `unshare(CLONE_NEWUSER)` is refused
//! (`EINVAL`) by a process with a sibling thread, and that namespace is the
//! only way an unprivileged VMM gets `CAP_SYS_ADMIN` for a mount namespace.
//! `noded` is a serving daemon with threads of its own, so its in-process VMs
//! are *not* jailed; the jailer reports that honestly and they keep the
//! seccomp half. A process that holds exactly one VM has no such problem: it
//! is single-threaded until it starts the VM, so the jailer applies — and this
//! mode is how a caller gets it.
//!
//! **What the parent does.** It runs the child binary ([`super::child_binary`])
//! with one VM's [`VmSpec`] on its command line ([`child_args`]) and three
//! pipes, and proxies exactly the five things [`VmHandle`] promises:
//!
//! | handle surface | in-process | per-VM process |
//! |---|---|---|
//! | serial capture | the UART writes `runtime.serial` | the child's **stdout** is the guest's serial stream; a reader thread appends it to `runtime.serial` |
//! | console input | the UART's input queue | the input hook writes the child's **stdin**; the child hands it to its UART |
//! | `wait` | joins the VMM's threads | joins the waiter thread, which reaps the **child** and turns its exit status into [`VmExit`] |
//! | `kill` | the stop control asks the vCPU loop to finish | the stop control signals the child: `SIGTERM`, then `SIGKILL` if it is still there |
//! | the vsock muxer | the VM's muxer binds `{uds_path}` | the **child's** muxer binds the same path (below) |
//!
//! **The exit reason is the child's exit status** ([`super::CHILD_EXIT_SHUTDOWN`]
//! / [`super::CHILD_EXIT_RESET`]), set by `apz-vmm` from the [`VmExit`] its own
//! VM reported. A child killed by a signal — or one that failed — matches
//! neither, so the parent reports [`VmExit::Killed`], never a guest exit it
//! did not observe. **Readiness is one line on stderr**
//! ([`super::CHILD_READY`]): the child prints it once its VM is up, and
//! [`boot`] does not return before it, so a child that cannot boot is a failed
//! [`spawn`](super::spawn) call (with the child's own log in the error) rather
//! than a handle to a VM that does not exist.
//!
//! **The VM control is a second socket** (every VM with a vsock socket path).
//! The child binds `{uds_path}.ctl` ([`control_path`], `--control`) before its
//! readiness line, and the parent makes one exchange over it per call — one
//! request line, one reply line ([`parse_reply`]). The requests are `usable N`
//! and `stats` (the balloon, [`ChildMemory`]) and `pause`, `resume`,
//! `snapshot <full|diff> <dir>`, `capture <full|diff> <dir>` and `write_captured`
//! ([`ChildControl`]); the child answers each from
//! its own VM's control. It lives beside the muxer's socket, so the jail's bind
//! of the UDS directory covers it at the same path. A VM with no vsock socket
//! has no such directory to live in, so it has no control socket in this mode:
//! its pause and snapshot calls fail with [`SpawnError::NoSnapshots`].
//!
//! **Restore and snapshots inside the jail.** `--restore DIR` (the spec's
//! `restore`) is bound read-only at its own path, and snapshots are written
//! under `--snapshot-root DIR`, bound read-write at its own path. The root is
//! the VM's socket directory (the one the jail already binds read-write), so a
//! snapshot directory must be under it; a snapshot request for any other path
//! is refused by the child. Both binds reach the jail through
//! [`crate::jail::jailer::bind_in_every_jail`], because the jail plan is built
//! inside the child's boot, which does not see its command line.
//!
//! **The child dies with its daemon, structurally.** `apz-vmm` arms
//! `PR_SET_PDEATHSIG(SIGKILL)` before it does anything else, so a parent that
//! dies *without* running any stop path — `SIGKILL`, a crash, an OOM kill —
//! takes the child and its guest with it, in the kernel, with no cooperation
//! from either side. That is the backstop; the graceful stop is the two
//! mechanisms above: the parent holds the child's stdin open for the VM's life
//! and closes it, and its stop control asks with `SIGTERM` (then `SIGKILL`).
//! Both are wanted because they cover different failures — a parent that is
//! shutting down *asks*, reads the VM's exit, and lets the guest end its own
//! way, while a parent that has been killed *cannot* ask. The stdin close is
//! not itself a stop: it ends the child's console-input forwarding (its reader
//! returns on `read` → 0) and leaves the guest running — measured, with the
//! backstop absent, a child outlived a `SIGKILL`ed parent with its
//! `plaza.halt` guest still parked — which is exactly why the backstop is a
//! kernel signal and not "the pipe closed".
//!
//! **The signal belongs to the spawning *thread*, so every child is spawned
//! from one thread that lives as long as the daemon.** `prctl(2)`'s parent-death
//! signal watches "the thread that created this process", and measured on
//! Linux 7.2.8 it is exactly that: a child spawned from a thread that then
//! exits is `SIGKILL`ed while its process lives on (this is the defect the
//! supervisor thread fixes — a daemon that boots a VM on an HTTP request thread
//! loses it milliseconds after the response is written). [`boot`] therefore
//! does not call `Command::spawn` on the caller's thread: it hands the built
//! command to [`supervisor`], a single thread created on the first process-mode
//! boot that lives until the process exits, which does the `spawn` and hands
//! the child straight back. The child's parent thread is then the supervisor —
//! alive for the daemon's life — so `PDEATHSIG` fires on the daemon's death
//! (the backstop's real purpose) and *not* on the caller's. The supervisor
//! holds no child after it answers, so it never outlives one while the daemon
//! lives, and there is nothing to shut down but the process itself. The one
//! window neither the backstop nor the supervisor covers is a daemon that dies
//! before the child's `prctl(2)` lands; it is one syscall wide, and it is
//! documented at the child's `arm_parent_death_signal`
//! (`crates/vmm/src/bin/apz-vmm/child.rs`).
//!
//! **The muxer: the child binds its own socket.** `VmSpec::vsock`'s
//! `uds_path` is handed to the child verbatim (`--uds`), and the child's
//! [`crate::builder::boot`] builds the same [`crate::devices::virtio::vsock`]
//! device the in-process shape builds, whose muxer binds that path itself.
//! Nothing about the host side changes: the path is on the host filesystem
//! (inside the jail it is the same directory, bind-mounted at the same
//! absolute path), so a caller's `UnixStream::connect("{uds_path}")` — and a
//! guest's `connect(2, port)` reaching `{uds_path}_{port}` — work as they
//! always did. The alternatives were worse: a passed file descriptor would be
//! `unsafe` surface (`SO_PASSCRED`/`SCM_RIGHTS`) the child does not need, and
//! having the parent bind the listener would mean the CLI-mode [`crate::sys`]
//! socket lands in the *parent*, which is the process the jail is supposed to
//! exclude. The consequence is stated rather than hidden: the socket appears
//! when the child reaches its boot (a moment after
//! [`spawn`](super::spawn) is called) rather than when the caller asked, and
//! the child — not the daemon — owns, unlinks on exit and re-clears on boot
//! the file (the muxer already refuses to take over a socket a live listener
//! owns and clears a stale one: `clear_stale_host_socket`).
//!
//! **What this does not do (stated).** The child inherits the parent's open
//! file descriptors across `exec` (a hardened deployment would close all but
//! the three pipes with a `pre_exec` hook, which is `unsafe`); the parent's
//! helper threads (console reader, waiter) are not themselves in the child's
//! jail — they are the *parent's* threads, reading a pipe and a status, and
//! the confinement the invariants care about is the child's; and there is no
//! pid namespace in the child (the jailer's stated gap, unchanged by this
//! mode).

use crate::cgroup::{VmCgroup, VmLimits};
use apz_sandbox::Sandboxed;
use apz_uds::UnixStream;

use std::io::{Read, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::{
    CHILD_BOOT_TIMEOUT, CHILD_READY, IoStats, MemoryControl, MemoryStats, SnapshotInfo,
    SnapshotKind, SpawnError, VmControl, VmRuntime, VmSpec,
};

/// How many lines of the child's stderr log are kept, so a failed boot's error
/// is its own last words and not an unbounded buffer.
const CHILD_LOG_LINES: usize = 200;

/// How long one control-socket exchange may take — the connect, the request and
/// the reply — before the call fails. A resize is a memory operation inside the
/// child, so two seconds is generous for it, and it bounds a stuck child.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);

/// The budget for `pause` and `resume`: the vCPUs leave `KVM_RUN` and rejoin it,
/// which is milliseconds, so five seconds bounds a stuck child without ever
/// failing a healthy one.
const PAUSE_TIMEOUT: Duration = Duration::from_secs(5);

/// The budget for `snapshot`: the child writes the VM's memory, which is a
/// sequential write of up to the VM's whole memory, so it can take seconds. A
/// request that outlasts this has failed, and the VM is left paused for the
/// caller to resume or stop.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(120);

/// The budget for `reclaim`: the child advises the VM's whole memory out of host RAM, and the
/// kernel may write dirty anonymous pages to swap on the way, so it can take seconds.
const RECLAIM_TIMEOUT: Duration = Duration::from_secs(120);

/// The environment the per-VM process is given — the whole of it. Each name is
/// a confinement switch this crate itself reads: the jailer's mode, base and
/// uid/gid target ([`crate::jail::jailer`]) and the seccomp jail's mode
/// ([`crate::jail`]). A deployment configures the child's confinement through
/// these and nothing else reaches it (see the `env_clear` in [`boot`]).
#[cfg(target_os = "linux")]
const CONFINEMENT_ENV: [&str; 6] = [
    crate::jail::jailer::JAILER_ENV,
    crate::jail::jailer::JAIL_ROOT_ENV,
    crate::jail::jailer::JAIL_UID_ENV,
    crate::jail::jailer::JAIL_GID_ENV,
    crate::jail::JAIL_ENV,
    crate::jail::NO_JAIL_ENV,
];

/// macOS has no jail (the node's sandbox confines the per-VM process), so there is no switch to pass on.
#[cfg(all(unix, not(target_os = "linux")))]
const CONFINEMENT_ENV: [&str; 0] = [];

/// The handle of one VM's child, the same on every platform: a `std` child on Unix (spawned on the supervisor
/// thread, see [`spawn_supervised`]) and an apz-sandbox process on Windows (a job object and an AppContainer).
trait Proc: Send {
    fn pid(&self) -> u32;
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>>;
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>>;
    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>>;
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>>;
    fn wait(&mut self) -> std::io::Result<ExitStatus>;
    fn kill(&mut self) -> std::io::Result<()>;
}

impl Proc for Child {
    fn pid(&self) -> u32 {
        self.id()
    }
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        self.stdin
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Write + Send>)
    }
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>)
    }
    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>)
    }
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }
    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        Child::wait(self)
    }
    fn kill(&mut self) -> std::io::Result<()> {
        Child::kill(self)
    }
}

impl Proc for Sandboxed {
    fn pid(&self) -> u32 {
        Sandboxed::id(self)
    }
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        Sandboxed::stdin(self)
    }
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        Sandboxed::stdout(self)
    }
    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        Sandboxed::stderr(self)
    }
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Sandboxed::try_wait(self)
    }
    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        Sandboxed::wait(self)
    }
    fn kill(&mut self) -> std::io::Result<()> {
        Sandboxed::kill(self)
    }
}

/// A child-spawn request for the supervisor thread: the fully-built command,
/// and the channel the child (or the spawn error) is answered on.
#[cfg(unix)]
struct SpawnRequest {
    command: Command,
    reply: mpsc::Sender<std::io::Result<Child>>,
}

/// The single thread every per-VM child is spawned from: the sender half of
/// its request channel, created on the first process-mode boot (`None` before
/// that) and living until the process exits. It is what makes the child's
/// parent *thread* outlive the caller that asked for the VM — see the module
/// documentation for why that is required, not merely tidy.
#[cfg(unix)]
static SUPERVISOR: Mutex<Option<mpsc::Sender<SpawnRequest>>> = Mutex::new(None);

/// Numbers the cgroups this process creates, so two VMs of one parent never
/// share a name (the VM id alone repeats across boots of the same id).
static CGROUP_SEQ: AtomicUsize = AtomicUsize::new(0);

/// Kills a VM's cgroup, if it still has one. The waiter takes the cgroup once
/// the child is reaped, so after that there is nothing to kill.
#[cfg(unix)]
fn kill_cgroup(cgroup: &Mutex<Option<VmCgroup>>) {
    let guard = cgroup.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(cgroup) = guard.as_ref()
        && let Err(err) = cgroup.kill()
    {
        eprintln!(
            "vmm: cannot kill the VM's cgroup {}: {err}",
            cgroup.path().display()
        );
    }
}

/// Removes a VM's cgroup, if it still has one: kills what is left in it, waits
/// briefly for it to empty, and `rmdir`s it. The `Drop` of the taken cgroup
/// retries the same work if the removal failed.
fn remove_cgroup(cgroup: &Mutex<Option<VmCgroup>>) {
    let taken = cgroup.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(mut cgroup) = taken
        && let Err(err) = cgroup.remove()
    {
        eprintln!(
            "vmm: cannot remove the VM's cgroup {}: {err}",
            cgroup.path().display()
        );
    }
}

/// The supervisor thread's loop: spawn each requested child and answer with
/// it. A caller that vanished before the answer (its receiver dropped) leaves
/// a child with no handle — a VM nothing can stop — so that child is killed
/// and reaped here rather than leaked. The loop ends when the channel closes,
/// which in practice is when the process exits; the thread is deliberately
/// never joined, because it is meant to outlive every VM.
#[cfg(unix)]
fn supervise(requests: mpsc::Receiver<SpawnRequest>) {
    while let Ok(mut request) = requests.recv() {
        let result = request.command.spawn();
        if let Err(mpsc::SendError(result)) = request.reply.send(result)
            && let Ok(mut child) = result
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The supervisor sender, starting the thread on first use. A process that
/// never boots a VM in [`super::SpawnMode::Process`] never starts it.
#[cfg(unix)]
fn supervisor() -> Result<mpsc::Sender<SpawnRequest>, SpawnError> {
    let mut slot = SUPERVISOR.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(sender) = slot.as_ref() {
        return Ok(sender.clone());
    }
    let (sender, requests) = mpsc::channel();
    thread::Builder::new()
        .name("apz_vmm_spawner".to_string())
        .spawn(move || supervise(requests))?;
    *slot = Some(sender.clone());
    Ok(sender)
}

/// Spawns `command` on the supervisor thread and returns the child, so the
/// child's parent *thread* is the supervisor and not the caller. The caller
/// still blocks here until the child exists, so `spawn()` keeps the
/// synchronous shape it had when it called [`Command::spawn`] itself.
#[cfg(unix)]
fn spawn_supervised(command: Command, binary: &std::path::Path) -> Result<Child, SpawnError> {
    let sender = supervisor()?;
    let (reply, answer) = mpsc::channel();
    sender.send(SpawnRequest { command, reply }).map_err(|_| {
        SpawnError::Thread(std::io::Error::other(
            "the per-VM supervisor thread has stopped",
        ))
    })?;
    answer
        .recv()
        .map_err(|_| {
            SpawnError::Thread(std::io::Error::other(
                "the per-VM supervisor thread answered no child",
            ))
        })?
        .map_err(|source| SpawnError::ChildSpawn {
            path: binary.to_path_buf(),
            source,
        })
}

/// Boots one VM in a child `apz-vmm` process, filling `runtime` in the same
/// way a backend does (see [`super::boot`] for the contract). Returns once the
/// child has reported its VM up.
pub(super) fn boot(spec: &VmSpec, runtime: Arc<VmRuntime>) -> Result<(), SpawnError> {
    let binary = super::child_binary()?;
    // The VM's cgroup, when the host has one, is created before the child
    // exists so its limits are in place for the child's first allocation; the
    // child joins it right after the spawn.
    let cgroup = match super::cgroup_root() {
        Some(root) => {
            let name = format!(
                "vm-{}-{}-{}",
                runtime.id,
                std::process::id(),
                CGROUP_SEQ.fetch_add(1, Ordering::SeqCst)
            );
            let limits = VmLimits {
                vcpus: spec.vcpus,
                memory_mib: spec.memory_mib,
            };
            Some(root.create(&name, limits).map_err(SpawnError::backend)?)
        }
        None => None,
    };
    // The spawn runs on the supervisor thread, not here: the child's
    // parent-death signal watches the *thread* that created it (see the module
    // documentation), and this caller may be an ephemeral request thread.
    let mut child = spawn_child(spec, &binary, child_env())?;
    let pid = child.pid();
    // The child joins its cgroup before anything waits on it. A child that
    // cannot join is killed here rather than left running without its limits.
    if let Some(cgroup) = &cgroup
        && let Err(err) = cgroup.add_process(pid)
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(SpawnError::backend(err));
    }

    // The three pipes are the whole interface. Missing one is not possible
    // after a successful `spawn` with all three requested, but it is reported
    // rather than unwrapped.
    let stdin = take(child.take_stdin(), &binary, "stdin")?;
    let stdout = take(child.take_stdout(), &binary, "stdout")?;
    let stderr = take(child.take_stderr(), &binary, "stderr")?;

    // The child's log (shared with the runtime, so `VmHandle::process_log`
    // shows it), its readiness gate, and the thread that reads both.
    let log = Arc::clone(&runtime.process_log);
    let (ready_tx, ready_rx) = mpsc::channel::<bool>();
    {
        let log = Arc::clone(&log);
        let spawned = thread::Builder::new()
            .name(format!("apz_vm{}_child_log", runtime.id))
            .spawn(move || read_child_stderr(stderr, &log, &ready_tx));
        if let Err(source) = spawned {
            fail_start(child.as_mut(), source)?;
        }
    }

    // The readiness gate: one line while the VM comes up, or the child's own
    // death. A child that neither reports nor dies is a stuck boot, bounded by
    // the timeout so `spawn` cannot hang on it.
    let ready = ready_rx.recv_timeout(CHILD_BOOT_TIMEOUT);
    match ready {
        Ok(true) => {}
        Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => {
            // Its stderr ended without a readiness line: it has failed, and is
            // on its way out — but a `try_wait` first means a child that closed
            // its stderr without exiting cannot hang this call in `wait`.
            let status = match child.try_wait() {
                Ok(Some(status)) => status.to_string(),
                _ => {
                    let _ = child.kill();
                    child
                        .wait()
                        .map(|status| status.to_string())
                        .unwrap_or_else(|_| "killed".to_string())
                }
            };
            return Err(SpawnError::ChildBoot {
                id: runtime.id,
                status,
                log: child_log(&log),
            });
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SpawnError::ChildBoot {
                id: runtime.id,
                status: format!("still running after {}s", CHILD_BOOT_TIMEOUT.as_secs()),
                log: child_log(&log),
            });
        }
    }

    // The VM is up and the process exists: from here the handle's surfaces are
    // the child's.
    runtime.set_process_id(pid);

    // The VM controls, one exchange with the child's control socket per call
    // (see [`ChildMemory`] and [`ChildControl`]). The pause/snapshot control is
    // every VM's; the memory control is a balloon's, as the in-process shape
    // installs it. A VM with no control socket installs nothing.
    if let Some(path) = control_path(spec) {
        if spec.balloon.is_some() {
            runtime.set_memory_control(Arc::new(ChildMemory {
                path: path.clone(),
                max_mib: u32::try_from(spec.memory_mib).unwrap_or(u32::MAX),
            }));
        }
        runtime.set_vm_control(Arc::new(ChildControl { path }));
    }

    // The guest's serial stream: the child's stdout. The reader blocks in
    // `read`, so it forwards each chunk as it arrives and ends when the child
    // closes the pipe (when it exits).
    let reader = {
        let serial = Arc::clone(&runtime.serial);
        thread::Builder::new()
            .name(format!("apz_vm{}_console", runtime.id))
            .spawn(move || {
                let mut stdout = stdout;
                let mut buffer = [0u8; 4096];
                loop {
                    match stdout.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => serial
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .extend_from_slice(&buffer[..n]),
                        Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            })
    };
    let reader = match reader {
        Ok(reader) => reader,
        Err(source) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SpawnError::Thread(source));
        }
    };
    runtime.add_thread(reader);

    // The console input hook: a caller's bytes for the guest's `ttyS0` go to
    // the child's stdin, which its own reader hands to the UART. The hook
    // holds the pipe, so a write after the child exited fails — which is what
    // `serial_input` must report.
    let input = Arc::new(Mutex::new(stdin));
    runtime.set_input_hook(move |data| {
        let mut stdin = input.lock().unwrap_or_else(PoisonError::into_inner);
        stdin
            .write_all(data)
            .and_then(|()| stdin.flush())
            .map_err(|err| format!("the per-VM process is not taking console input: {err}"))
    });

    // The stop control and the waiter share the child: the stop control asks it
    // to end, and the waiter reaps it and publishes how the VM ended, so
    // `exit_reason` reads the same in both modes.
    let cgroup = Arc::new(Mutex::new(cgroup));
    let gone = Arc::new(AtomicBool::new(false));
    let slot: Arc<Mutex<Option<Box<dyn Proc>>>> = Arc::new(Mutex::new(Some(child)));
    #[cfg(unix)]
    runtime.set_stop_control(unix_stop_control(
        pid,
        Arc::clone(&gone),
        Arc::clone(&cgroup),
    ));
    #[cfg(windows)]
    runtime.set_stop_control(windows_stop_control(Arc::clone(&gone), Arc::clone(&slot)));

    // The waiter: the last thread, and the one `VmHandle::wait` joins.
    let waiter = {
        let runtime = Arc::clone(&runtime);
        let gone = Arc::clone(&gone);
        let slot = Arc::clone(&slot);
        let cgroup = Arc::clone(&cgroup);
        thread::Builder::new()
            .name(format!("apz_vm{}_child", runtime.id))
            .spawn(move || {
                let Some(status) = wait_for_child(&slot) else {
                    return;
                };
                gone.store(true, Ordering::SeqCst);
                match status.map(|status| status.code()) {
                    Ok(Some(super::CHILD_EXIT_SHUTDOWN)) => runtime.mark_guest_exited(),
                    Ok(Some(super::CHILD_EXIT_RESET)) => runtime.mark_guest_reset(),
                    // A signal, a failure status, or a `wait` error: the VM was
                    // not stopped by its guest, so no guest exit is published.
                    _ => {}
                }
                // The child is reaped: its cgroup goes now, with whatever the
                // VM left in it.
                remove_cgroup(&cgroup);
            })
    };
    match waiter {
        Ok(waiter) => {
            runtime.add_thread(waiter);
            Ok(())
        }
        Err(source) => {
            // The thread never started, so the child is still in the slot:
            // kill and reap it, so a failed `spawn` leaves no process behind.
            gone.store(true, Ordering::SeqCst);
            if let Some(mut child) = slot.lock().unwrap_or_else(PoisonError::into_inner).take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            remove_cgroup(&cgroup);
            Err(SpawnError::Thread(source))
        }
    }
}

/// Starts the VM's child with `env` as its whole environment: a `Command` on Unix, spawned on the supervisor
/// thread (see [`spawn_supervised`]); on Windows an apz-sandbox process confined by [`monitor_sandbox`].
#[cfg(unix)]
fn spawn_child(
    spec: &VmSpec,
    binary: &Path,
    env: Vec<(String, String)>,
) -> Result<Box<dyn Proc>, SpawnError> {
    let mut command = Command::new(binary);
    command
        .args(child_args(spec))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.env_clear();
    for (name, value) in env {
        command.env(name, value);
    }
    Ok(Box::new(spawn_supervised(command, binary)?))
}

/// See the Unix version: the Windows child is confined by its job object and AppContainer, and a
/// [`monitor_sandbox`] names what it may read and write.
#[cfg(windows)]
fn spawn_child(
    spec: &VmSpec,
    binary: &Path,
    env: Vec<(String, String)>,
) -> Result<Box<dyn Proc>, SpawnError> {
    let sandbox = monitor_sandbox(binary, spec, env)?;
    let child = apz_sandbox::spawn(&sandbox, Stdio::piped(), Stdio::piped(), Stdio::piped())
        .map_err(SpawnError::backend)?;
    Ok(Box::new(child))
}

/// The child's whole environment. A Unix child gets the confinement switches a deployment set and nothing
/// else: no host variable (a token, a path, a secret) rides into the VM's process (invariant 2, extended to
/// the process that holds the guest). A Windows child gets only `SystemRoot`, which the system DLLs need.
#[cfg(unix)]
fn child_env() -> Vec<(String, String)> {
    CONFINEMENT_ENV
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| ((*name).to_string(), value))
        })
        .collect()
}

#[cfg(windows)]
fn child_env() -> Vec<(String, String)> {
    std::env::var("SystemRoot")
        .ok()
        .map(|value| vec![("SystemRoot".to_string(), value)])
        .unwrap_or_default()
}

/// The OS sandbox of a Windows monitor process (`apz-vmm`): the kernel, the initrd, the data volumes and a
/// restore directory are read-only; the state directory (the vsock and control sockets), the workspace image
/// and the snapshot root are writable; the process has no network and one process; its memory cap is the
/// guest's RAM plus the monitor's overhead ([`crate::confine::monitor_spec`]). The state directory is the
/// socket directory of the VM, so a VM without a vsock socket has no directory to name and is refused.
#[cfg(any(windows, test))]
fn monitor_sandbox(
    binary: &Path,
    spec: &VmSpec,
    env: Vec<(String, String)>,
) -> Result<apz_sandbox::Spec, SpawnError> {
    let uds = spec.vsock.as_ref().map(|vsock| vsock.uds_path.clone());
    let state_dir = uds
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| SpawnError::SpawnMode {
            reason: "a Windows monitor process needs the VM's vsock socket: its directory is the one the process may write".to_string(),
        })?;
    let mut read_only = vec![spec.kernel.clone()];
    read_only.extend(spec.initrd.clone());
    read_only.extend(spec.volumes.iter().map(|volume| volume.image.clone()));
    read_only.extend(spec.restore.clone());
    let mut read_write = vec![state_dir.clone()];
    read_write.extend(spec.workspace.clone());
    read_write.extend(snapshot_root(spec));
    let memory = u64::try_from(spec.memory_mib)
        .unwrap_or(u64::MAX)
        .saturating_mul(1 << 20);
    Ok(crate::confine::monitor_spec(
        binary,
        child_args(spec),
        env,
        &state_dir,
        &read_only,
        &read_write,
        memory,
    ))
}

/// The stop control of a Unix child: `SIGTERM` at the first request and `SIGKILL` at the second (the spawner
/// repeats a request that did not take). The escalation to `SIGKILL` also ends the VM's whole cgroup. A child
/// already reaped is not signalled: its pid may have been reused.
#[cfg(unix)]
fn unix_stop_control(
    pid: u32,
    gone: Arc<AtomicBool>,
    cgroup: Arc<Mutex<Option<VmCgroup>>>,
) -> impl Fn() + Send + Sync + 'static {
    let signals = AtomicUsize::new(0);
    move || {
        if gone.load(Ordering::SeqCst) {
            return;
        }
        let signal = if signals.fetch_add(1, Ordering::SeqCst) == 0 {
            libc::SIGTERM
        } else {
            libc::SIGKILL
        };
        if let Err(err) = send_signal(pid as i32, signal) {
            // `ESRCH` here is the VM having stopped between the two checks;
            // anything else is reported rather than swallowed, because a VM
            // that cannot be signalled is a VM that cannot be killed.
            if err.raw_os_error() != Some(libc::ESRCH) {
                eprintln!("vmm: cannot signal the per-VM process {pid}: {err}");
            }
        }
        if signal == libc::SIGKILL {
            kill_cgroup(&cgroup);
        }
    }
}

/// The stop control of a Windows child. Windows has no `SIGTERM` to ask for a graceful end and the monitor
/// handles none, so the child is terminated at once; the guest does not get its shutdown.
#[cfg(windows)]
fn windows_stop_control(
    gone: Arc<AtomicBool>,
    slot: Arc<Mutex<Option<Box<dyn Proc>>>>,
) -> impl Fn() + Send + Sync + 'static {
    move || {
        if gone.load(Ordering::SeqCst) {
            return;
        }
        let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(child) = guard.as_mut()
            && let Err(err) = child.kill()
        {
            eprintln!("vmm: cannot stop the per-VM process: {err}");
        }
    }
}

/// Waits for the VM's child and returns its status; `None` when the slot is already empty. On Unix the child is
/// taken out and waited on (a blocking `waitpid`, which the stop control's signals end). On Windows it is polled
/// where it is, because the stop control must reach the handle while the VM runs.
fn wait_for_child(slot: &Mutex<Option<Box<dyn Proc>>>) -> Option<std::io::Result<ExitStatus>> {
    #[cfg(unix)]
    {
        let child = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
        let mut child = child?;
        Some(child.wait())
    }
    #[cfg(windows)]
    {
        const POLL: Duration = Duration::from_millis(10);
        loop {
            let polled = {
                let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
                guard.as_mut()?.try_wait()
            };
            match polled {
                Ok(Some(status)) => return Some(Ok(status)),
                Ok(None) => thread::sleep(POLL),
                Err(err) => return Some(Err(err)),
            }
        }
    }
}

/// The child's command line: one VM's [`VmSpec`], field for field.
///
/// `--uds` names the **exact** host socket path the child's vsock muxer binds
/// (`VmSpec::vsock.uds_path`, which the caller chose), not a directory the
/// child would have to reconstruct a path from: the parent and the child must
/// agree on one path, and re-deriving `{dir}/vm{cid}` from a directory would
/// silently disagree with a caller that named anything else.
///
/// The VM's *id* is deliberately not on this command line: the child holds
/// exactly one VM, so every name it makes is id `0` (`apz_vm0_vsock`, the jail
/// root's `-0-`), while the parent's handle keeps the caller's id. Nothing
/// crosses the pipes that depends on the two agreeing.
pub fn child_args(spec: &VmSpec) -> Vec<String> {
    let mut args = vec![
        "--kernel".to_string(),
        spec.kernel.display().to_string(),
        "--cmdline".to_string(),
        spec.cmdline.clone(),
        "--vcpus".to_string(),
        spec.vcpus.to_string(),
        "--memory-mib".to_string(),
        spec.memory_mib.to_string(),
    ];
    if let Some(initrd) = &spec.initrd {
        args.push("--initrd".to_string());
        args.push(initrd.display().to_string());
    }
    if let Some(workspace) = &spec.workspace {
        args.push("--workspace".to_string());
        args.push(workspace.display().to_string());
    }
    // One `--volume NAME=IMAGE` per shared data volume, in the spec's order.
    for volume in &spec.volumes {
        args.push("--volume".to_string());
        args.push(format!("{}={}", volume.name, volume.image.display()));
    }
    if let Some(usable) = spec.balloon {
        args.push("--balloon-usable-mib".to_string());
        args.push(usable.to_string());
    }
    if let Some(gic) = spec.gic {
        args.push("--gic".to_string());
        args.push(
            match gic {
                super::GicVersion::V2 => "v2",
                super::GicVersion::V3 => "v3",
            }
            .to_string(),
        );
    }
    if let Some(vsock) = &spec.vsock {
        args.push("--cid".to_string());
        args.push(vsock.guest_cid.to_string());
        args.push("--uds".to_string());
        args.push(vsock.uds_path.display().to_string());
    }
    if let Some(control) = control_path(spec) {
        args.push("--control".to_string());
        args.push(control.display().to_string());
    }
    if let Some(restore) = &spec.restore {
        args.push("--restore".to_string());
        args.push(restore.display().to_string());
    }
    if let Some(root) = snapshot_root(spec) {
        args.push("--snapshot-root".to_string());
        args.push(root.display().to_string());
    }
    args
}

/// The control socket's path for a VM: `{uds_path}.ctl`, beside the muxer's
/// socket, so the jail's read-write bind of the UDS directory covers it at the
/// same path. Every VM with a vsock socket path has one; a VM with no vsock has
/// no directory to put it in, so it has none.
fn control_path(spec: &VmSpec) -> Option<PathBuf> {
    let vsock = spec.vsock.as_ref()?;
    let mut path = vsock.uds_path.clone().into_os_string();
    path.push(".ctl");
    Some(PathBuf::from(path))
}

/// The directory a process-mode VM's snapshots are written under: the daemon's snapshot root
/// ([`super::set_snapshot_root`]) when it named one, else the VM's socket directory, which the jail
/// binds read-write already. A relative path has no absolute directory to bind, so such a VM takes
/// no snapshots.
fn snapshot_root(spec: &VmSpec) -> Option<PathBuf> {
    if let Some(root) = super::snapshot_root().filter(|root| root.is_absolute()) {
        return Some(root);
    }
    let dir = spec.vsock.as_ref()?.uds_path.parent()?;
    dir.is_absolute().then(|| dir.to_path_buf())
}

/// The word a snapshot request and reply name a [`SnapshotKind`] by.
pub fn snapshot_kind_word(kind: SnapshotKind) -> &'static str {
    match kind {
        SnapshotKind::Full => "full",
        SnapshotKind::Diff => "diff",
        SnapshotKind::DiffUnsynced => "diff_unsynced",
    }
}

/// The [`SnapshotKind`] a request or reply word names, `None` for any other word.
pub fn parse_snapshot_kind(word: &str) -> Option<SnapshotKind> {
    match word {
        "full" => Some(SnapshotKind::Full),
        "diff" => Some(SnapshotKind::Diff),
        "diff_unsynced" => Some(SnapshotKind::DiffUnsynced),
        _ => None,
    }
}

/// One reply line of the child's control socket.
#[derive(Debug, PartialEq, Eq)]
enum Reply {
    /// `ok`: the resize, pause or resume took.
    Ok,
    /// `ok <kind> <memory_written> <paused_ms>`: the snapshot was written, and the
    /// kind it holds (which may differ from the one asked for, see
    /// [`SnapshotKind`]).
    Snapshot {
        kind: SnapshotKind,
        memory_written: u64,
        paused_ms: u64,
    },
    /// `ok <bytes>`: a reclaim took, and the child advised this many bytes of guest memory out of
    /// host RAM; or a `write_captured` wrote this many bytes of guest memory.
    Bytes(u64),
    /// `err <message>`: the child refused or could not carry out the request.
    Err(String),
    /// `stats …`: the balloon's counters.
    Stats(MemoryStats),
    /// `io …`: the VM's disk and vsock totals.
    Io(IoStats),
}

/// Parses one reply line: `ok`, `ok <bytes>` (a reclaim, a `write_captured`), `ok <kind>
/// <memory_written> <paused_ms>` (a snapshot or a capture), `err <message>`, or `stats <max> <usable>
/// <held> <free|-> <available|->`.
/// A line that is none of these is an error, never a guess.
fn parse_reply(line: &str) -> Result<Reply, String> {
    let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
    match verb {
        "ok" if rest.is_empty() => Ok(Reply::Ok),
        "ok" if !rest.contains(' ') => rest
            .parse::<u64>()
            .map(Reply::Bytes)
            .map_err(|_| format!("the reclaim reply {rest:?} is not a number of bytes")),
        "ok" => parse_snapshot(rest),
        "err" => Ok(Reply::Err(rest.to_string())),
        "stats" => parse_stats(rest).map(Reply::Stats),
        "io" => parse_io(rest).map(Reply::Io),
        _ => Err(format!(
            "the control socket answered {line:?}, which is not a reply"
        )),
    }
}

/// The three fields of a snapshot reply: the kind, the bytes written, the pause.
fn parse_snapshot(fields: &str) -> Result<Reply, String> {
    let words: Vec<&str> = fields.split(' ').collect();
    let [kind, memory_written, paused_ms] = words.as_slice() else {
        return Err(format!(
            "the snapshot reply {fields:?} does not have three fields"
        ));
    };
    let kind = parse_snapshot_kind(kind)
        .ok_or_else(|| format!("the snapshot reply's kind {kind:?} is not full or diff"))?;
    let number = |text: &str, name: &str| {
        text.parse::<u64>()
            .map_err(|_| format!("the snapshot reply's {name} {text:?} is not a number"))
    };
    Ok(Reply::Snapshot {
        kind,
        memory_written: number(memory_written, "memory written")?,
        paused_ms: number(paused_ms, "pause")?,
    })
}

/// The five fields of an `io` reply: the disk bytes read and written, the disk requests, then the vsock bytes
/// received and sent.
fn parse_io(fields: &str) -> Result<IoStats, String> {
    fn number(text: &str, name: &str) -> Result<u64, String> {
        text.parse()
            .map_err(|_| format!("the io reply's {name} {text:?} is not a number"))
    }
    let words: Vec<&str> = fields.split(' ').collect();
    let [read, write, ops, rx, tx] = words.as_slice() else {
        return Err(format!("the io reply {fields:?} does not have five fields"));
    };
    Ok(IoStats {
        blk_read_bytes: number(read, "disk read")?,
        blk_write_bytes: number(write, "disk write")?,
        blk_ops: number(ops, "disk requests")?,
        vsock_rx_bytes: number(rx, "vsock in")?,
        vsock_tx_bytes: number(tx, "vsock out")?,
    })
}

/// The five fields of a `stats` reply: the three counters, then the two optional
/// ones, which the child writes as `-` when the guest has not reported them.
fn parse_stats(fields: &str) -> Result<MemoryStats, String> {
    fn number<T: std::str::FromStr>(text: &str, name: &str) -> Result<T, String> {
        text.parse()
            .map_err(|_| format!("the stats reply's {name} {text:?} is not a number"))
    }
    fn optional(text: &str, name: &str) -> Result<Option<u64>, String> {
        if text == "-" {
            Ok(None)
        } else {
            number(text, name).map(Some)
        }
    }
    let words: Vec<&str> = fields.split(' ').collect();
    let [max, usable, held, free, available] = words.as_slice() else {
        return Err(format!(
            "the stats reply {fields:?} does not have five fields"
        ));
    };
    Ok(MemoryStats {
        max_mib: number(max, "max")?,
        usable_mib: number(usable, "usable")?,
        held_mib: number(held, "held")?,
        free_mib: optional(free, "free")?,
        available_mib: optional(available, "available")?,
    })
}

/// Connects to the Unix socket at `path`, and gives up after `timeout`.
///
/// A Unix-socket `connect` has no timeout of its own, so the socket is made
/// non-blocking and the connect is retried every 10 ms on the answers that mean
/// "not accepting yet": Linux refuses a full backlog with `EAGAIN`, macOS with
/// `ECONNREFUSED` while the listener exists, and `EINPROGRESS` is waited on
/// ([`finish_connect`]). A missing path and any other error return at once.
/// No thread is started, so none is left behind when the deadline passes.
#[cfg(windows)]
fn connect_within(path: &Path, timeout: Duration) -> std::io::Result<UnixStream> {
    const RETRY: Duration = Duration::from_millis(10);
    let deadline = Instant::now() + timeout;
    loop {
        // A socket the monitor has not bound yet refuses or is missing: retry until the deadline.
        match UnixStream::connect(path) {
            Ok(stream) => return Ok(stream),
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(err) => return Err(err),
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(timed_out());
        }
        thread::sleep(RETRY.min(left));
    }
}

#[cfg(unix)]
fn connect_within(path: &Path, timeout: Duration) -> std::io::Result<UnixStream> {
    const RETRY: Duration = Duration::from_millis(10);
    let (addr, len) = sockaddr_un(path)?;
    let deadline = Instant::now() + timeout;
    loop {
        // A fresh socket per attempt, since a refused connect is not reused.
        let fd = new_socket()?;
        match connect_once(&fd, &addr, len) {
            Ok(()) => return blocking(fd),
            Err(err) => match err.raw_os_error() {
                Some(libc::EINPROGRESS) => return finish_connect(fd, deadline),
                Some(libc::EAGAIN | libc::ECONNREFUSED) => {}
                Some(libc::EINTR) => continue,
                _ => return Err(err),
            },
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(timed_out());
        }
        thread::sleep(RETRY.min(left));
    }
}

fn timed_out() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "the connect did not complete in time",
    )
}

/// The `sockaddr_un` for `path`, and its length. A path that leaves no room for
/// the terminating NUL in `sun_path` (`SUN_LEN`) is refused.
/// Sends `signal` to the child `pid` (`kill(2)`): the portable form of what the supervisor needs,
/// with the errno kept for the `ESRCH` check the caller makes.
#[cfg(unix)]
#[allow(unsafe_code)]
fn send_signal(pid: libc::pid_t, signal: libc::c_int) -> std::io::Result<()> {
    // SAFETY: `kill` takes a pid and a signal number and touches no memory.
    let rc = unsafe { libc::kill(pid, signal) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn sockaddr_un(path: &Path) -> std::io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    use std::os::unix::ffi::OsStrExt;

    let bytes = path.as_os_str().as_bytes();
    // SAFETY: sockaddr_un is plain old data, so all-zero bytes are a valid value;
    // every field the kernel reads is set below.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= addr.sun_path.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "the socket path {} is too long for sun_path",
                path.display()
            ),
        ));
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (slot, byte) in addr.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as libc::c_char;
    }
    let len = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        addr.sun_len = len as u8;
    }
    Ok((addr, len as libc::socklen_t))
}

/// A new close-on-exec, non-blocking Unix stream socket.
#[cfg(unix)]
#[allow(unsafe_code)]
fn new_socket() -> std::io::Result<OwnedFd> {
    #[cfg(target_os = "linux")]
    let kind = libc::SOCK_STREAM | libc::SOCK_CLOEXEC;
    #[cfg(not(target_os = "linux"))]
    let kind = libc::SOCK_STREAM;
    // SAFETY: `socket` takes plain integers; a non-negative result is a new
    // descriptor that nothing else holds.
    let raw = unsafe { libc::socket(libc::AF_UNIX, kind, 0) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `raw` was just returned by `socket`, is open, and is handed over
    // to `fd` alone, which closes it.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    #[cfg(not(target_os = "linux"))]
    {
        // SAFETY: `fd` is open; FD_CLOEXEC is a descriptor flag and takes no pointer.
        let rc = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        if rc < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    // SAFETY: `fd` is open; F_GETFL takes no pointer.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` is open; the new flags are the current ones plus O_NONBLOCK.
    let rc = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(fd)
}

/// One non-blocking `connect(2)` of `fd` to `addr`.
#[cfg(unix)]
#[allow(unsafe_code)]
fn connect_once(
    fd: &OwnedFd,
    addr: &libc::sockaddr_un,
    len: libc::socklen_t,
) -> std::io::Result<()> {
    // SAFETY: `fd` is an open socket we own, and `addr` holds `len` initialized
    // bytes of a sockaddr_un; both outlive the call.
    let rc = unsafe { libc::connect(fd.as_raw_fd(), std::ptr::from_ref(addr).cast(), len) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Waits, until `deadline`, for the connect in progress on `fd` to finish, then
/// returns the stream, or the error the socket reports (`SO_ERROR`).
#[cfg(unix)]
#[allow(unsafe_code)]
fn finish_connect(fd: OwnedFd, deadline: Instant) -> std::io::Result<UnixStream> {
    let mut waiting = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLOUT,
        revents: 0,
    };
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let ms = libc::c_int::try_from(left.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: `waiting` is one valid pollfd for the call, and `fd` stays open
        // until this function returns.
        let ready = unsafe { libc::poll(&mut waiting, 1, ms) };
        if ready > 0 {
            break;
        }
        if ready == 0 {
            return Err(timed_out());
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EINTR) {
            return Err(err);
        }
    }
    let stream = UnixStream::from(fd);
    if let Some(err) = stream.take_error()? {
        return Err(err);
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

/// The connected socket, switched back to blocking mode: the caller bounds each
/// I/O step with its own timeouts.
#[cfg(unix)]
fn blocking(fd: OwnedFd) -> std::io::Result<UnixStream> {
    let stream = UnixStream::from(fd);
    stream.set_nonblocking(false)?;
    Ok(stream)
}

/// One exchange with the child's control socket: connect, send `request` as one
/// line, read the reply line. `timeout` bounds the connect and each I/O step, so
/// a request that the child is still working on (a snapshot) gets its budget.
///
/// The connect is bounded by `timeout` and starts no thread ([`connect_within`]),
/// so a listener that stops accepting cannot hold the caller past it, and no
/// detached thread is left blocked in `connect` when it does.
fn exchange(path: &Path, request: &str, timeout: Duration) -> Result<String, String> {
    use std::io::BufRead;

    let stream = connect_within(path, timeout).map_err(|err| {
        if err.kind() == std::io::ErrorKind::TimedOut {
            format!(
                "the control socket {} did not accept within {}s",
                path.display(),
                timeout.as_secs()
            )
        } else {
            format!("cannot reach the control socket {}: {err}", path.display())
        }
    })?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|err| format!("cannot time the control socket exchange: {err}"))?;
    let mut writer = &stream;
    writeln!(writer, "{request}")
        .map_err(|err| format!("cannot write to the control socket: {err}"))?;
    let mut reply = String::new();
    std::io::BufReader::new(&stream)
        .read_line(&mut reply)
        .map_err(|err| format!("cannot read the control socket's reply: {err}"))?;
    Ok(reply.trim_end().to_string())
}

/// The memory control of a VM in its own process: each call is one exchange
/// over the child's control socket, which the child serves from its own balloon
/// (`apz-vmm --control`).
struct ChildMemory {
    path: PathBuf,
    max_mib: u32,
}

impl MemoryControl for ChildMemory {
    fn set_usable_mib(&self, mib: u32) -> Result<(), SpawnError> {
        let reply = exchange(&self.path, &format!("usable {mib}"), CONTROL_TIMEOUT)
            .and_then(|line| parse_reply(&line));
        match reply {
            Ok(Reply::Ok) => Ok(()),
            Ok(Reply::Err(reason)) | Err(reason) => Err(SpawnError::MemoryControl { reason }),
            Ok(other) => Err(SpawnError::MemoryControl {
                reason: format!("the control socket answered a resize with {other:?}"),
            }),
        }
    }

    fn stats(&self) -> MemoryStats {
        match exchange(&self.path, "stats", CONTROL_TIMEOUT).and_then(|line| parse_reply(&line)) {
            Ok(Reply::Stats(stats)) => stats,
            // A child that cannot be asked reports nothing: zeros and no reading,
            // never a stale or invented one. The maximum is the VM's own spec.
            _ => MemoryStats {
                max_mib: self.max_mib,
                usable_mib: 0,
                held_mib: 0,
                free_mib: None,
                available_mib: None,
            },
        }
    }

    fn io_stats(&self) -> Option<IoStats> {
        match exchange(&self.path, "io", CONTROL_TIMEOUT).and_then(|line| parse_reply(&line)) {
            Ok(Reply::Io(io)) => Some(io),
            // A child that cannot be asked reports no reading, never zeros that would look like a quiet disk.
            _ => None,
        }
    }
}

/// The pause, resume and snapshot control of a VM in its own process: each call
/// is one exchange over the child's control socket, which the child answers from
/// its own VM's control (`apz-vmm --control`).
struct ChildControl {
    path: PathBuf,
}

impl ChildControl {
    /// One request whose only good answer is `ok`.
    fn acknowledged(&self, request: &str, timeout: Duration) -> Result<(), SpawnError> {
        match self.request(request, timeout)? {
            Reply::Ok => Ok(()),
            other => Err(unexpected(request, &other)),
        }
    }

    fn request(&self, request: &str, timeout: Duration) -> Result<Reply, SpawnError> {
        match exchange(&self.path, request, timeout).and_then(|line| parse_reply(&line)) {
            Ok(Reply::Err(reason)) | Err(reason) => Err(SpawnError::Snapshot { reason }),
            Ok(reply) => Ok(reply),
        }
    }
}

/// The error for a reply that does not answer the request that was sent.
fn unexpected(request: &str, reply: &Reply) -> SpawnError {
    SpawnError::Snapshot {
        reason: format!("the control socket answered {request:?} with {reply:?}"),
    }
}

impl VmControl for ChildControl {
    fn pause(&self) -> Result<(), SpawnError> {
        self.acknowledged("pause", PAUSE_TIMEOUT)
    }

    fn resume(&self) -> Result<(), SpawnError> {
        self.acknowledged("resume", PAUSE_TIMEOUT)
    }

    fn reclaim(&self) -> Result<u64, SpawnError> {
        match self.request("reclaim", RECLAIM_TIMEOUT)? {
            Reply::Bytes(bytes) => Ok(bytes),
            other => Err(unexpected("reclaim", &other)),
        }
    }

    fn snapshot(&self, dir: &Path, kind: SnapshotKind) -> Result<SnapshotInfo, SpawnError> {
        self.snapshot_request("snapshot", dir, kind)
    }

    fn capture(&self, dir: &Path, kind: SnapshotKind) -> Result<SnapshotInfo, SpawnError> {
        self.snapshot_request("capture", dir, kind)
    }

    fn write_captured(&self) -> Result<u64, SpawnError> {
        match self.request("write_captured", SNAPSHOT_TIMEOUT)? {
            Reply::Bytes(bytes) => Ok(bytes),
            other => Err(unexpected("write_captured", &other)),
        }
    }
}

impl ChildControl {
    /// One `snapshot` or `capture` request (`verb`): the child answers with the snapshot's three
    /// fields.
    fn snapshot_request(
        &self,
        verb: &str,
        dir: &Path,
        kind: SnapshotKind,
    ) -> Result<SnapshotInfo, SpawnError> {
        // The directory is the rest of the request line, so it must be one
        // absolute line of text: a relative path or a line break would not reach
        // the child as the directory the caller named.
        let Some(text) = dir
            .to_str()
            .filter(|text| Path::new(text).is_absolute() && !text.contains(['\n', '\r']))
        else {
            return Err(SpawnError::Snapshot {
                reason: format!(
                    "the snapshot directory {} must be an absolute path with no line break",
                    dir.display()
                ),
            });
        };
        let request = format!("{verb} {} {text}", snapshot_kind_word(kind));
        match self.request(&request, SNAPSHOT_TIMEOUT)? {
            Reply::Snapshot {
                kind,
                memory_written,
                paused_ms,
            } => Ok(SnapshotInfo {
                dir: dir.to_path_buf(),
                kind,
                memory_written,
                paused_ms,
            }),
            other => Err(unexpected(&request, &other)),
        }
    }
}

/// The child's stderr reader: it forwards every line to the child's log, and
/// the readiness line's arrival (or the child's last words, `false`) to the
/// parent.
fn read_child_stderr(
    stderr: Box<dyn Read + Send>,
    log: &Mutex<Vec<String>>,
    ready: &mpsc::Sender<bool>,
) {
    use std::io::BufRead;

    let mut reader = std::io::BufReader::new(stderr);
    let mut line = String::new();
    let mut reported = false;
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let text = line.trim_end_matches(['\n', '\r']).to_string();
                if !reported && text == CHILD_READY {
                    reported = true;
                    let _ = ready.send(true);
                }
                let mut log = log.lock().unwrap_or_else(PoisonError::into_inner);
                if log.len() < CHILD_LOG_LINES {
                    log.push(text);
                }
            }
        }
    }
    if !reported {
        // The child's stderr ended without a readiness line: it is gone (or on
        // its way), and its last words are the error.
        let _ = ready.send(false);
    }
}

/// The child's log, as the tail of it, for an error message.
fn child_log(log: &Mutex<Vec<String>>) -> String {
    let log = log.lock().unwrap_or_else(PoisonError::into_inner);
    if log.is_empty() {
        return "no output".to_string();
    }
    log.join("\n")
}

/// A pipe that a successful `spawn` should have provided.
fn take<T>(pipe: Option<T>, binary: &std::path::Path, which: &str) -> Result<T, SpawnError> {
    pipe.ok_or_else(|| SpawnError::ChildSpawn {
        path: binary.to_path_buf(),
        source: std::io::Error::other(format!("the per-VM process's {which} pipe is missing")),
    })
}

/// Kills and reaps a child after a failure in the parent's own setup, so a
/// failed `spawn` leaves no process behind.
fn fail_start(child: &mut dyn Proc, source: std::io::Error) -> Result<(), SpawnError> {
    let _ = child.kill();
    let _ = child.wait();
    Err(SpawnError::Thread(source))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::spawner::{DataVolume, GicVersion, VsockConfig};
    use std::io::BufRead;
    use std::os::unix::net::UnixListener;

    /// A socket path for one test, with nothing left at it.
    fn test_socket(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("apz-connect-{}-{name}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    /// A listener that never accepts, with a backlog of zero: one connect waits in
    /// the queue and the next is refused with `EAGAIN`.
    #[allow(unsafe_code)]
    fn listener_with_zero_backlog(path: &Path) -> OwnedFd {
        let (addr, len) = sockaddr_un(path).expect("a short test path");
        let fd = new_socket().expect("a socket");
        // SAFETY: `fd` is an open socket and `addr` holds `len` initialized bytes;
        // both outlive the call.
        let rc = unsafe { libc::bind(fd.as_raw_fd(), std::ptr::from_ref(&addr).cast(), len) };
        assert_eq!(rc, 0, "bind: {}", std::io::Error::last_os_error());
        // SAFETY: `fd` is a bound socket we own.
        let rc = unsafe { libc::listen(fd.as_raw_fd(), 0) };
        assert_eq!(rc, 0, "listen: {}", std::io::Error::last_os_error());
        fd
    }

    /// A binary that exits 0 at once, where this platform keeps `true` (macOS has no `/bin/true`).
    const TRUE_BINARY: &str = if cfg!(target_os = "macos") {
        "/usr/bin/true"
    } else {
        "/bin/true"
    };

    // macOS queues a connect past a zero backlog instead of refusing it (measured), so there is no
    // full backlog to give up on there.
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "macOS never refuses a connect for a full unix-socket backlog"
    )]
    // `connect_within` starts no thread (it retries a non-blocking connect), so nothing is left behind when it gives
    // up; this pins that it gives up in time.
    fn connect_within_gives_up_on_a_full_backlog_in_time() {
        let path = test_socket("full");
        let _listener = listener_with_zero_backlog(&path);
        let (addr, len) = sockaddr_un(&path).expect("a short test path");
        // Fill the backlog: connects succeed until one is refused with EAGAIN.
        let mut queued = Vec::new();
        let refused = (0..16).any(|_| {
            let fd = new_socket().expect("a socket");
            match connect_once(&fd, &addr, len) {
                Ok(()) => {
                    queued.push(fd);
                    false
                }
                Err(err) => err.raw_os_error() == Some(libc::EAGAIN),
            }
        });
        assert!(refused, "a full backlog should refuse with EAGAIN");

        let started = Instant::now();
        let err = connect_within(&path, Duration::from_millis(200))
            .expect_err("a full backlog must not connect");
        let took = started.elapsed();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        // The upper bound only catches a hang: an emulated or loaded runner is slow.
        assert!(
            (Duration::from_millis(200)..Duration::from_secs(10)).contains(&took),
            "connect_within took {took:?}"
        );
        drop(queued);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn connect_within_reaches_a_listener_and_exchanges_a_line() {
        let path = test_socket("normal");
        let listener = UnixListener::bind(&path).expect("bind");
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut line = String::new();
            std::io::BufReader::new(&stream)
                .read_line(&mut line)
                .expect("read");
            writeln!(&stream, "echo {}", line.trim_end()).expect("write");
        });
        let mut stream = connect_within(&path, Duration::from_secs(2)).expect("connect");
        writeln!(stream, "ping").expect("write");
        let mut reply = String::new();
        std::io::BufReader::new(&stream)
            .read_line(&mut reply)
            .expect("read");
        assert_eq!(reply, "echo ping\n");
        server.join().expect("server thread");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn connect_within_refuses_a_missing_path_at_once() {
        let path = test_socket("missing");
        let started = Instant::now();
        let err = connect_within(&path, Duration::from_secs(2)).expect_err("no socket there");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(started.elapsed() < Duration::from_millis(500));

        let long = std::env::temp_dir().join("x".repeat(200));
        let err = connect_within(&long, Duration::from_secs(2))
            .expect_err("a path too long for sun_path");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    /// Every `VmSpec` field reaches the child's command line, including the
    /// exact vsock socket path (never a directory the child re-derives) and the
    /// control socket beside it.
    #[test]
    fn the_child_gets_the_whole_spec() {
        let spec = VmSpec {
            kernel: PathBuf::from("/boot/vmlinuz-linux"),
            cmdline: "console=ttyS0 plaza.keepalive=1".to_string(),
            vcpus: 2,
            memory_mib: 768,
            initrd: Some(PathBuf::from("/tmp/initrd.gz")),
            gic: Some(GicVersion::V2),
            vsock: Some(VsockConfig {
                guest_cid: 42,
                uds_path: PathBuf::from("/tmp/plaza-vsock/vm42"),
            }),
            workspace: Some(PathBuf::from("/tmp/run/workspace-0.ext4")),
            balloon: Some(32),
            restore: None,
            volumes: vec![
                DataVolume {
                    name: "data-a".to_string(),
                    image: PathBuf::from("/tmp/data/a.ext4"),
                },
                DataVolume {
                    name: "data-b".to_string(),
                    image: PathBuf::from("/tmp/data/b.ext4"),
                },
            ],
            ..Default::default()
        };
        let args = child_args(&spec);
        let pairs: Vec<(&str, &str)> = args
            .chunks(2)
            .filter_map(|pair| match pair {
                [flag, value] => Some((flag.as_str(), value.as_str())),
                _ => None,
            })
            .collect();
        for (flag, value) in [
            ("--kernel", "/boot/vmlinuz-linux"),
            ("--cmdline", "console=ttyS0 plaza.keepalive=1"),
            ("--vcpus", "2"),
            ("--memory-mib", "768"),
            ("--initrd", "/tmp/initrd.gz"),
            ("--gic", "v2"),
            ("--cid", "42"),
            ("--uds", "/tmp/plaza-vsock/vm42"),
            ("--control", "/tmp/plaza-vsock/vm42.ctl"),
            ("--workspace", "/tmp/run/workspace-0.ext4"),
            ("--balloon-usable-mib", "32"),
            ("--volume", "data-a=/tmp/data/a.ext4"),
            ("--volume", "data-b=/tmp/data/b.ext4"),
        ] {
            assert!(
                pairs.contains(&(flag, value)),
                "{flag} {value} is not on the command line: {pairs:?}"
            );
        }
        // The optional fields are absent when the spec has none of them.
        let bare = child_args(&VmSpec::default());
        for absent in [
            "--initrd",
            "--gic",
            "--cid",
            "--uds",
            "--control",
            "--workspace",
            "--balloon-usable-mib",
            "--volume",
        ] {
            assert!(!bare.iter().any(|arg| arg == absent), "{absent} leaked");
        }
    }

    /// The control socket's replies parse to what the child meant, and anything
    /// else is refused rather than guessed at.
    #[test]
    fn the_control_replies_parse() {
        assert_eq!(parse_reply("ok"), Ok(Reply::Ok));
        assert_eq!(
            parse_reply("err this VM has no balloon"),
            Ok(Reply::Err("this VM has no balloon".to_string()))
        );
        assert_eq!(
            parse_reply("stats 512 256 200 - 180"),
            Ok(Reply::Stats(MemoryStats {
                max_mib: 512,
                usable_mib: 256,
                held_mib: 200,
                free_mib: None,
                available_mib: Some(180),
            }))
        );
        assert_eq!(
            parse_reply("io 1024 512 3 40 20"),
            Ok(Reply::Io(IoStats {
                blk_read_bytes: 1024,
                blk_write_bytes: 512,
                blk_ops: 3,
                vsock_rx_bytes: 40,
                vsock_tx_bytes: 20,
            }))
        );
        assert_eq!(
            parse_reply("ok diff 4096 37"),
            Ok(Reply::Snapshot {
                kind: SnapshotKind::Diff,
                memory_written: 4096,
                paused_ms: 37,
            })
        );
        assert_eq!(parse_reply("ok 8192"), Ok(Reply::Bytes(8192)));
        for bad in [
            "",
            "okay",
            "ok extra",
            "ok -1",
            "ok 1 2",
            "ok full 4096",
            "ok sparse 1 2",
            "ok full x 2",
            "stats 512 256",
            "stats 512 x 200 - -",
            "usable 5",
        ] {
            assert!(parse_reply(bad).is_err(), "{bad:?} is not a reply");
        }
    }

    /// Every snapshot kind names itself on the wire and parses back to itself, `DiffUnsynced`
    /// included, and a fork's request for it is not refused by the child.
    #[test]
    fn the_snapshot_kinds_round_trip_the_protocol() {
        for kind in [
            SnapshotKind::Full,
            SnapshotKind::Diff,
            SnapshotKind::DiffUnsynced,
        ] {
            assert_eq!(parse_snapshot_kind(snapshot_kind_word(kind)), Some(kind));
        }
        assert_eq!(parse_snapshot_kind("diff unsynced"), None);
        assert!(!SnapshotKind::DiffUnsynced.is_durable());
        assert!(SnapshotKind::Diff.is_durable());
    }

    /// The restore directory and the snapshot root reach the child's command line,
    /// and a VM whose socket path is relative has no absolute directory to bind
    /// for snapshots, so it gets no snapshot root.
    #[test]
    fn restore_and_the_snapshot_root_reach_the_child() {
        let vsock = |uds_path: &str| {
            Some(VsockConfig {
                guest_cid: 42,
                uds_path: PathBuf::from(uds_path),
            })
        };
        let spec = VmSpec {
            restore: Some(PathBuf::from("/var/snap/vm42")),
            vsock: vsock("/tmp/plaza-vsock/vm42"),
            ..VmSpec::default()
        };
        let args = child_args(&spec);
        let has = |flag: &str, value: &str| {
            args.windows(2)
                .any(|pair| pair[0] == flag && pair[1] == value)
        };
        assert!(has("--restore", "/var/snap/vm42"), "{args:?}");
        assert!(has("--snapshot-root", "/tmp/plaza-vsock"), "{args:?}");
        assert!(has("--control", "/tmp/plaza-vsock/vm42.ctl"), "{args:?}");

        let relative = VmSpec {
            vsock: vsock("vm42"),
            ..VmSpec::default()
        };
        assert!(
            !child_args(&relative)
                .iter()
                .any(|arg| arg == "--snapshot-root")
        );
        // The control socket still sits beside the relative socket path.
        assert!(child_args(&relative).iter().any(|arg| arg == "vm42.ctl"));
        assert!(
            !child_args(&VmSpec::default())
                .iter()
                .any(|arg| arg == "--restore")
        );
    }

    /// A stand-in child: it accepts one connection per reply, reads the request
    /// line, and answers with the next canned reply. It returns the requests.
    fn serve_replies(
        listener: UnixListener,
        replies: Vec<&'static str>,
    ) -> thread::JoinHandle<Vec<String>> {
        thread::spawn(move || {
            let mut requests = Vec::new();
            for reply in replies {
                let (stream, _) = listener.accept().expect("a connection");
                let mut request = String::new();
                std::io::BufReader::new(&stream)
                    .read_line(&mut request)
                    .expect("a request line");
                requests.push(request.trim_end().to_string());
                writeln!(&stream, "{reply}").expect("the reply");
            }
            requests
        })
    }

    /// The parent's pause, resume and snapshot calls make one request line each
    /// and read the child's answer: `ok` acknowledges, the snapshot's three
    /// fields become a [`SnapshotInfo`], and an `err` is an error. A snapshot
    /// directory that is not one absolute line is refused without a connection.
    #[test]
    fn the_child_control_speaks_the_request_protocol() {
        let dir = test_dir("child-control");
        let path = dir.join("vm.ctl");
        let listener = UnixListener::bind(&path).expect("a control socket");
        let server = serve_replies(
            listener,
            vec![
                "ok",
                "ok",
                "ok 8192",
                "ok full 4096 37",
                "err this VM cannot be paused or snapshotted on this backend",
                "ok diff 0 5",
            ],
        );
        let control = ChildControl { path: path.clone() };

        control.pause().expect("pause is acknowledged");
        control.resume().expect("resume is acknowledged");
        assert_eq!(control.reclaim().expect("reclaim is answered"), 8192);
        let full = control
            .snapshot(Path::new("/var/snap/a"), SnapshotKind::Full)
            .expect("the snapshot is written");
        assert_eq!(
            full,
            SnapshotInfo {
                dir: PathBuf::from("/var/snap/a"),
                kind: SnapshotKind::Full,
                memory_written: 4096,
                paused_ms: 37,
            }
        );
        assert!(matches!(
            control.snapshot(Path::new("/var/snap/b"), SnapshotKind::Full),
            Err(SpawnError::Snapshot { .. })
        ));
        let diff = control
            .snapshot(Path::new("/var/snap/c"), SnapshotKind::Diff)
            .expect("the diff is written");
        assert_eq!(diff.kind, SnapshotKind::Diff);

        // Refused before any connection: the server would otherwise be waiting.
        assert!(
            control
                .snapshot(Path::new("relative"), SnapshotKind::Full)
                .is_err()
        );
        assert!(
            control
                .snapshot(Path::new("/a\nb"), SnapshotKind::Full)
                .is_err()
        );

        assert_eq!(
            server.join().expect("the stand-in child ends"),
            vec![
                "pause".to_string(),
                "resume".to_string(),
                "reclaim".to_string(),
                "snapshot full /var/snap/a".to_string(),
                "snapshot full /var/snap/b".to_string(),
                "snapshot diff /var/snap/c".to_string(),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A capture is one `capture <kind> <dir>` request, answered with a snapshot's three fields.
    /// `write_captured` is one request answered `ok <bytes>`; any other shape of answer is an error.
    #[test]
    fn the_child_control_captures_and_writes_the_capture() {
        let dir = test_dir("child-capture");
        let path = dir.join("vm.ctl");
        let listener = UnixListener::bind(&path).expect("a control socket");
        let server = serve_replies(listener, vec!["ok diff 0 9", "ok 4096", "ok full 4096 12"]);
        let control = ChildControl { path: path.clone() };

        let captured = control
            .capture(Path::new("/var/snap/a"), SnapshotKind::Diff)
            .expect("the capture is kept");
        assert_eq!(
            captured,
            SnapshotInfo {
                dir: PathBuf::from("/var/snap/a"),
                kind: SnapshotKind::Diff,
                memory_written: 0,
                paused_ms: 9,
            }
        );
        assert_eq!(
            control.write_captured().expect("the capture is written"),
            4096
        );
        assert!(
            control.write_captured().is_err(),
            "a write answered with a snapshot's fields is an error"
        );
        // Refused before any connection, as a snapshot is.
        assert!(
            control
                .capture(Path::new("relative"), SnapshotKind::Full)
                .is_err()
        );

        assert_eq!(
            server.join().expect("the stand-in child ends"),
            vec![
                "capture diff /var/snap/a".to_string(),
                "write_captured".to_string(),
                "write_captured".to_string(),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A scratch directory for a socket test, emptied first.
    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-process-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a test directory");
        dir
    }

    /// The supervisor is one long-lived thread that is *not* the caller's: the
    /// thread that first needs it can exit and a later `spawn` from another
    /// thread is still answered. That is the property the per-VM child's
    /// parent thread depends on — if the spawn ran on the caller's thread,
    /// that thread's exit would `SIGKILL` the child. Two requests also reuse
    /// the one thread rather than leaving a thread behind each.
    #[test]
    fn the_supervisor_outlives_its_first_caller_and_is_reused() {
        // A caller thread starts the supervisor and exits.
        std::thread::spawn(|| {
            supervisor().expect("the supervisor starts");
        })
        .join()
        .expect("the caller thread ends");

        // A later request from this (different) thread is still served: the
        // supervisor did not die with the caller that first needed it.
        for _ in 0..2 {
            let mut child =
                spawn_supervised(Command::new(TRUE_BINARY), std::path::Path::new(TRUE_BINARY))
                    .expect("the supervisor spawns for a later caller");
            assert!(
                child.wait().expect("the child is reapable").success(),
                "{TRUE_BINARY} exits 0"
            );
        }
    }
}

/// The Windows monitor's sandbox spec, built on every host: what the spec grants and caps is the same whichever
/// platform enforces it.
#[cfg(test)]
mod sandbox_tests {
    use super::*;
    use crate::spawner::{DataVolume, VsockConfig};

    fn vm() -> VmSpec {
        VmSpec {
            kernel: PathBuf::from("/agentplaza/guest/x86_64/vmlinux"),
            cmdline: "console=ttyS0".to_string(),
            vcpus: 1,
            memory_mib: 512,
            initrd: Some(PathBuf::from("/agentplaza/guest/x86_64/initrd.gz")),
            vsock: Some(VsockConfig {
                guest_cid: 3,
                uds_path: PathBuf::from("/agentplaza/state/vsock/vm3"),
            }),
            workspace: Some(PathBuf::from("/agentplaza/state/vm3/workspace.ext4")),
            volumes: vec![DataVolume {
                name: "repo".to_string(),
                image: PathBuf::from("/agentplaza/data/repo.ext4"),
            }],
            ..VmSpec::default()
        }
    }

    #[test]
    fn the_images_and_data_are_read_only_and_the_socket_directory_is_writable() {
        let spec = monitor_sandbox(
            Path::new("/agentplaza/bin/apz-vmm"),
            &vm(),
            vec![("SystemRoot".to_string(), "/windows".to_string())],
        )
        .unwrap();
        assert_eq!(
            spec.read_only,
            vec![
                PathBuf::from("/agentplaza/guest/x86_64/vmlinux"),
                PathBuf::from("/agentplaza/guest/x86_64/initrd.gz"),
                PathBuf::from("/agentplaza/data/repo.ext4"),
            ]
        );
        assert!(
            spec.read_write
                .contains(&PathBuf::from("/agentplaza/state/vsock"))
        );
        assert!(
            spec.read_write
                .contains(&PathBuf::from("/agentplaza/state/vm3/workspace.ext4"))
        );
        assert_eq!(spec.workdir, PathBuf::from("/agentplaza/state/vsock"));
        assert_eq!(spec.program, PathBuf::from("/agentplaza/bin/apz-vmm"));
        assert_eq!(spec.env.len(), 1);
    }

    #[test]
    fn the_monitor_runs_the_child_command_line_and_is_capped_at_its_ram_plus_overhead() {
        let spec =
            monitor_sandbox(Path::new("/agentplaza/bin/apz-vmm"), &vm(), Vec::new()).unwrap();
        assert_eq!(spec.args, child_args(&vm()));
        assert_eq!(spec.network, apz_sandbox::Network::None);
        assert_eq!(
            spec.limits.memory_bytes,
            Some((512u64 << 20) + crate::confine::MONITOR_OVERHEAD_BYTES)
        );
        assert_eq!(spec.limits.processes, Some(1));
    }

    #[test]
    fn a_vm_without_a_socket_has_no_directory_to_name_and_is_refused() {
        let mut spec = vm();
        spec.vsock = None;
        let err =
            monitor_sandbox(Path::new("/agentplaza/bin/apz-vmm"), &spec, Vec::new()).unwrap_err();
        assert!(err.to_string().contains("vsock"), "{err}");
    }
}
