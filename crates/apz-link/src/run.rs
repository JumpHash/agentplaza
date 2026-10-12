//! `apz-link run`: the node's command channel and heartbeat.
//! - Commands: long-polls `GET /v1/nodes/{id}/commands?wait=25` (signed); each command
//!   (`{id, target: "noded"|"hub", method, path, body}`) is sent to the local noded or vsockd
//!   over loopback and its outcome posted to `POST /v1/nodes/{id}/results` as
//!   `{command, ok, status, body}`. A command for any other target is answered `ok: false`.
//! - Heartbeat: every [`HEARTBEAT`], `POST /v1/nodes/{id}/heartbeat` with `{machine, status,
//!   runs}` (the local vsockd's `GET /v1/status`, noded's `GET /v1/runs`; Null when down).
//! - A refused signature (403) stops the loop with an error: the node was revoked.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

/// How often the node reports itself.
pub const HEARTBEAT: Duration = Duration::from_secs(15);

/// The local daemons this node drives, on 127.0.0.1.
#[derive(Debug, Clone, Copy)]
pub struct Local {
    /// noded's port (default 8899).
    pub noded: u16,
    /// vsockd's HTTP port (default 9180).
    pub vsockd: u16,
    /// The forwarder's port, which vsockd uses as its `--apid` (default 7701).
    pub forward: u16,
}

impl Default for Local {
    fn default() -> Self {
        Self { noded: 8899, vsockd: 9180, forward: 7701 }
    }
}

/// Runs one command against the local daemons; returns the result body for the hub:
/// `{"command", "ok", "status", "body"}`.
pub fn execute(local: &Local, command: &Value) -> Value {
    let _ = (local, command);
    Value::Null
}

/// Runs the node until revoked or a fatal error: starts the forwarder, the heartbeat thread and
/// the command loop. Transient hub errors are retried with backoff (1 s doubling to 30 s).
pub fn run(dir: &Path, local: Local) -> Result<(), String> {
    let _ = (dir, local);
    Err("contract: L3".into())
}
