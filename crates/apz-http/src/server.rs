//! The threaded server.

use std::cell::Cell;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustls::{ServerConfig, ServerConnection, StreamOwned};

use crate::{Body, Limits, Request, Response, host_is_loopback, ws};

/// The most connections served at once; the next one is answered 503.
const MAX_CONNECTIONS: usize = 256;

/// What a node's relay (noded's `ws_relay`) writes first on a connection it relays from a VM to a hub
/// on its own machine. The VM is not this machine's own caller, so a loopback connection that opens
/// with it is served as a caller from elsewhere: [`Request::peer`] is `0.0.0.0`, never loopback, and
/// it gets no loopback trust. Writing it can only lower a caller's trust.
pub const RELAY_PREFACE: &[u8] = b"APZ-RELAY/1\r\n";

/// How long a loopback connection that has begun the preface may take to finish it.
const PREFACE_WAIT: Duration = Duration::from_secs(2);

/// Every connection's read and write timeout; it also bounds how long an SSE
/// client may stall a write, and how long a TLS client may stall a handshake
/// between bytes.
const TIMEOUT: Duration = Duration::from_secs(10);

/// After a response with a length to a client that asked for keep-alive (a browser, Python's
/// requests: it closes on `Connection: close`), how long the server waits for that client to close
/// first. The client then keeps the TIME_WAIT, not the server: a server that closes first on every
/// connection fills loopback with TIME_WAITs, and a client reusing one of their ports has its
/// connect stall for seconds. A client that keeps the connection open is cut off after this.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// The most a waiting server reads (and discards) before it closes anyway.
const CLOSE_DRAIN: usize = 64 << 10;

/// How long a connection has, from its accept, to send its relay preface, finish the TLS handshake and
/// send the request line and headers. Each read still has [`TIMEOUT`]; this bounds them together.
const HEAD_DEADLINE: Duration = Duration::from_secs(20);

/// How long a request body may take: this much, plus [`BODY_MIN_RATE`]'s time for its length.
const BODY_DEADLINE: Duration = Duration::from_secs(30);

/// The slowest body transfer the deadline allows for: bytes per second.
const BODY_MIN_RATE: u64 = 256 << 10;

/// The most of a response's body that is copied in front of the write of its head, so that a body up
/// to this size goes out in one write, with its head; a larger body is written after the head.
const COALESCE: usize = 8 << 10;

/// The room a response's head has beside its body in that copy.
const HEAD_ROOM: usize = 1 << 10;

/// Decides, from a request's line and headers alone, whether its body is read at all: `Err` is the
/// answer sent without reading it, `Ok` the most body bytes this caller may send (never more than the
/// server's [`Limits::max_body`]).
pub type Admit = Arc<dyn Fn(&Request) -> Result<usize, Response> + Send + Sync>;

/// A bound, not yet serving, listener.
pub struct Server {
    listener: TcpListener,
    limits: Limits,
    tls: Option<Arc<ServerConfig>>,
    admit: Option<Admit>,
}

impl Server {
    /// Binds `127.0.0.1:port` (`0` picks a free port).
    pub fn bind(port: u16) -> io::Result<Self> {
        Self::bind_addr(SocketAddr::from(([127, 0, 0, 1], port)))
    }

