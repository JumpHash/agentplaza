//! Requests, responses, and the SSE writer.

use std::fmt;
use std::io::{self, BufRead, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

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
        Self { max_head: 16 << 10, max_body: 1 << 20 }
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
    /// The connecting peer's address; None when the server did not record it.
    pub peer: Option<std::net::IpAddr>,
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

        if let Some(length) = content_length {
            reader
                .take(length as u64)
                .read_to_end(&mut request.body)
                .map_err(|_| "the request body could not be read")?;
            if request.body.len() != length {
                return Err("the request body is shorter than Content-Length");
            }
        }
        Ok(request)
    }
}

/// Reads one `\n`-terminated line into `line`, without its `\r\n`.
fn read_line(reader: &mut impl BufRead, limit: usize, line: &mut Vec<u8>) -> Result<(), &'static str> {
    line.clear();
    let read = reader
        .take(limit as u64 + 1)
        .read_until(b'\n', line)
        .map_err(|_| "the request could not be read")?;
    if read == 0 || line.last() != Some(&b'\n') {
        return Err("the request ended before it was complete");
    }
    if line.len() > limit {
        return Err("a request line or header is too long");
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

/// Whether a `Host` header names loopback: `127.0.0.1` or `localhost`
/// (case-insensitive), with an optional `:port`.
pub fn host_is_loopback(host: &str) -> bool {
    let name = host.split(':').next().unwrap_or(host);
    name.eq_ignore_ascii_case("127.0.0.1") || name.eq_ignore_ascii_case("localhost")
}

/// Writes a streamed body to the connection.
pub type StreamFn = Box<dyn FnOnce(&mut dyn Write) -> io::Result<()> + Send>;

/// A response body: bytes, or a stream written straight to the connection.
pub enum Body {
    Bytes(Vec<u8>),
    Stream(StreamFn),
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if matches!(self, Body::Bytes(_)) { "Bytes" } else { "Stream" })
    }
}

/// One response. Every response is written with `Cache-Control: no-store`
/// and `Connection: close`.
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
        Self::new(status, JSON, serde_json::json!({ "error": message }).to_string())
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
            Body::Stream(_) => &[],
        }
    }

    pub fn write_to(self, out: &mut impl Write) -> io::Result<()> {
        let mut head = format!("HTTP/1.1 {} {}\r\n", self.status, reason(self.status));
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if let Body::Bytes(bytes) = &self.body {
            head.push_str(&format!("Content-Length: {}\r\n", bytes.len()));
        }
        head.push_str("Cache-Control: no-store\r\nConnection: close\r\n\r\n");
        out.write_all(head.as_bytes())?;
        match self.body {
            Body::Bytes(bytes) => out.write_all(&bytes)?,
            Body::Stream(stream) => {
                out.flush()?;
                stream(out)?;
            }
        }
        out.flush()
    }
}

fn reason(status: u16) -> &'static str {
    const REASONS: [(u16, &str); 14] = [
        (200, "OK"), (201, "Created"), (202, "Accepted"), (400, "Bad Request"), (403, "Forbidden"),
        (404, "Not Found"), (405, "Method Not Allowed"), (409, "Conflict"), (413, "Content Too Large"),
        (422, "Unprocessable Content"), (500, "Internal Server Error"), (502, "Bad Gateway"),
        (503, "Service Unavailable"), (504, "Gateway Timeout"),
    ];
    REASONS.iter().find(|(code, _)| *code == status).map_or("Unknown", |(_, text)| text)
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
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "an SSE field spans lines"));
        }
        // One write per frame: a reader never sees half of one.
        self.out.write_all(format!("event: {event}\ndata: {data}\n\n").as_bytes())?;
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
        let huge_body = format!("POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", (1 << 20) + 1);
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
            assert!(read(raw.as_bytes()).is_err(), "should have been refused: {raw:.60?}");
        }
        assert!(read(b"GET /x HTTP/1.1\r\nX: \xff\r\n\r\n").is_err());
    }

    #[test]
    fn only_loopback_hosts_pass_the_guard() {
        for host in ["127.0.0.1", "127.0.0.1:8899", "localhost", "LOCALHOST:19080"] {
            assert!(host_is_loopback(host), "{host}");
        }
        for host in ["evil.example", "127.0.0.1.evil.example", "10.0.0.1:8899", "[::1]:8899", ""] {
            assert!(!host_is_loopback(host), "{host}");
        }
    }

    #[test]
    fn a_response_carries_its_length_and_closes() {
        let mut written = Vec::new();
        Response::error(404, "say \"what\"").write_to(&mut written).expect("a Vec takes it");
        let written = String::from_utf8(written).expect("ASCII");
        assert!(written.starts_with("HTTP/1.1 404 Not Found\r\n"), "{written}");
        assert!(written.contains("Content-Length: 24\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"));
        assert!(written.ends_with("{\"error\":\"say \\\"what\\\"\"}"), "{written}");
    }
}
