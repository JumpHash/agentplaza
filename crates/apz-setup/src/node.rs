//! The node: the three processes a machine that joined a hub runs, as one plan that the desktop app
//! and the CLI (`agentplaza node`) both use.
//!
//! A node is apz-link (forwards to the hub), vsockd (the agent gateway for the node's guest CIDs) and
//! noded (runs the agents), started in that order. [`plan`] gives each process its arguments,
//! environment and log file; both apps build it, so they set the same flags. A process is one of the
//! plan's when its binary is named as the plan names it and its arguments are the plan's exactly
//! ([`find_plan_pids`], [`runs_plan`]). A hub's vsockd or noded on the same machine has the same
//! binary with other arguments, so it is never taken for the node's, and stopping a node never stops
//! a hub's services. Where a command line cannot be read (Windows), [`stop_pids`] stops only the pids
//! the app itself started. Spawning stays with each app (the desktop's service runner, the CLI's
//! detached spawn).

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "linux"))]
use std::process::{Command, Stdio};

use serde_json::Value;

/// The node's processes, in start order.
pub const NAMES: [&str; 3] = ["apz-link", "vsockd", "noded"];

/// apz-link as `run`: noded on 8899, vsockd's HTTP port 19080 (apz-link's own default, 9180, is not
/// where vsockd listens), and the forwarder on 7701, which vsockd uses as its `--apid`.
pub const LINK_RUN: [&str; 7] = [
    "run",
    "--noded",
    "8899",
    "--vsockd",
    "19080",
    "--forward",
    "7701",
];

/// One process of the plan: its binary (by name), its exact arguments and an environment variable it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanProcess {
    pub name: &'static str,
    pub args: Vec<String>,
    pub env: Option<(&'static str, &'static str)>,
}

/// The paths the plan names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The agent roles file (`initial-roles.json`).
    pub roles: PathBuf,
    /// The directory the sockets of apid, vsockd and noded share.
    pub uds_dir: PathBuf,
    /// The internal token vsockd and noded guard their HTTP APIs with, and apz-link sends (`node/internal-token`).
    pub token: PathBuf,
}

impl Layout {
    /// This machine's layout: the plaza's directories, and the roles file of the installed app or of
    /// the source repository.
    pub fn current() -> Layout {
        Layout {
            roles: resource("roles/initial-roles.json", "docs/agents/initial-roles.json"),
            uds_dir: apz_os::dirs::vsock_sockets(),
            token: dir().join("internal-token"),
        }
    }
}

/// apz-link as the node runs it. Its arguments do not depend on the identity.
pub fn link() -> PlanProcess {
    PlanProcess {
        name: "apz-link",
        args: argv(&LINK_RUN),
        env: None,
    }
}

/// How the hub reaches a node: over the node's own tunnel (the default), or on a WireGuard mesh (Tailscale, Headscale or
/// plain WireGuard) at the node's mesh address, which the hub dials. The owner chooses; the mesh is never installed or
/// configured here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodePath {
    Tunnel,
    Mesh,
}

/// The mesh tools on this machine, found on the PATH: `tailscale` and `wg`. Detection only reads: nothing is run,
/// installed or configured.
pub fn mesh_tools() -> Vec<&'static str> {
    ["tailscale", "wg"]
        .into_iter()
        .filter(|tool| on_path(tool))
        .collect()
}

fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(tool).is_file()))
}

/// apz-link as the node runs it on `path`. On the mesh it also listens at its mesh address (`--mesh`), where the hub dials
/// it; a mesh node without an address is refused.
pub fn link_on(path: NodePath, mesh: Option<&str>) -> Result<PlanProcess, String> {
    Ok(PlanProcess {
        name: "apz-link",
        args: argv_for(path, mesh, &LINK_RUN)?,
        env: None,
    })
}

