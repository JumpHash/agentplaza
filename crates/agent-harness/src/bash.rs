//! The `bash` tool: one shell command in the workspace.
//!
//! `/bin/sh -c <command>` runs in the workspace root with a cleared environment
//! (`HOME` the workspace, `PATH` the system bins), stdin closed and its own
//! process group. A root guest drops to uid and gid 1000 first. Stdout and
//! stderr are captured together and capped; the result ends with `exit <code>`.
//!
//! A command that outlives its timeout has its shell killed. The crate forbids
//! `unsafe`, so the whole process group cannot be signalled; a grandchild the
//! shell started may outlive the timeout, and its output is taken without
//! waiting for it.

use std::io::{self, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use apz_proto::wire::{MAX_TOOL_OUTPUT, cap_output};
use serde_json::Value;

use crate::files::Workspace;

const DEFAULT_TIMEOUT_S: u64 = 60;
const MAX_TIMEOUT_S: u64 = 300;
const SHELL: &str = "/bin/sh";
const SYSTEM_PATH: &str = "/bin:/usr/bin";
/// The uid and gid a root guest runs commands as.
const UNPRIVILEGED: u32 = 1000;
/// The most output kept; past it the rest is drained and dropped.
const MAX_CAPTURE_BYTES: usize = 8 << 20;
/// Room kept under the tool cap for the `exit` line.
const EXIT_RESERVE: usize = 256;
const POLL: Duration = Duration::from_millis(20);
/// How long the output is awaited once the shell is gone.
const DRAIN: Duration = Duration::from_millis(500);

/// Runs `input`'s `command` in the workspace. `Ok` when it exits 0; otherwise
/// `Err` with the output and the status, so a failure is data the model can act
/// on and counts toward the stuck check.
pub fn run(workspace: &Workspace, input: &Value) -> Result<String, String> {
    let script = match input.get("command").and_then(Value::as_str) {
        Some(script) if !script.trim().is_empty() => script,
        _ => return Err("invalid input: \"command\" must be a non-empty string".to_string()),
    };
    let timeout_s = match input.get("timeout_s") {
        None | Some(Value::Null) => DEFAULT_TIMEOUT_S,
        Some(value) => value
            .as_u64()
            .filter(|secs| *secs >= 1)
            .ok_or_else(|| "invalid input: \"timeout_s\" must be a whole number of seconds".to_string())?
            .min(MAX_TIMEOUT_S),
    };

    let root = workspace.dir();
    let (reader, writer) = io::pipe().map_err(|err| format!("internal: cannot open the output pipe: {err}"))?;
    let stderr = writer.try_clone().map_err(|err| format!("internal: cannot open the output pipe: {err}"))?;
    let mut builder = Command::new(SHELL);
    builder
        .arg("-c")
        .arg(script)
        .current_dir(root)
        .env_clear()
        .env("HOME", root)
        .env("PATH", SYSTEM_PATH)
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(stderr)
        .process_group(0);
    if runs_as_root()? {
        builder.uid(UNPRIVILEGED).gid(UNPRIVILEGED);
    }
    let mut child = builder.spawn().map_err(|err| format!("internal: cannot start the shell: {err}"))?;
    // The builder holds the pipe's write ends; dropping it lets the reader see EOF.
    drop(builder);

    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&captured);
    let (drained_tx, drained) = mpsc::channel::<()>();
    thread::spawn(move || {
        let mut reader = reader;
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    if let Ok(mut kept) = sink.lock() {
                        let room = MAX_CAPTURE_BYTES.saturating_sub(kept.len());
                        kept.extend_from_slice(&chunk[..read.min(room)]);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = drained_tx.send(());
    });

    let deadline = Instant::now() + Duration::from_secs(timeout_s);
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL),
            Ok(None) => {
                let _ = child.kill();
                break (child.wait().ok(), true);
            }
            Err(err) => return Err(format!("internal: cannot wait for the shell: {err}")),
        }
    };

    let _ = drained.recv_timeout(DRAIN);
    let bytes = captured.lock().map(|kept| kept.clone()).unwrap_or_default();
    let mut output = cap_output(&String::from_utf8_lossy(&bytes), MAX_TOOL_OUTPUT - EXIT_RESERVE);
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    if timed_out {
        return Err(format!("{output}[timed out after {timeout_s} s; the shell was killed]"));
    }
    let status = status.ok_or_else(|| format!("{output}internal: the shell's exit status is unknown"))?;
    let exit = describe_exit(status);
    if status.success() { Ok(format!("{output}{exit}")) } else { Err(format!("{output}{exit}")) }
}

/// `exit <code>`, or the signal that ended the shell.
fn describe_exit(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exit {code}"),
        (None, Some(signal)) => format!("exit: killed by signal {signal}"),
        (None, None) => "exit: unknown".to_string(),
    }
}

/// Whether this process runs as root, from the effective uid in
/// `/proc/self/status`. An unreadable status is an error, not a guess.
fn runs_as_root() -> Result<bool, String> {
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|err| format!("unavailable: cannot read the guest's uid: {err}"))?;
    let effective = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|ids| ids.split_whitespace().nth(1));
    match effective {
        Some(uid) => Ok(uid == "0"),
        None => Err("unavailable: /proc/self/status has no Uid line".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> Workspace {
        let dir = std::env::temp_dir().join(format!("apz-harness-bash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    #[test]
    fn a_command_runs_in_the_workspace_with_a_clean_environment() {
        let workspace = scratch("env");
        let script = "echo \"$HOME\"; echo \"$PATH\"; cat; echo hi";
        let output = run(&workspace, &json!({ "command": script })).expect("exit 0");
        assert_eq!(output, format!("{}\n/bin:/usr/bin\nhi\nexit 0", workspace.dir().display()));
    }

    #[test]
    fn a_nonzero_exit_is_an_error_that_carries_its_status() {
        let workspace = scratch("exit");
        let err = run(&workspace, &json!({ "command": "echo bye; exit 3" })).expect_err("exit 3");
        assert_eq!(err, "bye\nexit 3");
    }

    #[test]
    fn a_command_past_its_timeout_is_killed() {
        let workspace = scratch("timeout");
        let started = Instant::now();
        let err = run(&workspace, &json!({ "command": "sleep 30", "timeout_s": 1 })).expect_err("timed out");
        assert!(err.ends_with("[timed out after 1 s; the shell was killed]"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    }

    #[test]
    fn the_output_is_capped_to_the_tool_limit() {
        let workspace = scratch("cap");
        let output = run(&workspace, &json!({ "command": "head -c 300000 /dev/zero | tr '\\0' a" })).expect("exit 0");
        assert!(output.len() <= MAX_TOOL_OUTPUT, "{}", output.len());
        assert!(output.contains("bytes cut") && output.ends_with("exit 0"), "{}", output.len());
    }
}
