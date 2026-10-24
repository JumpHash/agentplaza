# Hub database (SQLite)

The hub (`apid`) keeps its state in SQLite. The generic store is the crate `crates/apz-store`: the folder lock, the
writer, the readers, migrations, backups, the lint and the `derive(Table)` mapping. The hub uses it in
`crates/apid/src/store/`: its three files, its components and the saver. This document describes how the store runs.
The how-to for adding a component, a table or a query is in `docs/dev/sql.md`. The object store for bytes is a
separate store (section 12).

## 1. Files in the state folder

The state folder is `--state-dir`. `apid::store::open` calls `Store::open` with `FILES` (`crates/apid/src/store/mod.rs`).
The folder lock is taken first; then each file is opened, migrated and given its writer.

| File | Durability | Contents |
| --- | --- | --- |
| `plaza.db` | `Full` | The state of record: the 46 components' tables, the work engine's records, the vault, the audits, shares, skills, prompts, artifacts, and the write-through ledgers (30 definitions). The first migration creates 337 tables. |
| `transcripts.db` | `Normal` | `entry`: every session's transcript, appended as it happens. |
| `metrics.db` | `Normal` | Metric samples, 1-minute and 1-hour rollups, events, model calls, kept traces, and the metric settings, alerts and dashboards (24 tables). |

Rows in `transcripts.db` and `metrics.db` refer to ids in `plaza.db` by value. SQLite has no foreign keys across files,
so nothing enforces those references.

Other things in the state folder:

- `store.lock`: the folder lock (section 3).
- `backups/`: the copies taken before a migration (section 6).
- `blobs/`: the object store (section 12).
- `tasks-archive.jsonl` and `foundry/`: JSON files that are not in the store (section 9).

Each database file is created owner-only (mode 0600). Its `-wal` and `-shm` files take the same mode, and so does every
backup. A writer that opens a file with a looser mode tightens it (`private` in `crates/apz-store/src/conn.rs`).

## 2. Connections and durability

Every connection is opened by `conn::open` with the same settings: a busy timeout of 5 s; an 8 MiB page cache;
memory-mapped reads up to 256 MiB; temporary tables in memory; foreign keys on; 256 cached prepared statements.
A reader is also `query_only`. The writer sets the journal mode to WAL (the open fails if SQLite cannot use WAL) and
turns off SQLite's automatic checkpoints, because the writer runs its own (section 4).

`synchronous` follows the file's `Durability`:

- `Full` (`plaza.db`): `synchronous = FULL`, with `fullfsync` and `checkpoint_fullfsync` on. A commit that returned
  survives a power loss. On macOS a plain `fsync` does not reach the platter; `fullfsync` does, so both pragmas are set.
- `Normal` (`transcripts.db`, `metrics.db`): `synchronous = NORMAL`. A commit that returned survives a crash of the
  hub. In WAL mode a power loss can roll back the most recent commits; SQLite does not corrupt the file in that case.
  These files are large appends that need no transaction with `plaza.db`.

## 3. One hub per folder

`FolderLock::acquire` creates `store.lock` in the folder and takes an exclusive advisory lock on it (`try_lock`). The
file holds `pid <n>` for its holder, and a refusal names that holder. The operating system releases the lock when the
process ends, so a crash never leaves a stale lock.

A second hub on the same folder gets `StoreError::Locked` and does not start. `Store::open` takes the lock before it
touches any file, so the second hub neither migrates nor writes. If the same process opens a folder it has just
closed, the open retries for up to about a second (50 tries, 20 ms apart), because a forked child may briefly hold a
copy of the lock. `Store::restore` takes the same lock (section 6).

## 4. The writer

Each file has one writer: a thread that owns the file's only write connection (`crates/apz-store/src/writer.rs`).
No other code writes to the file, so a write never meets `SQLITE_BUSY`.

Callers send operations. An operation is a closure that receives the connection inside an open transaction and runs
typed functions or prepared statements against it. Callers never send SQL text to the writer. There are three ways
to send one:

- `write(op)` queues the operation and waits for its commit. It returns `Written { value, seq }`: the operation's
  result and the sequence number of the commit that holds it.
- `send(op)` queues the operation and returns at once. A failure is logged with the file's name and counted in
  `WriterStats::failed_ops`.
- `flush()` waits until every operation sent before the call is committed, and returns the committed sequence.

Group commit. When the thread wakes, it takes every operation waiting, up to 1024, and runs them in one
`BEGIN IMMEDIATE` transaction. Each operation runs in its own savepoint. An error or a panic rolls back that operation
alone; the others in the batch still commit. The batch then bumps the commit sequence and commits once, so a burst of
writes costs one sync. A batch commits even when every operation in it failed. If the commit itself fails, every
operation in the batch fails. The queue holds 8192 operations, and a sender waits while it is full.

