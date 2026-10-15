//! The `bash` tool: one shell command in the workspace; and `install_packages`,
//! which installs system packages in the VM with apk (as root, in the VM only).
//!
//! `/bin/sh -c <command>` runs in the workspace root with a cleared environment
//! (`HOME` the workspace, `PATH` the system bins), stdin closed and its own
//! process group. A root guest drops to uid and gid 1000 first. A coding engine's
//! process (see [`Privilege::Engine`]) runs as `apz-engine`, uid and gid 1100, with
//! its own home, so Unix permissions bound what the engine's own tools can do. Stdout
//! and stderr are captured together and capped; the result ends with `exit <code>`.
//!
//! A command that outlives its timeout is killed with its whole process group (the
//! shell and everything it started), through kill(1) since the crate forbids `unsafe`.
//! A root guest also kills every engine process still alive (see [`kill_engine_processes`]),
//! since one that left its process group would outlive its turn.

use std::io::{self, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use apz_proto::wire::{MAX_TOOL_OUTPUT, cap_output};
use rustix::process::{Pid, Signal, kill_process};
use serde_json::Value;

use crate::files::Workspace;
use crate::trust;

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
const AGENT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/workspace/.local/bin:/workspace/go/bin:/workspace/.npm-global/bin";
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
/// The user a coding engine runs as in a root guest: uid and gid 1100, named `apz-engine`
/// in `/etc/passwd` and `/etc/group` by [`ensure_engine_user`].
pub(crate) const ENGINE_UID: u32 = 1100;
const ENGINE_PASSWD: &str = "apz-engine:x:1100:1100:plaza coding engine:/nonexistent:/sbin/nologin";
const ENGINE_GROUP: &str = "apz-engine:x:1100:";
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
            .ok_or_else(|| {
                "invalid input: \"timeout_s\" must be a whole number of seconds".to_string()
            })?
            .min(MAX_TIMEOUT_S),
    };

    execute(
        workspace.dir(),
        SHELL,
        &["-c", script],
        timeout_s,
        Privilege::Agent,
    )
}

/// `install_packages`: installs system packages in this VM with Alpine's apk,
/// as root — inside the agent's own VM only; the host is never touched. Names
/// are checked against apk's package-name shape before anything runs.
pub fn install_packages(_workspace: &Workspace, input: &Value) -> Result<String, String> {
    let names: Vec<&str> = match input.get("packages").and_then(Value::as_array) {
        Some(list) => list
            .iter()
            .map(|v| {
                v.as_str().ok_or_else(|| {
                    "invalid input: \"packages\" must be a list of names".to_string()
                })
            })
            .collect::<Result<_, _>>()?,
        None => return Err("invalid input: \"packages\" must be a list of names".to_string()),
    };
    if names.is_empty() || names.len() > MAX_PACKAGES {
        return Err(format!(
            "invalid input: give 1 to {MAX_PACKAGES} package names"
        ));
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
    execute(
        std::path::Path::new("/"),
        apk,
        &args,
        INSTALL_TIMEOUT_S,
        Privilege::Root,
    )
}

/// apk's package-name shape: lowercase letters, digits and `._+-`, 1–64 characters.
fn package_name_ok(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.len() <= 64
        && chars.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '+' | '-')
        })
}

/// Who a command runs as: the agent (uid 1000 in a root guest), root for package installs, or a
/// coding engine. An engine's process runs as `apz-engine` (uid and gid [`ENGINE_UID`]) when the
/// harness is root, so an install script it runs never runs as root; `HOME` is the engine's own
/// folder, given by the caller. Not `Copy`, because the engine's home is a path.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Privilege {
    Agent,
    Root,
    Engine(PathBuf),
}

