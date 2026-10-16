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

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use noded::fleet::Fleet;
use noded::registry::{self, Detail};
use noded::{Config, Server, hostcheck, workspace_image};

const USAGE: &str = "\
usage: noded [--port PORT] [--kernel PATH] [--initrd PATH] [--uds-dir DIR]
             [--workspace IMAGE | --no-workspace] [--persist-dir DIR] [--agents MANIFEST]
             [--agents-cid-base N] [--agents-memory-mib N] [--agents-vcpus N]
             [--agents-balloon-mib N] [--no-snapshots]
             [--internal-token-file PATH]

  --port PORT     port to listen on, on 127.0.0.1 (default 8899)
  --kernel PATH    guest kernel image (default guest/ARCH/vmlinux of the installed resources
                   when present, else /boot/vmlinuz-linux)
  --initrd PATH    guest initrd (default guest/ARCH/initrd.gz of the installed resources when
                   present, else target/guest/initrd.x86_64.gz)
  --internal-token-file PATH
                   the machine's internal token: the API answers only requests that carry
                   it as x-apz-internal (default: none, the API is open)
  -V, --version    print the version and exit
  --uds-dir DIR    directory for the per-VM vsock Unix sockets and, with
                   --workspace, for the runs' image copies
                   (default /tmp/plaza-vsock on Linux and macOS, the state directory's
                   vsock on Windows; VM i's muxer binds DIR/vm{cid})
  --snapshot-dir DIR keep VM snapshots in DIR (default: snapshots/ under --persist-dir, else
                   under the state directory)
  --volume-dir DIR keep the shared data volumes in DIR (default: volumes/ under --persist-dir,
                   else under the state directory)
  --no-snapshots   keep no VM snapshots: sleep, checkpoint, restore, fork and export are
                   refused, and no warm templates are kept either
  --check-scale    print this host's scale-mode report (zswap, encrypted swap, reflinks in
                   the snapshot directory, cgroups, KSM, APZ_VMM_SWAP), one line per item,
                   and exit: 0 when every item is fit, 1 otherwise; the daemon does not start
  --no-warm-templates  boot new agent VMs cold instead of restoring a warm template
  --no-recover    leave a keepalive agent or sandbox that stops by itself exited, instead of
                  recovering it from its newest snapshot (or cold)
  --vm-processes   run each VM in its own apz-vmm process (jailed), with its own cgroup v2
                   limits (memory, CPU, pids) under agentplaza.slice when the user's cgroup
                   tree is delegated
  --persist-dir DIR  keep keepalive agents' workspace images in DIR/workspaces/
                     (cid-<guest_cid>.ext4): copied once, reused across restarts
                     (default: node/ under the plaza's data directory)
  --workspace IMAGE  base ext4 image of every run's writable workspace volume:
                   each run copies it once per VM into DIR/run-{id}/
                   (default: workspace.ext4 in the persist directory, made with
                   mkfs.ext4 when missing)
  --no-workspace   no workspace volume and no default persist directory: the
                   guests have no block device, as before
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
    if args.iter().any(|arg| arg == "--check-scale") {
        let rest = args
            .into_iter()
            .filter(|arg| arg != "--check-scale")
            .collect();
        return check_scale(rest);
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
    let mut config = parse_args(args)?;
    if let Some(persist) = config.persist_dir.as_deref() {
        fs::create_dir_all(persist).map_err(|err| {
            format!(
                "cannot create the persist directory {}: {err}",
                persist.display()
            )
        })?;
    }
    // The default workspace image is made here when it is missing; a failure leaves the node without a
    // workspace volume rather than without a node. An image named by `--workspace` is never made.
    let default_image = config
        .persist_dir
        .as_deref()
        .map(workspace_image::default_path);
    if let Some(image) = config
        .workspace
        .clone()
        .filter(|image| Some(image) == default_image.as_ref())
        && let Err(why) = workspace_image::ensure(&image)
    {
        eprintln!("noded: no workspace image: {why}; agents run without a workspace volume");
        config.workspace = None;
    }
    println!(
        "noded: workspace image {}, persist dir {}",
        shown(config.workspace.as_deref()),
        shown(config.persist_dir.as_deref())
    );
    // One process per VM (the flag, or `APZ_VMM_SPAWN=process` as the service plans set it): the
    // shape the jailer applies to, and the one cgroup limits need.
    if config.vm_processes {
        vmm::spawner::set_spawn_mode(vmm::spawner::SpawnMode::Process);
    }
    if matches!(
        vmm::spawner::spawn_mode(),
        Ok(vmm::spawner::SpawnMode::Process)
    ) {
        let root = vmm::cgroup::CgroupRoot::detect();
        match &root {
            Some(root) => eprintln!(
                "noded: per-VM processes, cgroup limits under {}",
                root.path().display()
            ),
            None => {
                eprintln!("noded: per-VM processes; no delegated cgroup tree, so no cgroup limits")
            }
        }
        vmm::spawner::set_cgroup_root(root);
    }
    // The VMs' snapshots and the warm templates live under the snapshot dir: a per-VM process's jail
    // binds it, so the process can write snapshots there and restore from them.
    vmm::spawner::set_snapshot_root(config.snapshot_dir.clone());
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
    // The copies of workstation disks and snapshots land in the guest directory: say once whether
    // they are reflinks (a copy is free until written) or sparse copies (a copy costs its data).
    println!(
        "noded: disk copies: {}",
        if vmm::clonefile::reflink_supported(&config.workstation.dir) {
            "reflink"
        } else {
            "sparse copy"
        }
    );
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

/// `--check-scale`: prints this host's scale-mode report and returns 0 when every item is fit. The
/// daemon does not start; the other flags name the snapshot directory the reflink item probes.
fn check_scale(args: Vec<String>) -> ExitCode {
    let config = match parse_args(args) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("noded: {message}");
            return ExitCode::FAILURE;
        }
    };
    let report = hostcheck::report(config.snapshot_dir.as_deref());
    for line in report.lines() {
        println!("{line}");
    }
    if report.ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// A path for the start line, or `none`.
fn shown(path: Option<&Path>) -> String {
    path.map_or_else(|| "none".to_string(), |path| path.display().to_string())
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
    let mut no_snapshots = false;
    let mut no_recover = false;
    let mut no_workspace = false;
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
            "--internal-token-file" => {
                config.internal_token =
                    apz_http::auth::InternalToken::file(value("--internal-token-file")?)
            }
            "--workspace" => config.workspace = Some(PathBuf::from(value("--workspace")?)),
            "--no-workspace" => no_workspace = true,
            "--persist-dir" => config.persist_dir = Some(PathBuf::from(value("--persist-dir")?)),
            "--snapshot-dir" => config.snapshot_dir = Some(PathBuf::from(value("--snapshot-dir")?)),
            "--volume-dir" => config.volume_dir = Some(PathBuf::from(value("--volume-dir")?)),
            "--no-snapshots" => no_snapshots = true,
            "--no-recover" => no_recover = true,
            "--no-warm-templates" => config.warm_templates = false,
            "--vm-processes" => config.vm_processes = true,
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
    // A keepalive agent or sandbox that stops by itself is recovered unless `--no-recover` says not.
    // The setting is the daemon's own (`Registry` reads it when a VM stops), so it is set here, as
    // the snapshot root is.
    registry::set_crash_recovery(!no_recover);
    // The persist directory and the workspace image default to the node's own data directory, unless
    // `--no-workspace` keeps the node as it was: no volume and no persist directory. The image is made
    // at start (see `run`), not here, so parsing never touches the disk.
    // Snapshots and data volumes stay where a node without `--persist-dir` always kept them (the
    // state directory), so the default persist directory below never hides an installed node's
    // checkpoints, sleeping agents, templates or volumes.
    let explicit_persist = config.persist_dir.clone();
    if no_workspace {
        if config.workspace.is_some() {
            return Err("--no-workspace and --workspace cannot both be given".to_string());
        }
    } else {
        if config.persist_dir.is_none() {
            config.persist_dir = Some(apz_os::dirs::data().join("node"));
        }
        if config.workspace.is_none() {
            config.workspace = config
                .persist_dir
                .as_deref()
                .map(workspace_image::default_path);
        }
    }
    // Snapshots go where the flag names; otherwise with the persistent state when there is one, else
    // under the node's state directory, so a node started with no state flags keeps them too.
    // `--no-snapshots` keeps none, and the warm templates live under the snapshot dir, so they go too.
    config.snapshot_dir = snapshot_dir(
        no_snapshots,
        config.snapshot_dir.take(),
        explicit_persist.as_deref(),
        &apz_os::dirs::state(),
    )?;
    // Data volumes live beside the snapshots unless the flag names a place.
    if config.volume_dir.is_none() {
        config.volume_dir = Some(match explicit_persist.as_deref() {
            Some(persist) => persist.join("volumes"),
            None => apz_os::dirs::state().join("volumes"),
        });
    }
    Ok(config)
}

/// The snapshot directory a node runs with (see [`parse_args`]): `None` with `--no-snapshots`, which
/// may not be combined with `--snapshot-dir`; the flag's directory when it is given; else
/// `persist/snapshots` under `--persist-dir`; else `snapshots` under the state directory `state`.
fn snapshot_dir(
    no_snapshots: bool,
    flag: Option<PathBuf>,
    persist: Option<&std::path::Path>,
    state: &std::path::Path,
) -> Result<Option<PathBuf>, String> {
    if no_snapshots {
        if flag.is_some() {
            return Err("--no-snapshots and --snapshot-dir cannot both be given".to_string());
        }
        return Ok(None);
    }
    Ok(Some(match (flag, persist) {
        (Some(dir), _) => dir,
        (None, Some(persist)) => persist.join("snapshots"),
        (None, None) => state.join("snapshots"),
    }))
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
    fn cli_arguments_parse_into_the_config() {
        // No arguments give the contract defaults and no fleet.
        {
            let config = parse_args(Vec::new()).expect("no arguments is the default");
            assert_eq!(config.port, 8899);
            assert_eq!(config.kernel, PathBuf::from("/boot/vmlinuz-linux"));
            assert_eq!(
                config.initrd,
                PathBuf::from("target/guest/initrd.x86_64.gz")
            );
            assert_eq!(config.run_cids, 20..=99);
            // The fleet is off unless the manifest is named.
            assert_eq!(config.agents, None);
            assert_eq!(config.agents_cid_base, 100);
            assert_eq!(config.agents_memory_mib, 1024);
            assert_eq!(config.agents_vcpus, 1);
            assert_eq!(config.agents_balloon_mib, 32);
            // The node's state and its workspace image default to the data directory.
            let node = apz_os::dirs::data().join("node");
            assert_eq!(config.persist_dir, Some(node.clone()));
            assert_eq!(config.workspace, Some(node.join("workspace.ext4")));
        }
        // Flags override the defaults.
        {
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
        // The fleet flags override their defaults.
        {
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
                    "--agents-balloon-mib",
                    "48",
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
            assert_eq!(config.agents_balloon_mib, 48);
        }
        // A bad command line is refused.
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

    #[test]
    fn the_internal_token_file_flag_names_the_token() {
        let config = parse_args(
            ["--internal-token-file", "/run/plaza/internal.token"]
                .map(String::from)
                .to_vec(),
        )
        .expect("a well-formed flag");
        assert_eq!(
            config.internal_token,
            apz_http::auth::InternalToken::file("/run/plaza/internal.token")
        );
        // Without the flag the API is open.
        let open = parse_args(Vec::new()).expect("no arguments is the default");
        assert_eq!(open.internal_token, apz_http::auth::InternalToken::none());
        assert!(parse_args(vec!["--internal-token-file".to_string()]).is_err());
    }

    fn args_of(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn storage_flags_pick_the_snapshot_and_workspace_paths() {
        // The snapshots default to the state directory, or to the persist directory when one is given.
        {
            let state = std::path::Path::new("/var/lib/apz");
            let persist = std::path::Path::new("/persist");
            assert_eq!(
                snapshot_dir(false, None, None, state),
                Ok(Some(PathBuf::from("/var/lib/apz/snapshots")))
            );
            assert_eq!(
                snapshot_dir(false, None, Some(persist), state),
                Ok(Some(PathBuf::from("/persist/snapshots")))
            );
            // The flag names the directory whatever the persist dir is.
            assert_eq!(
                snapshot_dir(false, Some(PathBuf::from("/snaps")), Some(persist), state),
                Ok(Some(PathBuf::from("/snaps")))
            );
        }
        // The snapshot flags parse into the config.
        {
            // With no flags the snapshots stay in the state directory, where an installed node keeps them,
            // even though the persist directory (for workspaces) defaults to the data directory.
            let config = parse_args(Vec::new()).expect("no arguments is the default");
            assert_eq!(
                config.snapshot_dir,
                Some(apz_os::dirs::state().join("snapshots"))
            );
            assert_eq!(
                config.volume_dir,
                Some(apz_os::dirs::state().join("volumes"))
            );
            let config = parse_args(args_of(&["--no-workspace"])).expect("--no-workspace parses");
            assert_eq!(
                config.snapshot_dir,
                Some(apz_os::dirs::state().join("snapshots"))
            );
            let config = parse_args(args_of(&["--no-snapshots"])).expect("--no-snapshots parses");
            assert_eq!(config.snapshot_dir, None);
            let config = parse_args(args_of(&["--persist-dir", "/p", "--no-snapshots"]))
                .expect("--no-snapshots with a persist dir parses");
            assert_eq!(config.snapshot_dir, None);
            let config =
                parse_args(args_of(&["--snapshot-dir", "/s"])).expect("--snapshot-dir parses");
            assert_eq!(config.snapshot_dir, Some(PathBuf::from("/s")));
            let config =
                parse_args(args_of(&["--persist-dir", "/p"])).expect("--persist-dir parses");
            assert_eq!(config.snapshot_dir, Some(PathBuf::from("/p/snapshots")));
            assert!(parse_args(args_of(&["--no-snapshots", "--snapshot-dir", "/s"])).is_err());
        }
        // The workspace defaults to an image in the persist dir.
        {
            let node = apz_os::dirs::data().join("node");
            let config = parse_args(Vec::new()).expect("no arguments is the default");
            assert_eq!(config.persist_dir, Some(node.clone()));
            assert_eq!(config.workspace, Some(node.join("workspace.ext4")));
            // An explicit persist directory moves the default image with it.
            let config =
                parse_args(args_of(&["--persist-dir", "/p"])).expect("--persist-dir parses");
            assert_eq!(config.workspace, Some(PathBuf::from("/p/workspace.ext4")));
            // An explicit image wins over the default, and the persist directory stays the default.
            let config =
                parse_args(args_of(&["--workspace", "/w.ext4"])).expect("--workspace parses");
            assert_eq!(config.workspace, Some(PathBuf::from("/w.ext4")));
            assert_eq!(config.persist_dir, Some(node));
        }
        // No workspace keeps the old behaviour.
        {
            let config = parse_args(args_of(&["--no-workspace"])).expect("--no-workspace parses");
            assert_eq!(config.workspace, None);
            assert_eq!(config.persist_dir, None);
            // An explicit persist directory is still honoured, with no workspace image in it.
            let config = parse_args(args_of(&["--no-workspace", "--persist-dir", "/p"]))
                .expect("--no-workspace with a persist dir parses");
            assert_eq!(config.workspace, None);
            assert_eq!(config.persist_dir, Some(PathBuf::from("/p")));
            assert!(parse_args(args_of(&["--no-workspace", "--workspace", "/w.ext4"])).is_err());
        }
    }
}