Sequence. The table `store_commit` holds one row: `seq`, the number of commits made, and `at_ms`, the time of the last
one. The writer creates it on start, outside the numbered migrations. The sequence only grows, across restarts too,
and a backup records it.

Read-your-writes. A reader on another thread that must see a write passes the write's `seq` to
`Db::wait_for(seq, timeout)`, which returns once that commit has happened. A reader that starts after `write()`
returns sees the write, because the commit finished before the reply.

Checkpoints. The writer runs a passive checkpoint after every 256 commits in a burst, and after 250 ms of quiet
following a write. At a quiet checkpoint a WAL larger than 64 MiB is truncated instead. `journal_size_limit` keeps at
most 16 MiB of WAL after a reset. On stop the writer runs `wal_checkpoint(TRUNCATE)`.

Stop. `Store::close` (also run on drop) stops each writer after it commits the operations it already holds.
Operations sent after the stop are not run, and a `write` caller waiting on one gets `StoreError::Closed`.

`WriterStats` counts commits, operations, failed operations, the largest batch and checkpoints. Tests and the hub's
metrics read them.

## 5. Readers

Each file has a pool of reader connections (`crates/apz-store/src/readers.rs`), sized to the CPUs
(`available_parallelism`, or 4 when that is unknown). Connections open when first needed, up to the pool size. A
caller takes one, uses it on one thread and gives it back when it drops; a connection left inside a transaction is
rolled back first. A caller waits up to 30 s for a free connection, then fails with "every reader connection has been
busy".

`Db::read(f)` runs `f` inside one read transaction, so every statement in `f` sees the same snapshot. Under WAL a
reader never waits for the writer.

## 6. Migrations, backups and restore

Migrations are SQL embedded at compile time with `include_str!` in `crates/apid/src/store/mod.rs`:
`PLAZA_MIGRATIONS`, `TRANSCRIPT_MIGRATIONS` and `METRIC_MIGRATIONS`. Each list is numbered 1, 2, 3 with no gaps, and
an out-of-order list is refused at start. The migrations shipped today are `sql/plaza/0001_init.sql`,
`sql/transcripts/0001_init.sql` and `sql/metrics/0001_init.sql`.

A file records its state in two places:

- `user_version` is the schema version: the number of the last applied migration.
- `schema_migration` has one row per applied migration: its version, name, `sha256` of its SQL text and the time it
  was applied.

`migrate::apply` runs when a file opens, before its writer starts, on the writer's connection:

1. A file whose schema version is above the newest migration in this build is refused (`StoreError::Newer`). The hub
   that wrote it has to run.
2. The `sha256` of each applied migration is compared with its embedded text. A migration whose text has changed is
   refused (`StoreError::Changed`). Applied migrations are append-only.
3. If the file was already migrated (schema version 1 or higher) and there is something to apply, the file is backed
   up first, to `backups/<file stem>-v<version>-<ms>.db`. A file at version 0 has nothing to keep and is not backed
   up. The condition is the schema version, not the number of rows.
4. Each pending migration runs in its own transaction, together with its `schema_migration` row and the new
   `user_version`. A migration must not contain `BEGIN` or `COMMIT`, since it already runs inside a transaction.

Backups. `Store::backup_to(dest)` copies every file of the store into `dest`. The folder is created if it is missing,
and files already there are replaced. For each file, in order:

1. The writer flushes, so everything sent so far is in the file.
2. A reader connection copies the file with SQLite's online backup API, in one step. The copy is one consistent
   snapshot of that file, taken while writes go on.
3. The copy is checked with `PRAGMA integrity_check` and switched to journal mode `DELETE`, so it has no WAL to carry.
4. Its schema version and commit sequence are read from the copy.

Then `MANIFEST` is written in `dest`, one `key=value` line per field, for example:

```
taken_ms=1760000000000
file=plaza.db schema=1 seq=4812 bytes=3145728
file=transcripts.db schema=1 seq=233 bytes=81920
file=metrics.db schema=1 seq=101 bytes=49152
```

The files are copied one after another, not at one instant. A write that commits between two copies is in one file's
backup and not in another's. Each file's `seq` says where that file's copy stands. Copying a running hub's folder
with file tools is not a backup; `backup_to` is.

Restore. `Store::restore(from, dir)` takes `dir`'s folder lock, so no hub may be running on it. It reads `MANIFEST`,
checks each listed file with `integrity_check`, removes the target's `-wal` and `-shm` files, and copies each listed
file over the target. The copy goes file by file and is not atomic: a failure part-way leaves some files restored and
others not. The next `Store::open` on the folder migrates the restored files as usual, and refuses one written by a
newer hub.

