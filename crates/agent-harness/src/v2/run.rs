//! `run`: a shell command in the workspace, in one persistent shell per work item, or in the background.
//!
//! The persistent shell is `/bin/sh` with the agent's environment (see [`crate::bash::environment`]), started on
//! the first command of a work item and kept until the work item ends or a command times out. Its directory and
//! exported variables persist from one call to the next. Each command is written to the shell's input inside a
//! group with its standard input closed, followed by a line that echoes a marker and the exit status, so the end
//! of the command is found in the output. The marker carries a per-shell nonce, which a command cannot guess.
//!
//! A background command (`background: true`) runs in its own process group, with its output in a file under
//! `.plaza/jobs/`. It returns its job id at once; when it ends, a note with its exit status and its last lines
//! goes into the next result of any tool. At most [`MAX_JOBS`] run at once.
//!
//! Arguments: `command` (required), `cwd` (optional; a directory in the workspace, the command runs from it),
//! `timeout_s` (optional; [`DEFAULT_TIMEOUT_S`] by default, at most [`MAX_TIMEOUT_S`]; a background job's default
//! and cap are [`BACKGROUND_TIMEOUT_S`]), and `background` (optional).
//!
//! Output: `{"output", "exit"}`, with `truncated` and `continue` when the output went past [`RUN_BUDGET`] (its head
//! and its tail are kept), `timed_out` when the shell was killed, and `job` for a background command.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use apz_proto::wire::cap_output;
use serde_json::{Value, json};

use crate::bash::{self, Privilege, SHELL};
use crate::files::{self, PLAZA_DIR, Workspace};
use crate::tools::ToolError;

use super::{Lane, locked};

/// The timeout a foreground command gets when it is not told one: 60 seconds.
pub const DEFAULT_TIMEOUT_S: u64 = 60;

/// The longest foreground timeout: 300 seconds.
pub const MAX_TIMEOUT_S: u64 = 300;

/// The timeout of a background command, by default and at most: an hour.
pub const BACKGROUND_TIMEOUT_S: u64 = 3600;

/// The most output a `run` result carries: 32 KiB, the head and the tail of a longer one.
pub const RUN_BUDGET: usize = 32 * 1024;

/// The most background jobs that run at once.
pub const MAX_JOBS: usize = 8;

/// How many of its last lines a finished background job reports, and at most this many bytes of them.
const JOB_TAIL_LINES: usize = 10;
const JOB_TAIL_BYTES: u64 = 1024;

/// How long a stopped job's process group gets after SIGTERM before SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// How often a background job is checked for its end.
const POLL: Duration = Duration::from_millis(50);

/// Source of the nonces that mark the end of a shell's commands.
static NONCES: AtomicU64 = AtomicU64::new(1);

/// The persistent shell of a work item: one `/bin/sh`, its input, and what its output has said.
pub(crate) struct Shell {
    child: Child,
    input: ChildStdin,
    output: Receiver<Vec<u8>>,
    nonce: String,
    /// Output read from the shell and not yet taken by a command.
    pending: Vec<u8>,
}

/// What one command of the shell did.
pub(crate) struct Exec {
    /// The output the command printed, up to its end (or the kill).
    pub(crate) output: String,
    /// The exit status, when the command ended by itself with one.
    pub(crate) exit: Option<i32>,
    /// The timeout passed: the shell was killed.
    pub(crate) timed_out: bool,
    /// The shell is gone after this command: it exited, or it was killed.
    pub(crate) ended: bool,
}

