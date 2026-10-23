//! The node's end of its tunnel (PLAN 17.4): one outbound WebSocket to the hub, `/v1/nodes/{id}/tunnel`, carrying every
//! stream between the node and the hub (apz_http::mux). Four kinds of stream, named by the header each opens with:
//! - `command` (hub → node): one command for noded or vsockd, run by [`execute`]; its result JSON is the answer.
//! - `port` (hub → node): bytes both ways to a fork's port, dialled at the address the node's [`PortMap`] gives. A
//!   terminal's bytes and a model call's deltas ride these.
//! - `request` (node → hub): one signed HTTP request (heartbeats, results, metrics, the agents' calls to the hub). The
//!   answer is one line of JSON `{status, content_type}`, then the body. [`request`] sends them; the hub's router answers.
//!
//! A hub that predates the tunnel answers the upgrade with 401 or 404; the node then keeps the long poll and its HTTP
//! requests ([`LEGACY`]) and tries the tunnel again between polls. A refused upgrade with 403 is a revoked node.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use apz_http::client::{self, Reply, Target};
use apz_http::mux::{Event, MAX_CHUNK, Mux, Side, Stream};
use apz_http::ws::{self, Limits};
use apz_trust::node::NodeKey;
use serde_json::{Value, json};

use crate::identity::Identity;
use crate::run::{Local, execute};

/// The longest the upgrade handshake may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a fork's port may take to answer a dial.
const DIAL_TIMEOUT: Duration = Duration::from_secs(5);
/// How often a relayed port checks whether the tunnel is still up.
const RELAY_POLL: Duration = Duration::from_millis(500);
/// How often the accepting loop looks for a stream while the tunnel is quiet.
const ACCEPT_POLL: Duration = Duration::from_millis(500);

/// Set while the hub predates the tunnel: requests then go over HTTP, as they did before.
static LEGACY: AtomicBool = AtomicBool::new(false);
/// The tunnel now up, if any.
static CURRENT: Mutex<Option<Arc<Mux>>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where the ports of forks on this node lead: (guest CID, guest port) → the local address its traffic goes to.
#[derive(Debug, Clone, Default)]
pub struct PortMap(Arc<Mutex<BTreeMap<(u64, u16), SocketAddr>>>);

impl PortMap {
    /// Routes a fork's port to `addr`.
    pub fn set(&self, cid: u64, port: u16, addr: SocketAddr) {
        lock(&self.0).insert((cid, port), addr);
    }

    /// Stops routing a fork's port.
    pub fn clear(&self, cid: u64, port: u16) {
        lock(&self.0).remove(&(cid, port));
    }

    /// Where a fork's port leads, if it is routed.
    pub fn resolve(&self, cid: u64, port: u16) -> Option<SocketAddr> {
        lock(&self.0).get(&(cid, port)).copied()
    }
}

/// Why the tunnel could not be opened.
#[derive(Debug)]
pub enum Connect {
    /// The hub refused the node's upgrade with 403: revoked.
    Revoked,
    /// The hub does not serve the tunnel (401 or 404): it predates it.
    Legacy,
    /// Anything else: retried.
    Failed(String),
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
                Some(401 | 404) => {
                    LEGACY.store(true, Ordering::SeqCst);
                    Err(Connect::Legacy)
                }
                _ => Err(Connect::Failed(message)),
            }
        }
    }
}

/// Serves the hub's streams on `mux` until the tunnel ends: each one on its own thread.
pub fn serve(mux: Arc<Mux>, local: Local) {
    *lock(&CURRENT) = Some(Arc::clone(&mux));
    loop {
        match mux.accept(ACCEPT_POLL) {
            Ok(Some(stream)) => {
                let local = local.clone();
                thread::spawn(move || answer(stream, &local));
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
fn answer(stream: Stream, local: &Local) {
    let header: Value = serde_json::from_slice(stream.header()).unwrap_or(Value::Null);
    match header["kind"].as_str() {
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

/// Carries a port stream to the fork's port, both ways, until either side closes.
fn relay(stream: Stream, local: &Local, cid: u64, port: u16) {
    let Some(addr) = local.ports.resolve(cid, port) else {
        stream.reset("no fork listens on that port");
        return;
    };
    let Ok(mut fork) = TcpStream::connect_timeout(&addr, DIAL_TIMEOUT) else {
        stream.reset("the fork's port is not reachable");
        return;
    };
    let Ok(mut from_fork) = fork.try_clone() else {
        stream.reset("the fork's port could not be read");
        return;
    };
    let stream = Arc::new(stream);
    let outgoing = Arc::clone(&stream);
    thread::spawn(move || {
        let mut buf = vec![0u8; MAX_CHUNK];
        loop {
            match from_fork.read(&mut buf) {
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
                if fork.write_all(&bytes).is_err() {
                    stream.reset("the fork closed its port");
                    break;
                }
            }
            Some(Event::Fin) => {
                let _ = fork.shutdown(Shutdown::Write);
            }
            Some(Event::Reset(_) | Event::Closed) => {
                let _ = fork.shutdown(Shutdown::Both);
                break;
            }
            None => {}
        }
    }
}

/// One request to the hub. Over the tunnel while it is up; over HTTP to a hub that predates the tunnel. The target's
/// headers (the node's signature, and any the caller added) travel with it.
pub fn request(
    target: &Target,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> io::Result<Reply> {
    if LEGACY.load(Ordering::SeqCst) {
        return client::request(target, method, path, body, timeout);
    }
    let mux = current().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotConnected, "the tunnel to the hub is down")
    })?;
    let header =
        json!({"kind": "request", "method": method, "path": path, "headers": target.headers});
    let stream = mux.open(header.to_string().as_bytes())?;
    stream.send(body.unwrap_or_default())?;
    stream.finish()?;
    let answer = stream.read_all(timeout)?;
    reply(&answer)
}

/// The hub's answer: one line of JSON (`status`, `content_type`), then the body.
fn reply(answer: &[u8]) -> io::Result<Reply> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "the hub's answer has no head");
    let at = answer
        .iter()
        .position(|b| *b == b'\n')
        .ok_or_else(invalid)?;
    let head: Value = serde_json::from_slice(&answer[..at]).map_err(|_| invalid())?;
    Ok(Reply {
        status: head["status"]
            .as_u64()
            .and_then(|s| u16::try_from(s).ok())
            .unwrap_or(502),
        content_type: head["content_type"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        body: answer[at + 1..].to_vec(),
        headers: Vec::new(),
    })
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
    fn a_reply_is_its_head_line_then_the_body() {
        let answer = b"{\"status\":201,\"content_type\":\"application/json\"}\n{\"ok\":true}";
        let got = reply(answer).expect("parses");
        assert_eq!(got.status, 201);
        assert_eq!(got.content_type, "application/json");
        assert_eq!(got.body, b"{\"ok\":true}");
        assert!(reply(b"no newline here").is_err());
    }

    #[test]
    fn a_port_map_routes_and_forgets() {
        let map = PortMap::default();
        let addr: SocketAddr = "127.0.0.1:9".parse().expect("an address");
        assert_eq!(map.resolve(1000, 7681), None);
        map.set(1000, 7681, addr);
        assert_eq!(map.resolve(1000, 7681), Some(addr));
        map.clear(1000, 7681);
        assert_eq!(map.resolve(1000, 7681), None);
    }
}
