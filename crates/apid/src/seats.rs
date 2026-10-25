//! Seats (PLAN 4.2, 7.2) and the change recipe's runs (Engine 0.4, 4.1).
//!
//! A seat is a step of a task's recipe: `builder`, `reviewer`, … A task's recipe (the built-in `change` recipe, or a
//! project's copy) gives its seats: one per step, named by the step's seat, and one per item of a step that runs
//! several (a fan-out of n attempts, or one per file, target or finding: `builder#1`, `builder#2`, …). Each seat serves
//! its step and is filled by one session at a time. Messages are addressed to seats, not sessions (threads.rs), so
//! direct talk survives sessions ending. A session belongs to its task's project by construction.
//!
//! What a step's outcome does to the task is decided by the recipe's walk (recipes/graph.rs): the caller passes the
//! walk's moves to [`Seats::ended`], and the seats follow them, for every seat of a step: a step that is ready starts,
//! a loop rewinds to its builder with what the review said, and the run's end or failure ends the task. A crashed
//! session is the seat's own business: it goes again, at most [`ATTEMPTS`] times.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use crate::recipes::graph::items_of;
use crate::recipes::model::Model;
use crate::recipes::{Move, Recipe};

/// Where a seat is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum SeatState {
    /// Its step's inputs are not ready (an earlier step has not passed).
    Waiting,
    /// Its step can start: a session is to be started for it.
    Ready,
    /// A session fills it.
    Filled,
    /// Its step passed.
    Done,
    /// Its step failed for good (attempts or rounds used up).
    Failed,
}

/// Which models may fill a seat. A step's model list or `eval` is chosen per session (work_loop.rs), so a seat of one
/// is `any`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum ModelPolicy {
    /// Any model the project allows for the role.
    Any,
    /// A model that no other seat of the task used (a reviewer's differs from the builder's, W27).
    Differ,
}

/// How many sessions a seat may go through for one round (a session that failed to start or died).
pub const ATTEMPTS: u32 = 3;

/// One seat of a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "seat")]
pub struct Seat {
    #[table(key)]
    pub task: String,
    #[table(key)]
    pub name: String,
    /// The recipe the task runs (its id).
    pub recipe: String,
    /// The recipe step the seat serves.
    pub step: String,
    pub role: String,
    pub model_policy: ModelPolicy,
    pub state: SeatState,
    /// The session filling it now.
    pub session: Option<String>,
    /// The model of the last session that filled it (what a `differ` seat must avoid on the other seats).
    pub last_model: Option<String>,
    /// Sessions started for it in this round.
    pub attempts: u32,
    /// The recipe's loop round it is in (1 at the start; a failed review starts round 2 for the builder).
    pub round: u32,
    /// What the step that sent the task back said (the findings), for the next session of this seat.
    #[table(content)]
    pub returned_with: Option<String>,
    pub updated_ms: u64,
}

/// What happened to a task when one of its steps ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// The next seat is ready.
    Ready(String),
    /// The task's recipe is done.
    Done,
    /// The step failed and its seat goes again (a new session).
    Retry(String),
    /// A later step failed: the first step's seat goes again with what it said.
    Back(String),
    /// Out of attempts or rounds: the task failed.
    Failed(String),
}

/// Every seat, by task and name.
#[derive(Default)]
pub struct Seats {
    seats: Mutex<BTreeMap<(String, String), Seat>>,
}

impl Seats {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<(String, String), Seat>> {
        self.seats.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Gives `task` the seats of `recipe` with no named fan-out counts (see [`Seats::open_with_items`]).
    pub fn open(&self, task: &str, recipe: &Recipe, now_ms: u64) -> Result<Vec<Seat>, String> {
        self.open_with_items(task, recipe, &BTreeMap::new(), now_ms)
    }

