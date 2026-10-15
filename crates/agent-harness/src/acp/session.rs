//! One engine session: the adapter process, the ACP handshake, one prompt turn, and the client
//! methods the engine calls back while the turn runs.
//!
//! The adapter starts as the agent (`bash::command`, `Privilege::Agent`), in its own process group,
//! with stdin and stdout as the ACP connection (`apz_acp::rpc::Peer`) and stderr kept in a small
//! ring for errors. The harness advertises `clientCapabilities {fs: {readTextFile, writeTextFile},
//! terminal: true}` and `clientInfo {name: "agentplaza", version}`, and requires protocol version 1.
//!
//! While the turn runs, the engine's requests are answered here:
//! - `session/request_permission`: [`gate::permission_check`] → [`EngineLink::decide`] →
//!   [`gate::permission_answer`];
//! - `fs/*` and `terminal/*`: [`gate::client_check`] → `decide` (a denial is a JSON-RPC error with
//!   the hub's reason) → [`fs`] / [`terminal::Terminals`]. Before the turn's first write the
//!   workspace is checkpointed (`crate::checkpoints::snapshot`, label `engine <id>`), so the owner
//!   can restore the engine's whole turn.
//!
//! A cancel (the link's `cancelled()`) or the deadline sends `session/cancel`; open permission
//! requests are answered `cancelled`; after [`CANCEL_GRACE`] the process group is killed. The process
//! group is always killed when the session ends.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use apz_acp::rpc::{self, Peer};
use apz_acp::types::{self as types, AGENT_METHOD_NAMES, CLIENT_METHOD_NAMES};
use serde_json::{Value, json};

use super::catalog::{Entry, ModelVia};
use super::link::{EngineLink, Verdict};
use super::stream::{Stream, Turn};
use super::terminal::Terminals;
use super::{fs, gate};
use crate::bash::{self, Privilege};
use crate::checkpoints::snapshot;
use crate::files::Workspace;

/// How long a cancelled engine gets to end its turn before its process group is killed.
pub const CANCEL_GRACE: Duration = Duration::from_secs(2);
/// How long `initialize` and `session/new` may take (the adapter starting node and its SDK).
pub const START_TIMEOUT: Duration = Duration::from_secs(120);
/// The stderr kept for errors, in bytes.
pub const STDERR_BYTES: usize = 8 * 1024;
/// How long the prompt may run past the deadline before the call itself gives up: the cancel and the
/// kill come first, so this only guards against an engine that answers neither.
const PROMPT_SLACK: Duration = Duration::from_secs(30);
/// How long the turn loop waits for an event before it checks the cancel and the flush again.
const TICK: Duration = Duration::from_millis(20);

/// How to start and drive one engine session.
pub struct Options<'w> {
    pub entry: &'static Entry,
    pub workspace: &'w Workspace,
    /// The folder the engine works in (absolute, inside the workspace): the process's cwd and the
    /// ACP session's `cwd`.
    pub cwd: PathBuf,
    /// The program and its arguments, environment included (`/usr/bin/env HOME=… KEY=… binary args…`).
    pub argv: Vec<String>,
    /// The model the hub allowed, when the call named one (sent as the catalogue says).
    pub model: Option<String>,
    /// The session mode to select, when the call named one: refused when the engine does not offer
    /// it or the catalogue refuses it.
    pub mode: Option<String>,
    /// A session to continue: `session/resume` when the engine advertises it, else `session/load`
    /// when it advertises `loadSession`, else a new session.
    pub resume: Option<String>,
    /// What the engine is told first when `resume` could not be honoured and a new session was
    /// opened instead: the previous turn's summary, put before the prompt as its own text block.
    pub context: Option<String>,
    /// When the turn must be over; then it is cancelled.
    pub deadline: Instant,
}

/// How one prompt turn went.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// The ACP session id (to continue with).
    pub session: String,
    /// True when `resume` was honoured (resumed or loaded); false when a new session was opened.
    pub continued: bool,
    /// `end_turn`, `max_tokens`, `max_turn_requests`, `refusal` or `cancelled`.
    pub stop_reason: String,
    pub turn: Turn,
    /// Commands run through the client's terminals, with exit codes.
    pub commands: Vec<(String, Option<i32>)>,
    /// The checkpoint taken before the turn's first write, if it wrote.
    pub checkpoint: Option<String>,
    /// The modes the engine offered (ids), for the summary.
    pub modes: Vec<String>,
}

