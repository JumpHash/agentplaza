//! The stages through the runner's local mode (PLAN 22.8): a topology declaring a data stage (SQLite files) and a cache
//! stage runs end to end on this machine. `apz-topo-run --local --stages` starts the cache service (`apz-topo-cache`, a
//! process on 127.0.0.1), waits for it to listen, and runs `apz-stage` over the repo, which prints the stages' numbers;
//! the test judges those numbers as the hub does. One run passes everything. The other has a broken migration (it drops
//! a column the new code reads) and a cache that does not invalidate on write (the seeded defect); its judgments fail
//! exactly those two items. The stage's backup is `cp`, so the file runs on Unix only.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use apz_topology::compare::Metric;
use apz_topology::plan::plan;
use apz_topology::spec;
use apz_topology::stage::{self, Judgment, Stages};

const CACHE: &str = env!("CARGO_BIN_EXE_apz-topo-cache");
const RUNNER: &str = env!("CARGO_BIN_EXE_apz-topo-run");

const PREVIOUS: &str = "CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL);
INSERT INTO tasks (id, title) VALUES (1, 'write'), (2, 'test'), (3, 'ship');";

const ADD_DUE: &str = "ALTER TABLE tasks ADD COLUMN due TEXT;";

/// The seeded break: the new code still reads `title`, and this migration drops it.
const DROP_TITLE: &str = "ALTER TABLE tasks DROP COLUMN title;
ALTER TABLE tasks ADD COLUMN due TEXT;";

/// The topology: one cache service (its port and its invalidation given), the data stage and the cache stage.
fn topology(port: u16, invalidate: bool) -> String {
    format!(
        r#"
name = "staged-local"

[[service]]
name = "api"
artifact = "cache"
ports = [{port}]
env = {{ APZ_CACHE_INVALIDATE = "{invalidate}" }}

[data]
previous = ["db/previous.sql"]
migrations = ["db/0002_due.sql"]
read = [{{ id = "list", sql = "SELECT id, title, due FROM tasks ORDER BY id", min_rows = 3 }}]
invariant = [{{ id = "rows", sql = "SELECT COUNT(*) FROM tasks", equals = 3 }}]
backup = {{ command = ["cp", "{{db}}", "{{out}}"], restore = ["cp", "{{backup}}", "{{db}}"] }}

[cache]
service = "api"
read = "/items/{{key}}"
write = "/items/{{key}}"
origin = "/origin/requests"
keys = 50
requests = 300
hit_floor_percent = 50
invalidation_within_ms = 500
stampede_clients = 20
stampede_origin_max = 1
"#,
        invalidate = if invalidate { "1" } else { "0" },
    )
}

/// Runs the topology through `apz-topo-run --local --stages` over a repo whose migration is `migration`, and judges
/// the numbers `apz-stage` printed.
fn run_staged(name: &str, port: u16, invalidate: bool, migration: &str) -> Vec<Judgment> {
    let dir = tempfile::tempdir().expect("a directory");
    let root = dir.path().join("repo");
    let work = dir.path().join("work");
    write(&root.join("db/previous.sql"), PREVIOUS);
    write(&root.join("db/0002_due.sql"), migration);
    let parsed = spec::parse(&topology(port, invalidate)).expect("the topology parses");
    let artifacts = BTreeMap::from([("cache".to_string(), CACHE.to_string())]);
    let planned = plan(&parsed, &artifacts).expect("the plan");
    let plan_path = dir.path().join(format!("{name}.plan.json"));
    fs::write(
        &plan_path,
        serde_json::to_string(&planned).expect("the plan as JSON"),
    )
    .expect("the plan file");

    let out = Command::new(RUNNER)
        .arg(&plan_path)
        .arg("--local")
        .arg("--stages")
        .arg(&root)
        .arg("--stage-work")
        .arg(&work)
        .output()
        .expect("apz-topo-run starts");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{name}: {stderr}");
    let values = metrics_of(&String::from_utf8_lossy(&out.stdout));
    stage::judge(&Stages::of(&parsed), &values)
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    fs::write(path, text).expect("a file");
}

/// The numbers `apz-stage` printed: a JSON object of metric names to values.
fn metrics_of(stdout: &str) -> Vec<Metric> {
    let object: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("stdout {stdout:?}: {e}"));
    object
        .into_iter()
        .map(|(name, value)| Metric {
            name,
            value: value.as_f64().expect("a number"),
        })
        .collect()
}

fn failing(judged: &[Judgment]) -> Vec<&str> {
    judged
        .iter()
        .filter(|j| !j.passed)
        .map(|j| j.id.as_str())
        .collect()
}

#[test]
fn a_staged_run_with_good_data_and_an_invalidating_cache_passes_every_judgment() {
    let judged = run_staged("good", 18190, true, ADD_DUE);
    assert!(failing(&judged).is_empty(), "{judged:?}");
    // Every stage the topology declares was measured (none is `not run`).
    assert!(judged.iter().all(|j| j.measured), "{judged:?}");
    for id in [
        "read:list",
        "invariant:rows",
        "restore",
        "profile",
        "invalidation",
        "stampede",
    ] {
        assert!(
            judged.iter().any(|j| j.id == id),
            "{id} is judged: {judged:?}"
        );
    }
}

#[test]
fn a_staged_run_with_a_broken_migration_and_a_non_invalidating_cache_fails_exactly_those_judgments()
{
    let judged = run_staged("seeded", 18191, false, DROP_TITLE);
    let failed = failing(&judged);
    // The read the new code runs fails: the migration dropped the column it reads.
    assert!(failed.contains(&"read:list"), "{judged:?}");
    // A write is not read back: the cache kept the old value.
    assert!(failed.contains(&"invalidation"), "{judged:?}");
    // The rest still passes: the migration applied, the cache served hits, the stampede coalesced.
    for id in ["migrations", "profile", "stampede", "previous"] {
        assert!(!failed.contains(&id), "{id} should pass: {judged:?}");
    }
}
