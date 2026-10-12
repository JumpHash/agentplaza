//! `agentplaza node`: This machine as a node of a network: join with a code, status, leave.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Subcommand;
use serde_json::{json, Value};

use crate::cmd::nodes::confirm;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Join this machine to a network with a join code (from `agentplaza nodes code`), then start
    /// the node in the background.
    Join {
        /// The code the hub showed once.
        code: String,
    },
    /// Show whether this machine is joined, to which hub, and whether its node process runs.
    Status,
    /// Stop this machine's node process. With --forget, also delete its identity (asks first).
    Leave {
        /// Delete the node identity: this machine can rejoin only with a new code.
        #[arg(long)]
        forget: bool,
        /// Forget without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Run the node in this terminal (foreground) instead of in the background.
    Run,
}

pub fn run(cmd: Cmd, json: bool) -> Result<(), String> {
    match cmd {
        Cmd::Join { code } => join(&code),
        Cmd::Status => status(json),
        Cmd::Leave { forget, yes } => leave(forget, yes),
        Cmd::Run => {
            let status = Command::new(apz_link()).arg("run").status().map_err(|e| format!("apz-link run: {e}"))?;
            if status.success() { Ok(()) } else { Err(format!("apz-link run exited with {status}")) }
        }
    }
}

/// The node's identity directory (apz-link's default state; `node.json`, `node.key`,
/// `internal-token`).
fn node_dir() -> PathBuf {
    apz_os::dirs::data().join("node")
}

fn pid_file() -> PathBuf {
    apz_os::dirs::state().join("apz-link.pid")
}

fn log_file() -> PathBuf {
    apz_os::dirs::state().join("logs/apz-link.log")
}

/// The `apz-link` binary next to this one, else the one on PATH.
fn apz_link() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name(apz_os::process::exe("apz-link"));
        if sibling.exists() {
            return sibling;
        }
    }
    PathBuf::from(apz_os::process::exe("apz-link"))
}

fn join(code: &str) -> Result<(), String> {
    if let Some(pid) = read_pid().filter(|pid| alive(*pid)) {
        return Err(format!("this machine's node is already running (pid {pid}); leave it first"));
    }
    let out = Command::new(apz_link())
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
    start_detached()
}

/// Starts `apz-link run` detached, its output appended to the log; records its pid.
fn start_detached() -> Result<(), String> {
    let log = log_file();
    if let Some(dir) = log.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let stdout = OpenOptions::new().create(true).append(true).open(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let stderr = stdout.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(apz_link());
    cmd.arg("run").stdin(Stdio::null()).stdout(Stdio::from(stdout)).stderr(Stdio::from(stderr));
    apz_os::process::detach(&mut cmd);
    let child = cmd.spawn().map_err(|e| format!("apz-link run: {e}"))?;
    let pid_path = pid_file();
    if let Some(dir) = pid_path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::write(&pid_path, child.id().to_string()).map_err(|e| format!("{}: {e}", pid_path.display()))?;
    println!("node running in the background (pid {}); log: {}", child.id(), log.display());
    Ok(())
}

fn status(json: bool) -> Result<(), String> {
    let identity = fs::read_to_string(node_dir().join("node.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let pid = read_pid();
    let running = pid.is_some_and(alive);
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
        let value = json!({
            "joined": true, "hub": hub, "hub_name": name, "node": node, "cids": cids,
            "running": running, "pid": pid, "log": log_file().display().to_string(),
        });
        println!("{value}");
        return Ok(());
    }
    let process = match (running, pid) {
        (true, Some(pid)) => format!("running (pid {pid})"),
        _ => "not running (start it with: agentplaza node run, or join again)".to_string(),
    };
    let width = 8;
    println!("{:>width$}  {hub}", "hub");
    println!("{:>width$}  {name}", "name");
    println!("{:>width$}  {node}", "node");
    println!("{:>width$}  {cids}", "agents");
    println!("{:>width$}  {process}", "process");
    Ok(())
}

fn leave(forget: bool, yes: bool) -> Result<(), String> {
    if let Some(pid) = read_pid() {
        if alive(pid) {
            if !signal(pid) {
                return Err(format!("could not stop the node process (pid {pid})"));
            }
            println!("stopped the node process (pid {pid})");
        }
        let _ = fs::remove_file(pid_file());
    }
    if !forget {
        return Ok(());
    }
    if !node_dir().join("node.json").exists() {
        println!("not joined; nothing to forget");
        return Ok(());
    }
    if !yes && !confirm("Forget this machine's node identity? It can rejoin only with a new code.")? {
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

fn read_pid() -> Option<u32> {
    fs::read_to_string(pid_file()).ok()?.trim().parse().ok()
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
