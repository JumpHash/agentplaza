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
//! Schema (the numbered migrations in `sql/migrations`, applied on connect; see `migrate`).
//! Every SQL statement is a file under `sql/queries`, embedded as a const in `sql` (docs/dev/sql.md).
//! Tables:
//!   plaza_state(component text primary key, data jsonb not null, hash bigint not null,
//!               updated_at timestamptz not null default now())
//!   transcript(cid bigint, seq bigint, work bigint, kind text, at_ms bigint, data jsonb,
//!              primary key (cid, seq))
//!   infraction(id bigserial, cid bigint, law text, at_ms bigint, tool text, detail text)
//!   ide_audit(seq bigserial, at_ms bigint, workspace text, kind text, data jsonb)
//!              (append-only: the owner's IDE actions, migration 0008)
//!   station_audit(seq bigserial, at_ms bigint, station text, agent bigint, kind text, data jsonb)
//!              (append-only: the owner's workstation actions and lease changes, migration 0009)

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::state::App;

/// Every snapshotted component, by name, in restore order (the treasury before the task
/// ledger, whose escrows live in it).
pub const COMPONENTS: &[&str] = &[
    "treasury", "tasks", "agora", "social", "council", "bonds",
    "projects", "toolbox", "harness", "approvals", "library", "deliverables", "connectors",
    "names", "settings", "providers", "spawned", "laws", "rules", "society", "work",
    "control", "custom_roles", "agent_proposals", "nodes", "stations", "prices", "usage", "looks", "foundry",
    "models", "sanctions", "vault", "mcp",
];

pub mod files;
pub mod data_migrate;
pub mod hub_mcp;
pub mod migrate;
pub mod roster;
pub mod sql;

const MINUTE: Duration = Duration::from_secs(60);

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
        "rules" => app.laws.rules_snapshot(),
        "society" => app.society.snapshot(),
        "work" => app.work.snapshot(),
        "control" => app.control.snapshot(),
        "custom_roles" => app.custom_roles.snapshot(),
        "agent_proposals" => app.agent_proposals.snapshot(),
        "nodes" => app.nodes.snapshot(),
        "stations" => app.stations.snapshot(),
        "prices" => app.prices.snapshot(),
        "usage" => app.usage.snapshot(),
        "looks" => app.looks.snapshot(),
        "foundry" => app.foundry.snapshot(),
        "models" => app.models.snapshot(),
        "sanctions" => crate::sanctions::snapshot(app),
        "vault" => app.vault.snapshot(),
        "mcp" => app.mcp.store.snapshot(),
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
        "rules" => app.laws.rules_restore(json),
        "society" => app.society.restore(json),
        "work" => app.work.restore(json),
        "control" => app.control.restore(json),
        "custom_roles" => app.custom_roles.restore(json),
        "agent_proposals" => app.agent_proposals.restore(json),
        "nodes" => app.nodes.restore(json),
        "stations" => app.stations.restore(json),
        "prices" => app.prices.restore(json),
        "usage" => app.usage.restore(json),
        "looks" => app.looks.restore(json),
        "foundry" => app.foundry.restore(json),
        "models" => app.models.restore(json),
        "sanctions" => crate::sanctions::restore(app, json),
        "vault" => app.vault.restore(json),
        "mcp" => app.mcp.store.restore(json),
        other => Err(format!("unknown component {other}")),
    }
}

/// FNV-1a, 64-bit, as i64: stable across runs (the std hasher is not).
fn fnv1a(bytes: &[u8]) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash as i64
}

/// Opens a connection (no schema work: the migrations run once, on connect).
fn connect_raw(url: &str) -> Result<postgres::Client, postgres::Error> {
    postgres::Client::connect(url, postgres::NoTls)
}

/// Whether a log line last written at `last` may be written again now.
fn due(last: Option<Instant>) -> bool {
    last.map_or(true, |t| t.elapsed() >= MINUTE)
}

/// The connection, and the hash of what each component last stored.
pub struct Db {
    /// The URL to reconnect with (empty when disabled).
    url: String,
    client: Option<Mutex<postgres::Client>>,
    stored: Mutex<BTreeMap<String, i64>>,
    /// Components whose stored state must not be overwritten this run: a failed load, or a
    /// restore that was refused.
    held: Mutex<BTreeSet<String>>,
    append_logged: Mutex<Option<Instant>>,
    infraction_logged: Mutex<Option<Instant>>,
    ide_audit_logged: Mutex<Option<Instant>>,
    station_audit_logged: Mutex<Option<Instant>>,
    /// The file fallback for transcripts, infractions and the IDE audit (see `attach_files`).
    files: std::sync::OnceLock<files::Files>,
}

