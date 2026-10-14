//! The windows runs. A run is `cmd.exe /S /C "<command>"` in the root, with the environment it is given. It is
//! started suspended and put in a job object before it runs, so the job holds the shell and everything the shell
//! starts. The timeout, a stop and the shell's exit end the job as a whole. Its output is one anonymous pipe,
//! which a reader thread keeps as the run's tail.
//!
//! The run table is shared with unix in [`super::table`]. This module starts the process and supervises it.
//!
//! This module calls the Win32 API through [`crate::win`], and `unsafe` only for the handle it reads from a
//! process. Each `unsafe` block says what it relies on.

#![allow(unsafe_code)]

use std::fs::OpenOptions;
use std::io::{self, Read};
use std::mem;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESTDHANDLES,
    STARTUPINFOEXW,
};

use super::table::{RUN_DRAIN, Registry, Run, locked, poll_run, stop_run, timeout_secs};
use super::{RunPoll, RunStarted, RunStopped};
use crate::env::Env;
use crate::error::OpError;
use crate::root::Root;
use crate::win::{
    AttrList, Job, Waited, create_suspended, environment_block, exit_code_of, plain_path, resume,
    set_inheritable, terminate, wait_for,
};

const SHELL: &str = "cmd.exe";
/// How long the supervisor waits between checks for a stop, the timeout and the shell's exit. Matches the table's
/// poll interval, [`RUN_POLL`](super::table::RUN_POLL) (20 ms).
const TICK_MS: u32 = 20;
/// How long a killed shell gets to end before the supervisor answers.
const KILL_WAIT_MS: u32 = 5_000;

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
        let spawned = spawn(root, env, command)?;
        let run = Arc::new(Run::default());
        let id = registry.insert(Arc::clone(&run));
        supervise(run, spawned, now + Duration::from_secs(timeout_s));
        Ok(RunStarted { run: id })
    }

    /// The run's output from `offset` (the start when `None`), at most one chunk.
    ///
    /// `offset` is where the output starts, which is later than asked when the tail has dropped earlier
    /// bytes; `next` is the offset to poll from next.
    pub fn poll(&self, run: &str, offset: Option<u64>) -> Result<RunPoll, OpError> {
        poll_run(&self.registry, run, offset)
    }

    /// Ends the run's job and waits briefly for the run to end.
    pub fn stop(&self, run: &str) -> Result<RunStopped, OpError> {
        stop_run(&self.registry, run)
    }
}

/// A started run, not yet supervised: its shell, the job that holds it, and the read end of its output.
struct Spawned {
    process: OwnedHandle,
    job: Job,
    reader: io::PipeReader,
}

