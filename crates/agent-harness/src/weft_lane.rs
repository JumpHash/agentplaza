//! A task's lane in Weft, from inside the VM: checked out into `<workspace>/<repo>/` when the work starts, and
//! snapshotted after every turn that ran tools (with that turn's tool-call ids) and once more before the work ends.
//! The `weft` CLI does the work and talks to the hub through the gateway (`http://plaza.gateway/weft`), where apid
//! signs each draft with this agent's key. Without the CLI the work goes on without a lane.

use std::path::{Path, PathBuf};
use std::process::Command;

use apz_proto::wire::WorkLane;

/// The CLI, when the VM image has it.
fn cli() -> Option<PathBuf> {
    [
        "/usr/local/bin/weft",
        "/usr/bin/weft",
        "/opt/agentplaza/bin/weft",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.exists())
}

fn hub() -> String {
    std::env::var("WEFT_HUB").unwrap_or_else(|_| "http://plaza.gateway/weft".into())
}

/// Where a lane's files live in the workspace.
pub fn dir(workspace: &Path, lane: &WorkLane) -> PathBuf {
    workspace.join(&lane.repo)
}

fn run(bin: &Path, cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .env("WEFT_HUB", hub())
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() {
        Ok(text)
    } else {
        Err(format!(
            "{text} {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Lays the lane out (its latest draft, or its base).
pub fn checkout(workspace: &Path, lane: &WorkLane) -> Result<String, String> {
    let bin = cli().ok_or("no weft CLI in this VM")?;
    let dir = dir(workspace, lane);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    run(
        &bin,
        workspace,
        &["checkout", &lane.change, "--dir", &dir.to_string_lossy()],
    )
}

/// Keeps this turn as a draft.
pub fn snapshot(
    workspace: &Path,
    lane: &WorkLane,
    turn: u32,
    tool_calls: &[String],
    model: &str,
) -> Result<String, String> {
    let bin = cli().ok_or("no weft CLI in this VM")?;
    let dir = dir(workspace, lane);
    if !dir.join(".weft").exists() {
        return Err("the lane is not checked out".into());
    }
    let turn = turn.to_string();
    let mut args: Vec<&str> = vec!["snapshot", "--turn", &turn, "--model", model];
    for id in tool_calls {
        args.push("--tool-call");
        args.push(id);
    }
    run(&bin, &dir, &args)
}
