//! The unix runs. A run is `/bin/sh -c <command>` in the root, with a cleared environment, in its own
//! process group (its pid is the group id), its output on one pipe read by a supervisor thread.
//!
//! The run table (the registry, the output tail, the start, poll and stop rules) is shared with the other
//! platforms in [`super::table`]. This module starts the shell, ends its group, and reports into a [`Run`].
//!
//! This module calls libc for the group kill. Each `unsafe` block says what it relies on.

#![allow(unsafe_code)]

use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::table::{RUN_DRAIN, RUN_POLL, Registry, Run, locked, poll_run, stop_run, timeout_secs};
use super::{RunPoll, RunStarted, RunStopped};
use crate::env::Env;
use crate::error::OpError;
use crate::root::Root;

const SHELL: &str = "/bin/sh";

/// The runs one table holds. A table is cheap, so a caller that wants one global set makes a `static`.
pub struct Runs {
    registry: Mutex<Registry>,
}

impl Default for Runs {
    fn default() -> Self {
        Self::new()
    }
}

impl Runs {
    /// A run table with no runs.
    pub const fn new() -> Self {
        Self {
            registry: Mutex::new(Registry::new()),
        }
    }

    /// Starts `command` in the root and returns at once.
    ///
    /// `timeout_s` defaults to 120 seconds and is capped at 600. When four runs are held, a finished one is
    /// made room for; when all are running the start is refused.
    pub fn start(
        &self,
        root: &Root,
        env: &Env,
        command: &str,
        timeout_s: Option<u64>,
    ) -> Result<RunStarted, OpError> {
        let timeout_s = timeout_secs(command, timeout_s)?;
        let mut registry = locked(&self.registry);
        let now = Instant::now();
        registry.admit(now)?;
        let (child, reader) = spawn(root, env, command)?;
        let run = Arc::new(Run::default());
        let id = registry.insert(Arc::clone(&run));
        supervise(run, child, reader, now + Duration::from_secs(timeout_s));
        Ok(RunStarted { run: id })
    }

    /// The run's output from `offset` (the start when `None`), at most one chunk.
    ///
    /// `offset` is where the output starts, which is later than asked when the tail has dropped earlier
    /// bytes; `next` is the offset to poll from next.
    pub fn poll(&self, run: &str, offset: Option<u64>) -> Result<RunPoll, OpError> {
        poll_run(&self.registry, run, offset)
    }

    /// Kills the run's process group and waits briefly for the run to end.
    pub fn stop(&self, run: &str) -> Result<RunStopped, OpError> {
        stop_run(&self.registry, run)
    }
}

/// Starts `command` in the root, its output on one pipe. The builder is dropped after the spawn, so
/// the pipe's write ends close in this process and the reader sees end of file.
fn spawn(root: &Root, env: &Env, command: &str) -> Result<(Child, io::PipeReader), OpError> {
    let (reader, writer) = io::pipe()
        .map_err(|err| OpError::internal(format!("cannot open the output pipe: {err}")))?;
    let stderr = writer
        .try_clone()
        .map_err(|err| OpError::internal(format!("cannot open the output pipe: {err}")))?;
    let mut builder = Command::new(SHELL);
    builder
        .args(["-c", command])
        .stdin(Stdio::null())
        .process_group(0);
    env.apply(&mut builder);
    builder
        .current_dir(root.dir())
        .stdout(writer)
        .stderr(stderr);
    let child = builder
        .spawn()
        .map_err(|err| OpError::internal(format!("cannot start the shell: {err}")))?;
    drop(builder);
    Ok((child, reader))
}

