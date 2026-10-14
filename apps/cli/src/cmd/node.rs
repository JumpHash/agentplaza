//! `agentplaza node`: This machine as a node of a network: join with a code, status, start, leave.
//! A node is three processes, started in this order as the desktop app starts them: `apz-link`
//! (forwards to the hub), `vsockd` (the agent gateway for the node's guest CIDs) and `noded` (runs
//! the agents). apz-link starts neither of the others: it only reaches them on their ports.

use std::env;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::confirm;

/// The node's processes in start order; `leave` stops them in reverse.
const NODE_SERVICES: [&str; 3] = ["apz-link", "vsockd", "noded"];

/// apz-link as `run`: noded on 8899, vsockd's HTTP port 19080 (apz-link's own default, 9180, is not
/// where vsockd listens), and the forwarder on 7701, which vsockd uses as its `--apid`.
const LINK_RUN: [&str; 7] = ["run", "--noded", "8899", "--vsockd", "19080", "--forward", "7701"];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Join this machine to a network with a join code (from `agentplaza nodes code`), then start
    /// the node's processes in the background.
    Join {
        /// The code the hub showed once.
        code: String,
    },
    /// Show whether this machine is joined, to which hub, and whether its node processes run.
    Status,
    /// Start this machine's node processes again, as joined (nothing rejoins; the identity stays).
    Start,
    /// Stop this machine's node processes. With --forget, also delete its identity (asks first).
    Leave {
        /// Delete the node identity: this machine can rejoin only with a new code.
        #[arg(long)]
        forget: bool,
        /// Forget without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Run the node's forwarder (apz-link) in this terminal (foreground) instead of in the background.
    Run,
}

pub fn run(cmd: Cmd, json: bool) -> Result<(), String> {
    match cmd {
        Cmd::Join { code } => join(&code),
        Cmd::Status => status(json),
        Cmd::Start => start(),
        Cmd::Leave { forget, yes } => leave(forget, yes),
        Cmd::Run => {
            let status = Command::new(bin("apz-link")).args(LINK_RUN).status().map_err(|e| format!("apz-link run: {e}"))?;
            if status.success() { Ok(()) } else { Err(format!("apz-link run exited with {status}")) }
        }
    }
}

/// The node's identity directory (apz-link's default state; `node.json`, `node.key`,
/// `internal-token`).
fn node_dir() -> PathBuf {
    apz_os::dirs::data().join("node")
}

/// The pid file a node process writes when it starts: `<state>/<name>.pid`.
fn pid_file(name: &str) -> PathBuf {
    apz_os::dirs::state().join(format!("{name}.pid"))
}

fn log_file(name: &str) -> PathBuf {
    apz_os::dirs::state().join("logs").join(format!("{name}.log"))
}

/// The binary `name`: beside this executable (the app and the CLI ship their services together),
/// else the first one on PATH.
fn bin(name: &str) -> PathBuf {
    let file = apz_os::process::exe(name);
    match env::current_exe().ok().map(|exe| exe.with_file_name(&file)).filter(|path| path.is_file()) {
        Some(sibling) => sibling,
        None => PathBuf::from(file),
    }
}

/// One process of the node: its binary (by name), its arguments and an environment variable it needs.
struct Service {
    name: &'static str,
    args: Vec<String>,
    env: Option<(&'static str, &'static str)>,
}

/// The node's processes for CIDs `base..base+count`, in start order: the same set, flags and order
/// as the desktop app's node plan (`apps/desktop/src/console/services.rs`, `node_plan`).
fn plan(base: u64, count: u64) -> Vec<Service> {
    let uds = apz_os::dirs::vsock_sockets().to_string_lossy().into_owned();
    let roles = resource("roles/initial-roles.json", "docs/agents/initial-roles.json").to_string_lossy().into_owned();
    let token = node_dir().join("internal-token").to_string_lossy().into_owned();
    let cids = cid_list(base, count);
    let run = cid_range(base, count);
    // The per-VM process mode and its jailer are Linux-only.
    let vmm = cfg!(target_os = "linux").then_some(("APZ_VMM_SPAWN", "process"));
    vec![
        Service { name: "apz-link", args: argv(&LINK_RUN), env: None },
        Service {
            name: "vsockd",
            args: argv(&[
                "--uds-dir", &uds, "--cids", &cids, "--port", "5000", "--http-port", "19080", "--apid", "http://127.0.0.1:7701",
                "--internal-token-file", &token,
            ]),
            env: None,
        },
        Service {
            name: "noded",
            args: argv(&["--port", "8899", "--uds-dir", &uds, "--agents", &roles, "--run-cids", &run]),
            env: vmm,
        },
    ]
}