/// Starts `cmd.exe /S /C "<command>"` suspended, puts it in a job, and resumes it. The shell is given NUL for
/// input and the output pipe for both outputs, and it inherits only those three handles.
fn spawn(root: &Root, env: &Env, command: &str) -> Result<Spawned, OpError> {
    if command.contains('\0') {
        return Err(OpError::invalid(
            "\"command\" holds a NUL, which a command cannot",
        ));
    }
    let (reader, writer) = io::pipe()
        .map_err(|err| OpError::internal(format!("cannot open the output pipe: {err}")))?;
    let stderr = writer
        .try_clone()
        .map_err(|err| OpError::internal(format!("cannot open the output pipe: {err}")))?;
    let stdin = OpenOptions::new()
        .read(true)
        .open("NUL")
        .map_err(|err| OpError::internal(format!("cannot open the null device: {err}")))?;
    let inherited = [
        stdin.as_raw_handle(),
        writer.as_raw_handle(),
        stderr.as_raw_handle(),
    ];
    for handle in inherited {
        set_inheritable(handle, true)
            .map_err(|err| OpError::internal(format!("cannot share a handle: {err}")))?;
    }
    let environment = environment_block(env, &[])
        .map_err(|err| OpError::invalid(format!("the environment is not usable: {err}")))?;
    let mut attributes = AttrList::new(1)
        .map_err(|err| OpError::internal(format!("cannot start the shell: {err}")))?;
    attributes
        .set(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            inherited.as_ptr().cast(),
            mem::size_of_val(&inherited),
        )
        .map_err(|err| OpError::internal(format!("cannot start the shell: {err}")))?;
    // SAFETY: an all-zero STARTUPINFOEXW is a valid value of that struct. Its fields are set below.
    let mut startup: STARTUPINFOEXW = unsafe { mem::zeroed() };
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = stdin.as_raw_handle();
    startup.StartupInfo.hStdOutput = writer.as_raw_handle();
    startup.StartupInfo.hStdError = stderr.as_raw_handle();
    startup.lpAttributeList = attributes.as_mut_ptr();
    let job =
        Job::new().map_err(|err| OpError::internal(format!("cannot start the shell: {err}")))?;
    let line = format!("{SHELL} /S /C \"{command}\"");
    let started = create_suspended(
        &line,
        &plain_path(root.dir()),
        &environment,
        &mut startup,
        true,
        CREATE_SUSPENDED | CREATE_NO_WINDOW,
    )
    .map_err(|err| OpError::internal(format!("cannot start the shell: {err}")))?;
    // The shell has its own copies now. Closing ours lets the reader see end of file once every holder is gone.
    drop(writer);
    drop(stderr);
    drop(stdin);
    let process = started.process;
    let thread = started.thread;
    // The shell is in its job before it runs, so nothing it starts escapes the job.
    if let Err(err) = job.assign(process.as_raw_handle()) {
        terminate(process.as_raw_handle());
        return Err(OpError::internal(format!(
            "cannot put the shell in its job: {err}"
        )));
    }
    if let Err(err) = resume(&thread) {
        job.kill();
        return Err(OpError::internal(format!("cannot start the shell: {err}")));
    }
    drop(thread);
    Ok(Spawned {
        process,
        job,
        reader,
    })
}

/// Reads the run's output into its tail, and ends the run at its exit, its timeout or a stop. The job is killed
/// on every end, and the run is finished once the output has drained.
fn supervise(run: Arc<Run>, spawned: Spawned, deadline: Instant) {
    let Spawned {
        process,
        job,
        reader,
    } = spawned;
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
                // A broken pipe: every holder of the write end has closed it.
                Err(_) => break,
            }
        }
        let _ = drained_tx.send(());
    });
    thread::spawn(move || {
        let handle: HANDLE = process.as_raw_handle();
        let (exit, timed_out) = loop {
            match wait_for(handle, TICK_MS) {
                Ok(Waited::Signaled) => break (exit_code_of(handle), false),
                Ok(Waited::TimedOut) if run.stop.load(Ordering::SeqCst) => {
                    job.kill();
                    let _ = wait_for(handle, KILL_WAIT_MS);
                    // A run ended by a stop has no exit code, as on unix when a signal ends it.
                    break (None, false);
                }
                Ok(Waited::TimedOut) if Instant::now() >= deadline => {
                    job.kill();
                    let _ = wait_for(handle, KILL_WAIT_MS);
                    break (None, true);
                }
                Ok(Waited::TimedOut) => {}
                Err(_) => break (None, false),
            }
        };
        // Whatever the shell left running in its job goes too, so the output can end.
        job.kill();
        let _ = drained.recv_timeout(RUN_DRAIN);
        run.finish(exit, timed_out);
    });
}

#[cfg(test)]
mod tests {
    use super::super::table::lookup;
    use super::*;
    use crate::runs::{MAX_RUNS, OUTPUT_CAP};
    use serde_json::{Value, json};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command as StdCommand;

    const PATH: &str = r"C:\Windows\System32;C:\Windows";

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

