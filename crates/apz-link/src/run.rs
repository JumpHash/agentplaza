//! `apz-link run`: the node's command channel and heartbeat.
//! - Commands: long-polls `GET /v1/nodes/{id}/commands?wait=25` (signed); each command
//!   (`{id, target: "noded"|"hub", method, path, body}`) is sent to the local noded or vsockd
//!   over loopback and its outcome posted to `POST /v1/nodes/{id}/results` as
//!   `{command, ok, status, body}`. A command for any other target is answered `ok: false`.
//! - Heartbeat: every [`HEARTBEAT`], `POST /v1/nodes/{id}/heartbeat` with `{machine, status,
//!   runs}` (the local vsockd's `GET /v1/status`, noded's `GET /v1/runs`; Null when down).
//! - A refused signature (403) stops the loop with an error: the node was revoked.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use apz_http::auth::InternalToken;
use apz_http::client;
use apz_trust::node::NodeKey;
use serde_json::{Value, json};

use crate::identity::{self, Identity};
use crate::{forward, join};

/// How often the node reports itself.
pub const HEARTBEAT: Duration = Duration::from_secs(15);

/// The local daemons this node drives, on 127.0.0.1.
#[derive(Debug, Clone)]
pub struct Local {
    /// noded's port (default 8899).
    pub noded: u16,
    /// vsockd's HTTP port (default 9180).
    pub vsockd: u16,
    /// The forwarder's port, which vsockd uses as its `--apid` (default 7701).
    pub forward: u16,
    /// The node's internal token, sent on every call to noded and vsockd (`run` sets it to
    /// `node/internal-token`, the file they are started with).
    pub token: InternalToken,
}

impl Default for Local {
    fn default() -> Self {
        Self {
            noded: 8899,
            vsockd: 9180,
            forward: 7701,
            token: InternalToken::none(),
        }
    }
}

/// A request to a local daemon on 127.0.0.1, with the node's internal token.
fn send_local(
    local: &Local,
    port: u16,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> io::Result<client::Reply> {
    let header = local.token.header();
    let headers: Vec<(&str, &str)> = header
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    client::send_with(port, method, path, &headers, body, timeout)
}

const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_TIMEOUT: Duration = Duration::from_secs(40);
const HUB_TIMEOUT: Duration = Duration::from_secs(15);
const LOCAL_TIMEOUT: Duration = Duration::from_secs(5);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
const REVOKED: &str = "the hub refused this node (revoked?)";

/// Runs one command against the local daemons; returns the result body for the hub:
/// `{"command", "ok", "status", "body"}`.
pub fn execute(local: &Local, command: &Value) -> Value {
    let id = command.get("id").cloned().unwrap_or(Value::Null);
    let port = match command.get("target").and_then(Value::as_str) {
        Some("noded") => local.noded,
        Some("hub") => local.vsockd,
        _ => {
            return json!({"command": id, "ok": false, "status": 0, "body": {"error": "unknown target"}});
        }
    };
    let method = command
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET");
    let path = command.get("path").and_then(Value::as_str).unwrap_or("/");
    let body = command
        .get("body")
        .filter(|body| !body.is_null())
        .map(|body| serde_json::to_vec(body).unwrap_or_default());
    match send_local(local, port, method, path, body.as_deref(), COMMAND_TIMEOUT) {
        Ok(reply) => {
            json!({"command": id, "ok": reply.status < 400, "status": reply.status, "body": reply_body(&reply.body)})
        }
        Err(err) => {
            json!({"command": id, "ok": false, "status": 502, "body": {"error": err.to_string()}})
        }
    }
}

/// The answer's body as JSON; Null when empty, `{"text": ...}` when it is not JSON.
fn reply_body(bytes: &[u8]) -> Value {
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| json!({"text": String::from_utf8_lossy(bytes).into_owned()}))
}

/// Runs the node until revoked or a fatal error: starts the forwarder, the heartbeat thread and
/// the command loop. Transient hub errors are retried with backoff (1 s doubling to 30 s).
pub fn run(dir: &Path, local: Local) -> Result<(), String> {
    let (identity, key) = identity::load(dir)?;
    let token = identity::internal_token(dir)?;
    // The file noded and vsockd are started with: each call to them re-reads it.
    let local = Local {
        token: InternalToken::file(dir.join("internal-token")),
        ..local
    };
    let identity = Arc::new(identity);
    let key = Arc::new(key);
    let _forwarder =
        forward::serve(identity.clone(), key.clone(), token, local.forward).map_err(|err| {
            format!(
                "the forwarder did not start on port {}: {err}",
                local.forward
            )
        })?;
    let _heartbeat = {
        let (identity, key, local) = (identity.clone(), key.clone(), local.clone());
        thread::spawn(move || beat_forever(identity, key, local))
    };
    command_loop(&identity, &key, &local)
}

