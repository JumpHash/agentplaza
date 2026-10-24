//! The ephemeral instances, as rows of `plaza.db` (table `ephemeral_instance`, one row per instance). The store holds the
//! rows in memory and the saver writes what changed, the way the environments do. A row keeps its subject and its target
//! as JSON; [`super::Instance`] is the form the API and the callers read.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, Table, TableDef};
use serde::{Deserialize, Serialize};

use super::{Instance, State, Subject, TEARDOWN_WAIT_CAP_MS, Target};

/// The first wait after a failed removal (one tick).
const FIRST_WAIT_MS: u64 = 60_000;

/// How many instances a project keeps (its newest ones; a torn-down one is only a record).
const KEEP: usize = 50;

/// One instance (a row of `ephemeral_instance`).
#[derive(Debug, Clone, Serialize, Deserialize, Table)]
#[table(name = "ephemeral_instance")]
pub struct Row {
    /// `eph-<hex>`.
    #[table(key)]
    pub id: String,
    pub project: String,
    pub environment: String,
    /// The subject as JSON (`{"lane":"…"}` or `{"release":"…"}`).
    pub subject: String,
    /// The target as JSON: what its teardown removes.
    pub target: String,
    /// `creating`, `ready`, `failed` or `torn_down`.
    pub state: String,
    /// The reason a failed instance failed, else empty.
    pub reason: String,
    pub url: Option<String>,
    pub created_ms: u64,
    pub expires_ms: u64,
    /// The workflow run that brought the instance up, else empty.
    pub run: Option<String>,
    /// Whether that run keeps the instance.
    pub keep: bool,
    /// The build's blobs by artifact name, as JSON (`{}` when the instance builds its own).
    pub built: String,
}

/// The ticker's retries of one instance's removal: how many tries failed, and when the next one is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    pub attempts: u32,
    pub next_ms: u64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The instances of every project.
#[derive(Default)]
pub struct Store {
    rows: Mutex<Vec<Row>>,
    /// The removals that failed and are retried, by instance (in memory: a restart starts the count again).
    retries: Mutex<BTreeMap<String, Retry>>,
}

/// The table of the instances: `ephemeral_instance`.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Row)
}

impl Store {
    /// The row of the instance, as the API form.
    pub fn get(&self, id: &str) -> Option<Instance> {
        lock(&self.rows)
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| to_instance(r).ok())
    }

    /// The project's instances, newest first.
    pub fn list(&self, project: &str) -> Vec<Instance> {
        let mut out: Vec<Instance> = lock(&self.rows)
            .iter()
            .filter(|r| r.project == project)
            .filter_map(|r| to_instance(r).ok())
            .collect();
        out.sort_by_key(|i| std::cmp::Reverse(i.created_ms));
        out
    }

    /// The instances a workflow run brought up, oldest first (torn down or not).
    pub fn for_run(&self, run: &str) -> Vec<Instance> {
        let mut out: Vec<Instance> = lock(&self.rows)
            .iter()
            .filter(|r| r.run.as_deref() == Some(run))
            .filter_map(|r| to_instance(r).ok())
            .collect();
        out.sort_by_key(|i| i.created_ms);
        out
    }

    /// The retries of an instance's removal, if any failed.
    pub fn retry_of(&self, id: &str) -> Option<Retry> {
        lock(&self.retries).get(id).copied()
    }

    /// Records a failed removal of `id` at `now`: the next try is due after a wait that doubles, up to the cap.
    pub fn failed_teardown(&self, id: &str, now: u64) -> Retry {
        let mut retries = lock(&self.retries);
        let attempts = retries.get(id).map_or(0, |r| r.attempts) + 1;
        let wait = FIRST_WAIT_MS
            .checked_shl(attempts - 1)
            .unwrap_or(u64::MAX)
            .min(TEARDOWN_WAIT_CAP_MS);
        let retry = Retry {
            attempts,
            next_ms: now.saturating_add(wait),
        };
        retries.insert(id.to_string(), retry);
        retry
    }

    /// Forgets an instance's failed removals (it came down, or it is gone).
    pub fn clear_retry(&self, id: &str) {
        lock(&self.retries).remove(id);
    }

    /// Every instance whose row is not torn down, for the ticker.
    pub fn live(&self) -> Vec<Instance> {
        lock(&self.rows)
            .iter()
            .filter(|r| r.state != TORN_DOWN)
            .filter_map(|r| to_instance(r).ok())
            .collect()
    }

    /// Adds an instance's row. A project past [`KEEP`] drops its oldest torn-down records first.
    pub fn insert(&self, instance: &Instance) -> Result<(), String> {
        let row = from_instance(instance)?;
        let mut rows = lock(&self.rows);
        let project = row.project.clone();
        rows.push(row);
        let mine: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.project == project)
            .map(|(i, _)| i)
            .collect();
        if mine.len() > KEEP {
            let oldest = mine
                .iter()
                .copied()
                .filter(|i| rows[*i].state == TORN_DOWN)
                .min_by_key(|i| rows[*i].created_ms);
            if let Some(i) = oldest {
                rows.remove(i);
            }
        }
        Ok(())
    }

    /// Sets an instance's target (a Pages deployment's id, once it is known).
    pub fn set_target(&self, id: &str, target: &Target) -> Result<(), String> {
        let mut rows = lock(&self.rows);
        let row = rows
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(|| format!("no instance {id}"))?;
        row.target = serde_json::to_string(target).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Sets an instance's state. `only_from`, when given, changes it only from that state (a finished rollout does not
    /// reopen an instance that was torn down meanwhile).
    pub fn set_state(
        &self,
        id: &str,
        state: &State,
        only_from: Option<&str>,
    ) -> Result<(), String> {
        let mut rows = lock(&self.rows);
        let row = rows
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(|| format!("no instance {id}"))?;
        if only_from.is_some_and(|from| row.state != from) {
            return Ok(());
        }
        let (name, reason) = state_columns(state);
        row.state = name.to_string();
        row.reason = reason;
        Ok(())
    }

    /// Sets an instance's address once it is known.
    pub fn set_url(&self, id: &str, url: Option<String>) {
        if let Some(row) = lock(&self.rows).iter_mut().find(|r| r.id == id) {
            row.url = url;
        }
    }

    /// The rows as they are now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        RowSet::list(def(), lock(&self.rows).iter())
    }

    /// Replaces the rows with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let mut stored: Vec<Row> = apz_store::table::load_list(conn, def())?;
        recover(&mut stored);
        *lock(&self.rows) = stored;
        Ok(())
    }
}