/// The command `program args` in `dir` as `privilege`: a cleared environment (the proxy
/// set), stdin closed, its own process group, and the agent's uid drop in a root guest.
/// The caller sets stdout and stderr.
pub(crate) fn command(
    program: &str,
    args: &[&str],
    dir: &std::path::Path,
    privilege: Privilege,
) -> Result<Command, String> {
    let mut builder = Command::new(program);
    builder.args(args).stdin(Stdio::null()).process_group(0);
    environment(&mut builder, dir, privilege)?;
    Ok(builder)
}

/// The environment and working directory every guest command runs in (see [`command`]), set on
/// a builder the caller has already made. The shared operations ([`apz_ops`]) build the same
/// environment for the IDE's runs and terminals through [`agent_env`].
pub(crate) fn environment(
    builder: &mut Command,
    dir: &std::path::Path,
    privilege: Privilege,
) -> Result<(), String> {
    builder.current_dir(dir);
    let env = match privilege {
        Privilege::Root => with_trust(with_proxy(apz_ops::Env::new("/root", ADMIN_PATH)), dir),
        Privilege::Agent => with_trust(agent_env(dir)?, dir),
        // An engine cannot read the plaza's files, so its trust variables name the system store
        // instead (see `coding_engine_env`).
        Privilege::Engine(home) => coding_engine_env(&home)?,
    };
    env.apply(builder);
    Ok(())
}

/// The guest's TLS clients trust the hub's egress CA as well as their roots (see [`crate::trust`]):
/// their variables point at the bundle, and are set only once the harness has written one.
fn with_trust(env: apz_ops::Env, dir: &std::path::Path) -> apz_ops::Env {
    trust::client_variables(&Workspace::at(dir))
        .into_iter()
        .fold(env, |env, (key, path)| {
            env.with_var(key, path.display().to_string())
        })
}

/// The environment of a coding engine's process, with `home` (its own folder, see
/// `acp::layout::engine_home`) as its `HOME`: the agent's PATH and the egress proxy. A root
/// harness drops the process to [`ENGINE_UID`]; otherwise (tests, dev) nothing is dropped.
pub(crate) fn coding_engine_env(home: &Path) -> Result<apz_ops::Env, String> {
    let uid = if runs_as_root()? {
        Some(ENGINE_UID)
    } else {
        None
    };
    let env = with_proxy(apz_ops::Env::new(home, AGENT_PATH)).with_uid(uid);
    // `home` is `<workspace>/.engines/home/<id>`: the trust bundle is the workspace's.
    let trust = home
        .ancestors()
        .nth(3)
        .map(|root| trust::engine_variables(&Workspace::at(root)))
        .unwrap_or_default();
    Ok(trust.into_iter().fold(env, |env, (key, path)| {
        env.with_var(key, path.display().to_string())
    }))
}

/// Makes sure `/etc/passwd` and `/etc/group` name the engine user, as root only and idempotently:
/// a missing line is appended, and a file is replaced through a rename so a reader sees one or the other.
/// Then, best effort, the kernel refuses hard and symbolic links that the linking user could not
/// make on its own (a link from the agent's commands to a root file would otherwise reach the engine).
pub(crate) fn ensure_engine_user() -> Result<(), String> {
    if !runs_as_root()? {
        return Ok(());
    }
    ensure_line(Path::new("/etc/passwd"), ENGINE_PASSWD)?;
    ensure_line(Path::new("/etc/group"), ENGINE_GROUP)?;
    for knob in ["protected_hardlinks", "protected_symlinks"] {
        let _ = std::fs::write(format!("/proc/sys/fs/{knob}"), "1");
    }
    Ok(())
}

fn ensure_line(path: &Path, line: &str) -> Result<(), String> {
    let current = std::fs::read_to_string(path)
        .map_err(|err| format!("unavailable: cannot read {}: {err}", path.display()))?;
    let Some(updated) = with_line(&current, line) else {
        return Ok(());
    };
    let tmp = PathBuf::from(format!("{}.apz-tmp", path.display()));
    let result = std::fs::write(&tmp, updated).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|err| format!("unavailable: cannot update {}: {err}", path.display()))
}

