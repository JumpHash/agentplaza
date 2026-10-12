//! The local services this machine runs, started and stopped by the console: in local mode the
//! hub (apid), the agent gateway (vsockd) and the node (noded), plus Postgres / Forgejo (Docker
//! containers) and Garage when installed; as a node, apz-link + vsockd + noded.

use std::env;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One local service and its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    /// "running" | "stopped" | "missing" (not installed).
    pub state: String,
    pub detail: String,
}

/// Which set of services a mode runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Hub and one node on this machine, loopback only.
    Local,
    /// A hub other machines join (TLS on, owner token, join codes).
    Hub,
    /// This machine is a node of another hub.
    Node,
}

/// The directory the sockets of apid, vsockd and noded share.
const UDS_DIR: &str = "/tmp/plaza-vsock";

/// One service of a mode: a Docker container (never stopped by the console) or a process.
struct Svc {
    name: &'static str,
    how: How,
}

enum How {
    Container,
    Process {
        /// `None` when the binary (or its config) is not installed.
        bin: Option<PathBuf>,
        args: Vec<String>,
        env: Option<(&'static str, &'static str)>,
    },
}

/// The services of `mode` and their state now.
pub fn status(mode: Mode) -> Vec<Service> {
    plan(mode, None)
        .iter()
        .map(|svc| {
            let (state, detail) = svc_state(svc);
            Service { name: svc.name.to_string(), state: state.to_string(), detail }
        })
        .collect()
}

/// Starts what `mode` needs that is not running; returns a line per service.
pub fn start(mode: Mode) -> Result<Vec<String>, String> {
    if mode == Mode::Hub {
        return start_hub("0.0.0.0", &format!("https://{}:7700", hostname()));
    }
    start_with(mode, None)
}

/// Starts a hub other machines join: apid binds `bind` with TLS and advertises `public_url`.
pub fn start_hub(bind: &str, public_url: &str) -> Result<Vec<String>, String> {
    start_with(Mode::Hub, Some((bind, public_url)))
}

/// Stops the services `mode` started (SIGTERM to the processes; containers are left running).
pub fn stop(mode: Mode) -> Result<Vec<String>, String> {
    Ok(plan(mode, None).iter().rev().map(stop_one).collect())
}

