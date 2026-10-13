//! The data step of the move to tables (docs/design/database.md section 4). After the SQL
//! migrations have applied, `run` moves each domain from its source (its `plaza_state` row, or its
//! component file) into its tables, once, and records the outcome in `data_migrations`. It runs
//! under the `APZMIG` advisory lock, the same lock as the schema migrations.
//!
//! Only `agents` exists so far. A real domain adds one step function and one `DOMAINS` entry, in
//! dependency order, with its migration.

use postgres::Client;
use serde::Deserialize;

use super::migrate::LOCK_KEY;
use super::sql;

/// Where a domain's rows came from (the `data_migrations.source` CHECK).
#[derive(Clone, Copy)]
pub enum Source {
    Blob,
    #[allow(dead_code)] // the first domain that reads a component file uses it
    File,
    None,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Source::Blob => "blob",
            Source::File => "file",
            Source::None => "none",
        }
    }
}

/// What one domain's step did. `status` is `verified` (the tables hold the source and the source
/// is no longer read) or `pending` (the source is still the truth and nothing was loaded).
pub struct Outcome {
    pub status: &'static str,
    pub source: Source,
    pub source_sha256: Option<String>,
    pub rows_expected: i64,
    pub rows_loaded: i64,
    pub detail: String,
}

/// One domain's step: reads its source and loads it when it can.
type Step = fn(&mut Client) -> Result<Outcome, String>;

/// Every domain, in dependency order (section 4.1). Only `agents` exists yet.
const DOMAINS: &[(&str, Step)] = &[("agents", agents)];

/// Runs every domain's step under the lock. A domain already `verified` is skipped. The first
/// failure stops the start: its row is recorded `failed` and the error names the domain (fail
/// closed, section 4.2).
pub fn run(client: &mut Client) -> Result<(), String> {
    client
        .query(sql::MIGRATION_LOCK, &[&LOCK_KEY])
        .map_err(|e| format!("db: data migration: take the lock: {e}"))?;
    let result = run_domains(client);
    let unlocked = client.query(sql::MIGRATION_UNLOCK, &[&LOCK_KEY]);
    result?;
    unlocked.map_err(|e| format!("db: data migration: release the lock: {e}"))?;
    Ok(())
}

fn run_domains(client: &mut Client) -> Result<(), String> {
    for &(domain, step) in DOMAINS {
        let status = client
            .query_opt(sql::DATA_MIGRATION_STATUS, &[&domain])
            .map_err(|e| format!("db: data migration {domain}: read status: {e}"))?;
        if status.is_some_and(|row| row.get::<_, String>(0) == "verified") {
            continue;
        }
        match step(client) {
            Ok(outcome) => {
                record(client, domain, &outcome)?;
                eprintln!("db: data migration {domain}: {} ({})", outcome.status, outcome.detail);
            }
            Err(detail) => {
                if let Err(e) = client.execute(sql::DATA_MIGRATION_FAIL, &[&domain, &detail]) {
                    eprintln!("db: data migration {domain}: record the failure: {e}");
                }
                return Err(format!("db: data migration {domain} failed: {detail}"));
            }
        }
    }
    Ok(())
}

fn record(client: &mut Client, domain: &str, outcome: &Outcome) -> Result<(), String> {
    client
        .execute(
            sql::DATA_MIGRATION_RECORD,
            &[
                &domain,
                &outcome.status,
                &outcome.source.as_str(),
                &outcome.source_sha256,
                &outcome.rows_expected,
                &outcome.rows_loaded,
                &outcome.detail,
            ],
        )
        .map(|_| ())
        .map_err(|e| format!("db: data migration {domain}: record: {e}"))
}

/// One agent as the `agents` blob holds it: a JSON array of these. The shape belongs to this
/// migration (the runtime keeps the roster in memory), so only this step reads it.
#[derive(Deserialize)]
struct BlobAgent {
    cid: u64,
    name: String,
    role: String,
    model: Option<String>,
    #[serde(default)]
    species: String,
    species_at_ms: Option<i64>,
    /// `active` or `retired`; absent means `active`.
    state: Option<String>,
    created_at_ms: Option<i64>,
    retired_at_ms: Option<i64>,
    updated_at_ms: Option<i64>,
}

