//! `process.Process/*` (E2B's `spec/envd/process/process.proto`): start a process (optionally on a
//! PTY), stream its output and exit, list the ones still listed, attach to one, send it input and
//! signals, and resize its PTY.
//!
//! **The table.** Every process started here is listed, keyed by pid, in one table shared by every
//! connection, until its end event has been handed to a stream. So a `List` or a `Connect` made
//! while it runs, or after it ended but before its end was read, still finds it. A process's events
//! wait in its queue for whichever stream takes them; a stream that attaches late gets what has not
//! been delivered yet, not a replay.
//!
//! **Unsafe.** The PTY calls (`openpty`, `TIOCSWINSZ`, `TIOCSCTTY`), `setsid` and `kill` are not in
//! `std`. Each block says what it relies on. The PTY calls belong with apz-ops' terminal module,
//! which has its own copy of them for the IDE's terminals.
#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Map, Value, json};

use super::{Ctx, RpcError, StreamSink};
use crate::bash;

/// How long a stream waits for output before it sends a keepalive, so an idle connection is not
/// taken for dead.
const KEEPALIVE: Duration = Duration::from_secs(50);
/// The most bytes one output read takes.
const READ_CHUNK: usize = 32 << 10;
/// The terminal type a PTY process sees.
const TERM: &str = "xterm-256color";

/// Every listed process, by pid.
static TABLE: LazyLock<Mutex<BTreeMap<u32, Arc<Proc>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// A unary method (`List`, `SendInput`, `SendSignal`, `CloseStdin`, `Update`).
pub fn unary(method: &str, request: &Value, _ctx: &Ctx) -> Result<Value, RpcError> {
    match method {
        "List" => Ok(list()),
        "SendInput" => send_input(request).map(|()| json!({})),
        "SendSignal" => send_signal(request).map(|()| json!({})),
        "CloseStdin" => close_stdin(request).map(|()| json!({})),
        "Update" => update(request).map(|()| json!({})),
        _ => Err(unimplemented(method)),
    }
}

/// A server-streaming method (`Start`, `Connect`): writes its events to `sink` until the process
/// ends or the client goes away (a failed write).
pub fn stream(
    method: &str,
    request: &Value,
    ctx: &Ctx,
    sink: &mut dyn StreamSink,
) -> Result<(), RpcError> {
    match method {
        "Start" => {
            let proc = spawn(request, ctx)?;
            deliver(&proc, sink)
        }
        "Connect" => {
            let proc = find(&selector(request.get("process"))?)?;
            deliver(&proc, sink)
        }
        _ => Err(unimplemented(method)),
    }
}

fn unimplemented(method: &str) -> RpcError {
    RpcError::new("unimplemented", format!("process.Process/{method}"))
}

/// One listed process.
struct Proc {
    pid: u32,
    tag: Option<String>,
    cmd: String,
    args: Vec<String>,
    envs: Vec<(String, String)>,
    cwd: String,
    /// The PTY's master, for a process started on a PTY.
    master: Option<Arc<File>>,
    /// The process's stdin pipe. Taken by `CloseStdin` (and by the end of the process, which
    /// closes it too); absent for a PTY, whose input goes to `master`.
    stdin: Mutex<Option<ChildStdin>>,
    queue: Mutex<Queue>,
    woken: Condvar,
}

/// Events not yet handed to a stream. `ended` is set with the end event, which is the last.
#[derive(Default)]
struct Queue {
    events: Vec<Value>,
    ended: bool,
}

impl Proc {
    fn push(&self, event: Value) {
        lock(&self.queue).events.push(event);
        self.woken.notify_all();
    }

    fn finish(&self, end: Value) {
        let mut queue = lock(&self.queue);
        queue.events.push(end);
        queue.ended = true;
        self.woken.notify_all();
    }

    /// Puts back events a stream could not deliver, ahead of any queued since.
    fn requeue(&self, mut rest: Vec<Value>) {
        if rest.is_empty() {
            return;
        }
        let mut queue = lock(&self.queue);
        rest.append(&mut queue.events);
        queue.events = rest;
        self.woken.notify_all();
    }

