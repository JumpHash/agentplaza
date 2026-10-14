//! The unix runs. A run is `/bin/sh -c <command>` in the root, with a cleared environment, in its own
//! process group (its pid is the group id), its output on one pipe read by a supervisor thread.
//!
//! This module calls libc for the group kill. Each `unsafe` block says what it relies on.

#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::{DEFAULT_TIMEOUT_S, MAX_RUNS, MAX_TIMEOUT_S, OUTPUT_CAP, RunPoll, RunStarted, RunStopped};
use crate::env::Env;
use crate::error::OpError;
use crate::page::{OUTPUT_BUDGET, byte_chunk_end, is_continuation};
use crate::root::Root;

const SHELL: &str = "/bin/sh";
/// How long a finished run stays to be polled.
const RUN_KEEP: Duration = Duration::from_secs(10 * 60);
const RUN_POLL: Duration = Duration::from_millis(20);
/// How long the output is awaited once the shell and its group are gone.
const RUN_DRAIN: Duration = Duration::from_millis(500);
/// How long a stop waits for the run to end before it answers.
const RUN_STOP_WAIT: Duration = Duration::from_secs(2);

/// The runs one table holds. A table is cheap, so a caller that wants one global set makes a `static`.
pub struct Runs {
    registry: Mutex<Registry>,
}

#[derive(Default)]
struct Registry {
    next: u64,
    runs: BTreeMap<String, Arc<Run>>,
}

/// One run: its output tail, how it ended, and whether a stop was asked for.
#[derive(Default)]
struct Run {
    output: Mutex<Tail>,
    state: Mutex<RunState>,
    stop: AtomicBool,
}

/// The last [`OUTPUT_CAP`] bytes of a run's output, and the offset of the first.
#[derive(Default)]
struct Tail {
    bytes: Vec<u8>,
    base: u64,
}

#[derive(Default)]
enum RunState {
    #[default]
    Running,
    Done {
        exit_code: Option<i32>,
        timed_out: bool,
        at: Instant,
    },
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
            registry: Mutex::new(Registry {
                next: 0,
                runs: BTreeMap::new(),
            }),
        }
    }

    /// Starts `command` in the root and returns at once.
    ///
    /// `timeout_s` defaults to [`DEFAULT_TIMEOUT_S`] and is capped at [`MAX_TIMEOUT_S`]. When [`MAX_RUNS`]
    /// runs are held, a finished one is made room for; when all are running the start is refused.
    pub fn start(
        &self,
        root: &Root,
        env: &Env,
        command: &str,
        timeout_s: Option<u64>,
    ) -> Result<RunStarted, OpError> {
        if command.trim().is_empty() {
            return Err(OpError::invalid("\"command\" must be a non-empty string"));
        }
        let timeout_s = match timeout_s {
            None => DEFAULT_TIMEOUT_S,
            Some(0) => return Err(OpError::invalid("\"timeout_s\" must be at least 1")),
            Some(secs) if secs > MAX_TIMEOUT_S => {
                return Err(OpError::limit(format!(
                    "\"timeout_s\" is {secs}, over the {MAX_TIMEOUT_S}-second cap"
                )));
            }
            Some(secs) => secs,
        };

        let mut registry = locked(&self.registry);
        let now = Instant::now();
        registry.runs.retain(|_, run| !run.expired(now));
        if registry.runs.len() >= MAX_RUNS {
            let oldest_finished = registry
                .runs
                .iter()
                .filter_map(|(id, run)| run.finished_at().map(|at| (at, id.clone())))
                .min();
            match oldest_finished {
                Some((_, id)) => {
                    registry.runs.remove(&id);
                }
                None => {
                    return Err(OpError::limit(format!(
                        "{MAX_RUNS} runs are running; stop one first"
                    )));
                }
            }
        }

        let (child, reader) = spawn(root, env, command)?;
        registry.next += 1;
        let id = format!("run-{}", registry.next);
        let run = Arc::new(Run::default());
        registry.runs.insert(id.clone(), Arc::clone(&run));
        supervise(run, child, reader, now + Duration::from_secs(timeout_s));
        Ok(RunStarted { run: id })
    }

    /// The run's output from `offset` (the start when `None`), at most one chunk.
    ///
    /// `offset` is where the output starts, which is later than asked when the tail has dropped earlier
    /// bytes; `next` is the offset to poll from next.
    pub fn poll(&self, run: &str, offset: Option<u64>) -> Result<RunPoll, OpError> {
        let found = lookup(&self.registry, run)?;
        let offset = offset.unwrap_or(0);
        let (done, exit_code, timed_out) = match *locked(&found.state) {
            RunState::Running => (false, None, false),
            RunState::Done {
                exit_code,
                timed_out,
                ..
            } => (true, exit_code, timed_out),
        };
        let tail = locked(&found.output);
        let total = tail.base + tail.bytes.len() as u64;
        if offset > total {
            return Err(OpError::invalid(format!(
                "offset {offset} is past the end of the output ({total} bytes)"
            )));
        }
        // Where the output starts: the offset, or the oldest byte still kept. A start inside a
        // character (a cut in the tail, or an offset given mid-character) moves on to the next one.
        let mut skip = (offset.max(tail.base) - tail.base) as usize;
        let limit = skip + 3;
        while skip < tail.bytes.len() && skip < limit && is_continuation(tail.bytes[skip]) {
            skip += 1;
        }
        let available = &tail.bytes[skip..];
        let start = tail.base + skip as u64;
        let reply = |chunk: &[u8]| RunPoll {
            done,
            exit_code,
            next: start + chunk.len() as u64,
            offset: start,
            output: String::from_utf8_lossy(chunk).into_owned(),
            run: run.to_string(),
            timed_out,
        };
        let end = byte_chunk_end(available, |end| {
            serde_json::to_string(&reply(&available[..end]))
                .is_ok_and(|json| json.len() <= OUTPUT_BUDGET)
        });
        Ok(reply(&available[..end]))
    }

    /// Kills the run's process group and waits briefly for the run to end.
    pub fn stop(&self, run: &str) -> Result<RunStopped, OpError> {
        let found = lookup(&self.registry, run)?;
        let was_running = found.is_running();
        found.stop.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + RUN_STOP_WAIT;
        while found.is_running() && Instant::now() < deadline {
            thread::sleep(RUN_POLL);
        }
        Ok(RunStopped {
            done: !found.is_running(),
            run: run.to_string(),
            stopped: was_running,
        })
    }
}

