//! The egress proxy. A guest has no network of its own, so its outbound HTTP
//! proxy traffic arrives over vsock port 5001 and the hub connects out on its
//! behalf. One listener per served CID ([`accept_loop`]), one thread per
//! connection, and a [`Policy`] that refuses every non-public destination.
//! Before a connection is dialled, apid decides it: the fetch blocklist and the
//! laws (`/internal/egress/check`, see [`egress_decision`]). Each connection
//! ends in one audit line and, when apid is configured, one record POSTed to apid.
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

use crate::apid::{Apid, EgressAnswer, TOKEN_HEADER};
use crate::sys;

/// The TLS intercept for an allowed CONNECT whose decision says `intercept`. Declared here, by path,
/// so the CONNECT path owns it; its module docs say how it works.
#[path = "egress_tls.rs"]
mod egress_tls;

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

/// An allowed decision is kept this long, so a repeat connection skips apid.
const ALLOW_TTL: Duration = Duration::from_secs(60);
/// A refused decision is kept this long: short, so a host the owner just allowed is not refused for long.
const DENY_TTL: Duration = Duration::from_secs(10);
/// The most decisions kept at once. Past it a decision is asked of apid and not kept.
const MAX_DECISIONS: usize = 4096;
/// The most decisions kept for one CID, so one guest cannot fill the table. Past it a decision is
/// asked of apid and not kept, and the other CIDs' decisions are unaffected.
const MAX_DECISIONS_PER_CID: usize = 512;

/// Kept decisions: (CID, host, port) to when the decision expires and the decision.
type Kept = BTreeMap<(u32, String, u16), (Instant, EgressAnswer)>;

/// Apid's decisions, kept for their TTL and shared by every connection thread.
static DECISIONS: Mutex<Kept> = Mutex::new(BTreeMap::new());
/// Each CID's generation, moved by [`forget`]. A decision asked of apid before a `forget` is not kept
/// after it (see [`keep_decision`]).
static GENERATIONS: Mutex<BTreeMap<u32, u64>> = Mutex::new(BTreeMap::new());

/// Forgets every decision kept for guest `cid`: its rules changed (an E2B sandbox's network update),
/// so its next connection asks apid again. Returns how many were dropped. A decision asked of apid
/// before this call is not kept either.
pub fn forget(cid: u32) -> usize {
    let mut kept = DECISIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut generations = GENERATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *generations.entry(cid).or_insert(0) += 1;
    drop(generations);
    let before = kept.len();
    kept.retain(|key, _| key.0 != cid);
    before - kept.len()
}

/// The generation of guest `cid`: it moves on each [`forget`].
fn generation(cid: u32) -> u64 {
    let generations = GENERATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    generations.get(&cid).copied().unwrap_or(0)
}

/// The two sockets of a tunnel, so either pump can hang both up, and the flag
/// that tells the other pump to stop. The flag is what ends the other
/// direction: its reads wake every [`PUMP_POLL`] to check it, so no pump
/// depends on a shutdown waking a read blocked in another thread (which
/// WinSock does not do).
///
/// The guest socket is a clone; the upstream is the tunnel's own TCP handle,
/// which both pumps read and write through (`&TcpStream`). Not a `try_clone`:
/// on Windows a clone is a `WSADuplicateSocketW` handle whose shutdown now and
/// then fails with WSAENOTCONN, so the upstream would not see the tunnel end,
/// and [`sys::wake_readers`] cancels only the reads issued on the handle it is given.
struct Hangup {
    guest: UnixStream,
    upstream: TcpStream,
    done: AtomicBool,
    /// Woken by [`Hangup::hang_up`] and never drained: a plain pump waits on its source and this, so the
    /// other direction's end stops it at once instead of at its next poll.
    waker: crate::waker::Waker,
}

impl Hangup {
    fn new(guest: UnixStream, upstream: TcpStream) -> std::io::Result<Self> {
        Ok(Self {
            guest,
            upstream,
            done: AtomicBool::new(false),
            waker: crate::waker::Waker::new()?,
        })
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// Stops both directions: the flag for the other pump, and both sockets
    /// shut down so the guest and the upstream see the tunnel end.
    fn hang_up(&self) {
        self.done.store(true, Ordering::SeqCst);
        self.waker.wake();
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
        Self {
            extra_ports,
            allow_loopback: false,
        }
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
    /// apid refused the connection; the reason (naming the law, when one applies) is the body.
    Refused(String),
}

impl Stop {
    fn answer(&self) -> Vec<u8> {
        let fixed: &[u8] = match self {
            Stop::Bad => {
                &b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..]
            }
            Stop::Forbidden => {
                &b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..]
            }
            Stop::Unreachable => {
                &b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..]
            }
            Stop::TooMany => {
                &b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    [..]
            }
            Stop::Refused(reason) => {
                let body = format!("{reason}\n");
                let head = format!(
                    "HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                return [head.as_bytes(), body.as_bytes()].concat();
            }
        };
        fixed.to_vec()
    }
}

/// A request the proxy serves.
#[derive(Debug, PartialEq, Eq)]
enum Head {
    /// `CONNECT host:port HTTP/1.1`
    Connect { host: String, port: u16 },
    /// `METHOD http://host[:port]/path HTTP/1.1`; the path is kept in origin form.
    Absolute {
        method: String,
        host: String,
        port: u16,
        path: String,
    },
}

/// What one connection did, for the audit line.
#[derive(Debug, Default)]
pub struct Audit {
    host: String,
    port: u16,
    allowed: bool,
    /// The TLS was terminated and credentials were injected (`injected` counts the requests).
    /// Never a header value.
    intercepted: bool,
    injected: u64,
    up: u64,
    down: u64,
}

