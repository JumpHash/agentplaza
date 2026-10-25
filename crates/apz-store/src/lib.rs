//! The hub's store: SQLite in one folder (PLAN 30.3, W65, W69, W70).
//!
//! - **One hub per folder.** [`Store::open`] takes an exclusive lock on the folder; a second hub on it refuses to
//!   start ([`StoreError::Locked`]).
//! - **Files.** A store opens the database files it is given ([`FileSpec`]): each with its own durability
//!   ([`Durability::Full`] for state that must survive a power loss, [`Durability::Normal`] for streams,
//!   [`Durability::Scratch`] for a test's throwaway store), WAL, foreign keys and a busy timeout on every connection.
//! - **One writer per file.** A dedicated thread owns the file's only write connection ([`Writer`]). Callers send it
//!   typed operations (closures over the open transaction, never SQL strings from outside); it runs the operations
//!   that arrive together in one `BEGIN IMMEDIATE` transaction, each in its own savepoint, commits once, and answers
//!   each with its result and the commit's sequence number ([`Written`]). There is never a second writer, so a write
//!   never meets `SQLITE_BUSY`, and a burst of writes costs one sync to disk.
//! - **Many readers.** A pool of connections per file, sized to the CPUs, each used by one thread at a time
//!   ([`Readers`]). With WAL a reader sees one consistent snapshot and never waits for the writer.
//! - **Reading your own write.** A [`Written`] carries its commit's sequence number; [`Db::wait_for`] waits until a
//!   sequence is committed, for a reader on another thread.
//! - **Checkpoints.** The writer checkpoints the WAL when it is idle and truncates it past a size bound.
//! - **Migrations** are embedded SQL, numbered from 1 ([`Migration`]): a file newer than the code is refused, a file
//!   whose applied migration text changed is refused, and a backup is taken before migrating a file that has data.
//! - **Tables, not JSON.** [`Table`] (derived) maps a struct to a `STRICT` table with typed columns, and its lists and
//!   maps to child tables; [`Column`] (derived for enums) maps a value to one typed column with a `CHECK`.
//!   [`lint`] fails a schema with a table that is not `STRICT`, an untyped or JSON column, or a query that uses JSON
//!   functions.
//! - **Backups** of every file at once, consistent, with a manifest of their sequence points ([`Store::backup_to`]).

mod backup;
mod conn;
mod error;
pub mod lint;
mod lock;
mod migrate;
mod readers;
mod store;
pub mod table;
mod writer;

pub use apz_store_derive::{Column, Table};
pub use backup::{Manifest, ManifestEntry};
pub use conn::{Durability, FileSpec};
pub use error::StoreError;
pub use lock::FolderLock;
pub use migrate::Migration;
pub use readers::{Reader, Readers};
pub use rusqlite;
pub use store::{Db, Store};
pub use table::{
    Changes, Column, ColumnDef, Content, Ctx, IndexDef, Loaded, Row, RowSet, Saved, SqlType, Table,
    TableDef,
};
pub use writer::{Writer, WriterStats, Written};

/// The result of a store call.
pub type Result<T, E = StoreError> = std::result::Result<T, E>;

/// Milliseconds since the Unix epoch, as the store records times.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}
