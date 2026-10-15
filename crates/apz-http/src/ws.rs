//! WebSockets (RFC 6455), the plaza's only real-time transport: the hub's event stream to the web
//! console and the desktop app, and noded's events to the hub. No SSE.
//!
//! Hardening (each rule has a test):
//! - Handshake: `GET` only; `Upgrade: websocket` and `Connection: … upgrade …` (tokens,
//!   case-insensitive); `Sec-WebSocket-Version: 13` (else 426 with `Sec-WebSocket-Version: 13`);
//!   `Sec-WebSocket-Key` is base64 of exactly 16 bytes; the Origin policy ([`Origins`]) holds; no
//!   extension is ever negotiated (no permessage-deflate: no compression bombs); a subprotocol is
//!   echoed only when it is the one the endpoint asked for. Any failure: a plain 4xx, no upgrade.
//! - Frames: the client masks every frame and the server none (a frame masked the wrong way is a
//!   protocol error, 1002); RSV bits set or an unknown opcode → 1002; control frames are FIN and at
//!   most 125 bytes; a 64-bit length with its top bit set → 1002; a frame or message over the limits
//!   ([`Limits`]) → 1009, refused *before* allocating for it; fragments reassemble in order (a new
//!   data frame mid-message → 1002; control frames may interleave); a text message that is not
//!   UTF-8 → 1007.
//! - Liveness: a ping after `ping_every` of quiet; no frame at all for `dead_after` → closed. Pongs
//!   answer pings with the same payload. Unsolicited pongs are ignored.
//! - Close: a close frame is echoed (code and UTF-8 reason validated: 1000–1003, 1007–1011,
//!   3000–4999), then the connection ends; after closing, nothing more is sent or received.
//! - Reading never blocks past the caller's wait and never corrupts on a timeout mid-frame (bytes
//!   are buffered until a whole frame is there; the buffer itself is capped).

use std::io::{self, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rustls::{ClientConnection, ServerConnection, StreamOwned};
use sha1::{Digest, Sha1};

use crate::client::{Target, dial, read_head, split_base, tls_session};
use crate::{Body, Request, Response};

/// The connection a socket runs on: plain TCP or TLS, both ways.
pub trait Duplex: Read + Write + Send {
    /// How long one read may block (`None`: forever).
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
}

impl Duplex for TcpStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        TcpStream::set_read_timeout(self, timeout)
    }
}

impl Duplex for StreamOwned<ServerConnection, TcpStream> {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.sock.set_read_timeout(timeout)
    }
}

/// A TLS client over any stream the caller connected (TCP, or a vsock relay's stream).
impl<S: Duplex> Duplex for StreamOwned<ClientConnection, S> {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.sock.set_read_timeout(timeout)
    }
}

/// Size and liveness limits for one socket.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The largest single frame payload accepted.
    pub max_frame: usize,
    /// The largest whole (reassembled) message accepted.
    pub max_message: usize,
    /// Quiet this long → a ping.
    pub ping_every: Duration,
    /// No frame at all this long → the connection is dead and closed.
    pub dead_after: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_frame: 1 << 20,
            max_message: 1 << 20,
            ping_every: Duration::from_secs(20),
            dead_after: Duration::from_secs(45),
        }
    }
}

/// Which browsers may open a socket (an `Origin` header is what a browser sends; other clients
/// send none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origins {
    /// Browser pages served by this same host only (`Origin`'s host:port equals `Host`), and
    /// clients that send no `Origin`. Stops cross-site WebSocket hijacking of a cookie session.
    SameHost,
    /// No browser at all: any request with an `Origin` is refused (daemon-to-daemon sockets).
    None,
}

/// A data message. Ping, pong and close are handled by the socket itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
}

const OP_CONTINUATION: u8 = 0x0;
const OP_TEXT: u8 = 0x1;
const OP_BINARY: u8 = 0x2;
const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xA;

/// RFC 6455 section 1.3: the GUID that every accept key is made with.
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// The shortest a blocking read may be asked to wait (a zero timeout means "forever").
const MIN_WAIT: Duration = Duration::from_millis(10);
/// The largest header a frame can have: 2 bytes, a 64-bit length, a mask.
const MAX_HEADER: usize = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// Accepted the connection: every incoming frame is masked, every outgoing one is not.
    Server,
    /// Opened the connection: the reverse.
    Client,
}

/// A message being reassembled from fragments.
struct Fragment {
    text: bool,
    data: Vec<u8>,
}

/// One frame's header, validated; its payload is not yet read.
struct Header {
    fin: bool,
    opcode: u8,
    /// Bytes of header (length and mask included).
    head: usize,
    len: usize,
    key: Option<[u8; 4]>,
}

/// One open WebSocket, either side.
pub struct Socket {
    io: Box<dyn Duplex>,
    role: Role,
    limits: Limits,
    /// Bytes read and not yet parsed into whole frames; at most `max_frame + 14`.
    buf: Vec<u8>,
    /// The message being reassembled, if its first frame came without FIN.
    fragment: Option<Fragment>,
    /// When the last whole frame arrived; the liveness clock.
    last_rx: Instant,
    /// A ping has gone out and no pong has come back.
    ping_out: bool,
    /// The close frame has gone out (or the peer's was received): nothing more moves.
    closed: bool,
}

impl Socket {
    fn new(role: Role, io: Box<dyn Duplex>, limits: Limits) -> Self {
        Socket {
            io,
            role,
            limits,
            buf: Vec::new(),
            fragment: None,
            last_rx: Instant::now(),
            ping_out: false,
            closed: false,
        }
    }

