//! apz-load: the load generator of a lab VM (PLAN 22.8). It runs a traffic profile against a URL, prints the run's
//! summary as JSON, and, given the run's collector, posts the summary to the run as OTLP gauges, so the hub keeps it as
//! the run's series and compares it with the last qualified stable release's.
//!
//! Usage: `apz-load --target <url> --profile <shape>-<n>rps [--duration-secs <s>] [--tool auto|builtin|k6|vegeta]
//!                  [--scratch <dir>] [--out <file>] [--collector <url> --token <token>]`
//!        `apz-load bench --format criterion-text|criterion-json|libtest|go --file <path> [--name <name>]
//!                  [--out <file>] [--collector <url> --token <token>]`: a benchmark run's output, parsed and recorded
//!        the same way (`bench.<name>.ns` gauges, PLAN 22.8).
//!
//! `auto` uses k6 or vegeta when the image has them on its PATH and the built-in generator otherwise; nothing is
//! installed.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use apz_topology::bench::{self, Format};
use apz_topology::collector::{Relay, now_ms};
use apz_topology::load::{self, Tool};

fn usage() -> ExitCode {
    eprintln!(
        "usage: apz-load --target <url> --profile <shape>-<n>rps [--duration-secs <s>] \
         [--tool auto|builtin|k6|vegeta] [--scratch <dir>] [--out <file>] [--collector <url> --token <token>]"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("bench") {
        return bench(&args[1..]);
    }
    let mut flags = BTreeMap::new();
    for pair in args.chunks(2) {
        match pair {
            [key, value] if key.starts_with("--") => {
                flags.insert(key.trim_start_matches("--").to_string(), value.clone());
            }
            _ => return usage(),
        }
    }
    let (Some(target), Some(name)) = (flags.get("target"), flags.get("profile")) else {
        return usage();
    };
    let duration_ms = match flags.get("duration-secs").map(|s| s.parse::<u64>()) {
        None => None,
        Some(Ok(secs)) => Some(secs * 1000),
        Some(Err(_)) => return usage(),
    };
    let profile = match load::parse(name, duration_ms) {
        Ok(profile) => profile,
        Err(why) => {
            eprintln!("apz-load: {why}");
            return ExitCode::from(2);
        }
    };
    let path_var = std::env::var("PATH").unwrap_or_default();
    let tool = match flags.get("tool").map_or("auto", String::as_str) {
        "auto" => load::choose(&profile, &path_var),
        "builtin" => Tool::Builtin,
        "k6" => match load::find_on(&path_var, "k6") {
            Some(k6) => Tool::K6(k6),
            None => return fail("k6 is not on the PATH"),
        },
        "vegeta" => match load::find_on(&path_var, "vegeta") {
            Some(vegeta) => Tool::Vegeta(vegeta),
            None => return fail("vegeta is not on the PATH"),
        },
        other => {
            return fail(&format!(
                "{other} is not a tool: auto, builtin, k6 or vegeta"
            ));
        }
    };
    let scratch = flags.get("scratch").map_or_else(
        || std::env::temp_dir().join(format!("apz-load-{}", std::process::id())),
        PathBuf::from,
    );
    if let Err(why) = std::fs::create_dir_all(&scratch) {
        return fail(&format!("{}: {why}", scratch.display()));
    }
    let summary = match load::run_with(&tool, target, &profile, &scratch) {
        Ok(summary) => summary,
        Err(why) => return fail(&why),
    };
    let text = match serde_json::to_string_pretty(&summary) {
        Ok(text) => text,
        Err(why) => return fail(&why.to_string()),
    };
    println!("{text}");
    if let Some(out) = flags.get("out")
        && let Err(why) = std::fs::write(out, &text)
    {
        return fail(&format!("{out}: {why}"));
    }
    if let Some(collector) = flags.get("collector") {
        let token = flags.get("token").cloned().unwrap_or_default();
        let body = load::otlp(&summary, now_ms()).to_string();
        if let Err(why) = Relay::new(collector, &token).post("metrics", body.as_bytes()) {
            return fail(&format!("the summary was not recorded at the run: {why}"));
        }
    }
    ExitCode::SUCCESS
}

fn fail(why: &str) -> ExitCode {
    eprintln!("apz-load: {why}");
    ExitCode::FAILURE
}

/// `apz-load bench`: parses a benchmark run's output and records each benchmark's mean at the run.
fn bench(args: &[String]) -> ExitCode {
    let mut flags = BTreeMap::new();
    for pair in args.chunks(2) {
        match pair {
            [key, value] if key.starts_with("--") => {
                flags.insert(key.trim_start_matches("--").to_string(), value.clone());
            }
            _ => return usage(),
        }
    }
    let format = match flags.get("format").map(String::as_str) {
        Some("criterion-text") => Format::CriterionText,
        Some("criterion-json") => Format::CriterionJson,
        Some("libtest") => Format::Libtest,
        Some("go") => Format::Go,
        _ => return fail("--format is criterion-text, criterion-json, libtest or go"),
    };
    let Some(file) = flags.get("file") else {
        return fail("--file names the benchmark output");
    };
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(why) => return fail(&format!("{file}: {why}")),
    };
    let name = flags
        .get("name")
        .map_or_else(|| file.clone(), String::clone);
    let benches = match bench::parse(format, &text, &name) {
        Ok(benches) if !benches.is_empty() => benches,
        Ok(_) => return fail("no benchmark in the output"),
        Err(why) => return fail(&why),
    };
    let text = match serde_json::to_string_pretty(&benches) {
        Ok(text) => text,
        Err(why) => return fail(&why.to_string()),
    };
    println!("{text}");
    if let Some(out) = flags.get("out")
        && let Err(why) = std::fs::write(out, &text)
    {
        return fail(&format!("{out}: {why}"));
    }
    if let Some(collector) = flags.get("collector") {
        let token = flags.get("token").cloned().unwrap_or_default();
        let body = bench::otlp(&benches, now_ms()).to_string();
        if let Err(why) = Relay::new(collector, &token).post("metrics", body.as_bytes()) {
            return fail(&format!(
                "the benchmarks were not recorded at the run: {why}"
            ));
        }
    }
    ExitCode::SUCCESS
}
