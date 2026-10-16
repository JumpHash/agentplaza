// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// The per-VM process body (Linux). Its dispatcher — and the reason a *binary*
// is the shape — is `../apz-vmm.rs`; `vmm::spawner::process` is the parent
// half. Read either of those first.
//
// What this program is: one microVM, in one process, booted through the same
// `vmm::spawner`/`vmm::builder` code `noded` and `vmctl` call. It adds
// nothing to the VMM; it adds the *process* the jailer needs. The jailer is
// entered inside `spawner::spawn` (it is a step of `builder::boot`), at a
// moment when this process has exactly one thread — so an unprivileged run on
// a host without any jail privilege still gets a per-VM root, a mount
// namespace and a `pivot_root`, which no in-daemon VM can.
//
// **Dying with its parent.** The first thing this process does is arm
// `PR_SET_PDEATHSIG(SIGKILL)` ([`arm_parent_death_signal`]), so the kernel
// kills it if the process that spawned it dies *without* asking — the
// structural backstop behind the normal stop path (the parent's `SIGTERM`,
// and the stdin close). The race that leaves, and what the stdin EOF does and
// does not do, are stated at that function.
//
// The three streams are the contract with the parent
// (`vmm::spawner::process::boot`):
//
// * **stdout** — the guest's serial console, as it is produced. Raw bytes, no
//   framing: the parent appends them to the VM handle's serial capture, so
//   `VmHandle::serial` in the parent sees exactly what the in-process shape
//   would have captured.
// * **stderr** — this program's own log. [`CHILD_READY`] is written once the
//   VM is up, and it is the parent's readiness gate; diagnostics and the exit
//   line follow. Keeping the two apart is what makes "the VM is up" a fact the
//   parent can rely on rather than something it has to guess from console
//   output.
// * **stdin** — console input, forwarded to the guest's `ttyS0` through the
//   same `VmHandle::serial_input` the in-process shape uses.
//
// The **exit status** is the exit reason ([`CHILD_EXIT_SHUTDOWN`] /
// [`CHILD_EXIT_RESET`]), so a parent learns how the VM ended from the kernel's
// own report of the child rather than from a message this program could get
// wrong.
//
// The **control socket** (`--control PATH`, a VM with a balloon only) is the
// one channel the parent uses to resize the VM's memory while it runs: one
// request line per connection (`usable N` or `stats`), one reply line back.
// See [`serve_control`].

// Syscall boundary: this binary's own unsafe allow entry (see
// `../apz-vmm.rs`, which is `deny(unsafe_code)`). `PR_SET_PDEATHSIG` is one
// `prctl(2)` call that no safe-Rust interface exposes, and it has to run in
// *this* process — it is this process's parent that dies. It is the only
// `unsafe` in the binary, its block carries the `// SAFETY:` note, and the
// crate's library keeps the same posture with the allowlist in `lib.rs`.
#![allow(unsafe_code)]

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::Duration;

use vmm::jail::jailer::{self, Access};
use vmm::spawner::process::{parse_snapshot_kind, snapshot_kind_word};
use vmm::spawner::{
    CHILD_EXIT_FAILED, CHILD_EXIT_RESET, CHILD_EXIT_SHUTDOWN, CHILD_EXIT_USAGE, CHILD_PDEATHSIG,
    CHILD_READY, GicVersion, MemoryControl, SpawnError, SpawnMode, VmControl, VmExit, VmRuntime,
    VmSpec, VsockConfig,
};

const USAGE: &str = "\
usage: apz-vmm --kernel PATH --cmdline STR [--initrd PATH] [--workspace IMAGE]
               [--volume NAME=IMAGE]... [--memory-mib M] [--vcpus V] [--cid N] [--uds PATH]
               [--gic v2|v3] [--balloon-usable-mib N] [--control PATH]
               [--restore DIR] [--snapshot-root DIR]

Boots exactly one microVM in this process, jailed on its own boot path, and
serves it:
  stdout  the guest's serial console (raw bytes)
  stderr  this program's log, starting with the readiness line
  stdin   console input, forwarded to the guest's serial line

  --kernel PATH      guest kernel image (uncompressed ELF vmlinux or bzImage)
  --cmdline STR      kernel command line
  --initrd PATH      optional initrd image
  --workspace IMAGE  optional writable virtio-blk volume (the guest's only
                     writable device)
  --volume NAME=IMAGE
                     shared read-only data volume, repeatable (at most 4); the
                     guest mounts it at /data/NAME
  --memory-mib M     guest memory in MiB (default 512)
  --vcpus V          vCPUs (default 1)
  --cid N            vsock guest CID; --uds names the host socket it uses
  --uds PATH         the host Unix socket the VM's vsock muxer binds (the
                     daemon's convention is {uds_dir}/vm{cid})
  --gic v2|v3        aarch64 interrupt controller (ignored on x86_64)
  --balloon-usable-mib N
                     with a balloon, the memory the guest may use at boot
  --control PATH     a Unix socket (mode 0600) the parent drives the VM through:
                     one request line per connection, one reply line back.
                     `usable N` and `stats` (a balloon), `pause`, `resume`,
                     `snapshot full|diff DIR` and `capture full|diff DIR` (DIR
                     under --snapshot-root), and `write_captured`.
  --restore DIR      restore the VM from the snapshot in DIR instead of booting
                     the kernel (DIR is bound read-only in the jail)
  --snapshot-root DIR
                     the absolute directory snapshots are written under (bound
                     read-write in the jail)
  -h, --help         print this help