Who calls them. `backup_to` and `restore` are functions of `apz-store`. Nothing in the hub calls them: there is no
`apid` command or schedule for a backup, and their only callers are the store's tests. The migration backups in
`backups/` are the only backups the hub takes by itself.

## 7. Components and the saver

The hub's state of record is held by components (`crates/apid/src/store/components.rs`). Each component has four
parts:

- `name`, used in logs and in the comments of the printed DDL;
- `def`, its tables: a `TableDef` derived from its Rust type;
- `rows`, its state as a `RowSet`, every row of its tables;
- `load`, which replaces its in-memory state with what its tables hold.

`components::all()` lists the 46 components in load order. A component whose load reads another component's state
comes after it. The work engine's records are the components `sessions`, `seats`, `budgets`, `cards`, `chats` and
`threads`, and the budget ledger is written through (below).

The working copy. Each component keeps its state in memory, and the hub serves requests from that copy. The tables
hold the saved copy, which the saver keeps equal to the working copy.

Load. At start, `saver::load` runs each component's `load` inside one read transaction, reads back the rows the
tables hold, and records those rows as the saved state. A component whose load fails is reported and held out: it is
not written while the hub runs, so a save cannot put defaults over stored rows. It is written again after a restart.

Save. The saver (`crates/apid/src/store/saver.rs`) runs a pass every second and once more at shutdown. For each
component, its rows now are diffed against the saved state, row by row, keyed by each row's key. New and changed rows
are upserted, and rows whose key has gone are deleted. The changes are applied in the writer's transaction. When that
commit succeeds, the new rows become the saved state. A failed write leaves the saved state as it was, so the next
pass computes the same diff again. Components are saved one at a time. `save_now(app, name)` saves one component at
once and returns after its commit, for a change that must be stored before the hub replies.

Written-through tables. The 30 definitions in `components::written_through()` are written by their own code as events
happen, not by the saver. They are: the budget ledger (`budget_entry`); the vault and its audit; usage calls, hours and
works; the IDE audit; the station audit, recordings, recording events, recording frames and sync files; the unfurl
cache; prompts with their versions and events; hub MCP tokens, digests, calls and settings; shares and share events;
skills with their versions, files, findings and events; and artifacts with their versions and blobs. Their DDL is
checked the same way as the components'.

Transcripts and metrics. These two files have no component. `crates/apid/src/transcript.rs` appends `entry` rows as
transcript lines are written, and `restore_transcripts` reads them back at start. The metrics store
(`crates/apid/src/metrics/sqlite.rs`) collects samples, rollups, events and model calls, and writes each batch in one
writer operation.

## 8. Typed tables

`#[derive(apz_store::Table)]` maps a struct to one `STRICT` table with a typed column for each field, and any
collections in it to child tables. `#[derive(apz_store::Column)]` maps an enum or a newtype to one column. Nothing is
stored as JSON.

Column types, from the `Column` implementations in `crates/apz-store/src/table.rs`:

| Rust type | SQLite column |
| --- | --- |
| `i8` to `i64`, `isize` | INTEGER |
| `u8` to `u64`, `usize` | INTEGER with `CHECK (x >= 0)`; a value past `i64` is refused |
| `bool` | INTEGER with `CHECK (x IN (0, 1))` |
| `f32`, `f64` | REAL; NaN is refused |
| `String`, `PathBuf` | TEXT (a path must be UTF-8) |
| `Vec<u8>` | BLOB |
| `Option<T>` | the column of `T`, nullable |
| unit-variant enum (`derive(Column)`) | TEXT with `CHECK (x IN ('name', ...))`; names follow serde's rules |
| newtype (`derive(Column)`) | the column of the inner type |

Field attributes (`#[table(...)]`):

- `key`: the field is part of the table's primary key.
- `rename = "col"`: the column's name.
- `skip`: not stored; the field loads as `Default`.
- `child`: a struct field becomes a child table with one row (`Default` on load when the row is missing). An
  `Option<T>` field becomes a child table with at most one row. A `Vec<T>` or `VecDeque<T>` field becomes a child
  table with one row per item.
- `list`: a `Vec` of plain values, one row each, with a `<field>_pos` column and a `value` column.
- `set`: a set of plain values; the value is the row's key.
- `map`: a map to structs, one child row per entry, keyed by `<field>_key`.
- `map_value`: a map to plain values, keyed by `<field>_key`, with a `value` column.
- `flatten`: a sub-struct, or an enum with data, stored as columns of the same row, prefixed `<field>_`.
- `content`: text kept as written, in a `<field>_content` column. It is never read back as state.
- `index`, `unique`: an index on the column.
- `child = "name"` (likewise `list`, `set`, `map`, `map_value`) names the child table. The default is
  `<parent table>_<field>`.

Keys and positions:

