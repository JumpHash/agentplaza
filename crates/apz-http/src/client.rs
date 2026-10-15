//! A blocking client for loopback daemons: one request per connection to
//! `127.0.0.1:port` with a loopback `Host`, the whole exchange bounded by a
//! timeout. Errors are `io::Error`s — refused, timed out, or `InvalidData` for
//! an answer that is not well-formed HTTP/1.x.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore};

use serde_json::Value;

use crate::ws::Duplex;

/// The most a status line and headers may occupy.
const MAX_HEAD: usize = 16 << 10;
/// The most a reply body may occupy.
const MAX_BODY: usize = 8 << 20;
/// The longest SSE line an [`EventStream`] assembles.
const MAX_EVENT_LINE: usize = 1 << 20;

/// One answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    /// The answer's headers (names as the server sent them, values as they came).
    pub headers: Vec<(String, String)>,
}

impl Reply {
    pub fn json(&self) -> io::Result<Value> {
        serde_json::from_slice(&self.body)
            .map_err(|err| invalid(format!("the answer is not JSON: {err}")))
    }
}

/// Sends one request and reads the whole answer.
pub fn send(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> io::Result<Reply> {
    send_with(port, method, path, &[], body, timeout)
}

/// [`send`] with extra headers (name, value); a name or value with a line
/// break is refused, so a header cannot smuggle another into the request.
pub fn send_with(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    timeout: Duration,
) -> io::Result<Reply> {
    let deadline = Instant::now() + timeout;
    let stream = connect(port, method, path, headers, body, timeout)?;
    read_reply(BufReader::new(Timed { stream, deadline }))
}

/// Reads one whole answer: the head, then a Content-Length body or everything to the close.
fn read_reply(mut reader: impl BufRead) -> io::Result<Reply> {
    let (status, headers) = read_head(&mut reader)?;
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    match header("transfer-encoding") {
        Some(te) if te.eq_ignore_ascii_case("chunked") => {
            let body = read_chunked(&mut reader)?;
            return Ok(reply_of(status, &headers, body));
        }
        Some(te) => return Err(invalid(format!("the answer's transfer encoding {te:?}"))),
        None => {}
    }
    // Without a Content-Length the body runs to the close.
    let length = header("content-length")
        .map(|length| {
            length
                .parse::<usize>()
                .map_err(|_| invalid(format!("Content-Length {length:?}")))
        })
        .transpose()?;
    let mut body = Vec::new();
    match reader
        .take(length.unwrap_or(MAX_BODY + 1).min(MAX_BODY + 1) as u64)
        .read_to_end(&mut body)
    {
        // A TLS peer may close without its close_notify; a body with no length ends there too.
        Err(err) if length.is_none() && err.kind() == io::ErrorKind::UnexpectedEof => {}
        result => {
            result?;
        }
    }
    if body.len() > MAX_BODY || length.is_some_and(|length| length != body.len()) {
        return Err(invalid(format!(
            "the answer's body is {} bytes, not {length:?}",
            body.len()
        )));
    }
    Ok(reply_of(status, &headers, body))
}

/// The reply of a head and its body.
fn reply_of(status: u16, headers: &[(String, String)], body: Vec<u8>) -> Reply {
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    // Printable ASCII only: a proxy writes this into its own response head.
    let content_type = match header("content-type") {
        Some(value)
            if !value.is_empty() && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) =>
        {
            value
        }
        _ => "application/octet-stream",
    };
    Reply {
        status,
        content_type: content_type.to_string(),
        body,
        headers: headers.to_vec(),
    }
}

/// A chunked body (RFC 9112 7.1): hex-sized chunks (extensions ignored) up to the zero chunk, then the trailers, which
/// are read and dropped. The whole body is held to MAX_BODY.
fn read_chunked(reader: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        reader.by_ref().take(MAX_HEAD as u64).read_line(&mut line)?;
        let size = line.trim_end().split(';').next().unwrap_or("").trim();
        let size =
            usize::from_str_radix(size, 16).map_err(|_| invalid(format!("chunk size {size:?}")))?;
        if size == 0 {
            break;
        }
        if body.len() + size > MAX_BODY {
            return Err(invalid(format!(
                "the answer's body is over {MAX_BODY} bytes"
            )));
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader.read_exact(&mut body[start..])?;
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf)?;
        if &crlf != b"\r\n" {
            return Err(invalid("a chunk does not end in CRLF".to_string()));
        }
    }
    // The trailers, to the empty line (or the close).
    loop {
        line.clear();
        if reader.by_ref().take(MAX_HEAD as u64).read_line(&mut line)? == 0
            || line.trim_end().is_empty()
        {
            return Ok(body);
        }
    }
}

