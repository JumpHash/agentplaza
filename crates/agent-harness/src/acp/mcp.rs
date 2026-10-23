//! The plaza's own tools for an engine, through an MCP server the harness offers it in `session/new`.
//!
//! The server is this binary in `--plaza-mcp <socket>` mode ([`run_stdio`]). The engine starts it
//! and speaks MCP on its stdio; it holds no state and no secrets. It forwards `tools/list` and
//! `tools/call` as one JSON line each over a unix socket to the engine session, which answers them
//! ([`handle`]) through [`EngineLink::call`]. So every call goes through the hub's gate, exactly as
//! the agent's own calls do.

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::collections::HashMap;
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(unix)]
use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_proto::chat::ToolSpec;
use serde_json::{Value, json};

use super::link::EngineLink;

/// The MCP protocol version the server speaks.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// The name the engine knows the server by (its `mcpServers` entry).
const SERVER_NAME: &str = "plaza";
/// How long the accept loop sleeps between checks of its stop flag.
#[cfg(unix)]
const ACCEPT_POLL: Duration = Duration::from_millis(20);
/// How long the accept loop waits after an accept error (EMFILE, say) before it tries again.
#[cfg(unix)]
const ACCEPT_RETRY: Duration = Duration::from_millis(50);
/// The most bytes in one line on the socket or the stdio side, newline included. A longer line is
/// answered with an error and its connection closed, so an engine cannot grow the harness's buffers.
#[cfg(unix)]
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
/// The most connections open at once; one more is closed as it is accepted.
#[cfg(unix)]
const MAX_CONNECTIONS: usize = 8;
#[cfg(unix)]
const LINE_TOO_LONG: &str = "the line is longer than 8 MiB";

const PARSE_ERROR: i64 = -32700;
#[cfg(unix)]
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

/// The groups of the plaza's MCP server (PLAN 12.3): what an engine is offered of the plaza's tools, by
/// the tool's name. A tool outside every group is not served; the engine has its own file, shell and
/// search tools, so `read`, `edit`, `find`, `search` and `run` are not among them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// Weft: the project's history and the seat's lane.
    Weft,
    /// Review: the review steps a reviewer seat has.
    Review,
    /// Code intelligence (PLAN 1.8): symbols, definitions, references, diagnostics and outlines, run in the VM.
    Code,
    /// Tests: `run_tests`, run in the VM.
    Tests,
    /// The library: the approved `x_` tools, through `tool_search`.
    Library,
    /// Skills: the skills the seat may use, listed and read.
    Skills,
    /// Helpers (PLAN 1.10): `spawn_helper` and `await_helper`.
    Helpers,
    /// Talk: seat messages, and reading and answering a seat's threads.
    Talk,
    /// The journal and the handover.
    Journal,
    /// The project's memory.
    Memory,
    /// The owner's MCP servers the project enables, proxied as `mcp__<server>__<tool>`.
    Owner,
}

/// The group a tool belongs to, or `None` when the plaza's MCP server does not serve it.
pub fn group_of(name: &str) -> Option<Group> {
    Some(match name {
        "weft_status" | "weft_diff" | "weft_describe" | "weft_log" | "weft_show" | "weft_blame"
        | "weft_map" | "weft_change_tree" | "weft_touches" => Group::Weft,
        "review_tree" | "review_score" | "review_verdict" | "review_spawn" => Group::Review,
        "symbols" | "definition" | "references" | "diagnostics" | "outline" => Group::Code,
        "run_tests" | "tests_select" => Group::Tests,
        "plaza_skills" | "plaza_skill_read" => Group::Skills,
        "spawn_helper" | "await_helper" => Group::Helpers,
        "plaza_message_seat" | "thread_read" | "thread_answer" => Group::Talk,
        "journal_note" | "journal_read" | "handover_write" => Group::Journal,
        name if name.starts_with("memory_") => Group::Memory,
        // The library's tools carry the library's prefix (`x_`, see the hub's `library::PREFIX`).
        name if name.starts_with("x_") => Group::Library,
        name if name.starts_with("mcp__") => Group::Owner,
        _ => return None,
    })
}

