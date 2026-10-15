//! The hub as an ACP agent: editors (Zed, JetBrains IDEs, Neovim, any Agent Client Protocol client)
//! prompt a plaza agent, watch its work stream in, and answer its approvals, through
//! `agentplaza acp` on the owner's machine.
//!
//! `GET /v1/acp` upgrades to a WebSocket (subprotocol [`PROTOCOL`]); each text message is one
//! JSON-RPC 2.0 message, and the connection is an `apz_acp::rpc::Peer` with the hub as the agent.
//! The owner may connect (the hub's front door, `hub_auth`: loopback, the owner token, a console
//! session), and so may a hub MCP token of operate or govern scope, limited to the agents its
//! allowlist names (see [`authenticate`]). It is off until the owner turns on `acp_enabled` in the
//! settings. Each editor session is a thread of its own on the agent (`acp-<id>`), so it never mixes
//! with the owner's console chat.
//!
//! - [`store`]: the sessions, kept in Postgres with the rest of the plaza state.
//! - [`agent`]: the ACP methods, over the [`agent::Hub`] the plaza gives them ([`app_hub`]).
//! - [`feed`]: a session's work as ACP `session/update`s, live from the bus or replayed.
//! - [`apz_acp::compat`]: the ACP v2 draft, next to v1. Off until the owner turns on `acp_v2`; an editor
//!   that asks for protocol 2 then gets it, and every other editor keeps v1. On a v2 connection the
//!   updates are translated to v2, a prompt answers with its message id and its turn runs after the
//!   answer (`state_update`s carry the running and the end), `session/load` and `session/set_mode` are
//!   not offered, and write-back is never offered (v2 has no `fs/write_text_file`).
//!
//! An agent never reaches the owner's machine: the editor's MCP servers are not connected, the
//! hub never calls the editor's `fs/*` or `terminal/*`, and the agent works in its own VM.

pub mod agent;
pub mod app_hub;
pub mod feed;
pub mod store;

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU16};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use apz_acp::rpc::Peer;
use apz_acp::types;
use apz_http::ws::{self, Limits, Message, Origins, Socket};
use apz_http::{Request, Response, Router};
use serde_json::{Value, json};

use crate::events::Slots;
use crate::hub_auth::is_owner;
use crate::hub_mcp::audit::CallRecord;
use crate::hub_mcp::auth::{Scope, TokenRecord, bearer, sha256_hex};
use crate::state::App;
use agent::{Caller, Conn, Detached};
use app_hub::AppHub;

/// The WebSocket subprotocol of `/v1/acp`.
pub const PROTOCOL: &str = "apz.acp.v1";

/// How long the socket waits for a message before it looks at the agent's lines again.
const POLL: Duration = Duration::from_millis(250);

/// The editors' sockets open now, under the same caps as the event sockets.
static SOCKETS: Slots = Slots::new();

/// What a connection's handlers share: the app, the agent the editor asked for, its client name, who it acts
/// for, and the protocol version `initialize` negotiated (1 until then).
struct ConnState {
    app: Arc<App>,
    agent: Option<String>,
    client: Mutex<String>,
    editor_writes: AtomicBool,
    version: AtomicU16,
    caller: Caller,
}

/// Hands a v2 prompt's turn to a thread of its own, once the request is answered. The thread builds the hub
/// from the connection's state, as the request does, and runs the turn with its own clone of the editor's peer.
fn detacher(state: Arc<ConnState>, peer: Peer) -> impl Fn(Detached) {
    move |turn| {
        let (state, peer) = (Arc::clone(&state), peer.clone());
        thread::spawn(move || {
            let hub = AppHub { app: &state.app };
            turn(&hub, &peer);
        });
    }
}

/// `GET /v1/acp` (the WebSocket) and `GET /v1/acp/sessions` (the console's list).
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/acp", |app, req, _| {
            let caller = match authenticate(&app, req) {
                Ok(caller) => caller,
                Err(refused) => return refused,
            };
            if !app.settings.get().acp_enabled {
                return Response::error(
                    409,
                    "the hub's ACP server is off; turn on Editors (ACP) in Setup › MCP › This hub as a server",
                );
            }
            let Some(slot) = SOCKETS.take(req.peer) else {
                return Response::error(429, "too many open editor sockets; close one first");
            };
            let agent = req.query_param("agent").map(str::to_string);
            let weak = app.handle();
            ws::accept(
                req,
                Origins::SameHost,
                Some(PROTOCOL),
                Limits::default(),
                move |mut socket| {
                    let _slot = slot;
                    match weak.upgrade() {
                        Some(app) => serve(&mut socket, app, agent, caller),
                        None => socket.close(1013, "the hub is shutting down"),
                    }
                },
            )
        })
        .get("/v1/acp/sessions", |app, req, _| {
            Response::json(200, &app.acp_sessions.list(req.query_param("cwd")))
        })
}