/// `GET path`, answered with JSON.
pub fn get_json(port: u16, path: &str, timeout: Duration) -> io::Result<(u16, Value)> {
    let reply = send(port, "GET", path, None, timeout)?;
    Ok((reply.status, reply.json()?))
}

/// `POST path` with a JSON body, answered with JSON.
pub fn post_json(
    port: u16,
    path: &str,
    body: &Value,
    timeout: Duration,
) -> io::Result<(u16, Value)> {
    let reply = send(
        port,
        "POST",
        path,
        Some(body.to_string().as_bytes()),
        timeout,
    )?;
    Ok((reply.status, reply.json()?))
}

/// Opens `GET path` as a server-sent event stream. A read waits at most
/// `idle` (pick more than the server's heartbeat interval).
pub fn events(port: u16, path: &str, idle: Duration) -> io::Result<EventStream> {
    let stream = connect(port, "GET", path, &[], None, idle)?;
    stream.set_read_timeout(Some(idle))?;
    let mut reader = BufReader::new(stream);
    let (status, headers) = read_head(&mut reader)?;
    let streaming = headers
        .iter()
        .any(|(key, value)| key == "content-type" && value.starts_with("text/event-stream"));
    if status != 200 || !streaming {
        return Err(io::Error::other(format!(
            "{path} answered {status}, not an event stream"
        )));
    }
    Ok(EventStream {
        reader,
        line: Vec::new(),
        event: String::new(),
        data: String::new(),
    })
}

/// One server-sent event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub event: String,
    pub data: String,
}

/// Reads events off a stream opened by [`events`].
pub struct EventStream {
    reader: BufReader<TcpStream>,
    line: Vec<u8>,
    event: String,
    data: String,
}

impl EventStream {
    /// The next event; `None` when the server closed the stream. Comments
    /// (heartbeats) are skipped. A timeout is an error the caller may retry:
    /// a partly read event is kept.
    pub fn next_event(&mut self) -> io::Result<Option<Event>> {
        loop {
            let room = MAX_EVENT_LINE.saturating_sub(self.line.len()) as u64 + 1;
            (&mut self.reader)
                .take(room)
                .read_until(b'\n', &mut self.line)?;
            if self.line.last() != Some(&b'\n') {
                if self.line.len() > MAX_EVENT_LINE {
                    return Err(invalid("an event line is too long".to_string()));
                }
                return Ok(None);
            }
            let line = String::from_utf8(std::mem::take(&mut self.line))
                .map_err(|_| invalid("an event line is not UTF-8".to_string()))?;
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if self.event.is_empty() && self.data.is_empty() {
                    continue;
                }
                let event = std::mem::take(&mut self.event);
                return Ok(Some(Event {
                    event: if event.is_empty() {
                        "message".to_string()
                    } else {
                        event
                    },
                    data: std::mem::take(&mut self.data),
                }));
            }
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => self.event = value.to_string(),
                "data" => {
                    if !self.data.is_empty() {
                        self.data.push('\n');
                    }
                    self.data.push_str(value);
                }
                _ => {}
            }
        }
    }
}

fn connect(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    timeout: Duration,
) -> io::Result<TcpStream> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|err| io::Error::new(err.kind(), format!("cannot connect to {addr}: {err}")))?;
    stream.set_write_timeout(Some(timeout))?;
    let wire = request_bytes(method, path, &format!("127.0.0.1:{port}"), headers, body)?;
    stream.write_all(&wire)?;
    stream.flush()?;
    Ok(stream)
}

