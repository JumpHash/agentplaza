//! The hub's status surface: a read-only HTTP/1.1 view of the agent registry.
//!
//! `vsockd --status-port <p>` binds `127.0.0.1:<p>` (loopback only) and serves
//! two endpoints, so the fleet's console can ask who is connected without
//! scraping the operator log:
//!
//! * `GET /status` — the host facts (the CIDs served, the vsock port, the
//!   daemon's uptime, `MAX_SESSIONS`, `KEEPALIVE_IDLE_SECS`) plus one row per
//!   registry entry (`cid`, `role`, `connected_at_ms`, `last_heartbeat_ms`,
//!   `calls`) and the number of live sessions. Timestamps are milliseconds
//!   since the Unix epoch, as on the daemon's other surfaces.
//! * `GET /healthz` — `ok`, a liveness probe.
//!
//! Everything else is a 404; every method but `GET` is a 405. Nothing is
//! mutated, nothing is logged per request, and the listener is off unless
//! `--status-port` asks for it.
//!
//! This module and `crates/noded/src/http.rs` are two hand-rolled HTTP/1.1
//! readers of the same shape — request line plus headers, one request per
//! connection, a bounded read, `Connection: close` — neither depends on an
//! HTTP library and `vsockd` cannot depend on `noded`, so the two stay in step
//! by hand. As there: a malformed request, a closed socket or a client that
//! dawdles is answered or dropped, never a panic.
//!
//! Reading the registry never blocks the session path: [`Hub::snapshot`] copies
//! the rows under the registry lock and this module serialises them after the
//! lock is released — the same discipline the hub's log lines follow.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::hub::{Hub, Snapshot};
use crate::sys;
use crate::{ACCEPT_POLL_MS, KEEPALIVE_IDLE_SECS, MAX_SESSIONS};

/// The most a request line and its headers may occupy, together.
pub const MAX_HEAD: usize = 8 * 1024;

/// How long one status connection may take to send its request, and to be
/// written its response. A probe or a console scrape is immediate; a client
/// that connects and then dawdles is closed, so it cannot pin a thread.
const STATUS_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a request could not be read. Every one of these is a 400.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError(&'static str);

impl ParseError {
    /// The reason, as it is reported to the client.
    pub fn message(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for ParseError {}

/// One parsed request: the method and the request target (path plus any
/// query).
///
/// The status surface is read-only, so there is no body: a request that
/// declares one is refused rather than swallowed.
#[derive(Debug)]
pub struct Request {
    method: String,
    target: String,
}

impl Request {
    /// The request method, as it arrived (`GET`, `POST`, ...).
    pub fn method(&self) -> &str {
        &self.method
    }

    /// The request target without its query string or fragment: what routing
    /// matches on.
    pub fn path(&self) -> &str {
        let end = self.target.find(['?', '#']).unwrap_or(self.target.len());
        &self.target[..end]
    }
}

/// Reads one request: the request line and the headers, bounded together by
/// [`MAX_HEAD`].
///
/// Anything malformed — a line that is not ASCII, a request line that is not
/// `METHOD TARGET HTTP/1.x`, a header without a `:`, a chunked or declared
/// body, a head that is too large, a connection that ends mid-request — is a
/// [`ParseError`], never a panic.
pub fn read_request(reader: &mut impl BufRead) -> Result<Request, ParseError> {
    let mut line = Vec::new();
    read_line(reader, MAX_HEAD, &mut line)?;
    let request_line =
        std::str::from_utf8(&line).map_err(|_| ParseError("the request line is not ASCII"))?;
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ParseError("the request line must be `METHOD TARGET HTTP/1.x`"));
    };
    if method.is_empty() || !method.bytes().all(is_token_byte) {
        return Err(ParseError("the method is not a token"));
    }
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(ParseError("only HTTP/1.0 and HTTP/1.1 are served"));
    }
    // Origin-form only: no absolute-form (`http://host/path`) and no bytes a
    // response header could not carry.
    if !target.starts_with('/') || !target.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
        return Err(ParseError("the request target must be an origin-form path"));
    }
    // The line buffer is reused for the headers, so the request line's parts
    // are taken before it is.
    let method = method.to_string();
    let target = target.to_string();

    let mut head_bytes = line.len() + 2;
    loop {
        read_line(reader, MAX_HEAD, &mut line)?;
        head_bytes += line.len() + 2;
        if head_bytes > MAX_HEAD {
            return Err(ParseError("the request headers are too large"));
        }
        if line.is_empty() {
            break;
        }
        let header =
            std::str::from_utf8(&line).map_err(|_| ParseError("a request header is not ASCII"))?;
        let Some((name, value)) = header.split_once(':') else {
            return Err(ParseError("a request header has no `:`"));
        };
        let value = value.trim_matches([' ', '\t']);
        if name.eq_ignore_ascii_case("content-length") {
            // Read-only: the endpoints take no body at all, and a request that
            // declares one is refused rather than half-read.
            if value != "0" {
                return Err(ParseError("a status request must not carry a body"));
            }
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(ParseError("chunked request bodies are not supported"));
        }
    }
    Ok(Request { method, target })
}