The exit status is the VM's: 0 the guest shut down, 3 the guest reset, 1 a
failure, 2 a refused command line.";

/// How many bytes one console-input read takes from stdin: the UART's input
/// queue is 64 bytes and `serial_input` refuses a slice it cannot take whole.
const CONSOLE_INPUT_CHUNK: usize = 64;

/// How long the console-input thread waits before retrying a slice the guest
/// has not drained yet.
const CONSOLE_INPUT_RETRY: Duration = Duration::from_millis(10);

/// How many times in a row a console-input slice is retried before it is
/// dropped, so a guest that stopped reading cannot build an unbounded backlog.
const CONSOLE_INPUT_ATTEMPTS: usize = 100;

/// How long one control-socket connection may take to send its request and
/// read the reply, so a stuck client cannot hold the control thread.
const CONTROL_IO_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the control thread waits after an accept error before trying again.
const CONTROL_RETRY: Duration = Duration::from_millis(10);

pub(crate) fn main() -> ExitCode {
    // Before anything else — before the command line is even read — so the
    // window in which this process has no parent-death backstop is as small as
    // a process can make it. See [`arm_parent_death_signal`] for the race that
    // window is, and for what this does *not* replace.
    if let Err(err) = arm_parent_death_signal() {
        // Fail closed. A process that cannot promise to die with the parent
        // that spawned it is not one to hold a VM in: the parent is entitled
        // to assume that when it dies, the VM goes with it.
        eprintln!("apz-vmm: cannot arm the parent-death signal: {err}");
        return ExitCode::from(CHILD_EXIT_FAILED as u8);
    }
    eprintln!("{CHILD_PDEATHSIG}");
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let launch = match parse(&args) {
        Ok(launch) => launch,
        Err(err) => {
            eprintln!("apz-vmm: {err}");
            eprintln!("{USAGE}");
            return ExitCode::from(CHILD_EXIT_USAGE as u8);
        }
    };
    match run(launch) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("apz-vmm: {err}");
            ExitCode::from(CHILD_EXIT_FAILED as u8)
        }
    }
}

/// The signal taken when the process that spawned this one dies: `SIGKILL`,
/// and deliberately the only one. The parent's own stop path is a graceful
/// `SIGTERM` *first* (`vmm::spawner::process`), so a `SIGKILL` here means the
/// parent had no chance to ask — and there is nothing left to negotiate with
/// on the other end of the pipes.
const PARENT_DEATH_SIGNAL: libc::c_int = libc::SIGKILL;

