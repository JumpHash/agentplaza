//! The threaded server.

use std::io::{self, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustls::{ServerConfig, ServerConnection, StreamOwned};

use crate::{Limits, Request, Response, host_is_loopback, ws};

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

/// A bound, not yet serving, listener.
pub struct Server {
    listener: TcpListener,
    limits: Limits,
    tls: Option<Arc<ServerConfig>>,
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
        })
    }

    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
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
            let (limits, tls) = (self.limits, self.tls.clone());
            thread::spawn(move || {
                serve_accepted(stream, tls, &*handler, limits);
                active.fetch_sub(1, Ordering::SeqCst);
            });
        }
    }
}

/// Serves one accepted connection, as plain HTTP or over TLS when `tls` is set.
fn serve_accepted<H>(stream: TcpStream, tls: Option<Arc<ServerConfig>>, handler: &H, limits: Limits)
where
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
        return serve_connection(stream, peer, handler, limits);
    };
    let Ok(connection) = ServerConnection::new(config) else {
        return;
    };
    let mut tls = StreamOwned::new(connection, stream);
    // A client that fails the handshake has no request to answer: close quietly.
    while tls.conn.is_handshaking() {
        if tls.conn.complete_io(&mut tls.sock).is_err() {
            return;
        }
    }
    serve_connection(tls, peer, handler, limits);
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
        thread::sleep(Duration::from_millis(5));
    }
}

/// One request, one response, close. `peer` is the connection's remote address. An upgrade keeps
/// the connection: its closure gets the connection with whatever the request reading had already
/// buffered still to be read.
fn serve_connection<S, H>(stream: S, peer: Option<IpAddr>, handler: &H, limits: Limits)
where
    S: ws::Duplex + 'static,
    H: Fn(&Request) -> Response,
{
    let mut reader = BufReader::new(stream);
    let response = match Request::read(&mut reader, limits) {
        Err(message) => Response::error(400, message),
        Ok(mut request) => {
            request.peer = peer;
            if host_allowed(request.header("host"), peer) {
                handler(&request)
            } else {
                Response::error(403, "the Host header must name loopback")
            }
        }
    };
    let mut connection = Buffered(reader);
    // A client that hung up has nobody left to tell.
    if let Ok(Some(run)) = response.write_to(&mut connection) {
        run(Box::new(connection));
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
        let get = b"GET /x HTTP/1.1\r\nHost: localhost\r\n\r\n".as_slice();
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
}
