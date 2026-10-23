//! Seats (PLAN 4.2, 7.2) and the change recipe's runs (Engine 0.4, 4.1).
//!
//! A seat is a step of a task's recipe: `builder`, `reviewer`, … A task's recipe (the built-in `change` recipe, or a
//! project's copy) gives its seats: one per step, named by the step's seat. Each seat serves its step and is filled by
//! one session at a time. Messages are addressed to seats, not sessions (threads.rs), so direct talk survives sessions
//! ending. A session belongs to its task's project by construction.
//!
//! What a step's outcome does to the task is decided by the recipe's walk (recipes/graph.rs): the caller passes the
//! walk's moves to [`Seats::ended`], and the seats follow them: a step that is ready starts, a loop rewinds to its builder
//! with what the review said, and the run's end or failure ends the task. A crashed session is the seat's own business:
//! it goes again, at most [`ATTEMPTS`] times.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use crate::recipes::{Move, Recipe, model::Model};

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

/// Which models may fill a seat.
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

    /// Gives `task` the seats of `recipe`, one per step, the steps that start the recipe ready. A task that has seats
    /// keeps them. Errors when a step's seat cannot be run yet (a fan-out, or a model that is pinned).
    pub fn open(&self, task: &str, recipe: &Recipe, now_ms: u64) -> Result<Vec<Seat>, String> {
        let mut seats = self.lock();
        if seats.keys().any(|(t, _)| t == task) {
            return Ok(seats.values().filter(|s| s.task == task).cloned().collect());
        }
        let mut out = Vec::new();
        for step in &recipe.steps {
            let Some(role) = &step.role else {
                return Err(format!("step {} has no role: its engine runs it", step.id));
            };
            if step.fan_out != crate::recipes::model::Fan::Count(1) {
                return Err(format!(
                    "step {}: a seat runs one item (fan-out comes later)",
                    step.id
                ));
            }
            let model_policy = match &step.model {
                Model::Name(m) if m == "any" => ModelPolicy::Any,
                Model::Name(m) if m == "differ" => ModelPolicy::Differ,
                _ => return Err(format!("step {}: a seat's model is any or differ", step.id)),
            };
            let seat = Seat {
                task: task.to_string(),
                name: step.seat_name().to_string(),
                recipe: recipe.id.clone(),
                step: step.id.clone(),
                role: role.clone(),
                model_policy,
                state: if recipe.has_forward_edge_into(&step.id) {
                    SeatState::Waiting
                } else {
                    SeatState::Ready
                },
                session: None,
                last_model: None,
                attempts: 0,
                round: 1,
                returned_with: None,
                updated_ms: now_ms,
            };
            seats.insert((task.to_string(), seat.name.clone()), seat.clone());
            out.push(seat);
        }
        Ok(out)
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
    /// seats follow them). `said` is what the session reported (a failed review's findings go to the builder's next
    /// session).
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
        let seat_of = |seats: &BTreeMap<(String, String), Seat>, step: &str| {
            seats
                .values()
                .find(|s| s.task == task && s.step == step)
                .map(|s| s.name.clone())
        };
        let mut next: Option<Next> = None;
        for m in moves {
            match m {
                Move::Ready(step) => {
                    if let Some(to) = seat_of(&seats, step) {
                        set(&mut seats, &to, &|s| {
                            s.state = SeatState::Ready;
                            s.attempts = 0;
                        });
                        next.get_or_insert(Next::Ready(to));
                    }
                }
                Move::Rewind(step) => {
                    if let Some(to) = seat_of(&seats, step) {
                        let said = said.to_string();
                        set(&mut seats, &to, &|s| {
                            s.state = SeatState::Ready;
                            s.attempts = 0;
                            s.round += 1;
                            s.returned_with = Some(said.clone());
                        });
                        next = Some(Next::Back(to));
                    }
                }
                Move::Reset(step) => {
                    if let Some(to) = seat_of(&seats, step) {
                        set(&mut seats, &to, &|s| {
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

    #[test]
    fn a_change_goes_build_review_done() {
        let (recipe, mut walk) = change();
        let seats = Seats::default();
        let opened = seats.open("t1", &recipe, 1).unwrap();
        assert_eq!(opened.len(), 2);
        assert_eq!(
            seats
                .ready()
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["builder"]
        );
        seats.fill("t1", "builder", "s1", Some("a/m1"), 2).unwrap();
        let moves = settle(&mut walk, &recipe, "build", true);
        assert_eq!(
            seats.ended("t1", "builder", true, false, "done", 3, &moves),
            Some(Next::Ready("reviewer".into()))
        );
        assert_eq!(
            seats.models_to_avoid("t1", "reviewer"),
            vec!["a/m1".to_string()]
        );
        seats.fill("t1", "reviewer", "s2", Some("b/m2"), 4).unwrap();
        let moves = settle(&mut walk, &recipe, "review", true);
        assert_eq!(
            seats.ended("t1", "reviewer", true, false, "looks right", 5, &moves),
            Some(Next::Done)
        );
    }

    #[test]
    fn a_failed_review_sends_it_back_with_the_findings_a_bounded_number_of_times() {
        let (recipe, mut walk) = change();
        let seats = Seats::default();
        seats.open("t1", &recipe, 1).unwrap();
        // The change recipe's loop allows two rounds back: three reviews in all.
        for round in 1..=2 {
            seats
                .fill("t1", "builder", &format!("b{round}"), Some("a/m1"), 2)
                .unwrap();
            let moves = settle(&mut walk, &recipe, "build", true);
            seats
                .ended("t1", "builder", true, false, "done", 3, &moves)
                .unwrap();
            seats
                .fill("t1", "reviewer", &format!("r{round}"), Some("b/m2"), 4)
                .unwrap();
            let moves = settle(&mut walk, &recipe, "review", false);
            let next = seats.ended(
                "t1",
                "reviewer",
                false,
                false,
                "the date filter is ignored",
                5,
                &moves,
            );
            assert_eq!(next, Some(Next::Back("builder".into())));
            let builder = seats.get("t1", "builder").unwrap();
            assert_eq!(
                builder.returned_with.as_deref(),
                Some("the date filter is ignored")
            );
            assert_eq!(builder.round, round + 1);
            assert_eq!(
                seats.get("t1", "reviewer").unwrap().state,
                SeatState::Waiting
            );
        }
        seats.fill("t1", "builder", "bn", Some("a/m1"), 2).unwrap();
        let moves = settle(&mut walk, &recipe, "build", true);
        seats
            .ended("t1", "builder", true, false, "done", 3, &moves)
            .unwrap();
        seats.fill("t1", "reviewer", "rn", Some("b/m2"), 4).unwrap();
        let moves = settle(&mut walk, &recipe, "review", false);
        assert!(matches!(
            seats.ended("t1", "reviewer", false, false, "still wrong", 5, &moves),
            Some(Next::Failed(_))
        ));
        assert_eq!(
            seats.get("t1", "reviewer").unwrap().state,
            SeatState::Failed
        );
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
    fn a_recipe_s_steps_with_an_unrunnable_model_or_fan_out_are_refused() {
        let mut recipe = change();
        recipe.0.steps[0].fan_out = crate::recipes::model::Fan::Count(2);
        let seats = Seats::default();
        assert!(
            seats
                .open("t1", &recipe.0, 1)
                .unwrap_err()
                .contains("one item")
        );
        let mut recipe = change();
        recipe.0.steps[1].model = Model::Name("provider/pinned".into());
        assert!(
            Seats::default()
                .open("t2", &recipe.0, 1)
                .unwrap_err()
                .contains("any or differ")
        );
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
        assert_eq!(view.run.state, crate::recipes::runs::RunState::Passed);
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