impl Run {
    fn append(&self, bytes: &[u8]) {
        let mut tail = locked(&self.output);
        tail.bytes.extend_from_slice(bytes);
        if tail.bytes.len() > OUTPUT_CAP {
            let dropped = tail.bytes.len() - OUTPUT_CAP;
            tail.bytes.drain(..dropped);
            tail.base += dropped as u64;
        }
    }

    fn finish(&self, exit_code: Option<i32>, timed_out: bool) {
        *locked(&self.state) = RunState::Done {
            exit_code,
            timed_out,
            at: Instant::now(),
        };
    }

    fn is_running(&self) -> bool {
        matches!(*locked(&self.state), RunState::Running)
    }

    /// Finished, and longer ago than [`RUN_KEEP`].
    fn expired(&self, now: Instant) -> bool {
        matches!(*locked(&self.state), RunState::Done { at, .. } if now.duration_since(at) >= RUN_KEEP)
    }

    fn finished_at(&self) -> Option<Instant> {
        match *locked(&self.state) {
            RunState::Running => None,
            RunState::Done { at, .. } => Some(at),
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
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

fn lookup(registry: &Mutex<Registry>, id: &str) -> Result<Arc<Run>, OpError> {
    locked(registry)
        .runs
        .get(id)
        .cloned()
        .ok_or_else(|| OpError::not_found(format!("no run {id}")))
}

#[cfg(test)]
mod tests {
    use super::*;
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
            .start(&root, &env, "echo \"$APZ_OPS_TEST:$PATH\"; echo \"$HOME\"", None)
            .expect("the run starts");
        let reply = settle(&runs, &started.run);
        assert!(reply.output.starts_with(&format!("seen:{PATH}\n")), "{:?}", reply.output);
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
        let pid = fs::read_to_string(dir.join("child.pid")).expect("the shell wrote its child's pid");
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
        assert!(reply.output.len() <= crate::page::CHUNK && reply.output.bytes().all(|b| b == b'a'));
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
