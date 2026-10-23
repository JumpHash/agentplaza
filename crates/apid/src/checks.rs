//! The CI recipe's runs (PLAN 4.5, 21.3): the checks of a change and their results as the gate.
//!
//! The recipe (`recipes/builtin/ci.toml`) has five steps. `lint` runs the format and lint commands of the project's
//! toolchains, `build` its build commands, `audit` `cargo deny check` where the repository configures it, and `tests`
//! the tests `tests_select` picks for the change from the project's coverage map on main (the full suite when no map is
//! recorded, or when the selection says so). Those four run their commands in one test fork (`base_vm::Fork`), and
//! `tests` starts once `build` has passed. `scan` is read by the hub: the lines the change adds are searched for
//! credentials (`secrets.rs`), so a found secret fails the gate with no fork at all.
//!
//! Each check is a row of its run (`CheckRow`: what ran and what it found) and a report the walker's gate reads. The
//! walker ends the run at the first check that fails (a failed step with no edge out), so the checks that were started
//! after it still run and keep their rows, and the steps the walk never settled stay as the walk left them.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use apz_testsel::map::{RUNNER_CARGO, RUNNER_NPM};
use apz_testsel::select::full_command;
use apz_testsel::{Change, Selection, Touch};

use crate::base_builds;
use crate::base_recipe::{self, Command, WORKSPACE};
use crate::base_vm::{Answer, Fork, Live};
use crate::placement;
use crate::project_stack::{self, Stack, Toolchain};
use crate::recipes::runs::{CheckRow, Runs, source_of};
use crate::recipes::{Move, Report};
use crate::secrets::{self, Added};
use crate::state::App;
use crate::test_select::{self, TestSelect};

/// The recipe a change's checks run, and the trigger a run of it records.
pub const RECIPE: &str = "ci";
pub const TRIGGER: &str = "change";

/// The most of a failing command's output a row keeps (its end: where the failure is).
pub const TAIL_CHARS: usize = 1200;

/// What runs a command in the test fork: the production fork, or a test's stand-in.
pub type Exec<'a> = dyn FnMut(&Command) -> Result<Answer, String> + 'a;

/// A file a change touches, as Weft's diff gives it: its path and its unified diff against its base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub unified: String,
}

/// What the checks of a change read.
pub struct Input<'a> {
    pub project: &'a str,
    pub change: &'a str,
    pub stack: &'a Stack,
    pub diff: &'a [FileDiff],
    /// Whether the repository configures `cargo deny` (a `deny.toml` at its root).
    pub deny: bool,
    /// The coverage maps the selection reads (`test_select::select_for`).
    pub maps: &'a TestSelect,
}

/// The commands of each check, and what the scan found, worked out before anything runs.
pub struct Plan {
    lint: Vec<Command>,
    build: Vec<Command>,
    audit: Vec<Command>,
    audit_note: String,
    tests: Vec<Command>,
    tests_note: String,
    findings: Vec<secrets::Finding>,
    scanned: usize,
}

/// The touches of a diff: each file, with the lines its hunks add (`None` when it adds none: the whole file).
pub fn touches(diff: &[FileDiff]) -> Change {
    Change {
        files: diff
            .iter()
            .map(|file| Touch {
                path: file.path.clone(),
                lines: added_ranges(&file.unified),
            })
            .collect(),
    }
}

/// The lines a diff adds, with their numbers in each file as it is after the change.
pub fn added(diff: &[FileDiff]) -> Vec<Added> {
    let mut out = Vec::new();
    for file in diff {
        walk_hunks(&file.unified, |line, text| {
            out.push(Added {
                path: file.path.clone(),
                line,
                text: text.to_string(),
            })
        });
    }
    out
}

/// The inclusive ranges of the lines a unified diff adds, or `None` when it adds none.
fn added_ranges(unified: &str) -> Option<Vec<(u32, u32)>> {
    let mut lines = Vec::new();
    walk_hunks(unified, |line, _| lines.push(line));
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for line in lines {
        match ranges.last_mut() {
            Some(last) if last.1 + 1 == line => last.1 = line,
            _ => ranges.push((line, line)),
        }
    }
    (!ranges.is_empty()).then_some(ranges)
}