/// The arguments of `apz-link join` on `path`: a mesh node names its mesh address, which the hub then dials.
pub fn join_args(path: NodePath, mesh: Option<&str>) -> Result<Vec<String>, String> {
    match path {
        NodePath::Tunnel => Ok(Vec::new()),
        NodePath::Mesh => Ok(vec!["--mesh".into(), mesh_address(mesh)?.into()]),
    }
}

fn argv_for(path: NodePath, mesh: Option<&str>, base: &[&str]) -> Result<Vec<String>, String> {
    let mut args = argv(base);
    if path == NodePath::Mesh {
        args.push("--mesh".into());
        args.push(mesh_address(mesh)?.into());
    }
    Ok(args)
}

fn mesh_address(mesh: Option<&str>) -> Result<&str, String> {
    match mesh {
        Some(address) if !address.is_empty() => Ok(address),
        _ => Err("a node on the mesh needs its mesh address (host:port)".into()),
    }
}

/// The node's processes for CIDs `base..base+count`, in start order.
pub fn plan(base: u64, count: u64, layout: &Layout) -> Vec<PlanProcess> {
    let uds = path_arg(&layout.uds_dir);
    let roles = path_arg(&layout.roles);
    let token = path_arg(&layout.token);
    let cids = cid_list(base, count);
    let run = cid_range(base, count);
    // The per-VM process mode and its jailer are Linux-only.
    let vmm = cfg!(target_os = "linux").then_some(("APZ_VMM_SPAWN", "process"));
    vec![
        link(),
        PlanProcess {
            name: "vsockd",
            args: argv(&[
                "--uds-dir",
                &uds,
                "--cids",
                &cids,
                "--port",
                "5000",
                "--http-port",
                "19080",
                "--apid",
                "http://127.0.0.1:7701",
                "--internal-token-file",
                &token,
            ]),
            env: None,
        },
        PlanProcess {
            name: "noded",
            args: argv(&[
                "--port",
                "8899",
                "--uds-dir",
                &uds,
                "--agents",
                &roles,
                "--run-cids",
                &run,
                "--internal-token-file",
                &token,
            ]),
            env: vmm,
        },
    ]
}

/// The node's directory under the data directory: `node.json`, `node.key`, `internal-token`.
pub fn dir() -> PathBuf {
    apz_os::dirs::data().join("node")
}

/// The log file of a node process: `<state>/logs/<name>.log`.
pub fn log_file(name: &str) -> PathBuf {
    apz_os::dirs::state()
        .join("logs")
        .join(format!("{name}.log"))
}

/// The CID range this machine was given when it joined, from `node.json` (`cid_base`, `cid_count`).
pub fn identity() -> Option<(u64, u64)> {
    parse_identity(&fs::read_to_string(dir().join("node.json")).ok()?)
}

/// The CID range in a `node.json` text; None when it has no CIDs.
pub fn parse_identity(text: &str) -> Option<(u64, u64)> {
    let node: Value = serde_json::from_str(text).ok()?;
    let base = node.get("cid_base")?.as_u64()?;
    let count = node.get("cid_count")?.as_u64()?;
    (count > 0).then_some((base, count))
}