/// The plaza's tools an engine may call through its MCP server: those of the profile that belong to a
/// group (see [`group_of`]). The profile is the hub's answer for the session's seat and project, so
/// the groups are filtered by it and nothing more is offered.
pub fn plaza_tools(profile_tools: &[ToolSpec]) -> Vec<ToolSpec> {
    profile_tools
        .iter()
        .filter(|tool| group_of(&tool.name).is_some())
        .cloned()
        .collect()
}

/// The stdio side: the reply to one line the engine sent, or None for a notification (a message
/// with no id, which gets no reply). `forward` carries `tools/list` and `tools/call` to the session
/// and returns its reply, a complete JSON-RPC response.
pub fn answer(line: &str, mut forward: impl FnMut(&Value) -> Value) -> Option<String> {
    let Ok(request) = serde_json::from_str::<Value>(line) else {
        return Some(failure(&Value::Null, PARSE_ERROR, "parse error").to_string());
    };
    let id = request.get("id")?.clone();
    let reply = match request.get("method").and_then(Value::as_str) {
        Some("initialize") => result(
            &id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
            }),
        ),
        Some("ping") => result(&id, json!({})),
        Some("tools/list" | "tools/call") => forward(&request),
        _ => failure(&id, METHOD_NOT_FOUND, "method not found"),
    };
    Some(reply.to_string())
}

/// The session's side: the reply to one request the stdio server forwarded. `tools/list` offers
/// `tools`; `tools/call` runs one of them through the link. A name not offered is refused before the
/// hub is asked, and a failed call is an MCP tool result with `isError`, as MCP has it.
pub fn handle(
    request: &Value,
    tools: &[ToolSpec],
    link: &dyn EngineLink,
    deadline: Instant,
) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    match request.get("method").and_then(Value::as_str) {
        Some("tools/list") => {
            let offered: Vec<Value> = tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": tool.input_schema,
                    })
                })
                .collect();
            result(&id, json!({ "tools": offered }))
        }
        Some("tools/call") => {
            let params = request.get("params").cloned().unwrap_or(Value::Null);
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return failure(&id, INVALID_PARAMS, "tools/call needs a tool name");
            };
            if !tools.iter().any(|tool| tool.name == name) {
                return failure(&id, INVALID_PARAMS, &format!("unknown tool {name}"));
            }
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            // Every tool goes through the link, helpers included: the hub's gate decides it, and the session runs
            // the ones the VM runs (a helper's start and await among them) as the agent's own loop does.
            let outcome = if arguments.is_object() {
                link.call(name, &arguments, deadline)
            } else {
                Err("invalid input: the arguments must be a JSON object".to_string())
            };
            let (text, is_error) = match outcome {
                Ok(text) => (text, false),
                Err(text) => (text, true),
            };
            result(
                &id,
                json!({"content": [{"type": "text", "text": text}], "isError": is_error}),
            )
        }
        _ => failure(&id, METHOD_NOT_FOUND, "method not found"),
    }
}

fn result(id: &Value, value: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": value})
}

fn failure(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The `mcpServers` a session is opened with: the plaza's server, started from this binary and
/// connected to `socket`; none when there is no socket.
pub fn servers(socket: Option<&Path>) -> io::Result<Value> {
    let Some(socket) = socket else {
        return Ok(json!([]));
    };
    Ok(json!([{
        "name": SERVER_NAME,
        "command": std::env::current_exe()?.to_string_lossy(),
        "args": ["--plaza-mcp", socket.to_string_lossy()],
        "env": [],
    }]))
}

/// Where an engine session's socket is bound: `<workspace>/.engines/run`, outside `.plaza/`, which an
/// engine in a root guest cannot enter (see `super::layout`).
pub fn socket_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".engines").join("run")
}

/// The engine session's socket for one turn: bound before the engine starts, removed when the turn
/// ends. Mode 0600; in a root guest its folder is root's at 0711 (enter, not list) and the socket is
/// the engine user's, so the engine's MCP server can connect and the agent's own commands cannot.
#[cfg(unix)]
pub struct Socket {
    listener: UnixListener,
    path: PathBuf,
}