/// Starts the engine, opens or continues a session, runs one prompt turn with `prompt`, and ends
/// the process. Errors are the engine's (it failed to start, refused the protocol version, crashed;
/// with the stderr tail) or the harness's.
pub fn prompt_turn(
    options: &Options,
    prompt: &str,
    link: &dyn EngineLink,
) -> Result<Outcome, String> {
    let entry = options.entry;
    let (program, rest) = options
        .argv
        .split_first()
        .ok_or_else(|| format!("the {} engine has no command", entry.id))?;
    let args: Vec<&str> = rest.iter().map(String::as_str).collect();
    let mut command = bash::command(program, &args, &options.cwd, Privilege::Agent)?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|err| format!("cannot start the {} engine: {err}", entry.id))?;
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        bash::kill_group(&mut child);
        let _ = child.wait();
        return Err(format!("the {} engine has no stdio", entry.id));
    };
    // The reader ends when the process group is gone (EOF), so it is joined after the kill.
    let tail = Arc::new(Mutex::new(Vec::new()));
    let stderr_reader = {
        let tail = Arc::clone(&tail);
        thread::spawn(move || read_stderr(stderr, &tail))
    };

    let result = drive(&mut child, options, prompt, link, stdin, stdout);
    bash::kill_group(&mut child);
    let _ = child.wait();
    let _ = stderr_reader.join();
    result.map_err(|err| with_stderr(err, &tail))
}

/// What the engine's threads send to the turn loop.
enum Event {
    /// A `session/update`'s `update`, in arrival order.
    Update(Value),
    /// A request the engine made; the answer goes back on `reply` (the peer's thread waits for it).
    Request {
        method: String,
        params: Value,
        reply: Sender<Result<Value, types::Error>>,
    },
    /// The `session/prompt` call returned (or failed).
    Done(Result<Value, rpc::Error>),
}

/// What the engine's client requests need: the session's checks, the workspace and the terminals.
/// Shared by the worker thread of each request, so everything in it is `Sync`.
struct Ctx<'a> {
    entry: &'static Entry,
    workspace: &'a Workspace,
    link: &'a dyn EngineLink,
    terminals: &'a Terminals,
    deadline: Instant,
    /// The checkpoint of this turn, taken before its first write.
    checkpoint: Mutex<Option<String>>,
}

impl Ctx<'_> {
    /// The answer to one client method the engine called.
    fn answer(&self, method: &str, params: &Value) -> Result<Value, types::Error> {
        let names = &CLIENT_METHOD_NAMES;
        if method == names.session_request_permission {
            let (name, input) = gate::permission_check(self.entry.id, params);
            let verdict = self.link.decide(&name, &input, self.deadline);
            Ok(gate::permission_answer(params, &verdict))
        } else if method == names.fs_read_text_file {
            let label = self.path_label(params)?;
            self.check(method, params, Some(&label))?;
            fs::read_text_file(self.workspace, params).map_err(failed)
        } else if method == names.fs_write_text_file {
            let label = self.path_label(params)?;
            self.check(method, params, Some(&label))?;
            self.checkpoint_once()?;
            fs::write_text_file(self.workspace, params).map_err(failed)
        } else if method == names.terminal_create {
            self.check(method, params, None)?;
            self.terminals.create(params).map_err(failed)
        } else if method == names.terminal_output {
            self.terminals.output(params).map_err(failed)
        } else if method == names.terminal_wait_for_exit {
            let cancelled = || self.link.cancelled();
            self.terminals
                .wait_for_exit(params, &cancelled)
                .map_err(failed)
        } else if method == names.terminal_kill {
            self.terminals.kill(params).map_err(failed)
        } else if method == names.terminal_release {
            self.terminals.release(params).map_err(failed)
        } else {
            Err(types::Error::method_not_found())
        }
    }

    /// The workspace-relative label of the `path` a file method names; a refused path is an
    /// invalid-params error.
    fn path_label(&self, params: &Value) -> Result<String, types::Error> {
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        fs::label(self.workspace, path)
            .map_err(|reason| types::Error::invalid_params().data(json!(reason)))
    }

    /// Asks the hub about a client method; a denial or a cancel is the error the engine gets.
    fn check(&self, method: &str, params: &Value, label: Option<&str>) -> Result<(), types::Error> {
        let Some((name, input)) = gate::client_check(self.entry.id, method, params, label) else {
            return Ok(());
        };
        match self.link.decide(&name, &input, self.deadline) {
            Verdict::Allow => Ok(()),
            Verdict::Deny(reason) => Err(failed(reason)),
            Verdict::Cancelled => Err(failed("the turn was cancelled".to_string())),
        }
    }

    /// Takes the turn's checkpoint before its first write. A checkpoint that fails refuses the
    /// write: the owner must be able to restore what the engine changed.
    fn checkpoint_once(&self) -> Result<(), types::Error> {
        let mut checkpoint = lock(&self.checkpoint);
        if checkpoint.is_none() {
            let label = format!("engine {}", self.entry.id);
            *checkpoint = Some(snapshot(self.workspace, &label).map_err(failed)?);
        }
        Ok(())
    }
}