/// A data file of the app: under the installed resources when they are there, else under the
/// source repository (development).
fn resource(rel_in_share: &str, rel_in_repo: &str) -> PathBuf {
    if let Some(share) = apz_os::dirs::resources() {
        let path = share.join(rel_in_share);
        if path.exists() {
            return path;
        }
    }
    repo_dir().join(rel_in_repo)
}

/// The repository: `APZ_REPO`, else the directory three levels above this executable.
fn repo_dir() -> PathBuf {
    match env::var_os("APZ_REPO").filter(|v| !v.is_empty()) {
        Some(repo) => PathBuf::from(repo),
        None => env::current_exe().ok().and_then(|exe| exe.ancestors().nth(3).map(Path::to_path_buf)).unwrap_or_default(),
    }
}

/// The CID range this machine was given when it joined (`cid_base`, `cid_count` in `node.json`).
fn node_identity() -> Option<(u64, u64)> {
    let node: Value = serde_json::from_str(&fs::read_to_string(node_dir().join("node.json")).ok()?).ok()?;
    let base = node.get("cid_base")?.as_u64()?;
    let count = node.get("cid_count")?.as_u64()?;
    (count > 0).then_some((base, count))
}

/// `base, base+1, …, base+count-1` as a comma list (`vsockd --cids`).
fn cid_list(base: u64, count: u64) -> String {
    (base..base + count).map(|cid| cid.to_string()).collect::<Vec<_>>().join(",")
}