    /// Gives `task` the seats of `recipe`: one per step, and one per item of a step that runs several (`items` counts a
    /// named fan-out, as the walk does); the steps that start the recipe are ready. A task that has seats keeps them.
    /// Errors for a step with no role: it runs by its engine, not in a task.
    pub fn open_with_items(
        &self,
        task: &str,
        recipe: &Recipe,
        items: &BTreeMap<String, u32>,
        now_ms: u64,
    ) -> Result<Vec<Seat>, String> {
        let mut seats = self.lock();
        if seats.keys().any(|(t, _)| t == task) {
            return Ok(seats.values().filter(|s| s.task == task).cloned().collect());
        }
        let mut out = Vec::new();
        for step in &recipe.steps {
            let Some(role) = &step.role else {
                return Err(format!("step {} has no role: its engine runs it", step.id));
            };
            let model_policy = match &step.model {
                Model::Name(m) if m == "differ" => ModelPolicy::Differ,
                _ => ModelPolicy::Any,
            };
            let count = items_of(step, items);
            let state = if recipe.has_forward_edge_into(&step.id) {
                SeatState::Waiting
            } else {
                SeatState::Ready
            };
            for i in 1..=count {
                let name = if count == 1 {
                    step.seat_name().to_string()
                } else {
                    format!("{}#{i}", step.seat_name())
                };
                let seat = Seat {
                    task: task.to_string(),
                    name: name.clone(),
                    recipe: recipe.id.clone(),
                    step: step.id.clone(),
                    role: role.clone(),
                    model_policy,
                    state,
                    session: None,
                    last_model: None,
                    attempts: 0,
                    round: 1,
                    returned_with: None,
                    updated_ms: now_ms,
                };
                seats.insert((task.to_string(), name), seat.clone());
                out.push(seat);
            }
        }
        Ok(out)
    }

    /// Gives `step` of `task` more seats, up to `count` (a risky change's third reviewer), as the seats it has: they
    /// start waiting, like the step's others.
    pub fn grow(&self, task: &str, step: &str, count: u32, now_ms: u64) {
        let mut seats = self.lock();
        let existing: Vec<Seat> = seats
            .values()
            .filter(|s| s.task == task && s.step == step)
            .cloned()
            .collect();
        let Some(template) = existing.first().cloned() else {
            return;
        };
        for i in existing.len() as u32 + 1..=count {
            let name = format!(
                "{}#{i}",
                template.name.split('#').next().unwrap_or(&template.name)
            );
            let seat = Seat {
                name: name.clone(),
                state: SeatState::Waiting,
                session: None,
                last_model: None,
                attempts: 0,
                returned_with: None,
                updated_ms: now_ms,
                ..template.clone()
            };
            seats.insert((task.to_string(), name), seat);
        }
    }

    pub fn get(&self, task: &str, name: &str) -> Option<Seat> {
        self.lock()
            .get(&(task.to_string(), name.to_string()))
            .cloned()
    }

    /// The seats of `task`, in their recipe's order.
    pub fn of_task(&self, task: &str) -> Vec<Seat> {
        let seats = self.lock();
        let mut out: Vec<Seat> = seats.values().filter(|s| s.task == task).cloned().collect();
        out.sort_by_key(|s| s.step.clone());
        out
    }

    /// Every seat that is ready for a session.
    pub fn ready(&self) -> Vec<Seat> {
        self.lock()
            .values()
            .filter(|s| s.state == SeatState::Ready)
            .cloned()
            .collect()
    }

    /// The models a `differ` seat of `task` must not use: those of the task's other seats' sessions.
    pub fn models_to_avoid(&self, task: &str, seat: &str) -> Vec<String> {
        self.lock()
            .values()
            .filter(|s| s.task == task && s.name != seat)
            .filter_map(|s| s.last_model.clone())
            .collect()
    }

    /// Records that `session` (on `model`) fills the seat.
    pub fn fill(
        &self,
        task: &str,
        name: &str,
        session: &str,
        model: Option<&str>,
        now_ms: u64,
    ) -> Option<Seat> {
        let mut seats = self.lock();
        let seat = seats.get_mut(&(task.to_string(), name.to_string()))?;
        seat.state = SeatState::Filled;
        seat.session = Some(session.to_string());
        if let Some(model) = model {
            seat.last_model = Some(model.to_string());
        }
        seat.attempts += 1;
        seat.updated_ms = now_ms;
        Some(seat.clone())
    }

