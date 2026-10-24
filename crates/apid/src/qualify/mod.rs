//! The qualification of a release candidate (PLAN 22.10, 22.11): the eleven stages as the `qualify` recipe's steps with
//! their gates, the depth the planner chose for the channel, the verdict, the fix tasks, the flakes and quarantines, the
//! acceptances with a second model's agreement, the release report and the trends.
//!
//! - `plan.rs`: the stages and the depth per channel (pure).
//! - `result.rs`: a stage's result, the findings and metrics it reports, and the verdict over the stages (pure).
//! - `testcases.rs`: re-runs of failed tests, the flake history, and quarantine (pure, the clock is given).
//! - `accept.rs`: a failure accepted with a reason, the planner's proposal and the second model's agreement.
//! - `compat.rs`: the adapter the compatibility part of stage 9 reads (10.10 fills it).
//! - `stages.rs`: each stage's result path over the lab (the [`stages::Lab`] port) and the release's artifacts.
//! - `engine.rs`: starts and walks a qualification on the restricted recipe, judges the verdict, and files the release's
//!   review graph.
//! - `store.rs`: the records in plaza.db (component `qualification`).
//! - `report.rs`: the release report and the trends.
//! - `routes.rs`: the HTTP routes.

pub mod accept;
pub mod compat;
pub mod engine;
pub mod plan;
pub mod report;
pub mod result;
pub mod routes;
pub mod stages;
pub mod store;
pub mod testcases;

#[cfg(test)]
mod tests;

pub use store::Store;