impl Shell {
    /// Starts the shell in `dir`, with the agent's environment.
    fn start(dir: &Path) -> Result<Shell, ToolError> {
        let (reader, writer) =
            io::pipe().map_err(|err| internal(format!("cannot open the shell's output: {err}")))?;
        let stderr = writer
            .try_clone()
            .map_err(|err| internal(format!("cannot open the shell's output: {err}")))?;
        let mut command = Command::new(SHELL);
        command
            .stdin(Stdio::piped())
            .stdout(writer)
            .stderr(stderr)
            .process_group(0);
        bash::environment(&mut command, dir, Privilege::Agent)
            .map_err(|reason| ToolError::Unavailable { reason })?;
        let mut child = command
            .spawn()
            .map_err(|err| internal(format!("cannot start the shell: {err}")))?;
        // The command holds the pipe's write ends; dropping it lets the reader see the shell's end.
        drop(command);
        let input = child
            .stdin
            .take()
            .ok_or_else(|| internal("the shell has no input".to_string()))?;
        let (chunks, output) = mpsc::channel();
        thread::spawn(move || read_into(reader, &chunks));
        Ok(Shell {
            child,
            input,
            output,
            nonce: format!(
                "{:x}{:x}",
                std::process::id(),
                NONCES.fetch_add(1, Ordering::Relaxed)
            ),
            pending: Vec::new(),
        })
    }

    /// Runs `script` in the shell and waits for its end, or for `timeout`. A shell that times out is killed, and
    /// one that exits is gone: the caller starts a new one for the next command.
    fn exec(&mut self, script: &str, timeout: Duration) -> Exec {
        let marker = format!("__APZ_END_{}__", self.nonce);
        let written = self
            .input
            .write_all(
                format!("{script}\n__apz_status=$?; echo \"{marker} $__apz_status\"\n").as_bytes(),
            )
            .and_then(|()| self.input.flush());
        if written.is_err() {
            return self.ended();
        }
        let deadline = Instant::now() + timeout;
        loop {
            if let Some((output, exit)) = take_until(&mut self.pending, &marker) {
                return Exec {
                    output,
                    exit,
                    timed_out: false,
                    ended: false,
                };
            }
            match self
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(chunk) => self.pending.extend(chunk),
                Err(RecvTimeoutError::Timeout) => {
                    bash::kill_group(&mut self.child);
                    let _ = self.child.wait();
                    return Exec {
                        output: lossy(&self.pending),
                        exit: None,
                        timed_out: true,
                        ended: true,
                    };
                }
                Err(RecvTimeoutError::Disconnected) => return self.ended(),
            }
        }
    }

    /// What is left of the shell once it is gone: its exit status, and the output it printed.
    fn ended(&mut self) -> Exec {
        let exit = self.child.wait().ok().and_then(exit_code);
        Exec {
            output: lossy(&self.pending),
            exit,
            timed_out: false,
            ended: true,
        }
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        bash::kill_group(&mut self.child);
        let _ = self.child.wait();
    }
}

