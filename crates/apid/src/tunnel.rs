//! The hub's end of a node's link (PLAN 17.4, Engine 6.2): one WebSocket per joined node, `/v1/nodes/{id}/tunnel`, dialled
//! by the node (the tunnel), or the hub's own dial to the node's mesh address (`keep_mesh`, the mesh). Either carries every
//! stream between the hub and the node (apz_http::mux):
//! - sends each command queued for the node as a `command` stream (`nodes::Store::enqueue`) and stores its result
//!   (`put_result`), so the callers that wait for results are unchanged;
//! - opens `port` streams to a fork's port through the node (`nodes::Store::open_port`): a terminal's bytes, a model
//!   call's deltas;
//! - answers the node's `request` streams (heartbeats, results, metrics, the agents' calls) through the same gate and
//!   routes as an HTTP request, with the node's signature headers as they were sent;
//! - pings the node every few seconds and records the round trip and any failure (`Store::note_latency`, `note_error`),
//!   which the Nodes screen shows.
//!
//! A node's path (`Node::path`) says which link the hub uses: `tunnel` (the node's dial; a session ends when the path
//! becomes `mesh`) or `mesh` (the hub's dial to the node's mesh address, `keep_mesh`, with a bearer and the node's key:
//! the node answers a `hello` by signing the hub's nonce; a session ends when the path becomes `tunnel`). The forks on
//! the node are the same either way: only the hub's way to them changes.
//!
//! The handshake is the caller's to authenticate (the route checks the node's signature before [`open`]).

use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use apz_http::client::Target;
use apz_http::mux::{Mux, Side, Stream};
use apz_http::ws::{self, Limits, Origins};
use apz_http::{Body, Request, Response};
use apz_trust::node::{H_NONCE, H_SIG, H_TS};
use serde_json::{Value, json};

use crate::nodes::{Command, CommandResult, Node, Store, check_freshness};
use crate::state::App;

/// Answers a request a node sent over its tunnel: the hub's gate and routes, as they answer an HTTP request.
pub type Dispatch = Arc<dyn Fn(&Request) -> Response + Send + Sync>;

/// How long a node's request may take to arrive whole, and how long a command may take to answer (a node runs one
/// for at most 60 s).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(90);
/// How long a session waits for queued commands before it looks at the tunnel again.
const QUEUE_WAIT_MS: u64 = 250;
/// How often a stream acceptor looks at whether the tunnel is still up.
const ACCEPT_WAIT: Duration = Duration::from_millis(500);
/// How often a session pings its node, and how long a ping may take.
const PING_EVERY: Duration = Duration::from_secs(5);
const PING_TIMEOUT: Duration = Duration::from_secs(5);
/// The keeper's dial: how long it may take to connect, to prove the node, and its retry back-off (1 s doubling to 30 s).
const MESH_CONNECT: Duration = Duration::from_secs(10);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const MESH_RETRY_MIN: Duration = Duration::from_secs(1);
const MESH_RETRY_MAX: Duration = Duration::from_secs(30);
/// How often the hub looks for nodes on the mesh that have no keeper yet.
const MESH_SCAN: Duration = Duration::from_secs(2);

/// The tunnel's handshake, after the route has authenticated the node: the WebSocket upgrade. The socket then runs the
/// session on its own thread until the node disconnects. A node on the mesh is not dialled here: 409.
pub fn open(req: &Request, node: String, store: Arc<Store>, dispatch: Dispatch) -> Response {
    if store.on_mesh(&node) {
        return Response::json(
            409,
            &json!({"error": "this node is on the mesh", "path": "mesh"}),
        );
    }
    let peer = req.peer;
    ws::accept(req, Origins::None, None, Limits::default(), move |socket| {
        session(
            Mux::new(socket, Side::Acceptor),
            node,
            peer,
            store,
            dispatch,
            false,
        )
    })
}