impl Db {
    fn with(url: String, client: Option<postgres::Client>) -> Db {
        Db {
            url,
            client: client.map(Mutex::new),
            stored: Mutex::new(BTreeMap::new()),
            held: Mutex::new(BTreeSet::new()),
            append_logged: Mutex::new(None),
            infraction_logged: Mutex::new(None),
            ide_audit_logged: Mutex::new(None),
            station_audit_logged: Mutex::new(None),
            files: std::sync::OnceLock::new(),
        }
    }

    /// Connects to `APZ_DATABASE_URL` from `env` and migrates the schema to this build's. Without
    /// the variable (or when the connection fails, logged) the plaza runs without Postgres. A
    /// migration that refuses (a changed applied file, a failed statement) stops apid at start.
    pub fn connect(env: &BTreeMap<String, String>) -> Db {
        let url = match env.get("APZ_DATABASE_URL") {
            Some(url) if !url.trim().is_empty() => url.clone(),
            _ => return Db::with(String::new(), None),
        };
        match connect_raw(&url) {
            Ok(mut client) => {
                if let Err(e) = migrate::migrate(&mut client) {
                    eprintln!("db: refusing to start: {e}");
                    std::process::exit(1);
                }
                if let Err(e) = data_migrate::run(&mut client) {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                Db::with(url, Some(client))
            }
            Err(e) => {
                eprintln!("db: connect failed: {e}");
                Db::with(String::new(), None)
            }
        }
    }

    /// Deletes the stored transcript rows of `cid` (a wipe).
    pub fn delete_transcript(&self, cid: u64) {
        if let Some(files) = self.file_store() {
            if let Err(err) = files.delete_transcript(cid) {
                eprintln!("apid: delete transcript {cid}: {err}");
            }
            return;
        }
        if !self.enabled() {
            return;
        }
        let cid = cid as i64;
        if let Err(err) = self.run(|c| c.execute(sql::TRANSCRIPT_DELETE, &[&cid])) {
            eprintln!("apid: delete transcript {cid}: {err}");
        }
    }

    pub fn enabled(&self) -> bool {
        self.client.is_some()
    }

    /// Makes `state_dir` the home of the file fallback: with no Postgres this run, transcripts,
    /// infractions and the IDE audit are written to `transcripts/`, `infractions.jsonl` and
    /// `ide-audit.jsonl` there (see `db::files`).
    pub fn attach_files(&self, state_dir: &std::path::Path) {
        let _ = self.files.set(files::Files::open(state_dir));
    }

    /// The file fallback, when this run has no Postgres and `attach_files` was called.
    fn file_store(&self) -> Option<&files::Files> {
        if self.enabled() {
            None
        } else {
            self.files.get()
        }
    }

    /// Fsyncs the file fallback's writes since the last call (the periodic writer runs it).
    pub fn sync_files(&self) {
        if let Some(files) = self.file_store() {
            if let Err(e) = files.sync() {
                eprintln!("db: sync files failed: {e}");
            }
        }
    }

    /// A database with no Postgres whose transcripts, infractions and IDE audit live under `state_dir`.
    #[cfg(test)]
    pub fn files_only(state_dir: &std::path::Path) -> Db {
        let db = Db::with(String::new(), None);
        db.attach_files(state_dir);
        db
    }

    /// Runs `op` on the connection; on a closed connection, reconnects once and retries.
    pub(crate) fn run<T>(&self, mut op: impl FnMut(&mut postgres::Client) -> Result<T, postgres::Error>) -> Result<T, String> {
        let slot = self.client.as_ref().ok_or_else(|| "no database".to_string())?;
        let mut client = slot.lock().unwrap_or_else(PoisonError::into_inner);
        match op(&mut *client) {
            Ok(value) => Ok(value),
            Err(e) if e.is_closed() => {
                *client = connect_raw(&self.url).map_err(|e| format!("reconnect failed: {e}"))?;
                op(&mut *client).map_err(|e| e.to_string())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn hold(&self, component: &str) {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).insert(component.to_string());
    }

    pub fn is_held(&self, component: &str) -> bool {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).contains(component)
    }

    /// The stored snapshot of `component`, if any. A failed read holds the component out of
    /// writes, so a restart never overwrites state it could not read.
    pub fn load(&self, component: &str) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        match self.run(|c| c.query_opt(sql::STATE_GET, &[&component])) {
            Ok(Some(row)) => {
                let text: String = row.get(0);
                let hash = fnv1a(text.as_bytes());
                self.stored.lock().unwrap_or_else(PoisonError::into_inner).insert(component.to_string(), hash);
                Some(text)
            }
            Ok(None) => None,
            Err(e) => {
                eprintln!("db: load {component} failed: {e}; keeping its files and not writing it");
                self.hold(component);
                None
            }
        }
    }

    /// Upserts `json` for `component` unless it is empty or unchanged since the last store.
    pub fn store(&self, component: &str, json: &str) -> Result<bool, String> {
        if json.is_empty() || !self.enabled() {
            return Ok(false);
        }
        let hash = fnv1a(json.as_bytes());
        let mut stored = self.stored.lock().unwrap_or_else(PoisonError::into_inner);
        if stored.get(component) == Some(&hash) {
            return Ok(false);
        }
        self.run(|c| c.execute(sql::STATE_UPSERT, &[&component, &json, &hash]))?;
        stored.insert(component.to_string(), hash);
        Ok(true)
    }

    /// Appends one transcript record. A failed append is dropped; the failure is logged at
    /// most once a minute.
    pub fn append_transcript(&self, cid: u64, seq: u64, work: u64, kind: &str, at_ms: u64, data: &Value) {
        if let Some(files) = self.file_store() {
            if let Err(e) = files.append_transcript(cid, seq, work, kind, at_ms, data) {
                let mut logged = self.append_logged.lock().unwrap_or_else(PoisonError::into_inner);
                if due(*logged) {
                    *logged = Some(Instant::now());
                    eprintln!("db: transcript append to files failed: {e} (dropped; logged once a minute)");
                }
            }
            return;
        }
        if !self.enabled() {
            return;
        }
        let (cid, seq, work, at_ms) = (cid as i64, seq as i64, work as i64, at_ms as i64);
        if let Err(e) = self.run(|c| c.execute(sql::TRANSCRIPT_APPEND, &[&cid, &seq, &work, &kind, &at_ms, data])) {
            let mut logged = self.append_logged.lock().unwrap_or_else(PoisonError::into_inner);
            if due(*logged) {
                *logged = Some(Instant::now());
                eprintln!("db: transcript append failed: {e} (dropped; logged once a minute)");
            }
        }
    }

    /// The newest `limit` records of `cid`'s transcript, oldest first, as (seq, work, kind, at_ms, data).
    pub fn transcript(&self, cid: u64, limit: i64) -> Vec<(u64, u64, String, u64, Value)> {
        if let Some(files) = self.file_store() {
            return files.transcript(cid, limit).unwrap_or_else(|e| {
                eprintln!("db: transcript read from files failed: {e}");
                Vec::new()
            });
        }
        if !self.enabled() {
            return Vec::new();
        }
        let cid = cid as i64;
        let rows = self.run(|c| {
            c.query(sql::TRANSCRIPT_PAGE, &[&cid, &limit])
        });
        match rows {
            Ok(rows) => rows
                .iter()
                .rev()
                .map(|r| {
                    (
                        r.get::<_, i64>(0) as u64,
                        r.get::<_, i64>(1) as u64,
                        r.get::<_, String>(2),
                        r.get::<_, i64>(3) as u64,
                        r.get::<_, Value>(4),
                    )
                })
                .collect(),
            Err(e) => {
                eprintln!("db: transcript read failed: {e}");
                Vec::new()
            }
        }
    }

    /// Every agent cid that has a transcript.
    pub fn transcript_cids(&self) -> Vec<u64> {
        if let Some(files) = self.file_store() {
            return files.transcript_cids().unwrap_or_else(|e| {
                eprintln!("db: transcript cids read from files failed: {e}");
                Vec::new()
            });
        }
        if !self.enabled() {
            return Vec::new();
        }
        match self.run(|c| c.query(sql::TRANSCRIPT_CIDS, &[])) {
            Ok(rows) => rows.iter().map(|r| r.get::<_, i64>(0) as u64).collect(),
            Err(e) => {
                eprintln!("db: transcript cids read failed: {e}");
                Vec::new()
            }
        }
    }
}

/// One infraction (a law refused an agent's act), as stored and as the console shows it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Infraction {
    pub cid: u64,
    pub law: String,
    pub at_ms: u64,
    pub tool: String,
    pub detail: String,
}