/// The owner token a local hub wrote for this machine (`<state>/owner-token`), if any.
pub fn local_owner_token() -> Option<String> {
    let text = fs::read_to_string(state_dir().join("owner-token")).ok()?;
    let token = text.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// Joins a hub as a node with a join token (runs `apz-link join`); returns the node id.
pub fn join(token: &str) -> Result<String, String> {
    let bin = bin_path("apz-link").unwrap_or_else(|| PathBuf::from(apz_os::process::exe("apz-link")));
    let out = Command::new(&bin)
        .arg("join")
        .arg(token)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("apz-link: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { "apz-link join failed".into() } else { err });
    }
    parse_join_output(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| "unexpected output from apz-link join".into())
}

fn start_with(mode: Mode, hub: Option<(&str, &str)>) -> Result<Vec<String>, String> {
    if mode == Mode::Node && node_identity().is_none() {
        return Err("join a hub first".into());
    }
    Ok(plan(mode, hub).iter().map(start_one).collect())
}

/// The services of `mode` in start order. `hub` adds the public bind arguments to apid.
fn plan(mode: Mode, hub: Option<(&str, &str)>) -> Vec<Svc> {
    if mode == Mode::Node {
        let (base, count) = node_identity().unwrap_or((0, 0));
        return node_plan(base, count);
    }
    let repo = repo_dir();
    let roles = path_arg(&repo.join("docs/agents/initial-roles.json"));
    let prompts = path_arg(&repo.join("docs/codex-draft/roles"));
    let console = path_arg(&repo.join("apps/web/dist"));
    let cids = local_cids();
    let internal = path_arg(&state_dir().join("internal.token"));
    let garage_bin = Path::new("/usr/bin/garage");
    let garage_cfg = data_dir().join("garage/garage.toml");
    let garage_ok = garage_bin.is_file() && garage_cfg.is_file();
    let garage_cfg = path_arg(&garage_cfg);

    let mut apid_args = argv(&[
        "--port", "7700", "--noded-port", "8899", "--hub-port", "19080", "--roles", &roles,
        "--prompts", &prompts, "--console", &console,
    ]);
    if let Some((bind, public_url)) = hub {
        apid_args.extend(argv(&["--bind", bind, "--tls", "--public-url", public_url]));
    }

    vec![
        Svc { name: "postgres", how: How::Container },
        Svc { name: "forgejo", how: How::Container },
        Svc {
            name: "garage",
            how: How::Process {
                bin: garage_ok.then(|| garage_bin.to_path_buf()),
                args: argv(&["-c", &garage_cfg, "server"]),
                env: None,
            },
        },
        Svc {
            name: "apid",
            how: How::Process { bin: bin_path("apid"), args: apid_args, env: None },
        },
        Svc {
            name: "vsockd",
            how: How::Process {
                bin: bin_path("vsockd"),
                args: argv(&[
                    "--uds-dir", UDS_DIR, "--cids", &cids, "--port", "5000", "--http-port",
                    "19080", "--apid", "http://127.0.0.1:7700", "--internal-token-file", &internal,
                ]),
                env: None,
            },
        },
        Svc {
            name: "noded",
            how: How::Process {
                bin: bin_path("noded"),
                args: argv(&[
                    "--port", "8899", "--uds-dir", UDS_DIR, "--agents", &roles,
                ]),
                env: Some(("APZ_VMM_SPAWN", "process")),
            },
        },
    ]
}

/// The node's services: apz-link forwards to the hub, vsockd and noded serve the node's CIDs.
fn node_plan(base: u64, count: u64) -> Vec<Svc> {
    let roles = path_arg(&repo_dir().join("docs/agents/initial-roles.json"));
    let token = path_arg(&data_dir().join("node/internal-token"));
    let cids = cid_list(base, count);
    let run = cid_range(base, count);
    vec![
        Svc {
            name: "apz-link",
            how: How::Process {
                bin: bin_path("apz-link"),
                args: argv(&["run", "--noded", "8899", "--vsockd", "19080", "--forward", "7701"]),
                env: None,
            },
        },
        Svc {
            name: "vsockd",
            how: How::Process {
                bin: bin_path("vsockd"),
                args: argv(&[
                    "--uds-dir", UDS_DIR, "--cids", &cids, "--port", "5000", "--http-port",
                    "19080", "--apid", "http://127.0.0.1:7701", "--internal-token-file", &token,
                ]),
                env: None,
            },
        },
        Svc {
            name: "noded",
            how: How::Process {
                bin: bin_path("noded"),
                args: argv(&[
                    "--port", "8899", "--uds-dir", UDS_DIR, "--agents", &roles, "--run-cids", &run,
                ]),
                env: Some(("APZ_VMM_SPAWN", "process")),
            },
        },
    ]
}

/// The CIDs of the local hub's agents: 20 through 130.
fn local_cids() -> String {
    cid_list(20, 111)
}

/// `base, base+1, …, base+count-1` as a comma list (`vsockd --cids`).
fn cid_list(base: u64, count: u64) -> String {
    (base..base + count).map(|cid| cid.to_string()).collect::<Vec<_>>().join(",")
}

/// `A-B` (`noded --run-cids`) for `count` CIDs from `base`.
fn cid_range(base: u64, count: u64) -> String {
    format!("{}-{}", base, base + count.saturating_sub(1))
}

/// The CID range this node was given when it joined: (`cid_base`, `cid_count`).
fn node_identity() -> Option<(u64, u64)> {
    let text = fs::read_to_string(data_dir().join("node/node.json")).ok()?;
    let node: serde_json::Value = serde_json::from_str(&text).ok()?;
    let base = node.get("cid_base")?.as_u64()?;
    let count = node.get("cid_count")?.as_u64()?;
    (count > 0).then_some((base, count))
}

fn svc_state(svc: &Svc) -> (&'static str, String) {
    match &svc.how {
        How::Container => match container_state(svc.name) {
            "missing" => ("missing", "docker or the container is absent".into()),
            state => (state, format!("container {}", svc.name)),
        },
        How::Process { bin, .. } => {
            if let Some(pid) = find_pids(svc.name).first() {
                return ("running", format!("pid {pid}"));
            }
            match bin {
                Some(_) => ("stopped", String::new()),
                None => ("missing", "not installed".into()),
            }
        }
    }
}

fn start_one(svc: &Svc) -> String {
    let name = svc.name;
    match &svc.how {
        How::Container => match container_state(name) {
            "running" => format!("{name}: already running"),
            "stopped" => match Command::new("docker").args(["start", name]).stdin(Stdio::null()).output() {
                Ok(out) if out.status.success() => format!("{name}: started"),
                Ok(out) => format!("{name}: failed ({})", first_line(&out.stderr)),
                Err(e) => format!("{name}: failed ({e})"),
            },
            _ => format!("{name}: not installed"),
        },
        How::Process { bin, args, env } => {
            if !find_pids(name).is_empty() {
                return format!("{name}: already running");
            }
            let Some(bin) = bin else {
                return format!("{name}: not installed");
            };
            match spawn(name, bin, args, *env) {
                Ok(pid) => format!("{name}: started (pid {pid})"),
                Err(e) => format!("{name}: failed ({e})"),
            }
        }
    }
}

fn stop_one(svc: &Svc) -> String {
    let name = svc.name;
    match &svc.how {
        How::Container => format!("{name}: left running (the container persists)"),
        How::Process { .. } => {
            let pids = find_pids(name);
            if pids.is_empty() {
                return format!("{name}: not running");
            }
            let signalled: Vec<bool> = pids.iter().map(|pid| signal(*pid)).collect();
            let list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
            if signalled.iter().all(|ok| *ok) {
                format!("{name}: stopped (pid {list})")
            } else {
                format!("{name}: could not signal pid {list}")
            }
        }
    }
}

/// Asks process `pid` to stop: `kill` on Unix, `taskkill` on Windows.
fn signal(pid: u32) -> bool {
    let mut cmd = if cfg!(windows) {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/F"]);
        cmd
    } else {
        let mut cmd = Command::new("kill");
        cmd.arg(pid.to_string());
        cmd
    };
    cmd.stdin(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// Starts a detached process: stdin closed, output appended to its log, its own process group.
fn spawn(name: &str, bin: &Path, args: &[String], env: Option<(&str, &str)>) -> Result<u32, String> {
    let logs = state_dir().join("logs");
    fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs.join(format!("{name}.log")))
        .map_err(|e| e.to_string())?;
    let errors = log.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(bin);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::from(log)).stderr(Stdio::from(errors));
    apz_os::process::detach(&mut cmd);
    if let Some((key, value)) = env {
        cmd.env(key, value);
    }
    let child = cmd.spawn().map_err(|e| e.to_string())?;
    Ok(child.id())
}

/// "running" | "stopped" | "missing" for a Docker container (missing when docker or it is absent).
fn container_state(name: &str) -> &'static str {
    match Command::new("docker")
        .args(["inspect", "-f", "{{.State.Running}}", name])
        .stdin(Stdio::null())
        .output()
    {
        Ok(out) if out.status.success() => {
            if String::from_utf8_lossy(&out.stdout).trim() == "true" {
                "running"
            } else {
                "stopped"
            }
        }
        _ => "missing",
    }
}

