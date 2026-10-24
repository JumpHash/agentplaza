//! Streams over one connection (PLAN 17.4, the node tunnel): many bidirectional byte streams multiplexed on a single
//! carrier, which is a WebSocket for the node tunnel ([`Carrier`] for [`ws::Socket`]) and any message pipe in tests.
//!
//! One carrier message is one frame: `kind (1 byte) | stream id (4 bytes, big endian) | payload`.
//! - `OPEN`: a new stream; its payload is the stream's header (at most [`MAX_HEADER`] bytes), read with [`Stream::header`].
//! - `DATA`: up to [`MAX_CHUNK`] bytes of the stream, in order.
//! - `FIN`: this side sends nothing more on the stream (half-close); the peer may still send.
//! - `RESET`: the stream is over at once; the payload is a reason.
//! - `CREDIT`: the receiver consumed that many bytes, so the sender may send that many more. Each direction starts with
//!   [`WINDOW`] bytes of credit, so a slow reader holds back at most one window per stream.
//!
//! Stream ids: the dialer of the connection opens odd ids, the acceptor even ones. An `OPEN` with the wrong parity, an
//! oversized frame or an unknown kind is a protocol error: the connection closes, and every stream on it ends with
//! [`Event::Closed`]. A stream is removed once both sides have finished it (or either resets it).

use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::ws::{Message, Socket};

/// The largest DATA payload.
pub const MAX_CHUNK: usize = 16 << 10;
/// The bytes a sender may have in flight, per stream and direction, before the receiver consumes some.
pub const WINDOW: u32 = 256 << 10;
/// The largest OPEN payload.
pub const MAX_HEADER: usize = 64 << 10;

const OPEN: u8 = 1;
const DATA: u8 = 2;
const FIN: u8 = 3;
const RESET: u8 = 4;
const CREDIT: u8 = 5;
/// How long the connection's thread waits for a frame before it checks what to send.
const POLL: Duration = Duration::from_millis(5);
/// How long a sender waits for credit before it looks again at whether the connection is still up.
const CREDIT_WAIT: Duration = Duration::from_millis(250);

/// What one connection carries: whole binary messages, in order.
pub trait Carrier: Send + 'static {
    fn send(&mut self, frame: &[u8]) -> io::Result<()>;
    /// Waits up to `wait` for a frame; `Ok(None)` when none came.
    fn recv(&mut self, wait: Duration) -> io::Result<Option<Vec<u8>>>;
}

impl Carrier for Socket {
    fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        Socket::send(self, &Message::Binary(frame.to_vec()))
    }

    fn recv(&mut self, wait: Duration) -> io::Result<Option<Vec<u8>>> {
        match Socket::recv(self, wait)? {
            None => Ok(None),
            Some(Message::Binary(bytes)) => Ok(Some(bytes)),
            Some(Message::Text(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a text message on a stream tunnel",
            )),
        }
    }
}

/// Which end of the connection this is: it fixes the parity of the stream ids it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Opened the connection (the node): odd ids.
    Dialer,
    /// Accepted it (the hub): even ids.
    Acceptor,
}

/// What a stream receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Data(Vec<u8>),
    /// The peer finished its side.
    Fin,
    /// The peer reset the stream, with its reason.
    Reset(String),
    /// The connection is gone, or the stream was dropped.
    Closed,
}

struct Entry {
    events: mpsc::Sender<Event>,
    /// Bytes this side may still send.
    credit: u32,
    /// Bytes the peer may still send before this side consumes some.
    room: u32,
    local_fin: bool,
    remote_fin: bool,
}

struct Shared {
    side: Side,
    streams: Mutex<BTreeMap<u32, Entry>>,
    credit: Condvar,
    /// The id the next stream this side opens takes.
    next: Mutex<u32>,
    frames: mpsc::Sender<Vec<u8>>,
    dead: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "the stream tunnel is closed")
}

