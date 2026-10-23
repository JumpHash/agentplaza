//! Test selection (PLAN 15.4): the tests a change needs, from a coverage map recorded on `main`.
//!
//! - [`map`]: the map's format, typed for the hub's store (one map per project and commit; the misses).
//! - [`llvm`]: recording the map from llvm-cov's per-test exports (Rust with coverage on).
//! - [`graph`]: the build graph, read from `cargo metadata`, `go list -json` and the npm workspace's manifests.
//! - [`inventory`]: the Rust tests of a repository (the `#[test]` functions, by file and package).
//! - [`select`]: the selection for a change, and the command that runs it.
//! - [`misses`]: the tests a selection skipped that failed in a full run, per area.
//!
//! The crate reads the tools' output and never runs them: the caller runs `cargo metadata`, `llvm-cov` and the rest on
//! the machine that records or selects.

#![deny(unsafe_code)]

use thiserror::Error;

pub mod graph;
pub mod inventory;
pub mod llvm;
pub mod map;
pub mod misses;
pub mod select;

pub use map::{CoverageMap, Definition, Miss, Package, TestEntry};
pub use select::{Change, Selection, Touch, Why, select};

/// What reading or recording a map can fail on.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// An llvm-cov export is not one, or names a file or a line that does not exist.
    #[error("llvm-cov export: {0}")]
    Export(String),
    /// A run names a test the inventory does not know.
    #[error("a run of {0}, which the inventory does not know")]
    UnknownTest(String),
    /// The package manager's graph output does not read.
    #[error("build graph: {0}")]
    Graph(String),
}
