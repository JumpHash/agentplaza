//! `agentplaza acp`: the stdio agent editors start (Zed, JetBrains IDEs, Neovim, any Agent Client
//! Protocol client). Each line the editor writes is one JSON-RPC message, sent as one text frame to
//! the hub's `GET /v1/acp`; each text frame the hub sends comes back as one line on stdout. The
//! bridge speaks no protocol of its own: the hub answers every method. It logs only to stderr,
//! because stdout is the protocol.
//!
//! `agentplaza acp sessions` lists the editor sessions the hub holds (`GET /v1/acp/sessions`).
//!
//! With `--repo <name>` the bridge also mirrors the editor's project to a plaza git repository
//! before each session starts and each prompt runs, so the agent can work on it in its own VM. The
//! mirror pushes the committed HEAD only; uncommitted changes stay in the editor.

use std::collections::HashMap;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use apz_http::ws::{self, Limits, Message, Socket};
use serde_json::{Value, json};

use super::acp_git;
use crate::cmd::mcp::bridge::error_line;
use crate::ctx::{self, Client, Ctx};

/// The WebSocket subprotocol of `GET /v1/acp` (the hub's `apid::acp::PROTOCOL`).
const PROTOCOL: &str = "apz.acp.v1";
/// The largest line the editor may send, and the largest message the bridge takes from the hub.
const MAX_LINE: usize = 8 << 20;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the socket loop waits for a frame before it looks at the editor's queue again.
const TICK: Duration = Duration::from_millis(50);
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
/// Appended to why the hub cannot be reached, in the error the editor's first request gets.
const HINT: &str = "(turn on Editors (ACP) in the console: Setup › MCP › This hub as a server; or run `agentplaza hub login`)";

#[derive(clap::Args, Debug)]
pub struct Args {
    /// The agent the editor's sessions are made on: its name or CID (default: the orchestrator).
    #[arg(long)]
    pub agent: Option<String>,
    /// Mirrors the editor's project to this plaza repository (created if missing) before each
    /// session starts and each prompt runs, on branch `editor/<branch>`.
    #[arg(long, value_parser = parse_repo)]
    pub repo: Option<String>,
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(clap::Subcommand, Debug)]
pub enum Cmd {
    /// The editor sessions the hub holds: id, agent, title, client, updated and closed.
    Sessions,
}

/// Runs `agentplaza acp` (the stdio agent) or `agentplaza acp sessions`. The hub comes from the
/// global flags or the saved hub, as for every command.
pub fn run(
    args: Args,
    hub: Option<String>,
    token: Option<String>,
    pin: Option<String>,
    json: bool,
) -> Result<(), String> {
    match args.command {
        Some(Cmd::Sessions) => {
            let api = ctx::resolve(hub, token, pin, json)?;
            sessions(&api)
        }
        None => stdio(args.agent.as_deref(), hub, token, pin, args.repo.as_deref()),
    }
}

/// The `--repo` value: a plaza repository name, `[a-z0-9._-]` up to 100 bytes. `.` and `..` are
/// refused too: they would climb out of the repository's URL path.
fn parse_repo(name: &str) -> Result<String, String> {
    let valid = (1..=100).contains(&name.len())
        && name != "."
        && name != ".."
        && name.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        });
    if valid {
        Ok(name.to_string())
    } else {
        Err(format!(
            "`{name}` is not a repository name: use 1 to 100 of a-z, 0-9, '.', '_' or '-'"
        ))
    }
}

fn sessions(api: &Ctx) -> Result<(), String> {
    let value = api.client.get("/v1/acp/sessions")?;
    api.out(&value, |value| {
        let rows: Vec<Vec<String>> = listed(value)
            .iter()
            .map(|session| {
                let closed = session
                    .get("closed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                vec![
                    ctx::text(session, "id"),
                    ctx::text(session, "cid"),
                    ctx::text(session, "title"),
                    ctx::text(session, "client"),
                    ctx::ago(session.get("updated_ms").unwrap_or(&Value::Null)),
                    if closed { "yes" } else { "" }.to_string(),
                ]
            })
            .collect();
        ctx::table(
            &["id", "agent", "title", "client", "updated", "closed"],
            &rows,
        );
    });
    Ok(())
}

