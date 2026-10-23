//! Recipe runs (PLAN 4.1, 21.1): one execution of a recipe, with every step's state, gate results and edges, stored typed
//! in `plaza.db` (tables `recipe_run`, with its `recipe_step_run` and `recipe_edge_run` children, and `recipe_version`
//! for the owner's versions). A run is started by a task (its change), by a review (the review recipe), by a landing
//! (the base) or by a project's curator (distill). The task's change runs its steps through the walker (graph.rs); the
//! others record their engine's outcome as their one step.
//!
//! The walk is the run's state; the seats (seats.rs) are what sessions fill. A run names the recipe it ran as a snapshot
//! (its TOML), so a later version of the file does not change a run that is under way.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use super::graph::{EdgeRec, EdgeState, Move, Outcome, Report, StepRec, StepState, Walk};

use super::model::{Model, Recipe, builtin};

/// Where a run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    /// Waiting for the owner's ruling (a review's owner rule).
    Owner,
    Passed,
    Failed,
    /// Stopped by the owner or by the work that owned it ending.
    Cancelled,
}

/// One run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "recipe_run")]
pub struct RecipeRun {
    #[table(key)]
    pub id: String,
    pub recipe: String,
    pub version: u32,
    /// Where the recipe came from: `built_in`, `owner` (a version the owner saved) or `project` (its `.plaza/recipes/`).
    pub source: String,
    /// The recipe as the run started it.
    #[table(content)]
    pub toml: String,
    /// What started it: `task`, `review`, `landing` or `curator`.
    pub trigger: String,
    #[table(index)]
    pub task: Option<String>,
    #[table(index)]
    pub project: Option<String>,
    /// What the run is about, for the engine that owns its steps: a review run, a base's key, a project.
    #[table(index)]
    pub subject: Option<String>,
    pub state: RunState,
    pub outcome: Option<String>,
    #[table(child)]
    pub steps: Vec<StepRow>,
    #[table(child)]
    pub edges: Vec<EdgeRow>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

/// One step of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct StepRow {
    #[table(key)]
    pub step: String,
    pub state: StepState,
    pub visits: u32,
    pub items: u32,
    pub reported: u32,
    /// The gate's conditions as its last check read them.
    pub gate: String,
}

/// One edge of a run, by its position in the recipe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct EdgeRow {
    #[table(key)]
    pub index: u32,
    pub from: String,
    pub to: String,
    pub state: EdgeState,
    pub taken: u32,
}

/// A version of a recipe the owner saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "recipe_version")]
pub struct VersionRow {
    #[table(key)]
    pub recipe: String,
    #[table(key)]
    pub version: u32,
    #[table(content)]
    pub toml: String,
    pub saved_ms: u64,
}

/// A run in memory: its record, the recipe it runs, and its walk.
#[derive(Clone)]
struct Entry {
    run: RecipeRun,
    recipe: Recipe,
    walk: Walk,
}

#[derive(Default)]
struct Inner {
    runs: BTreeMap<String, Entry>,
    next: u64,
    /// The owner's saved versions, by recipe id.
    versions: BTreeMap<String, Vec<Recipe>>,
    saved_ms: BTreeMap<(String, u32), u64>,
}

/// Every recipe run, and the owner's recipe versions.
#[derive(Default)]
pub struct Runs {
    inner: Mutex<Inner>,
}

/// What a route lists about a run.
#[derive(Debug, Clone)]
pub struct View {
    pub run: RecipeRun,
    pub recipe: Recipe,
    pub walk: Walk,
}

