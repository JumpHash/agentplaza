//! The egress proxy. A guest has no network of its own, so its outbound HTTP
//! proxy traffic arrives over vsock port 5001 and the hub connects out on its
//! behalf. One listener per served CID ([`accept_loop`]), one thread per
//! connection, and a [`Policy`] that refuses every non-public destination.
//! Each connection ends in one audit line and, when apid is configured, one
//! record POSTed to apid.
//!
//! One destination is not on the internet at all: `plaza.gateway`, which the
//! agent's model calls go to. Its plain-HTTP requests are relayed to apid's
//! gateway for the agent's CID ([`gateway`]); nothing is resolved for it.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use apz_uds::{UnixListener, UnixStream};
use serde_json::json;

use crate::apid::{Apid, TOKEN_HEADER};
use crate::sys;

/// The vsock port the guest's proxy traffic arrives on.
pub const EGRESS_PORT: u32 = 5001;
/// The request head (request line and headers) is capped at this many bytes.
const MAX_HEAD: usize = 8 * 1024;
/// How long a guest may take to send its request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// A tunnel with no traffic for this long is closed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// How often a plain tunnel's blocked read wakes to check whether the other
/// direction has ended (see [`Hangup`]).
const PUMP_POLL: Duration = Duration::from_secs(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const ESTABLISHED: &[u8] = b"HTTP/1.1 200 Connection established\r\n\r\n";
const COPY_BUF: usize = 16 * 1024;
/// The gateway paths a guest may upgrade to a WebSocket (the model streaming paths).
const UPGRADE_PATHS: &[&str] = &["/openai/v1/responses", "/anthropic/v1/messages"];
/// Upgraded sockets one agent may hold open at once.
const MAX_UPGRADES_PER_CID: usize = 4;
/// How often a relayed WebSocket checks its idle and lifetime limits.
const WS_POLL: Duration = Duration::from_secs(1);
/// A relayed WebSocket with no bytes either way for this long is closed.
const WS_IDLE: Duration = Duration::from_secs(120);
/// A relayed WebSocket is closed after this long, whatever its traffic.
const WS_MAX_LIFETIME: Duration = Duration::from_secs(60 * 60);
/// The destination whose plain-HTTP requests are relayed to apid's gateway.
const GATEWAY_HOST: &str = "plaza.gateway";
/// The only port the gateway answers on.
const GATEWAY_PORT: u16 = 80;
/// Guest headers that never reach apid: hop-by-hop ones, the guest's own
/// credentials, and any internal token the guest forged.
const GATEWAY_DROPPED: &[&str] = &[
    // apid only serves loopback Hosts (DNS-rebinding guard); the head gets its own below.
    "host",
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "upgrade",
    "authorization",
    "x-api-key",
    TOKEN_HEADER,
];

/// The two sockets of a tunnel, cloned so either pump can hang both up, and
/// the flag that tells the other pump to stop. The flag is what ends the other
/// direction: its reads wake every [`PUMP_POLL`] to check it, so no pump
/// depends on a shutdown waking a read blocked in another thread (which
/// WinSock does not do).
struct Hangup {
    guest: UnixStream,
    upstream: TcpStream,
    done: AtomicBool,
}

impl Hangup {
    fn new(guest: UnixStream, upstream: TcpStream) -> Self {
        Self { guest, upstream, done: AtomicBool::new(false) }
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// Stops both directions: the flag for the other pump, and both sockets
    /// shut down so the guest and the upstream see the tunnel end.
    fn hang_up(&self) {
        self.done.store(true, Ordering::SeqCst);
        let _ = self.guest.shutdown(Shutdown::Both);
        let _ = self.upstream.shutdown(Shutdown::Both);
        // Ends a read already waiting on the upstream socket at once on
        // Windows, rather than at its next poll.
        super::sys::wake_readers(&self.upstream);
    }
}

/// Which destinations and ports a guest may reach.
#[derive(Debug, Clone)]
pub struct Policy {
    /// Ports allowed beyond 80 and 443 (`--egress-ports`).
    extra_ports: Vec<u16>,
    /// Set only by tests, so the end-to-end test can reach its loopback echo server.
    allow_loopback: bool,
}

impl Policy {
    pub fn new(extra_ports: Vec<u16>) -> Self {
        Self { extra_ports, allow_loopback: false }
    }

    fn port_allowed(&self, port: u16) -> bool {
        port == 80 || port == 443 || self.extra_ports.contains(&port)
    }

    /// The reason an address is refused, or `None` when it may be reached.
    fn refusal(&self, ip: IpAddr) -> Option<&'static str> {
        if self.allow_loopback && ip.is_loopback() {
            return None;
        }
        addr_refusal(ip)
    }
}

/// Why a resolved address is refused, or `None` when it is public.
pub fn addr_refusal(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => v4_refusal(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4_refusal(v4),
            None => v6_refusal(v6),
        },
    }
}

