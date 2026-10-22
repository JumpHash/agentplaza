//! Seats (PLAN 4.2, 7.2) and the built-in `change` recipe (Engine 0.4).
//!
//! A seat is a role in a task: `builder`, `reviewer`, … A task's recipe decides its seats; each seat serves one step
//! and is filled by one session at a time. Messages are addressed to seats, not sessions (threads.rs), so direct talk
//! survives sessions ending. Seats replace the bench and project membership: a session belongs to its task's project
//! by construction.
//!
//! The minimal `change` recipe runs every task until the recipe engine (phase 4) brings its own: the builder's step,
//! then a review by a reviewer whose model differs from the builder's (W27), then done; a review that fails sends the
//! task back to the builder with the findings, at most [`CHANGE_ROUNDS`] times before the task fails and the planner is
//! told.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

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

/// One step of a built-in recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Step {
    pub id: &'static str,
    pub seat: &'static str,
    pub role: &'static str,
    pub model_policy: ModelPolicy,
}

/// A built-in recipe: its steps in order, and how many times a failed later step may send the task back to the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Recipe {
    pub id: &'static str,
    pub steps: &'static [Step],
    pub rounds: u32,
}

/// How many times a review may send a change back to its builder.
pub const CHANGE_ROUNDS: u32 = 3;
/// How many sessions a seat may go through for one round (a session that failed to start or died).
pub const ATTEMPTS: u32 = 3;

/// The minimal change recipe: build, then a review on another model.
pub const CHANGE: Recipe = Recipe {
    id: "change",
    steps: &[
        Step {
            id: "build",
            seat: "builder",
            role: "builder",
            model_policy: ModelPolicy::Any,
        },
        Step {
            id: "review",
            seat: "reviewer",
            role: "reviewer",
            model_policy: ModelPolicy::Differ,
        },
    ],
    rounds: CHANGE_ROUNDS,
};

/// The built-in recipe named `id`.
pub fn recipe(id: &str) -> Option<&'static Recipe> {
    [&CHANGE].into_iter().find(|r| r.id == id)
}

/// One seat of a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "seat")]
pub struct Seat {
    #[table(key)]
    pub task: String,
    #[table(key)]
    pub name: String,
    pub recipe: String,
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

