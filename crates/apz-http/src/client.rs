//! A blocking client for loopback daemons: one request per connection to
//! `127.0.0.1:port` with a loopback `Host`, the whole exchange bounded by a
//! timeout. Errors are `io::Error`s — refused, timed out, or `InvalidData` for
//! an answer that is not well-formed HTTP/1.x.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use serde_json::Value;

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
}

impl Reply {
    pub fn json(&self) -> io::Result<Value> {
        serde_json::from_slice(&self.body).map_err(|err| invalid(format!("the answer is not JSON: {err}")))
    }
}

/// Sends one request and reads the whole answer.
pub fn send(port: u16, method: &str, path: &str, body: Option<&[u8]>, timeout: Duration) -> io::Result<Reply> {
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
    let mut reader = BufReader::new(Timed { stream, deadline });
    let (status, headers) = read_head(&mut reader)?;
    let header = |name: &str| headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str());
    if header("transfer-encoding").is_some() {
        return Err(invalid("the answer is chunked".to_string()));
    }
    // Without a Content-Length the body runs to the close.
    let length = header("content-length")
        .map(|length| length.parse::<usize>().map_err(|_| invalid(format!("Content-Length {length:?}"))))
        .transpose()?;
    let mut body = Vec::new();
    reader.take(length.unwrap_or(MAX_BODY + 1).min(MAX_BODY + 1) as u64).read_to_end(&mut body)?;
    if body.len() > MAX_BODY || length.is_some_and(|length| length != body.len()) {
        return Err(invalid(format!("the answer's body is {} bytes, not {length:?}", body.len())));
    }
    // Printable ASCII only: a proxy writes this into its own response head.
    let content_type = match header("content-type") {
        Some(value) if !value.is_empty() && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) => value,
        _ => "application/octet-stream",
    };
    Ok(Reply { status, content_type: content_type.to_string(), body })
}

/// `GET path`, answered with JSON.
pub fn get_json(port: u16, path: &str, timeout: Duration) -> io::Result<(u16, Value)> {
    let reply = send(port, "GET", path, None, timeout)?;
    Ok((reply.status, reply.json()?))
}

/// `POST path` with a JSON body, answered with JSON.
pub fn post_json(port: u16, path: &str, body: &Value, timeout: Duration) -> io::Result<(u16, Value)> {
    let reply = send(port, "POST", path, Some(body.to_string().as_bytes()), timeout)?;
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
        return Err(io::Error::other(format!("{path} answered {status}, not an event stream")));
    }
    Ok(EventStream { reader, line: Vec::new(), event: String::new(), data: String::new() })
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
            (&mut self.reader).take(room).read_until(b'\n', &mut self.line)?;
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
                    event: if event.is_empty() { "message".to_string() } else { event },
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
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    for (name, value) in headers {
        if name.contains(['\r', '\n', ':']) || value.contains(['\r', '\n']) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("header {name:?} has a line break or colon")));
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    let mut wire = head.into_bytes();
    wire.extend_from_slice(body.unwrap_or_default());
    stream.write_all(&wire)?;
    stream.flush()?;
    Ok(stream)
}

/// The status code and the headers (names lower-cased).
fn read_head(reader: &mut impl BufRead) -> io::Result<(u16, Vec<(String, String)>)> {
    let mut line = Vec::new();
    let mut budget = MAX_HEAD;
    read_line(reader, &mut line, &mut budget)?;
    let status_line = String::from_utf8_lossy(&line);
    let mut parts = status_line.split(' ');
    let version = parts.next().unwrap_or_default();
    let code = parts.next().unwrap_or_default();
    if !matches!(version, "HTTP/1.1" | "HTTP/1.0") || code.len() != 3 {
        return Err(invalid(format!("{status_line:?} is not an HTTP/1.x status line")));
    }
    let status = code.parse().map_err(|_| invalid(format!("{code:?} is not a status code")))?;
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
        return Err(invalid("the answer's head is cut short or too large".to_string()));
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

/// A stream whose every read waits only for what is left of a deadline.
struct Timed {
    stream: TcpStream,
    deadline: Instant,
}

impl Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let timed_out = || io::Error::new(io::ErrorKind::TimedOut, "the server did not answer in time");
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
    use super::*;
    use crate::{Response, Router, Server};
    use std::net::TcpListener;

    fn server() -> crate::Handle {
        let router: Router<()> = Router::new()
            .post("/v1/echo/{name}", |_, request, params| {
                let body: Value = request.json().unwrap_or(Value::Null);
                Response::json(200, &serde_json::json!({ "name": params.get("name"), "body": body }))
            })
            .get("/v1/events", |_, _, _| {
                Response::sse(|sse| {
                    sse.event("run", r#"{"id":"r1"}"#)?;
                    sse.heartbeat()?;
                    sse.event("vm", r#"{"vm":0}"#)
                })
            });
        Server::bind(0).expect("an ephemeral port").spawn(move |request| router.handle(&(), request)).expect("serves")
    }

    #[test]
    fn a_request_round_trips_and_a_rebinding_host_is_refused() {
        let server = server();
        let (status, body) =
            post_json(server.port(), "/v1/echo/ada", &serde_json::json!({ "n": 1 }), Duration::from_secs(5))
                .expect("the server answers");
        assert_eq!(status, 200);
        assert_eq!(body, serde_json::json!({ "name": "ada", "body": { "n": 1 } }));
        let missing = send(server.port(), "GET", "/nope", None, Duration::from_secs(5)).expect("answers");
        assert_eq!((missing.status, missing.content_type.as_str()), (404, crate::JSON));

        // The rebinding guard: a foreign Host is refused, a Host-less probe served.
        let raw = |request: &str| {
            let mut stream = TcpStream::connect(("127.0.0.1", server.port())).expect("connects");
            stream.write_all(request.as_bytes()).expect("writes");
            let mut answer = String::new();
            stream.read_to_string(&mut answer).expect("reads");
            answer
        };
        let refused = raw("POST /v1/echo/x HTTP/1.1\r\nHost: evil.example:8899\r\nContent-Length: 0\r\n\r\n");
        assert!(refused.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{refused}");
        let served = raw("POST /v1/echo/x HTTP/1.1\r\nContent-Length: 0\r\n\r\n");
        assert!(served.starts_with("HTTP/1.1 200 OK\r\n"), "{served}");
        assert!(raw("GET /x\r\n\r\n").starts_with("HTTP/1.1 400 Bad Request\r\n"));
    }

    #[test]
    fn an_event_stream_is_relayed_event_by_event() {
        let server = server();
        let mut stream = events(server.port(), "/v1/events", Duration::from_secs(5)).expect("a stream");
        let event = |event: &str, data: &str| Event { event: event.into(), data: data.into() };
        assert_eq!(stream.next_event().expect("reads"), Some(event("run", r#"{"id":"r1"}"#)));
        assert_eq!(stream.next_event().expect("reads"), Some(event("vm", r#"{"vm":0}"#)));
        assert_eq!(stream.next_event().expect("reads"), None);
    }

    #[test]
    fn a_silent_server_is_a_timeout_not_a_hang() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let port = listener.local_addr().expect("an address").port();
        let started = Instant::now();
        let err = send(port, "GET", "/", None, Duration::from_millis(200)).expect_err("nobody answers");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(listener);
    }
}
