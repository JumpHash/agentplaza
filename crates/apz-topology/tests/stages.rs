//! The data, cache and messaging stages (PLAN 22.8), each judged by its own numbers: a seeded broken migration fails and a
//! good one passes; a backup restores, and a corrupted restore fails; a concurrent-writer invariant breach is caught; a
//! cache without invalidation fails its bound and with it passes; a stampede without coalescing exceeds the origin
//! bound and with it passes; a consumer that drops or repeats messages fails and a correct one passes.

use std::fs;
use std::path::Path;
use std::time::Duration;

use apz_topology::cache::{self, CacheStage, Memory};
use apz_topology::compare::Metric;
use apz_topology::data::{self, Backup, DataStage, Invariant, Read, Writers};
use apz_topology::messaging::{self, MessagingStage, Ordering};
use apz_topology::spec;
use apz_topology::stage::{self, Judgment};
use rusqlite::Connection;

/// Writes `files` (repo path, text) under `root`.
fn repo(root: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let full = root.join(path);
        fs::create_dir_all(full.parent().expect("a parent")).expect("a directory");
        fs::write(full, text).expect("a file");
    }
}

/// The previous release: a tasks table with three rows.
const PREVIOUS: &str = "CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL);
INSERT INTO tasks (id, title) VALUES (1, 'write'), (2, 'test'), (3, 'ship');";

/// The migration the new release ships: a due date column.
const ADD_DUE: &str = "ALTER TABLE tasks ADD COLUMN due TEXT;";

/// A migration that drops a column the new code still reads (the seeded break).
const DROP_TITLE: &str = "ALTER TABLE tasks DROP COLUMN title;
ALTER TABLE tasks ADD COLUMN due TEXT;";

fn read(id: &str, sql: &str, min_rows: u64) -> Read {
    Read {
        id: id.into(),
        sql: sql.into(),
        min_rows,
    }
}

fn invariant(id: &str, sql: &str, equals: i64, per_write: bool) -> Invariant {
    Invariant {
        id: id.into(),
        sql: sql.into(),
        equals,
        per_write,
    }
}

fn cp_backup() -> Option<Backup> {
    Some(Backup {
        command: vec!["cp".into(), "{db}".into(), "{out}".into()],
        restore: vec!["cp".into(), "{backup}".into(), "{db}".into()],
    })
}

/// A data stage over the tasks table: the previous schema and seed, `ADD_DUE` as the migration, and a down migration.
fn tasks_stage() -> DataStage {
    DataStage {
        previous: vec!["db/previous.sql".into()],
        migrations: vec!["db/0002_due.sql".into()],
        down: vec!["db/0002_due.down.sql".into()],
        reads: vec![read(
            "list",
            "SELECT id, title, due FROM tasks ORDER BY id",
            3,
        )],
        down_reads: vec![read("list-before", "SELECT id, title FROM tasks", 3)],
        writers: None,
        invariants: vec![
            invariant("rows", "SELECT COUNT(*) FROM tasks", 3, false),
            invariant(
                "unique",
                "SELECT COUNT(*) - COUNT(DISTINCT id) FROM tasks",
                0,
                false,
            ),
        ],
        backup: cp_backup(),
    }
}

fn tasks_repo(migration: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("a directory");
    repo(
        root.path(),
        &[
            ("db/previous.sql", PREVIOUS),
            ("db/0002_due.sql", migration),
            ("db/0002_due.down.sql", "ALTER TABLE tasks DROP COLUMN due;"),
        ],
    );
    root
}

/// The judged items that did not pass, by id.
fn failing(judged: &[(String, bool, String, bool)]) -> Vec<&str> {
    judged
        .iter()
        .filter(|(_, passed, _, _)| !passed)
        .map(|(id, _, _, _)| id.as_str())
        .collect()
}

fn judge_data(stage: &DataStage, metrics: &[Metric]) -> Vec<(String, bool, String, bool)> {
    data::judge(stage, metrics)
}

#[test]
fn a_good_migration_passes_its_reads_invariants_and_round_trip() {
    let root = tasks_repo(ADD_DUE);
    let work = tempfile::tempdir().expect("a directory");
    let stage = tasks_stage();
    let run = data::run(&stage, root.path(), work.path());
    assert!(run.notes.is_empty(), "{:?}", run.notes);
    let judged = judge_data(&stage, &run.metrics);
    assert!(failing(&judged).is_empty(), "{judged:?}");
    // The reads and invariants ran on the restored database too.
    assert!(
        judged
            .iter()
            .any(|(id, passed, _, _)| id == "restore-read:list" && *passed)
    );
    assert!(
        judged
            .iter()
            .any(|(id, passed, _, _)| id == "down-read:list-before" && *passed)
    );
}

