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
mod allowances;
mod api;
mod appearance;
mod approvals;
mod artifacts;
mod autoscale;
mod base_builds;
mod base_recipe;
mod base_vm;
mod budgets;
mod cards;
mod chats;
mod checks;
mod connectors;
mod content;
mod context_track;
mod control;
mod curator;
mod decisions;
mod deliverables;
mod deploy;
mod disk;
mod drafts;
mod e2b;
mod e2b_net;
mod e2b_obs;
mod e2b_tpl;
mod egress_inject;
mod engine_feed;
mod engine_instances;
mod engine_watch;
mod environments;
mod evals;
mod events;
mod fleet;
mod foundry;
mod full_run;
mod github_modes;
mod glue;
mod harness_control;
mod hub_auth;
mod hub_mcp;
mod ide;
mod ide_term;
mod incidents;
mod inference;
mod intake;
mod internal;
mod journal;
mod lab_observe;
mod lab_performance;
mod lab_scrub;
mod lab_security;
mod lab_ux;
mod laws;
mod library;
mod lifecycle;
mod llm_proxy;
mod login_bridge;
mod maintenance;
mod mcp;
mod metrics;
mod mover;
mod node_link;
mod nodes;
mod openapi;
mod owner_board;
mod placement;
mod planning;
mod platforms;
mod plaza;
mod policy;
mod previews;
mod prices;
mod probes;
mod project_egress;
mod project_files;
mod project_memory;
mod project_settings;
mod project_stack;
mod projects;
mod prompts;
mod provider_health;
mod provider_listings;
mod providers;
mod recipes;
mod releases;
mod replan;
mod review_graph;
mod role;
mod roles_custom;
mod rules;
mod scheduler;
mod seats;
mod secrets;
mod sessions;
mod settings;
mod shares;
mod shutdown;
mod skills;
mod sleep;
mod sources;
mod spawned;
mod standing_rules;
mod state;
mod stations;
mod steps;
mod storage;
mod store;
mod targets;
mod task_graph;
mod task_plan;
mod test_lab;
mod test_select;
mod threads;
mod tool_life;
mod toolbox;
mod toolbox_survey;
mod tools;
mod topology;
mod transcript;
mod transfer;
mod triggers;
mod tunnel;
mod unfurl;
mod upgrades;
mod upstream;
mod usage;
mod usage_providers;
mod user_test;
mod vault;
mod vendor_relay;
mod verifier;
mod vm_snapshots;
mod weft_hub;
mod work_loop;
mod work_routes;

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
  schema             print the stored components' tables (sql/plaza/0001_init.sql) and exit
  --env-file FILE    KEY=VALUE lines loaded into the environment at start
                     (default ~/.config/agentplaza/keys.env, if present)
  --vault-key FILE   the vault's key file (default: the config directory's vault.key); created on
                     first start, refused when group or world readable
  --mcp-proxy-port PORT  the MCP egress proxy's port on 127.0.0.1 (default 19128)
  --e2b-gateway-port PORT  vsockd's E2B gateway on 127.0.0.1 (default 49982)
  --content-port PORT  the content origin that serves HTML artifacts, on the API's address (default the
                     API port + 1)
  --dev-models       relax the model policy (mock and local models; tests only)";

fn main() -> ExitCode {
    // First, before any thread starts: the signal mask set here is inherited by every thread after it.
    shutdown::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("apid {}", apz_os::version());
        return ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|a| a == "openapi") {
        openapi::print();
        return ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|a| a == "schema") {
        store::print_schema();
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
    let args = match content::take_port(args) {
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
