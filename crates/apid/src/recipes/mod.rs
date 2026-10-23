//! The recipe engine (PLAN 4.1, 21.1): recipes as files, graphs of steps with gates and bounded loops, and runs that
//! record every step's sessions and gate results.
//!
//! - `model.rs`: a recipe's file (TOML) and its checks; the built-ins (`builtin/`), a project's copy and the owner's
//!   versions.
//! - `graph.rs`: the walker, the state of one run's graph as pure data: which steps start, what a gate decides, how
//!   loops rewind and fan-outs pass.
//! - `runs.rs`: the runs, stored typed in `plaza.db`, and the calls the work loop and the other engines make.
//! - `routes.rs`: `/v1/recipe-runs`.
//!
//! The task's change runs its steps through the walker: its seats are the steps (seats.rs). The review, base and
//! distill runs record the step their existing engine runs (review_graph, base_recipe, curator).

pub mod graph;
pub mod model;
pub mod routes;
pub mod runs;

pub use graph::{Move, Report};
pub use model::Recipe;