/// Reads the run's output into its tail, and ends the run at its exit, its timeout or a stop. The
/// process group is killed on every end, and the run is finished once the output has drained.
fn supervise(run: Arc<Run>, mut child: Child, reader: io::PipeReader, deadline: Instant) {
    let (drained_tx, drained) = mpsc::channel::<()>();
    let sink = Arc::clone(&run);
    thread::spawn(move || {
        let mut reader = reader;
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => sink.append(&chunk[..read]),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = drained_tx.send(());
    });
    thread::spawn(move || {
        let (exit_code, timed_out) = loop {
            match child.try_wait() {
                Ok(Some(status)) => break (status.code(), false),
                Ok(None) if run.stop.load(Ordering::SeqCst) => {
                    kill_group(&mut child);
                    break (child.wait().ok().and_then(|status| status.code()), false);
                }
                Ok(None) if Instant::now() >= deadline => {
                    kill_group(&mut child);
                    break (child.wait().ok().and_then(|status| status.code()), true);
                }
                Ok(None) => thread::sleep(RUN_POLL),
                Err(_) => break (None, false),
            }
        };
        // Whatever the shell left running in its group goes too, so the output can end.
        kill_group(&mut child);
        let _ = drained.recv_timeout(RUN_DRAIN);
        run.finish(exit_code, timed_out);
    });
}