impl Runs {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a run of `recipe`. Returns its id and the moves that start its entry steps.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        recipe: &Recipe,
        source: &str,
        trigger: &str,
        task: Option<&str>,
        project: Option<&str>,
        subject: Option<&str>,
        now: u64,
    ) -> (String, Vec<Move>) {
        self.start_with_items(
            recipe,
            source,
            trigger,
            task,
            project,
            subject,
            &BTreeMap::new(),
            now,
        )
    }

    /// Starts a run as [`Runs::start`], with the item counts of its named fan-outs (`items`, by step).
    #[allow(clippy::too_many_arguments)]
    pub fn start_with_items(
        &self,
        recipe: &Recipe,
        source: &str,
        trigger: &str,
        task: Option<&str>,
        project: Option<&str>,
        subject: Option<&str>,
        items: &BTreeMap<String, u32>,
        now: u64,
    ) -> (String, Vec<Move>) {
        let (walk, moves) = Walk::start(recipe, items);
        let mut inner = self.lock();
        inner.next += 1;
        let id = format!("rr-{}", inner.next);
        let run = RecipeRun {
            id: id.clone(),
            recipe: recipe.id.clone(),
            version: recipe.version,
            source: source.to_string(),
            toml: recipe.to_toml(),
            trigger: trigger.to_string(),
            task: task.map(str::to_string),
            project: project.map(str::to_string),
            subject: subject.map(str::to_string),
            state: RunState::Running,
            outcome: None,
            steps: Vec::new(),
            edges: Vec::new(),
            created_ms: now,
            updated_ms: now,
        };
        inner.runs.insert(
            id.clone(),
            Entry {
                run,
                recipe: recipe.clone(),
                walk,
            },
        );
        (id, moves)
    }

    /// Starts a run of recipe `recipe` (its newest version) whose steps another engine runs: its entry steps are
    /// started at once, and the engine settles them with [`Runs::finish_subject`]. None when no such recipe is there.
    pub fn start_engine(
        &self,
        recipe: &str,
        trigger: &str,
        task: Option<&str>,
        project: Option<&str>,
        subject: Option<&str>,
        now: u64,
    ) -> Option<String> {
        self.start_engine_items(
            recipe,
            &BTreeMap::new(),
            trigger,
            task,
            project,
            subject,
            now,
        )
    }

    /// As [`Runs::start_engine`], with the items of the fan-out steps (`items` by step id). A release's `build` step runs
    /// one item per placed target (targets.rs, the `release` recipe).
    #[allow(clippy::too_many_arguments)]
    pub fn start_engine_items(
        &self,
        recipe: &str,
        items: &BTreeMap<String, u32>,
        trigger: &str,
        task: Option<&str>,
        project: Option<&str>,
        subject: Option<&str>,
        now: u64,
    ) -> Option<String> {
        let recipe = self.recipe(recipe, None)?;
        let (id, moves) = self.start_with_items(
            &recipe,
            source_of(&recipe),
            trigger,
            task,
            project,
            subject,
            items,
            now,
        );
        for m in moves {
            if let Move::Ready(step) = m {
                self.started(&id, &step, now);
            }
        }
        Some(id)
    }

    /// A ready step started (its session or its engine is in flight).
    pub fn started(&self, run: &str, step: &str, now: u64) {
        self.update(run, now, |e| e.walk.started(step));
    }

    /// The run's active step of task `task` started (its seat was filled).
    pub fn task_step_started(&self, task: &str, step: &str, now: u64) {
        if let Some(run) = self.active_of_task(task) {
            self.started(&run, step, now);
        }
    }

    /// The model a step of the task's active run is to use, as its recipe file says.
    pub fn step_model(&self, task: &str, step: &str) -> Option<Model> {
        let inner = self.lock();
        let entry = inner
            .runs
            .values()
            .find(|e| e.run.task.as_deref() == Some(task) && e.walk.outcome.is_none())?;
        entry.recipe.step(step).map(|s| s.model.clone())
    }

    /// A step settled with `report`, in the task's active run. Returns the moves the caller applies.
    pub fn settle_task(&self, task: &str, step: &str, report: &Report, now: u64) -> Vec<Move> {
        match self.active_of_task(task) {
            Some(run) => self.settle(&run, step, report, now),
            None => Vec::new(),
        }
    }

    /// A step of run `run` settled with `report`. Returns the moves; the run's state follows its walk.
    pub fn settle(&self, run: &str, step: &str, report: &Report, now: u64) -> Vec<Move> {
        let mut moves = Vec::new();
        self.update(run, now, |e| {
            moves = e.walk.settle(&e.recipe, step, report);
        });
        moves
    }

    /// A run that a stand-in engine decides: its one step settles with `report` and the run ends with `outcome`. An
    /// owner's ruling (`owner`) leaves it open until the owner answers.
    pub fn finish_subject(
        &self,
        trigger: &str,
        subject: &str,
        report: &Report,
        outcome: &str,
        owner: bool,
        now: u64,
    ) {
        let found = self.active_by_subject(trigger, subject);
        let Some(run) = found else { return };
        if owner {
            self.update(&run, now, |e| {
                e.run.state = RunState::Owner;
                e.run.outcome = Some(outcome.to_string());
            });
            return;
        }
        let step = {
            let inner = self.lock();
            inner.runs.get(&run).and_then(|e| {
                e.walk
                    .steps
                    .iter()
                    .find(|(_, s)| s.state == StepState::Running || s.state == StepState::Ready)
                    .map(|(name, _)| name.clone())
            })
        };
        if let Some(step) = step {
            self.update(&run, now, |e| {
                e.walk.settle(&e.recipe, &step, report);
                e.run.outcome = Some(outcome.to_string());
            });
        }
    }

    /// Stops the task's active run (its task ended): it is cancelled, and its walk ends.
    pub fn cancel_task(&self, task: &str, now: u64) {
        if let Some(run) = self.active_of_task(task) {
            self.cancel(&run, "the task ended", now);
        }
    }

    /// Stops a run: it is cancelled and its walk ends with `why`.
    pub fn cancel(&self, run: &str, why: &str, now: u64) -> bool {
        let mut done = false;
        self.update(run, now, |e| {
            if e.walk.outcome.is_none() {
                e.walk.outcome = Some(Outcome::Failed(why.to_string()));
                e.run.state = RunState::Cancelled;
                e.run.outcome = Some(why.to_string());
                done = true;
            }
        });
        done
    }

    /// The task's run that has not ended, if any.
    pub fn active_of_task(&self, task: &str) -> Option<String> {
        self.lock()
            .runs
            .values()
            .find(|e| e.run.task.as_deref() == Some(task) && e.walk.outcome.is_none())
            .map(|e| e.run.id.clone())
    }

    fn active_by_subject(&self, trigger: &str, subject: &str) -> Option<String> {
        self.lock()
            .runs
            .values()
            .find(|e| {
                e.run.trigger == trigger
                    && e.run.subject.as_deref() == Some(subject)
                    && e.walk.outcome.is_none()
            })
            .map(|e| e.run.id.clone())
    }

    /// A run, for the routes: its record with its steps and edges, the recipe and the walk.
    pub fn view(&self, run: &str) -> Option<View> {
        self.lock().runs.get(run).map(view_of)
    }

    /// Every run, newest first, optionally of one task or project.
    pub fn list(&self, task: Option<&str>, project: Option<&str>) -> Vec<View> {
        let inner = self.lock();
        let mut out: Vec<View> = inner
            .runs
            .values()
            .filter(|e| task.is_none_or(|t| e.run.task.as_deref() == Some(t)))
            .filter(|e| project.is_none_or(|p| e.run.project.as_deref() == Some(p)))
            .map(view_of)
            .collect();
        out.sort_by_key(|v| std::cmp::Reverse(v.run.created_ms));
        out
    }

    /// The recipes a run can name: the built-ins and the owner's newest versions (the owner's win by version).
    pub fn recipes(&self) -> Vec<Recipe> {
        let inner = self.lock();
        let mut all = builtin();
        for versions in inner.versions.values() {
            all.extend(versions.iter().cloned());
        }
        all.sort_by(|a, b| a.id.cmp(&b.id).then(a.version.cmp(&b.version)));
        all
    }

    /// The newest version of recipe `id`, or the one named: the owner's saved version when there is one, else the
    /// built-in.
    pub fn recipe(&self, id: &str, version: Option<u32>) -> Option<Recipe> {
        self.recipes()
            .into_iter()
            .filter(|r| r.id == id && version.is_none_or(|v| r.version == v))
            .max_by_key(|r| r.version)
    }

    /// Saves a new version of a recipe (the owner's). It must check and be newer than every version there is.
    pub fn put_version(&self, recipe: Recipe, now: u64) -> Result<Recipe, String> {
        recipe.check()?;
        let newest = self
            .recipes()
            .iter()
            .filter(|r| r.id == recipe.id)
            .map(|r| r.version)
            .max()
            .unwrap_or(0);
        if recipe.version <= newest {
            return Err(format!(
                "{} is already at version {newest}; a new version is higher",
                recipe.id
            ));
        }
        let mut inner = self.lock();
        inner
            .saved_ms
            .insert((recipe.id.clone(), recipe.version), now);
        inner
            .versions
            .entry(recipe.id.clone())
            .or_default()
            .push(recipe.clone());
        Ok(recipe)
    }

    fn update(&self, run: &str, now: u64, f: impl FnOnce(&mut Entry)) {
        let mut inner = self.lock();
        if let Some(entry) = inner.runs.get_mut(run) {
            f(entry);
            // The walk decides the run's state once it has an outcome; a run the owner holds or that was cancelled
            // keeps its state until it is settled.
            if entry.run.state != RunState::Cancelled
                && let Some(outcome) = &entry.walk.outcome
            {
                entry.run.state = match outcome {
                    Outcome::Passed => RunState::Passed,
                    Outcome::Failed(why) => {
                        entry.run.outcome.get_or_insert_with(|| why.clone());
                        RunState::Failed
                    }
                };
            }
            entry.run.updated_ms = now;
        }
    }

    /// The runs' rows for the hub's store, each with its steps and edges.
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let inner = self.lock();
        let rows: Vec<RecipeRun> = inner.runs.values().map(stored).collect();
        apz_store::RowSet::list(def(), rows.iter())
    }

    /// Replaces the runs with what the store holds (at start). What was running is ready again: its session did not
    /// survive the hub.
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let stored: Vec<RecipeRun> = apz_store::table::load_list(conn, def())?;
        let mut runs = BTreeMap::new();
        let mut next = 0;
        for run in stored {
            let Ok(recipe) = Recipe::parse(&run.toml) else {
                continue;
            };
            let mut walk = walk_of(&recipe, &run);
            walk.recover();
            next = next.max(numbered(&run.id));
            runs.insert(run.id.clone(), Entry { run, recipe, walk });
        }
        let mut inner = self.lock();
        inner.runs = runs;
        inner.next = next;
        Ok(())
    }

    /// The owner's versions, for the hub's store.
    pub fn version_rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let inner = self.lock();
        let rows: Vec<VersionRow> = inner
            .versions
            .values()
            .flatten()
            .map(|r| VersionRow {
                recipe: r.id.clone(),
                version: r.version,
                toml: r.to_toml(),
                saved_ms: inner
                    .saved_ms
                    .get(&(r.id.clone(), r.version))
                    .copied()
                    .unwrap_or(0),
            })
            .collect();
        apz_store::RowSet::list(versions_def(), rows.iter())
    }

    /// Replaces the owner's versions with what the store holds (at start).
    pub fn load_versions(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let stored: Vec<VersionRow> = apz_store::table::load_list(conn, versions_def())?;
        let mut inner = self.lock();
        inner.versions.clear();
        inner.saved_ms.clear();
        for row in stored {
            let Ok(recipe) = Recipe::parse(&row.toml) else {
                continue;
            };
            inner
                .saved_ms
                .insert((row.recipe.clone(), row.version), row.saved_ms);
            inner.versions.entry(row.recipe).or_default().push(recipe);
        }
        Ok(())
    }
}