#[test]
fn a_seeded_broken_migration_fails_the_read_the_new_code_runs() {
    let root = tasks_repo(DROP_TITLE);
    let work = tempfile::tempdir().expect("a directory");
    let stage = tasks_stage();
    let run = data::run(&stage, root.path(), work.path());
    let judged = judge_data(&stage, &run.metrics);
    let failed = failing(&judged);
    assert!(failed.contains(&"read:list"), "{failed:?}");
    // The migration itself applied: the failure is the read's, and the note says why.
    assert!(!failed.contains(&"migrations"), "{failed:?}");
    assert!(
        run.notes.iter().any(|n| n.contains("no such column")),
        "{:?}",
        run.notes
    );
    let detail = &judged
        .iter()
        .find(|(id, _, _, _)| id == "read:list")
        .expect("judged")
        .2;
    assert!(detail.contains("-1") || detail.contains("no"), "{detail}");
}

#[test]
fn a_migration_that_does_not_apply_fails_the_migrations() {
    let root = tasks_repo("ALTER TABLE tasks ADD COLUMN;");
    let work = tempfile::tempdir().expect("a directory");
    let stage = tasks_stage();
    let run = data::run(&stage, root.path(), work.path());
    let judged = judge_data(&stage, &run.metrics);
    assert!(failing(&judged).contains(&"migrations"), "{judged:?}");
}

#[test]
fn a_backup_restores_into_a_fresh_database_with_the_declared_checks_passing() {
    let root = tasks_repo(ADD_DUE);
    let work = tempfile::tempdir().expect("a directory");
    let stage = tasks_stage();
    let run = data::run(&stage, root.path(), work.path());
    let judged = judge_data(&stage, &run.metrics);
    assert!(
        judged
            .iter()
            .any(|(id, passed, _, _)| id == "restore" && *passed),
        "{judged:?}"
    );
    assert_eq!(value_of(&run.metrics, "data.restore.checksum_ok"), 1.0);
    // The restored file is a database of its own.
    let restored =
        Connection::open(work.path().join(data::RESTORED_FILE)).expect("the restored database");
    let rows: i64 = restored
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .expect("the rows");
    assert_eq!(rows, 3);
}

#[test]
fn a_backup_corrupted_in_transit_is_refused_at_restore() {
    let root = tasks_repo(ADD_DUE);
    let work = tempfile::tempdir().expect("a directory");
    let stage = tasks_stage();
    // The backup is cut short between the backup and the restore: its checksum no longer matches.
    let run = data::run_with(&stage, root.path(), work.path(), &|dir| {
        let backup = dir.join(data::BACKUP_FILE);
        let bytes = fs::read(&backup).expect("the backup");
        fs::write(&backup, &bytes[..bytes.len() / 2]).expect("cut");
    });
    let judged = judge_data(&stage, &run.metrics);
    assert_eq!(value_of(&run.metrics, "data.restore.checksum_ok"), 0.0);
    assert!(failing(&judged).contains(&"restore"), "{judged:?}");
    assert!(
        run.notes.iter().any(|n| n.contains("checksum")),
        "{:?}",
        run.notes
    );
}

#[test]
fn a_restore_that_yields_a_corrupt_database_fails_its_checks() {
    let root = tasks_repo(ADD_DUE);
    let work = tempfile::tempdir().expect("a directory");
    let mut stage = tasks_stage();
    // The backup is intact, so the checksum passes; the restore command writes only its first 64 bytes into the fresh
    // database, which is no database, so the declared reads and invariants fail on what came back.
    stage.backup = Some(Backup {
        command: vec!["cp".into(), "{db}".into(), "{out}".into()],
        restore: vec![
            "dd".into(),
            "if={backup}".into(),
            "of={db}".into(),
            "bs=64".into(),
            "count=1".into(),
        ],
    });
    let run = data::run(&stage, root.path(), work.path());
    let judged = judge_data(&stage, &run.metrics);
    assert_eq!(value_of(&run.metrics, "data.restore.checksum_ok"), 1.0);
    assert_eq!(
        value_of(&run.metrics, "data.restore.failed"),
        0.0,
        "{:?}",
        run.notes
    );
    let failed = failing(&judged);
    assert!(failed.contains(&"restore-read:list"), "{failed:?}");
    assert!(failed.contains(&"restore-invariant:rows"), "{failed:?}");
}

