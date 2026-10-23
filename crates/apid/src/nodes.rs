//! The hub's nodes: the machines that run agents for it.
//!
//! A node enters only with a join token the owner issued (`apz_trust::token`), proves itself on
//! every later request with its Ed25519 key (`apz_trust::node`), and is driven through a command
//! queue it long-polls (it dials out, so NAT and firewalls do not matter). Each node owns a
//! disjoint guest-CID range so agent ids stay unique across the plaza. The local node (the one on
//! the hub's own machine, `n0`) needs no join and no queue: apid talks to it over loopback.
//! Saved in plaza.db (the `nodes` tables); join-code secrets are never stored, only their SHA-256.
//!
//! Owner routes (owner auth applies):
//!   GET    /v1/nodes                      → [NodeView]
//!   POST   /v1/nodes/codes   {label?, ttl_minutes?} → {id, token, expires} (the token is shown once)
//!   GET    /v1/nodes/codes                → [CodeView] (no secrets)
//!   DELETE /v1/nodes/codes/{id}           → revokes an unused code
//!   POST   /v1/nodes/{id}/revoke          → the node's signatures stop working at once
//!   POST   /v1/nodes/{id}/stop   {now?}   → out of service: its agents move to the other machines (`mover`)
//!   POST   /v1/nodes/{id}/resume {rebalance?} → back in service, and agents move onto it (`mover`)
//!   POST   /v1/nodes/{id}/drain  {on}     → the older name of stop (on) and resume (off)
//!   POST   /v1/nodes/rebalance   {dry_run?} → spreads the idle agents over the machines (`mover`)
//! Node routes (public, rate-limited / signed):
//!   POST   /v1/nodes/join   {secret, public_key, name, machine} → {node_id, cid_base, cid_count, hub}
//!   GET    /v1/nodes/{id}/commands?wait=25   (signed) → [Command]; also the node's heartbeat
//!   POST   /v1/nodes/{id}/results            (signed) {command, ok, status, body}
//!   POST   /v1/nodes/{id}/heartbeat          (signed) {machine, status, runs, metrics_z?}
//!   POST   /v1/nodes/{id}/metrics            (signed) {z} with a deflated, base64 metrics batch, or a bare batch
//!                                            (`apz_metrics::wire::Batch`)

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use apz_http::mux::Mux;
use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_metrics::wire::Batch;
use apz_metrics::{EntityKind, SeriesKey};
use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use apz_trust::node::{H_NODE, H_NONCE, H_SIG, H_TS, MAX_SKEW_SECS, NONCE_TTL_SECS};
use base64::Engine as _;
use flate2::read::DeflateDecoder;
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
/// A node beats every 15 seconds; a beat that comes later than that is lag (`link.beat_lag_ms`).
const BEAT_MS: u64 = 15_000;

/// The one message every failed redemption gets, so a caller learns nothing about the code.
pub(crate) const INVALID_CODE: &str = "this join code is not valid";
pub(crate) const RATE_LIMITED: &str = "too many attempts; wait a minute";
/// The rate-limit key of a request: its peer address (every caller without one shares a bucket).
pub(crate) fn source_of(req: &Request) -> String {
    req.peer
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "unknown".into())
}
pub(crate) const JOIN_WINDOW_MS: u64 = 60_000;
pub(crate) const DEFAULT_TTL_MINUTES: u64 = 15;

/// A joined node, as the hub keeps it (`node` rows; its machine as one child row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
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
    /// The machine the node last reported: its summary, and on a heartbeat noded's facts with its live stats.
    #[serde(default)]
    #[table(child = "node_machine")]
    pub machine: Machine,
    /// Out of service: the owner stopped it (`mover`). Its agents move off and no agent is placed on it
    /// until it is resumed. The field keeps its first name, from when this was only a drain.
    #[serde(default)]
    pub draining: bool,
    #[serde(default)]
    pub revoked: bool,
    /// Where it last called from (for the owner's eyes only).
    #[serde(default)]
    pub address: String,
}

/// The machine a node reports, typed (`node_machine`). A joining node sends the summary (`cpus`, `memory_mib`, `host`,
/// `version`); a heartbeat sends noded's facts (`apz-link`'s machine report: the fields of noded's `GET /v1/machine`
/// that a heartbeat carries, with its live `stats`). The hub's own machine (`n0`) carries noded's whole view. A field a
/// node does not send is absent, or null where the node sends null; the consoles read this JSON as it is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct Machine {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mib: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Sent by the hub's own machine only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub backend: Option<String>,
    /// The CPU baseline (noded's `cpu_baseline`): the features a snapshot depends on, which a pool shares
    /// (`Pool`). A node with none is in no pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_baseline: Option<String>,
    /// Whether the node's state filesystem clones files (plan 16.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clones: Option<bool>,
    /// The free space on the node's state filesystem at its last heartbeat, in GiB (`storage_gib` is the total).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_free_gib: Option<u64>,
    /// The warm templates (the bases of plan 16.1) the node holds, by key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[table(list)]
    pub bases: Vec<String>,
    /// Sent by the hub's own machine only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub irqchip: Option<String>,
    /// Sent by the hub's own machine only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// A Mac's Apple Silicon details; null elsewhere.
    #[serde(default)]
    #[table(flatten)]
    pub apple: Option<MachineApple>,
    #[serde(default)]
    #[table(flatten)]
    pub cpu: Option<MachineCpu>,
    #[serde(default)]
    #[table(child = "node_machine_gpu")]
    pub gpus: Vec<MachineGpu>,
    #[serde(default)]
    #[table(child = "node_machine_accelerator")]
    pub accelerators: Vec<MachineAccelerator>,
    #[serde(default)]
    #[table(flatten)]
    pub trainer: Option<MachineTrainer>,
    #[serde(default)]
    #[table(list = "node_machine_fabric")]
    pub fabric: Vec<String>,
    #[serde(default)]
    pub storage_gib: Option<u64>,
    /// Sent by the hub's own machine only (a heartbeat does not carry it): the launch backends the node runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[table(list = "node_machine_workstation_backend")]
    pub workstation_backends: Vec<String>,
    /// Sent by the hub's own machine only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vsock_mode: Option<String>,
    /// Sent by the hub's own machine only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    /// The node's live load, from its last heartbeat (`cpu_pct`, `mem_used_mib`); null until it has a sample.
    #[serde(default)]
    #[table(flatten)]
    pub stats: Option<MachineStats>,
}

