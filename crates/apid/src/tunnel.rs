//! The hub's end of a node's tunnel (PLAN 17.4): one WebSocket per joined node, `/v1/nodes/{id}/tunnel`, carrying every
//! stream between the hub and the node (apz_http::mux). The node dials it and keeps it up; the hub:
//! - sends each command queued for the node as a `command` stream (`nodes::Store::enqueue`) and stores its result
//!   (`put_result`), so the callers that wait for results are unchanged;
//! - opens `port` streams to a fork's port through the node (`nodes::Store::open_port`): a terminal's bytes, a model
//!   call's deltas;
//! - answers the node's `request` streams (heartbeats, results, metrics, the agents' calls) through the same gate and
//!   routes as an HTTP request, with the node's signature headers as they were sent.
//!
//! The handshake is the caller's to authenticate (the route checks the node's signature before [`open`]).

use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use apz_http::mux::{Mux, Side, Stream};
use apz_http::ws::{self, Limits, Origins, Socket};
use apz_http::{Body, Request, Response};
use serde_json::{Value, json};

use crate::nodes::{Command, CommandResult, Store};

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

/// The tunnel's handshake, after the route has authenticated the node: the WebSocket upgrade. The socket then runs the
/// session on its own thread until the node disconnects.
pub fn open(req: &Request, node: String, store: Arc<Store>, dispatch: Dispatch) -> Response {
    let peer = req.peer;
    ws::accept(req, Origins::None, None, Limits::default(), move |socket| {
        serve(socket, node, peer, store, dispatch)
    })
}

/// The session: accepts the node's streams on a thread of their own, and sends the node its queued commands until the
/// tunnel ends. Then the node's tunnel is forgotten, so the queue waits for its next one.
fn serve(
    socket: Socket,
    node: String,
    peer: Option<IpAddr>,
    store: Arc<Store>,
    dispatch: Dispatch,
) {
    let mux = Mux::new(socket, Side::Acceptor);
    store.set_tunnel(&node, Arc::clone(&mux));
    {
        let (mux, dispatch) = (Arc::clone(&mux), Arc::clone(&dispatch));
        thread::spawn(move || accept_streams(&mux, peer, &dispatch));
    }
    while mux.alive() {
        for command in store.take(&node, QUEUE_WAIT_MS) {
            send_command(&mux, &store, &node, command);
        }
    }
    store.clear_tunnel(&node, &mux);
}

/// Answers each stream the node opens, until the tunnel ends.
fn accept_streams(mux: &Mux, peer: Option<IpAddr>, dispatch: &Dispatch) {
    loop {
        match mux.accept(ACCEPT_WAIT) {
            Ok(Some(stream)) => {
                let dispatch = Arc::clone(dispatch);
                thread::spawn(move || answer(&stream, peer, &dispatch));
            }
            Ok(None) => {
                if !mux.alive() {
                    return;
                }
            }
            Err(_) => return,
        }
    }
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

    /// The end-to-end test: a hub (the tunnel's route behind TLS, pinned by the node) and one node link over loopback.
    /// One test, because the node link keeps its tunnel in one process-wide slot.
    #[test]
    fn the_tunnel_carries_a_terminal_a_model_call_commands_and_requests_over_loopback_tls() {
        // The hub's certificate and the node's identity, pinned as a join would pin them.
        let pair = apz_trust::tls::self_signed(&["localhost".to_string()]).expect("a certificate");
        let pin = apz_trust::tls::fingerprint(&pair.cert_pem).expect("a pin");
        let tls = apz_trust::tls::server_config(&pair).expect("a server config");
        let node_key = NodeKey::generate();
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
        let router: Router<()> = Router::new().get("/v1/nodes/n1/tunnel", move |_, req, _| {
            match route_store.authenticate(req, apz_plaza::now_ms()) {
                Ok(node) => open(
                    req,
                    node.id,
                    Arc::clone(&route_store),
                    Arc::clone(&route_dispatch),
                ),
                Err(reason) => Response::error(401, &reason),
            }
        });
        let hub = Server::bind(0)
            .expect("an ephemeral port")
            .tls(tls)
            .spawn(move |request| router.handle(&(), request))
            .expect("serves");

        // A fake noded for the command, and a fork's terminal and model ports, reached through the node's relay and vsockd's inbound listener.
        let noded_router: Router<()> = Router::new().get("/v1/runs", |_, _, _| {
            Response::json(200, &json!({"runs": []}))
        });
        let noded = Server::bind(0)
            .expect("an ephemeral port")
            .spawn(move |request| noded_router.handle(&(), request))
            .expect("serves");
        let terminal = echo_fork();
        let (model, model_result) = model_fork();
        // vsockd's inbound listener, with the table of what runs serve and where their guests answer on loopback.
        let mut table = apz_link::inbound::Table::default();
        table.served.lock().unwrap().insert((1000, 7681));
        table.served.lock().unwrap().insert((1000, 7690));
        table.guests.insert((1000, 7681), terminal);
        table.guests.insert((1000, 7690), model);
        let inbound_listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let inbound_port = inbound_listener.local_addr().expect("address").port();
        let table = Arc::new(table);
        thread::spawn(move || apz_link::inbound::serve(inbound_listener, table.clone(), table));

        let identity = Identity {
            hub_url: format!("https://localhost:{}", hub.port()),
            pin: apz_trust::b64(&pin),
            node_id: "n1".into(),
            cid_base: REMOTE_CID_BASE,
            cid_count: CIDS_PER_NODE,
            hub_name: "test".into(),
        };
        let local = Local {
            noded: noded.port(),
            vsockd: 0,
            forward: 0,
            token: apz_http::auth::InternalToken::none(),
            inbound: inbound_port,
        };

        // The node dials the hub, and serves the hub's streams.
        let mux = node_tunnel::connect(&identity, &node_key).expect("the tunnel opens over TLS");
        let serving = thread::spawn(move || node_tunnel::serve(mux, local));
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

        drop(hub);
        drop(serving);
    }
}