/// A deploy the hub was in the middle of when it stopped never finishes, and its target may be half made: it is failed on
/// load, so its teardown (by hand, or the ticker at its time) removes what was left.
pub fn recover(rows: &mut [Row]) {
    for row in rows.iter_mut().filter(|r| r.state == "creating") {
        row.state = "failed".to_string();
        row.reason =
            "the hub stopped while it was deploying; its target may be left behind".to_string();
    }
}

/// The state a row names for "torn down".
pub const TORN_DOWN: &str = "torn_down";

/// A state's column values: its name and its reason (empty unless it failed).
fn state_columns(state: &State) -> (&'static str, String) {
    match state {
        State::Creating => ("creating", String::new()),
        State::Ready => ("ready", String::new()),
        State::Failed(why) => ("failed", why.clone()),
        State::TornDown => (TORN_DOWN, String::new()),
    }
}

/// An instance as its row.
pub fn from_instance(instance: &Instance) -> Result<Row, String> {
    let (state, reason) = state_columns(&instance.state);
    let built = serde_json::to_string(&instance.built).map_err(|e| e.to_string())?;
    Ok(Row {
        run: instance.run.clone(),
        keep: instance.keep,
        built,
        id: instance.id.clone(),
        project: instance.project.clone(),
        environment: instance.environment.clone(),
        subject: serde_json::to_string(&instance.subject).map_err(|e| e.to_string())?,
        target: serde_json::to_string(&instance.target).map_err(|e| e.to_string())?,
        state: state.to_string(),
        reason,
        url: instance.url.clone(),
        created_ms: instance.created_ms,
        expires_ms: instance.expires_ms,
    })
}

/// A row as the API form. A row whose JSON or state does not read is not an instance (it is skipped, never guessed).
pub fn to_instance(row: &Row) -> Result<Instance, String> {
    let state = match row.state.as_str() {
        "creating" => State::Creating,
        "ready" => State::Ready,
        "failed" => State::Failed(row.reason.clone()),
        TORN_DOWN => State::TornDown,
        other => return Err(format!("instance {}: unknown state {other}", row.id)),
    };
    Ok(Instance {
        id: row.id.clone(),
        project: row.project.clone(),
        environment: row.environment.clone(),
        subject: serde_json::from_str::<Subject>(&row.subject).map_err(|e| e.to_string())?,
        target: serde_json::from_str::<Target>(&row.target).map_err(|e| e.to_string())?,
        url: row.url.clone(),
        state,
        created_ms: row.created_ms,
        expires_ms: row.expires_ms,
        run: row.run.clone(),
        keep: row.keep,
        built: serde_json::from_str(&row.built).map_err(|e| e.to_string())?,
    })
}