/// Calls `on_added` with the number and text of each line a unified diff adds. Only the hunks are read (a file's header
/// is not a change); a line of a hunk is added (`+`), removed (`-`, which leaves the new file's numbering alone) or kept.
fn walk_hunks(unified: &str, mut on_added: impl FnMut(u32, &str)) {
    let mut next: Option<u32> = None;
    for text in unified.lines() {
        if let Some(header) = text.strip_prefix("@@") {
            // `@@ -a,b +c,d @@`: the new file's hunk starts at line c.
            next = header
                .split_whitespace()
                .find_map(|part| part.strip_prefix('+'))
                .and_then(|range| range.split(',').next())
                .and_then(|start| start.parse().ok());
            continue;
        }
        let Some(line) = next.as_mut() else {
            continue;
        };
        if let Some(added) = text.strip_prefix('+') {
            on_added(*line, added);
            *line += 1;
        } else if !(text.starts_with('-') || text.starts_with('\\')) {
            *line += 1;
        }
    }
}

/// The environment every command of the checks adds (the base's compiler cache, when it has one).
type Env = Vec<(&'static str, &'static str)>;

/// A command of the checks: its label, its argument vector and the fork's environment for it.
fn command(label: &str, argv: &[&str], env: &Env) -> Command {
    Command {
        label: label.to_string(),
        argv: argv.iter().map(|part| part.to_string()).collect(),
        cwd: WORKSPACE,
        timeout_s: base_recipe::STEP_TIMEOUT_S,
        env: env.clone(),
    }
}

/// The commands of the checks, from the stack and the change. The tests come from `tests_select`'s selection for the
/// change; with no map recorded, the full suite.
pub fn plan(input: &Input) -> Plan {
    let env = base_recipe::cache_env(input.stack);
    let rust = input.stack.toolchains.contains_key(&Toolchain::Rust);
    let node = input.stack.toolchains.contains_key(&Toolchain::Node);
    let mut lint = Vec::new();
    if rust {
        lint.push(command(
            "format",
            &["cargo", "fmt", "--all", "--", "--check"],
            &env,
        ));
        lint.push(command(
            "clippy",
            &[
                "cargo",
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
            &env,
        ));
    }
    if node {
        lint.push(command(
            "lint",
            &["npm", "run", "lint", "--if-present"],
            &env,
        ));
    }
    let build = base_recipe::build_commands(input.stack)
        .iter()
        .map(|argv| {
            let parts: Vec<&str> = argv.iter().map(String::as_str).collect();
            command("build", &parts, &env)
        })
        .collect();
    let (audit, audit_note) = match (rust, input.deny) {
        (true, true) => (
            vec![command("audit", &["cargo", "deny", "check"], &env)],
            String::new(),
        ),
        (true, false) => (
            Vec::new(),
            "cargo deny is not configured (no deny.toml)".into(),
        ),
        (false, _) => (Vec::new(), "no Rust toolchain: nothing to audit".into()),
    };
    let (tests, tests_note) =
        match test_select::select_for(input.maps, input.project, &touches(input.diff)) {
            Some((map, selection)) => (
                split_commands(&selection.command, &env),
                tests_note(&selection, &map.commit),
            ),
            None => {
                let runner = if rust || !node {
                    RUNNER_CARGO
                } else {
                    RUNNER_NPM
                };
                (
                    split_commands(full_command(runner), &env),
                    "the full suite: no coverage map is recorded for the project".into(),
                )
            }
        };
    let findings = secrets::scan(&added(input.diff));
    let scanned = added(input.diff).len();
    Plan {
        lint,
        build,
        audit,
        audit_note,
        tests,
        tests_note,
        findings,
        scanned,
    }
}

/// The commands of a `&&`-joined command line, each as its argument vector (the runners' commands never need a shell).
fn split_commands(line: &str, env: &Env) -> Vec<Command> {
    line.split(" && ")
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            let argv: Vec<&str> = part.split_whitespace().collect();
            command("tests", &argv, env)
        })
        .collect()
}