/// The pids whose argv[0] is `name` or ends in `/<name>`, found by scanning /proc (Linux; other
/// systems have no such listing, so none are found there).
#[cfg(target_os = "linux")]
fn find_pids(name: &str) -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let suffix = format!("/{name}");
    let mut pids: Vec<u32> = entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let cmdline = fs::read(entry.path().join("cmdline")).ok()?;
            let argv0 = cmdline.split(|b| *b == 0).next()?;
            let argv0 = std::str::from_utf8(argv0).ok()?;
            (argv0 == name || argv0.ends_with(&suffix)).then_some(pid)
        })
        .collect();
    pids.sort_unstable();
    pids
}

#[cfg(not(target_os = "linux"))]
fn find_pids(_name: &str) -> Vec<u32> {
    Vec::new()
}

/// The first line of `apz-link join` stdout: "joined <hub> as <node id> (agents …)".
fn parse_join_output(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut words = line.trim().strip_prefix("joined ")?.split_whitespace();
        words.by_ref().find(|word| *word == "as")?;
        words.next().map(str::to_string)
    })
}

/// The plaza's state directory (`apz_os::dirs::state`).
fn state_dir() -> PathBuf {
    apz_os::dirs::state()
}

/// The plaza's data directory (`apz_os::dirs::data`).
fn data_dir() -> PathBuf {
    apz_os::dirs::data()
}

/// The repository: `APZ_REPO`, or the directory three levels above this executable.
fn repo_dir() -> PathBuf {
    match env::var_os("APZ_REPO").filter(|v| !v.is_empty()) {
        Some(repo) => PathBuf::from(repo),
        None => env::current_exe()
            .ok()
            .and_then(|exe| exe.ancestors().nth(3).map(Path::to_path_buf))
            .unwrap_or_default(),
    }
}

/// `<repo>/target/debug/<name>` when built, else the first `<name>` on `$PATH`.
fn bin_path(name: &str) -> Option<PathBuf> {
    let file = apz_os::process::exe(name);
    let repo = repo_dir();
    if !repo.as_os_str().is_empty() {
        let built = repo.join("target/debug").join(&file);
        if built.is_file() {
            return Some(built);
        }
    }
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|dir| dir.join(&file))
            .find(|candidate| candidate.is_file())
    })
}

fn hostname() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_cids_run_from_20_to_130() {
        let cids = local_cids();
        assert!(cids.starts_with("20,21"), "starts with {cids:.12}");
        assert!(cids.ends_with("130"), "ends with {cids:?}");
    }

    #[test]
    fn proc_scan_finds_nothing_for_an_impossible_name() {
        assert!(find_pids("agentplaza-no-such-service-zz9").is_empty());
    }

    #[test]
    fn join_output_yields_the_node_id() {
        let out = "joined https://hub.example:7700 as n3 (agents 200-219)\n";
        assert_eq!(parse_join_output(out).as_deref(), Some("n3"));
    }
}
