//! The scheduled full suite on `main` (PLAN 15.4, 21.3): every test of the project's head, run in a test fork, and the
//! coverage map and the misses recorded from it. The map is what `tests_select` and the CI recipe's `tests` step read;
//! a miss is a test the selection for the head's landing skipped that the full run failed.
//!
//! The schedule is a trigger (`triggers/mod.rs`): [`sync`] keeps each project's full-suite schedule at its interval
//! (`full_suite_minutes` in its settings, a day by default, 0 to turn it off) after its newest run or map, and the
//! scheduler's pass fires it through [`start`]. A project with no map runs at once; a run that failed (a project
//! without a declaration, a fork that did not start) waits out the interval before it is tried again.
//!
//! What the fork runs, in order (all in the Rust toolchain's commands, through `base_vm::Fork`):
//! 1. `cargo metadata`: the build graph (`apz_testsel::graph`);
//! 2. the test inventory: each `#[test]` function, with its file, found by a text scan (`INVENTORY`);
//! 3. one llvm-cov run per test (`cargo llvm-cov --json`), whose export says which functions that test executed
//!    (`apz_testsel::llvm::record`). The filter is the test's function name, so a test whose name is a part of another
//!    one's runs both: the map then says more tests cover a definition than do, which only selects more;
//! 4. the full suite, `cargo test --workspace --no-fail-fast`, whose failing tests are the misses' candidates.
//!
//! The recording itself is CI's measurement: `cargo llvm-cov` is not on the hub's machines, so the tests feed the same
//! steps with recorded fixtures (`record` takes the fork's commands as a closure).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use apz_testsel::map::{CoverageMap, Miss, Package, RUNNER_CARGO, TestEntry};
use apz_testsel::{Change, graph, llvm, select};

use crate::base_recipe::{self, Command, WORKSPACE};
use crate::base_vm::{Answer, Fork, Live};
use crate::checks;
use crate::placement;
use crate::project_settings;
use crate::project_stack::{self, Toolchain};
use crate::state::App;
use crate::test_select;

/// The interval between full runs when a project sets none: a day.
pub const DEFAULT_MINUTES: u32 = 24 * 60;

/// The test inventory, read in the fork: every `#[test]` (or `#[tokio::test]`) function's file and name, one per line as
/// `file<TAB>name`. A test attribute marks the next function that follows it.
const INVENTORY: &str = "find . -name '*.rs' -not -path '*/target/*' -exec awk 'FNR==1{armed=0} /#\\[(tokio::)?test/{armed=1; next} armed && /fn [A-Za-z0-9_]+/{match($0,/fn [A-Za-z0-9_]+/); print FILENAME \"\\t\" substr($0,RSTART+3,RLENGTH-3); armed=0}' {} +";

/// One test's coverage run: the filter is the test's name; the export goes to a file and is printed, so the hub reads it
/// from the command's output. A test whose run wrote no export has no coverage.
const COVERAGE: &str =
    "cargo llvm-cov --json --output-path \"$1\" -- \"$2\" >/dev/null 2>&1; cat \"$1\"";
const EXPORT_PATH: &str = "/tmp/apz-llvm-cov.json";

/// What one full run records: the map, and the misses the head's landing's selection had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub map: CoverageMap,
    pub misses: Vec<Miss>,
}

/// Runs the full suite's recording through `exec` (the fork's commands): the build graph, the inventory, one coverage
/// run per test, and the full run's failures. `previous` is the map the selection for the head's landing reads (none
/// before the project's first map: then nothing was skipped and nothing is a miss). `head` is what the head landing
/// changed.
pub fn record(
    exec: &mut dyn FnMut(&Command) -> Result<Answer, String>,
    project: &str,
    commit: &str,
    previous: Option<&CoverageMap>,
    head: Option<&Change>,
    now: u64,
) -> Result<Recorded, String> {
    let metadata = must(
        exec,
        "metadata",
        &["cargo", "metadata", "--no-deps", "--format-version", "1"],
    )?;
    let packages = graph::cargo_packages(&metadata, WORKSPACE).map_err(|e| e.to_string())?;
    let inventory = must(exec, "inventory", &["sh", "-c", INVENTORY])?;
    let tests = inventory_of(&inventory, &packages);

    let mut runs = Vec::new();
    for test in &tests {
        let name = test.id.rsplit("::").next().unwrap_or(&test.id).to_string();
        let command = Command {
            label: "coverage".into(),
            argv: ["sh", "-c", COVERAGE, "sh", EXPORT_PATH, name.as_str()]
                .iter()
                .map(|part| part.to_string())
                .collect(),
            cwd: WORKSPACE,
            timeout_s: base_recipe::STEP_TIMEOUT_S,
            env: Vec::new(),
            listen: None,
        };
        if let Ok(answer) = exec(&command)
            && answer.ok
        {
            runs.push(llvm::TestRun {
                test: test.id.clone(),
                export: answer.output,
            });
        }
    }

    let full = must(
        exec,
        "full run",
        &["cargo", "test", "--workspace", "--no-fail-fast"],
    )?;
    let failed = failed_tests(&full, &tests);

    let map = llvm::record(
        project,
        commit,
        WORKSPACE,
        RUNNER_CARGO,
        packages,
        tests,
        &runs,
        now,
    )
    .map_err(|e| e.to_string())?;
    let misses = match (previous, head) {
        (Some(previous), Some(head)) => {
            let selection = select(head, previous);
            let label = format!("landing {}", commit.chars().take(12).collect::<String>());
            test_select::misses_of(&map, &selection, &failed, &label, now)
        }
        _ => Vec::new(),
    };
    Ok(Recorded { map, misses })
}