fn tests_note(selection: &Selection, commit: &str) -> String {
    if selection.full {
        format!("the full suite: {}", selection.full_because.join(", "))
    } else {
        format!(
            "{} of {} tests selected by the map of {}",
            selection.chosen.len(),
            selection.total,
            commit.chars().take(12).collect::<String>()
        )
    }
}

/// Runs a run of the `ci` recipe for a change: its checks in order, each recorded on the run, and the run's outcome. The
/// fork is `exec`: the production one, or a test's. Returns the run's id.
pub fn run_with(runs: &Runs, input: &Input, exec: &mut Exec, now: u64) -> Result<String, String> {
    let recipe = runs
        .recipe(RECIPE, None)
        .ok_or("the ci recipe is not on the hub")?;
    let plan = plan(input);
    let (run, moves) = runs.start(
        &recipe,
        source_of(&recipe),
        TRIGGER,
        None,
        Some(input.project),
        Some(input.change),
        now,
    );
    let mut ready = starts(moves);
    while let Some(step) = ready.pop_front() {
        runs.started(&run, &step, now);
        let (report, row) = check(&step, &plan, exec);
        runs.record_check(&run, row, now);
        ready.extend(starts(runs.settle(&run, &step, &report, now)));
    }
    Ok(run)
}

fn starts(moves: Vec<Move>) -> VecDeque<String> {
    moves
        .into_iter()
        .filter_map(|m| match m {
            Move::Ready(step) => Some(step),
            _ => None,
        })
        .collect()
}

/// One check: its report for the gate and its row for the run.
fn check(step: &str, plan: &Plan, exec: &mut Exec) -> (Report, CheckRow) {
    match step {
        "scan" => {
            let findings = plan.findings.len() as u32;
            let summary = if plan.findings.is_empty() {
                format!("{} added lines scanned: none found", plan.scanned)
            } else {
                let named: Vec<String> = plan
                    .findings
                    .iter()
                    .take(5)
                    .map(|f| format!("{} at {}:{}", f.kind, f.path, f.line))
                    .collect();
                format!("{findings} found: {}", named.join("; "))
            };
            let report = Report {
                passed: true,
                findings: Some(findings),
                ..Report::default()
            };
            let row = CheckRow {
                step: step.to_string(),
                passed: findings == 0,
                commands: "the secret scan of the added lines (the hub)".into(),
                summary,
            };
            (report, row)
        }
        "lint" => run_checks(step, &plan.lint, "nothing to lint", exec, false),
        "build" => run_checks(step, &plan.build, "nothing to build", exec, false),
        "audit" => run_checks(step, &plan.audit, &plan.audit_note, exec, false),
        "tests" => run_checks(step, &plan.tests, &plan.tests_note, exec, true),
        other => {
            let report = Report::verdict(false);
            let row = CheckRow {
                step: other.to_string(),
                passed: false,
                commands: String::new(),
                summary: "the ci recipe has no such check".into(),
            };
            (report, row)
        }
    }
}

/// Runs a check's commands in the fork, all of them, and reads the result: passed when every one exits 0. With no
/// command the check passes and says why in `note`.
fn run_checks(
    step: &str,
    commands: &[Command],
    note: &str,
    exec: &mut Exec,
    tests: bool,
) -> (Report, CheckRow) {
    let mut passed = true;
    let mut failures = Vec::new();
    let mut ran = Vec::new();
    for command in commands {
        ran.push(command.argv.join(" "));
        match exec(command) {
            Ok(answer) if answer.ok => {}
            Ok(answer) => {
                passed = false;
                let status = match answer.exit_code {
                    Some(code) => format!("exit {code}"),
                    None => "no exit status".to_string(),
                };
                failures.push(format!(
                    "{} {status}: {}",
                    command.label,
                    tail(answer.output.trim_end())
                ));
            }
            Err(why) => {
                passed = false;
                failures.push(format!("{}: {why}", command.label));
            }
        }
    }
    let summary = if !passed {
        failures.join(" | ")
    } else if commands.is_empty() || tests {
        note.to_string()
    } else {
        format!("{} commands passed", commands.len())
    };
    let report = Report {
        passed,
        tests: tests.then_some(passed),
        ..Report::default()
    };
    let row = CheckRow {
        step: step.to_string(),
        passed,
        commands: ran.join(" && "),
        summary,
    };
    (report, row)
}

