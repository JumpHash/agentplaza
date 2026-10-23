//! Recording and selecting from per-test llvm-cov exports (fixtures), and the misses a fake full run records.
//!
//! The fixtures are two per-test exports of apz-link's `identity.rs`: one test executes `save`, the other `token`. Their
//! `<ROOT>` is the repository's root at run time, so the paths read as the recording machine would write them.

use apz_testsel::llvm::{TestRun, record};
use apz_testsel::map::{CoverageMap, Package, TestEntry};
use apz_testsel::misses::{missed, per_area};
use apz_testsel::{Change, Touch, select};

const ROOT: &str = "/work/repo";
const SAVE_TEST: &str = "crates/apz-link/src/identity.rs::save_load_round_trip_with_modes";
const TOKEN_TEST: &str =
    "crates/apz-link/src/identity.rs::internal_token_is_created_once_and_stable";

fn export(fixture: &str) -> String {
    fixture.replace("<ROOT>", ROOT)
}

fn fixture_runs() -> Vec<TestRun> {
    vec![
        TestRun {
            test: SAVE_TEST.into(),
            export: export(include_str!("fixtures/identity_save_load.json")),
        },
        TestRun {
            test: TOKEN_TEST.into(),
            export: export(include_str!("fixtures/identity_token.json")),
        },
    ]
}

fn inventory() -> (Vec<Package>, Vec<TestEntry>) {
    let packages = vec![
        Package {
            name: "apz-link".into(),
            dir: "crates/apz-link".into(),
            deps: vec![],
        },
        Package {
            name: "apid".into(),
            dir: "crates/apid".into(),
            deps: vec!["apz-link".into()],
        },
        Package {
            name: "apz-store".into(),
            dir: "crates/apz-store".into(),
            deps: vec![],
        },
    ];
    let entry = |id: &str, package: &str, file: &str, smoke: bool| TestEntry {
        id: id.into(),
        package: package.into(),
        file: file.into(),
        smoke,
    };
    let tests = vec![
        entry(
            SAVE_TEST,
            "apz-link",
            "crates/apz-link/src/identity.rs",
            false,
        ),
        entry(
            TOKEN_TEST,
            "apz-link",
            "crates/apz-link/src/identity.rs",
            false,
        ),
        entry(
            "crates/apz-link/src/join.rs::expired_token_is_refused",
            "apz-link",
            "crates/apz-link/src/join.rs",
            false,
        ),
        entry(
            "crates/apid/tests/api.rs::health",
            "apid",
            "crates/apid/tests/api.rs",
            true,
        ),
        entry(
            "crates/apid/tests/api.rs::sessions",
            "apid",
            "crates/apid/tests/api.rs",
            false,
        ),
        entry(
            "crates/apz-store/tests/store.rs::opens",
            "apz-store",
            "crates/apz-store/tests/store.rs",
            false,
        ),
    ];
    (packages, tests)
}

fn recorded() -> CoverageMap {
    let (packages, tests) = inventory();
    record(
        "agentplaza",
        "c0ffee",
        ROOT,
        "cargo",
        packages,
        tests,
        &fixture_runs(),
        1_700_000_000_000,
    )
    .expect("the fixture exports read")
}

fn change(path: &str, lines: Option<Vec<(u32, u32)>>) -> Change {
    Change {
        files: vec![Touch {
            path: path.into(),
            lines,
        }],
    }
}

#[test]
fn the_map_has_each_definition_with_the_tests_that_executed_it() {
    let map = recorded();
    assert_eq!(map.id, "agentplaza@c0ffee");
    assert_eq!(map.source, "llvm-cov");
    let by_name = |needle: &str| {
        map.definitions
            .iter()
            .find(|d| d.name.contains(needle))
            .unwrap_or_else(|| panic!("no definition {needle}"))
    };
    // `save` ran in the first test only, and its lines are the function's own (40 to 60).
    let save = by_name("4save");
    assert_eq!(
        (save.file.as_str(), save.start, save.end),
        ("crates/apz-link/src/identity.rs", 40, 60)
    );
    assert_eq!(save.covered_by, vec![SAVE_TEST.to_string()]);
    // `token` ran in the second test only.
    assert_eq!(by_name("5token").covered_by, vec![TOKEN_TEST.to_string()]);
    // `none` has no regions, so it is named by no file and is left out.
    assert!(map.definitions.iter().all(|d| !d.name.contains("4none")));
    // `expand` has its first region in identity.rs; its region in macros.rs does not make it macros.rs's.
    let expand = by_name("7expand");
    assert_eq!(expand.file, "crates/apz-link/src/identity.rs");
    assert_eq!(expand.covered_by, vec![SAVE_TEST.to_string()]);
    assert_eq!(
        map.files,
        vec!["crates/apz-link/src/identity.rs".to_string()]
    );
}

