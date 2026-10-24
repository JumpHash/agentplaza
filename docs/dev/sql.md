# SQL in apid

The hub's state is SQLite, through the crate `apz-store` (`crates/apz-store/`). The design is in
`docs/design/database.md`. This file is the how-to: where things go, and how to add a component, a table that is
written as events happen, a migration or a query, and which checks run.

## Where things live

```
crates/apid/src/store/
  mod.rs          the three files (FILES), their migration lists, open(), `apid schema`
  components.rs   the 46 components in load order, and written_through() (30 definitions)
  saver.rs        loads the components at start; writes what changed once a second
  sql.rs          one pub const per query file; QUERIES (test only) names each one's store file
  tests.rs        the four checks listed below
crates/apid/sql/
  plaza/0001_init.sql, transcripts/0001_init.sql, metrics/0001_init.sql   the migrations
  queries/        one statement per file (140 files)
crates/apz-store/src/   the lock, writer, readers, migrations, backups and lint
crates/apz-store-derive/src/   derive(Table) and derive(Column)
```

## Adding a component

A component is a part of the hub's state kept in `plaza.db`. Its state is a Rust struct, and its tables are derived
from that struct. The example below is a new component `notes`; it is not in the repository.

1. The state struct. Derive `Table` on it, name its root table with `#[table(name = "...")]`, and mark its lists,
   maps and children as described in `docs/design/database.md` (section 8). Keep the derived type beside the
   component's code, as `settings.rs` and `appearance.rs` do.

   ```rust
   #[derive(Debug, Clone, Default, apz_store::Table)]
   #[table(name = "notes")]
   pub struct Notes {
       pub title: String,
       #[table(list)]
       pub pinned: Vec<String>,
   }

   pub fn def() -> &'static apz_store::TableDef {
       apz_store::root_def!(Notes)
   }
   ```

2. The two methods the saver calls. `rows()` returns the state as a `RowSet`. `load()` replaces the in-memory state
   with what the tables hold. The component holds its state in a `Mutex<Notes>`, as `appearance.rs` does.

   ```rust
   pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
       apz_store::RowSet::one(def(), &self.get())
   }

   pub fn load(&self, conn: &apz_store::rusqlite::Connection) -> Result<(), apz_store::StoreError> {
       *self.lock() = apz_store::table::load_one::<Notes>(conn, def())?.unwrap_or_default();
       Ok(())
   }
   ```

3. Register it in `components::all()` (`crates/apid/src/store/components.rs`), at its place in the load order. The
   component needs a field on `App` (`crates/apid/src/state.rs`), created before `saver::load` runs.

   ```rust
   Component {
       name: "notes",
       def: crate::notes::def,
       rows: |app| app.notes.rows(),
       load: |app, conn| app.notes.load(conn),
   },
   ```

4. Give its tables a place in `plaza.db`. Print the DDL with `apid schema` and copy the lines for the new
   component. Then one of two things:
   - If no store has applied `plaza/0001_init.sql` yet, put the lines into the components' part of that file.
   - If any store has applied it, which is the case for a database an existing hub created, add a new migration: see
     "Adding a migration" below.

5. Run `cargo test -p apid store::tests`. The first check compares every component table of the migrated `plaza.db`
   with its definition, and checks that the component's statements prepare.

## Adding a table that is written as events happen

Use this for a table written by its own code as things happen (a ledger, an audit, a stream), not by the saver.

1. Define the table as a component's is defined: `#[derive(Table)]` and a `def()` in the module that owns it.
2. Add its `def` to `components::written_through()` with a name. The DDL check and `apid schema` then cover it.
3. Put its DDL in `plaza.db` as in step 4 above.
4. Write it through the file's `Db` (`app.store.db(crate::store::PLAZA)`): `write` waits for the commit and returns
   its sequence number, `send` queues the write and returns at once. Read it with `db.read`.

   ```rust
   db.write(move |conn| {
       conn.prepare_cached(sql::METRIC_SAMPLE_PUT)?
           .execute(params![metric, t_ms, series, value])?;
       Ok(())
   })?;
   ```

   A batch of events is best written as one operation, as `crates/apid/src/metrics/sqlite.rs` does.

For a table in `transcripts.db` or `metrics.db`, add its migration to that file's list in `store/mod.rs`
(`TRANSCRIPT_MIGRATIONS`, `METRIC_MIGRATIONS`). Those files have no component, so only the migrate-and-lint check
covers them.

## Adding a migration