/// The session list, bare or under `sessions`.
fn listed(value: &Value) -> &[Value] {
    value
        .as_array()
        .or_else(|| value.get("sessions").and_then(Value::as_array))
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// One socket's run ended.
enum Ended {
    /// The editor closed its input; the socket was closed with 1000.
    EditorDone,
    /// The socket ended, and why.
    Lost(String),
}

fn stdio(
    agent: Option<&str>,
    hub: Option<String>,
    token: Option<String>,
    pin: Option<String>,
    repo: Option<&str>,
) -> Result<(), String> {
    let path = socket_path(agent);
    let client = match ctx::resolve(hub, token, pin, false) {
        Ok(api) => Arc::new(api.client),
        Err(reason) => return refuse(&reason, io::stdin().lock(), io::stdout().lock()),
    };
    let mut socket = match connect(&client, &path) {
        Ok(socket) => socket,
        Err(reason) => return refuse(&reason, io::stdin().lock(), io::stdout().lock()),
    };
    // Mirror state is shared: the reader thread notes the projects it mirrors, the socket loop
    // notes the sessions the hub answers with. Without `--repo` there is none, and nothing is parsed.
    let repo = repo.map(str::to_string);
    let projects = repo
        .as_ref()
        .map(|_| Arc::new(Mutex::new(Projects::default())));

    // The reader thread owns stdin. It ends when the editor closes its input, which drops the
    // sender: the socket loop sees the channel disconnect once the queue is empty.
    let (tx, rx) = mpsc::channel::<Outgoing>();
    let finished = Arc::new(AtomicBool::new(false));
    let reader_finished = Arc::clone(&finished);
    let reader_client = Arc::clone(&client);
    let reader_projects = projects.clone();
    thread::spawn(move || {
        let plaza = Plaza {
            client: reader_client,
        };
        let result = forward_lines(
            io::stdin().lock(),
            MAX_LINE,
            |line| {
                let outgoing = match (&repo, &reader_projects) {
                    (Some(name), Some(projects)) => mirror_line(line, name, &plaza, projects),
                    _ => Some(Outgoing::Hub(line.to_string())),
                };
                // A line with no outgoing message (a dropped notification) is skipped, not fatal.
                outgoing.is_none_or(|outgoing| tx.send(outgoing).is_ok())
            },
            report_dropped,
        );
        if let Err(err) = result {
            eprintln!("agentplaza acp: reading the editor's input failed: {err}");
        }
        reader_finished.store(true, Ordering::SeqCst);
    });

    let mut out = io::stdout().lock();
    loop {
        match pump(&mut socket, &rx, &mut out, projects.as_deref())? {
            Ended::EditorDone => return Ok(()),
            Ended::Lost(reason) => {
                if finished.load(Ordering::SeqCst) {
                    return Ok(());
                }
                // One reconnect per loss: the editor's next request reaches the new socket. A
                // failed attempt is the second failure, and ends the bridge.
                eprintln!(
                    "agentplaza acp: the hub connection ended ({reason}); reconnecting in 1 s"
                );
                thread::sleep(RECONNECT_DELAY);
                socket =
                    connect(&client, &path).map_err(|e| format!("reconnecting failed: {e}"))?;
            }
        }
    }
}

/// What the socket loop writes next: a line for the hub, or a line for the editor (an answer the
/// bridge made itself, such as a mirror failure). Both go through one queue, so they keep their order.
enum Outgoing {
    Hub(String),
    Editor(String),
}

/// Sends queued lines and writes each hub message as a line, until the editor is done or the
/// socket ends. Stdout is written and flushed per message. With mirror state, each hub answer is
/// noted first, so the sessions it opens are known before the editor sees them.
fn pump(
    socket: &mut Socket,
    rx: &Receiver<Outgoing>,
    out: &mut impl Write,
    projects: Option<&Mutex<Projects>>,
) -> Result<Ended, String> {
    loop {
        loop {
            match rx.try_recv() {
                Ok(Outgoing::Hub(line)) => {
                    if let Err(err) = socket.send(&Message::Text(line)) {
                        return Ok(Ended::Lost(err.to_string()));
                    }
                }
                Ok(Outgoing::Editor(line)) => {
                    write_line(out, &line)
                        .map_err(|e| format!("writing to the editor failed: {e}"))?;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    socket.close(1000, "the editor closed its input");
                    return Ok(Ended::EditorDone);
                }
            }
        }
        match socket.recv(TICK) {
            Ok(Some(Message::Text(text))) => {
                if let Some(projects) = projects {
                    remember(&mut lock(projects), &text);
                }
                write_line(out, &text).map_err(|e| format!("writing to the editor failed: {e}"))?;
            }
            Ok(Some(Message::Binary(_))) | Ok(None) => {}
            Err(err) => return Ok(Ended::Lost(err.to_string())),
        }
    }
}

/// The hub cannot be reached: the editor's first request is answered with an error that says why,
/// so the editor shows the reason. Always returns the reason as the error, so the bridge exits 1.
pub fn refuse(reason: &str, input: impl BufRead, mut out: impl Write) -> Result<(), String> {
    let mut answer = None;
    forward_lines(
        input,
        MAX_LINE,
        |line| {
            answer = unreachable_answer(line, reason);
            answer.is_none()
        },
        report_dropped,
    )
    .map_err(|e| format!("reading the editor's input failed: {e}"))?;
    if let Some(answer) = answer {
        write_line(&mut out, &answer).map_err(|e| format!("writing to the editor failed: {e}"))?;
    }
    Err(reason.to_string())
}

/// The answer to an editor request the bridge cannot forward: a JSON-RPC error with the request's
/// id. None for a notification or a line that is not a JSON request, which owe no answer.
pub fn unreachable_answer(line: &str, reason: &str) -> Option<String> {
    let message: Value = serde_json::from_str(line).ok()?;
    let id = message.get("id")?;
    if message.get("method").is_none() {
        return None;
    }
    Some(error_line(id, -32000, &format!("{reason} {HINT}")))
}

/// Feeds each line of `input` to `send` in order, without its line break (a `\r\n` too) and
/// without blank lines, until the input ends or `send` returns false. A line over `max` bytes is
/// dropped whole and reported to `dropped` with its length; its bytes are read and discarded, so
/// memory stays near `max` whatever the editor writes. A line that is not UTF-8 is dropped too.
pub fn forward_lines<R: BufRead>(
    mut input: R,
    max: usize,
    mut send: impl FnMut(&str) -> bool,
    mut dropped: impl FnMut(usize),
) -> io::Result<()> {
    let mut line: Vec<u8> = Vec::new();
    loop {
        line.clear();
        // One byte past the cap tells a line that is too long from one that just fits.
        let read = input
            .by_ref()
            .take(max as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            return Ok(());
        }
        let complete = line.last() == Some(&b'\n');
        if complete {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
        } else if line.len() > max {
            let rest = discard_rest_of_line(&mut input)?;
            dropped(line.len() + rest);
            continue;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&line) else {
            dropped(line.len());
            continue;
        };
        if !send(text) {
            return Ok(());
        }
    }
}

/// Reads and drops bytes up to the next line break (not counted) or the end; returns how many
/// bytes were dropped before the break.
fn discard_rest_of_line<R: BufRead>(input: &mut R) -> io::Result<usize> {
    let mut dropped = 0;
    loop {
        let buf = input.fill_buf()?;
        if buf.is_empty() {
            return Ok(dropped);
        }
        match buf.iter().position(|&b| b == b'\n') {
            Some(at) => {
                input.consume(at + 1);
                return Ok(dropped + at);
            }
            None => {
                let len = buf.len();
                input.consume(len);
                dropped += len;
            }
        }
    }
}

/// Writes one message as one line and flushes, so the editor reads it at once. A message with a raw
/// break in it would become two lines for the editor, so it is dropped with a note instead.
pub fn write_line(out: &mut impl Write, text: &str) -> io::Result<()> {
    if text.contains(['\n', '\r']) {
        eprintln!("agentplaza acp: dropped a hub message with a raw line break");
        return Ok(());
    }
    out.write_all(text.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

fn report_dropped(size: usize) {
    eprintln!(
        "agentplaza acp: dropped an editor line of {size} bytes (the limit is {MAX_LINE} bytes)"
    );
}

/// `/v1/acp`, with `?agent=` when the editor named an agent.
fn socket_path(agent: Option<&str>) -> String {
    match agent {
        Some(agent) => format!("/v1/acp?agent={}", encode_query(agent)),
        None => "/v1/acp".to_string(),
    }
}

/// Percent-encodes a query value: unreserved characters stay, every other byte becomes `%XX`.
pub fn encode_query(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The session requests the bridge mirrors the project for before it forwards them.
const MIRRORED: [&str; 4] = [
    "session/new",
    "session/load",
    "session/resume",
    "session/prompt",
];
const NOT_A_REPO: &str = "--repo needs the editor's project to be a git repository";

/// The mirror's view of the editor's sessions: the project each one runs in. A new, loaded or
/// resumed session is known only from the hub's answer, so its request waits in `pending` until then.
#[derive(Default)]
struct Projects {
    sessions: HashMap<String, PathBuf>,
    /// Request id (as JSON text) to the request's project and the session it names, if any.
    pending: HashMap<String, Pending>,
}

struct Pending {
    dir: PathBuf,
    session: Option<String>,
}

/// The mirror state, whatever a panicking thread left in it: it is only a cache of ids and paths.
fn lock(projects: &Mutex<Projects>) -> MutexGuard<'_, Projects> {
    projects.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the mirror does outside the bridge: git in the editor's project, and the hub's repository
/// routes. `Plaza` is the real one; the tests fake it.
trait Mirror {
    /// The top of the working tree `dir` is in; None when `dir` is not in a git repository.
    fn toplevel(&self, dir: &Path) -> Result<Option<PathBuf>, String>;
    fn commit(&self, top: &Path) -> Result<String, String>;
    /// The branch HEAD is on; None when HEAD is detached.
    fn branch(&self, top: &Path) -> Result<Option<String>, String>;
    /// Whether `git status` lists anything: an uncommitted or untracked change.
    fn dirty(&self, top: &Path) -> Result<bool, String>;
    fn repo_exists(&self, name: &str) -> Result<bool, String>;
    fn create_repo(&self, name: &str) -> Result<(), String>;
    /// Force-pushes `refspec` of the repository at `top` to the plaza repository `name`.
    fn push(&self, top: &Path, name: &str, refspec: &str) -> Result<(), String>;
}

/// The real mirror: git runs in the editor's project, and the hub answers the repository routes.
struct Plaza {
    client: Arc<Client>,
}

impl Mirror for Plaza {
    fn toplevel(&self, dir: &Path) -> Result<Option<PathBuf>, String> {
        match git(dir, &["rev-parse", "--show-toplevel"], &[]) {
            Ok(top) => Ok(Some(PathBuf::from(top))),
            Err(reason) if reason.contains("not a git repository") => Ok(None),
            Err(reason) => Err(reason),
        }
    }

    fn commit(&self, top: &Path) -> Result<String, String> {
        git(top, &["rev-parse", "HEAD"], &[])
    }

    fn branch(&self, top: &Path) -> Result<Option<String>, String> {
        // The full name prints `HEAD` when detached, which no branch can be called.
        let full = git(top, &["rev-parse", "--symbolic-full-name", "HEAD"], &[])?;
        Ok(full.strip_prefix("refs/heads/").map(str::to_string))
    }

    fn dirty(&self, top: &Path) -> Result<bool, String> {
        Ok(!git(top, &["status", "--porcelain"], &[])?.is_empty())
    }

    fn repo_exists(&self, name: &str) -> Result<bool, String> {
        // `Client::get` cannot tell a 404 from other failures, so the status is read here.
        let path = format!("/v1/git/repos/{name}");
        let (status, value) = apz_http::client::request_json(
            &self.client.target,
            "GET",
            &path,
            None,
            self.client.timeout,
        )
        .map_err(|e| format!("{}{path}: {e}", self.client.base()))?;
        match status {
            200 => Ok(true),
            404 => Ok(false),
            _ => Err(value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("GET {path}: HTTP {status}"))),
        }
    }

    fn create_repo(&self, name: &str) -> Result<(), String> {
        self.client
            .post("/v1/git/repos", &json!({ "name": name }))
            .map(|_| ())
    }

    fn push(&self, top: &Path, name: &str, refspec: &str) -> Result<(), String> {
        acp_git::push(&self.client, top, name, refspec)
    }
}

/// Runs git in `dir` and returns its trimmed stdout. Git gets no stdin (the editor's protocol
/// runs on it) and never prompts, and its output is captured, so none of it reaches stdout. A
/// failure is the tail of git's stderr.
pub(super) fn git(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String, String> {
    // An empty directory for hooks: the repository's own hooks never run. It is made unique per call
    // (`create_dir` fails on a name that exists), so no other user can plant hooks in it.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let hooks =
        std::env::temp_dir().join(format!("apz-acp-no-hooks-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&hooks)
        .map_err(|e| format!("cannot make an empty hooks directory: {e}"))?;
    let output = git_command(dir, args, env, &hooks).output();
    let _ = std::fs::remove_dir(&hooks);
    let output = output.map_err(|e| format!("cannot run git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(stderr_tail(&String::from_utf8_lossy(&output.stderr)))
    }
}

/// The command for one git call. The repository's own config can run code (`core.fsmonitor` and the
/// hooks), so both are switched off, the hooks path naming the empty `hooks` directory. The CLI's hub
/// settings are removed from git's environment: git never sees the owner token, the hub or the pin.
fn git_command(dir: &Path, args: &[&str], env: &[(&str, &str)], hooks: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.fsmonitor=false"])
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.display()))
        .args(args)
        .envs(env.iter().copied())
        .env_remove("APZ_TOKEN")
        .env_remove("APZ_HUB")
        .env_remove("APZ_PIN")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    command
}

/// The last lines of git's stderr: enough to say why, without its progress output.
fn stderr_tail(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let tail = &lines[lines.len().saturating_sub(5)..];
    if tail.is_empty() {
        "git failed".to_string()
    } else {
        tail.join("; ")
    }
}

/// What a mirror pushed: the plaza branch, the commit, and whether the editor's working tree had
/// changes that the push leaves out.
#[derive(Debug, PartialEq)]
struct Mirrored {
    branch: String,
    commit: String,
    dirty: bool,
}

/// Mirrors the project in `dir` to the plaza repository `name`: creates the repository when the hub
/// does not have it, then pushes HEAD to its `editor/<branch>` branch.
fn mirror_project(mirror: &dyn Mirror, name: &str, dir: &Path) -> Result<Mirrored, String> {
    let top = mirror
        .toplevel(dir)
        .map_err(mirror_failed)?
        .ok_or_else(|| NOT_A_REPO.to_string())?;
    if !mirror.repo_exists(name).map_err(mirror_failed)? {
        mirror.create_repo(name).map_err(mirror_failed)?;
    }
    let commit = mirror.commit(&top).map_err(mirror_failed)?;
    let branch = branch_ref(
        mirror.branch(&top).map_err(mirror_failed)?.as_deref(),
        &commit,
    );
    let dirty = mirror.dirty(&top).map_err(mirror_failed)?;
    mirror
        .push(&top, name, &format!("HEAD:refs/heads/{branch}"))
        .map_err(mirror_failed)?;
    Ok(Mirrored {
        branch,
        commit,
        dirty,
    })
}

fn mirror_failed(reason: String) -> String {
    format!("--repo could not mirror the project: {reason}")
}

/// The cwd a session request runs in: `cwd` for new, load and resume; for a prompt, the project of
/// its session. None when the request names no project, or a session the bridge does not know.
fn mirror_target(
    method: &str,
    params: &Value,
    cwd_of_session: &dyn Fn(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    match method {
        "session/new" | "session/load" | "session/resume" => {
            params.get("cwd").and_then(Value::as_str).map(PathBuf::from)
        }
        "session/prompt" => params
            .get("sessionId")
            .and_then(Value::as_str)
            .and_then(cwd_of_session),
        _ => None,
    }
}

/// The plaza branch a mirror pushes to: `editor/<branch>`, or `editor/detached-<short sha>` when
/// HEAD is detached.
fn branch_ref(branch: Option<&str>, commit: &str) -> String {
    match branch {
        Some(branch) => format!("editor/{branch}"),
        None => format!("editor/detached-{}", commit.get(..7).unwrap_or(commit)),
    }
}

/// The request's params with `_meta.agentplaza.repo` set to `meta`; the keys already there stay.
fn with_repo_meta(mut params: Value, meta: Value) -> Value {
    if !params.is_object() {
        params = json!({});
    }
    if !params["_meta"].is_object() {
        params["_meta"] = json!({});
    }
    if !params["_meta"]["agentplaza"].is_object() {
        params["_meta"]["agentplaza"] = json!({});
    }
    params["_meta"]["agentplaza"]["repo"] = meta;
    params
}

fn repo_meta(name: &str, mirrored: &Mirrored) -> Value {
    json!({
        "name": name,
        "branch": mirrored.branch,
        "commit": mirrored.commit,
        "dirty": mirrored.dirty,
        "note": "the branch holds commits only: uncommitted changes in the editor are not in it",
    })
}

/// What the bridge does with one editor line when `--repo` is set. A session request is mirrored
/// first, then forwarded with its `_meta`; when the mirror fails, the request is answered with the
/// error and not forwarded. Any other line goes on byte for byte. None: nothing is sent (a
/// notification that cannot be mirrored has no answer to give).
fn mirror_line(
    line: &str,
    repo: &str,
    mirror: &dyn Mirror,
    projects: &Mutex<Projects>,
) -> Option<Outgoing> {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Some(Outgoing::Hub(line.to_string()));
    };
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !MIRRORED.contains(&method) {
        return Some(Outgoing::Hub(line.to_string()));
    }
    match mirror_request(&message, method, repo, mirror, projects) {
        Ok(rewritten) => Some(Outgoing::Hub(rewritten.to_string())),
        Err(reason) => match message.get("id") {
            Some(id) => Some(Outgoing::Editor(error_line(id, -32000, &reason))),
            None => {
                eprintln!("agentplaza acp: {reason}; {method} is not forwarded");
                None
            }
        },
    }
}

fn mirror_request(
    message: &Value,
    method: &str,
    repo: &str,
    mirror: &dyn Mirror,
    projects: &Mutex<Projects>,
) -> Result<Value, String> {
    let params = message.get("params").unwrap_or(&Value::Null);
    let dir = {
        let state = lock(projects);
        mirror_target(method, params, &|session| {
            state.sessions.get(session).cloned()
        })
    }
    .ok_or_else(|| format!("--repo cannot tell which of the editor's projects {method} is for"))?;
    let mirrored = mirror_project(mirror, repo, &dir)?;
    // The hub's answer to a new, loaded or resumed session is what names it, so the project waits
    // for that answer (see `remember`).
    if let Some(id) = message.get("id").filter(|_| method != "session/prompt") {
        let session = params
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string);
        lock(projects)
            .pending
            .insert(id.to_string(), Pending { dir, session });
    }
    let mut rewritten = message.clone();
    let params = rewritten["params"].take();
    rewritten["params"] = with_repo_meta(params, repo_meta(repo, &mirrored));
    Ok(rewritten)
}

/// Notes the session a hub answer opens. The answer to a request the bridge mirrored names the
/// session (or, for a load or resume, the request does); later prompts of that session run in the
/// project the request was mirrored from.
fn remember(projects: &mut Projects, text: &str) {
    if projects.pending.is_empty() {
        return;
    }
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return;
    };
    // Only answers settle a request; the hub's own requests carry a method.
    if message.get("method").is_some() {
        return;
    }
    let Some(id) = message.get("id") else {
        return;
    };
    let Some(pending) = projects.pending.remove(&id.to_string()) else {
        return;
    };
    if message.get("error").is_some() {
        return;
    }
    let session = pending
        .session
        .or_else(|| message["result"]["sessionId"].as_str().map(str::to_string));
    if let Some(session) = session {
        // A session keeps the project it was first mapped to: a later answer naming it does not move
        // its prompts to another project.
        if let Some(known) = projects.sessions.get(&session) {
            if *known != pending.dir {
                eprintln!(
                    "agentplaza acp: session {session} is already mapped to another project; keeping the first"
                );
            }
        } else {
            projects.sessions.insert(session, pending.dir);
        }
    }
}

fn connect(client: &Client, path: &str) -> Result<Socket, String> {
    let limits = Limits {
        max_frame: MAX_LINE,
        max_message: MAX_LINE,
        ..Limits::default()
    };
    ws::connect(
        &client.target,
        path,
        Some(PROTOCOL),
        CONNECT_TIMEOUT,
        limits,
    )
    .map_err(|e| format!("{}: {e}", client.base()))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    /// The lines `forward_lines` sends and the lengths it drops, for `input` read with `max`.
    fn split(input: &[u8], max: usize) -> (Vec<String>, Vec<usize>) {
        let mut sent = Vec::new();
        let mut dropped = Vec::new();
        forward_lines(
            input,
            max,
            |line| {
                sent.push(line.to_string());
                true
            },
            |size| dropped.push(size),
        )
        .unwrap();
        (sent, dropped)
    }

    #[test]
    fn lines_go_in_order_without_their_breaks_and_blank_lines_are_skipped() {
        let (sent, dropped) = split(b"{\"a\":1}\r\n\n  \n{\"b\":2}\nlast", MAX_LINE);
        assert_eq!(sent, ["{\"a\":1}", "{\"b\":2}", "last"]);
        assert!(dropped.is_empty());
    }

    #[test]
    fn a_line_over_the_cap_is_dropped_whole_and_the_next_line_still_goes() {
        let (sent, dropped) = split(b"0123456789ABCDEF\nok\n", 8);
        assert_eq!(sent, ["ok"]);
        assert_eq!(dropped, [16]);
    }

    #[test]
    fn a_line_of_exactly_the_cap_goes_through() {
        let (sent, dropped) = split(b"abcd\nef\n", 4);
        assert_eq!(sent, ["abcd", "ef"]);
        assert!(dropped.is_empty());
    }

    #[test]
    fn a_line_that_is_not_utf8_is_dropped() {
        let (sent, dropped) = split(&[0xff, b'\n', b'o', b'k', b'\n'], MAX_LINE);
        assert_eq!(sent, ["ok"]);
        assert_eq!(dropped, [1]);
    }

    #[test]
    fn forwarding_stops_when_send_says_no() {
        let mut sent = Vec::new();
        forward_lines(
            &b"a\nb\n"[..],
            MAX_LINE,
            |line| {
                sent.push(line.to_string());
                false
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(sent, ["a"]);
    }

    #[test]
    fn write_line_is_one_line_with_a_break() {
        let mut out = Vec::new();
        write_line(&mut out, "{\"id\":1}").unwrap();
        assert_eq!(out, b"{\"id\":1}\n");
    }

    #[test]
    fn a_hub_message_with_a_raw_break_is_dropped_not_split() {
        for text in ["{\"a\":1}\n{\"b\":2}", "{\"a\":1}\r{\"b\":2}"] {
            let mut out = Vec::new();
            write_line(&mut out, text).unwrap();
            assert!(out.is_empty(), "{text:?} wrote {out:?}");
        }
    }

    #[test]
    fn git_is_run_without_the_owner_token_or_the_hub_settings_and_without_repo_hooks() {
        let command = git_command(
            Path::new("/work/app"),
            &["status"],
            &[("NO_PROXY", "127.0.0.1")],
            Path::new("/tmp/no-hooks"),
        );
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.contains(&"core.fsmonitor=false".to_string()),
            "{args:?}"
        );
        assert!(
            args.contains(&"core.hooksPath=/tmp/no-hooks".to_string()),
            "{args:?}"
        );
        let envs: Vec<_> = command.get_envs().collect();
        for name in ["APZ_TOKEN", "APZ_HUB", "APZ_PIN"] {
            assert!(
                envs.iter().any(|(k, v)| *k == name && v.is_none()),
                "{name} is removed from git's environment"
            );
        }
    }

    #[test]
    fn a_session_keeps_the_project_it_was_first_mapped_to() {
        let mut projects = Projects::default();
        projects
            .sessions
            .insert("s9".into(), PathBuf::from("/work/app"));
        projects.pending.insert(
            "1".into(),
            Pending {
                dir: PathBuf::from("/work/other"),
                session: None,
            },
        );
        remember(
            &mut projects,
            r#"{"jsonrpc":"2.0","id":1,"result":{"sessionId":"s9"}}"#,
        );
        assert_eq!(
            projects.sessions.get("s9"),
            Some(&PathBuf::from("/work/app"))
        );
    }

    #[test]
    fn an_initialize_request_gets_the_error_with_its_id() {
        let line = r#"{"jsonrpc":"2.0","id":7,"method":"initialize","params":{}}"#;
        let answer = unreachable_answer(line, "no saved hub").expect("a request is answered");
        let value: Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(value["id"], 7);
        assert_eq!(value["error"]["code"], -32000);
        assert_eq!(
            value["error"]["message"],
            "no saved hub (turn on Editors (ACP) in the console: Setup › MCP › This hub as a server; or run `agentplaza hub login`)"
        );
    }

    #[test]
    fn notifications_and_broken_lines_owe_no_answer() {
        let note = r#"{"jsonrpc":"2.0","method":"session/cancel","params":{}}"#;
        assert_eq!(unreachable_answer(note, "x"), None);
        assert_eq!(unreachable_answer("{not json", "x"), None);
    }

    #[test]
    fn refuse_answers_the_first_request_and_fails_with_the_reason() {
        let input = b"{\"jsonrpc\":\"2.0\",\"method\":\"initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"initialize\"}\n";
        let mut out = Vec::new();
        let err = refuse("no saved hub", &input[..], &mut out).unwrap_err();
        assert_eq!(err, "no saved hub");
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text.lines().count(),
            1,
            "only the request is answered: {text}"
        );
        let value: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(value["id"], 7);
    }

    #[test]
    fn the_agent_name_is_percent_encoded_in_the_path() {
        assert_eq!(encode_query("builder-1"), "builder-1");
        assert_eq!(encode_query("a b/é"), "a%20b%2F%C3%A9");
        assert_eq!(socket_path(Some("builder 1")), "/v1/acp?agent=builder%201");
        assert_eq!(socket_path(None), "/v1/acp");
    }

    #[test]
    fn acp_takes_an_agent_name() {
        let cli = crate::Cli::try_parse_from(["agentplaza", "acp", "--agent", "builder-1"])
            .expect("parses");
        match cli.command {
            crate::Command::Acp(args) => {
                assert_eq!(args.agent.as_deref(), Some("builder-1"));
                assert!(args.command.is_none());
            }
            _ => panic!("not the acp command"),
        }
    }

    #[test]
    fn acp_sessions_is_a_subcommand() {
        let cli = crate::Cli::try_parse_from(["agentplaza", "acp", "sessions"]).expect("parses");
        match cli.command {
            crate::Command::Acp(args) => assert!(matches!(args.command, Some(Cmd::Sessions))),
            _ => panic!("not the acp command"),
        }
    }

    #[test]
    fn acp_takes_a_repo_name() {
        let cli =
            crate::Cli::try_parse_from(["agentplaza", "acp", "--repo", "my-app"]).expect("parses");
        match cli.command {
            crate::Command::Acp(args) => assert_eq!(args.repo.as_deref(), Some("my-app")),
            _ => panic!("not the acp command"),
        }
        assert!(crate::Cli::try_parse_from(["agentplaza", "acp", "--repo", "My App"]).is_err());
    }

    #[test]
    fn repo_names_are_lowercase_dotted_words_up_to_100_bytes() {
        assert_eq!(parse_repo("my-app.1_x").as_deref(), Ok("my-app.1_x"));
        assert!(parse_repo("").is_err());
        assert!(parse_repo("My App").is_err());
        assert!(parse_repo("a/b").is_err());
        assert!(parse_repo(".").is_err());
        assert!(parse_repo("..").is_err());
        assert!(parse_repo(&"a".repeat(100)).is_ok());
        assert!(parse_repo(&"a".repeat(101)).is_err());
    }

    use std::cell::RefCell;

    /// A mirror that records what it is asked, instead of running git or calling the hub.
    #[derive(Default)]
    struct Fake {
        top: Option<PathBuf>,
        branch: Option<String>,
        commit: String,
        dirty: bool,
        repo_exists: bool,
        push_error: Option<String>,
        asked: RefCell<Vec<PathBuf>>,
        created: RefCell<Vec<String>>,
        pushed: RefCell<Vec<(PathBuf, String, String)>>,
    }

    impl Mirror for Fake {
        fn toplevel(&self, dir: &Path) -> Result<Option<PathBuf>, String> {
            self.asked.borrow_mut().push(dir.to_path_buf());
            Ok(self.top.clone())
        }
        fn commit(&self, _top: &Path) -> Result<String, String> {
            Ok(self.commit.clone())
        }
        fn branch(&self, _top: &Path) -> Result<Option<String>, String> {
            Ok(self.branch.clone())
        }
        fn dirty(&self, _top: &Path) -> Result<bool, String> {
            Ok(self.dirty)
        }
        fn repo_exists(&self, _name: &str) -> Result<bool, String> {
            Ok(self.repo_exists)
        }
        fn create_repo(&self, name: &str) -> Result<(), String> {
            self.created.borrow_mut().push(name.to_string());
            Ok(())
        }
        fn push(&self, top: &Path, name: &str, refspec: &str) -> Result<(), String> {
            self.pushed.borrow_mut().push((
                top.to_path_buf(),
                name.to_string(),
                refspec.to_string(),
            ));
            match &self.push_error {
                Some(reason) => Err(reason.clone()),
                None => Ok(()),
            }
        }
    }

    /// A clean project on `main` at a fixed commit, in `/work/app`.
    fn project() -> Fake {
        Fake {
            top: Some(PathBuf::from("/work/app")),
            branch: Some("main".to_string()),
            commit: "0123456789abcdef".to_string(),
            ..Fake::default()
        }
    }

    fn to_hub(out: Option<Outgoing>) -> String {
        match out {
            Some(Outgoing::Hub(line)) => line,
            Some(Outgoing::Editor(_)) => panic!("answered the editor, not forwarded"),
            None => panic!("nothing sent"),
        }
    }

    #[test]
    fn a_missing_repository_is_created_and_the_branch_pushed() {
        let fake = project();
        let mirrored = mirror_project(&fake, "app", Path::new("/work/app/src")).expect("mirrors");
        assert_eq!(
            mirrored,
            Mirrored {
                branch: "editor/main".to_string(),
                commit: "0123456789abcdef".to_string(),
                dirty: false,
            }
        );
        assert_eq!(*fake.created.borrow(), ["app"]);
        assert_eq!(
            *fake.pushed.borrow(),
            [(
                PathBuf::from("/work/app"),
                "app".to_string(),
                "HEAD:refs/heads/editor/main".to_string(),
            )]
        );
    }

    #[test]
    fn an_existing_repository_is_not_created_again() {
        let fake = Fake {
            repo_exists: true,
            ..project()
        };
        mirror_project(&fake, "app", Path::new("/work/app")).expect("mirrors");
        assert!(fake.created.borrow().is_empty());
        assert_eq!(fake.pushed.borrow().len(), 1);
    }

    #[test]
    fn a_project_outside_git_is_refused_before_anything_is_pushed() {
        let fake = Fake {
            top: None,
            ..project()
        };
        let err = mirror_project(&fake, "app", Path::new("/tmp/notes")).unwrap_err();
        assert_eq!(err, NOT_A_REPO);
        assert!(fake.created.borrow().is_empty());
        assert!(fake.pushed.borrow().is_empty());
    }

    #[test]
    fn a_detached_head_is_pushed_under_its_short_sha() {
        let fake = Fake {
            branch: None,
            ..project()
        };
        let mirrored = mirror_project(&fake, "app", Path::new("/work/app")).expect("mirrors");
        assert_eq!(mirrored.branch, "editor/detached-0123456");
    }

    #[test]
    fn a_git_failure_is_reported_with_its_reason() {
        let fake = Fake {
            push_error: Some("remote rejected".to_string()),
            ..project()
        };
        let err = mirror_project(&fake, "app", Path::new("/work/app")).unwrap_err();
        assert_eq!(err, "--repo could not mirror the project: remote rejected");
    }

    #[test]
    fn a_session_request_is_mirrored_and_forwarded_with_its_meta() {
        let fake = Fake {
            dirty: true,
            ..project()
        };
        let projects = Mutex::new(Projects::default());
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"/work/app","mcpServers":[]}}"#;
        let sent = to_hub(mirror_line(line, "app", &fake, &projects));
        let value: Value = serde_json::from_str(&sent).unwrap();
        let repo = &value["params"]["_meta"]["agentplaza"]["repo"];
        assert_eq!(repo["name"], "app");
        assert_eq!(repo["branch"], "editor/main");
        assert_eq!(repo["commit"], "0123456789abcdef");
        assert_eq!(repo["dirty"], true);
        assert!(repo["note"].as_str().unwrap().contains("uncommitted"));
        assert_eq!(value["params"]["cwd"], "/work/app", "the params are kept");
        assert!(lock(&projects).pending.contains_key("1"));
    }

    #[test]
    fn a_prompt_runs_in_the_project_its_session_was_opened_in() {
        let fake = project();
        let projects = Mutex::new(Projects::default());
        lock(&projects)
            .sessions
            .insert("s1".to_string(), PathBuf::from("/work/other"));
        let line = r#"{"jsonrpc":"2.0","id":4,"method":"session/prompt","params":{"sessionId":"s1","prompt":[]}}"#;
        let sent = to_hub(mirror_line(line, "app", &fake, &projects));
        assert!(sent.contains("_meta"));
        assert_eq!(*fake.asked.borrow(), [PathBuf::from("/work/other")]);
    }

    #[test]
    fn a_prompt_of_an_unknown_session_is_answered_with_an_error() {
        let fake = project();
        let projects = Mutex::new(Projects::default());
        let line = r#"{"jsonrpc":"2.0","id":9,"method":"session/prompt","params":{"sessionId":"nope","prompt":[]}}"#;
        match mirror_line(line, "app", &fake, &projects) {
            Some(Outgoing::Editor(answer)) => {
                let value: Value = serde_json::from_str(&answer).unwrap();
                assert_eq!(value["id"], 9);
                assert_eq!(value["error"]["code"], -32000);
            }
            _ => panic!("the editor is not answered"),
        }
        assert!(fake.pushed.borrow().is_empty());
    }

    #[test]
    fn a_notification_that_cannot_be_mirrored_is_dropped() {
        let fake = project();
        let projects = Mutex::new(Projects::default());
        let line = r#"{"jsonrpc":"2.0","method":"session/prompt","params":{"sessionId":"nope"}}"#;
        assert!(mirror_line(line, "app", &fake, &projects).is_none());
    }

    #[test]
    fn other_lines_go_on_byte_for_byte() {
        let fake = project();
        let projects = Mutex::new(Projects::default());
        for line in [
            r#"{"jsonrpc":"2.0","id":2,"method":"fs/read_text_file","params":{"path":"a"}}"#,
            r#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"s1"}}"#,
            "not json",
        ] {
            assert_eq!(to_hub(mirror_line(line, "app", &fake, &projects)), line);
        }
        assert!(fake.asked.borrow().is_empty());
    }

    #[test]
    fn the_hubs_answer_to_a_session_request_names_its_project() {
        let fake = project();
        let projects = Mutex::new(Projects::default());
        let line =
            r#"{"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"/work/app"}}"#;
        to_hub(mirror_line(line, "app", &fake, &projects));
        remember(
            &mut lock(&projects),
            r#"{"jsonrpc":"2.0","id":1,"result":{"sessionId":"s9"}}"#,
        );
        assert_eq!(
            lock(&projects).sessions.get("s9"),
            Some(&PathBuf::from("/work/app"))
        );
    }

    #[test]
    fn a_failed_session_answer_names_no_session() {
        let fake = project();
        let projects = Mutex::new(Projects::default());
        let line =
            r#"{"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"/work/app"}}"#;
        to_hub(mirror_line(line, "app", &fake, &projects));
        remember(
            &mut lock(&projects),
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"no"}}"#,
        );
        assert!(lock(&projects).sessions.is_empty());
        assert!(lock(&projects).pending.is_empty());
    }

    #[test]
    fn mirror_target_is_the_cwd_of_the_request_or_of_its_session() {
        let known = |session: &str| (session == "s1").then(|| PathBuf::from("/work/s1"));
        let params = json!({"cwd": "/work/app", "sessionId": "s1"});
        assert_eq!(
            mirror_target("session/new", &params, &known),
            Some(PathBuf::from("/work/app"))
        );
        assert_eq!(
            mirror_target("session/load", &params, &known),
            Some(PathBuf::from("/work/app"))
        );
        assert_eq!(
            mirror_target("session/resume", &params, &known),
            Some(PathBuf::from("/work/app"))
        );
        assert_eq!(
            mirror_target("session/prompt", &params, &known),
            Some(PathBuf::from("/work/s1"))
        );
        assert_eq!(mirror_target("session/new", &json!({}), &known), None);
        assert_eq!(mirror_target("fs/read_text_file", &params, &known), None);
    }

    #[test]
    fn the_repo_meta_keeps_the_keys_already_in_meta() {
        let params = json!({"cwd": "/x", "_meta": {"other": 1, "agentplaza": {"keep": true}}});
        let value = with_repo_meta(params, json!({"name": "app"}));
        assert_eq!(value["_meta"]["other"], 1);
        assert_eq!(value["_meta"]["agentplaza"]["keep"], true);
        assert_eq!(value["_meta"]["agentplaza"]["repo"]["name"], "app");
        assert_eq!(value["cwd"], "/x");
    }

    #[test]
    fn a_branch_ref_is_the_branch_or_the_short_sha() {
        assert_eq!(branch_ref(Some("main"), "0123456789"), "editor/main");
        assert_eq!(branch_ref(Some("fix/x"), "0123456789"), "editor/fix/x");
        assert_eq!(branch_ref(None, "0123456789"), "editor/detached-0123456");
    }

    /// A scratch directory under the system temp dir, unique to this test run.
    fn scratch(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("apz-acp-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Runs git for the test's own setup; the test fails when git does.
    fn git_setup(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("git runs");
        assert!(status.status.success(), "git {args:?}: {status:?}");
    }

    #[test]
    fn the_real_git_mirrors_a_commit_to_a_bare_repository() {
        let root = scratch("mirror");
        let project = root.join("project");
        let hub = root.join("hub.git");
        std::fs::create_dir_all(&project).unwrap();
        git_setup(&project, &["init", "-q", "-b", "main"]);
        std::fs::write(project.join("main.rs"), "fn main() {}\n").unwrap();
        git_setup(&project, &["add", "main.rs"]);
        git_setup(
            &project,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "-m",
                "first",
            ],
        );
        git_setup(&root, &["init", "-q", "--bare", "hub.git"]);

        let mirror = Plaza {
            client: Arc::new(Client::new("http://127.0.0.1:9", None, None)),
        };
        let top = mirror.toplevel(&project).unwrap().expect("a repository");
        let commit = mirror.commit(&top).unwrap();
        assert_eq!(mirror.branch(&top).unwrap().as_deref(), Some("main"));
        assert!(!mirror.dirty(&top).unwrap());
        std::fs::write(project.join("draft.txt"), "not committed").unwrap();
        assert!(mirror.dirty(&top).unwrap(), "an untracked file is a change");

        let refspec = format!("HEAD:refs/heads/{}", branch_ref(Some("main"), &commit));
        let url = format!("file://{}", hub.display());
        git(&top, &["push", "--force", "--quiet", &url, &refspec], &[]).expect("pushes");
        assert_eq!(
            git(&hub, &["rev-parse", "refs/heads/editor/main"], &[]).unwrap(),
            commit
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
