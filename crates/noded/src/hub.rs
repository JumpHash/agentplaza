//! The hub's status surface, reached over the loopback so the console can read
//! it through this daemon.
//!
//! The console is served by this daemon under a `connect-src 'self'` policy,
//! so a cross-origin fetch of `vsockd`'s status surface (its `--status-port`,
//! 19080 on the live stack) is blocked before it is made.
//! `noded --hub-status <host:port>` names that surface and this module is the
//! daemon's one path to it, for two read-only endpoints:
//!
//! * `GET /api/v0/hub`         - the hub's `/status`, byte for byte
//! * `GET /api/v0/hub/healthz` - the hub's `/healthz`
//!
//! The surface is loopback-only, and so is this client: exactly `127.0.0.1` or
//! `localhost` is accepted ([`HubStatus::parse`]), and the address it holds is
//! the literal loopback - no name is ever resolved through DNS here - so
//! everything else is refused before a socket is opened. A hub that cannot be
//! reached, or that answers something this client cannot read, is an error the
//! caller reports as a 502 rather than an empty registry: the console must be
//! able to tell a down hub from a quiet one.
//!
//! Like [`crate::http`] on the serving side, this is the smallest HTTP/1.1 the
//! job needs - one `GET`, `Connection: close`, a bounded head and body, no
//! keep-alive - and the whole exchange is bounded by [`TIMEOUT`]: each read
//! consults what is left of the budget and the socket carries that budget as
//! its read timeout, so a hub that accepts a connection and then goes silent
//! cannot hold a daemon thread.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant};

/// The hub's status surface unless `--hub-status` says otherwise, as
/// `--hub-status` spells it.
pub const DEFAULT_HUB_STATUS: &str = "127.0.0.1:19080";

/// The port of [`DEFAULT_HUB_STATUS`]: the live stack's `--status-port`.
pub const DEFAULT_HUB_STATUS_PORT: u16 = 19080;

/// How long the whole exchange may take - connecting, sending, and reading the
/// answer. What is left of it is the socket's read timeout before every read,
/// so a hub that has gone quiet fails the read instead of holding the thread.
const TIMEOUT: Duration = Duration::from_secs(5);

/// The most a status line and the headers may occupy, together.
const MAX_HEAD: usize = 16 * 1024;

/// The most a response body may occupy. The frozen `/status` JSON is a few KiB
/// even with a full fleet; `/healthz` is two bytes.
const MAX_BODY: usize = 256 * 1024;

/// Where the hub's status surface is, once its address has been checked to be
/// the loopback.
///
/// The address is the literal `127.0.0.1`, so there is nothing left to resolve
/// when a socket is opened: a name that merely looks like the loopback cannot
/// redirect the daemon once it has been refused here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubStatus {
    addr: SocketAddr,
}

impl HubStatus {
    /// Parses `127.0.0.1:PORT` or `localhost:PORT`.
    ///
    /// Exact matching only: `127.0.0.1`, or `localhost` normalized to it, and
    /// a nonzero port. No other name, address, spelling or lookalike is
    /// accepted - `127.0.0.1.`, `LOCALHOST`, `127.0.0.1.evil.tld`, `10.0.0.1`
    /// and `[::1]` are all refused - and the host is never resolved through
    /// DNS.
    pub fn parse(value: &str) -> Result<Self, String> {
        let Some((host, port)) = value.rsplit_once(':') else {
            return Err(format!(
                "{value:?} names no port: the hub's status surface is `host:port`"
            ));
        };
        let host = match host {
            "127.0.0.1" | "localhost" => Ipv4Addr::LOCALHOST,
            other => {
                return Err(format!(
                    "{other:?} is not a loopback host: the hub's status surface is reached on \
                     127.0.0.1 only"
                ));
            }
        };
        let port: u16 = port
            .parse()
            .map_err(|_| format!("{port:?} is not a port"))?;
        if port == 0 {
            return Err("port 0 is not a port to reach the hub's status surface on".to_string());
        }
        Ok(Self {
            addr: SocketAddr::V4(SocketAddrV4::new(host, port)),
        })
    }

    /// The endpoint's address, always the literal loopback.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Default for HubStatus {
    fn default() -> Self {
        Self {
            addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, DEFAULT_HUB_STATUS_PORT)),
        }
    }
}

impl std::fmt::Display for HubStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.addr)
    }
}

/// The hub's answer: the status it answered with, the content type it declared
/// and its body.
#[derive(Debug)]
pub struct Answer {
    status: u16,
    content_type: String,
    body: Vec<u8>,
}

impl Answer {
    /// The status the hub answered with.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The answer in three parts: the status, the content type and the body,
    /// taken whole so the proxy passes the body through without a copy.
    pub fn into_parts(self) -> (u16, String, Vec<u8>) {
        (self.status, self.content_type, self.body)
    }
}

