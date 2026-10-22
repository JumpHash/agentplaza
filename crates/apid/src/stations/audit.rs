//! The station audit (`station_audit` in `plaza.db`): one entry per owner action on a workstation, per lease change,
//! per takeover and per work-folder sync change (docs/design/workstations.md). An entry is a typed [`StationAudit`],
//! one variant per kind, flattened into the table: its kind is a column, and each variant's fields are columns of
//! their own. What was written as it was performed (an operation's summary, a daemon's reply) is content, kept as
//! written; the hub never reads it back as state.
//!
//! Entries are written through the store's writer and the caller waits for the commit: an action whose entry cannot
//! be written is answered with the failure (`ops::audit`). The routes read them newest first (api.rs).

use std::sync::OnceLock;

use apz_station::{StationSettings, Takeover};
use apz_store::rusqlite::{Connection, params};
use apz_store::table::{Changes, Loaded, Row, Value as SqlValue};
use apz_store::{Db, IndexDef, RowSet, StoreError, Table, TableDef};
use serde::Serialize;
use serde_json::Value;

use super::leases::Kind;
use crate::settings::RecordingDefaults;
use crate::store::sql;

/// What one station audit entry records, by kind. The JSON of an entry is its row's fields and this kind's fields.
#[derive(Debug, Clone, PartialEq, Serialize, Table)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StationAudit {
    /// A join code was issued (`code` is its id, not the secret).
    CodeIssue {
        code: String,
        label: String,
        expires_ms: u64,
    },
    CodeRevoke {
        code: String,
    },
    /// A workstation enrolled with a join code.
    Join {
        name: String,
        code: String,
        address: String,
        hostname: String,
    },
    /// An enrollment from the hub's own machine, refused.
    JoinRefused {
        address: String,
    },
    /// A lease ended: revoked with its station (no session), or its daemon's session ended (`session`).
    LeaseEnded {
        lease: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session: Option<u64>,
        reason: String,
    },
    Revoke {
        name: String,
    },
    Drain,
    Undrain,
    Rename {
        name: String,
    },
    /// The station's settings, as stored.
    Settings {
        #[table(flatten)]
        settings: StationSettings,
    },
    /// A lease was opened. `lease_kind` is the lease's kind ("shared" or "pixel"); the row's kind is `lease_open`.
    LeaseOpen {
        lease: String,
        lease_kind: Kind,
        task: Option<String>,
    },
    LeaseReleased {
        lease: String,
        session: Option<u64>,
        close_error: Option<String>,
        reason: String,
    },
    LeaseGranted {
        lease: String,
        session: u64,
    },
    LeaseFailed {
        lease: String,
        reason: String,
    },
    Freeze,
    Unfreeze,
    FreezeFailed {
        reason: String,
    },
    /// The owner's input on a lease's session (`lease`), or on a session through the takeover socket (`source`
    /// and `action`).
    OwnerInput {
        #[serde(skip_serializing_if = "Option::is_none")]
        lease: Option<String>,
        session: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        action: Option<String>,
    },
    /// An agent's operation on a station: its summary as performed (content), the lease it ran under, and the answer.
    AgentOp {
        #[table(content)]
        op: Value,
        lease: Option<String>,
        ok: bool,
        error: Option<String>,
    },
    /// The owner took over a session: from the takeover socket (`source` "control") or from the daemon's input.
    Takeover {
        by: String,
        source: String,
        policy: Takeover,
        session: u64,
    },
    /// The owner gave a session back: by the owner (`reason` "owner") or after a quiet spell ("quiet").
    GiveBack {
        by: String,
        reason: String,
        session: u64,
    },
    /// What a lease cost its task for the time it held the station.
    LeaseCharge {
        lease: String,
        task: String,
        rate_per_hour: u64,
        active_ms: u64,
        wanted: i64,
        charged: i64,
        error: Option<String>,
    },
    RecordingDelete {
        recording: String,
        lease: String,
        frames: u64,
    },
    /// The hub's recording defaults, as set.
    RecordingDefaults {
        #[table(flatten)]
        defaults: RecordingDefaults,
    },
    SyncConflict {
        path: String,
        copy: String,
    },
    SyncPaused {
        reason: String,
        deletions: usize,
    },
    SyncResumed {
        by: String,
    },
    SyncResume {
        apply_deletions: bool,
        reason: String,
        deletions: usize,
    },
    SyncPut {
        path: String,
        hash: String,
        size: usize,
    },
    SyncDelete {
        path: String,
    },
    SyncDeliver {
        path: String,
        file: String,
    },
    VmReset {
        vm: String,
    },
    VmResetFailed {
        vm: String,
        reason: String,
    },
    VmCreate {
        vm: String,
        node: String,
        node_vm: String,
        os: String,
        code: String,
    },
    VmRemove {
        vm: String,
        node: String,
        node_vm: String,
        station: Option<String>,
    },
    VmSleep {
        vm: String,
        node: String,
        node_vm: String,
    },
    VmWake {
        vm: String,
        node: String,
        node_vm: String,
    },
    /// An owner action from the IDE on a workstation's work folder (its files, runs and terminals).
    OwnerIde {
        #[serde(flatten)]
        #[table(flatten)]
        action: OwnerIdeAction,
    },
}