/// `current` with `line` appended, or `None` when a line of the same name (the text before the
/// first `:`) is already there.
fn with_line(current: &str, line: &str) -> Option<String> {
    let name = line.split(':').next().unwrap_or(line);
    if current
        .lines()
        .any(|known| known.split(':').next() == Some(name))
    {
        return None;
    }
    let mut text = current.to_string();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    Some(text)
}

/// The environment of an agent's command, with `home` as its `HOME`: the agent's PATH and the egress
/// proxy. The agent is root in its own VM (the owner's choice): the VM is the boundary, so nothing is
/// dropped. Set `AGENT_RUNS_AS_ROOT` to false to run commands as uid 1000.
pub(crate) fn agent_env(home: &std::path::Path) -> Result<apz_ops::Env, String> {
    let uid = if !AGENT_RUNS_AS_ROOT && runs_as_root()? {
        Some(UNPRIVILEGED)
    } else {
        None
    };
    let path = if AGENT_RUNS_AS_ROOT {
        AGENT_PATH
    } else {
        USER_PATH
    };
    // The egress CA's bundle reaches every command the same way, IDE runs and terminals included.
    Ok(with_trust(
        with_proxy(apz_ops::Env::new(home, path)).with_uid(uid),
        home,
    ))
}

/// The egress proxy's variables, which every guest command gets.
fn with_proxy(env: apz_ops::Env) -> apz_ops::Env {
    env.with_var("http_proxy", PROXY_URL)
        .with_var("https_proxy", PROXY_URL)
        .with_var("HTTP_PROXY", PROXY_URL)
        .with_var("HTTPS_PROXY", PROXY_URL)
}

/// Runs `program args` in `dir` with a cleared environment (the proxy set), output captured
/// and capped, killed after `timeout_s`. `Ok` on exit 0, else `Err` with the output and status.
pub(crate) fn execute(
    dir: &std::path::Path,
    program: &str,
    args: &[&str],
    timeout_s: u64,
    privilege: Privilege,
) -> Result<String, String> {
    let root = dir;
    let (reader, writer) =
        io::pipe().map_err(|err| format!("internal: cannot open the output pipe: {err}"))?;
    let stderr = writer
        .try_clone()
        .map_err(|err| format!("internal: cannot open the output pipe: {err}"))?;
    let mut builder = command(program, args, root, privilege)?;
    builder.stdout(writer).stderr(stderr);
    let mut child = builder
        .spawn()
        .map_err(|err| format!("internal: cannot start the shell: {err}"))?;
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
    let mut output = cap_output(
        &String::from_utf8_lossy(&bytes),
        MAX_TOOL_OUTPUT - EXIT_RESERVE,
    );
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    if timed_out {
        return Err(format!(
            "{output}[timed out after {timeout_s} s; the shell was killed]"
        ));
    }
    let status =
        status.ok_or_else(|| format!("{output}internal: the shell's exit status is unknown"))?;
    let exit = describe_exit(status);
    if status.success() {
        Ok(format!("{output}{exit}"))
    } else {
        Err(format!("{output}{exit}"))
    }
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

/// How many rounds the engine kill makes at most, and the pause between two of them.
const KILL_ROUNDS: usize = 20;
const KILL_PAUSE: Duration = Duration::from_millis(50);

/// Kills every engine process on the guest until a scan finds none (root only; a no-op otherwise).
/// A turn's engine dies with its process group, but one that left the group (`setsid`) outlives
/// the turn, and it could swap a folder the harness is about to walk or install into. Engine
/// processes are those with [`ENGINE_UID`] as any of their uids.
pub(crate) fn kill_engine_processes() -> Result<(), String> {
    if !runs_as_root()? {
        return Ok(());
    }
    for _ in 0..KILL_ROUNDS {
        let pids = engine_pids()?;
        if pids.is_empty() {
            return Ok(());
        }
        for pid in pids {
            // A process that exits before this signal is no longer ours to stop.
            if let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) {
                let _ = kill_process(pid, Signal::KILL);
            }
        }
        thread::sleep(KILL_PAUSE);
    }
    if engine_pids()?.is_empty() {
        return Ok(());
    }
    Err("unavailable: engine processes are still running after the kill".to_string())
}

