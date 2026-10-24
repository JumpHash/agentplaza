//! The data stage (PLAN 22.8): the previous release's schema and data, the new release's migrations, the reads its code
//! runs after them, a declared down round trip, concurrent writers and their invariants, and a backup restored into a
//! fresh service with the same reads and invariants on what came back.
//!
//! The stand-in engine is SQLite: the project's database here, under the same declarations (SQL files of the repo, in
//! order). A backup and a restore are declared commands (argv with `{db}`, `{out}`, `{backup}`), run without a shell; the
//! backup's SHA-256 is written beside it and checked before the restore runs, so a backup that changed between the two is
//! refused. Everything is measured as numbers (see [`names`]) and judged by [`judge`].

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::compare::Metric;
use crate::stage::{UNREADABLE, not_run, value};

/// The database file the stage migrates, inside its work directory.
pub const DB_FILE: &str = "app.db";
/// The copy the down round trip runs on.
pub const DOWN_FILE: &str = "down.db";
/// The backup the backup command writes.
pub const BACKUP_FILE: &str = "backup.db";
/// The SHA-256 of the backup, written beside it.
pub const SIDECAR_FILE: &str = "backup.sha256";
/// The database the restore command writes: a fresh service's.
pub const RESTORED_FILE: &str = "restored.db";
/// The placeholders a backup or restore command may use.
const PLACEHOLDERS: [&str; 3] = ["{db}", "{out}", "{backup}"];
/// The most writers and operations a stage may declare (a lab VM is small).
const MAX_WRITERS: u32 = 64;
const MAX_OPS: u32 = 100_000;

/// The data stage of a topology: the `[data]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataStage {
    /// The previous release's schema and data: SQL files of the repo, run in order. Empty when `production` names a
    /// source: the scrubbed copy of production data is the previous release's data, schema included.
    #[serde(default)]
    pub previous: Vec<String>,
    /// The production source the stage runs over (PLAN 22.5, W59): a label naming a read-only source the project's
    /// scrubber copies. The run starts from that scrubbed copy, which the runner hands in ([`run_from`]).
    #[serde(default)]
    pub production: Option<String>,
    /// The new release's migrations: SQL files of the repo, run in order after `previous`.
    #[serde(default)]
    pub migrations: Vec<String>,
    /// The down migrations the project declares, run in order on a copy of the migrated database.
    #[serde(default)]
    pub down: Vec<String>,
    /// The queries the new release's code runs after migrating: each must run and return at least `min_rows` rows.
    #[serde(default, rename = "read")]
    pub reads: Vec<Read>,
    /// The queries the previous release's code runs after the down migrations.
    #[serde(default)]
    pub down_reads: Vec<Read>,
    /// The concurrent writers: each runs `ops` operations, each of the `statements` in one transaction.
    #[serde(default)]
    pub writers: Option<Writers>,
    /// The integrity invariants: each query returns `equals` (times the writes, when `per_write`).
    #[serde(default, rename = "invariant")]
    pub invariants: Vec<Invariant>,
    /// The declared backup and restore commands.
    #[serde(default)]
    pub backup: Option<Backup>,
}

/// A read the code runs: its id (named in findings), its SQL and the rows it must return at least.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Read {
    pub id: String,
    pub sql: String,
    #[serde(default)]
    pub min_rows: u64,
}

/// The concurrent writers: `count` writers run `ops` operations each. An operation runs `statements` in one transaction;
/// the first placeholder is the writer's number and the second the operation's sequence number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Writers {
    pub count: u32,
    pub ops: u32,
    pub statements: Vec<String>,
}

/// An integrity invariant: a query whose single value is `equals`; with `per_write`, `equals` times the writes the
/// writers made (a sum or a row count that the writers' work must produce).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invariant {
    pub id: String,
    pub sql: String,
    pub equals: i64,
    #[serde(default)]
    pub per_write: bool,
}

/// The declared backup and restore: argv of the commands, with `{db}` (the database), `{out}` (the backup) and
/// `{backup}` (the backup restored from).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Backup {
    pub command: Vec<String>,
    pub restore: Vec<String>,
}

