//! The hub as an ACP agent: editors (Zed, JetBrains IDEs, Neovim, any Agent Client Protocol client)
//! prompt a plaza agent, watch its work stream in, and answer its approvals, through
//! `agentplaza acp` on the owner's machine.
//!
//! `GET /v1/acp` upgrades to a WebSocket (subprotocol [`PROTOCOL`]); each text message is one
//! JSON-RPC 2.0 message, and the connection is an `apz_acp::rpc::Peer` with the hub as the agent.
//! It is an owner route (the hub's front door, `hub_auth`: loopback or the owner token), off until
//! the owner turns on `acp_enabled` in the settings. Each editor session is a thread of its own on
//! the agent (`acp-<id>`), so it never mixes with the owner's console chat.
//!
//! - [`store`]: the sessions, kept in Postgres with the rest of the plaza state.
//! - [`agent`]: the ACP methods, over the [`agent::Hub`] the plaza gives them ([`app_hub`]).
//! - [`feed`]: a session's work as ACP `session/update`s, live from the bus or replayed.
//!
//! An agent never reaches the owner's machine: the editor's MCP servers are not connected, the
//! hub never calls the editor's `fs/*` or `terminal/*`, and the agent works in its own VM.

pub mod agent;
pub mod app_hub;
pub mod feed;
pub mod store;

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use apz_acp::rpc::Peer;
use apz_http::ws::{self, Limits, Message, Origins, Socket};
use apz_http::{Response, Router};
use serde_json::Value;

use crate::events::Slots;
use crate::state::App;
use agent::Conn;
use app_hub::AppHub;

/// The WebSocket subprotocol of `/v1/acp`.
pub const PROTOCOL: &str = "apz.acp.v1";

/// How long the socket waits for a message before it looks at the agent's lines again.
const POLL: Duration = Duration::from_millis(250);

/// The editors' sockets open now, under the same caps as the event sockets.
static SOCKETS: Slots = Slots::new();

/// What a connection's handlers share: the app, the agent the editor asked for, and its client name.
struct ConnState {
    app: Arc<App>,
    agent: Option<String>,
    client: Mutex<String>,
}

/// `GET /v1/acp` (the WebSocket) and `GET /v1/acp/sessions` (the console's list).
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/acp", |app, req, _| {
            if !app.settings.get().acp_enabled {
                return Response::error(
                    409,
                    "the hub's ACP server is off; turn it on in Settings › Editors",
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
                        Some(app) => serve(&mut socket, app, agent),
                        None => socket.close(1013, "the hub is shutting down"),
                    }
                },
            )
        })
        .get("/v1/acp/sessions", |app, req, _| {
            Response::json(200, &app.acp_sessions.list(req.query_param("cwd")))
        })
}

/// Runs one editor connection until either side ends it. The editor's messages go into the
/// [`Peer`] as lines, and the Peer's lines go back to the editor as messages.
fn serve(socket: &mut Socket, app: Arc<App>, agent: Option<String>) {
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
    });
    let on_request = {
        let state = Arc::clone(&state);
        move |peer: &Peer, method: &str, params: Value| {
            let hub = AppHub { app: &state.app };
            let conn = Conn {
                hub: &hub,
                sessions: &state.app.acp_sessions,
                agent: state.agent.clone(),
                client: &state.client,
            };
            agent::request(&conn, peer, method, params)
        }
    };
    let on_notification = {
        let state = Arc::clone(&state);
        move |_peer: &Peer, method: &str, params: Value| {
            let hub = AppHub { app: &state.app };
            let conn = Conn {
                hub: &hub,
                sessions: &state.app.acp_sessions,
                agent: state.agent.clone(),
                client: &state.client,
            };
            agent::notification(&conn, method, params);
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
}