/// The writers' schema: a counter and a ledger of every write.
const LEDGER: &str = "CREATE TABLE counter (id INTEGER PRIMARY KEY, value INTEGER NOT NULL);
INSERT INTO counter (id, value) VALUES (1, 0);
CREATE TABLE ledger (writer INTEGER NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY (writer, seq));";

fn ledger_stage(writers: u32, ops: u32) -> DataStage {
    DataStage {
        previous: vec!["db/previous.sql".into()],
        migrations: vec![],
        down: vec![],
        reads: vec![],
        down_reads: vec![],
        writers: Some(Writers {
            count: writers,
            ops,
            statements: vec![
                "UPDATE counter SET value = value + 1 WHERE id = 1".into(),
                "INSERT INTO ledger (writer, seq) VALUES (?1, ?2)".into(),
            ],
        }),
        invariants: vec![
            invariant("counter", "SELECT value FROM counter", 1, true),
            invariant("ledger", "SELECT COUNT(*) FROM ledger", 1, true),
            invariant(
                "unique",
                "SELECT COUNT(*) - COUNT(DISTINCT writer || ':' || seq) FROM ledger",
                0,
                false,
            ),
        ],
        backup: None,
    }
}

#[test]
fn concurrent_writers_leave_the_declared_invariants_holding() {
    let root = tempfile::tempdir().expect("a directory");
    repo(root.path(), &[("db/previous.sql", LEDGER)]);
    let work = tempfile::tempdir().expect("a directory");
    let stage = ledger_stage(8, 40);
    let run = data::run(&stage, root.path(), work.path());
    assert_eq!(
        value_of(&run.metrics, "data.writers.errors"),
        0.0,
        "{:?}",
        run.notes
    );
    let judged = judge_data(&stage, &run.metrics);
    assert!(failing(&judged).is_empty(), "{judged:?}");
    assert_eq!(
        value_of(&run.metrics, "data.invariant.counter.value"),
        320.0
    );
}

#[test]
fn a_lost_update_between_concurrent_writers_breaches_the_invariant() {
    // A read-modify-write in application code, each statement in its own transaction: writers interleave between the
    // read and the write and lose updates.
    let work = tempfile::tempdir().expect("a directory");
    let db = work.path().join("app.db");
    let conn = Connection::open(&db).expect("a database");
    conn.execute_batch(LEDGER).expect("the schema");
    drop(conn);
    let racy = |conn: &Connection, writer: u32, seq: u32| -> rusqlite::Result<()> {
        let value: i64 =
            conn.query_row("SELECT value FROM counter WHERE id = 1", [], |r| r.get(0))?;
        std::thread::sleep(Duration::from_millis(1));
        conn.execute("UPDATE counter SET value = ?1 WHERE id = 1", [value + 1])?;
        conn.execute(
            "INSERT INTO ledger (writer, seq) VALUES (?1, ?2)",
            [i64::from(writer), i64::from(seq)],
        )?;
        Ok(())
    };
    let errors = data::drive(&db, 8, 20, &racy);
    assert_eq!(errors, 0);
    let after: i64 = Connection::open(&db)
        .expect("the database")
        .query_row("SELECT value FROM counter WHERE id = 1", [], |r| r.get(0))
        .expect("the counter");
    assert!(after < 160, "the racy writers kept every update: {after}");
    // The invariant the stage judges catches the breach.
    let stage = ledger_stage(8, 20);
    let values = vec![
        metric("data.writers.errors", 0.0),
        metric("data.invariant.counter.value", after as f64),
        metric("data.invariant.ledger.value", 160.0),
        metric("data.invariant.unique.value", 0.0),
    ];
    let judged = judge_data(&stage, &values);
    assert!(
        failing(&judged).contains(&"invariant:counter"),
        "{judged:?}"
    );
}

fn metric(name: &str, value: f64) -> Metric {
    Metric {
        name: name.into(),
        value,
    }
}

fn value_of(values: &[Metric], name: &str) -> f64 {
    values
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("{name} was not recorded"))
        .value
}

// ---- cache ----------------------------------------------------------------------------------------------------------

fn cache_stage(stampede_clients: u32, origin_max: u64) -> CacheStage {
    CacheStage {
        service: "api".into(),
        read: "/items/{key}".into(),
        write: "/items/{key}".into(),
        origin: "/origin/requests".into(),
        keys: 100,
        requests: 2000,
        hit_floor_percent: 80,
        invalidation_within_ms: 500,
        stampede_clients,
        stampede_origin_max: origin_max,
    }
}

/// A cache for the tests: a fast origin, so the profile runs in moments.
fn quick(invalidate: bool, coalesce: bool) -> Memory {
    let mut memory = Memory::new(invalidate, coalesce);
    memory.origin_latency = Duration::from_millis(2);
    memory
}

