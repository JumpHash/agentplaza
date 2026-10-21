//! Requests, responses, and the SSE writer.

use std::fmt;
use std::io::{self, BufRead, Read, Write};
use std::net::IpAddr;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::ws;

/// The JSON content type every daemon answers with.
pub const JSON: &str = "application/json; charset=utf-8";

/// How large one request may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The request line and headers together.
    pub max_head: usize,
    /// The body (`Content-Length`).
    pub max_body: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_head: 16 << 10,
            max_body: 1 << 20,
        }
    }
}

/// One parsed request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// The target's path, without query or fragment.
    pub path: String,
    /// The query string without its `?`; empty when there is none.
    pub query: String,
    /// Header names lower-cased, values trimmed, in arrival order.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The connection's remote address; `None` when the request was read
    /// from a plain buffer rather than a connection, and `0.0.0.0` when a
    /// node relayed it from a VM (see [`crate::RELAY_PREFACE`]).
    pub peer: Option<IpAddr>,
}

impl Request {
    /// The first header called `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The value of `name=` in the query string (not percent-decoded).
    pub fn query_param(&self, name: &str) -> Option<&str> {
        self.query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value)
    }

    /// The body as JSON.
    pub fn json<T: DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_slice(&self.body)
    }

    /// Reads one request. The error is the 400's message: anything malformed
    /// or over `limits` is refused, never a panic.
    pub fn read(reader: &mut impl BufRead, limits: Limits) -> Result<Self, &'static str> {
        let (mut request, length) = Self::read_head(reader, limits)?;
        request.read_body(reader, length)?;
        Ok(request)
    }

    /// Reads the request line and headers only, and returns the request (its body still empty) with
    /// the body's `Content-Length`. A server can decide whether, and how much, body to take before
    /// reading it ([`Request::read_body`]).
    pub fn read_head(
        reader: &mut impl BufRead,
        limits: Limits,
    ) -> Result<(Self, Option<usize>), &'static str> {
        let mut line = Vec::new();
        read_line(reader, limits.max_head, &mut line)?;
        let request_line =
            std::str::from_utf8(&line).map_err(|_| "the request line is not ASCII")?;
        let mut parts = request_line.split(' ');
        let (Some(method), Some(target), Some(version), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err("the request line must be `METHOD TARGET HTTP/1.x`");
        };
        if method.is_empty() || !method.bytes().all(is_token_byte) {
            return Err("the method is not a token");
        }
        if version != "HTTP/1.1" && version != "HTTP/1.0" {
            return Err("only HTTP/1.0 and HTTP/1.1 are served");
        }
        // Origin-form only, and only bytes a response header could carry.
        if !target.starts_with('/') || !target.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
            return Err("the request target must be an origin-form path");
        }
        let target = target.split('#').next().unwrap_or(target);
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let mut request = Request {
            method: method.to_string(),
            path: path.to_string(),
            query: query.to_string(),
            ..Request::default()
        };

        let mut head_bytes = line.len() + 2;
        let mut content_length = None;
        loop {
            read_line(reader, limits.max_head, &mut line)?;
            head_bytes += line.len() + 2;
            if head_bytes > limits.max_head {
                return Err("the request headers are too large");
            }
            if line.is_empty() {
                break;
            }
            let header = std::str::from_utf8(&line).map_err(|_| "a request header is not ASCII")?;
            let Some((name, value)) = header.split_once(':') else {
                return Err("a request header has no `:`");
            };
            let name = name.to_ascii_lowercase();
            let value = value.trim_matches([' ', '\t']);
            match name.as_str() {
                "content-length" => {
                    if content_length.is_some() {
                        return Err("the request has more than one Content-Length");
                    }
                    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                        return Err("Content-Length is not a number");
                    }
                    let length: usize = value.parse().map_err(|_| "Content-Length is too large")?;
                    if length > limits.max_body {
                        return Err("the request body is too large");
                    }
                    content_length = Some(length);
                }
                // One reading of a body's length only.
                "transfer-encoding" => return Err("chunked request bodies are not supported"),
                _ => {}
            }
            request.headers.push((name, value.to_string()));
        }

        Ok((request, content_length))
    }

    /// Reads the body [`Request::read_head`] announced (`length` bytes, or none).
    pub fn read_body(
        &mut self,
        reader: &mut impl BufRead,
        length: Option<usize>,
    ) -> Result<(), &'static str> {
        if let Some(length) = length {
            reader
                .take(length as u64)
                .read_to_end(&mut self.body)
                .map_err(|_| "the request body could not be read")?;
            if self.body.len() != length {
                return Err("the request body is shorter than Content-Length");
            }
        }
        Ok(())
    }
}

