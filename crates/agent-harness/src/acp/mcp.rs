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
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

use apz_proto::chat::ToolSpec;
use serde_json::{Value, json};

use super::link::EngineLink;
use crate::agent::is_guest_tool;

/// The MCP protocol version the server speaks.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// The name the engine knows the server by (its `mcpServers` entry).
const SERVER_NAME: &str = "plaza";
/// How long the accept loop sleeps between checks of its stop flag.
#[cfg(unix)]
const ACCEPT_POLL: Duration = Duration::from_millis(20);

const PARSE_ERROR: i64 = -32700;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

/// The tools an engine may call through the plaza's MCP server: the profile's tools the hub runs
/// (the board, tasks, messages, memory, library). The harness's own tools are left out, since the
/// engine has its own through ACP; so are the owner's IDE tools and `code_engine` itself.
pub fn plaza_tools(profile_tools: &[ToolSpec]) -> Vec<ToolSpec> {
    profile_tools
        .iter()
        .filter(|tool| {
            tool.name != "code_engine"
                && !tool.name.starts_with("ide_")
                && !is_guest_tool(&tool.name)
        })
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

/// The engine session's socket for one turn: bound before the engine starts, removed when the turn
/// ends. Mode 0600, so only the workspace's own processes can connect.
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
        let path = dir.join(format!("engine-mcp-{}-{n}.sock", std::process::id()));
        let _ = fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
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
        let mut open = Vec::new();
        thread::scope(|scope| {
            while !stop.load(Ordering::SeqCst) {
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        // Accepted sockets are blocking: a connection's thread waits for its lines.
                        if stream.set_nonblocking(false).is_err() {
                            continue;
                        }
                        if let Ok(clone) = stream.try_clone() {
                            open.push(clone);
                        }
                        scope.spawn(move || connection(stream, tools, link, deadline));
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(ACCEPT_POLL);
                    }
                    Err(_) => break,
                }
            }
            // Closing the connections ends their reads, so the threads end and the scope returns.
            for stream in &open {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        });
    }
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
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else {
            return;
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
    let stdin = io::stdin();
    let mut out = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = answer(&line, |request| forward(request, &mut writer, &mut replies));
        if let Some(reply) = reply {
            writeln!(out, "{reply}")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Sends one request to the session and returns its reply. A session that cannot be reached is an
/// error reply to the request, not a crash of the engine's server.
#[cfg(unix)]
fn forward(request: &Value, writer: &mut UnixStream, replies: &mut BufReader<UnixStream>) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let sent = writeln!(writer, "{request}").and_then(|()| writer.flush());
    let mut line = String::new();
    let read = sent.and_then(|()| replies.read_line(&mut line));
    match read {
        Ok(count) if count > 0 => serde_json::from_str(&line)
            .unwrap_or_else(|_| failure(&id, INTERNAL_ERROR, "the plaza's tools gave a bad reply")),
        _ => failure(&id, INTERNAL_ERROR, "the plaza's tools are not reachable"),
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

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
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
    fn plaza_tools_are_the_profiles_tools_the_hub_runs() {
        let profile = [
            spec("board_list"),
            spec("task_list"),
            spec("bash"),
            spec("list"),
            spec("fetch_url"),
            spec("edit_file"),
            spec("ide_read"),
            spec("code_engine"),
            spec("memory_write"),
        ];
        let names: Vec<String> = plaza_tools(&profile)
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert_eq!(names, ["board_list", "task_list", "memory_write"]);
    }

    #[test]
    fn answer_replies_to_the_handshake_and_ping_and_leaves_notifications_unanswered() {
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

    #[test]
    fn answer_forwards_the_tool_requests_and_refuses_what_it_does_not_know() {
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
                    "params": {"name": "bash", "arguments": {}}}),
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
                |request| forward(request, &mut writer, &mut replies),
            )
            .expect("a reply");
            assert!(listed.contains(r#""name":"board_list""#), "{listed}");
            let called = answer(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"board_list","arguments":{"column":"doing"}}}"#,
                |request| forward(request, &mut writer, &mut replies),
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
    fn no_tools_means_no_socket_and_no_server() {
        let dir = scratch("none");
        assert!(Socket::bind(&dir, &[]).expect("bind").is_none());
        assert_eq!(servers(None).expect("servers"), json!([]));
        let _ = fs::remove_dir_all(&dir);
    }
}
