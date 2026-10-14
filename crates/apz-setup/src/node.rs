//! The node: the three processes a machine that joined a hub runs, as one plan that the desktop app
//! and the CLI (`agentplaza node`) both use.
//!
//! A node is apz-link (forwards to the hub), vsockd (the agent gateway for the node's guest CIDs) and
//! noded (runs the agents), started in that order. [`plan`] gives each process its arguments,
//! environment and log file; both apps build it, so they set the same flags. [`find_pids`] finds the
//! processes that run, by name, so a node one app started is seen by the other and neither starts a
//! second set. Spawning stays with each app (the desktop's service runner, the CLI's detached spawn).

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "linux"))]
use std::process::{Command, Stdio};

use serde_json::Value;

/// The node's processes, in start order.
pub const NAMES: [&str; 3] = ["apz-link", "vsockd", "noded"];

/// apz-link as `run`: noded on 8899, vsockd's HTTP port 19080 (apz-link's own default, 9180, is not
/// where vsockd listens), and the forwarder on 7701, which vsockd uses as its `--apid`.
pub const LINK_RUN: [&str; 7] = ["run", "--noded", "8899", "--vsockd", "19080", "--forward", "7701"];

/// One process of the node: its binary (by name), its arguments and an environment variable it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
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
    /// The internal token vsockd presents to apz-link (`node/internal-token`).
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

/// The node's processes for CIDs `base..base+count`, in start order.
pub fn plan(base: u64, count: u64, layout: &Layout) -> Vec<Process> {
    let uds = path_arg(&layout.uds_dir);
    let roles = path_arg(&layout.roles);
    let token = path_arg(&layout.token);
    let cids = cid_list(base, count);
    let run = cid_range(base, count);
    // The per-VM process mode and its jailer are Linux-only.
    let vmm = cfg!(target_os = "linux").then_some(("APZ_VMM_SPAWN", "process"));
    vec![
        Process { name: "apz-link", args: argv(&LINK_RUN), env: None },
        Process {
            name: "vsockd",
            args: argv(&[
                "--uds-dir", &uds, "--cids", &cids, "--port", "5000", "--http-port", "19080", "--apid", "http://127.0.0.1:7701",
                "--internal-token-file", &token,
            ]),
            env: None,
        },
        Process {
            name: "noded",
            args: argv(&["--port", "8899", "--uds-dir", &uds, "--agents", &roles, "--run-cids", &run]),
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
    apz_os::dirs::state().join("logs").join(format!("{name}.log"))
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
    (base..base + count).map(|cid| cid.to_string()).collect::<Vec<_>>().join(",")
}

/// `A-B` (`noded --run-cids`) for `count` CIDs from `base`.
pub fn cid_range(base: u64, count: u64) -> String {
    format!("{}-{}", base, base + count.saturating_sub(1))
}

/// The pids of the running processes named `name`: /proc on Linux, `pgrep -x` on the other Unix
/// systems, `tasklist` on Windows. Sorted; empty when none runs.
#[cfg(target_os = "linux")]
pub fn find_pids(name: &str) -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let procs = entries.flatten().filter_map(|entry| {
        let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
        Some((pid, fs::read(entry.path().join("cmdline")).ok()?))
    });
    pids_running(name, procs)
}

/// The pids of processes named exactly `name`: `pgrep -x` (macOS and the BSDs).
#[cfg(all(unix, not(target_os = "linux")))]
pub fn find_pids(name: &str) -> Vec<u32> {
    let Ok(out) = Command::new("pgrep").args(["-x", name]).stdin(Stdio::null()).output() else {
        return Vec::new();
    };
    let mut pids: Vec<u32> = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect();
    pids.sort_unstable();
    pids
}

/// The pids of `<name>.exe` processes: `tasklist` as CSV ("image","pid",…), Windows.
#[cfg(windows)]
pub fn find_pids(name: &str) -> Vec<u32> {
    let image = apz_os::process::exe(name);
    let filter = format!("IMAGENAME eq {image}");
    let Ok(out) = Command::new("tasklist").args(["/FI", &filter, "/FO", "CSV", "/NH"]).stdin(Stdio::null()).output() else {
        return Vec::new();
    };
    let mut pids: Vec<u32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split("\",\"");
            let first = fields.next()?.trim_start_matches('"');
            first.eq_ignore_ascii_case(&image).then(|| fields.next()?.trim_end_matches('"').parse().ok())?
        })
        .collect();
    pids.sort_unstable();
    pids
}