/// Reads one `\n`-terminated line, without its `\r\n` or `\n`, into `line`.
///
/// A line longer than `limit`, or a connection that ends before the newline,
/// is an error: the request cannot be read as sent.
fn read_line(reader: &mut impl BufRead, limit: usize, line: &mut Vec<u8>) -> Result<(), ParseError> {
    line.clear();
    let read = (&mut *reader)
        .take(limit as u64 + 1)
        .read_until(b'\n', line)
        .map_err(|_| ParseError("the request could not be read"))?;
    if read == 0 || line.last() != Some(&b'\n') {
        return Err(ParseError("the request ended before it was complete"));
    }
    if line.len() > limit {
        return Err(ParseError("a request line or header is too long"));
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(())
}

/// Whether `byte` may appear in a method token (RFC 9110 `tchar`).
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// A response: a status, a content type and a body.
#[derive(Debug)]
pub struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    /// A JSON response with the given status.
    pub fn json(status: u16, body: String) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body: body.into_bytes(),
        }
    }

    /// A JSON error object, `{"error": "..."}`.
    pub fn error(status: u16, message: &str) -> Self {
        Self::json(status, serde_json::json!({ "error": message }).to_string())
    }

    /// A response with `content_type` and `body`.
    pub fn text(status: u16, content_type: &'static str, body: String) -> Self {
        Self {
            status,
            content_type,
            body: body.into_bytes(),
        }
    }

    /// The response's status code. Only the tests read it back: production
    /// writes the response and never re-inspects it.
    #[cfg(test)]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The response's body. Only the tests read it back.
    #[cfg(test)]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Writes the response head and body. The connection is closed afterwards
    /// by the caller, which is what the `Connection: close` says.
    pub fn write_to(&self, writer: &mut impl Write) -> io::Result<()> {
        let head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            self.status,
            reason(self.status),
            self.content_type,
            self.body.len(),
        );
        writer.write_all(head.as_bytes())?;
        writer.write_all(&self.body)?;
        writer.flush()
    }
}

/// The reason phrase for a status code.
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Unknown",
    }
}

/// The daemon's own facts, as `/status` reports them.
#[derive(Debug)]
pub struct NodeFacts {
    cids: Vec<u32>,
    port: u32,
    started: Instant,
    max_sessions: usize,
    keepalive_idle_s: u64,
}

impl NodeFacts {
    /// The facts of a daemon that serves `cids` on `port` and started at
    /// `started`. The caps are the daemon's own constants, so the surface
    /// cannot drift from the limits the sessions enforce.
    pub fn new(cids: Vec<u32>, port: u32, started: Instant) -> Self {
        Self {
            cids,
            port,
            started,
            max_sessions: MAX_SESSIONS,
            keepalive_idle_s: KEEPALIVE_IDLE_SECS,
        }
    }