    /// Sends one message (a client masks it with fresh random keys). A message longer than
    /// `max_frame` goes out as several frames; one over `max_message` is refused.
    pub fn send(&mut self, message: &Message) -> io::Result<()> {
        if self.closed {
            return Err(closed_error());
        }
        let (opcode, payload): (u8, &[u8]) = match message {
            Message::Text(text) => (OP_TEXT, text.as_bytes()),
            Message::Binary(bytes) => (OP_BINARY, bytes.as_slice()),
        };
        if payload.len() > self.limits.max_message {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the message is over max_message",
            ));
        }
        let parts: Vec<&[u8]> = if payload.is_empty() {
            vec![payload]
        } else {
            payload.chunks(self.limits.max_frame.max(1)).collect()
        };
        let last = parts.len() - 1;
        for (index, part) in parts.iter().enumerate() {
            let op = if index == 0 { opcode } else { OP_CONTINUATION };
            self.write_frame(index == last, op, part)?;
        }
        Ok(())
    }

    /// Waits up to `wait` for the next data message, answering pings, sending keepalive pings and
    /// enforcing every rule above on the way. `Ok(None)`: nothing within `wait` (the socket is
    /// still open). `Err`: closed (by either side) or broken; the socket is unusable after.
    pub fn recv(&mut self, wait: Duration) -> io::Result<Option<Message>> {
        if self.closed {
            return Err(closed_error());
        }
        let deadline = Instant::now() + wait;
        loop {
            if let Some(message) = self.parse()? {
                return Ok(Some(message));
            }
            let now = Instant::now();
            let silent = now.saturating_duration_since(self.last_rx);
            if silent >= self.limits.dead_after {
                self.close(1001, "no frame from the peer");
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the peer stopped answering",
                ));
            }
            if !self.ping_out && silent >= self.limits.ping_every {
                self.ping_out = true;
                self.write_frame(true, OP_PING, &[])?;
            }
            if now >= deadline {
                return Ok(None);
            }
            // Wake for the deadline, for the next keepalive ping, or for death, whichever is first.
            let mut wake = deadline.min(self.last_rx + self.limits.dead_after);
            if !self.ping_out {
                wake = wake.min(self.last_rx + self.limits.ping_every);
            }
            self.io
                .set_read_timeout(Some(wake.saturating_duration_since(now).max(MIN_WAIT)))?;
            let room = (self.limits.max_frame + MAX_HEADER).saturating_sub(self.buf.len());
            if room == 0 {
                return Err(self.fail(1009, "the read buffer is full"));
            }
            let mut scratch = [0u8; 8192];
            let want = room.min(scratch.len());
            match self.io.read(&mut scratch[..want]) {
                Ok(0) => {
                    self.closed = true;
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the connection ended without a close",
                    ));
                }
                Ok(read) => self.buf.extend_from_slice(&scratch[..read]),
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(err) => {
                    self.closed = true;
                    return Err(err);
                }
            }
        }
    }

    /// Closes with `code` and `reason` (the close handshake, best effort), then ends the connection.
    pub fn close(&mut self, code: u16, reason: &str) {
        if self.closed {
            return;
        }
        self.closed = true;
        let reason = truncate(reason, 123);
        let mut payload = code.to_be_bytes().to_vec();
        payload.extend_from_slice(reason.as_bytes());
        let _ = self.write_frame(true, OP_CLOSE, &payload);
    }

    /// Records a protocol violation: the close frame with the rule's code goes out, the socket is
    /// closed, and the error is returned.
    fn fail(&mut self, code: u16, reason: &'static str) -> io::Error {
        self.close(code, reason);
        io::Error::new(io::ErrorKind::InvalidData, reason)
    }

    /// Writes one frame, masked when this side is the client.
    fn write_frame(&mut self, fin: bool, opcode: u8, payload: &[u8]) -> io::Result<()> {
        let mask = if self.role == Role::Client {
            Some(apz_trust::random::<4>())
        } else {
            None
        };
        let mask_bit = if mask.is_some() { 0x80 } else { 0 };
        let mut out = Vec::with_capacity(payload.len() + MAX_HEADER);
        out.push((if fin { 0x80 } else { 0 }) | opcode);
        match payload.len() {
            n if n <= 125 => out.push(mask_bit | n as u8),
            n if n <= u16::MAX as usize => {
                out.push(mask_bit | 126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(mask_bit | 127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        match mask {
            Some(key) => {
                out.extend_from_slice(&key);
                out.extend(
                    payload
                        .iter()
                        .enumerate()
                        .map(|(index, byte)| byte ^ key[index % 4]),
                );
            }
            None => out.extend_from_slice(payload),
        }
        let result = self.io.write_all(&out).and_then(|()| self.io.flush());
        if result.is_err() {
            self.closed = true;
        }
        result
    }

    /// The next data message, if the buffered whole frames make one; control frames are answered
    /// on the way. `Ok(None)`: more bytes are needed.
    fn parse(&mut self) -> io::Result<Option<Message>> {
        loop {
            let Some(header) = self.header()? else {
                return Ok(None);
            };
            let total = header.head + header.len;
            if self.buf.len() < total {
                return Ok(None);
            }
            let mut payload = self.buf[header.head..total].to_vec();
            self.buf.drain(..total);
            if let Some(key) = header.key {
                for (index, byte) in payload.iter_mut().enumerate() {
                    *byte ^= key[index % 4];
                }
            }
            self.last_rx = Instant::now();
            match header.opcode {
                OP_PING => self.write_frame(true, OP_PONG, &payload)?,
                OP_PONG => self.ping_out = false,
                OP_CLOSE => return Err(self.peer_closed(&payload)),
                OP_CONTINUATION => {
                    let Some(mut fragment) = self.fragment.take() else {
                        return Err(self.fail(1002, "a continuation starts no message"));
                    };
                    fragment.data.extend_from_slice(&payload);
                    if !header.fin {
                        self.fragment = Some(fragment);
                        continue;
                    }
                    return self.complete(fragment.text, fragment.data).map(Some);
                }
                opcode => {
                    let text = opcode == OP_TEXT;
                    if !header.fin {
                        self.fragment = Some(Fragment {
                            text,
                            data: payload,
                        });
                        continue;
                    }
                    return self.complete(text, payload).map(Some);
                }
            }
        }
    }

    /// Validates the header at the front of the buffer, before any of its payload is read.
    /// `Ok(None)`: the header itself is not all here yet.
    fn header(&mut self) -> io::Result<Option<Header>> {
        if self.buf.len() < 2 {
            return Ok(None);
        }
        let (first, second) = (self.buf[0], self.buf[1]);
        if first & 0x70 != 0 {
            return Err(self.fail(1002, "a reserved bit is set"));
        }
        let opcode = first & 0x0f;
        if !matches!(
            opcode,
            OP_CONTINUATION | OP_TEXT | OP_BINARY | OP_CLOSE | OP_PING | OP_PONG
        ) {
            return Err(self.fail(1002, "an unknown opcode"));
        }
        let masked = second & 0x80 != 0;
        if masked != (self.role == Role::Server) {
            let reason = if masked {
                "a server frame is masked"
            } else {
                "a client frame is not masked"
            };
            return Err(self.fail(1002, reason));
        }
        let extended: usize = match second & 0x7f {
            126 => 2,
            127 => 8,
            _ => 0,
        };
        let key_at = 2 + extended;
        let head = key_at + if masked { 4 } else { 0 };
        if self.buf.len() < head {
            return Ok(None);
        }
        let len = match extended {
            2 => u64::from(u16::from_be_bytes([self.buf[2], self.buf[3]])),
            8 => {
                let mut eight = [0u8; 8];
                eight.copy_from_slice(&self.buf[2..10]);
                let len = u64::from_be_bytes(eight);
                if len >> 63 != 0 {
                    return Err(self.fail(1002, "the 64-bit length has its top bit set"));
                }
                len
            }
            _ => u64::from(second & 0x7f),
        };
        let key = if masked {
            let mut key = [0u8; 4];
            key.copy_from_slice(&self.buf[key_at..head]);
            Some(key)
        } else {
            None
        };
        let fin = first & 0x80 != 0;
        let control = opcode & 0x08 != 0;
        if control {
            if !fin {
                return Err(self.fail(1002, "a control frame is fragmented"));
            }
            if len > 125 {
                return Err(self.fail(1002, "a control frame is over 125 bytes"));
            }
        }
        if len > self.limits.max_frame as u64 {
            return Err(self.fail(1009, "a frame is over max_frame"));
        }
        if !control {
            match (opcode, self.fragment.is_some()) {
                (OP_CONTINUATION, false) => {
                    return Err(self.fail(1002, "a continuation starts no message"));
                }
                (OP_TEXT | OP_BINARY, true) => {
                    return Err(self.fail(1002, "a new message starts mid-fragment"));
                }
                _ => {}
            }
            let held = self
                .fragment
                .as_ref()
                .map_or(0, |fragment| fragment.data.len()) as u64;
            if held + len > self.limits.max_message as u64 {
                return Err(self.fail(1009, "a message is over max_message"));
            }
        }
        Ok(Some(Header {
            fin,
            opcode,
            head,
            len: len as usize,
            key,
        }))
    }

    /// The peer's close frame: validated, echoed, and the socket ends.
    fn peer_closed(&mut self, payload: &[u8]) -> io::Error {
        match payload {
            [] => {}
            [_] => return self.fail(1002, "a close frame has a one-byte body"),
            [high, low, reason @ ..] => {
                if !valid_close_code(u16::from_be_bytes([*high, *low])) {
                    return self.fail(1002, "the close code is not valid");
                }
                if std::str::from_utf8(reason).is_err() {
                    return self.fail(1007, "the close reason is not UTF-8");
                }
            }
        }
        self.closed = true;
        let _ = self.write_frame(true, OP_CLOSE, payload);
        io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "the peer closed the connection",
        )
    }

    /// A finished message: binary as it is, text only when it is UTF-8.
    fn complete(&mut self, text: bool, data: Vec<u8>) -> io::Result<Message> {
        if !text {
            return Ok(Message::Binary(data));
        }
        String::from_utf8(data)
            .map(Message::Text)
            .map_err(|_| self.fail(1007, "a text message is not UTF-8"))
    }
}

fn closed_error() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "the WebSocket is closed")
}

