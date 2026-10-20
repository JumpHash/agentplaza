//! The E2B gateway: the node's end of the E2B SDKs' traffic to plaza sandboxes.
//!
//! An SDK configured with `E2B_SANDBOX_URL=http://127.0.0.1:<port>` sends every sandbox request
//! to this one loopback address, naming the sandbox in `E2b-Sandbox-Id` (or in a `Host` of the
//! form `49983-<id>.<anything>`). The gateway asks apid which VM the sandbox is, checks the
//! sandbox's access token and that the port is the one the sandbox daemon serves, then relays the
//! request into the guest through the VM's vsock muxer: a Unix socket at `{uds-dir}/vm{cid}` that
//! answers `CONNECT <port>` with `OK <port>` and then carries the guest's stream.
//!
//! One connection serves one request. The guest is sent `Connection: close`, and the client is
//! closed once the guest has closed, so the SDK reconnects for its next call. One audit line is
//! printed per request: the sandbox, method, path (without its query), status, bytes each way and
//! milliseconds. No header value is ever printed, since the access token travels in one.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use apz_uds::UnixStream;

use super::apid::{Apid, E2bSandbox};
use super::sys;

/// The port the sandbox's daemon serves in the guest. It is the only one the gateway relays.
const ENVD_PORT: u32 = 49983;
/// The request head is capped here; a longer one is refused with 431.
const MAX_HEAD: usize = 16 * 1024;
/// A client must send its whole head within this long.
const HEAD_TIMEOUT: Duration = Duration::from_secs(30);
/// Connections served at once, relays included. One more is answered 503 before a thread is
/// started for it, so a flood of clients cannot start threads without end.
const MAX_PENDING: usize = 64;
/// A relay is closed after this long with no bytes in either direction.
const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// The muxer answers `CONNECT` at once; a guest that does not accept the port is reset.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Relays in flight per sandbox. The next one is refused with 503, so one sandbox cannot take
/// every connection the gateway has.
const MAX_RELAYS_PER_SANDBOX: usize = 32;
/// How long a sandbox apid has named is remembered. Refusals are not remembered, so a sandbox
/// created a moment ago is found at once.
const CACHE_TTL: Duration = Duration::from_secs(10);
/// Remembered sandboxes at most; past this the cache is pruned, then cleared.
const MAX_CACHE: usize = 1024;
/// How often the accept loop checks for a stop request.
const ACCEPT_POLL: Duration = Duration::from_millis(100);
/// The copy buffer for each direction of a relay.
const CHUNK: usize = 16 * 1024;
/// The longest status line the audit reads from the guest's answer.
const MAX_STATUS_LINE: usize = 256;
/// The longest sandbox id the gateway accepts; it becomes part of a path sent to apid.
const MAX_SANDBOX_ID: usize = 64;
/// Hop-by-hop headers (RFC 9110 section 7.6.1), none of which the guest is sent.
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "upgrade",
];

/// The gateway's shared state: how to ask apid, where the muxer sockets are, the sandbox lookups
/// remembered for a moment, and the relays in flight per sandbox.
pub struct Gateway {
    apid: Apid,
    uds_dir: PathBuf,
    cache: Mutex<HashMap<String, (Instant, E2bSandbox)>>,
    relays: Mutex<HashMap<String, usize>>,
}

impl Gateway {
    /// A gateway that asks `apid` for sandboxes and reaches guests through the muxer sockets in
    /// `uds_dir`.
    pub fn new(apid: Apid, uds_dir: PathBuf) -> Self {
        Self {
            apid,
            uds_dir,
            cache: Mutex::new(HashMap::new()),
            relays: Mutex::new(HashMap::new()),
        }
    }

    /// The sandbox `id` as apid names it, `None` when apid does not know it. A found sandbox is
    /// kept for [`CACHE_TTL`]; the access token is still checked on every request.
    fn resolve(&self, id: &str) -> Result<Option<E2bSandbox>, String> {
        if let Some((at, sandbox)) = lock(&self.cache).get(id)
            && at.elapsed() < CACHE_TTL
        {
            return Ok(Some(sandbox.clone()));
        }
        let found = self.apid.e2b_sandbox(id)?;
        if let Some(sandbox) = &found {
            let mut cache = lock(&self.cache);
            if cache.len() >= MAX_CACHE {
                cache.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
                if cache.len() >= MAX_CACHE {
                    cache.clear();
                }
            }
            cache.insert(id.to_string(), (Instant::now(), sandbox.clone()));
        }
        Ok(found)
    }

