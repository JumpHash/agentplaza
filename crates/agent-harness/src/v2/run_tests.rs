//! `run_tests`: the workspace's tests, run through [`super::run`], with their failures parsed.
//!
//! The runner is picked from the files at the workspace root: `Cargo.toml` runs `cargo test` (or `cargo nextest run`
//! when `cargo-nextest` is on the PATH), `go.mod` runs `go test ./...`, `pytest.ini` or a `conftest.py` runs
//! `pytest`, and a `package.json` with vitest or jest in it runs that. A `command` replaces the runner; its output
//! is parsed by the runner the command names, or by the first parser that finds a summary in it.
//!
//! Output: `{"runner", "command", "exit", "passed", "failed", "parsed", "failures": [{"name", "file"?, "line"?,
//! "message", "output"}]}`. `parsed` is false when no summary line was found: the result then carries the last
//! lines of the output as `tail`. The failures are cut to [`FAILURES_BUDGET`] bytes; a cut result says `continue`
//! with the filter that narrows the run to the first failure it left out.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use regex::Regex;
use serde_json::{Value, json};

use crate::files::{self, Workspace};
use crate::tools::ToolError;

use super::Lane;
use super::fit;
use super::run::{self, Exec};

/// The longest a test run may take: ten minutes.
pub const TEST_TIMEOUT_S: u64 = 600;

/// The most bytes of failures one `run_tests` result carries: 16 KiB.
pub const FAILURES_BUDGET: usize = 16 * 1024;

/// How many lines of a failure's own output it carries.
const FAILURE_LINES: usize = 6;

/// How many lines of the output a result without a summary carries as its `tail`.
const TAIL_LINES: usize = 40;

/// The runners `run_tests` knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Cargo,
    Nextest,
    Go,
    Pytest,
    Vitest,
    Jest,
}

/// Every kind, in the order a command without a known name is tried.
const KINDS: [Kind; 6] = [
    Kind::Cargo,
    Kind::Nextest,
    Kind::Go,
    Kind::Pytest,
    Kind::Vitest,
    Kind::Jest,
];

/// A runner picked from the workspace: its kind, the command that runs it, and its name in a result.
#[derive(Debug)]
struct Runner {
    kind: Kind,
    command: String,
    name: &'static str,
}

/// One test that failed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Failure {
    name: String,
    file: Option<String>,
    line: Option<u64>,
    message: String,
    output: Vec<String>,
}

/// What a test run's output says: its counts, and the failures it names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Report {
    passed: usize,
    failed: usize,
    failures: Vec<Failure>,
    /// A summary line was found.
    parsed: bool,
}