/// Reads one `\n`-terminated line into `line`, without its `\r\n`.
fn read_line(
    reader: &mut impl BufRead,
    limit: usize,
    line: &mut Vec<u8>,
) -> Result<(), &'static str> {
    line.clear();
    let read = reader
        .take(limit as u64 + 1)
        .read_until(b'\n', line)
        .map_err(|_| "the request could not be read")?;
    // The length first: a line past the limit is cut at `limit + 1` bytes, before its newline.
    if line.len() > limit {
        return Err("a request line or header is too long");
    }
    if read == 0 || line.last() != Some(&b'\n') {
        return Err("the request ended before it was complete");
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(())
}

/// RFC 9110 `tchar`.
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

/// Whether a `Host` header names this machine: `localhost`, `127.0.0.1`, `::1` or `*.localhost` with a
/// non-empty label (case-insensitive), with an optional `:port`. An IPv6 address is bracketed when a port
/// follows it (`[::1]:7700`). This is the one definition of the local part; the hub's owner trust uses it.
pub fn host_is_loopback(host: &str) -> bool {
    let name = if let Some(rest) = host.strip_prefix('[') {
        // Bracketed IPv6: the port, if any, follows the closing bracket and nothing else may.
        match rest.split_once(']') {
            Some((name, after)) if after.is_empty() || after.starts_with(':') => name,
            _ => return false,
        }
    } else if host.matches(':').count() > 1 {
        // A bare IPv6 address has no port to drop.
        host
    } else {
        host.split(':').next().unwrap_or_default()
    };
    let name = name.to_ascii_lowercase();
    matches!(name.as_str(), "localhost" | "127.0.0.1" | "::1")
        || name
            .strip_suffix(".localhost")
            .is_some_and(|label| !label.is_empty())
}

/// Writes a streamed body to the connection.
pub type StreamFn = Box<dyn FnOnce(&mut dyn Write) -> io::Result<()> + Send>;

/// Runs an upgraded connection (a WebSocket) on its own thread.
pub type UpgradeFn = Box<dyn FnOnce(Box<dyn ws::Duplex>) + Send>;

/// A response body: bytes, a stream written straight to the connection, or an upgrade: the 101
/// head goes out, then the connection itself is handed to the closure.
pub enum Body {
    Bytes(Vec<u8>),
    Stream(StreamFn),
    Upgrade(UpgradeFn),
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Body::Bytes(_) => "Bytes",
            Body::Stream(_) => "Stream",
            Body::Upgrade(_) => "Upgrade",
        })
    }
}

/// One response. Every response is written with `Connection: close`, and with
/// `Cache-Control: no-store` unless the handler set its own `Cache-Control`.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".to_string(), content_type.to_string())],
            body: Body::Bytes(body.into()),
        }
    }

    /// `value` as JSON; a 500 if it cannot be serialized.
    pub fn json(status: u16, value: &impl Serialize) -> Self {
        match serde_json::to_vec(value) {
            Ok(body) => Self::new(status, JSON, body),
            Err(err) => Self::error(500, &format!("cannot serialize the response: {err}")),
        }
    }

    /// `{"error": message}`.
    pub fn error(status: u16, message: &str) -> Self {
        Self::new(
            status,
            JSON,
            serde_json::json!({ "error": message }).to_string(),
        )
    }

    pub fn text(status: u16, text: &str) -> Self {
        Self::new(status, "text/plain; charset=utf-8", text)
    }

    /// A `text/event-stream`: `stream` runs on the connection's thread and the
    /// connection closes when it returns.
    pub fn sse(stream: impl FnOnce(&mut Sse<'_>) -> io::Result<()> + Send + 'static) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
            body: Body::Stream(Box::new(move |out| stream(&mut Sse { out }))),
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// The body's bytes; empty for a stream.
    pub fn bytes(&self) -> &[u8] {
        match &self.body {
            Body::Bytes(bytes) => bytes,
            Body::Stream(_) | Body::Upgrade(_) => &[],
        }
    }

    /// Writes the response. An upgrade writes its 101 head (no `Connection: close`: the upgrade
    /// headers say `Connection: Upgrade`) and returns its closure, which the caller runs on the
    /// connection; every other body returns `None`.
    pub fn write_to(self, out: &mut impl Write) -> io::Result<Option<UpgradeFn>> {
        let mut head = format!("HTTP/1.1 {} {}\r\n", self.status, reason(self.status));
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if let Body::Bytes(bytes) = &self.body {
            head.push_str(&format!("Content-Length: {}\r\n", bytes.len()));
        }
        if !self
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("cache-control"))
        {
            head.push_str("Cache-Control: no-store\r\n");
        }
        if !matches!(self.body, Body::Upgrade(_)) {
            head.push_str("Connection: close\r\n");
        }
        head.push_str("\r\n");
        out.write_all(head.as_bytes())?;
        let upgrade = match self.body {
            Body::Bytes(bytes) => {
                out.write_all(&bytes)?;
                None
            }
            Body::Stream(stream) => {
                out.flush()?;
                stream(out)?;
                None
            }
            Body::Upgrade(run) => Some(run),
        };
        out.flush()?;
        Ok(upgrade)
    }
}