fn v4_refusal(ip: Ipv4Addr) -> Option<&'static str> {
    if ip.is_unspecified() {
        Some("unspecified")
    } else if ip.is_loopback() {
        Some("loopback")
    } else if ip.is_private() {
        Some("private")
    } else if ip.is_link_local() {
        Some("link-local")
    } else if ip.is_broadcast() {
        Some("broadcast")
    } else if ip.is_multicast() {
        Some("multicast")
    } else {
        None
    }
}

fn v6_refusal(ip: Ipv6Addr) -> Option<&'static str> {
    let first = ip.segments()[0];
    if ip.is_unspecified() {
        Some("unspecified")
    } else if ip.is_loopback() {
        Some("loopback")
    } else if ip.is_multicast() {
        Some("multicast")
    } else if first & 0xffc0 == 0xfe80 {
        Some("link-local")
    } else if first & 0xfe00 == 0xfc00 {
        Some("unique-local")
    } else {
        None
    }
}

/// Why a connection was not served. Each maps to the answer the guest gets.
#[derive(Debug, PartialEq, Eq)]
enum Stop {
    /// A request this proxy does not speak.
    Bad,
    /// A destination or port the policy refuses.
    Forbidden,
    /// The destination did not resolve or did not accept the connection.
    Unreachable,
    /// The agent already holds its maximum number of upgraded sockets.
    TooMany,
}

impl Stop {
    fn answer(&self) -> &'static [u8] {
        match self {
            Stop::Bad => &b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..],
            Stop::Forbidden => &b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..],
            Stop::Unreachable => &b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..],
            Stop::TooMany => &b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..],
        }
    }
}

/// A request the proxy serves.
#[derive(Debug, PartialEq, Eq)]
enum Head {
    /// `CONNECT host:port HTTP/1.1`
    Connect { host: String, port: u16 },
    /// `METHOD http://host[:port]/path HTTP/1.1`; the path is kept in origin form.
    Absolute { method: String, host: String, port: u16, path: String },
}

/// What one connection did, for the audit line.
#[derive(Debug, Default)]
pub struct Audit {
    host: String,
    port: u16,
    allowed: bool,
    up: u64,
    down: u64,
}

impl Audit {
    fn report(&self, cid: u32, elapsed: Duration, apid: &Apid) {
        let ms = elapsed.as_millis() as u64;
        println!(
            "vsockd: egress cid={cid} host={} port={} allowed={} bytes_up={} bytes_down={} ms={ms}",
            self.host, self.port, self.allowed, self.up, self.down
        );
        if apid.enabled() {
            let record = json!({
                "cid": cid,
                "host": self.host,
                "port": self.port,
                "allowed": self.allowed,
                "bytes_up": self.up,
                "bytes_down": self.down,
                "ms": ms,
            })
            .to_string();
            if let Err(err) = apid.egress(cid, &record) {
                println!("vsockd: warning: cannot send the egress record for cid {cid} to apid: {err}");
            }
        }
    }
}

/// Accepts guest proxy connections for one CID, each served on its own
/// thread, until a stop request.
pub fn accept_loop(cid: u32, listener: UnixListener, policy: Arc<Policy>, apid: Apid) {
    while !sys::shutdown_requested() {
        if let Err(err) = sys::poll_readable(&listener, super::ACCEPT_POLL_MS) {
            eprintln!("vsockd: egress poll failed: {err}");
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let (policy, apid) = (Arc::clone(&policy), apid.clone());
                thread::spawn(move || serve_conn(stream, cid, &policy, &apid));
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(err) => eprintln!("vsockd: egress accept failed: {err}"),
        }
    }
}

/// Serves one guest connection, audits it, and returns what it did.
fn serve_conn(mut guest: UnixStream, cid: u32, policy: &Policy, apid: &Apid) -> Audit {
    let started = Instant::now();
    let mut audit = Audit::default();
    let _ = guest.set_read_timeout(Some(HEAD_TIMEOUT));
    if let Err(stop) = proxy(&mut guest, cid, policy, apid, &mut audit) {
        let _ = guest.write_all(stop.answer());
    }
    audit.report(cid, started.elapsed(), apid);
    audit
}

