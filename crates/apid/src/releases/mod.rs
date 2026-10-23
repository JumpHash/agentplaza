//! Releases as entities (PLAN 22.2, 22.12): a release is a record with its own life, routes and view, not the last step
//! of a recipe. Its life is Weft's `release.*` statements (weft's `state.rs`), read back into the entity after each one.
//!
//! - `entity.rs`: the typed release (`release` and its child tables in `plaza.db`) and its store.
//! - `flow.rs`: draft, candidate (every target built through the `release` recipe's fan-out), checks, qualification,
//!   publication, promotion and yanks; the [`flow::Ledger`] and [`flow::Builder`] seams.
//! - `versions.rs`: a release's version from its changes (22.3): the bumps, and the project's schemes.
//! - `channels.rs`: channels, their qualification depth and the store tracks they follow (22.3).
//! - `lanes.rs`: release lines, backports with their own review, and the lines' queues (22.3).
//! - `routes.rs`: `/v1/projects/{id}/releases`, `/v1/releases/{id}/…` and the backports.

pub mod channels;
pub mod entity;
pub mod flow;
pub mod lanes;
pub mod routes;
#[cfg(test)]
mod tests;
pub mod versions;
