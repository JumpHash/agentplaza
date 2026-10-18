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
// tract-linalg's x86_64 kernels say `.align 16` meaning 16 bytes; Mach-O reads it as 2^16, so ld64 on an
// Intel Mac prints that it reduced the alignment. The kernels are fine at 4 KiB; the note is not ours.
#![cfg_attr(
    all(target_os = "macos", target_arch = "x86_64"),
    allow(linker_messages)
)]

mod acp;
mod adopt;
mod agent_proposals;
mod agents;
mod api;
mod approvals;
mod autoscale;
mod connectors;
mod control;
mod conversations;
mod db;
mod decisions;
mod deliverables;
mod drafts;
mod e2b;
mod e2b_net;
mod e2b_obs;
mod e2b_tpl;
mod egress_inject;
mod engine_feed;
mod environments;
mod events;
mod fleet;
mod foundry;
mod harness_control;
mod hub_auth;
mod hub_mcp;
mod ide;
mod ide_term;
mod inference;
mod internal;
mod laws;
mod library;
mod lifecycle;
mod llm_proxy;
mod looks;
mod maintenance;
mod mcp;
mod membership;
mod memory;
mod metrics;
mod mover;
mod node_link;
mod nodes;
mod openapi;
mod owner_board;
mod payments;
mod placement;
mod platforms;
mod plaza;
mod prices;
mod project_files;
mod project_settings;
mod projects;
mod prompts;
mod providers;
mod replan;
mod review_graph;
mod role;
mod roles_custom;
mod rules;
mod sanctions;
mod scheduler;
mod settings;
mod skills;
mod sleep;
mod society;
mod spawned;
mod state;
mod stations;
mod steps;
mod task_graph;
mod task_plan;
mod toolbox;
mod toolbox_survey;
mod tools;
mod transcript;
mod unfurl;
mod upstream;
mod usage;
mod vault;
mod vendor_relay;
mod vm_snapshots;
mod weft_hub;

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
  openapi            print the hub API's OpenAPI spec (docs/api/openapi.json) and exit
  --env-file FILE    KEY=VALUE lines loaded into the environment at start
                     (default ~/.config/agentplaza/keys.env, if present)
  --vault-key FILE   the vault's key file (default: the config directory's vault.key); created on
                     first start, refused when group or world readable
  --mcp-proxy-port PORT  the MCP egress proxy's port on 127.0.0.1 (default 19128)
  --e2b-gateway-port PORT  vsockd's E2B gateway on 127.0.0.1 (default 49982)
  --dev-models       relax the model policy (mock and local models; tests only)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("apid {}", apz_os::version());
        return ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|a| a == "openapi") {
        openapi::print();
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let args = match e2b::take_gateway_port(args) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("apid: {err}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
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
