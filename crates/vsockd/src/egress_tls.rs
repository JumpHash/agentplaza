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
//!    chunk by chunk through the trailers). A head that two parsers could frame differently is
//!    refused (see `parse_request`). An `Upgrade` request is relayed like any other: the guest's
//!    bytes are copied upstream raw from then on only once the upstream answers
//!    `101 Switching Protocols`.
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
//! pauses while more than [`WINDOW`] bytes wait for the upstream or its bounded plaintext queue
//! is full (backpressure to the guest), and the caller waits on apid. The upstream reader only
//! waits on its socket. The caller drops the plaintext receiver before joining that reader, so
//! shutdown releases a blocked plaintext sender too. The other direction is
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
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
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
/// Plaintext queued while the caller awaits the hub: each item is at most COPY_BUF bytes.
const PLAINTEXT_CHUNKS: usize = WINDOW / COPY_BUF;
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
    let (to_caller, from_guest) = mpsc::sync_channel(PLAINTEXT_CHUNKS);
    // A handshake can leave decrypted bytes buffered inside rustls, and the readers only drain on
    // a read, so the buffered bytes are drained here before any reader starts.
    let (guest_early, _) = guest_side.receive(&[])?;
    if guest_early.len() > WINDOW {
        return Err("the guest sent too much plaintext with its handshake".into());
    }
    // Keep early plaintext directly in Inbound: a bounded send here would wait for a
    // receiver that cannot start until this setup finishes.
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
    // The upstream reader reads the status line of each Upgrade request's answer for `peek`.
    let (answers, answer_rx) = mpsc::channel();
    let peek = Arc::new(Peek {
        state: Mutex::new(None),
        answers,
    });
    let up_reader = {
        let (up, guest_side, link, peek) = (
            Arc::clone(&up_side),
            Arc::clone(&guest_side),
            Arc::clone(&link),
            Arc::clone(&peek),
        );
        thread::spawn(move || upstream_read_loop(&link.upstream, &up, &guest_side, &link, &peek))
    };

    let mut inbound = Inbound {
        rx: from_guest,
        buf: guest_early,
        pos: 0,
        link: Arc::clone(&link),
        last: Instant::now(),
    };
    let result = relay(
        &mut inbound,
        target,
        apid,
        &up_side,
        tally,
        &peek,
        &answer_rx,
    );
    link.hang_up();
    // A bounded plaintext sender may be blocked. Dropping the sole receiver releases
    // it before any worker join; socket shutdown alone does not wake channel sends.
    drop(inbound);
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
    /// This endpoint closed its end (EOF or its close_notify). A write that then fails is the echo of
    /// that close, not a failure of the connection.
    peer_closed: AtomicBool,
}

