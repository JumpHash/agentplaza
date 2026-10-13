//! Schema migrations. Each `sql/migrations/NNNN_name.sql` is one migration: embedded at compile
//! time, applied once, in version order, each in its own transaction with its
//! `schema_migrations` row. An applied migration is immutable: its sha256 is recorded and checked
//! on every start (a changed file refuses to start), and `sql/migrations/migrations.lock` pins the
//! same hashes in git, so CI can refuse an edit (docs/dev/sql.md).

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use super::sql;

/// One migration: its version (1, 2, … contiguous), its name (the file's part after `NNNN_`),
/// and its SQL.
pub struct Migration {
    pub version: i32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Every migration this build ships, in version order. A new migration is a new entry here, a new
/// file, and a new line in `migrations.lock` (`scripts/migrations.sh lock`).
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "init",
    sql: include_str!("../../sql/migrations/0001_init.sql"),
}, Migration {
    version: 2,
    name: "common",
    sql: include_str!("../../sql/migrations/0002_common.sql"),
}, Migration {
    version: 3,
    name: "agents",
    sql: include_str!("../../sql/migrations/0003_agents.sql"),
}, Migration {
    version: 4,
    name: "hub_mcp",
    sql: include_str!("../../sql/migrations/0004_hub_mcp.sql"),
}, Migration {
    version: 5,
    name: "hub_mcp_modes",
    sql: include_str!("../../sql/migrations/0005_hub_mcp_modes.sql"),
}];

/// The advisory lock every runner takes: "APZMIG" in ASCII. Fixed for the life of the schema.
pub(super) const LOCK_KEY: i64 = 0x4150_5a4d_4947;

/// The file name a migration is stored under: `0001_init.sql`.
pub fn file_name(migration: &Migration) -> String {
    format!("{:04}_{}.sql", migration.version, migration.name)
}