    /// Gives `task` the seats of `recipe`, the first step's ready. A task that has seats keeps them.
    pub fn open(&self, task: &str, recipe: &Recipe, now_ms: u64) -> Vec<Seat> {
        let mut seats = self.lock();
        if seats.keys().any(|(t, _)| t == task) {
            return seats.values().filter(|s| s.task == task).cloned().collect();
        }
        let mut out = Vec::new();
        for (i, step) in recipe.steps.iter().enumerate() {
            let seat = Seat {
                task: task.to_string(),
                name: step.seat.to_string(),
                recipe: recipe.id.to_string(),
                step: step.id.to_string(),
                role: step.role.to_string(),
                model_policy: step.model_policy,
                state: if i == 0 { SeatState::Ready } else { SeatState::Waiting },
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
        out
    }

    pub fn get(&self, task: &str, name: &str) -> Option<Seat> {
        self.lock().get(&(task.to_string(), name.to_string())).cloned()
    }

    /// The seats of `task`, in their recipe's order.
    pub fn of_task(&self, task: &str) -> Vec<Seat> {
        let seats = self.lock();
        let mut out: Vec<Seat> = seats.values().filter(|s| s.task == task).cloned().collect();
        let order = |s: &Seat| {
            recipe(&s.recipe)
                .and_then(|r| r.steps.iter().position(|st| st.id == s.step))
                .unwrap_or(usize::MAX)
        };
        out.sort_by_key(order);
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
    pub fn fill(&self, task: &str, name: &str, session: &str, model: Option<&str>, now_ms: u64) -> Option<Seat> {
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

    /// The session of a seat ended. `passed`: its step's gate held (the builder said done; the reviewer passed it).
    /// `crashed`: the session died or never started (another attempt, not a failed step). `said` is what it reported
    /// (a failed review's findings go to the builder's next session).
    pub fn ended(
        &self,
        task: &str,
        name: &str,
        passed: bool,
        crashed: bool,
        said: &str,
        now_ms: u64,
    ) -> Option<Next> {
        let mut seats = self.lock();
        let key = (task.to_string(), name.to_string());
        let seat = seats.get(&key)?.clone();
        let recipe = recipe(&seat.recipe)?;
        let index = recipe.steps.iter().position(|s| s.id == seat.step)?;
        let set = |seats: &mut BTreeMap<(String, String), Seat>, name: &str, f: &dyn Fn(&mut Seat)| {
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
        if passed {
            set(&mut seats, name, &|s| s.state = SeatState::Done);
            return match recipe.steps.get(index + 1) {
                Some(next) => {
                    set(&mut seats, next.seat, &|s| {
                        s.state = SeatState::Ready;
                        s.attempts = 0;
                    });
                    Some(Next::Ready(next.seat.to_string()))
                }
                None => Some(Next::Done),
            };
        }
        // The step failed. The first step failing is the task failing; a later step sends it back to the first.
        if index == 0 {
            set(&mut seats, name, &|s| s.state = SeatState::Failed);
            return Some(Next::Failed(format!("the {} step failed: {said}", seat.step)));
        }
        let first = recipe.steps[0].seat;
        let round = seats
            .get(&(task.to_string(), first.to_string()))
            .map_or(1, |s| s.round);
        if round >= recipe.rounds {
            set(&mut seats, name, &|s| s.state = SeatState::Failed);
            return Some(Next::Failed(format!(
                "the {} step failed {} times; last: {said}",
                seat.step, recipe.rounds
            )));
        }
        set(&mut seats, name, &|s| {
            s.state = SeatState::Waiting;
            s.attempts = 0;
        });
        let said = said.to_string();
        set(&mut seats, first, &|s| {
            s.state = SeatState::Ready;
            s.attempts = 0;
            s.round += 1;
            s.returned_with = Some(said.clone());
        });
        Some(Next::Back(first.to_string()))
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
    pub fn load(&self, conn: &apz_store::rusqlite::Connection) -> Result<(), apz_store::StoreError> {
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

    #[test]
    fn a_change_goes_build_review_done() {
        let seats = Seats::default();
        let opened = seats.open("t1", &CHANGE, 1);
        assert_eq!(opened.len(), 2);
        assert_eq!(seats.ready().iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["builder"]);
        seats.fill("t1", "builder", "s1", Some("a/m1"), 2).unwrap();
        assert_eq!(seats.ended("t1", "builder", true, false, "done", 3), Some(Next::Ready("reviewer".into())));
        assert_eq!(seats.models_to_avoid("t1", "reviewer"), vec!["a/m1".to_string()]);
        seats.fill("t1", "reviewer", "s2", Some("b/m2"), 4).unwrap();
        assert_eq!(seats.ended("t1", "reviewer", true, false, "looks right", 5), Some(Next::Done));
    }

    #[test]
    fn a_failed_review_sends_it_back_with_the_findings_a_bounded_number_of_times() {
        let seats = Seats::default();
        seats.open("t1", &CHANGE, 1);
        for round in 1..CHANGE_ROUNDS {
            seats.fill("t1", "builder", &format!("b{round}"), Some("a/m1"), 2).unwrap();
            seats.ended("t1", "builder", true, false, "done", 3).unwrap();
            seats.fill("t1", "reviewer", &format!("r{round}"), Some("b/m2"), 4).unwrap();
            let next = seats.ended("t1", "reviewer", false, false, "the date filter is ignored", 5);
            assert_eq!(next, Some(Next::Back("builder".into())));
            let builder = seats.get("t1", "builder").unwrap();
            assert_eq!(builder.returned_with.as_deref(), Some("the date filter is ignored"));
            assert_eq!(builder.round, round + 1);
        }
        seats.fill("t1", "builder", "bn", Some("a/m1"), 2).unwrap();
        seats.ended("t1", "builder", true, false, "done", 3).unwrap();
        seats.fill("t1", "reviewer", "rn", Some("b/m2"), 4).unwrap();
        assert!(matches!(seats.ended("t1", "reviewer", false, false, "still wrong", 5), Some(Next::Failed(_))));
    }

    #[test]
    fn a_crashed_session_is_retried_then_the_seat_fails() {
        let seats = Seats::default();
        seats.open("t1", &CHANGE, 1);
        for i in 1..ATTEMPTS {
            seats.fill("t1", "builder", &format!("s{i}"), None, 2).unwrap();
            assert_eq!(seats.ended("t1", "builder", false, true, "", 3), Some(Next::Retry("builder".into())));
        }
        seats.fill("t1", "builder", "sx", None, 2).unwrap();
        assert!(matches!(seats.ended("t1", "builder", false, true, "", 3), Some(Next::Failed(_))));
    }

    #[test]
    fn seats_round_trip_and_a_filled_seat_is_ready_after_a_restart() {
        let seats = Seats::default();
        seats.open("t1", &CHANGE, 1);
        seats.fill("t1", "builder", "s1", Some("a/m1"), 2).unwrap();
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &seats.rows().unwrap()).unwrap();
        let loaded = Seats::default();
        loaded.load(&conn).unwrap();
        let builder = loaded.get("t1", "builder").unwrap();
        assert_eq!((builder.state, builder.session.clone()), (SeatState::Ready, None));
        assert_eq!(builder.last_model.as_deref(), Some("a/m1"));
    }
}
