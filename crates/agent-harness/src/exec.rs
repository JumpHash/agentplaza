//! The base build's commands (PLAN 16.4): the hub runs a base's steps in the VM, one [`HostMessage::Exec`] each. A
//! command runs as the session's user, [`Privilege::Agent`], in its `cwd`, which must be under the workspace, with the
//! environment the item adds to the VM's. A foreground command is answered when it ends, with its exit code and its
//! output capped. A background one (a language server, a build daemon) is answered when it has started and keeps
//! running, its output going to a log under [`LOG_DIR`], until the VM stops; the snapshot takes it as it is.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Component, Path};
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{Duration, Instant};

use apz_proto::wire::{GuestMessage, HostMessage, MAX_TOOL_OUTPUT, cap_output};

use crate::bash::{self, Privilege};
use crate::files::Workspace;
use crate::lane::Shared;

/// Runs the exec items the reader hands over, in the order they came, and answers each. Returns when the reader has
/// gone and every queued item is answered.
pub fn serve<S: Write + Send, W: Write + Send>(
    calls: Receiver<HostMessage>,
    shared: &Shared<'_, S, W>,
    workspace: &Workspace,
) {
    // A background command that asked for its port to be announced gets a watcher on a thread of its own, which ends
    // when the port listens or its wait runs out. The scope keeps the watchers until the calls have ended.
    thread::scope(|scope| {
        for message in calls {
            let HostMessage::Exec {
                id,
                argv,
                env,
                cwd,
                background,
                timeout_s,
                listen,
            } = message
            else {
                continue;
            };
            let answer = run(workspace, id, &argv, &env, &cwd, background, timeout_s);
            if let GuestMessage::ExecResult {
                ok: false, output, ..
            } = &answer
            {
                shared.note(&format!("agent-harness: exec {id} refused: {output}"));
            }
            let started = matches!(answer, GuestMessage::ExecResult { ok: true, .. });
            if let Err(err) = shared.say(&answer) {
                // The connection is broken; the reader sees that too and ends the session.
                shared.note(&format!("agent-harness: exec {id} answer not sent: {err}"));
                return;
            }
            if let (true, true, Some(port)) = (started, background, listen) {
                scope.spawn(move || announce(shared, port));
            }
        }
    });
}

/// How long a background command's port may take to listen before nobody is told (two minutes: a preview's build is
/// waited for by its own step).
const LISTEN_WAIT: Duration = Duration::from_secs(120);

/// Tells the host that `port` listens, once something accepts connections on it (`listening`, v10). Gives up after
/// [`LISTEN_WAIT`], naming it on the session's log.
fn announce<S: Write + Send, W: Write + Send>(shared: &Shared<'_, S, W>, port: u16) {
    if !wait_until_listening(port, LISTEN_WAIT) {
        shared.note(&format!(
            "agent-harness: nothing listened on port {port} within {} s",
            LISTEN_WAIT.as_secs()
        ));
        return;
    }
    if let Err(err) = shared.say(&GuestMessage::Listening { port }) {
        shared.note(&format!("agent-harness: port {port} not announced: {err}"));
    }
}