/// Why a hub request failed: a refused signature is final, anything else is retried.
enum Failure {
    Revoked,
    Retry(String),
}

/// One signed request to the hub; the 2xx body on success.
fn hub_request(
    identity: &Identity,
    key: &NodeKey,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> Result<Vec<u8>, Failure> {
    let target = identity
        .signed(key, method, path, body.unwrap_or_default())
        .map_err(Failure::Retry)?;
    let reply = client::request(&target, method, path, body, timeout)
        .map_err(|err| Failure::Retry(err.to_string()))?;
    match reply.status {
        200..=299 => Ok(reply.body),
        403 => Err(Failure::Revoked),
        status => Err(Failure::Retry(format!("the hub answered {status}"))),
    }
}

/// The local daemon's JSON answer for a heartbeat field; Null when it is down.
fn local_json(local: &Local, port: u16, path: &str) -> Value {
    let answer = send_local(local, port, "GET", path, None, LOCAL_TIMEOUT)
        .and_then(|reply| Ok((reply.status, reply.json()?)));
    match answer {
        Ok((status, body)) if status < 400 => body,
        _ => Value::Null,
    }
}

/// The fields noded reports about the machine that a heartbeat carries to the hub.
const NODED_MACHINE_FIELDS: [&str; 14] = [
    "kind",
    "vendor",
    "model",
    "apple",
    "cpu",
    "os",
    "arch",
    "backend",
    "gpus",
    "accelerators",
    "trainer",
    "fabric",
    "memory_mib",
    "storage_gib",
];

/// The live load a heartbeat carries, from noded's `/v1/machine/stats`: `{cpu_pct, mem_used_mib}`;
/// null when the daemon has no sample yet or does not answer.
fn stats_report(stats: &Value) -> Value {
    if !stats.is_object() {
        return Value::Null;
    }
    json!({ "cpu_pct": stats["cpu_percent"].clone(), "mem_used_mib": stats["mem_used_mib"].clone() })
}

/// The machine a heartbeat reports: this machine's summary (`join::machine_summary`) with
/// noded's machine facts (`NODED_MACHINE_FIELDS`) laid over it when the daemon answers, and its
/// live `stats`, so the hub holds the same description a local view shows.
fn machine_report(local: &Local) -> Value {
    let mut machine = join::machine_summary();
    if let (Value::Object(report), Value::Object(facts)) =
        (&mut machine, local_json(local, local.noded, "/v1/machine"))
    {
        for key in NODED_MACHINE_FIELDS {
            if let Some(value) = facts.get(key) {
                report.insert(key.to_string(), value.clone());
            }
        }
    }
    if let Value::Object(report) = &mut machine {
        report.insert(
            "stats".to_string(),
            stats_report(&local_json(local, local.noded, "/v1/machine/stats")),
        );
    }
    machine
}

#[cfg(test)]
mod machine_tests {
    use super::*;

    #[test]
    fn live_stats_are_the_cpu_percent_and_memory_used() {
        let sample = json!({ "cpu_percent": 7.5, "mem_used_mib": 18000, "load": [0.1, 0.2, 0.3], "history": [] });
        assert_eq!(
            stats_report(&sample),
            json!({ "cpu_pct": 7.5, "mem_used_mib": 18000 })
        );
    }

    #[test]
    fn the_foundry_machine_facts_ride_the_heartbeat() {
        for field in ["accelerators", "trainer", "fabric", "gpus"] {
            assert!(
                NODED_MACHINE_FIELDS.contains(&field),
                "{field} is in the heartbeat"
            );
        }
    }

    #[test]
    fn stats_are_null_without_a_sample() {
        assert_eq!(stats_report(&Value::Null), Value::Null);
        assert_eq!(
            stats_report(&json!({"error": "no host sample yet"})),
            json!({ "cpu_pct": null, "mem_used_mib": null })
        );
    }
}

fn heartbeat_once(identity: &Identity, key: &NodeKey, local: &Local) {
    let path = format!("/v1/nodes/{}/heartbeat", identity.node_id);
    let body = json!({
        "machine": machine_report(local),
        "status": local_json(local, local.vsockd, "/v1/status"),
        "runs": local_json(local, local.noded, "/v1/runs"),
    });
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    // A failed beat is not reported here: the command loop reports the hub's state.
    let _ = hub_request(
        identity,
        key,
        "POST",
        &path,
        Some(bytes.as_slice()),
        HUB_TIMEOUT,
    );
}

fn beat_forever(identity: Arc<Identity>, key: Arc<NodeKey>, local: Local) {
    loop {
        heartbeat_once(&identity, &key, &local);
        thread::sleep(HEARTBEAT);
    }
}

fn poll_commands(identity: &Identity, key: &NodeKey, path: &str) -> Result<Vec<Value>, Failure> {
    let body = hub_request(identity, key, "GET", path, None, POLL_TIMEOUT)?;
    serde_json::from_slice(&body)
        .map_err(|err| Failure::Retry(format!("the command list is not JSON: {err}")))
}

fn revoked() -> String {
    eprintln!("revoked: the hub refused this node");
    REVOKED.into()
}

/// Posts one command's result, retrying until the hub takes it or refuses the node.
fn deliver(identity: &Identity, key: &NodeKey, path: &str, result: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(result).unwrap_or_default();
    let mut backoff = BACKOFF_MIN;
    loop {
        match hub_request(
            identity,
            key,
            "POST",
            path,
            Some(bytes.as_slice()),
            HUB_TIMEOUT,
        ) {
            Ok(_) => return Ok(()),
            Err(Failure::Revoked) => return Err(revoked()),
            Err(Failure::Retry(message)) => {
                eprintln!("retrying the result delivery: {message}");
                thread::sleep(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

fn command_loop(identity: &Identity, key: &NodeKey, local: &Local) -> Result<(), String> {
    let poll_path = format!("/v1/nodes/{}/commands?wait=25", identity.node_id);
    let results_path = format!("/v1/nodes/{}/results", identity.node_id);
    let mut backoff = BACKOFF_MIN;
    let mut connected: Option<bool> = None;
    loop {
        match poll_commands(identity, key, &poll_path) {
            Ok(commands) => {
                if connected != Some(true) {
                    eprintln!("connected to the hub");
                    connected = Some(true);
                }
                backoff = BACKOFF_MIN;
                for command in &commands {
                    deliver(identity, key, &results_path, &execute(local, command))?;
                }
            }
            Err(Failure::Revoked) => return Err(revoked()),
            Err(Failure::Retry(message)) => {
                if connected != Some(false) {
                    eprintln!("retrying the hub: {message}");
                    connected = Some(false);
                }
                thread::sleep(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_http::{Response, Router, Server};
    use std::fs;

    fn daemon() -> apz_http::Handle {
        let router: Router<()> = Router::new().get("/v1/runs", |_, _, _| {
            Response::json(200, &json!({"runs": []}))
        });
        Server::bind(0)
            .expect("an ephemeral port")
            .spawn(move |request| router.handle(&(), request))
            .expect("serves")
    }

    #[test]
    fn a_noded_command_is_answered_with_the_daemons_reply() {
        let daemon = daemon();
        let local = Local {
            noded: daemon.port(),
            vsockd: 0,
            forward: 0,
            ..Local::default()
        };
        let result = execute(
            &local,
            &json!({"id": 7, "target": "noded", "method": "GET", "path": "/v1/runs", "body": null}),
        );
        assert_eq!(
            result,
            json!({"command": 7, "ok": true, "status": 200, "body": {"runs": []}})
        );
    }

    #[test]
    fn a_guarded_daemon_gets_the_nodes_token_and_refuses_without_it() {
        let dir = std::env::temp_dir().join(format!("apz-link-run-token-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("internal-token"), "node-secret\n").unwrap();
        let router: Router<()> = Router::new().get("/v1/runs", |_, _, _| {
            Response::json(200, &json!({"runs": []}))
        });
        let guarded = InternalToken::file(dir.join("internal-token"))
            .guard(move |request| router.handle(&(), request));
        let daemon = Server::bind(0)
            .expect("an ephemeral port")
            .spawn(guarded)
            .expect("serves");
        let mut local = Local {
            noded: daemon.port(),
            vsockd: 0,
            forward: 0,
            ..Local::default()
        };
        let command =
            json!({"id": 8, "target": "noded", "method": "GET", "path": "/v1/runs", "body": null});
        // Without the token the daemon refuses; with it, the daemon answers.
        assert_eq!(execute(&local, &command)["status"], json!(401));
        local.token = InternalToken::file(dir.join("internal-token"));
        assert_eq!(execute(&local, &command)["status"], json!(200));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_target_is_refused_without_a_request() {
        let result = execute(
            &Local::default(),
            &json!({"id": "c1", "target": "elsewhere", "method": "GET", "path": "/", "body": null}),
        );
        assert_eq!(result["ok"], json!(false));
        assert_eq!(result["status"], json!(0));
        assert_eq!(result["command"], json!("c1"));
    }
}
