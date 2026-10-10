//! The smallest HTTP/1.1 client the dispatcher needs.
//!
//! The dispatcher talks to two daemons, both on the loopback: the node
//! (`noded` — the ledger and the fleet register) and the hub (`vsockd`'s admin
//! surface — the way into an agent's session). Both serve the same hand-rolled
//! HTTP/1.1 the plaza's daemons do — one request per connection,
//! `Connection: close`, JSON in and out — so this is deliberately not a
//! general HTTP implementation: no keep-alive, no pipelining, no chunked
//! bodies, no redirects, no TLS. What it does have to do is bound every
//! request, refuse anything it cannot read, and never panic.
//!
//! The pattern is `crates/noded/src/hub.rs`'s (noded's own client for the
//! hub's status surface): connect to a *checked literal loopback address*, a
//! whole-exchange deadline, a bounded head and body, and one honest error
//! string for a daemon that is down, quiet or unreadable. `dispatchd` cannot
//! depend on `noded` any more than `vsockd` can, so the two stay in step by
//! hand.
//!
//! The loopback policy is exact, and enforced before a socket is opened: only
//! `http://127.0.0.1:PORT` and `http://localhost:PORT` are accepted — no other
//! name, address or spelling, and nothing is resolved through DNS.

use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant};

/// The most a status line and the headers may occupy, together.
pub const MAX_HEAD: usize = 16 * 1024;

/// The most a response body may occupy. The ledger's whole task list is a few
/// hundred KiB even when it is full (1024 tasks), and the hub's status is a
/// few KiB per agent.
pub const MAX_BODY: usize = 1024 * 1024;

/// How long the whole `/tell` exchange may take: the hub's own wait is at most
/// five seconds, so this leaves room for the answer to come back after it.
pub const TELL_TIMEOUT: Duration = Duration::from_secs(8);

/// How long any other exchange may take — connecting, sending, and reading the
/// answer. A loopback daemon answers the ledger and registry calls at once.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// Where a daemon is, once its URL has been checked to be the loopback.
///
/// The address is the literal `127.0.0.1`, so there is nothing left to resolve
/// when a socket is opened: a name that merely looks like the loopback cannot
/// redirect the dispatcher once it has been refused here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    addr: SocketAddr,
}

impl Target {
    /// Parses `http://127.0.0.1:PORT` or `http://localhost:PORT`.
    ///
    /// Exact matching only: the `http` scheme, `127.0.0.1` or `localhost`
    /// normalized to it, a nonzero port, and no path — `https://…`,
    /// `127.0.0.1.`, `LOCALHOST`, `127.0.0.1.evil.tld`, `10.0.0.1`, `[::1]`
    /// and `127.0.0.1:8899/status` are all refused, and the host is never
    /// resolved through DNS.
    pub fn parse(url: &str) -> Result<Self, String> {
        let Some(rest) = url.strip_prefix("http://") else {
            return Err(format!(
                "{url:?} is not an http:// URL: the dispatcher reaches the node and the hub over \
                 plain HTTP on the loopback"
            ));
        };
        if rest.contains('/') {
            return Err(format!(
                "{url:?} names a path: a target is `http://host:port` only"
            ));
        }
        let Some((host, port)) = rest.rsplit_once(':') else {
            return Err(format!("{url:?} names no port"));
        };
        let host = match host {
            "127.0.0.1" | "localhost" => Ipv4Addr::LOCALHOST,
            other => {
                return Err(format!(
                    "{other:?} is not a loopback host: {url:?} must be reached on 127.0.0.1 only"
                ));
            }
        };
        let port: u16 = port
            .parse()
            .map_err(|_| format!("{port:?} is not a port"))?;
        if port == 0 {
            return Err(format!(
                "{url:?} names port 0, which is not a port to reach"
            ));
        }
        Ok(Self {
            addr: SocketAddr::V4(SocketAddrV4::new(host, port)),
        })
    }

    /// The target's address, always the literal loopback.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl fmt::Display for Target {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "http://{}", self.addr)
    }
}