/// A Mac's Apple Silicon details (noded's `apple`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct MachineApple {
    pub chip: String,
    pub p_cores: Option<u32>,
    pub e_cores: Option<u32>,
    pub gpu_cores: Option<u32>,
    pub neural_cores: u32,
    pub unified_memory_gib: Option<u64>,
}

/// The CPU: the model name, the physical cores and the logical CPUs (noded's `cpu`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct MachineCpu {
    pub model: String,
    pub cores: u32,
    pub threads: u32,
}

/// A GPU the host has (noded's `gpus`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct MachineGpu {
    pub vendor: String,
    pub name: String,
    pub memory_mib: Option<u64>,
}

/// A device that runs trainers (noded's `accelerators`): a GPU, or Apple Silicon's GPU.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct MachineAccelerator {
    pub index: u32,
    pub vendor: String,
    pub name: String,
    pub memory_mib: Option<u64>,
    pub backend: String,
    pub capability: Option<String>,
    pub link: String,
    pub busy: bool,
    pub util_pct: Option<u32>,
    pub mem_used_mib: Option<u64>,
}

/// The trainer runtime the node can run jobs in (noded's `trainer`): `runtime` is null when none applies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct MachineTrainer {
    pub runtime: Option<String>,
    pub ok: bool,
    pub detail: String,
}

/// The live load a heartbeat carries (`stats`): the CPU percentage and the memory in use, each null until sampled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct MachineStats {
    pub cpu_pct: Option<f64>,
    pub mem_used_mib: Option<u64>,
}

/// A pool (plan 17.2): the machines that share an architecture, a hypervisor and a CPU baseline, so a live VM can
/// move between them. A machine that reports no baseline is in no pool.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pool {
    /// `x86_64` or `aarch64`.
    pub arch: String,
    /// The VMM backend: `kvm`, `hvf` or `whp`.
    pub hypervisor: String,
    /// The CPU baseline text noded reports.
    pub cpu_baseline: String,
}

impl Pool {
    /// The pool of a machine with these three reports; None when any is missing.
    pub fn of(
        arch: Option<&str>,
        hypervisor: Option<&str>,
        cpu_baseline: Option<&str>,
    ) -> Option<Pool> {
        Some(Pool {
            arch: arch?.to_string(),
            hypervisor: hypervisor?.to_string(),
            cpu_baseline: cpu_baseline?.to_string(),
        })
    }
}

impl Machine {
    /// The pool this machine is in (see [`Pool`]).
    pub fn pool(&self) -> Option<Pool> {
        Pool::of(
            self.arch.as_deref(),
            self.backend.as_deref(),
            self.cpu_baseline.as_deref(),
        )
    }

    /// Lays noded's facts under this summary: a field the summary sets (`cpus`, `memory_mib`, `host`, `version`)
    /// stays, and every other field comes from the facts.
    fn overlay(&mut self, facts: Machine) {
        let summary = std::mem::take(self);
        let mut merged = facts;
        merged.cpus = summary.cpus.or(merged.cpus);
        merged.memory_mib = summary.memory_mib.or(merged.memory_mib);
        merged.host = summary.host.or(merged.host);
        merged.version = summary.version.or(merged.version);
        *self = merged;
    }
}

/// A node as the console lists it: the record plus "online", its agents' count and its service state.
#[derive(Debug, Clone, Serialize)]
pub struct NodeView {
    #[serde(flatten)]
    pub node: Node,
    pub online: bool,
    /// The agents it runs: moved agents count where they were moved to.
    pub agents: u32,
    /// "active"; "stopping" while a stopped node still runs agents that are not offline; "stopped" once
    /// none is left, when its machine can be turned off.
    pub state: &'static str,
    /// The agents that still have to leave a stopping node.
    pub left: u32,
    /// The ones of them that cannot leave yet, each with the reason (`mover`).
    pub waiting: Vec<Waiting>,
}

/// An agent that waits to leave a stopping node, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Waiting {
    pub cid: u64,
    pub reason: String,
}

/// An agent as a node's view counts it: the node that runs it, and whether it is offline.
#[derive(Debug, Clone)]
pub struct Hosted {
    pub node: String,
    pub offline: bool,
}

/// A join code: its hash of the secret (never the secret), its expiry and what it made. The stations use it too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
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

/// The nodes' state of record: the one `nodes` row, with the joined nodes (`node`), their join codes (`node_code`)
/// and the moved agents (`node_override`) as child tables.
#[derive(Default, apz_store::Table)]
#[table(name = "nodes")]
struct Persisted {
    #[table(child = "node")]
    nodes: Vec<Node>,
    #[table(child = "node_code")]
    codes: Vec<Code>,
    /// Agents moved off their CID range: cid → the node that runs them now (`placement::move_agent`).
    /// Saved with the rest, so a restart keeps every moved agent on its node.
    #[table(map_value = "node_override")]
    overrides: BTreeMap<u64, String>,
    /// `n0` is out of service. The hub's own machine has no record here, so its flag has its own field.
    local_stopped: bool,
}

/// The nodes' tables: `nodes`, with its nodes, join codes and moved agents, and each node's machine as a child row.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Persisted)
}

#[cfg(test)]
impl Store {
    /// A store holding `nodes` and nothing else: what plaza.db would hold at start (for tests in other modules).
    pub fn holding(nodes: Vec<Node>) -> Store {
        let store = Store::default();
        store.inner.lock().unwrap().nodes = nodes;
        store
    }
}

#[derive(Default)]
pub struct Store {
    inner: Mutex<Persisted>,
    /// Per node: queued commands, and results waiting for a caller. Each is paired with its signal.
    queues: Mutex<BTreeMap<String, VecDeque<Command>>>,
    queue_signal: Condvar,
    results: Mutex<BTreeMap<String, CommandResult>>,
    result_signal: Condvar,
    /// Per node with a live tunnel: the connection its streams run on (`tunnel.rs`).
    tunnels: Mutex<BTreeMap<String, Arc<Mux>>>,
    /// The last command sequence number handed out (`k<seq>`).
    seq: AtomicU64,
    /// Nonces seen (node id, nonce) → expiry ms; join attempts per source address.
    nonces: Mutex<BTreeMap<(String, String), u64>>,
    join_attempts: Mutex<BTreeMap<String, Vec<u64>>>,
    /// Per node, from its last heartbeat: its gateway's `/v1/status` and its noded's `/v1/runs`.
    reports: Mutex<BTreeMap<String, (Value, Value)>>,
}

