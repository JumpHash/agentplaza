//! `apid` — AgentPlaza's control plane. See `docs/architecture.md`.
//!
//! ```text
//! apid [--port 7700] [--state-dir DIR] [--noded-port 8899] [--hub-port 19080]
//!      [--roles docs/agents/initial-roles.json] [--prompts docs/codex-draft/roles]
//!      [--console apps/web/dist] [--env-file ~/.config/agentplaza/keys.env]
//!      [--dev-models]
//! ```

#![forbid(unsafe_code)]

mod agents;
mod approvals;
mod artifacts;
mod connectors;
mod deliverables;
mod autoscale;
mod api;
mod events;
mod internal;
mod llm_proxy;
mod laws;
mod library;
mod memory;
mod plaza;
mod providers;
mod role;
mod scheduler;
mod settings;
mod society;
mod spawned;
mod state;
mod tools;
mod transcript;
mod upstream;

use std::process::ExitCode;

use state::{App, Config};

const USAGE: &str = "\
usage: apid [--port PORT] [--state-dir DIR] [--noded-port PORT] [--hub-port PORT]
            [--roles MANIFEST] [--prompts DIR] [--console DIR] [--env-file FILE]
            [--dev-models]

  --port PORT        the API port on 127.0.0.1 (default 7700)
  --state-dir DIR    providers, settings, secrets, internal token
                     (default $XDG_STATE_HOME/agentplaza)
  --noded-port PORT  the node daemon (default 8899)
  --hub-port PORT    the hub (default 19080)
  --roles MANIFEST   roles manifest (default docs/agents/initial-roles.json)
  --prompts DIR      role prompts, DIR/<role>/prompt.md (default docs/codex-draft/roles)
  --console DIR      the web console's files (default apps/web/dist)
  --env-file FILE    KEY=VALUE lines loaded into the environment at start
                     (default ~/.config/agentplaza/keys.env, if present)
  --dev-models       relax the model policy (mock and local models; tests only)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let config = match Config::from_args(args) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("apid: {err}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match App::start(config) {
        Ok(server) => {
            println!("apid: listening on http://127.0.0.1:{}", server.port());
            match server.join() {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("apid: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(err) => {
            eprintln!("apid: {err}");
            ExitCode::FAILURE
        }
    }
}