impl Audit {
    fn report(&self, cid: u32, elapsed: Duration, apid: &Apid) {
        let ms = elapsed.as_millis() as u64;
        println!(
            "vsockd: egress cid={cid} host={} port={} allowed={} intercepted={} injected={} bytes_up={} bytes_down={} ms={ms}",
            self.host, self.port, self.allowed, self.intercepted, self.injected, self.up, self.down
        );
        if apid.enabled() {
            let record = json!({
                "cid": cid,
                "host": self.host,
                "port": self.port,
                "allowed": self.allowed,
                "intercepted": self.intercepted,
                "injected": self.injected,
                "bytes_up": self.up,
                "bytes_down": self.down,
                "ms": ms,
            })
            .to_string();
            if let Err(err) = apid.egress(cid, &record) {
                println!(
                    "vsockd: warning: cannot send the egress record for cid {cid} to apid: {err}"
                );
            }
        }
    }
}

/// Accepts guest proxy connections for one CID, each served on its own
/// thread, until a stop request.
pub fn accept_loop(
    cid: u32,
    listener: UnixListener,
    policy: Arc<Policy>,
    apid: Apid,
    stop: Arc<std::sync::atomic::AtomicBool>,
    stop_waker: Arc<crate::waker::Waker>,
) {
    #[cfg(unix)]
    let listener_handle = {
        use std::os::fd::AsRawFd;
        listener.as_raw_fd()
    };
    #[cfg(windows)]
    let listener_handle = {
        use std::os::windows::io::AsRawSocket;
        listener.as_raw_socket()
    };
    // `stop` is the CID's own stop request (it is no longer served), checked on every wake like
    // the daemon-wide shutdown.
    while !sys::shutdown_requested() && !stop.load(std::sync::atomic::Ordering::SeqCst) {
        let handles = [
            listener_handle,
            stop_waker.handle(),
            super::shutdown_waker().handle(),
        ];
        if let Err(err) = sys::wait_readable(&handles, None) {
            eprintln!("vsockd: egress poll failed: {err}");
            thread::sleep(Duration::from_millis(100));
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let (policy, apid) = (Arc::clone(&policy), apid.clone());
                thread::spawn(move || serve_conn(stream, cid, &policy, &apid));
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(err) => {
                eprintln!("vsockd: egress accept failed: {err}");
                // A broken listener or fd exhaustion must not spin the loop.
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// Serves one guest connection, audits it, and returns what it did.
fn serve_conn(mut guest: UnixStream, cid: u32, policy: &Policy, apid: &Apid) -> Audit {
    let started = Instant::now();
    let mut audit = Audit::default();
    let _ = guest.set_read_timeout(Some(HEAD_TIMEOUT));
    if let Err(stop) = proxy(&mut guest, cid, policy, apid, &mut audit) {
        let _ = guest.write_all(&stop.answer());
    }
    audit.report(cid, started.elapsed(), apid);
    audit
}

/// Reads the request, checks it, connects out, and tunnels or forwards.
/// Every `Err` happens before anything is written to the guest.
fn proxy(
    guest: &mut UnixStream,
    cid: u32,
    policy: &Policy,
    apid: &Apid,
    audit: &mut Audit,
) -> Result<(), Stop> {
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
            Head::Absolute {
                method,
                path,
                port: GATEWAY_PORT,
                ..
            } => gateway(
                guest,
                cid,
                apid,
                (method.as_str(), path.as_str()),
                &head,
                rest,
                audit,
            ),
            _ => Err(Stop::Forbidden),
        };
    }
    if !policy.port_allowed(port) {
        return Err(Stop::Forbidden);
    }
    // Decided before anything is resolved or dialled: a refused host is never contacted.
    let answer = egress_decision(apid, cid, &host, port);
    if !answer.allow {
        println!(
            "vsockd: egress refused cid={cid} host={host} port={port}: {}",
            answer.reason
        );
        return Err(Stop::Refused(answer.reason));
    }
    let addrs = checked_addrs(policy, &host, port, &answer.deny_cidrs)?;
    audit.allowed = true;

    // A CONNECT apid wants intercepted: the guest is answered, then its TLS is terminated and each
    // request is injected (see `egress_tls`). A failure after the answer cannot be an HTTP answer,
    // so the connection is closed and the reason logged.
    if let (true, Head::Connect { .. }) = (answer.intercept, &parsed) {
        guest
            .write_all(ESTABLISHED)
            .map_err(|_| Stop::Unreachable)?;
        audit.intercepted = true;
        let target = egress_tls::Target {
            cid,
            host: &host,
            port,
            addrs: &addrs,
        };
        let mut tally = egress_tls::Tally::default();
        let result = egress_tls::intercept(guest, &target, apid, &mut tally);
        audit.up = tally.up;
        audit.down = tally.down;
        audit.injected = tally.injected;
        if let Err(reason) = result {
            println!("vsockd: egress intercept cid={cid} host={host} port={port} closed: {reason}");
        }
        return Ok(());
    }

    let mut upstream = None;
    for addr in &addrs {
        if let Ok(stream) = TcpStream::connect_timeout(addr, CONNECT_TIMEOUT) {
            upstream = Some(stream);
            break;
        }
    }
    let upstream = upstream.ok_or(Stop::Unreachable)?;
    guest
        .set_read_timeout(Some(PUMP_POLL))
        .map_err(|_| Stop::Unreachable)?;
    upstream
        .set_read_timeout(Some(PUMP_POLL))
        .map_err(|_| Stop::Unreachable)?;

    // Every clone is made before anything is written, so a failure here still
    // leaves the guest an error answer rather than half a tunnel.
    let guest_read = guest.try_clone().map_err(|_| Stop::Unreachable)?;
    let guest_write = guest.try_clone().map_err(|_| Stop::Unreachable)?;
    let hangup = Arc::new(
        Hangup::new(guest.try_clone().map_err(|_| Stop::Unreachable)?, upstream)
            .map_err(|_| Stop::Unreachable)?,
    );
    let mut upstream = &hangup.upstream;

    let mut sent = 0u64;
    let mut guest_write = guest_write;
    match parsed {
        Head::Connect { .. } => {
            let _ = guest_write.write_all(ESTABLISHED);
        }
        Head::Absolute { method, path, .. } => {
            let out = origin_head(&method, &path, &head);
            upstream
                .write_all(out.as_bytes())
                .map_err(|_| Stop::Unreachable)?;
            sent += out.len() as u64;
        }
    }
    if upstream.write_all(&rest).is_ok() {
        sent += rest.len() as u64;
    }

    let up_hangup = Arc::clone(&hangup);
    let up = thread::spawn(move || pump(&guest_read, &up_hangup.upstream, &up_hangup));
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
    let _slot = if upgrade {
        Some(UpgradeSlot::take(cid).ok_or(Stop::TooMany)?)
    } else {
        None
    };
    let token = apid.token().map_err(|_| Stop::Unreachable)?;
    let upstream = apid
        .connect(CONNECT_TIMEOUT)
        .map_err(|_| Stop::Unreachable)?;
    guest
        .set_read_timeout(Some(IDLE_TIMEOUT))
        .map_err(|_| Stop::Unreachable)?;
    upstream
        .set_read_timeout(Some(IDLE_TIMEOUT))
        .map_err(|_| Stop::Unreachable)?;

    let guest_read = guest.try_clone().map_err(|_| Stop::Unreachable)?;
    let mut guest_write = guest.try_clone().map_err(|_| Stop::Unreachable)?;
    let hangup = Arc::new(
        Hangup::new(guest.try_clone().map_err(|_| Stop::Unreachable)?, upstream)
            .map_err(|_| Stop::Unreachable)?,
    );
    let mut upstream = &hangup.upstream;

    let out = gateway_head(method, cid, path, head, &token, upgrade);
    upstream
        .write_all(out.as_bytes())
        .map_err(|_| Stop::Unreachable)?;
    let mut sent = out.len() as u64;
    if upstream.write_all(&rest).is_ok() {
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
        println!(
            "vsockd: egress websocket cid={cid} path={} upgraded",
            path.split('?').next().unwrap_or("")
        );
        guest
            .set_read_timeout(Some(WS_POLL))
            .map_err(|_| Stop::Unreachable)?;
        upstream
            .set_read_timeout(Some(WS_POLL))
            .map_err(|_| Stop::Unreachable)?;
        let clock = Arc::new(WsClock::new());
        let up_clock = Arc::clone(&clock);
        let up_hangup = Arc::clone(&hangup);
        let up =
            thread::spawn(move || pump_ws(guest_read, &up_hangup.upstream, &up_clock, &up_hangup));
        audit.down = answered + pump_ws(upstream, guest_write, &clock, &hangup);
        audit.up = sent + up.join().unwrap_or(0);
        return Ok(());
    }

    guest
        .set_read_timeout(Some(PUMP_POLL))
        .map_err(|_| Stop::Unreachable)?;
    upstream
        .set_read_timeout(Some(PUMP_POLL))
        .map_err(|_| Stop::Unreachable)?;
    let up_hangup = Arc::clone(&hangup);
    let up = thread::spawn(move || pump(&guest_read, &up_hangup.upstream, &up_hangup));
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
    for line in head
        .split("\r\n")
        .skip(1)
        .take_while(|line| !line.is_empty())
    {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "upgrade" => upgrade |= value.eq_ignore_ascii_case("websocket"),
            "connection" => {
                connection |= value
                    .split(',')
                    .any(|item| item.trim().eq_ignore_ascii_case("upgrade"))
            }
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
        let mut open = UPGRADES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let mut open = UPGRADES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        Self {
            start: Instant::now(),
            last_ms: AtomicU64::new(0),
        }
    }

    fn touch(&self) {
        self.last_ms
            .store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
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
fn gateway_head(
    method: &str,
    cid: u32,
    path: &str,
    head: &str,
    token: &str,
    upgrade: bool,
) -> String {
    let lines: Vec<&str> = head
        .split("\r\n")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .collect();
    let listed: Vec<String> = lines
        .iter()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| {
            value
                .split(',')
                .map(|item| item.trim().to_ascii_lowercase())
        })
        .filter(|item| !(upgrade && item == "upgrade"))
        .collect();
    let mut out =
        format!("{method} /internal/v1/agents/{cid}/gateway{path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
    for line in lines {
        let name = line
            .split_once(':')
            .map_or(String::new(), |(name, _)| name.trim().to_ascii_lowercase());
        let kept_upgrade = upgrade && name == "upgrade";
        if (GATEWAY_DROPPED.contains(&name.as_str()) && !kept_upgrade) || listed.contains(&name) {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    let connection = if upgrade { "Upgrade" } else { "close" };
    out.push_str(&format!(
        "{TOKEN_HEADER}: {token}\r\nConnection: {connection}\r\n\r\n"
    ));
    out
}

/// Copies `from` to `to` until either side closes, this direction has been
/// idle for [`IDLE_TIMEOUT`], or the other direction has ended; then hangs
/// both up so the other direction ends too. The pump sleeps in a readiness
/// wait on `from` and the tunnel's waker, which the other direction's hang-up
/// wakes, so an idle tunnel costs no wakeups and an ended one stops at once.
/// `from`'s [`PUMP_POLL`] read timeout stays only as a backstop. Returns the
/// bytes copied.
fn pump(from: impl Read + Source, mut to: impl Write, hangup: &Arc<Hangup>) -> u64 {
    use std::io::ErrorKind::{Interrupted, TimedOut, WouldBlock};
    let mut from = from;
    let mut buf = vec![0u8; COPY_BUF];
    let mut total = 0u64;
    let mut last = Instant::now();
    while !hangup.is_done() {
        let Some(left) = IDLE_TIMEOUT.checked_sub(last.elapsed()) else {
            break;
        };
        match crate::sys::wait_readable(&[from.raw(), hangup.waker.handle()], Some(left)) {
            Ok(ready) if !ready[0] => continue,
            Ok(_) => {}
            Err(_) => break,
        }
        if hangup.is_done() {
            break;
        }
        match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
                total += n as u64;
                last = Instant::now();
            }
            Err(err) if matches!(err.kind(), WouldBlock | TimedOut | Interrupted) => {}
            Err(_) => break,
        }
    }
    hangup.hang_up();
    total
}

/// A socket a plain pump can wait on: its raw handle for [`crate::sys::wait_readable`].
trait Source {
    fn raw(&self) -> crate::sys::RawHandle;
}

#[cfg(unix)]
impl<T: std::os::fd::AsRawFd> Source for &T {
    fn raw(&self) -> crate::sys::RawHandle {
        self.as_raw_fd()
    }
}

#[cfg(windows)]
impl<T: std::os::windows::io::AsRawSocket> Source for &T {
    fn raw(&self) -> crate::sys::RawHandle {
        self.as_raw_socket()
    }
}

/// Reads the request head through its blank line; returns it with any bytes
/// that followed it.
fn read_head(guest: &mut impl Read) -> Result<(String, Vec<u8>), Stop> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(end) = buf
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|at| at + 4)
        {
            if end > MAX_HEAD {
                return Err(Stop::Bad);
            }
            let rest = buf.split_off(end);
            return String::from_utf8(buf)
                .map(|head| (head, rest))
                .map_err(|_| Stop::Bad);
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
    if !version.starts_with("HTTP/1.")
        || method.is_empty()
        || !method.bytes().all(|b| b.is_ascii_uppercase())
    {
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
    Ok(Head::Absolute {
        method: method.to_string(),
        host,
        port,
        path: path.to_string(),
    })
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
    for line in head
        .split("\r\n")
        .skip(1)
        .take_while(|line| !line.is_empty())
    {
        if !line.to_ascii_lowercase().starts_with("proxy-connection:") {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    out.push_str("\r\n");
    out
}

/// Apid's decision for a connection of agent `cid` to `host:port`: the kept one while it is fresh,
/// else apid's answer, which is then kept ([`ALLOW_TTL`] or [`DENY_TTL`]). An apid that cannot
/// answer refuses (fail closed), and the refusal is logged and kept like any other.
fn egress_decision(apid: &Apid, cid: u32, host: &str, port: u16) -> EgressAnswer {
    let key = (cid, host.to_ascii_lowercase(), port);
    {
        let kept = DECISIONS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((expires, answer)) = kept.get(&key) {
            if Instant::now() < *expires {
                return answer.clone();
            }
        }
    }
    // Read before apid is asked: a `forget` that runs meanwhile moves the generation, so the answer is not kept.
    let seen = generation(cid);
    let answer = apid.check_egress(cid, host, port).unwrap_or_else(|err| {
        println!("vsockd: warning: egress check for cid {cid} failed, refusing: {err}");
        EgressAnswer {
            allow: false,
            reason: "refused: apid could not decide".to_string(),
            intercept: false,
            deny_cidrs: Vec::new(),
        }
    });
    let ttl = if answer.allow { ALLOW_TTL } else { DENY_TTL };
    keep_decision(key, seen, &answer, ttl);
    answer
}

/// Keeps `answer` for `key` for `ttl`. It is not kept when the CID's generation has moved since `seen`
/// (a `forget` ran while apid was asked), or when the CID already holds [`MAX_DECISIONS_PER_CID`] live
/// decisions, or when the table is full.
fn keep_decision(key: (u32, String, u16), seen: u64, answer: &EgressAnswer, ttl: Duration) {
    let now = Instant::now();
    let cid = key.0;
    let mut kept = DECISIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    kept.retain(|_, (expires, _)| *expires > now);
    let held = kept.keys().filter(|held| held.0 == cid).count();
    if generation(cid) == seen && held < MAX_DECISIONS_PER_CID && kept.len() < MAX_DECISIONS {
        kept.insert(key, (now + ttl, answer.clone()));
    }
}

/// Resolves `host:port` and checks every address. One refused address refuses
/// the destination. The returned addresses are the ones that were checked, and
/// they are the only ones connected to.
fn checked_addrs(
    policy: &Policy,
    host: &str,
    port: u16,
    deny: &[(IpAddr, u8)],
) -> Result<Vec<SocketAddr>, Stop> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|_| Stop::Unreachable)?
        .collect();
    if addrs.is_empty() {
        return Err(Stop::Unreachable);
    }
    let refused = addrs.iter().any(|addr| {
        policy.refusal(addr.ip()).is_some() || deny.iter().any(|block| in_cidr(addr.ip(), *block))
    });
    if refused {
        return Err(Stop::Forbidden);
    }
    Ok(addrs)
}

/// Whether `ip` lies in the deny block `block` (see `apid::cidr_of`). An IPv4-mapped IPv6 address is
/// compared as its IPv4 address, and an IPv4 address never matches an IPv6 block, or the reverse.
fn in_cidr(ip: IpAddr, (net, len): (IpAddr, u8)) -> bool {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    };
    match (ip, net) {
        (IpAddr::V4(ip), IpAddr::V4(net)) => {
            let mask = u32::MAX
                .checked_shl(32u32.saturating_sub(u32::from(len)))
                .unwrap_or(0);
            u32::from(ip) & mask == u32::from(net) & mask
        }
        (IpAddr::V6(ip), IpAddr::V6(net)) => {
            let mask = u128::MAX
                .checked_shl(128u32.saturating_sub(u32::from(len)))
                .unwrap_or(0);
            u128::from(ip) & mask == u128::from(net) & mask
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn the_policy_refuses_non_public_addresses_and_unlisted_ports() {
        let policy = Policy::new(vec![8080]);
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "172.31.255.1",
            "192.168.1.1",
            "169.254.10.10",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::",
            "fe80::1",
            "fc00::5",
            "fd12::5",
            "::ffff:10.0.0.5",
        ] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(policy.refusal(ip).is_some(), "{ip} must be refused");
        }
        for ip in [
            "93.184.216.34",
            "172.32.0.1",
            "8.8.8.8",
            "2606:2800:220:1::1",
        ] {
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
            Ok(Head::Connect {
                host: "example.com".into(),
                port: 443
            })
        );
        assert_eq!(
            parse_head("CONNECT [2606:2800::1]:8443 HTTP/1.1\r\n\r\n"),
            Ok(Head::Connect {
                host: "2606:2800::1".into(),
                port: 8443
            })
        );
        assert_eq!(
            parse_head("GET http://example.com/a?b=1 HTTP/1.1\r\nHost: example.com\r\n\r\n"),
            Ok(Head::Absolute {
                method: "GET".into(),
                host: "example.com".into(),
                port: 80,
                path: "/a?b=1".into()
            })
        );
        assert_eq!(
            parse_head("POST HTTP://Example.com:8080 HTTP/1.1\r\n\r\n"),
            Ok(Head::Absolute {
                method: "POST".into(),
                host: "Example.com".into(),
                port: 8080,
                path: "/".into()
            })
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
            origin_head(
                "GET",
                "/a",
                "GET http://x/a HTTP/1.1\r\nHost: x\r\nProxy-Connection: keep-alive\r\n\r\n"
            ),
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
    fn the_gateway_refuses_connect_other_ports_and_foreign_upgrades() {
        // The original test: CONNECT to the gateway and plain requests to another gateway port
        // are refused with 403, and the audit records the gateway host as refused.
        for request in [
            "CONNECT plaza.gateway:443 HTTP/1.1\r\n\r\n",
            "CONNECT Plaza.Gateway:80 HTTP/1.1\r\n\r\n",
            "GET http://plaza.gateway:8080/v1/x HTTP/1.1\r\n\r\n",
        ] {
            let (mut client, server) = UnixStream::pair().unwrap();
            let policy = Policy::new(vec![]);
            let proxy = thread::spawn(move || serve_conn(server, 7, &policy, &Apid::off()));
            client.write_all(request.as_bytes()).unwrap();
            let answer =
                String::from_utf8(crate::read_until_closed(&client, Duration::from_secs(20)))
                    .unwrap();
            assert!(
                answer.starts_with("HTTP/1.1 403 Forbidden\r\n"),
                "{request:?}: {answer:?}"
            );
            let audit = proxy.join().unwrap();
            assert!(
                !audit.allowed && audit.host.eq_ignore_ascii_case(GATEWAY_HOST),
                "{audit:?}"
            );
        }

        // The original test: an upgrade to another path is refused without contacting apid.
        {
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
            let answer =
                String::from_utf8(crate::read_until_closed(&client, Duration::from_secs(20)))
                    .unwrap();
            assert!(
                answer.starts_with("HTTP/1.1 403 Forbidden\r\n"),
                "{answer:?}"
            );
            let audit = proxy.join().unwrap();
            assert!(!audit.allowed, "{audit:?}");
        }
    }

    #[test]
    fn a_gateway_request_is_relayed_to_apid_and_its_answer_comes_back_intact() {
        const ANSWER: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 11\r\n\r\nhello world";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token_file =
            std::env::temp_dir().join(format!("vsockd-gateway-test-{}", std::process::id()));
        std::fs::write(&token_file, "test-token\n").unwrap();
        let apid = Apid::new(
            &format!("http://127.0.0.1:{port}"),
            Some(token_file.clone()),
        )
        .unwrap();

        // The stand-in for apid: checks the rewritten head and the body, then answers.
        let stand_in = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let (head, mut body) = read_head(&mut conn).unwrap();
            assert!(
                head.starts_with("POST /internal/v1/agents/7/gateway/v1/messages HTTP/1.1\r\n"),
                "{head}"
            );
            assert!(
                head.contains("\r\nx-apz-internal: test-token\r\n"),
                "{head}"
            );
            assert!(
                !head.to_ascii_lowercase().contains("authorization"),
                "{head}"
            );
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
        // Every wait is bounded and reads a known length: on Windows a read already blocked on an
        // AF_UNIX socket is not reliably woken when the peer closes, so waiting for the end of the
        // stream could hang where the relay itself is done.
        client
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let policy = Policy::new(vec![]);
        let (finished, proxy) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = finished.send(serve_conn(server, 7, &policy, &apid));
        });
        client
            .write_all(
                b"POST http://plaza.gateway/v1/messages HTTP/1.1\r\nHost: plaza.gateway\r\n\
                  Authorization: Bearer guest\r\nContent-Length: 4\r\n\r\nping",
            )
            .unwrap();
        let mut answer = vec![0u8; ANSWER.len()];
        client
            .read_exact(&mut answer)
            .expect("apid's answer comes back");
        assert_eq!(answer, ANSWER);

        stand_in.join().unwrap();
        let audit = proxy
            .recv_timeout(Duration::from_secs(20))
            .expect("the relay ends once apid has answered");
        let _ = std::fs::remove_file(&token_file);
        assert!(
            audit.allowed && audit.host == GATEWAY_HOST && audit.port == 80,
            "{audit:?}"
        );
    }

    #[test]
    fn a_websocket_upgrade_to_a_model_path_is_relayed_and_bytes_flow_both_ways() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token_file =
            std::env::temp_dir().join(format!("vsockd-upgrade-test-{}", std::process::id()));
        std::fs::write(&token_file, "test-token\n").unwrap();
        let apid = Apid::new(
            &format!("http://127.0.0.1:{port}"),
            Some(token_file.clone()),
        )
        .unwrap();

        // The stand-in for apid: checks the rewritten upgrade head, switches protocols,
        // then echoes one message the other way.
        let stand_in = thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let (head, _) = read_head(&mut conn).unwrap();
            assert!(
                head.starts_with(
                    "GET /internal/v1/agents/7/gateway/openai/v1/responses HTTP/1.1\r\n"
                ),
                "{head}"
            );
            assert!(
                head.contains("\r\nx-apz-internal: test-token\r\n"),
                "{head}"
            );
            assert!(head.contains("\r\nUpgrade: websocket\r\n"), "{head}");
            assert!(head.contains("\r\nConnection: Upgrade\r\n"), "{head}");
            assert!(
                !head.to_ascii_lowercase().contains("authorization"),
                "{head}"
            );
            conn.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
            let mut ping = [0u8; 4];
            conn.read_exact(&mut ping).unwrap();
            assert_eq!(&ping, b"ping");
            conn.write_all(b"pong").unwrap();
        });

        let (mut client, server) = UnixStream::pair().unwrap();
        // Bounded waits of known lengths, as in the plain gateway test.
        client
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let policy = Policy::new(vec![]);
        let (finished, proxy) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = finished.send(serve_conn(server, 7, &policy, &apid));
        });
        client
            .write_all(
                b"GET http://plaza.gateway/openai/v1/responses HTTP/1.1\r\nHost: plaza.gateway\r\n\
                  Authorization: Bearer guest\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\n\
                  Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
            )
            .unwrap();
        let (switching, early) = read_head(&mut client).unwrap();
        assert!(switching.starts_with("HTTP/1.1 101 "), "{switching}");
        assert!(
            early.is_empty(),
            "nothing follows the 101 before the guest speaks"
        );
        client.write_all(b"ping").unwrap();
        let mut pong = [0u8; 4];
        client
            .read_exact(&mut pong)
            .expect("the pong comes back through the socket");
        assert_eq!(&pong, b"pong");

        stand_in.join().unwrap();
        let audit = proxy
            .recv_timeout(Duration::from_secs(20))
            .expect("the relay ends once apid has hung up");
        let _ = std::fs::remove_file(&token_file);
        assert!(
            audit.allowed && audit.host == GATEWAY_HOST && audit.port == 80,
            "{audit:?}"
        );
    }

    #[test]
    fn a_fifth_upgraded_socket_for_one_agent_is_refused_with_429() {
        const CID: u32 = 9_001;
        let held: Vec<UpgradeSlot> = (0..MAX_UPGRADES_PER_CID)
            .map(|_| UpgradeSlot::take(CID).unwrap())
            .collect();
        assert!(
            UpgradeSlot::take(CID).is_none(),
            "the fifth socket must be refused"
        );
        assert!(UpgradeSlot::take(CID + 1).is_some(), "the cap is per agent");
        drop(held);
        assert!(
            UpgradeSlot::take(CID).is_some(),
            "a closed socket frees its slot"
        );
        assert!(Stop::TooMany.answer().starts_with(b"HTTP/1.1 429 "));
    }

    /// The egress check's test token, and a stand-in for apid's `/internal/egress/check`. Every
    /// check that carries the token is answered by `decide` on its JSON body and counted. The
    /// responder serves until the test process ends. Returns apid and the token file (the test
    /// removes it).
    fn fake_apid(
        name: &str,
        decide: impl Fn(&serde_json::Value) -> serde_json::Value + Send + 'static,
    ) -> (Apid, Arc<AtomicUsize>, PathBuf) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token_file =
            std::env::temp_dir().join(format!("vsockd-egress-check-{name}-{}", std::process::id()));
        std::fs::write(&token_file, "test-token\n").unwrap();
        let apid = Apid::new(
            &format!("http://127.0.0.1:{port}"),
            Some(token_file.clone()),
        )
        .unwrap();
        let checks = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&checks);
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                let _ = conn.set_read_timeout(Some(Duration::from_secs(20)));
                let Ok((head, mut body)) = read_head(&mut conn) else {
                    continue;
                };
                let length = head
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while body.len() < length {
                    let mut chunk = [0u8; 256];
                    match conn.read(&mut chunk) {
                        Ok(n) if n > 0 => body.extend_from_slice(&chunk[..n]),
                        _ => break,
                    }
                }
                let trusted = head
                    .to_ascii_lowercase()
                    .contains("x-apz-internal: test-token");
                let (status, reply) = if trusted
                    && head.starts_with("POST /internal/egress/check HTTP/1.1\r\n")
                {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let request: serde_json::Value =
                        serde_json::from_slice(&body[..length.min(body.len())]).unwrap_or_default();
                    (200, decide(&request).to_string())
                } else {
                    (403, "{}".to_string())
                };
                let out = format!(
                    "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    if status == 200 { "OK" } else { "Forbidden" },
                    reply.len()
                );
                let _ = conn.write_all(out.as_bytes());
            }
        });
        (apid, checks, token_file)
    }

    /// A loopback listener nobody accepts from. A connect to it succeeds (it waits in the backlog),
    /// and an `accept` on it shows whether anything dialled it.
    fn quiet_listener() -> (TcpListener, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        (listener, port)
    }

    /// A policy under which the loopback port `port` may be reached (test only), and nothing else
    /// is refused by the policy, so only apid can refuse a connection to it.
    fn loopback_policy(port: u16) -> Policy {
        Policy {
            allow_loopback: true,
            ..Policy::new(vec![port])
        }
    }

    fn connect_request(host: &str, port: u16) -> String {
        format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n")
    }

    /// Serves one guest connection through [`serve_conn`]: writes `request`, and returns what the
    /// guest read until the proxy closed the connection, with the audit. Every wait is bounded.
    fn serve_one(cid: u32, policy: Policy, apid: &Apid, request: &str) -> (Vec<u8>, Audit) {
        let (mut client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let apid = apid.clone();
        let (finished, proxy) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = finished.send(serve_conn(server, cid, &policy, &apid));
        });
        client.write_all(request.as_bytes()).unwrap();
        let answer = crate::read_until_closed(&client, Duration::from_secs(20));
        let audit = proxy
            .recv_timeout(Duration::from_secs(20))
            .expect("the proxy ends once it has answered");
        (answer, audit)
    }

    /// One allowed CONNECT tunnel to `127.0.0.1:port` through [`serve_conn`]: established, closed
    /// by the guest, and audited. Every wait is bounded.
    fn tunnel(cid: u32, policy: Policy, apid: &Apid, port: u16) -> Audit {
        let (mut client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let apid = apid.clone();
        let (finished, proxy) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = finished.send(serve_conn(server, cid, &policy, &apid));
        });
        client
            .write_all(connect_request("127.0.0.1", port).as_bytes())
            .unwrap();
        let mut answer = vec![0u8; ESTABLISHED.len()];
        client
            .read_exact(&mut answer)
            .expect("the tunnel is established");
        assert_eq!(answer, ESTABLISHED);
        client.shutdown(Shutdown::Write).unwrap();
        proxy
            .recv_timeout(Duration::from_secs(20))
            .expect("the tunnel ends once the guest closed its side")
    }

    #[test]
    fn a_connection_the_check_refuses_gets_a_403_naming_the_law_and_never_dials() {
        let (echo, port) = quiet_listener();
        let (apid, checks, token) = fake_apid("refused", |_| {
            json!({
                "allow": false,
                "law": "S-013d",
                "reason": "refused by law S-013d: Researchers never write to repositories",
            })
        });
        let (answer, audit) = serve_one(
            7_002,
            loopback_policy(port),
            &apid,
            &connect_request("127.0.0.1", port),
        );
        let _ = std::fs::remove_file(&token);
        let answer = String::from_utf8(answer).unwrap();
        assert!(
            answer.starts_with("HTTP/1.1 403 Forbidden\r\n") && answer.contains("S-013d"),
            "{answer:?}"
        );
        assert!(!audit.allowed, "{audit:?}");
        assert_eq!(
            checks.load(Ordering::SeqCst),
            1,
            "the connection was checked once"
        );
        assert_eq!(
            echo.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "the refused host was dialled"
        );
    }

    #[test]
    fn a_kept_decision_spares_apid_a_second_check() {
        let (_echo, port) = quiet_listener();
        let (apid, checks, token) = fake_apid("cached", |body| {
            let allow = body["host"] == "127.0.0.1";
            json!({ "allow": allow, "reason": "refused: on the fetch blocklist" })
        });
        assert!(tunnel(7_003, loopback_policy(port), &apid, port).allowed);
        assert!(tunnel(7_003, loopback_policy(port), &apid, port).allowed);
        for _ in 0..2 {
            let (answer, audit) = serve_one(
                7_003,
                Policy::new(vec![]),
                &apid,
                &connect_request("denied.example", 443),
            );
            assert!(
                String::from_utf8(answer)
                    .unwrap()
                    .starts_with("HTTP/1.1 403 Forbidden\r\n")
            );
            assert!(!audit.allowed);
        }
        let _ = std::fs::remove_file(&token);
        assert_eq!(
            checks.load(Ordering::SeqCst),
            2,
            "one check for the allowed host and one for the refused, both kept"
        );
    }

    #[test]
    fn an_apid_that_cannot_be_reached_refuses_the_connection() {
        let (echo, port) = quiet_listener();
        let dead = TcpListener::bind("127.0.0.1:0").unwrap();
        let dead_port = dead.local_addr().unwrap().port();
        drop(dead);
        let token =
            std::env::temp_dir().join(format!("vsockd-egress-check-down-{}", std::process::id()));
        std::fs::write(&token, "test-token\n").unwrap();
        let apid = Apid::new(
            &format!("http://127.0.0.1:{dead_port}"),
            Some(token.clone()),
        )
        .unwrap();
        let (answer, audit) = serve_one(
            7_004,
            loopback_policy(port),
            &apid,
            &connect_request("127.0.0.1", port),
        );
        let _ = std::fs::remove_file(&token);
        let answer = String::from_utf8(answer).unwrap();
        assert!(
            answer.starts_with("HTTP/1.1 403 Forbidden\r\n") && answer.contains("could not decide"),
            "{answer:?}"
        );
        assert!(!audit.allowed, "{audit:?}");
        assert_eq!(
            echo.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "the host was dialled without apid"
        );
    }

    #[test]
    fn a_deny_block_refuses_the_address_a_name_resolves_to_without_dialling() {
        let (echo, port) = quiet_listener();
        let (apid, _checks, token) = fake_apid(
            "deny-block",
            |_| json!({ "allow": true, "deny_cidrs": ["127.0.0.0/8"] }),
        );
        let (answer, audit) = serve_one(
            7_005,
            loopback_policy(port),
            &apid,
            &connect_request("127.0.0.1", port),
        );
        let _ = std::fs::remove_file(&token);
        let answer = String::from_utf8(answer).unwrap();
        assert!(
            answer.starts_with("HTTP/1.1 403 Forbidden\r\n"),
            "{answer:?}"
        );
        assert!(!audit.allowed, "{audit:?}");
        assert_eq!(
            echo.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "the denied address was dialled"
        );
    }

    #[test]
    fn a_deny_block_matches_by_prefix_and_never_across_address_families() {
        let block = |text: &str, len: u8| (text.parse::<IpAddr>().unwrap(), len);
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        let v4 = block("8.8.8.0", 24);
        let v6 = block("2001:db8::", 32);
        assert!(in_cidr(ip("8.8.8.8"), v4));
        assert!(
            in_cidr(ip("::ffff:8.8.8.8"), v4),
            "a mapped address is its IPv4 address"
        );
        assert!(!in_cidr(ip("8.8.9.8"), v4));
        assert!(in_cidr(ip("2001:db8::1"), v6));
        assert!(!in_cidr(ip("2001:db9::1"), v6));
        assert!(
            !in_cidr(ip("8.8.8.8"), v6),
            "an IPv4 address never matches an IPv6 block"
        );
        assert!(!in_cidr(ip("2001:db8::1"), v4));
        assert!(
            !in_cidr(ip("::ffff:8.8.8.8"), block("::", 0)),
            "a mapped address is IPv4, so no IPv6 block holds it"
        );
        assert!(in_cidr(ip("1.2.3.4"), block("0.0.0.0", 0)));
    }

    #[test]
    fn a_decision_asked_across_a_forget_is_not_kept() {
        const CID: u32 = 9_301;
        let key = (CID, "example.test".to_string(), 443);
        let answer = EgressAnswer {
            allow: true,
            reason: String::new(),
            intercept: false,
            deny_cidrs: Vec::new(),
        };
        let seen = generation(CID);
        forget(CID);
        assert_ne!(generation(CID), seen, "a forget moves the generation");
        keep_decision(key.clone(), seen, &answer, ALLOW_TTL);
        assert!(
            !DECISIONS.lock().unwrap().contains_key(&key),
            "a decision that straddled a forget was kept"
        );
        keep_decision(key.clone(), generation(CID), &answer, ALLOW_TTL);
        assert!(
            DECISIONS.lock().unwrap().contains_key(&key),
            "a decision asked after the forget is kept"
        );
        forget(CID);
    }

    #[test]
    fn one_cid_keeps_at_most_512_decisions_and_others_are_unaffected() {
        const CID: u32 = 9_302;
        let answer = EgressAnswer {
            allow: true,
            reason: String::new(),
            intercept: false,
            deny_cidrs: Vec::new(),
        };
        for n in 0..=MAX_DECISIONS_PER_CID {
            keep_decision(
                (CID, format!("h{n}.test"), 443),
                generation(CID),
                &answer,
                ALLOW_TTL,
            );
        }
        let held = |cid: u32| {
            DECISIONS
                .lock()
                .unwrap()
                .keys()
                .filter(|key| key.0 == cid)
                .count()
        };
        assert_eq!(held(CID), MAX_DECISIONS_PER_CID, "the 513th was kept");
        keep_decision(
            (CID + 1, "other.test".to_string(), 443),
            generation(CID + 1),
            &answer,
            ALLOW_TTL,
        );
        assert_eq!(held(CID + 1), 1, "another CID's decision was refused");
        forget(CID);
        forget(CID + 1);
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
        let policy = Policy {
            allow_loopback: true,
            ..Policy::new(vec![port])
        };
        let (mut client, server) = UnixStream::pair().unwrap();
        // Every wait is bounded, so a stall fails at the step that stalled
        // instead of hanging the test.
        client
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let (apid, checks, token) = fake_apid("e2e", |_| json!({ "allow": true }));
        let (finished, proxy) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = finished.send(serve_conn(server, 7, &policy, &apid));
        });

        client
            .write_all(
                format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
                    .as_bytes(),
            )
            .unwrap();
        let mut answer = vec![0u8; ESTABLISHED.len()];
        client
            .read_exact(&mut answer)
            .expect("the tunnel is established");
        assert_eq!(answer, ESTABLISHED);

        let payload = b"ping through the tunnel";
        client.write_all(payload).unwrap();
        let mut echoed = vec![0u8; payload.len()];
        client
            .read_exact(&mut echoed)
            .expect("the payload comes back through the tunnel");
        assert_eq!(&echoed[..], &payload[..]);

        client.shutdown(Shutdown::Write).unwrap();
        let audit = proxy
            .recv_timeout(Duration::from_secs(20))
            .expect("the relay ends once the guest closed its side");
        let _ = std::fs::remove_file(&token);
        assert_eq!(
            checks.load(Ordering::SeqCst),
            1,
            "the connection was checked once"
        );
        assert!(
            audit.allowed
                && audit.port == port
                && audit.up == payload.len() as u64
                && audit.down == payload.len() as u64,
            "{audit:?}"
        );
    }
}
