//! The egress proxy for stdio servers (docs/design/mcp.md, "The proxy").
//!
//! A stdio server runs under the OS sandbox with no network except the proxy's loopback port (`apz_sandbox`'s
//! `LoopbackPort`). The proxy listens on `127.0.0.1:<port>` and serves a request only when it carries a lease
//! credential: each start of a server gets a new 256-bit token, which the server holds as the password of its proxy
//! URL (`http://apz:<token>@127.0.0.1:<port>`, so every request says `Proxy-Authorization: Basic`). The token names
//! the server the request is policed for: the fetch blocklist always, and private, loopback, link-local and metadata
//! addresses refused unless that server's `allow_private` says so. A request with no credential or a wrong one gets
//! 407 after a tarpit delay, and unauthenticated connections are capped: a new one beyond the cap closes the oldest.
//! A lease's token is forgotten when the lease drops, so the server's next start gets a new one.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

/// The address rules, shared with vsockd's proxy (crates/apz-egress-rules). `mcp/http.rs` reaches them here.
pub use apz_egress_rules::refusal;

/// The proxy's default port on loopback (apid's range; `--mcp-proxy-port` changes it).
pub const DEFAULT_PORT: u16 = 19128;
/// The longest request head the proxy reads.
const HEAD_CAP: usize = 16 << 10;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE: Duration = Duration::from_secs(300);
/// The user name of every lease's proxy URL (the token is the password).
const USER: &str = "apz";

/// How unauthenticated connections are bounded (docs/design/mcp.md, "The proxy"). A valid credential is always
/// accepted, whatever has failed before. A failed one is held until `tarpit` after it was accepted, then answered.
/// Unauthenticated connections are capped at `max_pending`: a new one beyond that closes the oldest at once. The
/// head's reads time out after `timeout`, so a connection that sends nothing cannot hold a slot.
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
            0 => "closed the oldest unauthenticated proxy connection to make room".to_string(),
            n => format!(
                "closed the oldest unauthenticated proxy connection to make room; {n} more since the last log line"
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

/// The server a lease's token belongs to.
#[derive(Clone)]
struct Tenant {
    server: String,
    allow_private: bool,
}

/// The proxy and its credential book. Shared by the listener's threads and the runtime.
pub struct Egress {
    /// The port asked for (0 binds any free port: tests).
    port: u16,
    /// The port bound, once the listener is up.
    bound: AtomicU16,
    listening: Mutex<bool>,
    /// The unauthenticated connections open now, oldest first (the cap is `handshake.max_pending`).
    pending: Mutex<VecDeque<Pending>>,
    next_pending: AtomicU64,
    handshake: Handshake,
    /// The leases' tokens, each with the server it was leased to.
    tenants: Mutex<BTreeMap<String, Tenant>>,
    blocklist: RwLock<Vec<String>>,
    /// The verdicts already logged, so each host is logged once per server and verdict.
    logged: Mutex<BTreeSet<(String, String, bool)>>,
    /// The rate limit on the eviction log line.
    eviction_log: Mutex<EvictionLog>,
}

/// A server's credential on the proxy, held while its process runs. Dropping it forgets the token.
pub struct Lease {
    token: String,
    proxy: String,
    port: u16,
    egress: Arc<Egress>,
}

impl Lease {
    /// The proxy URL the server gets: `http://apz:<token>@127.0.0.1:<port>`.
    pub fn proxy(&self) -> &str {
        &self.proxy
    }

    /// The loopback port the proxy listens on: the one port the sandbox lets the server reach.
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        lock(&self.egress.tenants).remove(&self.token);
    }
}

impl Egress {
    /// The proxy on loopback `port` (0 for any free port). It binds when the first server attaches.
    pub fn new(port: u16) -> Arc<Egress> {
        Egress::with_handshake(port, Handshake::default())
    }

    pub fn with_handshake(port: u16, handshake: Handshake) -> Arc<Egress> {
        Arc::new(Egress {
            port,
            bound: AtomicU16::new(0),
            listening: Mutex::new(false),
            pending: Mutex::new(VecDeque::new()),
            next_pending: AtomicU64::new(0),
            handshake,
            tenants: Mutex::new(BTreeMap::new()),
            blocklist: RwLock::new(Vec::new()),
            logged: Mutex::new(BTreeSet::new()),
            eviction_log: Mutex::new(EvictionLog::default()),
        })
    }

