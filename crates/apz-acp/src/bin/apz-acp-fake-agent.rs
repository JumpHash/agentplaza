//! A fake ACP agent for tests. It answers the protocol's methods on stdio and, on each `session/prompt`,
//! plays a script of steps, calling back into the client where a step asks for it.
//!
//! Usage: `apz-acp-fake-agent [script.json]`, or the script's path in `APZ_ACP_FAKE_SCRIPT`. The script is
//! one turn, `{"steps": [...], "stopReason": "end_turn"}`, or several turns in `"turns"`: prompt number n
//! plays turn min(n, last). The steps:
//!
//! - `{"update": <SessionUpdate>}`: a `session/update` notification.
//! - `{"permission": <RequestPermissionRequest>}`: asks the client for permission.
//! - `{"read": {"path": ...}}` and `{"write": {"path": ..., "content": ...}}`: file requests.
//! - `{"terminal": {"command": ..., "args": [...]}}`: create, wait for exit, read output, release.
//! - `{"sleep_ms": n}`: waits; a `session/cancel` ends the wait early.
//! - `{"ignore_cancel": true}`: from here on, a cancel does not end this turn.
//! - `{"crash": true}`: exits with status 1, as an agent that dies mid-turn would.
//!
//! A top-level `"modes"` value is what `session/new` offers (`{"availableModes": [...]}`).
//! `session/resume`, `session/set_mode` and `session/set_config_option` are answered `{}`.
//!
//! With `"protocol": 2` in the script and a client that offers 2 in `initialize`, the agent speaks the ACP v2
//! draft: `initialize` is answered in v2's shape, the updates and the permission steps are sent as written
//! (in v2's shapes, so they are not checked against the v1 types), and `session/prompt` is answered
//! `{"messageId"}` at once. The turn plays on in the background and ends with a `state_update` idle that
//! carries its stop reason. Any other client gets v1.
//!
//! The session id is always `fake-1`. Every answer the client gives to one of the agent's requests is
//! written to `APZ_ACP_FAKE_LOG` (when set) as a JSON array, rewritten after each answer, so a test can
//! check what the client replied.

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use apz_acp::rpc::{self, Peer};
use apz_acp::types::{
    self as types, AGENT_METHOD_NAMES, CLIENT_METHOD_NAMES, RequestPermissionRequest, SessionUpdate,
};
use serde_json::{Value, json};

const SESSION_ID: &str = "fake-1";

/// How long a call to the client may take before the turn fails.
const CLIENT_CALL_TIMEOUT: Duration = Duration::from_secs(30);

fn main() {
    let path = env::args_os()
        .nth(1)
        .or_else(|| env::var_os("APZ_ACP_FAKE_SCRIPT"))
        .map(PathBuf::from)
        .unwrap_or_else(|| die("usage: apz-acp-fake-agent <script.json>, or APZ_ACP_FAKE_SCRIPT"));
    let script: Value = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| {
            die(&format!(
                "cannot read a JSON script from {}",
                path.display()
            ))
        });
    let turns = script
        .get("turns")
        .and_then(Value::as_array)
        .filter(|turns| !turns.is_empty())
        .cloned()
        .unwrap_or_else(|| vec![script.clone()]);
    let agent = Arc::new(Agent {
        turns,
        modes: script.get("modes").cloned(),
        prompts: AtomicUsize::new(0),
        cancelled: AtomicBool::new(false),
        log_path: env::var_os("APZ_ACP_FAKE_LOG").map(PathBuf::from),
        log: Mutex::new(Vec::new()),
        v2_script: script.get("protocol").and_then(Value::as_u64) == Some(2),
        v2: AtomicBool::new(false),
        peer: OnceLock::new(),
    });
    let for_requests = Arc::clone(&agent);
    let for_notes = Arc::clone(&agent);
    let peer = Arc::new(Peer::new(
        io::stdin(),
        io::stdout(),
        move |peer, method, params| for_requests.request(peer, method, params),
        move |_peer, method, params| for_notes.notification(method, &params),
    ));
    // A v2 turn plays on in the background, so it needs the connection after its request is answered.
    let _ = agent.peer.set(Arc::clone(&peer));
    // The stdin stream ends when the client goes away; the connection closes with it.
    while !peer.is_closed() {
        thread::sleep(Duration::from_millis(10));
    }
}

fn die(message: &str) -> ! {
    eprintln!("apz-acp-fake-agent: {message}");
    process::exit(2)
}

struct Agent {
    turns: Vec<Value>,
    modes: Option<Value>,
    prompts: AtomicUsize,
    cancelled: AtomicBool,
    log_path: Option<PathBuf>,
    log: Mutex<Vec<Value>>,
    /// The script speaks v2 (`"protocol": 2`).
    v2_script: bool,
    /// The client offered v2 and this connection speaks it.
    v2: AtomicBool,
    /// The connection, for a v2 turn that plays after its answer.
    peer: OnceLock<Arc<Peer>>,
}

