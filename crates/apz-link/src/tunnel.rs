//! The node's end of its tunnel (PLAN 17.4): one outbound WebSocket to the hub, `/v1/nodes/{id}/tunnel`, carrying every
//! stream between the node and the hub (apz_http::mux). Four kinds of stream, named by the header each opens with:
//! - `command` (hub → node): one command for noded or vsockd, run by [`execute`]; its result JSON is the answer.
//! - `port` (hub → node): bytes both ways to a fork's port. The relay hands the stream to vsockd's inbound listener
//!   (`inbound.rs`), which checks noded and dials the guest. A terminal's bytes and a model call's deltas ride these.
//! - `request` (node → hub): one signed HTTP request (heartbeats, results, metrics, the agents' calls to the hub). The
//!   answer is one line of JSON (`status`, `content_type`, `stream`), then the body. A streamed answer (`stream`: true)
//!   arrives as the hub produces it, and [`request_answer`] hands its body on as it comes.
//!
//! The mesh (PLAN 17.4, Engine 6.2): a node whose hub holds it on a WireGuard mesh (Tailscale, Headscale or plain
//! WireGuard) listens at its mesh address ([`listen`]) and the hub dials it there. The hub's dial presents a bearer
//! the hub gave the node over its authenticated tunnel ([`fetch_link`]), and the hub then asks the node to sign a
//! nonce with its key (`hello`): the mesh says who can reach the node, the node's key says who the node is. A mesh
//! session serves the same streams as a tunnel, so the node's forks and their sessions do not notice the change of
//! path; a tunnel session ends when a mesh session starts ([`mesh_up`]), and the node then dials nothing.
//!
//! A hub that predates the tunnel answers the upgrade with 401 or 404; the node then keeps the long poll and its HTTP
//! requests ([`LEGACY`]) and tries the tunnel again between polls. A refused upgrade with 403 is a revoked node. A
//! refused upgrade with 409 means the hub holds the node on the mesh: the node asks for its path ([`fetch_link`]).

use std::io::{self, Read};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apz_http::client::{self, Reply, Target};
use apz_http::mux::{Event, MAX_CHUNK, Mux, Side, Stream};
use apz_http::ws::{self, Limits, Origins};
use apz_http::{Request, Response, Router, Server};
use apz_trust::node::NodeKey;
use serde_json::{Map, Value, json};

use crate::identity::Identity;
use crate::inbound;
use crate::run::{Local, execute};

/// The longest the upgrade handshake may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How often a relayed port checks whether the tunnel is still up.
const RELAY_POLL: Duration = Duration::from_millis(500);
/// How often the accepting loop looks for a stream while the tunnel is quiet.
const ACCEPT_POLL: Duration = Duration::from_millis(500);

/// Set while the hub predates the tunnel: requests then go over HTTP, as they did before.
static LEGACY: AtomicBool = AtomicBool::new(false);
/// The tunnel now up, if any.
static CURRENT: Mutex<Option<Arc<Mux>>> = Mutex::new(None);
/// The bearer a hub presents to the mesh listener, as the hub gave it over the tunnel (`fetch_link`).
static MESH_TOKEN: Mutex<Option<String>> = Mutex::new(None);
/// Set while a hub's mesh session is up: the node serves that session and holds no tunnel meanwhile.
static MESH_UP: AtomicBool = AtomicBool::new(false);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Why the tunnel could not be opened.
#[derive(Debug)]
pub enum Connect {
    /// The hub refused the node's upgrade with 403: revoked.
    Revoked,
    /// The hub does not serve the tunnel (401 or 404): it predates it.
    Legacy,
    /// The hub holds the node on the mesh (409): the node asks for its path (`fetch_link`), and waits to be dialled.
    Moved,
    /// Anything else: retried.
    Failed(String),
}

/// Who this node is on a link: its id and key, which answer a hub's `hello` (the node's key on top of the mesh's).
#[derive(Clone)]
pub struct Who {
    pub node_id: String,
    pub key: Arc<NodeKey>,
}

impl Who {
    pub fn new(identity: &Identity, key: &Arc<NodeKey>) -> Who {
        Who {
            node_id: identity.node_id.clone(),
            key: Arc::clone(key),
        }
    }
}

/// Tests of the HTTP path: requests go over HTTP to a fake hub, as they did before the tunnel.
#[cfg(test)]
pub fn use_http() {
    LEGACY.store(true, Ordering::SeqCst);
}

/// The tunnel that is up now, if any.
pub fn current() -> Option<Arc<Mux>> {
    lock(&CURRENT).clone().filter(|mux| mux.alive())
}