    /// Takes one of the sandbox's relay slots, or `None` when all are in use. The slot is given
    /// back when the guard drops, so a relay holds it for its whole life.
    fn take_slot(self: &Arc<Self>, id: &str) -> Option<Slot> {
        let mut relays = lock(&self.relays);
        let count = relays.entry(id.to_string()).or_insert(0);
        if *count >= MAX_RELAYS_PER_SANDBOX {
            return None;
        }
        *count += 1;
        Some(Slot {
            gateway: Arc::clone(self),
            id: id.to_string(),
        })
    }
}

/// One relay slot of a sandbox, returned on drop.
struct Slot {
    gateway: Arc<Gateway>,
    id: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut relays = lock(&self.gateway.relays);
        if let Some(count) = relays.get_mut(&self.id) {
            *count -= 1;
            if *count == 0 {
                relays.remove(&self.id);
            }
        }
    }
}

/// A place among the connections the gateway serves at once, given back when the guard drops.
struct Permit(Arc<AtomicUsize>);

impl Permit {
    /// A place for one more connection, or `None` when [`MAX_PENDING`] are taken.
    fn take(pending: &Arc<AtomicUsize>) -> Option<Self> {
        pending
            .try_update(Ordering::AcqRel, Ordering::Acquire, |taken| {
                (taken < MAX_PENDING).then_some(taken + 1)
            })
            .ok()
            .map(|_| Self(Arc::clone(pending)))
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Answers a connection that has no place with 503, then closes it. The write is bounded, so a
/// client that is not reading cannot hold up the accept loop.
fn turn_away(client: &mut TcpStream) {
    let _ = client.set_write_timeout(Some(ACCEPT_POLL));
    let _ = client.write_all(
        b"HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
    );
}

/// Accepts E2B connections on `listener` until a stop request, each served on its own thread. A
/// connection with no place among [`MAX_PENDING`] is turned away before its thread is started.
pub fn serve(listener: TcpListener, gateway: Arc<Gateway>) {
    if let Err(err) = listener.set_nonblocking(true) {
        eprintln!("vsockd: e2b cannot poll its listener: {err}");
        return;
    }
    let pending = Arc::new(AtomicUsize::new(0));
    while !sys::shutdown_requested() {
        match listener.accept() {
            Ok((mut client, _)) => {
                // A blocking socket again: the accepted one may inherit the listener's mode.
                let _ = client.set_nonblocking(false);
                let Some(permit) = Permit::take(&pending) else {
                    turn_away(&mut client);
                    continue;
                };
                let gateway = Arc::clone(&gateway);
                let started = thread::Builder::new().spawn(move || {
                    let _permit = permit;
                    serve_conn(client, &gateway);
                });
                if let Err(err) = started {
                    // The failed spawn dropped its closure, and with it the place and the connection.
                    eprintln!("vsockd: e2b cannot start a thread: {err}");
                }
            }
            Err(err) => {
                if err.kind() != io::ErrorKind::WouldBlock {
                    eprintln!("vsockd: e2b accept failed: {err}");
                }
                thread::sleep(ACCEPT_POLL);
            }
        }
    }
}

/// Serves one client connection and prints its audit line.
fn serve_conn(mut client: TcpStream, gateway: &Arc<Gateway>) {
    let started = Instant::now();
    let mut audit = Audit::default();
    let _ = client.set_read_timeout(Some(HEAD_TIMEOUT));
    let _ = client.set_write_timeout(Some(HEAD_TIMEOUT));
    if let Err(refusal) = handle(&mut client, gateway, &mut audit) {
        audit.status = Some(refusal.status);
        respond(&mut client, refusal);
    }
    println!("{}", audit_line(&audit, started.elapsed()));
}

/// Reads the head, checks it, and relays the request. Every `Err` happens before anything is sent
/// to the guest, so the client is answered with the refusal.
fn handle(
    client: &mut TcpStream,
    gateway: &Arc<Gateway>,
    audit: &mut Audit,
) -> Result<(), Refusal> {
    let (head, rest) = read_head(client, Instant::now() + HEAD_TIMEOUT)?;
    let parsed = parse_head(&head)?;
    audit.sandbox.clone_from(&parsed.sandbox);
    audit.method.clone_from(&parsed.method);
    audit.path.clone_from(&parsed.path);

    let found = gateway.resolve(&parsed.sandbox).map_err(|err| {
        eprintln!("vsockd: e2b sandbox lookup failed: {err}");
        refuse(502, "sandbox lookup failed")
    })?;
    // An unknown sandbox gets the same answer as a wrong token, so the gateway does not say which ids exist.
    let sandbox = found.ok_or(refuse(401, "wrong or missing access token"))?;
    if !sandbox.access_token.is_empty() && !token_ok(&parsed.tokens, &sandbox.access_token) {
        return Err(refuse(401, "wrong or missing access token"));
    }
    let _slot = gateway
        .take_slot(&parsed.sandbox)
        .ok_or(refuse(503, "too many requests for this sandbox"))?;
    let guest = connect_guest(&gateway.uds_dir, sandbox.cid).map_err(|err| {
        eprintln!(
            "vsockd: e2b sandbox {} is not reachable: {err}",
            parsed.sandbox
        );
        refuse(502, "sandbox is not reachable")
    })?;
    relay(client, guest, parsed.forward.as_bytes(), &rest, audit)
}

/// Sends the request to the guest and carries its answer back until the guest closes. The client
/// is closed then, and so is the guest side of a client that closed first.
fn relay(
    client: &mut TcpStream,
    mut guest: UnixStream,
    head: &[u8],
    rest: &[u8],
    audit: &mut Audit,
) -> Result<(), Refusal> {
    let _ = client.set_read_timeout(Some(IDLE_TIMEOUT));
    let _ = client.set_write_timeout(Some(IDLE_TIMEOUT));
    let (mut reader, mut writer) = match (client.try_clone(), guest.try_clone()) {
        (Ok(reader), Ok(writer)) => (reader, writer),
        _ => return Err(refuse(502, "sandbox relay failed")),
    };
    guest
        .write_all(head)
        .and_then(|()| guest.write_all(rest))
        .map_err(|err| {
            eprintln!("vsockd: e2b sending to the guest failed: {err}");
            refuse(502, "sandbox is not reachable")
        })?;
    audit.bytes_in = (head.len() + rest.len()) as u64;

    // The client's remaining body bytes go to the guest on their own thread, so the guest's answer
    // can be carried back while the body is still arriving.
    let uploader = thread::spawn(move || {
        let sent = pump(&mut reader, &mut writer);
        let _ = writer.shutdown(Shutdown::Write);
        sent
    });
    let mut status = StatusLine::default();
    let mut received = 0u64;
    let mut buf = vec![0u8; CHUNK];
    while let Some(n) = read_chunk(&mut guest, &mut buf) {
        if audit.status.is_none() {
            audit.status = status.feed(&buf[..n]);
        }
        if client.write_all(&buf[..n]).is_err() {
            break;
        }
        received += n as u64;
    }
    let _ = guest.shutdown(Shutdown::Both);
    let _ = client.shutdown(Shutdown::Both);
    audit.bytes_in += uploader.join().unwrap_or(0);
    audit.bytes_out = received;
    Ok(())
}

/// Copies the client's bytes to the guest until either side ends, and returns how many it sent.
fn pump(from: &mut TcpStream, to: &mut UnixStream) -> u64 {
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    while let Some(n) = read_chunk(from, &mut buf) {
        if to.write_all(&buf[..n]).is_err() {
            break;
        }
        total += n as u64;
    }
    total
}

/// Opens the guest's sandbox port through the VM's muxer socket. The muxer's answer is read one
/// byte at a time, so no byte of the guest's stream is taken with it.
fn connect_guest(uds_dir: &Path, cid: u32) -> io::Result<UnixStream> {
    let mut guest = UnixStream::connect(uds_dir.join(format!("vm{cid}")))?;
    guest.set_read_timeout(Some(CONNECT_TIMEOUT))?;
    guest.set_write_timeout(Some(CONNECT_TIMEOUT))?;
    guest.write_all(format!("CONNECT {ENVD_PORT}\n").as_bytes())?;
    let mut answer = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        if read_chunk(&mut guest, &mut byte).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the muxer closed before it answered",
            ));
        }
        if byte[0] == b'\n' {
            break;
        }
        answer.push(byte[0]);
        if answer.len() > 64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the muxer's answer is too long",
            ));
        }
    }
    if !answer.starts_with(b"OK ") {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "the guest does not serve the port",
        ));
    }
    guest.set_read_timeout(Some(IDLE_TIMEOUT))?;
    guest.set_write_timeout(Some(IDLE_TIMEOUT))?;
    Ok(guest)
}

