//! The egress proxy. A guest has no network of its own, so its outbound HTTP
//! proxy traffic arrives over vsock port 5001 and the hub connects out on its
//! behalf. One listener per served CID ([`accept_loop`]), one thread per
//! connection, and a [`Policy`] that refuses every non-public destination.
//! Each connection ends in one audit line and, when apid is configured, one
//! record POSTed to apid.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::apid::Apid;
use crate::sys;

/// The vsock port the guest's proxy traffic arrives on.
pub const EGRESS_PORT: u32 = 5001;
/// The request head (request line and headers) is capped at this many bytes.
const MAX_HEAD: usize = 8 * 1024;
/// How long a guest may take to send its request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// A tunnel with no traffic for this long is closed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const ESTABLISHED: &[u8] = b"HTTP/1.1 200 Connection established\r\n\r\n";
const COPY_BUF: usize = 16 * 1024;

/// The two sockets of a tunnel, cloned so either pump can hang both up.
type Hangup = (UnixStream, TcpStream);

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
}

impl Stop {
    fn answer(&self) -> &'static [u8] {
        match self {
            Stop::Bad => &b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..],
            Stop::Forbidden => &b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..],
            Stop::Unreachable => &b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..],
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
        if let Err(err) = sys::poll_readable(listener.as_raw_fd(), super::ACCEPT_POLL_MS) {
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
    if let Err(stop) = proxy(&mut guest, policy, &mut audit) {
        let _ = guest.write_all(stop.answer());
    }
    audit.report(cid, started.elapsed(), apid);
    audit
}

/// Reads the request, checks it, connects out, and tunnels or forwards.
/// Every `Err` happens before anything is written to the guest.
fn proxy(guest: &mut UnixStream, policy: &Policy, audit: &mut Audit) -> Result<(), Stop> {
    let (head, rest) = read_head(guest)?;
    let parsed = parse_head(&head)?;
    let (host, port) = match &parsed {
        Head::Connect { host, port } | Head::Absolute { host, port, .. } => (host.clone(), *port),
    };
    audit.host = host.clone();
    audit.port = port;
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
    guest.set_read_timeout(Some(IDLE_TIMEOUT)).map_err(|_| Stop::Unreachable)?;
    upstream.set_read_timeout(Some(IDLE_TIMEOUT)).map_err(|_| Stop::Unreachable)?;

    // Every clone is made before anything is written, so a failure here still
    // leaves the guest an error answer rather than half a tunnel.
    let guest_read = guest.try_clone().map_err(|_| Stop::Unreachable)?;
    let guest_write = guest.try_clone().map_err(|_| Stop::Unreachable)?;
    let mut upstream_write = upstream.try_clone().map_err(|_| Stop::Unreachable)?;
    let hangup = Arc::new((
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

/// Copies `from` to `to` until either side closes, then shuts down both
/// sockets so the other direction ends too. Returns the bytes copied.
fn pump(mut from: impl Read, mut to: impl Write, hangup: &Arc<Hangup>) -> u64 {
    let mut buf = vec![0u8; COPY_BUF];
    let mut total = 0u64;
    loop {
        match from.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if to.write_all(&buf[..n]).is_err() {
                    break;
                }
                total += n as u64;
            }
        }
    }
    let _ = hangup.0.shutdown(Shutdown::Both);
    let _ = hangup.1.shutdown(Shutdown::Both);
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
        let apid = Apid::off();
        let proxy = thread::spawn(move || serve_conn(server, 7, &policy, &apid));

        client
            .write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes())
            .unwrap();
        let mut answer = vec![0u8; ESTABLISHED.len()];
        client.read_exact(&mut answer).unwrap();
        assert_eq!(answer, ESTABLISHED);

        let payload = b"ping through the tunnel";
        client.write_all(payload).unwrap();
        let mut echoed = vec![0u8; payload.len()];
        client.read_exact(&mut echoed).unwrap();
        assert_eq!(&echoed[..], &payload[..]);

        client.shutdown(Shutdown::Write).unwrap();
        let audit = proxy.join().unwrap();
        assert!(
            audit.allowed && audit.port == port && audit.up == payload.len() as u64 && audit.down == payload.len() as u64,
            "{audit:?}"
        );
    }
}
