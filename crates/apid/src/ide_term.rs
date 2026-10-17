//! The IDE's terminal socket: `WS /v1/ide/terminal/{workspace}` (docs/design/ide.md sections 6.3, 6.5
//! and 8, phase 5). An owner's shell in an agent's VM, reached through the agent's `ide_term_*` tools
//! (`crates/agent-harness/src/term.rs`), or in a workstation's work folder, reached through the daemon's
//! terminal operations (`Op::Term`, docs/design/workstations.md section 3). A repo workspace is refused with 400.
//!
//! The socket speaks JSON text frames, each one object with a `t` field. It asks for the subprotocol
//! [`PROTOCOL`] (`Sec-WebSocket-Protocol: apz.ide.terminal.v1`), and only browsers from this host are
//! let in (`SameHost`, like `/v1/ws`).
//!
//! Client to hub:
//! - `{"t":"open","cols":80,"rows":24,"cwd":""}`: a new shell. `cwd` is a directory of the workspace,
//!   relative to its root, and is optional.
//! - `{"t":"attach","term":"term-1","offset":N}`: re-attaches after a reload. `offset` is where to
//!   resume: one past the last byte the client holds, which is the `offset` of its last `out` frame
//!   plus that frame's byte count. Optional; `0` (the oldest output the guest still keeps) by default.
//! - `{"t":"in","data":"<base64>"}`: keys for the shell.
//! - `{"t":"resize","cols":N,"rows":N}`: the shell's size, 1 to 1000 each.
//! - `{"t":"kill"}`: ends the guest terminal, then closes the socket (1000).
//! - `{"t":"ping"}`.
//!
//! Hub to client:
//! - `{"t":"opened","term":"term-1"}`: after an open or an attach.
//! - `{"t":"out","data":"<base64>","offset":N}`: output; `offset` is the byte offset of its first byte.
//! - `{"t":"lost","bytes":N}`: output the guest dropped before the client's offset. The next `out`
//!   starts after it.
//! - `{"t":"exit","code":N}`: the shell ended (`code` is null when the guest gave none). The socket
//!   then closes (1000).
//! - `{"t":"error","message":"..."}`: a frame or a call failed. The socket stays open, except where
//!   a close follows it.
//! - `{"t":"pong"}`.
//!
//! Closing the socket never kills the guest terminal: a reload re-attaches (the guest's own idle
//! janitor ends a shell after 30 minutes with no use). Only `kill` ends it. A socket with no client
//! frame for ten minutes is closed. Caps: [`PER_WORKSPACE`] terminal sockets per workspace, and the
//! event socket's per-peer and total caps (`PER_PEER` and `TOTAL` in `events.rs`, a pool of their own).
//! Each open, and each kill, is written to the IDE audit (kinds `term_open` and `term_kill`).

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use apz_http::ws::{self, Limits, Message, Origins, Socket};
use apz_http::{Request, Response, RouteDoc, Router};
use apz_ide::workspace::WorkspaceId;
use apz_station::{Op, TermOp};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::events::Slots;
use crate::ide::{TELL_TIMEOUT_MS, is_unreachable, station_gate, station_op};
use crate::state::App;

/// The subprotocol a terminal socket must be asked for.
pub const PROTOCOL: &str = "apz.ide.terminal.v1";
/// Terminal sockets one workspace may hold open.
pub const PER_WORKSPACE: usize = 4;
/// How long a socket may go without a client frame before it is closed.
const IDLE: Duration = Duration::from_secs(10 * 60);
/// Input typed within this window goes out as one write.
const COALESCE: Duration = Duration::from_millis(10);
/// The most input one write carries, in raw bytes (the harness takes 64 KiB, base64 is 4/3 of that).
pub const WRITE_CHUNK: usize = 32 * 1024;
/// The output poll interval while output keeps coming or arrived within [`SLOW_AFTER`] of output or input.
const FAST_POLL: Duration = Duration::from_millis(25);
/// The output poll interval after a quiet spell.
const SLOW_POLL: Duration = Duration::from_millis(250);
/// The quiet spell (no output and no input) after which polling backs off to [`SLOW_POLL`].
const SLOW_AFTER: Duration = Duration::from_secs(2);
/// The shortest a socket wait may be (a zero wait would never read the socket).
const MIN_WAIT: Duration = Duration::from_millis(10);
/// Consecutive output polls the node could not answer before the socket closes.
const READ_FAILURES: u32 = 3;
/// Writes in a row that the shell did not take before the input is given up.
const WRITE_STALLS: u32 = 5;
/// The largest terminal size, in columns or rows (the harness's cap).
const MAX_SIZE: u64 = 1000;

/// How fast the output is polled after a poll: at once while output keeps coming, else on a schedule
/// that backs off after [`SLOW_AFTER`] of quiet.
pub fn poll_delay(had_data: bool, quiet: Duration) -> Duration {
    if had_data {
        Duration::ZERO
    } else if quiet < SLOW_AFTER {
        FAST_POLL
    } else {
        SLOW_POLL
    }
}

/// Whether a socket that last heard from its client at `last_client` is closed by now.
pub fn idle_expired(last_client: Instant, now: Instant) -> bool {
    now.saturating_duration_since(last_client) >= IDLE
}

/// What a terminal socket is attached to: an agent's VM, or a workstation's work folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Agent(u64),
    Station(String),
}

impl Target {
    /// The workspace id text of the target: `agent:<cid>` or `station:<id>`.
    fn workspace(&self) -> String {
        match self {
            Target::Agent(cid) => WorkspaceId::Agent(*cid).to_string(),
            Target::Station(id) => WorkspaceId::Station(id.clone()).to_string(),
        }
    }
}

/// A frame from the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientFrame {
    Open {
        cols: u16,
        rows: u16,
        cwd: Option<String>,
    },
    Attach {
        term: String,
        offset: u64,
    },
    Input(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    Kill,
    Ping,
}

/// Reads one client frame. A frame that is not one of these is an error for the client to hear.
pub fn parse_client(text: &str) -> Result<ClientFrame, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|err| format!("the frame is not JSON: {err}"))?;
    let kind = value
        .get("t")
        .and_then(Value::as_str)
        .ok_or("the frame names no \"t\"")?;
    match kind {
        "open" => Ok(ClientFrame::Open {
            cols: size(&value, "cols")?,
            rows: size(&value, "rows")?,
            cwd: optional_text(&value, "cwd")?,
        }),
        "attach" => Ok(ClientFrame::Attach {
            term: text_field(&value, "term")?,
            offset: optional_offset(&value)?,
        }),
        "in" => {
            let data = text_field(&value, "data")?;
            STANDARD
                .decode(data)
                .map(ClientFrame::Input)
                .map_err(|err| format!("\"data\" is not base64: {err}"))
        }
        "resize" => Ok(ClientFrame::Resize {
            cols: size(&value, "cols")?,
            rows: size(&value, "rows")?,
        }),
        "kill" => Ok(ClientFrame::Kill),
        "ping" => Ok(ClientFrame::Ping),
        other => Err(format!("no frame named {other:?}")),
    }
}

/// A frame to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerFrame {
    Opened { term: String },
    Out { data: Vec<u8>, offset: u64 },
    Lost { bytes: u64 },
    Exit { code: Option<i64> },
    Error { message: String },
    Pong,
}

