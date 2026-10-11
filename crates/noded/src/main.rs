//! `noded` - the AgentPlaza node daemon v0.
//!
//! ```text
//! noded [--port 8899] [--kernel /boot/vmlinuz-linux]
//!       [--initrd target/guest/initrd.x86_64.gz]
//!       [--hub-status 127.0.0.1:19080] [--hub-admin 127.0.0.1:19081]
//! ```
//!
//! Binds `127.0.0.1` only and serves the run API plus the console's static
//! files; see the crate documentation for the endpoints. `--hub-status` names
//! the hub's status surface to proxy at `GET /api/v0/hub`, so the console can
//! read the fleet through this daemon despite its `connect-src 'self'` policy,
//! and `--hub-admin` names the hub's admin surface that
//! `POST /api/v0/tasks/{id}/artifact` drives a task's agent through, so the
//! console can read a task's artifact from the same origin, and that
//! `POST /api/v0/agents/{cid}/tell` drives one tool call through, so the
//! operator's console can drive the fleet from the same origin. The guest
//! images and the console directory are read relative to the working
//! directory, so the daemon is started from the repository root.

use std::path::PathBuf;
use std::process::ExitCode;

use noded::fleet::Fleet;
use noded::hub::{HubAdmin, HubStatus};
use noded::registry::Detail;
use noded::{Config, Server};

const USAGE: &str = "\
usage: noded [--port PORT] [--kernel PATH] [--initrd PATH] [--uds-dir DIR]
             [--workspace IMAGE] [--agents MANIFEST]
             [--agents-cid-base N] [--agents-memory-mib N] [--agents-vcpus N]
             [--hub-status HOST:PORT|off] [--hub-admin HOST:PORT|off]

  --port PORT      port to listen on, on 127.0.0.1 (default 8899)
  --kernel PATH    guest kernel image (default /boot/vmlinuz-linux)
  --initrd PATH    guest initrd (default target/guest/initrd.x86_64.gz)
  --uds-dir DIR    directory for the per-VM vsock Unix sockets and, with
                   --workspace, for the runs' image copies
                   (default /tmp/plaza-vsock; VM i's muxer binds DIR/vm{cid})
  --workspace IMAGE  base ext4 image of every run's writable workspace volume:
                   each run copies it once per VM into DIR/run-{id}/
                   (default: no block device in the guests)
  --agents MANIFEST  roles manifest (docs/agents/initial-roles.json): the
                   daemon boots one long-lived keepalive VM per role at
                   startup, in alphabetical role order, and registers them as
                   one run (default: no fleet)
  --agents-cid-base N   first guest CID of the fleet: role i gets N + i
                        (default 100)
  --agents-memory-mib N guest memory per agent VM, in MiB (default 128)
  --agents-vcpus N      vCPUs per agent VM (default 1)
  --hub-status HOST:PORT  where the hub (vsockd)'s status surface is, proxied
                   at GET /api/v0/hub and /api/v0/hub/healthz so the console
                   can read the fleet from this daemon's origin. Loopback
                   only: exactly 127.0.0.1 or localhost
                   (default 127.0.0.1:19080). `off` serves those endpoints
                   with a 503: a daemon not told where the hub is must not
                   report an empty registry.
  --hub-admin HOST:PORT  where the hub (vsockd)'s admin surface is, driven by
                   POST /api/v0/tasks/{id}/artifact so the console can read a
                   claimed task's artifact (the file its agent wrote), and by
                   POST /api/v0/agents/{cid}/tell so the operator's console can
                   drive one tool call on a connected agent - both from this
                   daemon's origin. Loopback only: exactly 127.0.0.1 or
                   localhost (default 127.0.0.1:19081). `off` serves those
                   endpoints with a 503: a daemon not told where the admin
                   surface is cannot ask any agent to read anything, or to run
                   anything.
  -h, --help       print this help

Serves GET/POST /api/v0/runs, GET/DELETE /api/v0/runs/{id},
POST /api/v0/runs/{id}/input, GET/POST /api/v0/tasks,
GET/DELETE /api/v0/tasks/{id}, POST /api/v0/tasks/{id}/claim,
/api/v0/tasks/{id}/finish, /api/v0/tasks/{id}/bid,
/api/v0/tasks/{id}/award and /api/v0/tasks/{id}/artifact,
GET /api/v0/treasury and /api/v0/treasury/journal, GET /api/v0/agora,
POST /api/v0/agents/{cid}/tell, GET /api/v0/hub[/healthz] and the web app's
files from apps/web.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("noded: {message}");
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let config = parse_args(args)?;
    // F4: the per-VM muxer binds `{uds_dir}/vm{cid}` with a deterministic
    // name, and the default is under world-writable `/tmp`. Create the
    // directory 0700 first, so another local user cannot squat a VM's path
    // (the boot would fail with EADDRINUSE) or bind `vm{cid}_{port}` and
    // impersonate the orchestrator.
    vmm::spawner::create_uds_dir(&config.uds_dir).map_err(|err| {
        format!(
            "cannot create the vsock socket directory {}: {err}",
            config.uds_dir.display()
        )
    })?;
    let server = Server::start(config.clone()).map_err(|err| format!("cannot listen: {err}"))?;
    // The initial role fleet is the service's setup: it is booted before the
    // daemon is left to serve, and a node that cannot stand it up says so and
    // exits rather than serving without its agents.
    boot_agents(server.node(), &config)?;
    println!(
        "noded: listening on http://127.0.0.1:{} (kernel {}, initrd {}, console {})",
        server.port(),
        config.kernel.display(),
        config.initrd.display(),
        config.console_dir.display()
    );
    server.wait().map_err(|err| format!("server error: {err}"))
}

