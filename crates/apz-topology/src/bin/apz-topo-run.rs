//! apz-topo-run: the guest's topology runner (PLAN 22.6). Inside a lab VM it builds the topology's network and starts its
//! containers' artifacts on it, with the runner's resolver; an image needs the guest's container runtime and is refused.
//!
//! Usage: `apz-topo-run <plan.json> [--net]`
//!
//! `--net` runs the `ip` commands of [`apz_topology::plan::bridge_commands`] first (root in the guest). The runner then
//! stays up until one of its containers exits, and exits with failure, naming it. A service listens on
//! `APZ_TOPO_BIND` at `APZ_TOPO_PORTS`; `apz-topo-run` sets the other `APZ_TOPO_*` variables (see `apz_topology::run`).

use std::collections::BTreeMap;
use std::process::{Command, ExitCode};
use std::time::Duration;

use apz_topology::plan::{GuestPlan, bridge_commands};
use apz_topology::run::{self, Addressing};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let net = args.iter().any(|arg| arg == "--net");
    let Some(path) = args.iter().find(|arg| !arg.starts_with("--")) else {
        eprintln!("usage: apz-topo-run <plan.json> [--net]");
        return ExitCode::from(2);
    };
    match serve(path, net) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("apz-topo-run: {why}");
            ExitCode::FAILURE
        }
    }
}

fn serve(path: &str, net: bool) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let plan: GuestPlan = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    if net {
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
    let mut running = run::start(&plan, Addressing::Plan, &BTreeMap::new())?;
    running.wait_listening(Duration::from_secs(60))?;
    loop {
        if let Some((name, status)) = running.exited().into_iter().next() {
            return Err(format!("{name} ended: {status}"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}
