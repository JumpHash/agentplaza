//! `noded` - the AgentPlaza node daemon.
//!
//! ```text
//! noded [--port 8899] [--kernel /boot/vmlinuz-linux]
//!       [--initrd target/guest/initrd.x86_64.gz]
//! ```
//!
//! Binds `127.0.0.1` only and serves the `/v1` VM API (see the crate
//! documentation). Guest images are read relative to the working directory,
//! so the daemon is started from the repository root.

use std::path::PathBuf;
use std::process::ExitCode;

use noded::fleet::Fleet;
use noded::registry::Detail;
use noded::{Config, Server};

const USAGE: &str = "\
usage: noded [--port PORT] [--kernel PATH] [--initrd PATH] [--uds-dir DIR]
             [--workspace IMAGE] [--persist-dir DIR] [--agents MANIFEST]
             [--agents-cid-base N] [--agents-memory-mib N] [--agents-vcpus N]
             [--agents-balloon-mib N]

  --port PORT      port to listen on, on 127.0.0.1 (default 8899)
  --kernel PATH    guest kernel image (default guest/ARCH/vmlinux of the installed resources
                   when present, else /boot/vmlinuz-linux)
  --initrd PATH    guest initrd (default guest/ARCH/initrd.gz of the installed resources when
                   present, else target/guest/initrd.x86_64.gz)
  -V, --version    print the version and exit
  --uds-dir DIR    directory for the per-VM vsock Unix sockets and, with
                   --workspace, for the runs' image copies
                   (default /tmp/plaza-vsock on Linux and macOS, the state directory's
                   vsock on Windows; VM i's muxer binds DIR/vm{cid})
  --persist-dir DIR  keep keepalive agents' workspace images in DIR/workspaces/
                     (cid-<guest_cid>.ext4): copied once, reused across restarts
  --workspace IMAGE  base ext4 image of every run's writable workspace volume:
                   each run copies it once per VM into DIR/run-{id}/
                   (default: no block device in the guests)
  --agents MANIFEST  roles manifest (docs/agents/initial-roles.json): the
                   daemon boots one long-lived keepalive VM per role at
                   startup, in slot order (alphabetical if no role has a slot),
                   and registers them as
                   one run (default: no fleet)
  --agents-cid-base N   first guest CID of the fleet: a role with slot s gets
                        N + s (default 100)
  --agents-memory-mib N guest memory per agent VM, in MiB (default 1024)
  --agents-vcpus N      vCPUs per agent VM (default 1)
  --run-cids A-B   guest CIDs runs draw from, inclusive (default 20-99; A >= 3)
  --workstation-dir DIR     where workstation VMs keep their disks (default
                        <state>/workstations)
  --workstation-kernel F    the desktop guest's kernel, for workstation VMs
  --workstation-initrd F    the desktop guest's initramfs (with --workstation-kernel)
  --workstation-image F     the desktop guest's disk image; each VM copies it
  --agents-balloon-mib N  MiB of each agent VM's memory usable at boot; the rest
                        of --agents-memory-mib is a balloon it grows into
                        (default 32; 0, or at least the memory: no balloon)
  -h, --help       print this help

Runs. Every VM of every run carries the agent harness (the guest's vsock
client), so every run has guest CIDs. A create request that names no
`vsock_cid` is given the first free span of the node's run range (the
--run-cids range, default 20..=99, comfortably wider than a full-size run's 64) - held for the run's
lifetime and released when the run ends, and refused before any VM boots
when the range has no room. Which session the guests speak follows who chose
the CIDs: a run whose `vsock_cid` the caller pinned keeps the demo task's
frozen one-shot session (it ends by rebooting the guest, so the run reaches
`done`), and a run the node allocated CIDs for is an operator run - its
guests run the keepalive agent session, role-less, so a hub that serves the
run range (vsockd --cids) shows them as connected agents and can drive them,
and they stay up until the run is killed or `beats` churns the guest.

Serves the /v1 VM API (machine, runs, events); the plaza is apid's.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("noded {}", apz_os::version());
        return ExitCode::SUCCESS;
    }
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
        "noded: listening on http://127.0.0.1:{} (kernel {}, initrd {})",
        server.port(),
        config.kernel.display(),
        config.initrd.display()
    );
    server.wait().map_err(|err| format!("server error: {err}"))
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
    .map_err(|err| format!("--agents: {err}"))?
    .with_balloon(config.agents_balloon_mib);
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

