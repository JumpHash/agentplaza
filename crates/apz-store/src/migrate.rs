//! Embedded migrations: numbered SQL applied at open, in order, each in its own transaction.
//!
//! The file's `user_version` is its schema version; `schema_migration` records each applied migration with a hash
//! of its text. A file past the newest migration this build knows is refused ([`StoreError::Newer`]); an applied
//! migration whose text changed is refused ([`StoreError::Changed`]); a file that has data is backed up before any
//! migration runs on it.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::{StoreError, now_ms};

/// One embedded migration: its version (1, 2, 3, … with no gaps), a short name and its SQL.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

/// The table every file keeps its migration history in. It is the store's own, outside the numbered migrations.
const HISTORY: &str = "CREATE TABLE IF NOT EXISTS schema_migration (
    version INTEGER PRIMARY KEY NOT NULL CHECK (version > 0),
    name TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    applied_ms INTEGER NOT NULL
) STRICT";

/// What [`apply`] did to a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub from: u32,
    pub to: u32,
    /// The backup taken before migrating, when the file had data.
    pub backup: Option<PathBuf>,
}

fn digest(sql: &str) -> String {
    let hash = Sha256::digest(sql.as_bytes());
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Checks that `migrations` are numbered 1..=n in order.
pub(crate) fn check_list(migrations: &[Migration]) -> Result<(), StoreError> {
    for (i, m) in migrations.iter().enumerate() {
        if m.version as usize != i + 1 {
            return Err(StoreError::invalid(format!(
                "migration {} ({}) is out of order: expected version {}",
                m.version,
                m.name,
                i + 1
            )));
        }
    }
    Ok(())
}

/// The schema version of an open file.
pub(crate) fn version(conn: &Connection) -> Result<u32, StoreError> {
    let v: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    u32::try_from(v).map_err(|_| StoreError::invalid(format!("schema version {v} is out of range")))
}

/// Brings the file at `path` (open as `conn`) up to the newest of `migrations`. `backups` is the folder a backup goes
/// to before a file with data is migrated.
pub(crate) fn apply(
    conn: &mut Connection,
    path: &Path,
    migrations: &[Migration],
    backups: &Path,
) -> Result<Applied, StoreError> {
    check_list(migrations)?;
    let known = migrations.len() as u32;
    let from = version(conn)?;
    if from > known {
        return Err(StoreError::Newer {
            file: path.to_path_buf(),
            found: from,
            known,
        });
    }
    conn.execute_batch(HISTORY)?;
    verify_history(conn, path, &migrations[..from as usize])?;
    if from == known {
        return Ok(Applied {
            from,
            to: from,
            backup: None,
        });
    }
    let backup = if from > 0 {
        Some(crate::backup::before_migration(conn, path, backups, from)?)
    } else {
        None
    };
    for m in &migrations[from as usize..] {
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql).map_err(|e| {
            StoreError::invalid(format!(
                "{}: migration {} ({}) failed: {e}",
                path.display(),
                m.version,
                m.name
            ))
        })?;
        tx.execute(
            "INSERT INTO schema_migration (version, name, sha256, applied_ms) VALUES (?1, ?2, ?3, ?4)",
            (m.version, m.name, digest(m.sql), now_ms()),
        )?;
        tx.pragma_update(None, "user_version", m.version)?;
        tx.commit()?;
    }
    Ok(Applied {
        from,
        to: known,
        backup,
    })
}

/// Fails when an applied migration's recorded hash differs from its embedded text.
fn verify_history(conn: &Connection, path: &Path, applied: &[Migration]) -> Result<(), StoreError> {
    let mut stmt = conn.prepare("SELECT version, sha256 FROM schema_migration ORDER BY version")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (v, sha) = row?;
        if let Some(m) = applied.get(v as usize - 1)
            && digest(m.sql) != sha
        {
            return Err(StoreError::Changed {
                file: path.to_path_buf(),
                version: v,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: Migration = Migration {
        version: 1,
        name: "init",
        sql: "CREATE TABLE thing (id INTEGER PRIMARY KEY, name TEXT NOT NULL) STRICT;",
    };
    const TWO: Migration = Migration {
        version: 2,
        name: "more",
        sql: "ALTER TABLE thing ADD COLUMN size INTEGER NOT NULL DEFAULT 0;",
    };

    fn open(path: &Path) -> Connection {
        Connection::open(path).unwrap()
    }

    #[test]
    fn migrations_apply_in_order_and_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.db");
        let mut conn = open(&path);
        let done = apply(&mut conn, &path, &[ONE], dir.path()).unwrap();
        assert_eq!((done.from, done.to, done.backup), (0, 1, None));
        conn.execute("INSERT INTO thing (id, name) VALUES (1, 'x')", [])
            .unwrap();
        let done = apply(&mut conn, &path, &[ONE, TWO], dir.path()).unwrap();
        assert_eq!((done.from, done.to), (1, 2));
        let backup = done.backup.expect("a file with data is backed up first");
        assert!(backup.exists());
        let again = apply(&mut conn, &path, &[ONE, TWO], dir.path()).unwrap();
        assert_eq!((again.from, again.to, again.backup), (2, 2, None));
        let size: i64 = conn
            .query_row("SELECT size FROM thing WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(size, 0);
    }

    #[test]
    fn a_newer_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.db");
        let mut conn = open(&path);
        apply(&mut conn, &path, &[ONE, TWO], dir.path()).unwrap();
        match apply(&mut conn, &path, &[ONE], dir.path()) {
            Err(StoreError::Newer { found, known, .. }) => assert_eq!((found, known), (2, 1)),
            other => panic!("expected Newer, got {other:?}"),
        }
    }

    #[test]
    fn an_edited_migration_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.db");
        let mut conn = open(&path);
        apply(&mut conn, &path, &[ONE], dir.path()).unwrap();
        let edited = Migration {
            sql: "CREATE TABLE thing (id INTEGER PRIMARY KEY) STRICT;",
            ..ONE
        };
        match apply(&mut conn, &path, &[edited], dir.path()) {
            Err(StoreError::Changed { version, .. }) => assert_eq!(version, 1),
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    #[test]
    fn gaps_in_the_numbering_are_refused() {
        let three = Migration { version: 3, ..TWO };
        assert!(check_list(&[ONE, three]).is_err());
    }
}