impl Db {
    /// Appends one infraction (table `infraction`, created with the schema). A failed append is
    /// dropped; the failure is logged at most once a minute.
    pub fn append_infraction(&self, infraction: &Infraction) {
        if let Some(files) = self.file_store() {
            if let Err(e) = files.append_infraction(infraction) {
                let mut logged = self.infraction_logged.lock().unwrap_or_else(PoisonError::into_inner);
                if due(*logged) {
                    *logged = Some(Instant::now());
                    eprintln!("db: infraction append to files failed: {e} (dropped; logged once a minute)");
                }
            }
            return;
        }
        if !self.enabled() {
            return;
        }
        let (cid, at_ms) = (infraction.cid as i64, infraction.at_ms as i64);
        let law = &infraction.law;
        let tool = &infraction.tool;
        let detail = &infraction.detail;
        if let Err(e) = self.run(|c| c.execute(sql::INFRACTION_APPEND, &[&cid, law, &at_ms, tool, detail])) {
            let mut logged = self.infraction_logged.lock().unwrap_or_else(PoisonError::into_inner);
            if due(*logged) {
                *logged = Some(Instant::now());
                eprintln!("db: infraction append failed: {e} (dropped; logged once a minute)");
            }
        }
    }

    /// Infractions, newest first: one agent's (`Some(cid)`) or the plaza's.
    pub fn infractions(&self, cid: Option<u64>, limit: i64) -> Vec<Infraction> {
        if let Some(files) = self.file_store() {
            return files.infractions(cid, limit).unwrap_or_else(|e| {
                eprintln!("db: infraction read from files failed: {e}");
                Vec::new()
            });
        }
        if !self.enabled() {
            return Vec::new();
        }
        let rows = match cid {
            Some(cid) => {
                let cid = cid as i64;
                self.run(|c| c.query(sql::INFRACTION_PAGE_BY_CID, &[&cid, &limit]))
            }
            None => self.run(|c| c.query(sql::INFRACTION_PAGE, &[&limit])),
        };
        match rows {
            Ok(rows) => rows
                .iter()
                .map(|r| Infraction {
                    cid: r.get::<_, i64>(0) as u64,
                    law: r.get(1),
                    at_ms: r.get::<_, i64>(2) as u64,
                    tool: r.get(3),
                    detail: r.get(4),
                })
                .collect(),
            Err(e) => {
                eprintln!("db: infraction read failed: {e}");
                Vec::new()
            }
        }
    }

