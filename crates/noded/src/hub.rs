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
//! `noded --hub-admin <host:port>` names the hub's *admin* surface (its
//! `--admin-port`, 19081 on the live stack) - the one loopback endpoint that
//! drives a connected agent - and this module is also the daemon's one path to
//! it. [`tell`] sends the `POST /tell` that two endpoints need:
//! `POST /api/v0/tasks/{id}/artifact`, where the daemon reads a file a task's
//! agent wrote by driving that agent's `read_file`, and
//! `POST /api/v0/agents/{cid}/tell`, where the operator's console drives a tool
//! of its own choosing on an agent (both in [`crate`]). One client, one
//! translation of the hub's answer, for both.
//!
//! Both surfaces are loopback-only, and so is this client: exactly `127.0.0.1`
//! or `localhost` is accepted ([`HubStatus::parse`], [`HubAdmin::parse`], one
//! shared [`parse_loopback`]), and the address it holds is the literal
//! loopback - no name is ever resolved through DNS here - so everything else
//! is refused before a socket is opened. A hub that cannot be reached, or that
//! answers something this client cannot read, is an error the caller reports
//! as a 502 rather than an empty registry: the console must be able to tell a
//! down hub from a quiet one.
//!
//! Like [`crate::http`] on the serving side, this is the smallest HTTP/1.1 the
//! job needs - one request, `Connection: close`, a bounded head and body, no
//! keep-alive - and the whole exchange is bounded by its budget ([`TIMEOUT`]
//! for the status surface, [`ADMIN_TIMEOUT`] for the admin one): each read
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

/// The hub's admin surface unless `--hub-admin` says otherwise, as
/// `--hub-admin` spells it.
pub const DEFAULT_HUB_ADMIN: &str = "127.0.0.1:19081";

/// The port of [`DEFAULT_HUB_ADMIN`]: the live stack's `--admin-port`.
pub const DEFAULT_HUB_ADMIN_PORT: u16 = 19081;

/// How long the whole status exchange may take - connecting, sending, and
/// reading the answer. What is left of it is the socket's read timeout before
/// every read, so a hub that has gone quiet fails the read instead of holding
/// the thread.
const TIMEOUT: Duration = Duration::from_secs(5);

/// How long a whole `/tell` may take. The hub waits up to [`TELL_TIMEOUT_MS`]
/// for the agent's answer before it writes its own 504, and the hub's admin
/// surface bounds one connection at 10s, so this client's budget is that same
/// 10s: long enough for the hub to give up on its own terms, short enough that
/// a hub which accepts the connection and then says nothing cannot hold a
/// daemon thread.
const ADMIN_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest a `/tell` may wait for its agent: the hub's own maximum
/// (`vsockd`'s `MAX_TELL_TIMEOUT_MS`), so a caller never cuts a call off
/// before the hub itself would have given up.
pub const TELL_TIMEOUT_MS: u64 = 5_000;

/// The tool the read-through drives on the agent: the harness's `read_file`.
pub const READ_FILE: &str = "read_file";

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
        Ok(Self {
            addr: parse_loopback(value, "the hub's status surface")?,
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

/// Where the hub's admin surface (`vsockd --admin-port`) is, once its address
/// has been checked to be the loopback.
///
/// The surface that can make an agent act is reached on the loopback only, and
/// this type holds the literal `127.0.0.1` exactly as [`HubStatus`] does: there
/// is nothing left to resolve when a socket is opened, and a name that merely
/// looks like the loopback cannot redirect the daemon once it has been refused
/// by [`parse_loopback`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubAdmin {
    addr: SocketAddr,
}

impl HubAdmin {
    /// Parses `127.0.0.1:PORT` or `localhost:PORT`, by the same exact-matcher
    /// as [`HubStatus::parse`]: the admin surface is one more loopback surface,
    /// and its lookalikes (`127.0.0.1.`, `LOCALHOST`, `127.0.0.1.evil.tld`,
    /// `10.0.0.1`, `[::1]`) are refused the same way, before a socket is
    /// opened.
    pub fn parse(value: &str) -> Result<Self, String> {
        Ok(Self {
            addr: parse_loopback(value, "the hub's admin surface")?,
        })
    }

    /// The endpoint's address, always the literal loopback.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Default for HubAdmin {
    fn default() -> Self {
        Self {
            addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, DEFAULT_HUB_ADMIN_PORT)),
        }
    }
}

impl std::fmt::Display for HubAdmin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.addr)
    }
}