/// Reads the request, checks it, connects out, and tunnels or forwards.
/// Every `Err` happens before anything is written to the guest.
fn proxy(guest: &mut UnixStream, cid: u32, policy: &Policy, apid: &Apid, audit: &mut Audit) -> Result<(), Stop> {
    let (head, rest) = read_head(guest)?;
    let parsed = parse_head(&head)?;
    let (host, port) = match &parsed {
        Head::Connect { host, port } | Head::Absolute { host, port, .. } => (host.clone(), *port),
    };
    audit.host = host.clone();
    audit.port = port;
    if host.eq_ignore_ascii_case(GATEWAY_HOST) {
        // Plain HTTP only; CONNECT to the gateway is refused.
        return match parsed {
            Head::Absolute { method, path, port: GATEWAY_PORT, .. } => {
                gateway(guest, cid, apid, (method.as_str(), path.as_str()), &head, rest, audit)
            }
            _ => Err(Stop::Forbidden),
        };
    }
    if !policy.port_allowed(port) {
        return Err(Stop::Forbidden);
    }
    let addrs = checked_addrs(policy, &host, port)?;
    audit.allowed = true;

    let mut upstream = None;
    for addr in &addrs {
        if let Ok(stream) = TcpStream::connect_timeout(addr, CONNECT_TIMEOUT) {
            upstream = Some(stream);
            break;
        }
    }
    let upstream = upstream.ok_or(Stop::Unreachable)?;
    guest.set_read_timeout(Some(PUMP_POLL)).map_err(|_| Stop::Unreachable)?;
    upstream.set_read_timeout(Some(PUMP_POLL)).map_err(|_| Stop::Unreachable)?;

    // Every clone is made before anything is written, so a failure here still
    // leaves the guest an error answer rather than half a tunnel.
    let guest_read = guest.try_clone().map_err(|_| Stop::Unreachable)?;
    let guest_write = guest.try_clone().map_err(|_| Stop::Unreachable)?;
    let mut upstream_write = upstream.try_clone().map_err(|_| Stop::Unreachable)?;
    let hangup = Arc::new(Hangup::new(
        guest.try_clone().map_err(|_| Stop::Unreachable)?,
        upstream.try_clone().map_err(|_| Stop::Unreachable)?,
    ));

    let mut sent = 0u64;
    let mut guest_write = guest_write;
    match parsed {
        Head::Connect { .. } => {
            let _ = guest_write.write_all(ESTABLISHED);
        }
        Head::Absolute { method, path, .. } => {
            let out = origin_head(&method, &path, &head);
            upstream_write.write_all(out.as_bytes()).map_err(|_| Stop::Unreachable)?;
            sent += out.len() as u64;
        }
    }
    if upstream_write.write_all(&rest).is_ok() {
        sent += rest.len() as u64;
    }

    let up_hangup = Arc::clone(&hangup);
    let up = thread::spawn(move || pump(guest_read, upstream_write, &up_hangup));
    let down = pump(upstream, guest_write, &hangup);
    audit.down = down;
    audit.up = sent + up.join().unwrap_or(0);
    Ok(())
}

/// Relays one plain-HTTP request from the agent at `cid` to apid's gateway and
/// streams apid's answer back unchanged. `request` is the method and the path
/// (origin form); `head` and `rest` are the guest's head and the bytes after
/// it. Nothing is written to the guest before apid is reached.
///
/// A WebSocket upgrade ([`is_websocket_upgrade`]) is checked against
/// [`UPGRADE_PATHS`] and the per-CID cap first. When apid answers `101`, the
/// tunnel is relayed both ways by [`pump_ws`]; any other answer is relayed as a
/// normal response.
fn gateway(
    guest: &mut UnixStream,
    cid: u32,
    apid: &Apid,
    request: (&str, &str),
    head: &str,
    rest: Vec<u8>,
    audit: &mut Audit,
) -> Result<(), Stop> {
    let (method, path) = request;
    let upgrade = is_websocket_upgrade(method, head);
    if upgrade && !upgrade_path_allowed(path) {
        return Err(Stop::Forbidden);
    }
    // Held until the tunnel ends, so the cap counts open sockets, not requests.
    let _slot = if upgrade { Some(UpgradeSlot::take(cid).ok_or(Stop::TooMany)?) } else { None };
    let token = apid.token().map_err(|_| Stop::Unreachable)?;
    let mut upstream = apid.connect(CONNECT_TIMEOUT).map_err(|_| Stop::Unreachable)?;
    guest.set_read_timeout(Some(IDLE_TIMEOUT)).map_err(|_| Stop::Unreachable)?;
    upstream.set_read_timeout(Some(IDLE_TIMEOUT)).map_err(|_| Stop::Unreachable)?;

    let guest_read = guest.try_clone().map_err(|_| Stop::Unreachable)?;
    let mut guest_write = guest.try_clone().map_err(|_| Stop::Unreachable)?;
    let mut upstream_write = upstream.try_clone().map_err(|_| Stop::Unreachable)?;
    let hangup = Arc::new(Hangup::new(
        guest.try_clone().map_err(|_| Stop::Unreachable)?,
        upstream.try_clone().map_err(|_| Stop::Unreachable)?,
    ));

    let out = gateway_head(method, cid, path, head, &token, upgrade);
    upstream_write.write_all(out.as_bytes()).map_err(|_| Stop::Unreachable)?;
    let mut sent = out.len() as u64;
    if upstream_write.write_all(&rest).is_ok() {
        sent += rest.len() as u64;
    }
    audit.allowed = true;

    let mut switched = false;
    let mut answered = 0u64;
    if upgrade {
        let (answer, answer_rest) = read_head(&mut upstream).map_err(|_| Stop::Unreachable)?;
        switched = answer.starts_with("HTTP/1.1 101 ");
        answered = (answer.len() + answer_rest.len()) as u64;
        let _ = guest_write.write_all(answer.as_bytes());
        let _ = guest_write.write_all(&answer_rest);
    }

    if switched {
        println!("vsockd: egress websocket cid={cid} path={} upgraded", path.split('?').next().unwrap_or(""));
        guest.set_read_timeout(Some(WS_POLL)).map_err(|_| Stop::Unreachable)?;
        upstream.set_read_timeout(Some(WS_POLL)).map_err(|_| Stop::Unreachable)?;
        let clock = Arc::new(WsClock::new());
        let up_clock = Arc::clone(&clock);
        let up_hangup = Arc::clone(&hangup);
        let up = thread::spawn(move || pump_ws(guest_read, upstream_write, &up_clock, &up_hangup));
        audit.down = answered + pump_ws(upstream, guest_write, &clock, &hangup);
        audit.up = sent + up.join().unwrap_or(0);
        return Ok(());
    }

    guest.set_read_timeout(Some(PUMP_POLL)).map_err(|_| Stop::Unreachable)?;
    upstream.set_read_timeout(Some(PUMP_POLL)).map_err(|_| Stop::Unreachable)?;
    let up_hangup = Arc::clone(&hangup);
    let up = thread::spawn(move || pump(guest_read, upstream_write, &up_hangup));
    audit.down = answered + pump(upstream, guest_write, &hangup);
    audit.up = sent + up.join().unwrap_or(0);
    Ok(())
}

