//! The egress proxy for stdio containers (docs/design/mcp.md, "Running a server").
//!
//! MCP containers sit on the internal network `apz-mcp`, which has no route out. Their proxy address is a relay
//! container (`relay.rs`), attached to `apz-mcp` and to a normal network from which it reaches the host as
//! `host.docker.internal`. The relay is a pipe: it sends this proxy a session token and a PROXY protocol v1 line
//! with the container's address, then copies bytes both ways. The proxy listens on loopback (Docker Desktop
//! delivers `host.docker.internal` there) and, where the engine gives the bridge a host address, on the bridge
//! gateway too. It accepts a relay connection only with the current 256-bit token as its first line, and only with
//! a valid PROXY line; failed handshakes are logged and, after repeated failures, refused for a while. Then the
//! policy applies to the server the PROXY source address belongs to: the fetch blocklist always, and private,
//! loopback, link-local and metadata addresses refused unless the server's `allow_private` says so. A server that
//! ignores the proxy has no network at all.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use super::relay::{self, RELAY_LAN_IP, RelayProcess};
use super::stdio::{Docker, SystemDocker, ensure_network, read_line_capped};

/// The proxy's default port on the host (apid's range).
pub const DEFAULT_PORT: u16 = 19128;
/// The port the relay listens on, on its LAN address: the port in the containers' proxy variables.
pub const RELAY_PORT: u16 = 3128;
/// The longest request head the proxy reads.
const HEAD_CAP: usize = 16 << 10;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE: Duration = Duration::from_secs(300);
/// The first line the relay sends: the token of its current start.
const TOKEN_PREFIX: &str = "APZ-RELAY-TOKEN ";
/// The metadata service address (refused unless the server allows private hosts).
const METADATA_V4: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);
/// The first addresses of `apz-mcp` are not leased: the relay has the second.
const FIRST_LEASE: u8 = 3;

/// Binds the proxy on an address (the real bind, or a test's).
pub type Bind = Box<dyn Fn(Ipv4Addr) -> io::Result<TcpListener> + Send + Sync>;
/// The relay binary for a Docker architecture (`x86_64` or `aarch64`).
pub type RelayFor = Arc<dyn Fn(&str) -> PathBuf + Send + Sync>;

/// How relay handshakes are bounded (docs/design/mcp.md, "The proxy"). A valid token is always accepted, whatever
/// has failed before. A failed handshake is held until `tarpit` after it was accepted, then closed. Unauthenticated
/// connections are capped at `max_pending`: a new one beyond that is closed at once. The handshake's reads time out
/// after `timeout`, so a connection that sends nothing cannot hold a slot.
#[derive(Debug, Clone, Copy)]
pub struct Handshake {
    pub timeout: Duration,
    pub tarpit: Duration,
    pub max_pending: usize,
}

impl Default for Handshake {
    fn default() -> Self {
        Handshake {
            timeout: Duration::from_secs(2),
            tarpit: Duration::from_secs(2),
            max_pending: 16,
        }
    }
}

#[cfg(test)]
impl Handshake {
    /// The same bounds with a short tarpit, so tests with many failures stay fast.
    pub fn fast() -> Handshake {
        Handshake {
            tarpit: Duration::from_millis(10),
            ..Handshake::default()
        }
    }
}

/// A relay source that is the same binary for every architecture (tests).
#[cfg(test)]
pub fn relay_to(path: PathBuf) -> RelayFor {
    Arc::new(move |_: &str| path.clone())
}

/// An unauthenticated connection, open until it authenticates, is evicted, or is closed. The socket is kept so an
/// eviction can close it from outside its thread. It is the connection's own handle, shared with its thread, not a
/// `try_clone`: on Windows a clone's shutdown now and then fails with WSAENOTCONN, and the evicted peer would wait.
struct Pending {
    id: u64,
    socket: Arc<TcpStream>,
}

/// The window in which evictions after the first are counted, not logged.
const EVICTION_LOG_WINDOW: Duration = Duration::from_secs(10);

/// Rate-limits the eviction log line: the first eviction is logged at once, then at most one line per
/// `EVICTION_LOG_WINDOW`, which reports how many evictions were suppressed since the last line. The clock is the
/// caller's `now`, so the limit is testable without sleeping.
#[derive(Default)]
struct EvictionLog {
    last_logged: Option<Instant>,
    suppressed: u64,
}