/// Sends one `GET {path}` to the hub and reads its whole answer.
///
/// Every step is bounded by what is left of [`TIMEOUT`], the head by
/// [`MAX_HEAD`] and the body by [`MAX_BODY`]; anything the hub answers that is
/// not a well-formed HTTP/1.x response is an error string, never a panic.
pub fn get(hub: &HubStatus, path: &str) -> Result<Answer, String> {
    let deadline = Instant::now() + TIMEOUT;
    // The address is the checked literal loopback: no resolution happens here,
    // which is where the loopback policy is enforced for good.
    let mut stream = TcpStream::connect_timeout(&hub.addr, remaining(deadline)?)
        .map_err(|err| format!("cannot connect to {hub}: {err}"))?;
    // The backstop: a read that outlives the budget fails as a timeout even if
    // it is the first read and no deadline check has run yet.
    stream
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|err| format!("the hub connection could not be bounded: {err}"))?;
    stream
        .set_write_timeout(Some(remaining(deadline)?))
        .map_err(|err| format!("the hub connection could not be bounded: {err}"))?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {hub}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .and_then(|()| stream.flush())
        .map_err(|err| format!("cannot send the request: {err}"))?;
    let mut reader = BufReader::with_capacity(MAX_HEAD + 1, stream);
    read_response(&mut reader, deadline)
}

/// Reads one response: the status line, the headers (bounded together by
/// [`MAX_HEAD`]) and the body (bounded by [`MAX_BODY`]).
///
/// Anything malformed - a status line that is not `HTTP/1.x CODE ...`, a
/// header without a `:`, a chunked body, a head or body that is too large, a
/// body shorter than its `Content-Length`, a connection that ends mid-head -
/// is an error, never a panic.
fn read_response<R: Read>(reader: &mut BufReader<R>, deadline: Instant) -> Result<Answer, String> {
    let mut line = Vec::new();
    read_line(reader, deadline, MAX_HEAD, &mut line)?;
    let status_line =
        std::str::from_utf8(&line).map_err(|_| "the hub's status line is not ASCII".to_string())?;
    let mut parts = status_line.split(' ');
    let Some(version) = parts.next() else {
        return Err("the hub answered no status line".to_string());
    };
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(format!("the hub answered {version:?}, not HTTP/1.x"));
    }
    let Some(code) = parts.next() else {
        return Err("the hub's status line carries no status code".to_string());
    };
    if code.len() != 3 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!(
            "the hub's status code {code:?} is not a status code"
        ));
    }
    let status: u16 = code
        .parse()
        .map_err(|_| format!("the hub's status code {code:?} is not a status code"))?;

    let mut head_bytes = line.len() + 2;
    let mut content_length = None;
    let mut content_type = None;
    loop {
        read_line(reader, deadline, MAX_HEAD, &mut line)?;
        head_bytes += line.len() + 2;
        if head_bytes > MAX_HEAD {
            return Err("the hub's answer headers are too large".to_string());
        }
        if line.is_empty() {
            break;
        }
        let header = std::str::from_utf8(&line)
            .map_err(|_| "a header in the hub's answer is not ASCII".to_string())?;
        let Some((name, value)) = header.split_once(':') else {
            return Err("a header in the hub's answer has no `:`".to_string());
        };
        let value = value.trim_matches([' ', '\t']);
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err("the hub's answer has more than one Content-Length".to_string());
            }
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("the hub's Content-Length is not a number".to_string());
            }
            let length = value
                .parse::<usize>()
                .map_err(|_| "the hub's Content-Length is too large".to_string())?;
            if length > MAX_BODY {
                return Err("the hub's answer is too large".to_string());
            }
            content_length = Some(length);
        } else if name.eq_ignore_ascii_case("content-type") {
            content_type = Some(value.to_string());
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            // The status surface never chunks; refusing keeps one answer from
            // having two readings of its own length.
            return Err("the hub's answer is chunked".to_string());
        }
    }

    let mut body = Vec::new();
    remaining(deadline)?;
    match content_length {
        Some(length) => {
            (&mut *reader)
                .take(length as u64)
                .read_to_end(&mut body)
                .map_err(read_error)?;
            if body.len() != length {
                return Err("the hub's answer ended before its Content-Length".to_string());
            }
        }
        // No Content-Length: the answer is read to the close, still bounded.
        None => {
            (&mut *reader)
                .take(MAX_BODY as u64 + 1)
                .read_to_end(&mut body)
                .map_err(read_error)?;
            if body.len() > MAX_BODY {
                return Err("the hub's answer is too large".to_string());
            }
        }
    }
    Ok(Answer {
        status,
        content_type: content_type.map_or_else(untyped_content_type, |value| sanitize_content_type(&value)),
        body,
    })
}