    /// Whether Postgres holds no infraction at all (the one-time import of the files' violations).
    fn no_infractions(&self) -> bool {
        self.infractions(None, 1).is_empty()
    }
}

/// The IDE audit: one owner action on an agent workspace (`ide.rs` builds the row). The row is the table's
/// data and its `at_ms`, `workspace` and `kind` are its columns. Unlike the infractions, a failed append is
/// an error: the owner's action has happened, so the route answers with it (see `append_ide_audit`).
impl Db {
    /// Appends one owner action to the IDE audit (table `ide_audit`, or the file fallback). A failed
    /// append is returned to the caller and logged at most once a minute.
    pub fn append_ide_audit(&self, row: &Value) -> Result<(), String> {
        let written = self.write_ide_audit(row);
        if let Err(e) = &written {
            let mut logged = self.ide_audit_logged.lock().unwrap_or_else(PoisonError::into_inner);
            if due(*logged) {
                *logged = Some(Instant::now());
                eprintln!("db: ide audit append failed: {e} (answered to the owner; logged once a minute)");
            }
        }
        written
    }

    fn write_ide_audit(&self, row: &Value) -> Result<(), String> {
        let at_ms = row["at_ms"].as_u64().ok_or("the audit row has no at_ms")? as i64;
        let workspace = row["workspace"].as_str().ok_or("the audit row has no workspace")?;
        let kind = row["kind"].as_str().ok_or("the audit row has no kind")?;
        if let Some(files) = self.file_store() {
            return files.append_ide_audit(row);
        }
        if !self.enabled() {
            return Err("no database and no state directory for the IDE audit".to_string());
        }
        self.run(|c| c.execute(sql::IDE_AUDIT_APPEND, &[&at_ms, &workspace, &kind, row])).map(|_| ())
    }