impl EvictionLog {
    /// Records one eviction at `now`. Returns the line to log, or None when this eviction is suppressed.
    fn record(&mut self, now: Instant) -> Option<String> {
        if self
            .last_logged
            .is_some_and(|at| now.duration_since(at) < EVICTION_LOG_WINDOW)
        {
            self.suppressed += 1;
            return None;
        }
        let line = match self.suppressed {
            0 => "closed the oldest unauthenticated relay connection to make room".to_string(),
            n => format!(
                "closed the oldest unauthenticated relay connection to make room; {n} more since the last log line"
            ),
        };
        self.last_logged = Some(now);
        self.suppressed = 0;
        Some(line)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// What the egress proxy is built from. Tests set the parts that touch the host or docker.
pub struct Setup {
    pub docker: Arc<dyn Docker>,
    /// Binds the proxy on an address (on `port`).
    pub bind: Bind,
    /// The host port the relay connects to.
    pub port: u16,
    /// Where the relay binary is copied to (`<state>/mcp/mcp_relay`), so Docker can mount it.
    pub state_dir: PathBuf,
    /// Where the relay binary is found, for a Docker architecture.
    pub relay_for: RelayFor,
    /// The handshake's bounds.
    pub handshake: Handshake,
}

/// The server a source address belongs to.
struct Tenant {
    server: String,
    allow_private: bool,
}

/// The proxy and its address book. Shared by the listeners' threads and the runtime.
pub struct Egress {
    docker: Arc<dyn Docker>,
    bind: Bind,
    port: u16,
    state_dir: PathBuf,
    relay_for: RelayFor,
    handshake: Handshake,
    /// The unauthenticated connections open now, oldest first (the cap is `handshake.max_pending`).
    pending: Mutex<VecDeque<Pending>>,
    next_pending: AtomicU64,
    /// Whether the listeners are bound (they are bound once).
    bound: Mutex<bool>,
    /// The running relay, and the token it was started with.
    relay: Mutex<Option<RelayProcess>>,
    token: Mutex<Option<String>>,
    tenants: Mutex<BTreeMap<Ipv4Addr, Tenant>>,
    blocklist: RwLock<Vec<String>>,
    /// The verdicts already logged, so each host is logged once per server and verdict.
    logged: Mutex<BTreeSet<(String, String, bool)>>,
    /// The rate limit on the eviction log line (one relay, so one limit).
    eviction_log: Mutex<EvictionLog>,
}

/// A container's address on `apz-mcp`, held while its instance runs. Dropping it frees the address.
pub struct Lease {
    ip: Ipv4Addr,
    proxy: String,
    egress: Arc<Egress>,
}

impl Lease {
    pub fn ip(&self) -> Ipv4Addr {
        self.ip
    }

    /// The proxy URL the container uses: the relay's address on `apz-mcp`.
    pub fn proxy(&self) -> &str {
        &self.proxy
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        lock(&self.egress.tenants).remove(&self.ip);
    }
}

impl Egress {
    /// The production proxy: the real docker client, the real binds on loopback and the bridge gateway, and
    /// the relay binary found by `relay::find_source`.
    pub fn new(state_dir: PathBuf, port: u16) -> Arc<Egress> {
        Egress::with_setup(Setup {
            docker: Arc::new(SystemDocker),
            bind: Box::new(move |ip: Ipv4Addr| TcpListener::bind((ip, port))),
            port,
            state_dir,
            relay_for: Arc::new(|arch: &str| relay::find_source(arch)),
            handshake: Handshake::default(),
        })
    }

    pub fn with_setup(setup: Setup) -> Arc<Egress> {
        Arc::new(Egress {
            docker: setup.docker,
            bind: setup.bind,
            port: setup.port,
            state_dir: setup.state_dir,
            relay_for: setup.relay_for,
            handshake: setup.handshake,
            pending: Mutex::new(VecDeque::new()),
            next_pending: AtomicU64::new(0),
            bound: Mutex::new(false),
            relay: Mutex::new(None),
            token: Mutex::new(None),
            tenants: Mutex::new(BTreeMap::new()),
            blocklist: RwLock::new(Vec::new()),
            logged: Mutex::new(BTreeSet::new()),
            eviction_log: Mutex::new(EvictionLog::default()),
        })
    }

    /// Makes the networks, binds the proxy, and starts the relay if it is not running. Then gives `server` a free
    /// address on `apz-mcp`.
    pub fn attach(
        self: &Arc<Self>,
        server: &str,
        allow_private: bool,
        blocklist: &[String],
    ) -> Result<Lease, String> {
        self.ensure_listeners()?;
        self.set_blocklist(blocklist);
        self.ensure_relay()?;
        let ip = {
            let tenants = lock(&self.tenants);
            (FIRST_LEASE..=254)
                .map(|host| Ipv4Addr::new(172, 29, 250, host))
                .find(|ip| !tenants.contains_key(ip))
                .ok_or("no free address on the apz-mcp network")?
        };
        self.admit_at(ip, server, allow_private);
        Ok(Lease {
            ip,
            proxy: format!("http://{RELAY_LAN_IP}:{RELAY_PORT}"),
            egress: Arc::clone(self),
        })
    }

    /// Records that a container at `ip` belongs to `server`. The proxy answers that address from now on.
    pub fn admit_at(&self, ip: Ipv4Addr, server: &str, allow_private: bool) {
        lock(&self.tenants).insert(
            ip,
            Tenant {
                server: server.to_string(),
                allow_private,
            },
        );
    }

    /// The fetch blocklist the proxy applies (the settings' list, as the last attach saw it).
    pub fn set_blocklist(&self, blocklist: &[String]) {
        *write(&self.blocklist) = blocklist.to_vec();
    }

    /// Sets the token the relay must present (the relay's own start sets it; tests set it directly).
    pub fn set_token(&self, token: &str) {
        *lock(&self.token) = Some(token.to_string());
    }

    fn token_matches(&self, presented: &str) -> bool {
        match lock(&self.token).as_deref() {
            Some(expected) => apz_trust::ct_eq(expected.as_bytes(), presented.as_bytes()),
            None => false,
        }
    }

    fn tenant(&self, ip: Ipv4Addr) -> Option<(String, bool)> {
        lock(&self.tenants)
            .get(&ip)
            .map(|t| (t.server.clone(), t.allow_private))
    }

    /// Admits a connection as pending. When the cap is full, the oldest pending connection is closed to make room. A
    /// real relay authenticates within milliseconds, so it is never the oldest for long; an attacker holding slots is
    /// churned out instead of locking the relay out.
    fn admit_pending(&self, socket: Arc<TcpStream>) -> u64 {
        let id = self.next_pending.fetch_add(1, Ordering::SeqCst);
        let mut pending = lock(&self.pending);
        if pending.len() >= self.handshake.max_pending {
            if let Some(oldest) = pending.pop_front() {
                let _ = oldest.socket.shutdown(Shutdown::Both);
                if let Some(line) = lock(&self.eviction_log).record(Instant::now()) {
                    eprintln!("apid: mcp: {line}");
                }
            }
        }
        pending.push_back(Pending { id, socket });
        id
    }

    /// Takes a connection out of the pending set. False when it was evicted meanwhile.
    fn leave_pending(&self, id: u64) -> bool {
        let mut pending = lock(&self.pending);
        match pending.iter().position(|p| p.id == id) {
            Some(at) => {
                pending.remove(at);
                true
            }
            None => false,
        }
    }

    /// Binds the proxy on loopback, which Docker Desktop delivers `host.docker.internal` to, and on the bridge
    /// gateway where the engine gives it a host address. The loopback bind is required; the gateway one is not.
    fn ensure_listeners(self: &Arc<Self>) -> Result<(), String> {
        let mut bound = lock(&self.bound);
        if *bound {
            return Ok(());
        }
        let loopback = (self.bind)(Ipv4Addr::LOCALHOST).map_err(|_| {
            format!(
                "the egress proxy cannot listen on 127.0.0.1:{}; remote (http) servers still work",
                self.port
            )
        })?;
        let egress = Arc::clone(self);
        std::thread::spawn(move || serve(loopback, egress));
        if let Some(gateway) = bridge_gateway(self.docker.as_ref()) {
            match (self.bind)(gateway) {
                Ok(listener) => {
                    let egress = Arc::clone(self);
                    std::thread::spawn(move || serve(listener, egress));
                }
                Err(_) => eprintln!(
                    "apid: mcp: the proxy is on 127.0.0.1:{} only (the bridge gateway {gateway} is not on this host)",
                    self.port
                ),
            }
        }
        *bound = true;
        Ok(())
    }

    /// Makes the networks, then starts the relay if it is not running. Each start gets a new token, which the relay
    /// is given through its environment and must send first on every connection.
    fn ensure_relay(&self) -> Result<(), String> {
        let mut slot = lock(&self.relay);
        if let Some(process) = slot.as_mut() {
            if process.alive() {
                return Ok(());
            }
        }
        *slot = None;
        ensure_network(self.docker.as_ref())?;
        relay::ensure_uplink(self.docker.as_ref())?;
        let arch = relay::docker_arch(self.docker.as_ref())?;
        let installed = relay::install(&(self.relay_for)(arch), &self.state_dir)?;
        let token =
            apz_os::random::hex(32).map_err(|e| format!("cannot make a relay token: {e}"))?;
        self.set_token(&token);
        *slot = Some(relay::start(
            Arc::clone(&self.docker),
            &installed,
            &token,
            self.port,
        )?);
        Ok(())
    }

    /// Logs one verdict for a host. The host only: never a URL, a path or a query.
    fn log(&self, server: &str, host: &str, allowed: bool, why: &str) {
        if lock(&self.logged).insert((server.to_string(), host.to_string(), allowed)) {
            let verdict = if allowed { "allowed" } else { "refused" };
            let suffix = if why.is_empty() {
                String::new()
            } else {
                format!(" ({why})")
            };
            eprintln!("apid: mcp {server}: {verdict} {host}{suffix}");
        }
    }
}

/// The bridge gateway of Docker's default bridge: the address `host-gateway` names on a native Linux engine. None
/// when docker cannot say, which on Docker Desktop is normal.
pub fn bridge_gateway(docker: &dyn Docker) -> Option<Ipv4Addr> {
    let args: Vec<String> = [
        "network",
        "inspect",
        "bridge",
        "-f",
        "{{range .IPAM.Config}}{{.Gateway}}{{end}}",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let ran = docker.run(&args, &[]).ok()?;
    if !ran.ok {
        return None;
    }
    ran.stdout.trim().parse().ok()
}

/// The environment a stdio container gets to reach the proxy.
pub fn proxy_env(proxy: &str) -> Vec<(String, String)> {
    [
        ("HTTP_PROXY", proxy),
        ("HTTPS_PROXY", proxy),
        ("http_proxy", proxy),
        ("https_proxy", proxy),
        ("NO_PROXY", ""),
        ("NODE_USE_ENV_PROXY", "1"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// Accepts relay connections on a listener, one thread each. Each is admitted as pending, which may evict the oldest.
pub fn serve(listener: TcpListener, egress: Arc<Egress>) {
    for stream in listener.incoming().flatten() {
        let stream = Arc::new(stream);
        let id = egress.admit_pending(Arc::clone(&stream));
        let egress = Arc::clone(&egress);
        std::thread::spawn(move || relay_connection(stream, &egress, id));
    }
}

/// The PROXY protocol v1 line the relay sends: `PROXY TCP4 <src> <dst> <sport> <dport>`. Returns the source.
pub fn parse_proxy_v1(line: &str) -> Option<Ipv4Addr> {
    let mut fields = line.split(' ');
    if fields.next()? != "PROXY" || fields.next()? != "TCP4" {
        return None;
    }
    let source: Ipv4Addr = fields.next()?.parse().ok()?;
    fields.next()?.parse::<Ipv4Addr>().ok()?;
    fields.next()?.parse::<u16>().ok()?;
    fields.next()?.parse::<u16>().ok()?;
    fields.next().is_none().then_some(source)
}

/// One relay connection. A valid handshake leaves the pending set and serves the request. A failed one is logged
/// (never with the token), held until the tarpit time while it is still pending, and closed. An evicted connection
/// is closed, and never served.
fn relay_connection(stream: Arc<TcpStream>, egress: &Egress, id: u64) {
    let accepted = Instant::now();
    let peer = match stream.peer_addr() {
        Ok(SocketAddr::V4(peer)) => *peer.ip(),
        _ => Ipv4Addr::UNSPECIFIED,
    };
    let _ = stream.set_read_timeout(Some(egress.handshake.timeout));
    match handshake(&stream, egress) {
        Ok((reader, source)) => {
            if !egress.leave_pending(id) {
                return;
            }
            let _ = stream.set_read_timeout(Some(IDLE));
            serve_request(&stream, reader, source, egress);
        }
        Err(why) => {
            eprintln!("apid: mcp: relay handshake from {peer} refused: {why}");
            std::thread::sleep(egress.handshake.tarpit.saturating_sub(accepted.elapsed()));
            egress.leave_pending(id);
            drop(stream);
        }
    }
}

/// The handshake: the token line, then the PROXY line. Returns the reader (with anything read past the two lines)
/// and the source address.
fn handshake<'a>(
    stream: &'a TcpStream,
    egress: &Egress,
) -> Result<(BufReader<&'a TcpStream>, Ipv4Addr), &'static str> {
    let mut reader = BufReader::new(stream);
    let hello = read_line_capped(&mut reader, 256)
        .map_err(|_| "no token before the timeout")?
        .ok_or("no token")?;
    let token = hello.trim_end().strip_prefix(TOKEN_PREFIX).unwrap_or("");
    if token.is_empty() || !egress.token_matches(token) {
        return Err("the token is missing or wrong");
    }
    let proxy_line = read_line_capped(&mut reader, 128)
        .map_err(|_| "no PROXY line before the timeout")?
        .ok_or("no PROXY line")?;
    let source = parse_proxy_v1(proxy_line.trim_end()).ok_or("the PROXY line is not valid")?;
    Ok((reader, source))
}

fn respond(mut client: &TcpStream, code: u16, reason: &str, text: &str) {
    let reply = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    let _ = client.write_all(reply.as_bytes());
    let _ = client.shutdown(Shutdown::Write);
}

/// Reads a request head up to its blank line. `Err(None)`: the client went away. `Err(Some(431))`: too large.
fn read_head(reader: &mut impl BufRead) -> Result<Vec<String>, Option<u16>> {
    let mut head: Vec<String> = Vec::new();
    let mut total = 0;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return Err(None),
            Ok(n) => {
                total += n;
                if total > HEAD_CAP {
                    return Err(Some(431));
                }
                if line == "\r\n" || line == "\n" {
                    return Ok(head);
                }
                head.push(line.trim_end().to_string());
            }
        }
    }
}

/// The request for one client: the policy for the server `source` belongs to, then the tunnel or the request.
fn serve_request(
    mut client: &TcpStream,
    mut reader: BufReader<&TcpStream>,
    source: Ipv4Addr,
    egress: &Egress,
) {
    // The head is read before any answer, whoever the client is, so nothing is left unread on close.
    let head = match read_head(&mut reader) {
        Ok(head) => head,
        Err(None) => return,
        Err(Some(code)) => {
            return respond(client, code, "Request Header Fields Too Large", "");
        }
    };
    let Some((server, allow_private)) = egress.tenant(source) else {
        return respond(
            client,
            403,
            "Forbidden",
            "no network lease for this address",
        );
    };
    let parts: Vec<String> = head
        .first()
        .map(|l| l.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let [method, target, version] = parts.as_slice() else {
        return respond(
            client,
            400,
            "Bad Request",
            "a request line is method, target and version",
        );
    };
    let connect = method.eq_ignore_ascii_case("CONNECT");
    let (host, port, rewritten) = if connect {
        match split_host_port(target, 443) {
            Some((host, port)) => (host, port, None),
            None => return respond(client, 400, "Bad Request", "CONNECT needs host:port"),
        }
    } else {
        let Some(rest) = target.strip_prefix("http://") else {
            return respond(
                client,
                400,
                "Bad Request",
                "only http:// URLs and CONNECT are proxied",
            );
        };
        let (authority, path) = match rest.find('/') {
            Some(at) => (&rest[..at], &rest[at..]),
            None => (rest, "/"),
        };
        match split_host_port(authority, 80) {
            Some((host, port)) => (host, port, Some(format!("{method} {path} {version}"))),
            None => return respond(client, 400, "Bad Request", "bad host"),
        }
    };
    let blocklist = read(&egress.blocklist).clone();
    let addr = match decide(&host, port, allow_private, &blocklist) {
        Ok(addr) => {
            egress.log(&server, &host, true, "");
            addr
        }
        Err(why) => {
            egress.log(&server, &host, false, &why);
            return respond(client, 403, "Forbidden", &why);
        }
    };
    let Ok(upstream) = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) else {
        return respond(client, 502, "Bad Gateway", "cannot reach the host");
    };
    let _ = upstream.set_read_timeout(Some(IDLE));
    let leftover = reader.buffer().to_vec();
    let mut upstream_out = &upstream;
    match rewritten {
        None => {
            if client
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .is_err()
            {
                return;
            }
        }
        Some(first) => {
            let mut out = format!("{first}\r\n");
            for line in &head[1..] {
                out.push_str(line);
                out.push_str("\r\n");
            }
            out.push_str("\r\n");
            if upstream_out.write_all(out.as_bytes()).is_err() {
                return;
            }
        }
    }
    if !leftover.is_empty() && upstream_out.write_all(&leftover).is_err() {
        return;
    }
    pump(client, &upstream);
}

/// The decision for a connection to `host:port`: the address to connect to, or why not.
pub fn decide(
    host: &str,
    port: u16,
    allow_private: bool,
    blocklist: &[String],
) -> Result<SocketAddr, String> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let verdict = crate::internal::egress_verdict(
        crate::internal::on_fetch_blocklist(blocklist, host),
        None,
        host,
    );
    if !verdict.allow {
        return Err(verdict.reason.unwrap_or_else(|| format!("refused: {host}")));
    }
    let addresses: Vec<IpAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => (host, port)
            .to_socket_addrs()
            .map_err(|_| format!("refused: {host} does not resolve"))?
            .map(|addr| addr.ip())
            .collect(),
    };
    if addresses.is_empty() {
        return Err(format!("refused: {host} does not resolve"));
    }
    if !allow_private {
        if let Some((ip, why)) = addresses
            .iter()
            .find_map(|ip| refusal(*ip).map(|why| (*ip, why)))
        {
            return Err(format!(
                "refused: {host} is a {why} address ({ip}); set allow_private to reach it"
            ));
        }
    }
    Ok(SocketAddr::new(addresses[0], port))
}

/// Why an address is refused. Same classes as `vsockd`'s egress (`addr_refusal` in
/// crates/vsockd/src/egress.rs); a binary crate cannot be imported, so the table is mirrored here.
pub fn refusal(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => v4_refusal(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4_refusal(v4),
            None => v6_refusal(v6),
        },
    }
}

fn v4_refusal(ip: Ipv4Addr) -> Option<&'static str> {
    if ip == METADATA_V4 {
        Some("metadata")
    } else if ip.is_unspecified() {
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
    if ip.segments() == [0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254] {
        Some("metadata")
    } else if ip.is_unspecified() {
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

/// `host[:port]` or `[v6][:port]`, with a default port when none is written.
fn split_host_port(authority: &str, default: u16) -> Option<(String, u16)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        let port = match rest[end + 1..].strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None => default,
        };
        return Some((rest[..end].to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && !host.contains(':') => {
            Some((host.to_string(), port.parse().ok()?))
        }
        None if !authority.is_empty() => Some((authority.to_string(), default)),
        _ => None,
    }
}

/// Copies both ways until either side closes. Both directions use each socket's one handle (`&TcpStream` reads and
/// writes), not a `try_clone`: on Windows a clone is a `WSADuplicateSocketW` handle whose shutdown now and then fails
/// with WSAENOTCONN, so the client's half-close would never reach the upstream, which would wait for more request.
fn pump(client: &TcpStream, upstream: &TcpStream) {
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = io::copy(&mut &*client, &mut &*upstream);
            let _ = upstream.shutdown(Shutdown::Write);
        });
        let _ = io::copy(&mut &*upstream, &mut &*client);
        let _ = client.shutdown(Shutdown::Both);
        let _ = upstream.shutdown(Shutdown::Both);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn list(hosts: &[&str]) -> Vec<String> {
        hosts.iter().map(|h| h.to_string()).collect()
    }