/// Copies what the shell prints into the channel until the pipe closes.
fn read_into(mut reader: io::PipeReader, chunks: &mpsc::Sender<Vec<u8>>) {
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                if chunks.send(chunk[..read].to_vec()).is_err() {
                    break;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

/// Takes the output up to `marker` from `pending`, and the exit status the marker line carries. None until the
/// marker has arrived.
fn take_until(pending: &mut Vec<u8>, marker: &str) -> Option<(String, Option<i32>)> {
    let at = find(pending, marker.as_bytes())?;
    let output = lossy(&pending[..at]);
    let rest = &pending[at + marker.len()..];
    let line_end = rest
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(rest.len());
    let exit = String::from_utf8_lossy(&rest[..line_end])
        .trim()
        .parse::<i32>()
        .ok();
    let consumed = (at + marker.len() + line_end + 1).min(pending.len());
    pending.drain(..consumed);
    Some((output, exit))
}

/// The first place `needle` occurs in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Bytes as text, invalid sequences replaced.
fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The exit code of a status, or None when a signal ended it.
fn exit_code(status: ExitStatus) -> Option<i32> {
    status.code()
}

/// `exit <code>`, or the signal that ended the process: how a background job reports its end.
fn describe(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exit {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => "exit unknown".to_string(),
    }
}

/// An internal failure of this side, in the taxonomy.
fn internal(reason: String) -> ToolError {
    ToolError::Internal { reason }
}

/// One background job: its process, and the file its output goes to.
pub(crate) struct Job {
    id: u64,
    child: std::sync::Mutex<Child>,
    log: PathBuf,
}

/// The background jobs of a workspace that run, and the number of the last one started.
#[derive(Default)]
pub(crate) struct Jobs {
    last: u64,
    running: Vec<Arc<Job>>,
}

/// Runs a `run` call.
pub(crate) fn run(
    workspace: &Workspace,
    lane: &Arc<Lane>,
    input: &Value,
) -> Result<Value, ToolError> {
    let command = match input.get("command") {
        Some(Value::String(command)) if !command.trim().is_empty() => command.clone(),
        _ => return Err(files::invalid("\"command\" must be a non-empty string")),
    };
    let background = match input.get("background") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => return Err(files::invalid("\"background\" must be true or false")),
    };
    let cwd = cwd_of(workspace, input)?;
    // The project's run hooks see the command before it starts: a block-mode finding refuses it (hooks.rs).
    let checked = crate::hooks::before_run(lane, &command)?;
    if background {
        let timeout = timeout_of(input, BACKGROUND_TIMEOUT_S, BACKGROUND_TIMEOUT_S)?;
        let mut reply = start_job(workspace, lane, &command, cwd.as_deref(), timeout)?;
        if !checked.is_empty() {
            reply["check"] = json!({ "summary": checked.join("; ") });
        }
        return Ok(reply);
    }
    let timeout = timeout_of(input, DEFAULT_TIMEOUT_S, MAX_TIMEOUT_S)?;
    let prefix = match &cwd {
        Some(dir) => format!("cd {} && ", quote(&dir.display().to_string())),
        None => String::new(),
    };
    let script = format!("{prefix}{{ {command}\n}} </dev/null");

    let exec = foreground(workspace, lane, &script, timeout)?;
    let truncated = exec.output.len() > RUN_BUDGET;
    let mut output = if truncated {
        cap_output(&exec.output, RUN_BUDGET)
    } else {
        exec.output.clone()
    };
    let mut reply = json!({ "exit": exec.exit });
    if exec.timed_out {
        output.push_str(&format!(
            "\n[timed out after {timeout} s; the shell was killed, and the next command starts a new one]\n"
        ));
        reply["timed_out"] = json!(true);
    } else if exec.ended {
        output.push_str("\n[the shell ended; the next command starts a new one]\n");
        reply["shell_ended"] = json!(true);
    }
    if truncated {
        reply["truncated"] = json!(true);
        reply["continue"] = json!(format!(
            "the middle {} bytes of the output are cut; run the command again with its output in a file (command > out.txt 2>&1) and read that file with read",
            exec.output.len() - RUN_BUDGET
        ));
    }
    if !checked.is_empty() {
        reply["check"] = json!({ "summary": checked.join("; ") });
    }
    reply["output"] = json!(output);
    Ok(reply)
}

/// Runs `script` in the workspace's persistent shell, starting the shell when the work item has none yet, and
/// waits for it to end or for `timeout_s`. A shell that timed out or ended is dropped: the next command starts a
/// new one.
pub(crate) fn foreground(
    workspace: &Workspace,
    lane: &Lane,
    script: &str,
    timeout_s: u64,
) -> Result<Exec, ToolError> {
    let mut slot = locked(&lane.shell);
    if slot.is_none() {
        *slot = Some(Shell::start(workspace.dir())?);
    }
    let shell = slot
        .as_mut()
        .ok_or_else(|| internal("the shell is missing".to_string()))?;
    let exec = shell.exec(script, Duration::from_secs(timeout_s));
    if exec.ended {
        *slot = None;
    }
    Ok(exec)
}

/// The `cwd` argument as an absolute path inside the workspace, which must be a directory.
fn cwd_of(workspace: &Workspace, input: &Value) -> Result<Option<PathBuf>, ToolError> {
    let raw = match input.get("cwd") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(raw)) => raw.clone(),
        Some(_) => return Err(files::invalid("\"cwd\" must be a string")),
    };
    let (path, label) = workspace.resolve(&raw)?;
    if !fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(files::invalid(format!("{label} is not a directory")));
    }
    Ok(Some(path))
}