impl ServerFrame {
    /// The frame's text message.
    pub fn wire(&self) -> String {
        let value = match self {
            ServerFrame::Opened { term } => json!({ "t": "opened", "term": term }),
            ServerFrame::Out { data, offset } => {
                json!({ "t": "out", "data": STANDARD.encode(data), "offset": offset })
            }
            ServerFrame::Lost { bytes } => json!({ "t": "lost", "bytes": bytes }),
            ServerFrame::Exit { code } => json!({ "t": "exit", "code": code }),
            ServerFrame::Error { message } => json!({ "t": "error", "message": message }),
            ServerFrame::Pong => json!({ "t": "pong" }),
        };
        value.to_string()
    }

    /// Reads a frame the hub sent (the client's side, for the tests).
    #[cfg(test)]
    pub fn parse(text: &str) -> Result<ServerFrame, String> {
        let value: Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
        match value.get("t").and_then(Value::as_str).unwrap_or_default() {
            "opened" => Ok(ServerFrame::Opened {
                term: text_field(&value, "term")?,
            }),
            "out" => Ok(ServerFrame::Out {
                data: STANDARD
                    .decode(text_field(&value, "data")?)
                    .map_err(|err| err.to_string())?,
                offset: value["offset"].as_u64().ok_or("no offset")?,
            }),
            "lost" => Ok(ServerFrame::Lost {
                bytes: value["bytes"].as_u64().ok_or("no bytes")?,
            }),
            "exit" => Ok(ServerFrame::Exit {
                code: value["code"].as_i64(),
            }),
            "error" => Ok(ServerFrame::Error {
                message: text_field(&value, "message")?,
            }),
            "pong" => Ok(ServerFrame::Pong),
            other => Err(format!("no frame named {other:?}")),
        }
    }
}

/// A terminal size field: a whole number from 1 to [`MAX_SIZE`].
fn size(value: &Value, key: &str) -> Result<u16, String> {
    let n = value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("\"{key}\" must be a whole number"))?;
    if (1..=MAX_SIZE).contains(&n) {
        Ok(n as u16)
    } else {
        Err(format!("\"{key}\" must be from 1 to {MAX_SIZE}"))
    }
}

fn text_field(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("\"{key}\" must be a string"))
}

/// A string field that may be absent, null or empty (all of which mean none).
fn optional_text(value: &Value, key: &str) -> Result<Option<String>, String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.is_empty() => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("\"{key}\" must be a string")),
    }
}

fn optional_offset(value: &Value) -> Result<u64, String> {
    match value.get("offset") {
        None | Some(Value::Null) => Ok(0),
        Some(offset) => offset
            .as_u64()
            .ok_or_else(|| "\"offset\" must be a whole number".to_string()),
    }
}

/// Input the hub has taken from the client and not yet sent to the shell. Input typed within
/// [`COALESCE`] of the first pending byte goes out together; a write is at most [`WRITE_CHUNK`].
#[derive(Debug, Default)]
pub struct Coalescer {
    pending: Vec<u8>,
    since: Option<Instant>,
}

impl Coalescer {
    pub fn push(&mut self, bytes: &[u8], now: Instant) {
        if bytes.is_empty() {
            return;
        }
        self.since.get_or_insert(now);
        self.pending.extend_from_slice(bytes);
    }

    /// When the pending input is due, if any is pending.
    pub fn deadline(&self) -> Option<Instant> {
        self.since.map(|since| since + COALESCE)
    }

    /// Whether the pending input must go out now: its window closed, or a full write is waiting.
    pub fn due(&self, now: Instant) -> bool {
        self.pending.len() >= WRITE_CHUNK || self.deadline().is_some_and(|at| now >= at)
    }

    /// The pending input, split into writes of at most [`WRITE_CHUNK`]. Nothing is pending after it.
    pub fn take(&mut self) -> Vec<Vec<u8>> {
        self.since = None;
        std::mem::take(&mut self.pending)
            .chunks(WRITE_CHUNK)
            .map(<[u8]>::to_vec)
            .collect()
    }
}

/// The socket as the bridge uses it. The live socket implements it; the tests use a script.
pub trait Wire {
    fn recv(&mut self, wait: Duration) -> io::Result<Option<Message>>;
    fn send(&mut self, message: &Message) -> io::Result<()>;
    fn close(&mut self, code: u16, reason: &str);
}

impl Wire for Socket {
    fn recv(&mut self, wait: Duration) -> io::Result<Option<Message>> {
        Socket::recv(self, wait)
    }

    fn send(&mut self, message: &Message) -> io::Result<()> {
        Socket::send(self, message)
    }

    fn close(&mut self, code: u16, reason: &str) {
        Socket::close(self, code, reason);
    }
}

/// What the bridge needs from the world: the agent's tool calls, the audit, and the bus. The live
/// hub implements it over the app; the tests with a fake.
pub trait TermHost {
    /// One tool call on agent `cid`: the gateway's status and body (the same call `ide.rs` makes).
    fn tell(&self, cid: u64, name: &str, args: &Value) -> Result<(u16, Value), String>;
    /// One terminal operation on workstation `station`: the daemon's answer, or the reason it gave none.
    fn request(&self, station: &str, op: Op) -> Result<Value, String>;
    /// Appends one row to the IDE audit.
    fn append_audit(&self, row: &Value) -> Result<(), String>;
    /// Appends one row to the station audit.
    fn append_station_audit(&self, row: &Value) -> Result<(), String>;
    /// Publishes one `ide` bus event.
    fn publish(&self, data: &Value);
}

/// The live hub. The tell is the gateway call `ide.rs` makes (its own helper is private there).
struct Live(Arc<App>);

impl TermHost for Live {
    fn tell(&self, cid: u64, name: &str, args: &Value) -> Result<(u16, Value), String> {
        let body = json!({ "name": name, "args": args, "timeout_ms": TELL_TIMEOUT_MS });
        crate::fleet::gateway(
            &self.0,
            cid,
            "POST",
            &format!("/v1/agents/{cid}/tell"),
            Some(&body),
        )
    }

    fn request(&self, station: &str, op: Op) -> Result<Value, String> {
        station_op(&self.0, station, op)
    }

    fn append_audit(&self, row: &Value) -> Result<(), String> {
        self.0.db.append_ide_audit(row)
    }

    fn append_station_audit(&self, row: &Value) -> Result<(), String> {
        self.0.db.append_station_audit(row)
    }

    fn publish(&self, data: &Value) {
        self.0.bus.publish("ide", data);
    }
}

/// One tool call that did not give its answer.
#[derive(Debug)]
enum Failure {
    /// The tool answered with a refusal: the terminal is gone, or the call was wrong.
    Refused(String),
    /// The call did not reach the guest, or the guest did not answer in time.
    Unreachable(String),
}

impl Failure {
    fn into_message(self) -> String {
        match self {
            Failure::Refused(message) | Failure::Unreachable(message) => message,
        }
    }
}

/// One tool call on the target, answered with its JSON output. A workstation takes the same call as its terminal
/// operation: `ide_term_open` is `Op::Term(TermOp::Open)`, and so on.
fn call(host: &dyn TermHost, target: &Target, name: &str, args: &Value) -> Result<Value, Failure> {
    match target {
        Target::Agent(cid) => call_agent(host, *cid, name, args),
        Target::Station(id) => {
            let op = station_term_op(name, args)
                .ok_or_else(|| Failure::Refused(format!("{name} is not a terminal operation")))?;
            host.request(id, Op::Term(op)).map_err(|message| {
                if is_unreachable(&message) {
                    Failure::Unreachable(message)
                } else {
                    Failure::Refused(message)
                }
            })
        }
    }
}