/// The handshake, the session, the prompt turn and its loop. The child is killed by the caller.
fn drive(
    child: &mut Child,
    options: &Options,
    prompt: &str,
    link: &dyn EngineLink,
    stdin: ChildStdin,
    stdout: ChildStdout,
) -> Result<Outcome, String> {
    let (events_tx, events) = mpsc::channel::<Event>();
    // The peer's closures are `'static`, so they reach the turn loop through the channel only.
    let requests = Mutex::new(events_tx.clone());
    let notes = Mutex::new(events_tx.clone());
    let peer = Peer::new(
        stdout,
        stdin,
        move |_peer, method, params| {
            let (reply, answer) = mpsc::channel();
            let event = Event::Request {
                method: method.to_string(),
                params,
                reply,
            };
            if lock(&requests).send(event).is_err() {
                return Err(failed("the turn is over".to_string()));
            }
            answer
                .recv()
                .unwrap_or_else(|_| Err(failed("the turn is over".to_string())))
        },
        move |_peer, method, mut params| {
            if method == CLIENT_METHOD_NAMES.session_update {
                if let Some(update) = params.get_mut("update").map(Value::take) {
                    let _ = lock(&notes).send(Event::Update(update));
                }
            }
        },
    );
    let result = run_turn(child, options, prompt, link, &peer, &events, &events_tx);
    peer.close();
    result
}