/// The longest prefix of `text` that is at most `max` bytes and ends on a character boundary.
fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The close codes a peer may send (RFC 6455 section 7.4).
fn valid_close_code(code: u16) -> bool {
    matches!(code, 1000..=1003 | 1007..=1011 | 3000..=4999)
}

/// The server side: validates `request` as a WebSocket handshake under `origins` (and `protocol`,
/// the subprotocol the endpoint requires, if any) and answers `101 Switching Protocols`, after
/// which `run` gets the socket on the connection's own thread. A request that is not a valid
/// handshake gets the 4xx it deserves and `run` is never called. Authentication is the caller's
/// job, before calling this.
pub fn accept(
    request: &Request,
    origins: Origins,
    protocol: Option<&str>,
    limits: Limits,
    run: impl FnOnce(Socket) + Send + 'static,
) -> Response {
    if request.method != "GET" {
        return Response::error(405, "a WebSocket is opened with GET");
    }
    let lists_token = |name: &str, token: &str| {
        request.header(name).is_some_and(|value| {
            value
                .split(',')
                .any(|item| item.trim().eq_ignore_ascii_case(token))
        })
    };
    if !lists_token("upgrade", "websocket") || !lists_token("connection", "upgrade") {
        return Response::error(
            400,
            "not a WebSocket upgrade: Upgrade: websocket and Connection: Upgrade",
        );
    }
    if request.header("sec-websocket-version").map(str::trim) != Some("13") {
        return Response::error(426, "the WebSocket version must be 13")
            .with_header("Sec-WebSocket-Version", "13");
    }
    let Some(key) = request.header("sec-websocket-key") else {
        return Response::error(400, "Sec-WebSocket-Key is missing");
    };
    match STANDARD.decode(key) {
        Ok(raw) if raw.len() == 16 && STANDARD.encode(&raw) == key => {}
        _ => return Response::error(400, "Sec-WebSocket-Key is not base64 of 16 bytes"),
    }
    if let Some(origin) = request.header("origin") {
        let allowed = match origins {
            Origins::None => false,
            Origins::SameHost => request
                .header("host")
                .is_some_and(|host| same_origin(origin, host)),
        };
        if !allowed {
            return Response::error(403, "the Origin is not allowed to open this socket");
        }
    }
    if let Some(wanted) = protocol {
        let offered = request
            .header("sec-websocket-protocol")
            .is_some_and(|list| list.split(',').any(|item| item.trim() == wanted));
        if !offered {
            return Response::error(400, "the subprotocol this endpoint requires is not offered");
        }
    }

    let mut headers = vec![
        ("Upgrade".to_string(), "websocket".to_string()),
        ("Connection".to_string(), "Upgrade".to_string()),
        ("Sec-WebSocket-Accept".to_string(), accept_key(key)),
    ];
    if let Some(wanted) = protocol {
        headers.push(("Sec-WebSocket-Protocol".to_string(), wanted.to_string()));
    }
    Response {
        status: 101,
        headers,
        body: Body::Upgrade(Box::new(move |conn| {
            run(Socket::new(Role::Server, conn, limits))
        })),
    }
}

