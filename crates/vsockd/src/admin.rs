//! The hub's admin surface: the one loopback endpoint that lets the control
//! plane *drive* a connected agent.
//!
//! `vsockd --admin-port <p>` binds `127.0.0.1:<p>` (loopback only) and serves:
//!
//! * `POST /tell` — `{"cid": N, "name": "<tool>", "args": {...},
//!   "timeout_ms": N?}`: send the agent connected as guest CID `N` one
//!   `tool_call` for `<tool>`, wait — bounded, ≤ [`MAX_TELL_TIMEOUT_MS`] — for
//!   that agent's matching `tool_result`, and answer
//!   `{"ok": <bool>, "output": "<the tool's output>"}`.
//!   * a CID the daemon does not serve → **404** (it can never connect);
//!   * a served CID with no live session → **409** (not connected);
//!   * an agent that does not answer in time → **504** with the honest
//!     `agent did not answer within N ms` (the session is untouched: it is
//!     still alive and may still answer, later and to nobody);
//!   * a request outside the body's contract → **400**.
//! * `GET /healthz` — `ok`, a liveness probe.
//!
//! Everything else is a 404; every other method is a 405. Nothing is
//! mutated except the agent's own workspace, through the tool it is asked to
//! run, and the request is logged only by what it did (the session's own
//! dispatch line). The listener is off unless `--admin-port` asks for it.
//!
//! The surface is the status surface's sibling and shares its machinery
//! ([`crate::status::read_request_body`], [`crate::status::Response`], the
//! head bound, the one-request-per-connection rule, the timeout policy) —
//! the only difference the read-only status surface cannot express is the
//! bounded request body, which the parser reads here.
//!
//! The dispatch itself is [`Hub::tell`]: the surface validates the request,
//! and the hub (behind the registry lock, never holding it) hands the call to
//! the session's steering channel and waits.

use std::io::{self, BufReader};
use std::net::TcpListener;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde::Deserialize;

use crate::hub::{Hub, TellOutcome};
use crate::status::{MAX_HEAD, NodeFacts, Request, Response, read_request_body};
use crate::{ACCEPT_POLL_MS, sys};

/// How long one admin connection may take to send its request, and to be
/// written its response.
///
/// The response cannot be written before the dispatch's wait ends, so this must
/// exceed [`MAX_TELL_TIMEOUT_MS`] with room for the agent to answer, the
/// session to route it and the socket to carry it.
const ADMIN_TIMEOUT: Duration = Duration::from_secs(10);

/// The most a `/tell` body may occupy. The tools' own arguments are small
/// (a path, a content field, a pattern), and the harness bounds what it will
/// accept; this is the transport's bound on what a caller may send at all.
pub const MAX_BODY: usize = 64 * 1024;

/// How long a `/tell` waits for its agent unless the request says otherwise.
pub const DEFAULT_TELL_TIMEOUT_MS: u64 = 5_000;

/// The longest a `/tell` may wait, and the longest a caller may ask for. The
/// control plane drives long-lived agents; a call that takes longer than this
/// is a fact to report, not a reason to hold the connection open.
pub const MAX_TELL_TIMEOUT_MS: u64 = 5_000;

/// The most a tool name may occupy, in bytes. The harness's own names are a
/// handful of bytes; this bounds what a caller may push into a session's log
/// and wire line.
pub const MAX_TOOL_NAME: usize = 256;