/// The terminal operation a terminal tool call means, for a workstation.
fn station_term_op(name: &str, args: &Value) -> Option<TermOp> {
    let text = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_string);
    let number = |key: &str| args.get(key).and_then(Value::as_u64);
    let size = |key: &str| number(key).and_then(|n| u16::try_from(n).ok());
    Some(match name {
        "ide_term_open" => TermOp::Open {
            cols: size("cols")?,
            rows: size("rows")?,
            cwd: text("cwd"),
        },
        "ide_term_write" => TermOp::Write {
            term: text("term")?,
            data_b64: text("data_b64")?,
        },
        "ide_term_read" => TermOp::Read {
            term: text("term")?,
            offset: number("offset"),
        },
        "ide_term_resize" => TermOp::Resize {
            term: text("term")?,
            cols: size("cols")?,
            rows: size("rows")?,
        },
        "ide_term_close" => TermOp::Close {
            term: text("term")?,
        },
        _ => return None,
    })
}

/// One tool call on agent `cid`, answered with its JSON output.
fn call_agent(host: &dyn TermHost, cid: u64, name: &str, args: &Value) -> Result<Value, Failure> {
    let (status, body) = host
        .tell(cid, name, args)
        .map_err(|err| Failure::Unreachable(format!("the agent's node did not answer: {err}")))?;
    let output = body
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if status != 200 {
        return Err(match status {
            403 => Failure::Refused(output),
            404 => Failure::Unreachable(format!("no agent {cid}")),
            409 => Failure::Unreachable("the agent has no session".into()),
            504 => Failure::Unreachable("the agent did not answer in time (it may be busy)".into()),
            other => Failure::Unreachable(format!("the agent's node answered {other}")),
        });
    }
    if !body.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        let message = if output.is_empty() {
            "the agent refused the call".to_string()
        } else {
            output
        };
        return Err(Failure::Refused(message));
    }
    serde_json::from_str(&output).map_err(|err| {
        Failure::Refused(format!(
            "the agent answered with text that is not JSON: {err}"
        ))
    })
}

/// One `ide_term_read` answer.
#[derive(Debug, PartialEq, Eq)]
struct Chunk {
    /// Where `data` starts.
    offset: u64,
    /// Where the next read starts.
    next: u64,
    data: Vec<u8>,
    exited: bool,
    exit_code: Option<i64>,
    /// Bytes the guest dropped between the asked offset and `offset`.
    lost: u64,
}

