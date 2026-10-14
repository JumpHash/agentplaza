//! The work-folder sync's mirror rows (stations/sync.rs): `station_sync_files` (migration 0012), or with no database
//! one JSON file per workstation under `<state-dir>/station-sync/` (see `db::files`).
//!
//! A pass writes its changed rows in one transaction. The rows are not append-only: a path's row is replaced.

use postgres::Row as PgRow;

use super::{Db, sql};
use crate::stations::sync::Row;

const NO_STORE: &str = "no database and no state directory for the work-folder sync";

/// One row, in the column order of `station_sync_rows.sql`.
fn row_of(row: &PgRow) -> Row {
    let unsigned = |i: usize| row.get::<_, i64>(i) as u64;
    Row {
        path: row.get(0),
        hash: row.get(1),
        size: unsigned(2),
        mtime_ms: unsigned(3),
        version: unsigned(4),
        base: row.get(5),
        conflict_of: row.get(6),
        synced_ms: unsigned(7),
    }
}

impl Db {
    /// Every sync row of one workstation (deleted ones included), by path.
    pub fn station_sync_rows(&self, station: &str) -> Result<Vec<Row>, String> {
        if let Some(files) = self.file_store() {
            return files.station_sync_rows(station);
        }
        if !self.enabled() {
            return Err(NO_STORE.into());
        }
        self.run(|c| c.query(sql::STATION_SYNC_ROWS, &[&station]))
            .map(|rows| rows.iter().map(row_of).collect())
            .inspect_err(|e| eprintln!("db: sync rows read failed: {e}"))
    }

    /// Writes the given rows of one workstation, each replacing its path's row, in one transaction.
    pub fn save_station_sync_rows(&self, station: &str, rows: &[Row]) -> Result<(), String> {
        if let Some(files) = self.file_store() {
            return files.save_station_sync_rows(station, rows);
        }
        if !self.enabled() {
            return Err(NO_STORE.into());
        }
        self.run(|c| {
            let mut tx = c.transaction()?;
            for r in rows {
                let (size, mtime, version, synced) = (
                    r.size as i64,
                    r.mtime_ms as i64,
                    r.version as i64,
                    r.synced_ms as i64,
                );
                tx.execute(
                    sql::STATION_SYNC_SAVE,
                    &[
                        &station,
                        &r.path,
                        &r.hash,
                        &size,
                        &mtime,
                        &version,
                        &r.base,
                        &r.conflict_of,
                        &synced,
                    ],
                )?;
            }
            tx.commit()
        })
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_database;

    fn row(path: &str, hash: Option<&str>, version: u64) -> Row {
        Row {
            path: path.into(),
            hash: hash.map(str::to_string),
            size: 12,
            mtime_ms: 1_000,
            version,
            base: hash.map(str::to_string),
            conflict_of: None,
            synced_ms: 2_000,
        }
    }

    /// The same answers from both backends: rows by path, a save replaces its paths and keeps the rest, a deleted row
    /// keeps its place, and another workstation's rows are apart.
    fn check_sync(db: &Db) {
        assert!(db.station_sync_rows("ws-1").unwrap().is_empty());
        let mut copy = row("plan (conflict desk 2026-10-14 1532).md", Some("bb"), 1);
        copy.conflict_of = Some("plan.md".into());
        db.save_station_sync_rows(
            "ws-1",
            &[
                row("plan.md", Some("aa"), 1),
                row("gone.txt", Some("cc"), 2),
                copy.clone(),
            ],
        )
        .unwrap();
        db.save_station_sync_rows("ws-2", &[row("other.txt", Some("dd"), 1)])
            .unwrap();

        let rows = db.station_sync_rows("ws-1").unwrap();
        let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "gone.txt",
                "plan (conflict desk 2026-10-14 1532).md",
                "plan.md"
            ],
            "by path"
        );
        assert_eq!(
            rows.iter().find(|r| r.path == copy.path),
            Some(&copy),
            "the conflict's origin round-trips"
        );

        let deleted = Row {
            hash: None,
            base: None,
            version: 3,
            ..row("gone.txt", Some("cc"), 2)
        };
        db.save_station_sync_rows("ws-1", std::slice::from_ref(&deleted))
            .unwrap();
        let rows = db.station_sync_rows("ws-1").unwrap();
        assert_eq!(rows.len(), 3, "a save replaces its path only");
        assert_eq!(
            rows.iter().find(|r| r.path == "gone.txt"),
            Some(&deleted),
            "the deletion is kept"
        );
        assert_eq!(
            rows.iter()
                .find(|r| r.path == "plan.md")
                .map(|r| r.hash.clone()),
            Some(Some("aa".into()))
        );

        let theirs = db.station_sync_rows("ws-2").unwrap();
        assert_eq!(theirs.len(), 1, "another workstation's rows are apart");
        assert_eq!(theirs[0].path, "other.txt");
    }

    /// The sync rows read and write the same on Postgres (a fresh test database, when the test server is set) and on
    /// the file fallback.
    #[test]
    fn sync_rows_behave_the_same_on_both_backends() {
        if let Some(url) = test_database("station_sync_parity") {
            let env = std::collections::BTreeMap::from([("APZ_DATABASE_URL".to_string(), url)]);
            let db = Db::connect(&env);
            assert!(db.enabled());
            check_sync(&db);
        }
        let dir = std::env::temp_dir().join(format!("apid-sync-parity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let db = Db::files_only(&dir);
        assert!(!db.enabled());
        check_sync(&db);
        assert!(
            db.save_station_sync_rows("../escape", &[]).is_err(),
            "a station id names a file, so it must be plain"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
