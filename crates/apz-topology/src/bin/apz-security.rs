//! apz-security: the security stages of a lab run (PLAN 22.8). It runs the stages the topology declares (`[security]`)
//! and prints their outcomes as JSON; apid judges them (`POST /v1/test-lab/runs/{id}/assess`, with the outcomes).
//!
//! Usage: `apz-security --topology <file.toml> [--tree <dir>] [--service <name>=<url>]... [--out <file>]`
//!
//! The tree is the release's (the audits, the secrets scan and the fuzzers read it); each `--service` gives the base URL
//! the DAST scan and schemathesis reach a service at. Nothing is installed: a tool that is not on the `PATH` is a skipped
//! stage with its reason. Exit status 0 when the stages were run (their hits are the judge's to read), 2 on a usage or a
//! topology error.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use apz_topology::security::{self, Context};
use apz_topology::spec;

fn usage(flag: &str) -> String {
    format!(
        "usage: apz-security --topology <file.toml> [--tree <dir>] [--service <name>=<url>]... [--out <file>] (at {flag})"
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("apz-security: {why}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let mut topology = None;
    let mut tree = None;
    let mut out = None;
    let mut services = BTreeMap::new();
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let value = rest.next().ok_or_else(|| usage(flag))?;
        match flag.as_str() {
            "--topology" => topology = Some(value.clone()),
            "--tree" => tree = Some(PathBuf::from(value)),
            "--out" => out = Some(PathBuf::from(value)),
            "--service" => {
                let (name, url) = value.split_once('=').ok_or_else(|| usage(flag))?;
                services.insert(name.to_string(), url.to_string());
            }
            _ => return Err(usage(flag)),
        }
    }
    let path = topology.ok_or_else(|| usage("--topology"))?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let topology = spec::parse(&text)?;
    let path_var = std::env::var("PATH").unwrap_or_default();
    let cx = Context {
        tree: tree.as_deref(),
        services: &services,
        path_var: &path_var,
    };
    let outcomes = security::run(&topology.security, &cx);
    let json = serde_json::to_string_pretty(&outcomes).map_err(|e| e.to_string())?;
    match out {
        Some(file) => {
            std::fs::write(&file, json + "\n").map_err(|e| format!("{}: {e}", file.display()))
        }
        None => {
            println!("{json}");
            Ok(())
        }
    }
}