/// The `timeout_s` argument: `default` when absent, and at most `cap`.
fn timeout_of(input: &Value, default: u64, cap: u64) -> Result<u64, ToolError> {
    match input.get("timeout_s") {
        None | Some(Value::Null) => Ok(default),
        Some(value) => match value.as_u64() {
            Some(secs) if secs >= 1 => Ok(secs.min(cap)),
            _ => Err(files::invalid(
                "\"timeout_s\" must be a whole number of seconds",
            )),
        },
    }
}

/// Single-quotes `text` for the shell.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Starts a background command and returns its job id.
fn start_job(
    workspace: &Workspace,
    lane: &Arc<Lane>,
    command: &str,
    cwd: Option<&Path>,
    timeout: u64,
) -> Result<Value, ToolError> {
    let mut jobs = locked(&lane.jobs);
    if jobs.running.len() >= MAX_JOBS {
        return Err(files::limit(format!(
            "{MAX_JOBS} background jobs already run; wait for one to end"
        )));
    }
    let id = jobs.last + 1;
    let folder = workspace.plaza_dir().join("jobs");
    files::create_dirs(&folder)
        .map_err(|err| files::io_tool_error(err, "cannot create the jobs folder"))?;
    let log = folder.join(format!("{id}.log"));
    let output = File::create(&log)
        .map_err(|err| files::io_tool_error(err, format!("cannot open job {id}'s output")))?;
    let errors = output
        .try_clone()
        .map_err(|err| files::io_tool_error(err, format!("cannot open job {id}'s output")))?;
    let dir = cwd.unwrap_or_else(|| workspace.dir());
    let mut process = bash::command(SHELL, &["-c", command], dir, Privilege::Agent)
        .map_err(|reason| ToolError::Unavailable { reason })?;
    process.stdout(output).stderr(errors);
    let child = process
        .spawn()
        .map_err(|err| internal(format!("cannot start job {id}: {err}")))?;
    drop(process);
    jobs.last = id;
    let job = Arc::new(Job {
        id,
        child: std::sync::Mutex::new(child),
        log: log.clone(),
    });
    jobs.running.push(Arc::clone(&job));
    drop(jobs);

    let watched = Arc::clone(lane);
    let timeout = Duration::from_secs(timeout);
    thread::spawn(move || watch(&watched, &job, timeout));
    Ok(json!({
        "job": id,
        "log": format!("{PLAZA_DIR}/jobs/{id}.log"),
        "started": true,
    }))
}

/// Waits for a background job to end, or kills it at its timeout, then notes how it ended.
fn watch(lane: &Lane, job: &Job, timeout: Duration) {
    let started = Instant::now();
    let (status, timed_out) = loop {
        {
            let mut child = locked(&job.child);
            match child.try_wait() {
                Ok(Some(status)) => break (Some(status), false),
                Ok(None) => {}
                Err(_) => break (None, false),
            }
            if started.elapsed() >= timeout {
                bash::kill_group(&mut child);
                break (child.wait().ok(), true);
            }
        }
        thread::sleep(POLL);
    };
    let how = match (timed_out, status) {
        (true, _) => format!("timed out after {} s; killed", timeout.as_secs()),
        (false, Some(status)) => describe(status),
        (false, None) => "exit unknown".to_string(),
    };
    lane.note(format!(
        "job {} ended: {how}, last lines:\n{}",
        job.id,
        tail(&job.log)
    ));
    locked(&lane.jobs)
        .running
        .retain(|other| other.id != job.id);
}