/// Parses `127.0.0.1:PORT` or `localhost:PORT` for the loopback surface named
/// by `what` (its error messages name the surface the caller asked for).
///
/// The one place the loopback policy lives, shared by [`HubStatus::parse`] and
/// [`HubAdmin::parse`]: exact equality on `127.0.0.1` or `localhost`
/// (normalized to the literal `127.0.0.1`) plus a nonzero port. The parsed
/// address is a literal [`SocketAddrV4`], so no name is ever resolved: a
/// lookalike is refused here, before a socket is opened.
fn parse_loopback(value: &str, what: &str) -> Result<SocketAddr, String> {
    let Some((host, port)) = value.rsplit_once(':') else {
        return Err(format!(
            "{value:?} names no port: {what} is `host:port`"
        ));
    };
    let host = match host {
        "127.0.0.1" | "localhost" => Ipv4Addr::LOCALHOST,
        other => {
            return Err(format!(
                "{other:?} is not a loopback host: {what} is reached on 127.0.0.1 only"
            ));
        }
    };
    let port: u16 = port
        .parse()
        .map_err(|_| format!("{port:?} is not a port"))?;
    if port == 0 {
        return Err(format!("port 0 is not a port to reach {what} on"));
    }
    Ok(SocketAddr::V4(SocketAddrV4::new(host, port)))
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

    /// The answer's body as text, for the JSON the admin surface answers with.
    /// A body that is not UTF-8 is an error naming that, never a lossy read.
    pub fn body_text(&self) -> Result<&str, String> {
        std::str::from_utf8(&self.body).map_err(|err| format!("the hub's answer is not UTF-8: {err}"))
    }
}

/// Sends one `GET {path}` to the hub's status surface and reads its whole
/// answer.
///
/// Every step is bounded by what is left of [`TIMEOUT`], the head by
/// [`MAX_HEAD`] and the body by [`MAX_BODY`]; anything the hub answers that is
/// not a well-formed HTTP/1.x response is an error string, never a panic.
pub fn get(hub: &HubStatus, path: &str) -> Result<Answer, String> {
    exchange(hub.addr(), TIMEOUT, "GET", path, None)
}

/// Drives one tool call on the agent connected as `cid`, through the hub's
/// admin surface: `POST /tell {"cid","name","args","timeout_ms"}`.
///
/// The answer is the hub's own: a 200 carries the agent's `ok` and `output`,
/// and any other status carries the hub's `{"error": "..."}` - a role policy
/// refusal, an unserved or unconnected CID, a timeout - which the caller
/// surfaces unchanged. A hub that cannot be reached, or that answers something
/// this client cannot read, is [`TellOutcome::Unreachable`], which the caller
/// reports as a 502: a transport failure is never dressed up as the agent's
/// word.
pub fn tell(
    admin: &HubAdmin,
    cid: u64,
    name: &str,
    args: serde_json::Value,
    timeout_ms: u64,
) -> TellOutcome {
    let body = serde_json::json!({
        "cid": cid,
        "name": name,
        "args": args,
        "timeout_ms": timeout_ms,
    })
    .to_string();
    let answer = match exchange(
        admin.addr(),
        ADMIN_TIMEOUT,
        "POST",
        "/tell",
        Some(body.as_bytes()),
    ) {
        Ok(answer) => answer,
        Err(reason) => return TellOutcome::Unreachable(reason),
    };
    let status = answer.status();
    match answer.body_text() {
        Ok(text) => tell_outcome(status, text),
        Err(reason) => TellOutcome::Unreachable(reason),
    }
}

/// Stops the agent connected as `cid`, through the hub's admin surface:
/// `POST /stop {"cid","reason"?}`.
///
/// The hub ends the session, waits up to its own bound for the agent to say
/// `bye`, and answers with its own stop view either way: `{"cid","stopped":
/// true,"calls":K}` or `{"cid","stopped":false,"reason":"..."}` - a session it
/// could not end is a fact to report, not an error. That view passes through
/// verbatim (the console reads `stopped` itself); any other status carries the
/// hub's `{"error": "..."}` - an unserved or unconnected CID - which the caller
/// surfaces unchanged. A hub that cannot be reached, or that answers something
/// this client cannot read, is [`StopOutcome::Unreachable`], reported as a 502:
/// a transport failure is never dressed up as the hub's word.
pub fn stop(admin: &HubAdmin, cid: u64, reason: Option<&str>) -> StopOutcome {
    let mut body = serde_json::json!({ "cid": cid });
    if let Some(reason) = reason {
        body["reason"] = serde_json::Value::String(reason.to_string());
    }
    let body = body.to_string();
    let answer = match exchange(
        admin.addr(),
        ADMIN_TIMEOUT,
        "POST",
        "/stop",
        Some(body.as_bytes()),
    ) {
        Ok(answer) => answer,
        Err(reason) => return StopOutcome::Unreachable(reason),
    };
    let status = answer.status();
    match answer.body_text() {
        Ok(text) => stop_outcome(cid, status, text),
        Err(reason) => StopOutcome::Unreachable(reason),
    }
}