/// `base, base+1, …, base+count-1` as a comma list (`vsockd --cids`).
pub fn cid_list(base: u64, count: u64) -> String {
    (base..base + count)
        .map(|cid| cid.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// `A-B` (`noded --run-cids`) for `count` CIDs from `base`.
pub fn cid_range(base: u64, count: u64) -> String {
    format!("{}-{}", base, base + count.saturating_sub(1))
}

/// Whether the command line `command` (program first, then its arguments) is `process`: its program is
/// `process.name` (a path may be in front of it) and its arguments are the plan's, exactly.
///
/// Exact, not a subset: the node's noded has every argument of a hub's noded and `--run-cids` besides,
/// so a subset match would let a hub's stop take a node's process.
pub fn runs_plan(command: &[String], process: &PlanProcess) -> bool {
    match command.split_first() {
        Some((program, args)) => {
            program_is(program, process.name) && args == process.args.as_slice()
        }
        None => false,
    }
}

/// Whether `program` is `name`, or a path ending in `/name`.
fn program_is(program: &str, name: &str) -> bool {
    program == name || program.ends_with(&format!("/{name}"))
}

/// The pids of the running processes of `process`'s plan (see [`runs_plan`]). Sorted; empty when none runs.
///
/// Linux reads each `/proc/<pid>/cmdline`; macOS and the BSDs read `ps -axo pid=,args=`. Windows cannot
/// read another process's command line without new dependencies, so there this is name-only: it says
/// whether such a process runs, and [`stop_pids`] never stops from it.
#[cfg(target_os = "linux")]
pub fn find_plan_pids(process: &PlanProcess) -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let procs = entries.flatten().filter_map(|entry| {
        let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
        Some((pid, fs::read(entry.path().join("cmdline")).ok()?))
    });
    pids_running_plan(process, procs)
}

/// The pids of the running processes of `process`'s plan, from `ps` (macOS and the BSDs). Sorted.
#[cfg(all(unix, not(target_os = "linux")))]
pub fn find_plan_pids(process: &PlanProcess) -> Vec<u32> {
    let Ok(out) = Command::new("ps")
        .args(["-axo", "pid=,args="])
        .stdin(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let mut pids: Vec<u32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (pid, args) = line.trim_start().split_once(char::is_whitespace)?;
            if !ps_args_run_plan(args.trim(), process) {
                return None;
            }
            pid.parse().ok()
        })
        .collect();
    pids.sort_unstable();
    pids
}

/// Windows: the pids of `process.name` by name only (see above). Not a basis for stopping.
#[cfg(windows)]
pub fn find_plan_pids(process: &PlanProcess) -> Vec<u32> {
    named_pids(process.name)
}

/// The pids that may be stopped for `process`. Where the command line can be read, these are the plan's
/// own processes ([`find_plan_pids`]). On Windows, they are the `recorded` pids (the ones this app
/// started) that still run `process.name`: a process found by name alone is never stopped.
#[cfg(not(windows))]
pub fn stop_pids(process: &PlanProcess, _recorded: &[u32]) -> Vec<u32> {
    find_plan_pids(process)
}

/// Windows: the `recorded` pids that still run `process.name` (see the other platforms above).
#[cfg(windows)]
pub fn stop_pids(process: &PlanProcess, recorded: &[u32]) -> Vec<u32> {
    let running = named_pids(process.name);
    recorded
        .iter()
        .copied()
        .filter(|pid| running.contains(pid))
        .collect()
}

/// The pids of the processes whose binary is `name`, by name only. Sorted; empty when none runs.
///
/// Used for one purpose: to refuse starting a second copy of a service another process already runs
/// (its ports would be taken). Never to stop anything: a hub's vsockd has the same name as the node's.
#[cfg(target_os = "linux")]
pub fn named_pids(name: &str) -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let procs = entries.flatten().filter_map(|entry| {
        let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
        Some((pid, fs::read(entry.path().join("cmdline")).ok()?))
    });
    pids_running(name, procs)
}

/// The pids of processes named exactly `name`: `pgrep -x` (macOS and the BSDs). Sorted.
#[cfg(all(unix, not(target_os = "linux")))]
pub fn named_pids(name: &str) -> Vec<u32> {
    let Ok(out) = Command::new("pgrep")
        .args(["-x", name])
        .stdin(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let mut pids: Vec<u32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    pids.sort_unstable();
    pids
}

/// The pids of `<name>.exe` processes: `tasklist` as CSV ("image","pid",…), Windows. Sorted.
#[cfg(windows)]
pub fn named_pids(name: &str) -> Vec<u32> {
    let image = apz_os::process::exe(name);
    let filter = format!("IMAGENAME eq {image}");
    let Ok(out) = Command::new("tasklist")
        .args(["/FI", &filter, "/FO", "CSV", "/NH"])
        .stdin(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let mut pids: Vec<u32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split("\",\"");
            let first = fields.next()?.trim_start_matches('"');
            first
                .eq_ignore_ascii_case(&image)
                .then(|| fields.next()?.trim_end_matches('"').parse().ok())?
        })
        .collect();
    pids.sort_unstable();
    pids
}