/// Arms the parent-death backstop: `prctl(PR_SET_PDEATHSIG, SIGKILL)`, so this
/// process is killed by the kernel the moment the process that spawned it dies
/// — however it dies (a `SIGKILL`ed daemon, a crash, an OOM kill, an exit that
/// skipped the stop path).
///
/// **The race, exactly.** The signal is delivered only for a parent that dies
/// *after* this call: a parent that dies in the window between the
/// `fork`/`clone` that made this process and this `prctl` leaves it with no
/// signal and no way to notice — `getppid(2)` becoming 1 is the only trace,
/// and nothing here polls it. Arming first is what makes that window one
/// syscall wide, and no ordering makes it zero, because the arming has to
/// happen in this process, after the parent exists.
///
/// The `execve(2)` that got here is not part of that window: the setting is
/// *not* cleared by `execve` in general (measured on Linux 7.2.8: a `prctl`ed
/// value survived an `execve` into an unrelated program and still fired) — it
/// is cleared on `fork(2)`, and, since 2.4.36/2.6.23, when executing a
/// set-user-ID/set-group-ID binary or one with file capabilities. This
/// process's parent `exec`s it, so nothing is inherited from the parent's own
/// state, and the arm happens after that exec: arm-then-serve is the ordering
/// that matters.
///
/// **Whose death counts.** The watched parent is "the thread that created this
/// process" (`prctl(2)`), and measured on Linux 7.2.8 it is exactly that: the
/// signal was delivered when the *thread* that spawned the child exited, even
/// though its process lived on — a parent process staying alive is not enough.
/// So the spawner spawns every child from one long-lived supervisor thread
/// (`vmm::spawner::process`), never from a caller's thread, and a daemon that
/// boots a VM on an ephemeral request thread keeps its VM. The backstop's
/// purpose is unchanged: when the daemon *process* dies, every thread in it —
/// the supervisor included — is gone, so the kernel still kills the child.
///
/// **What it is not.** This neither replaces the graceful path nor stops the
/// guest the way `VmHandle::kill` does. The parent holds this process's stdin
/// open for the VM's life and asks it to stop with `SIGTERM` (then `SIGKILL`);
/// closing stdin only ends console-input forwarding — [`forward_console_input`]
/// sees `read` → 0 and returns, and the *guest keeps running* (measured: with
/// this signal absent, a child outlived a `SIGKILL`ed parent with its
/// `plaza.halt` guest still parked). So the two cover different failures:
/// `SIGTERM` is the asked-for stop — the guest ends its own way and the parent
/// reads the VM's exit — while this is the backstop for a parent that could
/// not ask, and `SIGKILL` is the right backstop because there is nothing left
/// to negotiate with.
fn arm_parent_death_signal() -> std::io::Result<()> {
    // SAFETY: `PR_SET_PDEATHSIG` takes the signal as an integer in its second
    // argument and reads no memory; the remaining variadic slots are unused
    // and passed as 0, as the kernel requires. The call touches no pointer and
    // no other thread.
    let rc = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, PARENT_DEATH_SIGNAL, 0, 0, 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// What the command line asks for: the one VM's specification, the control
/// socket's path if the command line named one, and the directory snapshots go
/// under if it named that.
///
/// There is no VM *id* here: this process holds exactly one VM, so the VM is
/// id `0` in every name this process makes (the VMM thread names, the jail
/// root). The parent's own handle keeps the caller's id; nothing crosses the
/// pipe that depends on the two agreeing.
#[derive(Debug)]
struct Launch {
    spec: VmSpec,
    control: Option<PathBuf>,
    snapshot_root: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Launch, String> {
    let mut kernel: Option<PathBuf> = None;
    let mut cmdline: Option<String> = None;
    let mut initrd: Option<PathBuf> = None;
    let mut workspace: Option<PathBuf> = None;
    let mut memory_mib: Option<usize> = None;
    let mut vcpus: Option<u8> = None;
    let mut cid: Option<u64> = None;
    let mut uds: Option<PathBuf> = None;
    let mut gic: Option<GicVersion> = None;
    let mut balloon: Option<u32> = None;
    let mut control: Option<PathBuf> = None;
    let mut restore: Option<PathBuf> = None;
    let mut snapshot_root: Option<PathBuf> = None;
    let mut volumes: Vec<vmm::spawner::DataVolume> = Vec::new();

    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        match flag {
            "--kernel" => kernel = Some(PathBuf::from(value(args, &mut index, flag)?)),
            "--cmdline" => cmdline = Some(value(args, &mut index, flag)?.to_string()),
            "--initrd" => initrd = Some(PathBuf::from(value(args, &mut index, flag)?)),
            "--workspace" => workspace = Some(PathBuf::from(value(args, &mut index, flag)?)),
            "--volume" => volumes.push(parse_volume(value(args, &mut index, flag)?)?),
            "--memory-mib" => {
                let text = value(args, &mut index, flag)?;
                memory_mib = Some(
                    text.parse()
                        .map_err(|_| format!("{flag} {text:?} is not a number of MiB"))?,
                );
            }
            "--vcpus" => {
                let text = value(args, &mut index, flag)?;
                let count: u32 = text
                    .parse()
                    .map_err(|_| format!("{flag} {text:?} is not a number of vCPUs"))?;
                vcpus = Some(
                    u8::try_from(count)
                        .map_err(|_| format!("{flag} {text:?} is more vCPUs than 8 bits"))?,
                );
            }
            "--cid" => {
                let text = value(args, &mut index, flag)?;
                cid = Some(
                    text.parse()
                        .map_err(|_| format!("{flag} {text:?} is not a CID"))?,
                );
            }
            "--balloon-usable-mib" => {
                let text = value(args, &mut index, flag)?;
                balloon = Some(
                    text.parse()
                        .map_err(|_| format!("{flag} {text:?} is not a number of MiB"))?,
                );
            }
            "--uds" => uds = Some(PathBuf::from(value(args, &mut index, flag)?)),
            "--control" => control = Some(PathBuf::from(value(args, &mut index, flag)?)),
            "--restore" => restore = Some(PathBuf::from(value(args, &mut index, flag)?)),
            "--snapshot-root" => {
                let root = PathBuf::from(value(args, &mut index, flag)?);
                // Snapshot requests name directories under this root by their
                // absolute path, so a relative root could never match one.
                if !root.is_absolute() {
                    return Err(format!("{flag} {} is not an absolute path", root.display()));
                }
                snapshot_root = Some(root);
            }
            "--gic" => {
                let text = value(args, &mut index, flag)?;
                gic = Some(match text {
                    "v2" => GicVersion::V2,
                    "v3" => GicVersion::V3,
                    other => return Err(format!("{flag} {other:?} is not v2 or v3")),
                });
            }
            other => return Err(format!("{other:?} is not a flag this program takes")),
        }
        index += 1;
    }

    vmm::spawner::check_volumes(&volumes)?;

    // The vsock device is one thing named by two flags: a CID without a socket
    // path would leave the guest unreachable and a path without a CID would
    // boot a device whose config space says nothing.
    let vsock = match (cid, uds) {
        (Some(guest_cid), Some(uds_path)) => Some(VsockConfig {
            guest_cid,
            uds_path,
        }),
        (None, None) => None,
        (Some(_), None) => return Err("--cid needs --uds (the socket it listens on)".into()),
        (None, Some(_)) => return Err("--uds needs --cid (the guest it reaches)".into()),
    };

    // The two optional numbers default to the spec's own, so this binary and
    // `noded` cannot disagree about a VM's shape.
    let defaults = VmSpec::default();
    let spec = VmSpec {
        kernel: kernel.ok_or("--kernel is required")?,
        cmdline: cmdline.ok_or("--cmdline is required")?,
        vcpus: vcpus.unwrap_or(defaults.vcpus),
        memory_mib: memory_mib.unwrap_or(defaults.memory_mib),
        initrd,
        gic,
        vsock,
        workspace,
        balloon,
        restore,
        volumes,
    };
    Ok(Launch {
        spec,
        control,
        snapshot_root,
    })
}

/// A `--volume NAME=IMAGE` value: the name is everything before the first `=`, the image is
/// everything after it. Both parts must be present; the name is checked by `check_volumes`.
fn parse_volume(text: &str) -> Result<vmm::spawner::DataVolume, String> {
    match text.split_once('=') {
        Some((name, image)) if !name.is_empty() && !image.is_empty() => {
            Ok(vmm::spawner::DataVolume {
                name: name.to_string(),
                image: PathBuf::from(image),
            })
        }
        _ => Err(format!("--volume {text:?} is not NAME=IMAGE")),
    }
}

/// The value of `flag`: the argument after it, or why it is missing.
fn value<'a>(args: &'a [String], index: &mut usize, flag: &str) -> Result<&'a str, String> {
    *index += 1;
    args.get(*index)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// Boots the VM, serves its console, and returns its exit status.