/// Reads `--hub-status`: `off` disables the proxy, and anything else must be a
/// loopback `host:port`.
fn parse_hub_status(value: &str) -> Result<Option<HubStatus>, String> {
    if value == "off" {
        return Ok(None);
    }
    HubStatus::parse(value)
        .map(Some)
        .map_err(|err| format!("--hub-status: {err}"))
}

/// Reads `--hub-admin`: `off` disables the read-through, and anything else must
/// be a loopback `host:port` (the same policy as `--hub-status`).
fn parse_hub_admin(value: &str) -> Result<Option<HubAdmin>, String> {
    if value == "off" {
        return Ok(None);
    }
    HubAdmin::parse(value)
        .map(Some)
        .map_err(|err| format!("--hub-admin: {err}"))
}

/// Boots the initial role fleet, or reports why the node cannot serve it.
///
/// The manifest is read and checked before any VM is booted, so a manifest the
/// daemon cannot stand up never leaves a half-built fleet behind, and the fleet
/// itself is one run (`create_agent_fleet`). If a role's VM cannot be booted the
/// run ends as `failed` and the VMs that had come up are stopped — the whole
/// fleet is booted, or none of it is — and this fails the startup with what
/// stands (nothing) rather than serving a node whose agents are missing.
fn boot_agents(node: &noded::Node, config: &Config) -> Result<(), String> {
    let Some(path) = config.agents.as_ref() else {
        return Ok(());
    };
    let fleet = Fleet::load(
        path,
        config.agents_cid_base,
        config.agents_memory_mib,
        config.agents_vcpus,
    )
    .map_err(|err| format!("--agents: {err}"))?;
    let run = node
        .registry()
        .create_agent_fleet(&fleet)
        .map_err(|err| format!("the initial role fleet could not be created: {err}"))?;
    let view = run.view(Detail::Full);
    if view.state != "running" {
        let up = view.vms.iter().filter(|vm| vm.state == "running").count();
        return Err(format!(
            "the initial role fleet did not stand: run {} is {} ({up} of {} agents up)",
            run.id(),
            view.state,
            fleet.roles().len()
        ));
    }
    println!(
        "noded: initial role fleet {}: {} agents running (roles {}, CIDs {}..={})",
        run.id(),
        view.vms.len(),
        fleet.roles().join(", "),
        fleet.guest_cid(0).unwrap_or(config.agents_cid_base),
        fleet
            .guest_cid(fleet.roles().len().saturating_sub(1))
            .unwrap_or(config.agents_cid_base),
    );
    Ok(())
}