/// The IDE action an `owner_ide` entry records. `hash_after` and `run` are the daemon's replies, kept for display.
#[derive(Debug, Clone, PartialEq, Serialize, Table)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum OwnerIdeAction {
    Write {
        path: String,
        hash_before: Option<String>,
        #[table(content)]
        hash_after: Value,
    },
    Run {
        #[table(content)]
        run: Value,
        command: String,
        timeout_s: Option<u64>,
    },
    RunStop {
        run: String,
    },
    TermOpen {
        term: String,
        cols: u16,
        rows: u16,
        cwd: Option<String>,
    },
    TermKill {
        term: String,
    },
}

/// One station audit entry (a row of `station_audit`).
#[derive(Debug, Clone, PartialEq, Serialize, Table)]
#[table(name = "station_audit")]
pub struct StationAuditRow {
    /// The entry's place in the log, assigned when it is written.
    #[serde(skip)]
    #[table(key)]
    pub seq: u64,
    pub at_ms: u64,
    /// Who acted: "owner", "station", "pass" or "agent:<cid>".
    pub actor: String,
    /// The workstation the entry is about (none for the registry as a whole).
    pub station: Option<String>,
    /// The agent involved (a CID; 0 for the owner's own client).
    pub agent: Option<u64>,
    #[serde(flatten)]
    #[table(flatten)]
    pub event: StationAudit,
}

/// The table `station_audit`: keyed by seq, with an index on (station, at_ms) for one station's page.
pub fn def() -> &'static TableDef {
    static DEF: OnceLock<TableDef> = OnceLock::new();
    DEF.get_or_init(|| {
        let mut def = StationAuditRow::root();
        def.indexes.push(IndexDef {
            columns: vec!["station".to_string(), "at_ms".to_string()],
            unique: false,
        });
        def
    })
}

/// Writes one entry about `station` (None: the registry) for `agent` (when one is involved), as `actor`, and waits
/// for its commit. The entry takes the next seq in the log.
pub fn write(
    db: &Db,
    station: Option<&str>,
    agent: Option<u64>,
    actor: &str,
    event: StationAudit,
) -> Result<(), StoreError> {
    let row = StationAuditRow {
        seq: 0,
        at_ms: apz_plaza::now_ms(),
        actor: actor.to_string(),
        station: station.map(str::to_string),
        agent,
        event,
    };
    db.write(move |conn| {
        let mut row = row;
        row.seq = next_seq(conn)?;
        save(conn, def(), &row, row.seq)
    })
    .map(|_| ())
}

/// The newest `limit` entries, newest first: those of `station` alone when it is given.
pub fn page(
    db: &Db,
    station: Option<&str>,
    limit: i64,
) -> Result<Vec<StationAuditRow>, StoreError> {
    db.read(|conn| match station {
        Some(station) => read_rows(
            conn,
            sql::STATION_AUDIT_PAGE_BY_STATION,
            def(),
            params![station, limit],
        ),
        None => read_rows(conn, sql::STATION_AUDIT_PAGE, def(), params![limit]),
    })
}

