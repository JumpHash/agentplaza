//! Selection on this repository: its own `cargo metadata` (the build graph) and its own test inventory, with a fixture
//! coverage map for one of its crates. A one-file change in a leaf crate selects well under a fifth of the repository's
//! tests, by the build graph alone and with the recorded map.
//!
//! The real llvm-cov recording (one run per test, merged profiles) is a CI measurement and is not run here: it needs
//! `cargo llvm-cov`, which this test does not install.

use std::path::{Path, PathBuf};
use std::process::Command;

use apz_testsel::graph::cargo_packages;
use apz_testsel::inventory::scan_rust;
use apz_testsel::llvm::{TestRun, record};
use apz_testsel::map::CoverageMap;
use apz_testsel::{Change, Touch, select};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root is two levels above the crate")
}

fn cargo_metadata(root: &Path) -> String {
    let out = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--offline",
        ])
        .current_dir(root)
        .output()
        .expect("cargo runs");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("metadata is UTF-8")
}

fn fixture_map(
    root: &str,
    packages: Vec<apz_testsel::Package>,
    tests: Vec<apz_testsel::TestEntry>,
) -> CoverageMap {
    let runs = [
        (
            "crates/dispatchd/src/http.rs::a_real_request_and_answer_round_trip",
            include_str!("fixtures/http_round_trip.json"),
        ),
        (
            "crates/dispatchd/src/http.rs::a_target_is_the_loopback_and_only_the_loopback",
            include_str!("fixtures/http_loopback.json"),
        ),
    ]
    .map(|(test, export)| TestRun {
        test: test.to_string(),
        // The root goes into a JSON string: its backslashes (Windows) are escaped.
        export: export.replace("<ROOT>", &root.replace('\\', "\\\\")),
    });
    record(
        "agentplaza",
        "repo-head",
        root,
        "cargo",
        packages,
        tests,
        &runs,
        0,
    )
    .expect("the fixture exports read")
}

#[test]
fn a_one_file_change_in_a_leaf_crate_selects_under_a_fifth_of_the_repository_by_the_build_graph_and_the_map()
 {
    let root = repo_root();
    let root_text = root.to_string_lossy().to_string();

    let packages =
        cargo_packages(&cargo_metadata(&root), &root_text).expect("cargo metadata reads");
    let tests = scan_rust(&root, &packages).expect("the repository's sources scan");
    let total = tests.len();
    assert!(
        total >= 1000,
        "the repository has its tests in its sources (found {total})"
    );
    assert!(
        tests.iter().any(|t| t.id.starts_with("crates/dispatchd/")),
        "dispatchd's tests are in the inventory"
    );

    let change = Change {
        files: vec![Touch {
            path: "crates/dispatchd/src/http.rs".into(),
            lines: Some(vec![(45, 46)]),
        }],
    };

    // The build graph alone (a project without coverage).
    let graph_only = CoverageMap {
        id: CoverageMap::key("agentplaza", "repo-head"),
        format: apz_testsel::map::FORMAT,
        project: "agentplaza".into(),
        commit: "repo-head".into(),
        source: "build-graph".into(),
        runner: "cargo".into(),
        recorded_ms: 0,
        packages: packages.clone(),
        tests: tests.clone(),
        definitions: vec![],
        files: vec![],
    };
    let by_graph = select(&change, &graph_only);
    assert!(!by_graph.full, "{:?}", by_graph.full_because);
    assert!(
        by_graph.fraction() < 0.2,
        "build graph alone selected {} of {total}",
        by_graph.chosen.len()
    );

    // The build graph and the recorded map (the fixture's definitions and covering tests).
    let map = fixture_map(&root_text, packages, tests);
    let selection = select(&change, &map);
    assert!(!selection.full, "{:?}", selection.full_because);
    assert!(
        selection.fraction() < 0.2,
        "the map selected {} of {total}",
        selection.chosen.len()
    );
    assert!(
        selection
            .chosen
            .iter()
            .any(|c| c.test == "crates/dispatchd/src/http.rs::a_real_request_and_answer_round_trip")
    );
    assert_eq!(selection.packages, vec!["dispatchd".to_string()]);
    assert_eq!(selection.command, "cargo test -p dispatchd");
}

#[test]
fn a_build_file_in_this_repository_runs_every_test() {
    let root = repo_root();
    let root_text = root.to_string_lossy().to_string();
    let packages =
        cargo_packages(&cargo_metadata(&root), &root_text).expect("cargo metadata reads");
    let tests = scan_rust(&root, &packages).expect("the repository's sources scan");
    let map = fixture_map(&root_text, packages, tests.clone());
    let selection = select(
        &Change {
            files: vec![Touch {
                path: "Cargo.lock".into(),
                lines: None,
            }],
        },
        &map,
    );
    assert!(selection.full);
    assert_eq!(selection.chosen.len(), tests.len());
}