/// Whether a guest request asks for a WebSocket upgrade: a `GET` with
/// `Upgrade: websocket`, a `Connection` header listing `upgrade`, and a
/// `Sec-WebSocket-Key`.
fn is_websocket_upgrade(method: &str, head: &str) -> bool {
    if method != "GET" {
        return false;
    }
    let (mut upgrade, mut connection, mut key) = (false, false, false);
    for line in head.split("\r\n").skip(1).take_while(|line| !line.is_empty()) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "upgrade" => upgrade |= value.eq_ignore_ascii_case("websocket"),
            "connection" => connection |= value.split(',').any(|item| item.trim().eq_ignore_ascii_case("upgrade")),
            "sec-websocket-key" => key |= !value.is_empty(),
            _ => {}
        }
    }
    upgrade && connection && key
}

/// Whether an upgrade may go to this gateway path (the query string is ignored).
fn upgrade_path_allowed(path: &str) -> bool {
    UPGRADE_PATHS.contains(&path.split('?').next().unwrap_or(""))
}

/// One upgraded socket counted against its agent's cap. Dropping it frees the slot.
#[derive(Debug)]
struct UpgradeSlot(u32);

/// Open upgraded sockets per CID, shared by every connection thread.
static UPGRADES: Mutex<BTreeMap<u32, usize>> = Mutex::new(BTreeMap::new());

impl UpgradeSlot {
    /// A slot for `cid`, or `None` when it already holds [`MAX_UPGRADES_PER_CID`].
    fn take(cid: u32) -> Option<Self> {
        let mut open = UPGRADES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = open.entry(cid).or_insert(0);
        if *count >= MAX_UPGRADES_PER_CID {
            return None;
        }
        *count += 1;
        Some(Self(cid))
    }
}

impl Drop for UpgradeSlot {
    fn drop(&mut self) {
        let mut open = UPGRADES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = open.get_mut(&self.0) {
            *count -= 1;
            if *count == 0 {
                open.remove(&self.0);
            }
        }
    }
}

/// When a relayed WebSocket last carried bytes, shared by both of its pumps.
struct WsClock {
    start: Instant,
    last_ms: AtomicU64,
}

impl WsClock {
    fn new() -> Self {
        Self { start: Instant::now(), last_ms: AtomicU64::new(0) }
    }