    /// The daemon's uptime in whole seconds.
    pub fn uptime_s(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

/// The `/status` body, exactly the shape the fleet contract freezes.
#[derive(Serialize)]
struct StatusBody<'a> {
    node: NodeView<'a>,
    agents: Vec<AgentRow<'a>>,
    sessions_active: usize,
}

/// The host facts, as they are reported.
#[derive(Serialize)]
struct NodeView<'a> {
    cids: &'a [u32],
    port: u32,
    uptime_s: u64,
    max_sessions: usize,
    keepalive_idle_s: u64,
}

/// One registry row, as it is reported.
#[derive(Serialize)]
struct AgentRow<'a> {
    cid: u32,
    role: &'a str,
    connected_at_ms: u64,
    last_heartbeat_ms: u64,
    calls: u64,
}

/// The status body for a snapshot. `uptime_s` is passed in so the shape is
/// testable byte for byte.
fn status_json(snapshot: &Snapshot, facts: &NodeFacts, uptime_s: u64) -> String {
    let body = StatusBody {
        node: NodeView {
            cids: &facts.cids,
            port: facts.port,
            uptime_s,
            max_sessions: facts.max_sessions,
            keepalive_idle_s: facts.keepalive_idle_s,
        },
        agents: snapshot
            .rows
            .iter()
            .map(|row| AgentRow {
                cid: row.cid,
                role: &row.role,
                connected_at_ms: row.connected_at_ms,
                last_heartbeat_ms: row.last_heartbeat_ms,
                calls: row.calls,
            })
            .collect(),
        sessions_active: snapshot.sessions_active,
    };
    // The view is integers and strings, so serialising cannot fail; if it ever
    // did, the surface still must not panic.
    serde_json::to_string(&body).unwrap_or_else(|_| r#"{"error":"cannot serialise the status"}"#.to_string())
}

/// Answers one parsed request.
///
/// Read-only: `GET /status` (the registry), `GET /healthz` (the liveness
/// probe), 404 for any other path, 405 for any other method.
fn route(request: &Request, hub: &Hub, facts: &NodeFacts) -> Response {
    if request.method() != "GET" {
        return Response::error(405, "only GET is served");
    }
    match request.path() {
        "/status" => {
            // The lock is taken and released inside `snapshot`: the JSON is
            // built with the session path free to run.
            let snapshot = hub.snapshot();
            Response::json(200, status_json(&snapshot, facts, facts.uptime_s()))
        }
        "/healthz" => Response::text(200, "text/plain; charset=utf-8", "ok".to_string()),
        _ => Response::error(404, "no such endpoint"),
    }
}

/// Serves one connection: one bounded request, one response, close.
///
/// Both the read and the write are bounded by [`STATUS_READ_TIMEOUT`], and the
/// head by [`MAX_HEAD`]. A client that hung up before its answer arrived is
/// not an error — the answer is simply undeliverable.
fn handle_connection(stream: TcpStream, hub: &Hub, facts: &NodeFacts) {
    let _ = stream.set_read_timeout(Some(STATUS_READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(STATUS_READ_TIMEOUT));
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::with_capacity(MAX_HEAD + 1, stream);
    let response = match read_request(&mut reader) {
        Ok(request) => route(&request, hub, facts),
        Err(err) => Response::error(400, err.message()),
    };
    let _ = response.write_to(&mut writer);
}

/// Accepts status connections until the daemon is asked to stop.
///
/// Each connection is served on its own short-lived thread: the surface is
/// loopback-only, one request per connection, and bounded, so it cannot grow
/// an unbounded thread pool.
pub fn serve(listener: TcpListener, hub: Arc<Hub>, facts: Arc<NodeFacts>) {
    while !sys::shutdown_requested() {
        match sys::poll_readable(listener.as_raw_fd(), ACCEPT_POLL_MS) {
            Ok(false) => continue,
            // A connection, a shutdown request or a spurious wakeup: in every
            // case the loop re-checks the flag and tries to accept.
            Ok(true) => {}
            Err(err) => eprintln!("vsockd: status poll failed: {err}"),
        }
        match listener.accept() {
            Ok((stream, _peer)) => {
                let hub = Arc::clone(&hub);
                let facts = Arc::clone(&facts);
                thread::spawn(move || handle_connection(stream, &hub, &facts));
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            Err(err) => eprintln!("vsockd: status accept failed: {err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Cursor;

    use crate::serve::SessionLog;

    /// A [`SessionLog`] that keeps nothing: the status surface logs no
    /// transitions of its own.
    struct Quiet;

    impl SessionLog for Quiet {
        fn line(&self, _text: &str) {}
    }

    /// A hub whose instants are translated against `epoch_ms` at `anchor`.
    fn anchored(anchor: Instant, epoch_ms: u64) -> Hub {
        Hub::started_at(Arc::new(Quiet), anchor, epoch_ms)
    }

    /// The daemon's facts, with a caller-chosen start for the uptime.
    fn facts(started: Instant) -> NodeFacts {
        NodeFacts::new(vec![10, 11], 5000, started)
    }

    fn read(raw: &str) -> Result<Request, ParseError> {
        read_request(&mut Cursor::new(raw.as_bytes()))
    }

    fn get(path: &str) -> Request {
        read(&format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n"))
            .expect("a well-formed GET")
    }

    /// An empty registry is an empty `agents` list, and the host facts are the
    /// daemon's: the CIDs served, the port, the uptime, and both caps.
    #[test]
    fn an_empty_registry_is_an_empty_agents_list() {
        let hub = anchored(Instant::now(), 1_700_000_000_000);
        let json = status_json(&hub.snapshot(), &facts(Instant::now()), 7);
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(value["agents"], serde_json::json!([]));
        assert_eq!(value["sessions_active"], 0);
        assert_eq!(value["node"]["cids"], serde_json::json!([10, 11]));
        assert_eq!(value["node"]["port"], 5000);
        assert_eq!(value["node"]["uptime_s"], 7);
        assert_eq!(value["node"]["max_sessions"], MAX_SESSIONS);
        assert_eq!(value["node"]["keepalive_idle_s"], KEEPALIVE_IDLE_SECS);
    }

    /// A connected agent is one row with its role, its epoch-millisecond
    /// timestamps and its call count; a heartbeat advances the beat.
    #[test]
    fn a_connected_agent_is_a_row_with_its_role_and_calls() {
        let anchor = Instant::now();
        let hub = anchored(anchor, 1_700_000_000_000);
        let (_token, _steering) = hub.connect(10, "builder", anchor + Duration::from_millis(250));
        hub.heartbeat(10, 1, anchor + Duration::from_millis(5_250));
        hub.note_calls(10, 3);

        let snapshot = hub.snapshot();
        let json = status_json(&snapshot, &facts(anchor), 9);
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(value["sessions_active"], 1);
        assert_eq!(value["agents"][0]["cid"], 10);
        assert_eq!(value["agents"][0]["role"], "builder");
        assert_eq!(value["agents"][0]["connected_at_ms"], 1_700_000_000_250u64);
        assert_eq!(value["agents"][0]["last_heartbeat_ms"], 1_700_000_005_250u64);
        assert_eq!(value["agents"][0]["calls"], 3);
    }

    /// The body's byte shape is the fleet contract's: key names, nesting and
    /// order, timestamps in milliseconds since the Unix epoch, and the two
    /// frozen caps.
    #[test]
    fn the_status_shape_is_frozen() {
        let anchor = Instant::now();
        let hub = anchored(anchor, 1_700_000_000_000);
        hub.connect(10, "builder", anchor + Duration::from_millis(250));
        hub.note_calls(10, 2);

        let json = status_json(&hub.snapshot(), &facts(anchor), 42);
        assert_eq!(
            json,
            concat!(
                r#"{"node":{"cids":[10,11],"port":5000,"uptime_s":42,"max_sessions":64,"keepalive_idle_s":300},"#,
                r#""agents":[{"cid":10,"role":"builder","connected_at_ms":1700000000250,"last_heartbeat_ms":1700000000250,"calls":2}],"#,
                r#""sessions_active":1}"#,
            )
        );
    }

    /// Routing: `/status` (with or without a query), the `/healthz` probe,
    /// a 404 for anything else, a 405 for any method but GET.
    #[test]
    fn the_endpoints_route_and_everything_else_is_refused() {
        let hub = anchored(Instant::now(), 1_700_000_000_000);
        let facts = facts(Instant::now());

        let status = route(&get("/status"), &hub, &facts);
        assert_eq!(status.status(), 200);
        assert!(
            std::str::from_utf8(status.body())
                .expect("the body is JSON")
                .starts_with(r#"{"node":{"#),
            "the status body is the frozen object"
        );

        assert_eq!(route(&get("/status?verbose=1"), &hub, &facts).status(), 200);

        let health = route(&get("/healthz"), &hub, &facts);
        assert_eq!(health.status(), 200);
        assert_eq!(health.body(), b"ok");

        let missing = route(&get("/nope"), &hub, &facts);
        assert_eq!(missing.status(), 404);
        assert!(
            std::str::from_utf8(missing.body())
                .expect("the body is JSON")
                .starts_with(r#"{"error":""#),
            "a refusal is a JSON error object"
        );

        let posted = route(
            &read("POST /status HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .expect("a bodyless POST parses"),
            &hub,
            &facts,
        );
        assert_eq!(posted.status(), 405);
        assert_eq!(route(&get("/healthz"), &hub, &facts).status(), 200, "GET still works");
    }

    /// A malformed request is a 400 at the parser, never a panic: a head that
    /// is too long (one line, or a pile of them), a request that declares a
    /// body, chunked bodies, a non-ASCII header, a missing colon, a truncated
    /// request, and the request-line shapes HTTP does not have.
    #[test]
    fn malformed_requests_are_refused() {
        let bad = [
            "",
            "\r\n",
            "GET\r\n\r\n",
            "GET /status\r\n\r\n",
            "GET /status HTTP/2\r\n\r\n",
            "GET  /status HTTP/1.1\r\n\r\n",
            "GET status HTTP/1.1\r\n\r\n",
            "G ET /status HTTP/1.1\r\n\r\n",
            "GET /st\tatus HTTP/1.1\r\n\r\n",
            "GET /status HTTP/1.1\r\nno-colon\r\n\r\n",
            "GET /status HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello",
            "GET /status HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            "GET /status HTTP/1.1\r\nHost: localhost\r\n",
        ];
        for raw in bad {
            assert!(read(raw).is_err(), "should have been refused: {raw:?}");
        }

        // A bodyless GET that says so explicitly is fine.
        assert!(read("GET /status HTTP/1.1\r\nContent-Length: 0\r\n\r\n").is_ok());

        let mut non_ascii = b"GET /status HTTP/1.1\r\nX: ".to_vec();
        non_ascii.push(0xff);
        non_ascii.extend_from_slice(b"\r\n\r\n");
        assert!(read_request(&mut Cursor::new(&non_ascii)).is_err());
    }

    /// The head is bounded: neither one enormous header line nor a pile of
    /// small ones may grow the buffer past [`MAX_HEAD`].
    #[test]
    fn an_oversized_head_is_refused() {
        let one_long_line = format!(
            "GET /status HTTP/1.1\r\nHost: localhost\r\nX-Pad: {}\r\n\r\n",
            "a".repeat(MAX_HEAD)
        );
        assert!(read(&one_long_line).is_err());

        let many_lines = format!(
            "GET /status HTTP/1.1\r\nHost: localhost\r\n{}\r\n",
            "X: 1\r\n".repeat(MAX_HEAD / 5)
        );
        assert!(read(&many_lines).is_err());
    }

    /// The socket path end to end: a real loopback connection is read,
    /// answered and closed, and a malformed request gets the 400 rather than a
    /// panic.
    #[test]
    fn a_real_connection_is_answered() {
        let anchor = Instant::now();
        let hub = Arc::new(anchored(anchor, 1_700_000_000_000));
        hub.connect(10, "builder", anchor);
        let facts = Arc::new(facts(anchor));

        let listener = TcpListener::bind("127.0.0.1:0").expect("the test binds a port");
        let addr = listener.local_addr().expect("a bound address");
        let served = thread::spawn(move || {
            for _ in 0..2 {
                let (stream, _peer) = listener.accept().expect("the client connects");
                handle_connection(stream, &hub, &facts);
            }
        });

        let response = request_over_tcp(addr, "GET /status HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.contains("Content-Length: "), "{response}");
        assert!(response.contains("Connection: close\r\n"), "{response}");
        assert!(response.contains(r#""role":"builder""#), "{response}");

        let response = request_over_tcp(addr, "GET /status\r\n\r\n");
        assert!(
            response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
            "{response}"
        );
        assert!(response.contains(r#"{"error":""#), "{response}");
        served.join().expect("the server thread finished");
    }

    /// Sends one raw request and reads the response up to the server's close.
    fn request_over_tcp(addr: std::net::SocketAddr, raw: &str) -> String {
        let mut stream = TcpStream::connect(addr).expect("the client connects");
        stream.write_all(raw.as_bytes()).expect("the request is written");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("the response is read");
        response
    }

    /// A response carries its length and closes; the status codes the surface
    /// can emit all have their reason phrase.
    #[test]
    fn a_response_carries_its_length_and_closes() {
        let response = Response::json(200, "{\"ok\":true}".to_string());
        let mut written = Vec::new();
        response.write_to(&mut written).expect("writing to a Vec");
        let written = String::from_utf8(written).expect("the head is ASCII");
        assert!(written.starts_with("HTTP/1.1 200 OK\r\n"), "{written}");
        assert!(written.contains("Content-Length: 11\r\n"), "{written}");
        assert!(written.contains("Cache-Control: no-store\r\n"), "{written}");
        assert!(written.contains("Connection: close\r\n"), "{written}");
        assert!(written.ends_with("\r\n\r\n{\"ok\":true}"), "{written}");

        for (status, phrase) in [
            (400, "Bad Request"),
            (404, "Not Found"),
            (405, "Method Not Allowed"),
        ] {
            assert_eq!(reason(status), phrase);
        }
    }

    /// A row that has gone is gone from the surface too: a session that ended
    /// leaves no agent behind, and the empty body keeps the frozen shape.
    #[test]
    fn an_ended_session_leaves_nothing_in_the_status() {
        let anchor = Instant::now();
        let hub = anchored(anchor, 1_700_000_000_000);
        let (token, _steering) = hub.connect(10, "builder", anchor);
        let json = status_json(&hub.snapshot(), &facts(anchor), 0);
        assert!(json.contains(r#""role":"builder""#), "{json}");

        hub.end_clean(10, token, 0);
        let json = status_json(&hub.snapshot(), &facts(anchor), 1);
        assert_eq!(
            json,
            format!(
                concat!(
                    r#"{{"node":{{"cids":[10,11],"port":5000,"uptime_s":1,"max_sessions":{},"#,
                    r#""keepalive_idle_s":{}}},"agents":[],"sessions_active":0}}"#,
                ),
                MAX_SESSIONS, KEEPALIVE_IDLE_SECS
            )
        );
    }
}