/// The `POST /tell` body.
#[derive(Debug, Deserialize)]
struct TellRequest {
    /// The guest CID of the agent to drive.
    cid: u64,
    /// The tool to run on it.
    name: String,
    /// The tool's arguments; an object, empty when the request omits them.
    #[serde(default = "empty_args")]
    args: serde_json::Value,
    /// How long to wait for the agent's answer, in milliseconds.
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// The default `args`: an object with nothing in it, which is what the tools
/// that take no arguments (`uptime`) expect.
fn empty_args() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// The `/tell` answer: the agent's own `ok` and `output`.
fn tell_body(ok: bool, output: &str) -> String {
    serde_json::json!({ "ok": ok, "output": output }).to_string()
}

/// Answers one parsed request.
///
/// `POST /tell` is the surface's one endpoint that acts; `GET /healthz` is the
/// liveness probe. Anything else is a 404 (no such endpoint) or a 405 (the
/// method the matched endpoint does not accept).
fn route(request: &Request, hub: &Hub, facts: &NodeFacts) -> Response {
    match request.path() {
        "/tell" => match request.method() {
            "POST" => tell(hub, facts, request.body()),
            _ => Response::error(405, "POST /tell is the admin surface's one endpoint"),
        },
        "/healthz" => match request.method() {
            "GET" => Response::text(200, "text/plain; charset=utf-8", "ok".to_string()),
            _ => Response::error(405, "the health probe is a GET"),
        },
        _ => Response::error(404, "no such endpoint"),
    }
}

/// `POST /tell`: validates the request, then hands it to the hub.
///
/// Every refusal names what was wrong with the *request* (400), which CID the
/// hub does not serve (404) or does not have connected (409), or the honest
/// timeout (504). A tool that ran and refused is a 200 with `ok: false`: the
/// agent answered, and its answer is data.
fn tell(hub: &Hub, facts: &NodeFacts, body: &[u8]) -> Response {
    let request: TellRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(err) => return Response::error(400, &format!("invalid tell request: {err}")),
    };
    if request.name.is_empty() {
        return Response::error(400, "a tell needs a tool name");
    }
    if request.name.len() > MAX_TOOL_NAME {
        return Response::error(
            400,
            &format!("the tool name is longer than {MAX_TOOL_NAME} bytes"),
        );
    }
    if !request.args.is_object() {
        return Response::error(400, "args must be a JSON object");
    }
    let timeout_ms = request.timeout_ms.unwrap_or(DEFAULT_TELL_TIMEOUT_MS);
    if timeout_ms == 0 || timeout_ms > MAX_TELL_TIMEOUT_MS {
        return Response::error(
            400,
            &format!("timeout_ms must be 1..={MAX_TELL_TIMEOUT_MS}"),
        );
    }
    // A CID outside the daemon's served set can never connect: it is an
    // unknown agent, not one that has yet to connect. A CID the daemon does
    // serve but has no live session for is a conflict with the hub's current
    // state, and the caller may retry it.
    let Ok(cid) = u32::try_from(request.cid) else {
        return Response::error(404, &format!("no agent with guest CID {}", request.cid));
    };
    if !facts.cids().contains(&cid) {
        return Response::error(404, &format!("no agent with guest CID {cid}"));
    }
    match hub.tell(
        cid,
        &request.name,
        request.args,
        Duration::from_millis(timeout_ms),
    ) {
        TellOutcome::Answered { ok, output } => Response::json(200, tell_body(ok, &output)),
        TellOutcome::NotConnected => {
            Response::error(409, &format!("agent with guest CID {cid} is not connected"))
        }
        TellOutcome::TimedOut { .. } => {
            Response::error(504, &format!("agent did not answer within {timeout_ms} ms"))
        }
    }
}

/// Serves one connection: one bounded request (head and body), one response,
/// close.
///
/// Both the read and the write are bounded by [`ADMIN_TIMEOUT`], the head by
/// [`MAX_HEAD`] and the body by [`MAX_BODY`]. A client that hung up before its
/// answer arrived is not an error — the answer is simply undeliverable.
fn handle_connection(stream: std::net::TcpStream, hub: &Hub, facts: &NodeFacts) {
    let _ = stream.set_read_timeout(Some(ADMIN_TIMEOUT));
    let _ = stream.set_write_timeout(Some(ADMIN_TIMEOUT));
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::with_capacity(MAX_HEAD + 1, stream);
    let response = match read_request_body(&mut reader, MAX_BODY) {
        Ok(request) => route(&request, hub, facts),
        Err(err) => Response::error(400, err.message()),
    };
    let _ = response.write_to(&mut writer);
}

