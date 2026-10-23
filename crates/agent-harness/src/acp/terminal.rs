//! The engine's `terminal/*` methods: commands it runs in the VM through the client.
//!
//! The caps match the IDE's runs: at most [`MAX_TERMINALS`] at once per engine session, output kept
//! in a ring of the engine's `outputByteLimit` (capped), each command's whole process group killed on
//! `kill`, `release` and when the session ends. Commands run as the agent, in a folder inside the
//! workspace. `terminal/create` is checked by the hub (`engine.exec`) before it gets here.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read};
use std::os::unix::process::ExitStatusExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::bash::{self, Privilege};
use crate::files::{Workspace, names_plaza};

/// Terminals one engine session may have open at once.
pub const MAX_TERMINALS: usize = 4;
/// The output kept per terminal when the engine names no `outputByteLimit`.
pub const DEFAULT_OUTPUT_LIMIT: usize = 1024 * 1024;
/// The most output kept per terminal, whatever the engine asks.
pub const MAX_OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// How long `wait_for_exit` waits between checks of `cancelled`.
pub const WAIT_POLL: Duration = Duration::from_millis(50);
/// How long the output of an exited command is awaited while its pipe is still open.
const DRAIN: Duration = Duration::from_millis(500);
const DRAIN_POLL: Duration = Duration::from_millis(5);

/// One engine session's terminals.
pub struct Terminals {
    workspace: Workspace,
    /// The engine's own folder: the `HOME` its terminals run with (see `bash::Privilege::Engine`).
    home: PathBuf,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The number of the last terminal started: `term-1`, `term-2`, …
    last: u64,
    /// The terminals not yet released.
    terms: HashMap<String, Term>,
    /// Every command run in this session, in order; a terminal's `ran` index points here.
    ran: Vec<Ran>,
}

struct Term {
    child: Child,
    ring: Arc<Mutex<Ring>>,
    /// This command's index in `State::ran`.
    ran: usize,
}

struct Ran {
    command: String,
    exit: Option<i32>,
}

/// The newest output of a command, at most `limit` bytes. stdout and stderr share the pipe, so
/// both go into this one ring in the order they were written.
struct Ring {
    bytes: VecDeque<u8>,
    limit: usize,
    /// Something was dropped from the front.
    truncated: bool,
    /// The pipe has closed: no process of the command writes to it any more.
    closed: bool,
}

impl Ring {
    fn new(limit: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            limit,
            truncated: false,
            closed: false,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend(chunk);
        let excess = self.bytes.len().saturating_sub(self.limit);
        if excess > 0 {
            self.bytes.drain(..excess);
            self.truncated = true;
            // The kept tail starts on a character boundary: a character split by the cut is
            // dropped whole, so the output never opens with a stray continuation byte.
            while matches!(self.bytes.front(), Some(byte) if *byte & 0xC0 == 0x80) {
                self.bytes.pop_front();
            }
        }
    }

