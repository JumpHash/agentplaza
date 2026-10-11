//! Background shell commands: `bash_start` runs a command in the background and returns
//! an id, `bash_output` reads what it printed since last time, `bash_kill` stops it.
//!
//! A job runs as the `bash` tool's command does (the same environment, uid and process
//! group; see [`bash::command`]). Its stdout and stderr share one pipe, which a thread
//! reads into the job's buffer, keeping the newest 1 MiB. At most 8 jobs run at once.
//! A job that has exited stays readable until it is killed, or until the registry needs
//! its place for a new job.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use apz_proto::wire::cap_output;
use serde_json::Value;

use crate::bash::{self, Privilege, SHELL};
use crate::files::Workspace;

/// Jobs that run at once; the registry holds no more than this many.
const MAX_JOBS: usize = 8;
/// The newest output a job keeps; older output is dropped.
const MAX_BUFFER_BYTES: usize = 1 << 20;
/// The most output one `bash_output` call returns.
const MAX_OUTPUT_BYTES: usize = 32 << 10;
const MAX_WAIT_S: u64 = 60;
const POLL: Duration = Duration::from_millis(20);
/// How long the output of an exited job is awaited while its pipe is still open.
const DRAIN: Duration = Duration::from_millis(500);

/// Every job not yet killed, by id.
static JOBS: OnceLock<Mutex<HashMap<String, Arc<Job>>>> = OnceLock::new();
/// The number of the next job: `j1`, `j2`, …
static NEXT: AtomicU64 = AtomicU64::new(1);

struct Job {
    /// The job's number; the lowest is the oldest.
    seq: u64,
    child: Mutex<Child>,
    output: Arc<Mutex<Output>>,
}

#[derive(Default)]
struct Output {
    /// The newest bytes printed, at most `MAX_BUFFER_BYTES`.
    bytes: VecDeque<u8>,
    /// The offset, in everything printed, of `bytes[0]`.
    start: u64,
    /// Everything printed so far, as an offset.
    total: u64,
    /// The offset up to which `bash_output` has returned.
    read: u64,
    /// The pipe has closed: no process of the job writes to it any more.
    closed: bool,
}

/// Starts a background command; returns its id.
pub fn start(workspace: &Workspace, input: &Value) -> Result<String, String> {
    let script = match input.get("command").and_then(Value::as_str) {
        Some(script) if !script.trim().is_empty() => script,
        _ => return Err("invalid input: \"command\" must be a non-empty string".to_string()),
    };
    let mut registry = locked(jobs());
    let mut running = Vec::new();
    let mut finished = Vec::new();
    for (id, job) in registry.iter() {
        let entry = (job.seq, id.clone());
        if is_running(job) {
            running.push(entry);
        } else {
            finished.push(entry);
        }
    }
    if running.len() >= MAX_JOBS {
        return Err(format!(
            "{MAX_JOBS} jobs already run; kill one first (running: {})",
            describe_ids(running)
        ));
    }
    if registry.len() >= MAX_JOBS {
        // A finished job gives up its place: the oldest one goes. One exists, since
        // fewer than MAX_JOBS run and the registry is full.
        if let Some(oldest) = finished.iter().min().map(|(_, id)| id.clone()) {
            registry.remove(&oldest);
        }
    }

    let (reader, writer) = io::pipe().map_err(|err| format!("internal: cannot open the output pipe: {err}"))?;
    let stderr = writer.try_clone().map_err(|err| format!("internal: cannot open the output pipe: {err}"))?;
    let mut command = bash::command(SHELL, &["-c", script], workspace.dir(), Privilege::Agent)?;
    command.stdout(writer).stderr(stderr);
    let child = command
        .spawn()
        .map_err(|err| format!("internal: cannot start the shell: {err}"))?;
    // The command holds the pipe's write ends; dropping it lets the reader see the end.
    drop(command);

    let seq = NEXT.fetch_add(1, Ordering::Relaxed);
    let id = format!("j{seq}");
    let buffer = Arc::new(Mutex::new(Output::default()));
    let sink = Arc::clone(&buffer);
    thread::spawn(move || read_into(reader, &sink));
    registry.insert(
        id.clone(),
        Arc::new(Job {
            seq,
            child: Mutex::new(child),
            output: buffer,
        }),
    );
    Ok(format!("started job {id}"))
}