impl Store {
    /// The nodes' rows, as they are now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        let p = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        RowSet::one(def(), &*p)
    }
    /// Replaces the nodes with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored = apz_store::table::load_one::<Persisted>(conn, def())?.unwrap_or_default();
        *self.inner.lock().unwrap_or_else(|e| e.into_inner()) = stored;
        Ok(())
    }
    pub fn list(&self) -> Vec<Node> {
        self.inner
            .lock()
            .map(|p| p.nodes.clone())
            .unwrap_or_default()
    }
    pub fn get(&self, id: &str) -> Option<Node> {
        self.list().into_iter().find(|n| n.id == id)
    }
    /// The node whose CID range holds `cid`; None for the local node's CIDs (and unknown ones).
    pub fn owner_of_cid(&self, cid: u64) -> Option<Node> {
        self.list()
            .into_iter()
            .find(|n| !n.revoked && cid >= n.cid_base && cid < n.cid_base + n.cid_count)
    }
    /// The node a moved agent runs on, when it was moved off its CID range (None: the range decides).
    pub fn override_of(&self, cid: u64) -> Option<String> {
        self.inner.lock().ok()?.overrides.get(&cid).cloned()
    }
    /// Records where agent `cid` runs (None clears the record, back to its range); returns what it was.
    pub fn set_override(&self, cid: u64, node: Option<&str>) -> Option<String> {
        let mut p = self.inner.lock().ok()?;
        match node {
            Some(node) => p.overrides.insert(cid, node.to_string()),
            None => p.overrides.remove(&cid),
        }
    }

    /// Issues a code: returns its id, the token text (shown once) and its expiry.
    pub fn issue_code(
        &self,
        app: &App,
        label: &str,
        ttl_minutes: u64,
    ) -> Result<(String, String, u64), String> {
        let ttl = match ttl_minutes {
            0 => DEFAULT_TTL_MINUTES,
            m => m.min(MAX_CODE_MINUTES),
        };
        let now = apz_plaza::now_ms();
        let public_url = app
            .hub_auth
            .config
            .lock()
            .map_err(|_| "hub config unavailable".to_string())?
            .public_url
            .clone();
        let token = apz_trust::token::JoinToken::issue(
            &public_url,
            app.hub_auth.cert_sha256,
            now / 1000,
            ttl * 60,
        );
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
        let code = p
            .codes
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or("no such code")?;
        if code.used_by.is_some() {
            return Err("that code was already used".into());
        }
        code.revoked = true;
        Ok(())
    }
    /// Redeems a secret for a new node (rate limits, expiry, single use, constant-time compare).
    pub fn join(
        &self,
        source: &str,
        secret_b64: &str,
        public_key: &str,
        name: &str,
        machine: Machine,
        now_ms: u64,
    ) -> Result<Node, String> {
        self.admit(source, now_ms)?;
        let secret = apz_trust::unb64(secret_b64)
            .ok()
            .filter(|s| s.len() == 16)
            .ok_or(INVALID_CODE)?;
        let hash = hex(&apz_trust::sha256(&secret));

        let mut p = self.inner.lock().map_err(|e| e.to_string())?;
        let Persisted { nodes, codes, .. } = &mut *p;
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
        let usable = code.used_by.is_none()
            && !code.revoked
            && now_ms < code.expires_ms
            && code.failures < JOIN_TRIES_PER_CODE;
        if !usable {
            code.failures = code.failures.saturating_add(1);
            return Err(INVALID_CODE.to_string());
        }

        // The secret matched, so these failures are the caller's own tries against this code.
        let key_ok = apz_trust::unb64(public_key)
            .map(|k| k.len() == 32)
            .unwrap_or(false);
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
        let node = p
            .nodes
            .iter_mut()
            .find(|n| n.id == id)
            .ok_or("no such node")?;
        node.revoked = true;
        Ok(node.clone())
    }
    /// Whether node `id` is out of service (stopping or stopped); `n0` included.
    pub fn is_stopped(&self, id: &str) -> bool {
        let Ok(p) = self.inner.lock() else {
            return false;
        };
        if id == crate::fleet::LOCAL {
            return p.local_stopped;
        }
        p.nodes.iter().any(|n| n.id == id && n.draining)
    }
    /// The nodes out of service, `n0` among them when it is.
    pub fn stopped_ids(&self) -> BTreeSet<String> {
        let Ok(p) = self.inner.lock() else {
            return BTreeSet::new();
        };
        p.local_stopped
            .then(|| crate::fleet::LOCAL.to_string())
            .into_iter()
            .chain(p.nodes.iter().filter(|n| n.draining).map(|n| n.id.clone()))
            .collect()
    }
    /// Takes node `id` out of service (`on`) or puts it back; `n0` too. Returns whether it changed. A
    /// revoked node is refused (409), an unknown one is 404.
    pub fn set_stopped(&self, id: &str, on: bool) -> Result<bool, (u16, String)> {
        let mut p = self.inner.lock().map_err(|e| (500, e.to_string()))?;
        if id == crate::fleet::LOCAL {
            return Ok(std::mem::replace(&mut p.local_stopped, on) != on);
        }
        let node = p
            .nodes
            .iter_mut()
            .find(|n| n.id == id)
            .ok_or((404, format!("no node {id}")))?;
        if node.revoked {
            return Err((409, format!("node {id} is revoked")));
        }
        Ok(std::mem::replace(&mut node.draining, on) != on)
    }
    /// Checks a signed node request: known, not revoked, ts within skew, fresh nonce, valid
    /// signature. Records `last_seen_ms` and `address`. Returns the node.
    pub fn authenticate(&self, req: &Request, now_ms: u64) -> Result<Node, String> {
        let id = req.header(H_NODE).ok_or("missing node identity")?;
        let node = self
            .get(id)
            .filter(|n| !n.revoked)
            .ok_or("unknown or revoked node")?;
        let ts: u64 = req
            .header(H_TS)
            .and_then(|t| t.parse().ok())
            .ok_or("missing or bad timestamp")?;
        let nonce = req
            .header(H_NONCE)
            .filter(|n| !n.is_empty())
            .ok_or("missing nonce")?;
        let sig = req.header(H_SIG).ok_or("missing signature")?;
        check_freshness(ts, now_ms)?;
        let path = if req.query.is_empty() {
            req.path.clone()
        } else {
            format!("{}?{}", req.path, req.query)
        };
        if !apz_trust::node::verify(
            &node.public_key,
            &req.method,
            &path,
            ts,
            nonce,
            &req.body,
            sig,
        ) {
            return Err("bad signature".into());
        }
        {
            // After the signature check, so a forged request cannot burn a real node's nonce.
            let mut nonces = self.nonces.lock().map_err(|e| e.to_string())?;
            record_nonce(&mut nonces, (node.id.clone(), nonce.to_string()), now_ms)?;
        }
        let mut p = self.inner.lock().map_err(|e| e.to_string())?;
        let stored = p
            .nodes
            .iter_mut()
            .find(|n| n.id == node.id)
            .ok_or("unknown node")?;
        stored.last_seen_ms = now_ms;
        if let Some(ip) = req.peer {
            stored.address = ip.to_string();
        }
        Ok(stored.clone())
    }

    /// Join attempts per source in the last minute; a refused attempt is not recorded.
    fn admit(&self, source: &str, now_ms: u64) -> Result<(), String> {
        admit_join(&self.join_attempts, source, now_ms)
    }

    /// How many commands are waiting for the node to take them.
    pub fn queued(&self, node: &str) -> usize {
        self.queues
            .lock()
            .map_or(0, |queues| queues.get(node).map_or(0, VecDeque::len))
    }

    pub fn enqueue(
        &self,
        node: &str,
        target: &str,
        method: &str,
        path: &str,
        body: Value,
    ) -> String {
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
            queues
                .entry(node.to_string())
                .or_default()
                .push_back(command);
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
        queues
            .get_mut(node)
            .map(|q| q.drain(..).collect())
            .unwrap_or_default()
    }
    /// Records the tunnel a node just opened. A tunnel it had before is replaced.
    pub fn set_tunnel(&self, node: &str, mux: Arc<Mux>) {
        let mut tunnels = self.tunnels.lock().unwrap_or_else(|e| e.into_inner());
        tunnels.insert(node.to_string(), mux);
    }
    /// Forgets a node's tunnel, when it is still `mux`.
    pub fn clear_tunnel(&self, node: &str, mux: &Arc<Mux>) {
        let mut tunnels = self.tunnels.lock().unwrap_or_else(|e| e.into_inner());
        if tunnels.get(node).is_some_and(|now| Arc::ptr_eq(now, mux)) {
            tunnels.remove(node);
        }
    }
    /// The node's tunnel, while it is up.
    pub fn tunnel(&self, node: &str) -> Option<Arc<Mux>> {
        let tunnels = self.tunnels.lock().unwrap_or_else(|e| e.into_inner());
        tunnels.get(node).filter(|mux| mux.alive()).cloned()
    }
    /// Opens a stream to a fork's port on a node, through the node's tunnel: what is written goes to the fork, and
    /// what the fork sends comes back on the stream. The node's relay and vsockd check that noded serves the port
    /// (6.1); previews (18.1) ride it.
    pub fn open_port(
        &self,
        node: &str,
        cid: u64,
        port: u16,
    ) -> Result<apz_http::mux::Stream, String> {
        let mux = self.tunnel(node).ok_or("the node has no tunnel")?;
        let header = serde_json::json!({"kind": "port", "cid": cid, "port": port}).to_string();
        mux.open(header.as_bytes()).map_err(|err| err.to_string())
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
            results = self
                .result_signal
                .wait_timeout(results, deadline - now)
                .ok()?
                .0;
        }
    }
    /// Records the machine a node reported (its join's summary, or its heartbeat's facts).
    pub fn set_machine(&self, id: &str, machine: Machine) -> Result<(), String> {
        let mut persisted = self.inner.lock().map_err(|e| e.to_string())?;
        let node = persisted
            .nodes
            .iter_mut()
            .find(|n| n.id == id)
            .ok_or_else(|| "unknown node".to_string())?;
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

/// Join attempts per source in the last minute, shared with the workstation codes (stations/): an attempt past
/// [`JOIN_RATE_PER_MINUTE`] is refused, and a refused attempt is not recorded.
pub(crate) fn admit_join(
    attempts: &Mutex<BTreeMap<String, Vec<u64>>>,
    source: &str,
    now_ms: u64,
) -> Result<(), String> {
    let mut attempts = attempts.lock().map_err(|e| e.to_string())?;
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

pub(crate) fn code_state(code: &Code, now_ms: u64) -> &'static str {
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
fn record_nonce(
    nonces: &mut BTreeMap<(String, String), u64>,
    key: (String, String),
    now_ms: u64,
) -> Result<(), String> {
    nonces.retain(|_, expiry| *expiry > now_ms);
    if nonces.contains_key(&key) {
        return Err("nonce already used".into());
    }
    nonces.insert(key, now_ms + NONCE_TTL_SECS * 1000);
    Ok(())
}

/// The next sequence number after the ids that start with `prefix` (`n3` → 4; none → 1).
pub(crate) fn next_seq<'a>(ids: impl Iterator<Item = &'a str>, prefix: char) -> u64 {
    ids.filter_map(|id| id.strip_prefix(prefix)?.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A node's view (pure): `hosted` is every agent with the node that runs it, `waiting` the agents
/// that cannot leave it yet. Out of service, it is stopping while an agent that is not offline is
/// still on it, and stopped once none is.
pub(crate) fn view(node: Node, hosted: &[Hosted], waiting: Vec<Waiting>, now_ms: u64) -> NodeView {
    let online = !node.revoked && now_ms.saturating_sub(node.last_seen_ms) < OFFLINE_AFTER_MS;
    let here = hosted.iter().filter(|agent| agent.node == node.id);
    let count = here.clone().count();
    let live = here.filter(|agent| !agent.offline).count();
    let (state, left, waiting) = match (node.draining, live) {
        (false, _) => ("active", 0, Vec::new()),
        (true, 0) => ("stopped", 0, Vec::new()),
        (true, live) => ("stopping", live, waiting),
    };
    NodeView {
        node,
        online,
        agents: u32::try_from(count).unwrap_or(u32::MAX),
        state,
        left: u32::try_from(left).unwrap_or(u32::MAX),
        waiting,
    }
}

/// Every agent with the node that runs it now (`fleet::node_of`).
pub(crate) fn hosted(app: &App) -> Vec<Hosted> {
    app.agents
        .select(|a| Some((a.cid, a.status == crate::agents::Status::Offline)))
        .into_iter()
        .map(|(cid, offline)| Hosted {
            node: crate::fleet::node_of(app, cid),
            offline,
        })
        .collect()
}

/// One node's view as the console gets it.
pub(crate) fn view_of(app: &App, node: Node) -> NodeView {
    let waiting = crate::mover::waiting(&node.id);
    view(node, &hosted(app), waiting, apz_plaza::now_ms())
}

/// The view of node `id` (`n0` included), None when there is no such node.
pub(crate) fn view_by_id(app: &App, id: &str) -> Option<NodeView> {
    let node = if id == crate::fleet::LOCAL {
        local_record(app, local_up(app))
    } else {
        app.nodes.get(id)?
    };
    Some(view_of(app, node))
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/nodes", |app, _, _| Response::json(200, &views(app)))
        .doc(RouteDoc::new("List the plaza's machines"))
        .post("/v1/nodes/codes", issue_route)
        .doc(
            RouteDoc::new("Issue a join code for a new node")
                .status(201)
                .errors(&[400]),
        )
        .get("/v1/nodes/codes", |app, _, _| {
            Response::json(200, &app.nodes.codes(apz_plaza::now_ms()))
        })
        .doc(RouteDoc::new("List the join codes"))
        .delete("/v1/nodes/codes/{id}", revoke_code_route)
        .doc(RouteDoc::new("Revoke an unused join code").errors(&[404]))
        .post("/v1/nodes/{id}/revoke", revoke_route)
        .doc(RouteDoc::new("Revoke a node's access").errors(&[404]))
        .post("/v1/nodes/join", join_route)
        .doc(
            RouteDoc::new("Join a new node with a join code")
                .status(201)
                .errors(&[400, 403]),
        )
        .get("/v1/nodes/{id}/commands", commands_route)
        .doc(RouteDoc::new("Long-poll a node for its next commands"))
        .get("/v1/nodes/{id}/tunnel", tunnel_route)
        .doc(
            RouteDoc::new(
                "Open a node's tunnel: a WebSocket that carries its streams (signed by the node)",
            )
            .errors(&[401]),
        )
        .post("/v1/nodes/{id}/results", results_route)
        .doc(RouteDoc::new("Report the result of a node's command").errors(&[400]))
        .post("/v1/nodes/{id}/heartbeat", heartbeat_route)
        .doc(RouteDoc::new("Record a node's heartbeat").errors(&[400, 404]))
        .post("/v1/nodes/{id}/metrics", metrics_route)
        .doc(RouteDoc::new("Record a node's metrics batch").errors(&[400]))
}

/// `n0`, the hub's own machine, as a node record: never stored (it is always there), online while
/// its noded answers. Its `machine` is its noded's view (`workstation_backends` among the facts)
/// over the hub's own summary.
pub(crate) fn local_node(app: &App) -> Node {
    let noded = crate::probes::latest(app).machine;
    let local_up = noded.is_some();
    let mut node = local_record(app, local_up);
    if let Some(facts) = noded.and_then(|view| serde_json::from_value::<Machine>(view).ok()) {
        node.machine.overlay(facts);
    }
    node
}

/// Whether `n0`'s noded answers.
fn local_up(app: &App) -> bool {
    crate::probes::latest(app).noded_up()
}

/// `n0`'s record with the hub's own summary as its machine: seen now while its noded answers
/// (`up`), out of service when the owner stopped it.
fn local_record(app: &App, up: bool) -> Node {
    let machine = local_summary();
    Node {
        id: crate::fleet::LOCAL.into(),
        name: machine
            .host
            .clone()
            .unwrap_or_else(|| "this hub".to_string()),
        public_key: String::new(),
        cid_base: 0,
        cid_count: REMOTE_CID_BASE,
        joined_ms: 0,
        last_seen_ms: if up { apz_plaza::now_ms() } else { 0 },
        machine,
        draining: app.nodes.is_stopped(crate::fleet::LOCAL),
        revoked: false,
        address: "loopback".into(),
    }
}

/// The plaza's machines: `n0`, the hub's own (never stored: it is always there, online while
/// its noded answers), then every joined node.
pub(crate) fn views(app: &App) -> Vec<NodeView> {
    let now = apz_plaza::now_ms();
    let hosted = hosted(app);
    std::iter::once(local_record(app, local_up(app)))
        .chain(app.nodes.list())
        .map(|node| {
            let waiting = crate::mover::waiting(&node.id);
            view(node, &hosted, waiting, now)
        })
        .collect()
}

/// This machine's summary, as a joined node reports its own: cpus, memory_mib, host, version.
/// `memory_mib` comes from /proc on Linux; elsewhere it is unknown (absent), which the consoles show as unknown.
pub(crate) fn local_summary() -> Machine {
    let cpus = std::thread::available_parallelism().map_or(0, |n| n.get());
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| {
            std::env::var("HOSTNAME")
                .or_else(|_| std::env::var("COMPUTERNAME"))
                .ok()
        })
        .map(|h| h.trim().to_string())
        .unwrap_or_default();
    Machine {
        cpus: Some(cpus as u64),
        memory_mib: memory_mib(),
        host: Some(host),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        ..Machine::default()
    }
}

#[cfg(target_os = "linux")]
fn memory_mib() -> Option<u64> {
    let kb = std::fs::read_to_string("/proc/meminfo").ok().and_then(|t| {
        t.lines()
            .find(|l| l.starts_with("MemTotal:"))
            .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
    });
    Some(kb.map_or(0, |kb| kb / 1024))
}

#[cfg(not(target_os = "linux"))]
fn memory_mib() -> Option<u64> {
    None
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
    machine: Machine,
}

pub(crate) fn parse<T: DeserializeOwned>(req: &Request) -> Result<T, Response> {
    req.json::<T>()
        .map_err(|e| Response::error(400, &format!("bad body: {e}")))
}

pub(crate) fn parse_or_default<T: DeserializeOwned + Default>(
    req: &Request,
) -> Result<T, Response> {
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
    match app.nodes.issue_code(
        app,
        &body.label.unwrap_or_default(),
        body.ttl_minutes.unwrap_or(0),
    ) {
        Ok((id, token, expires_ms)) => Response::json(
            201,
            &serde_json::json!({"id": id, "token": token, "expires_ms": expires_ms}),
        ),
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

fn join_route(app: &App, req: &Request, _p: &Params<'_>) -> Response {
    let body: JoinBody = match parse(req) {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    match app.nodes.join(
        &source_of(req),
        &body.secret,
        &body.public_key,
        &body.name,
        body.machine,
        apz_plaza::now_ms(),
    ) {
        Ok(node) => {
            app.bus.publish("node", &node);
            let hub = app
                .hub_auth
                .config
                .lock()
                .map(|c| c.name.clone())
                .unwrap_or_default();
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
    let wait = req
        .query_param("wait")
        .and_then(|w| w.parse::<u64>().ok())
        .unwrap_or(25)
        .min(25);
    Response::json(200, &app.nodes.take(&node.id, wait * 1000))
}

/// The node's tunnel (`tunnel.rs`): authenticated by its signature, then the WebSocket upgrade. The node's requests over
/// it are answered by the same routes as HTTP requests.
fn tunnel_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let node = match signed_node(app, req, p) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let Some(router) = app.routes.get().map(Arc::clone) else {
        return Response::error(503, "the hub is starting");
    };
    let weak = app.handle();
    let dispatch: crate::tunnel::Dispatch = Arc::new(move |request: &Request| {
        let Some(app) = weak.upgrade() else {
            return Response::error(503, "the hub is stopping");
        };
        match crate::hub_auth::gate(&app, request) {
            Some(refused) => refused,
            None => router.handle(&app, request),
        }
    });
    crate::tunnel::open(req, node.id, Arc::clone(&app.nodes), dispatch)
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
    machine: Machine,
    /// The node's vsockd `GET /v1/status`.
    #[serde(default)]
    status: Value,
    /// The node's noded `GET /v1/runs`.
    #[serde(default)]
    runs: Value,
    /// The node's noded `GET /v1/metrics/batch`, deflated and base64 encoded; absent when it has none.
    #[serde(default)]
    metrics_z: Option<String>,
    /// The node's noded `GET /v1/metrics/batch` as plain JSON, from nodes that predate `metrics_z`.
    #[serde(default)]
    metrics: Value,
}

/// The most a metrics batch may hold once it is inflated.
const MAX_BATCH_BYTES: usize = 4 * 1024 * 1024;

/// Reads a metrics batch sent deflated and base64 encoded, refusing one that inflates past [`MAX_BATCH_BYTES`].
fn inflate_batch(encoded: &str) -> Result<Batch, String> {
    let deflated = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| e.to_string())?;
    let mut json = Vec::new();
    // One byte past the limit is read, so an oversized batch is seen without inflating all of it.
    DeflateDecoder::new(deflated.as_slice())
        .take(MAX_BATCH_BYTES as u64 + 1)
        .read_to_end(&mut json)
        .map_err(|e| e.to_string())?;
    if json.len() > MAX_BATCH_BYTES {
        return Err("the metrics batch inflates past 4 MB".to_string());
    }
    serde_json::from_slice(&json).map_err(|e| e.to_string())
}

/// A beat takes the node's machine, status and runs, and its metrics batch when it carries one. A
/// metrics body this hub cannot read is dropped; the beat itself still counts.
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
    if let Err(e) = app.nodes.set_machine(&node.id, beat.machine) {
        return Response::error(404, &e);
    }
    let now = apz_plaza::now_ms();
    // Read before the batch is ingested, which moves the node's last beat to now.
    let previous_beat = crate::metrics::lock(&app.metrics.links)
        .get(&node.id)
        .map_or(0, |link| link.last_beat_ms);
    app.metrics
        .registry
        .gauge(SeriesKey::new(
            "link.commands_queued",
            EntityKind::Node,
            &node.id,
        ))
        .set(app.nodes.queued(&node.id) as f64);
    // A beat without a batch carries neither field, or a null `metrics`, and skips this.
    let batch = match beat.metrics_z {
        Some(encoded) => inflate_batch(&encoded).ok(),
        None => serde_json::from_value::<Batch>(beat.metrics).ok(),
    };
    if let Some(batch) = batch {
        if previous_beat > 0 {
            let late = now.saturating_sub(previous_beat).saturating_sub(BEAT_MS);
            app.metrics
                .registry
                .hist(SeriesKey::new(
                    "link.beat_lag_ms",
                    EntityKind::Node,
                    &node.id,
                ))
                .record(late as f64);
        }
        crate::metrics::ingest::batch(&app.metrics, &node.id, batch, now);
    }
    Response::json(200, &serde_json::json!({"ok": true}))
}

/// A signed node's metrics batch, posted on its own: the live batch (every 2 seconds while a scope
/// on the node is watched) or one too big for a beat. It is ingested as a beat's batch is. The body is
/// `{"z": …}` with the deflated, base64 encoded batch, or the bare batch.
fn metrics_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let node = match signed_node(app, req, p) {
        Ok(n) => n,
        Err(resp) => return resp,
    };
    let body: Value = match req.json() {
        Ok(v) => v,
        Err(_) => return Response::error(400, "bad metrics batch"),
    };
    let parsed = match body.get("z").and_then(Value::as_str) {
        Some(encoded) => inflate_batch(encoded),
        None => serde_json::from_value::<Batch>(body).map_err(|e| e.to_string()),
    };
    let batch = match parsed {
        Ok(b) => b,
        Err(_) => return Response::error(400, "bad metrics batch"),
    };
    crate::metrics::ingest::batch(&app.metrics, &node.id, batch, apz_plaza::now_ms());
    Response::json(200, &serde_json::json!({"ok": true}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A body as a node sends it: the JSON deflated, then base64 encoded.
    fn deflated(json: &[u8]) -> String {
        use base64::Engine as _;
        use std::io::Write;
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
        encoder.write_all(json).unwrap();
        base64::engine::general_purpose::STANDARD.encode(encoder.finish().unwrap())
    }

    #[test]
    fn a_deflated_metrics_batch_inflates_to_the_batch_it_was_sent_as() {
        let batch = Batch {
            node: "n1".to_string(),
            sent_ms: 9,
            ..Batch::default()
        };
        let encoded = deflated(&serde_json::to_vec(&batch).unwrap());
        assert_eq!(inflate_batch(&encoded).unwrap(), batch);
        assert!(inflate_batch("not base64!").is_err());
    }

    #[test]
    fn a_batch_that_inflates_past_four_megabytes_is_refused() {
        // Valid JSON, so only the size limit can refuse it.
        let mut json = br#"{"node":"n1","sent_ms":1,"pad":""#.to_vec();
        json.extend(std::iter::repeat_n(b' ', MAX_BATCH_BYTES));
        json.extend_from_slice(br#""}"#);
        let error = inflate_batch(&deflated(&json)).unwrap_err();
        assert!(error.contains("4 MB"), "{error}");
    }

    #[test]
    fn a_heartbeat_deserializes_with_or_without_a_metrics_batch() {
        let bare: Heartbeat = serde_json::from_value(serde_json::json!({
            "machine": {"name": "n1"},
            "status": {},
            "runs": []
        }))
        .unwrap();
        assert!(bare.metrics.is_null());

        let with: Heartbeat = serde_json::from_value(serde_json::json!({
            "machine": {},
            "metrics": {"node": "n1", "sent_ms": 5}
        }))
        .unwrap();
        let batch: Batch = serde_json::from_value(with.metrics).unwrap();
        assert_eq!(batch.sent_ms, 5);
    }

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

    fn redeem(
        store: &Store,
        source: &str,
        secret: &[u8],
        name: &str,
        now: u64,
    ) -> Result<Node, String> {
        store.join(
            source,
            &apz_trust::b64(secret),
            &apz_trust::b64(&KEY),
            name,
            Machine::default(),
            now,
        )
    }

    #[test]
    fn join_rate_limit_holds_per_source_and_reopens_after_the_window() {
        // The join rate limit is per source and per window.
        {
            let store = Store::default();
            add_code(&store, "c1", &SECRET, NOW + TTL_MS);
            for _ in 0..JOIN_RATE_PER_MINUTE {
                assert_eq!(
                    redeem(&store, "a", &[1; 16], "box", NOW).unwrap_err(),
                    INVALID_CODE
                );
            }
            assert_eq!(
                redeem(&store, "a", &SECRET, "box", NOW).unwrap_err(),
                RATE_LIMITED
            );
            assert!(
                redeem(&store, "b", &SECRET, "box", NOW).is_ok(),
                "another source is not held back"
            );
        }
        // The join rate limit window reopens after a minute.
        {
            let store = Store::default();
            add_code(&store, "c1", &SECRET, NOW + TTL_MS);
            for _ in 0..JOIN_RATE_PER_MINUTE {
                let _ = redeem(&store, "a", &[1; 16], "box", NOW);
            }
            assert_eq!(
                redeem(&store, "a", &SECRET, "box", NOW + 59_000).unwrap_err(),
                RATE_LIMITED
            );
            assert!(redeem(&store, "a", &SECRET, "box", NOW + JOIN_WINDOW_MS + 1).is_ok());
        }
    }

    #[test]
    fn code_is_single_use_and_each_node_gets_its_own_cids() {
        let store = Store::default();
        add_code(&store, "c1", &SECRET, NOW + TTL_MS);
        add_code(&store, "c2", &OTHER_SECRET, NOW + TTL_MS);
        let first = redeem(&store, "a", &SECRET, "box", NOW).unwrap();
        assert_eq!(
            (first.id.as_str(), first.cid_base, first.cid_count),
            ("n1", REMOTE_CID_BASE, CIDS_PER_NODE)
        );
        assert_eq!(
            redeem(&store, "b", &SECRET, "box", NOW).unwrap_err(),
            INVALID_CODE
        );
        let codes = store.codes(NOW);
        assert_eq!(codes[0].state, "used");
        assert_eq!(codes[0].used_by.as_deref(), Some("n1"));
        let second = redeem(&store, "b", &OTHER_SECRET, "box", NOW).unwrap();
        assert_eq!(
            (second.id.as_str(), second.cid_base),
            ("n2", REMOTE_CID_BASE + CIDS_PER_NODE)
        );
    }

    #[test]
    fn expired_and_revoked_codes_are_refused() {
        // An expired code is refused.
        {
            let store = Store::default();
            add_code(&store, "c1", &SECRET, NOW + 1_000);
            assert_eq!(store.codes(NOW + 1_000)[0].state, "expired");
            assert_eq!(
                redeem(&store, "a", &SECRET, "box", NOW + 1_000).unwrap_err(),
                INVALID_CODE
            );
            assert!(store.list().is_empty());
        }
        // A revoked code is refused, and a used code cannot be revoked.
        {
            let store = Store::default();
            add_code(&store, "c1", &SECRET, NOW + TTL_MS);
            store.revoke_code("c1").unwrap();
            assert_eq!(store.codes(NOW)[0].state, "revoked");
            assert_eq!(
                redeem(&store, "a", &SECRET, "box", NOW).unwrap_err(),
                INVALID_CODE
            );
            assert!(store.revoke_code("c9").is_err());

            add_code(&store, "c2", &OTHER_SECRET, NOW + TTL_MS);
            redeem(&store, "a", &OTHER_SECRET, "box", NOW).unwrap();
            assert!(store.revoke_code("c2").is_err());
        }
    }

    #[test]
    fn failed_tries_against_a_code_burn_it() {
        let store = Store::default();
        add_code(&store, "c1", &SECRET, NOW + TTL_MS);
        for _ in 0..JOIN_TRIES_PER_CODE {
            let err = redeem(&store, "a", &SECRET, "", NOW).unwrap_err();
            assert!(
                err.contains("name"),
                "the matched secret reports the specific fault: {err}"
            );
        }
        assert_eq!(
            redeem(&store, "a", &SECRET, "box", NOW).unwrap_err(),
            INVALID_CODE
        );
        assert!(store.list().is_empty());
        assert!(store.inner.lock().unwrap().codes[0].failures >= JOIN_TRIES_PER_CODE);
    }

    #[test]
    fn wrong_length_secret_is_refused_without_a_code_lookup() {
        let store = Store::default();
        add_code(&store, "c1", &SECRET, NOW + TTL_MS);
        assert_eq!(
            redeem(&store, "a", &[7; 8], "box", NOW).unwrap_err(),
            INVALID_CODE
        );
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
        assert!(
            record_nonce(&mut nonces, key, NOW + NONCE_TTL_SECS * 1000 + 1).is_ok(),
            "pruned after its TTL"
        );
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
            machine: Machine::default(),
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
        assert!(
            store
                .authenticate(&signed("n9", now_s, "x"), NOW)
                .unwrap_err()
                .contains("unknown")
        );
        assert!(
            store
                .authenticate(&signed("n1", now_s - 300, "x"), NOW)
                .unwrap_err()
                .contains("window")
        );
        assert!(
            store
                .authenticate(&signed("n1", now_s + 300, "x"), NOW)
                .unwrap_err()
                .contains("window")
        );
        store.revoke("n1").unwrap();
        assert!(
            store
                .authenticate(&signed("n1", now_s, "x"), NOW)
                .unwrap_err()
                .contains("revoked")
        );
    }

    fn hosted_on(_cid: u64, node: &str, offline: bool) -> Hosted {
        Hosted {
            node: node.into(),
            offline,
        }
    }

    #[test]
    fn views_mark_stale_nodes_offline() {
        let fresh = node("n1");
        let mut stale = node("n2");
        stale.last_seen_ms = NOW - OFFLINE_AFTER_MS;
        assert!(view(fresh.clone(), &[], Vec::new(), NOW).online);
        assert!(!view(stale, &[], Vec::new(), NOW).online);
        let mut revoked = fresh.clone();
        revoked.revoked = true;
        assert!(!view(revoked, &[], Vec::new(), NOW).online);
    }

    #[test]
    fn a_view_counts_the_agents_a_node_runs_now_moved_ones_included() {
        // 1000 is in n1's range but was moved to n2; 1001 is n1's own; 5 was moved onto n1.
        let hosted = [
            hosted_on(1000, "n2", false),
            hosted_on(1001, "n1", false),
            hosted_on(5, "n1", true),
        ];
        assert_eq!(view(node("n1"), &hosted, Vec::new(), NOW).agents, 2);
        let mut n2 = node("n2");
        n2.cid_base = REMOTE_CID_BASE + CIDS_PER_NODE;
        assert_eq!(view(n2, &hosted, Vec::new(), NOW).agents, 1);
    }

    #[test]
    fn a_stopped_node_is_stopping_until_no_live_agent_is_left_on_it() {
        let waiting = vec![Waiting {
            cid: 1001,
            reason: "no machine in service has room for it".into(),
        }];
        let active = view(
            node("n1"),
            &[hosted_on(1001, "n1", false)],
            waiting.clone(),
            NOW,
        );
        assert_eq!(
            (active.state, active.left, active.waiting.len()),
            ("active", 0, 0),
            "a node in service waits for nothing"
        );
        let mut out = node("n1");
        out.draining = true;
        let stopping = view(
            out.clone(),
            &[hosted_on(1001, "n1", false), hosted_on(1002, "n1", true)],
            waiting.clone(),
            NOW,
        );
        assert_eq!((stopping.state, stopping.left), ("stopping", 1));
        assert_eq!(stopping.waiting, waiting);
        assert_eq!(stopping.agents, 2);
        let stopped = view(out, &[hosted_on(1002, "n1", true)], waiting, NOW);
        assert_eq!(
            (stopped.state, stopped.left, stopped.waiting.len()),
            ("stopped", 0, 0),
            "an offline agent does not keep its node stopping"
        );
    }

    #[test]
    fn stopping_holds_for_joined_nodes_and_n0_and_is_refused_for_a_revoked_one() {
        let store = Store::default();
        let mut revoked = node("n2");
        revoked.revoked = true;
        *store.inner.lock().unwrap() = Persisted {
            nodes: vec![node("n1"), revoked],
            ..Persisted::default()
        };
        assert!(store.stopped_ids().is_empty());
        assert_eq!(store.set_stopped("n1", true), Ok(true));
        assert_eq!(store.set_stopped("n1", true), Ok(false), "no change");
        assert_eq!(store.set_stopped("n0", true), Ok(true));
        assert!(store.is_stopped("n1") && store.is_stopped("n0"));
        assert_eq!(
            store.stopped_ids().into_iter().collect::<Vec<_>>(),
            ["n0", "n1"]
        );
        assert_eq!(store.set_stopped("n2", true).map_err(|e| e.0), Err(409));
        assert_eq!(store.set_stopped("n9", true).map_err(|e| e.0), Err(404));
        // n0's flag is saved with the rest: it comes back from the tables as it was, and an empty store reads as
        // in service.
        let again = loaded(&store);
        assert!(again.is_stopped("n0"));
        assert_eq!(again.set_stopped("n0", false), Ok(true));
        assert!(!again.is_stopped("n0"));
        assert!(!loaded(&Store::default()).is_stopped("n0"));
    }

    /// The store's rows saved to their tables and loaded into a fresh store.
    fn loaded(store: &Store) -> Store {
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &store.rows().unwrap()).unwrap();
        let again = Store::default();
        again.load(&conn).unwrap();
        again
    }

    #[test]
    fn the_hub_reports_its_memory_in_mib_or_as_unknown_never_as_text() {
        let memory = local_summary().memory_mib;
        assert_eq!(memory.is_some(), cfg!(target_os = "linux"), "{memory:?}");
    }

    /// A node's machine as the heartbeat of noded sends it, and the same node's join summary, both come back from the
    /// tables whole (the nested facts with their nulls, the accelerators, the stats).
    #[test]
    fn a_nodes_machine_comes_back_from_its_tables_as_it_was_reported() {
        let store = Store::default();
        let input = serde_json::json!({
            "cpus": 8, "memory_mib": 16384, "host": "box", "version": "0.1.0",
            "kind": "server", "vendor": null, "model": null, "apple": null,
            "os": "linux", "arch": "x86_64", "backend": "cuda",
            "cpu": {"model": "EPYC", "cores": 4, "threads": 8},
            "gpus": [{"vendor": "nvidia", "name": "A10", "memory_mib": null}],
            "accelerators": [{"index": 0, "vendor": "nvidia", "name": "A10", "memory_mib": 24576,
                "backend": "cuda", "capability": "8.6", "link": "pcie", "busy": true,
                "util_pct": 40, "mem_used_mib": null}],
            "trainer": {"runtime": "cuda", "ok": true, "detail": ""},
            "fabric": ["10.0.0.5"], "storage_gib": null,
            "stats": {"cpu_pct": 7.5, "mem_used_mib": 1800}
        });
        let heartbeat: Machine = serde_json::from_value(input.clone()).unwrap();
        *store.inner.lock().unwrap() = Persisted {
            nodes: vec![Node {
                machine: heartbeat.clone(),
                ..node("n1")
            }],
            ..Persisted::default()
        };
        let again = loaded(&store);
        assert_eq!(again.get("n1").unwrap().machine, heartbeat);
        assert_eq!(
            serde_json::to_value(&again.get("n1").unwrap().machine).unwrap(),
            input,
            "the JSON the consoles read is the JSON the node sent"
        );
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
        assert_eq!(
            redeem(&store, "a", &decoded.secret, "box", NOW).unwrap_err(),
            INVALID_CODE
        );
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
                CommandResult {
                    command: "k9".into(),
                    ok: true,
                    status: 200,
                    body: Value::Null,
                },
            );
        });
        let started = Instant::now();
        let got = store.wait_result("k9", 5_000);
        handle.join().unwrap();
        assert_eq!(got.map(|r| r.status), Some(200));
        // The wait returns on the writer's wake-up, well inside its 5 s limit. Not asserted under an emulator.
        match apz_os::runner::emulated() {
            Some(runner) => println!(
                "timing not asserted under an emulator: {runner} (woke after {:?})",
                started.elapsed()
            ),
            None => assert!(started.elapsed() < Duration::from_millis(4_000)),
        }
        assert!(
            store.wait_result("k9", 10).is_none(),
            "a result is removed once taken"
        );
    }

    #[test]
    fn set_machine_stores_for_known_node_only() {
        let store = Store::default();
        assert!(store.set_machine("n9", Machine::default()).is_err());
    }
}