    fn text(&self) -> String {
        let bytes: Vec<u8> = self.bytes.iter().copied().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl State {
    /// The exit status of terminal `id` once its shell has exited (reaping it), with the exit
    /// code recorded for [`Terminals::ran`].
    fn exited(&mut self, id: &str) -> Result<Option<ExitStatus>, String> {
        let term = self.terms.get_mut(id).ok_or_else(|| no_terminal(id))?;
        let status = term
            .child
            .try_wait()
            .map_err(|err| format!("internal: cannot check the terminal: {err}"))?;
        if let Some(status) = status {
            self.ran[term.ran].exit = status.code();
        }
        Ok(status)
    }
}

impl Terminals {
    pub fn new(workspace: Workspace, home: PathBuf) -> Self {
        Self {
            workspace,
            home,
            state: Mutex::new(State::default()),
        }
    }

    /// `terminal/create {sessionId, command, args?, env?: [{name, value}], cwd?, outputByteLimit?}`
    /// → `{"terminalId": …}`. `cwd` must be inside the workspace (default: its root).
    pub fn create(&self, params: &Value) -> Result<Value, String> {
        let command = command_arg(params)?;
        let args = args_arg(params)?;
        let env = env_arg(params)?;
        let dir = self.folder_arg(params)?;
        let limit = limit_arg(params)?;
        // The project's run hooks see the command line before it starts (hooks.rs).
        let line = std::iter::once(command)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        crate::hooks::before_run(&crate::v2::lane_of(&self.workspace), &line)
            .map_err(|err| err.to_string())?;

        let mut state = locked(&self.state);
        if state.terms.len() >= MAX_TERMINALS {
            return Err(format!(
                "{MAX_TERMINALS} terminals already open; release one first"
            ));
        }
        let (reader, writer) =
            io::pipe().map_err(|err| format!("internal: cannot open the output pipe: {err}"))?;
        let stderr = writer
            .try_clone()
            .map_err(|err| format!("internal: cannot open the output pipe: {err}"))?;
        // The base environment comes from `bash::command`; the engine's variables go on top.
        let mut process =
            bash::command(command, &args, &dir, Privilege::Engine(self.home.clone()))?;
        for (name, value) in env {
            process.env(name, value);
        }
        process.stdout(writer).stderr(stderr);
        let child = process
            .spawn()
            .map_err(|err| format!("internal: cannot start the command: {err}"))?;
        // The command holds the pipe's write ends; dropping it lets the reader see the end.
        drop(process);

        let ring = Arc::new(Mutex::new(Ring::new(limit)));
        let sink = Arc::clone(&ring);
        thread::spawn(move || read_into(reader, &sink));

        let line = std::iter::once(command)
            .chain(args)
            .collect::<Vec<_>>()
            .join(" ");
        state.ran.push(Ran {
            command: line,
            exit: None,
        });
        state.last += 1;
        let id = format!("term-{}", state.last);
        let ran = state.ran.len() - 1;
        state.terms.insert(id.clone(), Term { child, ring, ran });
        Ok(json!({ "terminalId": id }))
    }

    /// `terminal/output {sessionId, terminalId}` → `{output, truncated, exitStatus?: {exitCode, signal}}`.
    pub fn output(&self, params: &Value) -> Result<Value, String> {
        let id = terminal_id(params)?;
        let ring = self.ring_of(&id)?;
        let status = locked(&self.state).exited(&id)?;
        if status.is_some() {
            drain(&ring);
        }
        let buffer = locked(&ring);
        let mut body = json!({ "output": buffer.text(), "truncated": buffer.truncated });
        if let Some(status) = status {
            body["exitStatus"] = exit_json(status);
        }
        Ok(body)
    }

    /// `terminal/wait_for_exit {sessionId, terminalId}` → `{exitCode, signal}`. Returns early with an
    /// error once `cancelled()` is true.
    pub fn wait_for_exit(
        &self,
        params: &Value,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Value, String> {
        let id = terminal_id(params)?;
        let ring = self.ring_of(&id)?;
        loop {
            if cancelled() {
                return Err("cancelled".to_string());
            }
            let status = locked(&self.state).exited(&id)?;
            if let Some(status) = status {
                drain(&ring);
                return Ok(exit_json(status));
            }
            thread::sleep(WAIT_POLL);
        }
    }

    /// `terminal/kill {sessionId, terminalId}` → `{}`: kills the command's process group; the
    /// terminal stays until released.
    pub fn kill(&self, params: &Value) -> Result<Value, String> {
        let id = terminal_id(params)?;
        let mut guard = locked(&self.state);
        let state = &mut *guard;
        let term = state.terms.get_mut(&id).ok_or_else(|| no_terminal(&id))?;
        stop(term, &mut state.ran);
        Ok(json!({}))
    }

    /// `terminal/release {sessionId, terminalId}` → `{}`: kills the command if it still runs and
    /// forgets the terminal.
    pub fn release(&self, params: &Value) -> Result<Value, String> {
        let id = terminal_id(params)?;
        let mut guard = locked(&self.state);
        let state = &mut *guard;
        let mut term = state.terms.remove(&id).ok_or_else(|| no_terminal(&id))?;
        stop(&mut term, &mut state.ran);
        Ok(json!({}))
    }

    /// Kills and forgets every terminal (the session is ending).
    pub fn release_all(&self) {
        let mut guard = locked(&self.state);
        let state = &mut *guard;
        for (_, mut term) in state.terms.drain() {
            stop(&mut term, &mut state.ran);
        }
    }

    /// The commands run so far, in order, with their exit codes (`None` while running or when killed
    /// by a signal), for the turn's summary.
    pub fn ran(&self) -> Vec<(String, Option<i32>)> {
        let mut guard = locked(&self.state);
        let ids: Vec<String> = guard.terms.keys().cloned().collect();
        for id in &ids {
            // A command that exited since its last check gets its exit code recorded now.
            let _ = guard.exited(id);
        }
        guard
            .ran
            .iter()
            .map(|ran| (ran.command.clone(), ran.exit))
            .collect()
    }

    /// The output ring of terminal `id`, so a wait can drain it without holding the state lock.
    fn ring_of(&self, id: &str) -> Result<Arc<Mutex<Ring>>, String> {
        locked(&self.state)
            .terms
            .get(id)
            .map(|term| Arc::clone(&term.ring))
            .ok_or_else(|| no_terminal(id))
    }

    /// The folder a command starts in: `cwd` when given. It must be an existing folder inside the
    /// workspace, reached without `..`, and a symlink must not lead out of the workspace.
    fn folder_arg(&self, params: &Value) -> Result<PathBuf, String> {
        let root = self.workspace.dir();
        let cwd = match params.get("cwd") {
            None | Some(Value::Null) => return Ok(root.to_path_buf()),
            Some(value) => value
                .as_str()
                .ok_or_else(|| "invalid input: \"cwd\" must be a string".to_string())?,
        };
        let refused = || {
            format!(
                "invalid input: \"cwd\" must be an existing folder inside the workspace ({})",
                root.display()
            )
        };
        let path = Path::new(cwd);
        if !path.is_absolute()
            || !path.starts_with(root)
            || path.components().any(|part| part == Component::ParentDir)
        {
            return Err(refused());
        }
        let real = std::fs::canonicalize(path).map_err(|_| refused())?;
        let real_root = std::fs::canonicalize(root)
            .map_err(|err| format!("internal: cannot open the workspace: {err}"))?;
        if !real.starts_with(&real_root) {
            return Err(refused());
        }
        // A command started in the harness's state would read and write it, and
        // the terminal tools are not meant to reach it (see `files::PLAZA_DIR`).
        // Checked on the canonical path, so a link to `.plaza` is refused too.
        if names_plaza(&real, &real_root) {
            return Err(
                "invalid input: \"cwd\" names the harness's own state; terminals do not reach it"
                    .to_string(),
            );
        }
        Ok(real)
    }
}

impl Drop for Terminals {
    fn drop(&mut self) {
        self.release_all();
    }
}

/// Kills a terminal's whole process group, waits for its shell and records the exit code.
fn stop(term: &mut Term, ran: &mut [Ran]) {
    crate::bash::kill_group(&mut term.child);
    if let Ok(status) = term.child.wait() {
        ran[term.ran].exit = status.code();
    }
}

/// Waits, briefly, for the output of an exited command: its shell is gone, but what it printed may
/// still sit in the pipe. A grandchild that keeps the pipe open holds this up no longer than DRAIN.
fn drain(ring: &Mutex<Ring>) {
    let until = Instant::now() + DRAIN;
    while !locked(ring).closed && Instant::now() < until {
        thread::sleep(DRAIN_POLL);
    }
}

/// Copies what the pipe carries into the ring until the pipe closes.
fn read_into(mut reader: io::PipeReader, ring: &Mutex<Ring>) {
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => locked(ring).push(&chunk[..read]),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    locked(ring).closed = true;
}

/// `{exitCode, signal}` of an exited command; the signal is its number, or null.
fn exit_json(status: ExitStatus) -> Value {
    json!({
        "exitCode": status.code(),
        "signal": status.signal().map(|signal| signal.to_string()),
    })
}

/// A lock whose holder panicked stays usable: the data it guards is still consistent here.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn no_terminal(id: &str) -> String {
    format!("no terminal {id}")
}

fn terminal_id(params: &Value) -> Result<String, String> {
    params
        .get("terminalId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            "invalid input: \"terminalId\" must be a terminal id such as term-1".to_string()
        })
}

/// The `command`: a non-empty string without NUL bytes.
fn command_arg(params: &Value) -> Result<&str, String> {
    match params.get("command").and_then(Value::as_str) {
        Some(command) if !command.trim().is_empty() && !command.contains('\0') => Ok(command),
        _ => Err(
            "invalid input: \"command\" must be a non-empty string without NUL bytes".to_string(),
        ),
    }
}

/// The `args`: an array of strings without NUL bytes; none when absent.
fn args_arg(params: &Value) -> Result<Vec<&str>, String> {
    match params.get("args") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item.as_str() {
                Some(arg) if !arg.contains('\0') => Ok(arg),
                _ => Err(
                    "invalid input: \"args\" must be an array of strings without NUL bytes"
                        .to_string(),
                ),
            })
            .collect(),
        Some(_) => Err("invalid input: \"args\" must be an array of strings".to_string()),
    }
}