/// Runs a `run_tests` call.
pub(crate) fn run_tests(
    workspace: &Workspace,
    lane: &Arc<Lane>,
    input: &Value,
) -> Result<Value, ToolError> {
    let command = match input.get("command") {
        None | Some(Value::Null) => None,
        Some(Value::String(command)) if !command.trim().is_empty() => Some(command.clone()),
        Some(_) => return Err(files::invalid("\"command\" must be a non-empty string")),
    };
    let filter = match input.get("filter") {
        None | Some(Value::Null) => None,
        Some(Value::String(filter)) if !filter.is_empty() => Some(filter.clone()),
        Some(_) => return Err(files::invalid("\"filter\" must be a non-empty string")),
    };
    let (name, kind, command) = match command {
        Some(command) => {
            let kind = kind_named_by(&command);
            let command = match &filter {
                Some(filter) => format!("{command} {}", quote(filter)),
                None => command,
            };
            ("custom", kind, command)
        }
        None => {
            let runner = detect(workspace.dir(), filter.as_deref())?;
            (runner.name, Some(runner.kind), runner.command)
        }
    };

    let exec: Exec = run::foreground(workspace, lane, &command, TEST_TIMEOUT_S)?;
    let report = match kind {
        Some(kind) => parse(kind, &exec.output),
        None => KINDS
            .iter()
            .map(|kind| parse(*kind, &exec.output))
            .find(|report| report.parsed)
            .unwrap_or_default(),
    };

    let entries: Vec<Value> = report.failures.iter().map(failure_json).collect();
    let (shown, cut) = fit(entries, FAILURES_BUDGET);
    let mut reply = json!({
        "runner": name,
        "command": command,
        "exit": exec.exit,
        "passed": report.passed,
        "failed": report.failed,
        "parsed": report.parsed,
        "failures": shown,
    });
    if exec.timed_out {
        reply["timed_out"] = json!(true);
    }
    if cut {
        let next = report
            .failures
            .get(shown.len())
            .map_or_else(String::new, |failure| failure.name.clone());
        reply["truncated"] = json!(true);
        reply["continue"] = json!(super::continue_hint(
            "run_tests",
            &json!({ "filter": next })
        ));
    }
    if !report.parsed {
        let lines: Vec<&str> = exec.output.lines().collect();
        reply["tail"] = json!(lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n"));
    }
    Ok(reply)
}

/// The JSON of one failure.
fn failure_json(failure: &Failure) -> Value {
    let mut entry = json!({
        "name": failure.name,
        "message": failure.message,
        "output": failure.output,
    });
    if let Some(file) = &failure.file {
        entry["file"] = json!(file);
    }
    if let Some(line) = failure.line {
        entry["line"] = json!(line);
    }
    entry
}

/// The runner the workspace's root files name, with the filter applied.
fn detect(root: &Path, filter: Option<&str>) -> Result<Runner, ToolError> {
    let has = |name: &str| root.join(name).is_file();
    let with = |command: &str, flag: &str| match filter {
        Some(filter) => format!("{command} {flag}{}", quote(filter)),
        None => command.to_string(),
    };
    if has("Cargo.toml") {
        return Ok(if on_path("cargo-nextest") {
            Runner {
                kind: Kind::Nextest,
                command: with("cargo nextest run", " "),
                name: "cargo nextest run",
            }
        } else {
            Runner {
                kind: Kind::Cargo,
                command: with("cargo test", " "),
                name: "cargo test",
            }
        });
    }
    if has("go.mod") {
        return Ok(Runner {
            kind: Kind::Go,
            command: with("go test ./...", "-run "),
            name: "go test",
        });
    }
    if has("pytest.ini") || has("conftest.py") {
        return Ok(Runner {
            kind: Kind::Pytest,
            command: with("pytest", "-k "),
            name: "pytest",
        });
    }
    if let Ok(text) = fs::read_to_string(root.join("package.json"))
        && let Ok(package) = serde_json::from_str::<Value>(&text)
    {
        let depends = |name: &str| {
            ["dependencies", "devDependencies"]
                .iter()
                .any(|section| package[section].get(name).is_some())
        };
        if depends("vitest") {
            return Ok(Runner {
                kind: Kind::Vitest,
                command: with("npx vitest run", "-t "),
                name: "npx vitest run",
            });
        }
        if depends("jest") {
            return Ok(Runner {
                kind: Kind::Jest,
                command: with("npx jest", "-t "),
                name: "npx jest",
            });
        }
    }
    Err(ToolError::Unavailable {
        reason: format!(
            "no test runner: the workspace has no Cargo.toml, go.mod, pytest.ini or conftest.py, and no package.json with jest or vitest in it ({})",
            root.display()
        ),
    })
}

/// Whether a program of this name is on the PATH.
fn on_path(program: &str) -> bool {
    std::env::var("PATH").is_ok_and(|path| {
        path.split(':')
            .any(|dir| !dir.is_empty() && Path::new(dir).join(program).is_file())
    })
}

/// The kind a command names, if it names one.
fn kind_named_by(command: &str) -> Option<Kind> {
    [
        ("nextest", Kind::Nextest),
        ("cargo test", Kind::Cargo),
        ("go test", Kind::Go),
        ("pytest", Kind::Pytest),
        ("vitest", Kind::Vitest),
        ("jest", Kind::Jest),
    ]
    .into_iter()
    .find(|(word, _)| command.contains(word))
    .map(|(_, kind)| kind)
}

/// Single-quotes `text` for the shell.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Parses a run's output with the parser of `kind`.
pub(crate) fn parse(kind: Kind, output: &str) -> Report {
    match kind {
        Kind::Cargo => parse_cargo(output),
        Kind::Nextest => parse_nextest(output),
        Kind::Go => parse_go(output),
        Kind::Pytest => parse_pytest(output),
        Kind::Vitest | Kind::Jest => parse_js(output),
    }
}

/// The regex a parser uses; a pattern here is a literal, so it compiles.
fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|err| panic!("a parser pattern is valid: {err}"))
}