/// The pids of the live engine processes on the guest, the harness's own excepted. A process
/// that vanishes during the scan is not one.
pub(crate) fn engine_pids() -> Result<Vec<u32>, String> {
    let me = std::process::id();
    let procs = std::fs::read_dir("/proc")
        .map_err(|err| format!("unavailable: cannot list the processes: {err}"))?;
    Ok(procs
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let status = std::fs::read_to_string(entry.path().join("status")).ok()?;
            (pid != me && is_live_engine(&status)).then_some(pid)
        })
        .collect())
}

/// Whether a `/proc/<pid>/status` is a live engine process: [`ENGINE_UID`] as one of its real,
/// effective, saved or filesystem uids, and not a zombie (a zombie is dead and only its parent
/// can reap it).
fn is_live_engine(status: &str) -> bool {
    let engine = status.lines().any(|line| {
        line.strip_prefix("Uid:").is_some_and(|ids| {
            ids.split_whitespace()
                .any(|id| id.parse::<u32>() == Ok(ENGINE_UID))
        })
    });
    let zombie = status.lines().any(|line| {
        line.strip_prefix("State:")
            .is_some_and(|state| state.trim_start().starts_with('Z'))
    });
    engine && !zombie
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(name: &str) -> Workspace {
        let dir =
            std::env::temp_dir().join(format!("apz-harness-bash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    #[test]
    fn a_command_runs_in_the_workspace_with_a_clean_environment() {
        let workspace = scratch("env");
        let script = "echo \"$HOME\"; echo \"$PATH\"; cat; echo hi";
        let output = run(&workspace, &json!({ "command": script })).expect("exit 0");
        assert_eq!(
            output,
            format!("{}\n{AGENT_PATH}\nhi\nexit 0", workspace.dir().display())
        );
    }

    #[test]
    fn the_tls_variables_are_set_only_once_the_trust_bundle_exists() {
        use std::collections::HashMap;

        let workspace = scratch("trust");
        let plaza = workspace.plaza_dir();
        let set = |privilege| -> HashMap<String, String> {
            let mut builder = Command::new("true");
            environment(&mut builder, workspace.dir(), privilege).expect("the environment");
            builder
                .get_envs()
                .filter_map(|(key, value)| {
                    Some((
                        key.to_string_lossy().into_owned(),
                        value?.to_string_lossy().into_owned(),
                    ))
                })
                .collect()
        };
        for privilege in [Privilege::Agent, Privilege::Root] {
            assert!(!set(privilege).contains_key("SSL_CERT_FILE"));
        }

        std::fs::create_dir_all(&plaza).expect("the plaza directory");
        std::fs::write(plaza.join("ca-bundle.pem"), "bundle").expect("a bundle");
        std::fs::write(plaza.join("egress-ca.pem"), "ca").expect("a CA");
        for privilege in [Privilege::Agent, Privilege::Root] {
            let env = set(privilege);
            let bundle = plaza.join("ca-bundle.pem").display().to_string();
            for key in [
                "SSL_CERT_FILE",
                "CURL_CA_BUNDLE",
                "REQUESTS_CA_BUNDLE",
                "GIT_SSL_CAINFO",
            ] {
                assert_eq!(env[key], bundle, "{key}");
            }
            assert_eq!(
                env["NODE_EXTRA_CA_CERTS"],
                plaza.join("egress-ca.pem").display().to_string()
            );
            assert_eq!(env["HTTPS_PROXY"], PROXY_URL, "the proxy stays set");
        }
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
        let err = run(
            &workspace,
            &json!({ "command": "sleep 30", "timeout_s": 1 }),
        )
        .expect_err("timed out");
        assert!(
            err.ends_with("[timed out after 1 s; the shell was killed]"),
            "{err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn the_output_is_capped_to_the_tool_limit() {
        let workspace = scratch("cap");
        let output = run(
            &workspace,
            &json!({ "command": "head -c 300000 /dev/zero | tr '\\0' a" }),
        )
        .expect("exit 0");
        assert!(output.len() <= MAX_TOOL_OUTPUT, "{}", output.len());
        assert!(
            output.contains("bytes cut") && output.ends_with("exit 0"),
            "{}",
            output.len()
        );
    }

    #[test]
    fn the_engine_user_lines_are_appended_once() {
        let passwd = "root:x:0:0:root:/root:/bin/sh\n";
        let added = with_line(passwd, ENGINE_PASSWD).expect("missing, so appended");
        assert_eq!(added, format!("{passwd}{ENGINE_PASSWD}\n"));
        assert_eq!(with_line(&added, ENGINE_PASSWD), None);
        assert_eq!(
            with_line("root:x:0:", ENGINE_GROUP),
            Some(format!("root:x:0:\n{ENGINE_GROUP}\n"))
        );
    }

    #[test]
    fn package_names_follow_apks_shape() {
        for good in [
            "python3",
            "py3-pip",
            "go",
            "nodejs",
            "build-base",
            "g++",
            "libstdc++",
            "font-noto.ttf",
        ] {
            assert!(package_name_ok(good), "{good}");
        }
        for bad in [
            "",
            "-rf",
            "Python3",
            "a b",
            "x;rm",
            "../etc",
            &"a".repeat(65),
        ] {
            assert!(!package_name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn an_engine_process_is_one_with_its_uid_in_any_field_and_not_a_zombie() {
        assert!(is_live_engine(
            "Name:\tsleep\nState:\tS (sleeping)\nUid:\t0\t1100\t0\t0\n"
        ));
        assert!(is_live_engine(
            "Uid:\t1100\t1100\t1100\t1100\nState:\tS (sleeping)\n"
        ));
        assert!(!is_live_engine(
            "Uid:\t1000\t1000\t1000\t1000\nState:\tS (sleeping)\n"
        ));
        assert!(!is_live_engine(
            "Uid:\t1100\t1100\t1100\t1100\nState:\tZ (zombie)\n"
        ));
    }

    /// A folder for a root test, removed by the test before it returns.
    fn root_scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-root-bash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch folder");
        dir
    }

    #[test]
    #[ignore = "needs root: scripts/test-harness-root.sh"]
    fn root_a_coding_engine_command_runs_as_uid_1100() {
        assert!(
            runs_as_root().expect("the uid"),
            "needs root: run scripts/test-harness-root.sh"
        );
        let dir = root_scratch("uid");
        let output = command("id", &["-u"], &dir, Privilege::Engine(dir.join("home")))
            .expect("the command")
            .output()
            .expect("it runs");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1100");
    }

    #[test]
    #[ignore = "needs root: scripts/test-harness-root.sh"]
    fn root_kill_engine_processes_stops_a_setsid_sleep() {
        assert!(
            runs_as_root().expect("the uid"),
            "needs root: run scripts/test-harness-root.sh"
        );
        let dir = root_scratch("kill");
        // The shell ends and its `setsid sleep` stays behind, as an engine that left its group does.
        let shell = command(
            "/bin/sh",
            &["-c", "setsid sleep 60 </dev/null >/dev/null 2>&1 &"],
            &dir,
            Privilege::Engine(dir.join("home")),
        )
        .expect("the command")
        .status()
        .expect("the shell runs");
        assert!(shell.success());
        assert!(
            !engine_pids().expect("the scan").is_empty(),
            "the sleep runs"
        );
        kill_engine_processes().expect("the kill");
        let left = engine_pids().expect("the scan");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(left.is_empty(), "{left:?}");
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