/// A daemon's answer: the status it answered with and its body.
#[derive(Debug)]
pub struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    /// The status the daemon answered with.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The answer's body.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// The body as text. Lossy on purpose: the dispatcher only ever reports a
    /// refusal's words, and a daemon's error is the daemon's to spell.
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// The refusal a non-2xx answer carries: the daemon's `{"error": "..."}`,
    /// or its body as it stands when the answer is not that shape.
    ///
    /// The words are bounded here, where the answer is read, so nothing
    /// downstream has to remember to bound them before they reach a log line
    /// or a task's note.
    pub fn refusal(&self) -> String {
        let message = serde_json::from_slice::<serde_json::Value>(&self.body)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| self.text().into_owned());
        crate::workflow::bounded(&message)
    }

    /// An answer with the given status and body. Production only ever reads an
    /// answer off a socket; the tests build one to pin the refusal's shape.
    #[cfg(test)]
    pub fn for_test(status: u16, body: &[u8]) -> Self {
        Self {
            status,
            body: body.to_vec(),
        }
    }
}

/// Sends one `GET {path}` and reads the whole answer.
pub fn get(target: &Target, path: &str, timeout: Duration) -> Result<Answer, String> {
    let head = format!("GET {path} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n");
    exchange(target, &head, &[], timeout)
}

/// Sends one `POST {path}` with `body` as its JSON payload, and reads the
/// whole answer.
pub fn post(target: &Target, path: &str, body: &[u8], timeout: Duration) -> Result<Answer, String> {
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {target}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    exchange(target, &head, body, timeout)
}

/// Connects to the target, sends the request, and reads the whole answer.
///
/// Every step is bounded by what is left of `timeout`, the head by
/// [`MAX_HEAD`] and the body by [`MAX_BODY`]; anything that is not a
/// well-formed HTTP/1.x answer is an error string, never a panic.
fn exchange(target: &Target, head: &str, body: &[u8], timeout: Duration) -> Result<Answer, String> {
    let deadline = Instant::now() + timeout;
    // The address is the checked literal loopback: no resolution happens here,
    // which is where the loopback policy is enforced for good.
    let mut stream = TcpStream::connect_timeout(&target.addr, remaining(deadline, timeout)?)
        .map_err(|err| format!("cannot connect to {target}: {err}"))?;
    // The backstop: a read that outlives the budget fails as a timeout even if
    // it is the first read and no deadline check has run yet.
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|err| format!("the connection to {target} could not be bounded: {err}"))?;
    stream
        .set_write_timeout(Some(remaining(deadline, timeout)?))
        .map_err(|err| format!("the connection to {target} could not be bounded: {err}"))?;
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(body))
        .and_then(|()| stream.flush())
        .map_err(|err| format!("cannot send the request to {target}: {err}"))?;
    let mut reader = BufReader::with_capacity(MAX_HEAD + 1, stream);
    read_response(&mut reader, deadline, timeout)
}

/// Reads one answer: the status line, the headers (bounded together by
/// [`MAX_HEAD`]) and the body (bounded by [`MAX_BODY`]).
///
/// Anything malformed — a status line that is not `HTTP/1.x CODE …`, a header
/// without a `:`, a chunked body, a head or body that is too large, a body
/// shorter than its `Content-Length`, a connection that ends mid-head — is an
/// error, never a panic.
fn read_response<R: Read>(
    reader: &mut BufReader<R>,
    deadline: Instant,
    timeout: Duration,
) -> Result<Answer, String> {
    let mut line = Vec::new();
    read_line(reader, deadline, timeout, MAX_HEAD, &mut line)?;
    let status_line = std::str::from_utf8(&line)
        .map_err(|_| "the answer's status line is not ASCII".to_string())?;
    let mut parts = status_line.split(' ');
    let Some(version) = parts.next() else {
        return Err("the daemon answered no status line".to_string());
    };
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(format!("the daemon answered {version:?}, not HTTP/1.x"));
    }
    let Some(code) = parts.next() else {
        return Err("the status line carries no status code".to_string());
    };
    if code.len() != 3 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("the status code {code:?} is not a status code"));
    }
    let status: u16 = code
        .parse()
        .map_err(|_| format!("the status code {code:?} is not a status code"))?;

    let mut head_bytes = line.len() + 2;
    let mut content_length = None;
    loop {
        read_line(reader, deadline, timeout, MAX_HEAD, &mut line)?;
        head_bytes += line.len() + 2;
        if head_bytes > MAX_HEAD {
            return Err("the answer's headers are too large".to_string());
        }
        if line.is_empty() {
            break;
        }
        let header = std::str::from_utf8(&line)
            .map_err(|_| "a header in the answer is not ASCII".to_string())?;
        let Some((name, value)) = header.split_once(':') else {
            return Err("a header in the answer has no `:`".to_string());
        };
        let value = value.trim_matches([' ', '\t']);
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err("the answer has more than one Content-Length".to_string());
            }
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("the answer's Content-Length is not a number".to_string());
            }
            let length = value
                .parse::<usize>()
                .map_err(|_| "the answer's Content-Length is too large".to_string())?;
            if length > MAX_BODY {
                return Err("the answer is too large".to_string());
            }
            content_length = Some(length);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            // The daemons never chunk; refusing keeps one answer from having
            // two readings of its own length.
            return Err("the answer is chunked".to_string());
        }
    }

    let mut body = Vec::new();
    remaining(deadline, timeout)?;
    match content_length {
        Some(length) => {
            (&mut *reader)
                .take(length as u64)
                .read_to_end(&mut body)
                .map_err(|err| read_error(err, timeout))?;
            if body.len() != length {
                return Err("the answer ended before its Content-Length".to_string());
            }
        }
        // No Content-Length: the answer is read to the close, still bounded.
        None => {
            (&mut *reader)
                .take(MAX_BODY as u64 + 1)
                .read_to_end(&mut body)
                .map_err(|err| read_error(err, timeout))?;
            if body.len() > MAX_BODY {
                return Err("the answer is too large".to_string());
            }
        }
    }
    Ok(Answer { status, body })
}

