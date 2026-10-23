//! The selection (PLAN 15.4): the tests a change needs, from the coverage map recorded on `main`.
//!
//! For a change the selected tests are:
//! 1. the tests that cover the definitions the change touches (the map's definitions in each touched file, narrowed to
//!    the changed lines when the change gives them; the whole file otherwise, and when the lines fall in no definition);
//! 2. the tests in the packages the change touches, and in their dependents in the build graph;
//! 3. the tests whose own files changed;
//! 4. the smoke set the map names.
//!
//! The full suite instead when the change touches a build file, a CI file or the toolchain, or code the map does not
//! cover (a code file in no package, or, with coverage, a code file no definition is in and no test is).

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::graph::{package_of, with_dependents};
use crate::map::{CoverageMap, RUNNER_GO, RUNNER_NPM, SOURCE_LLVM_COV};

/// One file a change touches, with the lines it changes (`None`: the whole file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Touch {
    pub path: String,
    /// Inclusive line ranges in the file as it is after the change.
    pub lines: Option<Vec<(u32, u32)>>,
}

/// What a change touches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    pub files: Vec<Touch>,
}

/// Why a test is selected. The first reason that applies is the one kept, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    ChangedFile,
    Covers,
    Package,
    Smoke,
    /// The full suite ran: every test.
    Full,
}

/// One selected test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Chosen {
    pub test: String,
    pub package: String,
    pub why: Why,
}

/// The selection for a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Selection {
    /// Whether the full suite runs instead.
    pub full: bool,
    /// Why the full suite runs (empty when it does not).
    pub full_because: Vec<String>,
    /// The code files the map does not cover (they make the selection full).
    pub unmapped: Vec<String>,
    /// The selected tests, sorted by id.
    pub chosen: Vec<Chosen>,
    /// Every test the map knows.
    pub total: usize,
    /// The packages the selected tests are in, sorted.
    pub packages: Vec<String>,
    /// The command that runs the selection (the runner's; empty when nothing is selected).
    pub command: String,
}

impl Selection {
    /// The selected tests over every test: 0 when the map knows no test.
    pub fn fraction(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.chosen.len() as f64 / self.total as f64
    }
}

/// Files that change how every test runs: a change to one of them runs the full suite.
const BUILD_FILES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "rust-toolchain",
    "rust-toolchain.toml",
    "go.mod",
    "go.sum",
    "package.json",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "pnpm-workspace.yaml",
    "Makefile",
    "deny.toml",
];

/// Directories and files of CI and of the toolchain's configuration: a change to one runs the full suite.
const CI_PREFIXES: &[&str] = &[".github/", ".circleci/", ".cargo/", ".config/", ".gitlab/"];
const CI_FILES: &[&str] = &[".gitlab-ci.yml", "azure-pipelines.yml"];

/// Extensions of code: a changed file with one of these must be covered by the map (or it is unmapped).
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "go", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "c", "cc", "cpp", "h", "hpp",
    "swift", "kt", "java", "cs", "zig",
];

/// Why a path is a build, CI or toolchain file, or `None` when it is not.
pub fn full_suite_reason(path: &str) -> Option<String> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if BUILD_FILES.contains(&name) {
        return Some(format!("build file {path}"));
    }
    if CI_PREFIXES.iter().any(|p| path.starts_with(p)) || CI_FILES.contains(&name) {
        return Some(format!("CI file {path}"));
    }
    None
}

fn is_code(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| CODE_EXTENSIONS.contains(&ext))
}

/// The command that runs every test of the runner.
fn full_command(runner: &str) -> &'static str {
    match runner {
        RUNNER_GO => "go test ./...",
        RUNNER_NPM => "npm test --workspaces",
        _ => "cargo test --workspace",
    }
}

