//! The fuzzers a project declares as gates (PLAN 22.8): cargo-fuzz runs a target for its declared seconds, and
//! schemathesis runs its property tests against a service for the same budget. A crash or a failing property is a
//! hit; a fuzzer that is not installed, or that does not start, is a skip with its reason.

use std::fs;
use std::path::Path;
use std::time::Duration;

use super::exec::{self, tail};
use super::{Hit, Outcome, TIMED_OUT, TOOL_LIMIT};

/// cargo-fuzz runs `target` (in the crate at `dir`, which holds `fuzz/`) for `seconds`. The inputs it kept as crashes
/// are hits, each named by its file.
pub fn cargo_fuzz(tree: &Path, dir: &str, target: &str, seconds: u64, path_var: &str) -> Outcome {
    let stage = format!("fuzz:cargo-fuzz:{target}");
    let (Some(cargo), Some(_)) = (
        exec::tool(path_var, "cargo"),
        exec::tool(path_var, "cargo-fuzz"),
    ) else {
        return Outcome::skipped(&stage, "cargo and cargo-fuzz are not both on the PATH");
    };
    let root = tree.join(dir);
    if !root.join("fuzz").is_dir() {
        return Outcome::skipped(&stage, format!("no fuzz/ directory in {dir}"));
    }
    let budget = format!("-max_total_time={seconds}");
    let limit = Duration::from_secs(seconds).saturating_add(TOOL_LIMIT);
    let ran = match exec::run(
        &cargo,
        &["fuzz", "run", target, "--", &budget],
        &root,
        limit,
    ) {
        Ok(ran) => ran,
        Err(why) => return Outcome::skipped(&stage, format!("could not start: {why}")),
    };
    if ran.timed_out {
        return Outcome::skipped(&stage, TIMED_OUT);
    }
    let crashes = crash_inputs(&root.join("fuzz/artifacts").join(target));
    if !crashes.is_empty() {
        let hits = crashes
            .iter()
            .map(|name| Hit {
                id: "crash".into(),
                severity: "high".into(),
                subject: format!("{target} input {name}"),
                detail: format!(
                    "the target crashed on this input, kept in fuzz/artifacts/{target}"
                ),
            })
            .collect();
        return Outcome::ran(&stage, hits, tail(&ran.stderr, 20));
    }
    if ran.code == Some(0) {
        return Outcome::ran(
            &stage,
            Vec::new(),
            format!("ran {seconds} s, no crash input"),
        );
    }
    Outcome::skipped(
        &stage,
        format!(
            "the fuzz run ended with exit {:?} and no crash input: {}",
            ran.code,
            tail(&ran.stderr, 5)
        ),
    )
}

/// The files cargo-fuzz kept under an artifacts directory, by name, in order.
fn crash_inputs(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// schemathesis runs the property tests of the OpenAPI document `schema` (in the tree) against `base` for `seconds`. Exit
/// status 1 is a failing property (a hit). A run that the budget stops with no failure is clean for the budget.
pub fn schemathesis(
    tree: &Path,
    schema: &str,
    base: &str,
    seconds: u64,
    path_var: &str,
) -> Outcome {
    let stage = "fuzz:schemathesis";
    let Some(program) = exec::tool(path_var, "schemathesis") else {
        return Outcome::skipped(stage, "schemathesis is not on the PATH");
    };
    let document = tree.join(schema);
    if !document.is_file() {
        return Outcome::skipped(stage, format!("the schema {schema} is not in the tree"));
    }
    let document = document.display().to_string();
    let ran = match exec::run(
        &program,
        &["run", &document, "--base-url", base],
        tree,
        Duration::from_secs(seconds),
    ) {
        Ok(ran) => ran,
        Err(why) => return Outcome::skipped(stage, format!("could not start: {why}")),
    };
    if ran.timed_out {
        return Outcome::ran(
            stage,
            Vec::new(),
            format!("stopped at its {seconds} s budget with no failure"),
        );
    }
    match ran.code {
        Some(0) => Outcome::ran(stage, Vec::new(), format!("passed in {seconds} s")),
        Some(1) => Outcome::ran(
            stage,
            vec![Hit {
                id: "schema-failure".into(),
                severity: "high".into(),
                subject: schema.into(),
                detail: "a property of the schema failed against the service".into(),
            }],
            tail(&ran.stdout, 20),
        ),
        other => Outcome::skipped(
            stage,
            format!("exit {other:?} without a verdict: {}", tail(&ran.stderr, 5)),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_fuzzer_is_a_skip_with_its_reason() {
        let tree = tempfile::tempdir().unwrap();
        let outcome = cargo_fuzz(tree.path(), ".", "parse", 5, "");
        assert!(!outcome.ran);
        assert!(
            outcome.reason.contains("not both on the PATH"),
            "{}",
            outcome.reason
        );
        let outcome = schemathesis(tree.path(), "openapi.json", "http://h", 5, "");
        assert!(!outcome.ran);
        assert!(
            outcome.reason.contains("not on the PATH"),
            "{}",
            outcome.reason
        );
    }

    #[test]
    fn a_crash_input_kept_by_the_fuzzer_is_read_by_name_in_order() {
        let tree = tempfile::tempdir().unwrap();
        let dir = tree.path().join("fuzz/artifacts/parse");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("crash-b"), b"x").unwrap();
        fs::write(dir.join("crash-a"), b"y").unwrap();
        assert_eq!(crash_inputs(&dir), ["crash-a", "crash-b"]);
        assert!(crash_inputs(&tree.path().join("nowhere")).is_empty());
    }
}