/// The protocol steps of [`drive`], over the connection.
fn run_turn(
    child: &mut Child,
    options: &Options,
    prompt: &str,
    link: &dyn EngineLink,
    peer: &Peer,
    events: &Receiver<Event>,
    events_tx: &Sender<Event>,
) -> Result<Outcome, String> {
    let entry = options.entry;
    let names = &AGENT_METHOD_NAMES;
    let cwd = options.cwd.to_string_lossy().into_owned();

    let init = peer
        .call(
            names.initialize,
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": {"readTextFile": true, "writeTextFile": true},
                    "terminal": true,
                },
                "clientInfo": {"name": "agentplaza", "version": env!("CARGO_PKG_VERSION")},
            }),
            START_TIMEOUT,
        )
        .map_err(|err| rpc_failure("initialize", err))?;
    if init.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
        return Err(format!(
            "the {} engine speaks ACP protocol version {}, the harness speaks 1",
            entry.id,
            init.get("protocolVersion").unwrap_or(&Value::Null)
        ));
    }
    let caps = init
        .get("agentCapabilities")
        .cloned()
        .unwrap_or(Value::Null);

    // A session to continue: resumed or loaded when the engine can; any failure opens a new one.
    let resumed = match options.resume.as_deref() {
        Some(id) => {
            let can_resume = caps.pointer("/sessionCapabilities/resume").is_some();
            let can_load = caps.get("loadSession").and_then(Value::as_bool) == Some(true);
            let method = if can_resume {
                Some(names.session_resume)
            } else if can_load {
                Some(names.session_load)
            } else {
                None
            };
            method.and_then(|method| {
                let params = json!({"sessionId": id, "cwd": cwd, "mcpServers": []});
                peer.call(method, params, START_TIMEOUT)
                    .ok()
                    .map(|reply| (id.to_string(), reply))
            })
        }
        None => None,
    };
    let continued = resumed.is_some();
    let (session, reply) = match resumed {
        Some(found) => found,
        None => {
            let reply = peer
                .call(
                    names.session_new,
                    json!({"cwd": cwd, "mcpServers": []}),
                    START_TIMEOUT,
                )
                .map_err(|err| rpc_failure("session/new", err))?;
            let id = reply
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("the {} engine opened no session", entry.id))?
                .to_string();
            (id, reply)
        }
    };

    let modes = offered_modes(&reply);
    if let Some(mode) = options.mode.as_deref() {
        let refused = entry.refused_modes.iter().any(|refused| *refused == mode);
        if refused || !modes.iter().any(|offered| offered.as_str() == mode) {
            return Err(format!(
                "the {} engine does not offer the mode {mode}",
                entry.id
            ));
        }
        peer.call(
            names.session_set_mode,
            json!({"sessionId": session, "modeId": mode}),
            START_TIMEOUT,
        )
        .map_err(|err| rpc_failure("session/set_mode", err))?;
    }
    // The model is the one option the catalogue sets after the session; the other engines took it
    // from their command line or environment. An engine that refuses it runs on its default.
    if let (true, Some(model)) = (
        matches!(entry.model, ModelVia::ConfigOption),
        options.model.as_deref(),
    ) {
        let _ = peer.call(
            names.session_set_config_option,
            json!({"sessionId": session, "configId": "model", "value": model}),
            START_TIMEOUT,
        );
    }
    // The replayed history of a loaded session is not this turn.
    discard_pending(events);

    let mut blocks = Vec::new();
    if !continued {
        if let Some(context) = options.context.as_deref() {
            blocks.push(json!({"type": "text", "text": context}));
        }
    }
    blocks.push(json!({"type": "text", "text": prompt}));

    let terminals = Terminals::new(options.workspace.clone());
    let ctx = Ctx {
        entry,
        workspace: options.workspace,
        link,
        terminals: &terminals,
        deadline: options.deadline,
        checkpoint: Mutex::new(None),
    };
    let mut stream = Stream::new(link, entry.id, &session);
    let prompt_params = json!({"sessionId": session, "prompt": blocks});
    let budget =
        options.deadline.saturating_duration_since(Instant::now()) + CANCEL_GRACE + PROMPT_SLACK;

    let stop_reason = thread::scope(|s| -> Result<String, String> {
        let done = events_tx.clone();
        s.spawn(move || {
            let answer = peer.call(names.session_prompt, prompt_params, budget);
            let _ = done.send(Event::Done(answer));
        });

        let mut cancel_at: Option<Instant> = None;
        let mut killed = false;
        let mut finished: Option<Result<Value, rpc::Error>> = None;
        loop {
            let now = Instant::now();
            if cancel_at.is_none() && (link.cancelled() || now >= options.deadline) {
                let _ = peer.notify(names.session_cancel, json!({"sessionId": session}));
                cancel_at = Some(now);
            }
            // The engine had its grace to end the turn; its group is killed now, and the prompt
            // call fails with the closed connection.
            if cancel_at.is_some_and(|at| now >= at + CANCEL_GRACE) {
                bash::kill_group(child);
                killed = true;
                break;
            }
            match events.recv_timeout(TICK) {
                Ok(Event::Update(update)) => stream.absorb(update),
                Ok(Event::Request {
                    method,
                    params,
                    reply,
                }) => {
                    let ctx = &ctx;
                    s.spawn(move || {
                        let _ = reply.send(ctx.answer(&method, &params));
                    });
                }
                Ok(Event::Done(answer)) => {
                    finished = Some(answer);
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if stream.due(Instant::now()) {
                stream.flush();
            }
        }
        stream.flush();
        // A terminal's `wait_for_exit` does not see the deadline, so the commands end here, or the
        // workers that wait for them would hold the scope open.
        terminals.release_all();
        match finished {
            Some(Ok(reply)) => reply
                .get("stopReason")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| "the engine answered the prompt without a stop reason".to_string()),
            Some(Err(err)) => Err(rpc_failure("session/prompt", err)),
            None if killed => Ok("cancelled".to_string()),
            None => Err(format!(
                "the {} engine's connection ended before the prompt was answered",
                entry.id
            )),
        }
    })?;

    let turn = stream.turn().clone();
    let checkpoint = lock(&ctx.checkpoint).clone();
    Ok(Outcome {
        session,
        continued,
        stop_reason,
        turn,
        commands: terminals.ran(),
        checkpoint,
        modes,
    })
}

