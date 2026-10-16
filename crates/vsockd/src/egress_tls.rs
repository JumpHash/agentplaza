//! Credential injection for intercepted HTTPS (the design is in
//! `crates/apid/src/egress_inject.rs`). The owner's rule adds a secret header to an agent's request
//! to a host, so the secret never enters the VM.
//!
//! [`intercept`] runs after the proxy has answered `200 Connection established` for a CONNECT whose
//! decision said `intercept`:
//! 1. The guest's TLS is terminated with the leaf apid signs for the host ([`server_config`]). The
//!    leaf is kept per (CID, host) until an hour before it expires.
//! 2. A TLS client connection is opened to the real host, verified against the webpki roots, over
//!    the addresses the policy checked. Its SNI and name are the host.
//! 3. Each request head the guest sends is read (16 KiB cap), parsed in origin form, and asked of
//!    apid (`egress_inject`). The answer's header replaces the guest's own copy of that header, and
//!    the head goes upstream with its body by its framing (`Content-Length`, or `chunked` relayed
//!    chunk by chunk through the trailers). An `Upgrade` request ends the parsing: the guest's
//!    bytes are copied upstream raw from then on.
//! 4. The upstream's bytes are relayed to the guest raw, as they arrive.
//!
//! ALPN is `http/1.1` on both sides. A request this module cannot frame, or whose injection apid
//! cannot decide, is not sent: the connection is closed with a logged reason. Nothing logged or
//! audited carries a header value or a key.
//!
//! Threads and locks. A rustls connection is one state machine for both directions, so each TLS
//! endpoint is a [`Side`]: its `Connection` behind a mutex that is held only for CPU work (no
//! socket is touched under it), and a queue of encrypted bytes drained by one writer thread. Bytes
//! are queued under the lock, so records reach the socket in the order they were encrypted. Each
//! connection has, besides the caller's thread, a reader and a writer per side:
//! - the guest reader decrypts the guest's bytes and hands the plaintext to the caller;
//! - the upstream reader decrypts the upstream's bytes and queues them for the guest, raw;
//! - the guest writer and the upstream writer are the only threads that block on a socket write.
//!
//! No thread waits on a cycle. Every wait is a socket read with a poll timeout, a mutex held for
//! CPU work, or a queue push that never blocks, except two deliberate waits: the guest reader
//! pauses while more than [`WINDOW`] bytes wait for the upstream (backpressure to the guest), and
//! the caller waits on apid. The upstream reader never waits on anything but its socket, so the
//! upstream always drains, which is what lets the guest reader's pause end. The other direction is
//! bounded differently: a guest that stops reading its responses ends the connection past
//! [`QUEUE_LIMIT`] queued bytes, rather than stalling the upstream reader.
//!
//! When one side ends, the link is hung up: both sockets are shut down and every thread notices
//! within a poll. A side that ends cleanly (the upstream's TLS close, or a socket EOF) first queues
//! its close_notify and a `Close` behind its pending bytes, so the guest still gets the response it
//! was owed.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use apz_uds::UnixStream;
use rustls::crypto::ring::default_provider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{
    ClientConfig, ClientConnection, Connection, RootCertStore, ServerConfig, ServerConnection,
};

use super::{CONNECT_TIMEOUT, COPY_BUF, HEAD_TIMEOUT, IDLE_TIMEOUT, PUMP_POLL};
use crate::apid::{Apid, Injection};

/// The ALPN protocol both sides speak. The proxy only reads and writes HTTP/1.1.
const ALPN: &[u8] = b"http/1.1";
/// Plaintext the guest may have sent ahead of the upstream: the guest reader pauses past this.
const WINDOW: usize = 1024 * 1024;
/// Bytes queued for the guest that it has not read: past this the connection is ended.
const QUEUE_LIMIT: usize = 256 * 1024 * 1024;
/// A request head, a chunk-size line, and a trailer line are each capped at this many bytes.
const MAX_HEAD: usize = 16 * 1024;
/// A cached leaf is dropped this long before it expires, so a fresh one is fetched in time.
const LEAF_MARGIN: Duration = Duration::from_secs(60 * 60);
/// How long a paused guest reader sleeps between checks of the upstream queue.
const WINDOW_POLL: Duration = Duration::from_millis(5);

/// Bytes one intercepted connection moved, and the requests it injected into, for its audit line.
#[derive(Debug, Default)]
pub struct Tally {
    pub up: u64,
    pub down: u64,
    pub injected: u64,
}

/// Where the guest's connection goes: the agent, the host and port the policy checked, and the
/// addresses that were checked (the only ones dialled).
pub struct Target<'a> {
    pub cid: u32,
    pub host: &'a str,
    pub port: u16,
    pub addrs: &'a [SocketAddr],
}

/// Intercepts the guest's TLS on an accepted CONNECT (see the module docs). Returns when the
/// connection has ended; `Err` carries the reason it ended early. `tally` is filled as it goes.
pub fn intercept(
    guest: &mut UnixStream,
    target: &Target,
    apid: &Apid,
    tally: &mut Tally,
) -> Result<(), String> {
    let config = client_config(default_roots())?;
    intercept_with(guest, target, apid, tally, config)
}