    fn has_ended(&self) -> bool {
        lock(&self.queue).ended
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts the process `request.process` describes, lists it, and starts the threads that feed its
/// queue: one per output, and one that waits for the exit and then queues the end event.
fn spawn(request: &Value, ctx: &Ctx) -> Result<Arc<Proc>, RpcError> {
    let process = request
        .get("process")
        .filter(|value| value.is_object())
        .ok_or_else(|| invalid("process is required"))?;
    let cmd = process
        .get("cmd")
        .and_then(Value::as_str)
        .filter(|cmd| !cmd.is_empty())
        .ok_or_else(|| invalid("process.cmd must be a non-empty string"))?;
    let args = string_list(process.get("args"), "process.args")?;
    let envs = string_map(process.get("envs"), "process.envs")?;
    let dir = match process.get("cwd").and_then(Value::as_str) {
        Some(cwd) if Path::new(cwd).is_absolute() => PathBuf::from(cwd),
        Some(cwd) => ctx.home.join(cwd),
        None => ctx.home.clone(),
    };
    if !dir.is_dir() {
        return Err(invalid(format!("{} is not a directory", dir.display())));
    }
    let pty = match request.get("pty") {
        None | Some(Value::Null) => None,
        Some(pty) => Some(pty_size(pty)?),
    };
    let stdin = request
        .get("stdin")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tag = request
        .get("tag")
        .and_then(Value::as_str)
        .map(str::to_string);

    // The guest's own environment (the proxy, the CA bundle, the PATH), then the sandbox's, then the
    // request's: the last value of a name wins.
    let mut env = bash::agent_env(&ctx.home).map_err(|reason| RpcError::new("internal", reason))?;
    for (key, value) in ctx.env.iter().chain(envs.iter()) {
        env = env.with_var(key, value);
    }
    let mut command = Command::new(cmd);
    command.args(&args).current_dir(&dir);
    env.apply(&mut command);
    command.uid(ctx.uid).gid(ctx.gid);

    let started = match pty {
        Some((cols, rows)) => start_on_pty(&mut command, cols, rows),
        None => start_on_pipes(&mut command, stdin),
    }
    .map_err(|err| RpcError::io(&err, &format!("cannot start {cmd}")))?;
    drop(command);

    let Started {
        mut child,
        master,
        stdin,
        readers,
    } = started;
    let pid = child.id();
    let proc = Arc::new(Proc {
        pid,
        tag,
        cmd: cmd.to_string(),
        args,
        envs,
        cwd: dir.display().to_string(),
        master,
        stdin: Mutex::new(stdin),
        queue: Mutex::new(Queue::default()),
        woken: Condvar::new(),
    });
    proc.push(json!({"event": {"start": {"pid": pid}}}));
    lock(&TABLE).insert(pid, Arc::clone(&proc));

    let mut readers = readers
        .into_iter()
        .map(|(source, kind)| {
            let proc = Arc::clone(&proc);
            thread::spawn(move || pump(source, &proc, kind))
        })
        .collect::<Vec<JoinHandle<()>>>();
    let waiter = Arc::clone(&proc);
    thread::spawn(move || {
        let status = child.wait();
        // Every output is read to its end before the end event, so the end comes last.
        for reader in readers.drain(..) {
            let _ = reader.join();
        }
        waiter.finish(end_event(status));
    });
    Ok(proc)
}

/// What a started process hands back to `spawn`.
struct Started {
    child: Child,
    master: Option<Arc<File>>,
    stdin: Option<ChildStdin>,
    /// The outputs to read, each with the name its data events use.
    readers: Vec<(Box<dyn Read + Send>, &'static str)>,
}

/// Starts `command` with a stdin pipe (when asked), and stdout and stderr pipes.
fn start_on_pipes(command: &mut Command, stdin: bool) -> io::Result<Started> {
    command
        .stdin(if stdin { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    Ok(Started {
        child,
        master: None,
        stdin,
        readers: vec![
            (Box::new(stdout) as Box<dyn Read + Send>, "stdout"),
            (Box::new(stderr) as Box<dyn Read + Send>, "stderr"),
        ],
    })
}

/// Starts `command` on a new PTY of `cols` by `rows` as the leader of a new session, whose
/// controlling terminal is the PTY. Its output and input are the PTY's master.
fn start_on_pty(command: &mut Command, cols: u16, rows: u16) -> io::Result<Started> {
    let pair = open_pty(cols, rows)?;
    let master = File::from(pair.master);
    let reader = master.try_clone()?;
    let slave = pair.slave;
    command.env("TERM", TERM);
    command
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    // SAFETY: the closure runs in the forked child, between fork and exec. setsid and TIOCSCTTY are
    // async-signal-safe, and the closure allocates nothing. stdin is the slave by then, so fd 0 is
    // the terminal.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    Ok(Started {
        child,
        master: Some(Arc::new(master)),
        stdin: None,
        readers: vec![(Box::new(reader) as Box<dyn Read + Send>, "pty")],
    })
}

/// A PTY's two ends.
struct Pty {
    master: OwnedFd,
    slave: OwnedFd,
}

fn open_pty(cols: u16, rows: u16) -> io::Result<Pty> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let size = winsize(cols, rows);
    // SAFETY: openpty writes one new descriptor through each out-pointer (live locals), reads the
    // window size through `size` (live for the call), and is given no name and no terminal settings.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &size,
        )
    };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty succeeded, so both descriptors are open and owned by nothing else.
    Ok(Pty {
        master: unsafe { OwnedFd::from_raw_fd(master) },
        slave: unsafe { OwnedFd::from_raw_fd(slave) },
    })
}

fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// Copies one output into the queue as data events until it ends. A PTY's master fails with EIO
/// once its shell is gone, which is the end of its output.
fn pump(mut source: Box<dyn Read + Send>, proc: &Proc, kind: &'static str) {
    loop {
        let mut buf = vec![0u8; READ_CHUNK];
        match source.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => {
                buf.truncate(read);
                let data = base64::engine::general_purpose::STANDARD.encode(&buf);
                proc.push(json!({"event": {"data": {kind: data}}}));
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

/// The end event of a process: its exit code, or the signal that ended it.
fn end_event(status: io::Result<ExitStatus>) -> Value {
    let end = match status {
        Ok(status) => match (status.code(), status.signal()) {
            (Some(code), _) => json!({
                "exitCode": code,
                "exited": true,
                "status": format!("exit status {code}"),
            }),
            (None, Some(signal)) => json!({
                "exitCode": 128 + signal,
                "exited": false,
                "status": format!("signal {signal}"),
            }),
            (None, None) => json!({"exitCode": -1, "exited": false, "status": "unknown exit"}),
        },
        Err(err) => json!({
            "exitCode": -1,
            "exited": false,
            "status": "wait failed",
            "error": err.to_string(),
        }),
    };
    json!({"event": {"end": end}})
}

fn is_end(event: &Value) -> bool {
    event.pointer("/event/end").is_some()
}

fn is_keepalive(event: &Value) -> bool {
    event.pointer("/event/keepalive").is_some()
}

/// Waits for the next events of `proc` and takes them: a batch, a keepalive when nothing came for
/// [`KEEPALIVE`], or `None` once the end event has been taken already.
fn next_batch(proc: &Proc) -> Option<Vec<Value>> {
    let mut queue = lock(&proc.queue);
    loop {
        if !queue.events.is_empty() {
            return Some(std::mem::take(&mut queue.events));
        }
        if queue.ended {
            return None;
        }
        let (guard, timeout) = proc
            .woken
            .wait_timeout(queue, KEEPALIVE)
            .unwrap_or_else(PoisonError::into_inner);
        queue = guard;
        if timeout.timed_out() && queue.events.is_empty() && !queue.ended {
            return Some(vec![json!({"event": {"keepalive": {}}})]);
        }
    }
}

/// Hands `proc`'s events to `sink` until its end event is delivered, then unlists it. A failed write
/// puts the events it did not deliver back, so the process stays listed for a later `Connect`.
fn deliver(proc: &Arc<Proc>, sink: &mut dyn StreamSink) -> Result<(), RpcError> {
    while let Some(batch) = next_batch(proc) {
        for (index, event) in batch.iter().enumerate() {
            if let Err(err) = sink.message(event) {
                let rest = batch[index..]
                    .iter()
                    .filter(|event| !is_keepalive(event))
                    .cloned()
                    .collect();
                proc.requeue(rest);
                return Err(RpcError::new(
                    "internal",
                    format!("the stream broke: {err}"),
                ));
            }
            if is_end(event) {
                lock(&TABLE).remove(&proc.pid);
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Which listed process a request names: by pid, or by the tag it was started with.
enum Selector {
    Pid(u32),
    Tag(String),
}

fn selector(value: Option<&Value>) -> Result<Selector, RpcError> {
    let value = value.ok_or_else(|| invalid("process is required"))?;
    if let Some(pid) = value.get("pid") {
        return pid
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .map(Selector::Pid)
            .ok_or_else(|| invalid("process.pid must be a 32-bit number"));
    }
    match value.get("tag").and_then(Value::as_str) {
        Some(tag) => Ok(Selector::Tag(tag.to_string())),
        None => Err(invalid("process needs a pid or a tag")),
    }
}

fn find(selector: &Selector) -> Result<Arc<Proc>, RpcError> {
    let table = lock(&TABLE);
    let found = match selector {
        Selector::Pid(pid) => table.get(pid).cloned(),
        Selector::Tag(tag) => table
            .values()
            .find(|proc| proc.tag.as_deref() == Some(tag.as_str()))
            .cloned(),
    };
    found.ok_or_else(|| RpcError::new("not_found", "no such process"))
}

/// `List`: the listed processes, by pid.
fn list() -> Value {
    let table = lock(&TABLE);
    let processes: Vec<Value> = table
        .values()
        .map(|proc| {
            let envs: Map<String, Value> = proc
                .envs
                .iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect();
            let mut info = json!({
                "config": {
                    "cmd": proc.cmd,
                    "args": proc.args,
                    "envs": envs,
                    "cwd": proc.cwd,
                },
                "pid": proc.pid,
            });
            if let Some(tag) = &proc.tag {
                info["tag"] = json!(tag);
            }
            info
        })
        .collect();
    json!({"processes": processes})
}

/// `SendInput`: `input.stdin` to a process's stdin pipe, or `input.pty` (or either) to its PTY.
fn send_input(request: &Value) -> Result<(), RpcError> {
    let proc = find(&selector(request.get("process"))?)?;
    let input = request
        .get("input")
        .ok_or_else(|| invalid("input is required"))?;
    let (kind, encoded) = match (input.get("stdin"), input.get("pty")) {
        (Some(data), _) => ("stdin", data),
        (None, Some(data)) => ("pty", data),
        (None, None) => return Err(invalid("input needs stdin or pty bytes")),
    };
    let data = decode(encoded, "input")?;
    if let Some(master) = &proc.master {
        let mut file: &File = master;
        return file
            .write_all(&data)
            .map_err(|err| RpcError::io(&err, "cannot write to the terminal"));
    }
    if kind == "pty" {
        return Err(invalid("this process has no PTY; send stdin instead"));
    }
    match lock(&proc.stdin).as_mut() {
        Some(pipe) => pipe
            .write_all(&data)
            .map_err(|err| RpcError::io(&err, "cannot write to stdin")),
        None => Err(invalid("stdin is closed")),
    }
}

/// `SendSignal`: `SIGNAL_SIGTERM` or `SIGNAL_SIGKILL`, to a process that has not ended.
fn send_signal(request: &Value) -> Result<(), RpcError> {
    let signal = match request.get("signal").and_then(Value::as_str) {
        Some("SIGNAL_SIGTERM") => libc::SIGTERM,
        Some("SIGNAL_SIGKILL") => libc::SIGKILL,
        _ => {
            return Err(invalid("signal must be SIGNAL_SIGTERM or SIGNAL_SIGKILL"));
        }
    };
    let proc = find(&selector(request.get("process"))?)?;
    // A process whose end is queued has been reaped, so its pid may be reused: nothing to signal.
    if proc.has_ended() {
        return Err(RpcError::new("not_found", "the process has ended"));
    }
    // SAFETY: kill(2) with a pid this daemon started and has not reported ended (so it has not been
    // reaped and its pid cannot have been reused) and a constant signal number.
    let rc = unsafe { libc::kill(proc.pid as libc::pid_t, signal) };
    if rc < 0 {
        return Err(RpcError::io(
            &io::Error::last_os_error(),
            "cannot signal the process",
        ));
    }
    Ok(())
}

/// `CloseStdin`: closes a process's stdin pipe, so it reads end of file; on a PTY, sends the
/// end-of-file character instead.
fn close_stdin(request: &Value) -> Result<(), RpcError> {
    let proc = find(&selector(request.get("process"))?)?;
    if let Some(master) = &proc.master {
        let mut file: &File = master;
        return file
            .write_all(&[0x04])
            .map_err(|err| RpcError::io(&err, "cannot write to the terminal"));
    }
    lock(&proc.stdin).take();
    Ok(())
}

/// `Update`: resizes a process's PTY to `pty.size`.
fn update(request: &Value) -> Result<(), RpcError> {
    let (cols, rows) = pty_size(
        request
            .get("pty")
            .ok_or_else(|| invalid("pty is required"))?,
    )?;
    let proc = find(&selector(request.get("process"))?)?;
    let master = proc
        .master
        .as_ref()
        .ok_or_else(|| invalid("this process has no PTY"))?;
    let size = winsize(cols, rows);
    // SAFETY: TIOCSWINSZ reads one `winsize` through the pointer, which lives for the call. The
    // descriptor is the PTY master this daemon opened, kept open by `proc`.
    let rc = unsafe {
        libc::ioctl(
            master.as_raw_fd(),
            libc::TIOCSWINSZ as _,
            std::ptr::from_ref(&size),
        )
    };
    if rc < 0 {
        return Err(RpcError::io(
            &io::Error::last_os_error(),
            "cannot resize the terminal",
        ));
    }
    Ok(())
}

/// A PTY's `size` as `(cols, rows)`, each from 1 to 1000.
fn pty_size(pty: &Value) -> Result<(u16, u16), RpcError> {
    let size = pty
        .get("size")
        .ok_or_else(|| invalid("pty.size is required"))?;
    Ok((dimension(size, "cols")?, dimension(size, "rows")?))
}

fn dimension(size: &Value, key: &str) -> Result<u16, RpcError> {
    size.get(key)
        .and_then(Value::as_u64)
        .filter(|value| (1..=1000).contains(value))
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| invalid(format!("pty.size.{key} must be a number from 1 to 1000")))
}

fn string_list(value: Option<&Value>, what: &str) -> Result<Vec<String>, RpcError> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| invalid(format!("{what} must be a list of strings")))
            })
            .collect(),
        Some(_) => Err(invalid(format!("{what} must be a list of strings"))),
    }
}

fn string_map(value: Option<&Value>, what: &str) -> Result<Vec<(String, String)>, RpcError> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.clone(), value.to_string()))
                    .ok_or_else(|| invalid(format!("{what}.{key} must be a string")))
            })
            .collect(),
        Some(_) => Err(invalid(format!("{what} must be an object"))),
    }
}