/// Reads one `\n`-terminated line, without its `\r\n` or `\n`, into `line`.
fn read_line<R: Read>(
    reader: &mut BufReader<R>,
    deadline: Instant,
    timeout: Duration,
    limit: usize,
    line: &mut Vec<u8>,
) -> Result<(), String> {
    line.clear();
    remaining(deadline, timeout)?;
    let read = (&mut *reader)
        .take(limit as u64 + 1)
        .read_until(b'\n', line)
        .map_err(|err| read_error(err, timeout))?;
    if read == 0 || line.last() != Some(&b'\n') {
        return Err("the answer ended before it was complete".to_string());
    }
    if line.len() > limit {
        return Err("the answer has an overlong line".to_string());
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(())
}

/// The time left before `deadline`, or an error once the budget is spent.
fn remaining(deadline: Instant, timeout: Duration) -> Result<Duration, String> {
    match deadline.checked_duration_since(Instant::now()) {
        Some(left) if !left.is_zero() => Ok(left),
        _ => Err(quiet(timeout)),
    }
}

/// Why a read of the answer failed. A read that ran out the socket's timeout
/// is reported as one — the kernel hands it back as `WouldBlock` — so a quiet
/// daemon is named as quiet rather than blamed for a malformed answer.
fn read_error(err: io::Error, timeout: Duration) -> String {
    match err.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => quiet(timeout),
        _ => format!("the answer could not be read: {err}"),
    }
}

