//! The qualification's state of record in `plaza.db` (component `qualification`): each candidate's qualification run with
//! its depth, its stages' results and its verdict; the per-test histories with their quarantine; the acceptances; and the
//! test results the lab runs recorded (the functional stage's evidence). The engine and the reports read it through
//! [`Store`].

use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};

use super::accept::Acceptance;
use super::plan::{Excluded, Stage};
use super::result::StageResult;
use super::testcases::History;

/// A task the qualification filed for a failure or a quarantined test: the goal and the stage, and the task's id once
/// the plaza has created it (empty until then).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct FixTask {
    pub stage: String,
    pub goal: String,
    pub task: String,
}

/// One test's result in one lab run, as the lab's runner posted it (the first attempt; re-runs are the engine's).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct LabTest {
    #[table(key)]
    pub run: String,
    #[table(key)]
    pub name: String,
    pub passed: bool,
}

/// One qualification of a release candidate: its depth and why, its lab runs, its stages' results, its verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
pub struct QualifyRun {
    /// `qN`, unique in the hub.
    #[table(key)]
    pub id: String,
    #[table(index)]
    pub release: String,
    pub project: String,
    /// The planner's model and the second model for acceptances (empty: failures are not accepted).
    pub planner: String,
    pub second: String,
    pub channel: String,
    /// The stages that run, in order (their keys) and the short ones.
    #[table(list)]
    pub depth: Vec<String>,
    #[table(list)]
    pub short: Vec<String>,
    /// The planner's reason for the depth (`plan::Depth::reason`).
    pub reason: String,
    #[table(child = "qualify_run_excluded")]
    pub excluded: Vec<Excluded>,
    /// The recipe run that walks the stages (`recipes/runs.rs`).
    pub recipe_run: String,
    /// The lab runs the topologies started, in declaration order.
    #[table(list)]
    pub lab_runs: Vec<String>,
    #[table(child = "qualify_run_stage")]
    pub results: Vec<StageResult>,
    #[table(child = "qualify_run_task")]
    pub tasks: Vec<FixTask>,
    /// The keys of the failures accepted (`accept::key_of`).
    #[table(list)]
    pub accepted: Vec<String>,
    /// The verdict (`Outcome`) when the qualification is over: `passed`, `failed`, `incomplete`; empty while it runs.
    pub verdict: String,
    #[table(list)]
    pub reasons: Vec<String>,
    /// `running` or `done`.
    pub state: String,
    /// The review run that holds the release's own review graph (empty until it ends).
    pub review_run: String,
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
}

impl QualifyRun {
    /// Whether the qualification is over.
    pub fn done(&self) -> bool {
        self.state == "done"
    }

    /// The stages of the depth, as they are.
    pub fn stages(&self) -> Vec<Stage> {
        self.depth.iter().filter_map(|k| Stage::of_key(k)).collect()
    }

    /// The result of a stage, if it has one.
    pub fn result(&self, stage: Stage) -> Option<&StageResult> {
        self.results.iter().find(|r| r.stage == stage.key())
    }
}

#[derive(Default, apz_store::Table)]
#[table(name = "qualification")]
pub(crate) struct Persisted {
    #[table(child = "qualify_run")]
    pub(crate) runs: Vec<QualifyRun>,
    #[table(child = "qualify_test")]
    pub(crate) tests: Vec<History>,
    #[table(child = "qualify_acceptance")]
    pub(crate) acceptances: Vec<Acceptance>,
    #[table(child = "qualify_lab_test")]
    pub(crate) lab_tests: Vec<LabTest>,
}

/// The table of the qualification component: `qualification`, with its child tables.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Persisted)
}

/// The qualification's records, in memory (plaza.db is the record; the saver writes what changed).
#[derive(Default)]
pub struct Store {
    inner: Mutex<Persisted>,
}

