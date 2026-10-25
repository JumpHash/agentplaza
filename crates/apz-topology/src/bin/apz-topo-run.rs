//! apz-topo-run: the guest's topology runner (PLAN 22.6). Inside a lab VM it builds the topology's network and starts its
//! containers' artifacts on it, with the runner's resolver; an image needs the guest's container runtime and is refused.
//! A dependency that a resilience scenario faults is routed through an `apz-fault` proxy per port (PLAN 22.8).
//!
//! Usage: `apz-topo-run <plan.json> [--net | --local] [--collect <config.json>] [--scenarios]
//!        [--stages <repo-root> --stage-work <dir>]`
//!
//! `--net` runs the `ip` commands of [`apz_topology::plan::bridge_commands`] first (root in the guest). `--local` starts
//! the containers on 127.0.0.1 instead of the plan's addresses (no network to build: the runner's local mode, for a host
//! without the lab VM's bridge). `--collect` starts the lab VM's collector (`apz-topo-collect`, beside this binary) first,
//! and the containers' output goes to the collector's log directory (its config's `logs_dir`). Without `--scenarios` or
//! `--stages` the runner stays up until one of its containers exits, and exits with failure, naming it. With `--scenarios`
//! it runs the plan's resilience scenarios against its containers, prints what each did as JSON, records the measurements
//! at the run through the collector's relay, and exits. With `--stages` it runs the stages the plan declares (PLAN 22.8)
//! once the services are listening: `apz-stage` (beside this binary) runs them over the repo at `<repo-root>` in
//! `<dir>`, and posts their numbers through the collector's relay (or prints them when there is no collector); the hub
//! judges the numbers. A stage that cannot run is reported by its missing numbers, not by a failed run. A service listens on `APZ_TOPO_BIND` at `APZ_TOPO_PORTS`; `apz-topo-run` sets the other `APZ_TOPO_*` variables
//! (see `apz_topology::run`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::Duration;

use apz_topology::collector::{self, Config, Relay};
use apz_topology::plan::{GuestPlan, bridge_commands, collector_listen};
use apz_topology::run::{self, Addressing, Observed};
use apz_topology::scenarios;
use apz_topology::steps::{Commands, Guest, System};

/// The arguments: the plan's path, whether to build the network or run locally, the collector's config, whether to run
/// scenarios, and the stages' repo root and work directory.
struct Args {
    plan: String,
    net: bool,
    local: bool,
    collect: Option<String>,
    scenarios: bool,
    stages: Option<String>,
    stage_work: Option<String>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut plan = None;
    let mut net = false;
    let mut collect = None;
    let mut scenarios = false;
    let mut local = false;
    let mut stages = None;
    let mut stage_work = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--net" => net = true,
            "--local" => local = true,
            "--scenarios" => scenarios = true,
            "--collect" => {
                collect = Some(rest.next().ok_or("--collect names a config file")?.clone());
            }
            "--stages" => {
                stages = Some(rest.next().ok_or("--stages names the repo root")?.clone());
            }
            "--stage-work" => {
                stage_work = Some(rest.next().ok_or("--stage-work names a directory")?.clone());
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            other => {
                if plan.replace(other.to_string()).is_some() {
                    return Err("one plan file, please".into());
                }
            }
        }
    }
    if net && local {
        return Err(
            "--net builds the lab VM's bridge and --local runs without it: one or the other".into(),
        );
    }
    if stages.is_some() != stage_work.is_some() {
        return Err("--stages and --stage-work go together".into());
    }
    Ok(Args {
        plan: plan.ok_or("no plan file")?,
        net,
        local,
        collect,
        scenarios,
        stages,
        stage_work,
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match parse(&args) {
        Ok(parsed) => parsed,
        Err(why) => {
            eprintln!(
                "usage: apz-topo-run <plan.json> [--net] [--collect <config.json>] [--scenarios]: {why}"
            );
            return ExitCode::from(2);
        }
    };
    match serve(&parsed) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("apz-topo-run: {why}");
            ExitCode::FAILURE
        }
    }
}