/// The `env`: `{name, value}` pairs. A name is non-empty and has neither `=` nor NUL, since the
/// environment cannot carry them.
fn env_arg(params: &Value) -> Result<Vec<(&str, &str)>, String> {
    let items = match params.get("env") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(items)) => items,
        Some(_) => {
            return Err(
                "invalid input: \"env\" must be an array of {name, value} objects".to_string(),
            );
        }
    };
    items
        .iter()
        .map(|item| {
            let name = item.get("name").and_then(Value::as_str);
            let value = item.get("value").and_then(Value::as_str);
            match (name, value) {
                (Some(name), Some(value))
                    if !name.is_empty()
                        && !name.contains(['=', '\0'])
                        && !value.contains('\0') =>
                {
                    Ok((name, value))
                }
                _ => Err(
                    "invalid input: each \"env\" entry needs a name without \"=\" or NUL and a string value"
                        .to_string(),
                ),
            }
        })
        .collect()
}

/// The `outputByteLimit`: the default when absent, capped at [`MAX_OUTPUT_LIMIT`].
fn limit_arg(params: &Value) -> Result<usize, String> {
    match params.get("outputByteLimit") {
        None | Some(Value::Null) => Ok(DEFAULT_OUTPUT_LIMIT),
        Some(value) => value
            .as_u64()
            .map(|limit| limit.min(MAX_OUTPUT_LIMIT as u64) as usize)
            .ok_or_else(|| {
                "invalid input: \"outputByteLimit\" must be a whole number of bytes".to_string()
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::layout;

    /// A fresh folder for one test, under the system temp dir, as a workspace.
    fn workspace(name: &str) -> (Workspace, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("apz-terminal-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (Workspace::at(dir.clone()), dir)
    }

    /// `/bin/sh -c script`, as the engine would ask for it.
    fn shell(script: &str) -> Value {
        json!({"sessionId": "s", "command": "/bin/sh", "args": ["-c", script]})
    }

    fn new_id(terms: &Terminals, params: &Value) -> String {
        let created = terms.create(params).unwrap();
        created["terminalId"].as_str().unwrap().to_string()
    }

    fn wait(terms: &Terminals, id: &str) -> Result<Value, String> {
        terms.wait_for_exit(&json!({"terminalId": id}), &|| false)
    }

    #[test]
    fn create_then_wait_reports_exit_and_output() {
        let (ws, dir) = workspace("exit");
        let terms = Terminals::new(ws, std::env::temp_dir());
        let id = new_id(&terms, &shell("echo hi; exit 3"));

        assert_eq!(
            wait(&terms, &id).unwrap(),
            json!({"exitCode": 3, "signal": null})
        );
        let out = terms.output(&json!({"terminalId": id})).unwrap();
        assert_eq!(out["output"], "hi\n");
        assert_eq!(out["truncated"], false);
        assert_eq!(out["exitStatus"], json!({"exitCode": 3, "signal": null}));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn output_limit_keeps_the_tail_on_a_character_boundary() {
        // Each row: workspace name, printf argument, outputByteLimit, and the output kept.
        // "limit" keeps the last ten bytes of ASCII output.
        // "utf8" holds six bytes of "é" (two bytes each); a 3-byte cut would open on half a
        // character, so the cut moves to a character boundary.
        for (case, printf, limit, want) in [
            ("limit", "printf 0123456789abcdef", 10, "6789abcdef"),
            ("utf8", "printf 'ééé'", 3, "é"),
        ] {
            let (ws, dir) = workspace(case);
            let terms = Terminals::new(ws, std::env::temp_dir());
            let id = new_id(
                &terms,
                &json!({"command": "/bin/sh", "args": ["-c", printf], "outputByteLimit": limit}),
            );

            wait(&terms, &id).unwrap();
            let out = terms.output(&json!({"terminalId": id})).unwrap();
            assert_eq!(out["output"], want, "{case}");
            assert_eq!(out["truncated"], true, "{case}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn fifth_terminal_is_refused_until_one_is_released() {
        let (ws, dir) = workspace("cap");
        let terms = Terminals::new(ws, std::env::temp_dir());
        let ids: Vec<String> = (0..MAX_TERMINALS)
            .map(|_| new_id(&terms, &shell("/bin/sleep 30")))
            .collect();

        let err = terms.create(&shell("/bin/sleep 30")).unwrap_err();
        assert!(err.contains("already open"), "{err}");

        terms.release(&json!({"terminalId": ids[0]})).unwrap();
        new_id(&terms, &shell("/bin/sleep 30"));
        terms.release_all();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn kill_ends_a_running_command_promptly() {
        let (ws, dir) = workspace("kill");
        let terms = Terminals::new(ws, std::env::temp_dir());
        let id = new_id(&terms, &shell("/bin/sleep 30"));

        let started = Instant::now();
        terms.kill(&json!({"terminalId": id})).unwrap();
        let exit = wait(&terms, &id).unwrap();
        assert_eq!(exit["exitCode"], Value::Null);
        assert_eq!(exit["signal"], "9");
        assert!(started.elapsed() < Duration::from_secs(10));
        terms.release_all();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cwd_must_stay_inside_the_workspace() {
        let (ws, dir) = workspace("cwd");
        let inside = dir.join("sub");
        std::fs::create_dir_all(&inside).unwrap();
        // The engine runs in this folder, so under root it is given to the engine user (a no-op otherwise).
        layout::give_to_engine(&dir, &inside).unwrap();
        let terms = Terminals::new(ws, std::env::temp_dir());

        assert!(
            terms
                .create(&json!({"command": "/bin/sh", "cwd": "/"}))
                .is_err()
        );
        assert!(
            terms
                .create(&json!({"command": "/bin/sh", "cwd": format!("{}/../", dir.display())}))
                .is_err()
        );
        assert!(
            terms
                .create(&json!({"command": "/bin/sh", "cwd": "sub"}))
                .is_err()
        );

        let id = new_id(
            &terms,
            &json!({
                "command": "/bin/sh",
                "args": ["-c", "echo ok > marker"],
                "cwd": inside.display().to_string(),
            }),
        );
        assert_eq!(wait(&terms, &id).unwrap()["exitCode"], 0);
        assert!(inside.join("marker").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cwd_cannot_be_the_harness_state_directory() {
        let (ws, dir) = workspace("cwd-plaza");
        let state = dir.join(crate::files::PLAZA_DIR);
        std::fs::create_dir_all(state.join("nested")).unwrap();
        std::os::unix::fs::symlink(&state, dir.join("link")).unwrap();
        let terms = Terminals::new(ws, std::env::temp_dir());

        // Named directly, named below, and reached through a link.
        for cwd in [
            state.display().to_string(),
            state.join("nested").display().to_string(),
            dir.join("link").display().to_string(),
        ] {
            let err = terms
                .create(&json!({"command": "/bin/sh", "cwd": cwd}))
                .unwrap_err();
            assert!(err.contains("harness's own state"), "{cwd}: {err}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_is_passed_and_bad_names_are_refused() {
        let (ws, dir) = workspace("env");
        let terms = Terminals::new(ws, std::env::temp_dir());
        assert!(
            terms
                .create(&json!({"command": "/bin/sh", "env": [{"name": "A=B", "value": "x"}]}))
                .is_err()
        );

        let id = new_id(
            &terms,
            &json!({
                "command": "/bin/sh",
                "args": ["-c", "echo $APZ_TERM_TEST"],
                "env": [{"name": "APZ_TERM_TEST", "value": "set"}],
            }),
        );
        wait(&terms, &id).unwrap();
        let out = terms.output(&json!({"terminalId": id})).unwrap();
        assert_eq!(out["output"], "set\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_gives_up_once_cancelled() {
        let (ws, dir) = workspace("cancel");
        let terms = Terminals::new(ws, std::env::temp_dir());
        let id = new_id(&terms, &shell("/bin/sleep 30"));

        let err = terms
            .wait_for_exit(&json!({"terminalId": id}), &|| true)
            .unwrap_err();
        assert_eq!(err, "cancelled");
        terms.release_all();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ran_records_commands_and_exit_codes() {
        let (ws, dir) = workspace("ran");
        let terms = Terminals::new(ws, std::env::temp_dir());
        let id = new_id(
            &terms,
            &json!({"command": "/bin/sh", "args": ["-c", "echo x; exit 4"]}),
        );

        wait(&terms, &id).unwrap();
        assert_eq!(
            terms.ran(),
            vec![("/bin/sh -c echo x; exit 4".to_string(), Some(4))]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_ids_are_refused() {
        let (ws, dir) = workspace("unknown");
        let terms = Terminals::new(ws, std::env::temp_dir());
        let err = terms.output(&json!({"terminalId": "term-9"})).unwrap_err();
        assert_eq!(err, "no terminal term-9");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