/// The last [`TAIL_CHARS`] characters of `text` (where a failing command says what went wrong).
fn tail(text: &str) -> String {
    let count = text.chars().count();
    if count <= TAIL_CHARS {
        return text.to_string();
    }
    text.chars().skip(count - TAIL_CHARS).collect()
}

/// The CI runs asked for and not yet started, as (project, change). The route `POST /v1/recipe-runs/ci` queues them and
/// the scheduler's tick ([`tick`]) starts them, each on its own thread: a run holds a test fork for minutes.
static REQUESTED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn requested() -> MutexGuard<'static, Vec<(String, String)>> {
    REQUESTED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Asks for the CI checks of `change` of `project`. A request already waiting is kept once.
pub fn request(project: &str, change: &str) {
    let pair = (project.to_string(), change.to_string());
    let mut waiting = requested();
    if !waiting.contains(&pair) {
        waiting.push(pair);
    }
}

/// The requests waiting, emptied.
fn take_requests() -> Vec<(String, String)> {
    std::mem::take(&mut *requested())
}

/// The scheduler's entry point (every pass): starts the CI runs asked for since the last pass.
pub fn tick(app: &Arc<App>) {
    for (project, change) in take_requests() {
        let app = Arc::clone(app);
        std::thread::spawn(move || {
            if let Err(why) = start(&app, &project, &change, apz_plaza::now_ms()) {
                eprintln!("apid: checks of {change} ({project}): {why}");
            }
        });
    }
}

/// The CI run of a change of `project` (the production entry): the change's newest revision and its diff, the stack on
/// main, whether the repository configures `cargo deny`, and a test fork on the project's test base for its commands.
/// The run is recorded on the hub's runs; the fork is retired when its checks are done.
pub fn start(app: &App, project: &str, change: &str, now: u64) -> Result<String, String> {
    let rev = crate::project_memory::landed_revision(app, change)?;
    let diff = revision_diff(app, &rev)?;
    let stack = project_stack::read(app, project)?
        .ok_or_else(|| format!("{} is not on main", project_stack::FILE))?;
    let repo = app
        .projects
        .get(project)
        .and_then(|p| p.repo)
        .ok_or_else(|| format!("{project} has no repo yet"))?;
    let deny = revision_has(app, &rev, "deny.toml")?;
    let steps = base_recipe::fork_steps(&stack, &repo, &rev);
    let pool = base_builds::pool_keys(&stack)
        .first()
        .cloned()
        .unwrap_or_default();
    let host = Live(app);
    let fork: Result<Fork<'_, Live<'_>>, String> =
        match placement::base_node(app, project, &pool, &base_recipe::volumes(&stack)) {
            Some(node) => Fork::start(&host, node, project.to_string(), &steps),
            None => Err(format!("no node in the pool {pool} has room for a fork")),
        };
    let input = Input {
        project,
        change,
        stack: &stack,
        diff: &diff,
        deny,
        maps: &app.test_select,
    };
    let mut fork = fork;
    let outcome = {
        let mut exec = |command: &Command| match fork.as_mut() {
            Ok(fork) => fork.exec(command),
            Err(why) => Err(why.clone()),
        };
        run_with(&app.recipe_runs, &input, &mut exec, now)
    };
    if let Ok(fork) = fork
        && let Err(why) = fork.retire()
    {
        eprintln!("apid: the checks' fork of {change}: {why}");
    }
    outcome
}