/// Selects the tests of `change` from `map`.
pub fn select(change: &Change, map: &CoverageMap) -> Selection {
    let total = map.tests.len();
    let mut full_because = Vec::new();
    let mut unmapped = Vec::new();
    let test_files: BTreeSet<&str> = map.tests.iter().map(|t| t.file.as_str()).collect();
    let files: BTreeSet<&str> = map.files.iter().map(String::as_str).collect();
    for touch in &change.files {
        let path = touch.path.as_str();
        if let Some(reason) = full_suite_reason(path) {
            full_because.push(reason);
            continue;
        }
        if !is_code(path) || test_files.contains(path) {
            continue;
        }
        let in_package = package_of(&map.packages, path).is_some();
        let covered = map.source == SOURCE_LLVM_COV && files.contains(path);
        let unknown = if map.source == SOURCE_LLVM_COV {
            !covered
        } else {
            !in_package
        };
        if unknown {
            unmapped.push(path.to_string());
        }
    }
    for path in &unmapped {
        full_because.push(format!("unmapped code {path}"));
    }
    if map.tests.is_empty() && full_because.is_empty() {
        full_because.push("no tests are recorded for the project".to_string());
    }
    if !full_because.is_empty() {
        let chosen: Vec<Chosen> = map
            .tests
            .iter()
            .map(|t| Chosen {
                test: t.id.clone(),
                package: t.package.clone(),
                why: Why::Full,
            })
            .collect();
        let packages = map.packages.iter().map(|p| p.name.clone()).collect();
        return Selection {
            full: true,
            full_because,
            unmapped,
            chosen,
            total,
            packages,
            command: full_command(&map.runner).to_string(),
        };
    }

    let mut picked: BTreeMap<String, Why> = BTreeMap::new();
    let keep = |picked: &mut BTreeMap<String, Why>, id: &str, why: Why| {
        let slot = picked.entry(id.to_string()).or_insert(why);
        if why < *slot {
            *slot = why;
        }
    };

    // 3. the tests whose own files changed.
    for touch in &change.files {
        for test in map.tests.iter().filter(|t| t.file == touch.path) {
            keep(&mut picked, &test.id, Why::ChangedFile);
        }
    }

    // 1. the tests that cover the touched definitions.
    for touch in &change.files {
        if !files.contains(touch.path.as_str()) {
            continue;
        }
        let in_file: Vec<_> = map
            .definitions
            .iter()
            .filter(|d| d.file == touch.path)
            .collect();
        let hit: Vec<_> = match &touch.lines {
            None => in_file.clone(),
            Some(ranges) => {
                let narrowed: Vec<_> = in_file
                    .iter()
                    .copied()
                    .filter(|d| ranges.iter().any(|&(a, b)| d.start <= b && a <= d.end))
                    .collect();
                // Changed lines in no definition (a constant, an import): the whole file is the conservative answer.
                if narrowed.is_empty() {
                    in_file.clone()
                } else {
                    narrowed
                }
            }
        };
        for definition in hit {
            for test in &definition.covered_by {
                keep(&mut picked, test, Why::Covers);
            }
        }
    }

    // 2. the packages the change touches, and their dependents in the build graph.
    let touched: BTreeSet<String> = change
        .files
        .iter()
        .filter_map(|t| package_of(&map.packages, &t.path).map(|p| p.name.clone()))
        .collect();
    let closure = with_dependents(&map.packages, &touched);
    for test in &map.tests {
        if closure.contains(&test.package) {
            keep(&mut picked, &test.id, Why::Package);
        }
    }

    // 4. the smoke set.
    for test in map.tests.iter().filter(|t| t.smoke) {
        keep(&mut picked, &test.id, Why::Smoke);
    }

    let by_id: BTreeMap<&str, &crate::map::TestEntry> =
        map.tests.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut chosen: Vec<Chosen> = picked
        .into_iter()
        .filter_map(|(id, why)| {
            by_id.get(id.as_str()).map(|t| Chosen {
                test: t.id.clone(),
                package: t.package.clone(),
                why,
            })
        })
        .collect();
    chosen.sort_by(|a, b| a.test.cmp(&b.test));
    let packages: BTreeSet<String> = chosen.iter().map(|c| c.package.clone()).collect();
    let command = command_for(&map.runner, map, &chosen);
    Selection {
        full: false,
        full_because: Vec::new(),
        unmapped,
        chosen,
        total,
        packages: packages.into_iter().collect(),
        command,
    }
}

