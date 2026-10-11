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

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use super::{CHILD_BOOT_TIMEOUT, CHILD_READY, SpawnError, VmRuntime, VmSpec};

/// How many lines of the child's stderr log are kept, so a failed boot's error
/// is its own last words and not an unbounded buffer.
const CHILD_LOG_LINES: usize = 200;

/// The environment the per-VM process is given — the whole of it. Each name is
/// a confinement switch this crate itself reads: the jailer's mode, base and
/// uid/gid target ([`crate::jail::jailer`]) and the seccomp jail's mode
/// ([`crate::jail`]). A deployment configures the child's confinement through
/// these and nothing else reaches it (see the `env_clear` in [`boot`]).
const CONFINEMENT_ENV: [&str; 6] = [
    crate::jail::jailer::JAILER_ENV,
    crate::jail::jailer::JAIL_ROOT_ENV,
    crate::jail::jailer::JAIL_UID_ENV,
    crate::jail::jailer::JAIL_GID_ENV,
    crate::jail::JAIL_ENV,
    crate::jail::NO_JAIL_ENV,
];

/// Boots one VM in a child `apz-vmm` process, filling `runtime` in the same
/// way a backend does (see [`super::boot`] for the contract). Returns once the
/// child has reported its VM up.
pub(super) fn boot(spec: &VmSpec, runtime: Arc<VmRuntime>) -> Result<(), SpawnError> {
    let binary = super::child_binary()?;
    let mut command = Command::new(&binary);
    command
        .args(child_args(spec))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The child gets a *minimal* environment: the confinement switches (a
    // deployment's configuration of exactly what this mode exists to apply)
    // and nothing else. No host environment variable — a token, a path, a
    // secret — rides into the VM's process, which is invariant 2's posture
    // ("no credential ever enters a VM") extended to the process that holds
    // the guest, not just to guest-visible memory.
    command.env_clear();
    for name in CONFINEMENT_ENV {
        if let Ok(value) = std::env::var(name) {
            command.env(name, value);
        }
    }
    let mut child = command.spawn().map_err(|source| SpawnError::ChildSpawn {
        path: binary.clone(),
        source,
    })?;
    let pid = child.id();

    // The three pipes are the whole interface. Missing one is not possible
    // after a successful `spawn` with all three requested, but it is reported
    // rather than unwrapped.
    let stdin = take(child.stdin.take(), &binary, "stdin")?;
    let stdout = take(child.stdout.take(), &binary, "stdout")?;
    let stderr = take(child.stderr.take(), &binary, "stderr")?;

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
            fail_start(&mut child, source)?;
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

    // The stop control: `SIGTERM`, then `SIGKILL` on a second request (the
    // spawner repeats a request that did not take). A child that has already
    // been reaped is not signalled again: its pid may have been reused.
    let signals = Arc::new(AtomicUsize::new(0));
    let gone = Arc::new(AtomicBool::new(false));
    {
        let signals = Arc::clone(&signals);
        let gone = Arc::clone(&gone);
        runtime.set_stop_control(move || {
            if gone.load(Ordering::SeqCst) {
                return;
            }
            let signal = if signals.fetch_add(1, Ordering::SeqCst) == 0 {
                libc::SIGTERM
            } else {
                libc::SIGKILL
            };
            if let Err(err) = crate::sys::send_signal(pid as i32, signal) {
                // `ESRCH` here is the VM having stopped between the two checks;
                // anything else is reported rather than swallowed, because a
                // VM that cannot be signalled is a VM that cannot be killed.
                if err.raw_os_error() != Some(libc::ESRCH) {
                    eprintln!("vmm: cannot signal the per-VM process {pid}: {err}");
                }
            }
        });
    }

    // The waiter: the last thread, and the one `VmHandle::wait` joins. It
    // reaps the child and publishes how the VM ended, so `exit_reason` reads
    // the same in both modes.
    let slot = Arc::new(Mutex::new(Some(child)));
    let waiter = {
        let runtime = Arc::clone(&runtime);
        let gone = Arc::clone(&gone);
        let slot = Arc::clone(&slot);
        thread::Builder::new()
            .name(format!("apz_vm{}_child", runtime.id))
            .spawn(move || {
                // The slot is only held to *take* the child: the wait runs
                // outside the lock, so a failed start below can still reap it.
                let child = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
                let Some(mut child) = child else { return };
                let status = child.wait();
                gone.store(true, Ordering::SeqCst);
                match status.map(|status| status.code()) {
                    Ok(Some(super::CHILD_EXIT_SHUTDOWN)) => runtime.mark_guest_exited(),
                    Ok(Some(super::CHILD_EXIT_RESET)) => runtime.mark_guest_reset(),
                    // A signal, a failure status, or a `wait` error: the VM was
                    // not stopped by its guest, so no guest exit is published.
                    _ => {}
                }
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
            Err(SpawnError::Thread(source))
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
    args
}

/// The child's stderr reader: it forwards every line to the child's log, and
/// the readiness line's arrival (or the child's last words, `false`) to the
/// parent.
fn read_child_stderr(
    stderr: std::process::ChildStderr,
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
fn fail_start(child: &mut Child, source: std::io::Error) -> Result<(), SpawnError> {
    let _ = child.kill();
    let _ = child.wait();
    Err(SpawnError::Thread(source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spawner::{GicVersion, VsockConfig};
    use std::path::PathBuf;

    /// Every `VmSpec` field reaches the child's command line, including the
    /// exact vsock socket path (never a directory the child re-derives).
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
            ("--workspace", "/tmp/run/workspace-0.ext4"),
        ] {
            assert!(
                pairs.contains(&(flag, value)),
                "{flag} {value} is not on the command line: {pairs:?}"
            );
        }
        // The optional fields are absent when the spec has none of them.
        let bare = child_args(&VmSpec::default());
        for absent in ["--initrd", "--gic", "--cid", "--uds", "--workspace"] {
            assert!(!bare.iter().any(|arg| arg == absent), "{absent} leaked");
        }
    }
}
