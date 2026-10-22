//! How every connection is opened and set up.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::{Migration, StoreError};

/// How hard a file's commits hold on to the disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// `synchronous = FULL`: an acknowledged commit survives a power loss (budgets, cards, state of record).
    Full,
    /// `synchronous = NORMAL`: an acknowledged commit survives a crash of the hub, not always a power loss
    /// (transcripts and metrics: large streams that need no transaction with the main file).
    Normal,
}

impl Durability {
    fn pragma(self) -> &'static str {
        match self {
            Durability::Full => "FULL",
            Durability::Normal => "NORMAL",
        }
    }
}

/// One database file of a store: its name (the key [`crate::Store::db`] finds it by), its file name in the folder,
/// its durability and its embedded migrations.
#[derive(Debug, Clone, Copy)]
pub struct FileSpec {
    pub name: &'static str,
    pub file: &'static str,
    pub durability: Durability,
    pub migrations: &'static [Migration],
}

/// Whether a connection writes or only reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Writer,
    Reader,
}

/// How long a connection waits on a lock before it reports busy. Writers never wait on each other (there is one);
/// this covers a reader that meets a checkpoint or recovery.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// Each connection's page cache, in KiB.
const CACHE_KIB: i64 = 8 * 1024;
/// How much of the file each connection maps into memory for reads.
const MMAP_BYTES: i64 = 256 * 1024 * 1024;
/// Prepared statements each connection keeps compiled.
const STATEMENT_CACHE: usize = 256;

/// Makes `path` readable and writable by its owner only (0600), creating it empty when it is missing, before SQLite
/// opens it: the hub's files hold its state of record (and the vault's sealed records), and SQLite gives a file's
/// `-wal` and `-shm` the file's own mode. Tightens the mode of a file (and of its `-wal`/`-shm`) that has a looser one.
/// Elsewhere than Unix the file keeps the folder's access rules.
pub(crate) fn private(path: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
        {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                for p in [
                    path.to_path_buf(),
                    with_suffix(path, "-wal"),
                    with_suffix(path, "-shm"),
                ] {
                    if let Ok(meta) = std::fs::metadata(&p)
                        && meta.permissions().mode() & 0o077 != 0
                    {
                        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
                    }
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// `path` with `suffix` after its file name (`plaza.db` → `plaza.db-wal`).
#[cfg(unix)]
fn with_suffix(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    std::path::PathBuf::from(name)
}

/// Opens a connection to `path` with the settings every connection of a store shares: WAL (set by the writer), the
/// file's durability, foreign keys, the busy timeout, the cache, memory-mapped reads, temporary data in memory, the
/// statement cache; a reader is also `query_only`. Each connection is used by one thread at a time (SQLite's
/// per-connection mutex is off; the bundled library is built thread-safe).
pub(crate) fn open(
    path: &Path,
    durability: Durability,
    role: Role,
) -> Result<Connection, StoreError> {
    if role == Role::Writer {
        private(path)?;
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_URI;
    let conn = Connection::open_with_flags(path, flags)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    if role == Role::Writer {
        let mode: String =
            conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::invalid(format!(
                "{} cannot use WAL (journal mode {mode})",
                path.display()
            )));
        }
        // The writer checkpoints on its own schedule (writer.rs), not every 1000 pages inside a commit.
        conn.pragma_update(None, "wal_autocheckpoint", 0)?;
    }
    conn.pragma_update(None, "synchronous", durability.pragma())?;
    if durability == Durability::Full {
        // On macOS fsync does not reach the platter; F_FULLFSYNC does (ignored elsewhere). A FULL file's commit must
        // survive a power loss (a budget reservation, 4.10), so its syncs and checkpoints use it.
        conn.pragma_update(None, "fullfsync", true)?;
        conn.pragma_update(None, "checkpoint_fullfsync", true)?;
    }
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "cache_size", -CACHE_KIB)?;
    conn.pragma_update(None, "mmap_size", MMAP_BYTES)?;
    if role == Role::Reader {
        conn.pragma_update(None, "query_only", true)?;
    }
    conn.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
    Ok(conn)
}