#[cfg(unix)]
impl Socket {
    /// Binds a socket in `dir` for `tools`, or None when there are no tools to offer.
    pub fn bind(dir: &Path, tools: &[ToolSpec]) -> io::Result<Option<Self>> {
        if tools.is_empty() {
            return Ok(None);
        }
        // Each turn's socket has its own name, so a second turn cannot find the first one's.
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        fs::create_dir_all(dir)?;
        let root = crate::bash::runs_as_root().map_err(io::Error::other)?;
        if root {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o711))?;
        }
        let path = dir.join(format!("engine-mcp-{}-{n}.sock", std::process::id()));
        let _ = fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        if root {
            let id = crate::bash::ENGINE_UID;
            std::os::unix::fs::lchown(&path, Some(id), Some(id))?;
        }
        Ok(Some(Self { listener, path }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Serves the engine's MCP servers' connections until `stop` is set, then closes what is still
    /// open. Each connection has a thread; the scope returns when all of them have ended.
    pub fn serve(
        &self,
        tools: &[ToolSpec],
        link: &dyn EngineLink,
        deadline: Instant,
        stop: &AtomicBool,
    ) {
        if self.listener.set_nonblocking(true).is_err() {
            return;
        }
        // The connections still open, by number: a finished one takes itself out, so the cap counts
        // only live connections and the set does not keep their sockets.
        let open: Mutex<HashMap<u64, UnixStream>> = Mutex::new(HashMap::new());
        let mut next = 0u64;
        thread::scope(|scope| {
            while !stop.load(Ordering::SeqCst) {
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        // Accepted sockets are blocking: a connection's thread waits for its lines.
                        if stream.set_nonblocking(false).is_err() {
                            continue;
                        }
                        // Over the cap the connection is closed at once, not queued.
                        if lock(&open).len() >= MAX_CONNECTIONS {
                            continue;
                        }
                        let Ok(clone) = stream.try_clone() else {
                            continue;
                        };
                        next += 1;
                        let id = next;
                        lock(&open).insert(id, clone);
                        let registry = &open;
                        scope.spawn(move || {
                            connection(stream, tools, link, deadline);
                            lock(registry).remove(&id);
                        });
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(ACCEPT_POLL);
                    }
                    // An accept error (EMFILE, say) ends nothing: it is logged and tried again.
                    Err(err) => {
                        eprintln!(
                            "agent-harness: the plaza's tools cannot accept a connection: {err}"
                        );
                        thread::sleep(ACCEPT_RETRY);
                    }
                }
            }
            // Closing the connections ends their reads, so the threads end and the scope returns.
            for stream in lock(&open).values() {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        });
    }
}

/// A mutex's guard, past a poisoning: the sets it guards stay usable when a connection's thread died.
#[cfg(unix)]
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One line read under the cap.
#[cfg(unix)]
#[derive(Debug, PartialEq)]
enum Line {
    /// A line, without its newline.
    Text(String),
    /// A line longer than the cap; the reader has consumed only the cap's bytes of it.
    TooLong,
    /// The end of the input.
    End,
}

/// Reads one line of at most `max` bytes, newline included. A longer line is [`Line::TooLong`]: the
/// caller answers with an error and stops reading the input.
#[cfg(unix)]
fn read_line_capped(reader: &mut impl BufRead, max: usize) -> io::Result<Line> {
    let mut line = String::new();
    // One byte past the cap tells a line that is too long from one that exactly fits.
    let read = reader.take(max as u64 + 1).read_line(&mut line)?;
    if read == 0 {
        return Ok(Line::End);
    }
    if line.len() > max {
        return Ok(Line::TooLong);
    }
    Ok(Line::Text(line.trim_end_matches(['\r', '\n']).to_string()))
}

#[cfg(unix)]
impl Drop for Socket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// One stdio server's connection: each line is a request, answered by one line.
#[cfg(unix)]
fn connection(stream: UnixStream, tools: &[ToolSpec], link: &dyn EngineLink, deadline: Instant) {
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    loop {
        let line = match read_line_capped(&mut reader, MAX_LINE_BYTES) {
            Ok(Line::Text(line)) => line,
            // An over-long line is answered, then the connection is closed.
            Ok(Line::TooLong) => {
                let _ = writeln!(
                    writer,
                    "{}",
                    failure(&Value::Null, INVALID_REQUEST, LINE_TOO_LONG)
                );
                return;
            }
            Ok(Line::End) | Err(_) => return,
        };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle(&request, tools, link, deadline),
            Err(_) => failure(&Value::Null, PARSE_ERROR, "parse error"),
        };
        if writeln!(writer, "{reply}")
            .and_then(|()| writer.flush())
            .is_err()
        {
            return;
        }
    }
}