/// Whether `origin` (`scheme://host[:port]`, as a browser sends it) names the same host and port
/// as `host` (the Host header). Default ports are normalised: https is 443, http is 80, and a
/// Host without a port takes the origin's scheme's default.
fn same_origin(origin: &str, host: &str) -> bool {
    let (default, authority) = if let Some(rest) = origin.strip_prefix("https://") {
        (443, rest)
    } else if let Some(rest) = origin.strip_prefix("http://") {
        (80, rest)
    } else {
        return false;
    };
    let (Some((origin_host, origin_port)), Some((host_name, host_port))) = (
        split_authority(authority, default),
        split_authority(host, default),
    ) else {
        return false;
    };
    origin_host.eq_ignore_ascii_case(&host_name) && origin_port == host_port
}

/// `host[:port]` or `[v6][:port]` as (host, port), the port defaulted; None when malformed.
fn split_authority(authority: &str, default: u16) -> Option<(String, u16)> {
    if authority.is_empty() || authority.contains(['/', '@', ' ']) {
        return None;
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let (name, after) = rest.split_once(']')?;
        let port: u16 = match after {
            "" => default,
            _ => after.strip_prefix(':')?.parse().ok()?,
        };
        return Some((format!("[{name}]"), port));
    }
    match authority.rsplit_once(':') {
        Some((name, port)) => Some((name.to_string(), port.parse().ok()?)),
        None => Some((authority.to_string(), default)),
    }
}

/// The client side: opens `target.base + path` (ws over http, wss over TLS as [`Target`] says: pinned
/// for the hub, CA-verified for a public host; with the target's bearer and headers), sends the handshake with a random key and `protocol` if any, and
/// checks `101`, `Sec-WebSocket-Accept`, no extension and the protocol before returning the socket.
pub fn connect(
    target: &Target,
    path: &str,
    protocol: Option<&str>,
    timeout: Duration,
    limits: Limits,
) -> io::Result<Socket> {
    let (tls, host, port) = split_base(&target.base)?;
    let host_header = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let key = STANDARD.encode(apz_trust::random::<16>());
    let head = handshake_head(&host_header, &key, target, path, protocol)?;
    let tcp = dial(&host, port, timeout)?;
    let io: Box<dyn Duplex> = if tls {
        Box::new(StreamOwned::new(tls_session(target.pin, &host)?, tcp))
    } else {
        Box::new(tcp)
    };
    finish(io, &head, &key, protocol, limits)
}

/// [`connect`] over a stream the caller already connected, such as a vsock stream to a node's relay. TLS runs on it
/// when `tls` names the server, pinned by `target.pin` (required: a stream with no host name cannot be verified any
/// other way); the name is the SNI. The Host header is `localhost`: the stream names no host, and a hub on the node's
/// own machine sees the relayed connection come from loopback, where its server takes only a loopback Host. With
/// `tls: None` the handshake goes over the stream as it is.
/// The stream's read timeout bounds the handshake, so the caller sets one. `target.base` is not dialled.
pub fn connect_over<S: Duplex + 'static>(
    stream: S,
    tls: Option<&str>,
    target: &Target,
    path: &str,
    protocol: Option<&str>,
    limits: Limits,
) -> io::Result<Socket> {
    let host_header = "localhost".to_string();
    let key = STANDARD.encode(apz_trust::random::<16>());
    let head = handshake_head(&host_header, &key, target, path, protocol)?;
    let io: Box<dyn Duplex> = match tls {
        Some(name) => {
            let pin = target.pin.ok_or_else(|| {
                refused("a stream with no host name needs the hub's certificate pin")
            })?;
            Box::new(StreamOwned::new(tls_session(Some(pin), name)?, stream))
        }
        None => Box::new(stream),
    };
    finish(io, &head, &key, protocol, limits)
}

