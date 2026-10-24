//! apz-fault: the lab VM's fault injector (PLAN 22.8). It starts a fault proxy in front of each dependency the topology's
//! resilience scenarios fault, runs the scenarios one after another against the guest's services (probing them, applying
//! each action, lifting it, measuring the recovery), prints each outcome as JSON, and posts the measurements to the run's
//! collector, where the hub judges them against the topology's expectations.
//!
//! Usage: `apz-fault <topology.toml> <config.json>` (the config is `apz_topology::scenarios::Config`). It exits when every
//! scenario has run or been refused with its reason. Nothing is installed: an action the guest cannot perform is reported
//! as not run.

use std::process::ExitCode;

use apz_topology::resilience::{self, Ran};
use apz_topology::scenarios::{self, Config};
use apz_topology::spec;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [topology_path, config_path] = args.as_slice() else {
        eprintln!("usage: apz-fault <topology.toml> <config.json>");
        return ExitCode::from(2);
    };
    match serve(topology_path, config_path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("apz-fault: {why}");
            ExitCode::FAILURE
        }
    }
}

fn serve(topology_path: &str, config_path: &str) -> Result<(), String> {
    let text =
        std::fs::read_to_string(topology_path).map_err(|e| format!("{topology_path}: {e}"))?;
    let topology = spec::parse(&text).map_err(|e| format!("{topology_path}: {e}"))?;
    let config: Config = serde_json::from_str(
        &std::fs::read_to_string(config_path).map_err(|e| format!("{config_path}: {e}"))?,
    )
    .map_err(|e| format!("{config_path}: {e}"))?;
    let outcomes = scenarios::run_all(&topology, &config)?;
    let mut report = Vec::new();
    for outcome in &outcomes {
        let scenario = topology.resilience.iter().find(|s| s.id == outcome.id);
        let entry = match (&outcome.ran, scenario) {
            (Ran::Measured(measured), Some(scenario)) => {
                let judgment = resilience::judge(scenario, measured);
                serde_json::json!({"id": outcome.id, "measured": measured, "passed": judgment.passed, "detail": judgment.detail})
            }
            (Ran::NotRun(reason), _) => serde_json::json!({"id": outcome.id, "not_run": reason}),
            (Ran::Measured(_), None) => {
                serde_json::json!({"id": outcome.id, "not_run": "no such scenario"})
            }
        };
        report.push(entry);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    let recorded = scenarios::record(&config, &outcomes)?;
    eprintln!("apz-fault: {recorded} scenario(s) measured and recorded at the run");
    Ok(())
}