/// Where a recipe came from: the built-in one as the hub ships it, or a version the owner saved.
pub fn source_of(recipe: &Recipe) -> &'static str {
    if builtin().iter().any(|b| b == recipe) {
        "built_in"
    } else {
        "owner"
    }
}

fn numbered(id: &str) -> u64 {
    id.strip_prefix("rr-")
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// The run's record with its children filled from its walk, as the store keeps it.
fn stored(entry: &Entry) -> RecipeRun {
    let mut run = entry.run.clone();
    run.steps = entry
        .walk
        .steps
        .iter()
        .map(|(step, rec)| StepRow {
            step: step.clone(),
            state: rec.state,
            visits: rec.visits,
            items: rec.items,
            reported: rec.reported,
            gate: rec.gate.clone(),
        })
        .collect();
    run.edges = entry
        .walk
        .edges
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let edge = &entry.recipe.edges[i];
            EdgeRow {
                index: i as u32,
                from: edge.from.clone(),
                to: edge.to.clone(),
                state: e.state,
                taken: e.taken,
            }
        })
        .collect();
    run
}

fn view_of(entry: &Entry) -> View {
    View {
        run: stored(entry),
        recipe: entry.recipe.clone(),
        walk: entry.walk.clone(),
    }
}

/// The walk a stored run had, from its children (a step or edge the store lacks starts where the recipe does).
fn walk_of(recipe: &Recipe, run: &RecipeRun) -> Walk {
    let (mut walk, _) = Walk::start(recipe, &BTreeMap::new());
    for row in &run.steps {
        if let Some(rec) = walk.steps.get_mut(&row.step) {
            *rec = StepRec {
                state: row.state,
                visits: row.visits,
                items: row.items,
                reported: row.reported,
                gate: row.gate.clone(),
            };
        }
    }
    for row in &run.edges {
        if let Some(edge) = walk.edges.get_mut(row.index as usize) {
            *edge = EdgeRec {
                state: row.state,
                taken: row.taken,
            };
        }
    }
    walk.outcome = match run.state {
        RunState::Passed => Some(Outcome::Passed),
        RunState::Failed | RunState::Cancelled => Some(Outcome::Failed(
            run.outcome.clone().unwrap_or_else(|| "failed".into()),
        )),
        RunState::Running | RunState::Owner => None,
    };
    walk
}