/// [`intercept`] with the roots the upstream is verified against: the webpki roots, or in tests a
/// root of the test's own.
fn intercept_with(
    guest: &mut UnixStream,
    target: &Target,
    apid: &Apid,
    tally: &mut Tally,
    config: Arc<ClientConfig>,
) -> Result<(), String> {
    let server = server_config(apid, target.cid, target.host)?;
    guest
        .set_read_timeout(Some(HEAD_TIMEOUT))
        .map_err(io_text)?;
    let mut guest_tls = Connection::Server(ServerConnection::new(server).map_err(tls_text)?);
    guest_tls
        .complete_io(guest)
        .map_err(|err| format!("the guest's TLS handshake failed: {err}"))?;

    let mut upstream = dial(target.addrs)?;
    upstream
        .set_read_timeout(Some(HEAD_TIMEOUT))
        .map_err(io_text)?;
    let name = ServerName::try_from(target.host.to_string())
        .map_err(|_| format!("{} is not a host name", target.host))?;
    let mut upstream_tls =
        Connection::Client(ClientConnection::new(config, name).map_err(tls_text)?);
    upstream_tls
        .complete_io(&mut upstream)
        .map_err(|err| format!("the upstream TLS handshake failed: {err}"))?;

    guest.set_read_timeout(Some(PUMP_POLL)).map_err(io_text)?;
    upstream
        .set_read_timeout(Some(PUMP_POLL))
        .map_err(io_text)?;
    // Every clone is made before any thread starts, so a failure here leaves nothing running.
    let link = Arc::new(Link {
        done: AtomicBool::new(false),
        guest: clone_unix(guest)?,
        upstream,
        reason: Mutex::new(None),
    });
    let (guest_read, guest_write) = (clone_unix(guest)?, clone_unix(guest)?);

    let (guest_side, guest_queue) = Side::new(guest_tls);
    let (up_side, up_queue) = Side::new(upstream_tls);
    let (guest_side, up_side) = (Arc::new(guest_side), Arc::new(up_side));
    let (to_caller, from_guest) = mpsc::channel();
    // A handshake can leave decrypted bytes buffered inside rustls, and the readers only drain on
    // a read, so the buffered bytes are drained here before any reader starts.
    let (early, _) = guest_side.receive(&[])?;
    if !early.is_empty() {
        let _ = to_caller.send(early);
    }
    let (early, _) = up_side.receive(&[])?;
    if !early.is_empty() {
        guest_side.send(&early)?;
    }

    {
        let (up, link) = (Arc::clone(&up_side), Arc::clone(&link));
        let guest_side = Arc::clone(&guest_side);
        let _ = thread::spawn(move || {
            guest_read_loop(guest_read, &guest_side, &up, &link, to_caller);
        });
    }
    {
        let (side, link) = (Arc::clone(&guest_side), Arc::clone(&link));
        let _ = thread::spawn(move || write_loop(guest_write, guest_queue, &side, &link, "guest"));
    }
    {
        let (side, link) = (Arc::clone(&up_side), Arc::clone(&link));
        let _ = thread::spawn(move || {
            write_loop(&link.upstream, up_queue, &side, &link, "upstream");
        });
    }
    let up_reader = {
        let (up, guest_side, link) = (
            Arc::clone(&up_side),
            Arc::clone(&guest_side),
            Arc::clone(&link),
        );
        thread::spawn(move || upstream_read_loop(&link.upstream, &up, &guest_side, &link))
    };

    let mut inbound = Inbound {
        rx: from_guest,
        buf: Vec::new(),
        pos: 0,
        link: Arc::clone(&link),
        last: Instant::now(),
    };
    let result = relay(&mut inbound, target, apid, &up_side, tally);
    link.hang_up();
    tally.down = up_reader.join().unwrap_or(0);
    // A reason recorded by any thread is the one reported: it is the failure, and the caller's own
    // error is only its echo.
    match link.reason() {
        Some(reason) => Err(reason),
        None => result,
    }
}

/// The two sockets of one intercepted connection, the flag that ends it, and the first reason it
/// was ended for. Shared by every thread of the connection.
///
/// The guest socket is a clone; the upstream is the connection's own TCP handle, which its reader
/// and writer use too (`&TcpStream`), so its read timeout is theirs. Not a `try_clone`: on Windows
/// a clone is a `WSADuplicateSocketW` handle whose shutdown now and then fails with WSAENOTCONN,
/// so the upstream would not see the hang-up, and [`crate::sys::wake_readers`] cancels only the
/// reads issued on the handle it is given.
struct Link {
    done: AtomicBool,
    guest: UnixStream,
    upstream: TcpStream,
    reason: Mutex<Option<String>>,
}

impl Link {
    fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// Ends the connection because of `reason`, which the caller reports (the first one wins).
    fn fail(&self, reason: impl Into<String>) {
        // Once the link is ending (a clean close, or an earlier failure), its errors are its echo:
        // the socket errors a hang-up causes are not a reason of their own.
        if self.is_done() {
            return;
        }
        let mut kept = locked(&self.reason);
        if kept.is_none() {
            *kept = Some(reason.into());
        }
        drop(kept);
        self.hang_up();
    }

    fn reason(&self) -> Option<String> {
        locked(&self.reason).clone()
    }

    /// Stops every thread: the flag for the readers, and both sockets shut down so a blocked write
    /// returns.
    fn hang_up(&self) {
        self.done.store(true, Ordering::SeqCst);
        let _ = self.guest.shutdown(Shutdown::Both);
        let _ = self.upstream.shutdown(Shutdown::Both);
        crate::sys::wake_readers(&self.upstream);
    }
}

/// What a writer thread is given: bytes for its socket, or the end of the stream.
enum Out {
    Bytes(Vec<u8>),
    Close,
}

/// One TLS endpoint: its connection, the queue of encrypted bytes for its socket, and how many of
/// those bytes are still waiting to be written.
struct Side {
    tls: Mutex<Connection>,
    queue: Sender<Out>,
    waiting: AtomicUsize,
}

impl Side {
    fn new(tls: Connection) -> (Self, Receiver<Out>) {
        let (queue, rx) = mpsc::channel();
        let side = Self {
            tls: Mutex::new(tls),
            queue,
            waiting: AtomicUsize::new(0),
        };
        (side, rx)
    }

    /// Encrypts `data` for this endpoint and queues it. Never blocks on a socket.
    fn send(&self, data: &[u8]) -> Result<(), String> {
        for chunk in data.chunks(COPY_BUF) {
            let mut tls = locked(&self.tls);
            tls.writer()
                .write_all(chunk)
                .map_err(|err| format!("TLS write failed: {err}"))?;
            self.flush(&mut tls)?;
        }
        Ok(())
    }

