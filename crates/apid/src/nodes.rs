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
use apz_trust::node::{H_NODE, H_NONCE, H_SIG, H_TS, MAX_SKEW_SECS, NONCE_TTL_SECS};
use serde::de::DeserializeOwned;
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

/// The one message every failed redemption gets, so a caller learns nothing about the code.
const INVALID_CODE: &str = "this join code is not valid";
const RATE_LIMITED: &str = "too many attempts; wait a minute";
/// The rate-limit key of a request: its peer address (every caller without one shares a bucket).
fn source_of(req: &Request) -> String {
    req.peer.map(|ip| ip.to_string()).unwrap_or_else(|| "unknown".into())
}
const JOIN_WINDOW_MS: u64 = 60_000;
const DEFAULT_TTL_MINUTES: u64 = 15;

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
    /// Per node, from its last heartbeat: its gateway's `/v1/status` and its noded's `/v1/runs`.
    reports: Mutex<BTreeMap<String, (Value, Value)>>,
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
    pub fn issue_code(&self, app: &App, label: &str, ttl_minutes: u64) -> Result<(String, String, u64), String> {
        let ttl = match ttl_minutes {
            0 => DEFAULT_TTL_MINUTES,
            m => m.min(MAX_CODE_MINUTES),
        };
        let now = apz_plaza::now_ms();
        let public_url = app.hub_auth.config.lock().map_err(|_| "hub config unavailable".to_string())?.public_url.clone();
        let token = apz_trust::token::JoinToken::issue(&public_url, app.hub_auth.cert_sha256, now / 1000, ttl * 60);
        let text = token.encode();
        let expires_ms = now + ttl * 60_000;
        let mut p = self.inner.lock().map_err(|e| e.to_string())?;
        let id = format!("c{}", next_seq(p.codes.iter().map(|c| c.id.as_str()), 'c'));
        p.codes.push(Code {
            id: id.clone(),
            label: label.trim().chars().take(64).collect(),
            secret_sha256: hex(&token.secret_hash()),
            created_ms: now,
            expires_ms,
            used_by: None,
            revoked: false,
            failures: 0,
        });
        Ok((id, text, expires_ms))
    }
    pub fn codes(&self, now_ms: u64) -> Vec<CodeView> {
        self.inner
            .lock()
            .map(|p| {
                p.codes
                    .iter()
                    .map(|c| CodeView {
                        id: c.id.clone(),
                        label: c.label.clone(),
                        created_ms: c.created_ms,
                        expires_ms: c.expires_ms,
                        state: code_state(c, now_ms).to_string(),
                        used_by: c.used_by.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn revoke_code(&self, id: &str) -> Result<(), String> {
        let mut p = self.inner.lock().map_err(|e| e.to_string())?;
        let code = p.codes.iter_mut().find(|c| c.id == id).ok_or("no such code")?;
        if code.used_by.is_some() {
            return Err("that code was already used".into());
        }
        code.revoked = true;
        Ok(())
    }
    /// Redeems a secret for a new node (rate limits, expiry, single use, constant-time compare).
    pub fn join(&self, source: &str, secret_b64: &str, public_key: &str, name: &str, machine: Value, now_ms: u64) -> Result<Node, String> {
        self.admit(source, now_ms)?;
        let secret = apz_trust::unb64(secret_b64).ok().filter(|s| s.len() == 16).ok_or(INVALID_CODE)?;
        let hash = hex(&apz_trust::sha256(&secret));

        let mut p = self.inner.lock().map_err(|e| e.to_string())?;
        let Persisted { nodes, codes } = &mut *p;
        let mut found = None;
        for (i, code) in codes.iter().enumerate() {
            if apz_trust::ct_eq(code.secret_sha256.as_bytes(), hash.as_bytes()) {
                found = Some(i);
            }
        }
        let Some(i) = found else {
            return Err(INVALID_CODE.to_string());
        };
        let code = &mut codes[i];
        let usable = code.used_by.is_none() && !code.revoked && now_ms < code.expires_ms && code.failures < JOIN_TRIES_PER_CODE;
        if !usable {
            code.failures = code.failures.saturating_add(1);
            return Err(INVALID_CODE.to_string());
        }

        // The secret matched, so these failures are the caller's own tries against this code.
        let key_ok = apz_trust::unb64(public_key).map(|k| k.len() == 32).unwrap_or(false);
        if !key_ok {
            code.failures = code.failures.saturating_add(1);
            return Err("the public key must be an Ed25519 key (32 bytes, base64url)".into());
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 || name.chars().any(|c| c.is_control()) {
            code.failures = code.failures.saturating_add(1);
            return Err("the name must be 1 to 64 printable characters".into());
        }

        let seq = next_seq(nodes.iter().map(|n| n.id.as_str()), 'n');
        let node = Node {
            id: format!("n{seq}"),
            name: name.to_string(),
            public_key: public_key.to_string(),
            cid_base: REMOTE_CID_BASE + (seq - 1) * CIDS_PER_NODE,
            cid_count: CIDS_PER_NODE,
            joined_ms: now_ms,
            last_seen_ms: now_ms,
            machine,
            draining: false,
            revoked: false,
            address: source.to_string(),
        };
        code.used_by = Some(node.id.clone());
        nodes.push(node.clone());
        Ok(node)
    }
    pub fn revoke(&self, id: &str) -> Result<Node, String> {
        let mut p = self.inner.lock().map_err(|e| e.to_string())?;
        let node = p.nodes.iter_mut().find(|n| n.id == id).ok_or("no such node")?;
        node.revoked = true;
        Ok(node.clone())
    }
    pub fn drain(&self, id: &str, on: bool) -> Result<Node, String> {
        let mut p = self.inner.lock().map_err(|e| e.to_string())?;
        let node = p.nodes.iter_mut().find(|n| n.id == id).ok_or("no such node")?;
        node.draining = on;
        Ok(node.clone())
    }
    /// Checks a signed node request: known, not revoked, ts within skew, fresh nonce, valid
    /// signature. Records `last_seen_ms` and `address`. Returns the node.
    pub fn authenticate(&self, req: &Request, now_ms: u64) -> Result<Node, String> {
        let id = req.header(H_NODE).ok_or("missing node identity")?;
        let node = self.get(id).filter(|n| !n.revoked).ok_or("unknown or revoked node")?;
        let ts: u64 = req.header(H_TS).and_then(|t| t.parse().ok()).ok_or("missing or bad timestamp")?;
        let nonce = req.header(H_NONCE).filter(|n| !n.is_empty()).ok_or("missing nonce")?;
        let sig = req.header(H_SIG).ok_or("missing signature")?;
        check_freshness(ts, now_ms)?;
        let path = if req.query.is_empty() { req.path.clone() } else { format!("{}?{}", req.path, req.query) };
        if !apz_trust::node::verify(&node.public_key, &req.method, &path, ts, nonce, &req.body, sig) {
            return Err("bad signature".into());
        }
        {
            // After the signature check, so a forged request cannot burn a real node's nonce.
            let mut nonces = self.nonces.lock().map_err(|e| e.to_string())?;
            record_nonce(&mut nonces, (node.id.clone(), nonce.to_string()), now_ms)?;
        }
        let mut p = self.inner.lock().map_err(|e| e.to_string())?;
        let stored = p.nodes.iter_mut().find(|n| n.id == node.id).ok_or("unknown node")?;
        stored.last_seen_ms = now_ms;
        if let Some(ip) = req.peer {
            stored.address = ip.to_string();
        }
        Ok(stored.clone())
    }

    /// Join attempts per source in the last minute; a refused attempt is not recorded.
    fn admit(&self, source: &str, now_ms: u64) -> Result<(), String> {
        let mut attempts = self.join_attempts.lock().map_err(|e| e.to_string())?;
        let cutoff = now_ms.saturating_sub(JOIN_WINDOW_MS);
        attempts.retain(|_, times| {
            times.retain(|t| *t > cutoff);
            !times.is_empty()
        });
        let times = attempts.entry(source.to_string()).or_default();
        if times.len() >= JOIN_RATE_PER_MINUTE as usize {
            return Err(RATE_LIMITED.into());
        }
        times.push(now_ms);
        Ok(())
    }

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
    /// Keeps what a node's heartbeat said about its agents (gateway status) and VMs (runs).
    pub fn set_report(&self, id: &str, status: Value, runs: Value) {
        if let Ok(mut reports) = self.reports.lock() {
            reports.insert(id.to_string(), (status, runs));
        }
    }
    /// The node's last reported (gateway status, runs).
    pub fn report(&self, id: &str) -> Option<(Value, Value)> {
        self.reports.lock().ok()?.get(id).cloned()
    }
}

fn code_state(code: &Code, now_ms: u64) -> &'static str {
    if code.revoked {
        "revoked"
    } else if code.used_by.is_some() {
        "used"
    } else if now_ms >= code.expires_ms {
        "expired"
    } else {
        "active"
    }
}

/// The request's timestamp (unix seconds) must be within the allowed skew of `now_ms`.
pub fn check_freshness(ts_secs: u64, now_ms: u64) -> Result<(), String> {
    if (now_ms / 1000).abs_diff(ts_secs) > MAX_SKEW_SECS {
        return Err("request timestamp is outside the allowed window".into());
    }
    Ok(())
}

/// Records a nonce for `key` (node, nonce) until its TTL passes; refuses one already recorded.
fn record_nonce(nonces: &mut BTreeMap<(String, String), u64>, key: (String, String), now_ms: u64) -> Result<(), String> {
    nonces.retain(|_, expiry| *expiry > now_ms);
    if nonces.contains_key(&key) {
        return Err("nonce already used".into());
    }
    nonces.insert(key, now_ms + NONCE_TTL_SECS * 1000);
    Ok(())
}

/// The next sequence number after the ids that start with `prefix` (`n3` → 4; none → 1).
fn next_seq<'a>(ids: impl Iterator<Item = &'a str>, prefix: char) -> u64 {
    ids.filter_map(|id| id.strip_prefix(prefix)?.parse::<u64>().ok()).max().unwrap_or(0) + 1
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn view(node: Node, agents: &[crate::agents::Agent], now_ms: u64) -> NodeView {
    let online = !node.revoked && now_ms.saturating_sub(node.last_seen_ms) < OFFLINE_AFTER_MS;
    let end = node.cid_base + node.cid_count;
    let count = agents.iter().filter(|a| a.cid >= node.cid_base && a.cid < end).count();
    NodeView { node, online, agents: u32::try_from(count).unwrap_or(u32::MAX) }
}

fn view_of(app: &App, node: Node) -> NodeView {
    view(node, &app.agents.list(), apz_plaza::now_ms())
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

fn views(app: &App) -> Vec<NodeView> {
    let now = apz_plaza::now_ms();
    let agents = app.agents.list();
    app.nodes.list().into_iter().map(|node| view(node, &agents, now)).collect()
}

#[derive(Default, Deserialize)]
struct IssueBody {
    label: Option<String>,
    ttl_minutes: Option<u64>,
}

#[derive(Deserialize)]
struct JoinBody {
    secret: String,
    public_key: String,
    name: String,
    #[serde(default)]
    machine: Value,
}

#[derive(Default, Deserialize)]
struct DrainBody {
    on: Option<bool>,
}

fn parse<T: DeserializeOwned>(req: &Request) -> Result<T, Response> {
    req.json::<T>().map_err(|e| Response::error(400, &format!("bad body: {e}")))
}

fn parse_or_default<T: DeserializeOwned + Default>(req: &Request) -> Result<T, Response> {
    if req.body.is_empty() {
        Ok(T::default())
    } else {
        parse(req)
    }
}

fn issue_route(app: &App, req: &Request, _p: &Params<'_>) -> Response {
    let body: IssueBody = match parse_or_default(req) {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    match app.nodes.issue_code(app, &body.label.unwrap_or_default(), body.ttl_minutes.unwrap_or(0)) {
        Ok((id, token, expires_ms)) => Response::json(201, &serde_json::json!({"id": id, "token": token, "expires_ms": expires_ms})),
        Err(e) => Response::error(400, &e),
    }
}

fn revoke_code_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    match app.nodes.revoke_code(id) {
        Ok(()) => Response::json(200, &serde_json::json!({"id": id, "revoked": true})),
        Err(e) => Response::error(404, &e),
    }
}

fn revoke_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    match app.nodes.revoke(p.get("id")) {
        Ok(node) => {
            app.bus.publish("node", &node);
            Response::json(200, &view_of(app, node))
        }
        Err(e) => Response::error(404, &e),
    }
}

fn drain_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: DrainBody = match parse_or_default(req) {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    match app.nodes.drain(p.get("id"), body.on.unwrap_or(true)) {
        Ok(node) => {
            app.bus.publish("node", &node);
            Response::json(200, &view_of(app, node))
        }
        Err(e) => Response::error(404, &e),
    }
}

fn join_route(app: &App, req: &Request, _p: &Params<'_>) -> Response {
    let body: JoinBody = match parse(req) {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    match app.nodes.join(&source_of(req), &body.secret, &body.public_key, &body.name, body.machine, apz_plaza::now_ms()) {
        Ok(node) => {
            app.bus.publish("node", &node);
            let hub = app.hub_auth.config.lock().map(|c| c.name.clone()).unwrap_or_default();
            Response::json(
                201,
                &serde_json::json!({"node_id": node.id, "cid_base": node.cid_base, "cid_count": node.cid_count, "hub": hub}),
            )
        }
        Err(e) => Response::error(403, &e),
    }
}


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
    /// The node's vsockd `GET /v1/status`.
    #[serde(default)]
    status: Value,
    /// The node's noded `GET /v1/runs`.
    #[serde(default)]
    runs: Value,
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
    app.nodes.set_report(&node.id, beat.status, beat.runs);
    match app.nodes.set_machine(&node.id, beat.machine) {
        Ok(()) => Response::json(200, &serde_json::json!({"ok": true})),
        Err(e) => Response::error(404, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000_000;
    const TTL_MS: u64 = 15 * 60_000;
    const SECRET: [u8; 16] = [7; 16];
    const OTHER_SECRET: [u8; 16] = [8; 16];
    const KEY: [u8; 32] = [9; 32];

    fn add_code(store: &Store, id: &str, secret: &[u8], expires_ms: u64) {
        store.inner.lock().unwrap().codes.push(Code {
            id: id.into(),
            label: "test".into(),
            secret_sha256: hex(&apz_trust::sha256(secret)),
            created_ms: NOW,
            expires_ms,
            used_by: None,
            revoked: false,
            failures: 0,
        });
    }

    fn redeem(store: &Store, source: &str, secret: &[u8], name: &str, now: u64) -> Result<Node, String> {
        store.join(source, &apz_trust::b64(secret), &apz_trust::b64(&KEY), name, Value::Null, now)
    }

    #[test]
    fn join_rate_limit_is_per_source_and_window() {
        let store = Store::default();
        add_code(&store, "c1", &SECRET, NOW + TTL_MS);
        for _ in 0..JOIN_RATE_PER_MINUTE {
            assert_eq!(redeem(&store, "a", &[1; 16], "box", NOW).unwrap_err(), INVALID_CODE);
        }
        assert_eq!(redeem(&store, "a", &SECRET, "box", NOW).unwrap_err(), RATE_LIMITED);
        assert!(redeem(&store, "b", &SECRET, "box", NOW).is_ok(), "another source is not held back");
    }

    #[test]
    fn join_rate_limit_window_reopens_after_a_minute() {
        let store = Store::default();
        add_code(&store, "c1", &SECRET, NOW + TTL_MS);
        for _ in 0..JOIN_RATE_PER_MINUTE {
            let _ = redeem(&store, "a", &[1; 16], "box", NOW);
        }
        assert_eq!(redeem(&store, "a", &SECRET, "box", NOW + 59_000).unwrap_err(), RATE_LIMITED);
        assert!(redeem(&store, "a", &SECRET, "box", NOW + JOIN_WINDOW_MS + 1).is_ok());
    }

    #[test]
    fn code_is_single_use_and_each_node_gets_its_own_cids() {
        let store = Store::default();
        add_code(&store, "c1", &SECRET, NOW + TTL_MS);
        add_code(&store, "c2", &OTHER_SECRET, NOW + TTL_MS);
        let first = redeem(&store, "a", &SECRET, "box", NOW).unwrap();
        assert_eq!((first.id.as_str(), first.cid_base, first.cid_count), ("n1", REMOTE_CID_BASE, CIDS_PER_NODE));
        assert_eq!(redeem(&store, "b", &SECRET, "box", NOW).unwrap_err(), INVALID_CODE);
        let codes = store.codes(NOW);
        assert_eq!(codes[0].state, "used");
        assert_eq!(codes[0].used_by.as_deref(), Some("n1"));
        let second = redeem(&store, "b", &OTHER_SECRET, "box", NOW).unwrap();
        assert_eq!((second.id.as_str(), second.cid_base), ("n2", REMOTE_CID_BASE + CIDS_PER_NODE));
    }

    #[test]
    fn expired_code_is_refused() {
        let store = Store::default();
        add_code(&store, "c1", &SECRET, NOW + 1_000);
        assert_eq!(store.codes(NOW + 1_000)[0].state, "expired");
        assert_eq!(redeem(&store, "a", &SECRET, "box", NOW + 1_000).unwrap_err(), INVALID_CODE);
        assert!(store.list().is_empty());
    }

    #[test]
    fn revoked_code_is_refused_and_used_code_cannot_be_revoked() {
        let store = Store::default();
        add_code(&store, "c1", &SECRET, NOW + TTL_MS);
        store.revoke_code("c1").unwrap();
        assert_eq!(store.codes(NOW)[0].state, "revoked");
        assert_eq!(redeem(&store, "a", &SECRET, "box", NOW).unwrap_err(), INVALID_CODE);
        assert!(store.revoke_code("c9").is_err());

        add_code(&store, "c2", &OTHER_SECRET, NOW + TTL_MS);
        redeem(&store, "a", &OTHER_SECRET, "box", NOW).unwrap();
        assert!(store.revoke_code("c2").is_err());
    }

    #[test]
    fn failed_tries_against_a_code_burn_it() {
        let store = Store::default();
        add_code(&store, "c1", &SECRET, NOW + TTL_MS);
        for _ in 0..JOIN_TRIES_PER_CODE {
            let err = redeem(&store, "a", &SECRET, "", NOW).unwrap_err();
            assert!(err.contains("name"), "the matched secret reports the specific fault: {err}");
        }
        assert_eq!(redeem(&store, "a", &SECRET, "box", NOW).unwrap_err(), INVALID_CODE);
        assert!(store.list().is_empty());
        assert!(store.inner.lock().unwrap().codes[0].failures >= JOIN_TRIES_PER_CODE);
    }

    #[test]
    fn wrong_length_secret_is_refused_without_a_code_lookup() {
        let store = Store::default();
        add_code(&store, "c1", &SECRET, NOW + TTL_MS);
        assert_eq!(redeem(&store, "a", &[7; 8], "box", NOW).unwrap_err(), INVALID_CODE);
    }

    #[test]
    fn freshness_window_is_sixty_seconds_either_way() {
        let now_s = NOW / 1000;
        assert!(check_freshness(now_s, NOW).is_ok());
        assert!(check_freshness(now_s - MAX_SKEW_SECS, NOW).is_ok());
        assert!(check_freshness(now_s + MAX_SKEW_SECS, NOW).is_ok());
        assert!(check_freshness(now_s - MAX_SKEW_SECS - 1, NOW).is_err());
        assert!(check_freshness(now_s + MAX_SKEW_SECS + 1, NOW).is_err());
    }

    #[test]
    fn nonces_are_single_use_within_their_ttl() {
        let mut nonces = BTreeMap::new();
        let key = ("n1".to_string(), "abc".to_string());
        assert!(record_nonce(&mut nonces, key.clone(), NOW).is_ok());
        assert!(record_nonce(&mut nonces, key.clone(), NOW + 1_000).is_err());
        assert!(record_nonce(&mut nonces, ("n2".to_string(), "abc".to_string()), NOW).is_ok());
        assert!(record_nonce(&mut nonces, key, NOW + NONCE_TTL_SECS * 1000 + 1).is_ok(), "pruned after its TTL");
    }

    fn node(id: &str) -> Node {
        Node {
            id: id.into(),
            name: "box".into(),
            public_key: apz_trust::b64(&KEY),
            cid_base: REMOTE_CID_BASE,
            cid_count: CIDS_PER_NODE,
            joined_ms: NOW,
            last_seen_ms: NOW,
            machine: Value::Null,
            draining: false,
            revoked: false,
            address: String::new(),
        }
    }

    fn signed(node_id: &str, ts_secs: u64, nonce: &str) -> Request {
        Request {
            method: "GET".into(),
            path: "/v1/nodes/n1/commands".into(),
            query: "wait=25".into(),
            headers: vec![
                (H_NODE.into(), node_id.into()),
                (H_TS.into(), ts_secs.to_string()),
                (H_NONCE.into(), nonce.into()),
                (H_SIG.into(), "AAAA".into()),
            ],
            body: Vec::new(),
            peer: Some(std::net::IpAddr::from([203, 0, 113, 7])),
        }
    }

    #[test]
    fn authenticate_refuses_unknown_revoked_and_stale_requests() {
        let store = Store::default();
        store.inner.lock().unwrap().nodes.push(node("n1"));
        let now_s = NOW / 1000;
        assert!(store.authenticate(&signed("n9", now_s, "x"), NOW).unwrap_err().contains("unknown"));
        assert!(store.authenticate(&signed("n1", now_s - 300, "x"), NOW).unwrap_err().contains("window"));
        assert!(store.authenticate(&signed("n1", now_s + 300, "x"), NOW).unwrap_err().contains("window"));
        store.revoke("n1").unwrap();
        assert!(store.authenticate(&signed("n1", now_s, "x"), NOW).unwrap_err().contains("revoked"));
    }

    #[test]
    fn views_count_agents_by_cid_range_and_mark_stale_nodes_offline() {
        let fresh = node("n1");
        let mut stale = node("n2");
        stale.last_seen_ms = NOW - OFFLINE_AFTER_MS;
        assert!(view(fresh.clone(), &[], NOW).online);
        assert!(!view(stale, &[], NOW).online);
        let mut revoked = fresh.clone();
        revoked.revoked = true;
        assert!(!view(revoked, &[], NOW).online);
    }

    /// End to end with real crypto: token → redeem → node. Needs apz-trust's `JoinToken` (T1),
    /// so it is ignored until that lands; run with `--ignored`.
    #[test]
    fn end_to_end_token_redeems_once() {
        let token = apz_trust::token::JoinToken {
            hub_url: "https://hub.test:7700".into(),
            cert_sha256: [3; 32],
            secret: [5; 16],
            expires: NOW / 1000 + 900,
        };
        let decoded = apz_trust::token::JoinToken::decode(&token.encode()).unwrap();
        let store = Store::default();
        store.inner.lock().unwrap().codes.push(Code {
            id: "c1".into(),
            label: "e2e".into(),
            secret_sha256: hex(&token.secret_hash()),
            created_ms: NOW,
            expires_ms: token.expires * 1000,
            used_by: None,
            revoked: false,
            failures: 0,
        });
        let node = redeem(&store, "a", &decoded.secret, "box", NOW).unwrap();
        assert_eq!(node.id, "n1");
        assert_eq!(redeem(&store, "a", &decoded.secret, "box", NOW).unwrap_err(), INVALID_CODE);
    }
}

#[cfg(test)]
mod queue_tests {
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