/// The request's bytes: the request line, `Host`, `Connection: close`, the extra headers (a name
/// with a colon or a line break, or a value with a line break, is refused), then a JSON body.
fn request_bytes(
    method: &str,
    path: &str,
    host: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> io::Result<Vec<u8>> {
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (name, value) in headers {
        if name.contains(['\r', '\n', ':']) || value.contains(['\r', '\n']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("header {name:?} has a line break or colon"),
            ));
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        head.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    head.push_str("\r\n");
    let mut wire = head.into_bytes();
    wire.extend_from_slice(body.unwrap_or_default());
    Ok(wire)
}

/// The status code and the headers (names lower-cased).
pub(crate) fn read_head(reader: &mut impl BufRead) -> io::Result<(u16, Vec<(String, String)>)> {
    let mut line = Vec::new();
    let mut budget = MAX_HEAD;
    read_line(reader, &mut line, &mut budget)?;
    let status_line = String::from_utf8_lossy(&line);
    let mut parts = status_line.split(' ');
    let version = parts.next().unwrap_or_default();
    let code = parts.next().unwrap_or_default();
    if !matches!(version, "HTTP/1.1" | "HTTP/1.0") || code.len() != 3 {
        return Err(invalid(format!(
            "{status_line:?} is not an HTTP/1.x status line"
        )));
    }
    let status = code
        .parse()
        .map_err(|_| invalid(format!("{code:?} is not a status code")))?;
    let mut headers = Vec::new();
    loop {
        read_line(reader, &mut line, &mut budget)?;
        if line.is_empty() {
            return Ok((status, headers));
        }
        let header = String::from_utf8_lossy(&line);
        let Some((name, value)) = header.split_once(':') else {
            return Err(invalid("a header has no `:`".to_string()));
        };
        headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
    }
}

/// One head line, charged against what is left of the head's `budget`.
fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>, budget: &mut usize) -> io::Result<()> {
    line.clear();
    reader.take(*budget as u64).read_until(b'\n', line)?;
    if line.last() != Some(&b'\n') {
        return Err(invalid(
            "the answer's head is cut short or too large".to_string(),
        ));
    }
    *budget -= line.len();
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(())
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// A stream whose every read waits only for what is left of a deadline: the read timeout is set
/// on the connection's own socket before each read. (Not on a `try_clone`d handle: on Windows a
/// duplicated socket handle does not carry its timeout to the original, which would then block.)
struct Timed<R> {
    stream: R,
    deadline: Instant,
}

impl<R: Read + Duplex> Read for Timed<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let timed_out =
            || io::Error::new(io::ErrorKind::TimedOut, "the server did not answer in time");
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(timed_out());
        }
        self.stream.set_read_timeout(Some(left))?;
        self.stream.read(buf).map_err(|err| match err.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => timed_out(),
            _ => err,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_chunked_answer_is_read_whole() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n4;x=1\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\nX-Trailer: y\r\n\r\n";
        let reply = read_reply(std::io::BufReader::new(&raw[..])).expect("a chunked answer");
        assert_eq!(
            (reply.status, reply.body.as_slice()),
            (200, &b"{\"a\":1}"[..])
        );
        let bad = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n";
        assert!(read_reply(std::io::BufReader::new(&bad[..])).is_err());
    }

    use super::*;
    use crate::{Response, Router, Server};
    use std::net::TcpListener;

    fn server() -> crate::Handle {
        serve(Server::bind(0).expect("an ephemeral port"))
    }

    fn serve(server: Server) -> crate::Handle {
        let router: Router<()> = Router::new()
            .post("/v1/echo/{name}", |_, request, params| {
                let body: Value = request.json().unwrap_or(Value::Null);
                Response::json(
                    200,
                    &serde_json::json!({ "name": params.get("name"), "body": body }),
                )
            })
            .get("/v1/events", |_, _, _| {
                Response::sse(|sse| {
                    sse.event("run", r#"{"id":"r1"}"#)?;
                    sse.heartbeat()?;
                    sse.event("vm", r#"{"vm":0}"#)
                })
            });
        server
            .spawn(move |request| router.handle(&(), request))
            .expect("serves")
    }

    #[test]
    fn a_request_round_trips_and_a_rebinding_host_is_refused() {
        let server = server();
        let (status, body) = post_json(
            server.port(),
            "/v1/echo/ada",
            &serde_json::json!({ "n": 1 }),
            Duration::from_secs(5),
        )
        .expect("the server answers");
        assert_eq!(status, 200);
        assert_eq!(
            body,
            serde_json::json!({ "name": "ada", "body": { "n": 1 } })
        );
        let missing =
            send(server.port(), "GET", "/nope", None, Duration::from_secs(5)).expect("answers");
        assert_eq!(
            (missing.status, missing.content_type.as_str()),
            (404, crate::JSON)
        );

        // The rebinding guard: a foreign Host is refused, a Host-less probe served.
        let raw = |request: &str| {
            let mut stream = TcpStream::connect(("127.0.0.1", server.port())).expect("connects");
            stream.write_all(request.as_bytes()).expect("writes");
            let mut answer = String::new();
            stream.read_to_string(&mut answer).expect("reads");
            answer
        };
        let refused =
            raw("POST /v1/echo/x HTTP/1.1\r\nHost: evil.example:8899\r\nContent-Length: 0\r\n\r\n");
        assert!(
            refused.starts_with("HTTP/1.1 403 Forbidden\r\n"),
            "{refused}"
        );
        let served = raw("POST /v1/echo/x HTTP/1.1\r\nContent-Length: 0\r\n\r\n");
        assert!(served.starts_with("HTTP/1.1 200 OK\r\n"), "{served}");
        assert!(raw("GET /x\r\n\r\n").starts_with("HTTP/1.1 400 Bad Request\r\n"));
    }

    #[test]
    fn an_event_stream_is_relayed_event_by_event() {
        let server = server();
        let mut stream =
            events(server.port(), "/v1/events", Duration::from_secs(5)).expect("a stream");
        let event = |event: &str, data: &str| Event {
            event: event.into(),
            data: data.into(),
        };
        assert_eq!(
            stream.next_event().expect("reads"),
            Some(event("run", r#"{"id":"r1"}"#))
        );
        assert_eq!(
            stream.next_event().expect("reads"),
            Some(event("vm", r#"{"vm":0}"#))
        );
        assert_eq!(stream.next_event().expect("reads"), None);
    }

    #[test]
    fn a_silent_server_is_a_timeout_not_a_hang() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let port = listener.local_addr().expect("an address").port();
        let started = Instant::now();
        let err =
            send(port, "GET", "/", None, Duration::from_millis(200)).expect_err("nobody answers");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(listener);
    }

    #[test]
    fn a_target_over_plain_http_round_trips_json() {
        let server = server();
        let target = Target {
            base: format!("http://127.0.0.1:{}", server.port()),
            ..Target::default()
        };
        let (status, body) = request_json(
            &target,
            "POST",
            "/v1/echo/ada",
            Some(&serde_json::json!({ "n": 1 })),
            Duration::from_secs(5),
        )
        .expect("the server answers");
        assert_eq!(status, 200);
        assert_eq!(
            body,
            serde_json::json!({ "name": "ada", "body": { "n": 1 } })
        );
        let missing =
            request(&target, "GET", "/nope", None, Duration::from_secs(5)).expect("answers");
        assert_eq!(missing.status, 404);
        let ftp = Target {
            base: "ftp://127.0.0.1:1".to_string(),
            ..Target::default()
        };
        assert!(request(&ftp, "GET", "/", None, Duration::from_secs(5)).is_err());
    }

    /// An https server with a fresh self-signed certificate, and that certificate's pin.
    fn tls_server() -> (crate::Handle, [u8; 32]) {
        let pair = apz_trust::tls::self_signed(&["localhost".to_string()]).expect("a certificate");
        let config = apz_trust::tls::server_config(&pair).expect("a server config");
        let pin = apz_trust::tls::fingerprint(&pair.cert_pem).expect("a pin");
        (
            serve(Server::bind(0).expect("an ephemeral port").tls(config)),
            pin,
        )
    }

    #[test]
    fn a_target_over_https_answers_to_the_pinned_certificate() {
        let (server, pin) = tls_server();
        let target = Target {
            base: format!("https://localhost:{}", server.port()),
            pin: Some(pin),
            ..Target::default()
        };
        let (status, body) = request_json(
            &target,
            "POST",
            "/v1/echo/ada",
            Some(&serde_json::json!({ "n": 1 })),
            Duration::from_secs(5),
        )
        .expect("the server answers over TLS");
        assert_eq!(status, 200);
        assert_eq!(
            body,
            serde_json::json!({ "name": "ada", "body": { "n": 1 } })
        );
    }

    #[test]
    fn a_target_over_https_with_a_wrong_or_no_pin_fails() {
        let (server, _pin) = tls_server();
        let base = format!("https://localhost:{}", server.port());
        let wrong = Target {
            base: base.clone(),
            pin: Some([0; 32]),
            ..Target::default()
        };
        assert!(request(&wrong, "GET", "/v1/events", None, Duration::from_secs(5)).is_err());
        let unpinned = Target {
            base,
            pin: None,
            ..Target::default()
        };
        assert!(request(&unpinned, "GET", "/v1/events", None, Duration::from_secs(5)).is_err());
    }

    #[test]
    fn an_unpinned_https_target_refuses_a_self_signed_certificate() {
        // No pin means a public host: the Mozilla roots must sign it, and a self-signed hub cert is not signed by them.
        let (server, pin) = tls_server();
        let target = Target {
            base: format!("https://localhost:{}", server.port()),
            pin: None,
            ..Target::default()
        };
        let err = request(&target, "GET", "/v1/echo/ada", None, Duration::from_secs(5))
            .expect_err("no CA signs it");
        assert_ne!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        // The same server still answers its own pin, so the refusal is the CA check, not a dead server.
        let pinned = Target {
            base: target.base.clone(),
            pin: Some(pin),
            ..Target::default()
        };
        assert_eq!(
            request(&pinned, "GET", "/v1/nope", None, Duration::from_secs(5))
                .expect("answers")
                .status,
            404
        );
    }
}

/// Where a URL-based request goes: `http(s)://host:port` (a base, no path), an owner bearer token,
/// extra headers (e.g. a node's signature headers), and the TLS trust for https:
/// - `pin: Some(sha256)`: the plaza's own hub, whose self-signed certificate is pinned (SHA-256 of
///   its DER). No CA is consulted.
/// - `pin: None` with `https`: a public host, verified the normal way. The Mozilla root store
///   (webpki-roots) must sign the certificate, and it must name the host (an IP base is checked as
///   an IP address).
/// - `http`: plain TCP, for loopback daemons and tests only.
#[derive(Debug, Clone, Default)]
pub struct Target {
    pub base: String,
    pub pin: Option<[u8; 32]>,
    pub bearer: Option<String>,
    pub headers: Vec<(String, String)>,
}

/// One request to `target.base + path`, over plain TCP (http) or TLS (https: pinned for the hub,
/// CA-verified for a public host; see [`Target`]). Same reply
/// handling as [`send_with`] (Content-Length, chunked, or read to close).
pub fn request(
    target: &Target,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> io::Result<Reply> {
    let deadline = Instant::now() + timeout;
    let (tls, host, port) = split_base(&target.base)?;
    let host_header = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let bearer = target
        .bearer
        .as_ref()
        .map(|token| format!("Bearer {token}"));
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(bearer) = &bearer {
        headers.push(("Authorization", bearer.as_str()));
    }
    headers.extend(
        target
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
    );
    let wire = request_bytes(method, path, &host_header, &headers, body)?;
    if !tls {
        let stream = dial(&host, port, timeout)?;
        return exchange(stream, deadline, &wire);
    }
    let session = tls_session(target.pin, &host)?;
    let stream = dial(&host, port, timeout)?;
    exchange(rustls::StreamOwned::new(session, stream), deadline, &wire)
}

/// [`request`] with a JSON body and a JSON answer (Null when empty).
pub fn request_json(
    target: &Target,
    method: &str,
    path: &str,
    body: Option<&Value>,
    timeout: Duration,
) -> io::Result<(u16, Value)> {
    let bytes = body.map(|body| body.to_string().into_bytes());
    let reply = request(target, method, path, bytes.as_deref(), timeout)?;
    let value = if reply.body.is_empty() {
        Value::Null
    } else {
        reply.json()?
    };
    Ok((reply.status, value))
}

/// [`request`] over a stream the caller already connected, such as a vsock stream to a node's relay. TLS runs on it
/// when `tls` names the server (the name is the SNI and the Host header), pinned by `target.pin`, which is then
/// required: a stream with no host name cannot be verified any other way. With `tls: None` the bytes go over the
/// stream as they are. `target.base` is not dialled. The stream's own read timeout is replaced by `timeout`'s
/// deadline while the reply is read.
pub fn request_over<S: Read + Write + Duplex + 'static>(
    stream: S,
    tls: Option<&str>,
    target: &Target,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> io::Result<Reply> {
    let deadline = Instant::now() + timeout;
    let bearer = target
        .bearer
        .as_ref()
        .map(|token| format!("Bearer {token}"));
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(bearer) = &bearer {
        headers.push(("Authorization", bearer.as_str()));
    }
    headers.extend(
        target
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
    );
    // The stream names no host: the Host header is `localhost` whatever the TLS name (the SNI) is. A hub on the
    // relaying node's own machine sees the connection come from loopback, and takes only a loopback Host there.
    let wire = request_bytes(method, path, "localhost", &headers, body)?;
    match tls {
        None => exchange(stream, deadline, &wire),
        Some(name) => {
            let pin = target.pin.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a stream with no host name needs the hub's certificate pin",
                )
            })?;
            let session = tls_session(Some(pin), name)?;
            exchange(rustls::StreamOwned::new(session, stream), deadline, &wire)
        }
    }
}

/// [`request_over`] with a JSON body and a JSON answer (Null when empty).
pub fn request_json_over<S: Read + Write + Duplex + 'static>(
    stream: S,
    tls: Option<&str>,
    target: &Target,
    method: &str,
    path: &str,
    body: Option<&Value>,
    timeout: Duration,
) -> io::Result<(u16, Value)> {
    let bytes = body.map(|body| body.to_string().into_bytes());
    let reply = request_over(stream, tls, target, method, path, bytes.as_deref(), timeout)?;
    let value = if reply.body.is_empty() {
        Value::Null
    } else {
        reply.json()?
    };
    Ok((reply.status, value))
}

/// Splits `http://host:port` or `https://host:port` (no path) into (is https, host without
/// brackets, port). A missing port is 80 or 443.
pub fn split_base(base: &str) -> io::Result<(bool, String, u16)> {
    let bad = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{base:?} is not an http:// or https:// base of host:port"),
        )
    };
    let (tls, rest) = match (base.strip_prefix("https://"), base.strip_prefix("http://")) {
        (Some(rest), _) => (true, rest),
        (None, Some(rest)) => (false, rest),
        _ => return Err(bad()),
    };
    let (host, port) = match rest.strip_prefix('[') {
        Some(v6) => {
            let (host, after) = v6.split_once(']').ok_or_else(bad)?;
            if after.is_empty() {
                (host, "")
            } else {
                (host, after.strip_prefix(':').ok_or_else(bad)?)
            }
        }
        None => rest.split_once(':').unwrap_or((rest, "")),
    };
    if host.is_empty() || host.contains('/') {
        return Err(bad());
    }
    let port = if port.is_empty() {
        if tls { 443 } else { 80 }
    } else {
        port.parse::<u16>().map_err(|_| bad())?
    };
    Ok((tls, host.to_string(), port))
}

