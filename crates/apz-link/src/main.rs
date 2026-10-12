//! apz-link [--state DIR] join <token> [--name NAME]
//! apz-link [--state DIR] run [--noded PORT] [--vsockd PORT] [--forward PORT]
//! apz-link [--state DIR] show
//!
//! The state directory defaults to `~/.local/share/agentplaza/node`.

use std::path::PathBuf;
use std::process::ExitCode;

use apz_link::{identity, join, run};

const USAGE: &str = "\
usage: apz-link [--state DIR] join <token> [--name NAME]
       apz-link [--state DIR] run [--noded PORT] [--vsockd PORT] [--forward PORT]
       apz-link [--state DIR] show";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("apz-link {}", apz_os::VERSION);
        return ExitCode::SUCCESS;
    }
    match dispatch(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("apz-link: {err}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(args: &[String]) -> Result<(), String> {
    let mut state = default_state();
    let mut rest: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--state" {
            state = PathBuf::from(args.get(i + 1).ok_or("--state needs a directory")?);
            i += 2;
        } else {
            rest.push(&args[i]);
            i += 1;
        }
    }
    match rest.as_slice() {
        ["join", token, more @ ..] => {
            let name = flag(more, "--name")?.map_or_else(hostname, str::to_string);
            let id = join::join(token, &name, join::machine_summary(), &state)?;
            println!("joined {} as {} (agents {})", id.hub_url, id.node_id, id.cid_range());
            Ok(())
        }
        ["run", more @ ..] => {
            let mut local = run::Local::default();
            if let Some(port) = flag(more, "--noded")? {
                local.noded = port.parse().map_err(|_| "--noded: not a port")?;
            }
            if let Some(port) = flag(more, "--vsockd")? {
                local.vsockd = port.parse().map_err(|_| "--vsockd: not a port")?;
            }
            if let Some(port) = flag(more, "--forward")? {
                local.forward = port.parse().map_err(|_| "--forward: not a port")?;
            }
            run::run(&state, local)
        }
        ["show"] => {
            let (id, _) = identity::load(&state)?;
            println!(
                "{}",
                serde_json::json!({"hub": id.hub_url, "hub_name": id.hub_name, "node": id.node_id, "cids": id.cid_range()})
            );
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    }
}

/// The value after `name` in `args`, if present.
fn flag<'a>(args: &[&'a str], name: &str) -> Result<Option<&'a str>, String> {
    match args.iter().position(|arg| *arg == name) {
        None => Ok(None),
        Some(at) => args.get(at + 1).copied().map(Some).ok_or_else(|| format!("{name} needs a value")),
    }
}

fn default_state() -> PathBuf {
    // The node's identity sits in the data directory the desktop app reads it from.
    apz_os::dirs::data().join("node")
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname").map(|h| h.trim().to_string()).unwrap_or_else(|_| "node".to_string())
}