/// Drops the updates and answers the requests queued so far; a request that arrives before the turn
/// is refused.
fn discard_pending(events: &Receiver<Event>) {
    for event in events.try_iter() {
        if let Event::Request { reply, .. } = event {
            let _ = reply.send(Err(failed("the turn has not started".to_string())));
        }
    }
}

/// The mode ids in a `session/new`, `session/load` or `session/resume` reply.
fn offered_modes(reply: &Value) -> Vec<String> {
    reply
        .pointer("/modes/availableModes")
        .and_then(Value::as_array)
        .map(|modes| {
            modes
                .iter()
                .filter_map(|mode| mode.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Keeps the last [`STDERR_BYTES`] of the engine's stderr until it closes.
fn read_stderr(mut stderr: ChildStderr, tail: &Mutex<Vec<u8>>) {
    let mut buf = [0u8; 4096];
    loop {
        match stderr.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut ring = lock(tail);
                ring.extend_from_slice(&buf[..n]);
                if ring.len() > STDERR_BYTES {
                    let excess = ring.len() - STDERR_BYTES;
                    ring.drain(..excess);
                }
            }
        }
    }
}

/// An error with the engine's stderr tail, when it said anything.
fn with_stderr(err: String, tail: &Mutex<Vec<u8>>) -> String {
    let tail = String::from_utf8_lossy(&lock(tail)).trim().to_string();
    if tail.is_empty() {
        err
    } else {
        format!("{err}; the engine's stderr ends: {tail}")
    }
}

fn rpc_failure(step: &str, err: rpc::Error) -> String {
    format!("the engine did not complete {step}: {err}")
}

/// The JSON-RPC error an engine gets for a refused or failed client method.
fn failed(reason: String) -> types::Error {
    types::Error::internal_error().data(json!(reason))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::Path;
    use std::process::Command;
    use std::sync::Once;

    use super::super::catalog::ENGINES;
    use super::*;

    /// The fake agent, built before use (cargo does nothing when it is current).
    fn fake_agent() -> PathBuf {
        static BUILT: Once = Once::new();
        let exe = env::current_exe().expect("the test binary's path");
        let target = exe
            .parent()
            .and_then(Path::parent)
            .expect("the target directory");
        BUILT.call_once(|| {
            let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
            let status = Command::new(cargo)
                .args([
                    "build",
                    "--offline",
                    "-p",
                    "apz-acp",
                    "--bin",
                    "apz-acp-fake-agent",
                ])
                .status()
                .expect("cargo runs");
            assert!(status.success(), "cannot build the fake agent");
        });
        target.join("apz-acp-fake-agent")
    }

    fn claude() -> &'static Entry {
        ENGINES
            .iter()
            .find(|engine| engine.id == "claude")
            .expect("the claude engine")
    }

    /// A folder of the test's own, for its script and its log.
    fn scratch(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("apz-acp-session-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch folder");
        dir
    }

    fn write_script(dir: &Path, script: &Value) -> PathBuf {
        let path = dir.join("script.json");
        std::fs::write(&path, script.to_string()).expect("the script is written");
        path
    }

    fn argv(script: &Path) -> Vec<String> {
        vec![
            fake_agent().display().to_string(),
            script.display().to_string(),
        ]
    }

    /// The same, with the fake agent told where to log the client's answers.
    fn argv_logged(script: &Path, log: &Path) -> Vec<String> {
        vec![
            "/usr/bin/env".to_string(),
            format!("APZ_ACP_FAKE_LOG={}", log.display()),
            fake_agent().display().to_string(),
            script.display().to_string(),
        ]
    }

    fn options<'w>(workspace: &'w Workspace, dir: &Path, argv: Vec<String>) -> Options<'w> {
        Options {
            entry: claude(),
            workspace,
            cwd: dir.to_path_buf(),
            argv,
            model: None,
            mode: None,
            resume: None,
            context: None,
            deadline: Instant::now() + Duration::from_secs(20),
        }
    }

    /// The hub in memory: a fixed verdict, every decision recorded, and a cancel after a delay.
    struct TestLink {
        verdict: Verdict,
        cancel_after: Option<Duration>,
        started: Instant,
        decisions: Mutex<Vec<(String, Value)>>,
    }

    impl TestLink {
        fn new(verdict: Verdict, cancel_after: Option<Duration>) -> Self {
            Self {
                verdict,
                cancel_after,
                started: Instant::now(),
                decisions: Mutex::new(Vec::new()),
            }
        }
    }

    impl EngineLink for TestLink {
        fn decide(&self, name: &str, input: &Value, _deadline: Instant) -> Verdict {
            lock(&self.decisions).push((name.to_string(), input.clone()));
            self.verdict.clone()
        }

        fn update(&self, _engine: &str, _session: &str, _update: &Value) {}

        fn cancelled(&self) -> bool {
            self.cancel_after
                .is_some_and(|after| self.started.elapsed() >= after)
        }
    }

    /// The fake agent's log of the client's answers, as JSON.
    fn read_log(log: &Path) -> Value {
        let text = std::fs::read_to_string(log).expect("the fake agent logged its answers");
        serde_json::from_str(&text).expect("the log is JSON")
    }

    #[test]
    fn a_turn_ends_with_the_engines_stop_reason() {
        let dir = scratch("stop-reason");
        let script = write_script(&dir, &json!({"steps": [], "stopReason": "end_turn"}));
        let workspace = Workspace::at(&dir);
        let link = TestLink::new(Verdict::Allow, None);

        let outcome = prompt_turn(&options(&workspace, &dir, argv(&script)), "hello", &link)
            .expect("the turn runs");

        assert_eq!(outcome.stop_reason, "end_turn");
        assert_eq!(outcome.session, "fake-1");
        assert!(!outcome.continued);
        assert!(outcome.modes.is_empty());
    }

    #[test]
    fn a_session_is_continued_by_load_when_the_engine_can_load() {
        let dir = scratch("load");
        let script = write_script(&dir, &json!({"steps": [], "stopReason": "end_turn"}));
        let workspace = Workspace::at(&dir);
        let link = TestLink::new(Verdict::Allow, None);
        let mut options = options(&workspace, &dir, argv(&script));
        options.resume = Some("saved-1".to_string());

        let outcome = prompt_turn(&options, "hello", &link).expect("the turn runs");

        assert!(outcome.continued);
        assert_eq!(outcome.session, "saved-1");
    }

    #[test]
    fn a_mode_the_engine_does_not_offer_is_refused() {
        let dir = scratch("mode-missing");
        let script = write_script(&dir, &json!({"steps": [], "stopReason": "end_turn"}));
        let workspace = Workspace::at(&dir);
        let link = TestLink::new(Verdict::Allow, None);
        let mut options = options(&workspace, &dir, argv(&script));
        options.mode = Some("plan".to_string());

        let err = prompt_turn(&options, "hello", &link).expect_err("the mode is refused");

        assert!(err.contains("does not offer the mode plan"), "{err}");
    }

    #[test]
    fn an_offered_mode_is_selected_and_listed() {
        let dir = scratch("mode-offered");
        let script = write_script(
            &dir,
            &json!({
                "modes": {
                    "availableModes": [
                        {"id": "plan", "name": "Plan"},
                        {"id": "default", "name": "Default"},
                    ],
                    "currentModeId": "default",
                },
                "steps": [],
                "stopReason": "end_turn",
            }),
        );
        let workspace = Workspace::at(&dir);
        let link = TestLink::new(Verdict::Allow, None);
        let mut options = options(&workspace, &dir, argv(&script));
        options.mode = Some("plan".to_string());

        let outcome = prompt_turn(&options, "hello", &link).expect("the turn runs");

        assert_eq!(outcome.modes, vec!["plan", "default"]);
        assert_eq!(outcome.stop_reason, "end_turn");
    }

    #[test]
    fn a_crash_mid_turn_is_an_error() {
        let dir = scratch("crash");
        let script = write_script(&dir, &json!({"steps": [{"crash": true}]}));
        let workspace = Workspace::at(&dir);
        let link = TestLink::new(Verdict::Allow, None);

        let err = prompt_turn(&options(&workspace, &dir, argv(&script)), "hello", &link)
            .expect_err("the engine died");

        assert!(err.contains("session/prompt"), "{err}");
    }

    #[test]
    fn a_cancel_ends_a_long_turn() {
        let dir = scratch("cancel");
        let script = write_script(&dir, &json!({"steps": [{"sleep_ms": 10000}]}));
        let workspace = Workspace::at(&dir);
        // The fake is built here, before the clock starts, so that the build is not in the bound.
        let argv = argv(&script);
        let link = TestLink::new(Verdict::Allow, Some(Duration::from_millis(200)));
        let started = Instant::now();

        let outcome =
            prompt_turn(&options(&workspace, &dir, argv), "hello", &link).expect("the turn runs");

        assert_eq!(outcome.stop_reason, "cancelled");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn an_engine_that_ignores_the_cancel_is_killed_after_the_grace() {
        let dir = scratch("kill");
        let script = write_script(
            &dir,
            &json!({"steps": [{"ignore_cancel": true}, {"sleep_ms": 60000}]}),
        );
        let workspace = Workspace::at(&dir);
        let link = TestLink::new(Verdict::Allow, None);
        let mut options = options(&workspace, &dir, argv(&script));
        options.deadline = Instant::now() + Duration::from_millis(300);
        let started = Instant::now();

        let outcome = prompt_turn(&options, "hello", &link).expect("the turn runs");

        assert_eq!(outcome.stop_reason, "cancelled");
        assert!(started.elapsed() >= CANCEL_GRACE);
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    #[test]
    fn the_turn_keeps_the_engines_text() {
        let dir = scratch("text");
        let script = write_script(
            &dir,
            &json!({
                "steps": [
                    {"update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hello "}}},
                    {"update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "world"}}},
                ],
                "stopReason": "end_turn",
            }),
        );
        let workspace = Workspace::at(&dir);
        let link = TestLink::new(Verdict::Allow, None);

        let outcome = prompt_turn(&options(&workspace, &dir, argv(&script)), "hello", &link)
            .expect("the turn runs");

        assert_eq!(outcome.turn.message, "hello world");
    }

    #[test]
    fn a_permission_request_asks_the_hub_and_answers_the_engine() {
        let dir = scratch("permission");
        let log = dir.join("log.json");
        let script = write_script(
            &dir,
            &json!({
                "steps": [{"permission": {
                    "toolCall": {"toolCallId": "t1", "title": "edit a.txt", "kind": "edit"},
                    "options": [
                        {"optionId": "yes", "name": "Allow", "kind": "allow_once"},
                        {"optionId": "no", "name": "Reject", "kind": "reject_once"},
                    ],
                }}],
                "stopReason": "end_turn",
            }),
        );
        let workspace = Workspace::at(&dir);
        let link = TestLink::new(Verdict::Deny("not today".to_string()), None);

        prompt_turn(
            &options(&workspace, &dir, argv_logged(&script, &log)),
            "hello",
            &link,
        )
        .expect("the turn runs");

        let decisions = lock(&link.decisions).clone();
        assert_eq!(decisions[0].0, "engine.edit");
        let answer = read_log(&log)[0]["answer"]["result"].clone();
        assert_eq!(answer["outcome"]["optionId"], "no");
    }

    #[test]
    fn a_terminal_command_runs_through_the_client() {
        let dir = scratch("terminal");
        let log = dir.join("log.json");
        let script = write_script(
            &dir,
            &json!({"steps": [{"terminal": {"command": "true", "args": []}}], "stopReason": "end_turn"}),
        );
        let workspace = Workspace::at(&dir);
        let link = TestLink::new(Verdict::Allow, None);

        let outcome = prompt_turn(
            &options(&workspace, &dir, argv_logged(&script, &log)),
            "hello",
            &link,
        )
        .expect("the turn runs");

        assert_eq!(outcome.commands.len(), 1);
        assert!(read_log(&log)[0]["answer"]["result"]["terminalId"].is_string());
    }
}