/// A binary beside this runner (`apz-fault`, `apz-stage`, ...).
fn sibling(name: &str) -> Result<PathBuf, String> {
    Ok(std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
}

/// The `apz-fault` binary beside this runner.
fn fault_binary() -> Result<PathBuf, String> {
    sibling("apz-fault")
}

/// Runs the plan's stages with `apz-stage` over the repo at `root`, once the services are listening. Its numbers go to
/// the collector's relay when there is one, else to stdout.
fn run_stages(
    plan: &str,
    root: &str,
    work: &str,
    urls: &BTreeMap<String, String>,
    config: Option<&Config>,
) -> Result<(), String> {
    let binary = sibling("apz-stage")?;
    let mut command = Command::new(&binary);
    command.arg(plan).arg(root).arg(work);
    for (name, url) in urls {
        command.arg("--service").arg(format!("{name}={url}"));
    }
    if let Some(config) = config {
        command
            .arg("--upstream")
            .arg(&config.upstream)
            .arg("--token")
            .arg(&config.token);
    }
    let status = command
        .status()
        .map_err(|e| format!("{}: {e}", binary.display()))?;
    if !status.success() {
        eprintln!(
            "apz-topo-run: apz-stage ended {status}: the stages that could not run are judged as not run"
        );
    }
    Ok(())
}

fn serve(args: &Args) -> Result<(), String> {
    let text = std::fs::read_to_string(&args.plan).map_err(|e| format!("{}: {e}", args.plan))?;
    let plan: GuestPlan = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", args.plan))?;
    if args.net {
        for command in bridge_commands(&plan) {
            let status = Command::new(&command[0])
                .args(&command[1..])
                .status()
                .map_err(|e| format!("{}: {e}", command.join(" ")))?;
            if !status.success() {
                return Err(format!("{} failed: {status}", command.join(" ")));
            }
        }
    }
    // The collector runs for as long as the runner does: its child ends with it.
    let mut observed = Observed {
        fault_binary: Some(fault_binary()?),
        ..Observed::default()
    };
    let config: Option<Config> = match &args.collect {
        Some(path) => Some(
            serde_json::from_str(
                &std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
            )
            .map_err(|e| format!("{path}: {e}"))?,
        ),
        None => None,
    };
    let _collector = match (&args.collect, &config) {
        (Some(path), Some(config)) => {
            config.check()?;
            observed.logs_dir = Some(config.logs_dir.clone());
            let binary = std::env::current_exe()
                .map_err(|e| e.to_string())?
                .with_file_name(format!("apz-topo-collect{}", std::env::consts::EXE_SUFFIX));
            let child = collector::spawn(
                &binary,
                &PathBuf::from(path),
                &collector_listen(),
                Duration::from_secs(30),
            )?;
            Some(Ended(child))
        }
        _ => None,
    };
    let addressing = if args.local {
        Addressing::Local
    } else {
        Addressing::Plan
    };
    let mut running = run::start_observed(&plan, addressing, &BTreeMap::new(), &observed)?;
    running.wait_listening(Duration::from_secs(60))?;
    if let (Some(root), Some(work)) = (&args.stages, &args.stage_work) {
        run_stages(&args.plan, root, work, &running.urls(), config.as_ref())?;
        if !args.scenarios {
            return Ok(());
        }
    }
    if args.scenarios {
        let urls = running.urls();
        let mut guest = Guest {
            host: System {
                path: std::env::var("PATH").unwrap_or_default(),
            },
            runner: Commands,
            procs: running,
            device: plan.bridge.clone(),
        };
        let outcomes = scenarios::run_with(&plan.resilience, &urls, &mut guest);
        let report = scenarios::report(&plan.resilience, &outcomes);
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
        );
        if let Some(config) = &config {
            let relay = Relay::new(&config.upstream, &config.token);
            let recorded = scenarios::record(&relay, &outcomes)?;
            eprintln!("apz-topo-run: {recorded} scenario(s) measured and recorded at the run");
        }
        return Ok(());
    }
    loop {
        if let Some((name, status)) = running.exited().into_iter().next() {
            return Err(format!("{name} ended: {status}"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// A child process that is killed when it is dropped.
struct Ended(std::process::Child);

impl Drop for Ended {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