/// `agents`: the source is the `plaza_state` row `agents`, read with its sha256. No row means the
/// domain is verified with nothing to load. A row is loaded in one transaction: the table is
/// emptied, every agent inserted, and the row count checked against the blob's length before the
/// commit. Any failure rolls the transaction back, so the table keeps its earlier rows.
fn agents(client: &mut Client) -> Result<Outcome, String> {
    let Some(row) = client
        .query_opt(sql::AGENTS_SOURCE, &[])
        .map_err(|e| format!("read the agents source: {e}"))?
    else {
        return Ok(Outcome {
            status: "verified",
            source: Source::None,
            source_sha256: None,
            rows_expected: 0,
            rows_loaded: 0,
            detail: "no agents source".to_string(),
        });
    };
    let text: String = row.get(0);
    let sha256: String = row.get(1);
    let blob: Vec<BlobAgent> =
        serde_json::from_str(&text).map_err(|e| format!("the agents blob does not parse: {e}"))?;
    let expected = blob.len() as i64;

    let mut tx = client.transaction().map_err(|e| format!("open the agents transaction: {e}"))?;
    tx.execute(sql::AGENTS_DELETE, &[]).map_err(|e| format!("empty agents: {e}"))?;
    for agent in &blob {
        let cid = i64::try_from(agent.cid).map_err(|_| format!("agent cid {} is above i64::MAX", agent.cid))?;
        let state = agent.state.as_deref().unwrap_or("active");
        tx.execute(
            sql::AGENTS_INSERT,
            &[
                &cid,
                &agent.name,
                &agent.role,
                &agent.model,
                &agent.species,
                &agent.species_at_ms,
                &state,
                &agent.created_at_ms,
                &agent.retired_at_ms,
                &agent.updated_at_ms,
            ],
        )
        .map_err(|e| format!("insert agent {}: {e}", agent.cid))?;
    }
    let loaded: i64 = tx
        .query_one(sql::AGENTS_COUNT, &[])
        .map_err(|e| format!("count agents: {e}"))?
        .get(0);
    if loaded != expected {
        return Err(format!("expected {expected} rows, loaded {loaded}"));
    }
    tx.commit().map_err(|e| format!("commit the agents transaction: {e}"))?;
    Ok(Outcome {
        status: "verified",
        source: Source::Blob,
        source_sha256: Some(sha256),
        rows_expected: expected,
        rows_loaded: loaded,
        detail: format!("{loaded} agents from the blob"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrate as schema;

    const DATA_MIGRATIONS_EXISTS: &str = include_str!("../../sql/test/data_migrations_exists.sql");
    const AGENTS_ROW: &str = include_str!("../../sql/test/data_migrations_agents.sql");
    const PROBE_SETUP: &str = include_str!("../../sql/test/forbid_probe_setup.sql");
    const PROBE_UPDATE: &str = include_str!("../../sql/test/forbid_probe_update.sql");
    const PROBE_DELETE: &str = include_str!("../../sql/test/forbid_probe_delete.sql");
    const PROBE_TRUNCATE: &str = include_str!("../../sql/test/forbid_probe_truncate.sql");
    const PROBE_COUNT: &str = include_str!("../../sql/test/forbid_probe_count.sql");

    /// Whether the error is the server refusing a write with forbid_mutation(); the refusal is the
    /// server message, which the error's Display does not show.
    fn append_only(error: &postgres::Error) -> bool {
        error.as_db_error().is_some_and(|db| db.message().contains("append-only"))
    }

    fn connect(url: &str) -> Client {
        Client::connect(url, postgres::NoTls).expect("connect to the test database")
    }

    /// On a fresh test database 0001 and 0002 apply, data_migrations exists, and a table guarded
    /// by forbid_mutation() refuses UPDATE, DELETE and TRUNCATE and keeps its row.
    #[test]
    fn data_migrations_exists_and_forbid_mutation_rejects_update_and_delete() {
        let Some(url) = crate::db::test_database("forbid_mutation") else { return };
        let mut client = connect(&url);
        assert_eq!(schema::migrate(&mut client), Ok(vec![1, 2, 3, 4, 5]));
        assert!(
            client.query_one(DATA_MIGRATIONS_EXISTS, &[]).expect("look up data_migrations").get::<_, bool>(0),
            "data_migrations exists after 0002"
        );

        client.batch_execute(PROBE_SETUP).expect("the probe table guarded by forbid_mutation");
        let update = client.execute(PROBE_UPDATE, &[]).expect_err("UPDATE must be refused");
        assert!(append_only(&update), "{update:?}");
        let delete = client.execute(PROBE_DELETE, &[]).expect_err("DELETE must be refused");
        assert!(append_only(&delete), "{delete:?}");
        let truncate = client.execute(PROBE_TRUNCATE, &[]).expect_err("TRUNCATE must be refused");
        assert!(append_only(&truncate), "{truncate:?}");
        assert_eq!(client.query_one(PROBE_COUNT, &[]).expect("count").get::<_, i64>(0), 1, "the row is still there");
    }

    /// The agents stub with no source records none/verified, and a second run changes nothing.
    #[test]
    fn the_agents_stub_records_no_source_as_verified_and_runs_once() {
        let Some(url) = crate::db::test_database("data_agents") else { return };
        let mut client = connect(&url);
        assert_eq!(schema::migrate(&mut client), Ok(vec![1, 2, 3, 4, 5]));
        run(&mut client).expect("the first run");
        run(&mut client).expect("the second run");
        assert_eq!(client.query_one(AGENTS_ROW, &[]).expect("the agents row").get::<_, String>(0), "verified none");
    }

    const AGENTS_SEED: &str = include_str!("../../sql/test/agents_seed.sql");
    const AGENTS_SEED_DUPLICATE: &str = include_str!("../../sql/test/agents_seed_duplicate.sql");
    const AGENTS_ROWS: &str = include_str!("../../sql/test/agents_rows.sql");
    const AGENTS_STATUS: &str = include_str!("../../sql/test/data_migrations_agents_status.sql");

    /// The three seeded agents as agents_rows.sql prints them, by cid.
    const SEEDED_ROWS: &str = "1000|ada|worker|openrouter/x|fox|active|1700000000000;1001|bo|orchestrator|-|owl|retired|1700000000000;1002|cy|researcher|-|kite|active|1700000050000";

    fn agents_rows(client: &mut Client) -> String {
        client.query_one(AGENTS_ROWS, &[]).expect("the agents rows").get::<_, String>(0)
    }

    /// A plaza_state agents blob of three agents loads into the agents table with its columns; the
    /// status records verified, 3 of 3 rows and the blob's sha256; a second run changes nothing.
    #[test]
    fn the_agents_blob_loads_once_and_a_second_run_changes_nothing() {
        let Some(url) = crate::db::test_database("data_agents_blob") else { return };
        let mut client = connect(&url);
        assert_eq!(schema::migrate(&mut client), Ok(vec![1, 2, 3, 4, 5]));
        client.batch_execute(AGENTS_SEED).expect("seed the agents blob");
        run(&mut client).expect("the first run");
        assert_eq!(agents_rows(&mut client), SEEDED_ROWS);
        let status = client.query_one(AGENTS_STATUS, &[]).expect("the status row").get::<_, String>(0);
        assert_eq!(status, "verified blob 3 3 64");
        run(&mut client).expect("the second run");
        assert_eq!(agents_rows(&mut client), SEEDED_ROWS);
        assert_eq!(client.query_one(AGENTS_STATUS, &[]).expect("the status row").get::<_, String>(0), status);
    }

    /// A blob that repeats a cid fails closed: the run names the domain, the status records failed,
    /// and the agents table keeps no rows from the failed load.
    #[test]
    fn a_duplicate_agent_in_the_blob_fails_closed() {
        let Some(url) = crate::db::test_database("data_agents_dup") else { return };
        let mut client = connect(&url);
        assert_eq!(schema::migrate(&mut client), Ok(vec![1, 2, 3, 4, 5]));
        client.batch_execute(AGENTS_SEED_DUPLICATE).expect("seed the duplicate blob");
        let error = run(&mut client).expect_err("a duplicate cid must fail");
        assert!(error.contains("data migration agents failed"), "{error}");
        let status: String = client.query_one(sql::DATA_MIGRATION_STATUS, &[&"agents"]).expect("the status row").get(0);
        assert_eq!(status, "failed");
        assert_eq!(client.query_one(sql::AGENTS_COUNT, &[]).expect("count").get::<_, i64>(0), 0, "the load rolled back");
    }
}