fn judge_cache(stage: &CacheStage, subject: &Memory) -> Vec<Judgment> {
    let values = cache::run(stage, subject);
    stage_judgments(stage, &values)
}

fn stage_judgments(stage: &CacheStage, values: &[Metric]) -> Vec<Judgment> {
    cache::judge(stage, values)
        .into_iter()
        .map(|(id, passed, detail, measured)| Judgment {
            stage: "cache",
            id,
            passed,
            detail,
            measured,
        })
        .collect()
}

fn failed_ids(judged: &[Judgment]) -> Vec<&str> {
    judged
        .iter()
        .filter(|j| !j.passed)
        .map(|j| j.id.as_str())
        .collect()
}

#[test]
fn a_cache_that_invalidates_and_coalesces_passes_its_bounds() {
    let stage = cache_stage(20, 1);
    let judged = judge_cache(&stage, &quick(true, true));
    assert!(failed_ids(&judged).is_empty(), "{judged:?}");
    assert!(judged.iter().all(|j| j.measured));
}

#[test]
fn a_cache_without_invalidation_fails_its_bound() {
    let stage = cache_stage(20, 1);
    let judged = judge_cache(&stage, &quick(false, true));
    let failed = failed_ids(&judged);
    assert!(failed.contains(&"invalidation"), "{judged:?}");
    let detail = &judged
        .iter()
        .find(|j| j.id == "invalidation")
        .expect("judged")
        .detail;
    assert!(detail.contains("old value"), "{detail}");
}

#[test]
fn a_cache_that_serves_nothing_fails_the_hit_floor() {
    let stage = cache_stage(20, 1);
    let mut memory = quick(true, true);
    memory.ttl = Duration::ZERO;
    let judged = judge_cache(&stage, &memory);
    assert!(failed_ids(&judged).contains(&"profile"), "{judged:?}");
}

#[test]
fn a_stampede_without_coalescing_exceeds_the_origin_bound() {
    let stage = cache_stage(20, 1);
    let judged = judge_cache(&stage, &quick(true, false));
    let failed = failed_ids(&judged);
    assert!(failed.contains(&"stampede"), "{judged:?}");
    let origin = value_of(
        &cache::run(&stage, &quick(true, false)),
        "cache.stampede.origin_requests",
    );
    assert!(origin > 1.0, "{origin}");
}

#[test]
fn a_stampede_with_coalescing_stays_within_the_origin_bound() {
    let stage = cache_stage(20, 1);
    let values = cache::run(&stage, &quick(true, true));
    assert_eq!(value_of(&values, "cache.stampede.origin_requests"), 1.0);
    assert_eq!(value_of(&values, "cache.stampede.errors"), 0.0);
}

#[test]
fn a_cache_stage_with_no_measurements_is_not_run() {
    let stage = cache_stage(20, 1);
    let judged = stage_judgments(&stage, &[]);
    assert!(
        judged.iter().all(|j| !j.passed && !j.measured),
        "{judged:?}"
    );
}

// ---- messaging ------------------------------------------------------------------------------------------------------

fn messaging_stage() -> MessagingStage {
    MessagingStage {
        topic: "orders".into(),
        messages: 200,
        keys: 10,
        ordering: Ordering::PerKey,
        duplicate_every: 7,
        transient_every: 5,
        poison_every: 50,
        max_attempts: 3,
        max_duplicates: 0,
    }
}

fn judge_messages(
    stage: &MessagingStage,
    run: &messaging::Run,
) -> Vec<(String, bool, String, bool)> {
    messaging::judge(stage, &run.metrics)
}

#[test]
fn a_correct_consumer_passes_with_duplicates_retries_and_dead_letters_in_the_broker() {
    let stage = messaging_stage();
    let mut consumer = messaging::idempotent();
    let run = messaging::run(&stage, &mut consumer);
    let judged = judge_messages(&stage, &run);
    assert!(failing(&judged).is_empty(), "{judged:?}");
    // The faults were there: duplicates were redelivered, retries happened, poison went to the dead letters.
    assert!(value_of(&run.metrics, "messaging.deliveries") > 200.0);
    assert!(value_of(&run.metrics, "messaging.retries") > 0.0);
    assert_eq!(value_of(&run.metrics, "messaging.dead_letters"), 4.0);
    assert_eq!(value_of(&run.metrics, "messaging.duplicates"), 0.0);
    assert!(run.records.iter().any(|r| r.status == "dead-letter"));
}