    fn touch(&self) {
        self.last_ms.store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    /// True once the tunnel is past its lifetime or has been idle too long.
    fn expired(&self) -> bool {
        let now = self.start.elapsed();
        let last = Duration::from_millis(self.last_ms.load(Ordering::Relaxed));
        now >= WS_MAX_LIFETIME || now.saturating_sub(last) >= WS_IDLE
    }
}

/// Copies one direction of a relayed WebSocket. The reads wake every
/// [`WS_POLL`] so the clock is checked; a tunnel with bytes flowing either way
/// keeps its idle timer reset. When this direction ends, both sockets are shut
/// down so the other direction ends too. Returns the bytes copied.
fn pump_ws(mut from: impl Read, mut to: impl Write, clock: &WsClock, hangup: &Arc<Hangup>) -> u64 {
    use std::io::ErrorKind::{Interrupted, TimedOut, WouldBlock};
    let mut buf = vec![0u8; COPY_BUF];
    let mut total = 0u64;
    while !clock.expired() && !hangup.is_done() {
        match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
                clock.touch();
                total += n as u64;
            }
            Err(err) if matches!(err.kind(), WouldBlock | TimedOut | Interrupted) => {}
            Err(_) => break,
        }
    }
    hangup.hang_up();
    total
}

/// The head apid receives: the guest's head with its request line moved under
/// apid's gateway for `cid`, the headers in [`GATEWAY_DROPPED`] and any header
/// the guest listed in `Connection` dropped, the internal token added, and
/// `Connection: close` so apid ends its answer when it is done. Framing headers
/// (`Content-Length`, `Transfer-Encoding`) stay, because the body is relayed as bytes.
/// For an `upgrade`, the `Upgrade` header stays and the connection is `Upgrade`
/// instead of `close`, so apid can switch protocols.
fn gateway_head(method: &str, cid: u32, path: &str, head: &str, token: &str, upgrade: bool) -> String {
    let lines: Vec<&str> = head.split("\r\n").skip(1).take_while(|line| !line.is_empty()).collect();
    let listed: Vec<String> = lines
        .iter()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| value.split(',').map(|item| item.trim().to_ascii_lowercase()))
        .filter(|item| !(upgrade && item == "upgrade"))
        .collect();
    let mut out = format!("{method} /internal/v1/agents/{cid}/gateway{path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
    for line in lines {
        let name = line.split_once(':').map_or(String::new(), |(name, _)| name.trim().to_ascii_lowercase());
        let kept_upgrade = upgrade && name == "upgrade";
        if (GATEWAY_DROPPED.contains(&name.as_str()) && !kept_upgrade) || listed.contains(&name) {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    let connection = if upgrade { "Upgrade" } else { "close" };
    out.push_str(&format!("{TOKEN_HEADER}: {token}\r\nConnection: {connection}\r\n\r\n"));
    out
}

/// Copies `from` to `to` until either side closes, this direction has been
/// idle for [`IDLE_TIMEOUT`], or the other direction has ended; then hangs
/// both up so the other direction ends too. `from` reads with a
/// [`PUMP_POLL`] timeout, which is when the flag and the idle time are
/// checked. Returns the bytes copied.
fn pump(mut from: impl Read, mut to: impl Write, hangup: &Arc<Hangup>) -> u64 {
    use std::io::ErrorKind::{Interrupted, TimedOut, WouldBlock};
    let mut buf = vec![0u8; COPY_BUF];
    let mut total = 0u64;
    let mut last = Instant::now();
    while !hangup.is_done() {
        match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
                total += n as u64;
                last = Instant::now();
            }
            Err(err) if matches!(err.kind(), WouldBlock | TimedOut | Interrupted) => {
                if last.elapsed() >= IDLE_TIMEOUT {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    hangup.hang_up();
    total
}

/// Reads the request head through its blank line; returns it with any bytes
/// that followed it.
fn read_head(guest: &mut impl Read) -> Result<(String, Vec<u8>), Stop> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(end) = buf.windows(4).position(|window| window == b"\r\n\r\n").map(|at| at + 4) {
            if end > MAX_HEAD {
                return Err(Stop::Bad);
            }
            let rest = buf.split_off(end);
            return String::from_utf8(buf).map(|head| (head, rest)).map_err(|_| Stop::Bad);
        }
        if buf.len() >= MAX_HEAD {
            return Err(Stop::Bad);
        }
        match guest.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(Stop::Bad),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

/// Parses the request line of a head into a `CONNECT` or an absolute-URI request.
fn parse_head(head: &str) -> Result<Head, Stop> {
    let line = head.split("\r\n").next().unwrap_or("");
    let parts: Vec<&str> = line.split(' ').collect();
    let &[method, target, version] = parts.as_slice() else {
        return Err(Stop::Bad);
    };
    if !version.starts_with("HTTP/1.") || method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(Stop::Bad);
    }
    if method == "CONNECT" {
        let (host, port) = authority(target, None).ok_or(Stop::Bad)?;
        return Ok(Head::Connect { host, port });
    }
    let rest = match target.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("http://") => &target[7..],
        _ => return Err(Stop::Bad),
    };
    let (auth, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    let (host, port) = authority(auth, Some(80)).ok_or(Stop::Bad)?;
    Ok(Head::Absolute { method: method.to_string(), host, port, path: path.to_string() })
}

/// `host[:port]` or `[v6][:port]`; the port falls back to `default`.
fn authority(auth: &str, default: Option<u16>) -> Option<(String, u16)> {
    let (host, port) = if let Some(bracketed) = auth.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']')?;
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':')?),
        };
        (host, port)
    } else {
        match auth.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, Some(port)),
            // An unbracketed IPv6 literal is ambiguous.
            Some(_) => return None,
            None => (auth, None),
        }
    };
    if host.is_empty() {
        return None;
    }
    let port = match port {
        Some(text) => text.parse().ok()?,
        None => default?,
    };
    Some((host.to_string(), port))
}