/// Who may open an editor's socket. The owner (see `hub_auth::is_owner`) acts as the owner. Anyone else
/// must present a hub MCP token that the store accepts and whose scope admits an editor (see [`admit`]).
/// This runs before the ACP switch, so a refused caller learns nothing about the hub's ACP server.
fn authenticate(app: &App, req: &Request) -> Result<Caller, Response> {
    if is_owner(app, req) {
        return Ok(Caller::Owner);
    }
    let Some(token) = bearer(req) else {
        return Err(unauthorized());
    };
    let record = app
        .hub_mcp
        .tokens
        .verify(token, apz_plaza::now_ms())
        .map_err(|_| unauthorized())?;
    admit(&record).map_err(|(status, message)| Response::json(status, &json!({ "error": message })))
}

fn unauthorized() -> Response {
    Response::json(
        401,
        &json!({ "error": "sign in with the owner token or an operate/govern hub MCP token" }),
    )
    .with_header("www-authenticate", "Bearer")
}

/// What a hub MCP token's record lets its holder do on the ACP surface: operate or govern, never observe.
/// The record's allowlist names the agents it may drive (see [`agents_of`]).
fn admit(record: &TokenRecord) -> Result<Caller, (u16, &'static str)> {
    if record.scope < Scope::Operate {
        return Err((
            403,
            "this token's scope is observe; an editor needs an operate or govern token",
        ));
    }
    Ok(Caller::Token {
        id: record.id.clone(),
        scope: record.scope,
        agents: agents_of(&record.allowlist),
    })
}

/// The agents an allowlist names: its `acp:<name or cid>` entries, without the prefix. An allowlist with
/// no such entry names no agent, which means every agent.
fn agents_of(allowlist: &[String]) -> Vec<String> {
    allowlist
        .iter()
        .filter_map(|entry| entry.strip_prefix("acp:"))
        .filter(|agent| !agent.is_empty())
        .map(str::to_string)
        .collect()
}

/// Runs one editor connection until either side ends it. The editor's messages go into the
/// [`Peer`] as lines, and the Peer's lines go back to the editor as messages.
fn serve(socket: &mut Socket, app: Arc<App>, agent: Option<String>, caller: Caller) {
    let (Ok((to_peer_read, mut to_peer)), Ok((from_peer, from_peer_write))) =
        (io::pipe(), io::pipe())
    else {
        socket.close(1011, "cannot start the connection");
        return;
    };
    let state = Arc::new(ConnState {
        app,
        agent,
        client: Mutex::new(String::new()),
        editor_writes: AtomicBool::new(false),
        version: AtomicU16::new(1),
        caller,
    });
    let on_request = {
        let state = Arc::clone(&state);
        move |peer: &Peer, method: &str, params: Value| {
            let hub = AppHub { app: &state.app };
            let detach = detacher(Arc::clone(&state), peer.clone());
            let conn = Conn {
                hub: &hub,
                sessions: &state.app.acp_sessions,
                agent: state.agent.clone(),
                client: &state.client,
                editor_writes: &state.editor_writes,
                version: &state.version,
                allow_v2: state.app.settings.get().acp_v2,
                detach: &detach,
                caller: state.caller.clone(),
            };
            metered(&state.app, &state.caller, method, params, |params| {
                agent::request(&conn, peer, method, params)
            })
        }
    };
    let on_notification = {
        let state = Arc::clone(&state);
        move |peer: &Peer, method: &str, params: Value| {
            let hub = AppHub { app: &state.app };
            let detach = detacher(Arc::clone(&state), peer.clone());
            let conn = Conn {
                hub: &hub,
                sessions: &state.app.acp_sessions,
                agent: state.agent.clone(),
                client: &state.client,
                editor_writes: &state.editor_writes,
                version: &state.version,
                allow_v2: state.app.settings.get().acp_v2,
                detach: &detach,
                caller: state.caller.clone(),
            };
            // A notification has no answer, so a refused one is only audited.
            let _ = metered(&state.app, &state.caller, method, params, |params| {
                agent::notification(&conn, method, params);
                Ok::<(), types::Error>(())
            });
        }
    };
    let peer = Peer::new(to_peer_read, from_peer_write, on_request, on_notification);
    let lines = line_reader(from_peer);
    'relay: loop {
        // The Peer's lines first, so an answer goes out before the next message is read.
        loop {
            match lines.try_recv() {
                Ok(line) => {
                    if socket.send(&Message::Text(line)).is_err() {
                        break 'relay;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    socket.close(1000, "the agent connection ended");
                    break 'relay;
                }
            }
        }
        match socket.recv(POLL) {
            Ok(None) => {}
            Ok(Some(Message::Text(text))) => {
                if write_message(&mut to_peer, &text).is_err() {
                    break 'relay;
                }
            }
            Ok(Some(Message::Binary(_))) => {
                socket.close(1003, "text messages only");
                break 'relay;
            }
            Err(_) => break 'relay,
        }
    }
    // Ending the editor's stream ends the Peer's reader, and with it the Peer's threads.
    peer.close();
    drop(to_peer);
}

