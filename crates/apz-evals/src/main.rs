//! The eval suite (PLAN 13.6): runs a fixed set of tasks end to end on a scratch hub, with apid, vsockd and one real
//! harness process per VM, and the models scripted (or a real one with `--model`). Each task runs on a stack of its
//! own, so its numbers are its own. The report says, per task, the outcome, the tool calls, the model turns, the tokens,
//! the dollars and each tool's latency; `--compare` checks it against a committed baseline and fails when a task got
//! worse past the tolerance.
//!
//! Linux only: vsockd and the harness need it. The CI job (`.github/workflows/evals.yml`) runs it there.

// Off Linux the suite only refuses to run, so the report and the comparison have no caller there.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

mod cli;
mod compare;
mod report;
mod tasks;

#[cfg(target_os = "linux")]
mod noded;
#[cfg(target_os = "linux")]
mod run;
#[cfg(target_os = "linux")]
mod stack;

use std::process::ExitCode;

use report::Report;

fn main() -> ExitCode {
    let options = match cli::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(err) => {
            eprintln!("apz-evals: {err}");
            return ExitCode::from(2);
        }
    };
    match execute(&options) {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("apz-evals: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the suite (or the tasks `--only` names) and writes the report. Returns whether the comparison found a
/// regression.
#[cfg(target_os = "linux")]
fn execute(options: &cli::Options) -> Result<bool, String> {
    let all = tasks::load(&options.tasks)?;
    let selected: Vec<tasks::Task> = all
        .into_iter()
        .filter(|task| options.only.as_deref().is_none_or(|only| only == task.name))
        .collect();
    if selected.is_empty() {
        return Err("no task to run".into());
    }
    let roles = options.tasks.join("roles.json");
    let report = run::suite(options, &selected, &roles)?;
    write_report(options, &report)?;
    print!("{}", report::table(&report));
    match &options.compare {
        Some(path) => compare_with(path, options, &report),
        None => Ok(false),
    }
}

#[cfg(not(target_os = "linux"))]
fn execute(_: &cli::Options) -> Result<bool, String> {
    Err("the suite runs on Linux: vsockd and the harness are Linux programs".into())
}

fn write_report(options: &cli::Options, report: &Report) -> Result<(), String> {
    let Some(path) = &options.out else {
        return Ok(());
    };
    let text = serde_json::to_string_pretty(report).map_err(|err| err.to_string())?;
    std::fs::write(path, format!("{text}\n")).map_err(|err| format!("{}: {err}", path.display()))
}

fn compare_with(
    path: &std::path::Path,
    options: &cli::Options,
    report: &Report,
) -> Result<bool, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let baseline: Report =
        serde_json::from_str(&text).map_err(|err| format!("{}: {err}", path.display()))?;
    println!();
    print!("{}", compare::table(&baseline, report));
    let found = compare::regressions(&baseline, report, options.tolerance);
    for line in &found {
        println!("regression: {line}");
    }
    if found.is_empty() {
        println!(
            "no task is worse than the baseline by more than {:.0} %",
            options.tolerance * 100.0
        );
    }
    Ok(!found.is_empty())
}