    #[test]
    fn decide_applies_the_blocklist_and_the_private_address_rules() {
        // the_blocklist_refuses_even_when_private_is_allowed
        {
            let block = list(&["evil.example"]);
            assert!(
                decide("evil.example", 443, false, &block)
                    .unwrap_err()
                    .contains("blocklist")
            );
            assert!(
                decide("evil.example", 443, true, &block)
                    .unwrap_err()
                    .contains("blocklist")
            );
            assert!(decide("api.evil.example", 443, true, &block).is_err());
        }
        // private_loopback_link_local_and_metadata_are_refused_unless_allowed
        {
            for host in [
                "127.0.0.1",
                "10.0.0.5",
                "192.168.1.1",
                "169.254.169.254",
                "[fe80::1]",
                "[fd00:ec2::254]",
                "localhost",
                "0.0.0.0",
            ] {
                assert!(
                    decide(host, 80, false, &[]).is_err(),
                    "{host} must be refused"
                );
                assert!(
                    decide(host, 80, true, &[]).is_ok(),
                    "{host} is allowed with allow_private"
                );
            }
            assert!(
                decide("169.254.169.254", 80, false, &[])
                    .unwrap_err()
                    .contains("metadata")
            );
        }
        // a_public_address_is_allowed
        {
            assert_eq!(
                decide("8.8.8.8", 443, false, &[]).unwrap(),
                "8.8.8.8:443".parse().unwrap()
            );
            assert!(decide("[2001:4860:4860::8888]", 443, false, &[]).is_ok());
        }
    }

