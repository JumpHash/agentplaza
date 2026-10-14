//! `agentplaza node`: This machine as a node of a network: join with a code, status, start, leave.
//! A node is three processes, started in this order as the desktop app starts them: `apz-link`
//! (forwards to the hub), `vsockd` (the agent gateway for the node's guest CIDs) and `noded` (runs
//! the agents). apz-link starts neither of the others: it only reaches them on their ports.
//!
//! The plan (names, flags, order, logs) and the process check are apz-setup's `node` module, the same
//! ones the desktop app uses. A process is this node's when it runs the plan's arguments exactly, so a
//! hub's vsockd or noded on the same machine is never taken for the node's, and `leave` never stops
//! one. Spawning is this file's own (detached, with a pid file).

use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use apz_setup::node::{self, Layout, PlanProcess, LINK_RUN};
use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::confirm;

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

/// This machine's node processes as the plan gives them: with the identity (its CIDs are in the plan's
/// arguments), else apz-link alone, whose arguments need no identity.
fn processes() -> Vec<PlanProcess> {
    match node::identity() {
        Some((base, count)) => node::plan(base, count, &Layout::current()),
        None => vec![node::link()],
    }
}

/// The pid file a node process writes when it starts: `<state>/<name>.pid`. Only Windows stops by it
/// (see `node::stop_pids`), since a command line cannot be read there.
fn pid_file(name: &str) -> PathBuf {
    apz_os::dirs::state().join(format!("{name}.pid"))
}

/// The binary `name`: beside this executable (the app and the CLI ship their services together),
/// else the first one on PATH.
fn bin(name: &str) -> PathBuf {
    let file = apz_os::process::exe(name);
    match std::env::current_exe().ok().map(|exe| exe.with_file_name(&file)).filter(|path| path.is_file()) {
        Some(sibling) => sibling,
        None => PathBuf::from(file),
    }
}