- Name the file for the next number in the store file it changes: `crates/apid/sql/<file>/0002_<name>.sql`.
- Add a `Migration` at the end of that file's list in `crates/apid/src/store/mod.rs`:

  ```rust
  Migration {
      version: 2,
      name: "notes",
      sql: include_str!("../../sql/plaza/0002_notes.sql"),
  },
  ```

- The version must be the next number. The list is checked at start, and a gap or an out-of-order entry is refused.
- Write the SQL that brings the schema to the new state. For a table in `plaza.db`, the schema check compares the
  migrated table with its definition, so the migration has to produce the same names, types, NOT NULL flags and key
  positions. A new table's DDL is the output of `apid schema`.
- Never edit a migration that has been applied. The store hashes each applied migration and refuses to open a file
  whose migration text differs (`StoreError::Changed`).
- Do not write `BEGIN` or `COMMIT`. The migration already runs inside a transaction.
- Regenerating `0001_init.sql` is allowed only while no store has applied it. The schema check's failure message
  tells you to regenerate the components' part of `0001_init.sql`; that advice holds only in that case.
- A file at version 1 or higher is copied to `backups/` before a migration runs on it. A new file is not.

## Adding a query

Hand-written SQL lives one statement per file, in `crates/apid/sql/queries/<area>_<verb>.sql`. The area prefix groups
the files (`metric_`, `usage_`, `prompt_`, `station_`, and so on).

1. Create the file. Its first line is a `--` comment: what the statement does, and then each parameter as
   `?n = name`.

   `crates/apid/sql/queries/prompt_count_live.sql`:

   ```sql
   -- The number of prompts that are not archived. No parameters.
   SELECT count(*) FROM "prompt" WHERE "archived_ms" IS NULL
   ```

2. Add a const in `crates/apid/src/store/sql.rs`, in the section for its area:

   ```rust
   pub const PROMPT_COUNT_LIVE: &str = include_str!("../../sql/queries/prompt_count_live.sql");
   ```

3. Add its line to `QUERIES` in the same file, naming the store file it runs against: `plaza`, `transcripts` or
   `metrics`.

   ```rust
   ("plaza", "prompt_count_live", PROMPT_COUNT_LIVE),
   ```

4. Use it through a connection. `prepare_cached` compiles it once per connection:

   ```rust
   use apz_store::StoreError;
   use apz_store::rusqlite::Connection;
   use crate::store::sql;

   fn live_prompts(conn: &Connection) -> Result<i64, StoreError> {
       Ok(conn
           .prepare_cached(sql::PROMPT_COUNT_LIVE)?
           .query_row([], |r| r.get(0))?)
   }
   ```

   Read with `db.read(|conn| live_prompts(conn))`. Write with `db.write(...)`, as in the section above.

Nothing checks that every const is in `QUERIES`. A query that is missing from that list is not prepared or linted by
the test, so add the line yourself.

## The checks

These are in `crates/apid/src/store/tests.rs`, run with `cargo test -p apid store::tests`.

- `every_file_migrates_and_passes_the_lint`: each store file migrates from empty and passes `lint::schema`.
- `every_component_table_is_in_the_migration_as_defined`: every component table and every written-through table of
  the migrated `plaza.db` has the columns, types, NOT NULL flags and key positions of its definition. Each
  component's upsert, delete and select statements prepare against the schema. A failure names the table and prints
  both versions.
- `every_query_prepares_against_its_file_and_passes_the_lint`: each `QUERIES` entry prepares against its own migrated
  file and passes `lint::sql`.
- `every_query_file_has_one_const_and_every_const_has_a_file`: each file under `queries/` is included by exactly one
  const in `sql.rs` (the test reads the `include_str!` lines), each is non-empty, and each starts with `--`.

The schema check does not compare CHECK constraints, foreign keys or indexes. Review those by reading the output of
`apid schema`.

## The lint

`crates/apz-store/src/lint.rs` keeps JSON out of the store.

- Schema: every table is `STRICT`, every column has a concrete type (INTEGER, INT, REAL, TEXT or BLOB; `ANY` fails),
  and no column name contains `json`.
- Queries: no JSON function, type or name, and no `->` operator. Comments and quoted strings are ignored, so a word
  inside them is fine.
- Content (a message's text, a tool's input as the model wrote it) goes in a column named `*_content`. The lint
  allows it. Such a column is never read back as state.

## Common mistakes

- A query that calls `json_extract`, `json_each` or uses `->`: the lint fails it. Read the typed columns instead.
- A query file that does not start with `--`, or that two consts include: the fourth check fails.
- A query const that is not in `QUERIES`: not prepared, not linted.
- Changing a component's struct without a migration: the first check fails once a store has applied `0001`.
- Editing an applied migration: the store refuses to open that file (`StoreError::Changed`).