/// Accepts admin connections until the daemon is asked to stop.
///
/// Each connection is served on its own short-lived thread: the surface is
/// loopback-only, one request per connection, and bounded, so it cannot grow
/// an unbounded thread pool. A dispatch can hold its thread for up to
/// [`MAX_TELL_TIMEOUT_MS`] — one caller, one thread, bounded — which is the
/// price of an answer rather than a guess.
pub fn serve(listener: TcpListener, hub: Arc<Hub>, facts: Arc<NodeFacts>) {
    while !sys::shutdown_requested() {
        match sys::poll_readable(listener.as_raw_fd(), ACCEPT_POLL_MS) {
            Ok(false) => continue,
            // A connection, a shutdown request or a spurious wakeup: in every
            // case the loop re-checks the flag and tries to accept.
            Ok(true) => {}
            Err(err) => eprintln!("vsockd: admin poll failed: {err}"),
        }
        match listener.accept() {
            Ok((stream, _peer)) => {
                let hub = Arc::clone(&hub);
                let facts = Arc::clone(&facts);
                thread::spawn(move || handle_connection(stream, &hub, &facts));
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            Err(err) => eprintln!("vsockd: admin accept failed: {err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Cursor, Read, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::sync::mpsc::Receiver;
    use std::time::Instant;

    use crate::hub::{Command, ToolReply};
    use crate::serve::SessionLog;

    /// A [`SessionLog`] that keeps nothing: the admin surface logs no
    /// transitions of its own (the session logs the dispatch it takes).
    struct Quiet;

    impl SessionLog for Quiet {
        fn line(&self, _text: &str) {}
    }

    fn hub() -> Hub {
        Hub::new(Arc::new(Quiet))
    }

    fn facts(cids: Vec<u32>) -> NodeFacts {
        NodeFacts::new(cids, 5000, Instant::now())
    }

    fn post(path: &str, body: &str) -> Request {
        let raw = format!(
            "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        read_request_body(&mut Cursor::new(raw.as_bytes()), MAX_BODY).expect("a well-formed POST")
    }

    fn get(path: &str) -> Request {
        read_request_body(
            &mut Cursor::new(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes()),
            MAX_BODY,
        )
        .expect("a well-formed GET")
    }

    /// A stub agent: it answers the next dispatch the session sends it with
    /// `reply`, exactly as the harness's session loop would.
    fn stub_agent(steering: Receiver<Command>, reply: ToolReply) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            if let Ok(Command::Call {
                reply: Some(channel),
                ..
            }) = steering.recv()
            {
                let _ = channel.send(reply);
            }
        })
    }

    fn json(body: &[u8]) -> serde_json::Value {
        serde_json::from_slice(body).expect("a JSON response")
    }

    /// Routing: `POST /tell` is the one endpoint, `GET /healthz` the probe,
    /// a 404 for anything else, a 405 for the wrong method — and a request
    /// the parser cannot read is a 400, never a panic.
    #[test]
    fn the_endpoints_route_and_everything_else_is_refused() {
        let hub = hub();
        let facts = facts(vec![10]);

        assert_eq!(route(&get("/healthz"), &hub, &facts).status(), 200);
        assert_eq!(route(&get("/healthz"), &hub, &facts).body(), b"ok");
        assert_eq!(route(&get("/nope"), &hub, &facts).status(), 404);
        assert_eq!(route(&get("/status"), &hub, &facts).status(), 404);
        assert_eq!(route(&get("/tell"), &hub, &facts).status(), 405);

        let posted = route(&post("/tell", "{}"), &hub, &facts);
        assert_eq!(posted.status(), 400);
        assert!(json(posted.body())["error"].is_string());

        // The parser's own refusals are 400s: an unreadable head, a body that
        // does not match its Content-Length, a body past the bound.
        for raw in [
            "POST /tell HTTP/2\r\n\r\n",
            "POST /tell HTTP/1.1\r\nContent-Length: 5\r\n\r\nhi",
            "GET /tell HTTP/1.1\r\nno-colon\r\n\r\n",
        ] {
            let err = read_request_body(&mut Cursor::new(raw.as_bytes()), MAX_BODY)
                .expect_err("should have been refused");
            assert!(!err.message().is_empty(), "a refusal says why");
        }
    }

    /// A `/tell` outside the body's contract is a 400 that names the field:
    /// a missing or blank name, a name past the bound, args that are not an
    /// object, and a timeout outside `1..=MAX`.
    #[test]
    fn a_tell_body_outside_its_contract_is_refused() {
        let hub = hub();
        let facts = facts(vec![10]);

        let refused = vec![
            r#"{}"#.to_string(),
            r#"{"cid":10}"#.to_string(),
            r#"{"cid":10,"name":""}"#.to_string(),
            // no cid, and an unknown field is not one.
            r#"{"cit":10,"name":"echo"}"#.to_string(),
            format!(r#"{{"cid":10,"name":"{}"}}"#, "x".repeat(MAX_TOOL_NAME + 1)),
            r#"{"cid":10,"name":"echo","args":[1,2]}"#.to_string(),
            r#"{"cid":10,"name":"echo","args":"hi"}"#.to_string(),
            r#"{"cid":10,"name":"echo","timeout_ms":0}"#.to_string(),
            r#"{"cid":10,"name":"echo","timeout_ms":5001}"#.to_string(),
            r#"{"cid":-1,"name":"echo"}"#.to_string(),
            "not json".to_string(),
        ];
        for body in refused {
            let response = route(&post("/tell", &body), &hub, &facts);
            assert_eq!(response.status(), 400, "should have been refused: {body}");
        }

        // The two ends of the accepted range are accepted, and so is omitting
        // the timeout (the default) and the args (an empty object). The CID
        // here is not connected, so these are 409s — a refusal of the hub's
        // state, not of the request.
        let accepted = [
            r#"{"cid":10,"name":"echo","timeout_ms":1}"#,
            r#"{"cid":10,"name":"echo","timeout_ms":5000}"#,
            r#"{"cid":10,"name":"echo"}"#,
            r#"{"cid":10,"name":"echo","args":{}}"#,
        ];
        for body in accepted {
            assert_eq!(
                route(&post("/tell", body), &hub, &facts).status(),
                409,
                "should have been accepted: {body}"
            );
        }
        // At the name bound exactly.
        let at_bound = format!(r#"{{"cid":10,"name":"{}"}}"#, "x".repeat(MAX_TOOL_NAME));
        assert_eq!(route(&post("/tell", &at_bound), &hub, &facts).status(), 409);
    }

    /// The CID policy: a CID the daemon does not serve is a 404 (it can never
    /// connect, whatever its size), and a served CID with no live session is a
    /// 409.
    #[test]
    fn an_unserved_cid_is_unknown_and_a_served_one_is_not_connected() {
        let hub = hub();
        let facts = facts(vec![10, 11]);

        for cid in ["99", "4294967296", "18446744073709551615", "0"] {
            let response = route(
                &post("/tell", &format!(r#"{{"cid":{cid},"name":"echo"}}"#)),
                &hub,
                &facts,
            );
            assert_eq!(response.status(), 404, "cid {cid} is not served");
            assert!(
                json(response.body())["error"]
                    .as_str()
                    .is_some_and(|error| error.contains("no agent with guest CID")),
                "the refusal names the unknown agent"
            );
        }

        let response = route(&post("/tell", r#"{"cid":10,"name":"echo"}"#), &hub, &facts);
        assert_eq!(response.status(), 409);
        assert!(
            json(response.body())["error"]
                .as_str()
                .is_some_and(|error| error.contains("not connected")),
            "the refusal says it is not connected"
        );

        // A CID served but past u32 is still unknown, not a panic.
        assert_eq!(
            route(
                &post("/tell", r#"{"cid":4294967296,"name":"echo"}"#),
                &hub,
                &facts
            )
            .status(),
            404
        );
    }

    /// The dispatch: the agent's own answer comes back as `ok`/`output`
    /// (success and refusal alike), a silent agent is the honest 504, and
    /// neither tears the session down.
    #[test]
    fn a_dispatch_answers_with_the_agents_own_result() {
        let hub = hub();
        let facts = facts(vec![10]);

        let (token, steering) = hub.connect(10, "builder", Instant::now());
        let stub = stub_agent(
            steering,
            ToolReply {
                ok: true,
                output: r#"{"path":"build-notes.txt","bytes":7}"#.to_string(),
            },
        );
        let response = route(
            &post(
                "/tell",
                r#"{"cid":10,"name":"write_file","args":{"path":"build-notes.txt"}}"#,
            ),
            &hub,
            &facts,
        );
        assert_eq!(response.status(), 200);
        assert_eq!(
            json(response.body()),
            serde_json::json!({ "ok": true, "output": r#"{"path":"build-notes.txt","bytes":7}"# })
        );
        stub.join().expect("the stub agent finishes");

        // A refusal is the agent answering: 200 with `ok: false`.
        let (_token2, steering) = hub.connect(10, "builder", Instant::now());
        let stub = stub_agent(
            steering,
            ToolReply {
                ok: false,
                output: "denied: path \"../x\" contains \"..\"".to_string(),
            },
        );
        let response = route(
            &post("/tell", r#"{"cid":10,"name":"read_file"}"#),
            &hub,
            &facts,
        );
        assert_eq!(response.status(), 200);
        assert_eq!(json(response.body())["ok"], serde_json::json!(false));
        assert!(
            json(response.body())["output"]
                .as_str()
                .is_some_and(|out| out.starts_with("denied:")),
            "the agent's own refusal is what comes back"
        );
        stub.join().expect("the stub agent finishes");

        // A silent agent: nobody reads its steering channel, so nothing
        // answers. The wait is the request's, the answer is honest, and the
        // session is still registered afterwards.
        let (token3, _steering3) = hub.connect(10, "builder", Instant::now());
        let response = route(
            &post("/tell", r#"{"cid":10,"name":"echo","timeout_ms":60}"#),
            &hub,
            &facts,
        );
        assert_eq!(response.status(), 504);
        assert_eq!(
            json(response.body())["error"],
            serde_json::json!("agent did not answer within 60 ms")
        );
        assert_eq!(hub.agent_count(), 1, "a timeout does not reap the session");
        let _ = token3;
        let _ = token;
    }

    /// The socket path end to end: a real loopback connection carries a
    /// `POST /tell` to a real stub agent, and its answer comes back over the
    /// wire — the whole surface, with the session's own dispatch between.
    #[test]
    fn a_real_connection_is_answered() {
        let hub = Arc::new(hub());
        let facts = Arc::new(facts(vec![10]));
        let (token, steering) = hub.connect(10, "builder", Instant::now());
        let stub = stub_agent(
            steering,
            ToolReply {
                ok: true,
                output: "plaza-agent-10".to_string(),
            },
        );

        let listener = TcpListener::bind("127.0.0.1:0").expect("the test binds a port");
        let addr = listener.local_addr().expect("a bound address");
        let served = {
            let hub = Arc::clone(&hub);
            let facts = Arc::clone(&facts);
            thread::spawn(move || {
                for _ in 0..2 {
                    let (stream, _peer) = listener.accept().expect("the client connects");
                    handle_connection(stream, &hub, &facts);
                }
            })
        };

        let body = r#"{"cid":10,"name":"echo","args":{"text":"plaza-agent-10"}}"#;
        let response = request_over_tcp(
            addr,
            &format!(
                "POST /tell HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.contains("Connection: close\r\n"), "{response}");
        assert!(response.contains(r#""ok":true"#), "{response}");
        assert!(
            response.contains(r#""output":"plaza-agent-10""#),
            "{response}"
        );

        // A refused method, over the same surface, is the 405.
        let response = request_over_tcp(addr, "GET /tell HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert!(
            response.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"),
            "{response}"
        );

        served.join().expect("the server thread finished");
        stub.join().expect("the stub agent finishes");
        assert_eq!(hub.agent_count(), 1);
        let _ = token;
    }

    /// Sends one raw request and reads the response up to the server's close.
    fn request_over_tcp(addr: SocketAddr, raw: &str) -> String {
        let mut stream = TcpStream::connect(addr).expect("the client connects");
        stream
            .write_all(raw.as_bytes())
            .expect("the request is written");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("the response is read");
        response
    }
}