/// The first non-empty lines after `from`, trimmed on the right, at most [`FAILURE_LINES`].
fn detail(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.trim_end().to_string())
        .filter(|line| !line.is_empty())
        .take(FAILURE_LINES)
        .collect()
}

/// Where a panic says it happened, and what it said: `(file, line, message)`. Both the current form (the message
/// on the line after `panicked at file:line:col:`) and the older one (`panicked at 'message', file:line:col`).
fn panic_at(lines: &[&str]) -> (Option<String>, Option<u64>, String) {
    let current = re(r"panicked at (\S+?):(\d+):\d+:?\s*$");
    let older = re(r"panicked at '(.*)', (\S+?):(\d+):\d+");
    for (index, line) in lines.iter().enumerate() {
        if let Some(found) = older.captures(line) {
            return (
                Some(found[2].to_string()),
                found[3].parse().ok(),
                found[1].to_string(),
            );
        }
        if let Some(found) = current.captures(line) {
            let message = lines[index + 1..]
                .iter()
                .map(|next| next.trim())
                .find(|next| !next.is_empty())
                .unwrap_or_default()
                .to_string();
            return (Some(found[1].to_string()), found[2].parse().ok(), message);
        }
    }
    (None, None, String::new())
}

/// `cargo test`: the `test result:` lines give the counts, and each `---- name stdout ----` block a failure.
fn parse_cargo(output: &str) -> Report {
    let mut report = Report::default();
    let summary = re(r"test result: \w+\. (\d+) passed; (\d+) failed;");
    for found in summary.captures_iter(output) {
        report.passed += found[1].parse::<usize>().unwrap_or(0);
        report.failed += found[2].parse::<usize>().unwrap_or(0);
        report.parsed = true;
    }
    let lines: Vec<&str> = output.lines().collect();
    let header = re(r"^---- (.+) stdout ----$");
    for (index, line) in lines.iter().enumerate() {
        let Some(found) = header.captures(line) else {
            continue;
        };
        let block: Vec<&str> = lines[index + 1..]
            .iter()
            .take_while(|next| {
                !next.starts_with("---- ")
                    && !next.starts_with("failures:")
                    && !next.starts_with("test result:")
            })
            .copied()
            .collect();
        let (file, number, message) = panic_at(&block);
        report.failures.push(Failure {
            name: found[1].to_string(),
            file,
            line: number,
            message,
            output: detail(&block),
        });
    }
    let listed = re(r"^test (\S+) \.\.\. FAILED$");
    for line in &lines {
        if let Some(found) = listed.captures(line)
            && !report
                .failures
                .iter()
                .any(|failure| failure.name == found[1])
        {
            report.failures.push(Failure {
                name: found[1].to_string(),
                ..Failure::default()
            });
        }
    }
    if !report.parsed
        && let Some(error) = lines.iter().find(|line| line.starts_with("error"))
    {
        report.failures.push(Failure {
            name: "(build)".to_string(),
            message: error.trim().to_string(),
            ..Failure::default()
        });
    }
    report
}