impl DataStage {
    /// What TOML cannot say: the files are repo paths, the ids are labels and unique, the SQL is not empty, the writers
    /// are bounded, and the commands use only their placeholders.
    pub fn check(&self) -> Result<(), String> {
        match &self.production {
            Some(source) if !crate::spec::is_label(source) => {
                return Err(format!("data production {source:?} is not a label"));
            }
            Some(_) if !self.previous.is_empty() => {
                return Err("data declares both production and previous: the scrubbed copy is the previous release's data".into());
            }
            None if self.previous.is_empty() => {
                return Err("data declares no previous schema (the previous release's SQL)".into());
            }
            _ => {}
        }
        for path in self
            .previous
            .iter()
            .chain(&self.migrations)
            .chain(&self.down)
        {
            if !crate::spec::is_repo_path(path) || !path.ends_with(".sql") {
                return Err(format!(
                    "data file {path:?} is not a repo path to a .sql file"
                ));
            }
        }
        let mut ids: Vec<&str> = Vec::new();
        for read in self.reads.iter().chain(&self.down_reads) {
            check_read(read)?;
        }
        for read in &self.reads {
            if ids.contains(&read.id.as_str()) {
                return Err(format!("data read {} is declared twice", read.id));
            }
            ids.push(&read.id);
        }
        let mut invariant_ids: Vec<&str> = Vec::new();
        for invariant in &self.invariants {
            if !crate::spec::is_label(&invariant.id) || invariant.sql.trim().is_empty() {
                return Err(format!(
                    "data invariant {:?} needs a label id and a query",
                    invariant.id
                ));
            }
            if invariant_ids.contains(&invariant.id.as_str()) {
                return Err(format!("data invariant {} is declared twice", invariant.id));
            }
            invariant_ids.push(&invariant.id);
        }
        if let Some(writers) = &self.writers {
            if writers.count == 0 || writers.count > MAX_WRITERS {
                return Err(format!(
                    "data writers count {} is not 1 to {MAX_WRITERS}",
                    writers.count
                ));
            }
            if writers.ops == 0 || writers.ops > MAX_OPS {
                return Err(format!(
                    "data writers ops {} is not 1 to {MAX_OPS}",
                    writers.ops
                ));
            }
            if writers.statements.is_empty()
                || writers.statements.iter().any(|s| s.trim().is_empty())
            {
                return Err("data writers declare no statements".into());
            }
        }
        if let Some(backup) = &self.backup {
            for (what, argv) in [("command", &backup.command), ("restore", &backup.restore)] {
                let Some(program) = argv.first() else {
                    return Err(format!("data backup {what} is empty"));
                };
                if program.is_empty() || program.contains('{') {
                    return Err(format!("data backup {what} names no program"));
                }
                for arg in argv {
                    for token in placeholders(arg) {
                        if !PLACEHOLDERS.contains(&token.as_str()) {
                            return Err(format!(
                                "data backup {what} uses {token}, not a placeholder"
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn check_read(read: &Read) -> Result<(), String> {
    if !crate::spec::is_label(&read.id) {
        return Err(format!("data read {:?} is not a label", read.id));
    }
    if read.sql.trim().is_empty() {
        return Err(format!("data read {} has no query", read.id));
    }
    Ok(())
}

/// The `{...}` tokens of a command argument.
fn placeholders(arg: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = arg;
    while let Some(start) = rest.find('{') {
        match rest[start..].find('}') {
            Some(end) => {
                out.push(rest[start..start + end + 1].to_string());
                rest = &rest[start + end + 1..];
            }
            None => break,
        }
    }
    out
}

/// The metric names a data stage records at a run: the previous release's, the migrations', the reads' and the rest.
pub fn names(stage: &DataStage) -> Vec<String> {
    let mut names = vec![
        "data.previous.failed".to_string(),
        "data.migrations.failed".to_string(),
        "data.down.failed".to_string(),
        "data.writers.errors".to_string(),
        "data.backup.failed".to_string(),
        "data.restore.failed".to_string(),
        "data.restore.checksum_ok".to_string(),
    ];
    for read in &stage.reads {
        names.push(format!("data.read.{}.rows", read.id));
        names.push(format!("data.restore.read.{}.rows", read.id));
    }
    for read in &stage.down_reads {
        names.push(format!("data.down_read.{}.rows", read.id));
    }
    for invariant in &stage.invariants {
        names.push(format!("data.invariant.{}.value", invariant.id));
        names.push(format!("data.restore.invariant.{}.value", invariant.id));
    }
    names
}

/// What a run measured: the numbers (recorded at the run) and the notes a person reads (a failed query's error, a
/// refused restore). The notes stay with the stage's output; the hub judges the numbers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub metrics: Vec<Metric>,
    pub notes: Vec<String>,
}

impl Run {
    fn set(&mut self, name: impl Into<String>, value: f64) {
        let name = name.into();
        self.metrics.retain(|m| m.name != name);
        self.metrics.push(Metric { name, value });
    }
}

/// Runs the stage in `dir` (created if missing) over the repo at `root`: the normal run.
pub fn run(stage: &DataStage, root: &Path, dir: &Path) -> Run {
    run_inner(stage, root, dir, None, &|_| {})
}

/// Runs the stage over `copy`, the scrubbed copy of the production source the stage names (PLAN 22.5): the database
/// starts as that copy, and the stage's `previous` SQL is not run. A stage that names a production source without its
/// copy does not start (its numbers are unreadable: not run), and a copy handed to a stage with no source is not used.
pub fn run_from(stage: &DataStage, root: &Path, dir: &Path, copy: Option<&Path>) -> Run {
    run_inner(stage, root, dir, copy, &|_| {})
}

/// Runs the stage, calling `between` once after the backup and before the restore, with the work directory: the place a
/// fault can be put (a backup changed in transit, a disk that fills). A test corrupts the backup here.
pub fn run_with(stage: &DataStage, root: &Path, dir: &Path, between: &dyn Fn(&Path)) -> Run {
    run_inner(stage, root, dir, None, between)
}

fn run_inner(
    stage: &DataStage,
    root: &Path,
    dir: &Path,
    copy: Option<&Path>,
    between: &dyn Fn(&Path),
) -> Run {
    let mut run = Run::default();
    if let Err(why) = std::fs::create_dir_all(dir) {
        run.notes.push(format!(
            "the work directory {} is not usable: {why}",
            dir.display()
        ));
        return unreadable(stage, run);
    }
    let db = dir.join(DB_FILE);
    for stale in [DB_FILE, DOWN_FILE, BACKUP_FILE, SIDECAR_FILE, RESTORED_FILE] {
        let _ = std::fs::remove_file(dir.join(stale));
    }
    if let Some(source) = &stage.production {
        let Some(copy) = copy else {
            run.notes.push(format!(
                "the scrubbed copy of production source {source} was not given: the stage does not start"
            ));
            return unreadable(stage, run);
        };
        if let Err(why) = std::fs::copy(copy, &db) {
            run.notes
                .push(format!("the scrubbed copy was not read: {why}"));
            return unreadable(stage, run);
        }
    }
    let conn = match Connection::open(&db) {
        Ok(conn) => conn,
        Err(why) => {
            run.notes.push(format!("the database did not open: {why}"));
            return unreadable(stage, run);
        }
    };
    // The previous release's schema and data, then the new release's migrations, then the reads its code runs.
    let previous = if stage.production.is_some() {
        0
    } else {
        exec_files(&conn, root, &stage.previous, &mut run.notes)
    };
    run.set("data.previous.failed", previous as f64);
    let migrations = if previous == 0 {
        exec_files(&conn, root, &stage.migrations, &mut run.notes)
    } else {
        1
    };
    run.set("data.migrations.failed", migrations as f64);
    for read in &stage.reads {
        let rows = read_rows(
            &conn,
            &read.sql,
            &format!("read {}", read.id),
            &mut run.notes,
        );
        run.set(format!("data.read.{}.rows", read.id), rows);
    }
    // The down round trip runs on a copy, so the migrated database stays as it is.
    if !stage.down.is_empty() {
        let copy = dir.join(DOWN_FILE);
        let failed = match std::fs::copy(&db, &copy) {
            Ok(_) => match Connection::open(&copy) {
                Ok(down) => {
                    let failed = exec_files(&down, root, &stage.down, &mut run.notes);
                    for read in &stage.down_reads {
                        let rows = read_rows(
                            &down,
                            &read.sql,
                            &format!("down read {}", read.id),
                            &mut run.notes,
                        );
                        run.set(format!("data.down_read.{}.rows", read.id), rows);
                    }
                    failed
                }
                Err(why) => {
                    run.notes.push(format!("the down copy did not open: {why}"));
                    1
                }
            },
            Err(why) => {
                run.notes.push(format!("the down copy was not made: {why}"));
                1
            }
        };
        run.set("data.down.failed", failed as f64);
        if failed != 0 {
            for read in &stage.down_reads {
                run.set(format!("data.down_read.{}.rows", read.id), UNREADABLE);
            }
        }
    }
    // The writers, then the invariants over what they left.
    if let Some(writers) = &stage.writers {
        let op = declared_op(&writers.statements);
        let errors = drive(&db, writers.count, writers.ops, &op);
        run.set("data.writers.errors", errors as f64);
        if errors > 0 {
            run.notes.push(format!("{errors} writer operations failed"));
        }
    }
    for invariant in &stage.invariants {
        let value = scalar(
            &conn,
            &invariant.sql,
            &format!("invariant {}", invariant.id),
            &mut run.notes,
        );
        run.set(format!("data.invariant.{}.value", invariant.id), value);
    }
    // The backup, checked and restored into a fresh database, then the same reads and invariants on what came back.
    if let Some(backup) = &stage.backup {
        backup_and_restore(stage, backup, &db, dir, between, &mut run);
    } else {
        run.set("data.backup.failed", 1.0);
        run.set("data.restore.failed", 1.0);
        run.set("data.restore.checksum_ok", 0.0);
        run.notes.push("no backup is declared".into());
        unreadable_restore(stage, &mut run);
    }
    run
}

/// The run's measurements for every name the stage records, all unreadable: a stage that could not start.
fn unreadable(stage: &DataStage, mut run: Run) -> Run {
    for name in names(stage) {
        run.set(name, UNREADABLE);
    }
    run
}

fn unreadable_restore(stage: &DataStage, run: &mut Run) {
    for read in &stage.reads {
        run.set(format!("data.restore.read.{}.rows", read.id), UNREADABLE);
    }
    for invariant in &stage.invariants {
        run.set(
            format!("data.restore.invariant.{}.value", invariant.id),
            UNREADABLE,
        );
    }
}

fn backup_and_restore(
    stage: &DataStage,
    backup: &Backup,
    db: &Path,
    dir: &Path,
    between: &dyn Fn(&Path),
    run: &mut Run,
) {
    let out = dir.join(BACKUP_FILE);
    let sidecar = dir.join(SIDECAR_FILE);
    let restored = dir.join(RESTORED_FILE);
    let digest = match command(
        &backup.command,
        &[("{db}", db), ("{out}", &out)],
        "backup",
        &mut run.notes,
    ) {
        Ok(()) => match sha256(&out).and_then(|d| {
            std::fs::write(&sidecar, &d)
                .map(|()| d)
                .map_err(|w| w.to_string())
        }) {
            Ok(digest) => Some(digest),
            Err(why) => {
                run.notes
                    .push(format!("the backup's checksum was not recorded: {why}"));
                None
            }
        },
        Err(()) => None,
    };
    run.set(
        "data.backup.failed",
        if digest.is_some() { 0.0 } else { 1.0 },
    );
    between(dir);
    // The restore runs only over a backup whose digest still matches the one the backup command recorded.
    let checked = match (&digest, std::fs::read_to_string(&sidecar)) {
        (Some(digest), Ok(recorded)) => {
            recorded.trim() == digest.as_str() && sha256(&out).ok().as_ref() == Some(digest)
        }
        _ => false,
    };
    run.set("data.restore.checksum_ok", if checked { 1.0 } else { 0.0 });
    let restore_failed = if !checked {
        run.notes
            .push("the restore is refused: the backup does not match its recorded checksum".into());
        true
    } else {
        let _ = std::fs::remove_file(&restored);
        command(
            &backup.restore,
            &[("{backup}", &out), ("{db}", &restored)],
            "restore",
            &mut run.notes,
        )
        .is_err()
    };
    run.set(
        "data.restore.failed",
        if restore_failed { 1.0 } else { 0.0 },
    );
    let restored_conn = Connection::open_with_flags(&restored, OpenFlags::SQLITE_OPEN_READ_ONLY);
    for read in &stage.reads {
        let rows = match &restored_conn {
            Ok(conn) => read_rows(
                conn,
                &read.sql,
                &format!("restored read {}", read.id),
                &mut run.notes,
            ),
            Err(_) => UNREADABLE,
        };
        run.set(format!("data.restore.read.{}.rows", read.id), rows);
    }
    for invariant in &stage.invariants {
        let value = match &restored_conn {
            Ok(conn) => scalar(
                conn,
                &invariant.sql,
                &format!("restored invariant {}", invariant.id),
                &mut run.notes,
            ),
            Err(_) => UNREADABLE,
        };
        run.set(
            format!("data.restore.invariant.{}.value", invariant.id),
            value,
        );
    }
}

/// Runs one declared command (argv, placeholders substituted, no shell). Ok when it exits 0.
fn command(
    argv: &[String],
    subs: &[(&str, &Path)],
    what: &str,
    notes: &mut Vec<String>,
) -> Result<(), ()> {
    let args: Vec<String> = argv
        .iter()
        .map(|arg| {
            subs.iter().fold(arg.clone(), |text, (token, path)| {
                text.replace(token, &path.display().to_string())
            })
        })
        .collect();
    let Some((program, rest)) = args.split_first() else {
        notes.push(format!("the {what} command is empty"));
        return Err(());
    };
    match Command::new(program).args(rest).output() {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            notes.push(format!(
                "the {what} command exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
            Err(())
        }
        Err(why) => {
            notes.push(format!("the {what} command did not start: {why}"));
            Err(())
        }
    }
}

fn sha256(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|why| format!("{}: {why}", path.display()))?;
    let digest = Sha256::digest(&bytes);
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// Runs each SQL file in order; the first that fails stops the list. Returns 1 when one failed, 0 when all ran.
fn exec_files(conn: &Connection, root: &Path, paths: &[String], notes: &mut Vec<String>) -> usize {
    for path in paths {
        let sql = match std::fs::read_to_string(root.join(path)) {
            Ok(sql) => sql,
            Err(why) => {
                notes.push(format!("{path} is not readable: {why}"));
                return 1;
            }
        };
        if let Err(why) = conn.execute_batch(&sql) {
            notes.push(format!("{path} failed: {why}"));
            return 1;
        }
    }
    0
}

/// The rows a query returns, or [`UNREADABLE`] when it fails (its error is a note).
fn read_rows(conn: &Connection, sql: &str, what: &str, notes: &mut Vec<String>) -> f64 {
    let counted = conn.prepare(sql).and_then(|mut stmt| {
        let mut rows = stmt.query([])?;
        let mut n = 0_u64;
        while rows.next()?.is_some() {
            n += 1;
        }
        Ok(n)
    });
    match counted {
        Ok(n) => n as f64,
        Err(why) => {
            notes.push(format!("{what} failed: {why}"));
            UNREADABLE
        }
    }
}

/// The single value a query returns as a number, or [`UNREADABLE`] (a NULL, text that is no number, a failed query).
fn scalar(conn: &Connection, sql: &str, what: &str, notes: &mut Vec<String>) -> f64 {
    let fetched = conn.query_row(sql, [], |row| row.get::<_, rusqlite::types::Value>(0));
    match fetched {
        Ok(rusqlite::types::Value::Integer(i)) => i as f64,
        Ok(rusqlite::types::Value::Real(f)) => f,
        Ok(other) => {
            notes.push(format!("{what} returned {other:?}, not a number"));
            UNREADABLE
        }
        Err(why) => {
            notes.push(format!("{what} failed: {why}"));
            UNREADABLE
        }
    }
}

/// The declared operation of a writer: its statements in one `BEGIN IMMEDIATE` transaction, the writer's number and the
/// sequence number bound to the first two placeholders.
pub fn declared_op(
    statements: &[String],
) -> impl Fn(&Connection, u32, u32) -> rusqlite::Result<()> + Sync + '_ {
    move |conn, writer, seq| {
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = statements.iter().try_for_each(|sql| {
            let mut stmt = conn.prepare(sql)?;
            let bound = [i64::from(writer), i64::from(seq)];
            let n = stmt.parameter_count().min(bound.len());
            stmt.execute(rusqlite::params_from_iter(bound.iter().take(n)))?;
            Ok(())
        });
        // A commit that fails leaves its transaction open, holding the write lock for every writer after it: the
        // operation is rolled back instead, so a failure is one failed operation and no more.
        match result.and_then(|()| conn.execute_batch("COMMIT")) {
            Ok(()) => Ok(()),
            Err(why) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(why)
            }
        }
    }
}

/// How long a writer waits for the write lock before its operation fails: ten seconds on a developer's machine, scaled
/// by the time budgets' slack (`APZ_PERF_SLACK`, else 4 under CI, as the other budgets read it). On a shared runner every
/// writer's commit is an fsync queued behind the others', and a writer that gives up after a fixed ten seconds fails for
/// the runner, not for the project's invariants.
pub fn busy_timeout() -> Duration {
    Duration::from_secs(10).mul_f64(slack())
}

fn slack() -> f64 {
    match std::env::var("APZ_PERF_SLACK")
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
    {
        Some(text) => match text.parse::<f64>() {
            Ok(slack) if slack.is_finite() && slack > 0.0 => slack,
            _ => 1.0,
        },
        None if std::env::var_os("CI").is_some() => 4.0,
        None => 1.0,
    }
}

/// Runs `writers` threads, each performing `ops` calls of `op` on its own connection to `db` (with the [`busy_timeout`]),
/// and returns how many calls failed. The operation decides what is correct: [`declared_op`] for the project's statements.
pub fn drive(
    db: &Path,
    writers: u32,
    ops: u32,
    op: &(dyn Fn(&Connection, u32, u32) -> rusqlite::Result<()> + Sync),
) -> u64 {
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..writers)
            .map(|writer| {
                scope.spawn(move || {
                    let Ok(conn) = Connection::open(db) else {
                        return u64::from(ops);
                    };
                    let _ = conn.busy_timeout(busy_timeout());
                    (0..ops)
                        .filter(|&seq| op(&conn, writer, seq).is_err())
                        .count() as u64
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or(0)).sum()
    })
}

/// The judgments of a data stage over the values the run recorded: each item as `(id, passed, detail, measured)`.
pub fn judge(stage: &DataStage, values: &[Metric]) -> Vec<(String, bool, String, bool)> {
    let mut out = Vec::new();
    let writes = stage
        .writers
        .as_ref()
        .map(|w| u64::from(w.count) * u64::from(w.ops));
    let zero = |name: &str| value(values, name).map(|v| v == 0.0);
    push(
        &mut out,
        "previous",
        "data.previous.failed",
        zero("data.previous.failed"),
        "the previous release's schema and data load",
    );
    push(
        &mut out,
        "migrations",
        "data.migrations.failed",
        zero("data.migrations.failed"),
        "the new release's migrations apply to the previous release's data",
    );
    for read in &stage.reads {
        let name = format!("data.read.{}.rows", read.id);
        let what = format!("read {} after the migrations", read.id);
        rows_item(
            &mut out,
            &format!("read:{}", read.id),
            &name,
            values,
            read.min_rows,
            &what,
        );
    }
    if !stage.down.is_empty() {
        push(
            &mut out,
            "down",
            "data.down.failed",
            zero("data.down.failed"),
            "the declared down migrations run back",
        );
        for read in &stage.down_reads {
            let name = format!("data.down_read.{}.rows", read.id);
            let what = format!("read {} after the down migrations", read.id);
            rows_item(
                &mut out,
                &format!("down-read:{}", read.id),
                &name,
                values,
                read.min_rows,
                &what,
            );
        }
    }
    if stage.writers.is_some() {
        let errors = value(values, "data.writers.errors");
        let detail = format!(
            "the concurrent writers' operations all committed ({} failed)",
            errors.map_or("unknown".into(), describe)
        );
        push(
            &mut out,
            "writers",
            "data.writers.errors",
            errors.map(|e| e == 0.0),
            &detail,
        );
    }
    for invariant in &stage.invariants {
        let name = format!("data.invariant.{}.value", invariant.id);
        out.push(invariant_item(
            &format!("invariant:{}", invariant.id),
            &name,
            value(values, &name),
            invariant,
            writes,
        ));
    }
    if stage.backup.is_some() {
        push(
            &mut out,
            "backup",
            "data.backup.failed",
            zero("data.backup.failed"),
            "the backup command produced a backup and its checksum",
        );
        let restored = match (
            zero("data.restore.failed"),
            value(values, "data.restore.checksum_ok"),
        ) {
            (Some(failed), Some(checked)) => Some(failed && checked == 1.0),
            _ => None,
        };
        push(
            &mut out,
            "restore",
            "data.restore.failed",
            restored,
            "the backup restores into a fresh database after its checksum matched",
        );
        for read in &stage.reads {
            let name = format!("data.restore.read.{}.rows", read.id);
            let what = format!("read {} from the restored database", read.id);
            rows_item(
                &mut out,
                &format!("restore-read:{}", read.id),
                &name,
                values,
                read.min_rows,
                &what,
            );
        }
        for invariant in &stage.invariants {
            let name = format!("data.restore.invariant.{}.value", invariant.id);
            out.push(invariant_item(
                &format!("restore-invariant:{}", invariant.id),
                &name,
                value(values, &name),
                invariant,
                writes,
            ));
        }
    }
    out
}

/// One judged item from a pass-or-fail value: none when the run recorded no value (a stage that did not run).
fn push(
    out: &mut Vec<(String, bool, String, bool)>,
    id: &str,
    name: &str,
    ok: Option<bool>,
    what: &str,
) {
    match ok {
        Some(passed) => out.push((id.to_string(), passed, what.to_string(), true)),
        None => out.push(not_run(id, name)),
    }
}

/// A read's item: its rows are at least `min_rows` (an unreadable read has none).
fn rows_item(
    out: &mut Vec<(String, bool, String, bool)>,
    id: &str,
    name: &str,
    values: &[Metric],
    min_rows: u64,
    what: &str,
) {
    let rows = value(values, name);
    let ok = rows.map(|r| r >= 0.0 && r >= min_rows as f64);
    let shown = rows.map_or("no".into(), describe);
    let detail = format!("{what} returns {shown} rows (at least {min_rows} needed)");
    push(out, id, name, ok, &detail);
}

fn invariant_item(
    id: &str,
    name: &str,
    value: Option<f64>,
    invariant: &Invariant,
    writes: Option<u64>,
) -> (String, bool, String, bool) {
    let Some(value) = value else {
        return not_run(id, name);
    };
    let expected = if invariant.per_write {
        invariant.equals as f64 * writes.unwrap_or(0) as f64
    } else {
        invariant.equals as f64
    };
    let passed = value != UNREADABLE && (value - expected).abs() < 0.5;
    let shown = if value == UNREADABLE {
        "unreadable".into()
    } else {
        describe(value)
    };
    (
        id.to_string(),
        passed,
        format!(
            "{} is {shown}, the invariant says {}",
            invariant.id,
            describe(expected)
        ),
        true,
    )
}

fn describe(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value:.2}")
    }
}