/// Runs one link to a node: serves its streams, sends its queued commands, and pings it, until the link ends or the
/// node's path leaves this link (`mesh` says which path this link is). Then the link is forgotten, so the queue waits
/// for the node's next link.
fn session(
    mux: Arc<Mux>,
    node: String,
    peer: Option<IpAddr>,
    store: Arc<Store>,
    dispatch: Dispatch,
    mesh: bool,
) {
    store.set_tunnel(&node, Arc::clone(&mux));
    {
        let (mux, dispatch) = (Arc::clone(&mux), Arc::clone(&dispatch));
        let (store, node) = (Arc::clone(&store), node.clone());
        thread::spawn(move || accept_streams(&mux, peer, &dispatch, &store, &node, mesh));
    }
    {
        let (mux, store, node) = (Arc::clone(&mux), Arc::clone(&store), node.clone());
        thread::spawn(move || ping_forever(&mux, &store, &node, mesh));
    }
    while mux.alive() && store.on_mesh(&node) == mesh {
        for command in store.take(&node, QUEUE_WAIT_MS) {
            send_command(&mux, &store, &node, command);
        }
    }
    store.clear_tunnel(&node, &mux);
}

/// Answers each stream the node opens, until the link ends or its path leaves this session. Returning matters: the
/// thread holds the connection, which closes only when its last holder lets go (`Mux` closes on drop).
fn accept_streams(
    mux: &Mux,
    peer: Option<IpAddr>,
    dispatch: &Dispatch,
    store: &Store,
    node: &str,
    mesh: bool,
) {
    loop {
        match mux.accept(ACCEPT_WAIT) {
            Ok(Some(stream)) => {
                // A lab segment's trunk is spliced to the other node's trunk for the same segment, not answered.
                if let Some(segment) = apz_http::trunk::trunk_segment(&stream) {
                    trunks().offer(&segment, stream);
                    continue;
                }
                let dispatch = Arc::clone(dispatch);
                thread::spawn(move || answer(&stream, peer, &dispatch));
            }
            Ok(None) => {
                if !mux.alive() || store.on_mesh(node) != mesh {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

/// Pings the node every [`PING_EVERY`] while the link is up on its path: the round trip is the link's latency, and a
/// ping that fails is an error on the link.
fn ping_forever(mux: &Mux, store: &Store, node: &str, mesh: bool) {
    while mux.alive() && store.on_mesh(node) == mesh {
        let started = Instant::now();
        match ping(mux) {
            Ok(()) => store.note_latency(node, started.elapsed()),
            Err(reason) => store.note_error(node, &format!("ping: {reason}")),
        }
        thread::sleep(PING_EVERY);
    }
}

/// One ping: a `ping` stream the node finishes at once.
fn ping(mux: &Mux) -> Result<(), String> {
    let stream = mux
        .open(br#"{"kind":"ping"}"#)
        .map_err(|err| err.to_string())?;
    let _ = stream.finish();
    stream
        .read_all(PING_TIMEOUT)
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// Keeps the hub's dial to a node on the mesh while the node's path is `mesh`: dials its mesh address with the mesh
/// bearer, proves the node with a `hello`, serves the session, and dials again when it ends. Returns when the node is
/// no longer on the mesh, or is revoked or gone; the scan (`spawn_mesh`) starts it again when the node comes back.
pub fn keep_mesh(store: Arc<Store>, node: String, dispatch: Dispatch) {
    let mut backoff = MESH_RETRY_MIN;
    loop {
        let Some(found) = store.get(&node).filter(|n| n.on_mesh() && !n.revoked) else {
            return;
        };
        let target = Target {
            base: format!("http://{}", found.mesh_address),
            pin: None,
            bearer: Some(found.mesh_token.clone()),
            headers: Vec::new(),
        };
        match ws::connect(&target, "/v1/mesh", None, MESH_CONNECT, Limits::default()) {
            Ok(socket) => {
                let mux = Mux::new(socket, Side::Dialer);
                match hello(&mux, &node, &store) {
                    Ok(()) => {
                        backoff = MESH_RETRY_MIN;
                        session(
                            mux,
                            node.clone(),
                            None,
                            Arc::clone(&store),
                            dispatch.clone(),
                            true,
                        );
                    }
                    Err(reason) => store.note_error(
                        &node,
                        &format!("the mesh dial did not prove the node's key: {reason}"),
                    ),
                }
            }
            Err(err) => store.note_error(
                &node,
                &format!("the mesh does not reach {}: {err}", found.mesh_address),
            ),
        }
        thread::sleep(backoff);
        backoff = (backoff * 2).min(MESH_RETRY_MAX);
    }
}

/// The node's key on top of the mesh's: the hub sends a fresh nonce on a `hello` stream, and the node answers with its
/// signature over it (`apz_trust::node`, method `HELLO`). Only the node's key makes that signature.
fn hello(mux: &Mux, node: &str, store: &Store) -> Result<(), String> {
    let nonce = apz_trust::b64(&apz_trust::random::<16>());
    let stream = mux
        .open(
            json!({"kind": "hello", "nonce": nonce})
                .to_string()
                .as_bytes(),
        )
        .map_err(|err| err.to_string())?;
    let _ = stream.finish();
    let bytes = stream
        .read_all(HELLO_TIMEOUT)
        .map_err(|err| err.to_string())?;
    let answer: Value =
        serde_json::from_slice(&bytes).map_err(|_| "the node's answer is not JSON".to_string())?;
    let field = |name: &str| {
        answer[name]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("the answer has no {name}"))
    };
    let ts: u64 = field(H_TS)?
        .parse()
        .map_err(|_| "the answer's timestamp is not a number".to_string())?;
    check_freshness(ts, apz_plaza::now_ms())?;
    let public = store
        .get(node)
        .map(|n| n.public_key)
        .ok_or("the node is not known")?;
    let path = format!("/v1/nodes/{node}/hello");
    let ok = apz_trust::node::verify(
        &public,
        "HELLO",
        &path,
        ts,
        &field(H_NONCE)?,
        nonce.as_bytes(),
        &field(H_SIG)?,
    );
    if ok {
        Ok(())
    } else {
        Err("bad signature".into())
    }
}

/// The node's link as the node asks for it (`GET /v1/nodes/{id}/link`, signed by the node): its path, and on the mesh the
/// bearer its dial presents.
pub fn link_of(node: &Node) -> Value {
    if node.on_mesh() {
        json!({"path": "mesh", "mesh_token": node.mesh_token})
    } else {
        json!({"path": "tunnel"})
    }
}

/// Starts the keeper of each node whose path is the mesh, when it has none: at start, and again each time a node's path
/// comes back to the mesh. Runs for the life of the hub.
pub fn spawn_mesh(app: Arc<App>) {
    thread::spawn(move || {
        loop {
            thread::sleep(MESH_SCAN);
            for node in app.nodes.list() {
                if node.revoked || !node.on_mesh() || !app.nodes.start_keeper(&node.id) {
                    continue;
                }
                match crate::nodes::dispatch(&app) {
                    Some(dispatch) => {
                        let (store, id) = (Arc::clone(&app.nodes), node.id.clone());
                        thread::spawn(move || {
                            keep_mesh(Arc::clone(&store), id.clone(), dispatch);
                            store.end_keeper(&id);
                        });
                    }
                    None => app.nodes.end_keeper(&node.id),
                }
            }
        }
    });
}

/// Sends one queued command as a `command` stream, and stores its result when the node answers. A command the tunnel
/// cannot carry is answered with a failure at once.
fn send_command(mux: &Arc<Mux>, store: &Arc<Store>, node: &str, command: Command) {
    let id = command.id.clone();
    let header = json!({"kind": "command", "command": command}).to_string();
    match mux.open(header.as_bytes()) {
        Ok(stream) => {
            let (store, node) = (Arc::clone(store), node.to_string());
            thread::spawn(move || {
                let _ = stream.finish();
                let result = match stream.read_all(COMMAND_TIMEOUT) {
                    Ok(bytes) => serde_json::from_slice::<CommandResult>(&bytes)
                        .unwrap_or_else(|_| failed(id, "the node's answer is not a result")),
                    Err(err) => failed(id, &err.to_string()),
                };
                store.put_result(&node, result);
            });
        }
        Err(err) => store.put_result(node, failed(id, &err.to_string())),
    }
}

fn failed(command: String, reason: &str) -> CommandResult {
    CommandResult {
        command,
        ok: false,
        status: 502,
        body: json!({"error": reason}),
    }
}

/// The hub's pairing of lab trunks across nodes: one table for the process, keyed by segment.
fn trunks() -> &'static apz_http::trunk::TrunkHub {
    static HUB: std::sync::OnceLock<apz_http::trunk::TrunkHub> = std::sync::OnceLock::new();
    HUB.get_or_init(apz_http::trunk::TrunkHub::new)
}

/// Answers one stream the node opened. Only `request` streams come from a node; anything else is reset.
fn answer(stream: &Stream, peer: Option<IpAddr>, dispatch: &Dispatch) {
    let header: Value = serde_json::from_slice(stream.header()).unwrap_or(Value::Null);
    if header["kind"].as_str() != Some("request") {
        stream.reset("unknown stream kind");
        return;
    }
    let request = match stream.read_all(REQUEST_TIMEOUT) {
        Ok(body) => build_request(&header, body, peer),
        Err(err) => Err(err.to_string()),
    };
    match request {
        Ok(request) => write_response(stream, dispatch(&request)),
        Err(reason) => stream.reset(&reason),
    }
}

/// The request a node sent: its method, path (and query), headers and body, as an HTTP request off the wire would be.
fn build_request(header: &Value, body: Vec<u8>, peer: Option<IpAddr>) -> Result<Request, String> {
    let method = header["method"]
        .as_str()
        .ok_or("a request needs a method")?
        .to_string();
    let target = header["path"].as_str().ok_or("a request needs a path")?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let headers: Vec<(String, String)> =
        serde_json::from_value::<Vec<(String, String)>>(header["headers"].clone())
            .map_err(|err| format!("the request's headers: {err}"))?
            .into_iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
            .collect();
    Ok(Request {
        method,
        path: path.to_string(),
        query: query.to_string(),
        headers,
        body,
        peer,
    })
}

/// The answer, as the node reads it: one line of JSON (`status`, `content_type`, `stream`), then the body. A streamed
/// body (`stream`: true) is written as the response produces it, so the node passes each chunk on as it comes.
fn write_response(stream: &Stream, response: Response) {
    let content_type = response
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    let status = response.status;
    let head = |status: u16, streamed: bool| {
        format!(
            "{}\n",
            json!({"status": status, "content_type": content_type, "stream": streamed})
        )
    };
    match response.body {
        Body::Bytes(bytes) => {
            if stream.send(head(status, false).as_bytes()).is_ok() && stream.send(&bytes).is_ok() {
                let _ = stream.finish();
            }
        }
        Body::Stream(write) => {
            if stream.send(head(status, true).as_bytes()).is_err() {
                return;
            }
            match write(&mut Chunks(stream)) {
                Ok(()) => {
                    let _ = stream.finish();
                }
                Err(err) => stream.reset(&format!("the streamed answer failed: {err}")),
            }
        }
        Body::Upgrade(_) => {
            let message = b"an upgraded answer does not travel the tunnel";
            if stream.send(head(501, false).as_bytes()).is_ok() && stream.send(message).is_ok() {
                let _ = stream.finish();
            }
        }
    }
}

/// The writer a streamed body writes into: each write goes down the stream, in order.
struct Chunks<'a>(&'a Stream);

impl io::Write for Chunks<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.send(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Instant;

    use crate::nodes::{CIDS_PER_NODE, Node, REMOTE_CID_BASE};
    use apz_http::{Router, Server};
    use apz_link::identity::Identity;
    use apz_link::run::Local;
    use apz_link::tunnel as node_tunnel;
    use apz_proto::chat::Delta;
    use apz_proto::wire::HostMessage;
    use apz_trust::node::NodeKey;

    /// A fork's port that echoes what it reads, once per connection, then closes its side.
    fn echo_fork() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let addr = listener.local_addr().expect("address");
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    loop {
                        match std::io::Read::read(&mut stream, &mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if stream.write_all(&buf[..n]).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                });
            }
        });
        addr
    }

    /// A fork's port that says which connection it is (`fork 1`, `fork 2`, ...) and echoes the rest: a fork that
    /// kept running answers with the next number, a restarted one starts again at 1.
    fn counting_fork() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let addr = listener.local_addr().expect("address");
        thread::spawn(move || {
            let mut connections = 0u64;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                connections += 1;
                let line = format!("fork {connections}\n");
                thread::spawn(move || {
                    let _ = stream.write_all(line.as_bytes());
                    let mut buf = [0u8; 1024];
                    while matches!(std::io::Read::read(&mut stream, &mut buf), Ok(n) if n > 0) {}
                });
            }
        });
        addr
    }

    /// A fork's port that reads the model's deltas in order, checks them, and answers with how many arrived.
    fn model_fork() -> (std::net::SocketAddr, mpsc::Receiver<Result<usize, String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let addr = listener.local_addr().expect("address");
        let (done, result) = mpsc::channel();
        thread::spawn(move || {
            let outcome = (|| -> Result<usize, String> {
                let (mut stream, _) = listener.accept().map_err(|e| e.to_string())?;
                let mut lines = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
                let mut seen = 0usize;
                let mut line = String::new();
                loop {
                    line.clear();
                    if lines.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                        break;
                    }
                    let message: HostMessage =
                        serde_json::from_str(line.trim_end()).map_err(|e| e.to_string())?;
                    match message {
                        HostMessage::ModelDelta {
                            req,
                            delta: Delta::ToolInput { index, json },
                        } => {
                            if req != 7 || index != seen {
                                return Err(format!(
                                    "delta {seen} arrived as {index} for call {req}"
                                ));
                            }
                            if json.len() < 4000 {
                                return Err("a delta lost its text".to_string());
                            }
                            seen += 1;
                        }
                        other => return Err(format!("not a model delta: {other:?}")),
                    }
                }
                stream
                    .write_all(format!("received {seen}\n").as_bytes())
                    .map_err(|e| e.to_string())?;
                Ok(seen)
            })();
            let _ = done.send(outcome);
        });
        (addr, result)
    }

    /// The first bytes a stream receives, up to the first line break.
    fn first_line(stream: &Stream) -> Vec<u8> {
        let mut bytes = Vec::new();
        while !bytes.contains(&b'\n') {
            match stream.recv(Duration::from_secs(5)) {
                Some(apz_http::mux::Event::Data(more)) => bytes.extend_from_slice(&more),
                other => panic!("the fork did not answer: {other:?}"),
            }
        }
        bytes
    }

    /// A free port on `host`.
    fn free_port(host: &str) -> u16 {
        TcpListener::bind(format!("{host}:0"))
            .expect("binds")
            .local_addr()
            .expect("address")
            .port()
    }

    /// The end-to-end test: a hub (the tunnel's route behind TLS, pinned by the node) and one node link over loopback.
    /// The node's link runs as the node runs it (`apz_link::run::link`), so it redials and answers the mesh the way it
    /// does in production. One test, because the node link keeps its tunnel and mesh state in process-wide slots.
    #[test]
    fn a_node_carries_its_streams_over_the_tunnel_then_the_mesh_and_keeps_its_forks() {
        // The hub's certificate and the node's identity, pinned as a join would pin them.
        let pair = apz_trust::tls::self_signed(&["localhost".to_string()]).expect("a certificate");
        let pin = apz_trust::tls::fingerprint(&pair.cert_pem).expect("a pin");
        let tls = apz_trust::tls::server_config(&pair).expect("a server config");
        let node_key = Arc::new(NodeKey::generate());
        // The test's "mesh" is a second loopback listener the hub dials directly: on Linux, where all of 127/8 is loopback,
        // a second address (127.0.0.2); on macOS only 127.0.0.1 is routed without an alias (an alias would change the
        // machine's network settings), so there it is a second port on 127.0.0.1.
        let mesh_host = if cfg!(target_os = "linux") {
            "127.0.0.2"
        } else {
            "127.0.0.1"
        };
        let mesh_addr = format!("{mesh_host}:{}", free_port(mesh_host));
        let store = Arc::new(Store::holding(vec![Node {
            id: "n1".into(),
            name: "box".into(),
            public_key: node_key.public_b64(),
            cid_base: REMOTE_CID_BASE,
            cid_count: CIDS_PER_NODE,
            joined_ms: 0,
            last_seen_ms: 0,
            machine: Default::default(),
            draining: false,
            revoked: false,
            address: String::new(),
            label: String::new(),
            test_share: 0,
            path: "tunnel".into(),
            mesh_address: String::new(),
            mesh_token: String::new(),
        }]));

        // The hub's requests from the node are answered only when the node's signature checks out.
        let check = Arc::clone(&store);
        let dispatch: Dispatch = Arc::new(move |request: &Request| {
            match check.authenticate(request, apz_plaza::now_ms()) {
                Ok(_) if request.path == "/v1/nodes/n1/stream" => Response {
                    status: 200,
                    headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
                    body: Body::Stream(Box::new(|out: &mut dyn Write| -> io::Result<()> {
                        for index in 0..5 {
                            write!(out, "data: {index}\n\n")?;
                        }
                        Ok(())
                    })),
                },
                Ok(node) => Response::json(
                    200,
                    &json!({"node": node.id, "bytes": request.body.len(), "path": request.path, "query": request.query}),
                ),
                Err(reason) => Response::error(401, &reason),
            }
        });
        let route_store = Arc::clone(&store);
        let route_dispatch = Arc::clone(&dispatch);
        let link_store = Arc::clone(&store);
        let router: Router<()> = Router::new()
            .get("/v1/nodes/n1/tunnel", move |_, req, _| {
                match route_store.authenticate(req, apz_plaza::now_ms()) {
                    Ok(node) => open(
                        req,
                        node.id,
                        Arc::clone(&route_store),
                        Arc::clone(&route_dispatch),
                    ),
                    Err(reason) => Response::error(401, &reason),
                }
            })
            .get("/v1/nodes/n1/link", move |_, req, _| {
                match link_store.authenticate(req, apz_plaza::now_ms()) {
                    Ok(node) => Response::json(200, &link_of(&node)),
                    Err(reason) => Response::error(401, &reason),
                }
            });
        let hub = Server::bind(0)
            .expect("an ephemeral port")
            .tls(tls)
            .spawn(move |request| router.handle(&(), request))
            .expect("serves");

        // A fake noded for the command, and a fork's terminal, model and counting ports, reached through the node's
        // relay and vsockd's inbound listener.
        let noded_router: Router<()> = Router::new().get("/v1/runs", |_, _, _| {
            Response::json(200, &json!({"runs": []}))
        });
        let noded = Server::bind(0)
            .expect("an ephemeral port")
            .spawn(move |request| noded_router.handle(&(), request))
            .expect("serves");
        let terminal = echo_fork();
        let counted = counting_fork();
        let (model, model_result) = model_fork();
        // vsockd's inbound listener, with the table of what runs serve and where their guests answer on loopback.
        let mut table = apz_link::inbound::Table::default();
        table.served.lock().unwrap().insert((1000, 7681));
        table.served.lock().unwrap().insert((1000, 7682));
        table.served.lock().unwrap().insert((1000, 7690));
        table.guests.insert((1000, 7681), terminal);
        table.guests.insert((1000, 7682), counted);
        table.guests.insert((1000, 7690), model);
        let inbound_listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let inbound_port = inbound_listener.local_addr().expect("address").port();
        let table = Arc::new(table);
        thread::spawn(move || apz_link::inbound::serve(inbound_listener, table.clone(), table));

        let identity = Arc::new(Identity {
            hub_url: format!("https://localhost:{}", hub.port()),
            pin: apz_trust::b64(&pin),
            node_id: "n1".into(),
            cid_base: REMOTE_CID_BASE,
            cid_count: CIDS_PER_NODE,
            hub_name: "test".into(),
            mesh: Some(mesh_addr.clone()),
        });
        let local = Local {
            noded: noded.port(),
            vsockd: 0,
            forward: 0,
            token: apz_http::auth::InternalToken::none(),
            inbound: inbound_port,
            transfer: 0,
        };

        // The node runs its link: it dials the hub, and listens at its mesh address for the hub's dial.
        {
            let (identity, key, local, mesh) = (
                Arc::clone(&identity),
                Arc::clone(&node_key),
                local.clone(),
                mesh_addr.clone(),
            );
            thread::spawn(move || apz_link::run::link(identity, key, local, Some(mesh.as_str())));
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while store.tunnel("n1").is_none() {
            assert!(Instant::now() < deadline, "the hub never saw the tunnel");
            thread::sleep(Duration::from_millis(10));
        }

        // A terminal: bytes both ways through the fork's port.
        let terminal_stream = store
            .open_port("n1", 1000, 7681)
            .expect("a terminal stream opens");
        terminal_stream.send(b"ls -la\n").expect("sends");
        let mut echoed = Vec::new();
        while echoed.len() < 7 {
            match terminal_stream.recv(Duration::from_secs(5)) {
                Some(apz_http::mux::Event::Data(bytes)) => echoed.extend_from_slice(&bytes),
                other => panic!("the terminal did not echo: {other:?}"),
            }
        }
        assert_eq!(echoed, b"ls -la\n");
        terminal_stream.send(b"second\n").expect("sends again");
        terminal_stream.finish().expect("finishes");
        assert_eq!(
            terminal_stream
                .read_all(Duration::from_secs(5))
                .expect("the echo ends"),
            b"second\n"
        );

        // A model call: 200 deltas of about 4 KiB each (more than a window), in order, on one call.
        let model_stream = store
            .open_port("n1", 1000, 7690)
            .expect("a model stream opens");
        for index in 0..200 {
            let message = HostMessage::ModelDelta {
                req: 7,
                delta: Delta::ToolInput {
                    index,
                    json: "x".repeat(4096),
                },
            };
            let mut line = serde_json::to_string(&message).expect("serializes");
            line.push('\n');
            model_stream.send(line.as_bytes()).expect("sends a delta");
        }
        model_stream.finish().expect("finishes the call");
        let answer = model_stream
            .read_all(Duration::from_secs(20))
            .expect("the model fork answers");
        assert_eq!(answer, b"received 200\n");
        assert_eq!(
            model_result.recv_timeout(Duration::from_secs(5)),
            Ok(Ok(200))
        );

        // A command queued for the node runs on it and its result comes back to the waiter.
        let id = store.enqueue("n1", "noded", "GET", "/v1/runs", Value::Null);
        let result = store
            .wait_result(&id, 10_000)
            .expect("the node answers the command");
        assert!(result.ok, "{result:?}");
        assert_eq!(result.status, 200);
        assert_eq!(result.body, json!({"runs": []}));

        // A streamed answer from the hub: the node's request reports it streamed, and its body arrives whole, in order.
        let target = identity
            .signed(&node_key, "GET", "/v1/nodes/n1/stream", &[])
            .expect("signs");
        let mut answer = node_tunnel::request_answer(
            &target,
            "GET",
            "/v1/nodes/n1/stream",
            None,
            Duration::from_secs(10),
        )
        .expect("the hub answers");
        assert!(answer.streamed, "the hub streamed this answer");
        assert_eq!(answer.status, 200);
        assert_eq!(answer.content_type, "text/event-stream");
        let mut text = String::new();
        answer
            .body
            .read_to_string(&mut text)
            .expect("reads to the end");
        assert_eq!(
            text,
            "data: 0\n\ndata: 1\n\ndata: 2\n\ndata: 3\n\ndata: 4\n\n"
        );

        // A node request, signed as the node signs them, answered by the hub's dispatch.
        let target = identity
            .signed(&node_key, "POST", "/v1/nodes/n1/heartbeat?x=1", b"{}")
            .expect("signs");
        let reply = node_tunnel::request(
            &target,
            "POST",
            "/v1/nodes/n1/heartbeat?x=1",
            Some(b"{}"),
            Duration::from_secs(10),
        )
        .expect("the hub answers over the tunnel");
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        let body: Value = serde_json::from_slice(&reply.body).expect("json");
        assert_eq!(body["node"], json!("n1"));
        assert_eq!(body["bytes"], json!(2));
        assert_eq!(body["query"], json!("x=1"));

        // A fork on the tunnel: its first connection is its first answer.
        assert_eq!(
            first_line(&store.open_port("n1", 1000, 7682).expect("opens")),
            b"fork 1\n"
        );

        // The switch to the mesh: the hub's path to the node changes, and the node's forks stay where they were.
        // Only the tunnel's identity is kept: holding its Arc would keep the connection open after the switch.
        let tunnel_before = Arc::as_ptr(&store.tunnel("n1").expect("the tunnel is up")) as usize;
        store
            .set_path("n1", "mesh", &mesh_addr)
            .expect("the path changes");
        {
            let (store, dispatch) = (Arc::clone(&store), Arc::clone(&dispatch));
            thread::spawn(move || keep_mesh(store, "n1".to_string(), dispatch));
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        while !store
            .tunnel("n1")
            .is_some_and(|mux| Arc::as_ptr(&mux) as usize != tunnel_before)
        {
            assert!(
                Instant::now() < deadline,
                "the mesh never took the node: {:?}",
                store.link("n1")
            );
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(store.get("n1").expect("the node").path, "mesh");

        // The fork kept its state: its second connection is served on the mesh by the same process.
        assert_eq!(
            first_line(
                &store
                    .open_port("n1", 1000, 7682)
                    .expect("opens on the mesh")
            ),
            b"fork 2\n"
        );
        // The terminal answers on the mesh as it did on the tunnel.
        let terminal_stream = store
            .open_port("n1", 1000, 7681)
            .expect("a terminal stream opens on the mesh");
        terminal_stream.send(b"mesh\n").expect("sends");
        let mut echoed = Vec::new();
        while echoed.len() < 5 {
            match terminal_stream.recv(Duration::from_secs(5)) {
                Some(apz_http::mux::Event::Data(bytes)) => echoed.extend_from_slice(&bytes),
                other => panic!("the terminal did not echo on the mesh: {other:?}"),
            }
        }
        assert_eq!(echoed, b"mesh\n");
        terminal_stream.finish().expect("finishes");

        // The node's own requests travel the mesh too, and the link reports its latency.
        let target = identity
            .signed(&node_key, "POST", "/v1/nodes/n1/heartbeat", b"{}")
            .expect("signs");
        let reply = node_tunnel::request(
            &target,
            "POST",
            "/v1/nodes/n1/heartbeat",
            Some(b"{}"),
            Duration::from_secs(10),
        )
        .expect("the hub answers over the mesh");
        assert_eq!(reply.status, 200);
        let deadline = Instant::now() + Duration::from_secs(20);
        while store.link("n1").latency_ms.is_none() {
            assert!(
                Instant::now() < deadline,
                "no latency: {:?}",
                store.link("n1")
            );
            thread::sleep(Duration::from_millis(20));
        }
        assert!(store.link("n1").connected);

        drop(hub);
    }
}