/// The configuration before the flags: the installed guest images (`guest/<arch>/vmlinux` and
/// `initrd.gz` under the installed resources) where they exist, else the built-in defaults.
fn defaults() -> Config {
    let mut config = Config::default();
    if let Some(guest) =
        apz_os::dirs::resources().map(|share| share.join("guest").join(std::env::consts::ARCH))
    {
        let (kernel, initrd) = (guest.join("vmlinux"), guest.join("initrd.gz"));
        if kernel.is_file() {
            config.kernel = kernel;
        }
        if initrd.is_file() {
            config.initrd = initrd;
        }
    }
    config
}

/// Reads `--port`, `--kernel` and `--initrd`, leaving the rest at its default.
fn parse_args(args: Vec<String>) -> Result<Config, String> {
    let mut config = defaults();
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        let mut value = |flag: &str| {
            args.next()
                .ok_or_else(|| format!("missing value for {flag}"))
        };
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
            "--persist-dir" => config.persist_dir = Some(PathBuf::from(value("--persist-dir")?)),
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
            "--agents-balloon-mib" => {
                config.agents_balloon_mib = value("--agents-balloon-mib")?
                    .parse()
                    .map_err(|err| format!("--agents-balloon-mib: {err}"))?;
            }
            "--run-cids" => config.run_cids = parse_run_cids(&value("--run-cids")?)?,
            "--workstation-dir" => {
                config.workstation.dir = PathBuf::from(value("--workstation-dir")?)
            }
            "--workstation-kernel" => {
                config.workstation.kernel = Some(PathBuf::from(value("--workstation-kernel")?))
            }
            "--workstation-initrd" => {
                config.workstation.initrd = Some(PathBuf::from(value("--workstation-initrd")?))
            }
            "--workstation-image" => {
                config.workstation.image = Some(PathBuf::from(value("--workstation-image")?))
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(config)
}

/// Reads `A-B`, an inclusive CID range: both ends are numbers, `A` is at least
/// the first CID a guest may have, and `A` does not exceed `B`.
fn parse_run_cids(text: &str) -> Result<std::ops::RangeInclusive<u64>, String> {
    let bad = || format!("--run-cids: {text:?} is not a range A-B of CIDs");
    let (first, last) = text.split_once('-').ok_or_else(bad)?;
    let first: u64 = first.parse().map_err(|_| bad())?;
    let last: u64 = last.parse().map_err(|_| bad())?;
    if first < vmm::spawner::MIN_GUEST_CID {
        return Err(format!(
            "--run-cids: {first} is below {} (1 and 2 are reserved)",
            vmm::spawner::MIN_GUEST_CID
        ));
    }
    if first > last {
        return Err(format!(
            "--run-cids: {first}-{last} is empty (A must not exceed B)"
        ));
    }
    Ok(first..=last)
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
        assert_eq!(config.run_cids, 20..=99);
    }

    #[test]
    fn flags_override_the_defaults() {
        let config = parse_args(
            [
                "--port",
                "9000",
                "--kernel",
                "/k",
                "--initrd",
                "/i",
                "--run-cids",
                "1000-1099",
            ]
            .map(String::from)
            .to_vec(),
        )
        .expect("well-formed flags");
        assert_eq!(config.port, 9000);
        assert_eq!(config.kernel, PathBuf::from("/k"));
        assert_eq!(config.initrd, PathBuf::from("/i"));
        assert_eq!(config.run_cids, 1000..=1099);
    }

    #[test]
    fn the_fleet_is_off_unless_the_manifest_is_named() {
        let config = parse_args(Vec::new()).expect("no arguments is the default");
        assert_eq!(config.agents, None);
        assert_eq!(config.agents_cid_base, 100);
        assert_eq!(config.agents_memory_mib, 1024);
        assert_eq!(config.agents_vcpus, 1);
        assert_eq!(config.agents_balloon_mib, 32);
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
    fn the_balloon_flag_sets_the_agents_usable_memory() {
        let config = parse_args(["--agents-balloon-mib", "48"].map(String::from).to_vec())
            .expect("a well-formed flag");
        assert_eq!(config.agents_balloon_mib, 48);
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
            vec!["--agents-balloon-mib".to_string(), "x".to_string()],
            vec!["--run-cids".to_string(), "1099-1000".to_string()],
            vec!["--run-cids".to_string(), "2-50".to_string()],
            vec!["--run-cids".to_string(), "a-b".to_string()],
            vec!["--run-cids".to_string(), "1000".to_string()],
        ] {
            assert!(parse_args(args.clone()).is_err(), "{args:?}");
        }
    }
}