/// What the gateway answers instead of relaying: a status and the message that is also its
/// reason phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Refusal {
    status: u16,
    message: &'static str,
}

fn refuse(status: u16, message: &'static str) -> Refusal {
    Refusal { status, message }
}

/// Writes a refusal and closes. A client that has already gone is not an error.
fn respond(client: &mut TcpStream, refusal: Refusal) {
    let body = format!("{}\n", refusal.message);
    let reply = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        refusal.status,
        refusal.message,
        body.len()
    );
    let _ = client.write_all(reply.as_bytes());
}

/// The head as the gateway reads it: what the request is, who it is for, the tokens it carries,
/// and the head the guest is sent.
#[derive(Debug, PartialEq, Eq)]
struct Parsed {
    method: String,
    /// The target without its query, for the audit line.
    path: String,
    sandbox: String,
    tokens: Vec<String>,
    /// The request line and the headers the guest gets, ending with the blank line.
    forward: String,
}

/// A client connection whose reads can be bounded in time, so that a whole head is read against
/// one deadline rather than a timeout per read.
trait Bounded: Read {
    /// Makes the next read wait no longer than `limit`.
    fn bound_reads(&mut self, limit: Duration) -> io::Result<()>;
}

impl Bounded for TcpStream {
    fn bound_reads(&mut self, limit: Duration) -> io::Result<()> {
        self.set_read_timeout(Some(limit))
    }
}