fn frame(kind: u8, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(5 + payload.len());
    bytes.push(kind);
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

impl Shared {
    fn send_frame(&self, bytes: Vec<u8>) -> io::Result<()> {
        if self.dead.load(Ordering::SeqCst) {
            return Err(closed());
        }
        self.frames.send(bytes).map_err(|_| closed())
    }

    /// Waits until this stream may send `n` bytes, then takes the credit.
    fn reserve(&self, id: u32, n: u32) -> io::Result<()> {
        let mut streams = lock(&self.streams);
        loop {
            if self.dead.load(Ordering::SeqCst) {
                return Err(closed());
            }
            match streams.get_mut(&id) {
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "the stream is closed",
                    ));
                }
                Some(entry) if entry.local_fin => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "the stream is finished",
                    ));
                }
                Some(entry) if entry.credit >= n => {
                    entry.credit -= n;
                    return Ok(());
                }
                Some(_) => {}
            }
            streams = self
                .credit
                .wait_timeout(streams, CREDIT_WAIT)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// The receiver consumed `n` bytes: the peer may send that many more.
    fn consumed(&self, id: u32, n: usize) {
        let n = u32::try_from(n).unwrap_or(u32::MAX);
        let known = {
            let mut streams = lock(&self.streams);
            match streams.get_mut(&id) {
                Some(entry) => {
                    entry.room = entry.room.saturating_add(n);
                    true
                }
                None => false,
            }
        };
        if known {
            let _ = self.send_frame(frame(CREDIT, id, &n.to_be_bytes()));
        }
    }

    /// Removes the stream. With a reason, and the stream still open (not finished by both sides), the peer is told
    /// with a RESET; a stream both sides finished is removed silently.
    fn drop_stream(&self, id: u32, reason: Option<&str>) {
        let removed = lock(&self.streams).remove(&id);
        self.credit.notify_all();
        if let (Some(_), Some(reason)) = (removed, reason) {
            let reason = &reason.as_bytes()[..reason.len().min(1024)];
            let _ = self.send_frame(frame(RESET, id, reason));
        }
    }
}

/// One connection's streams. Dropping it closes the connection.
pub struct Mux {
    shared: Arc<Shared>,
    incoming: Mutex<mpsc::Receiver<Stream>>,
}

impl Mux {
    /// Runs `carrier` as a multiplexed connection on its own thread.
    pub fn new(carrier: impl Carrier, side: Side) -> Arc<Mux> {
        let (frames, outbox) = mpsc::channel();
        let (incoming_tx, incoming) = mpsc::channel();
        let shared = Arc::new(Shared {
            side,
            streams: Mutex::new(BTreeMap::new()),
            credit: Condvar::new(),
            next: Mutex::new(if side == Side::Dialer { 1 } else { 2 }),
            frames,
            dead: AtomicBool::new(false),
        });
        let io_shared = Arc::clone(&shared);
        thread::spawn(move || {
            let _ = pump(carrier, &io_shared, &outbox, &incoming_tx);
            io_shared.dead.store(true, Ordering::SeqCst);
            lock(&io_shared.streams).clear();
            io_shared.credit.notify_all();
        });
        Arc::new(Mux {
            shared,
            incoming: Mutex::new(incoming),
        })
    }

    /// Whether the connection is still up.
    pub fn alive(&self) -> bool {
        !self.shared.dead.load(Ordering::SeqCst)
    }

    /// Opens a stream with `header` as its OPEN payload.
    pub fn open(&self, header: &[u8]) -> io::Result<Stream> {
        if header.len() > MAX_HEADER {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the stream header is too large",
            ));
        }
        let id = {
            let mut next = lock(&self.shared.next);
            let id = *next;
            *next += 2;
            id
        };
        let (events, inbox) = mpsc::channel();
        lock(&self.shared.streams).insert(
            id,
            Entry {
                events,
                credit: WINDOW,
                room: WINDOW,
                local_fin: false,
                remote_fin: false,
            },
        );
        if let Err(err) = self.shared.send_frame(frame(OPEN, id, header)) {
            self.shared.drop_stream(id, None);
            return Err(err);
        }
        Ok(Stream {
            id,
            header: header.to_vec(),
            shared: Arc::clone(&self.shared),
            events: Mutex::new(inbox),
        })
    }

    /// Waits up to `wait` for a stream the peer opened. `Ok(None)`: none came. `Err`: the connection is closed.
    pub fn accept(&self, wait: Duration) -> io::Result<Option<Stream>> {
        let incoming = lock(&self.incoming);
        match incoming.recv_timeout(wait) {
            Ok(stream) => Ok(Some(stream)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(closed()),
        }
    }
}

