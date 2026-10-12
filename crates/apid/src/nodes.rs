//! The hub's nodes: the machines that run agents for it.
//!
//! A node enters only with a join token the owner issued (`apz_trust::token`), proves itself on
//! every later request with its Ed25519 key (`apz_trust::node`), and is driven through a command
//! queue it long-polls (it dials out, so NAT and firewalls do not matter). Each node owns a
//! disjoint guest-CID range so agent ids stay unique across the plaza. The local node (the one on
//! the hub's own machine, `n0`) needs no join and no queue: apid talks to it over loopback.
//! Saved in Postgres ("nodes"); join-code secrets are never stored, only their SHA-256.
//!
//! Owner routes (owner auth applies):
//!   GET    /v1/nodes                      → [NodeView]
//!   POST   /v1/nodes/codes   {label?, ttl_minutes?} → {id, token, expires} (the token is shown once)
//!   GET    /v1/nodes/codes                → [CodeView] (no secrets)
//!   DELETE /v1/nodes/codes/{id}           → revokes an unused code
//!   POST   /v1/nodes/{id}/revoke          → the node's signatures stop working at once
//!   POST   /v1/nodes/{id}/drain  {on}     → no new agents are placed on it
//! Node routes (public, rate-limited / signed):
//!   POST   /v1/nodes/join   {secret, public_key, name, machine} → {node_id, cid_base, cid_count, hub}
//!   GET    /v1/nodes/{id}/commands?wait=25   (signed) → [Command]; also the node's heartbeat
//!   POST   /v1/nodes/{id}/results            (signed) {command, ok, status, body}
//!   POST   /v1/nodes/{id}/heartbeat          (signed) {machine}

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::App;

/// Guest CIDs per node; node `n0` (local) keeps today's 200..=230 and 20..=99 fleets.
pub const CIDS_PER_NODE: u64 = 100;
/// Remote nodes' CID ranges start here (n1: 1000..1099, n2: 1100..1199, …).
pub const REMOTE_CID_BASE: u64 = 1000;
/// A join code lives at most this long.
pub const MAX_CODE_MINUTES: u64 = 60;
/// Redemptions allowed per source address per minute, and attempts per code.
pub const JOIN_RATE_PER_MINUTE: u32 = 5;
pub const JOIN_TRIES_PER_CODE: u32 = 3;
/// A node is offline when it has not polled for this long.
pub const OFFLINE_AFTER_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// `n<seq>`; `n0` is the hub's own machine.
    pub id: String,
    pub name: String,
    /// Ed25519 public key, base64url (empty for `n0`).
    pub public_key: String,
    pub cid_base: u64,
    pub cid_count: u64,
    pub joined_ms: u64,
    pub last_seen_ms: u64,
    /// The machine summary the node last reported (cpus, memory, host, version).
    #[serde(default)]
    pub machine: Value,
    #[serde(default)]
    pub draining: bool,
    #[serde(default)]
    pub revoked: bool,
    /// Where it last called from (for the owner's eyes only).
    #[serde(default)]
    pub address: String,
}

/// A node as the console lists it: the record plus "online" and its agents' count.
#[derive(Debug, Clone, Serialize)]
pub struct NodeView {
    #[serde(flatten)]
    pub node: Node,
    pub online: bool,
    pub agents: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Code {
    /// `c<seq>`.
    pub id: String,
    pub label: String,
    /// SHA-256 of the secret, hex. The secret itself is never stored.
    pub secret_sha256: String,
    pub created_ms: u64,
    pub expires_ms: u64,
    /// Set when redeemed: the node it made.
    #[serde(default)]
    pub used_by: Option<String>,
    #[serde(default)]
    pub revoked: bool,
    /// Failed redemption attempts against it.
    #[serde(default)]
    pub failures: u32,
}

/// What the console shows of a code (no hash).
#[derive(Debug, Clone, Serialize)]
pub struct CodeView {
    pub id: String,
    pub label: String,
    pub created_ms: u64,
    pub expires_ms: u64,
    /// "active" | "used" | "expired" | "revoked".
    pub state: String,
    pub used_by: Option<String>,
}

/// One command for a node to run against its local noded / vsockd.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    /// `k<seq>`.
    pub id: String,
    /// "noded" (an HTTP call to the node's noded) | "hub" (an HTTP call to the node's vsockd).
    pub target: String,
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub body: Value,
    pub created_ms: u64,
}

/// A command's outcome, as the node posts it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandResult {
    pub command: String,
    pub ok: bool,
    pub status: u16,
    #[serde(default)]
    pub body: Value,
}

#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    nodes: Vec<Node>,
    codes: Vec<Code>,
}