    /// Binds `addr`, whichever address that is; the caller decides how far
    /// it is exposed.
    pub fn bind_addr(addr: SocketAddr) -> io::Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(addr)?,
            limits: Limits::default(),
            tls: None,
            admit: None,
        })
    }

    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Asks `admit` about every request once its headers are read, before its body is ([`Admit`]).
    pub fn admit(
        mut self,
        admit: impl Fn(&Request) -> Result<usize, Response> + Send + Sync + 'static,
    ) -> Self {
        self.admit = Some(Arc::new(admit));
        self
    }

    /// Serves every connection over TLS with `config`.
    pub fn tls(mut self, config: Arc<ServerConfig>) -> Self {
        self.tls = Some(config);
        self
    }

    /// Serves on a background thread, one thread per connection, until the
    /// returned [`Handle`] is stopped or dropped.
    pub fn spawn<H>(self, handler: H) -> io::Result<Handle>
    where
        H: Fn(&Request) -> Response + Send + Sync + 'static,
    {
        let addr = self.listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = thread::spawn(move || self.accept_loop(Arc::new(handler), &flag));
        Ok(Handle {
            addr,
            stop,
            thread: Some(thread),
        })
    }

    fn accept_loop<H>(self, handler: Arc<H>, stop: &AtomicBool)
    where
        H: Fn(&Request) -> Response + Send + Sync + 'static,
    {
        let active = Arc::new(AtomicUsize::new(0));
        for stream in self.listener.incoming() {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            let stream = match stream {
                Ok(stream) => stream,
                Err(err) => {
                    // EMFILE and friends: back off rather than spin.
                    eprintln!("apz-http: accept failed: {err}");
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }
            };
            if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                active.fetch_sub(1, Ordering::SeqCst);
                let mut stream = stream;
                let _ = stream.set_write_timeout(Some(TIMEOUT));
                let _ = Response::error(503, "too many connections").write_to(&mut stream);
                continue;
            }
            let (handler, active) = (Arc::clone(&handler), Arc::clone(&active));
            let (limits, tls, admit) = (self.limits, self.tls.clone(), self.admit.clone());
            let accepted = Instant::now();
            thread::spawn(move || {
                let head_deadline = accepted + HEAD_DEADLINE;
                serve_accepted(
                    stream,
                    tls,
                    &*handler,
                    limits,
                    admit.as_ref(),
                    head_deadline,
                );
                active.fetch_sub(1, Ordering::SeqCst);
            });
        }
    }
}

/// Serves one accepted connection, as plain HTTP or over TLS when `tls` is set.
fn serve_accepted<H>(
    stream: TcpStream,
    tls: Option<Arc<ServerConfig>>,
    handler: &H,
    limits: Limits,
    admit: Option<&Admit>,
    head_deadline: Instant,
) where
    H: Fn(&Request) -> Response,
{
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    let mut peer = stream.peer_addr().ok().map(|addr| addr.ip());
    if peer.is_some_and(|peer| peer.is_loopback()) && relayed(&stream) {
        peer = Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }
    let Some(config) = tls else {
        return serve_connection(stream, peer, handler, limits, admit, head_deadline);
    };
    let Ok(connection) = ServerConnection::new(config) else {
        return;
    };
    let mut tls = StreamOwned::new(connection, stream);
    // A client that fails the handshake has no request to answer: close quietly.
    while tls.conn.is_handshaking() {
        let Some(left) = head_deadline.checked_duration_since(Instant::now()) else {
            return;
        };
        if left < TIMEOUT {
            let _ = tls.sock.set_read_timeout(Some(left));
        }
        if tls.conn.complete_io(&mut tls.sock).is_err() {
            return;
        }
    }
    let _ = tls.sock.set_read_timeout(Some(TIMEOUT));
    serve_connection(tls, peer, handler, limits, admit, head_deadline);
}

