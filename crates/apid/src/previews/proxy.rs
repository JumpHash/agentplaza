//! The hub's HTTP to a preview's app (PLAN 18.1): one request over a port stream (`nodes::Store::open_port`), or over
//! vsockd's inbound listener on the hub's own machine. The answer's head is read first, so a failure before it is an
//! error answer, and its body is relayed as it arrives: each piece the app sends is written to the viewer at once, so
//! server-sent events and chunked answers reach the viewer as they are produced. The body's framing (chunked, a length,
//! or the end of the connection) is decoded here and the viewer's server frames it again.

use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use apz_http::mux::{Event, Stream};
use apz_http::{Body, Request, Response};

use crate::fleet::LOCAL;
use crate::nodes;

/// vsockd's inbound listener on the hub's own machine (apz-link's default): the port the local node's streams go to.
pub const LOCAL_INBOUND: u16 = 19095;
/// How long the app may stay silent (before its head, or between pieces of its body) before the answer fails.
const SILENCE: Duration = Duration::from_secs(300);
/// The most of an answer's head the hub reads before it refuses it.
const MAX_HEAD: usize = 64 << 10;
/// How much one read from a local connection takes.
const CHUNK: usize = 16 << 10;

/// The request headers the app gets from the viewer: what it needs to answer. No cookie, no authorization: the hub's
/// own credentials never reach a preview.
const FORWARDED: &[&str] = &[
    "accept",
    "accept-language",
    "content-type",
    "user-agent",
    "last-event-id",
    "if-none-match",
    "if-modified-since",
];

/// The response headers the viewer gets back from the app.
const RELAYED: &[&str] = &[
    "content-type",
    "cache-control",
    "etag",
    "last-modified",
    "content-language",
    "location",
];

/// One request to the preview's app at `path` (with `query`), over the node's port `port` of guest `cid`, and its answer,
/// streamed. An unreachable node, a port the app does not serve and an answer whose head does not parse are errors to
/// the viewer (502 or 503), named.
pub fn fetch(
    nodes: &nodes::Store,
    node: &str,
    cid: u64,
    port: u32,
    req: &Request,
    path: &str,
    query: &str,
) -> Response {
    fetch_via(nodes, node, LOCAL_INBOUND, cid, port, req, (path, query))
}

/// [`fetch`] with the hub's own machine's inbound listener on `local_inbound` (a test serves its own). `target` is the
/// path and the query.
fn fetch_via(
    nodes: &nodes::Store,
    node: &str,
    local_inbound: u16,
    cid: u64,
    port: u32,
    req: &Request,
    target: (&str, &str),
) -> Response {
    let Ok(port) = u16::try_from(port) else {
        return Response::error(500, "the preview's port is not a port");
    };
    let request = request_bytes(req, target.0, target.1);
    let opened = if node == LOCAL {
        apz_link::inbound::connect(local_inbound, cid, port)
            .map(Source::Local)
            .map_err(|why| format!("the preview's machine does not answer: {why}"))
    } else {
        nodes
            .open_port(node, cid, port)
            .map(Source::Tunnel)
            .map_err(|why| format!("the preview's node is not reachable: {why}"))
    };
    let mut source = match opened {
        Ok(source) => source,
        Err(why) => return Response::error(503, &why),
    };
    if let Err(why) = source.send_request(&request) {
        return Response::error(502, &why);
    }
    let (head, rest) = match read_head(&mut source) {
        Ok(read) => read,
        Err(why) => return Response::error(502, &why),
    };
    let pump = Pump::new(head.framing, rest);
    Response {
        status: head.status,
        headers: head.headers,
        body: Body::Stream(Box::new(move |out: &mut dyn Write| {
            relay(source, pump, out)
        })),
    }
}

/// Where an answer comes from: a port stream through the node's tunnel, or a connection to vsockd's inbound listener.
enum Source {
    Tunnel(Stream),
    Local(TcpStream),
}

impl Source {
    /// Writes the request and ends the viewer's side of it, so the app knows the request is whole.
    fn send_request(&mut self, request: &[u8]) -> Result<(), String> {
        match self {
            Source::Tunnel(stream) => stream
                .send(request)
                .and_then(|()| stream.finish())
                .map_err(|_| "the preview's app did not take the request".to_string()),
            Source::Local(conn) => conn
                .set_read_timeout(Some(SILENCE))
                .and_then(|()| conn.write_all(request))
                .and_then(|()| conn.shutdown(Shutdown::Write))
                .map_err(|err| format!("the preview's app did not take the request: {err}")),
        }
    }