impl Store {
    fn lock(&self) -> MutexGuard<'_, Persisted> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The rows the saver keeps.
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        RowSet::one(def(), &*self.lock())
    }

    /// Replaces the records with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored = apz_store::table::load_one::<Persisted>(conn, def())?.unwrap_or_default();
        *self.lock() = stored;
        Ok(())
    }

    /// The next qualification's id: `qual-<n>`, one past the highest.
    pub fn next_id(&self) -> String {
        let max = self
            .lock()
            .runs
            .iter()
            .filter_map(|r| {
                r.id.strip_prefix("qual-")
                    .and_then(|n| n.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(0);
        format!("qual-{}", max + 1)
    }

    /// Stores a qualification run, replacing the one with its id.
    pub fn put_run(&self, run: QualifyRun) {
        let mut p = self.lock();
        p.runs.retain(|r| r.id != run.id);
        p.runs.push(run);
    }

    /// Changes a qualification run in place; returns it as it is now.
    pub fn update_run(&self, id: &str, change: impl FnOnce(&mut QualifyRun)) -> Option<QualifyRun> {
        let mut p = self.lock();
        let run = p.runs.iter_mut().find(|r| r.id == id)?;
        change(run);
        Some(run.clone())
    }

    /// A qualification run by its id.
    pub fn run(&self, id: &str) -> Option<QualifyRun> {
        self.lock().runs.iter().find(|r| r.id == id).cloned()
    }

    /// The newest qualification of a release.
    pub fn of_release(&self, release: &str) -> Option<QualifyRun> {
        self.lock()
            .runs
            .iter()
            .rev()
            .find(|r| r.release == release)
            .cloned()
    }

    /// Every qualification, in the order they were made.
    pub fn all_runs(&self) -> Vec<QualifyRun> {
        self.lock().runs.clone()
    }

    /// Every qualification of a project, newest first.
    pub fn of_project(&self, project: &str) -> Vec<QualifyRun> {
        self.lock()
            .runs
            .iter()
            .rev()
            .filter(|r| r.project == project)
            .cloned()
            .collect()
    }

    /// A test's history, if the project has run it.
    pub fn history(&self, project: &str, name: &str) -> Option<History> {
        self.lock()
            .tests
            .iter()
            .find(|h| h.project == project && h.name == name)
            .cloned()
    }

    /// Every test history of a project, by name.
    pub fn histories(&self, project: &str) -> Vec<History> {
        let mut out: Vec<History> = self
            .lock()
            .tests
            .iter()
            .filter(|h| h.project == project)
            .cloned()
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Changes a test's history (creating it when the project has not run the test yet); returns it as it is now.
    pub fn update_history(
        &self,
        project: &str,
        name: &str,
        change: impl FnOnce(&mut History),
    ) -> History {
        let mut p = self.lock();
        let at = match p
            .tests
            .iter()
            .position(|h| h.project == project && h.name == name)
        {
            Some(at) => at,
            None => {
                p.tests.push(History::new(project, name));
                p.tests.len() - 1
            }
        };
        change(&mut p.tests[at]);
        p.tests[at].clone()
    }

    /// Records an accepted failure.
    pub fn put_acceptance(&self, acceptance: Acceptance) {
        self.lock().acceptances.push(acceptance);
    }

    /// The acceptances of a release.
    pub fn acceptances(&self, release: &str) -> Vec<Acceptance> {
        self.lock()
            .acceptances
            .iter()
            .filter(|a| a.release == release)
            .cloned()
            .collect()
    }

    /// Records the tests a lab run reported (replacing a test's result in that run).
    pub fn record_lab_tests(&self, run: &str, tests: &[LabTest]) {
        let mut p = self.lock();
        for test in tests {
            p.lab_tests
                .retain(|t| !(t.run == run && t.name == test.name));
            p.lab_tests.push(LabTest {
                run: run.to_string(),
                name: test.name.clone(),
                passed: test.passed,
            });
        }
    }

    /// The tests a lab run reported, in the order they were recorded.
    pub fn lab_tests(&self, run: &str) -> Vec<LabTest> {
        self.lock()
            .lab_tests
            .iter()
            .filter(|t| t.run == run)
            .cloned()
            .collect()
    }
}