    #[test]
    fn authorities_split_into_host_and_port() {
        assert_eq!(
            split_host_port("example.com:8443", 443),
            Some(("example.com".into(), 8443))
        );
        assert_eq!(
            split_host_port("example.com", 443),
            Some(("example.com".into(), 443))
        );
        assert_eq!(split_host_port("[::1]:9", 443), Some(("::1".into(), 9)));
        assert_eq!(split_host_port("::1", 443), None);
        assert_eq!(split_host_port("host:notaport", 443), None);
    }

    #[test]
    fn containers_get_the_proxy_variables_and_an_empty_no_proxy() {
        let env = proxy_env("http://172.29.250.2:3128");
        let get = |k: &str| {
            env.iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(
            get("HTTPS_PROXY").as_deref(),
            Some("http://172.29.250.2:3128")
        );
        assert_eq!(
            get("https_proxy").as_deref(),
            Some("http://172.29.250.2:3128")
        );
        assert_eq!(get("NO_PROXY").as_deref(), Some(""));
        assert_eq!(get("NODE_USE_ENV_PROXY").as_deref(), Some("1"));
        assert_eq!(env.len(), 6);
    }

    #[test]
    fn the_proxy_line_parses_only_when_it_is_exact() {
        assert_eq!(
            parse_proxy_v1("PROXY TCP4 172.29.250.9 172.29.250.2 5000 3128"),
            Some(Ipv4Addr::new(172, 29, 250, 9))
        );
        assert_eq!(parse_proxy_v1("PROXY TCP6 ::1 ::1 1 2"), None);
        assert_eq!(
            parse_proxy_v1("PROXY TCP4 172.29.250.9 172.29.250.2 5000"),
            None
        );
        assert_eq!(
            parse_proxy_v1("PROXY TCP4 172.29.250.9 172.29.250.2 5000 3128 extra"),
            None
        );
        assert_eq!(parse_proxy_v1("GET / HTTP/1.1"), None);
        assert_eq!(
            parse_proxy_v1("PROXY TCP4 not-an-ip 172.29.250.2 1 2"),
            None
        );
    }

    /// A proxy that accepts relay connections on loopback, with the token `tok`.
    fn proxy(allow_private: bool, blocklist: &[&str]) -> (SocketAddr, Arc<Egress>) {
        let egress = test_egress(|_| panic!("no bind in this test"));
        egress.set_token("tok");
        egress.admit_at(Ipv4Addr::new(172, 29, 250, 9), "srv", allow_private);
        egress.set_blocklist(&list(blocklist));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let serving = Arc::clone(&egress);
        std::thread::spawn(move || serve(listener, serving));
        (addr, egress)
    }

    fn test_egress(
        bind: impl Fn(Ipv4Addr) -> io::Result<TcpListener> + Send + Sync + 'static,
    ) -> Arc<Egress> {
        Egress::with_setup(Setup {
            docker: Arc::new(crate::mcp::stdio::tests::MockDocker::silent()),
            bind: Box::new(bind),
            port: DEFAULT_PORT,
            state_dir: std::env::temp_dir(),
            relay_for: relay_to(PathBuf::from("mcp_relay")),
            handshake: Handshake::fast(),
        })
    }

    /// Sends the relay's hello (token, PROXY line from `source`) and then `request`; returns what came back.
    fn relayed(addr: SocketAddr, token: &str, source: &str, request: &str) -> String {
        let mut stream = TcpStream::connect(addr).unwrap();
        let hello =
            format!("APZ-RELAY-TOKEN {token}\r\nPROXY TCP4 {source} 172.29.250.2 5000 3128\r\n");
        stream.write_all(hello.as_bytes()).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.read_to_string(&mut out);
        out
    }

    #[test]
    fn a_relayed_request_is_policed_for_the_server_its_proxy_source_names() {
        let (addr, _) = proxy(false, &["evil.example"]);
        let blocked = relayed(
            addr,
            "tok",
            "172.29.250.9",
            "CONNECT evil.example:443 HTTP/1.1\r\nHost: x\r\n\r\n",
        );
        assert!(
            blocked.starts_with("HTTP/1.1 403") && blocked.contains("blocklist"),
            "{blocked}"
        );
        let private = relayed(
            addr,
            "tok",
            "172.29.250.9",
            "CONNECT 127.0.0.1:9 HTTP/1.1\r\nHost: x\r\n\r\n",
        );
        assert!(
            private.starts_with("HTTP/1.1 403") && private.contains("loopback"),
            "{private}"
        );
        let plain = relayed(
            addr,
            "tok",
            "172.29.250.9",
            "GET http://10.1.1.1/secret?token=1 HTTP/1.1\r\nHost: 10.1.1.1\r\n\r\n",
        );
        assert!(
            plain.starts_with("HTTP/1.1 403") && !plain.contains("token"),
            "{plain}"
        );
    }

    #[test]
    fn a_forwarded_request_reaches_an_allowed_local_server_through_the_relay() {
        let origin = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin_port = origin.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Some(Ok(mut s)) = origin.incoming().next() {
                let mut buf = [0u8; 512];
                let n = s.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                let body = if head.starts_with("GET /hello HTTP/1.1") {
                    "origin-ok"
                } else {
                    "wrong-path"
                };
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let (addr, _) = proxy(true, &[]);
        let reply = relayed(
            addr,
            "tok",
            "172.29.250.9",
            &format!(
                "GET http://127.0.0.1:{origin_port}/hello HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(reply.ends_with("origin-ok"), "{reply}");
    }

    #[test]
    fn a_forged_or_missing_token_is_dropped_without_an_answer() {
        let (addr, _) = proxy(false, &[]);
        let request = "CONNECT 8.8.8.8:443 HTTP/1.1\r\n\r\n";
        assert_eq!(relayed(addr, "forged", "172.29.250.9", request), "");
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(format!("PROXY TCP4 172.29.250.9 172.29.250.2 1 2\r\n{request}").as_bytes())
            .unwrap();
        let mut out = String::new();
        let _ = stream.read_to_string(&mut out);
        assert_eq!(out, "", "no token line, no answer");
    }

    /// A proxy with the given handshake bounds, on loopback, with the token `tok` and one leased server.
    fn bounded(handshake: Handshake) -> (SocketAddr, Arc<Egress>) {
        let egress = Egress::with_setup(Setup {
            docker: Arc::new(crate::mcp::stdio::tests::MockDocker::silent()),
            bind: Box::new(|_: Ipv4Addr| -> io::Result<TcpListener> {
                panic!("no bind in this test")
            }),
            port: DEFAULT_PORT,
            state_dir: std::env::temp_dir(),
            relay_for: relay_to(PathBuf::from("mcp_relay")),
            handshake,
        });
        egress.set_token("tok");
        egress.admit_at(Ipv4Addr::new(172, 29, 250, 9), "srv", false);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let serving = Arc::clone(&egress);
        std::thread::spawn(move || serve(listener, serving));
        (addr, egress)
    }

    #[test]
    fn a_hundred_bad_handshakes_then_a_good_one_still_succeeds() {
        let (addr, _) = bounded(Handshake::fast());
        for _ in 0..100 {
            assert_eq!(
                relayed(
                    addr,
                    "forged",
                    "172.29.250.9",
                    "CONNECT 8.8.8.8:443 HTTP/1.1\r\n\r\n"
                ),
                ""
            );
        }
        // The valid token is accepted whatever the failure count: the request reaches the policy (loopback is refused).
        let reply = relayed(
            addr,
            "tok",
            "172.29.250.9",
            "CONNECT 127.0.0.1:9 HTTP/1.1\r\n\r\n",
        );
        assert!(
            reply.starts_with("HTTP/1.1 403") && reply.contains("loopback"),
            "{reply}"
        );
    }

    #[test]
    fn a_failed_handshake_is_held_until_the_tarpit_time() {
        let (addr, _) = bounded(Handshake {
            timeout: Duration::from_secs(2),
            tarpit: Duration::from_millis(400),
            max_pending: 16,
        });
        let started = Instant::now();
        assert_eq!(
            relayed(
                addr,
                "forged",
                "172.29.250.9",
                "CONNECT 8.8.8.8:443 HTTP/1.1\r\n\r\n"
            ),
            ""
        );
        assert!(
            started.elapsed() >= Duration::from_millis(350),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn the_oldest_pending_connection_is_evicted_for_a_new_one() {
        let handshake = Handshake {
            timeout: Duration::from_secs(5),
            tarpit: Duration::from_millis(10),
            max_pending: 16,
        };
        let (addr, _) = bounded(handshake);
        // Sixteen connections that send nothing fill the pending set; their handshakes have seconds to finish.
        let mut held: Vec<TcpStream> = (0..16).map(|_| TcpStream::connect(addr).unwrap()).collect();
        std::thread::sleep(Duration::from_millis(100));
        // A seventeenth arrives: it is accepted, and the oldest of the held is closed.
        let mut newest = TcpStream::connect(addr).unwrap();
        let mut oldest = held.remove(0);
        oldest
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let started = Instant::now();
        let mut out = String::new();
        let _ = oldest.read_to_string(&mut out);
        assert_eq!(out, "");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "evicted at once, not timed out: {:?}",
            started.elapsed()
        );
        // The new connection is still open.
        newest
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        let mut buf = [0u8; 1];
        let err = newest.read(&mut buf).unwrap_err();
        assert!(
            matches!(
                err.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ),
            "{err:?}"
        );
        drop(held);
        drop(newest);
    }

    #[test]
    fn a_relay_is_served_at_once_among_sixteen_attackers_fifty_times_and_is_never_evicted() {
        let handshake = Handshake {
            timeout: Duration::from_secs(2),
            tarpit: Duration::from_millis(10),
            max_pending: 16,
        };
        let (addr, _) = bounded(handshake);
        for round in 0..50 {
            // Sixteen attackers hold the pending set. The relay arrives after them, so the oldest attacker is
            // evicted for it, and it authenticates within milliseconds: nothing arrives after it to evict it.
            let attackers: Vec<TcpStream> =
                (0..16).map(|_| TcpStream::connect(addr).unwrap()).collect();
            let started = Instant::now();
            let reply = relayed(
                addr,
                "tok",
                "172.29.250.9",
                "CONNECT 127.0.0.1:9 HTTP/1.1\r\n\r\n",
            );
            assert!(
                reply.starts_with("HTTP/1.1 403") && reply.contains("loopback"),
                "round {round}: {reply:?}"
            );
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "round {round}: served at once, took {:?}",
                started.elapsed()
            );
            drop(attackers);
        }
    }

    #[test]
    fn a_handshake_that_sends_nothing_or_a_partial_token_times_out() {
        let handshake = Handshake {
            timeout: Duration::from_millis(200),
            tarpit: Duration::ZERO,
            max_pending: 16,
        };
        let (addr, _) = bounded(handshake);
        let started = Instant::now();
        let mut silent = TcpStream::connect(addr).unwrap();
        silent
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut out = String::new();
        let _ = silent.read_to_string(&mut out);
        let waited = started.elapsed();
        assert_eq!(out, "");
        assert!(
            waited >= Duration::from_millis(150) && waited < Duration::from_secs(3),
            "{waited:?}"
        );
        // A slowloris: part of the token line, and no newline, so the line never completes.
        let mut slow = TcpStream::connect(addr).unwrap();
        slow.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        slow.write_all(b"APZ-RELAY-TOKEN tok").unwrap();
        let started = Instant::now();
        let mut out = String::new();
        let _ = slow.read_to_string(&mut out);
        assert_eq!(out, "");
        assert!(
            started.elapsed() >= Duration::from_millis(150)
                && started.elapsed() < Duration::from_secs(3)
        );
    }

    #[test]
    fn a_relayed_source_with_no_lease_gets_nothing_useful() {
        let (addr, _) = proxy(false, &[]);
        let reply = relayed(
            addr,
            "tok",
            "172.29.250.99",
            "CONNECT 8.8.8.8:443 HTTP/1.1\r\n\r\n",
        );
        assert!(
            reply.starts_with("HTTP/1.1 403") && reply.contains("no network lease"),
            "{reply}"
        );
    }
}

#[cfg(test)]
mod listener_tests {
    use super::*;
    use crate::mcp::stdio::Ran;
    use crate::mcp::stdio::tests::MockDocker;
    use std::sync::Arc;

    fn ok(stdout: &str) -> io::Result<Ran> {
        Ok(Ran {
            ok: true,
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }

    /// A healthy host: both networks exist, and the default bridge has `gateway` (or none).
    fn healthy(
        gateway: Option<&'static str>,
    ) -> impl Fn(&[String]) -> io::Result<Ran> + Send + Sync {
        move |args: &[String]| {
            if args.first().map(String::as_str) == Some("info") {
                return ok("x86_64");
            }
            if args.iter().any(|a| a == "bridge") && args.iter().any(|a| a == "inspect") {
                return match gateway {
                    Some(g) => ok(g),
                    None => Ok(Ran {
                        ok: false,
                        stdout: String::new(),
                        stderr: "no".into(),
                    }),
                };
            }
            if args.iter().any(|a| a == "inspect") && args.iter().any(|a| a == relay::UPLINK) {
                return ok("false 172.29.251.0/24 172.29.251.1");
            }
            ok("true 172.29.250.0/24 172.29.250.1")
        }
    }

    fn binary(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("apz-mcp-relay-src-{name}-{}", std::process::id()));
        std::fs::write(&path, b"stand-in").unwrap();
        path
    }

    #[test]
    fn ensure_listeners_binds_loopback_and_the_optional_bridge_gateway() {
        // the_proxy_listens_on_loopback_and_on_the_bridge_gateway_when_the_host_has_it
        {
            let bound = Arc::new(Mutex::new(Vec::new()));
            let record = Arc::clone(&bound);
            let egress = Egress::with_setup(Setup {
                docker: Arc::new(MockDocker::new(
                    Default::default(),
                    healthy(Some("172.17.0.1")),
                )),
                bind: Box::new(move |ip: Ipv4Addr| {
                    record.lock().unwrap().push(ip);
                    TcpListener::bind("127.0.0.1:0")
                }),
                port: DEFAULT_PORT,
                state_dir: std::env::temp_dir(),
                relay_for: relay_to(binary("both")),
                handshake: Handshake::fast(),
            });
            egress.ensure_listeners().unwrap();
            assert_eq!(
                *bound.lock().unwrap(),
                [Ipv4Addr::LOCALHOST, Ipv4Addr::new(172, 17, 0, 1)]
            );
        }
        // docker_desktop_without_a_host_gateway_still_serves_loopback
        {
            let bound = Arc::new(Mutex::new(Vec::new()));
            let record = Arc::clone(&bound);
            let egress = Egress::with_setup(Setup {
                docker: Arc::new(MockDocker::new(
                    Default::default(),
                    healthy(Some("172.17.0.1")),
                )),
                bind: Box::new(move |ip: Ipv4Addr| {
                    record.lock().unwrap().push(ip);
                    if ip == Ipv4Addr::LOCALHOST {
                        TcpListener::bind("127.0.0.1:0")
                    } else {
                        Err(io::Error::from(io::ErrorKind::AddrNotAvailable))
                    }
                }),
                port: DEFAULT_PORT,
                state_dir: std::env::temp_dir(),
                relay_for: relay_to(binary("desktop")),
                handshake: Handshake::fast(),
            });
            egress.ensure_listeners().unwrap();
            assert_eq!(
                bound.lock().unwrap().len(),
                2,
                "both were tried; the gateway one was optional"
            );
        }
    }

    #[test]
    fn the_relay_starts_after_the_networks_with_the_host_upstream_and_its_token_only_in_the_environment()
     {
        let log = Arc::new(Mutex::new(Vec::new()));
        let docker = Arc::new(MockDocker::new(Arc::clone(&log), healthy(None)));
        let egress = Egress::with_setup(Setup {
            docker: Arc::clone(&docker) as Arc<dyn Docker>,
            bind: Box::new(|_: Ipv4Addr| TcpListener::bind("127.0.0.1:0")),
            port: 19555,
            state_dir: std::env::temp_dir()
                .join(format!("apz-egress-state-{}", std::process::id())),
            relay_for: relay_to(binary("order")),
            handshake: Handshake::fast(),
        });
        let lease = egress.attach("srv", false, &[]).unwrap();
        assert_eq!(lease.proxy(), "http://172.29.250.2:3128");
        let calls = log.lock().unwrap().clone();
        let token = lock(&egress.token).clone().unwrap();
        assert_eq!(token.len(), 64, "a 256-bit token, in hex");
        assert!(
            calls.iter().all(|c| !c.contains(&token)),
            "the token is never in a command line"
        );
        let create = calls
            .iter()
            .position(|c| c.starts_with("create --name apz-mcp-relay"))
            .unwrap();
        assert!(
            calls[create].contains("--add-host=host.docker.internal:host-gateway"),
            "{}",
            calls[create]
        );
        assert!(
            calls[create].contains("APZ_RELAY_UPSTREAM=host.docker.internal:19555"),
            "{}",
            calls[create]
        );
        let connect = calls
            .iter()
            .position(|c| c.starts_with("network connect"))
            .unwrap();
        let start = calls
            .iter()
            .position(|c| c.contains("start -a -i"))
            .unwrap();
        assert!(create < connect && connect < start, "{calls:?}");
        drop(lease);
        drop(egress);
    }
}