/// The last lines of a job's output file, at most [`JOB_TAIL_LINES`] lines and [`JOB_TAIL_BYTES`] bytes.
fn tail(path: &Path) -> String {
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    let length = file.metadata().map_or(0, |metadata| metadata.len());
    let from = length.saturating_sub(JOB_TAIL_BYTES);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(from)).is_err() || file.read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    let text = lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(JOB_TAIL_LINES)..].join("\n")
}

/// Stops what the workspace's agent runs here: the persistent shell, and every background job. Each job's process
/// group gets SIGTERM, then SIGKILL once [`STOP_GRACE`] has passed or the groups are gone.
pub(crate) fn stop(lane: &Lane) {
    let shell = locked(&lane.shell).take();
    drop(shell);
    let jobs: Vec<Arc<Job>> = std::mem::take(&mut locked(&lane.jobs).running);
    for job in &jobs {
        bash::signal_group(&locked(&job.child), "-TERM");
    }
    let until = Instant::now() + STOP_GRACE;
    while Instant::now() < until
        && jobs
            .iter()
            .any(|job| bash::group_alive(&mut locked(&job.child)))
    {
        thread::sleep(POLL);
    }
    for job in &jobs {
        let mut child = locked(&job.child);
        bash::signal_group(&child, "-KILL");
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::{lane_of, serve};

    /// A scratch workspace, empty.
    fn workspace(name: &str) -> Workspace {
        let root = std::env::temp_dir().join(format!("apz-v2-run-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub")).expect("a scratch workspace");
        Workspace::at(root)
    }

    /// A `run` call's reply, parsed.
    fn run_in(ws: &Workspace, work: u64, input: Value) -> Value {
        let text = serve(ws, work, "run", &input).expect("a run");
        serde_json::from_str(&text).expect("JSON")
    }

    #[test]
    fn the_shell_keeps_its_directory_and_its_variables_between_calls() {
        let ws = workspace("persist");
        run_in(
            &ws,
            1,
            json!({ "command": "cd sub && export APZ_KEPT=yes" }),
        );
        let reply = run_in(
            &ws,
            1,
            json!({ "command": "echo $APZ_KEPT; basename \"$PWD\"" }),
        );
        assert_eq!(reply["output"], "yes\nsub\n", "{reply}");
        assert_eq!(reply["exit"], 0);
    }

    #[test]
    fn a_command_runs_in_the_workspace_with_a_clean_environment() {
        let ws = workspace("env");
        let reply = run_in(
            &ws,
            1,
            json!({ "command": "echo \"$HOME\"; echo \"$PATH\"; cat; echo hi" }),
        );
        let lines: Vec<&str> = reply["output"]
            .as_str()
            .unwrap_or_default()
            .lines()
            .collect();
        assert_eq!(lines.len(), 3, "{reply}");
        assert_eq!(lines[0], ws.dir().display().to_string());
        assert!(lines[1].starts_with("/usr/local/sbin:"), "{reply}");
        assert_eq!(lines[2], "hi");
    }

    #[test]
    fn a_failing_command_reports_its_status_and_the_shell_lives_on() {
        let ws = workspace("status");
        let reply = run_in(&ws, 1, json!({ "command": "echo nope >&2; false" }));
        assert_eq!(reply["exit"], 1, "{reply}");
        assert_eq!(reply["output"], "nope\n");
        assert!(reply.get("shell_ended").is_none());
    }

    #[test]
    fn a_new_work_item_starts_a_new_shell() {
        let ws = workspace("work");
        run_in(&ws, 1, json!({ "command": "export APZ_ONCE=1" }));
        let reply = run_in(&ws, 2, json!({ "command": "echo \"[$APZ_ONCE]\"" }));
        assert_eq!(reply["output"], "[]\n", "{reply}");
    }

    #[test]
    fn a_command_past_its_timeout_kills_the_shell_and_says_so() {
        let ws = workspace("timeout");
        let started = Instant::now();
        let reply = run_in(&ws, 1, json!({ "command": "sleep 30", "timeout_s": 1 }));
        assert_eq!(reply["timed_out"], true, "{reply}");
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
        assert!(
            reply["output"]
                .as_str()
                .unwrap()
                .contains("timed out after 1 s"),
            "{reply}"
        );
        let next = run_in(&ws, 1, json!({ "command": "echo back" }));
        assert_eq!(next["output"], "back\n");
    }

    #[test]
    fn a_long_output_keeps_its_head_and_tail_and_marks_the_cut() {
        let ws = workspace("budget");
        let reply = run_in(
            &ws,
            1,
            json!({ "command": "i=0; while [ $i -lt 6000 ]; do echo \"line $i of the output\"; i=$((i+1)); done" }),
        );
        let output = reply["output"].as_str().unwrap();
        assert!(output.starts_with("line 0 of"), "{}", &output[..40]);
        assert!(output.contains("bytes cut"), "the cut is marked");
        assert!(output.ends_with("line 5999 of the output\n"));
        assert_eq!(reply["truncated"], true);
        assert!(
            reply["continue"]
                .as_str()
                .unwrap()
                .contains("bytes of the output are cut")
        );
    }

    #[test]
    fn a_background_job_returns_at_once_and_its_end_is_noted_in_a_result() {
        let ws = workspace("job");
        let started = run_in(
            &ws,
            1,
            json!({ "command": "echo done-soon; exit 0", "background": true }),
        );
        let id = started["job"].as_u64().expect("a job id");
        assert_eq!(started["log"], format!("{PLAZA_DIR}/jobs/{id}.log"));
        let deadline = Instant::now() + Duration::from_secs(20);
        // A fast job can finish before serve drains the notes for the start reply.
        // That reply counts too: the note is delivered once, not repeated later.
        let mut text = started.to_string();
        loop {
            if text.contains(&format!("job {id} ended: exit 0")) {
                assert!(
                    text.contains("done-soon"),
                    "the last lines come with the note: {text}"
                );
                break;
            }
            assert!(Instant::now() < deadline, "the job's end was never noted");
            thread::sleep(POLL);
            text = serve(&ws, 1, "run", &json!({ "command": "true" })).expect("a run");
        }
    }

    #[test]
    fn stop_ends_the_shell_and_the_background_jobs() {
        let ws = workspace("stop");
        run_in(&ws, 1, json!({ "command": "export APZ_LIVE=1" }));
        let job = run_in(&ws, 1, json!({ "command": "sleep 60", "background": true }));
        assert!(job["job"].as_u64().is_some());
        let started = Instant::now();
        stop(&lane_of(&ws));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert!(locked(&lane_of(&ws).jobs).running.is_empty());
        let fresh = run_in(&ws, 1, json!({ "command": "echo \"[$APZ_LIVE]\"" }));
        assert_eq!(fresh["output"], "[]\n");
    }

    #[test]
    fn a_cwd_outside_the_workspace_or_not_a_directory_is_refused() {
        let ws = workspace("cwd");
        let err = serve(&ws, 1, "run", &json!({ "command": "true", "cwd": "../x" }))
            .expect_err("outside");
        assert!(err.to_string().starts_with("denied:"), "{err}");
        let err = serve(
            &ws,
            1,
            "run",
            &json!({ "command": "true", "cwd": "missing" }),
        )
        .expect_err("missing");
        assert!(
            err.to_string().starts_with("not found:")
                || err.to_string().contains("not a directory"),
            "{err}"
        );
    }

    #[test]
    fn quoting_keeps_a_directory_with_a_quote_in_it_one_word() {
        assert_eq!(quote("a'b"), "'a'\\''b'");
    }
}