    /// Binds the proxy (once), and gives `server` a new credential whose requests are policed for it.
    pub fn attach(
        self: &Arc<Self>,
        server: &str,
        allow_private: bool,
        blocklist: &[String],
    ) -> Result<Lease, String> {
        self.ensure_listener()?;
        self.set_blocklist(blocklist);
        let token =
            apz_os::random::hex(32).map_err(|e| format!("cannot make a proxy credential: {e}"))?;
        self.admit(&token, server, allow_private);
        let port = self.bound.load(Ordering::SeqCst);
        Ok(Lease {
            proxy: format!("http://{USER}:{token}@127.0.0.1:{port}"),
            token,
            port,
            egress: Arc::clone(self),
        })
    }

    /// Records that `token` belongs to `server`. The proxy answers that credential from now on.
    pub fn admit(&self, token: &str, server: &str, allow_private: bool) {
        lock(&self.tenants).insert(
            token.to_string(),
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

    fn tenant(&self, token: &str) -> Option<Tenant> {
        lock(&self.tenants).get(token).cloned()
    }

    /// Admits a connection as pending. When the cap is full, the oldest pending connection is closed to make room.
    fn admit_pending(&self, socket: Arc<TcpStream>) -> u64 {
        let id = self.next_pending.fetch_add(1, Ordering::SeqCst);
        let mut pending = lock(&self.pending);
        if pending.len() >= self.handshake.max_pending
            && let Some(oldest) = pending.pop_front()
        {
            let _ = oldest.socket.shutdown(Shutdown::Both);
            if let Some(line) = lock(&self.eviction_log).record(Instant::now()) {
                eprintln!("apid: mcp: {line}");
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

    /// Binds the proxy on loopback, once. Its accept loop runs on its own thread.
    fn ensure_listener(self: &Arc<Self>) -> Result<(), String> {
        let mut listening = lock(&self.listening);
        if *listening {
            return Ok(());
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, self.port)).map_err(|_| {
            format!(
                "the egress proxy cannot listen on 127.0.0.1:{}; remote (http) servers still work",
                self.port
            )
        })?;
        let port = listener
            .local_addr()
            .map(|addr| addr.port())
            .map_err(|e| format!("the egress proxy has no address: {e}"))?;
        self.bound.store(port, Ordering::SeqCst);
        let egress = Arc::clone(self);
        std::thread::spawn(move || serve(listener, egress));
        *listening = true;
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

/// The environment a stdio server gets to reach the proxy: its URL for every scheme, and no `NO_PROXY`.
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

/// Accepts connections on a listener, one thread each. Each is admitted as pending, which may evict the oldest.
pub fn serve(listener: TcpListener, egress: Arc<Egress>) {
    for stream in listener.incoming().flatten() {
        let stream = Arc::new(stream);
        let id = egress.admit_pending(Arc::clone(&stream));
        let egress = Arc::clone(&egress);
        std::thread::spawn(move || connection(stream, &egress, id));
    }
}

/// One proxy connection. A request with a valid credential leaves the pending set and is served. One without it
/// is logged (never with the token), held until the tarpit time, and answered 407. An evicted connection is closed.
fn connection(stream: Arc<TcpStream>, egress: &Egress, id: u64) {
    let accepted = Instant::now();
    let _ = stream.set_read_timeout(Some(egress.handshake.timeout));
    let mut reader = BufReader::new(&*stream);
    let head = read_head(&mut reader);
    let tenant = match &head {
        Ok(head) => authenticate(head, egress),
        Err(_) => None,
    };
    match (head, tenant) {
        (Ok(head), Some(tenant)) => {
            if !egress.leave_pending(id) {
                return;
            }
            let _ = stream.set_read_timeout(Some(IDLE));
            serve_request(&stream, reader, head, tenant, egress);
        }
        (head, _) => {
            if head.is_ok() {
                eprintln!("apid: mcp: a proxy request without the server's credential was refused");
            }
            std::thread::sleep(egress.handshake.tarpit.saturating_sub(accepted.elapsed()));
            egress.leave_pending(id);
            respond(
                &stream,
                407,
                "Proxy Authentication Required",
                "a proxy credential is needed",
            );
        }
    }
}

/// The tenant a request's `Proxy-Authorization: Basic` credential names, if it names a live lease.
fn authenticate(head: &[String], egress: &Egress) -> Option<Tenant> {
    let value = head.iter().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("proxy-authorization")
            .then(|| value.trim().to_string())
    })?;
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = super::runtime::decode_base64(encoded)?;
    let text = String::from_utf8(decoded).ok()?;
    let token = text.strip_prefix(&format!("{USER}:"))?;
    if token.is_empty() {
        return None;
    }
    egress.tenant(token)
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

/// The request for one authenticated client: the policy for the server the credential names, then the tunnel or the
/// request. `head` is the request's head as read (its request line first).
fn serve_request(
    mut client: &TcpStream,
    reader: BufReader<&TcpStream>,
    head: Vec<String>,
    tenant: Tenant,
    egress: &Egress,
) {
    let server = tenant.server;
    let allow_private = tenant.allow_private;
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
    if !allow_private
        && let Some((ip, why)) = addresses
            .iter()
            .find_map(|ip| refusal(*ip).map(|why| (*ip, why)))
    {
        return Err(format!(
            "refused: {host} is a {why} address ({ip}); set allow_private to reach it"
        ));
    }
    Ok(SocketAddr::new(addresses[0], port))
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
        assert!(decide("127.0.0.1", 80, false, &[]).is_err());
        assert!(decide("127.0.0.1", 80, true, &[]).is_ok());
        assert!(decide("169.254.169.254", 80, false, &[]).is_err());
        assert!(decide("example.invalid", 443, false, &list(&["example.invalid"])).is_err());
    }

    #[test]
    fn authorities_split_into_host_and_port() {
        assert_eq!(
            split_host_port("a.test:8443", 443),
            Some(("a.test".into(), 8443))
        );
        assert_eq!(split_host_port("a.test", 443), Some(("a.test".into(), 443)));
        assert_eq!(split_host_port("[::1]:9", 443), Some(("::1".into(), 9)));
        assert_eq!(split_host_port("[::1]", 80), Some(("::1".into(), 80)));
        assert_eq!(split_host_port("a.test:x", 443), None);
    }

    #[test]
    fn the_children_get_the_proxy_variables_and_an_empty_no_proxy() {
        let env = proxy_env("http://apz:t@127.0.0.1:19128");
        let get = |k: &str| {
            env.iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("HTTPS_PROXY").unwrap(), "http://apz:t@127.0.0.1:19128");
        assert_eq!(get("NO_PROXY").unwrap(), "");
        assert_eq!(get("NODE_USE_ENV_PROXY").unwrap(), "1");
    }

    /// Sends `request` to the proxy and returns its status line.
    fn ask(port: u16, request: &str) -> String {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        let _ = stream.read_to_string(&mut reply);
        reply.lines().next().unwrap_or("").to_string()
    }

    fn connect_with(token: &str, target: &str) -> String {
        let credential = crate::connectors::base64(format!("apz:{token}").as_bytes());
        format!(
            "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {credential}\r\n\r\n"
        )
    }

    #[test]
    fn a_request_without_a_lease_credential_gets_407_and_none_is_served() {
        let egress = Egress::with_handshake(0, Handshake::fast());
        let lease = egress.attach("fetch", false, &[]).unwrap();
        let port = lease.port();
        assert!(ask(port, "CONNECT example.invalid:443 HTTP/1.1\r\n\r\n").contains("407"));
        assert!(ask(port, &connect_with("not-the-token", "example.invalid:443")).contains("407"));
    }

    #[test]
    fn a_lease_is_policed_for_its_own_server_and_forgotten_when_it_drops() {
        let egress = Egress::with_handshake(0, Handshake::fast());
        let fetch = egress.attach("fetch", false, &[]).unwrap();
        let private = egress.attach("local", true, &[]).unwrap();
        let port = fetch.port();
        // A loopback target is refused for a server without allow_private, and allowed for one with it.
        let refused = ask(port, &connect_with(lease_token(&fetch), "127.0.0.1:9"));
        assert!(refused.contains("403"), "{refused}");
        let allowed = ask(
            private.port(),
            &connect_with(lease_token(&private), "127.0.0.1:9"),
        );
        assert!(
            !allowed.contains("403") && !allowed.contains("407"),
            "{allowed}"
        );
        let token = lease_token(&fetch).to_string();
        drop(fetch);
        assert!(lock(&egress.tenants).get(&token).is_none());
    }

    fn lease_token(lease: &Lease) -> &str {
        lease.token.as_str()
    }

    #[test]
    fn the_lease_url_carries_the_proxy_user_and_the_port() {
        let egress = Egress::with_handshake(0, Handshake::fast());
        let lease = egress.attach("memory", false, &[]).unwrap();
        assert!(lease.proxy().starts_with("http://apz:"));
        assert!(
            lease
                .proxy()
                .ends_with(&format!("@127.0.0.1:{}", lease.port()))
        );
        assert_eq!(lease.token.len(), 64);
    }
}