fn next_seq(conn: &Connection) -> Result<u64, StoreError> {
    let last: i64 = conn
        .prepare_cached(sql::STATION_AUDIT_LAST_SEQ)?
        .query_row([], |r| r.get(0))?;
    Ok(u64::try_from(last).unwrap_or(0) + 1)
}

/// Writes one row of a stream table, inserted or replaced by its key. `pos` is the row's place in its stream (the
/// table's `pos` column; a log passes its seq).
pub fn save<T: Table>(
    conn: &Connection,
    def: &TableDef,
    value: &T,
    pos: u64,
) -> Result<(), StoreError> {
    let pos = i64::try_from(pos).unwrap_or(i64::MAX);
    let mut rows = RowSet::new();
    value.put(def, vec![SqlValue::Integer(pos)], &mut rows)?;
    let mut changes = Changes::default();
    changes.upserts = rows
        .tables
        .remove(&def.name)
        .unwrap_or_default()
        .into_iter()
        .map(|row| (def.name.clone(), row))
        .collect();
    apz_store::table::apply(conn, def, &changes)
}

/// Reads the rows a query selects (every column of `def`'s table, in its order) as `T`.
pub fn read_rows<T: Table>(
    conn: &Connection,
    query: &str,
    def: &TableDef,
    params: impl apz_store::rusqlite::Params,
) -> Result<Vec<T>, StoreError> {
    let width = def.key.len() + def.columns.len();
    let mut stmt = conn.prepare_cached(query)?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let mut values = Vec::with_capacity(width);
        for i in 0..width {
            values.push(r.get::<_, SqlValue>(i)?);
        }
        let columns = values.split_off(def.key.len());
        let row = Row {
            key: values,
            columns,
        };
        out.push(T::take(def, &row, &Loaded::default())?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(station: Option<&str>, agent: Option<u64>, event: StationAudit) -> StationAuditRow {
        StationAuditRow {
            seq: 0,
            at_ms: 0,
            actor: "owner".into(),
            station: station.map(str::to_string),
            agent,
            event,
        }
    }

    /// One entry of every kind, with the fields filled the way the call sites fill them.
    fn every_kind() -> Vec<StationAudit> {
        let settings = StationSettings {
            work_folder: "~/Work".into(),
            ..StationSettings::default()
        };
        vec![
            StationAudit::CodeIssue {
                code: "c1".into(),
                label: "lab".into(),
                expires_ms: 99,
            },
            StationAudit::CodeRevoke { code: "c1".into() },
            StationAudit::Join {
                name: "w1".into(),
                code: "c1".into(),
                address: "10.0.0.2".into(),
                hostname: "mac".into(),
            },
            StationAudit::JoinRefused {
                address: "10.0.0.1".into(),
            },
            StationAudit::LeaseEnded {
                lease: "l1".into(),
                session: None,
                reason: "revoked".into(),
            },
            StationAudit::LeaseEnded {
                lease: "l2".into(),
                session: Some(4),
                reason: "closed".into(),
            },
            StationAudit::Revoke { name: "w1".into() },
            StationAudit::Drain,
            StationAudit::Undrain,
            StationAudit::Rename { name: "w2".into() },
            StationAudit::Settings { settings },
            StationAudit::LeaseOpen {
                lease: "l3".into(),
                lease_kind: Kind::Pixel,
                task: Some("t1".into()),
            },
            StationAudit::LeaseReleased {
                lease: "l3".into(),
                session: None,
                close_error: Some("gone".into()),
                reason: "released".into(),
            },
            StationAudit::LeaseGranted {
                lease: "l4".into(),
                session: 7,
            },
            StationAudit::LeaseFailed {
                lease: "l5".into(),
                reason: "no session".into(),
            },
            StationAudit::Freeze,
            StationAudit::Unfreeze,
            StationAudit::FreezeFailed {
                reason: "busy".into(),
            },
            StationAudit::OwnerInput {
                lease: Some("l4".into()),
                session: 7,
                source: None,
                action: None,
            },
            StationAudit::OwnerInput {
                lease: None,
                session: 8,
                source: Some("control".into()),
                action: Some("click".into()),
            },
            StationAudit::AgentOp {
                op: json!({"op": "input", "session": 7}),
                lease: Some("l4".into()),
                ok: true,
                error: None,
            },
            StationAudit::Takeover {
                by: "owner".into(),
                source: "control".into(),
                policy: Takeover::ReleaseLease,
                session: 7,
            },
            StationAudit::GiveBack {
                by: "owner".into(),
                reason: "quiet".into(),
                session: 7,
            },
            StationAudit::LeaseCharge {
                lease: "l4".into(),
                task: "t1".into(),
                rate_per_hour: 60,
                active_ms: 120_000,
                wanted: 2,
                charged: 2,
                error: None,
            },
            StationAudit::RecordingDelete {
                recording: "r1".into(),
                lease: "l4".into(),
                frames: 12,
            },
            StationAudit::RecordingDefaults {
                defaults: RecordingDefaults {
                    record: true,
                    retention_days: 30,
                    record_fps: 2,
                    record_takeovers: false,
                },
            },
            StationAudit::SyncConflict {
                path: "plan.md".into(),
                copy: "plan (conflict).md".into(),
            },
            StationAudit::SyncPaused {
                reason: "deletions".into(),
                deletions: 3,
            },
            StationAudit::SyncResumed { by: "pass".into() },
            StationAudit::SyncResume {
                apply_deletions: true,
                reason: "owner".into(),
                deletions: 3,
            },
            StationAudit::SyncPut {
                path: "a.txt".into(),
                hash: "ab".into(),
                size: 12,
            },
            StationAudit::SyncDelete {
                path: "a.txt".into(),
            },
            StationAudit::SyncDeliver {
                path: "out.pdf".into(),
                file: "d1".into(),
            },
            StationAudit::VmReset { vm: "vm1".into() },
            StationAudit::VmResetFailed {
                vm: "vm1".into(),
                reason: "no snapshot".into(),
            },
            StationAudit::VmCreate {
                vm: "vm2".into(),
                node: "n1".into(),
                node_vm: "nv2".into(),
                os: "linux".into(),
                code: "c2".into(),
            },
            StationAudit::VmRemove {
                vm: "vm2".into(),
                node: "n1".into(),
                node_vm: "nv2".into(),
                station: None,
            },
            StationAudit::VmSleep {
                vm: "vm2".into(),
                node: "n1".into(),
                node_vm: "nv2".into(),
            },
            StationAudit::VmWake {
                vm: "vm2".into(),
                node: "n1".into(),
                node_vm: "nv2".into(),
            },
            StationAudit::OwnerIde {
                action: OwnerIdeAction::Write {
                    path: "a.md".into(),
                    hash_before: None,
                    hash_after: json!("ff"),
                },
            },
            StationAudit::OwnerIde {
                action: OwnerIdeAction::Run {
                    run: json!("run-1"),
                    command: "ls".into(),
                    timeout_s: Some(30),
                },
            },
            StationAudit::OwnerIde {
                action: OwnerIdeAction::RunStop {
                    run: "run-1".into(),
                },
            },
            StationAudit::OwnerIde {
                action: OwnerIdeAction::TermOpen {
                    term: "t1".into(),
                    cols: 80,
                    rows: 24,
                    cwd: None,
                },
            },
            StationAudit::OwnerIde {
                action: OwnerIdeAction::TermKill { term: "t1".into() },
            },
        ]
    }

    /// The hub's store in a fresh folder (every file migrated, the way the hub opens it).
    fn plaza(label: &str) -> (tempfile::TempDir, apz_store::Store) {
        let dir = tempfile::Builder::new().prefix(label).tempdir().unwrap();
        let store = crate::store::open(dir.path()).expect("the store opens");
        (dir, store)
    }

    #[test]
    fn every_kind_round_trips_through_the_table_with_its_json_unchanged() {
        let (_dir, store) = plaza("station-audit-kinds");
        let db = store.db(crate::store::PLAZA);
        let events = every_kind();
        let mut wanted = Vec::new();
        for (i, event) in events.iter().enumerate() {
            let station = (i % 2 == 0).then_some("w1");
            let agent = (i % 3 == 0).then_some(i as u64);
            write(db, station, agent, "owner", event.clone()).unwrap();
            wanted.push(serde_json::to_value(entry(station, agent, event.clone())).unwrap());
        }
        // Newest first: the reverse of the order they were written in.
        wanted.reverse();
        let back = page(db, None, 1000).unwrap();
        assert_eq!(back.len(), events.len(), "one row per entry");
        let mut kinds = std::collections::BTreeSet::new();
        for (row, want) in back.iter().zip(&wanted) {
            let mut got = serde_json::to_value(row).unwrap();
            let mut want = want.clone();
            got.as_object_mut().unwrap().remove("at_ms");
            want.as_object_mut().unwrap().remove("at_ms");
            kinds.insert(want["kind"].as_str().unwrap().to_string());
            assert_eq!(got, want, "the JSON of {}", want["kind"]);
        }
        assert_eq!(kinds.len(), 38, "every kind is covered: {kinds:?}");
    }

    #[test]
    fn a_lease_open_keeps_its_row_kind_and_its_lease_kind_apart() {
        let (_dir, store) = plaza("station-audit-lease");
        let db = store.db(crate::store::PLAZA);
        let open = StationAudit::LeaseOpen {
            lease: "l9".into(),
            lease_kind: Kind::Shared,
            task: None,
        };
        write(db, Some("w1"), Some(9), "owner", open).unwrap();
        let row = serde_json::to_value(&page(db, Some("w1"), 10).unwrap()[0]).unwrap();
        assert_eq!(row["kind"], "lease_open");
        assert_eq!(row["lease_kind"], "shared");
        assert_eq!(row["lease"], "l9");
        assert_eq!(row["agent"], 9);
    }

    #[test]
    fn a_station_page_is_newest_first_and_apart_per_station() {
        let (_dir, store) = plaza("station-audit-page");
        let db = store.db(crate::store::PLAZA);
        write(db, Some("w1"), None, "owner", StationAudit::Drain).unwrap();
        write(db, Some("w2"), None, "owner", StationAudit::Undrain).unwrap();
        write(
            db,
            None,
            None,
            "owner",
            StationAudit::CodeRevoke { code: "c".into() },
        )
        .unwrap();
        let kinds = |rows: Vec<StationAuditRow>| {
            rows.into_iter()
                .map(|r| serde_json::to_value(r).unwrap()["kind"].clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kinds(page(db, None, 10).unwrap()),
            vec![json!("code_revoke"), json!("undrain"), json!("drain")]
        );
        assert_eq!(
            kinds(page(db, Some("w1"), 10).unwrap()),
            vec![json!("drain")]
        );
        assert_eq!(kinds(page(db, Some("w1"), 0).unwrap()), Vec::<Value>::new());
        assert!(page(db, Some("w9"), 10).unwrap().is_empty());
    }

    #[test]
    fn an_owner_ide_write_keeps_the_route_json() {
        let (_dir, store) = plaza("station-audit-ide");
        let db = store.db(crate::store::PLAZA);
        let event = StationAudit::OwnerIde {
            action: OwnerIdeAction::Write {
                path: "notes.txt".into(),
                hash_before: Some("aa".into()),
                hash_after: json!("bb"),
            },
        };
        write(db, Some("w3"), None, "owner", event).unwrap();
        let row = serde_json::to_value(&page(db, Some("w3"), 1).unwrap()[0]).unwrap();
        let keys: std::collections::BTreeSet<&str> = row
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "actor",
                "agent",
                "at_ms",
                "action",
                "hash_after",
                "hash_before",
                "kind",
                "path",
                "station"
            ]
            .into_iter()
            .collect(),
            "{row}"
        );
        assert_eq!(row["kind"], "owner_ide");
        assert_eq!(row["action"], "write");
        assert_eq!(row["hash_before"], "aa");
        assert_eq!(row["hash_after"], "bb");
    }
}