/// A TCP connection to `host:port`, tried address by address; reads and writes time out after `timeout`.
pub(crate) fn dial(host: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, format!("{host} has no address"));
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => {
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))?;
                return Ok(stream);
            }
            Err(err) => last = err,
        }
    }
    Err(last)
}

/// The TLS session to `host`, shared by [`request`] and `ws::connect`. `Some(pin)` is the plaza's
/// own hub: its self-signed certificate, pinned, with no CA; the name only has to be valid for the
/// handshake. `None` is a public host: the Mozilla roots must sign the certificate and it must name
/// `host` (an IP address is checked as an IP address; there is no fallback name).
pub(crate) fn tls_session(pin: Option<[u8; 32]>, host: &str) -> io::Result<ClientConnection> {
    let (config, name) = match pin {
        Some(pin) => {
            let name = ServerName::try_from(host.to_string())
                .or_else(|_| ServerName::try_from("hub".to_string()));
            (apz_trust::tls::pinned_client_config(pin), name)
        }
        None => (
            public_client_config(),
            ServerName::try_from(host.to_string()),
        ),
    };
    let name = name.map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{host:?} as a TLS name: {err}"),
        )
    })?;
    ClientConnection::new(config, name).map_err(|err| io::Error::other(format!("TLS setup: {err}")))
}