/// The sha256 of a migration's SQL, hex, lowercase (the form recorded and locked).
pub fn checksum(sql: &str) -> String {
    Sha256::digest(sql.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Brings the database to this build's schema. Returns the versions it applied (empty when the
/// database is current). Any refusal is an error naming the file: a changed applied migration,
/// an applied migration this build does not ship, or a migration that fails.
pub fn migrate(client: &mut postgres::Client) -> Result<Vec<i32>, String> {
    migrate_with(client, MIGRATIONS)
}

/// `migrate` over an explicit list (the tests use it to simulate a tampered file).
pub fn migrate_with(client: &mut postgres::Client, migrations: &[Migration]) -> Result<Vec<i32>, String> {
    client.query(sql::MIGRATION_LOCK, &[&LOCK_KEY]).map_err(|e| format!("migrations: take the lock: {e}"))?;
    let applied = apply(client, migrations);
    let unlocked = client.query(sql::MIGRATION_UNLOCK, &[&LOCK_KEY]);
    let applied = applied?;
    unlocked.map_err(|e| format!("migrations: release the lock: {e}"))?;
    Ok(applied)
}

/// The work done under the lock.
fn apply(client: &mut postgres::Client, migrations: &[Migration]) -> Result<Vec<i32>, String> {
    client.execute(sql::MIGRATION_TABLE_ENSURE, &[]).map_err(|e| format!("migrations: schema_migrations: {e}"))?;
    let recorded: BTreeMap<i32, (String, String)> = client
        .query(sql::MIGRATION_APPLIED, &[])
        .map_err(|e| format!("migrations: read schema_migrations: {e}"))?
        .iter()
        .map(|row| (row.get::<_, i32>(0), (row.get::<_, String>(1), row.get::<_, String>(2))))
        .collect();

    for (version, (name, _)) in &recorded {
        if !migrations.iter().any(|m| m.version == *version) {
            return Err(format!(
                "the database has applied migration {version:04}_{name}.sql, which this build does not ship: refusing to start (is an older build running against a newer database?)"
            ));
        }
    }

    let mut applied = Vec::new();
    for migration in migrations {
        let file = file_name(migration);
        let sum = checksum(migration.sql);
        match recorded.get(&migration.version) {
            Some((name, stored)) => {
                if *name != migration.name || *stored != sum {
                    return Err(format!(
                        "migration {file} changed after it was applied (recorded {name} {stored}, file {sum}): applied migrations are immutable: add a new migration instead"
                    ));
                }
            }
            None => {
                if let Some(later) = recorded.keys().find(|v| **v > migration.version) {
                    return Err(format!(
                        "migration {file} is not applied but migration {later:04} is: migrations apply in order"
                    ));
                }
                let mut tx = client.transaction().map_err(|e| format!("migration {file}: begin: {e}"))?;
                tx.batch_execute(migration.sql).map_err(|e| format!("migration {file} failed: {e}"))?;
                tx.execute(sql::MIGRATION_RECORD, &[&migration.version, &migration.name, &sum])
                    .map_err(|e| format!("migration {file}: record: {e}"))?;
                tx.commit().map_err(|e| format!("migration {file}: commit: {e}"))?;
                eprintln!("db: applied migration {file}");
                applied.push(migration.version);
            }
        }
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Barrier};

    use super::*;

    /// The schema the apid db module ran before migrations existed.
    const LEGACY_SCHEMA: &str = include_str!("../../sql/test/legacy_schema.sql");
    /// One text row describing a database's structure (test only).
    const SCHEMA_DESCRIBE: &str = include_str!("../../sql/test/schema_describe.sql");

    fn migrations_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("sql").join("migrations")
    }

    /// Every `NNNN_name.sql` in the folder as (version, name, file bytes), by version. Panics on
    /// any other `.sql` name; the lock file is not a migration.
    fn folder() -> Vec<(i32, String, String)> {
        let dir = migrations_dir();
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
            let file = entry.expect("directory entry").file_name().to_string_lossy().into_owned();
            if !file.ends_with(".sql") {
                continue;
            }
            let stem = file.strip_suffix(".sql").expect("suffix checked");
            let (number, name) = stem.split_once('_').unwrap_or_else(|| panic!("{file}: want NNNN_name.sql"));
            assert!(number.len() == 4 && number.bytes().all(|b| b.is_ascii_digit()), "{file}: want four digits");
            let text = std::fs::read_to_string(dir.join(&file)).expect("read migration");
            out.push((number.parse().expect("digits"), name.to_string(), text));
        }
        out.sort_by_key(|(version, _, _)| *version);
        out
    }

    /// The migration folder and the embedded list agree exactly: versions contiguous from 1,
    /// names match the file names, and no file is left out.
    #[test]
    fn migrations_match_the_folder_exactly() {
        let files = folder();
        assert_eq!(files.len(), MIGRATIONS.len(), "every file in sql/migrations is in MIGRATIONS, and only those");
        for (index, (file, migration)) in files.iter().zip(MIGRATIONS).enumerate() {
            let expected = index as i32 + 1;
            assert_eq!(file.0, expected, "versions must be contiguous from 1");
            assert_eq!(migration.version, expected, "MIGRATIONS out of order");
            assert_eq!(file.1, migration.name, "name of version {expected} differs from its file");
            assert_eq!(file.2, migration.sql, "embedded SQL of {} differs from the file", file_name(migration));
        }
    }

    /// migrations.lock has one `NNNN name sha256` line per migration file, and nothing else: the
    /// versions are contiguous from 1, each sha256 is the file's, and no line is extra or missing.
    #[test]
    fn migrations_lock_matches_the_files() {
        let path = migrations_dir().join("migrations.lock");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let lines: Vec<&str> = text.lines().collect();
        let files = folder();
        assert_eq!(lines.len(), files.len(), "migrations.lock must have one line per migration and no extra lines");
        for (index, (line, (version, name, body))) in lines.iter().zip(&files).enumerate() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            assert_eq!(fields.len(), 3, "migrations.lock line {}: want `NNNN name sha256`", index + 1);
            assert_eq!(fields[0], format!("{version:04}"), "migrations.lock line {}: version", index + 1);
            assert_eq!(fields[1], name.as_str(), "migrations.lock line {}: name", index + 1);
            assert_eq!(fields[2], checksum(body), "migrations.lock line {}: sha256 of {version:04}_{name}.sql", index + 1);
        }
    }

    /// The database URL for a test on a fresh database, or `None` (skip) without the test server.
    fn fresh(label: &str) -> Option<String> {
        crate::db::test_database(label)
    }

    fn connect(url: &str) -> postgres::Client {
        postgres::Client::connect(url, postgres::NoTls).expect("connect to the test database")
    }

    /// The structure of the database (see sql/test/schema_describe.sql); `None` when it has no tables.
    fn describe(client: &mut postgres::Client) -> Option<String> {
        client.query_one(SCHEMA_DESCRIBE, &[]).expect("describe").get(0)
    }

    fn applied(client: &mut postgres::Client) -> Vec<(i32, String, String)> {
        client
            .query(sql::MIGRATION_APPLIED, &[])
            .expect("read schema_migrations")
            .iter()
            .map(|row| (row.get(0), row.get(1), row.get(2)))
            .collect()
    }

    /// The three rows a fresh run records: 0001, 0002 and 0003.
    fn both_applied() -> Vec<(i32, String, String)> {
        vec![(1, "init".to_string(), checksum(MIGRATIONS[0].sql)), (2, "common".to_string(), checksum(MIGRATIONS[1].sql)), (3, "agents".to_string(), checksum(MIGRATIONS[2].sql)), (4, "hub_mcp".to_string(), checksum(MIGRATIONS[3].sql)), (5, "hub_mcp_modes".to_string(), checksum(MIGRATIONS[4].sql))]
    }

    #[test]
    fn applies_on_an_empty_database() {
        let Some(url) = fresh("applies_on_empty") else { return };
        let mut client = connect(&url);
        assert_eq!(migrate(&mut client), Ok(vec![1, 2, 3, 4, 5]));
        assert_eq!(applied(&mut client), both_applied());
        assert!(describe(&mut client).is_some_and(|d| d.contains("column plaza_state.component")));
    }

    #[test]
    fn rerunning_is_a_no_op() {
        let Some(url) = fresh("rerun") else { return };
        let mut client = connect(&url);
        assert_eq!(migrate(&mut client), Ok(vec![1, 2, 3, 4, 5]));
        let before = describe(&mut client);
        assert_eq!(migrate(&mut client), Ok(vec![]));
        assert_eq!(migrate(&mut client), Ok(vec![]));
        assert_eq!(applied(&mut client).len(), 5);
        assert_eq!(describe(&mut client), before);
    }

    /// A database made by the old code (no schema_migrations) migrates: 0001 is recorded, the
    /// existing rows survive, and the structure is identical to a fresh migration.
    #[test]
    fn a_legacy_schema_migrates_and_records_0001() {
        let (Some(legacy_url), Some(fresh_url)) = (fresh("legacy"), fresh("fresh_for_legacy")) else { return };

        let mut legacy = connect(&legacy_url);
        legacy.batch_execute(LEGACY_SCHEMA).expect("create the legacy schema");
        legacy
            .execute(sql::STATE_UPSERT, &[&"apz_legacy_component", &r#"{"kept":true}"#, &7i64])
            .expect("a row written by the old code");
        let legacy_before = describe(&mut legacy);

        assert_eq!(migrate(&mut legacy), Ok(vec![1, 2, 3, 4, 5]), "0001 records as applied over the legacy tables");
        assert_eq!(applied(&mut legacy), both_applied());
        let kept: String = legacy.query_one(sql::STATE_GET, &[&"apz_legacy_component"]).expect("the old row").get(0);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&kept).unwrap(), serde_json::json!({ "kept": true }));

        let mut fresh_db = connect(&fresh_url);
        assert_eq!(migrate_with(&mut fresh_db, &MIGRATIONS[..1]), Ok(vec![1]));
        assert_eq!(legacy_before, describe(&mut fresh_db), "0001 must reproduce the legacy schema exactly");
    }

    /// A recorded checksum that no longer matches the file refuses, naming the file.
    #[test]
    fn a_changed_applied_migration_refuses() {
        let Some(url) = fresh("tampered") else { return };
        let mut client = connect(&url);
        assert_eq!(migrate(&mut client), Ok(vec![1, 2, 3, 4, 5]));

        let edited = [Migration { version: 1, name: "init", sql: "-- edited after it was applied\n" }, Migration { version: 2, name: "common", sql: MIGRATIONS[1].sql }, Migration { version: 3, name: "agents", sql: MIGRATIONS[2].sql }, Migration { version: 4, name: "hub_mcp", sql: MIGRATIONS[3].sql }, Migration { version: 5, name: "hub_mcp_modes", sql: MIGRATIONS[4].sql }];
        let error = migrate_with(&mut client, &edited).expect_err("a changed applied migration must refuse");
        assert!(error.contains("0001_init.sql"), "{error}");
        assert!(error.contains("applied migrations are immutable: add a new migration instead"), "{error}");

        let error = migrate_with(&mut client, &MIGRATIONS[..2]).expect_err("a database ahead of this build must refuse");
        assert!(error.contains("does not ship"), "{error}");
    }

    /// A migration that fails applies nothing (its transaction rolls back) and refuses.
    #[test]
    fn a_failed_migration_rolls_back() {
        let Some(url) = fresh("rollback") else { return };
        let mut client = connect(&url);
        let broken = [Migration { version: 1, name: "init", sql: "-- broken\nCREATE TABLE apz_partial (x int);\nSELECT 1/0;\n" }];
        let error = migrate_with(&mut client, &broken).expect_err("the broken migration must refuse");
        assert!(error.contains("0001_init.sql"), "{error}");
        assert_eq!(describe(&mut client), None, "nothing from the failed migration may remain");
        assert!(applied(&mut client).is_empty());
    }

    /// Two runners at once take turns on the advisory lock: one applies 0001, the other finds it
    /// applied, and both succeed with one schema_migrations row.
    #[test]
    fn concurrent_runners_serialise() {
        let Some(url) = fresh("concurrent") else { return };
        let barrier = Arc::new(Barrier::new(2));
        let runners: Vec<_> = (0..2)
            .map(|_| {
                let (url, barrier) = (url.clone(), Arc::clone(&barrier));
                std::thread::spawn(move || {
                    let mut client = connect(&url);
                    barrier.wait();
                    migrate(&mut client)
                })
            })
            .collect();
        let results: Vec<Result<Vec<i32>, String>> = runners.into_iter().map(|t| t.join().expect("runner thread")).collect();
        let total: Vec<i32> = results.into_iter().flat_map(|r| r.expect("both runners succeed")).collect();
        assert_eq!(total, vec![1, 2, 3, 4, 5], "exactly one runner applies 0001, 0002 and 0003");
        assert_eq!(applied(&mut connect(&url)).len(), 5);
    }
}