/// A command the fork must answer with success: its output, or the reason it failed.
fn must(
    exec: &mut dyn FnMut(&Command) -> Result<Answer, String>,
    what: &str,
    argv: &[&str],
) -> Result<String, String> {
    let command = Command {
        label: what.to_string(),
        argv: argv.iter().map(|part| part.to_string()).collect(),
        cwd: WORKSPACE,
        timeout_s: base_recipe::STEP_TIMEOUT_S,
        env: Vec::new(),
        listen: None,
    };
    let answer = exec(&command)?;
    if answer.ok {
        return Ok(answer.output);
    }
    let status = match answer.exit_code {
        Some(code) => format!("exit {code}"),
        None => "no exit status".to_string(),
    };
    Err(format!(
        "{what} failed ({status}): {}",
        answer.output.trim_end()
    ))
}

/// The tests of an inventory's output (`file<TAB>name` lines), each with its package. Paths are from the root; the
/// output is sorted by id and each test is listed once.
pub fn inventory_of(output: &str, packages: &[Package]) -> Vec<TestEntry> {
    let mut by_id: BTreeMap<String, TestEntry> = BTreeMap::new();
    for line in output.lines() {
        let Some((file, name)) = line.split_once('\t') else {
            continue;
        };
        let file = file.trim_start_matches("./").to_string();
        let id = format!("{file}::{name}");
        let package = apz_testsel::graph::package_of(packages, &file)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        by_id.entry(id.clone()).or_insert(TestEntry {
            id,
            package,
            file,
            smoke: false,
        });
    }
    by_id.into_values().collect()
}

/// The tests a full run failed, from its output's `test <name> ... FAILED` lines, named by the inventory's ids: a
/// name matches the tests whose function has its last path segment.
pub fn failed_tests(output: &str, tests: &[TestEntry]) -> Vec<String> {
    let mut failed = BTreeSet::new();
    for line in output.lines() {
        let Some(name) = line
            .strip_prefix("test ")
            .and_then(|rest| rest.strip_suffix(" ... FAILED"))
        else {
            continue;
        };
        let last = name.rsplit("::").next().unwrap_or(name);
        for test in tests {
            if test.id.rsplit("::").next() == Some(last) {
                failed.insert(test.id.clone());
            }
        }
    }
    failed.into_iter().collect()
}

/// The runs under way (one per project). The schedule is the trigger's (`triggers::Triggers::keep_full_suite`).
static RUNNING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Keeps every project's full-suite schedule in step with its settings and its newest map (once a pass): a project
/// whose full run is off, paused, archived or without a repository has no schedule that fires.
pub fn sync(app: &App, now: u64) {
    for project in app.projects.list() {
        let minutes = project_settings::full_suite_minutes(app, &project.id);
        let eligible = !project.paused && !project.archived && project.repo.is_some();
        let mapped = app.test_select.latest(&project.id).map(|m| m.recorded_ms);
        app.triggers
            .keep_full_suite(&project.id, minutes, eligible, mapped, now);
    }
}