    /// IDE audit rows, newest first (by `at_ms`, then the later row first), of one workspace when
    /// `workspace` is given. A failed read is an error.
    pub fn ide_audit(&self, workspace: Option<&str>, limit: i64) -> Result<Vec<Value>, String> {
        if let Some(files) = self.file_store() {
            return files.ide_audit(workspace, limit);
        }
        if !self.enabled() {
            return Err("no database and no state directory for the IDE audit".to_string());
        }
        let rows = match workspace {
            Some(workspace) => self.run(|c| c.query(sql::IDE_AUDIT_PAGE_BY_WORKSPACE, &[&workspace, &limit])),
            None => self.run(|c| c.query(sql::IDE_AUDIT_PAGE, &[&limit])),
        };
        rows.map(|rows| rows.iter().map(|r| r.get::<_, Value>(0)).collect()).inspect_err(|e| {
            eprintln!("db: ide audit read failed: {e}");
        })
    }
}

/// The station audit (stations/): one owner action on a workstation, or one lease change. Its row is the
/// table's data; `at_ms`, `kind` are required, `station` (null for the registry as a whole) and `agent` (a
/// CID, or null) are its columns. Like the IDE audit, a failed append is an error the caller answers with.
impl Db {
    /// Appends one station audit row (table `station_audit`, or the file fallback).
    pub fn append_station_audit(&self, row: &Value) -> Result<(), String> {
        let written = self.write_station_audit(row);
        if let Err(e) = &written {
            let mut logged = self.station_audit_logged.lock().unwrap_or_else(PoisonError::into_inner);
            if due(*logged) {
                *logged = Some(Instant::now());
                eprintln!("db: station audit append failed: {e} (answered to the owner; logged once a minute)");
            }
        }
        written
    }

    fn write_station_audit(&self, row: &Value) -> Result<(), String> {
        let at_ms = row["at_ms"].as_u64().ok_or("the audit row has no at_ms")? as i64;
        let kind = row["kind"].as_str().ok_or("the audit row has no kind")?;
        if let Some(files) = self.file_store() {
            return files.append_station_audit(row);
        }
        if !self.enabled() {
            return Err("no database and no state directory for the station audit".to_string());
        }
        let station = row["station"].as_str();
        let agent = row["agent"].as_u64().map(|cid| cid as i64);
        self.run(|c| c.execute(sql::STATION_AUDIT_APPEND, &[&at_ms, &station, &agent, &kind, row])).map(|_| ())
    }

    /// Station audit rows, newest first, of one workstation when `station` is given. A failed read is an error.
    pub fn station_audit(&self, station: Option<&str>, limit: i64) -> Result<Vec<Value>, String> {
        if let Some(files) = self.file_store() {
            return files.station_audit(station, limit);
        }
        if !self.enabled() {
            return Err("no database and no state directory for the station audit".to_string());
        }
        let rows = match station {
            Some(station) => self.run(|c| c.query(sql::STATION_AUDIT_PAGE_BY_STATION, &[&station, &limit])),
            None => self.run(|c| c.query(sql::STATION_AUDIT_PAGE, &[&limit])),
        };
        rows.map(|rows| rows.iter().map(|r| r.get::<_, Value>(0)).collect()).inspect_err(|e| {
            eprintln!("db: station audit read failed: {e}");
        })
    }
}

/// At start: restore every component that Postgres holds; a component Postgres does not
/// hold yet keeps what its own files loaded (the first run imports them). Logs each outcome.
pub fn restore_all(app: &App) {
    wire_violations(app);
    if app.db.enabled() {
        restore_components(app);
        import_ide_audit(app);
    } else {
        eprintln!("db: no APZ_DATABASE_URL; components stay in their files; transcripts, infractions and the IDE audit go to files under the state dir");
        app.db.attach_files(&app.config.state_dir);
        crate::sanctions::migrate(app);
    }
    restore_transcripts_and_infractions(app);
}

/// The IDE audit rows the state directory's file kept before Postgres held any: imported once, oldest
/// first, as the infractions are (a Postgres run whose `ide_audit` is empty and whose file has rows).
fn import_ide_audit(app: &App) {
    match app.db.ide_audit(None, 1) {
        Ok(rows) if rows.is_empty() => {}
        Ok(_) => return,
        Err(e) => {
            eprintln!("db: ide audit import skipped: {e}");
            return;
        }
    }
    let mut rows = match files::Files::open(&app.config.state_dir).ide_audit(None, i64::MAX) {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("db: ide audit file cannot be read for the import: {e}");
            return;
        }
    };
    rows.reverse();
    for row in &rows {
        if let Err(e) = app.db.append_ide_audit(row) {
            eprintln!("db: ide audit import stopped part way: {e}");
            return;
        }
    }
    if !rows.is_empty() {
        eprintln!("db: imported {} IDE audit rows from the file", rows.len());
    }
}