/// The `--plaza-mcp <socket>` mode: this process is the engine's MCP server. It answers the
/// engine's JSON-RPC lines on stdout (stderr is for errors only) and forwards the tool requests
/// over `socket` to the engine session.
#[cfg(unix)]
pub fn run_stdio(socket: &Path) -> io::Result<()> {
    let stream = UnixStream::connect(socket)?;
    let mut writer = stream.try_clone()?;
    let mut replies = BufReader::new(stream);
    let mut stdin = io::stdin().lock();
    let mut out = io::stdout().lock();
    // Set when a session's reply could not be read whole: the rest of that line is not a reply, so
    // the server stops after answering.
    let mut broken = false;
    loop {
        let line = match read_line_capped(&mut stdin, MAX_LINE_BYTES)? {
            Line::Text(line) => line,
            Line::TooLong => {
                writeln!(
                    out,
                    "{}",
                    failure(&Value::Null, INVALID_REQUEST, LINE_TOO_LONG)
                )?;
                out.flush()?;
                return Ok(());
            }
            Line::End => return Ok(()),
        };
        if line.trim().is_empty() {
            continue;
        }
        let reply = answer(&line, |request| {
            forward(request, &mut writer, &mut replies, &mut broken)
        });
        if let Some(reply) = reply {
            writeln!(out, "{reply}")?;
            out.flush()?;
        }
        if broken {
            return Ok(());
        }
    }
}

/// Sends one request to the session and returns its reply. A session that cannot be reached is an
/// error reply to the request, not a crash of the engine's server.
#[cfg(unix)]
fn forward(
    request: &Value,
    writer: &mut UnixStream,
    replies: &mut BufReader<UnixStream>,
    broken: &mut bool,
) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let sent = writeln!(writer, "{request}").and_then(|()| writer.flush());
    let read = sent.and_then(|()| read_line_capped(replies, MAX_LINE_BYTES));
    match read {
        Ok(Line::Text(line)) => serde_json::from_str(&line)
            .unwrap_or_else(|_| failure(&id, INTERNAL_ERROR, "the plaza's tools gave a bad reply")),
        Ok(Line::TooLong) => {
            *broken = true;
            failure(
                &id,
                INTERNAL_ERROR,
                "the plaza's tools gave a reply longer than 8 MiB",
            )
        }
        Err(_) => {
            *broken = true;
            failure(&id, INTERNAL_ERROR, "the plaza's tools are not reachable")
        }
        Ok(Line::End) => failure(&id, INTERNAL_ERROR, "the plaza's tools are not reachable"),
    }
}

/// Without unix sockets there is no engine session to serve: the tools are offered only where the
/// socket can be bound, which is every host the harness runs on.
#[cfg(not(unix))]
pub struct Socket;

#[cfg(not(unix))]
impl Socket {
    pub fn bind(_dir: &Path, tools: &[ToolSpec]) -> io::Result<Option<Self>> {
        if tools.is_empty() {
            Ok(None)
        } else {
            Err(io::Error::other(
                "the plaza's tools need a unix socket, which this host does not have",
            ))
        }
    }

    pub fn path(&self) -> &Path {
        unreachable!("a socket is never bound on this host")
    }

