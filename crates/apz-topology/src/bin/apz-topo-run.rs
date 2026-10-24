//! apz-topo-run: the guest's topology runner (PLAN 22.6). Inside a lab VM it builds the topology's network and starts its
//! containers' artifacts on it, with the runner's resolver; an image needs the guest's container runtime and is refused.
//! A dependency that a resilience scenario faults is routed through an `apz-fault` proxy per port (PLAN 22.8).
//!
//! Usage: `apz-topo-run <plan.json> [--net] [--collect <config.json>] [--scenarios]`
//!
//! `--net` runs the `ip` commands of [`apz_topology::plan::bridge_commands`] first (root in the guest). `--collect` starts
//! the lab VM's collector (`apz-topo-collect`, beside this binary) first, and the containers' output goes to the
//! collector's log directory (its config's `logs_dir`). Without `--scenarios` the runner stays up until one of its
//! containers exits, and exits with failure, naming it. With `--scenarios` it runs the plan's resilience scenarios against
//! its containers, prints what each did as JSON, records the measurements at the run through the collector's relay, and
//! exits. A service listens on `APZ_TOPO_BIND` at `APZ_TOPO_PORTS`; `apz-topo-run` sets the other `APZ_TOPO_*` variables
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

/// The arguments: the plan's path, whether to build the network, the collector's config, and whether to run scenarios.
struct Args {
    plan: String,
    net: bool,
    collect: Option<String>,
    scenarios: bool,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut plan = None;
    let mut net = false;
    let mut collect = None;
    let mut scenarios = false;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--net" => net = true,
            "--scenarios" => scenarios = true,
            "--collect" => {
                collect = Some(rest.next().ok_or("--collect names a config file")?.clone());
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            other => {
                if plan.replace(other.to_string()).is_some() {
                    return Err("one plan file, please".into());
                }
            }
        }
    }
    Ok(Args {
        plan: plan.ok_or("no plan file")?,
        net,
        collect,
        scenarios,
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

/// The `apz-fault` binary beside this runner.
fn fault_binary() -> Result<PathBuf, String> {
    Ok(std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("apz-fault"))
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
                .with_file_name("apz-topo-collect");
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
    let mut running = run::start_observed(&plan, Addressing::Plan, &BTreeMap::new(), &observed)?;
    running.wait_listening(Duration::from_secs(60))?;
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