/// `cargo nextest run`: `Summary` gives the counts, each `FAIL [time] binary test` line a failure, and the panics
/// in its output the locations, in order.
fn parse_nextest(output: &str) -> Report {
    let mut report = Report::default();
    let summary = re(r"Summary \[\s*[\d.]+s\] (\d+) tests? run: (\d+) passed(?:.*?(\d+) failed)?");
    if let Some(found) = summary.captures(output) {
        report.passed = found[2].parse().unwrap_or(0);
        report.failed = found
            .get(3)
            .and_then(|count| count.as_str().parse().ok())
            .unwrap_or(0);
        report.parsed = true;
    }
    let failed = re(r"^\s*(?:FAIL|TIMEOUT|SIGSEGV|SIGABRT)\s+\[\s*[\d.]+s\]\s+(.+)$");
    let lines: Vec<&str> = output.lines().collect();
    for line in &lines {
        let Some(found) = failed.captures(line) else {
            continue;
        };
        let name = found[1]
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .to_string();
        if !report.failures.iter().any(|failure| failure.name == name) {
            report.failures.push(Failure {
                name,
                ..Failure::default()
            });
        }
    }
    let panics = re(r"panicked at (\S+?):(\d+):\d+:?\s*$");
    let mut located = lines.iter().enumerate().filter_map(|(index, line)| {
        panics
            .captures(line)
            .map(|found| (index, found[1].to_string(), found[2].parse::<u64>().ok()))
    });
    for failure in report
        .failures
        .iter_mut()
        .filter(|failure| failure.file.is_none())
    {
        if let Some((index, file, number)) = located.next() {
            let (_, _, message) = panic_at(&lines[index..]);
            failure.file = Some(file);
            failure.line = number;
            failure.message = message;
        }
    }
    report
}

/// `go test`: each `--- FAIL: name` is a failure, and the `file_test.go:N: message` line above it its location.
/// The counts are the `--- PASS:` and `--- FAIL:` lines (subtests included).
fn parse_go(output: &str) -> Report {
    let lines: Vec<&str> = output.lines().collect();
    let mut report = Report {
        passed: lines
            .iter()
            .filter(|line| line.trim_start().starts_with("--- PASS:"))
            .count(),
        failed: lines
            .iter()
            .filter(|line| line.trim_start().starts_with("--- FAIL:"))
            .count(),
        ..Report::default()
    };
    report.parsed =
        report.passed + report.failed > 0 || lines.iter().any(|line| line.starts_with("ok  \t"));
    let failed = re(r"^\s*--- FAIL: (\S+)");
    let location = re(r"^\s+(\S+\.go):(\d+): (.*)$");
    for (index, line) in lines.iter().enumerate() {
        let Some(found) = failed.captures(line) else {
            continue;
        };
        let mut failure = Failure {
            name: found[1].to_string(),
            ..Failure::default()
        };
        let mut block = Vec::new();
        for above in lines[..index].iter().rev() {
            if above.starts_with("=== RUN") || above.trim_start().starts_with("--- ") {
                break;
            }
            block.push(*above);
        }
        block.reverse();
        if let Some(found) = block.iter().rev().find_map(|line| location.captures(line)) {
            failure.file = Some(found[1].to_string());
            failure.line = found[2].parse().ok();
            failure.message = found[3].to_string();
        }
        failure.output = detail(&block);
        report.failures.push(failure);
    }
    report
}