    /// Feeds bytes read from this endpoint's socket through the connection. Returns the plaintext
    /// they complete, and whether the peer ended its TLS stream. Queues any TLS output (handshake
    /// replies, alerts) the bytes call for. Never blocks on a socket.
    fn receive(&self, raw: &[u8]) -> Result<(Vec<u8>, bool), String> {
        let mut tls = locked(&self.tls);
        let mut rest = raw;
        loop {
            if !rest.is_empty() {
                tls.read_tls(&mut rest)
                    .map_err(|err| format!("TLS read failed: {err}"))?;
            }
            tls.process_new_packets()
                .map_err(|err| format!("TLS failed: {err}"))?;
            if rest.is_empty() {
                break;
            }
        }
        let mut plain = Vec::new();
        let mut buf = [0u8; COPY_BUF];
        let mut closed = false;
        loop {
            match tls.reader().read(&mut buf) {
                Ok(0) => {
                    closed = true;
                    break;
                }
                Ok(n) => plain.extend_from_slice(&buf[..n]),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) => return Err(format!("TLS read failed: {err}")),
            }
        }
        self.flush(&mut tls)?;
        Ok((plain, closed))
    }

    /// Queues the connection's pending encrypted output. The caller holds the lock, so the records
    /// are queued in the order they were made.
    fn flush(&self, tls: &mut Connection) -> Result<(), String> {
        let mut out = Vec::new();
        while tls.wants_write() {
            tls.write_tls(&mut out)
                .map_err(|err| format!("TLS write failed: {err}"))?;
        }
        if out.is_empty() {
            return Ok(());
        }
        self.waiting.fetch_add(out.len(), Ordering::SeqCst);
        self.queue
            .send(Out::Bytes(out))
            .map_err(|_| "the connection's writer ended".to_string())
    }
}

/// Ends one side's TLS stream: close_notify, then the writer hangs up once it has written what came
/// before. Used when the other end closed cleanly.
fn finish(side: &Side) {
    let mut tls = locked(&side.tls);
    tls.send_close_notify();
    let _ = side.flush(&mut tls);
    let _ = side.queue.send(Out::Close);
}

