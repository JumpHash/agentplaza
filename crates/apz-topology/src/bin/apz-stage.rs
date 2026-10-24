//! apz-stage: the lab VM's stage runner (PLAN 22.8). Runs the data, cache and messaging stages a topology declares and
//! records their numbers at the run, where the hub judges them as findings.
//!
//! Usage: `apz-stage <plan.json> <repo-root> <work-dir> [--service NAME=URL]... [--upstream URL --token TOKEN]`
//!
//! `<plan.json>` is the guest plan the runner was given: its `stages` are the stages the topology declares. The runner
//! (`apz-topo-run --stages`) runs this once the services are up.
//!
//! - The data stage runs over the repo at `<repo-root>` (its SQL files) in `<work-dir>/data`.
//! - The cache stage needs the URL of its service (`--service`, one for each service the run started): the service's
//!   declared paths are read over HTTP.
//! - The messaging stage runs its producer and the idempotent consumer in process; its deliveries are written to
//!   `<work-dir>/messages.jsonl`.
//!
//! The numbers are posted as OTLP gauges to the run's collector (`--upstream`, with `--token`) when given, and printed as
//! a JSON object of metric names to values otherwise. The notes (a failed query, a refused restore) go to stderr. Exit 0
//! when every stage ran, 1 when one could not start, 2 on a usage error.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use apz_topology::cache;
use apz_topology::collector::{Relay, now_ms};
use apz_topology::compare::Metric;
use apz_topology::data;
use apz_topology::messaging;
use apz_topology::plan::GuestPlan;
use apz_topology::stage;
use serde_json::{Map, Value};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(why)) => {
            eprintln!("apz-stage: {why}");
            eprintln!(
                "usage: apz-stage <plan.json> <repo-root> <work-dir> [--service NAME=URL]... [--upstream URL --token TOKEN]"
            );
            ExitCode::from(2)
        }
        Err(Failure::Stage(why)) => {
            eprintln!("apz-stage: {why}");
            ExitCode::FAILURE
        }
    }
}

enum Failure {
    Usage(String),
    Stage(String),
}

/// The options after the three paths: the services' URLs, and the collector's upstream and token.
type Options = (Vec<(String, String)>, Option<String>, Option<String>);

fn run(args: &[String]) -> Result<(), Failure> {
    let [plan_path, root, work, rest @ ..] = args else {
        return Err(Failure::Usage("three paths are needed".into()));
    };
    let (services, upstream, token) = options(rest).map_err(Failure::Usage)?;
    let text = std::fs::read_to_string(plan_path)
        .map_err(|why| Failure::Stage(format!("{plan_path}: {why}")))?;
    let plan: GuestPlan =
        serde_json::from_str(&text).map_err(|why| Failure::Stage(format!("{plan_path}: {why}")))?;
    let stages = &plan.stages;
    let root = Path::new(root);
    let work = Path::new(work);
    std::fs::create_dir_all(work)
        .map_err(|why| Failure::Stage(format!("{}: {why}", work.display())))?;
    let mut metrics: Vec<Metric> = Vec::new();
    let mut failed = Vec::new();

    if let Some(stage) = &stages.data {
        let run = data::run(stage, root, &work.join("data"));
        for note in &run.notes {
            eprintln!("data: {note}");
        }
        metrics.extend(run.metrics);
    }
    if let Some(stage) = &stages.cache {
        match services.iter().find(|(name, _)| *name == stage.service) {
            Some((_, url)) => {
                let subject = cache::Http::new(url, stage);
                metrics.extend(cache::run(stage, &subject));
            }
            None => failed.push(format!(
                "cache: no --service {}=URL was given",
                stage.service
            )),
        }
    }
    if let Some(stage) = &stages.messaging {
        let mut consumer = messaging::idempotent();
        let run = messaging::run(stage, &mut consumer);
        write_records(&work.join("messages.jsonl"), &run.records)
            .map_err(|why| Failure::Stage(format!("the deliveries were not written: {why}")))?;
        metrics.extend(run.metrics);
    }
    for why in &failed {
        eprintln!("apz-stage: {why}");
    }

    let body = stage::otlp("apz-stage", &metrics, now_ms()).to_string();
    match (upstream, token) {
        (Some(upstream), Some(token)) => {
            Relay::new(&upstream, &token)
                .post("metrics", body.as_bytes())
                .map_err(|why| {
                    Failure::Stage(format!("the numbers were not recorded at the run: {why}"))
                })?;
        }
        _ => {
            let summary: Map<String, Value> = metrics
                .iter()
                .map(|m| (m.name.clone(), Value::from(m.value)))
                .collect();
            println!("{}", Value::Object(summary));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Failure::Stage(format!(
            "{} stage(s) could not run",
            failed.len()
        )))
    }
}

fn options(rest: &[String]) -> Result<Options, String> {
    let mut services = Vec::new();
    let mut upstream = None;
    let mut token = None;
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        let value = it
            .next()
            .ok_or_else(|| format!("{flag} takes a value"))?
            .clone();
        match flag.as_str() {
            "--service" => {
                let (name, url) = value
                    .split_once('=')
                    .ok_or_else(|| format!("--service {value:?} is not NAME=URL"))?;
                services.push((name.to_string(), url.to_string()));
            }
            "--upstream" => upstream = Some(value),
            "--token" => token = Some(value),
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok((services, upstream, token))
}

/// The deliveries the consumer recorded, one JSON line each.
fn write_records(path: &Path, records: &[messaging::Record]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    for record in records {
        let line = serde_json::to_string(record).map_err(std::io::Error::other)?;
        writeln!(file, "{line}")?;
    }
    Ok(())
}