/// Reads the request head up to its blank line, all of it by `deadline`. Returns the head and
/// whatever body bytes came with it. A head that does not end within [`MAX_HEAD`] bytes is refused
/// with 431, and one not complete by `deadline` with 408. Each read waits no longer than what is
/// left of the deadline, so a client sending a byte at a time cannot hold the head open.
fn read_head(client: &mut impl Bounded, deadline: Instant) -> Result<(String, Vec<u8>), Refusal> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = vec![0u8; 1024];
    loop {
        if let Some(end) = buf
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|at| at + 4)
        {
            if end > MAX_HEAD {
                return Err(refuse(431, "request head too large"));
            }
            let rest = buf.split_off(end);
            let head = String::from_utf8(buf).map_err(|_| refuse(400, "malformed request head"))?;
            return Ok((head, rest));
        }
        if buf.len() >= MAX_HEAD {
            return Err(refuse(431, "request head too large"));
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(refuse(408, "request head timed out"));
        }
        if client.bound_reads(left).is_err() {
            return Err(refuse(400, "incomplete request head"));
        }
        match read_chunk(client, &mut chunk) {
            Some(n) => buf.extend_from_slice(&chunk[..n]),
            None => return Err(refuse(400, "incomplete request head")),
        }
    }
}

/// Parses a request head. Refuses a malformed one (400), a sandbox id that is missing or
/// malformed (400), a sandbox header given twice (400), and any port but 49983 (403).
fn parse_head(head: &str) -> Result<Parsed, Refusal> {
    let mut lines = head.split("\r\n");
    let (method, target, version) = request_line(lines.next().unwrap_or(""))?;
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or(refuse(400, "malformed header"))?;
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(refuse(400, "malformed header"));
        }
        headers.push((name.to_string(), value.trim().to_string()));
    }

    // The headers the `Connection` header names are hop-by-hop too.
    let listed: Vec<String> = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| {
            value
                .split(',')
                .map(|token| token.trim().to_ascii_lowercase())
        })
        .filter(|token| !token.is_empty())
        .collect();

    let id_header = single_header(&headers, "E2b-Sandbox-Id")?;
    let port_header = single_header(&headers, "E2b-Sandbox-Port")?;
    let host_target = single_header(&headers, "Host")?.and_then(host_sandbox);
    let sandbox = match (id_header, host_target) {
        (Some(id), _) => id,
        (None, Some((_, id))) => id,
        (None, None) => return Err(refuse(400, "no sandbox named")),
    };
    if !valid_sandbox_id(sandbox) {
        return Err(refuse(400, "malformed sandbox id"));
    }
    let port = match port_header {
        Some(text) => Some(
            text.parse::<u32>()
                .map_err(|_| refuse(400, "malformed sandbox port"))?,
        ),
        None => host_target.map(|(port, _)| port),
    };
    if port.is_some_and(|port| port != ENVD_PORT) {
        return Err(refuse(403, "only port 49983 is served"));
    }

    let tokens: Vec<String> = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("x-access-token"))
        .map(|(_, value)| value.clone())
        .collect();
    let mut forward = format!("{method} {target} {version}\r\n");
    for (name, value) in &headers {
        if forwarded(name, &listed) {
            forward.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    // The guest sees its own host, and one request per connection.
    forward.push_str("Host: localhost\r\nConnection: close\r\n\r\n");
    Ok(Parsed {
        method: method.to_string(),
        path: target.split('?').next().unwrap_or(target).to_string(),
        sandbox: sandbox.to_string(),
        tokens,
        forward,
    })
}

/// The method, target and version of a request line, each checked for the shape the gateway can
/// relay.
fn request_line(line: &str) -> Result<(&str, &str, &str), Refusal> {
    let mut parts = line.split(' ');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(method), Some(target), Some(version), None)
            if !method.is_empty()
                && method.bytes().all(|byte| byte.is_ascii_alphabetic())
                && !target.is_empty()
                && target.bytes().all(|byte| byte.is_ascii_graphic())
                && matches!(version, "HTTP/1.1" | "HTTP/1.0") =>
        {
            Ok((method, target, version))
        }
        _ => Err(refuse(400, "malformed request line")),
    }
}