/// The head with its request line in origin form and the proxy's own
/// `Proxy-Connection` header dropped.
fn origin_head(method: &str, path: &str, head: &str) -> String {
    let mut out = format!("{method} {path} HTTP/1.1\r\n");
    for line in head.split("\r\n").skip(1).take_while(|line| !line.is_empty()) {
        if !line.to_ascii_lowercase().starts_with("proxy-connection:") {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    out.push_str("\r\n");
    out
}

/// Resolves `host:port` and checks every address. One refused address refuses
/// the destination. The returned addresses are the ones that were checked, and
/// they are the only ones connected to.
fn checked_addrs(policy: &Policy, host: &str, port: u16) -> Result<Vec<SocketAddr>, Stop> {
    let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs().map_err(|_| Stop::Unreachable)?.collect();
    if addrs.is_empty() {
        return Err(Stop::Unreachable);
    }
    if addrs.iter().any(|addr| policy.refusal(addr.ip()).is_some()) {
        return Err(Stop::Forbidden);
    }
    Ok(addrs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn the_policy_refuses_non_public_addresses_and_unlisted_ports() {
        let policy = Policy::new(vec![8080]);
        for ip in [
            "127.0.0.1", "10.1.2.3", "172.16.0.9", "172.31.255.1", "192.168.1.1", "169.254.10.10", "0.0.0.0",
            "255.255.255.255", "224.0.0.1", "::1", "::", "fe80::1", "fc00::5", "fd12::5", "::ffff:10.0.0.5",
        ] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(policy.refusal(ip).is_some(), "{ip} must be refused");
        }
        for ip in ["93.184.216.34", "172.32.0.1", "8.8.8.8", "2606:2800:220:1::1"] {
            let ip: IpAddr = ip.parse().unwrap();
            assert_eq!(policy.refusal(ip), None, "{ip} is public and allowed");
        }
        assert!(policy.port_allowed(80) && policy.port_allowed(443) && policy.port_allowed(8080));
        assert!(!policy.port_allowed(22) && !policy.port_allowed(25));
    }

    #[test]
    fn heads_parse_into_connect_and_absolute_requests() {
        assert_eq!(
            parse_head("CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n"),
            Ok(Head::Connect { host: "example.com".into(), port: 443 })
        );
        assert_eq!(
            parse_head("CONNECT [2606:2800::1]:8443 HTTP/1.1\r\n\r\n"),
            Ok(Head::Connect { host: "2606:2800::1".into(), port: 8443 })
        );
        assert_eq!(
            parse_head("GET http://example.com/a?b=1 HTTP/1.1\r\nHost: example.com\r\n\r\n"),
            Ok(Head::Absolute { method: "GET".into(), host: "example.com".into(), port: 80, path: "/a?b=1".into() })
        );
        assert_eq!(
            parse_head("POST HTTP://Example.com:8080 HTTP/1.1\r\n\r\n"),
            Ok(Head::Absolute { method: "POST".into(), host: "Example.com".into(), port: 8080, path: "/".into() })
        );
        for bad in [
            "CONNECT example.com HTTP/1.1\r\n\r\n",
            "CONNECT :443 HTTP/1.1\r\n\r\n",
            "CONNECT a:b:c HTTP/1.1\r\n\r\n",
            "GET /relative HTTP/1.1\r\n\r\n",
            "GET https://example.com/ HTTP/1.1\r\n\r\n",
            "get http://example.com/ HTTP/1.1\r\n\r\n",
            "GET http://example.com/ SPDY/3\r\n\r\n",
        ] {
            assert_eq!(parse_head(bad), Err(Stop::Bad), "{bad:?}");
        }
        assert_eq!(
            origin_head("GET", "/a", "GET http://x/a HTTP/1.1\r\nHost: x\r\nProxy-Connection: keep-alive\r\n\r\n"),
            "GET /a HTTP/1.1\r\nHost: x\r\n\r\n"
        );
    }

    #[test]
    fn the_gateway_head_is_rewritten_for_apid() {
        let head = "POST http://plaza.gateway/v1/messages?stream=1 HTTP/1.1\r\nHost: plaza.gateway\r\n\
                    Authorization: Bearer guest\r\nx-api-key: guest\r\nX-Apz-Internal: forged\r\n\
                    Proxy-Connection: keep-alive\r\nConnection: keep-alive, X-Hop\r\nX-Hop: 1\r\n\
                    Keep-Alive: 5\r\nContent-Type: application/json\r\nContent-Length: 4\r\n\r\n";
        assert_eq!(
            gateway_head("POST", 7, "/v1/messages?stream=1", head, "s3cret", false),
            "POST /internal/v1/agents/7/gateway/v1/messages?stream=1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\
             Content-Type: application/json\r\nContent-Length: 4\r\nx-apz-internal: s3cret\r\n\
             Connection: close\r\n\r\n"
        );
    }

    #[test]
    fn connect_to_the_gateway_is_refused_and_so_is_any_other_gateway_port() {
        for request in [
            "CONNECT plaza.gateway:443 HTTP/1.1\r\n\r\n",
            "CONNECT Plaza.Gateway:80 HTTP/1.1\r\n\r\n",
            "GET http://plaza.gateway:8080/v1/x HTTP/1.1\r\n\r\n",
        ] {
            let (mut client, server) = UnixStream::pair().unwrap();
            let policy = Policy::new(vec![]);
            let proxy = thread::spawn(move || serve_conn(server, 7, &policy, &Apid::off()));
            client.write_all(request.as_bytes()).unwrap();
            let mut answer = String::new();
            client.read_to_string(&mut answer).unwrap();
            assert!(answer.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{request:?}: {answer:?}");
            let audit = proxy.join().unwrap();
            assert!(!audit.allowed && audit.host.eq_ignore_ascii_case(GATEWAY_HOST), "{audit:?}");
        }
    }

    #[test]
    fn a_gateway_request_is_relayed_to_apid_and_its_answer_comes_back_intact() {
        const ANSWER: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 11\r\n\r\nhello world";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token_file = std::env::temp_dir().join(format!("vsockd-gateway-test-{}", std::process::id()));
        std::fs::write(&token_file, "test-token\n").unwrap();
        let apid = Apid::new(&format!("http://127.0.0.1:{port}"), Some(token_file.clone())).unwrap();

        // The stand-in for apid: checks the rewritten head and the body, then answers.
        let stand_in = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let (head, mut body) = read_head(&mut conn).unwrap();
            assert!(head.starts_with("POST /internal/v1/agents/7/gateway/v1/messages HTTP/1.1\r\n"), "{head}");
            assert!(head.contains("\r\nx-apz-internal: test-token\r\n"), "{head}");
            assert!(!head.to_ascii_lowercase().contains("authorization"), "{head}");
            while body.len() < 4 {
                let mut chunk = [0u8; 64];
                let n = conn.read(&mut chunk).unwrap();
                assert!(n > 0, "the guest's body ended early");
                body.extend_from_slice(&chunk[..n]);
            }
            assert_eq!(body, b"ping");
            conn.write_all(ANSWER).unwrap();
        });

        let (mut client, server) = UnixStream::pair().unwrap();
        let policy = Policy::new(vec![]);
        let proxy = thread::spawn(move || serve_conn(server, 7, &policy, &apid));
        client
            .write_all(
                b"POST http://plaza.gateway/v1/messages HTTP/1.1\r\nHost: plaza.gateway\r\n\
                  Authorization: Bearer guest\r\nContent-Length: 4\r\n\r\nping",
            )
            .unwrap();
        let mut answer = Vec::new();
        client.read_to_end(&mut answer).unwrap();
        assert_eq!(answer, ANSWER);

        stand_in.join().unwrap();
        let audit = proxy.join().unwrap();
        let _ = std::fs::remove_file(&token_file);
        assert!(audit.allowed && audit.host == GATEWAY_HOST && audit.port == 80, "{audit:?}");
    }

    #[test]
    fn a_websocket_upgrade_to_a_model_path_is_relayed_and_bytes_flow_both_ways() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token_file = std::env::temp_dir().join(format!("vsockd-upgrade-test-{}", std::process::id()));
        std::fs::write(&token_file, "test-token\n").unwrap();
        let apid = Apid::new(&format!("http://127.0.0.1:{port}"), Some(token_file.clone())).unwrap();

        // The stand-in for apid: checks the rewritten upgrade head, switches protocols,
        // then echoes one message the other way.
        let stand_in = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let (head, _) = read_head(&mut conn).unwrap();
            assert!(head.starts_with("GET /internal/v1/agents/7/gateway/openai/v1/responses HTTP/1.1\r\n"), "{head}");
            assert!(head.contains("\r\nx-apz-internal: test-token\r\n"), "{head}");
            assert!(head.contains("\r\nUpgrade: websocket\r\n"), "{head}");
            assert!(head.contains("\r\nConnection: Upgrade\r\n"), "{head}");
            assert!(!head.to_ascii_lowercase().contains("authorization"), "{head}");
            conn.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
            let mut ping = [0u8; 4];
            conn.read_exact(&mut ping).unwrap();
            assert_eq!(&ping, b"ping");
            conn.write_all(b"pong").unwrap();
        });

        let (mut client, server) = UnixStream::pair().unwrap();
        let policy = Policy::new(vec![]);
        let proxy = thread::spawn(move || serve_conn(server, 7, &policy, &apid));
        client
            .write_all(
                b"GET http://plaza.gateway/openai/v1/responses HTTP/1.1\r\nHost: plaza.gateway\r\n\
                  Authorization: Bearer guest\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\n\
                  Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
            )
            .unwrap();
        let (switching, _) = read_head(&mut client).unwrap();
        assert!(switching.starts_with("HTTP/1.1 101 "), "{switching}");
        client.write_all(b"ping").unwrap();
        let mut pong = Vec::new();
        client.read_to_end(&mut pong).unwrap();
        assert_eq!(pong, b"pong");

        stand_in.join().unwrap();
        let audit = proxy.join().unwrap();
        let _ = std::fs::remove_file(&token_file);
        assert!(audit.allowed && audit.host == GATEWAY_HOST && audit.port == 80, "{audit:?}");
    }

    #[test]
    fn an_upgrade_to_another_path_is_refused_without_contacting_apid() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let policy = Policy::new(vec![]);
        // Apid is off: were the request sent on, the answer would be 502, not 403.
        let proxy = thread::spawn(move || serve_conn(server, 7, &policy, &Apid::off()));
        client
            .write_all(
                b"GET http://plaza.gateway/v1/messages HTTP/1.1\r\nHost: plaza.gateway\r\nUpgrade: websocket\r\n\
                  Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
            )
            .unwrap();
        let mut answer = String::new();
        client.read_to_string(&mut answer).unwrap();
        assert!(answer.starts_with("HTTP/1.1 403 Forbidden\r\n"), "{answer:?}");
        let audit = proxy.join().unwrap();
        assert!(!audit.allowed, "{audit:?}");
    }

    #[test]
    fn a_fifth_upgraded_socket_for_one_agent_is_refused_with_429() {
        const CID: u32 = 9_001;
        let held: Vec<UpgradeSlot> = (0..MAX_UPGRADES_PER_CID).map(|_| UpgradeSlot::take(CID).unwrap()).collect();
        assert!(UpgradeSlot::take(CID).is_none(), "the fifth socket must be refused");
        assert!(UpgradeSlot::take(CID + 1).is_some(), "the cap is per agent");
        drop(held);
        assert!(UpgradeSlot::take(CID).is_some(), "a closed socket frees its slot");
        assert!(Stop::TooMany.answer().starts_with(b"HTTP/1.1 429 "));
    }

    #[test]
    fn a_connect_is_tunnelled_end_to_end_through_an_echo_server() {
        let echo = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = echo.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut conn, _) = echo.accept().unwrap();
            let mut buf = [0u8; 1024];
            loop {
                match conn.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => conn.write_all(&buf[..n]).unwrap(),
                }
            }
        });

        // Test-only override: loopback is reachable, and the echo port is allowed.
        let policy = Policy { allow_loopback: true, ..Policy::new(vec![port]) };
        let (mut client, server) = UnixStream::pair().unwrap();
        // Every wait is bounded, so a stall fails at the step that stalled
        // instead of hanging the test.
        client.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let apid = Apid::off();
        let (finished, proxy) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = finished.send(serve_conn(server, 7, &policy, &apid));
        });

        client
            .write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes())
            .unwrap();
        let mut answer = vec![0u8; ESTABLISHED.len()];
        client.read_exact(&mut answer).expect("the tunnel is established");
        assert_eq!(answer, ESTABLISHED);

        let payload = b"ping through the tunnel";
        client.write_all(payload).unwrap();
        let mut echoed = vec![0u8; payload.len()];
        client.read_exact(&mut echoed).expect("the payload comes back through the tunnel");
        assert_eq!(&echoed[..], &payload[..]);

        client.shutdown(Shutdown::Write).unwrap();
        let audit = proxy
            .recv_timeout(Duration::from_secs(20))
            .expect("the relay ends once the guest closed its side");
        assert!(
            audit.allowed && audit.port == port && audit.up == payload.len() as u64 && audit.down == payload.len() as u64,
            "{audit:?}"
        );
    }
}