/// `pytest`: the `FAILED path::name - message` lines of the short summary are the failures, the `path:N:` line of
/// the traceback their location. The counts are on the final `=== N failed, M passed in T ===` line.
fn parse_pytest(output: &str) -> Report {
    let lines: Vec<&str> = output.lines().collect();
    let mut report = Report::default();
    let summary = re(r"^=+ .* in [\d.]+s");
    if let Some(line) = lines.iter().rev().find(|line| summary.is_match(line)) {
        let count = |word: &str| {
            re(&format!(r"(\d+) {word}"))
                .captures(line)
                .and_then(|found| found[1].parse::<usize>().ok())
                .unwrap_or(0)
        };
        report.passed = count("passed");
        report.failed = count("failed");
        report.parsed = true;
    }
    let failed = re(r"^FAILED (\S+?)(?: - (.*))?$");
    let traceback = re(r"^(\S+\.py):(\d+): ");
    for line in &lines {
        let Some(found) = failed.captures(line) else {
            continue;
        };
        let nodeid = found[1].to_string();
        let file = nodeid
            .split("::")
            .next()
            .map(str::to_string)
            .filter(|file| file.ends_with(".py"));
        let line_no = file.as_deref().and_then(|file| {
            lines.iter().find_map(|candidate| {
                traceback
                    .captures(candidate)
                    .filter(|found| found[1] == *file)
                    .and_then(|found| found[2].parse().ok())
            })
        });
        report.failures.push(Failure {
            name: nodeid,
            file,
            line: line_no,
            message: found
                .get(2)
                .map_or_else(String::new, |message| message.as_str().to_string()),
            output: Vec::new(),
        });
    }
    report
}