/// The pids among `procs` (pid, and the `/proc/<pid>/cmdline` bytes) whose argv[0] is `name` or ends
/// in `/<name>`. Sorted.
#[cfg(any(target_os = "linux", test))]
fn pids_running(name: &str, procs: impl IntoIterator<Item = (u32, Vec<u8>)>) -> Vec<u32> {
    let mut pids: Vec<u32> = procs
        .into_iter()
        .filter(|(_, cmdline)| argv0_is(cmdline, name))
        .map(|(pid, _)| pid)
        .collect();
    pids.sort_unstable();
    pids
}

/// The pids among `procs` (pid, and the `/proc/<pid>/cmdline` bytes) that run `process` (see
/// [`runs_plan`]). Sorted.
#[cfg(any(target_os = "linux", test))]
fn pids_running_plan(
    process: &PlanProcess,
    procs: impl IntoIterator<Item = (u32, Vec<u8>)>,
) -> Vec<u32> {
    let mut pids: Vec<u32> = procs
        .into_iter()
        .filter(|(_, cmdline)| {
            cmdline_argv(cmdline).is_some_and(|command| runs_plan(&command, process))
        })
        .map(|(pid, _)| pid)
        .collect();
    pids.sort_unstable();
    pids
}

/// Whether the NUL-separated `cmdline` starts with a program called `name` (its path may be in front).
#[cfg(any(target_os = "linux", test))]
fn argv0_is(cmdline: &[u8], name: &str) -> bool {
    let argv0 = cmdline.split(|b| *b == 0).next().unwrap_or_default();
    match std::str::from_utf8(argv0) {
        Ok(argv0) => program_is(argv0, name),
        Err(_) => false,
    }
}

/// The arguments of a NUL-terminated `/proc/<pid>/cmdline` (one NUL after each argument, so an empty
/// argument is an empty field). None when the command line is empty or not UTF-8.
#[cfg(any(target_os = "linux", test))]
fn cmdline_argv(cmdline: &[u8]) -> Option<Vec<String>> {
    if cmdline.is_empty() {
        return None;
    }
    let body = cmdline.strip_suffix(&[0u8]).unwrap_or(cmdline);
    body.split(|b| *b == 0)
        .map(|arg| std::str::from_utf8(arg).ok().map(str::to_string))
        .collect()
}

/// Whether a `ps -o args=` line (the program, a space, then the arguments as ps joins them) is `process`.
#[cfg(any(all(unix, not(target_os = "linux")), test))]
fn ps_args_run_plan(args: &str, process: &PlanProcess) -> bool {
    let (program, rest) = args.split_once(' ').unwrap_or((args, ""));
    program_is(program, process.name) && rest == process.args.join(" ")
}

/// A data file of the app: under the installed resources when they are there, else under the source
/// repository (development).
pub fn resource(rel_in_share: &str, rel_in_repo: &str) -> PathBuf {
    if let Some(share) = apz_os::dirs::resources() {
        let path = share.join(rel_in_share);
        if path.exists() {
            return path;
        }
    }
    repo_dir().join(rel_in_repo)
}