/// Decodes a base64 `bytes` field.
fn decode(value: &Value, what: &str) -> Result<Vec<u8>, RpcError> {
    let text = value
        .as_str()
        .ok_or_else(|| invalid(format!("{what} bytes must be base64 text")))?;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|err| invalid(format!("{what} bytes are not base64: {err}")))
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new("invalid_argument", message)
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use serde_json::json;

    use super::super::test_support::{call, call_stream, json_body, test_ctx, wait_for_tag};
    use super::*;

    fn output_of(events: &[Value], stream: &str) -> Vec<u8> {
        events
            .iter()
            .filter_map(|event| event.pointer(&format!("/event/data/{stream}")))
            .flat_map(|data| {
                base64::engine::general_purpose::STANDARD
                    .decode(data.as_str().unwrap_or(""))
                    .unwrap_or_default()
            })
            .collect()
    }

    #[test]
    fn a_start_streams_its_pid_its_output_and_its_end() {
        let ctx = test_ctx();
        let (events, end) = call_stream(
            &ctx,
            "/process.Process/Start",
            json!({"process": {"cmd": "echo", "args": ["hi"]}}),
        );
        assert!(end.get("error").is_none(), "{end}");
        assert!(events[0].pointer("/event/start/pid").is_some());
        assert_eq!(output_of(&events, "stdout"), b"hi\n");
        let last = events.last().expect("an end event");
        assert_eq!(last.pointer("/event/end/exitCode"), Some(&json!(0)));
        assert_eq!(last.pointer("/event/end/exited"), Some(&json!(true)));
    }

    #[test]
    fn a_non_zero_exit_is_the_end_event_and_stderr_is_its_own_stream() {
        let ctx = test_ctx();
        let (events, _) = call_stream(
            &ctx,
            "/process.Process/Start",
            json!({"process": {"cmd": "sh", "args": ["-c", "echo oops >&2; exit 3"]}}),
        );
        assert_eq!(output_of(&events, "stderr"), b"oops\n");
        assert_eq!(
            events.last().and_then(|e| e.pointer("/event/end/exitCode")),
            Some(&json!(3))
        );
    }

    #[test]
    fn a_command_that_does_not_exist_ends_the_stream_with_its_error() {
        let ctx = test_ctx();
        let (events, end) = call_stream(
            &ctx,
            "/process.Process/Start",
            json!({"process": {"cmd": "/no/such/command"}}),
        );
        assert!(events.is_empty());
        assert_eq!(end.pointer("/error/code"), Some(&json!("not_found")));
    }

    #[test]
    fn list_and_send_input_reach_a_running_cat_and_its_end_is_reported() {
        let ctx = test_ctx();
        let tag = "cat-round-trip";
        let starting = {
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                call_stream(
                    &ctx,
                    "/process.Process/Start",
                    json!({
                        "process": {"cmd": "cat"},
                        "tag": tag,
                        "stdin": true,
                    }),
                )
            })
        };
        wait_for_tag(&ctx, tag);

        let listed = json_body(&call(&ctx, "/process.Process/List", json!({})));
        let processes = listed["processes"].as_array().expect("a list");
        assert!(processes.iter().any(|p| p["tag"] == json!(tag)));

        let selector = json!({"tag": tag});
        let sent = call(
            &ctx,
            "/process.Process/SendInput",
            json!({
                "process": selector,
                "input": {"stdin": base64::engine::general_purpose::STANDARD.encode("hello\n")},
            }),
        );
        assert!(
            sent.starts_with(b"HTTP/1.1 200"),
            "{}",
            String::from_utf8_lossy(&sent)
        );
        let closed = call(
            &ctx,
            "/process.Process/CloseStdin",
            json!({"process": {"tag": tag}}),
        );
        assert!(closed.starts_with(b"HTTP/1.1 200"));

        let (events, end) = starting.join().expect("the start thread");
        assert!(end.get("error").is_none(), "{end}");
        assert_eq!(output_of(&events, "stdout"), b"hello\n");
        assert_eq!(
            events.last().and_then(|e| e.pointer("/event/end/exitCode")),
            Some(&json!(0))
        );
        // Once its end was delivered, the process is no longer listed.
        let listed = json_body(&call(&ctx, "/process.Process/List", json!({})));
        assert!(
            !listed["processes"]
                .as_array()
                .expect("a list")
                .iter()
                .any(|p| p["tag"] == json!(tag))
        );
    }

    #[test]
    fn a_pty_start_runs_on_a_terminal_of_the_size_it_asks_for() {
        let ctx = test_ctx();
        let (events, _) = call_stream(
            &ctx,
            "/process.Process/Start",
            json!({
                "process": {"cmd": "sh", "args": ["-c", "stty size"]},
                "pty": {"size": {"cols": 100, "rows": 40}},
            }),
        );
        let text = String::from_utf8_lossy(&output_of(&events, "pty")).into_owned();
        assert!(text.contains("40 100"), "stty said {text:?}");
        assert_eq!(
            events.last().and_then(|e| e.pointer("/event/end/exitCode")),
            Some(&json!(0))
        );
    }

    #[test]
    fn a_signal_ends_a_running_process_with_its_signal_status() {
        let ctx = test_ctx();
        let tag = "sleeper-signal";
        let starting = {
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                call_stream(
                    &ctx,
                    "/process.Process/Start",
                    json!({"process": {"cmd": "sleep", "args": ["30"]}, "tag": tag}),
                )
            })
        };
        wait_for_tag(&ctx, tag);
        let sent = call(
            &ctx,
            "/process.Process/SendSignal",
            json!({"process": {"tag": tag}, "signal": "SIGNAL_SIGKILL"}),
        );
        assert!(sent.starts_with(b"HTTP/1.1 200"));
        let (events, _) = starting.join().expect("the start thread");
        let end = events.last().expect("an end event");
        assert_eq!(end.pointer("/event/end/exited"), Some(&json!(false)));
        assert_eq!(end.pointer("/event/end/exitCode"), Some(&json!(128 + 9)));
    }

    #[test]
    fn a_connect_to_an_unknown_process_is_not_found() {
        let ctx = test_ctx();
        let (_, end) = call_stream(
            &ctx,
            "/process.Process/Connect",
            json!({"process": {"pid": 2_000_000_000u64}}),
        );
        assert_eq!(end.pointer("/error/code"), Some(&json!("not_found")));
    }

    #[test]
    fn selectors_and_pty_sizes_are_checked() {
        assert!(selector(Some(&json!({"pid": 7}))).is_ok());
        assert!(selector(Some(&json!({"tag": "x"}))).is_ok());
        assert!(selector(Some(&json!({}))).is_err());
        assert!(selector(None).is_err());
        assert!(pty_size(&json!({"size": {"cols": 0, "rows": 5}})).is_err());
        assert_eq!(
            pty_size(&json!({"size": {"cols": 80, "rows": 24}})).expect("a size"),
            (80, 24)
        );
    }
}