    /// The session of a seat ended. `crashed`: the session died or never started (another attempt, not a failed step);
    /// otherwise `passed` says whether the step passed and `moves` are what the recipe's walk did with that (the
    /// seats follow them, every seat of a step at once). `said` is what the session reported (a failed review's
    /// findings go to the builder's next session).
    pub fn ended(
        &self,
        task: &str,
        name: &str,
        passed: bool,
        crashed: bool,
        said: &str,
        now_ms: u64,
        moves: &[Move],
    ) -> Option<Next> {
        let mut seats = self.lock();
        let key = (task.to_string(), name.to_string());
        let seat = seats.get(&key)?.clone();
        let set =
            |seats: &mut BTreeMap<(String, String), Seat>, name: &str, f: &dyn Fn(&mut Seat)| {
                if let Some(s) = seats.get_mut(&(task.to_string(), name.to_string())) {
                    f(s);
                    s.session = None;
                    s.updated_ms = now_ms;
                }
            };
        if crashed {
            if seat.attempts >= ATTEMPTS {
                set(&mut seats, name, &|s| s.state = SeatState::Failed);
                return Some(Next::Failed(format!(
                    "seat {name} failed to finish in {ATTEMPTS} sessions"
                )));
            }
            set(&mut seats, name, &|s| s.state = SeatState::Ready);
            return Some(Next::Retry(name.to_string()));
        }
        set(&mut seats, name, &|s| {
            s.state = if passed {
                SeatState::Done
            } else {
                SeatState::Failed
            }
        });
        // The seats of a step: one, or one per item.
        let seats_of = |seats: &BTreeMap<(String, String), Seat>, step: &str| -> Vec<String> {
            seats
                .values()
                .filter(|s| s.task == task && s.step == step)
                .map(|s| s.name.clone())
                .collect()
        };
        let mut next: Option<Next> = None;
        for m in moves {
            match m {
                Move::Ready(step) => {
                    let to = seats_of(&seats, step);
                    for name in &to {
                        set(&mut seats, name, &|s| {
                            s.state = SeatState::Ready;
                            s.attempts = 0;
                        });
                    }
                    if let Some(first) = to.into_iter().next() {
                        next.get_or_insert(Next::Ready(first));
                    }
                }
                Move::Rewind(step) => {
                    let to = seats_of(&seats, step);
                    let said = said.to_string();
                    for name in &to {
                        set(&mut seats, name, &|s| {
                            s.state = SeatState::Ready;
                            s.attempts = 0;
                            s.round += 1;
                            s.returned_with = Some(said.clone());
                        });
                    }
                    if let Some(first) = to.into_iter().next() {
                        next = Some(Next::Back(first));
                    }
                }
                Move::Reset(step) => {
                    for name in seats_of(&seats, step) {
                        set(&mut seats, &name, &|s| {
                            s.state = SeatState::Waiting;
                            s.attempts = 0;
                        });
                    }
                }
                Move::Done => {
                    next.get_or_insert(Next::Done);
                }
                Move::Failed(why) => {
                    set(&mut seats, name, &|s| s.state = SeatState::Failed);
                    return Some(Next::Failed(why.clone()));
                }
            }
        }
        next
    }

    /// Drops every seat of `task` (it ended).
    pub fn close(&self, task: &str) {
        self.lock().retain(|(t, _), _| t != task);
    }

    /// The seats' rows, for the hub's store.
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let seats = self.lock();
        apz_store::RowSet::list(def(), seats.values())
    }

    /// Replaces the seats with what the store holds (at start). A seat that was filled is ready again: its session did
    /// not survive the hub.
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let stored: Vec<Seat> = apz_store::table::load_list(conn, def())?;
        let mut seats = BTreeMap::new();
        for mut seat in stored {
            if seat.state == SeatState::Filled {
                seat.state = SeatState::Ready;
                seat.session = None;
            }
            seats.insert((seat.task.clone(), seat.name.clone()), seat);
        }
        *self.lock() = seats;
        Ok(())
    }
}