/// Restores every component that Postgres holds (the first run imports the files' copies).
fn restore_components(app: &App) {
    for &name in COMPONENTS {
        match app.db.load(name) {
            Some(json) => match restore_into(app, name, &json) {
                Ok(()) => eprintln!("db: restored {name}"),
                Err(e) => {
                    eprintln!("db: {name} refused: {e}");
                    app.db.hold(name);
                }
            },
            // A failed read holds the component; its files stay the copy, not overwritten.
            None if app.db.is_held(name) => {}
            None => {
                if name == "sanctions" {
                    crate::sanctions::migrate(app);
                }
                eprintln!("db: {name} not in Postgres yet; importing");
                if let Err(e) = app.db.store(name, &snapshot_of(app, name)) {
                    eprintln!("db: import of {name} failed: {e}");
                }
            }
        }
    }
}

/// Loads the stored transcripts and infractions (from Postgres, or the files when there is none),
/// then writes every record from now on to the same place.
fn restore_transcripts_and_infractions(app: &App) {
    for cid in app.db.transcript_cids() {
        app.transcripts.preload(cid, app.db.transcript(cid, 500));
    }
    let db = Arc::clone(&app.db);
    app.transcripts.set_sink(Box::new(move |cid: u64, seq: u64, work: u64, kind: &str, at_ms: u64, data: &Value| {
        db.append_transcript(cid, seq, work, kind, at_ms, data)
    }));
    // Infractions recorded before Postgres held any (from the laws' own file) are imported once.
    if app.db.no_infractions() {
        for (cid, v) in app.laws.all_violations() {
            let tool = String::new();
            app.db.append_infraction(&Infraction { cid, law: v.law, at_ms: v.at_ms, tool, detail: v.detail });
        }
    }
}

/// Every recorded violation goes to Postgres (when enabled) and to the sanctions. Wired with or
/// without a database, so a violation is sanctioned whether or not the state lives in Postgres.
fn wire_violations(app: &App) {
    let db = Arc::clone(&app.db);
    let me = app.handle();
    app.laws.set_sink(Box::new(move |cid: u64, v: &crate::laws::Violation, tool: &str| {
        db.append_infraction(&Infraction {
            cid,
            law: v.law.clone(),
            at_ms: v.at_ms,
            tool: tool.to_string(),
            detail: v.detail.clone(),
        });
        if let Some(app) = me.upgrade() {
            crate::sanctions::on_violation(&app, cid, v, tool);
        }
    }));
}

/// The writer: every two seconds, store each component whose snapshot changed.
pub fn spawn_writer(app: Arc<App>) {
    if !app.db.enabled() && app.db.file_store().is_none() {
        return;
    }
    let spawned = std::thread::Builder::new().name("db-writer".into()).spawn(move || {
        let mut logged: BTreeMap<&str, Instant> = BTreeMap::new();
        loop {
            std::thread::sleep(Duration::from_secs(2));
            // A run without Postgres has no component stores to write; its files are only synced.
            let components: &[&str] = if app.db.enabled() { COMPONENTS } else { &[] };
            for &name in components {
                if app.db.is_held(name) {
                    continue;
                }
                if let Err(e) = app.db.store(name, &snapshot_of(&app, name)) {
                    if due(logged.get(name).copied()) {
                        eprintln!("db: store {name} failed: {e}");
                        logged.insert(name, Instant::now());
                    }
                }
            }
            app.db.sync_files();
        }
    });
    if let Err(e) = spawned {
        eprintln!("db: writer thread not started: {e}");
    }
}

