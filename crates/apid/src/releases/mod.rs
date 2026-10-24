//! Releases as entities (PLAN 22.2, 22.12): a release is a record with its own life, routes and view, not the last step
//! of a recipe. Its life is Weft's `release.*` statements (weft's `state.rs`), read back into the entity after each one.
//!
//! - `entity.rs`: the typed release (`release` and its child tables in `plaza.db`) and its store.
//! - `builder.rs`: the production builder: each target placed on a pool and built in a fork of the project's base.
//! - `flow.rs`: draft, candidate (every target built through the `release` recipe's fan-out), checks, qualification,
//!   publication, promotion and yanks; the [`flow::Ledger`] and [`flow::Builder`] seams.
//! - `versions.rs`: a release's version from its changes (22.3): the bumps, and the project's schemes.
//! - `channels.rs`: channels, their qualification depth and the store tracks they follow (22.3).
//! - `lanes.rs`: release lines, backports with their own review, and the lines' queues (22.3).
//! - `routes.rs`: `/v1/projects/{id}/releases`, `/v1/releases/{id}/…` and the backports.
//! - `sync.rs`: the release on GitHub, GitLab and Forgejo: its tag, notes, assets, pre-release flag, checks and yanks (22.4).
//! - `sbom.rs`, `attest.rs`, `signing.rs`: each artifact's CycloneDX SBOM, its in-toto provenance in a DSSE envelope,
//!   and its platform signatures (9.3); publish verifies them all first.
//! - `registries/`: each artifact published to its ecosystem's package registry after the release is published (9.5).
//! - `stores/`: a release promoted to a store track submits its build there: TestFlight, the App Store, Google Play (9.6).

pub mod attest;
pub mod builder;
pub mod channels;
pub mod entity;
pub mod flow;
pub mod lanes;
pub mod registries;
pub mod routes;
pub mod sbom;
#[cfg(test)]
mod sign_tests;
pub mod signing;
pub mod stores;
pub mod sync;
#[cfg(test)]
mod sync_tests;
#[cfg(test)]
mod tests;
pub mod versions;