/// The one value of the named header; `None` when it is absent, a refusal when it is given twice.
fn single_header<'a>(
    headers: &'a [(String, String)],
    name: &str,
) -> Result<Option<&'a str>, Refusal> {
    let mut found = headers
        .iter()
        .filter(|(header, _)| header.eq_ignore_ascii_case(name));
    let first = found.next().map(|(_, value)| value.as_str());
    if found.next().is_some() {
        return Err(refuse(400, "a sandbox header is repeated"));
    }
    Ok(first)
}

/// The port and sandbox id a `Host` names, as in `49983-<id>.<anything>`. A `:port` after the
/// name is ignored. `None` for any other host.
fn host_sandbox(host: &str) -> Option<(u32, &str)> {
    let name = host.split(':').next()?;
    let label = name.split('.').next()?;
    let (port, id) = label.split_once('-')?;
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((port.parse().ok()?, id))
}

/// A sandbox id is short and made of letters, digits, `-` and `_`, since it becomes part of a path
/// that apid is asked for.
fn valid_sandbox_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SANDBOX_ID
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Whether a header is passed to the guest. Identity headers (`E2b-*`, `X-Access-Token`, `Host`)
/// and hop-by-hop headers are not.
fn forwarded(name: &str, listed: &[String]) -> bool {
    let lower = name.to_ascii_lowercase();
    !(HOP_BY_HOP.contains(&lower.as_str())
        || lower == "host"
        || lower == "x-access-token"
        || lower.starts_with("e2b-")
        || listed.contains(&lower))
}

/// Whether the request's tokens all match the sandbox's, and there is at least one. Compared in
/// constant time, so a wrong token does not reveal how much of it was right.
fn token_ok(given: &[String], expected: &str) -> bool {
    !given.is_empty() && given.iter().all(|token| same_secret(token, expected))
}