- A root struct with no `key` field is keyed by `pos`, its position in the list it is stored as.
- A root with `key` fields uses them as its primary key, and keeps the list order in a `pos` column.
- A child's key starts with its parent's key. For a root parent, the root's key columns appear as
  `<table>_<column>`. Each child has a foreign key to its parent, `ON DELETE CASCADE`, deferred to commit.
- An enum with data is stored flattened: a `kind` column (checked against the variant names) and a nullable column
  for each variant field, named `<variant>_<field>`.

Saved state. The writer's diff needs each row's content and key. `Saved` (`crates/apz-store/src/table.rs`) keeps a
hash per row and key, so a save writes only the rows whose hash changed.

An example, trimmed from `crates/apid/src/settings.rs`:

```rust
#[derive(Debug, Clone, apz_store::Table)]
#[table(name = "settings")]
struct SettingsRow {
    default_model: String,
    #[table(map_value)]
    role_models: BTreeMap<String, String>,
    #[table(child)]
    model_policy: Option<ModelPolicy>,
    #[table(list)]
    fetch_block: Vec<String>,
}

pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(SettingsRow)
}
```

`root_def!` derives the definition once and keeps it in a static.

## 9. STRICT tables, the lint and what stays out of the store

The lint is in `crates/apz-store/src/lint.rs`.

- `lint::schema` fails a database that has a table which is not `STRICT`, a column whose type is not INTEGER, INT,
  REAL, TEXT or BLOB (so `ANY` fails), or a column whose name contains `json`.
- `lint::sql` fails a query whose code contains `json` (a JSON function, type or name) or the `->` operator. Comments
  and quoted strings are ignored.
- `*_content` columns pass. They are TEXT and not JSON-named. The lint cannot see whether a content column is read
  back as state; that is a rule for review.

The lint runs in tests: the store's own tests, several component tests, and `crates/apid/src/store/tests.rs`. It does
not run when the hub starts.

Outside the store, the state folder holds JSON that the lint does not see:

- `tasks-archive.jsonl`: `save_archives` in `crates/apid/src/state.rs` appends the task ledger's archived lines, one
  JSON object per line.
- `foundry/`: the foundry's dataset files, which are JSON Lines.

Content inside the store is text. `apid` enables the `json` feature of `apz-store`, which lets a `content` field be a
`serde_json::Value`; it is stored as its serialized text in a `*_content` column.

## 10. Reading the schema

- `apid schema` prints the DDL of the components' tables and of the written-through tables to stdout. It runs before
  the hub reads its configuration and does not open the state folder. The components' part of
  `crates/apid/sql/plaza/0001_init.sql` is this output, with a header comment.
- The transcripts and metrics schemas are written by hand, in their own `0001_init.sql`.
- To see what a live file holds, open a copy (a backup), not the live file, with any SQLite client. Useful statements:
  `PRAGMA user_version` (the schema version), `SELECT version, name, applied_ms FROM schema_migration` (the applied
  migrations) and `SELECT seq, at_ms FROM store_commit` (the commit sequence).

The test `every_component_table_is_in_the_migration_as_defined` (`crates/apid/src/store/tests.rs`) checks every
component and written-through table of the migrated `plaza.db` against its definition. It compares column names,
types, NOT NULL flags and primary-key positions. It does not compare CHECK constraints, foreign keys or indexes.

## 11. Limits

- A backup is consistent per file, not across files (section 6).
- A restore is not atomic (section 6).
- The failure message of the schema test says to regenerate the components' part of `0001_init.sql`. That is right only
  while no database has applied `0001`. Once one has, the store refuses any edit to it (`StoreError::Changed`), so the
  change is a new migration (`docs/dev/sql.md`).
- The migration backup depends on the schema version, not on the row count (section 6).
- The schema test does not compare CHECK constraints, foreign keys or indexes (section 10).

## 12. The object store

Bytes (the Library's files, artifact and skill files, station data) are kept by the `apz-blobs` crate in `blobs/`.
They are append-only packs of blobs, each addressed by its BLAKE3 hash, with a SQLite index (`index.db`) that says
where each blob lives. The packs are the record. The format and the chunking are described in the module docs of
`crates/apz-blobs/src/lib.rs`.

## 13. Tests

- `cargo test -p apz-store`: the migration unit tests (order, applied once, newer file refused, edited migration
  refused, gaps refused); the folder lock test; the lint tests; `tests/store.rs`, which covers writes readable by
  their writer, failed operations rolled back alone, sequence numbers across restarts, a second hub refused, a newer
  schema refused, FTS5 compiled in, a backup taken during writes and restored, owner-only modes, and a load test with
  32 reader threads and 10,000 writes that must see no busy error; and `tests/tables.rs`, which covers derive round
  trips, saves that write only changed rows, and the lint on generated schemas.
- `cargo test -p apid store::tests`: the four checks in `docs/dev/sql.md`.
