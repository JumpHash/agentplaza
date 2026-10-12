//! Postgres: the plaza's state of record. Memory stays the working copy every screen and
//! agent reads; Postgres is where it lives across restarts.
//!
//! Each stateful part of the plaza gives a `snapshot()` (its whole state as JSON) and a
//! `restore(json)`. At start apid restores every component from Postgres; every two seconds a
//! writer thread snapshots each component and upserts the ones that changed into
//! `plaza_state`. Transcripts are a stream, not a snapshot: each record is appended to
//! `transcript` as it happens. With no `APZ_DATABASE_URL` the plaza runs as before (the
//! components' own files only).
//!
//! Schema (created on connect):
//!   plaza_state(component text primary key, data jsonb not null, hash bigint not null,
//!               updated_at timestamptz not null default now())
//!   transcript(cid bigint, seq bigint, work bigint, kind text, at_ms bigint, data jsonb,
//!              primary key (cid, seq))

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::state::App;

/// Every snapshotted component, by name, in restore order (the treasury before the task
/// ledger, whose escrows live in it).
pub const COMPONENTS: &[&str] = &[
    "treasury", "tasks", "agora", "social", "council", "bonds",
    "projects", "toolbox", "harness", "approvals", "library", "deliverables", "connectors",
    "names", "settings", "providers", "spawned", "laws", "society", "work",
];

/// The component's snapshot ("" when it has nothing to save yet).
pub fn snapshot_of(app: &App, component: &str) -> String {
    match component {
        "treasury" => app.plaza.treasury.snapshot(),
        "tasks" => app.plaza.tasks.snapshot(),
        "agora" => app.plaza.agora.snapshot(),
        "social" => app.plaza.social.snapshot(),
        "council" => app.plaza.council.snapshot(),
        "bonds" => app.bonds.snapshot(),
        "projects" => app.projects.snapshot(),
        "toolbox" => app.toolbox.snapshot(),
        "harness" => app.harness.snapshot(),
        "approvals" => app.approvals.snapshot(),
        "library" => app.library.snapshot(),
        "deliverables" => app.deliverables.snapshot(),
        "connectors" => app.connectors.snapshot(),
        "names" => app.names.snapshot(),
        "settings" => app.settings.snapshot(),
        "providers" => app.providers.snapshot(),
        "spawned" => app.spawned.snapshot(),
        "laws" => app.laws.snapshot(),
        "society" => app.society.snapshot(),
        "work" => app.work.snapshot(),
        _ => String::new(),
    }
}

/// Restores one component from its stored snapshot.
pub fn restore_into(app: &App, component: &str, json: &str) -> Result<(), String> {
    match component {
        "treasury" => app.plaza.treasury.restore(json),
        "tasks" => app.plaza.tasks.restore(json),
        "agora" => app.plaza.agora.restore(json),
        "social" => app.plaza.social.restore(json),
        "council" => app.plaza.council.restore(json),
        "bonds" => app.bonds.restore(json),
        "projects" => app.projects.restore(json),
        "toolbox" => app.toolbox.restore(json),
        "harness" => app.harness.restore(json),
        "approvals" => app.approvals.restore(json),
        "library" => app.library.restore(json),
        "deliverables" => app.deliverables.restore(json),
        "connectors" => app.connectors.restore(json),
        "names" => app.names.restore(json),
        "settings" => app.settings.restore(json),
        "providers" => app.providers.restore(json),
        "spawned" => app.spawned.restore(json),
        "laws" => app.laws.restore(json),
        "society" => app.society.restore(json),
        "work" => app.work.restore(json),
        other => Err(format!("unknown component {other}")),
    }
}

/// The connection, and the hash of what each component last stored.
pub struct Db {
    client: Option<Mutex<postgres::Client>>,
    stored: Mutex<BTreeMap<String, i64>>,
}

impl Db {
    /// Connects to `APZ_DATABASE_URL` from `env` and creates the schema; without the
    /// variable (or when the connection fails, logged) the plaza runs without Postgres.
    pub fn connect(_env: &BTreeMap<String, String>) -> Db {
        Db { client: None, stored: Mutex::new(BTreeMap::new()) } // contract: D1
    }

    pub fn enabled(&self) -> bool {
        self.client.is_some()
    }

    /// The stored snapshot of `component`, if any.
    pub fn load(&self, _component: &str) -> Option<String> {
        let _ = &self.stored;
        None // contract: D1
    }

    /// Upserts `json` for `component` unless it is empty or unchanged since the last store.
    pub fn store(&self, _component: &str, _json: &str) -> Result<bool, String> {
        Ok(false) // contract: D1
    }

    /// Appends one transcript record.
    pub fn append_transcript(&self, _cid: u64, _seq: u64, _work: u64, _kind: &str, _at_ms: u64, _data: &Value) {
        // contract: D1
    }

    /// The newest `limit` records of `cid`'s transcript, oldest first, as (seq, work, kind, at_ms, data).
    pub fn transcript(&self, _cid: u64, _limit: i64) -> Vec<(u64, u64, String, u64, Value)> {
        Vec::new() // contract: D1
    }

    /// Every agent cid that has a transcript.
    pub fn transcript_cids(&self) -> Vec<u64> {
        Vec::new() // contract: D1
    }
}

/// At start: restore every component that Postgres holds; a component Postgres does not
/// hold yet keeps what its own files loaded (the first run imports them). Logs each outcome.
pub fn restore_all(app: &App) {
    // contract: D1
    let _ = app;
}

/// The writer: every two seconds, store each component whose snapshot changed.
pub fn spawn_writer(app: Arc<App>) {
    // contract: D1
    let _ = app;
}