fn same_secret(given: &str, expected: &str) -> bool {
    let (a, b) = (given.as_bytes(), expected.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Reads the status code from the first line of the guest's answer, as the audit names it.
#[derive(Default)]
struct StatusLine {
    head: Vec<u8>,
    done: bool,
}

impl StatusLine {
    /// Feeds the next bytes of the answer. Returns the status once the first line is complete.
    fn feed(&mut self, chunk: &[u8]) -> Option<u16> {
        if self.done {
            return None;
        }
        self.head.extend_from_slice(chunk);
        match self.head.iter().position(|byte| *byte == b'\n') {
            Some(end) => {
                self.done = true;
                parse_status(&self.head[..end])
            }
            None => {
                if self.head.len() > MAX_STATUS_LINE {
                    self.done = true;
                }
                None
            }
        }
    }
}

/// The status code of an HTTP/1.x status line, if it is one.
fn parse_status(line: &[u8]) -> Option<u16> {
    let text = std::str::from_utf8(line).ok()?.trim_end_matches('\r');
    let mut parts = text.split(' ');
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    parts
        .next()?
        .parse::<u16>()
        .ok()
        .filter(|status| (100..=599).contains(status))
}

/// What the audit line records about one request.
#[derive(Default)]
struct Audit {
    sandbox: String,
    method: String,
    path: String,
    status: Option<u16>,
    bytes_in: u64,
    bytes_out: u64,
}

/// The audit line for one request. Fields that were never learned print as `-`.
fn audit_line(audit: &Audit, elapsed: Duration) -> String {
    let or_dash = |text: &str| {
        if text.is_empty() {
            "-".to_string()
        } else {
            printable(text)
        }
    };
    let status = audit
        .status
        .map_or_else(|| "-".to_string(), |status| status.to_string());
    format!(
        "vsockd: e2b sandbox={} method={} path={} status={status} in={} out={} ms={}",
        or_dash(&audit.sandbox),
        or_dash(&audit.method),
        or_dash(&audit.path),
        audit.bytes_in,
        audit.bytes_out,
        elapsed.as_millis()
    )
}

/// The text with anything that is not printable ASCII replaced, so a request cannot forge a line.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_graphic() { c } else { '?' })
        .collect()
}

/// Reads at most one buffer's worth. `None` at end of stream or on any error other than an
/// interrupted read, which is retried.
fn read_chunk(from: &mut impl Read, buf: &mut [u8]) -> Option<usize> {
    loop {
        match from.read(buf) {
            Ok(0) => return None,
            Ok(n) => return Some(n),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_uds::UnixListener;

    const TOKEN: &str = "gateway-test-token";
    const STREAM_CHUNKS: [&str; 3] = ["data: one\n\n", "data: two\n\n", "data: three\n\n"];

    /// A head read from memory has no socket whose reads could be bounded.
    impl Bounded for &[u8] {
        fn bound_reads(&mut self, _limit: Duration) -> io::Result<()> {
            Ok(())
        }
    }

    /// A fresh directory for one test's sockets and token file.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vsockd-e2b-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Reads through the first `end`, one byte at a time, so nothing past it is taken.
    fn read_through(stream: &mut impl Read, end: &[u8]) -> Option<String> {
        let mut bytes = Vec::new();
        while !bytes.ends_with(end) {
            let mut byte = [0u8; 1];
            read_chunk(stream, &mut byte)?;
            bytes.push(byte[0]);
        }
        String::from_utf8(bytes).ok()
    }

    fn content_length(head: &str) -> usize {
        head.lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse().ok())
            .unwrap_or(0)
    }

    /// apid's sandbox lookup, faked. `sbx-secret` (CID 3, token `s3cret`) and `sbx-open` (CID 3,
    /// no token) are known; `sbx-gone` is known at CID 9, where no VM is served. A lookup without
    /// the internal token is refused. Returns the port and the count of answered lookups.
    fn fake_apid() -> (u16, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let lookups = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&lookups);
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                let counter = Arc::clone(&counter);
                thread::spawn(move || {
                    let Some(head) = read_through(&mut conn, b"\r\n\r\n") else {
                        return;
                    };
                    let path = head.split(' ').nth(1).unwrap_or_default();
                    let token_ok = head.lines().any(|line| {
                        line.to_ascii_lowercase() == format!("x-apz-internal: {TOKEN}")
                    });
                    let (status, body) = if !token_ok {
                        (403, "{}")
                    } else {
                        counter.fetch_add(1, Ordering::SeqCst);
                        match path.strip_prefix("/internal/e2b/sandboxes/") {
                            Some("sbx-secret") => (200, r#"{"cid":3,"access_token":"s3cret"}"#),
                            Some("sbx-open") => (200, r#"{"cid":3,"access_token":""}"#),
                            Some("sbx-gone") => (200, r#"{"cid":9,"access_token":""}"#),
                            _ => (404, r#"{"error":"unknown"}"#),
                        }
                    };
                    let reply = format!(
                        "HTTP/1.1 {status} x\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = conn.write_all(reply.as_bytes());
                });
            }
        });
        (port, lookups)
    }

    /// The muxer's socket for CID 3, faked. It answers `CONNECT 49983` with `OK 1024` and records
    /// each request head it is sent. `/unary` answers JSON naming the request's body length;
    /// `/stream` answers chunked, one chunk at a time. Any other path is a 404.
    fn fake_muxer(dir: &Path) -> Arc<Mutex<Vec<String>>> {
        let listener = UnixListener::bind(dir.join("vm3")).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                let record = Arc::clone(&record);
                thread::spawn(move || {
                    let Some(connect) = read_through(&mut conn, b"\n") else {
                        return;
                    };
                    if connect != "CONNECT 49983\n" {
                        return;
                    }
                    let _ = conn.write_all(b"OK 1024\n");
                    let Some(head) = read_through(&mut conn, b"\r\n\r\n") else {
                        return;
                    };
                    let mut request_body = vec![0u8; content_length(&head)];
                    if conn.read_exact(&mut request_body).is_err() {
                        return;
                    }
                    record.lock().unwrap().push(head.clone());
                    let path = head.split(' ').nth(1).unwrap_or_default().to_string();
                    if path.starts_with("/stream") {
                        let _ = conn.write_all(
                            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                        );
                        for chunk in STREAM_CHUNKS {
                            let _ = conn
                                .write_all(format!("{:x}\r\n{chunk}\r\n", chunk.len()).as_bytes());
                            thread::sleep(Duration::from_millis(20));
                        }
                        let _ = conn.write_all(b"0\r\n\r\n");
                    } else if path.starts_with("/unary") {
                        let body = format!(r#"{{"ok":true,"body_len":{}}}"#, request_body.len());
                        let _ = conn.write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            )
                            .as_bytes(),
                        );
                    } else {
                        let _ = conn.write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                });
            }
        });
        seen
    }

    struct Rig {
        port: u16,
        seen: Arc<Mutex<Vec<String>>>,
        lookups: Arc<AtomicUsize>,
    }

    /// A gateway on a loopback port, over the fake apid and the fake muxer for CID 3.
    fn start_rig(name: &str) -> Rig {
        let dir = scratch(name);
        let seen = fake_muxer(&dir);
        let (apid_port, lookups) = fake_apid();
        let token_file = dir.join("token");
        std::fs::write(&token_file, format!("{TOKEN}\n")).unwrap();
        let apid = Apid::new(&format!("http://127.0.0.1:{apid_port}"), Some(token_file)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let gateway = Arc::new(Gateway::new(apid, dir));
        thread::spawn(move || serve(listener, gateway));
        Rig {
            port,
            seen,
            lookups,
        }
    }

    /// Sends `request` to the gateway and returns all it answers, which ends when it closes.
    fn ask(port: u16, request: &str) -> String {
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        client.write_all(request.as_bytes()).unwrap();
        let mut answer = Vec::new();
        client.read_to_end(&mut answer).unwrap();
        String::from_utf8(answer).unwrap()
    }

    fn status_of(answer: &str) -> &str {
        answer.split(' ').nth(1).unwrap_or("")
    }

    #[test]
    fn e2b_routes_by_header_and_by_host() {
        let rig = start_rig("route");
        let by_header = ask(
            rig.port,
            "GET /unary HTTP/1.1\r\nHost: 127.0.0.1\r\nE2b-Sandbox-Id: sbx-open\r\nE2b-Sandbox-Port: 49983\r\n\r\n",
        );
        assert_eq!(status_of(&by_header), "200", "{by_header}");
        assert!(
            by_header.ends_with(r#"{"ok":true,"body_len":0}"#),
            "{by_header}"
        );

        let by_host = ask(
            rig.port,
            "GET /unary?x=1 HTTP/1.1\r\nHost: 49983-sbx-open.sandboxes.test\r\n\r\n",
        );
        assert_eq!(status_of(&by_host), "200", "{by_host}");
        assert_eq!(
            rig.lookups.load(Ordering::SeqCst),
            1,
            "the second request was answered from the cache"
        );
        let heads = rig.seen.lock().unwrap().clone();
        assert_eq!(heads.len(), 2);
        assert!(heads[0].starts_with("GET /unary HTTP/1.1\r\n"));
        assert!(heads[1].starts_with("GET /unary?x=1 HTTP/1.1\r\n"));
    }

    #[test]
    fn e2b_refuses_bad_tokens_and_ports_and_answers_400_or_404() {
        // The original test: a missing or wrong access token is refused; the right one is relayed.
        {
            let rig = start_rig("token");
            let missing = ask(
                rig.port,
                "GET /unary HTTP/1.1\r\nE2b-Sandbox-Id: sbx-secret\r\n\r\n",
            );
            assert_eq!(status_of(&missing), "401", "{missing}");
            let wrong = ask(
                rig.port,
                "GET /unary HTTP/1.1\r\nE2b-Sandbox-Id: sbx-secret\r\nX-Access-Token: nope\r\n\r\n",
            );
            assert_eq!(status_of(&wrong), "401", "{wrong}");
            let right = ask(
                rig.port,
                "GET /unary HTTP/1.1\r\nE2b-Sandbox-Id: sbx-secret\r\nX-Access-Token: s3cret\r\n\r\n",
            );
            assert_eq!(status_of(&right), "200", "{right}");
            let open = ask(
                rig.port,
                "GET /unary HTTP/1.1\r\nE2b-Sandbox-Id: sbx-open\r\n\r\n",
            );
            assert_eq!(status_of(&open), "200", "{open}");
            assert_eq!(
                rig.seen.lock().unwrap().len(),
                2,
                "a refused request never reaches the guest"
            );
        }

        // The original test: every port but the sandbox daemons' is refused.
        {
            let rig = start_rig("port");
            let other = ask(
                rig.port,
                "GET /unary HTTP/1.1\r\nE2b-Sandbox-Id: sbx-open\r\nE2b-Sandbox-Port: 8080\r\n\r\n",
            );
            assert_eq!(status_of(&other), "403", "{other}");
            let other_host = ask(
                rig.port,
                "GET /unary HTTP/1.1\r\nHost: 3000-sbx-open.sandboxes.test\r\n\r\n",
            );
            assert_eq!(status_of(&other_host), "403", "{other_host}");
            let daemon = ask(
                rig.port,
                "GET /unary HTTP/1.1\r\nE2b-Sandbox-Id: sbx-open\r\nE2b-Sandbox-Port: 49983\r\n\r\n",
            );
            assert_eq!(status_of(&daemon), "200", "{daemon}");
            assert_eq!(
                rig.seen.lock().unwrap().len(),
                1,
                "only the daemon's port is relayed"
            );
        }

        // No sandbox id is answered 400, an unknown one 401 (as a wrong token is), a gone VM 502.
        {
            let rig = start_rig("unknown");
            let none = ask(rig.port, "GET / HTTP/1.1\r\nHost: 127.0.0.1:19080\r\n\r\n");
            assert_eq!(status_of(&none), "400", "{none}");
            let malformed = ask(
                rig.port,
                "GET /unary HTTP/1.1\r\nE2b-Sandbox-Id: sbx/../x\r\n\r\n",
            );
            assert_eq!(status_of(&malformed), "400", "{malformed}");
            let unknown = ask(
                rig.port,
                "GET /unary HTTP/1.1\r\nE2b-Sandbox-Id: nope\r\n\r\n",
            );
            assert_eq!(status_of(&unknown), "401", "{unknown}");
            let no_vm = ask(
                rig.port,
                "GET /unary HTTP/1.1\r\nE2b-Sandbox-Id: sbx-gone\r\n\r\n",
            );
            assert_eq!(status_of(&no_vm), "502", "{no_vm}");
            let no_target = ask(rig.port, "GET\r\n\r\n");
            assert_eq!(status_of(&no_target), "400", "{no_target}");
            assert_eq!(rig.seen.lock().unwrap().len(), 0);
        }
    }

    #[test]
    fn e2b_strips_the_identity_and_hop_headers_and_keeps_the_rest() {
        let head = "POST /files?path=/a HTTP/1.1\r\nHost: 49983-sbx-open.x\r\nE2b-Sandbox-Id: sbx-open\r\nX-Access-Token: t\r\nKeep-Alive: timeout=5\r\nProxy-Authorization: p\r\nConnection: keep-alive, X-Private\r\nX-Private: 1\r\nX-Trace: kept\r\nContent-Length: 5\r\n\r\n";
        let parsed = parse_head(head).unwrap();
        assert_eq!(parsed.sandbox, "sbx-open");
        assert_eq!(parsed.tokens, vec!["t".to_string()]);
        assert_eq!(parsed.path, "/files");
        assert_eq!(
            parsed.forward,
            "POST /files?path=/a HTTP/1.1\r\nX-Trace: kept\r\nContent-Length: 5\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        );
    }

    #[test]
    fn e2b_streams_a_chunked_answer_intact() {
        let rig = start_rig("stream");
        let answer = ask(
            rig.port,
            "GET /stream HTTP/1.1\r\nE2b-Sandbox-Id: sbx-open\r\n\r\n",
        );
        let chunks: String = STREAM_CHUNKS
            .iter()
            .map(|chunk| format!("{:x}\r\n{chunk}\r\n", chunk.len()))
            .collect();
        let expected = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{chunks}0\r\n\r\n"
        );
        assert_eq!(answer, expected);
    }

    #[test]
    fn e2b_relays_a_request_body_sent_after_the_head() {
        let rig = start_rig("body");
        let mut client = TcpStream::connect(("127.0.0.1", rig.port)).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        client
            .write_all(
                b"POST /unary HTTP/1.1\r\nE2b-Sandbox-Id: sbx-open\r\nContent-Length: 11\r\n\r\n",
            )
            .unwrap();
        thread::sleep(Duration::from_millis(50));
        client.write_all(b"hello world").unwrap();
        let mut answer = String::new();
        client.read_to_string(&mut answer).unwrap();
        assert!(answer.ends_with(r#"{"ok":true,"body_len":11}"#), "{answer}");
    }

    #[test]
    fn e2b_refuses_a_head_over_16_kib_and_reads_one_under() {
        let far = Instant::now() + HEAD_TIMEOUT;
        let big = format!("GET / HTTP/1.1\r\nX-Pad: {}\r\n\r\n", "a".repeat(MAX_HEAD));
        assert_eq!(read_head(&mut big.as_bytes(), far).unwrap_err().status, 431);
        let small = "GET / HTTP/1.1\r\nHost: x\r\n\r\nbody";
        let (head, rest) = read_head(&mut small.as_bytes(), far).unwrap();
        assert_eq!(head, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(rest, b"body".to_vec());
        let unfinished = "GET / HTTP/1.1\r\nHost: x\r\n";
        assert_eq!(
            read_head(&mut unfinished.as_bytes(), Instant::now())
                .unwrap_err()
                .status,
            408,
            "a head not complete by its deadline is refused"
        );
    }

    #[test]
    fn e2b_admits_at_most_64_connections_at_once() {
        let pending = Arc::new(AtomicUsize::new(0));
        let held: Vec<Permit> = (0..MAX_PENDING)
            .map(|_| Permit::take(&pending).expect("a place"))
            .collect();
        assert!(
            Permit::take(&pending).is_none(),
            "the 65th connection is turned away"
        );
        drop(held);
        assert!(
            Permit::take(&pending).is_some(),
            "a returned place is usable again"
        );
    }

    #[test]
    fn e2b_caps_relays_per_sandbox_at_32() {
        let gateway = Arc::new(Gateway::new(Apid::off(), PathBuf::new()));
        let held: Vec<Slot> = (0..MAX_RELAYS_PER_SANDBOX)
            .map(|_| gateway.take_slot("sbx-a").expect("a slot"))
            .collect();
        assert!(
            gateway.take_slot("sbx-a").is_none(),
            "the 33rd relay is refused"
        );
        assert!(
            gateway.take_slot("sbx-b").is_some(),
            "another sandbox has its own slots"
        );
        drop(held);
        assert!(
            gateway.take_slot("sbx-a").is_some(),
            "a returned slot is usable again"
        );
    }
}
