//! The social layer over the daemon's real HTTP surface: the board, the read
//! cursors, the contacts and the conversations — and every rule that can be
//! checked without a live agent.
//!
//! This daemon boots no VMs, so no CID is connected: everything here is the
//! *refusal* matrix (the register's answer, the caps, the bad bodies, the
//! methods) plus the shapes of the views a quiet daemon answers with. What
//! needs two connected agents — a post that lands, a cursor that advances, a
//! delivery that is taken, the award's note and links — is exercised in
//! `tests/smoke.rs` (a real two-role fleet behind `APZ_NODED_SMOKE=1`) and in
//! `src/social.rs`'s own tests against the store.
//!
//! The daemon here is given **no** hub surfaces (`start_with_hubs(…, None,
//! None)`), so nothing in this suite can reach the live `vsockd` — and a `say`
//! never gets far enough to try, because no agent is connected.

mod common;

use std::fs;
use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use common::{request, send, start_with_hubs};
use noded::events::Event;

/// A fresh directory to serve as the console, unique to the calling test.
fn console_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("noded-social-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a console directory");
    dir
}

/// A daemon with no hub surfaces and no agents: every refusal, no delivery.
///
/// The `Server` is returned, not its port: dropping it stops the daemon, so a
/// caller must hold it for as long as it makes requests.
fn daemon(name: &str) -> noded::Server {
    start_with_hubs(console_dir(name), None, None)
}

/// A body inside every cap, for a request whose *other* field is under test.
const GOOD_POST: &str = r#"{"agent_cid":300,"subject":"hiring for t1","body":"the builder is free"}"#;

#[test]
fn an_empty_board_answers_with_an_empty_bounded_view() {
    let server = daemon("board-empty");
    let port = server.port();
    let board = request(port, "GET", "/api/v0/board", None);
    assert_eq!(board.status, 200, "{}", board.body);
    let view = board.json();
    assert_eq!(view["posts"].as_array().expect("posts").len(), 0);
    assert_eq!(view["total"], 0);
    assert_eq!(view["limit"], 256);
    assert_eq!(view["truncated"], false);
    assert_eq!(
        view["unread"].as_array().expect("unread").len(),
        0,
        "no agent is connected, so nobody has an unread count: {view}"
    );
}