#[derive(Default)]
pub struct Store {
    inner: Mutex<Persisted>,
    /// Per node: queued commands, and results waiting for a caller. Each is paired with its signal.
    queues: Mutex<BTreeMap<String, VecDeque<Command>>>,
    queue_signal: Condvar,
    results: Mutex<BTreeMap<String, CommandResult>>,
    result_signal: Condvar,
    /// The last command sequence number handed out (`k<seq>`).
    seq: AtomicU64,
    /// Nonces seen (node id, nonce) → expiry ms; join attempts per source address.
    nonces: Mutex<BTreeMap<(String, String), u64>>,
    join_attempts: Mutex<BTreeMap<String, Vec<u64>>>,
}

impl Store {
    pub fn snapshot(&self) -> String {
        self.inner.lock().ok().and_then(|p| serde_json::to_string(&*p).ok()).unwrap_or_default()
    }
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let p: Persisted = serde_json::from_str(json).map_err(|e| e.to_string())?;
        *self.inner.lock().map_err(|e| e.to_string())? = p;
        Ok(())
    }
    pub fn list(&self) -> Vec<Node> {
        self.inner.lock().map(|p| p.nodes.clone()).unwrap_or_default()
    }
    pub fn get(&self, id: &str) -> Option<Node> {
        self.list().into_iter().find(|n| n.id == id)
    }
    /// The node whose CID range holds `cid`; None for the local node's CIDs (and unknown ones).
    pub fn owner_of_cid(&self, cid: u64) -> Option<Node> {
        self.list().into_iter().find(|n| !n.revoked && cid >= n.cid_base && cid < n.cid_base + n.cid_count)
    }

    /// Issues a code: returns its id, the token text (shown once) and its expiry.
    pub fn issue_code(&self, _app: &App, _label: &str, _ttl_minutes: u64) -> Result<(String, String, u64), String> {
        Err("contract: N1".into())
    }
    pub fn codes(&self, _now_ms: u64) -> Vec<CodeView> {
        Vec::new() // contract: N1
    }
    pub fn revoke_code(&self, _id: &str) -> Result<(), String> {
        Err("contract: N1".into())
    }
    /// Redeems a secret for a new node (rate limits, expiry, single use, constant-time compare).
    pub fn join(&self, _source: &str, _secret_b64: &str, _public_key: &str, _name: &str, _machine: Value, _now_ms: u64) -> Result<Node, String> {
        Err("contract: N1".into())
    }
    pub fn revoke(&self, _id: &str) -> Result<Node, String> {
        Err("contract: N1".into())
    }
    pub fn drain(&self, _id: &str, _on: bool) -> Result<Node, String> {
        Err("contract: N1".into())
    }
    /// Checks a signed node request: known, not revoked, ts within skew, fresh nonce, valid
    /// signature. Records `last_seen_ms` and `address`. Returns the node.
    pub fn authenticate(&self, _req: &Request, _now_ms: u64) -> Result<Node, String> {
        Err("contract: N1".into())
    }

    /// Queues a command for a node; the caller may wait for its result with `wait_result`.
    pub fn enqueue(&self, node: &str, target: &str, method: &str, path: &str, body: Value) -> String {
        let id = format!("k{}", self.seq.fetch_add(1, Ordering::Relaxed) + 1);
        let command = Command {
            id: id.clone(),
            target: target.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            body,
            created_ms: apz_plaza::now_ms(),
        };
        if let Ok(mut queues) = self.queues.lock() {
            queues.entry(node.to_string()).or_default().push_back(command);
        }
        self.queue_signal.notify_all();
        id
    }
    /// Takes the node's queued commands, waiting up to `wait_ms` for one to arrive (capped at 25 s).
    pub fn take(&self, node: &str, wait_ms: u64) -> Vec<Command> {
        let deadline = Instant::now() + Duration::from_millis(wait_ms.min(25_000));
        let mut queues = match self.queues.lock() {
            Ok(q) => q,
            Err(_) => return Vec::new(),
        };
        loop {
            if queues.get(node).is_some_and(|q| !q.is_empty()) {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            queues = match self.queue_signal.wait_timeout(queues, deadline - now) {
                Ok((guard, _)) => guard,
                Err(_) => return Vec::new(),
            };
        }
        queues.get_mut(node).map(|q| q.drain(..).collect()).unwrap_or_default()
    }
    /// Stores a command's result and wakes whoever waits for it.
    pub fn put_result(&self, _node: &str, result: CommandResult) {
        if let Ok(mut results) = self.results.lock() {
            results.insert(result.command.clone(), result);
        }
        self.result_signal.notify_all();
    }
    /// Waits up to `wait_ms` for a command's result, removing it when found.
    pub fn wait_result(&self, command: &str, wait_ms: u64) -> Option<CommandResult> {
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        let mut results = self.results.lock().ok()?;
        loop {
            if let Some(result) = results.remove(command) {
                return Some(result);
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            results = self.result_signal.wait_timeout(results, deadline - now).ok()?.0;
        }
    }
    /// Records the machine summary a node reported with its heartbeat.
    pub fn set_machine(&self, id: &str, machine: Value) -> Result<(), String> {
        let mut persisted = self.inner.lock().map_err(|e| e.to_string())?;
        let node = persisted.nodes.iter_mut().find(|n| n.id == id).ok_or_else(|| "unknown node".to_string())?;
        node.machine = machine;
        Ok(())
    }
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/nodes", |app, _, _| Response::json(200, &views(app)))
        .post("/v1/nodes/codes", issue_route)
        .get("/v1/nodes/codes", |app, _, _| Response::json(200, &app.nodes.codes(apz_plaza::now_ms())))
        .delete("/v1/nodes/codes/{id}", revoke_code_route)
        .post("/v1/nodes/{id}/revoke", revoke_route)
        .post("/v1/nodes/{id}/drain", drain_route)
        .post("/v1/nodes/join", join_route)
        .get("/v1/nodes/{id}/commands", commands_route)
        .post("/v1/nodes/{id}/results", results_route)
        .post("/v1/nodes/{id}/heartbeat", heartbeat_route)
}

fn views(_app: &App) -> Vec<NodeView> {
    Vec::new() // contract: N1
}
fn issue_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: N1") }
fn revoke_code_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: N1") }
fn revoke_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: N1") }
fn drain_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: N1") }
fn join_route(_app: &App, _req: &Request, _p: &Params<'_>) -> Response { Response::error(501, "contract: N1") }
/// The node a signed request speaks for, provided it is the node named in the path.
fn signed_node(app: &App, req: &Request, p: &Params<'_>) -> Result<Node, Response> {
    match app.nodes.authenticate(req, apz_plaza::now_ms()) {
        Ok(n) if n.id == p.get("id") => Ok(n),
        _ => Err(Response::error(401, "not this node")),
    }
}

fn commands_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let node = match signed_node(app, req, p) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let wait = req.query_param("wait").and_then(|w| w.parse::<u64>().ok()).unwrap_or(25).min(25);
    Response::json(200, &app.nodes.take(&node.id, wait * 1000))
}

fn results_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let node = match signed_node(app, req, p) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let result: CommandResult = match req.json() {
        Ok(r) => r,
        Err(_) => return Response::error(400, "bad command result"),
    };
    app.nodes.put_result(&node.id, result);
    Response::json(200, &serde_json::json!({"ok": true}))
}