/// The hub's answer to a `/stop`, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    /// The hub answered 200 with its own stop view: `stopped: true` and the
    /// session's call count, or `stopped: false` and why it stands. Passed
    /// through verbatim - the console reads `stopped` from the hub's words.
    Answered(String),
    /// The hub refused, in its own words and with its own status: the admin
    /// surface's `{"error": "..."}` for an unserved or unconnected CID. The
    /// caller surfaces both unchanged.
    Refused {
        /// The status the hub answered with.
        status: u16,
        /// The hub's own message.
        message: String,
    },
    /// The admin surface could not be reached, or answered something this
    /// client cannot read. The caller reports it as a 502.
    Unreachable(String),
}

/// The `/stop` answer's envelope, as the admin surface writes it. Only the
/// identity is read: `stopped` is part of the view the caller passes through
/// verbatim, not something this client reinterprets.
#[derive(Debug, serde::Deserialize)]
struct StopAnswer {
    cid: u64,
    #[allow(dead_code)]
    stopped: bool,
}

/// Reads one `/stop` answer's body: a 200 is the hub's own `{"cid","stopped",
/// …}` view, and any other status is the hub's `{"error":"..."}`. An answer for
/// another CID, or a body that is not the envelope it claims to be, is not
/// guessed at.
fn stop_outcome(cid: u64, status: u16, body: &str) -> StopOutcome {
    if status == 200 {
        return match serde_json::from_str::<StopAnswer>(body) {
            Ok(StopAnswer { cid: answered, .. }) if answered == cid => {
                StopOutcome::Answered(body.trim().to_string())
            }
            Ok(StopAnswer { cid: answered, .. }) => StopOutcome::Unreachable(format!(
                "the hub's /stop answered for guest CID {answered} while {cid} was asked"
            )),
            Err(err) => StopOutcome::Unreachable(format!(
                "the hub's /stop answer is not the admin surface's JSON: {err}"
            )),
        };
    }
    StopOutcome::Refused {
        status,
        message: error_message(body),
    }
}

/// The agent's answer to a `/tell`, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TellOutcome {
    /// The agent answered: the hub's 200 with the agent's own `ok` and
    /// `output`. `ok: false` is still an answer - the agent refused the call -
    /// and its words are data, not a transport failure.
    Answered {
        /// The agent's own `ok`.
        ok: bool,
        /// The agent's own `output` (its answer, or its refusal's reason).
        output: String,
    },
    /// The hub refused, in its own words and with its own status: the admin
    /// surface's `{"error": "..."}` for a role policy, an unserved or
    /// unconnected CID, or a timeout. The caller surfaces both unchanged.
    Refused {
        /// The status the hub answered with.
        status: u16,
        /// The hub's own message.
        message: String,
    },
    /// The admin surface could not be reached, or answered something this
    /// client cannot read. The caller reports it as a 502.
    Unreachable(String),
}

/// The `/tell` answer's envelope, as the admin surface writes it.
#[derive(Debug, serde::Deserialize)]
struct TellAnswer {
    ok: bool,
    output: String,
}

/// Reads one `/tell` answer's body: a 200 is the agent's `{"ok","output"}`, and
/// any other status is the hub's `{"error":"..."}`. A body that is not the
/// envelope it claims to be is not guessed at.
fn tell_outcome(status: u16, body: &str) -> TellOutcome {
    if status == 200 {
        return match serde_json::from_str::<TellAnswer>(body) {
            Ok(TellAnswer { ok, output }) => TellOutcome::Answered { ok, output },
            Err(err) => TellOutcome::Unreachable(format!(
                "the hub's /tell answer is not the admin surface's JSON: {err}"
            )),
        };
    }
    TellOutcome::Refused {
        status,
        message: error_message(body),
    }
}

/// The hub's own message from an error answer: its `{"error": "..."}` when the
/// body is that object, and the body itself otherwise (still the hub's words,
/// never one this client invents).
fn error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.trim().to_string())
}