#[test]
fn a_consumer_that_drops_messages_fails_delivery() {
    let stage = messaging_stage();
    let mut dropping = |d: &messaging::Delivery, effects: &mut Vec<u64>| -> Result<(), String> {
        if !d.id.is_multiple_of(7) {
            effects.push(d.id);
        }
        Ok(())
    };
    let run = messaging::run(&stage, &mut dropping);
    let judged = judge_messages(&stage, &run);
    assert!(failing(&judged).contains(&"delivery"), "{judged:?}");
    assert!(value_of(&run.metrics, "messaging.lost") > 0.0);
}

#[test]
fn a_consumer_that_repeats_effects_fails_duplicates_and_ordering() {
    let stage = messaging_stage();
    let mut repeating = |d: &messaging::Delivery, effects: &mut Vec<u64>| -> Result<(), String> {
        effects.push(d.id);
        Ok(())
    };
    let run = messaging::run(&stage, &mut repeating);
    let judged = judge_messages(&stage, &run);
    let failed = failing(&judged);
    assert!(failed.contains(&"duplicates"), "{judged:?}");
    assert!(failed.contains(&"ordering"), "{judged:?}");
    assert!(value_of(&run.metrics, "messaging.duplicates") > 0.0);
}

#[test]
fn a_topic_that_declares_no_order_is_not_judged_on_order() {
    let mut stage = messaging_stage();
    stage.ordering = Ordering::None;
    let mut repeating = |d: &messaging::Delivery, effects: &mut Vec<u64>| -> Result<(), String> {
        effects.push(d.id);
        Ok(())
    };
    let run = messaging::run(&stage, &mut repeating);
    let judged = judge_messages(&stage, &run);
    assert!(!failing(&judged).contains(&"ordering"), "{judged:?}");
}

// ---- the topology file declares the stages --------------------------------------------------------------------------

const STAGED: &str = r#"
name = "staged"

[[service]]
name = "api"
artifact = "api-linux-x86_64"
ports = [8080]

[cache]
service = "api"
read = "/items/{key}"
write = "/items/{key}"
origin = "/origin/requests"
keys = 100
requests = 2000
hit_floor_percent = 80
invalidation_within_ms = 500
stampede_clients = 20
stampede_origin_max = 1

[data]
previous = ["db/previous.sql"]
migrations = ["db/0002_due.sql"]
read = [{ id = "list", sql = "SELECT id, title, due FROM tasks", min_rows = 3 }]
invariant = [{ id = "rows", sql = "SELECT COUNT(*) FROM tasks", equals = 3 }]
backup = { command = ["cp", "{db}", "{out}"], restore = ["cp", "{backup}", "{db}"] }

[messaging]
topic = "orders"
messages = 200
keys = 10
ordering = "per-key"
duplicate_every = 7
transient_every = 5
poison_every = 50
max_attempts = 3
"#;

#[test]
fn a_topology_declares_its_stages_and_checks_them() {
    let topology = spec::parse(STAGED).expect("the topology parses");
    assert!(topology.data.is_some() && topology.cache.is_some() && topology.messaging.is_some());
    assert_eq!(
        stage::declared(&topology),
        vec!["data", "cache", "messaging"]
    );
    let names = stage::names(&topology);
    assert!(
        names.contains(&"data.read.list.rows".to_string()),
        "{names:?}"
    );
    assert!(names.contains(&"cache.stampede.origin_requests".to_string()));
    assert!(names.contains(&"messaging.lost".to_string()));
}

#[test]
fn a_cache_stage_naming_an_undeclared_service_is_refused() {
    let text = STAGED.replace("service = \"api\"", "service = \"web\"");
    let err = spec::parse(&text).expect_err("refused");
    assert!(err.contains("web"), "{err}");
}

#[test]
fn a_stage_with_a_bad_command_placeholder_is_refused() {
    let text = STAGED.replace("\"{out}\"]", "\"{target}\"]");
    let err = spec::parse(&text).expect_err("refused");
    assert!(err.contains("{target}"), "{err}");
}

#[test]
fn stage_numbers_post_as_gauges_the_hub_reads_back() {
    let stage = messaging_stage();
    let mut consumer = messaging::idempotent();
    let run = messaging::run(&stage, &mut consumer);
    let body = stage::otlp("apz-stage", &run.metrics, 1_000);
    let gauges = body["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
        .as_array()
        .expect("gauges");
    assert_eq!(gauges.len(), run.metrics.len());
    assert_eq!(gauges[0]["name"], "messaging.produced");
    assert_eq!(gauges[0]["gauge"]["dataPoints"][0]["asDouble"], 200.0);
}