/// A revision's diff against its base, as Weft gives it.
pub fn revision_diff(app: &App, rev: &str) -> Result<Vec<FileDiff>, String> {
    let diff = app.weft.get(&format!("/revisions/{rev}/diff"))?;
    Ok(diff["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| {
            Some(FileDiff {
                path: file["path"].as_str()?.to_string(),
                unified: file["unified"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect())
}

/// Whether a revision has a file at `path` (its root-level files, from Weft's listing).
fn revision_has(app: &App, rev: &str, path: &str) -> Result<bool, String> {
    let listing = app.weft.get(&format!("/revisions/{rev}/files"))?;
    Ok(listing["files"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|file| file["path"].as_str() == Some(path)))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    #[test]
    fn a_request_is_kept_once_and_taken_by_the_tick() {
        request("p-queue", "t-1");
        request("p-queue", "t-1");
        request("p-queue", "t-2");
        let taken: Vec<(String, String)> = take_requests()
            .into_iter()
            .filter(|(project, _)| project == "p-queue")
            .collect();
        assert_eq!(
            taken,
            vec![
                ("p-queue".to_string(), "t-1".to_string()),
                ("p-queue".to_string(), "t-2".to_string()),
            ]
        );
        assert!(
            take_requests()
                .iter()
                .all(|(project, _)| project != "p-queue")
        );
    }

    use apz_testsel::map::{CoverageMap, FORMAT, Package, TestEntry};

    use super::*;
    use crate::project_stack::parse;

    /// Rust and Node, with a deny config on the repository in some tests.
    const RUST: &str = "languages = []\n[toolchains]\nrust = \"1.88.0\"\n";

    fn diff_of(path: &str, unified: &str) -> FileDiff {
        FileDiff {
            path: path.into(),
            unified: unified.into(),
        }
    }

    /// A one-line change of `crates/leaf/src/lib.rs` at line 2, with its file header.
    fn leaf_diff() -> Vec<FileDiff> {
        vec![diff_of(
            "crates/leaf/src/lib.rs",
            "--- a/crates/leaf/src/lib.rs\n+++ b/crates/leaf/src/lib.rs\n@@ -1,3 +1,3 @@\n fn a() {}\n-fn b() {}\n+fn b() -> u8 { 1 }\n fn c() {}\n",
        )]
    }

    /// Two packages and one test each, the leaf's test covering the leaf's definition.
    fn map() -> CoverageMap {
        CoverageMap {
            id: CoverageMap::key("p", "c1"),
            format: FORMAT,
            project: "p".into(),
            commit: "c1".into(),
            source: "build-graph".into(),
            runner: RUNNER_CARGO.into(),
            recorded_ms: 1,
            packages: vec![
                Package {
                    name: "leaf".into(),
                    dir: "crates/leaf".into(),
                    deps: vec![],
                },
                Package {
                    name: "other".into(),
                    dir: "crates/other".into(),
                    deps: vec![],
                },
            ],
            tests: vec![
                TestEntry {
                    id: "crates/leaf/tests/a.rs::one".into(),
                    package: "leaf".into(),
                    file: "crates/leaf/tests/a.rs".into(),
                    smoke: false,
                },
                TestEntry {
                    id: "crates/other/tests/b.rs::two".into(),
                    package: "other".into(),
                    file: "crates/other/tests/b.rs".into(),
                    smoke: false,
                },
            ],
            definitions: vec![],
            files: vec![],
        }
    }

    fn ok() -> Answer {
        Answer {
            ok: true,
            exit_code: Some(0),
            output: String::new(),
        }
    }

    fn failed(output: &str) -> Answer {
        Answer {
            ok: false,
            exit_code: Some(1),
            output: output.into(),
        }
    }

    /// A fork that passes every command, except those whose argv starts with a `fail` prefix, and records what ran.
    fn fork<'a>(
        fail: &'a [&'a str],
        ran: &'a RefCell<Vec<Vec<String>>>,
    ) -> impl FnMut(&Command) -> Result<Answer, String> + 'a {
        move |command: &Command| {
            ran.borrow_mut().push(command.argv.clone());
            let joined = command.argv.join(" ");
            if fail.iter().any(|prefix| joined.starts_with(prefix)) {
                Ok(failed("error[E0308]: mismatched types\n"))
            } else {
                Ok(ok())
            }
        }
    }

    fn input<'a>(
        stack: &'a Stack,
        diff: &'a [FileDiff],
        maps: &'a TestSelect,
        deny: bool,
    ) -> Input<'a> {
        Input {
            project: "p",
            change: "t1",
            stack,
            diff,
            deny,
            maps,
        }
    }

    fn row(runs: &Runs, run: &str, step: &str) -> CheckRow {
        runs.view(run)
            .unwrap()
            .run
            .checks
            .into_iter()
            .find(|c| c.step == step)
            .unwrap_or_else(|| panic!("no row for {step}"))
    }

    #[test]
    fn the_added_lines_and_the_touches_come_from_the_hunks() {
        let diff = leaf_diff();
        assert_eq!(
            touches(&diff).files,
            vec![Touch {
                path: "crates/leaf/src/lib.rs".into(),
                lines: Some(vec![(2, 2)]),
            }]
        );
        let lines = added(&diff);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            (lines[0].line, lines[0].text.as_str()),
            (2, "fn b() -> u8 { 1 }")
        );

        // Two hunks, the second further down the file: its line numbers are the new file's.
        let two = vec![diff_of(
            "a.rs",
            "@@ -1,2 +1,3 @@\n x\n+y\n z\n@@ -10,1 +11,2 @@\n w\n+v\n",
        )];
        assert_eq!(touches(&two).files[0].lines, Some(vec![(2, 2), (12, 12)]));
        // A deletion adds no line: the whole file is what it touches.
        let deletion = vec![diff_of("b.rs", "@@ -4,1 +3,0 @@\n-gone\n")];
        assert_eq!(touches(&deletion).files[0].lines, None);
    }

    #[test]
    fn a_change_with_every_check_passing_passes_the_gate_and_each_check_is_on_the_run() {
        let stack = parse(RUST).unwrap();
        let maps = TestSelect::default();
        maps.record(map()).unwrap();
        let diff = leaf_diff();
        let runs = Runs::default();
        let ran = RefCell::new(Vec::new());
        let mut fork = fork(&[], &ran);
        let run = run_with(&runs, &input(&stack, &diff, &maps, false), &mut fork, 5).unwrap();
        let view = runs.view(&run).unwrap();
        assert_eq!(view.run.state, crate::recipes::runs::RunState::Passed);
        assert_eq!(view.run.checks.len(), 5, "{:?}", view.run.checks);
        assert!(view.run.checks.iter().all(|c| c.passed));
        // The selection picked the leaf's test: the tests step ran only that package's tests.
        let tests = row(&runs, &run, "tests");
        assert_eq!(tests.commands, "cargo test -p leaf");
        assert!(
            tests.summary.contains("1 of 2 tests selected"),
            "{}",
            tests.summary
        );
        let argvs = ran.borrow();
        assert!(argvs.contains(&vec![
            "cargo".to_string(),
            "test".into(),
            "-p".into(),
            "leaf".into()
        ]));
        assert!(!argvs.iter().any(|argv| argv.contains(&"other".to_string())));
        // The tests ran after the build (its edge), and the audit has nothing to run without a deny config.
        let build_at = argvs.iter().position(|a| a[1] == "build").unwrap();
        let tests_at = argvs.iter().position(|a| a[1] == "test").unwrap();
        assert!(build_at < tests_at);
        assert!(row(&runs, &run, "audit").summary.contains("not configured"));
    }

    #[test]
    fn a_failing_lint_fails_the_gate_and_its_output_is_on_the_run() {
        let stack = parse(RUST).unwrap();
        let maps = TestSelect::default();
        let diff = leaf_diff();
        let runs = Runs::default();
        let ran = RefCell::new(Vec::new());
        let mut fork = fork(&["cargo clippy"], &ran);
        let run = run_with(&runs, &input(&stack, &diff, &maps, false), &mut fork, 5).unwrap();
        let view = runs.view(&run).unwrap();
        assert_eq!(view.run.state, crate::recipes::runs::RunState::Failed);
        let lint = row(&runs, &run, "lint");
        assert!(!lint.passed);
        assert!(lint.summary.contains("clippy exit 1"), "{}", lint.summary);
        assert!(
            lint.summary.contains("mismatched types"),
            "{}",
            lint.summary
        );
        // The other checks still ran and kept their rows; the build passed on its own.
        assert!(row(&runs, &run, "build").passed);
        assert!(row(&runs, &run, "scan").passed);
    }

    #[test]
    fn a_found_secret_fails_the_gate_at_the_scan_and_names_its_line_only() {
        let stack = parse(RUST).unwrap();
        let maps = TestSelect::default();
        let diff = vec![diff_of(
            "src/config.rs",
            "@@ -0,0 +1,2 @@\n+let fine = 1;\n+const KEY: &str = \"AKIAIOSFODNN7EXAMPLE\";\n",
        )];
        let runs = Runs::default();
        let ran = RefCell::new(Vec::new());
        let mut fork = fork(&[], &ran);
        let run = run_with(&runs, &input(&stack, &diff, &maps, false), &mut fork, 5).unwrap();
        assert_eq!(
            runs.view(&run).unwrap().run.state,
            crate::recipes::runs::RunState::Failed
        );
        let scan = row(&runs, &run, "scan");
        assert!(!scan.passed);
        assert!(
            scan.summary.contains("AWS access key at src/config.rs:2"),
            "{}",
            scan.summary
        );
        assert!(!scan.summary.contains("IOSFOD"));
        // The other checks passed: the scan is what failed the gate.
        assert!(row(&runs, &run, "lint").passed);
    }

    #[test]
    fn a_failing_build_leaves_the_tests_unstarted() {
        let stack = parse(RUST).unwrap();
        let maps = TestSelect::default();
        let diff = leaf_diff();
        let runs = Runs::default();
        let ran = RefCell::new(Vec::new());
        let mut fork = fork(&["cargo build"], &ran);
        let run = run_with(&runs, &input(&stack, &diff, &maps, false), &mut fork, 5).unwrap();
        assert_eq!(
            runs.view(&run).unwrap().run.state,
            crate::recipes::runs::RunState::Failed
        );
        assert!(!row(&runs, &run, "build").passed);
        assert!(
            runs.view(&run)
                .unwrap()
                .run
                .checks
                .iter()
                .all(|c| c.step != "tests")
        );
        assert!(!ran.borrow().iter().any(|argv| argv[1] == "test"));
    }

    #[test]
    fn with_no_map_the_tests_are_the_full_suite_and_a_deny_config_runs_the_audit() {
        let stack = parse(RUST).unwrap();
        let maps = TestSelect::default();
        let diff = leaf_diff();
        let runs = Runs::default();
        let ran = RefCell::new(Vec::new());
        let mut fork = fork(&[], &ran);
        let run = run_with(&runs, &input(&stack, &diff, &maps, true), &mut fork, 5).unwrap();
        assert_eq!(row(&runs, &run, "tests").commands, "cargo test --workspace");
        assert!(
            row(&runs, &run, "tests")
                .summary
                .starts_with("the full suite")
        );
        assert_eq!(row(&runs, &run, "audit").commands, "cargo deny check");
    }

    #[test]
    fn a_fork_that_does_not_start_fails_every_fork_check_and_the_scan_still_runs() {
        let stack = parse(RUST).unwrap();
        let maps = TestSelect::default();
        let diff = leaf_diff();
        let runs = Runs::default();
        let mut exec = |_: &Command| -> Result<Answer, String> { Err("no node has room".into()) };
        let run = run_with(&runs, &input(&stack, &diff, &maps, false), &mut exec, 5).unwrap();
        assert_eq!(
            runs.view(&run).unwrap().run.state,
            crate::recipes::runs::RunState::Failed
        );
        assert!(
            row(&runs, &run, "lint")
                .summary
                .contains("no node has room")
        );
        assert!(row(&runs, &run, "scan").passed);
    }

    #[test]
    fn a_tail_keeps_the_end_of_a_long_output() {
        let text = format!("{}END", "x".repeat(TAIL_CHARS + 50));
        let kept = tail(&text);
        assert_eq!(kept.chars().count(), TAIL_CHARS);
        assert!(kept.ends_with("END"));
        assert_eq!(tail("short"), "short");
    }
}