fn join(code: &str) -> Result<(), String> {
    if let Some((name, pid)) = processes().iter().find_map(|process| running_pid(process).map(|pid| (process.name, pid))) {
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

/// Starts the node's processes of a machine that has joined, detached, in the plan's order.
/// Nothing is joined again; a process of the plan already running is left as it is. A process with
/// the same name that is not the plan's (a hub's, on the same ports) is not started over: the node's
/// would fail to bind its ports.
fn start() -> Result<(), String> {
    let (base, count) = node::identity()
        .ok_or_else(|| "this machine has not joined a network: join with `agentplaza node join <code>`".to_string())?;
    let mut failed = Vec::new();
    for svc in node::plan(base, count, &Layout::current()) {
        if let Some(pid) = running_pid(&svc) {
            println!("{}: already running (pid {pid})", svc.name);
            continue;
        }
        if let Some(pid) = node::named_pids(svc.name).first() {
            println!("{}: not started: another {} runs (pid {pid}) that is not this node's", svc.name, svc.name);
            failed.push(svc.name);
            continue;
        }
        match spawn(&svc) {
            Ok(pid) => println!("{}: started (pid {pid}); log: {}", svc.name, node::log_file(svc.name).display()),
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
fn spawn(svc: &PlanProcess) -> Result<u32, String> {
    let log = node::log_file(svc.name);
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
    let identity = fs::read_to_string(node::dir().join("node.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let Some(id) = identity else {
        if json {
            println!("{}", json!({ "joined": false }));
        } else {
            println!("not joined to a network; join with: agentplaza node join <code>");
        }
        return Ok(());
    };
    let services: Vec<(&str, Option<u32>)> = processes().iter().map(|process| (process.name, running_pid(process))).collect();
    let running = services.iter().all(|(_, pid)| pid.is_some());
    let base = id.get("cid_base").and_then(Value::as_u64).unwrap_or(0);
    let count = id.get("cid_count").and_then(Value::as_u64).unwrap_or(0);
    let cids = if count == 0 { "-".to_string() } else { format!("{}-{}", base, base + count - 1) };
    let hub = text(&id, "hub_url");
    let name = text(&id, "hub_name");
    let node_id = text(&id, "node_id");
    if json {
        let processes: serde_json::Map<String, Value> = services.iter().map(|(n, pid)| (n.to_string(), json!(pid))).collect();
        let value = json!({
            "joined": true, "hub": hub, "hub_name": name, "node": node_id, "cids": cids,
            "running": running, "pid": services[0].1, "services": processes, "log": node::log_file("apz-link").display().to_string(),
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
    println!("{:>width$}  {node_id}", "node");
    println!("{:>width$}  {cids}", "agents");
    println!("{:>width$}  {process}", "process");
    Ok(())
}

/// Stops this machine's node processes, last first. Only the plan's own processes are stopped (see
/// `node::stop_pids`); a process of the same name that is not the plan's is left alone.
fn leave(forget: bool, yes: bool) -> Result<(), String> {
    let mut failed = Vec::new();
    for process in processes().iter().rev() {
        let name = process.name;
        let pids = node::stop_pids(process, &recorded_pids(name));
        if pids.is_empty() && running_pid(process).is_some() {
            println!("{name}: running, but not started by this CLI; left alone");
            failed.push(format!("{name} (not started by this CLI)"));
            continue;
        }
        let mut stopped = true;
        for pid in pids {
            if !signal(pid) {
                failed.push(format!("{name} (pid {pid})"));
                stopped = false;
                continue;
            }
            println!("stopped {name} (pid {pid})");
        }
        if stopped {
            let _ = fs::remove_file(pid_file(name));
        }
    }
    if !failed.is_empty() {
        return Err(format!("could not stop {}", failed.join(", ")));
    }
    if !forget {
        return Ok(());
    }
    let dir = node::dir();
    if !dir.join("node.json").exists() {
        println!("not joined; nothing to forget");
        return Ok(());
    }
    if !confirm(yes, "Forget this machine's node identity? It can rejoin only with a new code.")? {
        println!("kept the node identity");
        return Ok(());
    }
    for file in ["node.json", "node.key", "internal-token"] {
        remove_if_exists(&dir.join(file))?;
    }
    println!("forgot this machine's node identity");
    Ok(())
}

fn remove_if_exists(path: &std::path::Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn read_pid(name: &str) -> Option<u32> {
    fs::read_to_string(pid_file(name)).ok()?.trim().parse().ok()
}

/// The pids this CLI recorded for a node process (its pid file): what stop trusts where a command line
/// cannot be read (Windows).
fn recorded_pids(name: &str) -> Vec<u32> {
    read_pid(name).into_iter().collect()
}

/// A node process's pid while it runs, if it is the plan's.
fn running_pid(process: &PlanProcess) -> Option<u32> {
    node::find_plan_pids(process).first().copied()
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
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
    use apz_setup::node::NAMES;

    use super::*;

    fn layout() -> Layout {
        Layout { roles: "/r.json".into(), uds_dir: "/u".into(), token: "/t".into() }
    }

    #[test]
    fn the_cli_starts_the_shared_plan_in_its_order() {
        let names: Vec<&str> = node::plan(20, 4, &layout()).iter().map(|svc| svc.name).collect();
        assert_eq!(names, NAMES);
        assert_eq!(names, ["apz-link", "vsockd", "noded"]);
    }

    #[test]
    fn a_hubs_vsockd_on_this_machine_is_not_the_nodes() {
        let node_vsockd = node::plan(20, 4, &layout())[1].clone();
        // The hub's vsockd: its CIDs and its apid on 7700, not the node's 7701.
        let hub_args: Vec<String> = [
            "--uds-dir", "/u", "--cids", "20,21,22,23", "--port", "5000", "--http-port", "19080", "--apid",
            "http://127.0.0.1:7700", "--internal-token-file", "/t",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let hub_command: Vec<String> = std::iter::once("vsockd".to_string()).chain(hub_args).collect();
        assert!(!node::runs_plan(&hub_command, &node_vsockd));
    }

    #[test]
    fn a_pid_file_names_its_service() {
        assert!(pid_file("vsockd").ends_with("vsockd.pid"));
        assert!(node::log_file("noded").ends_with("logs/noded.log"));
    }
}