/// The content type of an answer that declared none.
fn untyped_content_type() -> String {
    "application/octet-stream".to_string()
}

/// A content type this daemon will write into a response header: a nonempty
/// run of printable ASCII, which by construction carries no `\r`, `\n` or
/// other control byte. The hub is a loopback peer, but it is still another
/// process, so its header value is checked before it is echoed rather than
/// trusted to be well formed.
fn sanitize_content_type(value: &str) -> String {
    if !value.is_empty() && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
        value.to_string()
    } else {
        untyped_content_type()
    }
}

/// Reads one `\n`-terminated line, without its `\r\n` or `\n`, into `line`.
///
/// A line longer than `limit`, an answer that ends before the newline, or a
/// read the remaining budget cuts short, is an error: the answer cannot be
/// read as sent.
fn read_line<R: Read>(
    reader: &mut BufReader<R>,
    deadline: Instant,
    limit: usize,
    line: &mut Vec<u8>,
) -> Result<(), String> {
    line.clear();
    remaining(deadline)?;
    let read = (&mut *reader)
        .take(limit as u64 + 1)
        .read_until(b'\n', line)
        .map_err(read_error)?;
    if read == 0 || line.last() != Some(&b'\n') {
        return Err("the hub's answer ended before it was complete".to_string());
    }
    if line.len() > limit {
        return Err("the hub's answer has an overlong line".to_string());
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(())
}

/// The time left before `deadline`, or an error once the budget is spent.
fn remaining(deadline: Instant) -> Result<Duration, String> {
    match deadline.checked_duration_since(Instant::now()) {
        Some(left) if !left.is_zero() => Ok(left),
        _ => Err(format!("the hub did not answer within {}s", TIMEOUT.as_secs())),
    }
}

/// Why a read of the hub's answer failed. A read that ran out the socket's
/// timeout is reported as one - the kernel hands it back as `WouldBlock` - so
/// a 502 can say the hub went quiet rather than blaming a malformed answer.
fn read_error(err: io::Error) -> String {
    match err.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            format!("the hub did not answer within {}s", TIMEOUT.as_secs())
        }
        _ => format!("the hub's answer could not be read: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Cursor;
    use std::time::Duration;

    /// A deadline far enough out that the budget is never the reason a read
    /// fails: these tests are about the parsing, not the clock.
    fn roomy() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    fn read(raw: &[u8]) -> Result<Answer, String> {
        read_response(&mut BufReader::new(Cursor::new(raw.to_vec())), roomy())
    }

    /// Only the literal loopback is a hub: every lookalike, every other
    /// address, and every spelling of the loopback that is not exactly right
    /// is refused before a socket could be opened.
    #[test]
    fn only_the_loopback_is_a_hub() {
        for value in [
            "127.0.0.1:19080",
            "localhost:19080",
            "127.0.0.1:1",
            "localhost:65535",
        ] {
            let hub = HubStatus::parse(value).unwrap_or_else(|err| panic!("{value}: {err}"));
            assert_eq!(
                hub.addr(),
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, hub.addr().port())),
                "{value}"
            );
        }
        assert_eq!(
            HubStatus::parse("localhost:19080")
                .expect("localhost is loopback")
                .addr(),
            HubStatus::parse("127.0.0.1:19080")
                .expect("the loopback")
                .addr(),
        );
    }

    /// The negatives: a lookalike name, the loopback with a trailing dot, a
    /// case variant, another address, another family, a bare host, an empty
    /// port, port 0, and a URL - none of them is the hub, and none of them is
    /// ever resolved.
    #[test]
    fn a_lookalike_is_not_the_hub() {
        for value in [
            "",
            "127.0.0.1",
            "localhost",
            "127.0.0.1:",
            "127.0.0.1:0",
            "127.0.0.1.",
            "127.0.0.1.:19080",
            "127.0.0.1.evil.tld:19080",
            "localhost.evil.tld:19080",
            "api.127.0.0.1:19080",
            "LOCALHOST:19080",
            "Localhost:19080",
            "127.0.0.1:19080 ",
            "127.0.0.1 :19080",
            "10.0.0.1:19080",
            "192.168.1.10:19080",
            "0.0.0.0:19080",
            "[::1]:19080",
            "::1:19080",
            "example.com:19080",
            "http://127.0.0.1:19080",
            "http://localhost:19080/status",
            "127.0.0.1:19080/status",
            "127.0.0.1:19080?q=1",
            "127.0.0.1:notaport",
            "127.0.0.1:70000",
            "127.0.0.1:-1",
            "off",
        ] {
            assert!(
                HubStatus::parse(value).is_err(),
                "{value:?} must not be a hub"
            );
        }
    }

    /// The default is the surface the help text and the docs name, so the two
    /// spellings of it cannot drift.
    #[test]
    fn the_default_is_the_documented_surface() {
        assert_eq!(
            HubStatus::default(),
            HubStatus::parse(DEFAULT_HUB_STATUS).expect("the documented default")
        );
        assert_eq!(HubStatus::default().addr().port(), DEFAULT_HUB_STATUS_PORT);
        assert_eq!(
            HubStatus::default().addr(),
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 19080))
        );
    }

    /// A well-formed answer is read whole: its status, its content type, and
    /// its body exactly as it was sent - the bytes the proxy passes through.
    #[test]
    fn a_body_is_read_whole() {
        let body = r#"{"node":{"cids":[10,11],"port":5000,"uptime_s":1,"max_sessions":64,"keepalive_idle_s":300},"agents":[],"sessions_active":0}"#;
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let answer = read(raw.as_bytes()).expect("a well-formed answer");
        let (status, content_type, read) = answer.into_parts();
        assert_eq!(status, 200);
        assert_eq!(content_type, "application/json; charset=utf-8");
        assert_eq!(read, body.as_bytes());
    }

    /// A body with no `Content-Length` is read to the close, and a 502 from
    /// the hub comes back with its own status: an error the hub sent is not
    /// this client's parse error.
    #[test]
    fn a_closed_body_and_a_non_200_are_read() {
        let answer = read(b"HTTP/1.0 200 OK\r\n\r\nok").expect("a body read to the close");
        assert_eq!(answer.status(), 200);
        assert_eq!(answer.into_parts().2, b"ok");

        let answer = read(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 2\r\n\r\nno")
            .expect("the hub's own 502");
        assert_eq!(answer.status(), 502);
        assert_eq!(answer.into_parts().2, b"no");
    }

    /// A content type that is not a plain printable value is not echoed into a
    /// response header: the proxy falls back to bytes.
    #[test]
    fn a_content_type_is_sanitized() {
        assert_eq!(
            sanitize_content_type("application/json; charset=utf-8"),
            "application/json; charset=utf-8"
        );
        assert_eq!(sanitize_content_type(""), "application/octet-stream");
        assert_eq!(
            sanitize_content_type("text/plain\u{7}"),
            "application/octet-stream"
        );
        assert_eq!(untyped_content_type(), "application/octet-stream");
    }

    /// Anything that is not a well-formed HTTP/1.x answer is refused: a
    /// non-HTTP line, an HTTP/2 version, a missing or non-numeric code, a
    /// header without a `:`, a chunked body, a body shorter than its
    /// Content-Length, and an answer that ends mid-head.
    #[test]
    fn a_malformed_answer_is_refused() {
        let bad: [&[u8]; 10] = [
            b"",
            b"not http at all\r\n\r\n",
            b"HTTP/2 200 OK\r\n\r\n",
            b"HTTP/1.1\r\n\r\n",
            b"HTTP/1.1 OK OK\r\n\r\n",
            b"HTTP/1.1 20 OK\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nno-colon\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabc",
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n",
        ];
        for raw in bad {
            assert!(read(raw).is_err(), "should have been refused: {raw:?}");
        }
    }

    /// A head or a body larger than the client will read is refused, so a hub
    /// that answers with a flood fails instead of filling memory.
    #[test]
    fn an_oversized_answer_is_refused() {
        let one_long_line = format!(
            "HTTP/1.1 200 OK\r\nX-Pad: {}\r\n\r\n",
            "a".repeat(MAX_HEAD)
        );
        assert!(read(one_long_line.as_bytes()).is_err());

        let many_lines = format!(
            "HTTP/1.1 200 OK\r\n{}\r\n",
            "X: 1\r\n".repeat(MAX_HEAD / 5)
        );
        assert!(read(many_lines.as_bytes()).is_err());

        let declared = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert!(read(declared.as_bytes()).is_err());
    }

    /// A spent budget is the reason a read fails, and a timeout is reported as
    /// one rather than as a malformed answer.
    #[test]
    fn a_spent_budget_is_a_timeout() {
        let error = read_response(
            &mut BufReader::new(Cursor::new(b"HTTP/1.1 200 OK\r\n".to_vec())),
            Instant::now(),
        )
        .expect_err("a deadline in the past");
        assert!(error.contains("did not answer within"), "{error}");

        assert_eq!(
            read_error(io::Error::from(io::ErrorKind::WouldBlock)),
            "the hub did not answer within 5s"
        );
        assert!(
            read_error(io::Error::from(io::ErrorKind::Other)).contains("could not be read"),
            "another error keeps its own reason"
        );
    }
}