fn run(launch: Launch) -> Result<ExitCode, String> {
    let Launch {
        spec,
        control,
        snapshot_root,
    } = launch;
    // This process exists to hold one VM: it can never take the per-VM process
    // mode itself, whatever the operator's environment says — that would be
    // `apz-vmm` spawning `apz-vmm` without end.
    vmm::spawner::set_spawn_mode(SpawnMode::InProcess);

    // What the VM reads and writes outside its own images must be bound into
    // the jail the boot builds, at the path it is named by: the restore
    // directory read-only, the snapshot root read-write. See
    // `jailer::bind_in_every_jail`.
    if let Some(restore) = &spec.restore {
        jailer::bind_in_every_jail(restore, Access::ReadOnly);
    }
    if let Some(root) = &snapshot_root {
        jailer::bind_in_every_jail(root, Access::ReadWrite);
    }

    let mut handle = vmm::spawner::spawn(spec).map_err(|err| err.to_string())?;

    // The control socket, when the command line asked for one: bound before the
    // readiness line, so the parent never holds a handle to a VM whose socket is
    // not there yet. Each request is answered from what this VM has: a balloon,
    // a pause and snapshot control, or neither (see [`answer`]).
    if let Some(path) = control {
        let controls = Controls {
            memory: handle.runtime().memory_control(),
            vm: handle.runtime().vm_control(),
            snapshot_root,
        };
        serve_control(path, controls)?;
    }

    // The VM is up. This line is the parent's readiness gate; the jailer has
    // already reported itself on stderr by now (it runs inside the boot), so
    // an operator reading the log sees the jail before this.
    eprintln!("{CHILD_READY}");

    let stop = Arc::new(AtomicBool::new(false));
    let ready = handle.serial_ready();
    let serial = {
        let stop = Arc::clone(&stop);
        let ready = Arc::clone(&ready);
        let sink = handle.serial_sink();
        thread::Builder::new()
            .name("apz_vmm_console_out".to_string())
            .spawn(move || stream_serial(sink, ready, stop))
            .map_err(|err| format!("cannot start the serial stream: {err}"))?
    };
    // The console-input thread is deliberately not joined: it sits in a
    // blocking read of stdin, which the parent holds open for as long as the
    // VM lives, so joining it would hang this program after the VM stopped.
    // It is a daemon of this process: the exit below ends it.
    {
        let stop = Arc::clone(&stop);
        let runtime = handle.runtime();
        thread::Builder::new()
            .name("apz_vmm_console_in".to_string())
            .spawn(move || forward_console_input(runtime, stop))
            .map_err(|err| format!("cannot start the console reader: {err}"))?;
    }

    let exit = handle.wait().map_err(|err| err.to_string())?;
    // The VM stopped: let the serial stream drain what is left, then report the
    // exit the guest asked for. The flag is set under the serial sink's lock, so
    // the serial thread cannot sit between its check and its wait when the
    // notify goes out (that would lose the wakeup until its 1 s timeout).
    {
        let sink = handle.serial_sink();
        let _guard = sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        stop.store(true, Ordering::SeqCst);
    }
    ready.notify_all();
    let _ = serial.join();
    match exit {
        VmExit::GuestExited { reset } => {
            eprintln!(
                "apz-vmm: the guest {} the VM",
                if reset { "reset" } else { "shut down" }
            );
            Ok(ExitCode::from(if reset {
                CHILD_EXIT_RESET as u8
            } else {
                CHILD_EXIT_SHUTDOWN as u8
            }))
        }
        // Nothing in this process kills its own VM, so this is the stop control
        // of a caller that made the VM stop *without* the guest exiting — which
        // the parent reports as a kill, not a guest exit.
        VmExit::Killed => {
            eprintln!("apz-vmm: the VM was stopped by the host");
            Ok(ExitCode::from(CHILD_EXIT_FAILED as u8))
        }
    }
}