/// The seats' table.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Seat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipes::runs::{RunState, Runs};
    use crate::recipes::{Report, graph::Walk, model::builtin_named};

    /// The change recipe and a walk of it, as a task's run keeps them.
    fn change() -> (Recipe, Walk) {
        let recipe = builtin_named("change").unwrap();
        let (walk, _) = Walk::start(&recipe, &BTreeMap::new());
        (recipe, walk)
    }

    /// The moves of a step's outcome in the walk.
    fn settle(walk: &mut Walk, recipe: &Recipe, step: &str, passed: bool) -> Vec<Move> {
        walk.started(step);
        walk.settle(recipe, step, &Report::verdict(passed))
    }

    /// Settles `step` with `report` (a step's verdict, and the count of findings a verifier reports), and returns the moves.
    fn settle_with(walk: &mut Walk, recipe: &Recipe, step: &str, report: Report) -> Vec<Move> {
        walk.started(step);
        walk.settle(recipe, step, &report)
    }

    #[test]
    fn a_clean_change_goes_build_then_two_sceptics_then_the_verifier_and_is_done() {
        let (recipe, mut walk) = change();
        let seats = Seats::default();
        let opened = seats.open("t1", &recipe, 1).unwrap();
        assert_eq!(
            opened.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["builder", "reviewer#1", "reviewer#2", "verifier"]
        );
        seats.fill("t1", "builder", "b1", Some("a/m1"), 2).unwrap();
        let moves = settle(&mut walk, &recipe, "build", true);
        assert_eq!(
            seats.ended("t1", "builder", true, false, "done", 3, &moves),
            Some(Next::Ready("reviewer#1".into()))
        );
        // Both sceptics review in parallel: the step passes when the second one reports.
        seats
            .fill("t1", "reviewer#1", "r1", Some("b/m2"), 4)
            .unwrap();
        seats
            .fill("t1", "reviewer#2", "r2", Some("b/m3"), 4)
            .unwrap();
        let moves = settle_with(&mut walk, &recipe, "review", Report::verdict(true));
        assert!(moves.is_empty(), "one sceptic of two has not reported");
        assert_eq!(
            seats.ended("t1", "reviewer#1", true, false, "fine", 5, &moves),
            None
        );
        let moves = settle_with(&mut walk, &recipe, "review", Report::verdict(true));
        assert_eq!(
            seats.ended("t1", "reviewer#2", true, false, "fine", 6, &moves),
            Some(Next::Ready("verifier".into()))
        );
        // The verifier finds nothing confirmed: the change is done.
        seats.fill("t1", "verifier", "v1", Some("c/m4"), 7).unwrap();
        let moves = settle_with(
            &mut walk,
            &recipe,
            "verify",
            Report {
                passed: true,
                findings: Some(0),
                ..Report::default()
            },
        );
        assert_eq!(
            seats.ended("t1", "verifier", true, false, "nothing to prove", 8, &moves),
            Some(Next::Done)
        );
    }

    #[test]
    fn a_confirmed_finding_sends_the_builder_back_at_most_twice_and_then_the_run_fails() {
        let (recipe, mut walk) = change();
        let seats = Seats::default();
        seats.open("t1", &recipe, 1).unwrap();
        // Three rounds: the verifier confirms a finding in each of the first two, which sends the builder back; the
        // third confirmation is past the loop's bound, and the run fails.
        for round in 1..=3u32 {
            seats
                .fill("t1", "builder", &format!("b{round}"), Some("a/m1"), 2)
                .unwrap();
            let moves = settle(&mut walk, &recipe, "build", true);
            assert_eq!(
                seats.ended("t1", "builder", true, false, "done", 3, &moves),
                Some(Next::Ready("reviewer#1".into()))
            );
            seats
                .fill("t1", "reviewer#1", &format!("r{round}a"), Some("b/m2"), 4)
                .unwrap();
            seats
                .fill("t1", "reviewer#2", &format!("r{round}b"), Some("b/m3"), 4)
                .unwrap();
            let moves = settle_with(&mut walk, &recipe, "review", Report::verdict(true));
            seats.ended("t1", "reviewer#1", true, false, "fine", 5, &moves);
            let moves = settle_with(&mut walk, &recipe, "review", Report::verdict(true));
            assert_eq!(
                seats.ended("t1", "reviewer#2", true, false, "fine", 6, &moves),
                Some(Next::Ready("verifier".into()))
            );
            seats
                .fill("t1", "verifier", &format!("v{round}"), Some("c/m4"), 7)
                .unwrap();
            let moves = settle_with(
                &mut walk,
                &recipe,
                "verify",
                Report {
                    passed: true,
                    findings: Some(1),
                    ..Report::default()
                },
            );
            let next = seats.ended("t1", "verifier", true, false, "f1 confirmed", 8, &moves);
            if round < 3 {
                assert_eq!(next, Some(Next::Back("builder".into())), "round {round}");
                assert_eq!(
                    seats.get("t1", "builder").unwrap().returned_with.as_deref(),
                    Some("f1 confirmed")
                );
            } else {
                assert!(matches!(next, Some(Next::Failed(_))), "{next:?}");
            }
        }
    }

    #[test]
    fn a_crashed_session_is_retried_then_the_seat_fails() {
        let (recipe, _) = change();
        let seats = Seats::default();
        seats.open("t1", &recipe, 1).unwrap();
        for i in 1..ATTEMPTS {
            seats
                .fill("t1", "builder", &format!("s{i}"), None, 2)
                .unwrap();
            assert_eq!(
                seats.ended("t1", "builder", false, true, "", 3, &[]),
                Some(Next::Retry("builder".into()))
            );
        }
        seats.fill("t1", "builder", "sx", None, 2).unwrap();
        assert!(matches!(
            seats.ended("t1", "builder", false, true, "", 3, &[]),
            Some(Next::Failed(_))
        ));
    }

    #[test]
    fn a_run_that_fails_fails_the_task_with_the_walk_s_reason() {
        let (recipe, mut walk) = change();
        let seats = Seats::default();
        seats.open("t1", &recipe, 1).unwrap();
        seats.fill("t1", "builder", "s1", None, 2).unwrap();
        let moves = settle(&mut walk, &recipe, "build", false);
        let next = seats.ended("t1", "builder", false, false, "stuck", 3, &moves);
        assert_eq!(next, Some(Next::Failed("the build step failed".into())));
        assert_eq!(seats.get("t1", "builder").unwrap().state, SeatState::Failed);
    }

    #[test]
    fn a_fan_out_of_n_attempts_has_a_seat_per_attempt_and_passes_when_every_attempt_passes() {
        let recipe = Recipe::parse(
            r#"
id = "best-of"
version = 1
[[steps]]
id = "build"
seat = "builder"
role = "builder"
fan_out = 3
[[steps]]
id = "review"
seat = "reviewer"
role = "reviewer"
model = "differ"
[[edges]]
from = "build"
to = "review"
when = "passed"
"#,
        )
        .unwrap();
        let seats = Seats::default();
        let opened = seats.open("t2", &recipe, 1).unwrap();
        let names: Vec<&str> = opened.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["builder#1", "builder#2", "builder#3", "reviewer"]);
        assert_eq!(seats.ready().len(), 3, "all three attempts start at once");

        let (mut walk, _) = Walk::start(&recipe, &BTreeMap::new());
        let mut moves_of = Vec::new();
        for i in 1..=3 {
            let name = format!("builder#{i}");
            seats.fill("t2", &name, &format!("s{i}"), None, 2).unwrap();
            walk.started("build");
            let moves = walk.settle(&recipe, "build", &Report::verdict(true));
            moves_of.push(seats.ended("t2", &name, true, false, "done", 3, &moves));
        }
        // Only the last attempt to pass moves the task on.
        assert_eq!(moves_of, [None, None, Some(Next::Ready("reviewer".into()))]);
        assert_eq!(seats.get("t2", "builder#2").unwrap().state, SeatState::Done);
    }

    #[test]
    fn a_named_fan_out_takes_the_items_its_run_is_given_and_one_failing_item_fails_the_step() {
        let recipe = Recipe::parse(
            r#"
id = "per-file"
version = 1
[[steps]]
id = "check"
seat = "checker"
role = "reviewer"
fan_out = "per_file"
"#,
        )
        .unwrap();
        let items = BTreeMap::from([("check".to_string(), 2u32)]);
        let seats = Seats::default();
        let opened = seats.open_with_items("t3", &recipe, &items, 1).unwrap();
        assert_eq!(opened.len(), 2);
        assert_eq!(opened[1].name, "checker#2");

        let (mut walk, _) = Walk::start(&recipe, &items);
        seats.fill("t3", "checker#1", "s1", None, 2).unwrap();
        walk.started("check");
        let moves = walk.settle(&recipe, "check", &Report::verdict(false));
        assert_eq!(
            seats.ended(
                "t3",
                "checker#1",
                false,
                false,
                "the file is wrong",
                3,
                &moves
            ),
            Some(Next::Failed("the check step failed".into()))
        );
        assert!(walk.outcome.is_some());
        assert_eq!(
            seats.get("t3", "checker#2").unwrap().state,
            SeatState::Ready
        );
    }

    #[test]
    fn a_recipe_with_an_engine_step_in_a_task_is_refused_and_a_list_model_is_no_refusal() {
        let mut recipe = builtin_named("change").unwrap();
        recipe.steps[0].role = None;
        recipe.steps[0].engine = Some("harness".into());
        assert!(
            Seats::default()
                .open("t1", &recipe, 1)
                .unwrap_err()
                .contains("no role")
        );

        let mut recipe = builtin_named("change").unwrap();
        recipe.steps[0].model = Model::List(vec!["a/one".into(), "b/two".into()]);
        let opened = Seats::default().open("t2", &recipe, 1).unwrap();
        assert_eq!(opened[0].model_policy, ModelPolicy::Any);
    }

    #[test]
    fn seats_round_trip_and_a_filled_seat_is_ready_after_a_restart() {
        let (recipe, _) = change();
        let seats = Seats::default();
        seats.open("t1", &recipe, 1).unwrap();
        seats.fill("t1", "builder", "s1", Some("a/m1"), 2).unwrap();
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &seats.rows().unwrap()).unwrap();
        let loaded = Seats::default();
        loaded.load(&conn).unwrap();
        let builder = loaded.get("t1", "builder").unwrap();
        assert_eq!(
            (builder.state, builder.session.clone()),
            (SeatState::Ready, None)
        );
        assert_eq!(builder.last_model.as_deref(), Some("a/m1"));
    }

    /// A project's recipe file: a review that must also be green, with one loop back to the builder.
    const GATED: &str = r#"
