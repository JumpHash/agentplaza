//! The coverage map (PLAN 15.4): what each test covers, recorded on `main` for one commit, with the build graph and the
//! test inventory the selection reads. The hub keeps one map per project and commit as typed tables: the map is the root
//! row, and its packages, tests, definitions (with their covering tests) and files are child tables.
//!
//! Vocabulary: a *definition* is a function as the coverage tool numbers it (its file, its lines); a *test* is a test
//! function (its id is `<file>::<name>`); a *package* is a crate, a Go package or an npm workspace, by the build graph.

use serde::{Deserialize, Serialize};

/// The map's format. A stored map of another format is refused rather than read.
pub const FORMAT: u32 = 1;

/// The map has definitions from llvm-cov's per-test regions (Rust with coverage on).
pub const SOURCE_LLVM_COV: &str = "llvm-cov";
/// The map has the build graph and the inventory only: a project without coverage, selected by the build graph.
pub const SOURCE_BUILD_GRAPH: &str = "build-graph";

/// The runners a map names: a test command is built for one of them (see [`crate::select`]).
pub const RUNNER_CARGO: &str = "cargo";
pub const RUNNER_GO: &str = "go";
pub const RUNNER_NPM: &str = "npm";

/// One recorded map: the root row of the hub's `test_map` table, keyed by its project and commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "test_map")]
pub struct CoverageMap {
    /// `<project>@<commit>`.
    #[table(key)]
    pub id: String,
    pub format: u32,
    pub project: String,
    /// The commit on `main` the map was recorded at.
    pub commit: String,
    /// [`SOURCE_LLVM_COV`] or [`SOURCE_BUILD_GRAPH`].
    pub source: String,
    /// [`RUNNER_CARGO`], [`RUNNER_GO`] or [`RUNNER_NPM`].
    pub runner: String,
    /// When it was recorded, in milliseconds since the epoch.
    pub recorded_ms: u64,
    /// The build graph: the packages of the workspace and the workspace packages each depends on.
    #[table(child = "test_map_package")]
    pub packages: Vec<Package>,
    /// Every test of the inventory, with its package, its file and whether it is in the smoke set.
    #[table(child = "test_map_test")]
    pub tests: Vec<TestEntry>,
    /// The definitions the coverage run saw, with the tests that executed each (empty without llvm-cov).
    #[table(child = "test_map_definition")]
    pub definitions: Vec<Definition>,
    /// The source files the definitions are in: a changed file outside this list is unmapped code.
    #[table(list = "test_map_file")]
    pub files: Vec<String>,
}

impl CoverageMap {
    /// The key a map is stored under: one map per project and commit.
    pub fn key(project: &str, commit: &str) -> String {
        format!("{project}@{commit}")
    }
}

/// One package of the build graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Package {
    /// The name the runner selects it by (`cargo test -p <name>`, `go test <name>`, `npm --workspace <name>`).
    pub name: String,
    /// Its directory from the repository's root, `/`-separated (`""` for the root).
    pub dir: String,
    /// The workspace packages it depends on, by name.
    #[table(list = "test_map_dep")]
    pub deps: Vec<String>,
}

/// One test (a row of the inventory).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct TestEntry {
    /// `<file>::<name>`, with the file from the repository's root.
    pub id: String,
    /// The package it is in ("" when no package owns its file).
    pub package: String,
    /// Its file from the repository's root.
    pub file: String,
    /// Whether it is in the project's smoke set (always selected).
    pub smoke: bool,
}

/// One definition a coverage run saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Definition {
    /// Its file from the repository's root.
    pub file: String,
    /// The function's name as the coverage tool gives it.
    pub name: String,
    /// Its first and last line in the file.
    pub start: u32,
    pub end: u32,
    /// The ids of the tests that executed it (empty: no test does).
    #[table(list = "test_map_cover")]
    pub covered_by: Vec<String>,
}

/// A test the selection skipped that failed in a full run (PLAN 15.4: a miss). Kept per project and commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "test_miss")]
pub struct Miss {
    /// `<project>@<commit>/<test id>`.
    #[table(key)]
    pub id: String,
    pub project: String,
    /// The commit the full run ran at.
    pub commit: String,
    /// The test that failed.
    pub test: String,
    /// Its package ("" when none owns it).
    pub package: String,
    /// What the selection was for (the change's label, or its commit).
    pub selection: String,
    pub recorded_ms: u64,
}

impl Miss {
    /// The key a miss is stored under.
    pub fn key(project: &str, commit: &str, test: &str) -> String {
        format!("{project}@{commit}/{test}")
    }
}