/// The runs' table.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(RecipeRun)
}

/// The owner's recipe versions' table.
pub fn versions_def() -> &'static apz_store::TableDef {
    apz_store::root_def!(VersionRow)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHANGE: &str = r#"
id = "change"
version = 1
[[steps]]
id = "build"
role = "builder"
[[steps]]
id = "review"
seat = "reviewer"
role = "reviewer"
model = "differ"
[[edges]]
from = "build"
to = "review"
when = "passed"
[[edges]]
from = "review"
to = "build"
when = "failed"
max = 2
"#;

    #[test]
    fn a_task_run_settles_through_its_walk_and_records_its_state() {
        let runs = Runs::default();
        let recipe = Recipe::parse(CHANGE).unwrap();
        let (id, moves) = runs.start(&recipe, "built_in", "task", Some("t1"), Some("p1"), None, 1);
        assert_eq!(moves, vec![Move::Ready("build".into())]);
        assert_eq!(runs.active_of_task("t1"), Some(id.clone()));
        runs.task_step_started("t1", "build", 2);
        assert_eq!(
            runs.settle_task("t1", "build", &Report::verdict(true), 3),
            vec![Move::Ready("review".into())]
        );
        runs.task_step_started("t1", "review", 4);
        let moves = runs.settle_task("t1", "review", &Report::verdict(true), 5);
        assert_eq!(moves, vec![Move::Done]);
        let view = runs.view(&id).unwrap();
        assert_eq!(view.run.state, RunState::Passed);
        assert_eq!(view.run.steps.len(), 2);
        assert!(runs.active_of_task("t1").is_none());
    }

    #[test]
    fn a_run_survives_a_restart_with_its_walk_and_its_versions() {
        let runs = Runs::default();
        let recipe = Recipe::parse(CHANGE).unwrap();
        let (id, _) = runs.start(&recipe, "built_in", "task", Some("t1"), None, None, 1);
        runs.task_step_started("t1", "build", 2);
        runs.settle_task("t1", "build", &Report::verdict(true), 3);
        let mut v2 = recipe.clone();
        v2.version = 2;
        runs.put_version(v2, 4).unwrap();

        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        conn.execute_batch(&versions_def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &runs.rows().unwrap()).unwrap();
        apz_store::table::replace(&conn, versions_def(), &runs.version_rows().unwrap()).unwrap();

        let loaded = Runs::default();
        loaded.load(&conn).unwrap();
        loaded.load_versions(&conn).unwrap();
        let view = loaded.view(&id).unwrap();
        assert_eq!(view.walk.steps["review"].state, StepState::Ready);
        assert_eq!(view.walk.steps["build"].state, StepState::Passed);
        assert_eq!(loaded.recipe("change", None).unwrap().version, 2);
        // The next run takes a new id, after the stored ones.
        let (next, _) = loaded.start(&recipe, "built_in", "task", Some("t2"), None, None, 9);
        assert_eq!(next, "rr-2");
    }

    #[test]
    fn a_version_must_be_newer_and_check() {
        let runs = Runs::default();
        let mut same = builtin().into_iter().find(|r| r.id == "change").unwrap();
        assert!(
            runs.put_version(same.clone(), 1)
                .unwrap_err()
                .contains("higher")
        );
        same.version = 2;
        same.steps.clear();
        assert!(runs.put_version(same, 1).unwrap_err().contains("no steps"));
    }

    #[test]
    fn an_owner_held_review_stays_open_until_it_is_settled_and_a_cancel_ends_a_run() {
        let runs = Runs::default();
        let recipe = builtin().into_iter().find(|r| r.id == "review").unwrap();
        let (id, _) = runs.start(
            &recipe,
            "built_in",
            "review",
            Some("t9"),
            None,
            Some("rv3"),
            1,
        );
        runs.started(&id, "graph", 2);
        runs.finish_subject("review", "rv3", &Report::verdict(false), "owner", true, 3);
        assert_eq!(runs.view(&id).unwrap().run.state, RunState::Owner);

        let (other, _) = runs.start(
            &recipe,
            "built_in",
            "review",
            Some("t8"),
            None,
            Some("rv4"),
            4,
        );
        runs.started(&other, "graph", 5);
        runs.finish_subject("review", "rv4", &Report::verdict(true), "pass", false, 6);
        let done = runs.view(&other).unwrap();
        assert_eq!(done.run.state, RunState::Passed);
        assert_eq!(done.run.outcome.as_deref(), Some("pass"));

        assert!(runs.cancel(&id, "stopped by the owner", 7));
        let stopped = runs.view(&id).unwrap();
        assert_eq!(stopped.run.state, RunState::Cancelled);
        assert!(!runs.cancel(&id, "again", 8));
    }
}