/// Binds the control socket at `path` and serves it on a thread of its own:
/// one request line per connection, one reply line back (see [`answer`]).
///
/// The socket is `0600`, and it lives in the UDS directory, which the jail binds
/// read-write at the same path (`jail::jailer`), so the parent reaches it at the
/// path it named. A socket file left by an earlier run is removed first, since
/// `bind(2)` refuses a path that exists. The directory is `0700`, so the window
/// between `bind` and the `chmod` is closed by the directory, not by the umask.
fn serve_control(path: PathBuf, controls: Controls) -> Result<(), String> {
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(format!(
                "cannot clear the control socket {}: {err}",
                path.display()
            ));
        }
    }
    let listener = UnixListener::bind(&path)
        .map_err(|err| format!("cannot bind the control socket {}: {err}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(|err| {
        format!(
            "cannot restrict the control socket {}: {err}",
            path.display()
        )
    })?;
    thread::Builder::new()
        .name("apz_vmm_control".to_string())
        .spawn(move || {
            for connection in listener.incoming() {
                match connection {
                    Ok(stream) => serve_one(&stream, &controls),
                    Err(err) => {
                        eprintln!("apz-vmm: control socket: {err}");
                        thread::sleep(CONTROL_RETRY);
                    }
                }
            }
        })
        .map(|_| ())
        .map_err(|err| format!("cannot start the control thread: {err}"))
}

/// What the control socket answers from: the VM's balloon, its pause and
/// snapshot control, and the directory its snapshots go under. Each is `None`
/// when the VM has no such thing, and a request for it is refused by name.
struct Controls {
    memory: Option<Arc<dyn MemoryControl>>,
    vm: Option<Arc<dyn VmControl>>,
    snapshot_root: Option<PathBuf>,
}

/// Answers one control connection: reads one request line, writes one reply
/// line. A connection that sends nothing, or stalls past [`CONTROL_IO_TIMEOUT`],
/// is dropped without a reply.
fn serve_one(stream: &UnixStream, controls: &Controls) {
    use std::io::BufRead;

    if stream.set_read_timeout(Some(CONTROL_IO_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(CONTROL_IO_TIMEOUT)).is_err()
    {
        return;
    }
    let mut request = String::new();
    if std::io::BufReader::new(stream)
        .read_line(&mut request)
        .is_err()
    {
        return;
    }
    let reply = answer(request.trim(), controls);
    let mut writer = stream;
    let _ = writeln!(writer, "{reply}");
}

/// The reply to one request line:
///
/// * `usable N` resizes the balloon, `stats` answers its counters as
///   `stats <max> <usable> <held> <free|-> <available|->`;
/// * `pause` and `resume` answer `ok` once the VM's vCPUs have stopped or
///   started;
/// * `reclaim` pages a paused VM's guest memory out of host RAM and answers
///   `ok <bytes>` with the bytes advised;
/// * `snapshot full|diff DIR` writes a snapshot under the snapshot root and
///   answers `ok <kind> <memory_written> <paused_ms>`, leaving the VM paused;
/// * `capture full|diff DIR` takes the same snapshot with its memory and state
///   kept in RAM, and answers the same way;
/// * `write_captured` writes the capture the VM holds and answers `ok <bytes>`,
///   the guest memory bytes written (`ok 0` when there is no capture).
///
/// Anything else, and any request the VM cannot carry out, is `err <why>`.
/// Never a guess: an error is one line, and a snapshot directory that is not
/// under the snapshot root (or has a `..` in it) is refused before any write.
fn answer(request: &str, controls: &Controls) -> String {
    if let Some(rest) = request.strip_prefix("snapshot ") {
        return snapshot_reply(rest, controls, |vm, dir, kind| vm.snapshot(dir, kind));
    }
    if let Some(rest) = request.strip_prefix("capture ") {
        return snapshot_reply(rest, controls, |vm, dir, kind| vm.capture(dir, kind));
    }
    let mut words = request.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some("usable"), Some(text), None) => match text.parse::<u32>() {
            Ok(mib) => match controls.memory.as_deref() {
                Some(memory) => match memory.set_usable_mib(mib) {
                    Ok(()) => "ok".to_string(),
                    Err(err) => err_line(err),
                },
                None => NO_BALLOON.to_string(),
            },
            Err(_) => format!("err {text:?} is not a number of MiB"),
        },
        (Some("stats"), None, None) => match controls.memory.as_deref() {
            Some(memory) => {
                let stats = memory.stats();
                format!(
                    "stats {} {} {} {} {}",
                    stats.max_mib,
                    stats.usable_mib,
                    stats.held_mib,
                    field(stats.free_mib),
                    field(stats.available_mib)
                )
            }
            None => NO_BALLOON.to_string(),
        },
        (Some("pause"), None, None) => acknowledge(controls.vm.as_deref(), |vm| vm.pause()),
        (Some("resume"), None, None) => acknowledge(controls.vm.as_deref(), |vm| vm.resume()),
        (Some("reclaim"), None, None) => reclaim_reply(controls.vm.as_deref()),
        (Some("write_captured"), None, None) => {
            bytes_reply(controls.vm.as_deref(), |vm| vm.write_captured())
        }
        _ => format!("err {request:?} is not a request this control socket takes"),
    }
}

