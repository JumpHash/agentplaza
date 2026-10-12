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
//!   infraction(id bigserial, cid bigint, law text, at_ms bigint, tool text, detail text)

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
    "names", "settings", "providers", "spawned", "laws", "society", "work",
    "control", "custom_roles", "agent_proposals", "nodes",
];

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS plaza_state (
    component text PRIMARY KEY,
    data jsonb NOT NULL,
    hash bigint NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS transcript (
    cid bigint,
    seq bigint,
    work bigint,
    kind text,
    at_ms bigint,
    data jsonb,
    PRIMARY KEY (cid, seq)
);
CREATE TABLE IF NOT EXISTS infraction (
    id bigserial PRIMARY KEY,
    cid bigint NOT NULL,
    law text NOT NULL,
    at_ms bigint NOT NULL,
    tool text NOT NULL,
    detail text NOT NULL
);
CREATE INDEX IF NOT EXISTS infraction_cid ON infraction (cid, at_ms DESC);";

const UPSERT: &str = "INSERT INTO plaza_state (component, data, hash, updated_at) VALUES ($1, $2::text::jsonb, $3, now()) ON CONFLICT (component) DO UPDATE SET data = EXCLUDED.data, hash = EXCLUDED.hash, updated_at = now()";

const APPEND: &str = "INSERT INTO transcript (cid, seq, work, kind, at_ms, data) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING";

const INFRACTION_APPEND: &str = "INSERT INTO infraction (cid, law, at_ms, tool, detail) VALUES ($1,$2,$3,$4,$5)";

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
        "society" => app.society.snapshot(),
        "work" => app.work.snapshot(),
        "control" => app.control.snapshot(),
        "custom_roles" => app.custom_roles.snapshot(),
        "agent_proposals" => app.agent_proposals.snapshot(),
        "nodes" => app.nodes.snapshot(),
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
        "control" => app.control.restore(json),
        "custom_roles" => app.custom_roles.restore(json),
        "agent_proposals" => app.agent_proposals.restore(json),
        "nodes" => app.nodes.restore(json),
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

/// Opens the connection and makes sure the schema exists.
fn open(url: &str) -> Result<postgres::Client, postgres::Error> {
    let mut client = postgres::Client::connect(url, postgres::NoTls)?;
    client.batch_execute(SCHEMA)?;
    Ok(client)
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
        }
    }

    /// Connects to `APZ_DATABASE_URL` from `env` and creates the schema; without the
    /// variable (or when the connection fails, logged) the plaza runs without Postgres.
    pub fn connect(env: &BTreeMap<String, String>) -> Db {
        let url = match env.get("APZ_DATABASE_URL") {
            Some(url) if !url.trim().is_empty() => url.clone(),
            _ => return Db::with(String::new(), None),
        };
        match open(&url) {
            Ok(client) => Db::with(url, Some(client)),
            Err(e) => {
                eprintln!("db: connect failed: {e}");
                Db::with(String::new(), None)
            }
        }
    }

    /// Deletes the stored transcript rows of `cid` (a wipe).
    pub fn delete_transcript(&self, cid: u64) {
        if !self.enabled() {
            return;
        }
        let cid = cid as i64;
        if let Err(err) = self.run(|c| c.execute("DELETE FROM transcript WHERE cid = $1", &[&cid])) {
            eprintln!("apid: delete transcript {cid}: {err}");
        }
    }

    pub fn enabled(&self) -> bool {
        self.client.is_some()
    }

    /// Runs `op` on the connection; on a closed connection, reconnects once and retries.
    fn run<T>(&self, mut op: impl FnMut(&mut postgres::Client) -> Result<T, postgres::Error>) -> Result<T, String> {
        let slot = self.client.as_ref().ok_or_else(|| "no database".to_string())?;
        let mut client = slot.lock().unwrap_or_else(PoisonError::into_inner);
        match op(&mut *client) {
            Ok(value) => Ok(value),
            Err(e) if e.is_closed() => {
                *client = open(&self.url).map_err(|e| format!("reconnect failed: {e}"))?;
                op(&mut *client).map_err(|e| e.to_string())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn hold(&self, component: &str) {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).insert(component.to_string());
    }

    fn is_held(&self, component: &str) -> bool {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).contains(component)
    }

    /// The stored snapshot of `component`, if any. A failed read holds the component out of
    /// writes, so a restart never overwrites state it could not read.
    pub fn load(&self, component: &str) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        match self.run(|c| c.query_opt("SELECT data::text FROM plaza_state WHERE component = $1", &[&component])) {
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
        self.run(|c| c.execute(UPSERT, &[&component, &json, &hash]))?;
        stored.insert(component.to_string(), hash);
        Ok(true)
    }

    /// Appends one transcript record. A failed append is dropped; the failure is logged at
    /// most once a minute.
    pub fn append_transcript(&self, cid: u64, seq: u64, work: u64, kind: &str, at_ms: u64, data: &Value) {
        if !self.enabled() {
            return;
        }
        let (cid, seq, work, at_ms) = (cid as i64, seq as i64, work as i64, at_ms as i64);
        if let Err(e) = self.run(|c| c.execute(APPEND, &[&cid, &seq, &work, &kind, &at_ms, data])) {
            let mut logged = self.append_logged.lock().unwrap_or_else(PoisonError::into_inner);
            if due(*logged) {
                *logged = Some(Instant::now());
                eprintln!("db: transcript append failed: {e} (dropped; logged once a minute)");
            }
        }
    }

    /// The newest `limit` records of `cid`'s transcript, oldest first, as (seq, work, kind, at_ms, data).
    pub fn transcript(&self, cid: u64, limit: i64) -> Vec<(u64, u64, String, u64, Value)> {
        if !self.enabled() {
            return Vec::new();
        }
        let cid = cid as i64;
        let rows = self.run(|c| {
            c.query(
                "SELECT seq, work, kind, at_ms, data FROM transcript WHERE cid = $1 ORDER BY seq DESC LIMIT $2",
                &[&cid, &limit],
            )
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
        if !self.enabled() {
            return Vec::new();
        }
        match self.run(|c| c.query("SELECT DISTINCT cid FROM transcript", &[])) {
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
        if !self.enabled() {
            return;
        }
        let (cid, at_ms) = (infraction.cid as i64, infraction.at_ms as i64);
        let law = &infraction.law;
        let tool = &infraction.tool;
        let detail = &infraction.detail;
        if let Err(e) = self.run(|c| c.execute(INFRACTION_APPEND, &[&cid, law, &at_ms, tool, detail])) {
            let mut logged = self.infraction_logged.lock().unwrap_or_else(PoisonError::into_inner);
            if due(*logged) {
                *logged = Some(Instant::now());
                eprintln!("db: infraction append failed: {e} (dropped; logged once a minute)");
            }
        }
    }

    /// Infractions, newest first: one agent's (`Some(cid)`) or the plaza's.
    pub fn infractions(&self, cid: Option<u64>, limit: i64) -> Vec<Infraction> {
        if !self.enabled() {
            return Vec::new();
        }
        let rows = match cid {
            Some(cid) => {
                let cid = cid as i64;
                self.run(|c| {
                    c.query(
                        "SELECT cid, law, at_ms, tool, detail FROM infraction WHERE cid = $1 ORDER BY at_ms DESC, id DESC LIMIT $2",
                        &[&cid, &limit],
                    )
                })
            }
            None => self.run(|c| {
                c.query("SELECT cid, law, at_ms, tool, detail FROM infraction ORDER BY at_ms DESC, id DESC LIMIT $1", &[&limit])
            }),
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

/// At start: restore every component that Postgres holds; a component Postgres does not
/// hold yet keeps what its own files loaded (the first run imports them). Logs each outcome.
pub fn restore_all(app: &App) {
    if !app.db.enabled() {
        eprintln!("db: no APZ_DATABASE_URL; state stays in files");
        return;
    }
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
    let db = Arc::clone(&app.db);
    app.laws.set_sink(Box::new(move |cid: u64, v: &crate::laws::Violation, tool: &str| {
        db.append_infraction(&Infraction {
            cid,
            law: v.law.clone(),
            at_ms: v.at_ms,
            tool: tool.to_string(),
            detail: v.detail.clone(),
        })
    }));
}

/// The writer: every two seconds, store each component whose snapshot changed.
pub fn spawn_writer(app: Arc<App>) {
    if !app.db.enabled() {
        return;
    }
    let spawned = std::thread::Builder::new().name("db-writer".into()).spawn(move || {
        let mut logged: BTreeMap<&str, Instant> = BTreeMap::new();
        loop {
            std::thread::sleep(Duration::from_secs(2));
            for &name in COMPONENTS {
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
        }
    });
    if let Err(e) = spawned {
        eprintln!("db: writer thread not started: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs only with APZ_TEST_DATABASE_URL set; uses a test-only component and a unique cid.
    #[test]
    fn db_round_trip() {
        let url = match std::env::var("APZ_TEST_DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("db: APZ_TEST_DATABASE_URL not set; skipping");
                return;
            }
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

        let cid_param = cid as i64;
        db.run(|c| c.execute("DELETE FROM transcript WHERE cid = $1", &[&cid_param])).unwrap();
        db.run(|c| c.execute("DELETE FROM plaza_state WHERE component = $1", &[&component])).unwrap();
    }
}