/// Whether a hub's mesh session is up.
pub fn mesh_up() -> bool {
    MESH_UP.load(Ordering::SeqCst)
}

/// Listens for the hub's mesh dial at `addr` (the node's address on its mesh). The dial must present the bearer the
/// hub gave the node ([`fetch_link`]); a dial without it, or before the node has one, is refused.
pub fn listen(addr: SocketAddr, who: Who, local: Local) -> io::Result<apz_http::Handle> {
    let router: Router<()> =
        Router::new().get("/v1/mesh", move |_, req, _| admit_mesh(req, &who, &local));
    Server::bind_addr(addr)?.spawn(move |request| router.handle(&(), request))
}

/// Upgrades a hub's mesh dial, when it presents the mesh bearer; its session then serves the hub's streams.
fn admit_mesh(req: &Request, who: &Who, local: &Local) -> Response {
    let presented = req
        .header("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();
    let token = lock(&MESH_TOKEN).clone();
    let admitted = token.is_some_and(|token| {
        !token.is_empty() && apz_trust::ct_eq(presented.as_bytes(), token.as_bytes())
    });
    if !admitted {
        return Response::error(401, "the mesh does not admit this caller");
    }
    let (who, local) = (who.clone(), local.clone());
    ws::accept(req, Origins::None, None, Limits::default(), move |socket| {
        MESH_UP.store(true, Ordering::SeqCst);
        serve(Mux::new(socket, Side::Acceptor), local, who, true);
        MESH_UP.store(false, Ordering::SeqCst);
    })
}

/// Asks the hub how it holds this node (`GET /v1/nodes/{id}/link`, signed by the node): on the mesh, the hub gives
/// the bearer its dial presents; on the tunnel, none is kept. Returns the path the hub names.
pub fn fetch_link(identity: &Identity, key: &NodeKey) -> Result<String, String> {
    let path = format!("/v1/nodes/{}/link", identity.node_id);
    let target = identity.signed(key, "GET", &path, &[])?;
    let reply = client::request(&target, "GET", &path, None, CONNECT_TIMEOUT)
        .map_err(|err| err.to_string())?;
    if reply.status != 200 {
        return Err(format!("the hub answered {}", reply.status));
    }
    let link: Value = serde_json::from_slice(&reply.body)
        .map_err(|err| format!("the hub's link is not JSON: {err}"))?;
    let path = link["path"].as_str().unwrap_or("tunnel").to_string();
    *lock(&MESH_TOKEN) = match (path.as_str(), link["mesh_token"].as_str()) {
        ("mesh", Some(token)) => Some(token.to_string()),
        _ => None,
    };
    Ok(path)
}

/// The status a refused upgrade carries: the text is `the server answered {status}, not 101 ...`.
fn refused_status(message: &str) -> Option<u16> {
    let rest = message.split("answered ").nth(1)?;
    rest.split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// Opens the tunnel: the node's signed WebSocket upgrade to its path on the hub.
pub fn connect(identity: &Identity, key: &NodeKey) -> Result<Arc<Mux>, Connect> {
    let path = format!("/v1/nodes/{}/tunnel", identity.node_id);
    let target = identity
        .signed(key, "GET", &path, &[])
        .map_err(Connect::Failed)?;
    match ws::connect(&target, &path, None, CONNECT_TIMEOUT, Limits::default()) {
        Ok(socket) => {
            LEGACY.store(false, Ordering::SeqCst);
            Ok(Mux::new(socket, Side::Dialer))
        }
        Err(err) => {
            let message = err.to_string();
            match refused_status(&message) {
                Some(403) => Err(Connect::Revoked),
                Some(409) => Err(Connect::Moved),
                Some(401 | 404) => {
                    LEGACY.store(true, Ordering::SeqCst);
                    Err(Connect::Legacy)
                }
                _ => Err(Connect::Failed(message)),
            }
        }
    }
}

/// Serves the hub's streams on `mux` until the session ends: each one on its own thread. A tunnel session
/// (`on_mesh` false) ends when a mesh session starts; a mesh session ends when the hub closes it.
pub fn serve(mux: Arc<Mux>, local: Local, who: Who, on_mesh: bool) {
    *lock(&CURRENT) = Some(Arc::clone(&mux));
    loop {
        if !on_mesh && mesh_up() {
            break;
        }
        match mux.accept(ACCEPT_POLL) {
            Ok(Some(stream)) => {
                let (local, who) = (local.clone(), who.clone());
                thread::spawn(move || answer(stream, &local, &who));
            }
            Ok(None) => {}
            Err(_) => break,
        }
    }
    let mut current = lock(&CURRENT);
    if current.as_ref().is_some_and(|now| Arc::ptr_eq(now, &mux)) {
        *current = None;
    }
}

/// Answers one stream the hub opened, by its header's kind.
fn answer(stream: Stream, local: &Local, who: &Who) {
    let header: Value = serde_json::from_slice(stream.header()).unwrap_or(Value::Null);
    match header["kind"].as_str() {
        Some("hello") => hello(&stream, who, &header),
        Some("ping") => {
            if stream.send(b"{}").is_ok() {
                let _ = stream.finish();
            }
        }
        Some("command") => {
            let result = execute(local, &header["command"]);
            let bytes = serde_json::to_vec(&result).unwrap_or_default();
            if stream.send(&bytes).is_ok() {
                let _ = stream.finish();
            }
        }
        Some("port") => {
            let cid = header["cid"].as_u64();
            let port = header["port"].as_u64().and_then(|p| u16::try_from(p).ok());
            match (cid, port) {
                (Some(cid), Some(port)) => relay(stream, local, cid, port),
                _ => stream.reset("a port stream needs a cid and a port"),
            }
        }
        _ => stream.reset("unknown stream kind"),
    }
}

/// Answers a hub's hello: the node signs the hub's nonce with its key (method `HELLO`), so the hub knows that the
/// machine on the mesh is this node. The answer is the signature's headers as a JSON object.
fn hello(stream: &Stream, who: &Who, header: &Value) {
    let Some(nonce) = header["nonce"].as_str() else {
        stream.reset("a hello needs a nonce");
        return;
    };
    let path = format!("/v1/nodes/{}/hello", who.node_id);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let signed: Map<String, Value> = who
        .key
        .sign(&who.node_id, "HELLO", &path, ts, nonce.as_bytes())
        .into_iter()
        .map(|(name, value)| (name.to_string(), Value::String(value)))
        .collect();
    let bytes = serde_json::to_vec(&Value::Object(signed)).unwrap_or_default();
    if stream.send(&bytes).is_ok() {
        let _ = stream.finish();
    }
}

/// Carries a port stream to a guest's port through vsockd's inbound listener, both ways, until either side closes.
fn relay(stream: Stream, local: &Local, cid: u64, port: u16) {
    let mut guest = match inbound::connect(local.inbound, cid, port) {
        Ok(guest) => guest,
        Err(err) => {
            stream.reset(&format!("the node cannot reach the fork's port: {err}"));
            return;
        }
    };
    let Ok(mut from_guest) = guest.try_clone() else {
        stream.reset("the fork's port could not be read");
        return;
    };
    let stream = Arc::new(stream);
    let outgoing = Arc::clone(&stream);
    thread::spawn(move || {
        let mut buf = vec![0u8; MAX_CHUNK];
        loop {
            match from_guest.read(&mut buf) {
                Ok(0) | Err(_) => {
                    let _ = outgoing.finish();
                    break;
                }
                Ok(n) => {
                    if outgoing.send(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
    });
    loop {
        match stream.recv(RELAY_POLL) {
            Some(Event::Data(bytes)) => {
                if std::io::Write::write_all(&mut guest, &bytes).is_err() {
                    stream.reset("the fork closed its port");
                    break;
                }
            }
            Some(Event::Fin) => {
                let _ = guest.shutdown(std::net::Shutdown::Write);
            }
            Some(Event::Reset(_) | Event::Closed) => {
                let _ = guest.shutdown(std::net::Shutdown::Both);
                break;
            }
            None => {}
        }
    }
}

/// A hub's answer whose body may still be arriving: a streamed answer is read as it comes, not all at once.
pub struct Answer {
    pub status: u16,
    pub content_type: String,
    /// Whether the hub streamed the body (its `stream` head field), rather than sending it whole.
    pub streamed: bool,
    pub body: Box<dyn Read + Send>,
}

/// One request to the hub, answered with its body still to be read. Over the tunnel while it is up; over HTTP to a hub
/// that predates the tunnel. The target's headers (the node's signature, and any the caller added) travel with it.
pub fn request_answer(
    target: &Target,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> io::Result<Answer> {
    if LEGACY.load(Ordering::SeqCst) {
        let reply = client::request(target, method, path, body, timeout)?;
        return Ok(Answer {
            status: reply.status,
            content_type: reply.content_type,
            streamed: false,
            body: Box::new(io::Cursor::new(reply.body)),
        });
    }
    let mux = current().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotConnected, "the tunnel to the hub is down")
    })?;
    let header =
        json!({"kind": "request", "method": method, "path": path, "headers": target.headers});
    let stream = mux.open(header.to_string().as_bytes())?;
    stream.send(body.unwrap_or_default())?;
    stream.finish()?;
    let (head, rest) = read_head(&stream, timeout)?;
    Ok(Answer {
        status: head.status,
        content_type: head.content_type,
        streamed: head.streamed,
        body: Box::new(Rest {
            stream,
            pending: rest,
            wait: timeout,
            ended: false,
        }),
    })
}

/// One request to the hub, answered whole. See [`request_answer`].
pub fn request(
    target: &Target,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> io::Result<Reply> {
    let mut answer = request_answer(target, method, path, body, timeout)?;
    let mut bytes = Vec::new();
    answer.body.read_to_end(&mut bytes)?;
    Ok(Reply {
        status: answer.status,
        content_type: answer.content_type,
        body: bytes,
        headers: Vec::new(),
    })
}

/// The head line of a hub answer.
struct Head {
    status: u16,
    content_type: String,
    streamed: bool,
}

/// Reads the head line of an answer, and returns the bytes that followed it on the stream.
fn read_head(stream: &Stream, wait: Duration) -> io::Result<(Head, Vec<u8>)> {
    let mut buf = Vec::new();
    loop {
        if let Some(at) = buf.iter().position(|b| *b == b'\n') {
            let head = parse_head(&buf[..at])?;
            return Ok((head, buf[at + 1..].to_vec()));
        }
        match stream.recv(wait) {
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the hub did not answer",
                ));
            }
            Some(Event::Data(bytes)) => buf.extend_from_slice(&bytes),
            Some(Event::Fin) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the hub's answer has no head",
                ));
            }
            Some(Event::Reset(reason)) => {
                return Err(io::Error::new(io::ErrorKind::ConnectionReset, reason));
            }
            Some(Event::Closed) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "the tunnel closed",
                ));
            }
        }
    }
}