/// Sends one HTTP/1.1 request to `addr` and reads its whole answer: one
/// request per connection, `Connection: close`, the whole exchange bounded by
/// `budget`.
///
/// The address is the checked literal loopback ([`parse_loopback`]), so no
/// resolution happens here - which is where the loopback policy is enforced
/// for good.
fn exchange(
    addr: SocketAddr,
    budget: Duration,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<Answer, String> {
    let deadline = Instant::now() + budget;
    let mut stream = TcpStream::connect_timeout(&addr, remaining(deadline, budget)?)
        .map_err(|err| format!("cannot connect to {addr}: {err}"))?;
    // The backstop: a read that outlives the budget fails as a timeout even if
    // it is the first read and no deadline check has run yet.
    stream
        .set_read_timeout(Some(budget))
        .map_err(|err| format!("the hub connection could not be bounded: {err}"))?;
    stream
        .set_write_timeout(Some(remaining(deadline, budget)?))
        .map_err(|err| format!("the hub connection could not be bounded: {err}"))?;
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(body) = body {
        request.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    request.push_str("\r\n");
    let mut wire = request.into_bytes();
    if let Some(body) = body {
        wire.extend_from_slice(body);
    }
    stream
        .write_all(&wire)
        .and_then(|()| stream.flush())
        .map_err(|err| format!("cannot send the request: {err}"))?;
    let mut reader = BufReader::with_capacity(MAX_HEAD + 1, stream);
    read_response(&mut reader, deadline, budget)
}

/// Reads one response: the status line, the headers (bounded together by
/// [`MAX_HEAD`]) and the body (bounded by [`MAX_BODY`]).
///
/// Anything malformed - a status line that is not `HTTP/1.x CODE ...`, a
/// header without a `:`, a chunked body, a head or body that is too large, a
/// body shorter than its `Content-Length`, a connection that ends mid-head -
/// is an error, never a panic.
fn read_response<R: Read>(
    reader: &mut BufReader<R>,
    deadline: Instant,
    budget: Duration,
) -> Result<Answer, String> {
    let mut line = Vec::new();
    read_line(reader, deadline, MAX_HEAD, &mut line, budget)?;
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
        read_line(reader, deadline, MAX_HEAD, &mut line, budget)?;
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
    remaining(deadline, budget)?;
    match content_length {
        Some(length) => {
            (&mut *reader)
                .take(length as u64)
                .read_to_end(&mut body)
                .map_err(|err| read_error(err, budget))?;
            if body.len() != length {
                return Err("the hub's answer ended before its Content-Length".to_string());
            }
        }
        // No Content-Length: the answer is read to the close, still bounded.
        None => {
            (&mut *reader)
                .take(MAX_BODY as u64 + 1)
                .read_to_end(&mut body)
                .map_err(|err| read_error(err, budget))?;
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
    budget: Duration,
) -> Result<(), String> {
    line.clear();
    remaining(deadline, budget)?;
    let read = (&mut *reader)
        .take(limit as u64 + 1)
        .read_until(b'\n', line)
        .map_err(|err| read_error(err, budget))?;
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
fn remaining(deadline: Instant, budget: Duration) -> Result<Duration, String> {
    match deadline.checked_duration_since(Instant::now()) {
        Some(left) if !left.is_zero() => Ok(left),
        _ => Err(format!("the hub did not answer within {}s", budget.as_secs())),
    }
}

/// Why a read of the hub's answer failed. A read that ran out the socket's
/// timeout is reported as one - the kernel hands it back as `WouldBlock` - so
/// a 502 can say the hub went quiet rather than blaming a malformed answer.
fn read_error(err: io::Error, budget: Duration) -> String {
    match err.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            format!("the hub did not answer within {}s", budget.as_secs())
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
        read_response(
            &mut BufReader::new(Cursor::new(raw.to_vec())),
            roomy(),
            TIMEOUT,
        )
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
            TIMEOUT,
        )
        .expect_err("a deadline in the past");
        assert!(error.contains("did not answer within"), "{error}");

        assert_eq!(
            read_error(io::Error::from(io::ErrorKind::WouldBlock), TIMEOUT),
            "the hub did not answer within 5s"
        );
        assert_eq!(
            read_error(io::Error::from(io::ErrorKind::WouldBlock), ADMIN_TIMEOUT),
            "the hub did not answer within 10s",
            "the admin budget names its own bound"
        );
        assert!(
            read_error(io::Error::from(io::ErrorKind::Other), TIMEOUT)
                .contains("could not be read"),
            "another error keeps its own reason"
        );
    }

    /// The admin surface is the loopback's too, by the same exact matcher: the
    /// positives normalize to the literal loopback, and every lookalike that
    /// is not the status surface is not the admin surface either.
    #[test]
    fn only_the_loopback_is_an_admin_surface() {
        assert_eq!(
            HubAdmin::parse("localhost:19081").expect("localhost is loopback"),
            HubAdmin::parse("127.0.0.1:19081").expect("the loopback"),
        );
        assert_eq!(
            HubAdmin::parse("127.0.0.1:19081").expect("the loopback").addr(),
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 19081)),
        );
        for value in [
            "",
            "127.0.0.1",
            "localhost",
            "127.0.0.1:",
            "127.0.0.1:0",
            "127.0.0.1.",
            "127.0.0.1.:19081",
            "127.0.0.1.evil.tld:19081",
            "localhost.evil.tld:19081",
            "api.127.0.0.1:19081",
            "LOCALHOST:19081",
            "Localhost:19081",
            "127.0.0.1:19081 ",
            "127.0.0.1 :19081",
            "10.0.0.1:19081",
            "0.0.0.0:19081",
            "[::1]:19081",
            "::1:19081",
            "example.com:19081",
            "http://127.0.0.1:19081",
            "http://localhost:19081/tell",
            "127.0.0.1:19081/tell",
            "127.0.0.1:19081?q=1",
            "127.0.0.1:notaport",
            "127.0.0.1:70000",
            "127.0.0.1:-1",
            "off",
        ] {
            assert!(
                HubAdmin::parse(value).is_err(),
                "{value:?} must not be an admin surface"
            );
        }
    }

    /// The admin default is the live `--admin-port`, so the help text, the
    /// config default and the live stack cannot drift.
    #[test]
    fn the_admin_default_is_the_documented_surface() {
        assert_eq!(
            HubAdmin::default(),
            HubAdmin::parse(DEFAULT_HUB_ADMIN).expect("the documented default")
        );
        assert_eq!(HubAdmin::default().addr().port(), DEFAULT_HUB_ADMIN_PORT);
        assert_eq!(
            HubAdmin::default().addr(),
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 19081))
        );
    }

    /// The `/tell` envelope: a 200 is the agent's own `ok`/`output`, and any
    /// other status is the hub's own message, from its `{"error": "..."}` or,
    /// failing that, its body. A 200 that is not the envelope is not guessed
    /// at: it is unreadable, and the caller reports a 502.
    #[test]
    fn a_tell_answer_is_read_as_the_hub_wrote_it() {
        assert_eq!(
            tell_outcome(200, r#"{"ok":true,"output":"{\"size\":3}"}"#),
            TellOutcome::Answered {
                ok: true,
                output: r#"{"size":3}"#.to_string(),
            }
        );
        assert_eq!(
            tell_outcome(200, r#"{"ok":false,"output":"denied: ../escape"}"#),
            TellOutcome::Answered {
                ok: false,
                output: "denied: ../escape".to_string(),
            }
        );
        assert_eq!(
            tell_outcome(403, r#"{"error":"role librarian does not grant read_file"}"#),
            TellOutcome::Refused {
                status: 403,
                message: "role librarian does not grant read_file".to_string(),
            }
        );
        assert_eq!(
            tell_outcome(409, r#"{"error":"agent with guest CID 100 is not connected"}"#),
            TellOutcome::Refused {
                status: 409,
                message: "agent with guest CID 100 is not connected".to_string(),
            }
        );
        assert_eq!(
            tell_outcome(504, r#"{"error":"agent did not answer within 5000 ms"}"#),
            TellOutcome::Refused {
                status: 504,
                message: "agent did not answer within 5000 ms".to_string(),
            }
        );
        // A refusal whose body is not the hub's JSON still carries the hub's
        // own words rather than an invented message.
        assert_eq!(
            tell_outcome(502, "bad gateway"),
            TellOutcome::Refused {
                status: 502,
                message: "bad gateway".to_string(),
            }
        );
        // A 200 that is not `{"ok","output"}` is unreadable, never a guess.
        assert!(matches!(
            tell_outcome(200, "not json"),
            TellOutcome::Unreachable(_)
        ));
        assert!(matches!(
            tell_outcome(200, r#"{"ok":true}"#),
            TellOutcome::Unreachable(_)
        ));
    }
}