impl Side {
    fn new(tls: Connection) -> (Self, Receiver<Out>) {
        let (queue, rx) = mpsc::channel();
        let side = Self {
            tls: Mutex::new(tls),
            queue,
            waiting: AtomicUsize::new(0),
            peer_closed: AtomicBool::new(false),
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

/// Whether a write error is the peer having hung up (its socket closed or reset).
fn is_hang_up(err: &std::io::Error) -> bool {
    use std::io::ErrorKind::{BrokenPipe, ConnectionAborted, ConnectionReset, NotConnected};
    matches!(
        err.kind(),
        BrokenPipe | ConnectionReset | ConnectionAborted | NotConnected
    )
}

/// Whether this side's reader has seen (or within [`HANG_UP_GRACE`] sees) the peer close its end.
fn peer_closes_soon(side: &Side) -> bool {
    let deadline = Instant::now() + HANG_UP_GRACE;
    while !side.peer_closed.load(Ordering::SeqCst) {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
    true
}

/// How long a writer that saw the peer hang up waits for the reader to see the peer's close.
const HANG_UP_GRACE: Duration = Duration::from_millis(500);

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
                    // A peer that closed first (the guest dropping its connection after its last
                    // response, then our close_notify reaching it) is a clean end. The write can
                    // see the hang-up before this side's reader has read the peer's end, so a
                    // hang-up gets a moment for the reader to record it.
                    if !(is_hang_up(&err) && peer_closes_soon(side)) {
                        link.fail(format!("the {which} socket write failed: {err}"));
                    }
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
/// upstream is more than [`WINDOW`] bytes behind or the bounded plaintext queue is full.
/// Ends when the guest closes, the caller stops
/// listening (dropping the receiver tells it so), or the link ends.
fn guest_read_loop(
    mut raw: UnixStream,
    side: &Side,
    up: &Side,
    link: &Link,
    to_caller: SyncSender<Vec<u8>>,
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
            Ok(0) => {
                side.peer_closed.store(true, Ordering::SeqCst);
                return;
            }
            Ok(n) => {
                last = Instant::now();
                match side.receive(&buf[..n]) {
                    Ok((plain, closed)) => {
                        // A slow hub decision must backpressure the guest too: at this point
                        // the encrypted upstream queue may still be completely empty.
                        for chunk in plain.chunks(COPY_BUF) {
                            if link.is_done() || to_caller.send(chunk.to_vec()).is_err() {
                                return;
                            }
                        }
                        if closed {
                            side.peer_closed.store(true, Ordering::SeqCst);
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
            // A guest that hangs up with our close_notify unread gets its socket reset: it closed,
            // as with an end of stream.
            Err(err) if is_hang_up(&err) => {
                side.peer_closed.store(true, Ordering::SeqCst);
                return;
            }
            Err(err) => {
                link.fail(format!("the guest socket read failed: {err}"));
                return;
            }
        }
    }
}

/// Reads the upstream's socket, decrypts it, and queues the plaintext for the guest, raw. It never
/// waits on the guest: a guest that stops reading ends the connection past [`QUEUE_LIMIT`]. `peek`
/// reads the status line of an Upgrade request's answer on the way. Returns the bytes relayed.
fn upstream_read_loop(
    mut raw: &TcpStream,
    up: &Side,
    guest: &Side,
    link: &Link,
    peek: &Peek,
) -> u64 {
    let mut buf = vec![0u8; COPY_BUF];
    let mut total = 0u64;
    let mut last = Instant::now();
    loop {
        match raw.read(&mut buf) {
            Ok(0) => {
                up.peer_closed.store(true, Ordering::SeqCst);
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
                peek.feed(&plain);
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
                    up.peer_closed.store(true, Ordering::SeqCst);
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

/// A header name is a token (RFC 9110): letters, digits and the tchar punctuation, and nothing else.
/// Whitespace before a colon is not one.
fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

/// Reads the status line of the upstream's answer to an Upgrade request. It is armed before that
/// request is sent. The upstream reader feeds it the upstream's bytes while armed, and it sends the
/// answer's status code once: `None` when a line is not part of a response head or runs past
/// [`MAX_HEAD`]. Interim responses other than `101 Switching Protocols` (such as `100 Continue`) are
/// skipped with their headers, at most [`MAX_INTERIM`] of them.
struct Peek {
    /// The reading state while armed; `None` while no answer is awaited.
    state: Mutex<Option<PeekState>>,
    answers: Sender<Option<u16>>,
}

/// Where the peek is in the upstream's response heads.
#[derive(Default)]
struct PeekState {
    /// The line read so far, through its LF.
    line: Vec<u8>,
    /// Whether the status line of an interim response was read, and its headers are being skipped.
    in_interim: bool,
    /// The bytes of the interim head being skipped.
    head: usize,
    /// The interim responses skipped so far.
    interim: usize,
}

/// The most interim responses (other than `101`) that an upgrade's answer may follow.
const MAX_INTERIM: usize = 5;

impl PeekState {
    /// Reads `bytes` as the rest of the upstream's response heads. `Some` is the answer, once known.
    fn advance(&mut self, bytes: &[u8]) -> Option<Option<u16>> {
        for &byte in bytes {
            self.line.push(byte);
            if self.line.len() > MAX_HEAD {
                return Some(None);
            }
            if byte == b'\n' {
                let line = std::mem::take(&mut self.line);
                if let Some(answer) = self.settle(&line) {
                    return Some(answer);
                }
            }
        }
        None
    }

    /// Takes one complete line of the upstream's heads. `Some` settles the answer.
    fn settle(&mut self, line: &[u8]) -> Option<Option<u16>> {
        if self.in_interim {
            self.head += line.len();
            if self.head > MAX_HEAD {
                return Some(None);
            }
            if line == b"\r\n" || line == b"\n" {
                self.in_interim = false;
                self.head = 0;
            }
            return None;
        }
        match status_code(line) {
            Some(code) if (100..200).contains(&code) && code != 101 => {
                self.interim += 1;
                if self.interim > MAX_INTERIM {
                    return Some(None);
                }
                self.in_interim = true;
                self.head = line.len();
                None
            }
            answer => Some(answer),
        }
    }
}

impl Peek {
    fn arm(&self) {
        *locked(&self.state) = Some(PeekState::default());
    }

    /// Feeds the upstream's plaintext to the peek. Only the bytes after [`Peek::arm`] are read, up to
    /// the answer.
    fn feed(&self, bytes: &[u8]) {
        let mut armed = locked(&self.state);
        let Some(state) = armed.as_mut() else {
            return;
        };
        let Some(answer) = state.advance(bytes) else {
            return;
        };
        *armed = None;
        let _ = self.answers.send(answer);
    }
}

/// The code of a status line (`HTTP/1.x NNN ...`), or `None` when the line is not one.
fn status_code(line: &[u8]) -> Option<u16> {
    let text = std::str::from_utf8(line).ok()?;
    let mut words = text.split_ascii_whitespace();
    let version = words.next()?;
    let code = words.next()?;
    if !version.starts_with("HTTP/1.")
        || code.len() != 3
        || !code.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    code.parse().ok()
}

/// Waits for the upstream reader's answer to the Upgrade request just sent. A connection that ends
/// first is an error, and the link's reason is the one reported.
fn answered(answers: &Receiver<Option<u16>>, link: &Link) -> Result<Option<u16>, String> {
    loop {
        match answers.recv_timeout(PUMP_POLL) {
            Ok(status) => return Ok(status),
            Err(RecvTimeoutError::Timeout) if link.is_done() => {
                return Err("the connection ended before the upstream answered".to_string());
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err("the upstream reader ended before it answered".to_string());
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

    /// The next line, through its CRLF (a chunk-size line, a chunk's CRLF, or a trailer line). A bare
    /// LF, or a CR that is not followed by the LF, is refused.
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
                    if !line.ends_with(b"\r\n") || line[..line.len() - 2].contains(&b'\r') {
                        return Err("a line in a chunked body ends in a bare CR or LF, not CRLF"
                            .to_string());
                    }
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

/// Parses a request head (through its blank line). Only origin-form targets are injected. A head
/// whose body framing two parsers could read differently is refused, so the body is relayed exactly
/// as the guest meant it: a line ended by a bare CR or LF, an obs-fold continuation, a header name
/// with whitespace before its colon, a Content-Length that is not digits or differs from another,
/// a Transfer-Encoding other than one `chunked`, and both framings at once.
fn parse_request(head: &str) -> Result<Request, String> {
    let head = head
        .strip_suffix("\r\n\r\n")
        .ok_or("the request head is not complete")?;
    let lines: Vec<&str> = head.split("\r\n").collect();
    if lines.iter().any(|line| line.contains(['\r', '\n'])) {
        return Err("a line ends in a bare CR or LF, not CRLF".to_string());
    }
    let Some((line, headers)) = lines.split_first() else {
        return Err("the request head is empty".to_string());
    };
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
    for line in headers {
        let (name, value) = header_line(line)?;
        if name.eq_ignore_ascii_case("content-length") {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("Content-Length is not a number".to_string());
            }
            let parsed: u64 = value
                .parse()
                .map_err(|_| "Content-Length is out of range".to_string())?;
            if length
                .replace(parsed)
                .is_some_and(|before| before != parsed)
            {
                return Err("two different Content-Length headers".to_string());
            }
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if chunked {
                return Err("more than one Transfer-Encoding header".to_string());
            }
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

/// Binds the HTTP authority to the CONNECT destination whose credential rule was checked.
/// TLS authenticates the CONNECT name; the upstream may route HTTP by Host independently.
fn check_request_host(head: &str, target: &Target<'_>) -> Result<(), String> {
    let mut hosts = head
        .split("\r\n")
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.trim());
    let value = hosts
        .next()
        .ok_or("an intercepted request needs a Host header")?;
    if hosts.next().is_some() {
        return Err("an intercepted request has more than one Host header".into());
    }
    let (host, port) =
        super::authority(value, Some(443)).ok_or("the intercepted request's Host is malformed")?;
    if !host
        .trim_end_matches('.')
        .eq_ignore_ascii_case(target.host.trim_end_matches('.'))
        || port != target.port
    {
        return Err("the intercepted request's Host differs from its CONNECT destination".into());
    }
    Ok(())
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

/// A header line (without its CRLF), read by the framing rules: no obs-fold continuation, a colon, and
/// a token name with nothing before the colon. Returns the name and the value without the HTTP
/// whitespace around it; a Unicode space is part of the value.
fn header_line(line: &str) -> Result<(&str, &str), String> {
    if line.starts_with([' ', '\t']) {
        return Err("a header line is folded (obs-fold)".to_string());
    }
    let (name, value) = line.split_once(':').ok_or("a header line has no colon")?;
    if !is_token(name) {
        return Err("a header name is not a token (whitespace before the colon?)".to_string());
    }
    Ok((name, value.trim_matches([' ', '\t'])))
}

/// The size of a chunk, from its size line: one to sixteen hex digits, then optionally `;` and an
/// extension. A sign, whitespace before the digits, or more digits is refused.
fn chunk_size(line: &[u8]) -> Result<u64, String> {
    let text =
        std::str::from_utf8(line).map_err(|_| "a chunk size line is not text".to_string())?;
    let text = text
        .strip_suffix("\r\n")
        .ok_or("a chunk size line does not end in CRLF")?;
    let digits = text.split(';').next().unwrap_or("");
    if digits.is_empty()
        || digits.len() > 16
        || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("a chunk size is not 1 to 16 hex digits".to_string());
    }
    u64::from_str_radix(digits, 16).map_err(|_| "a chunk size is out of range".to_string())
}

/// Copies a chunked body to `sink` byte for byte, through its last chunk and trailers. Each chunk is
/// checked as it is read: its size line, its data and the CRLF after the data. Each trailer is a
/// header line. The next request's bytes are left unread.
fn relay_chunked(
    input: &mut Inbound,
    sink: &mut impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    loop {
        let size_line = input.line()?;
        let size = chunk_size(&size_line)?;
        sink(&size_line)?;
        if size == 0 {
            loop {
                let trailer = input.line()?;
                if trailer == b"\r\n" {
                    sink(&trailer)?;
                    return Ok(());
                }
                let text = std::str::from_utf8(&trailer)
                    .map_err(|_| "a trailer line is not text".to_string())?;
                let text = text
                    .strip_suffix("\r\n")
                    .ok_or("a trailer line does not end in CRLF")?;
                header_line(text)?;
                sink(&trailer)?;
            }
        }
        input.forward(size, sink)?;
        let crlf = input.line()?;
        if crlf != b"\r\n" {
            return Err("a chunk's data is not followed by CRLF".to_string());
        }
        sink(&crlf)?;
    }
}

/// Counts and queues bytes for the upstream.
fn send_up(up: &Side, sent: &mut u64, bytes: &[u8]) -> Result<(), String> {
    *sent += bytes.len() as u64;
    up.send(bytes)
}

/// Relays the guest's requests upstream, one at a time, each with apid's injection. Returns when
/// the guest is done, or after an upgrade's bytes have been copied. An Upgrade request is relayed
/// like any other; the status line of its answer decides whether the guest's bytes after it are
/// copied raw, and any answer but `101 Switching Protocols` keeps the requests parsed.
fn relay(
    inbound: &mut Inbound,
    target: &Target,
    apid: &Apid,
    up: &Side,
    tally: &mut Tally,
    peek: &Peek,
    answers: &Receiver<Option<u16>>,
) -> Result<(), String> {
    while let Some(head) = inbound.head()? {
        let request = parse_request(&head)?;
        check_request_host(&head, target)?;
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
        if request.upgrade {
            // Armed before the head is sent, so the answer cannot pass unread.
            peek.arm();
        }
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
        if request.upgrade && answered(answers, &inbound.link)? == Some(101) {
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
        assert!(parse_request("GET http://upstream.test/a HTTP/1.1\r\n\r\n").is_err());
        assert!(parse_request("GET /a HTTP/1.1\r\nBroken header\r\n\r\n").is_err());
        assert!(parse_request("GET /a\r\n\r\n").is_err());
    }

    /// Asserts that `head` is refused, for a reason that contains `reason`.
    fn assert_refused(head: &str, reason: &str) {
        let error = parse_request(head).expect_err("the head must be refused");
        assert!(error.contains(reason), "{head:?} refused as: {error}");
    }

    #[test]
    fn a_head_with_both_framings_is_refused() {
        assert_refused(
            "POST /a HTTP/1.1\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n",
            "both set",
        );
        assert_refused(
            "POST /a HTTP/1.1\r\nTransfer-Encoding: chunked\r\nContent-Length: 5\r\n\r\n",
            "both set",
        );
    }

    #[test]
    fn content_lengths_that_differ_are_refused_and_equal_ones_are_not() {
        assert_refused(
            "POST /a HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\n",
            "two different",
        );
        assert_refused(
            "POST /a HTTP/1.1\r\nContent-Length: 5, 5\r\n\r\n",
            "not a number",
        );
        let same =
            parse_request("POST /a HTTP/1.1\r\nContent-Length: 5\r\ncontent-length: 5\r\n\r\n")
                .unwrap();
        assert_eq!(same.framing, Framing::Length(5));
    }

    #[test]
    fn a_transfer_encoding_other_than_chunked_is_refused() {
        assert_refused(
            "POST /a HTTP/1.1\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
            "only chunked",
        );
        assert_refused(
            "POST /a HTTP/1.1\r\nTransfer-Encoding: identity\r\n\r\n",
            "only chunked",
        );
    }

    #[test]
    fn a_second_transfer_encoding_header_is_refused() {
        assert_refused(
            "POST /a HTTP/1.1\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n",
            "more than one",
        );
    }

    #[test]
    fn an_obs_fold_continuation_line_is_refused() {
        assert_refused("GET /a HTTP/1.1\r\nX-A: b\r\n c\r\n\r\n", "obs-fold");
        assert_refused("GET /a HTTP/1.1\r\nX-A: b\r\n\td\r\n\r\n", "obs-fold");
    }

    #[test]
    fn a_space_before_a_header_colon_is_refused() {
        assert_refused(
            "POST /a HTTP/1.1\r\nContent-Length : 5\r\n\r\n",
            "not a token",
        );
        assert_refused(
            "POST /a HTTP/1.1\r\nTransfer-Encoding : chunked\r\n\r\n",
            "not a token",
        );
    }

    #[test]
    fn a_bare_lf_in_a_head_is_refused() {
        assert_refused(
            "GET /a HTTP/1.1\r\nHost: a\nX-B: c\r\n\r\n",
            "bare CR or LF",
        );
        assert_refused("GET /a HTTP/1.1\nHost: a\r\n\r\n", "bare CR or LF");
    }

    #[test]
    fn a_content_length_that_is_not_digits_is_refused() {
        assert_refused(
            "POST /a HTTP/1.1\r\nContent-Length: +5\r\n\r\n",
            "not a number",
        );
        assert_refused(
            "POST /a HTTP/1.1\r\nContent-Length: \r\n\r\n",
            "not a number",
        );
    }

    #[test]
    fn interim_responses_other_than_101_are_skipped_up_to_a_bound() {
        let (answers, replies) = mpsc::channel();
        let peek = Peek {
            state: Mutex::new(None),
            answers,
        };
        peek.arm();
        peek.feed(
            b"HTTP/1.1 100 Continue\r\nX-Hint: 1\r\n\r\nHTTP/1.1 101 Switching Protocols\r\n\r\n",
        );
        assert_eq!(replies.try_recv(), Ok(Some(101)));
        // The same answer, with the reads split inside its lines.
        peek.arm();
        for piece in [
            "HTTP/1.1 100 Con",
            "tinue\r\nX-Hint: 1\r",
            "\n\r\nHTTP/1.1 101 Switch",
            "ing Protocols\r\n\r\n",
        ] {
            peek.feed(piece.as_bytes());
        }
        assert_eq!(replies.try_recv(), Ok(Some(101)));
        // A 100 followed by a 200 answers 200, which does not switch protocols.
        peek.arm();
        peek.feed(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\n\r\n");
        assert_eq!(replies.try_recv(), Ok(Some(200)));
        // Up to MAX_INTERIM interim responses are skipped; one more ends the peek.
        peek.arm();
        for _ in 0..MAX_INTERIM {
            peek.feed(b"HTTP/1.1 100 Continue\r\n\r\n");
        }
        peek.feed(b"HTTP/1.1 101 Switching Protocols\r\n\r\n");
        assert_eq!(replies.try_recv(), Ok(Some(101)));
        peek.arm();
        for _ in 0..=MAX_INTERIM {
            peek.feed(b"HTTP/1.1 100 Continue\r\n\r\n");
        }
        assert_eq!(replies.try_recv(), Ok(None));
        // An interim head is bounded too, however many short header lines it has.
        peek.arm();
        peek.feed(b"HTTP/1.1 100 Continue\r\n");
        let header = format!("X-Pad: {}\r\n", "a".repeat(1000));
        for _ in 0..=MAX_HEAD / 1000 {
            peek.feed(header.as_bytes());
        }
        assert_eq!(replies.try_recv(), Ok(None));
    }

    #[test]
    fn the_upgrade_answer_is_the_first_status_line_after_arming() {
        let (answers, replies) = mpsc::channel();
        let peek = Peek {
            state: Mutex::new(None),
            answers,
        };
        peek.feed(b"HTTP/1.1 101 Switching Protocols\r\n");
        assert!(replies.try_recv().is_err(), "nothing is read while unarmed");
        peek.arm();
        peek.feed(b"HTTP/1.1 10");
        peek.feed(b"1 Switching Protocols\r\nUpgrade: websocket\r\n\r\n");
        assert_eq!(replies.try_recv(), Ok(Some(101)));
        peek.arm();
        peek.feed(b"HTTP/1.1 200 OK\r\n\r\n");
        assert_eq!(replies.try_recv(), Ok(Some(200)));
        peek.arm();
        peek.feed(&[b'x'; MAX_HEAD + 1]);
        assert_eq!(replies.try_recv(), Ok(None));
    }

    #[test]
    fn an_upgrade_the_upstream_answers_with_200_keeps_the_requests_parsed() {
        let (upstream_leaf, upstream_key, upstream_ca) = issued("upstream.test");
        let (egress_leaf, egress_key, egress_ca) = issued("upstream.test");
        let (port, seen) = upstream_server(&upstream_leaf, &upstream_key, 2);
        let (apid, injections, token) = fake_apid("upgrade-200", &egress_leaf, &egress_key, true);
        let config = client_config(roots_of(&upstream_ca)).unwrap();

        let (mut guest, mut proxy_side) = UnixStream::pair().unwrap();
        guest
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        let addrs = [SocketAddr::from(([127, 0, 0, 1], port))];
        let (finished, ended) = mpsc::channel();
        thread::spawn(move || {
            let target = Target {
                cid: 12,
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
                b"GET /v1/socket HTTP/1.1\r\nHost: upstream.test\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",
            )
            .unwrap();
        client.flush().unwrap();
        let (head, _) = read_message(&mut client).expect("the upgrade's answer");
        assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");

        // The upstream did not switch protocols, so this request is still parsed and injected.
        client
            .write_all(
                b"POST /v1/x HTTP/1.1\r\nHost: upstream.test\r\nX-Api-Key: guest-own\r\nContent-Length: 5\r\n\r\nhello",
            )
            .unwrap();
        client.flush().unwrap();
        let (_, body) = read_message(&mut client).expect("the next answer");
        assert_eq!(body, b"ok:hello");
        drop(client);

        let (upgrade_head, _) = seen.recv_timeout(Duration::from_secs(20)).unwrap();
        assert!(
            upgrade_head.starts_with("GET /v1/socket "),
            "{upgrade_head}"
        );
        let (head, body) = seen.recv_timeout(Duration::from_secs(20)).unwrap();
        assert!(head.contains("x-api-key: sk-injected"), "{head}");
        assert!(!head.contains("guest-own"), "{head}");
        assert_eq!(body, b"hello");

        let (result, tally) = ended
            .recv_timeout(Duration::from_secs(20))
            .expect("the connection ends once the guest is done");
        let _ = std::fs::remove_file(&token);
        result.expect("the connection ends cleanly");
        assert_eq!(injections.load(Ordering::SeqCst), 2);
        assert_eq!(tally.injected, 2);
    }

    #[test]
    fn an_intercepted_request_cannot_change_its_credential_authority() {
        let target = Target {
            cid: 3,
            host: "api.example.test",
            port: 443,
            addrs: &[],
        };
        for head in [
            "GET /v1 HTTP/1.1\r\nHost: api.example.test\r\n\r\n",
            "GET /v1 HTTP/1.1\r\nHost: API.EXAMPLE.TEST:443\r\n\r\n",
        ] {
            assert!(check_request_host(head, &target).is_ok());
        }
        for head in [
            "GET /v1 HTTP/1.1\r\nHost: another.example.test\r\n\r\n",
            "GET /v1 HTTP/1.1\r\nHost: api.example.test:8443\r\n\r\n",
            "GET /v1 HTTP/1.1\r\n\r\n",
            "GET /v1 HTTP/1.1\r\nHost: api.example.test\r\nHost: another.example.test\r\n\r\n",
        ] {
            assert!(check_request_host(head, &target).is_err());
        }
        let alternate = Target {
            port: 8443,
            ..target
        };
        assert!(
            check_request_host(
                "GET /v1 HTTP/1.1\r\nHost: api.example.test:8443\r\n\r\n",
                &alternate,
            )
            .is_ok()
        );
        assert!(
            check_request_host(
                "GET /v1 HTTP/1.1\r\nHost: api.example.test\r\n\r\n",
                &alternate,
            )
            .is_err()
        );
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

    /// Relays the chunked body that `pieces` make up, each piece as one read, and returns why it is
    /// refused.
    fn chunked_refusal(pieces: &[&str]) -> String {
        let (feed, from_guest) = mpsc::channel();
        let mut inbound = Inbound {
            rx: from_guest,
            buf: Vec::new(),
            pos: 0,
            link: test_link(),
            last: Instant::now(),
        };
        for piece in pieces {
            feed.send(piece.as_bytes().to_vec()).unwrap();
        }
        relay_chunked(&mut inbound, &mut |_: &[u8]| Ok(())).expect_err("the body must be refused")
    }

    #[test]
    fn a_chunk_size_line_ended_by_a_bare_lf_is_refused() {
        let reason = chunked_refusal(&["5\nworld\r\n0\r\n\r\n"]);
        assert!(reason.contains("bare CR or LF"), "{reason}");
    }

    #[test]
    fn a_chunk_size_with_a_sign_is_refused() {
        let reason = chunked_refusal(&["+5\r\nworld\r\n0\r\n\r\n"]);
        assert!(reason.contains("hex digits"), "{reason}");
    }

    #[test]
    fn chunk_data_without_its_crlf_is_refused() {
        let reason = chunked_refusal(&["5\r\nworldX\r\n0\r\n\r\n"]);
        assert!(reason.contains("not followed by CRLF"), "{reason}");
    }

    #[test]
    fn a_chunk_size_of_17_digits_is_refused() {
        let body = format!("{}5\r\nworld\r\n0\r\n\r\n", "0".repeat(16));
        let reason = chunked_refusal(&[body.as_str()]);
        assert!(reason.contains("hex digits"), "{reason}");
    }

    #[test]
    fn a_trailer_must_be_a_header_line() {
        let reason = chunked_refusal(&["0\r\nX-T : 1\r\n\r\n"]);
        assert!(reason.contains("not a token"), "{reason}");
        let reason = chunked_refusal(&["0\r\nX-T: 1\r\n folded\r\n\r\n"]);
        assert!(reason.contains("obs-fold"), "{reason}");
        let reason = chunked_refusal(&["0\r\nX-T: 1\n\r\n"]);
        assert!(reason.contains("bare CR or LF"), "{reason}");
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