fn reason(status: u16) -> &'static str {
    const REASONS: [(u16, &str); 21] = [
        (101, "Switching Protocols"),
        (200, "OK"),
        (201, "Created"),
        (202, "Accepted"),
        (204, "No Content"),
        (400, "Bad Request"),
        (401, "Unauthorized"),
        (403, "Forbidden"),
        (404, "Not Found"),
        (405, "Method Not Allowed"),
        (408, "Request Timeout"),
        (409, "Conflict"),
        (413, "Content Too Large"),
        (422, "Unprocessable Content"),
        (423, "Locked"),
        (426, "Upgrade Required"),
        (429, "Too Many Requests"),
        (500, "Internal Server Error"),
        (502, "Bad Gateway"),
        (503, "Service Unavailable"),
        (504, "Gateway Timeout"),
    ];
    REASONS
        .iter()
        .find(|(code, _)| *code == status)
        .map_or("Unknown", |(_, text)| text)
}

/// Writes server-sent events: `event: <name>` plus one `data:` line, and
/// `: heartbeat` comments. Every write is flushed, so a dead client fails the
/// next write.
pub struct Sse<'a> {
    out: &'a mut dyn Write,
}

impl Sse<'_> {
    /// One event. `event` and `data` must each be a single line.
    pub fn event(&mut self, event: &str, data: &str) -> io::Result<()> {
        if event.contains(['\n', '\r']) || data.contains(['\n', '\r']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "an SSE field spans lines",
            ));
        }
        // One write per frame: a reader never sees half of one.
        self.out
            .write_all(format!("event: {event}\ndata: {data}\n\n").as_bytes())?;
        self.out.flush()
    }

    pub fn heartbeat(&mut self) -> io::Result<()> {
        self.out.write_all(b": heartbeat\n\n")?;
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn read(raw: &[u8]) -> Result<Request, &'static str> {
        Request::read(&mut Cursor::new(raw), Limits::default())
    }

    #[test]
    fn a_request_is_read_with_its_query_headers_and_body() {
        let request = read(
            b"POST /v1/runs?after=3&x=y#frag HTTP/1.1\r\nHost: localhost\r\ncontent-length: 7\r\n\r\n{\"n\":1}",
        )
        .expect("a well-formed POST");
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/v1/runs");
        assert_eq!(request.query_param("after"), Some("3"));
        assert_eq!(request.header("HOST"), Some("localhost"));
        assert_eq!(request.body, b"{\"n\":1}");
    }

    #[test]
    fn malformed_and_oversized_requests_are_refused() {
        let long_header = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(16 << 10));
        let many_headers = format!("GET / HTTP/1.1\r\n{}\r\n", "X: 1\r\n".repeat(4 << 10));
        let huge_body = format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            (1 << 20) + 1
        );
        for raw in [
            "",
            "GET\r\n\r\n",
            "GET /x\r\n\r\n",
            "GET /x HTTP/2\r\n\r\n",
            "GET  /x HTTP/1.1\r\n\r\n",
            "GET x HTTP/1.1\r\n\r\n",
            "G ET /x HTTP/1.1\r\n\r\n",
            "GET /a\tb HTTP/1.1\r\n\r\n",
            "GET /x HTTP/1.1\r\nno-colon\r\n\r\n",
            "POST /x HTTP/1.1\r\nContent-Length: +5\r\n\r\n12345",
            "POST /x HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx",
            "POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            "POST /x HTTP/1.1\r\nContent-Length: 9\r\n\r\nshort",
            "GET /x HTTP/1.1\r\nHost: localhost\r\n",
            &long_header,
            &many_headers,
            &huge_body,
        ] {
            assert!(
                read(raw.as_bytes()).is_err(),
                "should have been refused: {raw:.60?}"
            );
        }
        assert!(read(b"GET /x HTTP/1.1\r\nX: \xff\r\n\r\n").is_err());
    }

    /// Each limit refuses with its own reason, so a limit that stops working cannot hide behind another check
    /// (an oversized body is also too short when it is not sent: the reason says which check refused it).
    #[test]
    fn each_limit_refuses_with_its_own_reason() {
        let limits = Limits {
            max_head: 64,
            max_body: 8,
        };
        let read_with = |raw: &[u8]| Request::read(&mut Cursor::new(raw), limits);
        // At the limits: served.
        let at_limit = read_with(b"POST /x HTTP/1.1\r\nContent-Length: 8\r\n\r\n12345678")
            .expect("8 bytes is the cap");
        assert_eq!(at_limit.body, b"12345678");
        // The request line (15 + CRLF), one header (3 + 40 + CRLF) and the blank line: exactly max_head bytes.
        let head_at_limit = format!("GET /x HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(40));
        assert_eq!(head_at_limit.len(), 64);
        assert!(
            read_with(head_at_limit.as_bytes()).is_ok(),
            "a head of exactly max_head bytes is served"
        );
        // One past each limit: refused, for that reason.
        for (raw, reason) in [
            (
                "POST /x HTTP/1.1\r\nContent-Length: 9\r\n\r\n123456789".to_string(),
                "the request body is too large",
            ),
            (
                format!("GET /x HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(64)),
                "a request line or header is too long",
            ),
            (
                format!("GET /x HTTP/1.1\r\n{}\r\n", "X: 12345678\r\n".repeat(4)),
                "the request headers are too large",
            ),
            (
                "G@T /x HTTP/1.1\r\n\r\n".to_string(),
                "the method is not a token",
            ),
            (
                " /x HTTP/1.1\r\n\r\n".to_string(),
                "the method is not a token",
            ),
        ] {
            assert_eq!(read_with(raw.as_bytes()), Err(reason), "{raw:.60?}");
        }
    }

    #[test]
    fn only_loopback_hosts_pass_the_guard() {
        for host in [
            "127.0.0.1",
            "127.0.0.1:8899",
            "localhost",
            "LOCALHOST:19080",
            "::1",
            "[::1]",
            "[::1]:8899",
            "app.localhost:7700",
            "x.localhost",
        ] {
            assert!(host_is_loopback(host), "{host}");
        }
        for host in [
            "evil.example",
            "127.0.0.1.evil.example",
            "10.0.0.1:8899",
            "",
            ".localhost",
            "localhost.evil.example",
            "[::1].evil.example",
            "[::1",
            "::1:8899",
            "[::2]:8899",
        ] {
            assert!(!host_is_loopback(host), "{host}");
        }
    }

    #[test]
    fn a_response_carries_its_length_and_closes() {
        let mut written = Vec::new();
        Response::error(404, "say \"what\"")
            .write_to(&mut written)
            .expect("a Vec takes it");
        let written = String::from_utf8(written).expect("ASCII");
        assert!(
            written.starts_with("HTTP/1.1 404 Not Found\r\n"),
            "{written}"
        );
        assert!(written.contains(
            "Content-Length: 24\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
        ));
        assert!(
            written.ends_with("{\"error\":\"say \\\"what\\\"\"}"),
            "{written}"
        );
    }

    #[test]
    fn a_handler_cache_control_replaces_no_store() {
        let mut written = Vec::new();
        Response::text(200, "x")
            .with_header("Cache-Control", "max-age=60")
            .write_to(&mut written)
            .expect("a Vec takes it");
        let written = String::from_utf8(written).expect("ASCII");
        assert!(
            written.contains("Cache-Control: max-age=60\r\n"),
            "{written}"
        );
        assert!(!written.contains("no-store"), "{written}");
    }
}