/// The pids among `procs` (pid, and the `/proc/<pid>/cmdline` bytes) whose argv[0] is `name` or ends
/// in `/<name>`. Sorted.
#[cfg(any(target_os = "linux", test))]
fn pids_running(name: &str, procs: impl IntoIterator<Item = (u32, Vec<u8>)>) -> Vec<u32> {
    let mut pids: Vec<u32> = procs.into_iter().filter(|(_, cmdline)| argv0_is(cmdline, name)).map(|(pid, _)| pid).collect();
    pids.sort_unstable();
    pids
}

/// Whether the NUL-separated `cmdline` starts with a program called `name` (its path may be in front).
#[cfg(any(target_os = "linux", test))]
fn argv0_is(cmdline: &[u8], name: &str) -> bool {
    let argv0 = cmdline.split(|b| *b == 0).next().unwrap_or_default();
    match std::str::from_utf8(argv0) {
        Ok(argv0) => argv0 == name || argv0.ends_with(&format!("/{name}")),
        Err(_) => false,
    }
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
        None => std::env::current_exe().ok().and_then(|exe| exe.ancestors().nth(3).map(Path::to_path_buf)).unwrap_or_default(),
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
        Layout { roles: "/share/roles.json".into(), uds_dir: "/run/uds".into(), token: "/state/node/internal-token".into() }
    }

    fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
    }

    #[test]
    fn the_plan_starts_apz_link_then_vsockd_then_noded_with_the_desktop_flags() {
        let plan = plan(20, 4, &layout());
        let names: Vec<&str> = plan.iter().map(|p| p.name).collect();
        assert_eq!(names, NAMES);
        assert_eq!(plan[0].args, LINK_RUN);
        assert!(plan[0].args.windows(2).any(|pair| pair == ["--vsockd", "19080"]), "apz-link must reach vsockd's 19080");
        assert_eq!(flag(&plan[1].args, "--cids"), Some("20,21,22,23"));
        assert_eq!(flag(&plan[1].args, "--apid"), Some("http://127.0.0.1:7701"));
        assert_eq!(flag(&plan[1].args, "--http-port"), Some("19080"));
        assert_eq!(flag(&plan[1].args, "--uds-dir"), Some("/run/uds"));
        assert_eq!(flag(&plan[1].args, "--internal-token-file"), Some("/state/node/internal-token"));
        assert_eq!(flag(&plan[2].args, "--run-cids"), Some("20-23"));
        assert_eq!(flag(&plan[2].args, "--agents"), Some("/share/roles.json"));
        assert_eq!(flag(&plan[2].args, "--port"), Some("8899"));
        let vmm = cfg!(target_os = "linux").then_some(("APZ_VMM_SPAWN", "process"));
        assert_eq!(plan[2].env, vmm);
        assert_eq!(plan[0].env, None);
        assert_eq!(plan[1].env, None);
    }

    #[test]
    fn the_plan_is_the_same_for_the_same_identity() {
        assert_eq!(plan(20, 4, &layout()), plan(20, 4, &layout()));
    }

    #[test]
    fn cid_ranges_read_inclusive() {
        assert_eq!(cid_list(20, 3), "20,21,22");
        assert_eq!(cid_range(20, 3), "20-22");
        assert_eq!(cid_range(20, 1), "20-20");
    }

    #[test]
    fn a_node_identity_needs_cids() {
        assert_eq!(parse_identity(r#"{"cid_base":20,"cid_count":4,"node_id":"n3"}"#), Some((20, 4)));
        assert_eq!(parse_identity(r#"{"cid_base":20,"cid_count":0}"#), None);
        assert_eq!(parse_identity(r#"{"cid_base":20}"#), None);
        assert_eq!(parse_identity("not json"), None);
    }

    #[test]
    fn a_proc_scan_finds_the_program_by_its_argv0_only() {
        // A fixture of /proc/<pid>/cmdline: the desktop's services and a CLI-started node run the same binaries.
        let procs = vec![
            (4242, b"/usr/bin/noded\0--port\x008899\0--run-cids\x0020-23\0".to_vec()),
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
    fn a_name_that_runs_nowhere_finds_no_pid() {
        assert!(find_pids("agentplaza-no-such-service-zz9").is_empty());
    }
}
