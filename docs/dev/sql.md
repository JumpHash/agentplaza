# SQL in apid

apid keeps its state in SQLite, through the crate `apz-store` (`crates/apz-store/`). The state lives in three
files in the state folder. The components' tables are defined in Rust, and their DDL is printed from those
definitions. The hand-written statements live one per file in `crates/apid/sql/queries/`.

## The three files

- `plaza.db` is the hub's state of record: every component's tables, the work engine's records, the audits and
  the settings. It runs with `synchronous = FULL` (and `fullfsync` on macOS), so an acknowledged write survives a
  power loss.
- `transcripts.db` holds the transcripts, written through as they happen.
- `metrics.db` holds the metric samples and rollups, events, model calls and kept traces.

Both large files run with `synchronous = NORMAL`. All three use WAL. Their rows carry the ids of what they belong
to in `plaza.db`. A lock file, `store.lock`, sits in the state folder: one hub per folder, and a second one
refuses to start.

```
crates/apid/src/store/
  mod.rs          the three files, their migration lists, open()
  components.rs  the components' tables, as derive(apz_store::Table) definitions
  saver.rs        writes changed component state through plaza.db's one writer
  sql.rs          one pub const per query file; QUERIES lists each with its file (test only)
  tests.rs        the checks listed below
crates/apid/sql/
  plaza/0001_init.sql          the components' tables, as `apid schema` prints them
  transcripts/0001_init.sql
  metrics/0001_init.sql
  queries/                     one statement per file
crates/apz-store/              the store: lock, migrations, lint, backups, writer
```

The components keep an in-memory working copy. The saver turns what changed into rows and writes them through
the file's writer. Streams (transcripts, audits, metric samples) are written as they happen.

## Migrations

- Each file has its own folder of migrations, embedded at compile time with `include_str!` in `store/mod.rs`
  (`PLAZA_MIGRATIONS`, `TRANSCRIPT_MIGRATIONS`, `METRIC_MIGRATIONS`). A file's `user_version` is the number of its
  last applied migration.
- Each migration runs in one transaction together with its row in `schema_migration` (name and sha256). The
  migration file must not contain `BEGIN` or `COMMIT`.
- An applied migration is immutable. When a store opens, apz-store compares each applied migration's sha256 with
  the embedded text, and refuses a file whose text differs or whose version is newer than this build.
- A change is a new file with the next number, for example `crates/apid/sql/plaza/0002_<name>.sql`, added to that
  file's list in `store/mod.rs`. Never edit `0001_init.sql` once any database has applied it.
- A file that is already at an earlier version is copied to `backups/` in the state folder before the migration
  runs. The copy is named `<file>-v<version>-<ms>.db`. A file that has never been migrated is not backed up.
- The test `every_component_table_is_in_the_migration_as_defined` compares the migrated `plaza.db` with the
  component definitions. A change to a table's definition therefore needs a migration that brings the table to
  match. `cargo run -p apid -- schema` prints the components' DDL, the source for the components' part of
  `plaza/0001_init.sql`.

## Queries

- One statement per file: `crates/apid/sql/queries/<area>_<verb>.sql`. The area prefix groups the files (`metric_`,
  `usage_`, `prompt_`, `station_`, and so on).
- Line 1 onward is a `--` comment: what the statement does, then each parameter as `?n = name`, for example
  `-- ?1 = prompt id`.
- Register the file in `crates/apid/src/store/sql.rs`: one `pub const NAME: &str = include_str!(...)` for it, and one
  `(file, name, NAME)` line in `QUERIES`. The file is `plaza`, `transcripts` or `metrics`, the store file the
  statement runs against. `QUERIES` is test-only; the test below prepares every entry.

## The lint

apz-store's lint (`crates/apz-store/src/lint.rs`) forbids JSON in the store.

- Schema: every table is `STRICT`, every column has a concrete type (`ANY` is refused), and no column name
  contains `json`.
- Queries: no JSON function or type, and no `->` operator.
- Content (a message's text, a tool's input as the model wrote it) goes in columns named `*_content`. The lint
  allows those, and they are never read back as state.

## Tests

Run them with `cargo test -p apid store::tests`. The tests in `crates/apid/src/store/tests.rs` are:

- `every_file_migrates_and_passes_the_lint`: each file migrates from empty and passes the schema lint.
- `every_component_table_is_in_the_migration_as_defined`: the migrated `plaza.db` has every component table as
  defined.
- `every_query_prepares_against_its_file_and_passes_the_lint`: each `QUERIES` entry prepares against its own
  migrated file and passes the query lint.
- `every_query_file_has_one_const_and_every_const_has_a_file`: each file under `queries/` is included by exactly one
  const, every include is a file, and each file is non-empty and opens with a `--` comment.