/// The opening handshake's request text, checked before anything is dialled or sent.
fn handshake_head(
    host_header: &str,
    key: &str,
    target: &Target,
    path: &str,
    protocol: Option<&str>,
) -> io::Result<String> {
    let mut head = format!(
        "GET {path} HTTP/1.1\r\nHost: {host_header}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n"
    );
    printable(path, "the path")?;
    if !path.starts_with('/') {
        return Err(refused("the path must start with /"));
    }
    if let Some(name) = protocol {
        printable(name, "the subprotocol")?;
        head.push_str(&format!("Sec-WebSocket-Protocol: {name}\r\n"));
    }
    if let Some(token) = &target.bearer {
        printable(token, "the bearer token")?;
        head.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    for (name, value) in &target.headers {
        printable(name, "a header name")?;
        printable(value, "a header value")?;
        if name.is_empty() || name.contains([':', ' ']) {
            return Err(refused("a header name has a colon or a space"));
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    Ok(head)
}

/// Sends the handshake over `io`, then checks the server's `101` and returns the socket.
fn finish(
    mut io: Box<dyn Duplex>,
    head: &str,
    key: &str,
    protocol: Option<&str>,
    limits: Limits,
) -> io::Result<Socket> {
    io.write_all(head.as_bytes())?;
    io.flush()?;

    let mut reader = BufReader::new(io);
    let (status, headers) = read_head(&mut reader)?;
    let header = |name: &str| {
        headers
            .iter()
            .find(|(found, _)| found == name)
            .map(|(_, value)| value.as_str())
    };
    if status != 101 {
        return Err(io::Error::other(format!(
            "the server answered {status}, not 101 Switching Protocols"
        )));
    }
    let upgrades = header("upgrade").is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    let connection = header("connection").is_some_and(|value| {
        value
            .split(',')
            .any(|item| item.trim().eq_ignore_ascii_case("upgrade"))
    });
    if !upgrades || !connection {
        return Err(refused("the server's 101 is not a WebSocket upgrade"));
    }
    if header("sec-websocket-accept") != Some(accept_key(key).as_str()) {
        return Err(refused("Sec-WebSocket-Accept does not match the key"));
    }
    if header("sec-websocket-extensions").is_some() {
        return Err(refused(
            "the server negotiated an extension, which this client never asks for",
        ));
    }
    match (protocol, header("sec-websocket-protocol")) {
        (None, None) => {}
        (Some(wanted), Some(got)) if got == wanted => {}
        _ => return Err(refused("the server's subprotocol is not the one asked for")),
    }

    // Whatever the server sent after its head is already the first of the frames.
    let leftover = reader.buffer().to_vec();
    let mut socket = Socket::new(Role::Client, reader.into_inner(), limits);
    socket.buf = leftover;
    Ok(socket)
}

/// Printable ASCII only: a value that cannot break the request's lines.
fn printable(text: &str, what: &str) -> io::Result<()> {
    if text.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
        Ok(())
    } else {
        Err(refused(&format!("{what} is not printable ASCII")))
    }
}

fn refused(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

/// `Sec-WebSocket-Accept` for a key: base64(SHA-1(key + the RFC's GUID)).
pub fn accept_key(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(GUID.as_bytes());
    STANDARD.encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Server;
    use crate::client::Target;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// An in-memory connection: reads drain `input` and time out when it is empty (as a socket
    /// with nothing to say does); writes append to `output`. The test holds both ends.
    #[derive(Clone, Default)]
    struct Pipe {
        input: Arc<Mutex<VecDeque<u8>>>,
        output: Arc<Mutex<Vec<u8>>>,
    }

    impl Read for Pipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut input = self.input.lock().expect("lock");
            if input.is_empty() {
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            let n = buf.len().min(input.len());
            for (slot, byte) in buf.iter_mut().zip(input.drain(..n)) {
                *slot = byte;
            }
            Ok(n)
        }
    }

    impl Write for Pipe {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.lock().expect("lock").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Duplex for Pipe {
        fn set_read_timeout(&self, _timeout: Option<Duration>) -> io::Result<()> {
            Ok(())
        }
    }

    const KEY: [u8; 4] = [0x37, 0xfa, 0x21, 0x3d];

    fn server_socket(limits: Limits) -> (Socket, Pipe) {
        let pipe = Pipe::default();
        (
            Socket::new(Role::Server, Box::new(pipe.clone()), limits),
            pipe,
        )
    }

    fn client_socket(limits: Limits) -> (Socket, Pipe) {
        let pipe = Pipe::default();
        (
            Socket::new(Role::Client, Box::new(pipe.clone()), limits),
            pipe,
        )
    }

    fn feed(pipe: &Pipe, bytes: &[u8]) {
        pipe.input
            .lock()
            .expect("lock")
            .extend(bytes.iter().copied());
    }

    fn written(pipe: &Pipe) -> Vec<u8> {
        pipe.output.lock().expect("lock").clone()
    }

    /// A frame as a browser sends it: masked with `KEY`.
    fn client_frame(fin: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![(if fin { 0x80 } else { 0 }) | opcode];
        match payload.len() {
            n if n < 126 => frame.push(0x80 | n as u8),
            n if n <= u16::MAX as usize => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        frame.extend_from_slice(&KEY);
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ KEY[index % 4]),
        );
        frame
    }

    /// The frames a socket wrote: (opcode, payload), read as unmasked server frames.
    fn frames_written(pipe: &Pipe) -> Vec<(u8, Vec<u8>)> {
        let bytes = written(pipe);
        let mut frames = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let opcode = bytes[at] & 0x0f;
            let masked = bytes[at + 1] & 0x80 != 0;
            let mut len = (bytes[at + 1] & 0x7f) as usize;
            at += 2;
            if len == 126 {
                len = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
                at += 2;
            } else if len == 127 {
                let mut eight = [0u8; 8];
                eight.copy_from_slice(&bytes[at..at + 8]);
                len = u64::from_be_bytes(eight) as usize;
                at += 8;
            }
            let key = if masked {
                at += 4;
                Some([bytes[at - 4], bytes[at - 3], bytes[at - 2], bytes[at - 1]])
            } else {
                None
            };
            let payload: Vec<u8> = bytes[at..at + len]
                .iter()
                .enumerate()
                .map(|(index, byte)| key.map_or(*byte, |key| byte ^ key[index % 4]))
                .collect();
            frames.push((opcode, payload));
            at += len;
        }
        frames
    }

    /// The code of the first close frame the socket wrote.
    fn close_code(pipe: &Pipe) -> Option<u16> {
        frames_written(pipe)
            .into_iter()
            .find(|(opcode, _)| *opcode == OP_CLOSE)
            .and_then(|(_, payload)| {
                payload
                    .get(..2)
                    .map(|code| u16::from_be_bytes([code[0], code[1]]))
            })
    }

    /// Asserts that `socket.recv` fails and the socket closed with `code`.
    fn assert_refused(socket: &mut Socket, pipe: &Pipe, code: u16) {
        let result = socket.recv(Duration::from_secs(1));
        assert!(
            result.is_err(),
            "a frame that breaks the rules must fail, got {result:?}"
        );
        assert_eq!(close_code(pipe), Some(code));
    }

    fn header_of(response: &Response, name: &str) -> Option<String> {
        response
            .headers
            .iter()
            .find(|(found, _)| found.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    const KEY_HEADER: &str = "dGhlIHNhbXBsZSBub25jZQ==";

    /// A handshake request: the valid headers, with `overrides` replacing or (empty value)
    /// dropping them, and any other override added.
    fn request(overrides: &[(&str, &str)]) -> Request {
        let defaults = [
            ("host", "127.0.0.1:7700"),
            ("upgrade", "websocket"),
            ("connection", "keep-alive, Upgrade"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", KEY_HEADER),
        ];
        let mut headers: Vec<(String, String)> = Vec::new();
        for (name, value) in defaults {
            let value = overrides
                .iter()
                .find(|(found, _)| *found == name)
                .map_or(value, |(_, v)| *v);
            if !value.is_empty() {
                headers.push((name.to_string(), value.to_string()));
            }
        }
        for (name, value) in overrides {
            if !defaults.iter().any(|(found, _)| found == name) && !value.is_empty() {
                headers.push((name.to_string(), value.to_string()));
            }
        }
        Request {
            method: "GET".to_string(),
            path: "/ws".to_string(),
            headers,
            ..Request::default()
        }
    }

    fn status(request: &Request, origins: Origins, protocol: Option<&str>) -> u16 {
        accept(request, origins, protocol, Limits::default(), |_| {}).status
    }

    #[test]
    fn the_accept_key_is_the_rfc_example() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn a_valid_handshake_is_upgraded_with_its_accept_key() {
        let response = accept(
            &request(&[]),
            Origins::SameHost,
            None,
            Limits::default(),
            |_| {},
        );
        assert_eq!(response.status, 101);
        assert_eq!(
            header_of(&response, "Sec-WebSocket-Accept").as_deref(),
            Some("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
        );
        assert!(matches!(response.body, Body::Upgrade(_)));
    }

    #[test]
    fn a_wrong_version_is_426_and_names_the_one_it_speaks() {
        let response = accept(
            &request(&[("sec-websocket-version", "8")]),
            Origins::SameHost,
            None,
            Limits::default(),
            |_| {},
        );
        assert_eq!(response.status, 426);
        assert_eq!(
            header_of(&response, "Sec-WebSocket-Version").as_deref(),
            Some("13")
        );
    }

    #[test]
    fn a_bad_key_is_refused() {
        // "short" is five bytes, not sixteen; an empty key is no key at all.
        assert_eq!(
            status(
                &request(&[("sec-websocket-key", "c2hvcnQ=")]),
                Origins::SameHost,
                None
            ),
            400
        );
        assert_eq!(
            status(
                &request(&[("sec-websocket-key", "")]),
                Origins::SameHost,
                None
            ),
            400
        );
    }

    #[test]
    fn a_request_that_is_not_a_websocket_upgrade_is_refused() {
        assert_eq!(
            status(&request(&[("upgrade", "")]), Origins::SameHost, None),
            400
        );
        assert_eq!(
            status(
                &request(&[("connection", "keep-alive")]),
                Origins::SameHost,
                None
            ),
            400
        );
        let post = Request {
            method: "POST".to_string(),
            ..request(&[])
        };
        assert_eq!(status(&post, Origins::SameHost, None), 405);
    }

    #[test]
    fn the_origin_policy_matches_the_host_with_default_ports() {
        let same = |host: &str, origin: &str| {
            status(
                &request(&[("host", host), ("origin", origin)]),
                Origins::SameHost,
                None,
            )
        };
        assert_eq!(same("127.0.0.1:7700", "http://127.0.0.1:7700"), 101);
        assert_eq!(same("example.com", "https://Example.com:443"), 101);
        assert_eq!(same("example.com:80", "http://example.com"), 101);
        assert_eq!(same("example.com:8080", "http://example.com"), 403);
        assert_eq!(same("127.0.0.1:7700", "https://evil.example"), 403);
        assert_eq!(same("127.0.0.1:7700", "null"), 403);
        assert_eq!(
            status(
                &request(&[("host", ""), ("origin", "http://127.0.0.1:7700")]),
                Origins::SameHost,
                None
            ),
            403
        );
        // A client that sends no Origin (a daemon) is served.
        assert_eq!(status(&request(&[]), Origins::SameHost, None), 101);
    }

    #[test]
    fn with_no_browsers_any_origin_is_refused() {
        assert_eq!(
            status(
                &request(&[("origin", "http://127.0.0.1:7700")]),
                Origins::None,
                None
            ),
            403
        );
        assert_eq!(status(&request(&[]), Origins::None, None), 101);
    }

    #[test]
    fn the_subprotocol_is_echoed_only_when_it_is_the_one_asked_for() {
        let offered = request(&[("sec-websocket-protocol", "other, chat.v1")]);
        let asked = accept(
            &offered,
            Origins::SameHost,
            Some("chat.v1"),
            Limits::default(),
            |_| {},
        );
        assert_eq!(asked.status, 101);
        assert_eq!(
            header_of(&asked, "Sec-WebSocket-Protocol").as_deref(),
            Some("chat.v1")
        );
        assert_eq!(
            status(
                &request(&[("sec-websocket-protocol", "other")]),
                Origins::SameHost,
                Some("chat.v1")
            ),
            400
        );
        let unasked = accept(&offered, Origins::SameHost, None, Limits::default(), |_| {});
        assert_eq!(unasked.status, 101);
        assert_eq!(header_of(&unasked, "Sec-WebSocket-Protocol"), None);
    }

    #[test]
    fn no_extension_is_ever_negotiated() {
        let offered = request(&[("sec-websocket-extensions", "permessage-deflate")]);
        let response = accept(&offered, Origins::SameHost, None, Limits::default(), |_| {});
        assert_eq!(response.status, 101);
        assert_eq!(header_of(&response, "Sec-WebSocket-Extensions"), None);
    }

    /// Echoes every data message until the peer closes.
    fn echo_session(mut socket: Socket) {
        loop {
            match socket.recv(Duration::from_secs(5)) {
                Ok(Some(message)) => {
                    if socket.send(&message).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(_) => return,
            }
        }
    }

    fn echo(request: &Request) -> Response {
        accept(
            request,
            Origins::SameHost,
            None,
            Limits::default(),
            echo_session,
        )
    }

    fn next(socket: &mut Socket) -> Message {
        socket
            .recv(Duration::from_secs(5))
            .expect("a read")
            .expect("a message in time")
    }

    fn round_trip(target: &Target) {
        let mut socket = connect(
            target,
            "/ws",
            None,
            Duration::from_secs(5),
            Limits::default(),
        )
        .expect("the handshake");
        socket
            .send(&Message::Text("hello".to_string()))
            .expect("send");
        assert_eq!(next(&mut socket), Message::Text("hello".to_string()));
        socket.send(&Message::Binary(vec![1, 2, 3])).expect("send");
        assert_eq!(next(&mut socket), Message::Binary(vec![1, 2, 3]));
        socket.close(1000, "bye");
    }

    #[test]
    fn a_message_round_trips_over_plain_tcp() {
        let server = Server::bind(0).expect("bind").spawn(echo).expect("spawn");
        let target = Target {
            base: format!("http://127.0.0.1:{}", server.port()),
            ..Target::default()
        };
        round_trip(&target);
    }

    #[test]
    fn a_message_round_trips_over_pinned_tls() {
        let pair = apz_trust::tls::self_signed(&["localhost".to_string()]).expect("a certificate");
        let config = apz_trust::tls::server_config(&pair).expect("a server config");
        let pin = apz_trust::tls::fingerprint(&pair.cert_pem).expect("a pin");
        let server = Server::bind(0)
            .expect("bind")
            .tls(config)
            .spawn(echo)
            .expect("spawn");
        let target = Target {
            base: format!("https://localhost:{}", server.port()),
            pin: Some(pin),
            ..Target::default()
        };
        round_trip(&target);
    }

    #[test]
    fn an_unpinned_wss_target_refuses_a_self_signed_certificate() {
        // No pin means a public host: the Mozilla roots must sign the certificate, and a self-signed one is refused.
        let pair = apz_trust::tls::self_signed(&["localhost".to_string()]).expect("a certificate");
        let config = apz_trust::tls::server_config(&pair).expect("a server config");
        let server = Server::bind(0)
            .expect("bind")
            .tls(config)
            .spawn(echo)
            .expect("spawn");
        let target = Target {
            base: format!("https://localhost:{}", server.port()),
            pin: None,
            ..Target::default()
        };
        let refused = connect(
            &target,
            "/ws",
            None,
            Duration::from_secs(5),
            Limits::default(),
        );
        assert!(
            refused.is_err(),
            "a self-signed certificate must not open a wss socket without a pin"
        );
    }

    /// RFC 6455 section 5: a frame, or a run of frames, that a conforming peer never sends is
    /// refused with the close code its rule names. Each case gets its own socket, so it is
    /// judged alone.
    #[test]
    fn a_frame_that_breaks_the_protocol_is_refused_with_its_close_code() {
        let mut reserved = client_frame(true, OP_TEXT, b"x");
        reserved[0] |= 0x40;
        let mut top_bit_length = vec![0x82, 0x80 | 127];
        top_bit_length.extend_from_slice(&(1u64 << 63).to_be_bytes());
        top_bit_length.extend_from_slice(&KEY);
        let cases: [(&str, u16, Vec<Vec<u8>>); 9] = [
            (
                "an unmasked client frame",
                1002,
                vec![vec![0x81, 0x02, b'h', b'i']],
            ),
            ("a reserved bit", 1002, vec![reserved]),
            (
                "an unknown opcode",
                1002,
                vec![client_frame(true, 0x3, b"x")],
            ),
            (
                "a control frame over 125 bytes",
                1002,
                vec![client_frame(true, OP_PING, &[0u8; 126])],
            ),
            (
                "a fragmented control frame",
                1002,
                vec![client_frame(false, OP_PING, b"")],
            ),
            (
                "a 64-bit length with its top bit set",
                1002,
                vec![top_bit_length],
            ),
            (
                "a new message in the middle of a fragment",
                1002,
                vec![
                    client_frame(false, OP_TEXT, b"a"),
                    client_frame(true, OP_TEXT, b"b"),
                ],
            ),
            (
                "a close code that may not be sent",
                1002,
                vec![client_frame(true, OP_CLOSE, &1005u16.to_be_bytes())],
            ),
            (
                "a text message that is not UTF-8",
                1007,
                vec![client_frame(true, OP_TEXT, &[0xff])],
            ),
        ];
        for (case, code, frames) in cases {
            let (mut socket, pipe) = server_socket(Limits::default());
            for frame in &frames {
                feed(&pipe, frame);
            }
            let result = socket.recv(Duration::from_secs(1));
            assert!(
                result.is_err(),
                "{case}: a frame that breaks the rules must fail, got {result:?}"
            );
            assert_eq!(close_code(&pipe), Some(code), "{case}");
        }
    }

    #[test]
    fn a_frame_over_max_frame_is_1009_before_its_payload_is_allocated() {
        // A header announcing 2^40 bytes: refused on the header alone, nothing allocated for it.
        let (mut socket, pipe) = server_socket(Limits::default());
        let mut frame = vec![0x82, 0x80 | 127];
        frame.extend_from_slice(&(1u64 << 40).to_be_bytes());
        frame.extend_from_slice(&KEY);
        feed(&pipe, &frame);
        assert_refused(&mut socket, &pipe, 1009);

        let small = Limits {
            max_frame: 16,
            ..Limits::default()
        };
        let (mut socket, pipe) = server_socket(small);
        feed(&pipe, &client_frame(true, OP_BINARY, &[0u8; 17]));
        assert_refused(&mut socket, &pipe, 1009);
    }

    #[test]
    fn a_fragmented_text_message_reassembles_around_a_ping_which_is_answered() {
        let (mut socket, pipe) = server_socket(Limits::default());
        feed(&pipe, &client_frame(false, OP_TEXT, b"hel"));
        feed(&pipe, &client_frame(true, OP_PING, b"p"));
        feed(&pipe, &client_frame(true, OP_CONTINUATION, b"lo"));
        assert_eq!(
            socket.recv(Duration::from_secs(1)).expect("a read"),
            Some(Message::Text("hello".to_string()))
        );
        assert!(
            frames_written(&pipe).contains(&(OP_PONG, b"p".to_vec())),
            "the pong repeats the ping's payload"
        );
    }

    #[test]
    fn a_message_over_max_message_is_1009_and_sending_one_is_refused() {
        let limits = Limits {
            max_frame: 8,
            max_message: 10,
            ..Limits::default()
        };
        let (mut socket, pipe) = server_socket(limits);
        feed(&pipe, &client_frame(false, OP_TEXT, b"aaaaa"));
        feed(&pipe, &client_frame(true, OP_CONTINUATION, b"bbbbbb"));
        assert_refused(&mut socket, &pipe, 1009);
        assert!(socket.send(&Message::Binary(vec![0; 11])).is_err());
    }

    #[test]
    fn a_close_is_echoed_and_the_socket_ends() {
        let (mut socket, pipe) = server_socket(Limits::default());
        feed(
            &pipe,
            &client_frame(true, OP_CLOSE, &[0x03, 0xe8, b'b', b'y', b'e']),
        );
        assert!(socket.recv(Duration::from_secs(1)).is_err());
        let echoed =
            frames_written(&pipe).contains(&(OP_CLOSE, vec![0x03, 0xe8, b'b', b'y', b'e']));
        assert!(echoed, "the close is echoed with its code and reason");
        assert!(
            socket.recv(Duration::from_secs(1)).is_err(),
            "nothing more is received after a close"
        );
    }

    #[test]
    fn a_timeout_mid_frame_is_none_and_the_frame_completes_when_the_rest_arrives() {
        let (mut socket, pipe) = server_socket(Limits::default());
        let frame = client_frame(true, OP_TEXT, b"hello");
        feed(&pipe, &frame[..8]);
        assert_eq!(
            socket.recv(Duration::from_millis(20)).expect("a read"),
            None
        );
        feed(&pipe, &frame[8..]);
        assert_eq!(
            socket.recv(Duration::from_secs(1)).expect("a read"),
            Some(Message::Text("hello".to_string()))
        );
    }

    #[test]
    fn a_silent_peer_is_pinged_then_closed_after_dead_after() {
        let limits = Limits {
            ping_every: Duration::from_millis(10),
            dead_after: Duration::from_millis(30),
            ..Limits::default()
        };
        let (mut socket, pipe) = server_socket(limits);
        assert!(socket.recv(Duration::from_secs(2)).is_err());
        let frames = frames_written(&pipe);
        assert!(
            frames.iter().any(|(opcode, _)| *opcode == OP_PING),
            "a keepalive ping went out"
        );
        assert_eq!(close_code(&pipe), Some(1001));
    }

    #[test]
    fn an_unsolicited_pong_is_ignored() {
        let (mut socket, pipe) = server_socket(Limits::default());
        feed(&pipe, &client_frame(true, OP_PONG, b""));
        assert_eq!(
            socket.recv(Duration::from_millis(20)).expect("a read"),
            None
        );
        assert_eq!(close_code(&pipe), None);
    }

    #[test]
    fn the_client_masks_every_frame_and_the_server_sends_none_masked() {
        let (mut server, server_pipe) = server_socket(Limits::default());
        server.send(&Message::Text("hi".to_string())).expect("send");
        assert_eq!(written(&server_pipe), vec![0x81, 0x02, b'h', b'i']);

        let (mut client, client_pipe) = client_socket(Limits::default());
        client.send(&Message::Text("hi".to_string())).expect("send");
        let sent = written(&client_pipe);
        assert_eq!(sent[0], 0x81);
        assert_eq!(sent[1], 0x80 | 2, "the mask bit is set");
        let key = [sent[2], sent[3], sent[4], sent[5]];
        let unmasked: Vec<u8> = sent[6..]
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ key[index % 4])
            .collect();
        assert_eq!(unmasked, b"hi");
    }

    #[test]
    fn a_client_refuses_a_masked_frame_from_the_server() {
        let (mut client, pipe) = client_socket(Limits::default());
        feed(&pipe, &client_frame(true, OP_TEXT, b"x"));
        assert_refused(&mut client, &pipe, 1002);
    }

    #[test]
    fn the_close_frame_goes_out_once() {
        let (mut socket, pipe) = server_socket(Limits::default());
        socket.close(1000, "a");
        socket.close(1001, "b");
        let closes = frames_written(&pipe)
            .iter()
            .filter(|(opcode, _)| *opcode == OP_CLOSE)
            .count();
        assert_eq!(closes, 1);
        assert!(socket.send(&Message::Text("late".to_string())).is_err());
    }
}

/// The WebSocket client over a stream the caller dialled itself, the way a vsock relay's stream is handed in.
#[cfg(test)]
mod connect_over_tests {
    use super::*;
    use crate::Server;
    use crate::client::Target;
    use std::net::{TcpListener, TcpStream};

    fn echo_session(mut socket: Socket) {
        while let Ok(message) = socket.recv(Duration::from_secs(5)) {
            if let Some(message) = message
                && socket.send(&message).is_err()
            {
                return;
            }
        }
    }

    #[test]
    fn a_message_round_trips_over_pinned_tls_on_a_stream_the_caller_dialled() {
        let pair = apz_trust::tls::self_signed(&["localhost".to_string()]).expect("a certificate");
        let config = apz_trust::tls::server_config(&pair).expect("a server config");
        let pin = apz_trust::tls::fingerprint(&pair.cert_pem).expect("a pin");
        let server = Server::bind(0)
            .expect("bind")
            .tls(config)
            .spawn(|request: &crate::Request| {
                accept(
                    request,
                    Origins::None,
                    None,
                    Limits::default(),
                    echo_session,
                )
            })
            .expect("spawn");
        let tcp = TcpStream::connect(("127.0.0.1", server.port())).expect("dial");
        tcp.set_read_timeout(Some(Duration::from_secs(5)))
            .expect("a read timeout");
        let target = Target {
            pin: Some(pin),
            ..Target::default()
        };
        let mut socket = connect_over(
            tcp,
            Some("localhost"),
            &target,
            "/ws",
            None,
            Limits::default(),
        )
        .expect("the handshake");
        socket
            .send(&Message::Text("hello".to_string()))
            .expect("send");
        let reply = socket
            .recv(Duration::from_secs(5))
            .expect("a read")
            .expect("a message in time");
        assert_eq!(reply, Message::Text("hello".to_string()));
        socket.close(1000, "bye");
    }

    #[test]
    fn a_stream_with_no_host_name_is_refused_before_anything_is_sent() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let tcp = TcpStream::connect(listener.local_addr().expect("addr")).expect("dial");
        let Err(err) = connect_over(
            tcp,
            Some("hub"),
            &Target::default(),
            "/ws",
            None,
            Limits::default(),
        ) else {
            panic!("a stream with no pin must be refused");
        };
        assert!(err.to_string().contains("pin"), "{err}");
    }
}