/// The reply to `snapshot <kind> <dir>` or `capture <kind> <dir>` (`rest` is the line after the
/// verb), with `take` the call it makes. The directory is the rest of the line, so it may contain
/// spaces.
fn snapshot_reply(
    rest: &str,
    controls: &Controls,
    take: impl FnOnce(
        &dyn VmControl,
        &Path,
        vmm::spawner::SnapshotKind,
    ) -> Result<vmm::spawner::SnapshotInfo, SpawnError>,
) -> String {
    let Some((word, dir)) = rest.split_once(' ') else {
        return "err snapshot takes a kind (full or diff) and a directory".to_string();
    };
    let Some(kind) = parse_snapshot_kind(word) else {
        return format!("err {word:?} is not a snapshot kind (full or diff)");
    };
    let Some(root) = controls.snapshot_root.as_deref() else {
        return "err this VM has no snapshot root (apz-vmm --snapshot-root)".to_string();
    };
    // Component-wise, so `/snap2` is not under `/snap`; and a `..` is refused
    // even where it would stay under the root, because the jail's view of the
    // path is the one that is written.
    let dir = Path::new(dir);
    let inside = dir.is_absolute()
        && dir.starts_with(root)
        && !dir.components().any(|part| part == Component::ParentDir);
    if !inside {
        return format!(
            "err {} is not an absolute directory under the snapshot root {}",
            dir.display(),
            root.display()
        );
    }
    match controls.vm.as_deref() {
        Some(vm) => match take(vm, dir, kind) {
            Ok(info) => format!(
                "ok {} {} {}",
                snapshot_kind_word(info.kind),
                info.memory_written,
                info.paused_ms
            ),
            Err(err) => err_line(err),
        },
        None => NO_SNAPSHOTS.to_string(),
    }
}

/// The reply to `pause` or `resume`: `ok`, or why the VM could not.
fn acknowledge(
    vm: Option<&dyn VmControl>,
    call: impl FnOnce(&dyn VmControl) -> Result<(), SpawnError>,
) -> String {
    match vm {
        Some(vm) => match call(vm) {
            Ok(()) => "ok".to_string(),
            Err(err) => err_line(err),
        },
        None => NO_SNAPSHOTS.to_string(),
    }
}

/// The reply to `reclaim`: `ok <bytes>`, the guest memory the VM advised out of host RAM, or
/// why it could not (the VM must be paused).
fn reclaim_reply(vm: Option<&dyn VmControl>) -> String {
    bytes_reply(vm, |vm| vm.reclaim())
}

/// The reply to a call that answers the bytes it moved: `ok <bytes>`, or why it could not.
fn bytes_reply(
    vm: Option<&dyn VmControl>,
    call: impl FnOnce(&dyn VmControl) -> Result<u64, SpawnError>,
) -> String {
    match vm {
        Some(vm) => match call(vm) {
            Ok(bytes) => format!("ok {bytes}"),
            Err(err) => err_line(err),
        },
        None => NO_SNAPSHOTS.to_string(),
    }
}

/// The reply that carries an error: one line, so its message never breaks the
/// framing.
fn err_line(err: impl std::fmt::Display) -> String {
    format!("err {}", err.to_string().replace('\n', " "))
}

/// The replies for a request this VM has no thing to carry out with.
const NO_BALLOON: &str = "err this VM has no balloon";
const NO_SNAPSHOTS: &str = "err this VM cannot be paused or snapshotted on this backend";

/// An optional counter as a reply writes it: its number, or `-` when unreported.
fn field<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "-".to_string(), |value| value.to_string())
}

