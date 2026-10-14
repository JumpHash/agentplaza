# SQL in apid

apid keeps its state in Postgres (`APZ_DATABASE_URL`) and falls back to files without it. All SQL
lives in `crates/apid/sql/`, as `.sql` files embedded at compile time. No Rust file holds a SQL
statement to run as a string literal; the test-only `CREATE DATABASE` template is a file too.

## Layout

```
crates/apid/sql/
  migrations/
    0001_init.sql … 0013_station_recording_frames.sql
                             one migration per file, numbered from 0001, never edited once applied
    migrations.lock          one line per migration: NNNN name sha256
  queries/                   92 files, one statement each; the first comment names the parameters
    <area>_<verb>.sql        areas: state_, transcript_, infraction_, agents_, prompt_, skill_,
                             hub_mcp_, ide_audit_, station_audit_, station_recording_, station_sync_,
                             unfurl_, migration_, data_migration_
  test/                      SQL used only by tests (legacy schema, schema description, forbid probes,
                             CREATE DATABASE)
crates/apid/src/db.rs        the Db: connection, dispatch, component snapshots, the restore at start, the writer
crates/apid/src/db/sql.rs    one `pub const` per query file, each `include_str!`
crates/apid/src/db/migrate.rs  MIGRATIONS (the embedded list) and the runner
crates/apid/src/db/data_migrate.rs  data migrations, run once the schema is current
crates/apid/src/db/files.rs  the file fallback (see "Without Postgres")
crates/apid/src/db/        hub_mcp.rs, recordings.rs, roster.rs, sync.rs: the per-area store modules
scripts/test-db.sh           runs the db tests against a throwaway Postgres 17 container
scripts/migrations.sh        lock | check, for migrations.lock
scripts/check-migrations.sh  the immutability check (CI, and the pre-commit hook)
scripts/test-check-migrations.sh  tests for the check, in throwaway git repositories
.githooks/pre-commit         runs check-migrations.sh --staged
scripts/install-hooks.sh     sets core.hooksPath to .githooks (run it once per clone)
```

## How to add a query

1. Write `crates/apid/sql/queries/<area>_<verb>.sql`. The first line(s) are `--` comments: what the
   statement does, and each parameter as `$n = name (type)`.
2. Add `pub const NAME: &str = include_str!("../../sql/queries/<area>_<verb>.sql");` to
   `crates/apid/src/db/sql.rs`. Add `#[cfg(test)]` above it when only tests use it.
3. Call it as `sql::NAME` from `db.rs` (or the module that needs it).

The test `db::sql::tests::every_query_file_has_a_const_and_every_const_has_a_file` fails when a file
has no const, a const includes no file, or a file is empty or lacks the leading `--` comment.

## How to add a migration

Never edit a migration that has been applied anywhere, including a developer's database. The
runner refuses to start when an applied file's checksum no longer matches, and the CI check refuses
the change.

1. Create `crates/apid/sql/migrations/NNNN_name.sql` with the next number (`0002`, `0003`, …), one
   change set per file, and a leading `--` comment.
2. Add the entry to `MIGRATIONS` in `crates/apid/src/db/migrate.rs`:
   `Migration { version: 2, name: "name", sql: include_str!("../../sql/migrations/0002_name.sql") }`.
3. Append its line to the lock: `scripts/migrations.sh lock`. It appends lines for new files only and
   refuses (writing nothing) if a locked file has changed.
4. Run `cargo test -p apid db::migrate` and `scripts/test-check-migrations.sh`.

The migration runs in its own transaction with its `schema_migrations` row, so a failure leaves
nothing behind. Do not put `BEGIN`/`COMMIT` inside a migration file.

## Migrations at start

`Db::connect` (called once at start) runs `migrate::migrate`:

- takes the advisory lock `APZMIG` (key `0x41505a4d4947`), so concurrent runners take turns;
- creates `schema_migrations (version, name, checksum, applied_at)` if missing;
- checks every recorded migration against this build: the name and sha256 must match the file, and
  a recorded migration this build does not ship is refused (an older build against a newer database);
- applies each missing migration in order, each in a transaction with its row, and refuses a gap
  (a later migration applied while an earlier one is not);
- releases the lock.

Then `data_migrate::run` runs the data migrations (`crates/apid/src/db/data_migrate.rs`); a failure there
also exits with status 1.

A refusal prints `db: refusing to start: …` naming the file and exits with status 1. A database
that is not reachable at start is not a refusal: apid logs `db: connect failed` and runs on files.

`0001_init.sql` reproduces the tables the code created before migrations existed, with `IF NOT
EXISTS`. A database made by the old code therefore migrates cleanly: 0001 is recorded and the rows
are kept. The test `a_legacy_schema_migrates_and_records_0001` checks the structure is identical.

## Immutability: the lock, the CI check, the hook

`crates/apid/sql/migrations/migrations.lock` pins each migration's sha256 in git. Three checks keep
applied migrations unchanged:

- `cargo test -p apid db::migrate` asserts the folder, the embedded list and the lock agree.
- `scripts/check-migrations.sh <base-ref>` compares the merge base of `<base-ref>` and `HEAD` with
  `HEAD`, and fails when a migration file is modified, deleted or renamed, an existing lock line
  changed or was removed, or a new migration number is not exactly the base's maximum + 1, + 2, …
  The message names the file and says "applied migrations are immutable: add a new migration instead".