/// Whether a loopback connection opens with [`RELAY_PREFACE`]; the preface is consumed when it does.
/// A connection whose first bytes are not the preface's is left untouched.
fn relayed(stream: &TcpStream) -> bool {
    let mut buf = [0u8; RELAY_PREFACE.len()];
    let deadline = Instant::now() + PREFACE_WAIT;
    loop {
        let Ok(seen) = stream.peek(&mut buf) else {
            return false;
        };
        if seen == 0 || buf[..seen] != RELAY_PREFACE[..seen] {
            return false;
        }
        if seen == buf.len() {
            return (&*stream).read_exact(&mut buf).is_ok();
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// One request, one response; after a response with a length to a pooling client, the client closes
/// first ([`CLOSE_WAIT`]). `peer` is the connection's remote address. An upgrade keeps the connection: its
/// closure gets the connection with whatever the request reading had already buffered still to be
/// read.
fn serve_connection<S, H>(
    stream: S,
    peer: Option<IpAddr>,
    handler: &H,
    limits: Limits,
    admit: Option<&Admit>,
    head_deadline: Instant,
) where
    S: ws::Duplex + 'static,
    H: Fn(&Request) -> Response,
{
    let mut reader = BufReader::new(Deadlined {
        inner: stream,
        deadline: Some(head_deadline),
        at_timeout: Cell::new(true),
    });
    let (response, pooled) = match read_admitted(&mut reader, peer, limits, admit) {
        Err(refused) => (refused, false),
        Ok(request) => {
            let pooled = keeps_alive(&request);
            (handler(&request).revalidated(&request), pooled)
        }
    };
    // A client reading to the close (no length) needs the server to close first.
    let wait = pooled && matches!(response.body, Body::Bytes(_));
    let mut connection = Buffered(reader);
    // A body of bytes goes out with its head in one write, one flush; a stream or an upgrade is
    // written straight through, as it is produced.
    let written = if let Body::Bytes(bytes) = &response.body {
        let capacity = bytes.len().min(COALESCE) + HEAD_ROOM;
        let mut out = BufWriter::with_capacity(capacity, &mut connection);
        let result = response.write_to(&mut out);
        // A write that failed is not sent again by the buffer's drop: the client has gone or stalled.
        let _ = out.into_parts();
        result
    } else {
        response.write_to(&mut connection)
    };
    // A client that hung up has nobody left to tell.
    match written {
        Ok(Some(run)) => run(Box::new(connection)),
        Ok(None) if wait => await_close(&mut connection),
        _ => {}
    }
}

/// Reads one request: its head within the head deadline, then, if `admit` lets it in, a body no larger
/// than it allows, within a deadline for that body's length. `Err` is the answer to send instead (a
/// refusal is sent without reading the body). The deadline is lifted before the handler runs, so an
/// upgraded connection keeps only the per-read timeouts.
fn read_admitted<S: ws::Duplex>(
    reader: &mut BufReader<Deadlined<S>>,
    peer: Option<IpAddr>,
    limits: Limits,
    admit: Option<&Admit>,
) -> Result<Request, Response> {
    let (mut request, length) =
        Request::read_head(reader, limits).map_err(|message| Response::error(400, message))?;
    request.peer = peer;
    if !host_allowed(request.header("host"), peer) {
        return Err(Response::error(403, "the Host header must name loopback"));
    }
    let allowed = match admit {
        Some(admit) => admit(&request)?.min(limits.max_body),
        None => limits.max_body,
    };
    if length.is_some_and(|length| length > allowed) {
        return Err(Response::error(413, "the request body is too large"));
    }
    if let Some(length) = length {
        let budget = BODY_DEADLINE + Duration::from_secs(length as u64 / BODY_MIN_RATE);
        reader.get_mut().deadline = Some(Instant::now() + budget);
    }
    request
        .read_body(reader, length)
        .map_err(|message| Response::error(400, message))?;
    reader.get_mut().deadline = None;
    Ok(request)
}

/// A connection whose reads, while `deadline` is set, end with `TimedOut` once it passes: each read
/// waits at most the time left (and at most [`TIMEOUT`]). A slow sender cannot hold a connection past
/// it by sending a byte now and then.
struct Deadlined<S> {
    inner: S,
    deadline: Option<Instant>,
    /// Whether the socket's read timeout is already TIMEOUT, so a read need not set it again.
    at_timeout: Cell<bool>,
}

impl<S: ws::Duplex> Read for Deadlined<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(deadline) = self.deadline {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the request took too long to arrive",
                ));
            };
            let want = left.min(TIMEOUT);
            if want != TIMEOUT || !self.at_timeout.get() {
                self.inner.set_read_timeout(Some(want))?;
                self.at_timeout.set(want == TIMEOUT);
            }
        }
        self.inner.read(buf)
    }
}

impl<S: ws::Duplex> Write for Deadlined<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<S: ws::Duplex> ws::Duplex for Deadlined<S> {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.at_timeout.set(false);
        self.inner.set_read_timeout(timeout)
    }
}

/// Whether the client asked to keep the connection (`Connection: keep-alive`): a pooling client
/// (a browser, Python's requests) that closes as soon as the answer says `Connection: close`. A
/// client without it may read the answer to the close, so the server closes for it.
fn keeps_alive(request: &Request) -> bool {
    request.header("connection").is_some_and(|value| {
        let has = |token: &str| {
            value
                .split(',')
                .any(|t| t.trim().eq_ignore_ascii_case(token))
        };
        has("keep-alive") && !has("close")
    })
}