    pub fn serve(&self, _: &[ToolSpec], _: &dyn EngineLink, _: Instant, _: &AtomicBool) {
        unreachable!("a socket is never bound on this host")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::link::Verdict;
    use crate::helpers::{self, Board};
    use apz_proto::wire::AgentProfile;

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
            deferred: false,
            name: name.to_string(),
            description: format!("the {name} tool"),
            input_schema: json!({"type": "object"}),
        }
    }

    /// A link whose `board_list` answers with the column it was asked for, and which offers no
    /// decisions of its own.
    struct Fake;

    impl EngineLink for Fake {
        fn decide(&self, _: &str, _: &Value, _: Instant) -> Verdict {
            Verdict::Allow
        }
        fn update(&self, _: &str, _: &str, _: &Value) {}
        fn cancelled(&self) -> bool {
            false
        }
        fn call(&self, name: &str, input: &Value, _: Instant) -> Result<String, String> {
            match name {
                "board_list" => Ok(format!(
                    "cards in {}",
                    input["column"].as_str().unwrap_or_default()
                )),
                _ => Err(format!("{name} failed")),
            }
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apz-harness-acp-mcp-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    #[test]
    fn plaza_tools_are_the_profiles_tools_that_belong_to_a_group() {
        let profile = [
            spec("weft_status"),
            spec("symbols"),
            spec("run_tests"),
            spec("x_fmt"),
            spec("plaza_skills"),
            spec("journal_note"),
            spec("memory_search"),
            spec("plaza_message_seat"),
            spec("mcp__docs__search"),
            spec("board_list"),
            spec("run"),
            spec("read"),
            spec("edit"),
            spec("find"),
            spec("fetch_url"),
            spec("ide_read"),
            spec("code_engine"),
        ];
        let names: Vec<String> = plaza_tools(&profile)
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert_eq!(
            names,
            [
                "weft_status",
                "symbols",
                "run_tests",
                "x_fmt",
                "plaza_skills",
                "journal_note",
                "memory_search",
                "plaza_message_seat",
                "mcp__docs__search",
            ]
        );
    }

    #[test]
    fn each_group_takes_its_own_tools_and_nothing_the_engine_has_itself() {
        assert_eq!(group_of("weft_touches"), Some(Group::Weft));
        assert_eq!(group_of("weft_change_tree"), Some(Group::Weft));
        assert_eq!(group_of("review_verdict"), Some(Group::Review));
        assert_eq!(group_of("diagnostics"), Some(Group::Code));
        assert_eq!(group_of("run_tests"), Some(Group::Tests));
        assert_eq!(group_of("tests_select"), Some(Group::Tests));
        assert_eq!(group_of("x_lint"), Some(Group::Library));
        assert_eq!(group_of("plaza_skill_read"), Some(Group::Skills));
        assert_eq!(group_of("await_helper"), Some(Group::Helpers));
        assert_eq!(group_of("thread_answer"), Some(Group::Talk));
        assert_eq!(group_of("handover_write"), Some(Group::Journal));
        assert_eq!(group_of("memory_propose"), Some(Group::Memory));
        assert_eq!(group_of("mcp__docs__search"), Some(Group::Owner));
        // The engine's own tools, the board and the tasks are not the plaza's to serve here.
        for name in [
            "read",
            "edit",
            "find",
            "search",
            "run",
            "board_list",
            "plaza_tasks",
        ] {
            assert_eq!(group_of(name), None, "{name}");
        }
    }

    /// A link that runs the helper tools on `board` for work item 7, as the session does for a call the hub
    /// lets run in the VM. The helper's own run is a stand-in: see the test below.
    struct Helpers<'a> {
        board: &'a Board,
        profile: AgentProfile,
    }

    impl EngineLink for Helpers<'_> {
        fn decide(&self, _: &str, _: &Value, _: Instant) -> Verdict {
            Verdict::Allow
        }
        fn update(&self, _: &str, _: &str, _: &Value) {}
        fn cancelled(&self) -> bool {
            false
        }
        fn call(&self, name: &str, input: &Value, _: Instant) -> Result<String, String> {
            helpers::run_tool(self.board, 7, Some(&self.profile), None, name, input)
        }
    }

    #[test]
    fn a_helper_started_and_awaited_through_the_server_returns_its_report() {
        let profile: AgentProfile = serde_json::from_value(json!({
            "name": "ada", "role": "builder", "model": "anthropic/x",
            "system_parts": [{"text": "be brief"}],
            "tools": [{"name": "read", "description": "read", "input_schema": {"type": "object"}}],
            "max_turns": 4,
        }))
        .expect("a profile");
        let (board, events) = Board::new(1);
        let link = Helpers {
            board: &board,
            profile,
        };
        let tools = [spec("spawn_helper"), spec("await_helper")];
        let deadline = Instant::now() + Duration::from_secs(10);
        let report = thread::scope(|scope| {
            // The dispatcher's part: the one helper the board starts ends with its report.
            let board = &board;
            scope.spawn(move || {
                let Ok(helpers::Event::Start(start)) = events.recv_timeout(Duration::from_secs(5))
                else {
                    panic!("the helper was not started");
                };
                assert_eq!(start.job, "find the parser");
                board.finish(
                    start.id,
                    helpers::Report {
                        status: helpers::Status::Done,
                        text: "the parser is in lib.rs".to_string(),
                        branch: None,
                        stat: None,
                    },
                );
            });
            let spawned = handle(
                &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                        "params": {"name": "spawn_helper", "arguments": {"job": "find the parser"}}}),
                &tools,
                &link,
                deadline,
            );
            assert_eq!(spawned["result"]["isError"], false, "{spawned}");
            let started = spawned["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default();
            assert!(started.starts_with("started helper h1"), "{started}");
            handle(
                &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                        "params": {"name": "await_helper", "arguments": {"id": "h1", "timeout_s": 5}}}),
                &tools,
                &link,
                deadline,
            )
        });
        assert_eq!(report["result"]["isError"], false, "{report}");
        assert_eq!(
            report["result"]["content"][0]["text"],
            "helper h1 ended: the parser is in lib.rs"
        );
    }

    #[test]
    fn answer_replies_forwards_and_refuses_as_the_protocol_asks() {
        // Checked: answer replies to the handshake and ping and leaves notifications unanswered.
        {
            let init = answer(
                r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#,
                |_| panic!("initialize is not forwarded"),
            )
            .expect("a reply");
            let init: Value = serde_json::from_str(&init).expect("json");
            assert_eq!(init["id"], 0);
            assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
            assert_eq!(init["result"]["capabilities"], json!({"tools": {}}));
            assert_eq!(init["result"]["serverInfo"]["name"], "plaza");

            let ping = answer(r#"{"jsonrpc":"2.0","id":"p","method":"ping"}"#, |_| {
                panic!("ping is not forwarded")
            })
            .expect("a reply");
            assert_eq!(
                serde_json::from_str::<Value>(&ping).expect("json")["result"],
                json!({})
            );

            let initialized = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
            assert_eq!(answer(initialized, |_| panic!("not forwarded")), None);
        }
        // Checked: answer forwards the tool requests and refuses what it does not know.
        {
            let mut forwarded = Vec::new();
            let reply = answer(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"board_list"}}"#,
                |request| {
                    forwarded.push(request["method"].clone());
                    json!({"jsonrpc": "2.0", "id": 3, "result": {"isError": false}})
                },
            )
            .expect("a reply");
            assert_eq!(forwarded, [json!("tools/call")]);
            assert!(reply.contains(r#""isError":false"#), "{reply}");

            let unknown = answer(
                r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#,
                |_| panic!("not forwarded"),
            )
            .expect("a reply");
            assert!(unknown.contains(r#""code":-32601"#), "{unknown}");

            let broken = answer("{not json", |_| panic!("not forwarded")).expect("a reply");
            assert!(broken.contains(r#""code":-32700"#), "{broken}");
        }
    }

    #[test]
    fn handle_lists_the_offered_tools_and_calls_only_those_through_the_link() {
        let tools = [spec("board_list")];
        let deadline = Instant::now() + Duration::from_secs(5);
        let listed = handle(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            &tools,
            &Fake,
            deadline,
        );
        assert_eq!(listed["result"]["tools"][0]["name"], "board_list");
        assert_eq!(
            listed["result"]["tools"][0]["inputSchema"],
            json!({"type": "object"})
        );

        let called = handle(
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": {"name": "board_list", "arguments": {"column": "todo"}}}),
            &tools,
            &Fake,
            deadline,
        );
        assert_eq!(called["result"]["content"][0]["text"], "cards in todo");
        assert_eq!(called["result"]["isError"], false);

        let failed = handle(
            &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                    "params": {"name": "board_list", "arguments": "todo"}}),
            &tools,
            &Fake,
            deadline,
        );
        assert_eq!(failed["result"]["isError"], true);

        let refused = handle(
            &json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                    "params": {"name": "run", "arguments": {}}}),
            &tools,
            &Fake,
            deadline,
        );
        assert_eq!(refused["error"]["code"], INVALID_PARAMS);
    }

    #[cfg(unix)]
    #[test]
    fn a_turns_socket_round_trips_the_stdio_servers_requests_and_is_removed_after() {
        let dir = scratch("socket");
        let tools = [spec("board_list")];
        let socket = Socket::bind(&dir.join(".plaza"), &tools)
            .expect("a socket")
            .expect("one for the tools");
        let path = socket.path().to_path_buf();
        let mode = fs::metadata(&path)
            .expect("the socket")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

        let stop = AtomicBool::new(false);
        let deadline = Instant::now() + Duration::from_secs(5);
        thread::scope(|scope| {
            scope.spawn(|| socket.serve(&tools, &Fake, deadline, &stop));

            // The engine's server: its stdio lines go through `answer`, the forwarding to the socket.
            let stream = UnixStream::connect(&path).expect("the engine's server connects");
            let mut writer = stream.try_clone().expect("a second handle");
            let mut replies = BufReader::new(stream);
            let listed = answer(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
                |request| forward(request, &mut writer, &mut replies, &mut false),
            )
            .expect("a reply");
            assert!(listed.contains(r#""name":"board_list""#), "{listed}");
            let called = answer(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"board_list","arguments":{"column":"doing"}}}"#,
                |request| forward(request, &mut writer, &mut replies, &mut false),
            )
            .expect("a reply");
            assert!(called.contains("cards in doing"), "{called}");
            stop.store(true, Ordering::SeqCst);
        });
        drop(socket);
        assert!(!path.exists(), "the socket file is removed with the turn");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_is_read_up_to_the_cap_and_a_longer_one_is_too_long() {
        let mut short: &[u8] = b"one\n";
        assert_eq!(
            read_line_capped(&mut short, 8).expect("a line"),
            Line::Text("one".to_string())
        );
        assert_eq!(read_line_capped(&mut short, 8).expect("the end"), Line::End);
        // Eight bytes with the newline exactly fit; nine do not.
        let mut exact: &[u8] = b"abcdefg\n";
        assert_eq!(
            read_line_capped(&mut exact, 8).expect("a line"),
            Line::Text("abcdefg".to_string())
        );
        let mut long: &[u8] = b"abcdefgh\n";
        assert_eq!(
            read_line_capped(&mut long, 8).expect("a line"),
            Line::TooLong
        );
    }

    #[test]
    fn the_socket_serves_eight_connections_and_a_finished_one_frees_its_slot() {
        let dir = scratch("cap");
        let tools = vec![spec("board_list")];
        let socket = Socket::bind(&dir, &tools).expect("bind").expect("a socket");
        let path = socket.path().to_path_buf();
        let stop = AtomicBool::new(false);
        let deadline = Instant::now() + Duration::from_secs(30);
        thread::scope(|scope| {
            scope.spawn(|| socket.serve(&tools, &Fake, deadline, &stop));
            let mut open: Vec<UnixStream> = (0..MAX_CONNECTIONS)
                .map(|_| UnixStream::connect(&path).expect("connects"))
                .collect();
            // The ninth is closed as it is accepted: its read ends at once.
            let extra = UnixStream::connect(&path).expect("connects");
            let mut reply = String::new();
            assert_eq!(
                BufReader::new(&extra)
                    .read_line(&mut reply)
                    .expect("a closed read"),
                0
            );
            // A finished connection leaves the set, so the next one is served.
            drop(open.remove(0));
            thread::sleep(Duration::from_millis(100));
            let mut served = UnixStream::connect(&path).expect("connects");
            writeln!(
                served,
                r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list"}}"#
            )
            .expect("a request");
            let mut listed = String::new();
            BufReader::new(&served)
                .read_line(&mut listed)
                .expect("a reply");
            assert!(listed.contains(r#""name":"board_list""#), "{listed}");
            drop(served);
            drop(open);
            stop.store(true, Ordering::SeqCst);
        });
        drop(socket);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_tools_means_no_socket_and_no_server() {
        let dir = scratch("none");
        assert!(Socket::bind(&dir, &[]).expect("bind").is_none());
        assert_eq!(servers(None).expect("servers"), json!([]));
        let _ = fs::remove_dir_all(&dir);
    }
}
