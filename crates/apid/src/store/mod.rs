//! The hub's store (PLAN 30.3, W65, W69, W70): SQLite in the state folder, through `apz-store`.
//!
//! - `plaza.db` (`synchronous = FULL`; on macOS with `fullfsync`): every component of the hub, the work engine's
//!   records (sessions, seats, budgets, cards, chats), the audits and settings. An acknowledged write there survives a
//!   power loss.
//! - `transcripts.db` and `metrics.db` (`NORMAL`): the large streams that grow by appending and need no transaction
//!   with the main file. Their rows carry the ids of what they belong to in the main file.
//! - `store.lock` in the folder: one hub per state folder; a second refuses to start.
//! - Migrations are `sql/<file>/NNNN_*.sql`, embedded; a backup goes to `backups/` before one runs on a file with data.
//!
//! The components keep their in-memory working copy; each turns its state into rows of typed tables
//! (`derive(apz_store::Table)`), and the saver (`saver.rs`) writes what changed through the file's one writer.
//! Streams (transcripts, audits, metric samples) are written through as they happen.

use std::path::Path;

use apz_store::{Durability, FileSpec, Migration, Store, StoreError};

pub mod components;
pub mod saver;
pub mod search;
pub mod sql;
#[cfg(test)]
mod tests;

/// The main file.
pub const PLAZA: &str = "plaza";
/// The transcripts.
pub const TRANSCRIPTS: &str = "transcripts";
/// The metric samples and rollups.
pub const METRICS: &str = "metrics";

const PLAZA_MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "init",
    sql: include_str!("../../sql/plaza/0001_init.sql"),
}];

const TRANSCRIPT_MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "init",
    sql: include_str!("../../sql/transcripts/0001_init.sql"),
}];

const METRIC_MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "init",
    sql: include_str!("../../sql/metrics/0001_init.sql"),
}];

/// The hub's three files.
pub const FILES: &[FileSpec] = &[
    FileSpec {
        name: PLAZA,
        file: "plaza.db",
        durability: Durability::Full,
        migrations: PLAZA_MIGRATIONS,
    },
    FileSpec {
        name: TRANSCRIPTS,
        file: "transcripts.db",
        durability: Durability::Normal,
        migrations: TRANSCRIPT_MIGRATIONS,
    },
    FileSpec {
        name: METRICS,
        file: "metrics.db",
        durability: Durability::Normal,
        migrations: METRIC_MIGRATIONS,
    },
];

/// Opens the hub's store in `state_dir`: takes the folder's lock (refusing when another hub holds it), then opens and
/// migrates the three files.
pub fn open(state_dir: &Path) -> Result<Store, StoreError> {
    Store::open(state_dir, FILES)
}

/// The DDL of every component's tables, as `sql/plaza/0001_init.sql` holds it (`apid schema` prints it).
pub fn components_ddl() -> String {
    let mut out = String::new();
    for c in components::all() {
        out.push_str(&format!("-- component {}\n", c.name));
        out.push_str(&(c.def)().ddl());
        out.push('\n');
    }
    for (name, def) in components::written_through() {
        out.push_str(&format!("-- {name} (written through)\n"));
        out.push_str(&def().ddl());
        out.push('\n');
    }
    out
}

/// `apid schema`: prints the components' DDL, for `sql/plaza/0001_init.sql`.
pub fn print_schema() {
    print!("{}", components_ddl());
}