/// A new, empty database on the throwaway test server named by `APZ_TEST_DATABASE_URL` (the
/// server's URL, ending in any database name, which is only used to connect). `None` (skip, logged)
/// without the variable. The name is `apz_test_<label>_<pid>`, unique per test and run. Refuses the
/// live `APZ_DATABASE_URL`: tests never touch the owner's database.
#[cfg(test)]
pub fn test_database(label: &str) -> Option<String> {
    const DATABASE_CREATE: &str = include_str!("../sql/test/database_create.sql");
    let base = match std::env::var("APZ_TEST_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            eprintln!("db: APZ_TEST_DATABASE_URL not set; skipping");
            return None;
        }
    };
    assert!(
        std::env::var("APZ_DATABASE_URL").map_or(true, |live| live != base),
        "APZ_TEST_DATABASE_URL must not be the live APZ_DATABASE_URL"
    );
    assert!(
        label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
        "test database label {label:?}: want lowercase letters, digits and underscores"
    );
    let name = format!("apz_test_{label}_{}", std::process::id());
    let (server, _) = base.rsplit_once('/').expect("APZ_TEST_DATABASE_URL names a database");
    let mut admin = match postgres::Client::connect(&base, postgres::NoTls) {
        Ok(client) => client,
        Err(err) => {
            eprintln!("db: APZ_TEST_DATABASE_URL server unreachable ({err}); skipping");
            return None;
        }
    };
    admin.batch_execute(&DATABASE_CREATE.replace("{name}", &name)).expect("create the test database");
    Some(format!("{server}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry is a snapshotted component: restored at start and written by the writer.
    #[test]
    fn models_is_a_component() {
        assert!(COMPONENTS.contains(&"models"));
    }

    /// Runs on a fresh database of the test server (skipped without APZ_TEST_DATABASE_URL); uses a
    /// test-only component and a unique cid. Exercises every runtime query in `sql`.
    #[test]
    fn db_round_trip() {
        let Some(url) = test_database("round_trip") else { return };
        let env = BTreeMap::from([("APZ_DATABASE_URL".to_string(), url)]);
        let db = Db::connect(&env);
        assert!(db.enabled());

        let component = "apz_test_component";
        let cid = 7_000_000_000_000_000_000u64 + u64::from(std::process::id());
        let json = format!(r#"{{"test":{cid}}}"#);
        assert_eq!(db.store(component, &json), Ok(true));
        assert_eq!(db.store(component, &json), Ok(false));
        let loaded = db.load(component).expect("stored row");
        assert_eq!(
            serde_json::from_str::<Value>(&loaded).unwrap(),
            serde_json::from_str::<Value>(&json).unwrap()
        );

        let data = serde_json::json!({ "note": "round trip" });
        db.append_transcript(cid, 1, 2, "test", 3, &data);
        db.append_transcript(cid, 1, 2, "test", 3, &data); // duplicate: ON CONFLICT DO NOTHING
        db.append_transcript(cid, 2, 2, "test", 4, &data);
        assert!(db.transcript_cids().contains(&cid));
        let rows = db.transcript(cid, 500);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, 1);
        assert_eq!(rows[1].0, 2);
        assert_eq!(rows[1].2, "test");
        assert_eq!(rows[1].4, data);

        let infraction = Infraction { cid, law: "law.test".into(), at_ms: 10, tool: "tool.test".into(), detail: "first".into() };
        let newer = Infraction { at_ms: 11, detail: "second".into(), ..infraction.clone() };
        db.append_infraction(&infraction);
        db.append_infraction(&newer);
        assert_eq!(db.infractions(Some(cid), 10), vec![newer.clone(), infraction.clone()]);
        assert_eq!(db.infractions(None, 1), vec![newer]);

        db.delete_transcript(cid);
        assert!(db.transcript(cid, 500).is_empty());
        assert!(!db.transcript_cids().contains(&cid));
        db.run(|c| c.execute(sql::STATE_DELETE, &[&component])).unwrap();
        assert!(db.load(component).is_none());
    }

    /// The query-level behaviour both backends must show: the same records in the same orders.
    fn check_queries(db: &Db, cid: u64) {
        db.append_transcript(cid, 1, 9, "work_start", 100, &serde_json::json!({ "n": 1 }));
        db.append_transcript(cid, 2, 9, "model_request", 101, &serde_json::json!({ "n": 2 }));
        db.append_transcript(cid, 2, 9, "model_request", 101, &serde_json::json!({ "again": true })); // duplicate seq
        db.append_transcript(cid, 3, 9, "work_end", 102, &serde_json::json!({ "n": 3 }));
        let all = db.transcript(cid, 500);
        assert_eq!(all.iter().map(|r| r.0).collect::<Vec<_>>(), vec![1, 2, 3], "oldest first");
        assert_eq!(all[1].4, serde_json::json!({ "n": 2 }), "the duplicate is ignored");
        assert_eq!(db.transcript(cid, 2).iter().map(|r| r.0).collect::<Vec<_>>(), vec![2, 3], "the newest two");
        assert!(db.transcript(cid, 0).is_empty());
        assert!(db.transcript_cids().contains(&cid));

        let first = Infraction { cid, law: "law.test".into(), at_ms: 50, tool: "tool.test".into(), detail: "first".into() };
        let second = Infraction { at_ms: 60, detail: "second".into(), ..first.clone() };
        // Same at_ms as `second`: the later record (higher id) comes first.
        let third = Infraction { detail: "third".into(), ..second.clone() };
        for row in [&first, &second, &third] {
            db.append_infraction(row);
        }
        assert_eq!(db.infractions(Some(cid), 10), vec![third.clone(), second.clone(), first.clone()]);
        assert_eq!(db.infractions(Some(cid), 1), vec![third.clone()]);
        assert_eq!(db.infractions(None, 10), vec![third, second, first]);

        // The IDE audit: newest first by at_ms (a tie goes to the later row), filtered by workspace.
        let (ws, other) = (format!("agent:{cid}"), format!("agent:{}", cid + 1));
        let audit = |at_ms: u64, workspace: &str, kind: &str| {
            serde_json::json!({ "at_ms": at_ms, "actor": "owner", "workspace": workspace, "kind": kind, "path": null })
        };
        let (w1, w2, w3) = (audit(10, &ws, "write"), audit(20, &other, "write"), audit(20, &ws, "restore"));
        for row in [&w1, &w2, &w3] {
            db.append_ide_audit(row).unwrap();
        }
        assert_eq!(db.ide_audit(None, 10).unwrap(), vec![w3.clone(), w2.clone(), w1.clone()]);
        assert_eq!(db.ide_audit(Some(&ws), 10).unwrap(), vec![w3.clone(), w1.clone()]);
        assert_eq!(db.ide_audit(Some(&ws), 1).unwrap(), vec![w3.clone()]);
        assert_eq!(db.ide_audit(Some(&other), 10).unwrap(), vec![w2.clone()]);
        assert!(db.ide_audit(Some("agent:0"), 10).unwrap().is_empty());
        assert!(db.append_ide_audit(&serde_json::json!({ "kind": "write" })).is_err(), "a row with no at_ms is refused");

        // The station audit: newest first by at_ms (a tie goes to the later row), filtered by station; the
        // registry-wide rows (station null) appear only in the whole list.
        let (st, st_other) = (format!("ws-{cid}"), format!("ws-{}", cid + 1));
        let station_row = |at_ms: u64, station: Option<&str>, kind: &str| {
            serde_json::json!({ "at_ms": at_ms, "actor": "owner", "station": station, "agent": null, "kind": kind })
        };
        let (s1, s2, s3) = (station_row(10, Some(&st), "join"), station_row(20, Some(&st_other), "join"), station_row(20, None, "code_issue"));
        for row in [&s1, &s2, &s3] {
            db.append_station_audit(row).unwrap();
        }
        assert_eq!(db.station_audit(None, 10).unwrap(), vec![s3.clone(), s2.clone(), s1.clone()]);
        assert_eq!(db.station_audit(Some(&st), 10).unwrap(), vec![s1.clone()]);
        assert_eq!(db.station_audit(Some(&st_other), 10).unwrap(), vec![s2.clone()]);
        assert!(db.station_audit(Some("ws-0"), 10).unwrap().is_empty());
        assert!(db.append_station_audit(&serde_json::json!({ "kind": "join" })).is_err(), "a row with no at_ms is refused");

        db.delete_transcript(cid);
        assert!(db.transcript(cid, 500).is_empty());
        assert!(!db.transcript_cids().contains(&cid));
    }

    /// Every query gives the same results on Postgres (a fresh test database, when the test server
    /// is set) and on the file fallback.
    #[test]
    fn every_query_behaves_the_same_on_both_backends() {
        let cid = 8_100_000_000_000 + u64::from(std::process::id());
        if let Some(url) = test_database("parity") {
            let env = BTreeMap::from([("APZ_DATABASE_URL".to_string(), url)]);
            let db = Db::connect(&env);
            assert!(db.enabled());
            check_queries(&db, cid);
        }
        let dir = std::env::temp_dir().join(format!("apid-parity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let db = Db::files_only(&dir);
        assert!(!db.enabled());
        check_queries(&db, cid);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