/// The CA-verified configuration for public hosts, built once: the Mozilla root store, ring, the
/// default protocol versions, no client certificate.
fn public_client_config() -> Arc<ClientConfig> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("the ring provider supports the default protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// Writes the request, then reads the answer through [`Timed`], bounded by `deadline`.
fn exchange<S: Read + Write + Duplex>(
    mut stream: S,
    deadline: Instant,
    wire: &[u8],
) -> io::Result<Reply> {
    stream.write_all(wire)?;
    stream.flush()?;
    read_reply(BufReader::new(Timed { stream, deadline }))
}

/// Requests over a stream the caller holds, not a TCP socket: a Unix-socket pair stands in for a vsock relay.
#[cfg(all(test, unix))]
mod over_stream_tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::thread;

    /// A Unix stream the client hands to `request_json_over`.
    struct Wire(UnixStream);

    impl Read for Wire {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl Write for Wire {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    impl Duplex for Wire {
        fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            self.0.set_read_timeout(timeout)
        }
    }

    #[test]
    fn a_json_request_is_sent_over_pinned_tls_on_a_stream_the_caller_holds() {
        let pair = apz_trust::tls::self_signed(&["hub".to_string()]).expect("a certificate");
        let config = apz_trust::tls::server_config(&pair).expect("a server config");
        let pin = apz_trust::tls::fingerprint(&pair.cert_pem).expect("a pin");
        let (client, server) = UnixStream::pair().expect("a stream pair");

        let serving = thread::spawn(move || {
            let connection = rustls::ServerConnection::new(config).expect("a server connection");
            let mut tls = rustls::StreamOwned::new(connection, server);
            let mut seen = Vec::new();
            let mut chunk = [0u8; 512];
            while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = tls.read(&mut chunk).expect("the request");
                assert!(n > 0, "the client closed before its head");
                seen.extend_from_slice(&chunk[..n]);
            }
            let body = br#"{"station":"ws-9"}"#;
            let reply = format!(
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            tls.write_all(reply.as_bytes()).expect("the head");
            tls.write_all(body).expect("the body");
            tls.flush().expect("flush");
            String::from_utf8_lossy(&seen).to_string()
        });

        let target = Target {
            base: String::new(),
            pin: Some(pin),
            bearer: Some("tok".into()),
            headers: Vec::new(),
        };
        let (status, value) = request_json_over(
            Wire(client),
            Some("hub"),
            &target,
            "POST",
            "/v1/stations/join",
            Some(&serde_json::json!({"code": "c"})),
            Duration::from_secs(5),
        )
        .expect("the reply");
        assert_eq!(
            (status, value),
            (201, serde_json::json!({"station": "ws-9"}))
        );

        let seen = serving.join().expect("the server");
        assert!(
            seen.starts_with("POST /v1/stations/join HTTP/1.1\r\n"),
            "{seen}"
        );
        assert!(
            seen.contains("Host: localhost\r\n"),
            "the stream names no host: {seen}"
        );
        assert!(seen.contains("Authorization: Bearer tok\r\n"), "{seen}");
    }

    #[test]
    fn a_stream_with_no_host_name_is_refused_without_a_pin() {
        let (client, _server) = UnixStream::pair().expect("a stream pair");
        let target = Target::default();
        let err = request_over(
            Wire(client),
            Some("hub"),
            &target,
            "GET",
            "/",
            None,
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{err}");
    }
}