/// `A-B` (`noded --run-cids`) for `count` CIDs from `base`.
fn cid_range(base: u64, count: u64) -> String {
    format!("{}-{}", base, base + count.saturating_sub(1))
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

fn join(code: &str) -> Result<(), String> {
    if let Some((name, pid)) = NODE_SERVICES.iter().find_map(|name| running_pid(name).map(|pid| (*name, pid))) {
        return Err(format!("this machine's node is already running ({name}, pid {pid}); leave it first"));
    }
    let out = Command::new(bin("apz-link"))
        .arg("join")
        .arg(code)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("apz-link: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { "apz-link join failed".into() } else { err });
    }
    print!("{}", String::from_utf8_lossy(&out.stdout));
    start()
}

/// Starts the node's processes of a machine that has joined, detached, in the desktop's order.
/// Nothing is joined again; a process already running is left as it is.
fn start() -> Result<(), String> {
    let (base, count) = node_identity()
        .ok_or_else(|| "this machine has not joined a network: join with `agentplaza node join <code>`".to_string())?;
    let mut failed = Vec::new();
    for svc in plan(base, count) {
        if let Some(pid) = running_pid(svc.name) {
            println!("{}: already running (pid {pid})", svc.name);
            continue;
        }
        match spawn(&svc) {
            Ok(pid) => println!("{}: started (pid {pid}); log: {}", svc.name, log_file(svc.name).display()),
            Err(e) => {
                println!("{}: failed ({e})", svc.name);
                failed.push(svc.name);
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("not started: {}", failed.join(", ")))
    }
}

/// Starts one process detached, its output appended to its log; records its pid.
fn spawn(svc: &Service) -> Result<u32, String> {
    let log = log_file(svc.name);
    if let Some(dir) = log.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let stdout = OpenOptions::new().create(true).append(true).open(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let stderr = stdout.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(bin(svc.name));
    cmd.args(&svc.args).stdin(Stdio::null()).stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr));
    apz_os::process::detach(&mut cmd);
    if let Some((key, value)) = svc.env {
        cmd.env(key, value);
    }
    let child = cmd.spawn().map_err(|e| format!("{}: {e}", svc.name))?;
    let pid_path = pid_file(svc.name);
    if let Some(dir) = pid_path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::write(&pid_path, child.id().to_string()).map_err(|e| format!("{}: {e}", pid_path.display()))?;
    Ok(child.id())
}

fn status(json: bool) -> Result<(), String> {
    let identity = fs::read_to_string(node_dir().join("node.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let services: Vec<(&str, Option<u32>)> = NODE_SERVICES.iter().map(|name| (*name, running_pid(name))).collect();
    let running = services.iter().all(|(_, pid)| pid.is_some());
    let Some(id) = identity else {
        if json {
            println!("{}", json!({ "joined": false }));
        } else {
            println!("not joined to a network; join with: agentplaza node join <code>");
        }
        return Ok(());
    };
    let base = id.get("cid_base").and_then(Value::as_u64).unwrap_or(0);
    let count = id.get("cid_count").and_then(Value::as_u64).unwrap_or(0);
    let cids = if count == 0 { "-".to_string() } else { format!("{}-{}", base, base + count - 1) };
    let hub = text(&id, "hub_url");
    let name = text(&id, "hub_name");
    let node = text(&id, "node_id");
    if json {
        let processes: serde_json::Map<String, Value> = services.iter().map(|(n, pid)| (n.to_string(), json!(pid))).collect();
        let value = json!({
            "joined": true, "hub": hub, "hub_name": name, "node": node, "cids": cids,
            "running": running, "pid": services[0].1, "services": processes, "log": log_file("apz-link").display().to_string(),
        });
        println!("{value}");
        return Ok(());
    }
    let mut process = services
        .iter()
        .map(|(n, pid)| match pid {
            Some(pid) => format!("{n} running (pid {pid})"),
            None => format!("{n} not running"),
        })
        .collect::<Vec<_>>()
        .join(", ");
    if !running {
        process.push_str(" (start them with: agentplaza node start)");
    }
    let width = 8;
    println!("{:>width$}  {hub}", "hub");
    println!("{:>width$}  {name}", "name");
    println!("{:>width$}  {node}", "node");
    println!("{:>width$}  {cids}", "agents");
    println!("{:>width$}  {process}", "process");
    Ok(())
}

fn leave(forget: bool, yes: bool) -> Result<(), String> {
    let mut failed = Vec::new();
    for name in NODE_SERVICES.iter().rev() {
        let Some(pid) = read_pid(name) else { continue };
        if alive(pid) {
            if !signal(pid) {
                failed.push(format!("{name} (pid {pid})"));
                continue;
            }
            println!("stopped {name} (pid {pid})");
        }
        let _ = fs::remove_file(pid_file(name));
    }
    if !failed.is_empty() {
        return Err(format!("could not stop {}", failed.join(", ")));
    }
    if !forget {
        return Ok(());
    }
    if !node_dir().join("node.json").exists() {
        println!("not joined; nothing to forget");
        return Ok(());
    }
    if !confirm(yes, "Forget this machine's node identity? It can rejoin only with a new code.")? {
        println!("kept the node identity");
        return Ok(());
    }
    for file in ["node.json", "node.key", "internal-token"] {
        remove_if_exists(&node_dir().join(file))?;
    }
    println!("forgot this machine's node identity");
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn read_pid(name: &str) -> Option<u32> {
    fs::read_to_string(pid_file(name)).ok()?.trim().parse().ok()
}

/// A node process's pid while it is alive.
fn running_pid(name: &str) -> Option<u32> {
    read_pid(name).filter(|pid| alive(*pid))
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Whether process `pid` exists.
fn alive(pid: u32) -> bool {
    if cfg!(windows) {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .stdin(Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    } else {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Asks process `pid` to stop: `kill` on Unix, `taskkill` on Windows.
fn signal(pid: u32) -> bool {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("taskkill");
        c.args(["/PID", &pid.to_string(), "/F"]);
        c
    } else {
        let mut c = Command::new("kill");
        c.arg(pid.to_string());
        c
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_node_starts_apz_link_then_vsockd_then_noded_with_the_desktop_flags() {
        let names: Vec<&str> = plan(20, 4).iter().map(|svc| svc.name).collect();
        assert_eq!(names, NODE_SERVICES);
        let services = plan(20, 4);
        assert_eq!(services[0].args, LINK_RUN);
        assert!(services[0].args.windows(2).any(|pair| pair == ["--vsockd", "19080"]), "apz-link must reach vsockd's 19080");
        assert!(services[1].args.windows(2).any(|pair| pair == ["--cids", "20,21,22,23"]));
        assert!(services[1].args.windows(2).any(|pair| pair == ["--apid", "http://127.0.0.1:7701"]));
        assert!(services[2].args.windows(2).any(|pair| pair == ["--run-cids", "20-23"]));
    }

    #[test]
    fn cid_ranges_read_inclusive() {
        assert_eq!(cid_list(20, 3), "20,21,22");
        assert_eq!(cid_range(20, 3), "20-22");
        assert_eq!(cid_range(20, 1), "20-20");
    }

    #[test]
    fn a_pid_file_names_its_service() {
        assert!(pid_file("vsockd").ends_with("vsockd.pid"));
        assert!(log_file("noded").ends_with("logs/noded.log"));
    }
}
