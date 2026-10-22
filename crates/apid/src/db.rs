//! Postgres: the plaza's state of record. Memory stays the working copy every screen and
//! agent reads; Postgres is where it lives across restarts.
//!
//! Each stateful part of the plaza gives a `snapshot()` (its whole state as JSON) and a
//! `restore(json)`. At start apid restores every component from Postgres; every two seconds a
//! writer thread snapshots each component and upserts the ones that changed into
//! `plaza_state`. Transcripts are no longer kept here: they live in the store (`transcript`, in
//! `transcripts.db`). With no `APZ_DATABASE_URL` the plaza runs as before (the components' own
//! files only).
//!
//! Schema (the numbered migrations in `sql/migrations`, applied on connect; see `migrate`).
//! Every SQL statement is a file under `sql/queries`, embedded as a const in `sql` (docs/dev/sql.md).
//! Tables:
//!   plaza_state(component text primary key, data jsonb not null, hash bigint not null,
//!               updated_at timestamptz not null default now())
//!   ide_audit(seq bigserial, at_ms bigint, workspace text, kind text, data jsonb)
//!              (append-only: the owner's IDE actions, migration 0008)
//!   station_audit(seq bigserial, at_ms bigint, station text, agent bigint, kind text, data jsonb)
//!              (append-only: the owner's workstation actions and lease changes, migration 0010)

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::state::App;

/// Every component still snapshotted into Postgres, by name, in restore order. The task ledger and the components of
/// `plaza.db` are not here: `store` keeps them.
pub const COMPONENTS: &[&str] = &[
    "treasury",
    "social",
    "members",
    "conversations",
    "toolbox",
    "harness",
    "approvals",
    "acp_sessions",
    "library",
    "deliverables",
    "spawned",
    "rules",
    "work",
    "control",
    "custom_roles",
    "agent_proposals",
    "nodes",
    "stations",
    "prices",
    "usage",
    "foundry",
    "models",
    "vault",
    "mcp",
];

pub mod data_migrate;
pub mod files;
pub mod hub_mcp;
pub mod migrate;
pub mod recordings;
pub mod roster;
pub mod sql;
pub mod sync;

const MINUTE: Duration = Duration::from_secs(60);

/// The component's snapshot ("" when it has nothing to save yet).
pub fn snapshot_of(app: &App, component: &str) -> String {
    match component {
        "treasury" => app.plaza.treasury.snapshot(),
        "social" => app.plaza.social.snapshot(),
        "members" => app.members.snapshot(),
        "conversations" => app.conversations.snapshot(),
        "toolbox" => app.toolbox.snapshot(),
        "harness" => app.harness.snapshot(),
        "approvals" => app.approvals.snapshot(),
        "acp_sessions" => app.acp_sessions.snapshot(),
        "library" => app.library.snapshot(),
        "deliverables" => app.deliverables.snapshot(),
        "spawned" => app.spawned.snapshot(),
        "rules" => app.laws.rules_snapshot(),
        "work" => app.work.snapshot(),
        "control" => app.control.snapshot(),
        "custom_roles" => app.custom_roles.snapshot(),
        "agent_proposals" => app.agent_proposals.snapshot(),
        "nodes" => app.nodes.snapshot(),
        "stations" => app.stations.snapshot(),
        "prices" => app.prices.snapshot(),
        "usage" => app.usage.snapshot(),
        "foundry" => app.foundry.snapshot(),
        "models" => app.models.snapshot(),
        "vault" => app.vault.snapshot(),
        "mcp" => app.mcp.store.snapshot(),
        _ => String::new(),
    }
}