fn parse_chunk(value: &Value) -> Result<Chunk, Failure> {
    let bad = |what: &str| Failure::Refused(format!("the agent's read answer has no valid {what}"));
    let data = STANDARD
        .decode(
            value
                .get("data_b64")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .map_err(|_| bad("data_b64"))?;
    Ok(Chunk {
        offset: value
            .get("offset")
            .and_then(Value::as_u64)
            .ok_or_else(|| bad("offset"))?,
        next: value
            .get("next")
            .and_then(Value::as_u64)
            .ok_or_else(|| bad("next"))?,
        data,
        exited: value
            .get("exited")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        exit_code: value.get("exit_code").and_then(Value::as_i64),
        lost: value.get("lost").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// Sends `data` to the shell. The harness takes what fits in its wait and says how much it took; the
/// rest goes out again until all of it is in.
fn write_all(
    host: &dyn TermHost,
    target: &Target,
    term: &str,
    mut rest: &[u8],
) -> Result<(), Failure> {
    let mut stalls = 0;
    while !rest.is_empty() {
        let answer = call(
            host,
            target,
            "ide_term_write",
            &json!({ "term": term, "data_b64": STANDARD.encode(rest) }),
        )?;
        let written = answer
            .get("written")
            .and_then(Value::as_u64)
            .filter(|n| *n <= rest.len() as u64)
            .ok_or_else(|| {
                Failure::Refused("the agent's write answer has no valid \"written\"".into())
            })? as usize;
        stalls = if written == 0 { stalls + 1 } else { 0 };
        if stalls >= WRITE_STALLS {
            return Err(Failure::Unreachable(
                "the shell is not reading its input".into(),
            ));
        }
        rest = &rest[written..];
    }
    Ok(())
}

/// The bridge is over: the socket was closed (by either side) or could not be used.
#[derive(Debug)]
struct Ended;

type Flow = Result<(), Ended>;

/// One socket's bridge: its guest terminal (once it has one), where its output stands, and its clocks.
struct Session<'a> {
    host: &'a dyn TermHost,
    target: Target,
    socket: &'a mut dyn Wire,
    term: Option<String>,
    /// The next offset to read from the guest.
    offset: u64,
    failures: u32,
    poll_at: Option<Instant>,
    last_data: Instant,
    /// When input was last sent to the shell: typing keeps the output polls fast.
    last_input: Instant,
    last_client: Instant,
    input: Coalescer,
}

impl Session<'_> {
    fn send(&mut self, frame: &ServerFrame) -> Flow {
        self.socket
            .send(&Message::Text(frame.wire()))
            .map_err(|_| Ended)
    }

    fn error(&mut self, message: impl Into<String>) -> Flow {
        self.send(&ServerFrame::Error {
            message: message.into(),
        })
    }

    fn end(&mut self, code: u16, reason: &str) -> Flow {
        self.socket.close(code, reason);
        Err(Ended)
    }

    fn workspace(&self) -> String {
        self.target.workspace()
    }

    /// The audit row for one owner action on a terminal, then the bus event for it. On a workstation the action
    /// is also a row of the station audit.
    fn audit(&self, kind: &str, term: &str, detail: Value) -> Result<(), String> {
        let mut row = json!({
            "at_ms": apz_plaza::now_ms(),
            "actor": "owner",
            "workspace": self.workspace(),
            "kind": kind,
            "path": null,
            "term": term,
        });
        if let (Some(map), Value::Object(detail)) = (row.as_object_mut(), detail.clone()) {
            map.extend(detail);
        }
        self.host.append_audit(&row)?;
        if let Target::Station(id) = &self.target {
            let mut data = json!({ "action": kind, "term": term });
            if let (Some(map), Value::Object(detail)) = (data.as_object_mut(), detail) {
                map.extend(detail);
            }
            self.host.append_station_audit(&crate::stations::audit_row(
                Some(id),
                None,
                "owner_ide",
                "owner",
                data,
            ))?;
        }
        Ok(())
    }

    fn publish(&self, kind: &str) {
        self.host
            .publish(&json!({ "workspace": self.workspace(), "path": null, "kind": kind }));
    }

    /// One client frame, acted on.
    fn client(&mut self, text: &str) -> Flow {
        let frame = match parse_client(text) {
            Ok(frame) => frame,
            Err(message) => return self.error(message),
        };
        match frame {
            ClientFrame::Open { cols, rows, cwd } => self.open(cols, rows, cwd),
            ClientFrame::Attach { term, offset } => self.attach(term, offset),
            ClientFrame::Input(bytes) => {
                if self.term.is_none() {
                    return self.error("there is no terminal yet: open or attach one first");
                }
                self.input.push(&bytes, Instant::now());
                Ok(())
            }
            ClientFrame::Resize { cols, rows } => self.resize(cols, rows),
            ClientFrame::Kill => self.kill(),
            ClientFrame::Ping => self.send(&ServerFrame::Pong),
        }
    }

    fn open(&mut self, cols: u16, rows: u16, cwd: Option<String>) -> Flow {
        if self.term.is_some() {
            return self.error(
                "this socket already has a terminal; open another socket for a second shell",
            );
        }
        let mut args = json!({ "cols": cols, "rows": rows });
        if let Some(cwd) = &cwd {
            args["cwd"] = json!(cwd);
        }
        let answer = match call(self.host, &self.target, "ide_term_open", &args) {
            Ok(answer) => answer,
            Err(failure) => return self.error(failure.into_message()),
        };
        let Some(term) = answer
            .get("term")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return self.error("the agent's answer names no terminal");
        };
        // A shell that is not in the audit is closed again: every terminal the owner opens is recorded.
        if let Err(err) = self.audit(
            "term_open",
            &term,
            json!({ "cols": cols, "rows": rows, "cwd": cwd }),
        ) {
            let _ = call(
                self.host,
                &self.target,
                "ide_term_close",
                &json!({ "term": term }),
            );
            self.error(format!(
                "the terminal was closed: its audit row could not be written ({err})"
            ))?;
            return self.end(1011, "the audit log is unavailable");
        }
        self.publish("term_open");
        self.term = Some(term.clone());
        self.offset = 0;
        self.last_data = Instant::now();
        self.poll_at = Some(Instant::now());
        self.send(&ServerFrame::Opened { term })
    }

    /// Re-attaches to a terminal that is running. A refusal (no such terminal) leaves the socket open
    /// with no terminal, so the client can attach another.
    fn attach(&mut self, term: String, offset: u64) -> Flow {
        if self.term.is_some() {
            return self.error("this socket already has a terminal");
        }
        self.offset = offset;
        let chunk = match self.read(&term) {
            Ok(chunk) => chunk,
            Err(failure) => return self.error(failure.into_message()),
        };
        self.term = Some(term.clone());
        self.send(&ServerFrame::Opened { term })?;
        let had = !chunk.data.is_empty();
        self.emit(chunk)?;
        self.schedule(had);
        Ok(())
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Flow {
        let Some(term) = self.term.clone() else {
            return self.error("there is no terminal yet: open or attach one first");
        };
        match call(
            self.host,
            &self.target,
            "ide_term_resize",
            &json!({ "term": term, "cols": cols, "rows": rows }),
        ) {
            Ok(_) => Ok(()),
            Err(failure) => self.error(failure.into_message()),
        }
    }

    fn kill(&mut self) -> Flow {
        let Some(term) = self.term.clone() else {
            return self.error("there is no terminal to kill");
        };
        match call(
            self.host,
            &self.target,
            "ide_term_close",
            &json!({ "term": term }),
        ) {
            Ok(_) => {
                if let Err(err) = self.audit("term_kill", &term, json!({})) {
                    self.error(format!(
                        "the terminal was closed, but its audit row could not be written ({err})"
                    ))?;
                }
                self.publish("term_kill");
                self.end(1000, "the terminal was killed")
            }
            Err(failure) => self.error(failure.into_message()),
        }
    }

    fn read(&self, term: &str) -> Result<Chunk, Failure> {
        let answer = call(
            self.host,
            &self.target,
            "ide_term_read",
            &json!({ "term": term, "offset": self.offset }),
        )?;
        parse_chunk(&answer)
    }

    /// Forwards one read: a `lost` for what the guest dropped, the `out`, and the `exit` once the shell
    /// has ended and its last output is sent.
    fn emit(&mut self, chunk: Chunk) -> Flow {
        let exited_and_drained = chunk.exited && chunk.data.is_empty();
        if chunk.lost > 0 {
            self.send(&ServerFrame::Lost { bytes: chunk.lost })?;
        }
        self.offset = chunk.next;
        if !chunk.data.is_empty() {
            self.last_data = Instant::now();
            self.send(&ServerFrame::Out {
                data: chunk.data,
                offset: chunk.offset,
            })?;
        }
        if exited_and_drained {
            self.send(&ServerFrame::Exit {
                code: chunk.exit_code,
            })?;
            self.socket.close(1000, "the shell exited");
            return Err(Ended);
        }
        Ok(())
    }

    /// Sets the next output poll from whether this one had data. The quiet spell runs from the later of
    /// the last output and the last input.
    fn schedule(&mut self, had_data: bool) {
        let now = Instant::now();
        let last = self.last_data.max(self.last_input);
        self.poll_at = Some(now + poll_delay(had_data, now.saturating_duration_since(last)));
    }

    /// One scheduled output poll. Three in a row that the node cannot answer end the socket; so does a
    /// refusal, which means the terminal is gone.
    fn poll(&mut self) -> Flow {
        self.poll_at = None;
        let Some(term) = self.term.clone() else {
            return Ok(());
        };
        match self.read(&term) {
            Ok(chunk) => {
                self.failures = 0;
                let had_data = !chunk.data.is_empty();
                self.emit(chunk)?;
                self.schedule(had_data);
                Ok(())
            }
            Err(Failure::Refused(message)) => {
                self.error(message)?;
                self.end(1011, "the terminal cannot be read")
            }
            Err(Failure::Unreachable(message)) => {
                self.failures += 1;
                self.error(message)?;
                if self.failures >= READ_FAILURES {
                    return self.end(1011, "the terminal cannot be read");
                }
                self.poll_at = Some(Instant::now() + SLOW_POLL);
                Ok(())
            }
        }
    }

    /// Sends the pending input to the shell, a write at a time. A failure is reported, and the rest of
    /// that input is dropped. A flush that wrote its input polls the output at once: the shell echoes it.
    fn flush_input(&mut self) -> Flow {
        let chunks = self.input.take();
        let Some(term) = self.term.clone() else {
            return Ok(());
        };
        let mut written = false;
        for chunk in chunks {
            if let Err(failure) = write_all(self.host, &self.target, &term, &chunk) {
                self.error(failure.into_message())?;
                break;
            }
            written = true;
        }
        if written {
            let now = Instant::now();
            self.last_input = now;
            self.poll_at = Some(now);
        }
        Ok(())
    }

    /// How long the socket may wait for a client frame: until the next poll, input flush or idle close.
    fn wait(&self, now: Instant) -> Duration {
        let mut at = self.last_client + IDLE;
        if let Some(poll) = self.poll_at {
            at = at.min(poll);
        }
        if let Some(input) = self.input.deadline() {
            at = at.min(input);
        }
        at.saturating_duration_since(now).max(MIN_WAIT)
    }
}

/// Bridges one socket to one guest terminal until either ends: the target's terminals, reached through
/// `host`, and the socket `socket`.
pub fn serve(host: &dyn TermHost, target: Target, socket: &mut dyn Wire) {
    let now = Instant::now();
    let mut session = Session {
        host,
        target,
        socket,
        term: None,
        offset: 0,
        failures: 0,
        poll_at: None,
        last_data: now,
        last_input: now,
        last_client: now,
        input: Coalescer::default(),
    };
    loop {
        let now = Instant::now();
        if idle_expired(session.last_client, now) {
            session
                .socket
                .close(1000, "idle: closed, the terminal keeps running");
            return;
        }
        if session.input.due(now) && session.flush_input().is_err() {
            return;
        }
        if session.poll_at.is_some_and(|at| at <= now) && session.poll().is_err() {
            return;
        }
        let wait = session.wait(now);
        match session.socket.recv(wait) {
            Ok(None) => {}
            Ok(Some(Message::Text(text))) => {
                session.last_client = Instant::now();
                if session.client(&text).is_err() {
                    return;
                }
            }
            Ok(Some(Message::Binary(_))) => {
                session.socket.close(1003, "text frames only");
                return;
            }
            Err(_) => return,
        }
    }
}

/// The target a terminal route names: an agent with a VM, or an online workstation (not while STOP ALL holds).
/// A repo workspace has neither a VM nor a work folder, so it is refused.
fn target_of(app: &App, id: &str) -> Result<Target, Response> {
    match id.parse::<WorkspaceId>() {
        Ok(WorkspaceId::Agent(cid)) => {
            if !app
                .agents
                .get(cid)
                .is_some_and(|agent| agent.run.is_some() && agent.vm.is_some())
            {
                return Err(Response::error(
                    404,
                    &format!("no workspace {}", WorkspaceId::Agent(cid)),
                ));
            }
            // A terminal on a sleeping agent's workspace wakes the agent first.
            crate::sleep::awake_or_503(app, cid)?;
            Ok(Target::Agent(cid))
        }
        Ok(WorkspaceId::Station(station)) => {
            station_gate(app, &station)?;
            Ok(Target::Station(station))
        }
        Ok(WorkspaceId::Repo { .. }) => Err(Response::error(
            400,
            "a terminal is for agent and station workspaces; a repo has no VM or work folder to open a shell in",
        )),
        Err(err) => Err(Response::error(400, &err.to_string())),
    }
}

/// The socket's upgrade: the subprotocol, and the origin rule of `/v1/ws`.
fn upgrade(req: &Request, run: impl FnOnce(Socket) + Send + 'static) -> Response {
    ws::accept(
        req,
        Origins::SameHost,
        Some(PROTOCOL),
        Limits::default(),
        run,
    )
}

/// The terminal sockets open now, per peer and in all (the event socket's caps, its own pool).
static SOCKETS: Slots = Slots::new();

/// The terminal sockets open now, per workspace (by its id text).
pub struct Places {
    open: Mutex<BTreeMap<String, usize>>,
}

impl Places {
    pub const fn new() -> Self {
        Places {
            open: Mutex::new(BTreeMap::new()),
        }
    }

    /// A place for a socket on `workspace`, or `None` when the workspace is at [`PER_WORKSPACE`].
    pub fn take(&'static self, workspace: &str) -> Option<Place> {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let held = open.get(workspace).copied().unwrap_or(0);
        if held >= PER_WORKSPACE {
            return None;
        }
        open.insert(workspace.to_string(), held + 1);
        Some(Place {
            places: self,
            workspace: workspace.to_string(),
        })
    }

    fn release(&self, workspace: &str) {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = open.get_mut(workspace) {
            *held = held.saturating_sub(1);
            if *held == 0 {
                open.remove(workspace);
            }
        }
    }

    #[cfg(test)]
    fn held(&self, workspace: &str) -> usize {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(workspace)
            .copied()
            .unwrap_or(0)
    }
}

/// One terminal socket's place on its workspace; it frees the place when it drops.
pub struct Place {
    places: &'static Places,
    workspace: String,
}

impl Drop for Place {
    fn drop(&mut self) {
        self.places.release(&self.workspace);
    }
}

static PLACES: Places = Places::new();

pub fn routes(router: Router<App>) -> Router<App> {
    router.get("/v1/ide/terminal/{workspace}", |app, req, p| {
        let target = match target_of(app, p.get("workspace")) {
            Ok(target) => target,
            Err(refused) => return refused,
        };
        let Some(slot) = SOCKETS.take(req.peer) else {
            return Response::error(429, "too many open terminal sockets; close one first");
        };
        let Some(place) = PLACES.take(&target.workspace()) else {
            return Response::error(
                429,
                &format!(
                    "{PER_WORKSPACE} terminal sockets are open on this workspace; close one first"
                ),
            );
        };
        let weak = app.handle();
        upgrade(req, move |mut socket| {
            let _slot = slot;
            let _place = place;
            let Some(app) = weak.upgrade() else {
                socket.close(1001, "the hub is shutting down");
                return;
            };
            serve(&Live(app), target, &mut socket);
        })
    })
    .doc(RouteDoc::new("Open a terminal socket on a workspace").errors(&[400, 429]))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::MutexGuard;

    use super::*;

    fn leaked_places() -> &'static Places {
        Box::leak(Box::new(Places::new()))
    }

    // The frame codec.

    #[test]
    fn client_frames_parse_to_their_variants_and_bad_ones_are_refused() {
        // Each good frame parses to its variant.
        for (case, input, want) in [
            (
                "an open frame takes its size",
                r#"{"t":"open","cols":80,"rows":24}"#,
                ClientFrame::Open {
                    cols: 80,
                    rows: 24,
                    cwd: None,
                },
            ),
            (
                "an empty cwd is none",
                r#"{"t":"open","cols":80,"rows":24,"cwd":""}"#,
                ClientFrame::Open {
                    cols: 80,
                    rows: 24,
                    cwd: None,
                },
            ),
            (
                "an open frame takes an optional cwd",
                r#"{"t":"open","cols":1,"rows":1000,"cwd":"src"}"#,
                ClientFrame::Open {
                    cols: 1,
                    rows: 1000,
                    cwd: Some("src".into()),
                },
            ),
            (
                "an attach takes its offset",
                r#"{"t":"attach","term":"term-1","offset":4096}"#,
                ClientFrame::Attach {
                    term: "term-1".into(),
                    offset: 4096,
                },
            ),
            (
                "an attach without an offset resumes at the start",
                r#"{"t":"attach","term":"term-1"}"#,
                ClientFrame::Attach {
                    term: "term-1".into(),
                    offset: 0,
                },
            ),
            (
                "an input frame carries its decoded bytes",
                r#"{"t":"in","data":"aGkK"}"#,
                ClientFrame::Input(b"hi\n".to_vec()),
            ),
            (
                "a resize frame takes its size",
                r#"{"t":"resize","cols":100,"rows":30}"#,
                ClientFrame::Resize {
                    cols: 100,
                    rows: 30,
                },
            ),
            ("a kill frame", r#"{"t":"kill"}"#, ClientFrame::Kill),
            ("a ping frame", r#"{"t":"ping"}"#, ClientFrame::Ping),
        ] {
            assert_eq!(parse_client(input), Ok(want), "{case}");
        }
        // A bad client frame is refused with its reason.
        for bad in [
            "not json",
            r#"{"cols":80}"#,
            r#"{"t":"fly"}"#,
            r#"{"t":"open","cols":0,"rows":24}"#,
            r#"{"t":"open","cols":1001,"rows":24}"#,
            r#"{"t":"open","cols":"80","rows":24}"#,
            r#"{"t":"attach","term":"term-1","offset":-1}"#,
            r#"{"t":"in","data":"%%%"}"#,
            r#"{"t":"in"}"#,
        ] {
            assert!(parse_client(bad).is_err(), "{bad} is refused");
        }
    }

    #[test]
    fn every_server_frame_serialises_to_its_documented_shape() {
        let cases = [
            (
                ServerFrame::Opened {
                    term: "term-1".into(),
                },
                json!({"t":"opened","term":"term-1"}),
            ),
            (
                ServerFrame::Out {
                    data: b"hi".to_vec(),
                    offset: 5,
                },
                json!({"t":"out","data":"aGk=","offset":5}),
            ),
            (
                ServerFrame::Lost { bytes: 3 },
                json!({"t":"lost","bytes":3}),
            ),
            (
                ServerFrame::Exit { code: Some(0) },
                json!({"t":"exit","code":0}),
            ),
            (
                ServerFrame::Exit { code: None },
                json!({"t":"exit","code":null}),
            ),
            (
                ServerFrame::Error {
                    message: "no".into(),
                },
                json!({"t":"error","message":"no"}),
            ),
            (ServerFrame::Pong, json!({"t":"pong"})),
        ];
        for (frame, expected) in cases {
            let wire = frame.wire();
            assert_eq!(
                serde_json::from_str::<Value>(&wire).unwrap(),
                expected,
                "{wire}"
            );
            assert_eq!(ServerFrame::parse(&wire), Ok(frame), "{wire} reads back");
        }
    }

    // The poll schedule, idle close and write coalescing.

    #[test]
    fn input_polls_the_output_at_once_and_keeps_the_polls_fast() {
        let fake = Fake::with_output(b"", 0);
        let mut socket = Client::new(Vec::new());
        let mut session = Session {
            host: &fake,
            target: Target::Agent(1),
            socket: &mut socket,
            term: Some("term-1".into()),
            offset: 0,
            failures: 0,
            poll_at: None,
            last_data: Instant::now() - Duration::from_secs(9),
            last_input: Instant::now() - Duration::from_secs(9),
            last_client: Instant::now(),
            input: Coalescer::default(),
        };
        // Quiet for a long while: the next poll is on the slow schedule.
        session.schedule(false);
        let slow = session
            .poll_at
            .unwrap()
            .saturating_duration_since(Instant::now());
        assert!(
            slow > FAST_POLL,
            "a quiet terminal polls slowly, got {slow:?}"
        );

        // Typing: the flush writes the keys, so the output is polled at once.
        session.input.push(b"ls\n", Instant::now());
        session.flush_input().unwrap();
        assert!(session.poll_at.unwrap() <= Instant::now());
        assert_eq!(fake.guest().written, b"ls\n");

        // The echo comes back on that poll, and the poll after it finds nothing: the input keeps both fast.
        session.poll().unwrap();
        assert!(
            session.poll_at.unwrap() <= Instant::now(),
            "output that flows is polled at once"
        );
        session.poll().unwrap();
        let fast = session
            .poll_at
            .unwrap()
            .saturating_duration_since(Instant::now());
        assert!(
            fast <= FAST_POLL,
            "typing keeps the polls fast, got {fast:?}"
        );
    }

    #[test]
    fn a_socket_quiet_from_its_client_for_ten_minutes_is_idle() {
        let t0 = Instant::now();
        assert!(!idle_expired(t0, t0 + Duration::from_secs(599)));
        assert!(idle_expired(t0, t0 + Duration::from_secs(600)));
    }

    #[test]
    fn a_write_of_more_than_the_chunk_size_is_split_and_due_at_once() {
        let t0 = Instant::now();
        let mut input = Coalescer::default();
        input.push(&vec![7u8; 40 * 1024], t0);
        assert!(input.due(t0), "a 40 KiB write is due without waiting");
        let sizes: Vec<usize> = input.take().iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![32 * 1024, 8 * 1024]);
    }

    // The refusals and the caps.

    #[test]
    fn the_terminal_targets_are_agents_and_stations_and_a_bad_id_is_refused() {
        assert_eq!(
            "agent:12".parse::<WorkspaceId>().ok(),
            Some(WorkspaceId::Agent(12))
        );
        assert_eq!(
            "station:w3".parse::<WorkspaceId>().ok(),
            Some(WorkspaceId::Station("w3".into()))
        );
        assert_eq!(Target::Station("w3".into()).workspace(), "station:w3");
        assert_eq!(Target::Agent(12).workspace(), "agent:12");
        for refused in [
            "repo:p1-plaza@main",
            "agent:x",
            "agent:012",
            "station:3",
            "nope",
        ] {
            assert!(
                refused
                    .parse::<WorkspaceId>()
                    .map_or(true, |id| matches!(id, WorkspaceId::Repo { .. })),
                "{refused} is not a terminal target"
            );
        }
    }

    #[test]
    fn a_workspace_holds_four_terminal_sockets_and_a_closed_one_frees_its_place() {
        let places = leaked_places();
        let held: Vec<Place> = (0..PER_WORKSPACE)
            .map(|_| places.take("agent:12").expect("under the cap"))
            .collect();
        assert!(
            places.take("agent:12").is_none(),
            "a fifth socket on the workspace is refused"
        );
        assert!(
            places.take("station:w12").is_some(),
            "another workspace still gets one"
        );
        drop(held);
        assert_eq!(places.held("agent:12"), 0);
        assert!(
            places.take("agent:12").is_some(),
            "a closed socket frees its place"
        );
    }

    fn upgrade_request(origin: Option<&str>, protocol: &str) -> Request {
        let mut headers = vec![
            ("upgrade".to_string(), "websocket".to_string()),
            ("connection".to_string(), "Upgrade".to_string()),
            ("sec-websocket-version".to_string(), "13".to_string()),
            (
                "sec-websocket-key".to_string(),
                "dGhlIHNhbXBsZSBub25jZQ==".to_string(),
            ),
            ("host".to_string(), "127.0.0.1:7700".to_string()),
            ("sec-websocket-protocol".to_string(), protocol.to_string()),
        ];
        if let Some(origin) = origin {
            headers.push(("origin".to_string(), origin.to_string()));
        }
        Request {
            method: "GET".into(),
            path: "/v1/ide/terminal/agent:12".into(),
            headers,
            ..Request::default()
        }
    }

    #[test]
    fn a_socket_is_upgraded_or_refused_by_origin_and_subprotocol() {
        for (case, origin, protocol, want) in [
            (
                "a socket from this host is upgraded",
                Some("http://127.0.0.1:7700"),
                PROTOCOL,
                101,
            ),
            (
                "a client without an origin is upgraded",
                None,
                PROTOCOL,
                101,
            ),
            (
                "a socket from another origin is refused with 403",
                Some("http://evil.example"),
                PROTOCOL,
                403,
            ),
            (
                "a socket that does not ask for the subprotocol is refused with 400",
                None,
                "apz.events.v1",
                400,
            ),
        ] {
            let request = upgrade_request(origin, protocol);
            assert_eq!(upgrade(&request, |_| {}).status, want, "{case}");
        }
    }

    // The bridge, against a fake guest and a scripted client.

    /// The fake guest's state: one terminal `term-1` that echoes its input.
    #[derive(Default)]
    struct Guest {
        output: Vec<u8>,
        /// The offset of the oldest output kept.
        oldest: usize,
        exited: bool,
        written: Vec<u8>,
        /// Writes that take only half their bytes, this many times.
        short_writes: usize,
        unreachable_reads: bool,
        audit_fails: bool,
        calls: Vec<(String, Value)>,
        audit: Vec<Value>,
        station_audit: Vec<Value>,
        published: Vec<Value>,
    }

    #[derive(Default)]
    struct Fake(Mutex<Guest>);

    impl Fake {
        fn with_output(output: &[u8], oldest: usize) -> Fake {
            Fake(Mutex::new(Guest {
                output: output.to_vec(),
                oldest,
                ..Guest::default()
            }))
        }

        fn guest(&self) -> MutexGuard<'_, Guest> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    fn ok(value: Value) -> (u16, Value) {
        (200, json!({ "ok": true, "output": value.to_string() }))
    }

    fn refused(message: impl Into<String>) -> (u16, Value) {
        (200, json!({ "ok": false, "output": message.into() }))
    }

    impl TermHost for Fake {
        fn request(&self, _station: &str, op: Op) -> Result<Value, String> {
            // The inverse of the production mapping: a workstation's terminal operation as the agent's tool call.
            let (name, args) = match op {
                Op::Term(TermOp::Open { cols, rows, cwd }) => (
                    "ide_term_open",
                    json!({ "cols": cols, "rows": rows, "cwd": cwd }),
                ),
                Op::Term(TermOp::Write { term, data_b64 }) => (
                    "ide_term_write",
                    json!({ "term": term, "data_b64": data_b64 }),
                ),
                Op::Term(TermOp::Read { term, offset }) => (
                    "ide_term_read",
                    json!({ "term": term, "offset": offset.unwrap_or(0) }),
                ),
                Op::Term(TermOp::Resize { term, cols, rows }) => (
                    "ide_term_resize",
                    json!({ "term": term, "cols": cols, "rows": rows }),
                ),
                Op::Term(TermOp::Close { term }) => ("ide_term_close", json!({ "term": term })),
                other => return Err(format!("the fake has no answer for {other:?}")),
            };
            let (status, body) = self.tell(0, name, &args)?;
            let output = body["output"].as_str().unwrap_or_default().to_string();
            if status != 200 || body["ok"] != json!(true) {
                return Err(output);
            }
            serde_json::from_str(&output).map_err(|err| err.to_string())
        }

        fn append_station_audit(&self, row: &Value) -> Result<(), String> {
            self.guest().station_audit.push(row.clone());
            Ok(())
        }

        fn tell(&self, _cid: u64, name: &str, args: &Value) -> Result<(u16, Value), String> {
            let mut guest = self.guest();
            guest.calls.push((name.to_string(), args.clone()));
            if name == "ide_term_read" && guest.unreachable_reads {
                return Ok((504, json!({ "ok": false, "output": "" })));
            }
            if name != "ide_term_open" && args.get("term").and_then(Value::as_str) != Some("term-1")
            {
                return Ok(refused(format!("no terminal {:?}", args.get("term"))));
            }
            Ok(match name {
                "ide_term_open" => ok(json!({ "term": "term-1" })),
                "ide_term_write" => {
                    let data = STANDARD
                        .decode(args["data_b64"].as_str().unwrap_or_default())
                        .unwrap_or_default();
                    let take = if guest.short_writes > 0 {
                        guest.short_writes -= 1;
                        data.len() / 2
                    } else {
                        data.len()
                    };
                    guest.written.extend_from_slice(&data[..take]);
                    guest.output.extend_from_slice(&data[..take]);
                    if guest.written.ends_with(b"exit\n") {
                        guest.exited = true;
                    }
                    ok(json!({ "ok": take == data.len(), "written": take }))
                }
                "ide_term_read" => {
                    let offset = args["offset"].as_u64().unwrap_or(0) as usize;
                    if offset > guest.output.len() {
                        return Ok(refused("offset is past the end of the output"));
                    }
                    let start = offset.max(guest.oldest);
                    let data = guest.output[start..].to_vec();
                    let exited = guest.exited;
                    ok(json!({
                        "term": "term-1",
                        "offset": start,
                        "next": start + data.len(),
                        "data_b64": STANDARD.encode(&data),
                        "exited": exited,
                        "exit_code": if exited { json!(0) } else { Value::Null },
                        "lost": guest.oldest.saturating_sub(offset),
                    }))
                }
                "ide_term_resize" => ok(json!({ "term": "term-1" })),
                "ide_term_close" => ok(json!({ "term": "term-1", "closed": true })),
                other => refused(format!("no tool {other}")),
            })
        }

        fn append_audit(&self, row: &Value) -> Result<(), String> {
            let mut guest = self.guest();
            if guest.audit_fails {
                return Err("the disk is full".into());
            }
            guest.audit.push(row.clone());
            Ok(())
        }

        fn publish(&self, data: &Value) {
            self.guest().published.push(data.clone());
        }
    }

    /// Client frames sent one at a time, each once the gate on what the hub has sent so far is open.
    enum Gate {
        Now,
        /// A frame of this `t` has been sent.
        Sent(&'static str),
        /// The shell's output so far contains this text.
        Received(&'static str),
    }

    /// After its script, the client keeps the socket quiet this long (so the hub's own polls and closes
    /// can happen), then ends it.
    const QUIET_AFTER_SCRIPT: Duration = Duration::from_secs(1);

    struct Client {
        script: VecDeque<(Gate, Message)>,
        sent: Vec<ServerFrame>,
        received: Vec<u8>,
        closed: Option<(u16, String)>,
        started: Instant,
        script_done_at: Option<Instant>,
    }

    impl Client {
        fn new(script: Vec<(Gate, Message)>) -> Client {
            Client {
                script: script.into(),
                sent: Vec::new(),
                received: Vec::new(),
                closed: None,
                started: Instant::now(),
                script_done_at: None,
            }
        }

        fn open(&self, gate: &Gate) -> bool {
            match gate {
                Gate::Now => true,
                Gate::Sent(kind) => self.sent.iter().any(|frame| {
                    let wire: Value = serde_json::from_str(&frame.wire()).unwrap_or(Value::Null);
                    wire["t"] == *kind
                }),
                Gate::Received(text) => self
                    .received
                    .windows(text.len())
                    .any(|window| window == text.as_bytes()),
            }
        }
    }

    impl Wire for Client {
        fn recv(&mut self, wait: Duration) -> io::Result<Option<Message>> {
            if self.started.elapsed() > Duration::from_secs(10) {
                return Err(io::Error::other("the test waited too long"));
            }
            let ready = match self.script.front() {
                None => {
                    let done = *self.script_done_at.get_or_insert_with(Instant::now);
                    if done.elapsed() >= QUIET_AFTER_SCRIPT {
                        return Err(io::Error::other("the script is done"));
                    }
                    false
                }
                Some((gate, _)) => self.open(gate),
            };
            if ready {
                return Ok(self.script.pop_front().map(|(_, message)| message));
            }
            std::thread::sleep(wait.min(Duration::from_millis(2)));
            Ok(None)
        }

        fn send(&mut self, message: &Message) -> io::Result<()> {
            let Message::Text(text) = message else {
                return Err(io::Error::other("the hub sends text"));
            };
            let frame = ServerFrame::parse(text).map_err(io::Error::other)?;
            if let ServerFrame::Out { data, .. } = &frame {
                self.received.extend_from_slice(data);
            }
            self.sent.push(frame);
            Ok(())
        }

        fn close(&mut self, code: u16, reason: &str) {
            self.closed = Some((code, reason.to_string()));
        }
    }

    fn text(frame: Value) -> Message {
        Message::Text(frame.to_string())
    }

    fn input(bytes: &[u8]) -> Message {
        text(json!({ "t": "in", "data": STANDARD.encode(bytes) }))
    }

    #[test]
    fn a_shell_opens_takes_keys_resizes_and_exits_with_its_code() {
        let host = Fake::default();
        host.guest().short_writes = 1;
        let mut client = Client::new(vec![
            (
                Gate::Now,
                text(json!({ "t": "open", "cols": 80, "rows": 24 })),
            ),
            (Gate::Sent("opened"), input(b"echo hi\n")),
            (
                Gate::Received("echo hi"),
                text(json!({ "t": "resize", "cols": 100, "rows": 30 })),
            ),
            (Gate::Received("echo hi"), input(b"exit\n")),
        ]);
        serve(&host, Target::Agent(12), &mut client);

        assert_eq!(client.closed, Some((1000, "the shell exited".to_string())));
        assert_eq!(
            client.sent.first(),
            Some(&ServerFrame::Opened {
                term: "term-1".into()
            })
        );
        assert!(
            client
                .received
                .windows(7)
                .any(|window| window == b"echo hi"),
            "the echo came back"
        );
        assert_eq!(
            client.sent.last(),
            Some(&ServerFrame::Exit { code: Some(0) })
        );

        let guest = host.guest();
        assert_eq!(
            guest.written, b"echo hi\nexit\n",
            "a short write is sent again from where it stopped"
        );
        let calls: Vec<&str> = guest.calls.iter().map(|(name, _)| name.as_str()).collect();
        assert!(calls.contains(&"ide_term_open"));
        assert!(calls.contains(&"ide_term_resize"));
        assert!(!calls.contains(&"ide_term_close"), "an exit is not a kill");
        let open = guest
            .calls
            .iter()
            .find(|(name, _)| name == "ide_term_open")
            .expect("opened");
        assert_eq!(
            open.1,
            json!({ "cols": 80, "rows": 24 }),
            "no cwd is sent when there is none"
        );
        assert_eq!(guest.audit.len(), 1, "one open is audited");
        assert_eq!(guest.audit[0]["kind"], "term_open");
        assert_eq!(guest.audit[0]["workspace"], "agent:12");
        assert_eq!(guest.audit[0]["term"], "term-1");
        assert_eq!(guest.audit[0]["actor"], "owner");
    }

    #[test]
    fn a_workstation_shell_is_audited_in_both_logs_and_takes_keys() {
        let host = Fake::default();
        let mut client = Client::new(vec![
            (
                Gate::Now,
                text(json!({ "t": "open", "cols": 80, "rows": 24 })),
            ),
            (Gate::Sent("opened"), input(b"exit\n")),
        ]);
        serve(&host, Target::Station("w3".into()), &mut client);

        assert_eq!(
            client.sent.first(),
            Some(&ServerFrame::Opened {
                term: "term-1".into()
            })
        );
        assert_eq!(client.closed, Some((1000, "the shell exited".to_string())));
        let guest = host.guest();
        assert_eq!(guest.written, b"exit\n");
        assert_eq!(guest.audit.len(), 1);
        assert_eq!(guest.audit[0]["workspace"], "station:w3");
        assert_eq!(guest.audit[0]["kind"], "term_open");
        assert_eq!(
            guest.station_audit.len(),
            1,
            "a workstation's shell is also a station audit row"
        );
        assert_eq!(guest.station_audit[0]["kind"], "owner_ide");
        assert_eq!(guest.station_audit[0]["station"], "w3");
        assert_eq!(guest.station_audit[0]["action"], "term_open");
        assert_eq!(guest.station_audit[0]["term"], "term-1");
    }

    #[test]
    fn a_workstation_operation_that_is_not_linked_is_unreachable_and_a_refusal_is_not() {
        assert!(matches!(
            call(
                &Fake::default(),
                &Target::Station("w3".into()),
                "ide_term_close",
                &json!({ "term": "term-9" })
            ),
            Err(Failure::Refused(_))
        ));
        assert_eq!(
            station_term_op("ide_term_open", &json!({ "cols": 80_000, "rows": 24 })),
            None,
            "an oversized size is no operation"
        );
        assert!(is_unreachable("the workstation is not linked"));
        assert!(!is_unreachable("no such terminal"));
    }

    #[test]
    fn a_reattach_resumes_at_the_offset_the_client_holds_and_kill_ends_the_shell() {
        let host = Fake::with_output(b"abcdefgh", 0);
        let mut client = Client::new(vec![
            (
                Gate::Now,
                text(json!({ "t": "attach", "term": "term-1", "offset": 4 })),
            ),
            (Gate::Received("efgh"), text(json!({ "t": "kill" }))),
        ]);
        serve(&host, Target::Agent(12), &mut client);

        assert_eq!(
            client.sent[0],
            ServerFrame::Opened {
                term: "term-1".into()
            }
        );
        assert_eq!(
            client.sent[1],
            ServerFrame::Out {
                data: b"efgh".to_vec(),
                offset: 4
            }
        );
        assert_eq!(
            client.closed,
            Some((1000, "the terminal was killed".to_string()))
        );

        let guest = host.guest();
        assert!(
            guest
                .calls
                .iter()
                .any(|(name, args)| name == "ide_term_read" && args["offset"] == 4)
        );
        assert!(
            guest
                .calls
                .iter()
                .any(|(name, args)| name == "ide_term_close" && args["term"] == "term-1")
        );
        assert_eq!(
            guest.audit.len(),
            1,
            "the kill is audited; an attach is not an open"
        );
        assert_eq!(guest.audit[0]["kind"], "term_kill");
        assert!(guest.calls.iter().all(|(name, _)| name != "ide_term_open"));
    }

    #[test]
    fn a_reattach_from_before_the_kept_output_is_told_what_was_lost() {
        let host = Fake::with_output(b"abcdefgh", 2);
        let mut client = Client::new(vec![
            (Gate::Now, text(json!({ "t": "attach", "term": "term-1" }))),
            (Gate::Received("cdefgh"), text(json!({ "t": "kill" }))),
        ]);
        serve(&host, Target::Agent(12), &mut client);

        assert_eq!(client.sent[1], ServerFrame::Lost { bytes: 2 });
        assert_eq!(
            client.sent[2],
            ServerFrame::Out {
                data: b"cdefgh".to_vec(),
                offset: 2
            }
        );
    }

    #[test]
    fn a_closed_socket_leaves_the_shell_running() {
        let host = Fake::default();
        let mut client = Client::new(vec![(
            Gate::Now,
            text(json!({ "t": "open", "cols": 80, "rows": 24 })),
        )]);
        serve(&host, Target::Agent(12), &mut client);

        assert_eq!(
            client.sent,
            vec![ServerFrame::Opened {
                term: "term-1".into()
            }]
        );
        assert!(
            host.guest()
                .calls
                .iter()
                .all(|(name, _)| name != "ide_term_close"),
            "the socket's end kills nothing"
        );
    }

    #[test]
    fn a_ping_gets_a_pong_and_a_bad_frame_keeps_the_socket_open() {
        let host = Fake::default();
        let mut client = Client::new(vec![
            (Gate::Now, Message::Text("not json".into())),
            (Gate::Now, input(b"x")),
            (Gate::Now, text(json!({ "t": "ping" }))),
        ]);
        serve(&host, Target::Agent(12), &mut client);

        let kinds: Vec<&ServerFrame> = client.sent.iter().collect();
        assert!(
            matches!(kinds[0], ServerFrame::Error { .. }),
            "a frame that is not JSON is an error"
        );
        assert!(
            matches!(kinds[1], ServerFrame::Error { .. }),
            "input with no terminal is an error"
        );
        assert_eq!(kinds[2], &ServerFrame::Pong);
        assert_eq!(client.closed, None);
    }

    #[test]
    fn a_binary_frame_closes_the_socket() {
        let host = Fake::default();
        let mut client = Client::new(vec![(Gate::Now, Message::Binary(vec![1, 2]))]);
        serve(&host, Target::Agent(12), &mut client);
        assert_eq!(client.closed.map(|(code, _)| code), Some(1003));
    }

    #[test]
    fn a_terminal_whose_open_cannot_be_audited_is_closed_again() {
        let host = Fake::default();
        host.guest().audit_fails = true;
        let mut client = Client::new(vec![(
            Gate::Now,
            text(json!({ "t": "open", "cols": 80, "rows": 24 })),
        )]);
        serve(&host, Target::Agent(12), &mut client);

        assert!(
            !client
                .sent
                .iter()
                .any(|frame| matches!(frame, ServerFrame::Opened { .. })),
            "nothing is opened"
        );
        assert!(matches!(
            client.sent.first(),
            Some(ServerFrame::Error { .. })
        ));
        assert_eq!(client.closed.map(|(code, _)| code), Some(1011));
        assert!(
            host.guest()
                .calls
                .iter()
                .any(|(name, _)| name == "ide_term_close"),
            "the shell is closed again"
        );
    }

    #[test]
    fn a_node_that_cannot_answer_three_reads_in_a_row_ends_the_socket() {
        let host = Fake::default();
        host.guest().unreachable_reads = true;
        let mut client = Client::new(vec![(
            Gate::Now,
            text(json!({ "t": "open", "cols": 80, "rows": 24 })),
        )]);
        serve(&host, Target::Agent(12), &mut client);

        let errors = client
            .sent
            .iter()
            .filter(|frame| matches!(frame, ServerFrame::Error { .. }))
            .count();
        assert_eq!(errors, 3, "each failed read is reported");
        assert_eq!(client.closed.map(|(code, _)| code), Some(1011));
    }
}