/// Reads until the client closes (or [`CLOSE_WAIT`] or [`CLOSE_DRAIN`] runs out), discarding what
/// it sends, so the client's close comes first ([`CLOSE_WAIT`] says why).
fn await_close<S: ws::Duplex>(connection: &mut S) {
    let _ = connection.set_read_timeout(Some(CLOSE_WAIT));
    let mut buf = [0u8; 4096];
    let mut left = CLOSE_DRAIN;
    while left > 0 {
        match connection.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(read) => left = left.saturating_sub(read),
        }
    }
}

/// A connection whose reads first drain what the request reading had buffered ahead; writes go
/// straight to the connection.
struct Buffered<S: Read>(BufReader<S>);

impl<S: Read> Read for Buffered<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl<S: Read + Write> Write for Buffered<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.get_mut().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.get_mut().flush()
    }
}

impl<S: ws::Duplex> ws::Duplex for Buffered<S> {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.get_ref().set_read_timeout(timeout)
    }
}

/// The rebinding guard: a caller on this machine (loopback peer, or one not known) must name a
/// loopback host, since loopback callers are trusted — a web page whose domain resolves to
/// 127.0.0.1 is refused. A caller from another machine may name any host (a hub bound off
/// loopback is reached by its own name); it has to authenticate anyway.
fn host_allowed(host: Option<&str>, peer: Option<IpAddr>) -> bool {
    match host {
        None => true,
        Some(host) => host_is_loopback(host) || peer.is_some_and(|peer| !peer.is_loopback()),
    }
}

/// A serving [`Server`]. Dropping it stops the accept loop.
pub struct Handle {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Blocks until the server is stopped.
    pub fn join(mut self) -> io::Result<()> {
        match self.thread.take() {
            Some(thread) => thread
                .join()
                .map_err(|_| io::Error::other("the accept thread panicked")),
            None => Ok(()),
        }
    }

    /// Stops accepting; connections in flight finish on their own threads.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            // Wake the loop out of `accept(2)`.
            let _ = TcpStream::connect(self.addr);
            let _ = thread.join();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::Ipv4Addr;

    #[test]
    fn a_foreign_host_is_refused_from_loopback_and_allowed_from_another_machine() {
        let local: IpAddr = "127.0.0.1".parse().unwrap();
        let remote: IpAddr = "203.0.113.7".parse().unwrap();
        assert!(host_allowed(Some("127.0.0.1:7700"), Some(local)));
        assert!(!host_allowed(Some("evil.example"), Some(local)));
        assert!(!host_allowed(Some("evil.example"), None));
        assert!(host_allowed(Some("hub.example.org:7700"), Some(remote)));
        assert!(host_allowed(None, Some(local)));
    }
    use std::sync::Mutex;

    /// Sends one `GET /x` over a plain socket and returns the whole reply.
    fn get_plain(port: u16) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream
            .write_all(b"GET /x HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .expect("send");
        let mut reply = String::new();
        stream.read_to_string(&mut reply).expect("read");
        reply
    }

