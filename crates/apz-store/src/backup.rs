//! Backups: SQLite's online backup of one file, and of every file of a store at once with a manifest.
//!
//! A backup is taken from a fresh read connection in one step, so it is one consistent snapshot of its file while
//! writes go on. Copying a running hub's folder is not a backup; this is.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;
use rusqlite::backup::{Backup, StepResult};

use crate::{StoreError, now_ms};

/// Copies the database open as `src` to `dest` (replaced if it exists), in one step, and checks the copy's integrity.
pub(crate) fn copy(src: &Connection, dest: &Path) -> Result<(), StoreError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(dest);
    // A backup holds what the file holds: owner-only, like it.
    crate::conn::private(dest)?;
    {
        let mut out = Connection::open(dest)?;
        let backup = Backup::new(src, &mut out)?;
        // -1: every page in one step, under one read transaction: a consistent snapshot. Busy or locked (a
        // checkpoint in progress) is retried after a moment.
        loop {
            match backup.step(-1)? {
                StepResult::Done => break,
                _ => {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }
    let out = Connection::open(dest)?;
    let check: String = out.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if check != "ok" {
        return Err(StoreError::invalid(format!(
            "the backup {} fails its integrity check: {check}",
            dest.display()
        )));
    }
    // The copy is a plain database file: no WAL to carry along.
    out.pragma_update(None, "journal_mode", "DELETE")?;
    Ok(())
}

/// The backup taken before migrating `path` from schema `version`: `<backups>/<stem>-v<version>-<ms>.db`.
pub(crate) fn before_migration(
    conn: &Connection,
    path: &Path,
    backups: &Path,
    version: u32,
) -> Result<PathBuf, StoreError> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("db");
    let dest = backups.join(format!("{stem}-v{version}-{}.db", now_ms()));
    copy(conn, &dest)?;
    Ok(dest)
}

/// One file of a backup: the file's name in the store, its schema version and the writer's commit sequence when the
/// copy was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub file: String,
    pub schema: u32,
    pub seq: u64,
    pub bytes: u64,
}

/// What a store backup holds: when it was taken and every file with its sequence point. Written as `MANIFEST` in the
/// backup folder, one `key=value` line per field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub taken_ms: i64,
    pub files: Vec<ManifestEntry>,
}

impl Manifest {
    /// The manifest as its file holds it.
    pub fn render(&self) -> String {
        let mut out = format!("taken_ms={}\n", self.taken_ms);
        for f in &self.files {
            let _ = writeln!(
                out,
                "file={} schema={} seq={} bytes={}",
                f.file, f.schema, f.seq, f.bytes
            );
        }
        out
    }

    /// Reads a manifest written by [`Manifest::render`].
    pub fn parse(text: &str) -> Result<Manifest, StoreError> {
        let mut taken_ms = None;
        let mut files = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            if let Some(ms) = line.strip_prefix("taken_ms=") {
                taken_ms = Some(ms.trim().parse().map_err(StoreError::invalid)?);
                continue;
            }
            let mut entry = ManifestEntry {
                file: String::new(),
                schema: 0,
                seq: 0,
                bytes: 0,
            };
            for field in line.split_whitespace() {
                let (k, v) = field
                    .split_once('=')
                    .ok_or_else(|| StoreError::invalid(format!("manifest line {line:?}")))?;
                match k {
                    "file" => entry.file = v.to_string(),
                    "schema" => entry.schema = v.parse().map_err(StoreError::invalid)?,
                    "seq" => entry.seq = v.parse().map_err(StoreError::invalid)?,
                    "bytes" => entry.bytes = v.parse().map_err(StoreError::invalid)?,
                    _ => {}
                }
            }
            files.push(entry);
        }
        Ok(Manifest {
            taken_ms: taken_ms
                .ok_or_else(|| StoreError::invalid("the manifest has no taken_ms"))?,
            files,
        })
    }
}