#[test]
fn a_change_to_a_covered_definition_selects_its_tests_and_leaves_the_others_out() {
    let map = recorded();
    // Lines 45 to 46 are inside `save`.
    let selection = select(
        &change("crates/apz-link/src/identity.rs", Some(vec![(45, 46)])),
        &map,
    );
    assert!(!selection.full, "{:?}", selection.full_because);
    let chosen: Vec<&str> = selection.chosen.iter().map(|c| c.test.as_str()).collect();
    assert!(chosen.contains(&SAVE_TEST));
    // The package is apz-link: its own tests come with the build graph, and apid (its dependent) with them.
    assert!(chosen.contains(&"crates/apid/tests/api.rs::sessions"));
    assert!(
        chosen.contains(&"crates/apid/tests/api.rs::health"),
        "the smoke set"
    );
    assert!(!chosen.contains(&"crates/apz-store/tests/store.rs::opens"));
}

#[test]
fn a_cross_package_test_is_selected_by_the_map_alone() {
    // A test in apz-store that executed a definition of apz-link: the build graph does not connect them, the map does.
    let (packages, mut tests) = inventory();
    tests.push(TestEntry {
        id: "crates/apz-store/tests/store.rs::uses_link".into(),
        package: "apz-store".into(),
        file: "crates/apz-store/tests/store.rs".into(),
        smoke: false,
    });
    let mut runs = fixture_runs();
    runs.push(TestRun {
        test: "crates/apz-store/tests/store.rs::uses_link".into(),
        export: export(include_str!("fixtures/identity_save_load.json")),
    });
    let map = record(
        "agentplaza",
        "c0ffee",
        ROOT,
        "cargo",
        packages,
        tests,
        &runs,
        0,
    )
    .unwrap();
    let selection = select(
        &change("crates/apz-link/src/identity.rs", Some(vec![(45, 46)])),
        &map,
    );
    let why = selection
        .chosen
        .iter()
        .find(|c| c.test == "crates/apz-store/tests/store.rs::uses_link")
        .map(|c| c.why);
    assert_eq!(why, Some(apz_testsel::Why::Covers));
}

#[test]
fn an_export_that_is_not_llvm_cov_json_is_refused() {
    let (packages, tests) = inventory();
    let runs = vec![TestRun {
        test: SAVE_TEST.into(),
        export: "{\"not\": \"an export\"}".into(),
    }];
    let err = record(
        "agentplaza",
        "c0ffee",
        ROOT,
        "cargo",
        packages,
        tests,
        &runs,
        0,
    )
    .unwrap_err();
    assert!(matches!(err, apz_testsel::Error::Export(_)), "{err:?}");
}

#[test]
fn a_run_of_a_test_the_inventory_does_not_know_is_refused() {
    let (packages, tests) = inventory();
    let runs = vec![TestRun {
        test: "crates/gone.rs::renamed".into(),
        export: export(include_str!("fixtures/identity_token.json")),
    }];
    let err = record(
        "agentplaza",
        "c0ffee",
        ROOT,
        "cargo",
        packages,
        tests,
        &runs,
        0,
    )
    .unwrap_err();
    assert_eq!(
        err,
        apz_testsel::Error::UnknownTest("crates/gone.rs::renamed".into())
    );
}

#[test]
fn a_fake_full_run_that_fails_a_test_the_selection_skipped_records_a_miss() {
    let map = recorded();
    // A change in apz-store: its selection is that package's own test and the smoke set, not apid's other tests.
    let selection = select(&change("crates/apz-store/tests/store.rs", None), &map);
    assert!(!selection.full);
    let skipped_before = selection
        .chosen
        .iter()
        .all(|c| c.test != "crates/apid/tests/api.rs::sessions");
    assert!(skipped_before);
    // The full run fails `sessions` (skipped) and `opens` (selected): only the skipped one is a miss.
    let failed = vec![
        "crates/apid/tests/api.rs::sessions".to_string(),
        "crates/apz-store/tests/store.rs::opens".to_string(),
    ];
    let misses = missed(&selection, &failed);
    assert_eq!(
        misses,
        vec!["crates/apid/tests/api.rs::sessions".to_string()]
    );
    // Recorded per area: apid has the miss, and its tests in the map are counted.
    let recorded_misses: Vec<apz_testsel::Miss> = misses
        .iter()
        .map(|test| apz_testsel::Miss {
            id: apz_testsel::Miss::key("agentplaza", "c0ffee", test),
            project: "agentplaza".into(),
            commit: "c0ffee".into(),
            test: test.clone(),
            package: map
                .tests
                .iter()
                .find(|t| &t.id == test)
                .unwrap()
                .package
                .clone(),
            selection: "change-1".into(),
            recorded_ms: 0,
        })
        .collect();
    let areas = per_area(&map, &recorded_misses);
    let apid = areas
        .iter()
        .find(|a| a.package == "apid")
        .expect("apid is an area");
    assert_eq!((apid.tests, apid.misses), (2, 1));
}

#[test]
fn a_full_selection_misses_nothing() {
    let map = recorded();
    let selection = select(&change("Cargo.lock", None), &map);
    assert!(selection.full);
    assert!(
        missed(
            &selection,
            &["crates/apid/tests/api.rs::sessions".to_string()]
        )
        .is_empty()
    );
}
