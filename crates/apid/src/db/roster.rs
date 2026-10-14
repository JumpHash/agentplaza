//! The roster's store: the `agents` table behind `crate::agents::Roster` (docs/design/database.md
//! section 4.3). The roster stays the in-memory cache; this module writes its durable fields through
//! and loads the active rows at start. Every statement lives in `sql/queries/agents_*.sql`.

use crate::agents::{Agent, Status};
use crate::db::Db;
use crate::db::sql;

/// An active `agents` row: what the roster needs at start.
pub struct StoredAgent {
    pub cid: u64,
    pub name: String,
    pub role: String,
    pub model: Option<String>,
    pub species: String,
}

impl StoredAgent {
    /// The roster entry for the row: offline, with no session, run or VM. Reconcile decides what
    /// happens to the agent next.
    pub fn into_agent(self) -> Agent {
        Agent {
            cid: self.cid,
            name: self.name,
            role: self.role,
            model_override: self.model,
            status: Status::Offline,
            connected_at_ms: 0,
            last_seen_ms: 0,
            run: None,
            vm: None,
            usage: Default::default(),
            species: self.species,
            kind: "llm".into(),
        }
    }
}

impl Db {
    /// Writes an agent's durable fields to its row (upsert by cid). The roster calls it only when a
    /// durable field changed, so the hot path does not wait on Postgres for usage or status changes.
    pub fn write_agent(&self, agent: &Agent) {
        if !self.enabled() {
            return;
        }
        let cid = agent.cid as i64;
        let result = self.run(|c| {
            c.execute(
                sql::AGENTS_UPSERT,
                &[
                    &cid,
                    &agent.name,
                    &agent.role,
                    &agent.model_override,
                    &agent.species,
                ],
            )
        });
        if let Err(e) = result {
            eprintln!("apid: agent {cid} not written to the database: {e}");
        }
    }

    /// Soft-deletes an agent's row (state retired); the row stays.
    pub fn retire_agent(&self, cid: u64) {
        if !self.enabled() {
            return;
        }
        let cid = cid as i64;
        if let Err(e) = self.run(|c| c.execute(sql::AGENTS_RETIRE, &[&cid])) {
            eprintln!("apid: agent {cid} not retired in the database: {e}");
        }
    }

    /// The active agents, by cid. Empty without a database, or when the read fails (logged).
    pub fn active_agents(&self) -> Vec<StoredAgent> {
        if !self.enabled() {
            return Vec::new();
        }
        match self.run(|c| c.query(sql::AGENTS_ACTIVE, &[])) {
            Ok(rows) => rows
                .iter()
                .map(|row| StoredAgent {
                    cid: row.get::<_, i64>(0) as u64,
                    name: row.get(1),
                    role: row.get(2),
                    model: row.get(3),
                    species: row.get(4),
                })
                .collect(),
            Err(e) => {
                eprintln!("db: agents load failed: {e}; the roster starts empty");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::*;
    use crate::agents::Roster;

    const BY_ROLE: &str = include_str!("../../sql/test/agents_by_role.sql");
    const STATE_OF: &str = include_str!("../../sql/test/agents_state_of.sql");

    fn agent(cid: u64, name: &str, role: &str) -> Agent {
        Agent {
            cid,
            name: name.into(),
            role: role.into(),
            model_override: None,
            status: Status::Idle,
            connected_at_ms: 1,
            last_seen_ms: 1,
            run: None,
            vm: Some(0),
            usage: Default::default(),
            species: "owl".into(),
            kind: "llm".into(),
        }
    }

    /// A fresh test database with the schema applied (skipped without APZ_TEST_DATABASE_URL).
    fn database(label: &str) -> Option<Arc<Db>> {
        let url = crate::db::test_database(label)?;
        let env = BTreeMap::from([("APZ_DATABASE_URL".to_string(), url)]);
        let db = Db::connect(&env);
        assert!(db.enabled());
        Some(Arc::new(db))
    }

    /// Upserts and updates write through, a fresh roster loaded from the table holds the same agents,
    /// a remove soft-deletes the row, and the table answers a GROUP BY on role.
    #[test]
    fn roster_writes_through_and_loads_back() {
        let Some(db) = database("agents_roster") else {
            return;
        };
        let roster = Roster::with_db(Some(Arc::clone(&db)));
        let mut marlow = agent(101, "Marlow", "builder");
        marlow.model_override = Some("model-a".into());
        roster.upsert(marlow);
        roster.upsert(agent(102, "Iris", "builder"));
        roster.upsert(agent(103, "Quill", "planner"));

        let fresh = Roster::default();
        fresh.restore(
            db.active_agents()
                .into_iter()
                .map(StoredAgent::into_agent)
                .collect(),
        );
        let loaded = fresh.get(101).expect("101 loaded from the table");
        assert_eq!(loaded.name, "Marlow");
        assert_eq!(loaded.role, "builder");
        assert_eq!(loaded.model_override.as_deref(), Some("model-a"));
        assert_eq!(loaded.species, "owl");
        assert_eq!(loaded.status, Status::Offline);
        assert_eq!(fresh.list().len(), 3);

        // A change to a durable field changes the row.
        assert!(roster.update(101, |a| {
            a.role = "planner".into();
            a.model_override = Some("model-b".into());
        }));
        let row = db
            .active_agents()
            .into_iter()
            .find(|row| row.cid == 101)
            .expect("101 active");
        assert_eq!(row.role, "planner");
        assert_eq!(row.model.as_deref(), Some("model-b"));

        // A remove marks the row retired: it leaves the active rows and the roster.
        assert!(roster.remove(103));
        let state: String = db
            .run(|c| c.query_one(STATE_OF, &[&103i64]))
            .expect("state of 103")
            .get(0);
        assert_eq!(state, "retired");
        assert!(db.active_agents().iter().all(|row| row.cid != 103));
        assert!(roster.get(103).is_none());

        // The rows are queryable by role, retired rows included.
        let counts: Vec<(String, i64)> = db
            .run(|c| c.query(BY_ROLE, &[]))
            .expect("group by role")
            .iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect();
        assert_eq!(
            counts,
            vec![("builder".to_string(), 1), ("planner".to_string(), 2)]
        );
    }
}