    /// The next piece of the answer, `None` when it has ended. Silence longer than [`SILENCE`] is an error.
    fn next(&mut self) -> io::Result<Option<Vec<u8>>> {
        match self {
            Source::Tunnel(stream) => match stream.recv(SILENCE) {
                Some(Event::Data(bytes)) => Ok(Some(bytes)),
                Some(Event::Fin | Event::Closed) => Ok(None),
                Some(Event::Reset(why)) => Err(io::Error::other(why)),
                None => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the preview's app went silent",
                )),
            },
            Source::Local(conn) => {
                let mut buf = vec![0u8; CHUNK];
                let n = conn.read(&mut buf)?;
                if n == 0 {
                    return Ok(None);
                }
                buf.truncate(n);
                Ok(Some(buf))
            }
        }
    }
}

/// An answer's head as the viewer gets it.
pub struct Head {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub framing: Framing,
}

/// How an answer's body ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    Chunked,
    Length(u64),
    /// The connection's end.
    Close,
}

/// Reads the answer's head, and returns it with the body bytes that came with it.
fn read_head(source: &mut Source) -> Result<(Head, Vec<u8>), String> {
    let mut buf = Vec::new();
    loop {
        if let Some(split) = find(&buf, b"\r\n\r\n") {
            let head = parse_head(&buf[..split])?;
            return Ok((head, buf[split + 4..].to_vec()));
        }
        if buf.len() > MAX_HEAD {
            return Err("the preview's answer head is too long".into());
        }
        match source.next() {
            Ok(Some(bytes)) => buf.extend_from_slice(&bytes),
            Ok(None) => return Err("the preview's app closed before its answer".into()),
            Err(err) => return Err(format!("the preview's app did not answer: {err}")),
        }
    }
}

/// The status, the relayed headers and the framing of an answer's head (the bytes before its blank line).
pub fn parse_head(bytes: &[u8]) -> Result<Head, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the preview's answer head is not text")?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or("the preview's answer has no status")?;
    let mut headers = Vec::new();
    let mut chunked = false;
    let mut length = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if name == "transfer-encoding" && value.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        }
        if name == "content-length" {
            length = value.parse::<u64>().ok();
        }
        if RELAYED.contains(&name.as_str()) {
            headers.push((name, value));
        }
    }
    let framing = if chunked {
        Framing::Chunked
    } else if let Some(length) = length {
        Framing::Length(length)
    } else {
        Framing::Close
    };
    Ok(Head {
        status,
        headers,
        framing,
    })
}

/// Turns the answer's bytes into the body's: a chunked body is decoded as it arrives, a length-framed body stops at its
/// length, and a body that ends with the connection passes through.
struct Pump {
    framing: Framing,
    decoder: Dechunker,
    /// Body bytes still to come, for a length-framed body.
    left: u64,
    /// Bytes that arrived before the body's pump started (with the head), fed first.
    pending: Vec<u8>,
}

impl Pump {
    fn new(framing: Framing, rest: Vec<u8>) -> Self {
        let left = match framing {
            Framing::Length(length) => length,
            _ => 0,
        };
        Self {
            framing,
            decoder: Dechunker::default(),
            left,
            pending: rest,
        }
    }

    /// The body bytes `input` holds, and whether the body has ended with them.
    fn feed(&mut self, input: &[u8]) -> Result<(Vec<u8>, bool), String> {
        match self.framing {
            Framing::Chunked => self.decoder.push(input),
            Framing::Length(_) => {
                let take = input
                    .len()
                    .min(usize::try_from(self.left).unwrap_or(usize::MAX));
                self.left -= take as u64;
                Ok((input[..take].to_vec(), self.left == 0))
            }
            Framing::Close => Ok((input.to_vec(), false)),
        }
    }
}

/// Relays the body: each piece of the app's answer is decoded and written to the viewer at once, then flushed.
fn relay(mut source: Source, mut pump: Pump, out: &mut dyn Write) -> io::Result<()> {
    if matches!(pump.framing, Framing::Length(0)) {
        return Ok(());
    }
    let mut incoming = std::mem::take(&mut pump.pending);
    loop {
        if !incoming.is_empty() {
            let (bytes, ended) = pump.feed(&incoming).map_err(io::Error::other)?;
            if !bytes.is_empty() {
                out.write_all(&bytes)?;
                out.flush()?;
            }
            if ended {
                return Ok(());
            }
        }
        match source.next()? {
            Some(bytes) => incoming = bytes,
            None => return Ok(()),
        }
    }
}

