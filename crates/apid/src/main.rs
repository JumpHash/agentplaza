//! `apid` — AgentPlaza's control plane. See `docs/architecture.md`.
//!
//! ```text
//! apid [--port 7700] [--state-dir DIR] [--noded-port 8899] [--hub-port 19080]
//!      [--roles docs/agents/initial-roles.json] [--prompts docs/codex-draft/roles]
//!      [--console apps/web/dist] [--env-file ~/.config/agentplaza/keys.env]
//!      [--dev-models] [--bind ADDR] [--public-url URL] [--tls]
//! ```

// The vault's prctl call is the one `unsafe` block, allowed in vault/process.rs only.
#![deny(unsafe_code)]

mod agents;
mod approvals;
mod connectors;
mod deliverables;
mod autoscale;
mod api;
mod events;
mod inference;
mod internal;
mod llm_proxy;
mod laws;
mod library;
mod memory;
mod decisions;
mod payments;
mod plaza;
mod prices;
mod providers;
mod role;
mod scheduler;
mod sanctions;
mod settings;
mod society;
mod spawned;
mod state;
mod tools;
mod toolbox;
mod toolbox_survey;
mod projects;
mod harness_control;
mod git;
mod db;
mod rules;
mod control;
mod lifecycle;
mod looks;
mod mcp;
mod roles_custom;
mod agent_proposals;
mod nodes;
mod fleet;
mod foundry;
mod node_link;
mod hub_auth;
mod hub_mcp;
mod transcript;
mod upstream;
mod usage;
mod vault;

use std::process::ExitCode;

use state::{App, Config};

const USAGE: &str = "\
usage: apid [--port PORT] [--state-dir DIR] [--noded-port PORT] [--hub-port PORT]
            [--roles MANIFEST] [--prompts DIR] [--console DIR] [--env-file FILE]
            [--dev-models] [--vault-key FILE]

  --port PORT        the API port on 127.0.0.1 (default 7700)
  --state-dir DIR    providers, settings, secrets, internal token
                     (default: the platform's state directory, see apz-os)
  --noded-port PORT  the node daemon (default 8899)
  --hub-port PORT    the hub (default 19080)
  --bind ADDR        the address to serve on (default 127.0.0.1); off loopback the hub
                     serves HTTPS
  --public-url URL   how nodes reach this hub (default http://127.0.0.1:PORT, https with TLS)
  --tls              serve HTTPS with the hub's self-signed certificate
  --roles MANIFEST   roles manifest (default share/roles/initial-roles.json beside the binary
                     when installed, else docs/agents/initial-roles.json)
  --prompts DIR      role prompts, DIR/<role>/prompt.md (default share/prompts when installed,
                     else docs/codex-draft/roles)
  --console DIR      the web console's files (default share/console when installed, else
                     apps/web/dist)
  -V, --version      print the version and exit
  --env-file FILE    KEY=VALUE lines loaded into the environment at start
                     (default ~/.config/agentplaza/keys.env, if present)
  --vault-key FILE   the vault's key file (default: the config directory's vault.key); created on
                     first start, refused when group or world readable
  --mcp-proxy-port PORT  the MCP egress proxy's port on 127.0.0.1 (default 19128)
  --dev-models       relax the model policy (mock and local models; tests only)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("apid {}", apz_os::version());
        return ExitCode::SUCCESS;
    }
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
    vault::process::harden();
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