/// Writes one side's queue to its socket until the queue ends, a write fails, or the link ends. The
/// writer is the only thread that blocks on this socket's writes, so a stalled peer stalls only it.
fn write_loop(mut socket: impl Write, queue: Receiver<Out>, side: &Side, link: &Link, which: &str) {
    loop {
        match queue.recv_timeout(PUMP_POLL) {
            Ok(Out::Bytes(bytes)) => {
                let written = socket.write_all(&bytes);
                side.waiting.fetch_sub(bytes.len(), Ordering::SeqCst);
                if let Err(err) = written {
                    link.fail(format!("the {which} socket write failed: {err}"));
                    break;
                }
            }
            Ok(Out::Close) => break,
            Err(RecvTimeoutError::Timeout) => {
                if link.is_done() {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    link.hang_up();
}

/// Reads the guest's socket, decrypts it, and hands the plaintext to the caller. Pauses while the
/// upstream is more than [`WINDOW`] bytes behind. Ends when the guest closes, the caller stops
/// listening (dropping `to_caller` tells it so), or the link ends.
fn guest_read_loop(
    mut raw: UnixStream,
    side: &Side,
    up: &Side,
    link: &Link,
    to_caller: Sender<Vec<u8>>,
) {
    let mut buf = vec![0u8; COPY_BUF];
    let mut last = Instant::now();
    loop {
        while up.waiting.load(Ordering::SeqCst) > WINDOW && !link.is_done() {
            thread::sleep(WINDOW_POLL);
        }
        if link.is_done() {
            return;
        }
        match raw.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                last = Instant::now();
                match side.receive(&buf[..n]) {
                    Ok((plain, closed)) => {
                        if !plain.is_empty() && to_caller.send(plain).is_err() {
                            return;
                        }
                        if closed {
                            return;
                        }
                    }
                    Err(reason) => {
                        link.fail(reason);
                        return;
                    }
                }
            }
            Err(err) if is_poll(&err) => {
                if link.is_done() {
                    return;
                }
                if last.elapsed() >= IDLE_TIMEOUT {
                    link.fail("the guest was idle");
                    return;
                }
            }
            Err(err) => {
                link.fail(format!("the guest socket read failed: {err}"));
                return;
            }
        }
    }
}

/// Reads the upstream's socket, decrypts it, and queues the plaintext for the guest, raw. It never
/// waits on the guest: a guest that stops reading ends the connection past [`QUEUE_LIMIT`]. Returns
/// the bytes relayed.
fn upstream_read_loop(mut raw: &TcpStream, up: &Side, guest: &Side, link: &Link) -> u64 {
    let mut buf = vec![0u8; COPY_BUF];
    let mut total = 0u64;
    let mut last = Instant::now();
    loop {
        match raw.read(&mut buf) {
            Ok(0) => {
                finish(guest);
                return total;
            }
            Ok(n) => {
                last = Instant::now();
                let (plain, closed) = match up.receive(&buf[..n]) {
                    Ok(received) => received,
                    Err(reason) => {
                        link.fail(reason);
                        return total;
                    }
                };
                if guest.waiting.load(Ordering::SeqCst) > QUEUE_LIMIT {
                    link.fail("the guest stopped reading its responses");
                    return total;
                }
                if !plain.is_empty() {
                    total += plain.len() as u64;
                    if let Err(reason) = guest.send(&plain) {
                        link.fail(reason);
                        return total;
                    }
                }
                if closed {
                    finish(guest);
                    return total;
                }
            }
            Err(err) if is_poll(&err) => {
                if link.is_done() {
                    return total;
                }
                if last.elapsed() >= IDLE_TIMEOUT {
                    link.fail("the upstream was idle");
                    return total;
                }
            }
            Err(err) => {
                link.fail(format!("the upstream socket read failed: {err}"));
                return total;
            }
        }
    }
}

/// The guest's plaintext as the caller reads it: a buffer over the guest reader's channel.
struct Inbound {
    rx: Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
    link: Arc<Link>,
    last: Instant,
}

impl Inbound {
    /// Makes at least one unread byte available. `Ok(false)` when the guest reader has ended.
    fn fill(&mut self) -> Result<bool, String> {
        while self.pos >= self.buf.len() {
            match self.rx.recv_timeout(PUMP_POLL) {
                Ok(bytes) => {
                    self.buf = bytes;
                    self.pos = 0;
                    self.last = Instant::now();
                }
                Err(RecvTimeoutError::Timeout) => {
                    // Ended by the other side (a failure is reported through the link's reason).
                    if self.link.is_done() {
                        return Ok(false);
                    }
                    if self.last.elapsed() >= IDLE_TIMEOUT {
                        return Err("the guest was idle".to_string());
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(false),
            }
        }
        Ok(true)
    }

    /// The next request head, through its blank line; `None` when the guest closed between requests.
    fn head(&mut self) -> Result<Option<String>, String> {
        let mut head = Vec::new();
        loop {
            if !self.fill()? {
                return if head.is_empty() {
                    Ok(None)
                } else {
                    Err("the guest closed inside a request head".to_string())
                };
            }
            while self.pos < self.buf.len() {
                head.push(self.buf[self.pos]);
                self.pos += 1;
                if head.ends_with(b"\r\n\r\n") {
                    return String::from_utf8(head)
                        .map(Some)
                        .map_err(|_| "the request head is not text".to_string());
                }
                if head.len() > MAX_HEAD {
                    return Err("the request head is over 16 KiB".to_string());
                }
            }
        }
    }

    /// The next line, through its LF (a chunk-size line or a trailer line).
    fn line(&mut self) -> Result<Vec<u8>, String> {
        let mut line = Vec::new();
        loop {
            if !self.fill()? {
                return Err("the guest closed inside a body".to_string());
            }
            while self.pos < self.buf.len() {
                line.push(self.buf[self.pos]);
                self.pos += 1;
                if line.ends_with(b"\n") {
                    return Ok(line);
                }
                if line.len() > MAX_HEAD {
                    return Err("a chunk line is over 16 KiB".to_string());
                }
            }
        }
    }

    /// Hands the next `n` bytes to `sink` as they arrive.
    fn forward(
        &mut self,
        mut n: u64,
        sink: &mut impl FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        while n > 0 {
            if !self.fill()? {
                return Err("the guest closed inside a body".to_string());
            }
            let available = self.buf.len() - self.pos;
            let take = available.min(usize::try_from(n).unwrap_or(usize::MAX));
            sink(&self.buf[self.pos..self.pos + take])?;
            self.pos += take;
            n -= take as u64;
        }
        Ok(())
    }

    /// Hands every remaining byte to `sink` until the guest reader ends.
    fn drain(&mut self, sink: &mut impl FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
        while self.fill()? {
            sink(&self.buf[self.pos..])?;
            self.pos = self.buf.len();
        }
        Ok(())
    }
}

/// The request framing the proxy relays.
#[derive(Debug, PartialEq, Eq)]
enum Framing {
    None,
    Length(u64),
    Chunked,
}

/// A parsed request head.
#[derive(Debug, PartialEq, Eq)]
struct Request {
    method: String,
    target: String,
    framing: Framing,
    upgrade: bool,
}

impl Request {
    /// The path the owner's rules match: the target without its query.
    fn path_for_rules(&self) -> &str {
        self.target.split('?').next().unwrap_or("/")
    }
}

/// Parses a request head (through its blank line). Only origin-form targets are injected, and a
/// head whose body framing is ambiguous is refused, so the body is relayed exactly as the guest
/// meant it.
fn parse_request(head: &str) -> Result<Request, String> {
    let head = head
        .strip_suffix("\r\n\r\n")
        .ok_or("the request head is not complete")?;
    let mut lines = head.split("\r\n");
    let line = lines.next().unwrap_or("");
    let parts: Vec<&str> = line.split(' ').collect();
    let &[method, target, version] = parts.as_slice() else {
        return Err("the request line is not METHOD TARGET VERSION".to_string());
    };
    if !version.starts_with("HTTP/1.")
        || method.is_empty()
        || !method.bytes().all(|byte| byte.is_ascii_uppercase())
    {
        return Err("the request line is not HTTP/1.x".to_string());
    }
    if !target.starts_with('/') {
        return Err("only origin-form request targets are relayed".to_string());
    }
    let mut length = None;
    let mut chunked = false;
    let mut upgrade = false;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or("a header line has no colon")?;
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("content-length") {
            let parsed: u64 = value
                .parse()
                .map_err(|_| "Content-Length is not a number".to_string())?;
            if length
                .replace(parsed)
                .is_some_and(|before| before != parsed)
            {
                return Err("two different Content-Length headers".to_string());
            }
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if !value.eq_ignore_ascii_case("chunked") {
                return Err("only chunked Transfer-Encoding is relayed".to_string());
            }
            chunked = true;
        } else if name.eq_ignore_ascii_case("upgrade") {
            upgrade = true;
        }
    }
    if chunked && length.is_some() {
        return Err("Content-Length and Transfer-Encoding are both set".to_string());
    }
    let framing = match (chunked, length) {
        (true, _) => Framing::Chunked,
        (false, Some(n)) => Framing::Length(n),
        (false, None) => Framing::None,
    };
    Ok(Request {
        method: method.to_string(),
        target: target.to_string(),
        framing,
        upgrade,
    })
}

/// The request head with `injection` added and the guest's own copies of its header removed. The
/// request line and every other header are kept byte for byte.
fn rewrite_head(head: &str, injection: Option<&Injection>) -> String {
    let Some(injection) = injection else {
        return head.to_string();
    };
    let lines = head.strip_suffix("\r\n\r\n").unwrap_or(head);
    let mut out =
        String::with_capacity(head.len() + injection.header.len() + injection.value.len());
    for (index, line) in lines.split("\r\n").enumerate() {
        // The request line is index 0 and is never a header.
        let ours = index > 0
            && line
                .split_once(':')
                .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case(&injection.header));
        if !ours {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    out.push_str(&injection.header);
    out.push_str(": ");
    out.push_str(&injection.value);
    out.push_str("\r\n\r\n");
    out
}

/// Copies a chunked body to `sink` byte for byte, through its last chunk and trailers. The next
/// request's bytes are left unread.
fn relay_chunked(
    input: &mut Inbound,
    sink: &mut impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    loop {
        let size_line = input.line()?;
        sink(&size_line)?;
        let text = std::str::from_utf8(&size_line).map_err(|_| "a chunk size is not text")?;
        let digits = text
            .trim_end_matches(['\r', '\n'])
            .split(';')
            .next()
            .unwrap_or("");
        let size = u64::from_str_radix(digits.trim(), 16)
            .map_err(|_| "a chunk size is not hex".to_string())?;
        if size == 0 {
            loop {
                let trailer = input.line()?;
                sink(&trailer)?;
                if trailer == b"\r\n" || trailer == b"\n" {
                    return Ok(());
                }
            }
        }
        let framed = size.checked_add(2).ok_or("a chunk size overflows")?;
        input.forward(framed, sink)?;
    }
}

/// Counts and queues bytes for the upstream.
fn send_up(up: &Side, sent: &mut u64, bytes: &[u8]) -> Result<(), String> {
    *sent += bytes.len() as u64;
    up.send(bytes)
}

/// Relays the guest's requests upstream, one at a time, each with apid's injection. Returns when
/// the guest is done, or after an upgrade's bytes have been copied.
fn relay(
    inbound: &mut Inbound,
    target: &Target,
    apid: &Apid,
    up: &Side,
    tally: &mut Tally,
) -> Result<(), String> {
    while let Some(head) = inbound.head()? {
        let request = parse_request(&head)?;
        // Fail closed: a request whose injection cannot be decided is not sent.
        let injection = apid
            .egress_inject(
                target.cid,
                target.host,
                target.port,
                &request.method,
                request.path_for_rules(),
            )
            .map_err(|err| format!("apid could not decide the injection: {err}"))?;
        if injection.is_some() {
            tally.injected += 1;
        }
        let out = rewrite_head(&head, injection.as_ref());
        send_up(up, &mut tally.up, out.as_bytes())?;
        match request.framing {
            Framing::None => {}
            Framing::Length(n) => {
                inbound.forward(n, &mut |bytes: &[u8]| send_up(up, &mut tally.up, bytes))?;
            }
            Framing::Chunked => {
                relay_chunked(inbound, &mut |bytes: &[u8]| {
                    send_up(up, &mut tally.up, bytes)
                })?;
            }
        }
        if request.upgrade {
            return inbound.drain(&mut |bytes: &[u8]| send_up(up, &mut tally.up, bytes));
        }
    }
    Ok(())
}

/// A cached leaf: when it must be dropped, and its server config.
type CachedLeaf = (Instant, Arc<ServerConfig>);

/// A leaf's server config, cached per (CID, host). An entry is kept until an hour before the leaf's
/// expiry; a leaf whose expiry cannot be read is used but not kept.
static LEAVES: Mutex<BTreeMap<(u32, String), CachedLeaf>> = Mutex::new(BTreeMap::new());

/// The server config presenting apid's leaf for `host` to the guest of `cid`.
fn server_config(apid: &Apid, cid: u32, host: &str) -> Result<Arc<ServerConfig>, String> {
    let key = (cid, host.to_ascii_lowercase());
    if let Some((_, config)) = locked(&LEAVES)
        .get(&key)
        .filter(|(until, _)| Instant::now() < *until)
    {
        return Ok(Arc::clone(config));
    }
    let leaf = apid.egress_cert(cid, host)?;
    let (config, not_after) = server_from_pem(&leaf.cert_pem, &leaf.key_pem)?;
    let config = Arc::new(config);
    if let Some(until) = keep_until(not_after) {
        let mut kept = locked(&LEAVES);
        kept.retain(|_, (until, _)| Instant::now() < *until);
        kept.insert(key, (until, Arc::clone(&config)));
    }
    Ok(config)
}

/// A server config for a PEM leaf and its key, with ALPN `http/1.1`, and the leaf's notAfter.
fn server_from_pem(cert_pem: &str, key_pem: &str) -> Result<(ServerConfig, Option<i64>), String> {
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<_, _>>()
        .map_err(|_| "apid's certificate is not PEM".to_string())?;
    let first = chain.first().ok_or("apid's certificate is empty")?;
    let not_after = not_after_secs(first.as_ref());
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
        .map_err(|_| "apid's key is not PEM".to_string())?;
    let mut config = ServerConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(tls_text)?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(tls_text)?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok((config, not_after))
}

/// A client config that verifies the upstream against `roots`, with ALPN `http/1.1`.
fn client_config(roots: RootCertStore) -> Result<Arc<ClientConfig>, String> {
    let mut config = ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(tls_text)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(Arc::new(config))
}

/// The webpki roots, which the upstream's certificate must chain to.
fn default_roots() -> RootCertStore {
    RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned())
}

/// When a cache entry for a leaf expiring at `not_after` (Unix seconds) must be dropped: an hour
/// before the expiry, as an `Instant`. `None` when that moment is past or the expiry is unknown.
fn keep_until(not_after: Option<i64>) -> Option<Instant> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    let left = not_after? - now - LEAF_MARGIN.as_secs() as i64;
    (left > 0).then(|| Instant::now() + Duration::from_secs(left as u64))
}

/// The first address that accepts a connection, from the ones the policy checked.
fn dial(addrs: &[SocketAddr]) -> Result<TcpStream, String> {
    addrs
        .iter()
        .find_map(|addr| TcpStream::connect_timeout(addr, CONNECT_TIMEOUT).ok())
        .ok_or_else(|| "the upstream did not accept a connection".to_string())
}

/// The notAfter of a DER certificate, in seconds since the Unix epoch, or `None` when it does not
/// parse that far. Only the validity is needed, so the TBSCertificate is walked by hand.
fn not_after_secs(der: &[u8]) -> Option<i64> {
    let (_, certificate, _) = der_element(der)?;
    let (_, tbs, _) = der_element(certificate)?;
    let mut rest = tbs;
    if rest.first() == Some(&0xA0) {
        rest = der_element(rest)?.2; // the optional [0] version
    }
    for _ in 0..3 {
        rest = der_element(rest)?.2; // serialNumber, signature, issuer
    }
    let (_, validity, _) = der_element(rest)?;
    let (_, _, after_not_before) = der_element(validity)?;
    let (tag, not_after, _) = der_element(after_not_before)?;
    let text = std::str::from_utf8(not_after).ok()?.strip_suffix('Z')?;
    if !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, from) = match tag {
        // UTCTime (RFC 5280): two-digit years, 1950 to 2049.
        0x17 if text.len() == 12 => {
            let yy = number(0..2)?;
            (if yy < 50 { 2000 + yy } else { 1900 + yy }, 2)
        }
        0x18 if text.len() == 14 => (number(0..4)?, 4),
        _ => return None,
    };
    let month = number(from..from + 2)?;
    let day = number(from + 2..from + 4)?;
    let hour = number(from + 4..from + 6)?;
    let minute = number(from + 6..from + 8)?;
    let second = number(from + 8..from + 10)?;
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// One DER element: its tag, its content, and the bytes after it.
fn der_element(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *input.first()?;
    let first = *input.get(1)?;
    let (len, header) = if first < 0x80 {
        (first as usize, 2)
    } else {
        let count = (first & 0x7f) as usize;
        if count == 0 || count > 4 {
            return None;
        }
        let mut len = 0usize;
        for &byte in input.get(2..2 + count)? {
            len = (len << 8) | byte as usize;
        }
        (len, 2 + count)
    };
    let end = header.checked_add(len)?;
    Some((tag, input.get(header..end)?, input.get(end..)?))
}

/// Days from 1970-01-01 to the given civil date (proleptic Gregorian).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn is_poll(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

/// Locks a mutex, and reads through a poisoned one: every critical section here leaves its state
/// whole.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn clone_unix(stream: &UnixStream) -> Result<UnixStream, String> {
    stream.try_clone().map_err(io_text)
}

fn io_text(err: io::Error) -> String {
    format!("socket: {err}")
}

fn tls_text(err: impl Display) -> String {
    format!("TLS: {err}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::StreamOwned;
    use serde_json::json;
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::mpsc::Receiver as Rx;

    /// A CA and a leaf for `name` it signs: the leaf's certificate and key (PEM) and the CA's DER.
    fn issued(name: &str) -> (String, String, Vec<u8>) {
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let mut ca_params = rcgen::CertificateParams::new(vec![format!("{name} test CA")]).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let leaf = rcgen::CertificateParams::new(vec![name.to_string()])
            .unwrap()
            .signed_by(&key, &ca, &ca_key)
            .unwrap();
        (leaf.pem(), key.serialize_pem(), ca.der().to_vec())
    }

    fn roots_of(ca_der: &[u8]) -> RootCertStore {
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(ca_der.to_vec())).unwrap();
        roots
    }

    fn read_byte(stream: &mut impl Read) -> Option<u8> {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).ok()?;
        Some(byte[0])
    }

    fn read_line(stream: &mut impl Read) -> Option<Vec<u8>> {
        let mut line = Vec::new();
        while !line.ends_with(b"\n") {
            line.push(read_byte(stream)?);
        }
        Some(line)
    }

    /// One whole HTTP/1.1 message (head and body, by its framing) read from `stream`.
    fn read_message(stream: &mut impl Read) -> Option<(String, Vec<u8>)> {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(read_byte(stream)?);
        }
        let text = String::from_utf8(head).ok()?;
        let lower = text.to_ascii_lowercase();
        let mut body = Vec::new();
        if lower.contains("transfer-encoding: chunked") {
            loop {
                let size_line = read_line(stream)?;
                let size = std::str::from_utf8(&size_line)
                    .ok()?
                    .trim()
                    .split(';')
                    .next()?;
                let size = usize::from_str_radix(size, 16).ok()?;
                if size == 0 {
                    loop {
                        if read_line(stream)? == b"\r\n" {
                            break;
                        }
                    }
                    break;
                }
                for _ in 0..size {
                    body.push(read_byte(stream)?);
                }
                read_line(stream)?; // the CRLF after the data
            }
        } else if let Some(len) = lower
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse::<usize>().ok())
        {
            body = vec![0; len];
            stream.read_exact(&mut body).ok()?;
        }
        Some((text, body))
    }

    /// An upstream TLS server for `leaf`: answers `requests` requests on one connection with
    /// `ok:<request body>`, and reports each request's head and body.
    fn upstream_server(leaf: &str, key: &str, requests: usize) -> (u16, Rx<(String, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (config, _) = server_from_pem(leaf, key).unwrap();
        let (seen, reports) = mpsc::channel();
        thread::spawn(move || {
            let Ok((tcp, _)) = listener.accept() else {
                return;
            };
            tcp.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
            let mut tls = StreamOwned::new(ServerConnection::new(Arc::new(config)).unwrap(), tcp);
            for _ in 0..requests {
                let Some((head, body)) = read_message(&mut tls) else {
                    return;
                };
                let reply = format!("ok:{}", String::from_utf8_lossy(&body));
                let out = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{reply}",
                    reply.len()
                );
                if tls
                    .write_all(out.as_bytes())
                    .and_then(|()| tls.flush())
                    .is_err()
                {
                    return;
                }
                let _ = seen.send((head, body));
            }
        });
        (port, reports)
    }

    /// A stand-in for apid's internal surface. Every request must carry the test token. The cert is
    /// `leaf`/`key` for upstream.test (refused when `allow_cert` is false), and the injection is
    /// `x-api-key: sk-injected` for paths under /v1/. Returns apid, the count of injections asked,
    /// and the token file (the test removes it).
    fn fake_apid(
        name: &str,
        leaf: &str,
        key: &str,
        allow_cert: bool,
    ) -> (Apid, Arc<AtomicUsize>, PathBuf) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let token =
            std::env::temp_dir().join(format!("vsockd-egress-tls-{name}-{}", std::process::id()));
        std::fs::write(&token, "test-token\n").unwrap();
        let apid = Apid::new(&format!("http://127.0.0.1:{port}"), Some(token.clone())).unwrap();
        let injections = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&injections);
        let (leaf, key) = (leaf.to_string(), key.to_string());
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else {
                    continue;
                };
                conn.set_read_timeout(Some(Duration::from_secs(20)))
                    .unwrap();
                let Some((head, body)) = read_message(&mut conn) else {
                    continue;
                };
                let trusted = head
                    .to_ascii_lowercase()
                    .contains("x-apz-internal: test-token");
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
                let (status, reply) = if !trusted {
                    (403, json!({}))
                } else if head.starts_with("POST /internal/egress/cert ") {
                    if allow_cert && request["host"] == "upstream.test" {
                        (200, json!({ "cert_pem": leaf, "key_pem": key }))
                    } else {
                        (403, json!({}))
                    }
                } else if head.starts_with("POST /internal/egress/inject ") {
                    counter.fetch_add(1, Ordering::SeqCst);
                    if request["path"].as_str().unwrap_or("").starts_with("/v1/") {
                        (
                            200,
                            json!({ "header": "x-api-key", "value": "sk-injected", "rule": "r1" }),
                        )
                    } else {
                        (200, json!({}))
                    }
                } else {
                    (404, json!({}))
                };
                let text = reply.to_string();
                let out = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
                let _ = conn.write_all(out.as_bytes());
            }
        });
        (apid, injections, token)
    }

    /// A connection of the test's own: the link has sockets that nothing reads, for the framing tests.
    fn test_link() -> Arc<Link> {
        let (guest, _peer) = UnixStream::pair().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let upstream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        Arc::new(Link {
            done: AtomicBool::new(false),
            guest,
            upstream,
            reason: Mutex::new(None),
        })
    }

    #[test]
    fn a_request_head_names_its_framing_and_ambiguous_ones_are_refused() {
        let plain =
            parse_request("POST /v1/x?k=1 HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\n\r\n")
                .unwrap();
        assert_eq!(
            (plain.method.as_str(), plain.path_for_rules()),
            ("POST", "/v1/x")
        );
        assert_eq!(plain.framing, Framing::Length(5));
        assert!(!plain.upgrade);
        let chunked =
            parse_request("PUT /a HTTP/1.1\r\nTransfer-Encoding: Chunked\r\n\r\n").unwrap();
        assert_eq!(chunked.framing, Framing::Chunked);
        assert_eq!(
            parse_request("GET /a HTTP/1.1\r\n\r\n").unwrap().framing,
            Framing::None
        );
        assert!(
            parse_request("GET /a HTTP/1.1\r\nUpgrade: websocket\r\n\r\n")
                .unwrap()
                .upgrade
        );
        for bad in [
            "POST /a HTTP/1.1\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n",
            "POST /a HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\n",
            "POST /a HTTP/1.1\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
            "GET http://upstream.test/a HTTP/1.1\r\n\r\n",
            "GET /a HTTP/1.1\r\nBroken header\r\n\r\n",
            "GET /a\r\n\r\n",
        ] {
            assert!(parse_request(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn the_injected_header_replaces_the_guests_own_copy() {
        let head = "POST /v1/x HTTP/1.1\r\nHost: upstream.test\r\nX-Api-Key: guest-own\r\nAccept: */*\r\n\r\n";
        let injection = Injection {
            header: "x-api-key".into(),
            value: "sk-1".into(),
        };
        assert_eq!(
            rewrite_head(head, Some(&injection)),
            "POST /v1/x HTTP/1.1\r\nHost: upstream.test\r\nAccept: */*\r\nx-api-key: sk-1\r\n\r\n"
        );
        assert_eq!(rewrite_head(head, None), head);
    }

    #[test]
    fn chunked_framing_is_relayed_to_its_last_chunk_and_trailers() {
        let (feed, from_guest) = mpsc::channel();
        let mut inbound = Inbound {
            rx: from_guest,
            buf: Vec::new(),
            pos: 0,
            link: test_link(),
            last: Instant::now(),
        };
        // Pieces that split a chunk and a line, then the next request, whole.
        for piece in [
            "5\r\nwo",
            "rld\r\n6;ext=1\r\n chunk\r\n0\r\nX-T: 1\r\n\r\n",
            "GET /next HTTP/1.1\r\n\r\n",
        ] {
            feed.send(piece.as_bytes().to_vec()).unwrap();
        }
        let mut relayed = Vec::new();
        relay_chunked(&mut inbound, &mut |bytes: &[u8]| {
            relayed.extend_from_slice(bytes);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            relayed,
            b"5\r\nworld\r\n6;ext=1\r\n chunk\r\n0\r\nX-T: 1\r\n\r\n"
        );
        assert_eq!(
            inbound.head().unwrap().as_deref(),
            Some("GET /next HTTP/1.1\r\n\r\n")
        );
    }

    #[test]
    fn the_leaf_expiry_is_read_from_the_certificate() {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["upstream.test".to_string()]).unwrap();
        params.not_before = rcgen::date_time_ymd(2025, 1, 1);
        params.not_after = rcgen::date_time_ymd(2030, 1, 2);
        let cert = params.self_signed(&key).unwrap();
        // 2030-01-02T00:00:00Z.
        assert_eq!(not_after_secs(cert.der().as_ref()), Some(1_893_542_400));
        assert_eq!(not_after_secs(b"not a certificate"), None);
    }

    #[test]
    fn a_refused_certificate_ends_the_intercept_before_the_guest_is_answered() {
        let (leaf, key, _) = issued("upstream.test");
        let (apid, injections, token) = fake_apid("refused-cert", &leaf, &key, false);
        let (mut guest, mut proxy_side) = UnixStream::pair().unwrap();
        let addrs = [SocketAddr::from(([127, 0, 0, 1], 9))];
        let target = Target {
            cid: 11,
            host: "upstream.test",
            port: 443,
            addrs: &addrs,
        };
        let mut tally = Tally::default();
        let reason = intercept(&mut proxy_side, &target, &apid, &mut tally).unwrap_err();
        let _ = std::fs::remove_file(&token);
        assert!(reason.contains("403"), "{reason}");
        assert_eq!(injections.load(Ordering::SeqCst), 0);
        // Nothing was written to the guest: its next read is the end of the stream.
        drop(proxy_side);
        assert!(matches!(guest.read(&mut [0u8; 1]), Ok(0)));
    }

    #[test]
    fn an_intercepted_connection_injects_into_each_request_and_relays_both_ways() {
        let (upstream_leaf, upstream_key, upstream_ca) = issued("upstream.test");
        let (egress_leaf, egress_key, egress_ca) = issued("upstream.test");
        let (port, seen) = upstream_server(&upstream_leaf, &upstream_key, 2);
        let (apid, injections, token) = fake_apid("e2e", &egress_leaf, &egress_key, true);
        let config = client_config(roots_of(&upstream_ca)).unwrap();

        let (mut guest, mut proxy_side) = UnixStream::pair().unwrap();
        guest
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let addrs = [SocketAddr::from(([127, 0, 0, 1], port))];
        let (finished, ended) = mpsc::channel();
        thread::spawn(move || {
            let target = Target {
                cid: 9,
                host: "upstream.test",
                port: 443,
                addrs: &addrs,
            };
            let mut tally = Tally::default();
            let result = intercept_with(&mut proxy_side, &target, &apid, &mut tally, config);
            let _ = finished.send((result, tally));
        });

        let mut client_config = ClientConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots_of(&egress_ca))
            .with_no_client_auth();
        client_config.alpn_protocols = vec![ALPN.to_vec()];
        let mut client_tls = ClientConnection::new(
            Arc::new(client_config),
            ServerName::try_from("upstream.test").unwrap(),
        )
        .unwrap();
        client_tls
            .complete_io(&mut guest)
            .expect("the guest's handshake");
        let mut client = StreamOwned::new(client_tls, guest);

        client
            .write_all(
                b"POST /v1/messages HTTP/1.1\r\nHost: upstream.test\r\nX-Api-Key: guest-own\r\nContent-Length: 5\r\n\r\nhello",
            )
            .unwrap();
        client.flush().unwrap();
        let (head, body) = read_message(&mut client).expect("the first response");
        assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
        assert_eq!(body, b"ok:hello");

        client
            .write_all(
                b"POST /v1/chat HTTP/1.1\r\nHost: upstream.test\r\nTransfer-Encoding: chunked\r\nx-api-key: guest-again\r\n\r\n5\r\nworld\r\n6;ext=1\r\n chunk\r\n0\r\nX-Trailer: t\r\n\r\n",
            )
            .unwrap();
        client.flush().unwrap();
        let (_, body) = read_message(&mut client).expect("the second response");
        assert_eq!(body, b"ok:world chunk");
        drop(client);

        let (first_head, first_body) = seen.recv_timeout(Duration::from_secs(20)).unwrap();
        let (second_head, second_body) = seen.recv_timeout(Duration::from_secs(20)).unwrap();
        for head in [&first_head, &second_head] {
            assert_eq!(
                head.to_ascii_lowercase().matches("x-api-key:").count(),
                1,
                "{head}"
            );
            assert!(head.contains("x-api-key: sk-injected"), "{head}");
            assert!(
                !head.contains("guest-own") && !head.contains("guest-again"),
                "{head}"
            );
        }
        assert_eq!(first_body, b"hello");
        assert_eq!(second_body, b"world chunk");

        let (result, tally) = ended
            .recv_timeout(Duration::from_secs(20))
            .expect("the connection ends once the guest is done");
        let _ = std::fs::remove_file(&token);
        result.expect("the connection ends cleanly");
        assert_eq!(injections.load(Ordering::SeqCst), 2);
        assert_eq!(tally.injected, 2);
        assert!(tally.up > 0 && tally.down > 0, "{tally:?}");
    }

    #[test]
    fn an_upstream_that_is_not_trusted_ends_the_connection_with_a_reason() {
        let (upstream_leaf, upstream_key, _) = issued("upstream.test");
        let (egress_leaf, egress_key, egress_ca) = issued("upstream.test");
        let (port, _seen) = upstream_server(&upstream_leaf, &upstream_key, 1);
        let (apid, _injections, token) = fake_apid("untrusted", &egress_leaf, &egress_key, true);
        // The roots hold the egress CA, not the upstream's, so the upstream's chain is refused.
        let config = client_config(roots_of(&egress_ca)).unwrap();

        let (mut guest, mut proxy_side) = UnixStream::pair().unwrap();
        guest
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let addrs = [SocketAddr::from(([127, 0, 0, 1], port))];
        let (finished, ended) = mpsc::channel();
        thread::spawn(move || {
            let target = Target {
                cid: 10,
                host: "upstream.test",
                port: 443,
                addrs: &addrs,
            };
            let mut tally = Tally::default();
            let result = intercept_with(&mut proxy_side, &target, &apid, &mut tally, config);
            let _ = finished.send(result);
        });
        let mut client_config = ClientConfig::builder_with_provider(Arc::new(default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots_of(&egress_ca))
            .with_no_client_auth();
        client_config.alpn_protocols = vec![ALPN.to_vec()];
        let mut client_tls = ClientConnection::new(
            Arc::new(client_config),
            ServerName::try_from("upstream.test").unwrap(),
        )
        .unwrap();
        let _ = client_tls.complete_io(&mut guest);

        let result = ended
            .recv_timeout(Duration::from_secs(20))
            .expect("the connection ends");
        let _ = std::fs::remove_file(&token);
        let reason = result.expect_err("an untrusted upstream is refused");
        assert!(reason.contains("upstream TLS handshake"), "{reason}");
    }
}