/// The incremental decoder of a chunked body: it keeps what it has not decoded yet, and returns each chunk's bytes as
/// they complete. A chunk's data is returned as it arrives, before its end.
#[derive(Default)]
pub struct Dechunker {
    buf: Vec<u8>,
    state: ChunkState,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum ChunkState {
    #[default]
    Size,
    Data(u64),
    DataEnd,
    Trailer,
    Done,
}

impl Dechunker {
    /// Takes `input` and returns the body bytes it completes, and whether the body has ended.
    pub fn push(&mut self, input: &[u8]) -> Result<(Vec<u8>, bool), String> {
        self.buf.extend_from_slice(input);
        let mut out = Vec::new();
        loop {
            match self.state {
                ChunkState::Size => {
                    let Some(eol) = find(&self.buf, b"\r\n") else {
                        break;
                    };
                    let line = std::str::from_utf8(&self.buf[..eol])
                        .map_err(|_| "a chunk size is not text")?;
                    let size =
                        u64::from_str_radix(line.split(';').next().unwrap_or_default().trim(), 16)
                            .map_err(|_| "a chunk size is not hex")?;
                    self.buf.drain(..eol + 2);
                    self.state = if size == 0 {
                        ChunkState::Trailer
                    } else {
                        ChunkState::Data(size)
                    };
                }
                ChunkState::Data(left) => {
                    if self.buf.is_empty() {
                        break;
                    }
                    let take = self
                        .buf
                        .len()
                        .min(usize::try_from(left).unwrap_or(usize::MAX));
                    out.extend(self.buf.drain(..take));
                    let left = left - take as u64;
                    self.state = if left == 0 {
                        ChunkState::DataEnd
                    } else {
                        ChunkState::Data(left)
                    };
                }
                ChunkState::DataEnd => {
                    if self.buf.len() < 2 {
                        break;
                    }
                    if &self.buf[..2] != b"\r\n" {
                        return Err("a chunk is not followed by its line end".into());
                    }
                    self.buf.drain(..2);
                    self.state = ChunkState::Size;
                }
                ChunkState::Trailer => {
                    if self.buf.starts_with(b"\r\n") || find(&self.buf, b"\r\n\r\n").is_some() {
                        self.state = ChunkState::Done;
                    } else {
                        break;
                    }
                }
                ChunkState::Done => break,
            }
        }
        Ok((out, self.state == ChunkState::Done))
    }
}

/// The request as HTTP/1.1 bytes: the method, the target, the forwarded headers, `Connection: close`, and the body.
pub fn request_bytes(req: &Request, path: &str, query: &str) -> Vec<u8> {
    let target = if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{query}")
    };
    let mut head = format!(
        "{} {target} HTTP/1.1\r\nHost: preview\r\nConnection: close\r\nContent-Length: {}\r\n",
        req.method,
        req.body.len()
    );
    for name in FORWARDED {
        if let Some(value) = req.header(name) {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(&req.body);
    bytes
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(parts: &[&[u8]]) -> (Vec<u8>, bool) {
        let mut decoder = Dechunker::default();
        let mut out = Vec::new();
        let mut ended = false;
        for part in parts {
            let (bytes, done) = decoder.push(part).expect("decodes");
            out.extend(bytes);
            ended = done;
        }
        (out, ended)
    }

    #[test]
    fn a_chunked_body_decodes_across_any_split_of_its_bytes() {
        let whole = b"4\r\nabcd\r\n3;ext=1\r\nefg\r\n0\r\n\r\n";
        for split in 0..whole.len() {
            let (first, second) = whole.split_at(split);
            let (out, ended) = decode_all(&[first, second]);
            assert_eq!(out, b"abcdefg", "split at {split}");
            assert!(ended, "split at {split}");
        }
    }

    #[test]
    fn chunk_data_is_returned_before_its_chunk_ends() {
        let mut decoder = Dechunker::default();
        let (out, ended) = decoder.push(b"a\r\n01234").expect("decodes");
        assert_eq!(out, b"01234", "the first five of ten bytes come now");
        assert!(!ended);
        let (out, _) = decoder.push(b"56789\r\n0\r\n\r\n").expect("decodes");
        assert_eq!(out, b"56789");
    }

    #[test]
    fn a_head_gives_its_status_relayed_headers_and_framing() {
        let head = parse_head(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nX-Hidden: no\r\nTransfer-Encoding: chunked",
        )
        .expect("parses");
        assert_eq!(head.status, 200);
        assert_eq!(head.framing, Framing::Chunked);
        assert_eq!(
            head.headers,
            vec![("content-type".to_string(), "text/event-stream".to_string())]
        );
        let sized = parse_head(b"HTTP/1.1 404 Not Found\r\nContent-Length: 4").expect("parses");
        assert_eq!(sized.framing, Framing::Length(4));
        assert!(parse_head(b"not http").is_err());
    }

    #[test]
    fn the_app_gets_what_it_needs_and_none_of_the_hubs_credentials() {
        let req = Request {
            method: "POST".into(),
            path: "/echo".into(),
            query: "x=1".into(),
            headers: vec![
                ("content-type".into(), "text/plain".into()),
                ("authorization".into(), "Bearer owner".into()),
                ("cookie".into(), "apz_preview=secret".into()),
            ],
            body: b"hello".to_vec(),
            peer: None,
        };
        let bytes = String::from_utf8(request_bytes(&req, "/echo", "x=1")).expect("text");
        assert!(bytes.starts_with("POST /echo?x=1 HTTP/1.1\r\n"), "{bytes}");
        assert!(bytes.contains("Content-Length: 5\r\n"));
        assert!(bytes.contains("content-type: text/plain\r\n"));
        assert!(!bytes.to_ascii_lowercase().contains("authorization"));
        assert!(!bytes.to_ascii_lowercase().contains("cookie"));
        assert!(bytes.ends_with("\r\n\r\nhello"));
    }
}

#[cfg(test)]
mod local_tests {
    use std::net::{SocketAddr, TcpListener};
    use std::sync::Arc;
    use std::thread;

    use apz_link::inbound::{Duplex, GuestDial, PortCheck};

    use super::*;
    use crate::nodes::Store;

    /// vsockd's check and dial on the hub's machine, with the fork's app behind them.
    struct Local(SocketAddr);

    impl PortCheck for Local {
        fn served(&self, cid: u64, port: u16) -> bool {
            (cid, port) == (7, 8080)
        }
    }

    impl GuestDial for Local {
        fn dial(&self, _cid: u64, _port: u16) -> std::io::Result<Box<dyn Duplex>> {
            Ok(Box::new(TcpStream::connect(self.0)?))
        }
    }

    /// The fork's app: answers any request with its path, and closes.
    fn app() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let addr = listener.local_addr().expect("address");
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { return };
                thread::spawn(move || {
                    let mut head = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match conn.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let line = String::from_utf8_lossy(&head)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    let path = line.split(' ').nth(1).unwrap_or_default().to_string();
                    let body = format!("you asked for {path}");
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = conn.write_all(reply.as_bytes());
                });
            }
        });
        addr
    }

    #[test]
    fn a_preview_on_the_hubs_own_machine_is_reached_through_vsockds_inbound_listener() {
        let check = Arc::new(Local(app()));
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let inbound = listener.local_addr().expect("address").port();
        let served = Arc::clone(&check);
        thread::spawn(move || {
            apz_link::inbound::serve(listener, served.clone(), served);
        });
        let req = Request {
            method: "GET".into(),
            path: "/".into(),
            query: String::new(),
            headers: Vec::new(),
            body: Vec::new(),
            peer: None,
        };
        let nodes = Store::holding(Vec::new());
        let answer = fetch_via(&nodes, LOCAL, inbound, 7, 8080, &req, ("/docs", "x=1"));
        assert_eq!(answer.status, 200);
        let mut text = Vec::new();
        match answer.body {
            Body::Stream(stream) => stream(&mut text).expect("relays"),
            _ => panic!("a relayed answer streams"),
        }
        assert_eq!(text, b"you asked for /docs?x=1");
        let refused = fetch_via(&nodes, LOCAL, inbound, 7, 9999, &req, ("/", ""));
        assert_eq!(refused.status, 503, "a port no run serves is refused");
    }
}