impl Agent {
    fn request(
        self: &Arc<Self>,
        peer: &Peer,
        method: &str,
        params: Value,
    ) -> Result<Value, types::Error> {
        let names = &AGENT_METHOD_NAMES;
        if method == names.initialize {
            if self.v2_script && params["protocolVersion"] == 2 {
                self.v2.store(true, Ordering::SeqCst);
                Ok(json!({
                    "protocolVersion": 2,
                    "info": {"name": "fake-agent", "version": "0.0.0"},
                    "capabilities": {"session": {"prompt": {}, "mcp": {"stdio": {}}}},
                    "authMethods": [],
                }))
            } else {
                Ok(json!({
                    "protocolVersion": 1,
                    "agentCapabilities": {"loadSession": true},
                    "authMethods": [],
                }))
            }
        } else if method == names.session_new {
            Ok(match &self.modes {
                Some(modes) => json!({"sessionId": SESSION_ID, "modes": modes}),
                None => json!({"sessionId": SESSION_ID}),
            })
        } else if method == names.session_load
            || method == names.session_resume
            || method == names.session_set_mode
            || method == names.session_set_config_option
        {
            Ok(json!({}))
        } else if method == names.session_prompt {
            self.prompt(peer)
        } else {
            Err(types::Error::method_not_found())
        }
    }

    fn notification(&self, method: &str, _params: &Value) {
        if method == AGENT_METHOD_NAMES.session_cancel {
            self.cancelled.store(true, Ordering::SeqCst);
        }
    }

    /// One prompt. A v1 turn plays before its answer. A v2 turn is answered at once with its message id
    /// and plays on in the background.
    fn prompt(self: &Arc<Self>, peer: &Peer) -> Result<Value, types::Error> {
        let turn = self.begin_turn();
        if self.v2.load(Ordering::SeqCst) {
            let agent = Arc::clone(self);
            thread::spawn(move || agent.end_v2(&turn));
            return Ok(json!({"messageId": "fake-message"}));
        }
        self.play(peer, &turn)
    }

    /// Counts the prompt and clears a cancel left by the last one. The turn is the script's next one, and
    /// the last one once the script runs out.
    fn begin_turn(&self) -> Value {
        let number = self.prompts.fetch_add(1, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);
        self.turns[number.min(self.turns.len() - 1)].clone()
    }

    /// The end of a v2 turn: its steps, then the idle update that carries the stop reason.
    fn end_v2(&self, turn: &Value) {
        let Some(peer) = self.peer.get() else {
            return;
        };
        let Ok(reply) = self.play(peer, turn) else {
            return;
        };
        let update = json!({
            "sessionUpdate": "state_update",
            "state": "idle",
            "stopReason": reply["stopReason"],
        });
        let _ = peer.notify(
            CLIENT_METHOD_NAMES.session_update,
            json!({"sessionId": SESSION_ID, "update": update}),
        );
    }

    /// Plays a turn's steps and answers its stop reason: `cancelled` when a cancel came first.
    fn play(&self, peer: &Peer, turn: &Value) -> Result<Value, types::Error> {
        let stop_reason = turn
            .get("stopReason")
            .cloned()
            .unwrap_or_else(|| json!("end_turn"));
        let steps = turn
            .get("steps")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut ignore_cancel = false;
        for (index, step) in steps.iter().enumerate() {
            if self.is_cancelled(ignore_cancel) {
                return Ok(json!({"stopReason": "cancelled"}));
            }
            self.step(peer, index, step, &mut ignore_cancel)?;
        }
        if self.is_cancelled(ignore_cancel) {
            return Ok(json!({"stopReason": "cancelled"}));
        }
        Ok(json!({"stopReason": stop_reason}))
    }

    fn is_cancelled(&self, ignore_cancel: bool) -> bool {
        !ignore_cancel && self.cancelled.load(Ordering::SeqCst)
    }