    /// Whether an image named `image` is running, by asking tasklist.
    fn image_running(image: &str) -> bool {
        let out = StdCommand::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {image}"), "/NH"])
            .output()
            .expect("tasklist runs");
        String::from_utf8_lossy(&out.stdout)
            .to_ascii_lowercase()
            .contains(&image.to_ascii_lowercase())
    }

    #[test]
    fn a_run_reports_its_output_and_exit_code() {
        let (_dir, root, env) = scratch("output");
        let runs = Runs::new();
        let started = runs
            .start(&root, &env, "echo hi & echo err 1>&2 & exit 3", None)
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
        assert!(matches!(
            runs.start(&root, &env, "echo a\0b", None),
            Err(OpError::InvalidInput(_))
        ));
    }

    #[test]
    fn the_environment_is_the_one_given() {
        let (_dir, root, env) = scratch("env");
        let env = env.with_var("APZ_OPS_TEST", "seen");
        let runs = Runs::new();
        let started = runs
            .start(&root, &env, "echo %APZ_OPS_TEST%;%PATH%", None)
            .expect("the run starts");
        let reply = settle(&runs, &started.run);
        assert!(
            reply.output.starts_with(&format!("seen;{PATH}\r\n")),
            "{:?}",
            reply.output
        );
    }

    #[test]
    fn a_run_past_its_timeout_is_killed_with_its_whole_tree() {
        let (_dir, root, env) = scratch("timeout");
        let runs = Runs::new();
        let started = runs
            .start(&root, &env, "ping -n 60 127.0.0.1 > nul", Some(1))
            .expect("the run starts");
        let reply = settle(&runs, &started.run);
        assert!(reply.timed_out);
        assert_eq!(reply.exit_code, None);
        // The ping is a child of the shell, so the job held it, and it must be gone too.
        let deadline = Instant::now() + Duration::from_secs(5);
        while image_running("PING.EXE") && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
        }
        assert!(!image_running("PING.EXE"), "the child outlived the job");
    }

    #[test]
    fn a_stopped_run_ends_and_a_second_stop_says_so() {
        let (_dir, root, env) = scratch("stop");
        let runs = Runs::new();
        let started = runs
            .start(&root, &env, "ping -n 60 127.0.0.1 > nul", None)
            .expect("the run starts");
        let stopped = runs.stop(&started.run).expect("the stop answers");
        assert!(stopped.stopped);
        assert!(stopped.done);
        let again = runs.stop(&started.run).expect("the stop answers");
        assert!(!again.stopped);
    }

    #[test]
    fn a_run_keeps_the_tail_of_its_output_within_the_cap() {
        let (dir, root, env) = scratch("cap");
        let total = 2_500_000u64;
        // `type` is a shell builtin, so the run needs no program found on the PATH it is given (PowerShell is
        // not on `C:\Windows\System32`, which is all the test PATH names).
        fs::write(dir.join("big.txt"), "a".repeat(total as usize)).unwrap();
        let runs = Runs::new();
        let started = runs
            .start(&root, &env, "type big.txt", None)
            .expect("the run starts");
        let reply = settle(&runs, &started.run);
        assert_eq!(reply.exit_code, Some(0), "{:?}", reply.output);
        let kept = OUTPUT_CAP as u64;
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
                .start(&root, &env, "ping -n 60 127.0.0.1 > nul", None)
                .expect("the run starts");
            ids.push(started.run);
        }
        assert!(matches!(
            runs.start(&root, &env, "echo x", None),
            Err(OpError::LimitExceeded(_))
        ));
        runs.stop(&ids[0]).expect("the stop answers");
        let fifth = runs
            .start(&root, &env, "ping -n 60 127.0.0.1 > nul", None)
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
                "output": "x\r\n",
                "run": started.run,
                "timed_out": false,
            })
        );
        let text = serde_json::to_string(&reply).expect("text");
        assert!(text.starts_with("{\"done\":"), "{text}");
    }
}