#[derive(Deserialize)]
struct Heartbeat {
    machine: Value,
}

fn heartbeat_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let node = match signed_node(app, req, p) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let beat: Heartbeat = match req.json() {
        Ok(b) => b,
        Err(_) => return Response::error(400, "bad heartbeat"),
    };
    match app.nodes.set_machine(&node.id, beat.machine) {
        Ok(()) => Response::json(200, &serde_json::json!({"ok": true})),
        Err(e) => Response::error(404, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn enqueue_then_take_returns_it_once() {
        let store = Store::default();
        let id = store.enqueue("n1", "noded", "GET", "/v1/status", Value::Null);
        let got = store.take("n1", 0);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, id);
        assert_eq!(got[0].path, "/v1/status");
        assert!(store.take("n1", 0).is_empty());
        assert!(store.take("n2", 0).is_empty());
    }

    #[test]
    fn take_waits_then_returns_empty_on_timeout() {
        let store = Store::default();
        let started = Instant::now();
        assert!(store.take("n1", 60).is_empty());
        assert!(started.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    fn take_wakes_when_a_command_arrives() {
        let store = Arc::new(Store::default());
        let writer = store.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            writer.enqueue("n1", "hub", "POST", "/x", Value::Null);
        });
        let got = store.take("n1", 5_000);
        handle.join().unwrap();
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn result_from_another_thread_wakes_wait_result() {
        let store = Arc::new(Store::default());
        let writer = store.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            writer.put_result(
                "n1",
                CommandResult { command: "k9".into(), ok: true, status: 200, body: Value::Null },
            );
        });
        let started = Instant::now();
        let got = store.wait_result("k9", 5_000);
        handle.join().unwrap();
        assert_eq!(got.map(|r| r.status), Some(200));
        assert!(started.elapsed() < Duration::from_millis(4_000));
        assert!(store.wait_result("k9", 10).is_none(), "a result is removed once taken");
    }

    #[test]
    fn set_machine_stores_for_known_node_only() {
        let store = Store::default();
        assert!(store.set_machine("n9", Value::Null).is_err());
    }
}