/// The command that runs the chosen tests. Cargo runs a whole package with `cargo test -p`, and only the chosen tests
/// of a package that is partly chosen (`cargo test -p <name> -- <test>`, one per test, as substring filters). Go and npm
/// run whole packages and workspaces.
fn command_for(runner: &str, map: &CoverageMap, chosen: &[Chosen]) -> String {
    if chosen.is_empty() {
        return String::new();
    }
    let packages: BTreeSet<&str> = chosen.iter().map(|c| c.package.as_str()).collect();
    let mut parts = Vec::new();
    for package in packages {
        if package.is_empty() {
            continue;
        }
        match runner {
            RUNNER_GO => parts.push(format!("go test {package}")),
            RUNNER_NPM => parts.push(format!("npm test --workspace={package}")),
            _ => {
                let all: usize = map.tests.iter().filter(|t| t.package == package).count();
                let mine: Vec<&Chosen> = chosen.iter().filter(|c| c.package == package).collect();
                if mine.len() == all {
                    parts.push(format!("cargo test -p {package}"));
                } else {
                    for c in mine {
                        let name = c.test.rsplit("::").next().unwrap_or(&c.test);
                        parts.push(format!("cargo test -p {package} -- {name}"));
                    }
                }
            }
        }
    }
    parts.join(" && ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{Definition, Package, RUNNER_CARGO, TestEntry};

    fn test(id: &str, package: &str, file: &str, smoke: bool) -> TestEntry {
        TestEntry {
            id: id.into(),
            package: package.into(),
            file: file.into(),
            smoke,
        }
    }

    fn pkg(name: &str, dir: &str, deps: &[&str]) -> Package {
        Package {
            name: name.into(),
            dir: dir.into(),
            deps: deps.iter().map(|d| d.to_string()).collect(),
        }
    }

    fn touch(path: &str, lines: Option<Vec<(u32, u32)>>) -> Change {
        Change {
            files: vec![Touch {
                path: path.into(),
                lines,
            }],
        }
    }

    /// leaf <- mid <- top (dependents), and `other` depends on nothing. `other`'s cross test covers leaf's `fb`.
    fn map(source: &str) -> CoverageMap {
        CoverageMap {
            id: "p@c".into(),
            format: crate::map::FORMAT,
            project: "p".into(),
            commit: "c".into(),
            source: source.into(),
            runner: RUNNER_CARGO.into(),
            recorded_ms: 0,
            packages: vec![
                pkg("leaf", "crates/leaf", &[]),
                pkg("mid", "crates/mid", &["leaf"]),
                pkg("top", "crates/top", &["mid"]),
                pkg("other", "crates/other", &[]),
            ],
            tests: vec![
                test(
                    "crates/leaf/tests/leaf.rs::leaf_a",
                    "leaf",
                    "crates/leaf/tests/leaf.rs",
                    false,
                ),
                test(
                    "crates/leaf/tests/leaf.rs::leaf_b",
                    "leaf",
                    "crates/leaf/tests/leaf.rs",
                    false,
                ),
                test(
                    "crates/mid/tests/mid.rs::mid_a",
                    "mid",
                    "crates/mid/tests/mid.rs",
                    false,
                ),
                test(
                    "crates/top/tests/top.rs::top_a",
                    "top",
                    "crates/top/tests/top.rs",
                    false,
                ),
                test(
                    "crates/other/tests/t.rs::other_a",
                    "other",
                    "crates/other/tests/t.rs",
                    false,
                ),
                test(
                    "crates/other/tests/t.rs::smoke",
                    "other",
                    "crates/other/tests/t.rs",
                    true,
                ),
                test(
                    "crates/other/tests/cross.rs::covers_fb",
                    "other",
                    "crates/other/tests/cross.rs",
                    false,
                ),
            ],
            definitions: vec![
                Definition {
                    file: "crates/leaf/src/lib.rs".into(),
                    name: "fa".into(),
                    start: 1,
                    end: 10,
                    covered_by: vec!["crates/leaf/tests/leaf.rs::leaf_a".into()],
                },
                Definition {
                    file: "crates/leaf/src/lib.rs".into(),
                    name: "fb".into(),
                    start: 12,
                    end: 20,
                    covered_by: vec![
                        "crates/leaf/tests/leaf.rs::leaf_b".into(),
                        "crates/other/tests/cross.rs::covers_fb".into(),
                    ],
                },
            ],
            files: vec![
                "crates/leaf/src/lib.rs".into(),
                "crates/mid/src/lib.rs".into(),
            ],
        }
    }

    fn ids(s: &Selection) -> Vec<&str> {
        s.chosen.iter().map(|c| c.test.as_str()).collect()
    }

    fn why_of(s: &Selection, test: &str) -> Option<Why> {
        s.chosen.iter().find(|c| c.test == test).map(|c| c.why)
    }

    #[test]
    fn a_touched_definition_selects_its_covering_test_and_the_dependents_packages() {
        let m = map("llvm-cov");
        let s = select(&touch("crates/leaf/src/lib.rs", Some(vec![(3, 4)])), &m);
        assert!(!s.full, "{:?}", s.full_because);
        assert_eq!(
            why_of(&s, "crates/leaf/tests/leaf.rs::leaf_a"),
            Some(Why::Covers)
        );
        // The build graph: leaf's own tests, and its dependents' (mid, top).
        assert!(ids(&s).contains(&"crates/leaf/tests/leaf.rs::leaf_b"));
        assert!(ids(&s).contains(&"crates/mid/tests/mid.rs::mid_a"));
        assert!(ids(&s).contains(&"crates/top/tests/top.rs::top_a"));
        // fa is not fb: the cross-package test that covers fb is not selected by this change.
        assert!(!ids(&s).contains(&"crates/other/tests/cross.rs::covers_fb"));
        assert!(!ids(&s).contains(&"crates/other/tests/t.rs::other_a"));
    }

    #[test]
    fn a_cross_package_test_that_covers_a_touched_definition_is_selected_by_the_map() {
        let m = map("llvm-cov");
        let s = select(&touch("crates/leaf/src/lib.rs", Some(vec![(13, 14)])), &m);
        // `other` is not a dependent of leaf: only the map brings its test in.
        assert_eq!(
            why_of(&s, "crates/other/tests/cross.rs::covers_fb"),
            Some(Why::Covers)
        );
        assert!(!ids(&s).contains(&"crates/other/tests/t.rs::other_a"));
    }

    #[test]
    fn a_change_in_a_package_with_no_dependents_selects_only_its_own_tests_and_the_smoke_set() {
        let m = map("build-graph");
        let s = select(&touch("crates/other/src/x.rs", None), &m);
        assert!(!s.full, "{:?}", s.full_because);
        assert_eq!(
            ids(&s),
            vec![
                "crates/other/tests/cross.rs::covers_fb",
                "crates/other/tests/t.rs::other_a",
                "crates/other/tests/t.rs::smoke",
            ]
        );
        assert_eq!(s.packages, vec!["other".to_string()]);
    }

    #[test]
    fn a_changed_test_file_selects_its_own_tests() {
        let m = map("llvm-cov");
        let s = select(&touch("crates/top/tests/top.rs", None), &m);
        assert_eq!(
            why_of(&s, "crates/top/tests/top.rs::top_a"),
            Some(Why::ChangedFile)
        );
    }

    #[test]
    fn changed_lines_in_no_definition_fall_back_to_the_whole_file() {
        let m = map("llvm-cov");
        // Line 11 sits between fa and fb: both definitions' tests are the conservative answer.
        let s = select(&touch("crates/leaf/src/lib.rs", Some(vec![(11, 11)])), &m);
        assert!(ids(&s).contains(&"crates/leaf/tests/leaf.rs::leaf_a"));
        assert!(ids(&s).contains(&"crates/leaf/tests/leaf.rs::leaf_b"));
        assert!(ids(&s).contains(&"crates/other/tests/cross.rs::covers_fb"));
    }

    #[test]
    fn build_ci_and_toolchain_files_run_the_full_suite() {
        let m = map("llvm-cov");
        for path in [
            "Cargo.lock",
            "crates/leaf/Cargo.toml",
            ".github/workflows/ci.yml",
            "rust-toolchain.toml",
        ] {
            let s = select(&touch(path, None), &m);
            assert!(s.full, "{path} must run the full suite");
            assert_eq!(s.chosen.len(), m.tests.len());
            assert_eq!(s.command, "cargo test --workspace");
        }
    }

    #[test]
    fn unmapped_code_runs_the_full_suite_and_is_named() {
        let m = map("llvm-cov");
        // A new code file that no definition is in.
        let s = select(&touch("crates/other/src/new.rs", None), &m);
        assert!(s.full);
        assert_eq!(s.unmapped, vec!["crates/other/src/new.rs".to_string()]);
    }

    #[test]
    fn a_non_code_file_selects_nothing_but_the_smoke_set() {
        let m = map("llvm-cov");
        let s = select(&touch("docs/notes.md", None), &m);
        assert!(!s.full);
        assert_eq!(ids(&s), vec!["crates/other/tests/t.rs::smoke"]);
    }

    #[test]
    fn packages_are_run_whole_and_a_partly_chosen_one_test_by_test() {
        let m = map("llvm-cov");
        let s = select(&touch("crates/leaf/src/lib.rs", Some(vec![(13, 14)])), &m);
        assert_eq!(
            s.command,
            "cargo test -p leaf && cargo test -p mid && cargo test -p other -- covers_fb && cargo test -p other -- smoke && cargo test -p top"
        );
    }

    #[test]
    fn a_map_with_no_tests_is_not_trusted() {
        let mut m = map("build-graph");
        m.tests.clear();
        let s = select(&touch("crates/other/src/x.rs", None), &m);
        assert!(s.full);
        assert_eq!(
            s.full_because,
            vec!["no tests are recorded for the project".to_string()]
        );
    }
}
