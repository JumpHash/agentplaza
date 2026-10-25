//! apz-topo-collect: the lab VM's collector (PLAN 22.7). It serves OTLP for the topology's services on the bridge's
//! gateway, relays it to the hub's receiver for the run, and ships the lines the services write to their log files.
//!
//! Usage: `apz-topo-collect <config.json>` (the config is `apz_topology::collector::Config`). It runs until it is killed.

use std::process::ExitCode;

use apz_topology::collector::{self, Config};

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: apz-topo-collect <config.json>");
        return ExitCode::from(2);
    };
    let config: Result<Config, String> = std::fs::read_to_string(&path)
        .map_err(|e| format!("{path}: {e}"))
        .and_then(|text| serde_json::from_str(&text).map_err(|e| format!("{path}: {e}")));
    match config.and_then(|config| collector::run(&config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("apz-topo-collect: {why}");
            ExitCode::FAILURE
        }
    }
}