/// Kills the shell's process group, then the shell. The group id is the shell's pid, which
/// `process_group(0)` made the group's leader.
fn kill_group(child: &mut Child) {
    let group = child.id() as libc::pid_t;
    // SAFETY: kill(2) takes a pid and a signal and touches no memory. The negated pid names the group
    // the shell leads, which holds only this run's processes.
    unsafe { libc::kill(-group, libc::SIGKILL) };
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::super::table::lookup;
    use super::*;
    use crate::runs::{MAX_RUNS, OUTPUT_CAP};
    use serde_json::{Value, json};
    use std::fs;
    use std::path::PathBuf;

    const PATH: &str = "/usr/local/bin:/usr/bin:/bin";

    fn scratch(name: &str) -> (PathBuf, Root, Env) {
        let dir = std::env::temp_dir().join(format!("apz-ops-run-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch root");
        let env = Env::new(&dir, PATH);
        let root = Root::new(&dir);
        (dir, root, env)
    }

    fn settle(runs: &Runs, id: &str) -> RunPoll {
        let started = Instant::now();
        loop {
            let reply = runs.poll(id, Some(0)).expect("the run polls");
            if reply.done {
                return reply;
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "run {id} did not end"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(target_os = "linux")]
    fn pid_is_alive(pid: &str) -> bool {
        match fs::read_to_string(format!("/proc/{pid}/stat")) {
            // A zombie has ended; its state is the field after the parenthesised name.
            Ok(stat) => stat
                .rsplit(')')
                .next()
                .is_some_and(|rest| !rest.trim_start().starts_with('Z')),
            Err(_) => false,
        }
    }

    #[test]
    fn a_run_reports_its_output_and_exit_code() {
        let (_dir, root, env) = scratch("output");
        let runs = Runs::new();
        let started = runs
            .start(&root, &env, "echo hi; echo err 1>&2; exit 3", None)
            .expect("the run starts");
        let reply = settle(&runs, &started.run);
        assert_eq!(reply.exit_code, Some(3));
        assert!(!reply.timed_out);
        assert!(
            reply.output.contains("hi") && reply.output.contains("err"),
            "{:?}",
            reply.output
        );
        assert!(matches!(
            runs.poll("run-99", None),
            Err(OpError::NotFound(_))
        ));
        assert!(matches!(
            runs.start(&root, &env, "true", Some(601)),
            Err(OpError::LimitExceeded(_))
        ));
        assert!(matches!(
            runs.start(&root, &env, "   ", None),
            Err(OpError::InvalidInput(_))
        ));
        assert!(matches!(
            runs.start(&root, &env, "true", Some(0)),
            Err(OpError::InvalidInput(_))
        ));
    }

    #[test]
    fn the_environment_is_the_one_given() {
        let (_dir, root, env) = scratch("env");
        let env = env.with_var("APZ_OPS_TEST", "seen");
        let runs = Runs::new();
        let started = runs
            .start(
                &root,
                &env,
                "echo \"$APZ_OPS_TEST:$PATH\"; echo \"$HOME\"",
                None,
            )
            .expect("the run starts");
        let reply = settle(&runs, &started.run);
        assert!(
            reply.output.starts_with(&format!("seen:{PATH}\n")),
            "{:?}",
            reply.output
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_run_past_its_timeout_is_killed_with_its_whole_group() {
        let (dir, root, env) = scratch("timeout");
        let runs = Runs::new();
        let command = "sleep 60 & echo $! > child.pid; sleep 60";
        let started = runs
            .start(&root, &env, command, Some(1))
            .expect("the run starts");
        let reply = settle(&runs, &started.run);
        assert!(reply.timed_out);
        assert_eq!(reply.exit_code, None);
        let pid =
            fs::read_to_string(dir.join("child.pid")).expect("the shell wrote its child's pid");
        let pid = pid.trim();
        assert!(
            !pid_is_alive(pid),
            "the background child {pid} was killed with the group"
        );
    }

    #[test]
    fn a_stopped_run_ends_and_a_second_stop_says_so() {
        let (_dir, root, env) = scratch("stop");
        let runs = Runs::new();
        let started = runs
            .start(&root, &env, "sleep 60", None)
            .expect("the run starts");
        let stopped = runs.stop(&started.run).expect("the stop answers");
        assert!(stopped.stopped);
        assert!(stopped.done);
        let again = runs.stop(&started.run).expect("the stop answers");
        assert!(!again.stopped);
    }

    #[test]
    fn a_run_keeps_the_tail_of_its_output_within_the_cap() {
        let (_dir, root, env) = scratch("cap");
        let runs = Runs::new();
        let started = runs
            .start(&root, &env, "head -c 2500000 /dev/zero | tr '\\0' a", None)
            .expect("the run starts");
        let reply = settle(&runs, &started.run);
        let kept = OUTPUT_CAP as u64;
        let total = 2_500_000u64;
        assert!(
            reply.offset >= total - kept && reply.offset <= total - kept + 3,
            "offset {}",
            reply.offset
        );
        assert!(
            reply.output.len() <= crate::page::CHUNK && reply.output.bytes().all(|b| b == b'a')
        );
        assert_eq!(reply.next, reply.offset + reply.output.len() as u64);
        assert!(runs.poll(&started.run, Some(total + 1)).is_err());
    }

    #[test]
    fn at_most_four_runs_are_held_and_a_finished_one_makes_room() {
        let (_dir, root, env) = scratch("limit");
        let runs = Runs::new();
        let mut ids = Vec::new();
        for _ in 0..MAX_RUNS {
            let started = runs
                .start(&root, &env, "sleep 60", None)
                .expect("the run starts");
            ids.push(started.run);
        }
        assert!(matches!(
            runs.start(&root, &env, "true", None),
            Err(OpError::LimitExceeded(_))
        ));
        runs.stop(&ids[0]).expect("the stop answers");
        let fifth = runs
            .start(&root, &env, "sleep 60", None)
            .expect("the freed run makes room");
        ids.push(fifth.run);
        assert!(
            lookup(&runs.registry, &ids[0]).is_err(),
            "the finished run made room"
        );
        for id in &ids[1..] {
            runs.stop(id).expect("the stop answers");
        }
    }

    #[test]
    fn a_reply_serializes_with_its_fields_in_key_order() {
        let (_dir, root, env) = scratch("shape");
        let runs = Runs::new();
        let started = runs
            .start(&root, &env, "echo x", None)
            .expect("the run starts");
        let reply = settle(&runs, &started.run);
        let value: Value = serde_json::to_value(&reply).expect("a value");
        assert_eq!(
            value,
            json!({
                "done": true,
                "exit_code": 0,
                "next": reply.next,
                "offset": 0,
                "output": "x\n",
                "run": started.run,
                "timed_out": false,
            })
        );
        let text = serde_json::to_string(&reply).expect("text");
        assert!(text.starts_with("{\"done\":"), "{text}");
    }
}