/// Restores one component from its stored snapshot.
pub fn restore_into(app: &App, component: &str, json: &str) -> Result<(), String> {
    match component {
        "treasury" => app.plaza.treasury.restore(json),
        "social" => app.plaza.social.restore(json),
        "members" => app.members.restore(json),
        "conversations" => app.conversations.restore(json),
        "toolbox" => app.toolbox.restore(json),
        "harness" => app.harness.restore(json),
        "approvals" => app.approvals.restore(json),
        "acp_sessions" => app.acp_sessions.restore(json),
        "library" => app.library.restore(json),
        "deliverables" => app.deliverables.restore(json),
        "spawned" => app.spawned.restore(json),
        "rules" => app.laws.rules_restore(json),
        "work" => app.work.restore(json),
        "control" => app.control.restore(json),
        "custom_roles" => app.custom_roles.restore(json),
        "agent_proposals" => app.agent_proposals.restore(json),
        "nodes" => app.nodes.restore(json),
        "stations" => app.stations.restore(json),
        "prices" => app.prices.restore(json),
        "usage" => app.usage.restore(json),
        "foundry" => app.foundry.restore(json),
        "models" => app.models.restore(json),
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

/// Why `url` must not carry plaza state, or `None` when it may. This build connects without TLS, so a database off
/// this machine would receive the plaza's state in clear text: refused. Allowed: loopback, a Unix socket, and
/// `sslmode=disable`, the owner's explicit choice (its warning is printed here; called once, at start).
/// `sslmode=require` is refused even on loopback, as this build cannot satisfy it.
fn tls_refusal(url: &str) -> Option<String> {
    const HINT: &str = "use a database on this machine, or add sslmode=disable to accept an unencrypted connection";
    let config = match url.parse::<postgres::Config>() {
        Ok(config) => config,
        // tokio-postgres knows only disable, prefer and require: verify-ca and verify-full stop here.
        Err(e) => {
            return Some(format!(
                "db: APZ_DATABASE_URL cannot be used ({e}): this build connects without TLS; {HINT}"
            ));
        }
    };
    // A host that starts with `/` is a Unix-socket directory, on this machine. Off Unix the driver parses it as a
    // TCP name (it has no Unix sockets there), so it is recognised here by the slash.
    let remote = config.get_hosts().iter().any(|host| {
        matches!(host, postgres::config::Host::Tcp(name) if !is_loopback_host(name) && !name.starts_with('/'))
    }) || config.get_hostaddrs().iter().any(|ip| !ip.is_loopback());
    match config.get_ssl_mode() {
        postgres::config::SslMode::Disable => {
            if remote {
                eprintln!(
                    "db: APZ_DATABASE_URL has sslmode=disable and names a database off this machine: the connection is not encrypted"
                );
            }
            None
        }
        postgres::config::SslMode::Prefer if !remote => None,
        postgres::config::SslMode::Prefer => Some(format!(
            "db: APZ_DATABASE_URL names a database off this machine, and this build cannot encrypt the connection; {HINT}"
        )),
        // sslmode=require, verify-ca and verify-full (and any mode added later): not satisfiable here.
        _ => Some(format!(
            "db: APZ_DATABASE_URL asks for sslmode=require or stronger, which this build cannot provide; {HINT}"
        )),
    }
}

/// A host name that is this machine: `localhost`, or a loopback address (IPv6 brackets allowed).
fn is_loopback_host(name: &str) -> bool {
    let bare = name.trim_start_matches('[').trim_end_matches(']');
    bare.eq_ignore_ascii_case("localhost")
        || bare
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Opens a connection (no schema work: the migrations run once, on connect).
fn connect_raw(url: &str) -> Result<postgres::Client, postgres::Error> {
    postgres::Client::connect(url, postgres::NoTls)
}

/// Connects, retrying with a doubling wait (1 s, 2 s, 4 s, ...) until `budget` runs out. The
/// wait is cut to what is left of the budget, so the last attempt falls at its end; the error
/// returned is the last one seen.
fn connect_with_retry(url: &str, budget: Duration) -> Result<postgres::Client, String> {
    let started = Instant::now();
    let mut wait = Duration::from_secs(1);
    loop {
        let err = match connect_raw(url) {
            Ok(client) => return Ok(client),
            Err(err) => err,
        };
        let left = budget.saturating_sub(started.elapsed());
        if left.is_zero() {
            return Err(err.to_string());
        }
        std::thread::sleep(wait.min(left));
        wait *= 2;
    }
}

/// Whether a log line last written at `last` may be written again now.
fn due(last: Option<Instant>) -> bool {
    last.is_none_or(|t| t.elapsed() >= MINUTE)
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
    ide_audit_logged: Mutex<Option<Instant>>,
    station_audit_logged: Mutex<Option<Instant>>,
    /// The file fallback for the audit logs and recordings (see `attach_files`).
    files: std::sync::OnceLock<files::Files>,
}

impl Db {
    fn with(url: String, client: Option<postgres::Client>) -> Db {
        Db {
            url,
            client: client.map(Mutex::new),
            stored: Mutex::new(BTreeMap::new()),
            held: Mutex::new(BTreeSet::new()),
            ide_audit_logged: Mutex::new(None),
            station_audit_logged: Mutex::new(None),
            files: std::sync::OnceLock::new(),
        }
    }

    /// Connects to `APZ_DATABASE_URL` from `env` and migrates the schema to this build's. Without
    /// the variable the plaza runs without Postgres, on its files. With the variable set, a database
    /// that does not answer within 30 s, or a migration that refuses (a changed applied file, a
    /// failed statement), stops apid at start: it never runs on files when a database is configured.
    pub fn connect(env: &BTreeMap<String, String>) -> Db {
        let url = match env.get("APZ_DATABASE_URL") {
            Some(url) if !url.trim().is_empty() => url.clone(),
            _ => return Db::with(String::new(), None),
        };
        if let Some(reason) = tls_refusal(&url) {
            eprintln!("{reason}");
            std::process::exit(1);
        }
        let mut client = match connect_with_retry(&url, Duration::from_secs(30)) {
            Ok(client) => client,
            Err(e) => {
                eprintln!(
                    "db: APZ_DATABASE_URL is set but the database did not answer within 30 s: {e}; refusing to start on files"
                );
                std::process::exit(1);
            }
        };
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

    pub fn enabled(&self) -> bool {
        self.client.is_some()
    }

    /// Makes `state_dir` the home of the file fallback: with no Postgres this run, the audit logs and
    /// the recordings are written to files there (see `db::files`).
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

    /// A database with no Postgres whose audit logs and recordings live under `state_dir`.
    #[cfg(test)]
    pub fn files_only(state_dir: &std::path::Path) -> Db {
        let db = Db::with(String::new(), None);
        db.attach_files(state_dir);
        db
    }

    /// Runs `op` on the connection; on a closed connection, reconnects once and retries.
    pub(crate) fn run<T>(
        &self,
        mut op: impl FnMut(&mut postgres::Client) -> Result<T, postgres::Error>,
    ) -> Result<T, String> {
        let slot = self
            .client
            .as_ref()
            .ok_or_else(|| "no database".to_string())?;
        let mut client = slot.lock().unwrap_or_else(PoisonError::into_inner);
        // Each call of `op` is one database round trip, timed into `hub.pg_ms`.
        let mut timed = |client: &mut postgres::Client| {
            let started = Instant::now();
            let result = op(client);
            crate::metrics::collect::database(started.elapsed().as_secs_f64() * 1000.0);
            result
        };
        match timed(&mut client) {
            Ok(value) => Ok(value),
            Err(e) if e.is_closed() => {
                *client = connect_raw(&self.url).map_err(|e| format!("reconnect failed: {e}"))?;
                timed(&mut client).map_err(|e| e.to_string())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn hold(&self, component: &str) {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(component.to_string());
    }

    pub fn is_held(&self, component: &str) -> bool {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(component)
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
                self.stored
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(component.to_string(), hash);
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

    /// Upserts every item in one transaction, so the components stored together are persisted from one instant:
    /// all of them or none. Empty and unchanged items are skipped, as `store` skips them. A failed transaction
    /// keeps every hash, so the next pass stores them again.
    pub fn store_many(&self, items: &[(&str, String)]) -> Result<(), String> {
        if !self.enabled() {
            return Ok(());
        }
        let mut stored = self.stored.lock().unwrap_or_else(PoisonError::into_inner);
        let mut changed: Vec<(&str, &str, i64)> = Vec::new();
        for (component, json) in items {
            if json.is_empty() {
                continue;
            }
            let hash = fnv1a(json.as_bytes());
            if stored.get(*component) != Some(&hash) {
                changed.push((*component, json.as_str(), hash));
            }
        }
        if changed.is_empty() {
            return Ok(());
        }
        self.run(|c| {
            let mut tx = c.transaction()?;
            for &(component, json, hash) in &changed {
                tx.execute(sql::STATE_UPSERT, &[&component, &json, &hash])?;
            }
            tx.commit()
        })?;
        for (component, _, hash) in changed {
            stored.insert(component.to_string(), hash);
        }
        Ok(())
    }
}

/// The IDE audit: one owner action on an agent workspace (`ide.rs` builds the row). The row is the table's
/// data and its `at_ms`, `workspace` and `kind` are its columns. A failed append is
/// an error: the owner's action has happened, so the route answers with it (see `append_ide_audit`).
impl Db {
    /// Appends one owner action to the IDE audit (table `ide_audit`, or the file fallback). A failed
    /// append is returned to the caller and logged at most once a minute.
    pub fn append_ide_audit(&self, row: &Value) -> Result<(), String> {
        let written = self.write_ide_audit(row);
        if let Err(e) = &written {
            let mut logged = self
                .ide_audit_logged
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if due(*logged) {
                *logged = Some(Instant::now());
                eprintln!(
                    "db: ide audit append failed: {e} (answered to the owner; logged once a minute)"
                );
            }
        }
        written
    }

    fn write_ide_audit(&self, row: &Value) -> Result<(), String> {
        let at_ms = row["at_ms"].as_u64().ok_or("the audit row has no at_ms")? as i64;
        let workspace = row["workspace"]
            .as_str()
            .ok_or("the audit row has no workspace")?;
        let kind = row["kind"].as_str().ok_or("the audit row has no kind")?;
        if let Some(files) = self.file_store() {
            return files.append_ide_audit(row);
        }
        if !self.enabled() {
            return Err("no database and no state directory for the IDE audit".to_string());
        }
        self.run(|c| c.execute(sql::IDE_AUDIT_APPEND, &[&at_ms, &workspace, &kind, row]))
            .map(|_| ())
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
            Some(workspace) => {
                self.run(|c| c.query(sql::IDE_AUDIT_PAGE_BY_WORKSPACE, &[&workspace, &limit]))
            }
            None => self.run(|c| c.query(sql::IDE_AUDIT_PAGE, &[&limit])),
        };
        rows.map(|rows| rows.iter().map(|r| r.get::<_, Value>(0)).collect())
            .inspect_err(|e| {
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
            let mut logged = self
                .station_audit_logged
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if due(*logged) {
                *logged = Some(Instant::now());
                eprintln!(
                    "db: station audit append failed: {e} (answered to the owner; logged once a minute)"
                );
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
        self.run(|c| {
            c.execute(
                sql::STATION_AUDIT_APPEND,
                &[&at_ms, &station, &agent, &kind, row],
            )
        })
        .map(|_| ())
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
            Some(station) => {
                self.run(|c| c.query(sql::STATION_AUDIT_PAGE_BY_STATION, &[&station, &limit]))
            }
            None => self.run(|c| c.query(sql::STATION_AUDIT_PAGE, &[&limit])),
        };
        rows.map(|rows| rows.iter().map(|r| r.get::<_, Value>(0)).collect())
            .inspect_err(|e| {
                eprintln!("db: station audit read failed: {e}");
            })
    }
}

/// The unfurl cache (`unfurl_cache`, migration 0009): one answer per reference, upserted. Without Postgres the
/// unfurl keeps its own memory copy (unfurl/mod.rs), so these answer nothing when the database is not enabled.
impl Db {
    /// The cached answer for `key`: its data, whether it is ok, and when it was fetched (ms). None when none is stored.
    pub fn unfurl_get(&self, key: &str) -> Result<Option<(Value, bool, u64)>, String> {
        if !self.enabled() {
            return Ok(None);
        }
        let row = self.run(|c| c.query_opt(sql::UNFURL_GET, &[&key]))?;
        Ok(row.map(|row| {
            (
                row.get::<_, Value>(0),
                row.get::<_, bool>(1),
                row.get::<_, i64>(2) as u64,
            )
        }))
    }

    /// Stores `data` as the answer for `key`, replacing the one stored.
    pub fn unfurl_put(
        &self,
        key: &str,
        data: &Value,
        ok: bool,
        fetched_ms: u64,
    ) -> Result<(), String> {
        let fetched = fetched_ms as i64;
        self.run(|c| c.execute(sql::UNFURL_PUT, &[&key, data, &ok, &fetched]))
            .map(|_| ())
    }
}

/// At start: restore every component that Postgres holds; a component Postgres does not
/// hold yet keeps what its own files loaded (the first run imports them). Logs each outcome.
pub fn restore_all(app: &App) {
    if app.db.enabled() {
        restore_components(app);
        import_ide_audit(app);
    } else {
        eprintln!(
            "db: no APZ_DATABASE_URL; components stay in their files; the audit logs and recordings go to files under the state dir"
        );
        app.db.attach_files(&app.config.state_dir);
    }
    restore_transcripts(app);
}

/// The IDE audit rows the state directory's file kept before Postgres held any: imported once, oldest
/// first (a Postgres run whose `ide_audit` is empty and whose file has rows).
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
                eprintln!("db: {name} not in Postgres yet; importing");
                if let Err(e) = app.db.store(name, &snapshot_of(app, name)) {
                    eprintln!("db: import of {name} failed: {e}");
                }
            }
        }
    }
    reconcile_escrows(app);
}

/// The finished tasks (done, failed, withdrawn) whose escrow still holds money, from `(id, state, escrow)` triples.
fn stranded_escrows(tasks: &[(String, String, i128)]) -> Vec<&(String, String, i128)> {
    tasks
        .iter()
        .filter(|(_, state, escrow)| {
            matches!(state.as_str(), "done" | "failed" | "withdrawn") && *escrow > 0
        })
        .collect()
}

/// After a restore, refunds each finished task whose escrow still holds money: a stop between the treasury's store
/// and the task ledger's can leave one (the task finished, its escrow not yet paid back). Returns how many it found.
fn reconcile_escrows(app: &App) -> usize {
    let tasks: Vec<(String, String, i128)> = app
        .plaza
        .tasks
        .list()
        .iter()
        .map(|task| {
            let view = task.view();
            let escrow = app.plaza.treasury.escrow_balance(view.id);
            (view.id.to_string(), view.state.to_string(), escrow)
        })
        .collect();
    let stranded = stranded_escrows(&tasks);
    for (id, state, escrow) in &stranded {
        eprintln!(
            "db: restore: task {id} is {state} but its escrow holds {escrow}; refunding it to the plaza's budget"
        );
        if let Err(e) = app.plaza.treasury.refund_task_to_budget(id) {
            eprintln!("db: restore: refund of task {id} failed: {e}");
        }
    }
    if !stranded.is_empty() {
        eprintln!(
            "db: restore: {} finished tasks had escrow to refund",
            stranded.len()
        );
    }
    stranded.len()
}

/// Loads the stored transcripts into the transcript store from the store's database, which then
/// writes every record from now on there.
fn restore_transcripts(app: &App) {
    if let Err(e) =
        crate::transcript::restore(&app.transcripts, app.store.db(crate::store::TRANSCRIPTS))
    {
        eprintln!("apid: transcripts not restored: {e}");
    }
}

/// One pass of the writer: store every component that is not held. A store that fails is
/// passed to `on_error` with its name and the error; the loop throttles its log, the final pass logs each one.
fn write_pass_with(app: &App, mut on_error: impl FnMut(&str, &str)) {
    // A run without Postgres has no component stores to write.
    let components: &[&str] = if app.db.enabled() { COMPONENTS } else { &[] };
    // The treasury is snapshotted before the other stores and stored alone in its transaction: the escrows of the
    // tasks live in it, and the task ledger is in `plaza.db` now.
    const LEDGERS: &[&str] = &["treasury"];
    let ledgers: Vec<(&str, String)> = components
        .iter()
        .copied()
        .filter(|name| LEDGERS.contains(name) && !app.db.is_held(name))
        .map(|name| (name, snapshot_of(app, name)))
        .collect();
    if let Err(e) = app.db.store_many(&ledgers) {
        for &(name, _) in &ledgers {
            on_error(name, &e);
        }
    }
    for &name in components {
        if LEDGERS.contains(&name) || app.db.is_held(name) {
            continue;
        }
        if let Err(e) = app.db.store(name, &snapshot_of(app, name)) {
            on_error(name, &e.to_string());
        }
    }
}

/// The final pass at shutdown: every component and file, and every store error logged.
pub fn write_pass(app: &App) {
    write_pass_with(app, |name, err| eprintln!("db: store {name} failed: {err}"));
}

/// The writer: every two seconds, store each component whose snapshot changed.
pub fn spawn_writer(app: Arc<App>) {
    if !app.db.enabled() && app.db.file_store().is_none() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("db-writer".into())
        .spawn(move || {
            let mut logged: BTreeMap<String, Instant> = BTreeMap::new();
            loop {
                std::thread::sleep(Duration::from_secs(2));
                write_pass_with(&app, |name, err| {
                    if due(logged.get(name).copied()) {
                        eprintln!("db: store {name} failed: {err}");
                        logged.insert(name.to_string(), Instant::now());
                    }
                });
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
        label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
        "test database label {label:?}: want lowercase letters, digits and underscores"
    );
    let name = format!("apz_test_{label}_{}", std::process::id());
    // The URL's query (sslmode, say) goes with every test database.
    let (path, query) = match base.split_once('?') {
        Some((path, query)) => (path, format!("?{query}")),
        None => (base.as_str(), String::new()),
    };
    let (server, _) = path
        .rsplit_once('/')
        .expect("APZ_TEST_DATABASE_URL names a database");
    let mut admin = match postgres::Client::connect(&base, postgres::NoTls) {
        Ok(client) => client,
        Err(err) => {
            eprintln!("db: APZ_TEST_DATABASE_URL server unreachable ({err}); skipping");
            return None;
        }
    };
    admin
        .batch_execute(&DATABASE_CREATE.replace("{name}", &name))
        .expect("create the test database");
    Some(format!("{server}/{name}{query}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A finished task whose escrow still holds money is stranded; open or claimed tasks, and empty escrows, are not.
    #[test]
    fn the_ledger_components_are_stored_together_and_read_back() {
        let Some(url) = test_database("store_many") else {
            return;
        };
        let env = BTreeMap::from([("APZ_DATABASE_URL".to_string(), url)]);
        let db = Db::connect(&env);
        assert!(db.enabled());
        let items = [
            ("treasury", r#"{"entries":[1]}"#.to_string()),
            ("tasks", r#"{"tasks":[]}"#.to_string()),
        ];
        db.store_many(&items).expect("one transaction");
        for (name, json) in &items {
            let stored: serde_json::Value =
                serde_json::from_str(&db.load(name).expect("stored")).expect("json");
            let wanted: serde_json::Value = serde_json::from_str(json).expect("json");
            assert_eq!(stored, wanted, "{name}");
        }
        // An unchanged item is skipped; a changed one is written again.
        db.store_many(&[("tasks", r#"{"tasks":[1]}"#.to_string())])
            .expect("a second pass");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&db.load("tasks").expect("stored"))
                .expect("json"),
            serde_json::json!({"tasks": [1]})
        );
    }

    #[test]
    fn stranded_escrows_are_finished_tasks_with_money_in_escrow() {
        let tasks = [
            ("t1".to_string(), "done".to_string(), 5),
            ("t2".to_string(), "withdrawn".to_string(), 0),
            ("t3".to_string(), "open".to_string(), 9),
            ("t4".to_string(), "failed".to_string(), 3),
            ("t5".to_string(), "claimed".to_string(), 0),
        ];
        let stranded = stranded_escrows(&tasks);
        let ids: Vec<&str> = stranded.iter().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(ids, ["t1", "t4"]);
    }

    /// The registry is a snapshotted component: restored at start and written by the writer.
    #[test]
    fn models_is_a_component() {
        assert!(COMPONENTS.contains(&"models"));
    }

    #[test]
    fn tls_loopback_and_unix_socket_are_allowed() {
        for url in [
            "postgres://plaza:x@127.0.0.1:5432/plaza",
            "postgres://plaza:x@localhost/plaza",
            "postgres://plaza:x@[::1]:5432/plaza",
            "postgres://plaza@/plaza?host=/var/run/postgresql",
            "postgres://plaza@%2Fvar%2Frun%2Fpostgresql/plaza",
        ] {
            assert_eq!(tls_refusal(url), None, "{url}");
        }
    }

    #[test]
    fn tls_refuses_remote_hosts_unless_sslmode_disable() {
        for url in [
            "postgres://plaza:x@db.example.com/plaza",
            "postgres://plaza:x@db.example.com/plaza?sslmode=prefer",
            "postgres://plaza:x@localhost/plaza?hostaddr=10.0.0.5",
            "postgres://plaza:x@localhost/plaza?sslmode=require",
            "postgres://plaza:x@localhost/plaza?sslmode=verify-ca",
            "postgres://plaza:x@localhost/plaza?sslmode=verify-full",
        ] {
            assert!(tls_refusal(url).is_some(), "{url}");
        }
        assert_eq!(
            tls_refusal("postgres://plaza:x@db.example.com/plaza?sslmode=disable"),
            None
        );
    }

    /// A database that never answers is retried for the budget, then given up on with the last error: port 9 on
    /// loopback refuses at once, so the call returns within about the budget (here 300 ms), not after the 30 s
    /// `connect` allows. Windows retries a refused loopback connect for about 2 s before it reports it, so there one
    /// attempt alone takes that long.
    #[test]
    fn connect_with_retry_gives_up_within_the_budget() {
        let started = Instant::now();
        let result = connect_with_retry("postgres://x:y@127.0.0.1:9/x", Duration::from_millis(300));
        assert!(result.is_err());
        let limit = Duration::from_secs(if cfg!(windows) { 6 } else { 1 });
        assert!(started.elapsed() < limit, "{:?}", started.elapsed());
    }

    /// The unfurl cache on Postgres: a miss, a store, a read back, and an upsert that replaces the row. A database
    /// that is not enabled answers a miss and stores nothing (the unfurl's memory copy covers that case).
    #[test]
    fn unfurl_cache_round_trips_and_an_upsert_replaces() {
        use serde_json::json;
        let disabled = Db::with(String::new(), None);
        assert_eq!(disabled.unfurl_get("k"), Ok(None));
        assert!(disabled.unfurl_put("k", &json!({}), true, 1).is_err());

        let Some(url) = test_database("unfurl_cache") else {
            return;
        };
        let env = BTreeMap::from([("APZ_DATABASE_URL".to_string(), url)]);
        let db = Db::connect(&env);
        assert!(db.enabled());
        let key = format!("test-key-{}", std::process::id());
        assert_eq!(db.unfurl_get(&key), Ok(None));
        db.unfurl_put(
            &key,
            &json!({"ok": true, "card": {"title": "a"}}),
            true,
            1_000,
        )
        .expect("stored");
        assert_eq!(
            db.unfurl_get(&key),
            Ok(Some((
                json!({"ok": true, "card": {"title": "a"}}),
                true,
                1_000
            )))
        );
        db.unfurl_put(
            &key,
            &json!({"ok": false, "error": "rate_limited"}),
            false,
            2_000,
        )
        .expect("replaced");
        assert_eq!(
            db.unfurl_get(&key),
            Ok(Some((
                json!({"ok": false, "error": "rate_limited"}),
                false,
                2_000
            )))
        );
    }

    /// Runs on a fresh database of the test server (skipped without APZ_TEST_DATABASE_URL); uses a
    /// test-only component and a unique value. Exercises the component's queries.
    #[test]
    fn db_round_trip() {
        let Some(url) = test_database("round_trip") else {
            return;
        };
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

        db.run(|c| c.execute(sql::STATE_DELETE, &[&component]))
            .unwrap();
        assert!(db.load(component).is_none());
    }

    /// The query-level behaviour both backends must show: the same rows in the same orders.
    fn check_queries(db: &Db, cid: u64) {
        // The IDE audit: newest first by at_ms (a tie goes to the later row), filtered by workspace.
        let (ws, other) = (format!("agent:{cid}"), format!("agent:{}", cid + 1));
        let audit = |at_ms: u64, workspace: &str, kind: &str| serde_json::json!({ "at_ms": at_ms, "actor": "owner", "workspace": workspace, "kind": kind, "path": null });
        let (w1, w2, w3) = (
            audit(10, &ws, "write"),
            audit(20, &other, "write"),
            audit(20, &ws, "restore"),
        );
        for row in [&w1, &w2, &w3] {
            db.append_ide_audit(row).unwrap();
        }
        assert_eq!(
            db.ide_audit(None, 10).unwrap(),
            vec![w3.clone(), w2.clone(), w1.clone()]
        );
        assert_eq!(
            db.ide_audit(Some(&ws), 10).unwrap(),
            vec![w3.clone(), w1.clone()]
        );
        assert_eq!(db.ide_audit(Some(&ws), 1).unwrap(), vec![w3.clone()]);
        assert_eq!(db.ide_audit(Some(&other), 10).unwrap(), vec![w2.clone()]);
        assert!(db.ide_audit(Some("agent:0"), 10).unwrap().is_empty());
        assert!(
            db.append_ide_audit(&serde_json::json!({ "kind": "write" }))
                .is_err(),
            "a row with no at_ms is refused"
        );

        // The station audit: newest first by at_ms (a tie goes to the later row), filtered by station; the
        // registry-wide rows (station null) appear only in the whole list.
        let (st, st_other) = (format!("ws-{cid}"), format!("ws-{}", cid + 1));
        let station_row = |at_ms: u64, station: Option<&str>, kind: &str| serde_json::json!({ "at_ms": at_ms, "actor": "owner", "station": station, "agent": null, "kind": kind });
        let (s1, s2, s3) = (
            station_row(10, Some(&st), "join"),
            station_row(20, Some(&st_other), "join"),
            station_row(20, None, "code_issue"),
        );
        for row in [&s1, &s2, &s3] {
            db.append_station_audit(row).unwrap();
        }
        assert_eq!(
            db.station_audit(None, 10).unwrap(),
            vec![s3.clone(), s2.clone(), s1.clone()]
        );
        assert_eq!(db.station_audit(Some(&st), 10).unwrap(), vec![s1.clone()]);
        assert_eq!(
            db.station_audit(Some(&st_other), 10).unwrap(),
            vec![s2.clone()]
        );
        assert!(db.station_audit(Some("ws-0"), 10).unwrap().is_empty());
        assert!(
            db.append_station_audit(&serde_json::json!({ "kind": "join" }))
                .is_err(),
            "a row with no at_ms is refused"
        );
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

    /// The board's pauses are a setting, kept in the store: rows saved and loaded again hold them, and a save that
    /// names no pauses keeps the ones in force.
    #[test]
    fn board_pauses_are_saved_in_the_store() {
        let store = crate::settings::Store::new(false);
        let changed = |(changed, _): (bool, crate::settings::BoardPauses)| changed;
        assert_eq!(
            store.set_board_pause(Some("pr4"), true).map(changed),
            Ok(true)
        );
        assert_eq!(
            store.set_board_pause(Some("pr4"), true).map(changed),
            Ok(false)
        );
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::settings::def().ddl()).unwrap();
        apz_store::table::replace(&conn, crate::settings::def(), &store.rows().unwrap()).unwrap();
        let reloaded = crate::settings::Store::new(false);
        reloaded.load(&conn).unwrap();
        let pauses = reloaded.get().board_pauses;
        assert!(pauses.holds(Some("pr4")) && !pauses.holds(Some("pr5")) && !pauses.holds(None));
        // A save that names no pauses keeps the ones in force.
        let saved = crate::settings::Settings {
            board_pauses: Default::default(),
            ..reloaded.get()
        };
        reloaded.put(saved).unwrap();
        assert!(
            reloaded.get().board_pauses.holds(Some("pr4")),
            "a save keeps the pauses in force"
        );
    }
}