/// The repository: `APZ_REPO`, else the directory three levels above this executable.
pub fn repo_dir() -> PathBuf {
    match std::env::var_os("APZ_REPO").filter(|v| !v.is_empty()) {
        Some(repo) => PathBuf::from(repo),
        None => std::env::current_exe()
            .ok()
            .and_then(|exe| exe.ancestors().nth(3).map(Path::to_path_buf))
            .unwrap_or_default(),
    }
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        Layout {
            roles: "/share/roles.json".into(),
            uds_dir: "/run/uds".into(),
            token: "/state/node/internal-token".into(),
        }
    }

    fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
    }

    /// The command line a process of `process`'s plan runs: its program, then its arguments.
    fn command_of(process: &PlanProcess) -> Vec<String> {
        std::iter::once(format!("/usr/bin/{}", process.name))
            .chain(process.args.iter().cloned())
            .collect()
    }

    /// The bytes of a `/proc/<pid>/cmdline` for `parts` (NUL after each argument).
    fn cmdline(parts: &[&str]) -> Vec<u8> {
        parts
            .iter()
            .flat_map(|part| part.bytes().chain(std::iter::once(0)))
            .collect()
    }

    #[test]
    fn the_plan_starts_apz_link_then_vsockd_then_noded_with_the_desktop_flags() {
        let plan = plan(20, 4, &layout());
        let names: Vec<&str> = plan.iter().map(|p| p.name).collect();
        assert_eq!(names, NAMES);
        assert_eq!(plan[0].args, LINK_RUN);
        assert!(
            plan[0]
                .args
                .windows(2)
                .any(|pair| pair == ["--vsockd", "19080"]),
            "apz-link must reach vsockd's 19080"
        );
        assert_eq!(flag(&plan[1].args, "--cids"), Some("20,21,22,23"));
        assert_eq!(flag(&plan[1].args, "--apid"), Some("http://127.0.0.1:7701"));
        assert_eq!(flag(&plan[1].args, "--http-port"), Some("19080"));
        assert_eq!(flag(&plan[1].args, "--uds-dir"), Some("/run/uds"));
        assert_eq!(
            flag(&plan[1].args, "--internal-token-file"),
            Some("/state/node/internal-token")
        );
        assert_eq!(flag(&plan[2].args, "--run-cids"), Some("20-23"));
        assert_eq!(flag(&plan[2].args, "--agents"), Some("/share/roles.json"));
        assert_eq!(flag(&plan[2].args, "--port"), Some("8899"));
        assert_eq!(
            flag(&plan[2].args, "--internal-token-file"),
            Some("/state/node/internal-token")
        );
        let vmm = cfg!(target_os = "linux").then_some(("APZ_VMM_SPAWN", "process"));
        assert_eq!(plan[2].env, vmm);
        assert_eq!(plan[0].env, None);
        assert_eq!(plan[1].env, None);
    }

    #[test]
    fn cid_ranges_read_inclusive() {
        assert_eq!(cid_list(20, 3), "20,21,22");
        assert_eq!(cid_range(20, 3), "20-22");
        assert_eq!(cid_range(20, 1), "20-20");
    }

    #[test]
    fn a_node_identity_needs_cids() {
        assert_eq!(
            parse_identity(r#"{"cid_base":20,"cid_count":4,"node_id":"n3"}"#),
            Some((20, 4))
        );
        assert_eq!(parse_identity(r#"{"cid_base":20,"cid_count":0}"#), None);
        assert_eq!(parse_identity(r#"{"cid_base":20}"#), None);
        assert_eq!(parse_identity("not json"), None);
    }

    #[test]
    fn a_proc_scan_finds_the_program_by_its_argv0_only() {
        // A fixture of /proc/<pid>/cmdline: the desktop's services and a CLI-started node run the same binaries.
        let procs = vec![
            (
                4242,
                b"/usr/bin/noded\0--port\x008899\0--run-cids\x0020-23\0".to_vec(),
            ),
            (311, b"noded\0".to_vec()),
            (900, b"/opt/tools/noded-helper\0".to_vec()),
            (901, b"/usr/bin/vsockd\0--cids\x0020,21\0".to_vec()),
            (902, Vec::new()),
            (903, b"/usr/bin/zsh\0-c\0noded\0".to_vec()),
            (904, vec![b'/', 0xff, b'/', b'n', b'o', b'd', b'e', b'd']),
            (905, b"/home/u/target/debug/apz-link\0run\0".to_vec()),
        ];
        assert_eq!(pids_running("noded", procs.clone()), vec![311, 4242]);
        assert_eq!(pids_running("vsockd", procs.clone()), vec![901]);
        assert_eq!(pids_running("apz-link", procs.clone()), vec![905]);
        assert!(pids_running("apid", procs).is_empty());
    }

    #[test]
    fn node_identity_is_matched_by_its_own_arguments_only() {
        // a_node_vsockd_is_found_by_its_arguments_and_a_hubs_is_not: a hub's vsockd on the same
        // machine is not the node's, and neither is a vsockd with one argument more.
        {
            let node_vsockd = plan(20, 4, &layout())[1].clone();
            // A hub's vsockd on the same machine: the same binary, its own CIDs and its apid on 7700.
            let hub_vsockd = PlanProcess {
                name: "vsockd",
                args: argv(&[
                    "--uds-dir",
                    "/run/uds",
                    "--cids",
                    "20,21,22,23,24",
                    "--port",
                    "5000",
                    "--http-port",
                    "19080",
                    "--apid",
                    "http://127.0.0.1:7700",
                    "--internal-token-file",
                    "/state/internal.token",
                ]),
                env: None,
            };
            let procs = vec![
                (
                    100,
                    cmdline(&[
                        "/usr/bin/vsockd",
                        "--uds-dir",
                        "/run/uds",
                        "--cids",
                        "20,21,22,23",
                        "--port",
                        "5000",
                        "--http-port",
                        "19080",
                        "--apid",
                        "http://127.0.0.1:7701",
                        "--internal-token-file",
                        "/state/node/internal-token",
                    ]),
                ),
                (
                    101,
                    cmdline(&[
                        "/usr/bin/vsockd",
                        "--uds-dir",
                        "/run/uds",
                        "--cids",
                        "20,21,22,23,24",
                        "--port",
                        "5000",
                        "--http-port",
                        "19080",
                        "--apid",
                        "http://127.0.0.1:7700",
                        "--internal-token-file",
                        "/state/internal.token",
                    ]),
                ),
                // The owner's main plaza's vsockd: its own directory and CIDs.
                (
                    102,
                    cmdline(&[
                        "vsockd",
                        "--uds-dir",
                        "/home/u/.local/state/agentplaza/vsock",
                        "--cids",
                        "200,201,202",
                    ]),
                ),
                // The node's vsockd with one argument more: not the node's.
                (
                    103,
                    cmdline(&[
                        "/usr/bin/vsockd",
                        "--uds-dir",
                        "/run/uds",
                        "--cids",
                        "20,21,22,23",
                        "--port",
                        "5000",
                        "--http-port",
                        "19080",
                        "--apid",
                        "http://127.0.0.1:7701",
                        "--internal-token-file",
                        "/state/node/internal-token",
                        "--extra",
                    ]),
                ),
            ];
            assert_eq!(pids_running_plan(&node_vsockd, procs.clone()), vec![100]);
            assert_eq!(pids_running_plan(&hub_vsockd, procs), vec![101]);
        }
        // a_hubs_noded_is_not_a_nodes_even_though_it_has_all_of_the_nodes_flags: a hub's noded
        // carries all the node's flags but --run-cids, so it must not match either way.
        {
            let node_noded = plan(20, 4, &layout())[2].clone();
            let hub_noded = PlanProcess {
                name: "noded",
                args: argv(&[
                    "--port",
                    "8899",
                    "--uds-dir",
                    "/run/uds",
                    "--agents",
                    "/share/roles.json",
                ]),
                env: None,
            };
            // The node's noded is the hub's plus --run-cids: a subset match would have stopped it from the hub.
            let node_cmd = command_of(&node_noded);
            let hub_cmd = command_of(&hub_noded);
            assert!(runs_plan(&node_cmd, &node_noded));
            assert!(!runs_plan(&node_cmd, &hub_noded));
            assert!(runs_plan(&hub_cmd, &hub_noded));
            assert!(!runs_plan(&hub_cmd, &node_noded));
            // The owner's main plaza's noded, started with its own paths.
            let procs = vec![(
                200,
                cmdline(&[
                    "noded",
                    "--port",
                    "8899",
                    "--uds-dir",
                    "/home/u/.local/state/agentplaza/vsock",
                    "--agents",
                    "/srv/roles.json",
                ]),
            )];
            assert!(pids_running_plan(&node_noded, procs.clone()).is_empty());
            assert!(pids_running_plan(&hub_noded, procs).is_empty());
        }
        // apz_link_is_the_nodes_only_with_its_own_arguments: apz-link is the node's only when
        // it runs with the node's own --noded and --vsockd ports and the forward.
        {
            let procs = vec![
                (
                    300,
                    cmdline(&[
                        "/usr/bin/apz-link",
                        "run",
                        "--noded",
                        "8899",
                        "--vsockd",
                        "19080",
                        "--forward",
                        "7701",
                    ]),
                ),
                (
                    301,
                    cmdline(&[
                        "/usr/bin/apz-link",
                        "run",
                        "--noded",
                        "8899",
                        "--vsockd",
                        "19080",
                    ]),
                ),
                (302, cmdline(&["/usr/bin/apz-link", "join", "code"])),
            ];
            assert_eq!(pids_running_plan(&link(), procs), vec![300]);
        }
    }

    #[test]
    fn a_ps_line_is_the_plans_only_with_the_same_program_and_arguments() {
        let node_vsockd = plan(20, 4, &layout())[1].clone();
        let node_line = command_of(&node_vsockd).join(" ");
        assert!(ps_args_run_plan(&node_line, &node_vsockd));
        // Another binary path in front of the same arguments is still the program.
        assert!(ps_args_run_plan(
            &node_line.replacen("/usr/bin/vsockd", "/opt/x/vsockd", 1),
            &node_vsockd
        ));
        // The hub's vsockd (apid on 7700) and a `vsockd-helper` are not.
        let hub_line = node_line.replace("127.0.0.1:7701", "127.0.0.1:7700");
        assert!(!ps_args_run_plan(&hub_line, &node_vsockd));
        assert!(!ps_args_run_plan(
            &node_line.replacen("/usr/bin/vsockd", "/usr/bin/vsockd-helper", 1),
            &node_vsockd
        ));
    }

    #[test]
    fn a_command_line_is_split_into_its_arguments() {
        assert_eq!(cmdline_argv(b""), None);
        assert_eq!(cmdline_argv(b"noded\0"), Some(vec!["noded".to_string()]));
        assert_eq!(
            cmdline_argv(b"x\0\0"),
            Some(vec!["x".to_string(), String::new()])
        );
    }

    #[test]
    fn a_mesh_node_listens_at_its_mesh_address_and_a_tunnel_node_does_not() {
        let tunnel = link_on(NodePath::Tunnel, None).expect("a tunnel node needs no address");
        assert_eq!(tunnel.args, argv(&LINK_RUN));
        let mesh = link_on(NodePath::Mesh, Some("100.64.0.7:7447")).expect("a mesh address");
        assert_eq!(
            mesh.args.last().map(String::as_str),
            Some("100.64.0.7:7447")
        );
        assert_eq!(mesh.args[mesh.args.len() - 2], "--mesh");
        assert!(
            link_on(NodePath::Mesh, None).is_err(),
            "no address, no mesh"
        );
        assert!(
            link_on(NodePath::Mesh, Some("")).is_err(),
            "an empty address is no address"
        );
    }

    #[test]
    fn the_join_names_the_mesh_address_only_on_the_mesh() {
        assert_eq!(join_args(NodePath::Tunnel, None), Ok(Vec::<String>::new()));
        assert_eq!(
            join_args(NodePath::Mesh, Some("node.tailnet:7447")),
            Ok(vec!["--mesh".to_string(), "node.tailnet:7447".to_string()])
        );
        assert!(join_args(NodePath::Mesh, None).is_err());
    }

    #[test]
    fn mesh_tools_are_only_names_found_on_the_path() {
        for tool in mesh_tools() {
            assert!(["tailscale", "wg"].contains(&tool), "{tool}");
        }
    }
}