/// New output of a background command and whether it still runs.
pub fn output(_workspace: &Workspace, input: &Value) -> Result<String, String> {
    let id = job_id(input)?;
    let wait_s = match input.get("wait_s") {
        None | Some(Value::Null) => 0,
        Some(value) => value
            .as_u64()
            .filter(|secs| *secs <= MAX_WAIT_S)
            .ok_or_else(|| format!("invalid input: \"wait_s\" must be a whole number from 0 to {MAX_WAIT_S}"))?,
    };
    let job = find(&id)?;
    let deadline = Instant::now() + Duration::from_secs(wait_s);
    let mut exit = loop {
        if let Some(status) = exit_status(&job)? {
            break Some(status);
        }
        if has_new(&job) || Instant::now() >= deadline {
            break None;
        }
        thread::sleep(POLL);
    };
    if exit.is_none() {
        exit = exit_status(&job)?;
    }
    if exit.is_some() {
        // The shell is gone, but its output may still be in the pipe: wait for the pipe to close.
        let until = Instant::now() + DRAIN;
        while !locked(&job.output).closed && Instant::now() < until {
            thread::sleep(POLL);
        }
    }
    let mut body = take_new(&mut locked(&job.output));
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    Ok(format!("{body}{}", describe(exit)))
}

/// Stops a background command and drops it from the registry.
pub fn kill(_workspace: &Workspace, input: &Value) -> Result<String, String> {
    let id = job_id(input)?;
    let job = {
        let mut registry = locked(jobs());
        match registry.remove(&id) {
            Some(job) => job,
            None => return Err(unknown(&id, &registry)),
        }
    };
    let mut child = locked(&job.child);
    let was_running = matches!(child.try_wait(), Ok(None));
    // The whole process group goes: the shell and whatever it started.
    crate::bash::kill_group(&mut child);
    let _ = child.wait();
    Ok(if was_running {
        format!("killed job {id}")
    } else {
        format!("job {id} had already exited")
    })
}

fn jobs() -> &'static Mutex<HashMap<String, Arc<Job>>> {
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A lock whose holder panicked stays usable: the data it guards is still consistent here.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn job_id(input: &Value) -> Result<String, String> {
    input
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "invalid input: \"id\" must be a job id such as j1".to_string())
}

fn find(id: &str) -> Result<Arc<Job>, String> {
    let registry = locked(jobs());
    registry.get(id).cloned().ok_or_else(|| unknown(id, &registry))
}

fn unknown(id: &str, registry: &HashMap<String, Arc<Job>>) -> String {
    let live = registry.iter().map(|(name, job)| (job.seq, name.clone())).collect();
    format!("unknown job {id:?}; live jobs: {}", describe_ids(live))
}

/// The ids in the order the jobs were started, or `none`.
fn describe_ids(mut ids: Vec<(u64, String)>) -> String {
    ids.sort();
    if ids.is_empty() {
        "none".to_string()
    } else {
        ids.into_iter().map(|(_, id)| id).collect::<Vec<_>>().join(", ")
    }
}

fn is_running(job: &Job) -> bool {
    matches!(locked(&job.child).try_wait(), Ok(None))
}

fn exit_status(job: &Job) -> Result<Option<ExitStatus>, String> {
    locked(&job.child)
        .try_wait()
        .map_err(|err| format!("internal: cannot check the job: {err}"))
}

fn has_new(job: &Job) -> bool {
    let buffer = locked(&job.output);
    buffer.total > buffer.read
}

/// Copies what the pipe carries into the job's buffer until the pipe closes.
fn read_into(mut reader: io::PipeReader, output: &Mutex<Output>) {
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                let mut buffer = locked(output);
                buffer.bytes.extend(&chunk[..read]);
                buffer.total += read as u64;
                let excess = buffer.bytes.len().saturating_sub(MAX_BUFFER_BYTES);
                buffer.bytes.drain(..excess);
                buffer.start += excess as u64;
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    locked(output).closed = true;
}