    #[test]
    fn a_plain_request_reaches_the_handler_with_its_peer() {
        let seen = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&seen);
        let handle = Server::bind(0)
            .expect("bind")
            .spawn(move |request: &Request| {
                *sink.lock().expect("lock") = Some((request.path.clone(), request.peer));
                Response::text(200, "ok")
            })
            .expect("spawn");
        let reply = get_plain(handle.port());
        assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
        assert!(reply.ends_with("ok"), "{reply}");
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert_eq!(
            *seen.lock().expect("lock"),
            Some(("/x".to_string(), Some(loopback)))
        );
    }

    #[test]
    fn a_client_without_keep_alive_is_closed_at_once() {
        let handle = Server::bind(0)
            .expect("bind")
            .spawn(move |_request: &Request| Response::text(200, "ok"))
            .expect("spawn");
        let started = Instant::now();
        let reply = get_plain(handle.port());
        assert!(reply.ends_with("ok"), "{reply}");
        assert!(
            started.elapsed() < CLOSE_WAIT / 2,
            "the server waited for a client reading to the close"
        );
    }

    #[test]
    fn the_client_closes_first_after_a_response_with_a_length() {
        let handle = Server::bind(0)
            .expect("bind")
            .spawn(move |_request: &Request| Response::text(200, "ok"))
            .expect("spawn");
        let mut stream = TcpStream::connect(("127.0.0.1", handle.port())).expect("connect");
        stream
            .write_all(b"GET /x HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
            .expect("send");
        // Read the whole response (it ends with the body "ok").
        let mut reply = Vec::new();
        let mut buf = [0u8; 1024];
        while !reply.ends_with(b"ok") {
            let n = stream.read(&mut buf).expect("read");
            assert!(n > 0, "the server closed before the whole response arrived");
            reply.extend_from_slice(&buf[..n]);
        }
        // The server has not closed: it waits for this client to close first.
        stream
            .set_read_timeout(Some(Duration::from_millis(300)))
            .expect("timeout");
        let err = stream
            .read(&mut buf)
            .expect_err("the server should still be waiting");
        assert!(matches!(
            err.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
    }

    #[test]
    fn a_relayed_loopback_connection_is_served_as_one_from_elsewhere() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let handle = Server::bind(0)
            .expect("bind")
            .spawn(move |request: &Request| {
                sink.lock().expect("lock").push(request.peer);
                Response::text(200, "ok")
            })
            .expect("spawn");
        let send = |bytes: &[&[u8]]| {
            let mut stream = TcpStream::connect(("127.0.0.1", handle.port())).expect("connect");
            for part in bytes {
                stream.write_all(part).expect("send");
                thread::sleep(Duration::from_millis(20));
            }
            let mut reply = String::new();
            stream.read_to_string(&mut reply).expect("read");
            reply
        };
        let get =
            b"GET /x HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n".as_slice();
        // The preface whole, then in two writes: either way it is consumed.
        assert!(send(&[RELAY_PREFACE, get]).starts_with("HTTP/1.1 200"));
        let (head, tail) = RELAY_PREFACE.split_at(4);
        assert!(send(&[head, tail, get]).starts_with("HTTP/1.1 200"));
        // A relayed caller may name any host, as a caller from another machine may.
        let other = b"GET /x HTTP/1.1\r\nHost: hub\r\n\r\n".as_slice();
        assert!(send(&[RELAY_PREFACE, other]).starts_with("HTTP/1.1 200"));
        assert!(send(&[get]).starts_with("HTTP/1.1 200"));
        let unspecified = Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let loopback = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(
            *seen.lock().expect("lock"),
            [unspecified, unspecified, unspecified, loopback]
        );
    }

    #[test]
    fn a_tls_request_is_served_after_the_handshake() {
        let key = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("a certificate");
        let cert = key.cert.der().clone();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let server_key = rustls::pki_types::PrivatePkcs8KeyDer::from(key.key_pair.serialize_der());
        let config = ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .expect("protocol versions")
            .with_no_client_auth()
            .with_single_cert(vec![cert.clone()], server_key.into())
            .expect("the certificate and key");
        let handle = Server::bind(0)
            .expect("bind")
            .tls(Arc::new(config))
            .spawn(|_request: &Request| Response::text(200, "secure"))
            .expect("spawn");

        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert).expect("the certificate is trusted");
        let client = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from("localhost").expect("a name");
        let connection = rustls::ClientConnection::new(Arc::new(client), name).expect("a client");
        let socket = TcpStream::connect(("127.0.0.1", handle.port())).expect("connect");
        let mut tls = StreamOwned::new(connection, socket);
        tls.write_all(b"GET /x HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .expect("send");
        let mut reply = Vec::new();
        // The server closes without a TLS close_notify, so the read ends in an
        // error after the whole reply has arrived.
        let _ = tls.read_to_end(&mut reply);
        let reply = String::from_utf8_lossy(&reply);
        assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
        assert!(reply.ends_with("secure"), "{reply}");
    }

    /// Sends `parts` one after another on a fresh connection and returns the whole reply.
    fn send_parts(port: u16, parts: &[&[u8]]) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        for part in parts {
            stream.write_all(part).expect("send");
        }
        let mut reply = String::new();
        stream.read_to_string(&mut reply).expect("read");
        reply
    }

    #[test]
    fn a_refused_request_is_answered_without_reading_its_body() {
        // The default limits refuse a body this long in the head, before `admit` is asked: raise them so
        // the hook decides.
        let handle = Server::bind(0)
            .expect("bind")
            .limits(Limits {
                max_body: 16 << 20,
                ..Limits::default()
            })
            .admit(|_request: &Request| Err(Response::error(401, "no")))
            .spawn(|_request: &Request| Response::text(200, "handled"))
            .expect("spawn");
        let mut stream = TcpStream::connect(("127.0.0.1", handle.port())).expect("connect");
        // The head announces ten MiB and no body bytes ever follow it.
        stream
            .write_all(b"POST /x HTTP/1.1\r\nHost: localhost\r\nContent-Length: 10485760\r\n\r\n")
            .expect("send");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let started = Instant::now();
        let mut reply = String::new();
        stream
            .read_to_string(&mut reply)
            .expect("the refusal arrives while the body is still unsent");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert!(reply.starts_with("HTTP/1.1 401"), "{reply}");
    }

    #[test]
    fn a_body_over_the_admitted_allowance_is_refused_and_one_within_it_reaches_the_handler() {
        let handle = Server::bind(0)
            .expect("bind")
            .admit(|_request: &Request| Ok(16))
            .spawn(|request: &Request| Response::text(200, &request.body.len().to_string()))
            .expect("spawn");
        // Only the head is sent: the 413 comes before any body would be read.
        let over = send_parts(
            handle.port(),
            &[b"POST /x HTTP/1.1\r\nHost: localhost\r\nContent-Length: 17\r\n\r\n".as_slice()],
        );
        assert!(over.starts_with("HTTP/1.1 413"), "{over}");
        let within = send_parts(
            handle.port(),
            &[
                b"POST /x HTTP/1.1\r\nHost: localhost\r\nContent-Length: 16\r\n\r\n".as_slice(),
                b"0123456789abcdef".as_slice(),
            ],
        );
        assert!(within.starts_with("HTTP/1.1 200"), "{within}");
        assert!(within.ends_with("16"), "{within}");
    }

    /// The socket pair's end is a [`ws::Duplex`] too, so [`Deadlined`] can wrap it.
    #[cfg(unix)]
    impl ws::Duplex for std::os::unix::net::UnixStream {
        fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            std::os::unix::net::UnixStream::set_read_timeout(self, timeout)
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_deadline_ends_a_read_that_waits_past_it() {
        use std::os::unix::net::UnixStream;
        let mut buf = [0u8; 8];

        // A deadline already passed: the read fails at once, as TimedOut.
        let (near, _far) = UnixStream::pair().expect("a socket pair");
        let past = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .expect("an instant 1 ms back");
        let mut stream = Deadlined {
            inner: near,
            at_timeout: Cell::new(true),
            deadline: Some(past),
        };
        let err = stream.read(&mut buf).expect_err("the deadline has passed");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);

        // A deadline 100 ms ahead and nothing written: the wait ends within a second. The socket's
        // own timeout ends the wait itself, which on Unix surfaces as WouldBlock, not TimedOut.
        let (near, _far) = UnixStream::pair().expect("a socket pair");
        let mut stream = Deadlined {
            inner: near,
            at_timeout: Cell::new(true),
            deadline: Some(Instant::now() + Duration::from_millis(100)),
        };
        let started = Instant::now();
        let err = stream.read(&mut buf).expect_err("nothing was written");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert!(
            matches!(
                err.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn without_a_deadline_a_read_waits_for_the_peer() {
        use std::os::unix::net::UnixStream;
        let (near, mut far) = UnixStream::pair().expect("a socket pair");
        far.write_all(b"x").expect("send");
        let mut stream = Deadlined {
            inner: near,
            at_timeout: Cell::new(true),
            deadline: None,
        };
        let mut buf = [0u8; 8];
        assert_eq!(stream.read(&mut buf).expect("the byte the peer wrote"), 1);
        assert_eq!(buf[0], b'x');
    }
}
