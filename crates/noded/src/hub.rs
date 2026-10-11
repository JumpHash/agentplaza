//! The hub's HTTP surface (`vsockd --http-port`), reached over loopback.
//!
//! The console's `GET /v1/hub` proxies the hub's status; the operator's
//! `POST /v1/agents/{cid}/{tell,stop}`, the artifact read-through and the
//! social layer's delivery drive agents through [`tell`] and [`stop`]. A hub
//! that cannot be reached, or answers something unreadable, is an error the
//! caller reports as a 502 — never an empty registry or an invented answer.

use std::io;
use std::time::Duration;

use apz_http::client::{self, Reply};

/// The hub's HTTP port unless `--hub-port` says otherwise.
pub const DEFAULT_HUB_PORT: u16 = 19080;

/// The longest a `/tell` may wait for its agent: the hub's own maximum.
pub const TELL_TIMEOUT_MS: u64 = 5_000;

/// The tool the artifact read-through drives.
pub const READ_FILE: &str = "read_file";

/// A status read's whole budget.
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// A tell's or stop's whole budget: the hub's own 5 s wait, with room.
const DRIVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the hub is: a port on 127.0.0.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hub {
    port: u16,
}

impl Hub {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new(DEFAULT_HUB_PORT)
    }
}

/// `GET path` on the hub, its answer whole.
pub fn get(hub: &Hub, path: &str) -> io::Result<Reply> {
    client::send(hub.port, "GET", path, None, STATUS_TIMEOUT)
}

/// The agent's answer to a `/tell`, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TellOutcome {
    /// The agent answered; `ok: false` is its own refusal, still an answer.
    Answered { ok: bool, output: String },
    /// The hub refused in its own words and status (403 role, 404 unserved,
    /// 409 not connected, 504 timeout).
    Refused { status: u16, message: String },
    /// The hub could not be reached or answered something unreadable.
    Unreachable(String),
}

/// Drives one tool call on the agent connected as `cid`.
pub fn tell(hub: &Hub, cid: u64, name: &str, args: serde_json::Value, timeout_ms: u64) -> TellOutcome {
    let body = serde_json::json!({ "name": name, "args": args, "timeout_ms": timeout_ms });
    let reply = match post(hub, &format!("/v1/agents/{cid}/tell"), &body) {
        Ok(reply) => reply,
        Err(err) => return TellOutcome::Unreachable(err.to_string()),
    };
    if reply.status != 200 {
        return TellOutcome::Refused { status: reply.status, message: error_message(&reply) };
    }
    #[derive(serde::Deserialize)]
    struct Answer {
        ok: bool,
        output: String,
    }
    match serde_json::from_slice::<Answer>(&reply.body) {
        Ok(Answer { ok, output }) => TellOutcome::Answered { ok, output },
        Err(err) => TellOutcome::Unreachable(format!("the hub's /tell answer is not its JSON: {err}")),
    }
}

/// The hub's answer to a `/stop`, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    /// The hub's own stop view, verbatim: `{cid, stopped: true, calls}` or
    /// `{cid, stopped: false, reason}`.
    Answered(String),
    Refused { status: u16, message: String },
    Unreachable(String),
}

/// Stops the session of the agent connected as `cid`.
pub fn stop(hub: &Hub, cid: u64, reason: Option<&str>) -> StopOutcome {
    let body = match reason {
        Some(reason) => serde_json::json!({ "reason": reason }),
        None => serde_json::json!({}),
    };
    let reply = match post(hub, &format!("/v1/agents/{cid}/stop"), &body) {
        Ok(reply) => reply,
        Err(err) => return StopOutcome::Unreachable(err.to_string()),
    };
    if reply.status != 200 {
        return StopOutcome::Refused { status: reply.status, message: error_message(&reply) };
    }
    // Only the identity is checked; the view passes through as the hub wrote it.
    match reply.json() {
        Ok(view) if view["cid"].as_u64() == Some(cid) && view["stopped"].is_boolean() => {
            StopOutcome::Answered(String::from_utf8_lossy(&reply.body).trim().to_string())
        }
        Ok(view) => StopOutcome::Unreachable(format!("the hub's /stop answered {view} for guest CID {cid}")),
        Err(err) => StopOutcome::Unreachable(err.to_string()),
    }
}

fn post(hub: &Hub, path: &str, body: &serde_json::Value) -> io::Result<Reply> {
    client::send(hub.port, "POST", path, Some(body.to_string().as_bytes()), DRIVE_TIMEOUT)
}

/// The hub's `{"error": ...}`, or its body as it is.
fn error_message(reply: &Reply) -> String {
    reply
        .json()
        .ok()
        .and_then(|value| value["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| String::from_utf8_lossy(&reply.body).trim().to_string())
}