    fn step(
        &self,
        peer: &Peer,
        index: usize,
        step: &Value,
        ignore_cancel: &mut bool,
    ) -> Result<(), types::Error> {
        let names = &CLIENT_METHOD_NAMES;
        if let Some(update) = step.get("update") {
            // Checked here so that a typo in a script fails the turn instead of being dropped by the client.
            // A v2 script's updates are v2's, which the v1 types do not describe.
            if !self.v2.load(Ordering::SeqCst) {
                serde_json::from_value::<SessionUpdate>(update.clone())
                    .map_err(|err| invalid_step(index, &err.to_string()))?;
            }
            let params = json!({"sessionId": SESSION_ID, "update": update});
            peer.notify(names.session_update, params)
                .map_err(|err| transport(&err))?;
        } else if let Some(params) = step.get("permission") {
            let params = with_session(params.clone());
            if !self.v2.load(Ordering::SeqCst) {
                serde_json::from_value::<RequestPermissionRequest>(params.clone())
                    .map_err(|err| invalid_step(index, &err.to_string()))?;
            }
            let answer = peer.call(
                names.session_request_permission,
                params,
                CLIENT_CALL_TIMEOUT,
            );
            self.record(index, names.session_request_permission, &answer)?;
        } else if let Some(params) = step.get("read") {
            let answer = peer.call(
                names.fs_read_text_file,
                with_session(params.clone()),
                CLIENT_CALL_TIMEOUT,
            );
            self.record(index, names.fs_read_text_file, &answer)?;
        } else if let Some(params) = step.get("write") {
            let answer = peer.call(
                names.fs_write_text_file,
                with_session(params.clone()),
                CLIENT_CALL_TIMEOUT,
            );
            self.record(index, names.fs_write_text_file, &answer)?;
        } else if let Some(spec) = step.get("terminal") {
            self.terminal(peer, index, with_session(spec.clone()))?;
        } else if let Some(millis) = step.get("sleep_ms").and_then(Value::as_u64) {
            self.sleep(millis, *ignore_cancel);
        } else if step.get("ignore_cancel") == Some(&json!(true)) {
            *ignore_cancel = true;
        } else if step.get("crash") == Some(&json!(true)) {
            process::exit(1);
        } else {
            return Err(invalid_step(index, "unknown step"));
        }
        Ok(())
    }

    /// The four calls of a terminal: create it, wait for it to exit, read its output, release it. The
    /// sequence stops early if the client does not give a terminal id.
    fn terminal(&self, peer: &Peer, index: usize, spec: Value) -> Result<(), types::Error> {
        let names = &CLIENT_METHOD_NAMES;
        let created = peer.call(names.terminal_create, spec, CLIENT_CALL_TIMEOUT);
        self.record(index, names.terminal_create, &created)?;
        let Some(terminal) = created
            .as_ref()
            .ok()
            .and_then(|reply| reply.get("terminalId"))
            .cloned()
        else {
            return Ok(());
        };
        let calls = [
            (
                names.terminal_wait_for_exit,
                json!({"terminalId": terminal}),
            ),
            (names.terminal_output, json!({"terminalId": terminal})),
            (names.terminal_release, json!({"terminalId": terminal})),
        ];
        for (method, params) in calls {
            let answer = peer.call(method, with_session(params), CLIENT_CALL_TIMEOUT);
            self.record(index, method, &answer)?;
        }
        Ok(())
    }

    /// Waits `millis`, checking for a cancel every few milliseconds unless the turn ignores cancels.
    fn sleep(&self, millis: u64, ignore_cancel: bool) {
        let end = Instant::now() + Duration::from_millis(millis);
        while Instant::now() < end && !self.is_cancelled(ignore_cancel) {
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Logs one answer from the client and rewrites the log file. A reply that refuses the request is an
    /// answer, not a failure of the turn; only a lost connection fails it.
    fn record(
        &self,
        index: usize,
        method: &str,
        answer: &Result<Value, rpc::Error>,
    ) -> Result<(), types::Error> {
        let logged = match answer {
            Ok(result) => json!({"result": result}),
            Err(rpc::Error::Remote(error)) => json!({"error": error}),
            Err(err) => json!({"error": {"message": err.to_string()}}),
        };
        let mut log = lock(&self.log);
        log.push(json!({"step": index, "method": method, "answer": logged}));
        if let Some(path) = &self.log_path {
            match serde_json::to_vec_pretty(&*log) {
                Ok(bytes) => {
                    if let Err(err) = fs::write(path, bytes) {
                        eprintln!("apz-acp-fake-agent: cannot write the log: {err}");
                    }
                }
                Err(err) => eprintln!("apz-acp-fake-agent: cannot encode the log: {err}"),
            }
        }
        match answer {
            Err(err @ (rpc::Error::Closed | rpc::Error::Timeout | rpc::Error::Io(_))) => {
                Err(transport(err))
            }
            _ => Ok(()),
        }
    }
}

/// Fills in the session id a step leaves out, so that a script does not repeat it.
fn with_session(mut params: Value) -> Value {
    if let Some(fields) = params.as_object_mut() {
        fields
            .entry("sessionId")
            .or_insert_with(|| json!(SESSION_ID));
    }
    params
}

fn invalid_step(index: usize, reason: &str) -> types::Error {
    types::Error::invalid_params().data(json!({"step": index, "reason": reason}))
}

fn transport(err: &rpc::Error) -> types::Error {
    types::Error::internal_error().data(json!(err.to_string()))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