#[test]
fn a_post_needs_a_connected_agent_and_a_body_inside_the_caps() {
    let server = daemon("board-post");
    let port = server.port();

    // The author must be a connected agent — the register's own check, with the
    // register's own words, before the board is touched.
    let refused = request(port, "POST", "/api/v0/board", Some(GOOD_POST));
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(
        refused.json()["error"],
        "no live agent has guest CID 300"
    );

    // An out-of-contract body is a 400 naming the field, and it is refused
    // before the register is asked at all.
    for (body, expected) in [
        (
            r#"{"subject":"s","body":"b"}"#,
            "invalid post request: missing field `agent_cid`",
        ),
        (
            r#"{"agent_cid":300,"subject":"  ","body":"b"}"#,
            "subject must not be empty",
        ),
        (
            r#"{"agent_cid":300,"subject":"s","body":""}"#,
            "body must not be empty",
        ),
    ] {
        let reply = request(port, "POST", "/api/v0/board", Some(body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.body);
        let message = reply.json()["error"].as_str().unwrap_or_default().to_string();
        assert!(
            message.starts_with(expected),
            "{body}: expected {expected:?}, got {message:?}"
        );
    }
    // The caps are the contract's, and the over-cap body names it.
    let long = format!(
        r#"{{"agent_cid":300,"subject":"s","body":"{}"}}"#,
        "x".repeat(4097)
    );
    let reply = request(port, "POST", "/api/v0/board", Some(&long));
    assert_eq!(reply.status, 400, "{}", reply.body);
    assert_eq!(reply.json()["error"], "body must be at most 4096 bytes");

    // A task link the ledger does not hold is this endpoint's 400: the board
    // must not point at work that does not exist.
    let dangling = request(
        port,
        "POST",
        "/api/v0/board",
        Some(
            r#"{"agent_cid":300,"subject":"hiring for t9","body":"the builder is free","task":"t9"}"#,
        ),
    );
    assert_eq!(dangling.status, 400, "{}", dangling.body);
    assert_eq!(dangling.json()["error"], "no task with id t9");

    // The board took nothing from any of that.
    let board = request(port, "GET", "/api/v0/board", None).json();
    assert_eq!(board["total"], 0, "{board}");
}

#[test]
fn a_read_cursor_needs_a_connected_agent_and_a_post_to_name() {
    let server = daemon("board-read");
    let port = server.port();

    let refused = request(
        port,
        "POST",
        "/api/v0/board/read",
        Some(r#"{"agent_cid":300,"up_to":"p1"}"#),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.json()["error"], "no live agent has guest CID 300");

    // A body the endpoint cannot read is a 400, whatever the register says.
    let malformed = request(port, "POST", "/api/v0/board/read", Some(r#"{"agent_cid":300}"#));
    assert_eq!(malformed.status, 400, "{}", malformed.body);
    assert!(
        malformed.json()["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("invalid read request: missing field `up_to`"),
        "{}",
        malformed.body
    );

    // The cursor semantics themselves (`up_to` must name a post, must not be
    // past the newest or behind the agent's own) live with the store, which is
    // where the cursor is: they are asserted in `src/social.rs`, and over HTTP
    // in `tests/smoke.rs` with two real agents.
    assert_eq!(
        request(port, "GET", "/api/v0/board/read", None).status,
        405,
        "a cursor advances on a POST"
    );
}

#[test]
fn the_contacts_matrix_refuses_what_it_must_and_answers_a_held_list() {
    let server = daemon("contacts");
    let port = server.port();

    // A CID the daemon knows nothing about is the 404 ...
    let unknown = request(port, "GET", "/api/v0/agents/300/contacts", None);
    assert_eq!(unknown.status, 404, "{}", unknown.body);
    assert_eq!(unknown.json()["error"], "no agent with guest CID 300");

    // ... and a segment that could never name an agent of this node is a 400,
    // before the store is asked.
    for segment in ["x", "0", "4294967296"] {
        let reply = request(port, "GET", &format!("/api/v0/agents/{segment}/contacts"), None);
        assert_eq!(reply.status, 400, "{segment}: {}", reply.body);
    }

    // Adding needs both agents connected: the agent ...
    let refused = request(
        port,
        "POST",
        "/api/v0/agents/300/contacts",
        Some(r#"{"peer_cid":301}"#),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.json()["error"], "no live agent has guest CID 300");

    // ... a body that names no peer at all is the endpoint's 400 ...
    let malformed = request(port, "POST", "/api/v0/agents/300/contacts", Some("{}"));
    assert_eq!(malformed.status, 400, "{}", malformed.body);
    assert!(
        malformed.json()["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("invalid contact request: missing field `peer_cid`"),
        "{}",
        malformed.body
    );

    // ... and the list itself is GET or POST, nothing else.
    assert_eq!(
        request(port, "DELETE", "/api/v0/agents/300/contacts", None).status,
        405
    );
    assert_eq!(
        request(port, "PUT", "/api/v0/agents/300/contacts", Some("{}")).status,
        405
    );
}

#[test]
fn saying_needs_two_connected_agents_and_text_inside_the_cap() {
    let server = daemon("say");
    let port = server.port();

    let refused = request(
        port,
        "POST",
        "/api/v0/agents/300/say",
        Some(r#"{"to_cid":301,"text":"are you free?"}"#),
    );
    assert_eq!(refused.status, 409, "{}", refused.body);
    assert_eq!(refused.json()["error"], "no live agent has guest CID 300");

    // The text is checked before the register is: an empty or over-cap text is
    // a 400 naming it.
    for (body, expected) in [
        (
            r#"{"to_cid":301,"text":"   "}"#,
            "text must not be empty",
        ),
        (
            r#"{"to_cid":301,"text":""}"#,
            "text must not be empty",
        ),
    ] {
        let reply = request(port, "POST", "/api/v0/agents/300/say", Some(body));
        assert_eq!(reply.status, 400, "{body}: {}", reply.body);
        assert_eq!(reply.json()["error"], expected);
    }
    let long = format!(
        r#"{{"to_cid":301,"text":"{}"}}"#,
        "x".repeat(2049)
    );
    let reply = request(port, "POST", "/api/v0/agents/300/say", Some(&long));
    assert_eq!(reply.status, 400, "{}", reply.body);
    assert_eq!(reply.json()["error"], "text must be at most 2048 bytes");

    // A malformed body, a bad CID segment, and the wrong method.
    let malformed = request(port, "POST", "/api/v0/agents/300/say", Some(r#"{"text":"hi"}"#));
    assert_eq!(malformed.status, 400, "{}", malformed.body);
    assert_eq!(
        request(port, "POST", "/api/v0/agents/nope/say", Some(r#"{"to_cid":301,"text":"hi"}"#)).status,
        400
    );
    assert_eq!(
        request(port, "GET", "/api/v0/agents/300/say", None).status,
        405,
        "speaking is a POST"
    );

    // Nothing was recorded: a `say` that never passed the register is not an
    // exchange.
    let record = request(port, "GET", "/api/v0/conversations", None).json();
    assert_eq!(record["total"], 0, "{record}");
}

#[test]
fn an_empty_conversation_record_and_a_pair_thread_are_derived_views() {
    let server = daemon("conversations");
    let port = server.port();

    let record = request(port, "GET", "/api/v0/conversations", None);
    assert_eq!(record.status, 200, "{}", record.body);
    let view = record.json();
    assert_eq!(view["conversations"].as_array().expect("conversations").len(), 0);
    assert_eq!(view["pairs"].as_array().expect("pairs").len(), 0);
    assert_eq!(view["active_secs"], 120, "the contract's window is exported");
    assert_eq!(view["total"], 0);
    assert_eq!(view["limit"], 256);
    assert_eq!(view["truncated"], false);
    assert_eq!(view["pairs_total"], 0);
    assert_eq!(view["pairs_truncated"], false);

    // A pair that has never spoken is an empty thread, not a 404 — and the
    // pair is unordered, so both orders answer the same thread.
    let thread = request(port, "GET", "/api/v0/conversations/300/301", None);
    assert_eq!(thread.status, 200, "{}", thread.body);
    let view = thread.json();
    assert_eq!(view["a"], 300);
    assert_eq!(view["b"], 301);
    assert_eq!(view["active"], false);
    assert_eq!(view["active_secs"], 120);
    assert_eq!(view["messages"].as_array().expect("messages").len(), 0);
    assert_eq!(view["total"], 0);
    assert_eq!(view["limit"], 256);
    assert_eq!(view["truncated"], false);
    let reversed = request(port, "GET", "/api/v0/conversations/301/300", None).json();
    assert_eq!(reversed["a"], 301, "the path's order is echoed, not sorted");
    assert_eq!(reversed["total"], 0);

    // The pair's CIDs are held to the transport's range, and the path must be
    // exactly a pair.
    assert_eq!(
        request(port, "GET", "/api/v0/conversations/300/x", None).status,
        400
    );
    assert_eq!(
        request(port, "GET", "/api/v0/conversations/300/301/302", None).status,
        404,
        "a thread is a pair, not a path"
    );
    assert_eq!(
        request(port, "GET", "/api/v0/conversations/300", None).status,
        404
    );
    assert_eq!(
        request(port, "POST", "/api/v0/conversations/300/301", None).status,
        405
    );
    assert_eq!(
        request(port, "DELETE", "/api/v0/conversations", None).status,
        405
    );
}

#[test]
fn the_stream_carries_the_board_and_conversation_frames() {
    let server = daemon("stream");
    let port = server.port();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("the stream connects");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("a read timeout");
    stream
        .write_all(b"GET /api/v0/events HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .expect("the events request is sent");
    let mut stream = std::io::BufReader::new(stream);
    let mut head = String::new();
    stream.read_line(&mut head).expect("the stream head");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    for _ in 0..3 {
        let mut header = String::new();
        stream.read_line(&mut header).expect("a header line");
    }
    let mut blank = String::new();
    stream.read_line(&mut blank).expect("the head ends");
    assert_eq!(blank, "\r\n");
    let mut socket = stream.into_inner();

    // The board and conversation actions of a real fleet publish these, and the
    // store's own tests assert that they do; what this asserts is the stream's
    // own half — the daemon subscribes each client and frames what the bus is
    // given, through the real socket, as SSE. Publishing on the daemon's own
    // bus is exactly what a post or a `say` does.
    server.node().events().publish(Event::Board {
        kind: "post",
        post: Some(r#"{"id":"p1","agent_cid":300}"#.to_string()),
        agent_cid: 300,
        role: "builder".to_string(),
        read_up_to: None,
    });
    server.node().events().publish(Event::Conversation {
        id: "c1".to_string(),
        from_cid: 300,
        to_cid: 301,
        delivered: false,
    });

    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    while !String::from_utf8_lossy(&raw).contains("event: conversation") {
        match socket.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(err) => panic!("the stream failed: {err}"),
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    for expected in [
        "event: board\ndata: {\"kind\":\"post\",\"post\":{\"id\":\"p1\",\"agent_cid\":300},\"agent_cid\":300,\"role\":\"builder\",\"read_up_to\":null}\n\n",
        "event: conversation\ndata: {\"id\":\"c1\",\"from_cid\":300,\"to_cid\":301,\"delivered\":false}\n\n",
    ] {
        assert!(text.contains(expected), "the stream carried {expected:?}: {text}");
    }
}

#[test]
fn the_social_routes_do_not_shadow_the_others() {
    let server = daemon("routes");
    let port = server.port();

    // The board's own paths are matched exactly, so a near miss is not a
    // cursor and not a 404 dressed as one.
    assert_eq!(
        request(port, "GET", "/api/v0/boards", None).status,
        404,
        "a near miss is not the board"
    );
    assert_eq!(
        request(port, "GET", "/api/v0/agents/300/contact", None).status,
        404
    );
    // The loopback guard applies to the new routes like every other.
    let refused = send(
        port,
        b"POST /api/v0/board HTTP/1.1\r\nHost: evil.example\r\nContent-Length: 2\r\n\r\n{}",
    );
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert_eq!(
        refused.json()["error"],
        "the Host header must name loopback"
    );
    // And the console's static files are still served under the same method
    // rule as before: a POST to a file path is a 405.
    assert_eq!(request(port, "POST", "/some-file.js", Some("{}")).status, 405);
}