/// Reads `--port`, `--kernel` and `--initrd`, leaving the rest at its default.
fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut config = Config::default();
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        let mut value = |flag: &str| args.next().ok_or_else(|| format!("missing value for {flag}"));
        match argument.as_str() {
            "--port" => {
                config.port = value("--port")?
                    .parse()
                    .map_err(|err| format!("--port: {err}"))?;
            }
            "--kernel" => config.kernel = PathBuf::from(value("--kernel")?),
            "--initrd" => config.initrd = PathBuf::from(value("--initrd")?),
            "--uds-dir" => config.uds_dir = PathBuf::from(value("--uds-dir")?),
            "--workspace" => config.workspace = Some(PathBuf::from(value("--workspace")?)),
            "--agents" => config.agents = Some(PathBuf::from(value("--agents")?)),
            "--agents-cid-base" => {
                config.agents_cid_base = value("--agents-cid-base")?
                    .parse()
                    .map_err(|err| format!("--agents-cid-base: {err}"))?;
            }
            "--agents-memory-mib" => {
                config.agents_memory_mib = value("--agents-memory-mib")?
                    .parse()
                    .map_err(|err| format!("--agents-memory-mib: {err}"))?;
            }
            "--agents-vcpus" => {
                config.agents_vcpus = value("--agents-vcpus")?
                    .parse()
                    .map_err(|err| format!("--agents-vcpus: {err}"))?;
            }
            "--hub-status" => config.hub_status = parse_hub_status(&value("--hub-status")?)?,
            "--hub-admin" => config.hub_admin = parse_hub_admin(&value("--hub-admin")?)?,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_contracts_defaults() {
        let config = parse_args(Vec::new()).expect("no arguments is the default");
        assert_eq!(config.port, 8899);
        assert_eq!(config.kernel, PathBuf::from("/boot/vmlinuz-linux"));
        assert_eq!(
            config.initrd,
            PathBuf::from("target/guest/initrd.x86_64.gz")
        );
    }

    #[test]
    fn flags_override_the_defaults() {
        let config = parse_args(
            ["--port", "9000", "--kernel", "/k", "--initrd", "/i"]
                .map(String::from)
                .to_vec(),
        )
        .expect("well-formed flags");
        assert_eq!(config.port, 9000);
        assert_eq!(config.kernel, PathBuf::from("/k"));
        assert_eq!(config.initrd, PathBuf::from("/i"));
    }

    #[test]
    fn the_fleet_is_off_unless_the_manifest_is_named() {
        let config = parse_args(Vec::new()).expect("no arguments is the default");
        assert_eq!(config.agents, None);
        assert_eq!(config.agents_cid_base, 100);
        assert_eq!(config.agents_memory_mib, 128);
        assert_eq!(config.agents_vcpus, 1);
    }

    #[test]
    fn the_fleet_flags_override_their_defaults() {
        let config = parse_args(
            [
                "--agents",
                "docs/agents/initial-roles.json",
                "--agents-cid-base",
                "200",
                "--agents-memory-mib",
                "256",
                "--agents-vcpus",
                "2",
            ]
            .map(String::from)
            .to_vec(),
        )
        .expect("well-formed flags");
        assert_eq!(
            config.agents,
            Some(PathBuf::from("docs/agents/initial-roles.json"))
        );
        assert_eq!(config.agents_cid_base, 200);
        assert_eq!(config.agents_memory_mib, 256);
        assert_eq!(config.agents_vcpus, 2);
    }

    #[test]
    fn a_bad_command_line_is_refused() {
        for args in [
            vec!["--port".to_string()],
            vec!["--port".to_string(), "nope".to_string()],
            vec!["--kernel".to_string()],
            vec!["--what".to_string()],
            vec!["--agents".to_string()],
            vec!["--agents-cid-base".to_string(), "x".to_string()],
            vec!["--agents-memory-mib".to_string(), "x".to_string()],
            vec!["--agents-vcpus".to_string(), "x".to_string()],
            vec!["--hub-status".to_string()],
            vec!["--hub-status".to_string(), "10.0.0.1:19080".to_string()],
            vec!["--hub-status".to_string(), "127.0.0.1".to_string()],
            vec!["--hub-admin".to_string()],
            vec!["--hub-admin".to_string(), "10.0.0.1:19081".to_string()],
            vec!["--hub-admin".to_string(), "127.0.0.1.evil.tld:19081".to_string()],
            vec!["--hub-admin".to_string(), "127.0.0.1".to_string()],
        ] {
            assert!(parse_args(args.clone()).is_err(), "{args:?}");
        }
    }

    #[test]
    fn the_hub_status_defaults_to_the_live_surface() {
        let config = parse_args(Vec::new()).expect("no arguments is the default");
        assert_eq!(
            config.hub_status.map(|hub| hub.addr().to_string()),
            Some("127.0.0.1:19080".to_string())
        );
    }

    #[test]
    fn the_hub_status_flag_overrides_and_disables() {
        let config = parse_args(
            ["--hub-status", "localhost:19081"]
                .map(String::from)
                .to_vec(),
        )
        .expect("a loopback surface");
        assert_eq!(
            config.hub_status.map(|hub| hub.addr().to_string()),
            Some("127.0.0.1:19081".to_string()),
            "localhost normalizes to the literal loopback"
        );

        let config = parse_args(["--hub-status", "off"].map(String::from).to_vec())
            .expect("off is not an error");
        assert_eq!(config.hub_status, None);
    }

    #[test]
    fn the_hub_admin_defaults_to_the_live_surface() {
        let config = parse_args(Vec::new()).expect("no arguments is the default");
        assert_eq!(
            config.hub_admin.map(|hub| hub.addr().to_string()),
            Some("127.0.0.1:19081".to_string())
        );
    }

    #[test]
    fn the_hub_admin_flag_overrides_and_disables() {
        let config = parse_args(
            ["--hub-admin", "localhost:19082"].map(String::from).to_vec(),
        )
        .expect("a loopback surface");
        assert_eq!(
            config.hub_admin.map(|hub| hub.addr().to_string()),
            Some("127.0.0.1:19082".to_string()),
            "localhost normalizes to the literal loopback"
        );

        let config = parse_args(["--hub-admin", "off"].map(String::from).to_vec())
            .expect("off is not an error");
        assert_eq!(config.hub_admin, None);
    }
}