/// What a daemon that went silent is reported as.
fn quiet(timeout: Duration) -> String {
    format!("the daemon did not answer within {}s", timeout.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Cursor;
    use std::net::TcpListener;
    use std::thread;

    /// A deadline far enough out that the budget is never the reason a read
    /// fails: these tests are about the parsing, not the clock.
    fn roomy() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    /// A target is the loopback and only the loopback, spelled exactly: the
    /// policy is enforced before any socket is opened, so nothing is resolved
    /// and no lookalike can reach a name resolver.
    #[test]
    fn a_target_is_the_loopback_and_only_the_loopback() {
        for good in [
            "http://127.0.0.1:8899",
            "http://localhost:19080",
            "http://127.0.0.1:1",
        ] {
            let target = Target::parse(good).expect("a loopback URL");
            assert_eq!(target.addr().ip(), Ipv4Addr::LOCALHOST);
            assert_eq!(target.to_string(), format!("http://{}", target.addr()));
        }
        for bad in [
            "127.0.0.1:8899",
            "https://127.0.0.1:8899",
            "http://127.0.0.1",
            "http://127.0.0.1:0",
            "http://127.0.0.1:8899/status",
            "http://127.0.0.1:8899/",
            "http://127.0.0.1.:8899",
            "http://LOCALHOST:8899",
            "http://localhost.evil.tld:8899",
            "http://127.0.0.1.evil.tld:8899",
            "http://10.0.0.1:8899",
            "http://[::1]:8899",
            "http://user@127.0.0.1:8899",
            "http://127.0.0.1:port",
            "http://:8899",
            "",
        ] {
            assert!(
                Target::parse(bad).is_err(),
                "should have been refused: {bad:?}"
            );
        }
    }

    /// A well-formed answer is read whole: status, headers, and a body taken
    /// as its `Content-Length` says (or read to the close when it does not
    /// say).
    #[test]
    fn an_answer_is_read_as_its_headers_say() {
        let raw = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}";
        let answer = read_response(
            &mut BufReader::new(Cursor::new(raw.as_bytes())),
            roomy(),
            TIMEOUT,
        )
        .expect("a well-formed answer");
        assert_eq!(answer.status(), 200);
        assert_eq!(answer.body(), b"{\"ok\":true}");
        assert_eq!(answer.text(), "{\"ok\":true}");

        let raw = "HTTP/1.0 409 Conflict\r\n\r\n{\"error\":\"no\"}";
        let answer = read_response(
            &mut BufReader::new(Cursor::new(raw.as_bytes())),
            roomy(),
            TIMEOUT,
        )
        .expect("a bodyless-length answer reads to the close");
        assert_eq!(answer.status(), 409);
        assert_eq!(answer.refusal(), "no");
    }

    /// A malformed answer is an error, never a panic: every shape the parser
    /// refuses is refused here.
    #[test]
    fn a_malformed_answer_is_refused() {
        for bad in [
            "",
            "HTTP/1.1\r\n\r\n",
            "HTTP/2 200 OK\r\n\r\n",
            "HTTP/1.1 20 OK\r\n\r\n",
            "HTTP/1.1 2xx OK\r\n\r\n",
            "HTTP/1.1 200 OK\r\nno-colon\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhi",
            "HTTP/1.1 200 OK\r\nContent-Length: x\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        ] {
            let read = read_response(
                &mut BufReader::new(Cursor::new(bad.as_bytes())),
                roomy(),
                TIMEOUT,
            );
            assert!(read.is_err(), "should have been refused: {bad:?}");
        }

        // A refusal that is not the daemon's error shape still says something.
        let answer = Answer {
            status: 500,
            body: b"boom".to_vec(),
        };
        assert_eq!(answer.refusal(), "boom");

        // And a refusal's words are bounded, whatever the daemon sent.
        let answer = Answer {
            status: 400,
            body: format!(r#"{{"error":"{}"}}"#, "x".repeat(10_000)).into_bytes(),
        };
        assert!(
            answer.refusal().chars().count() <= crate::workflow::MAX_NOTE_DETAIL + 1,
            "the refusal is bounded: {} chars",
            answer.refusal().chars().count()
        );
    }

    /// The socket path end to end: a real loopback connection carries a real
    /// request and its answer comes back — and a daemon that is not there is
    /// an honest connection error.
    #[test]
    fn a_real_request_and_answer_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the test binds a port");
        let addr = listener.local_addr().expect("a bound address");
        let target = Target { addr };
        let served = thread::spawn(move || {
            let (mut stream, _peer) = listener.accept().expect("the client connects");
            let request = request_bytes(&mut stream);
            let body = r#"{"tasks":[]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("the answer is written");
            request
        });

        let answer = get(&target, "/api/v0/tasks", TIMEOUT).expect("the answer is read");
        assert_eq!(answer.status(), 200);
        assert_eq!(answer.body(), br#"{"tasks":[]}"#);
        let request =
            String::from_utf8(served.join().expect("the server finishes")).expect("ASCII");
        assert!(
            request.starts_with("GET /api/v0/tasks HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(request.contains("Connection: close\r\n"), "{request}");
    }

    /// Reads one whole request off a test's socket: the head, then exactly the
    /// body its `Content-Length` declares, however the segments fell.
    fn request_bytes(stream: &mut TcpStream) -> Vec<u8> {
        use std::io::Read;
        let mut buffer: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let read = stream.read(&mut chunk).expect("the request is read");
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
            if String::from_utf8_lossy(&buffer).contains("\r\n\r\n") {
                break;
            }
        }
        buffer
    }

    /// A target nothing listens on is a connection error naming the target,
    /// not a hang and not a panic.
    #[test]
    fn a_daemon_that_is_not_there_is_an_honest_error() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port is free for a moment");
        let addr = listener.local_addr().expect("a bound address");
        drop(listener);
        let target = Target { addr };
        let err =
            get(&target, "/status", Duration::from_millis(500)).expect_err("nothing is listening");
        assert!(err.contains("cannot connect"), "{err}");
    }
}