id = "gated"
version = 1
scope = "project"
[[steps]]
id = "build"
seat = "builder"
role = "builder"
model = "any"
[[steps]]
id = "check"
seat = "checker"
role = "reviewer"
model = "differ"
gate = ["passed", "tests green"]
[[edges]]
from = "build"
to = "check"
when = "passed"
[[edges]]
from = "check"
to = "build"
when = "failed"
max = 1
"#;

    /// Runs one step the way the work loop does when a fake session reports: the run settles the step, the seats follow.
    fn report(seats: &Seats, runs: &Runs, task: &str, seat: &str, r: &Report) -> Option<Next> {
        let step = seats.get(task, seat).unwrap().step;
        runs.task_step_started(task, &step, 0);
        let moves = runs.settle_task(task, &step, r, 0);
        seats.ended(
            task,
            seat,
            r.passed,
            false,
            "what the checker said",
            0,
            &moves,
        )
    }

    #[test]
    fn a_recipe_file_with_a_gate_and_a_bounded_loop_runs_to_its_outcome() {
        let recipe = Recipe::parse(GATED).unwrap();
        let runs = Runs::default();
        let (run, _) = runs.start(&recipe, "project", "task", Some("t7"), None, None, 1);
        let seats = Seats::default();
        seats.open("t7", &recipe, 1).unwrap();

        // First round: the checker says it passed, but its tests are not green: the gate fails and the loop sends the
        // change back to the builder.
        seats.fill("t7", "builder", "b1", Some("a/m1"), 2).unwrap();
        assert_eq!(
            report(&seats, &runs, "t7", "builder", &Report::verdict(true)),
            Some(Next::Ready("checker".into()))
        );
        seats.fill("t7", "checker", "c1", Some("b/m2"), 3).unwrap();
        let red = Report {
            passed: true,
            tests: Some(false),
            ..Report::default()
        };
        assert_eq!(
            report(&seats, &runs, "t7", "checker", &red),
            Some(Next::Back("builder".into()))
        );
        assert_eq!(seats.get("t7", "builder").unwrap().round, 2);

        // Second round: green, and the run passes.
        seats.fill("t7", "builder", "b2", Some("a/m1"), 4).unwrap();
        assert_eq!(
            report(&seats, &runs, "t7", "builder", &Report::verdict(true)),
            Some(Next::Ready("checker".into()))
        );
        seats.fill("t7", "checker", "c2", Some("b/m2"), 5).unwrap();
        let green = Report {
            passed: true,
            tests: Some(true),
            ..Report::default()
        };
        assert_eq!(
            report(&seats, &runs, "t7", "checker", &green),
            Some(Next::Done)
        );
        let view = runs.view(&run).unwrap();
        assert_eq!(view.run.state, RunState::Passed);
        assert_eq!(view.run.recipe, "gated");

        // The same file with a checker that never goes green: after its one loop, the run fails the task.
        let runs = Runs::default();
        let (run, _) = runs.start(&recipe, "project", "task", Some("t8"), None, None, 1);
        seats.open("t8", &recipe, 1).unwrap();
        for round in 0..2 {
            seats
                .fill("t8", "builder", &format!("b{round}"), None, 2)
                .unwrap();
            report(&seats, &runs, "t8", "builder", &Report::verdict(true));
            seats
                .fill("t8", "checker", &format!("c{round}"), None, 3)
                .unwrap();
            let next = report(&seats, &runs, "t8", "checker", &red);
            if round == 0 {
                assert_eq!(next, Some(Next::Back("builder".into())));
            } else {
                assert!(matches!(next, Some(Next::Failed(_))), "{next:?}");
            }
        }
        assert_eq!(runs.active_of_task("t8"), None);
        assert_eq!(runs.view(&run).unwrap().run.state, RunState::Failed);
    }
}