impl Drop for Mux {
    fn drop(&mut self) {
        self.shared.dead.store(true, Ordering::SeqCst);
    }
}

/// The connection's thread: writes the queued frames, reads and routes the peer's, until it fails or the mux is dropped.
fn pump(
    mut carrier: impl Carrier,
    shared: &Arc<Shared>,
    outbox: &mpsc::Receiver<Vec<u8>>,
    incoming: &mpsc::Sender<Stream>,
) -> io::Result<()> {
    while !shared.dead.load(Ordering::SeqCst) {
        loop {
            match outbox.try_recv() {
                Ok(bytes) => carrier.send(&bytes)?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        if let Some(bytes) = carrier.recv(POLL)? {
            route(shared, incoming, &bytes)?;
        }
    }
    Ok(())
}

fn protocol(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

/// Applies one frame from the peer.
fn route(shared: &Arc<Shared>, incoming: &mpsc::Sender<Stream>, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() < 5 {
        return Err(protocol("a stream frame without a header"));
    }
    let kind = bytes[0];
    let id = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
    let payload = &bytes[5..];
    match kind {
        OPEN => {
            let peer_parity = match shared.side {
                Side::Dialer => 0,
                Side::Acceptor => 1,
            };
            if id == 0 || id % 2 != peer_parity {
                return Err(protocol("a stream opened with the wrong id"));
            }
            if payload.len() > MAX_HEADER {
                return Err(protocol("a stream header over the limit"));
            }
            let (events, inbox) = mpsc::channel();
            let previous = lock(&shared.streams).insert(
                id,
                Entry {
                    events,
                    credit: WINDOW,
                    room: WINDOW,
                    local_fin: false,
                    remote_fin: false,
                },
            );
            if previous.is_some() {
                return Err(protocol("a stream id opened twice"));
            }
            let stream = Stream {
                id,
                header: payload.to_vec(),
                shared: Arc::clone(shared),
                events: Mutex::new(inbox),
            };
            // The acceptor is gone only when the mux is being dropped; the stream is then refused by closing.
            let _ = incoming.send(stream);
        }
        DATA => {
            if payload.len() > MAX_CHUNK {
                return Err(protocol("a stream chunk over the limit"));
            }
            let mut streams = lock(&shared.streams);
            if let Some(entry) = streams.get_mut(&id) {
                if entry.remote_fin {
                    return Err(protocol("data after the peer finished the stream"));
                }
                let n = u32::try_from(payload.len()).unwrap_or(u32::MAX);
                if n > entry.room {
                    return Err(protocol("the peer sent past its credit"));
                }
                entry.room -= n;
                let _ = entry.events.send(Event::Data(payload.to_vec()));
            }
        }
        FIN => {
            let remove = {
                let mut streams = lock(&shared.streams);
                match streams.get_mut(&id) {
                    Some(entry) => {
                        entry.remote_fin = true;
                        let _ = entry.events.send(Event::Fin);
                        entry.local_fin
                    }
                    None => false,
                }
            };
            if remove {
                shared.drop_stream(id, None);
            }
        }
        RESET => {
            let reason = String::from_utf8_lossy(payload).into_owned();
            if let Some(entry) = lock(&shared.streams).remove(&id) {
                let _ = entry.events.send(Event::Reset(reason));
            }
            shared.credit.notify_all();
        }
        CREDIT => {
            let Ok(raw) = <[u8; 4]>::try_from(payload) else {
                return Err(protocol("a credit frame of the wrong size"));
            };
            let mut streams = lock(&shared.streams);
            if let Some(entry) = streams.get_mut(&id) {
                entry.credit = entry.credit.saturating_add(u32::from_be_bytes(raw));
            }
            drop(streams);
            shared.credit.notify_all();
        }
        _ => return Err(protocol("an unknown stream frame")),
    }
    Ok(())
}

/// One stream on a connection: its header, and its bytes both ways.
pub struct Stream {
    id: u32,
    header: Vec<u8>,
    shared: Arc<Shared>,
    events: Mutex<mpsc::Receiver<Event>>,
}

impl Stream {
    /// The id both sides know this stream by.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// The OPEN payload the opener sent.
    pub fn header(&self) -> &[u8] {
        &self.header
    }

    /// Sends `data` in order, waiting for the receiver's credit as needed.
    pub fn send(&self, data: &[u8]) -> io::Result<()> {
        for chunk in data.chunks(MAX_CHUNK) {
            let n = u32::try_from(chunk.len()).unwrap_or(u32::MAX);
            self.shared.reserve(self.id, n)?;
            self.shared.send_frame(frame(DATA, self.id, chunk))?;
        }
        Ok(())
    }

    /// Finishes this side: the peer gets its `Fin` once everything sent before it.
    pub fn finish(&self) -> io::Result<()> {
        let remove = {
            let mut streams = lock(&self.shared.streams);
            match streams.get_mut(&self.id) {
                Some(entry) if !entry.local_fin => {
                    entry.local_fin = true;
                    entry.remote_fin
                }
                _ => return Ok(()),
            }
        };
        self.shared.send_frame(frame(FIN, self.id, &[]))?;
        if remove {
            self.shared.drop_stream(self.id, None);
        }
        Ok(())
    }

    /// Ends the stream at once, both ways; the peer receives `reason`.
    pub fn reset(&self, reason: &str) {
        self.shared.drop_stream(self.id, Some(reason));
    }

    /// Waits up to `wait` for the next event: `None` when nothing came in time.
    pub fn recv(&self, wait: Duration) -> Option<Event> {
        let inbox = lock(&self.events);
        match inbox.recv_timeout(wait) {
            Ok(Event::Data(bytes)) => {
                drop(inbox);
                self.shared.consumed(self.id, bytes.len());
                Some(Event::Data(bytes))
            }
            Ok(event) => Some(event),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Event::Closed),
        }
    }

    /// Reads the peer's side to its `Fin` and returns what it sent. Errors on a reset, a closed connection, or when
    /// `timeout` passes first.
    pub fn read_all(&self, timeout: Duration) -> io::Result<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        let mut bytes = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the stream timed out",
                ));
            }
            match self.recv(left.min(Duration::from_millis(250))) {
                None => {}
                Some(Event::Data(more)) => bytes.extend_from_slice(&more),
                Some(Event::Fin) => return Ok(bytes),
                Some(Event::Reset(reason)) => {
                    return Err(io::Error::new(io::ErrorKind::ConnectionReset, reason));
                }
                Some(Event::Closed) => return Err(closed()),
            }
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.shared
            .drop_stream(self.id, Some("the stream was dropped"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, Sender, channel};

    /// Two in-memory ends: what one sends the other receives.
    struct Pipe {
        tx: Sender<Vec<u8>>,
        rx: Receiver<Vec<u8>>,
    }

    impl Carrier for Pipe {
        fn send(&mut self, frame: &[u8]) -> io::Result<()> {
            self.tx.send(frame.to_vec()).map_err(|_| closed())
        }

        fn recv(&mut self, wait: Duration) -> io::Result<Option<Vec<u8>>> {
            match self.rx.recv_timeout(wait) {
                Ok(frame) => Ok(Some(frame)),
                Err(RecvTimeoutError::Timeout) => Ok(None),
                Err(RecvTimeoutError::Disconnected) => Err(closed()),
            }
        }
    }

    fn pair() -> (Arc<Mux>, Arc<Mux>) {
        let (a_tx, b_rx) = channel();
        let (b_tx, a_rx) = channel();
        (
            Mux::new(Pipe { tx: a_tx, rx: a_rx }, Side::Dialer),
            Mux::new(Pipe { tx: b_tx, rx: b_rx }, Side::Acceptor),
        )
    }

    fn accepted(mux: &Mux) -> Stream {
        mux.accept(Duration::from_secs(5))
            .expect("the connection is up")
            .expect("a stream arrives")
    }

    #[test]
    fn a_header_arrives_with_its_stream_and_each_side_opens_its_own_parity() {
        let (dialer, acceptor) = pair();
        let stream = dialer.open(b"hello").expect("opens");
        assert_eq!(stream.id() % 2, 1, "the dialer opens odd ids");
        let got = accepted(&acceptor);
        assert_eq!(got.header(), b"hello");
        assert_eq!(got.id(), stream.id());
        let back = acceptor.open(b"reply").expect("opens");
        assert_eq!(back.id() % 2, 0, "the acceptor opens even ids");
        assert_eq!(accepted(&dialer).header(), b"reply");
    }

    #[test]
    fn bytes_flow_both_ways_in_order_past_one_window() {
        let (dialer, acceptor) = pair();
        let stream = dialer.open(b"bulk").expect("opens");
        let peer = accepted(&acceptor);
        let payload: Vec<u8> = (0..WINDOW as usize * 3 + 123)
            .map(|i| (i % 251) as u8)
            .collect();
        let expected = payload.clone();
        // The reader must run beside the sender: the payload is larger than the window.
        let reader = thread::spawn(move || {
            peer.read_all(Duration::from_secs(30))
                .map(|got| (peer, got))
        });
        stream.send(&payload).expect("sends");
        stream.finish().expect("finishes");
        let (peer, got) = reader.join().expect("joins").expect("reads to the finish");
        assert_eq!(got, expected, "every byte, in order");
        peer.send(b"ack").expect("sends back");
        peer.finish().expect("finishes");
        assert_eq!(
            stream.read_all(Duration::from_secs(5)).expect("reads"),
            b"ack"
        );
    }

    #[test]
    fn many_streams_share_one_connection_without_mixing() {
        let (dialer, acceptor) = pair();
        let opened: Vec<Stream> = (0..20)
            .map(|n| dialer.open(format!("s{n}").as_bytes()).expect("opens"))
            .collect();
        for (n, stream) in opened.iter().enumerate() {
            stream
                .send(format!("payload {n}").as_bytes())
                .expect("sends");
            stream.finish().expect("finishes");
        }
        for _ in 0..20 {
            let peer = accepted(&acceptor);
            let header = String::from_utf8(peer.header().to_vec()).expect("text");
            let n: usize = header[1..].parse().expect("the number");
            assert_eq!(
                peer.read_all(Duration::from_secs(5)).expect("reads"),
                format!("payload {n}").as_bytes()
            );
        }
    }

    #[test]
    fn a_reset_ends_the_stream_at_the_peer_with_its_reason() {
        let (dialer, acceptor) = pair();
        let stream = dialer.open(b"doomed").expect("opens");
        let peer = accepted(&acceptor);
        stream.reset("the fork went away");
        assert_eq!(
            peer.recv(Duration::from_secs(5)),
            Some(Event::Reset("the fork went away".to_string()))
        );
    }

    #[test]
    fn a_closed_connection_ends_its_streams_and_its_accept() {
        let (dialer, acceptor) = pair();
        let stream = dialer.open(b"open").expect("opens");
        let _peer = accepted(&acceptor);
        drop(acceptor);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match stream.recv(Duration::from_millis(50)) {
                Some(Event::Closed) => break,
                _ if Instant::now() > deadline => panic!("the stream never saw the close"),
                _ => {}
            }
        }
        assert!(!dialer.alive());
        assert!(dialer.accept(Duration::from_millis(10)).is_err());
    }

    #[test]
    fn an_open_with_the_wrong_parity_is_a_protocol_error_that_closes_the_connection() {
        let (to_mux, mux_inbox) = channel();
        let (mux_outbox, _from_mux) = channel();
        let acceptor = Mux::new(
            Pipe {
                tx: mux_outbox,
                rx: mux_inbox,
            },
            Side::Acceptor,
        );
        // The acceptor's peer (the dialer) opens odd ids; id 2 is the acceptor's own parity.
        to_mux.send(frame(OPEN, 2, b"wrong")).expect("sends");
        let deadline = Instant::now() + Duration::from_secs(5);
        while acceptor.alive() {
            assert!(
                Instant::now() < deadline,
                "the connection should have closed"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert!(acceptor.open(b"after").is_err());
    }
}
