//! The `bash` tool: one shell command in the workspace; and `install_packages`,
//! which installs system packages in the VM with apk (as root, in the VM only).
//!
//! `/bin/sh -c <command>` runs in the workspace root with a cleared environment
//! (`HOME` the workspace, `PATH` the system bins), stdin closed and its own
//! process group. A root guest drops to uid and gid 1000 first. Stdout and
//! stderr are captured together and capped; the result ends with `exit <code>`.
//!
//! A command that outlives its timeout is killed with its whole process group (the
//! shell and everything it started), through kill(1) since the crate forbids `unsafe`.

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
pub(crate) const SHELL: &str = "/bin/sh";
/// The agent's PATH: the system bins, then what it installs for itself (pip
/// `--user`, `go install`, npm's global prefix under its home).
const USER_PATH: &str = "/usr/local/bin:/usr/bin:/bin:/workspace/.local/bin:/workspace/go/bin:/workspace/.npm-global/bin";
/// Whether agent commands run as root inside the VM. The VM itself (no NIC, vsock only,
/// egress through the hub, keys held by apid) is what keeps agents off the host and away
/// from each other; inside it, the agent may do anything.
pub(crate) const AGENT_RUNS_AS_ROOT: bool = true;
/// The agent's PATH as root: the system and admin bins, then what it installs for itself.
const AGENT_PATH: &str =
    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/workspace/.local/bin:/workspace/go/bin:/workspace/.npm-global/bin";
/// PATH for package installs, which run as root inside the VM.
const ADMIN_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
/// A package install may take a while (toolchains are large).
const INSTALL_TIMEOUT_S: u64 = 600;
const MAX_PACKAGES: usize = 20;
/// The egress shim's listen address and the proxy URL every guest client is given
/// (see [`crate::egress`]).
pub const PROXY_ADDR: &str = "127.0.0.1:3128";
pub const PROXY_URL: &str = "http://127.0.0.1:3128";
/// The uid and gid a root guest runs commands as.
pub(crate) const UNPRIVILEGED: u32 = 1000;
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

    execute(workspace.dir(), SHELL, &["-c", script], timeout_s, Privilege::Agent)
}

/// `install_packages`: installs system packages in this VM with Alpine's apk,
/// as root — inside the agent's own VM only; the host is never touched. Names
/// are checked against apk's package-name shape before anything runs.
pub fn install_packages(_workspace: &Workspace, input: &Value) -> Result<String, String> {
    let names: Vec<&str> = match input.get("packages").and_then(Value::as_array) {
        Some(list) => list
            .iter()
            .map(|v| v.as_str().ok_or_else(|| "invalid input: \"packages\" must be a list of names".to_string()))
            .collect::<Result<_, _>>()?,
        None => return Err("invalid input: \"packages\" must be a list of names".to_string()),
    };
    if names.is_empty() || names.len() > MAX_PACKAGES {
        return Err(format!("invalid input: give 1 to {MAX_PACKAGES} package names"));
    }
    if let Some(bad) = names.iter().find(|name| !package_name_ok(name)) {
        return Err(format!("invalid input: {bad:?} is not a package name"));
    }
    let apk = ["/sbin/apk", "/usr/sbin/apk", "/bin/apk", "/usr/bin/apk"]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
        .ok_or_else(|| "this VM has no package manager yet".to_string())?;
    let mut args = vec!["add", "--no-cache"];
    args.extend(names.iter().copied());
    execute(std::path::Path::new("/"), apk, &args, INSTALL_TIMEOUT_S, Privilege::Root)
}

/// apk's package-name shape: lowercase letters, digits and `._+-`, 1–64 characters.
fn package_name_ok(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '+' | '-'))
}

/// Who a command runs as: the agent (uid 1000 in a root guest), or root for package installs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Privilege {
    Agent,
    Root,
}

/// The command `program args` in `dir` as `privilege`: a cleared environment (the proxy
/// set), stdin closed, its own process group, and the agent's uid drop in a root guest.
/// The caller sets stdout and stderr.
pub(crate) fn command(program: &str, args: &[&str], dir: &std::path::Path, privilege: Privilege) -> Result<Command, String> {
    let mut builder = Command::new(program);
    builder
        .args(args)
        .current_dir(dir)
        .env_clear()
        .env("HOME", if privilege == Privilege::Root { std::path::Path::new("/root") } else { dir })
        .env("PATH", match privilege {
            Privilege::Root => ADMIN_PATH,
            Privilege::Agent if AGENT_RUNS_AS_ROOT => AGENT_PATH,
            Privilege::Agent => USER_PATH,
        })
        .env("http_proxy", PROXY_URL)
        .env("https_proxy", PROXY_URL)
        .env("HTTP_PROXY", PROXY_URL)
        .env("HTTPS_PROXY", PROXY_URL)
        .stdin(Stdio::null())
        .process_group(0);
    // The agent is root in its own VM (the owner's choice): the VM is the boundary, so
    // nothing is dropped. Set `AGENT_RUNS_AS_ROOT` to false to run commands as uid 1000.
    if privilege == Privilege::Agent && !AGENT_RUNS_AS_ROOT && runs_as_root()? {
        builder.uid(UNPRIVILEGED).gid(UNPRIVILEGED);
    }
    Ok(builder)
}

/// Runs `program args` in `dir` with a cleared environment (the proxy set), output captured
/// and capped, killed after `timeout_s`. `Ok` on exit 0, else `Err` with the output and status.
pub(crate) fn execute(dir: &std::path::Path, program: &str, args: &[&str], timeout_s: u64, privilege: Privilege) -> Result<String, String> {
    let root = dir;
    let (reader, writer) = io::pipe().map_err(|err| format!("internal: cannot open the output pipe: {err}"))?;
    let stderr = writer.try_clone().map_err(|err| format!("internal: cannot open the output pipe: {err}"))?;
    let mut builder = command(program, args, root, privilege)?;
    builder.stdout(writer).stderr(stderr);
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
                kill_group(&mut child);
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
pub(crate) fn runs_as_root() -> Result<bool, String> {
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
        assert_eq!(output, format!("{}\n{AGENT_PATH}\nhi\nexit 0", workspace.dir().display()));
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

    #[test]
    fn package_names_follow_apks_shape() {
        for good in ["python3", "py3-pip", "go", "nodejs", "build-base", "g++", "libstdc++", "font-noto.ttf"] {
            assert!(package_name_ok(good), "{good}");
        }
        for bad in ["", "-rf", "Python3", "a b", "x;rm", "../etc", &"a".repeat(65)] {
            assert!(!package_name_ok(bad), "{bad}");
        }
    }

}

/// Kills a command started in its own process group (`process_group(0)`, so the group id is
/// the shell's pid) together with everything it started — a dev server, a sleeping
/// grandchild. The crate forbids `unsafe`, so the group is signalled with kill(1).
pub(crate) fn kill_group(child: &mut std::process::Child) {
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{}", child.id())])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
}