/// The output since the last call, capped to `MAX_OUTPUT_BYTES`. Output dropped from the
/// buffer before it was read is counted in a note.
fn take_new(buffer: &mut Output) -> String {
    let lost = buffer.start.saturating_sub(buffer.read);
    let from = (buffer.read.max(buffer.start) - buffer.start) as usize;
    let fresh: Vec<u8> = buffer.bytes.range(from..).copied().collect();
    buffer.read = buffer.total;
    let text = cap_output(&String::from_utf8_lossy(&fresh), MAX_OUTPUT_BYTES);
    if lost > 0 {
        format!("[{lost} bytes lost before this read: the job keeps its newest 1 MiB]\n{text}")
    } else {
        text
    }
}

/// The status line: `[running]`, or how the shell ended.
fn describe(exit: Option<ExitStatus>) -> String {
    match exit {
        None => "[running]".to_string(),
        Some(status) => match (status.code(), status.signal()) {
            (Some(code), _) => format!("[exited {code}]"),
            (None, Some(signal)) => format!("[exited: killed by signal {signal}]"),
            (None, None) => "[exited: unknown]".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The registry is process-wide, so the tests that start jobs take turns.
    static TURN: Mutex<()> = Mutex::new(());

    fn turn() -> MutexGuard<'static, ()> {
        locked(&TURN)
    }

    fn workspace(name: &str) -> Workspace {
        let dir = std::env::temp_dir().join(format!("apz-harness-shells-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    /// Starts `command` and returns its job id.
    fn spawn(workspace: &Workspace, command: &str) -> String {
        let reply = start(workspace, &json!({ "command": command })).expect("a started job");
        reply.strip_prefix("started job ").expect("the reply names the job").to_string()
    }

    #[test]
    fn output_is_incremental_and_the_exit_status_shows() {
        let _turn = turn();
        let ws = workspace("incremental");
        let id = spawn(&ws, "echo hi; sleep 0.2; echo bye");
        let mut replies = Vec::new();
        for _ in 0..50 {
            let reply = output(&ws, &json!({ "id": id, "wait_s": 5 })).expect("the job's output");
            let done = reply.ends_with("[exited 0]");
            replies.push(reply);
            if done {
                break;
            }
        }
        assert!(replies[0].contains("hi"), "{replies:?}");
        let all = replies.concat();
        assert_eq!(all.matches("hi").count(), 1, "{all:?}");
        assert!(all.contains("bye") && all.ends_with("[exited 0]"), "{all:?}");
        kill(&ws, &json!({ "id": id })).expect("the job is dropped");
    }

    #[test]
    fn kill_stops_a_job_and_drops_it() {
        let _turn = turn();
        let ws = workspace("kill");
        let id = spawn(&ws, "sleep 30");
        let started = Instant::now();
        let reply = kill(&ws, &json!({ "id": id })).expect("the job is killed");
        assert_eq!(reply, format!("killed job {id}"));
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        let err = output(&ws, &json!({ "id": id })).expect_err("the job is gone");
        assert!(err.starts_with("unknown job"), "{err}");
    }

    #[test]
    fn an_unknown_id_is_an_error_that_lists_the_live_jobs() {
        let _turn = turn();
        let ws = workspace("unknown");
        let id = spawn(&ws, "sleep 30");
        let err = output(&ws, &json!({ "id": "j999999" })).expect_err("no such job");
        assert!(err.starts_with("unknown job \"j999999\""), "{err}");
        assert!(err.contains(&id), "{err}");
        kill(&ws, &json!({ "id": id })).expect("the job is killed");
    }

    #[test]
    fn at_most_eight_jobs_run_at_once() {
        let _turn = turn();
        let ws = workspace("limit");
        let ids: Vec<String> = (0..MAX_JOBS).map(|_| spawn(&ws, "sleep 30")).collect();
        let err = start(&ws, &json!({ "command": "sleep 30" })).expect_err("a ninth job");
        assert!(err.contains("already run"), "{err}");
        kill(&ws, &json!({ "id": ids[0].as_str() })).expect("the job is killed");
        let next = spawn(&ws, "sleep 30");
        for id in ids.iter().skip(1).chain(std::iter::once(&next)) {
            kill(&ws, &json!({ "id": id.as_str() })).expect("the job is killed");
        }
    }
}