/// Jest and Vitest: the `Tests:` line gives the counts; a `● suite › name` block (Jest) or a `FAIL file > suite >
/// name` line (Vitest) a failure, with the `file:line` its trace names.
fn parse_js(output: &str) -> Report {
    let lines: Vec<&str> = output.lines().collect();
    let mut report = Report::default();
    let tests = re(r"^\s*Tests:?\s+(.*)$");
    if let Some(found) = lines.iter().rev().find_map(|line| tests.captures(line)) {
        let segment = found[1].to_string();
        let count = |word: &str| {
            re(&format!(r"(\d+) {word}"))
                .captures(&segment)
                .and_then(|found| found[1].parse::<usize>().ok())
                .unwrap_or(0)
        };
        report.passed = count("passed");
        report.failed = count("failed");
        report.parsed = true;
    }
    let jest_header = re(r"^\s+● (.+)$");
    let vitest_header = re(r"^ FAIL  (\S+) > (.+)$");
    let trace = re(r"(?:\(|❯ )(\S+?):(\d+):\d+\)?\s*$");
    let file_of_jest = re(r"^ FAIL  (\S+)");
    let mut current_file: Option<String> = None;
    for (index, line) in lines.iter().enumerate() {
        if let Some(found) = file_of_jest.captures(line) {
            current_file = Some(found[1].to_string());
        }
        let (file, name) = if let Some(found) = jest_header.captures(line) {
            (current_file.clone(), found[1].to_string())
        } else if let Some(found) = vitest_header.captures(line) {
            (Some(found[1].to_string()), found[2].to_string())
        } else {
            continue;
        };
        let block: Vec<&str> = lines[index + 1..]
            .iter()
            .take_while(|next| {
                !jest_header.is_match(next)
                    && !vitest_header.is_match(next)
                    && !next.starts_with("Test")
            })
            .copied()
            .collect();
        let located = block.iter().find_map(|candidate| {
            trace.captures(candidate).filter(|found| {
                file.as_deref()
                    .is_none_or(|file| file.ends_with(&found[1]) || found[1].ends_with(file))
            })
        });
        let message = detail(&block)
            .into_iter()
            .find(|text| {
                !text.trim_start().starts_with("at ") && !text.trim_start().starts_with("❯")
            })
            .unwrap_or_default();
        report.failures.push(Failure {
            name,
            file: located.as_ref().map(|found| found[1].to_string()).or(file),
            line: located.as_ref().and_then(|found| found[2].parse().ok()),
            message: message.trim().to_string(),
            output: detail(&block),
        });
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARGO: &str = "\
   Compiling foo v0.1.0 (/src)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.31s
     Running unittests src/lib.rs (target/debug/deps/foo-1234abcd)

running 4 tests
test tests::adds ... ok
test tests::subtracts ... FAILED
test tests::divides ... FAILED
test tests::ignored_one ... ignored

failures:

---- tests::subtracts stdout ----
thread 'tests::subtracts' panicked at src/lib.rs:12:9:
assertion `left == right` failed
  left: 3
 right: 4
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    tests::divides
    tests::subtracts

test result: FAILED. 1 passed; 2 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s

error: test failed, to rerun pass `--lib`
";

    #[test]
    fn cargo_test_counts_its_results_and_names_each_failure_with_its_panic() {
        let report = parse(Kind::Cargo, CARGO);
        assert_eq!((report.passed, report.failed), (1, 2));
        assert!(report.parsed);
        let subtracts = report
            .failures
            .iter()
            .find(|f| f.name == "tests::subtracts")
            .expect("named");
        assert_eq!(subtracts.file.as_deref(), Some("src/lib.rs"));
        assert_eq!(subtracts.line, Some(12));
        assert_eq!(subtracts.message, "assertion `left == right` failed");
        let divides = report
            .failures
            .iter()
            .find(|f| f.name == "tests::divides")
            .expect("listed");
        assert!(
            divides.file.is_none(),
            "a failure with no panic has no location"
        );
    }

    const CARGO_BUILD_BROKEN: &str = "\
error[E0432]: unresolved import `crate::missing`
 --> src/lib.rs:3:5
error: could not compile `foo` (lib test) due to 1 previous error
";

    #[test]
    fn a_build_that_fails_before_any_test_runs_is_reported_as_the_build() {
        let report = parse(Kind::Cargo, CARGO_BUILD_BROKEN);
        assert!(!report.parsed);
        assert_eq!(report.failures[0].name, "(build)");
        assert!(report.failures[0].message.contains("error[E0432]"));
    }

    const NEXTEST: &str = "\
    Starting 4 tests across 2 binaries
        FAIL [   0.010s] foo tests::subtracts
  stdout ───

    running 1 test
  stderr ───
    thread 'tests::subtracts' panicked at src/lib.rs:12:9:
    assertion `left == right` failed
────────────
     Summary [   0.021s] 4 tests run: 3 passed, 1 failed, 0 skipped
        FAIL [   0.010s] foo tests::subtracts
";

    #[test]
    fn nextest_reads_its_summary_and_the_failures_it_names() {
        let report = parse(Kind::Nextest, NEXTEST);
        assert_eq!((report.passed, report.failed), (3, 1));
        assert_eq!(
            report.failures.len(),
            1,
            "a failure listed twice is one failure"
        );
        assert_eq!(report.failures[0].name, "tests::subtracts");
        assert_eq!(report.failures[0].file.as_deref(), Some("src/lib.rs"));
        assert_eq!(report.failures[0].line, Some(12));
    }

    const GO: &str = "\
=== RUN   TestAdd
--- PASS: TestAdd (0.00s)
=== RUN   TestSub
    sub_test.go:14: got 3, want 4
--- FAIL: TestSub (0.00s)
=== RUN   TestDiv
--- PASS: TestDiv (0.00s)
FAIL
FAIL\texample.com/calc\t0.021s
";

    #[test]
    fn go_test_counts_pass_and_fail_lines_and_locates_the_failure_above_its_line() {
        let report = parse(Kind::Go, GO);
        assert_eq!((report.passed, report.failed), (2, 1));
        let failure = &report.failures[0];
        assert_eq!(failure.name, "TestSub");
        assert_eq!(failure.file.as_deref(), Some("sub_test.go"));
        assert_eq!(failure.line, Some(14));
        assert_eq!(failure.message, "got 3, want 4");
    }

    const PYTEST: &str = "\
============================= test session starts ==============================
platform linux -- Python 3.12.1, pytest-8.1.1, pluggy-1.5.0
collected 3 items

tests/test_calc.py .F.                                                   [100%]

=================================== FAILURES ===================================
__________________________________ test_sub ___________________________________

    def test_sub():
>       assert sub(3, 1) == 4
E       assert 2 == 4
E        +  where 2 = sub(3, 1)

tests/test_calc.py:9: AssertionError
=========================== short test summary info ============================
FAILED tests/test_calc.py::test_sub - assert 2 == 4
========================= 1 failed, 2 passed in 0.05s ==========================
";

    #[test]
    fn pytest_reads_its_final_counts_and_the_failed_lines_with_their_traceback_location() {
        let report = parse(Kind::Pytest, PYTEST);
        assert_eq!((report.passed, report.failed), (2, 1));
        let failure = &report.failures[0];
        assert_eq!(failure.name, "tests/test_calc.py::test_sub");
        assert_eq!(failure.file.as_deref(), Some("tests/test_calc.py"));
        assert_eq!(failure.line, Some(9));
        assert_eq!(failure.message, "assert 2 == 4");
    }

    const JEST: &str = "\
 PASS  src/add.test.ts
 FAIL  src/calc.test.ts
  ● calc › subtracts

    expect(received).toBe(expected) // Object.is equality

    Expected: 4
    Received: 2

      12 |   expect(sub(3, 1)).toBe(4);

    at Object.<anonymous> (src/calc.test.ts:12:24)

Test Suites: 1 failed, 1 passed, 2 total
Tests:       1 failed, 2 passed, 3 total
";

    #[test]
    fn jest_reads_its_tests_line_and_the_failure_with_its_trace() {
        let report = parse(Kind::Jest, JEST);
        assert_eq!((report.passed, report.failed), (2, 1));
        let failure = &report.failures[0];
        assert_eq!(failure.name, "calc › subtracts");
        assert_eq!(failure.file.as_deref(), Some("src/calc.test.ts"));
        assert_eq!(failure.line, Some(12));
        assert_eq!(
            failure.message,
            "expect(received).toBe(expected) // Object.is equality"
        );
    }

    const VITEST: &str = "\
 ✓ src/add.test.ts (2 tests) 3ms
 ❯ src/calc.test.ts (2 tests | 1 failed) 5ms
   × calc > subtracts 2ms
 FAIL  src/calc.test.ts > calc > subtracts
AssertionError: expected 2 to be 4 // Object.is equality
 ❯ src/calc.test.ts:12:24

 Test Files  1 failed | 1 passed (2)
      Tests  1 failed | 2 passed (3)
";

    #[test]
    fn vitest_reads_its_tests_line_and_the_fail_line_with_its_location() {
        let report = parse(Kind::Vitest, VITEST);
        assert_eq!((report.passed, report.failed), (2, 1));
        let failure = &report.failures[0];
        assert_eq!(failure.name, "calc > subtracts");
        assert_eq!(failure.file.as_deref(), Some("src/calc.test.ts"));
        assert_eq!(failure.line, Some(12));
        assert_eq!(
            failure.message,
            "AssertionError: expected 2 to be 4 // Object.is equality"
        );
    }

    #[test]
    fn a_command_names_its_runner_and_a_filter_is_quoted_into_it() {
        assert_eq!(kind_named_by("cargo test -p foo"), Some(Kind::Cargo));
        assert_eq!(kind_named_by("npx vitest run"), Some(Kind::Vitest));
        assert_eq!(kind_named_by("make check"), None);
        assert_eq!(quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn a_workspace_with_no_runner_says_which_files_it_looked_for() {
        let dir = std::env::temp_dir().join(format!("apz-v2-tests-none-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch folder");
        let err = detect(&dir, None).expect_err("no runner");
        assert!(
            err.to_string().starts_with("unavailable: no test runner"),
            "{err}"
        );
        fs::write(dir.join("Cargo.toml"), "[package]\n").expect("a manifest");
        let runner = detect(&dir, Some("adds")).expect("cargo");
        assert!(runner.command.starts_with("cargo "), "{}", runner.command);
        assert!(runner.command.ends_with(" 'adds'"), "{}", runner.command);
    }
}