- `scripts/check-migrations.sh --staged` runs the same comparison on the index against `HEAD`.

CI runs the check in `.github/workflows/migrations.yml` on a push to main, against
`github.event.before`. No workflow runs on pull requests or on other branches. The job runs on
the org's self-hosted runner (`runs-on: [self-hosted, Linux, X64, hetzner]`).

The pre-commit hook is `.githooks/pre-commit`. It does nothing until the clone points git at it.
Run this once per clone, if you want the hook (the owner decides whether to install it):

```sh
scripts/install-hooks.sh     # sets: git config core.hooksPath .githooks
```

Check the tooling: `scripts/test-check-migrations.sh` (14 cases in throwaway repositories: modify,
delete, rename, a new migration, a gap (two cases), reuse, lock edit, lock removal, lock append, the lock
tool refusing an edited file, and the three staged cases). Set `TMPDIR` to choose where its repositories go.

## Running the database tests

The db tests need a Postgres. They never use `APZ_DATABASE_URL`, the owner's database, or a
container that is not theirs. They read `APZ_TEST_DATABASE_URL` and create their own database per
test (`apz_test_<label>_<pid>`) on that server, then leave it behind for the server to drop.

```sh
scripts/test-db.sh            # cargo test -p apid db, then the registry's database test
scripts/test-db.sh migrate    # only tests matching a filter
```

The script:

- requires the image `postgres:17` to be present (it does not pull; `docker images` shows it);
- starts `docker run --rm -d` on a random free port of `127.0.0.1`, with a random password, and
  waits for `pg_isready`;
- exports `APZ_TEST_DATABASE_URL` and unsets `APZ_DATABASE_URL`;
- removes the container on exit, including on failure.

Without `APZ_TEST_DATABASE_URL` the db tests print `db: APZ_TEST_DATABASE_URL not set; skipping` and
pass without checking anything, so a passing run without the script proves nothing about the database.
`test_database` refuses a `APZ_TEST_DATABASE_URL` equal to `APZ_DATABASE_URL`.

## Without Postgres: the file fallback

With no `APZ_DATABASE_URL` (or when the connection fails), apid keeps the plaza's component snapshots in
their own files, as before. The rows that have no file of their own go to files under the state directory
(`db::files`): transcripts, infractions, the IDE audit, the station audit and the station recordings. The
same operations and orders as the Postgres queries apply.

```
<state_dir>/transcripts/<cid>.jsonl          the live file, one record per line
<state_dir>/transcripts/<cid>.1.jsonl        the previous generation
<state_dir>/transcripts/<cid>.2.jsonl        the one before (older ones are dropped)
<state_dir>/infractions.jsonl (.1, .2)       the same, for infractions
<state_dir>/ide-audit.jsonl (.1, .2)         the same, for the IDE audit
<state_dir>/station-audit.jsonl (.1, .2)     the same, for the station audit
<state_dir>/station-recordings.jsonl (.1, .2)          the recordings' index: a put or a delete per line
<state_dir>/station-recording-events.jsonl (.1, .2)    the recordings' actions
<state_dir>/station-recording-frames.jsonl (.1, .2)    the capture time of each frame
```

- **Records.** A transcript line is `{"seq","work","kind","at_ms","data"}`; an infraction line is
  `{"id","cid","law","at_ms","tool","detail"}`, with `id` counting from 1 as a bigserial does. The other
  files hold one row per line, in the shape of their table.
- **Writes.** Each record is written when it is appended (a `write` to an open file; no buffer).
  The periodic writer (every two seconds) fsyncs what was written since its last run.
- **Rotation.** A file rotates once it reaches 32 MiB: `.1` moves to `.2` (the old `.2` is dropped)
  and the live file starts again. Rotation fsyncs the file it retires and its folder.
- **Duplicates.** A transcript seq already stored for the cid in a kept generation is ignored, as
  `ON CONFLICT DO NOTHING` ignores it. A seq above the cid's highest stored seq is appended without a
  scan. Unlike Postgres, a seq rotated out of the two kept generations is forgotten.
- **Pages.** `transcript(cid, n)` returns the newest `n`, oldest first; infraction pages are newest
  first by `(at_ms, id)`. Pages read backwards from the end of the files in 64 KiB blocks, so a page
  costs its own size. Ordering is exact when records are appended in seq order and in `at_ms` order,
  which the agents' store and the plaza's clock guarantee.
- **Torn lines.** A line that does not parse (a write cut short by a crash) is skipped on read. The
  next record is written on a fresh line, so it never joins the torn bytes.
- **Restart.** At start the transcripts of every cid are loaded back (up to 500 per cid, as with
  Postgres), and the infraction id and the per-cid high seq are read from the tails of the files.
- **Shutdown.** apid has no graceful shutdown hook: a `SIGTERM` ends the process without a final
  fsync. Writes reach the operating system at once, so a process crash loses nothing; the periodic
  fsync bounds what a power loss can take to about two seconds of records.

The tests run the same expectations against both backends (`db::tests::every_query_behaves_the_same_on_both_backends`),
and `db::files::tests` cover the reverse reader, rotation, duplicates after rotation, a torn line,
and infraction ids across a restart.