/// Writes the guest's console to stdout as it is produced, until `stop` is set
/// and the capture is drained. Waits on `ready`, which the UART signals after
/// each append, instead of polling the capture.
fn stream_serial(sink: Arc<Mutex<Vec<u8>>>, ready: Arc<Condvar>, stop: Arc<AtomicBool>) {
    let mut stdout = std::io::stdout();
    let mut printed = 0usize;
    loop {
        let pending = {
            let buffer = sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if buffer.len() > printed {
                Some(buffer[printed..].to_vec())
            } else {
                None
            }
        };
        match pending {
            Some(chunk) => {
                if stdout
                    .write_all(&chunk)
                    .and_then(|()| stdout.flush())
                    .is_err()
                {
                    // The parent stopped reading (it is gone): nothing left to
                    // stream to.
                    return;
                }
                printed += chunk.len();
            }
            // Nothing to print and the VM has stopped: the capture is drained.
            None if stop.load(Ordering::SeqCst) => return,
            // Nothing new yet: wait for the UART's next append. The timeout only
            // bounds how long a stop request can go unnoticed.
            None => {
                let buffer = sink
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let _ = ready
                    .wait_timeout_while(buffer, Duration::from_secs(1), |buffer| {
                        buffer.len() <= printed && !stop.load(Ordering::SeqCst)
                    })
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
    }
}

/// Reads stdin and hands it to the guest's serial console, the same way
/// `VmHandle::serial_input` does: a slice whole or not at all, retried while
/// the guest's console has no room for it.
fn forward_console_input(runtime: Arc<VmRuntime>, stop: Arc<AtomicBool>) {
    let mut stdin = std::io::stdin();
    let mut buffer = [0u8; CONSOLE_INPUT_CHUNK];
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match stdin.read(&mut buffer) {
            Ok(0) => return,
            Ok(n) => {
                let data = &buffer[..n];
                for attempt in 1..=CONSOLE_INPUT_ATTEMPTS {
                    match runtime.serial_input(data) {
                        Ok(()) => break,
                        Err(err) => {
                            if attempt == CONSOLE_INPUT_ATTEMPTS {
                                eprintln!("apz-vmm: console input dropped: {err}");
                            } else {
                                thread::sleep(CONSOLE_INPUT_RETRY);
                            }
                        }
                    }
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The command line the parent writes parses back into the same spec, and
    /// the control socket the parent names comes back with it.
    #[test]
    fn a_command_line_round_trips() {
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
                vmm::spawner::DataVolume {
                    name: "data-a".to_string(),
                    image: PathBuf::from("/tmp/data/a.ext4"),
                },
                vmm::spawner::DataVolume {
                    name: "data-b".to_string(),
                    image: PathBuf::from("/tmp/data/b.ext4"),
                },
            ],
        };
        let args = vmm::spawner::process::child_args(&spec);
        let Launch {
            spec: parsed,
            control,
            snapshot_root,
        } = parse(&args).expect("the parent's command line parses");
        assert_eq!(parsed.kernel, spec.kernel);
        assert_eq!(parsed.cmdline, spec.cmdline);
        assert_eq!(parsed.vcpus, spec.vcpus);
        assert_eq!(parsed.memory_mib, spec.memory_mib);
        assert_eq!(parsed.initrd, spec.initrd);
        assert_eq!(parsed.gic, spec.gic);
        assert_eq!(parsed.vsock, spec.vsock);
        assert_eq!(parsed.workspace, spec.workspace);
        assert_eq!(parsed.balloon, spec.balloon);
        assert_eq!(parsed.volumes, spec.volumes);
        assert_eq!(parsed.volumes.len(), 2);
        assert_eq!(parsed.restore, None);
        assert_eq!(control, Some(PathBuf::from("/tmp/plaza-vsock/vm42.ctl")));
        assert_eq!(snapshot_root, Some(PathBuf::from("/tmp/plaza-vsock")));
    }

    /// `parse` reads the command line: `--restore` becomes the spec's restore and
    /// `--snapshot-root` must be absolute; a command line that does not describe one
    /// VM is refused with the flag named; the optional fields take the spec's defaults.
    #[test]
    fn parse_reads_the_command_line() {
        // Original restore_and_the_snapshot_root_are_parsed: `--restore` becomes the
        // spec's restore, and `--snapshot-root` must be an absolute path.
        {
            let args: Vec<String> = [
                "--kernel",
                "/k",
                "--cmdline",
                "c",
                "--restore",
                "/var/snap/vm42",
                "--snapshot-root",
                "/tmp/plaza-vsock",
            ]
            .iter()
            .map(|arg| arg.to_string())
            .collect();
            let launch = parse(&args).expect("a restoring command line");
            assert_eq!(launch.spec.restore, Some(PathBuf::from("/var/snap/vm42")));
            assert_eq!(
                launch.snapshot_root,
                Some(PathBuf::from("/tmp/plaza-vsock"))
            );

            let relative: Vec<String> = [
                "--kernel",
                "/k",
                "--cmdline",
                "c",
                "--snapshot-root",
                "snaps",
            ]
            .iter()
            .map(|arg| arg.to_string())
            .collect();
            let err = parse(&relative).expect_err("a relative snapshot root is refused");
            assert!(err.contains("--snapshot-root"), "{err}");
        }

        // Original a_command_line_that_names_no_vm_is_refused: never a VM booted with a
        // field missing; the flag is named.
        {
            for (args, wanted) in [
                (vec!["--cmdline", "x"], "--kernel"),
                (vec!["--kernel", "/k"], "--cmdline"),
                (
                    vec!["--kernel", "/k", "--cmdline", "x", "--cid", "9"],
                    "--uds",
                ),
                (
                    vec!["--kernel", "/k", "--cmdline", "x", "--uds", "/u"],
                    "--cid",
                ),
                (
                    vec!["--kernel", "/k", "--cmdline", "x", "--memory-mib", "lots"],
                    "--memory-mib",
                ),
                (vec!["--kernel"], "needs a value"),
                (vec!["--nonsense"], "not a flag"),
            ] {
                let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
                let err = parse(&args).expect_err("must be refused");
                assert!(err.contains(wanted), "{args:?}: {err}");
            }
        }

        // Original the_defaults_are_the_specs_defaults: both optional numbers default to
        // the spec's own, and every optional field is absent where the parent omitted it.
        {
            let args: Vec<String> = ["--kernel", "/k", "--cmdline", "c"]
                .iter()
                .map(|arg| arg.to_string())
                .collect();
            let Launch {
                spec,
                control,
                snapshot_root,
            } = parse(&args).expect("a minimal command line");
            assert_eq!(spec.vcpus, VmSpec::default().vcpus);
            assert_eq!(spec.memory_mib, VmSpec::default().memory_mib);
            assert!(spec.vsock.is_none());
            assert!(spec.workspace.is_none());
            assert!(spec.initrd.is_none());
            assert!(spec.gic.is_none());
            assert!(spec.restore.is_none());
            assert!(control.is_none());
            assert!(snapshot_root.is_none());
        }
    }

    /// A balloon whose counters and resizes are fixed, for the control tests.
    struct FakeMemory;

    impl MemoryControl for FakeMemory {
        fn set_usable_mib(&self, _mib: u32) -> Result<(), SpawnError> {
            Ok(())
        }
        fn stats(&self) -> vmm::spawner::MemoryStats {
            vmm::spawner::MemoryStats {
                max_mib: 512,
                usable_mib: 256,
                held_mib: 200,
                free_mib: None,
                available_mib: Some(180),
            }
        }
    }

    /// A VM whose pause and snapshot control succeed: the snapshot holds the kind
    /// asked for and a fixed size, so the reply is checkable.
    struct FakeVm;

    impl VmControl for FakeVm {
        fn pause(&self) -> Result<(), SpawnError> {
            Ok(())
        }
        fn resume(&self) -> Result<(), SpawnError> {
            Ok(())
        }
        fn snapshot(
            &self,
            dir: &Path,
            kind: vmm::spawner::SnapshotKind,
        ) -> Result<vmm::spawner::SnapshotInfo, SpawnError> {
            Ok(vmm::spawner::SnapshotInfo {
                dir: dir.to_path_buf(),
                kind,
                memory_written: 4096,
                paused_ms: 3,
            })
        }
        fn capture(
            &self,
            dir: &Path,
            kind: vmm::spawner::SnapshotKind,
        ) -> Result<vmm::spawner::SnapshotInfo, SpawnError> {
            Ok(vmm::spawner::SnapshotInfo {
                dir: dir.to_path_buf(),
                kind,
                memory_written: 8192,
                paused_ms: 2,
            })
        }
        fn write_captured(&self) -> Result<u64, SpawnError> {
            Ok(8192)
        }
    }

    /// The control answers each request in the shape the parent parses, refuses a
    /// snapshot outside its root and a request the VM has nothing for, and refuses
    /// pause, resume and snapshot when the VM has no snapshot control.
    #[test]
    fn control_answers_and_refuses_requests() {
        // Original the_control_answers_every_request_it_has_a_thing_for: every control
        // present; each reply is `ok`, `ok <kind> <bytes> <ms>` or `stats …`.
        {
            let controls = Controls {
                memory: Some(Arc::new(FakeMemory)),
                vm: Some(Arc::new(FakeVm)),
                snapshot_root: Some(PathBuf::from("/srv/snaps")),
            };
            assert_eq!(answer("usable 128", &controls), "ok");
            assert_eq!(answer("stats", &controls), "stats 512 256 200 - 180");
            assert_eq!(answer("pause", &controls), "ok");
            assert_eq!(answer("resume", &controls), "ok");
            assert_eq!(answer("reclaim", &controls), "ok 0");
            assert_eq!(
                answer("snapshot full /srv/snaps/a", &controls),
                "ok full 4096 3"
            );
            assert_eq!(
                answer("snapshot diff /srv/snaps/with space", &controls),
                "ok diff 4096 3"
            );
            // A capture answers like a snapshot (its own sizes here); its write answers the bytes.
            assert_eq!(
                answer("capture full /srv/snaps/a", &controls),
                "ok full 8192 2"
            );
            assert_eq!(answer("write_captured", &controls), "ok 8192");
            assert!(answer("capture sparse /srv/snaps/a", &controls).starts_with("err "));
            assert!(answer("capture full /etc/passwd", &controls).starts_with("err "));
        }

        // Original a_snapshot_outside_the_root_is_refused: refused before any write unless
        // the directory is absolute and under the root component-wise, with no `..`.
        {
            let controls = Controls {
                memory: None,
                vm: Some(Arc::new(FakeVm)),
                snapshot_root: Some(PathBuf::from("/srv/snaps")),
            };
            for bad in [
                "snapshot full /etc/passwd",
                "snapshot full /srv/snapsx/a",
                "snapshot full /srv/snaps/../etc",
                "snapshot full relative/dir",
                "snapshot sparse /srv/snaps/a",
                "snapshot full",
            ] {
                assert!(answer(bad, &controls).starts_with("err "), "{bad}");
            }
            // No balloon: a resize and stats are refused, not served from nothing.
            assert_eq!(answer("usable 5", &controls), NO_BALLOON);
            assert_eq!(answer("stats", &controls), NO_BALLOON);

            let rootless = Controls {
                memory: None,
                vm: Some(Arc::new(FakeVm)),
                snapshot_root: None,
            };
            assert!(answer("snapshot full /srv/snaps/a", &rootless).starts_with("err "));
        }

        // Original a_vm_without_the_control_refuses_it: with no pause and snapshot control
        // those requests are refused, and `stats` still works with a balloon.
        {
            let controls = Controls {
                memory: Some(Arc::new(FakeMemory)),
                vm: None,
                snapshot_root: Some(PathBuf::from("/srv/snaps")),
            };
            assert_eq!(answer("pause", &controls), NO_SNAPSHOTS);
            assert_eq!(answer("resume", &controls), NO_SNAPSHOTS);
            assert_eq!(answer("reclaim", &controls), NO_SNAPSHOTS);
            assert_eq!(answer("write_captured", &controls), NO_SNAPSHOTS);
            assert_eq!(
                answer("snapshot full /srv/snaps/a", &controls),
                NO_SNAPSHOTS
            );
            assert_eq!(answer("capture full /srv/snaps/a", &controls), NO_SNAPSHOTS);
            assert_eq!(answer("stats", &controls), "stats 512 256 200 - 180");
        }
    }
}