/// Starts the full run of `project` on its own thread, when none is under way. The trigger that calls it records the
/// attempt, so a run that failed waits out the interval. False when a run of the project is already under way.
pub fn start(app: &Arc<App>, project: &str) -> bool {
    if !lock(&RUNNING).insert(project.to_string()) {
        return false;
    }
    let (app, id) = (Arc::clone(app), project.to_string());
    thread::spawn(move || {
        if let Err(why) = run(&app, &id, apz_plaza::now_ms()) {
            eprintln!("apid: full suite of {id}: {why}");
        }
        lock(&RUNNING).remove(&id);
    });
    true
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One full run of `project` on the head of its main: its checkout in a test fork, the recording, and the map and the
/// misses on the hub. A project without a Rust toolchain records nothing yet: its selection stays the full suite.
pub fn run(app: &App, project: &str, now: u64) -> Result<(), String> {
    let repo = app
        .projects
        .get(project)
        .and_then(|p| p.repo)
        .ok_or_else(|| format!("{project} has no repo yet"))?;
    let detail = app.weft.get(&format!("/repos/{repo}"))?;
    let commit = detail["refs"]["main"]
        .as_str()
        .ok_or_else(|| format!("{repo} has no main"))?
        .to_string();
    let stack = project_stack::read(app, project)?
        .ok_or_else(|| format!("{} is not on main", project_stack::FILE))?;
    if !stack.toolchains.contains_key(&Toolchain::Rust) {
        return Ok(());
    }
    let head = checks::revision_diff(app, &commit)?;
    let head = checks::touches(&head);
    let previous = app.test_select.latest(project);
    let steps = base_recipe::fork_steps(&stack, &repo, &commit);
    let pool = crate::base_builds::pool_keys(&stack)
        .first()
        .cloned()
        .unwrap_or_default();
    let host = Live(app);
    let fork: Result<Fork<'_, Live<'_>>, String> =
        match placement::base_node(app, project, &pool, &base_recipe::volumes(&stack)) {
            Some(node) => Fork::start(&host, node, project.to_string(), &steps),
            None => Err(format!("no node in the pool {pool} has room for a fork")),
        };
    let mut fork = fork?;
    let recorded = {
        let mut exec = |command: &Command| fork.exec(command);
        record(
            &mut exec,
            project,
            &commit,
            previous.as_ref(),
            Some(&head),
            now,
        )
    };
    if let Err(why) = fork.retire() {
        eprintln!("apid: the full suite's fork of {project}: {why}");
    }
    let recorded = recorded?;
    let tests = recorded.map.tests.len();
    app.test_select.record(recorded.map)?;
    eprintln!(
        "apid: recorded the coverage map of {project} at {commit}: {tests} tests, {} misses",
        recorded.misses.len()
    );
    app.test_select.record_misses(recorded.misses);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use apz_testsel::Touch;
    use apz_testsel::map::{FORMAT, SOURCE_BUILD_GRAPH};

    use super::*;

    const METADATA: &str = r#"{"packages":[
        {"name":"leaf","manifest_path":"/workspace/crates/leaf/Cargo.toml","dependencies":[]},
        {"name":"other","manifest_path":"/workspace/crates/other/Cargo.toml","dependencies":[]}
    ]}"#;

    /// Two independent packages with one test each: `one` in leaf (its export covers a definition of leaf's lib) and `two`
    /// in other (no coverage). `other` depends on nothing, so a change to leaf selects `one` alone.
    const INVENTORY_OUTPUT: &str =
        "./crates/leaf/tests/a.rs\tone\n./crates/other/tests/b.rs\ttwo\n";

    const LEAF_EXPORT: &str = r#"{"type":"llvm.coverage.json.export","data":[{"functions":[
        {"name":"leaf::f","count":3,"filenames":["/workspace/crates/leaf/src/lib.rs"],"regions":[[2,1,4,1,3,0,0,0]]}
    ]}]}"#;

    const EMPTY_EXPORT: &str = r#"{"type":"llvm.coverage.json.export","data":[{"functions":[]}]}"#;

    /// A fork for the fixtures: the metadata, the inventory and each test's export by its name, and a full run in which
    /// `two` fails. The commands it was asked are recorded.
    fn fork<'a>(
        asked: &'a RefCell<Vec<Vec<String>>>,
        full_output: &str,
    ) -> impl FnMut(&Command) -> Result<Answer, String> + 'a {
        let full_output = full_output.to_string();
        move |command: &Command| {
            asked.borrow_mut().push(command.argv.clone());
            let argv = &command.argv;
            let answer = |output: &str| Answer {
                ok: true,
                exit_code: Some(0),
                output: output.to_string(),
            };
            if argv[0] == "cargo" && argv[1] == "metadata" {
                return Ok(answer(METADATA));
            }
            if argv[0] == "sh" && argv.len() == 3 {
                return Ok(answer(INVENTORY_OUTPUT));
            }
            if argv[0] == "sh" && argv.len() == 6 {
                return Ok(match argv[5].as_str() {
                    "one" => answer(LEAF_EXPORT),
                    _ => answer(EMPTY_EXPORT),
                });
            }
            if argv[..3] == ["cargo", "test", "--workspace"] {
                return Ok(answer(&full_output));
            }
            Err(format!("unexpected command {argv:?}"))
        }
    }

    const FULL_OUTPUT: &str = "test tests::ok ... ok\ntest tests::two ... FAILED\n";

    fn previous_map() -> CoverageMap {
        CoverageMap {
            id: CoverageMap::key("p", "c0"),
            format: FORMAT,
            project: "p".into(),
            commit: "c0".into(),
            source: SOURCE_BUILD_GRAPH.into(),
            runner: RUNNER_CARGO.into(),
            recorded_ms: 1,
            packages: graph::cargo_packages(METADATA, WORKSPACE).unwrap(),
            tests: inventory_of(
                INVENTORY_OUTPUT,
                &graph::cargo_packages(METADATA, WORKSPACE).unwrap(),
            ),
            definitions: Vec::new(),
            files: Vec::new(),
        }
    }

    fn leaf_change() -> Change {
        Change {
            files: vec![Touch {
                path: "crates/leaf/src/lib.rs".into(),
                lines: Some(vec![(2, 2)]),
            }],
        }
    }

    #[test]
    fn a_full_run_records_the_map_and_the_misses_of_the_head_landing() {
        let asked = RefCell::new(Vec::new());
        let mut exec = fork(&asked, FULL_OUTPUT);
        let previous = previous_map();
        let head = leaf_change();
        let recorded = record(&mut exec, "p", "c1", Some(&previous), Some(&head), 9).unwrap();

        let map = &recorded.map;
        assert_eq!(map.commit, "c1");
        assert_eq!(map.tests.len(), 2);
        assert_eq!(map.tests[0].package, "leaf");
        assert_eq!(map.tests[1].package, "other");
        assert_eq!(map.definitions.len(), 1, "{:?}", map.definitions);
        assert_eq!(
            map.definitions[0].covered_by,
            vec!["crates/leaf/tests/a.rs::one".to_string()]
        );
        assert_eq!(map.files, vec!["crates/leaf/src/lib.rs".to_string()]);

        // The head's change touches leaf: the selection took leaf's test, so `two` (failing in the full run) is a miss.
        assert_eq!(recorded.misses.len(), 1, "{:?}", recorded.misses);
        let miss = &recorded.misses[0];
        assert_eq!(miss.test, "crates/other/tests/b.rs::two");
        assert_eq!(miss.package, "other");
        assert!(
            miss.selection.starts_with("landing c1"),
            "{}",
            miss.selection
        );
        // One coverage run per test, then the full suite.
        let asked = asked.borrow();
        assert_eq!(asked.iter().filter(|argv| argv.len() == 6).count(), 2);
    }

    #[test]
    fn without_a_previous_map_nothing_was_skipped_so_nothing_is_a_miss() {
        let asked = RefCell::new(Vec::new());
        let mut exec = fork(&asked, FULL_OUTPUT);
        let recorded = record(&mut exec, "p", "c1", None, Some(&leaf_change()), 9).unwrap();
        assert!(recorded.misses.is_empty());
        assert_eq!(recorded.map.tests.len(), 2);
    }

    #[test]
    fn a_change_to_the_build_files_selects_the_full_suite_and_misses_nothing() {
        let asked = RefCell::new(Vec::new());
        let mut exec = fork(&asked, FULL_OUTPUT);
        let head = Change {
            files: vec![Touch {
                path: "Cargo.toml".into(),
                lines: None,
            }],
        };
        let recorded = record(&mut exec, "p", "c1", Some(&previous_map()), Some(&head), 9).unwrap();
        assert!(recorded.misses.is_empty());
    }

    #[test]
    fn the_inventory_and_the_failures_are_read_by_their_names() {
        let packages = graph::cargo_packages(METADATA, WORKSPACE).unwrap();
        let tests = inventory_of(
            "./crates/leaf/src/a.rs\tx\n./crates/leaf/src/a.rs\tx\n./crates/other/src/lib.rs\tcheck\n",
            &packages,
        );
        assert_eq!(tests.len(), 2, "a test is listed once");
        assert_eq!(tests[0].id, "crates/leaf/src/a.rs::x");
        assert_eq!(tests[0].package, "leaf");
        assert_eq!(tests[1].package, "other");
        let failed = failed_tests(
            "test a::b::check ... FAILED\ntest x ... ok\n    check\n",
            &tests,
        );
        assert_eq!(failed, vec!["crates/other/src/lib.rs::check".to_string()]);
    }

    #[test]
    fn a_failed_command_that_must_succeed_is_the_run_s_failure() {
        let mut exec = |_: &Command| -> Result<Answer, String> {
            Ok(Answer {
                ok: false,
                exit_code: Some(101),
                output: "error: no metadata".into(),
            })
        };
        let why = record(&mut exec, "p", "c1", None, None, 9).unwrap_err();
        assert!(why.starts_with("metadata failed (exit 101)"), "{why}");
        assert!(why.contains("no metadata"));
    }
}