/// Runs one editor message. A token's message counts against the token's rate limit (the window `/mcp`
/// shares) and is audited as `acp/<method>`; the owner's runs as it always has. A message over the limit is
/// refused with why, and audited as refused.
fn metered<T>(
    app: &App,
    caller: &Caller,
    method: &str,
    params: Value,
    call: impl FnOnce(Value) -> Result<T, types::Error>,
) -> Result<T, types::Error> {
    let Caller::Token { id, scope, .. } = caller else {
        return call(params);
    };
    let now = apz_plaza::now_ms();
    let digest = sha256_hex(params.to_string().as_bytes());
    let audit = |ok: bool, error: Option<&str>| {
        app.hub_mcp.audit.record(CallRecord {
            at_ms: now,
            token_id: Some(id.clone()),
            client_ip: None,
            tool: format!("acp/{method}").chars().take(64).collect(),
            args_sha256: digest.clone(),
            ok,
            error: error.map(str::to_string),
            duration_ms: apz_plaza::now_ms().saturating_sub(now),
        })
    };
    if let Err(wait_ms) = app.hub_mcp.limiter.take(id, *scope, now) {
        audit(false, Some("rate_limited"));
        let secs = wait_ms.div_ceil(1000);
        return Err(
            types::Error::invalid_request().data(json!(format!("rate limited, retry in {secs}s")))
        );
    }
    app.hub_mcp.tokens.record_use(id, now);
    let result = call(params);
    audit(result.is_ok(), result.is_err().then_some("rpc_error"));
    result
}

/// Writes one editor message as one line. A raw newline cannot sit inside a JSON string, so one
/// between tokens is only whitespace and becomes a space.
fn write_message(stream: &mut impl Write, text: &str) -> io::Result<()> {
    stream.write_all(text.replace(['\r', '\n'], " ").as_bytes())?;
    stream.write_all(b"\n")
}

/// Reads the lines of `stream` on a thread of its own, so the socket loop can poll them. The
/// channel disconnects when the stream ends.
fn line_reader(stream: impl Read + Send + 'static) -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bridge_carries_each_message_as_one_line_both_ways() {
        let (to_peer_read, mut to_peer) = io::pipe().unwrap();
        let (from_peer, mut from_peer_write) = io::pipe().unwrap();
        let peer_input = line_reader(to_peer_read);
        let editor_output = line_reader(from_peer);
        // An editor message, even one with a newline between its tokens, reaches the Peer as one line.
        write_message(&mut to_peer, "{\"jsonrpc\":\"2.0\",\n\"id\":1}").unwrap();
        let timeout = Duration::from_secs(5);
        assert_eq!(
            peer_input.recv_timeout(timeout).unwrap(),
            "{\"jsonrpc\":\"2.0\", \"id\":1}"
        );
        // A line from the Peer reaches the editor as it was.
        from_peer_write
            .write_all(b"{\"id\":1,\"result\":{}}\n")
            .unwrap();
        assert_eq!(
            editor_output.recv_timeout(timeout).unwrap(),
            "{\"id\":1,\"result\":{}}"
        );
    }

    fn token(scope: Scope, allowlist: &[&str]) -> TokenRecord {
        TokenRecord {
            id: "mcp_test".into(),
            name: "editor".into(),
            scope,
            allowlist: allowlist.iter().map(|entry| entry.to_string()).collect(),
            created_ms: 0,
            expires_ms: None,
            last_used_ms: None,
            calls: 0,
            revoked_at_ms: None,
        }
    }

    #[test]
    fn an_observe_token_is_refused_and_operate_and_govern_are_admitted() {
        assert_eq!(
            admit(&token(Scope::Observe, &[])),
            Err((
                403,
                "this token's scope is observe; an editor needs an operate or govern token"
            ))
        );
        for scope in [Scope::Operate, Scope::Govern] {
            assert_eq!(
                admit(&token(scope, &[])),
                Ok(Caller::Token {
                    id: "mcp_test".into(),
                    scope,
                    agents: Vec::new(),
                })
            );
        }
    }

    #[test]
    fn the_allowlist_names_the_agents_its_acp_entries_name() {
        let allowlist = ["acp:research", "fs.read", "acp:7", "acp:"];
        assert_eq!(agents_of(&allowlist.map(str::to_string)), ["research", "7"]);
        // No acp entry names no agent, so the token may drive every agent its scope admits.
        assert!(agents_of(&["fs.read".to_string()]).is_empty());
        let admitted = admit(&token(Scope::Operate, &["acp:research"])).unwrap();
        assert_eq!(
            admitted,
            Caller::Token {
                id: "mcp_test".into(),
                scope: Scope::Operate,
                agents: vec!["research".into()],
            }
        );
    }
}