/// The head line: `{"status", "content_type", "stream"}`. A missing status is a 502.
fn parse_head(line: &[u8]) -> io::Result<Head> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the hub's answer has a bad head",
        )
    };
    let head: Value = serde_json::from_slice(line).map_err(|_| invalid())?;
    Ok(Head {
        status: head["status"]
            .as_u64()
            .and_then(|s| u16::try_from(s).ok())
            .unwrap_or(502),
        content_type: head["content_type"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        streamed: head["stream"].as_bool().unwrap_or(false),
    })
}

/// The rest of a hub answer: what its tunnel stream carries after the head, in order.
struct Rest {
    stream: Stream,
    pending: Vec<u8>,
    wait: Duration,
    ended: bool,
}

impl Read for Rest {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if !self.pending.is_empty() {
            let n = out.len().min(self.pending.len());
            out[..n].copy_from_slice(&self.pending[..n]);
            self.pending.drain(..n);
            return Ok(n);
        }
        loop {
            if self.ended {
                return Ok(0);
            }
            match self.stream.recv(self.wait) {
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the hub stopped answering",
                    ));
                }
                Some(Event::Data(bytes)) => {
                    let n = out.len().min(bytes.len());
                    out[..n].copy_from_slice(&bytes[..n]);
                    self.pending.extend_from_slice(&bytes[n..]);
                    if n > 0 {
                        return Ok(n);
                    }
                }
                Some(Event::Fin) => {
                    self.ended = true;
                    return Ok(0);
                }
                Some(Event::Reset(reason)) => {
                    return Err(io::Error::new(io::ErrorKind::ConnectionReset, reason));
                }
                Some(Event::Closed) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "the tunnel closed",
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refused_status_is_read_from_the_handshake_error() {
        assert_eq!(
            refused_status("the server answered 403, not 101 Switching Protocols"),
            Some(403)
        );
        assert_eq!(refused_status("the connection reset"), None);
    }

    #[test]
    fn a_head_says_its_status_type_and_whether_the_body_streams() {
        let head =
            parse_head(br#"{"status":201,"content_type":"text/event-stream","stream":true}"#)
                .expect("parses");
        assert_eq!(head.status, 201);
        assert_eq!(head.content_type, "text/event-stream");
        assert!(head.streamed);
        let whole =
            parse_head(br#"{"status":200,"content_type":"application/json"}"#).expect("parses");
        assert!(!whole.streamed, "a body sent whole is not a stream");
        assert_eq!(
            parse_head(b"not json").err().map(|e| e.kind()),
            Some(io::ErrorKind::InvalidData)
        );
        assert_eq!(parse_head(br#"{}"#).expect("parses").status, 502);
    }
}
