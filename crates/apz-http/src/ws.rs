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

use std::io::{self, Read, Write};
use std::time::Duration;

use crate::client::Target;
use crate::{Request, Response};

/// The connection a socket runs on: plain TCP or TLS, both ways.
pub trait Duplex: Read + Write + Send {
    /// How long one read may block (`None`: forever).
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
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

/// One open WebSocket, either side.
pub struct Socket {
    // contract: WS1 — the connection, the read buffer, the role (server/client), the limits, the
    // fragment being reassembled, the liveness clocks, closed or not.
    _private: (),
}

impl Socket {
    /// Sends one message (a client masks it with fresh random keys).
    pub fn send(&mut self, message: &Message) -> io::Result<()> {
        let _ = message;
        Err(io::Error::other("contract: WS1"))
    }

    /// Waits up to `wait` for the next data message, answering pings, sending keepalive pings and
    /// enforcing every rule above on the way. `Ok(None)`: nothing within `wait` (the socket is
    /// still open). `Err`: closed (by either side) or broken; the socket is unusable after.
    pub fn recv(&mut self, wait: Duration) -> io::Result<Option<Message>> {
        let _ = wait;
        Err(io::Error::other("contract: WS1"))
    }

    /// Closes with `code` and `reason` (the close handshake, best effort), then ends the connection.
    pub fn close(&mut self, code: u16, reason: &str) {
        let _ = (code, reason); // contract: WS1
    }
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
    let _ = (request, origins, protocol, limits, run);
    Response::error(501, "contract: WS1")
}

/// The client side: opens `target.base + path` (ws over http, wss over pinned TLS, with the
/// target's bearer and headers), sends the handshake with a random key and `protocol` if any, and
/// checks `101`, `Sec-WebSocket-Accept`, no extension and the protocol before returning the socket.
pub fn connect(target: &Target, path: &str, protocol: Option<&str>, timeout: Duration, limits: Limits) -> io::Result<Socket> {
    let _ = (target, path, protocol, timeout, limits);
    Err(io::Error::other("contract: WS2"))
}

/// `Sec-WebSocket-Accept` for a key: base64(SHA-1(key + the RFC's GUID)).
pub fn accept_key(key: &str) -> String {
    let _ = key;
    String::new() // contract: WS1
}