/// Whether something accepts connections on 127.0.0.1:`port` within `wait`, polled every 100 ms.
pub fn wait_until_listening(port: u16, wait: Duration) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + wait;
    loop {
        if TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Runs one exec item and returns its answer. A refused item (no command, a `cwd` outside the workspace, no timeout for
/// a foreground command) is answered `ok: false` with the reason, and nothing runs.
pub fn run(
    workspace: &Workspace,
    id: u64,
    argv: &[String],
    env: &BTreeMap<String, String>,
    cwd: &str,
    background: bool,
    timeout_s: u64,
) -> GuestMessage {
    match check(workspace, argv, cwd, background, timeout_s) {
        Ok(()) => {}
        Err(reason) => return refused(id, &reason),
    }
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    let mut builder = match bash::command(&argv[0], &args, Path::new(cwd), Privilege::Agent) {
        Ok(builder) => builder,
        Err(reason) => return refused(id, &reason),
    };
    for (name, value) in env {
        builder.env(name, value);
    }
    if background {
        return start(workspace, id, builder);
    }
    match bash::capture(builder, timeout_s) {
        Ok(bash::Captured {
            timed_out: true,
            output,
            ..
        }) => GuestMessage::ExecResult {
            id,
            ok: false,
            exit_code: None,
            output: format!("{output}[timed out after {timeout_s} s; the command was killed]"),
        },
        Ok(bash::Captured {
            status: Some(status),
            output,
            ..
        }) => GuestMessage::ExecResult {
            id,
            ok: status.success(),
            exit_code: status.code(),
            output: cap_output(&output, MAX_TOOL_OUTPUT),
        },
        Ok(bash::Captured {
            status: None,
            output,
            ..
        }) => GuestMessage::ExecResult {
            id,
            ok: false,
            exit_code: None,
            output: cap_output(
                &format!("{output}internal: the command's exit status is unknown"),
                MAX_TOOL_OUTPUT,
            ),
        },
        Err(reason) => refused(id, &reason),
    }
}

/// What makes an item unrunnable (pure): no command, a `cwd` that is not inside the workspace (or climbs out of it),
/// and a foreground command with no timeout.
fn check(
    workspace: &Workspace,
    argv: &[String],
    cwd: &str,
    background: bool,
    timeout_s: u64,
) -> Result<(), String> {
    if argv.first().is_none_or(String::is_empty) {
        return Err("the command has no program".into());
    }
    let dir = Path::new(cwd);
    let inside = dir.starts_with(workspace.dir())
        && !dir
            .components()
            .any(|part| matches!(part, Component::ParentDir));
    if !inside {
        return Err(format!(
            "the directory {cwd:?} is not inside {}",
            workspace.dir().display()
        ));
    }
    if !background && timeout_s == 0 {
        return Err("a command that runs to its end needs a timeout".into());
    }
    Ok(())
}

/// Starts a background command with its output in its log, and returns once it is running. Its exit is waited for on a
/// thread of its own, so it leaves no zombie behind when it ends.
fn start(workspace: &Workspace, id: u64, mut builder: std::process::Command) -> GuestMessage {
    let dir = workspace.plaza_dir().join("exec");
    let path = dir.join(format!("{id}.log"));
    let log = fs::create_dir_all(&dir)
        .and_then(|()| File::create(&path))
        .and_then(|file| Ok((file.try_clone()?, file)));
    let (stdout, stderr) = match log {
        Ok(files) => files,
        Err(err) => return refused(id, &format!("cannot open {}: {err}", path.display())),
    };
    builder.stdout(stdout).stderr(stderr);
    match builder.spawn() {
        Ok(mut child) => {
            thread::spawn(move || {
                let _ = child.wait();
            });
            GuestMessage::ExecResult {
                id,
                ok: true,
                exit_code: None,
                output: format!("started; its output goes to {}", path.display()),
            }
        }
        Err(err) => refused(id, &format!("cannot start the command: {err}")),
    }
}

fn refused(id: u64, reason: &str) -> GuestMessage {
    GuestMessage::ExecResult {
        id,
        ok: false,
        exit_code: None,
        output: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A workspace in a fresh scratch directory: its root is where the test's commands run.
    fn scratch(name: &str) -> Workspace {
        let root = std::env::temp_dir().join(format!("apz-exec-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("a scratch workspace");
        Workspace::at(&root)
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    fn inside(workspace: &Workspace) -> String {
        workspace.dir().display().to_string()
    }

    fn exit(id: u64, ok: bool, exit_code: Option<i32>, output: &str) -> GuestMessage {
        GuestMessage::ExecResult {
            id,
            ok,
            exit_code,
            output: output.to_string(),
        }
    }

    #[test]
    fn a_foreground_command_is_answered_with_its_exit_code_and_output() {
        let ws = scratch("exit");
        let cwd = inside(&ws);
        let none = BTreeMap::new();
        assert_eq!(
            run(
                &ws,
                1,
                &argv(&["sh", "-c", "echo hello; exit 3"]),
                &none,
                &cwd,
                false,
                30
            ),
            exit(1, false, Some(3), "hello\n")
        );
        assert_eq!(
            run(
                &ws,
                2,
                &argv(&["sh", "-c", "echo fine"]),
                &none,
                &cwd,
                false,
                30
            ),
            exit(2, true, Some(0), "fine\n")
        );
    }

    #[test]
    fn the_items_environment_is_added_to_the_command() {
        let ws = scratch("env");
        let env = BTreeMap::from([("APZ_EXEC_TEST".to_string(), "from the hub".to_string())]);
        let answer = run(
            &ws,
            3,
            &argv(&["sh", "-c", "echo $APZ_EXEC_TEST"]),
            &env,
            &inside(&ws),
            false,
            30,
        );
        assert_eq!(answer, exit(3, true, Some(0), "from the hub\n"));
    }

    #[test]
    fn an_item_that_cannot_run_is_refused_with_the_reason_and_nothing_runs() {
        let ws = scratch("refuse");
        let cwd = inside(&ws);
        let none = BTreeMap::new();
        let refused = |answer: GuestMessage| match answer {
            GuestMessage::ExecResult {
                ok: false,
                exit_code: None,
                output,
                ..
            } => output,
            other => panic!("not refused: {other:?}"),
        };
        assert!(refused(run(&ws, 4, &[], &none, &cwd, false, 30)).contains("no program"));
        assert!(
            refused(run(&ws, 5, &argv(&["true"]), &none, "/tmp", false, 30))
                .contains("is not inside")
        );
        let climbing = format!("{cwd}/../elsewhere");
        assert!(
            refused(run(&ws, 6, &argv(&["true"]), &none, &climbing, false, 30))
                .contains("is not inside")
        );
        assert!(refused(run(&ws, 7, &argv(&["true"]), &none, &cwd, false, 0)).contains("timeout"));
    }

    #[test]
    fn a_command_that_outlives_its_timeout_is_killed_and_answered_without_an_exit_code() {
        let ws = scratch("timeout");
        let answer = run(
            &ws,
            8,
            &argv(&["sleep", "30"]),
            &BTreeMap::new(),
            &inside(&ws),
            false,
            1,
        );
        match answer {
            GuestMessage::ExecResult {
                id: 8,
                ok: false,
                exit_code: None,
                output,
            } => assert!(output.contains("timed out after 1 s"), "{output}"),
            other => panic!("not timed out: {other:?}"),
        }
    }

    #[test]
    fn a_long_output_is_capped_head_and_tail() {
        let ws = scratch("cap");
        let answer = run(
            &ws,
            9,
            &argv(&["sh", "-c", "head -c 200000 /dev/zero | tr '\\0' a"]),
            &BTreeMap::new(),
            &inside(&ws),
            false,
            30,
        );
        match answer {
            GuestMessage::ExecResult {
                ok: true, output, ..
            } => {
                assert!(output.len() <= MAX_TOOL_OUTPUT, "{}", output.len());
                assert!(
                    output.contains("bytes cut"),
                    "{}",
                    &output[..200.min(output.len())]
                );
            }
            other => panic!("not run: {other:?}"),
        }
    }

    #[test]
    fn a_background_command_is_answered_at_once_and_keeps_running_after_it() {
        let ws = scratch("daemon");
        let answer = run(
            &ws,
            10,
            &argv(&["sh", "-c", "echo up; sleep 1; echo later"]),
            &BTreeMap::new(),
            &inside(&ws),
            true,
            0,
        );
        match &answer {
            GuestMessage::ExecResult {
                id: 10,
                ok: true,
                exit_code: None,
                output,
            } => assert!(
                output.starts_with("started; its output goes to"),
                "{output}"
            ),
            other => panic!("not started: {other:?}"),
        }
        // The answer came before the command's second line: the daemon is still running, and its output is in the log.
        let log = ws.plaza_dir().join("exec").join("10.log");
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let text = fs::read_to_string(&log).unwrap_or_default();
            if text.contains("later") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the daemon never finished: {text:?}"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn a_port_something_listens_on_is_announced_to_the_host_and_a_silent_one_is_not() {
        use crate::session::testing::{next, pair};
        use apz_proto::line::Lines;
        let (guest, mut host) = pair();
        let mut log = Vec::new();
        let shared = Shared::new(
            Lines::new(guest.get_ref().try_clone().expect("a second handle")),
            &mut log,
        );
        let port = {
            let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("binds");
            probe.local_addr().expect("address").port()
        };
        let listener = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            let bound = std::net::TcpListener::bind(("127.0.0.1", port)).expect("binds again");
            thread::sleep(Duration::from_secs(3));
            drop(bound);
        });
        announce(&shared, port);
        listener.join().expect("the listener ran");
        let line = next(&mut host);
        assert!(
            line.contains("\"listening\"") && line.contains(&port.to_string()),
            "{line}"
        );
    }

    #[test]
    fn a_port_nobody_listens_on_is_not_reported_listening() {
        let port = {
            let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("binds");
            probe.local_addr().expect("address").port()
        };
        let started = Instant::now();
        assert!(!wait_until_listening(port, Duration::from_millis(300)));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the wait is bounded"
        );
    }
}
