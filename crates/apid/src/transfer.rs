//! Node-to-node transfers, the hub's side (plan 17.5). A move's files and a base's files go from the node that holds
//! them to the node that needs them. The hub only coordinates: it signs a grant for the pair and the manifest (the
//! grant carries the sender's certificate pin), and it gives the receiver its routes to the sender: a direct dial, then
//! the hub's broker with a ticket. The receiver fetches the chunks it lacks over TLS, pinned to the sender.
//!
//! The brokered path (`apz_transfer::broker`): when the receiver cannot dial the sender, it dials the hub's broker, and
//! the hub pipes the bytes into a stream over the sender's tunnel. The hub cannot read them (the TLS session is the
//! receiver's and the sender's), and it counts them: per transfer (`brokered_bytes` in the record) and in total
//! (`brokered` in the meter). The broker runs when `APZ_TRANSFER_BROKER` names its address.
//!
//! The hub's byte counts ([`meter`]): `control` is the requests and answers that coordinate transfers, `payload` is chunk
//! data it relayed through noded's file routes, `brokered` is the bytes it relayed for a receiver that could not dial.
//!
//! Routes (owner):
//!   POST /v1/transfers  {scope, from, to, files: [{name, path}]} → {manifest, from, to, via, brokered_bytes, fetched, hub_bytes}

use std::collections::BTreeSet;
use std::io;
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_transfer::Grant;
use apz_transfer::broker::{self, Tickets};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::fleet;
use crate::state::App;

static CONTROL: AtomicU64 = AtomicU64::new(0);
static PAYLOAD: AtomicU64 = AtomicU64::new(0);

/// What the hub has carried for transfers, in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Meter {
    /// The requests and answers that coordinate transfers: node names, grants, manifests.
    pub control: u64,
    /// Chunk data the hub relayed through noded's file routes. A transfer the hub only coordinates carries none.
    pub payload: u64,
    /// The bytes the hub relayed for receivers that could not dial their sender (the broker's), TLS and HTTP included.
    pub brokered: u64,
}

/// The hub's byte counts since it started.
pub fn meter() -> Meter {
    Meter {
        control: CONTROL.load(Ordering::Relaxed),
        payload: PAYLOAD.load(Ordering::Relaxed),
        brokered: tickets().total(),
    }
}

/// Counts chunk data the hub relayed through a file route (a call that moves file bytes through it).
pub fn note_payload(bytes: usize) {
    PAYLOAD.fetch_add(bytes as u64, Ordering::Relaxed);
}

/// The hub's tickets for the broker, shared with the broker's listener.
fn tickets() -> &'static Arc<Tickets> {
    static TICKETS: OnceLock<Arc<Tickets>> = OnceLock::new();
    TICKETS.get_or_init(|| Arc::new(Tickets::new(Arc::new(AtomicU64::new(0)))))
}

/// The hub's calls to nodes, as `(node, method, path, body) -> reply`.
pub type Caller<'a> = dyn FnMut(&str, &str, &str, &Value) -> Result<Value, String> + 'a;

/// One call for a transfer: the body and the reply are counted as control.
pub fn exchange(
    send: &mut Caller,
    node: &str,
    method: &str,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    CONTROL.fetch_add(body.to_string().len() as u64, Ordering::Relaxed);
    let reply = send(node, method, path, body)?;
    CONTROL.fetch_add(reply.to_string().len() as u64, Ordering::Relaxed);
    Ok(reply)
}

/// The hub's grant key (`<state_dir>/transfer_key`, made on first use).
pub fn key(app: &App) -> Result<SigningKey, String> {
    crate::shares::relay::load_key(&app.config.state_dir.join("transfer_key"))
        .map_err(|err| format!("the transfer key: {err}"))
}

/// The body of a call to `node`: `extra` plus the hub's name for the node and its grant key, which the node takes for
/// the call.
pub fn hub_body(key: &SigningKey, node: &str, mut extra: Value) -> Value {
    extra["hub"] = json!({
        "node": node,
        "hub_key": apz_transfer::key_to_text(&key.verifying_key()),
    });
    extra
}

/// A sender as the hub knows it: its node id, the address its transfers are dialed at, and its certificate pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sender {
    pub node: String,
    pub addr: String,
    pub pin: String,
}

/// Where node `node` takes transfers from: its address as the hub has it, its transfer port, and its certificate pin
/// (node facts, from its heartbeat). The hub's own machine, or one with no address, is 127.0.0.1.
pub fn sender_of(app: &App, node: &str) -> Result<Sender, String> {
    let record = app
        .nodes
        .get(node)
        .ok_or_else(|| format!("no node {node}"))?;
    let port = record
        .machine
        .transfer_port
        .ok_or_else(|| format!("node {node} serves no transfers"))?;
    let pin = record
        .machine
        .transfer_pin
        .clone()
        .ok_or_else(|| format!("node {node} reports no transfer certificate"))?;
    let host = match record.address.as_str() {
        "" | "loopback" => "127.0.0.1".to_string(),
        ip if ip.contains(':') => format!("[{ip}]"),
        ip => ip.to_string(),
    };
    Ok(Sender {
        node: node.to_string(),
        addr: format!("{host}:{port}"),
        pin,
    })
}

/// The broker the hub offers the brokered path on: `APZ_TRANSFER_BROKER`, as `host:port`. Unset, there is no brokered path.
pub fn broker_addr() -> Option<String> {
    std::env::var("APZ_TRANSFER_BROKER")
        .ok()
        .filter(|addr| !addr.is_empty())
}

/// Starts the broker on its address: a receiver that cannot dial a sender says a ticket here, and the hub relays the
/// bytes into the sender's tunnel (a stream of kind `transfer`, which the sender's node pipes to its noded).
pub fn start_broker(app: Arc<App>) {
    let Some(addr) = broker_addr() else { return };
    let listener = match TcpListener::bind(&addr) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("apid: the transfer broker is not started on {addr}: {err}");
            return;
        }
    };
    let open: broker::Open = Arc::new(move |node: &str| {
        let mux = app
            .nodes
            .tunnel(node)
            .ok_or_else(|| io::Error::other(format!("node {node} has no tunnel")))?;
        mux.open(br#"{"kind":"transfer"}"#)
    });
    broker::serve(listener, Arc::clone(tickets()), open);
}

/// Where a transfer's files are on the sender.
pub enum Source<'a> {
    /// Files on the sender's disk, by name and path.
    Files(&'a Value),
    /// A base's checkpoint on the sender: run `run`'s VM 0, checkpoint `name`.
    Checkpoint { run: &'a str, name: &'a str },
}

/// The routes a receiver tries to reach `from`: its direct address, then the broker with a ticket for `from`, when the
/// broker is offered. Returns the routes and the ticket.
pub(crate) fn routes_for(
    from: &Sender,
    broker: Option<&str>,
    expires_ms: u64,
) -> (Value, Option<String>) {
    let mut routes = vec![json!({"direct": from.addr})];
    let ticket = broker.map(|broker| {
        let ticket = tickets().issue(&from.node, expires_ms);
        routes.push(json!({"broker": broker, "ticket": ticket}));
        ticket
    });
    (Value::Array(routes), ticket)
}

/// One transfer: `from` offers the source's files as blobs, the hub signs a grant for `to` (pinned to `from`) and gives
/// its routes, and `to` fetches the chunks it lacks. Returns the manifest, the path taken (`via`) and, when brokered, the
/// bytes the hub relayed for it.
pub fn distribute_with(
    key: &SigningKey,
    scope: &str,
    from: &Sender,
    to: &str,
    source: Source,
    broker: Option<&str>,
    send: &mut Caller,
) -> Result<Value, String> {
    let published = match source {
        Source::Files(files) => exchange(
            send,
            &from.node,
            "POST",
            "/v1/transfers/manifests",
            &hub_body(key, &from.node, json!({"files": files})),
        )?,
        Source::Checkpoint { run, name } => exchange(
            send,
            &from.node,
            "POST",
            &format!("/v1/runs/{run}/vms/0/checkpoints/{name}/manifest"),
            &hub_body(key, &from.node, json!({})),
        )?,
    };
    let id = text(&published, "id")?;
    let expires = apz_transfer::now_ms() + apz_transfer::grant::TTL_MS;
    let grant = Grant::issue(key, scope, &id, &from.node, to, &from.pin, expires);
    let (routes, ticket) = routes_for(from, broker, expires);
    let fetched = exchange(
        send,
        to,
        "POST",
        "/v1/transfers/fetch",
        &hub_body(
            key,
            to,
            json!({
                "routes": routes,
                "grant": grant.encode(),
                "manifest": published["manifest"],
            }),
        ),
    )?;
    let via = fetched["via"].as_str().unwrap_or("direct").to_string();
    let brokered_bytes = ticket
        .filter(|_| via == "brokered")
        .map_or(0, |ticket| tickets().bytes(&ticket));
    Ok(json!({
        "manifest": id,
        "from": from.node,
        "to": to,
        "via": via,
        "brokered_bytes": brokered_bytes,
        "fetched": fetched,
    }))
}

/// `POST /v1/transfers`: [`distribute_with`] over the plaza's nodes.
pub fn distribute(
    app: &App,
    scope: &str,
    from: &str,
    to: &str,
    files: &Value,
) -> Result<Value, String> {
    let key = key(app)?;
    let from = sender_of(app, from)?;
    let broker = broker_addr();
    let mut send = |node: &str, method: &str, path: &str, body: &Value| {
        answered(
            &format!("{node} {method} {path}"),
            fleet::noded(app, node, method, path, Some(body)),
        )
    };
    distribute_with(
        &key,
        scope,
        &from,
        to,
        Source::Files(files),
        broker.as_deref(),
        &mut send,
    )
}

/// The nodes that receive a base built on `built`: the other nodes where a live session of `project` runs (its forks),
/// in `pool` (`any` for every pool).
pub fn fork_nodes(app: &App, project: &str, pool: &str, built: &str) -> Vec<String> {
    let sessions = app.sessions.list(&crate::sessions::Filter {
        project: Some(project.to_string()),
        task: None,
        state: None,
        live: true,
    });
    let in_pool = |node: &str| {
        pool == "any"
            || crate::placement::pool_of(app, node)
                .is_some_and(|p| format!("{}-{}", p.arch, p.hypervisor) == pool)
    };
    receivers(
        built,
        sessions
            .into_iter()
            .filter_map(|session| session.node)
            .filter(|node| in_pool(node)),
    )
}

/// The receivers among `nodes`: distinct, sorted, and never the node the base was built on.
pub fn receivers(built: &str, nodes: impl IntoIterator<Item = String>) -> Vec<String> {
    nodes
        .into_iter()
        .filter(|node| node != built)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// A base built on `built` (its checkpoint `name` of run `run`) reaches each receiver, one transfer each, by the grant
/// path. One answer per receiver; a failed transfer is its own answer, and the others go on.
pub fn distribute_built_with(
    key: &SigningKey,
    project: &str,
    built: &Sender,
    (run, name): (&str, &str),
    receivers: &[String],
    broker: Option<&str>,
    send: &mut Caller,
) -> Vec<Value> {
    let scope = format!("base:{project}:build");
    let mut answers = Vec::with_capacity(receivers.len());
    for to in receivers {
        let source = Source::Checkpoint { run, name };
        answers.push(
            match distribute_with(key, &scope, built, to, source, broker, send) {
                Ok(done) => done,
                Err(why) => json!({"to": to, "error": why}),
            },
        );
    }
    answers
}

/// A base's distribution on the plaza: [`distribute_built_with`] over the nodes that hold the project's forks.
pub fn distribute_built(
    app: &App,
    project: &str,
    pool: &str,
    built: &str,
    run: &str,
    name: &str,
) -> Result<Vec<Value>, String> {
    let key = key(app)?;
    let from = sender_of(app, built)?;
    let receivers = fork_nodes(app, project, pool, built);
    let broker = broker_addr();
    let mut send = |node: &str, method: &str, path: &str, body: &Value| {
        answered(
            &format!("{node} {method} {path}"),
            fleet::noded(app, node, method, path, Some(body)),
        )
    };
    Ok(distribute_built_with(
        &key,
        project,
        &from,
        (run, name),
        &receivers,
        broker.as_deref(),
        &mut send,
    ))
}

/// A node's answer as its body when it is 2xx; otherwise the failure as text.
fn answered(what: &str, result: Result<(u16, Value), String>) -> Result<Value, String> {
    match result {
        Ok((code, body)) if (200..300).contains(&code) => Ok(body),
        Ok((code, body)) => Err(format!("{what} answered {code}: {body}")),
        Err(err) => Err(format!("{what}: {err}")),
    }
}

fn text(body: &Value, key: &str) -> Result<String, String> {
    body[key]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("the node's answer has no {key}"))
}

/// Routes: distribute files between two nodes.
pub fn routes(router: Router<App>) -> Router<App> {
    router.post("/v1/transfers", distribute_route).doc(
        RouteDoc::new("Distribute files from one node to another: the receiver fetches the chunks it lacks from the sender, directly or through the hub's broker")
            .request::<TransferBody>()
            .errors(&[400, 502]),
    )
}

#[derive(Deserialize, schemars::JsonSchema)]
struct TransferBody {
    /// What the grant is for, for the record (`base:<project>:<kind>`).
    scope: String,
    from: String,
    to: String,
    /// The files, by name and the path on `from`.
    files: Vec<TransferFile>,
}

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
struct TransferFile {
    name: String,
    path: String,
}

fn distribute_route(app: &App, req: &Request, _p: &Params<'_>) -> Response {
    let body: TransferBody = match req.json() {
        Ok(body) => body,
        Err(err) => return Response::error(400, &format!("invalid transfer: {err}")),
    };
    if body.files.is_empty() {
        return Response::error(400, "a transfer needs at least one file");
    }
    if body.from == body.to {
        return Response::error(400, "a transfer needs two nodes");
    }
    let files = serde_json::to_value(&body.files).unwrap_or(Value::Null);
    match distribute(app, &body.scope, &body.from, &body.to, &files) {
        Ok(mut done) => {
            // What the hub itself carried so far: control only, when the transfer went node to node.
            done["hub_bytes"] = json!(meter());
            Response::json(200, &done)
        }
        Err(err) => Response::error(502, &err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use apz_blobs::Blobs;
    use apz_transfer::{Manifest, Node, Route};

    /// Three nodes, each a blob store served over TLS on loopback: "a" builds and holds the files, "b" and "c" receive.
    struct Fleet {
        hub: SigningKey,
        nodes: Vec<(String, Arc<Node>, apz_http::Handle, tempfile::TempDir)>,
    }

    fn fleet() -> Fleet {
        let hub = SigningKey::from_bytes(&[5; 32]);
        let nodes = ["a", "b", "c"]
            .into_iter()
            .map(|id| {
                let dir = tempfile::tempdir().unwrap();
                let blobs = Arc::new(Blobs::open(dir.path()).unwrap());
                let identity = apz_trust::tls::self_signed(&["localhost".to_string()]).unwrap();
                let node = Node::new(id, blobs, &identity).unwrap();
                node.trust(hub.verifying_key());
                let server = node.spawn("127.0.0.1:0".parse().unwrap()).unwrap();
                (id.to_string(), node, server, dir)
            })
            .collect();
        Fleet { hub, nodes }
    }

    impl Fleet {
        fn node(&self, id: &str) -> &(String, Arc<Node>, apz_http::Handle, tempfile::TempDir) {
            self.nodes.iter().find(|(n, ..)| n == id).unwrap()
        }

        fn sender(&self, id: &str) -> Sender {
            let (_, node, server, _) = self.node(id);
            Sender {
                node: id.to_string(),
                addr: format!("127.0.0.1:{}", server.port()),
                pin: apz_transfer::pin_text(&node.pin()),
            }
        }
    }

    /// A stand-in for the nodes' noded: it adopts the hub's name and grant key from each call, as noded does, and answers
    /// the routes a transfer uses. `checkpoints` maps a checkpoint's manifest path to its directory on the sender.
    fn stand_in<'a>(
        f: &'a Fleet,
        checkpoints: Vec<(String, std::path::PathBuf)>,
    ) -> impl FnMut(&str, &str, &str, &Value) -> Result<Value, String> + 'a {
        move |node, _method, path, body| {
            let key = apz_transfer::key_from_text(body["hub"]["hub_key"].as_str().unwrap())?;
            let (_, me, _, _) = f.node(node);
            me.set_id(node);
            me.trust(key);
            if let Some(found) = checkpoints.iter().find(|(route, _)| route == path) {
                let mut files = Vec::new();
                let mut names: Vec<_> = std::fs::read_dir(&found.1)
                    .unwrap()
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect();
                names.sort();
                for name in names {
                    let handle = std::fs::File::open(found.1.join(&name)).unwrap();
                    files.push(Manifest::store(me.blobs(), &name, handle)?);
                }
                let manifest = Manifest { files };
                let id = me.publish(&manifest)?;
                return Ok(json!({"id": id, "manifest": manifest}));
            }
            match path {
                "/v1/transfers/manifests" => {
                    let mut files = Vec::new();
                    for file in body["files"].as_array().unwrap() {
                        let handle = std::fs::File::open(file["path"].as_str().unwrap())
                            .map_err(|e| e.to_string())?;
                        files.push(Manifest::store(
                            me.blobs(),
                            file["name"].as_str().unwrap(),
                            handle,
                        )?);
                    }
                    let manifest = Manifest { files };
                    let id = me.publish(&manifest)?;
                    Ok(json!({"id": id, "manifest": manifest}))
                }
                "/v1/transfers/fetch" => {
                    let grant = Grant::decode(body["grant"].as_str().unwrap())?;
                    let manifest: Manifest =
                        serde_json::from_value(body["manifest"].clone()).unwrap();
                    let routes: Vec<Route> = body["routes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|r| match r["broker"].as_str() {
                            Some(broker) => Route::Brokered {
                                broker: broker.to_string(),
                                ticket: r["ticket"].as_str().unwrap_or_default().to_string(),
                            },
                            None => {
                                Route::Direct(r["direct"].as_str().unwrap_or_default().to_string())
                            }
                        })
                        .collect();
                    let report = apz_transfer::fetch(me.blobs(), node, &routes, &grant, &manifest)?;
                    Ok(json!({
                        "fetched": report.fetched,
                        "reused": report.reused,
                        "bytes": report.bytes,
                        "via": report.via.as_str(),
                    }))
                }
                other => Err(format!("no route {other}")),
            }
        }
    }

    fn noise(path: &std::path::Path, len: usize, seed: u64) {
        let mut x = seed | 1;
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_base_reaches_a_second_node_and_the_hub_carries_no_chunk_bytes() {
        let f = fleet();
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.bin");
        noise(&base, 24 << 20, 3);
        let files = json!([{"name": "base.bin", "path": base}]);
        let before = meter();
        let mut send = stand_in(&f, Vec::new());
        let done = distribute_with(
            &f.hub,
            "base:p:build",
            &f.sender("a"),
            "b",
            Source::Files(&files),
            None,
            &mut send,
        )
        .unwrap();
        let after = meter();
        let moved = done["fetched"]["bytes"].as_u64().unwrap();
        assert_eq!(moved, 24 << 20, "the whole base came: {done}");
        assert_eq!(done["via"], "direct");
        assert_eq!(f.node("a").1.served_bytes(), moved, "a sent them to b");
        assert_eq!(
            after.payload, before.payload,
            "the hub relayed no chunk bytes"
        );
        assert!(
            after.control - before.control < 64 * 1024,
            "control only: {after:?}"
        );
        let bytes = std::fs::read(&base).unwrap();
        assert_eq!(
            f.node("b")
                .1
                .blobs()
                .get(&apz_blobs::Hash::of(&bytes))
                .unwrap(),
            bytes
        );
    }

    #[test]
    fn a_built_base_reaches_the_nodes_that_hold_its_forks_by_the_grant_path() {
        let f = fleet();
        let dir = tempfile::tempdir().unwrap();
        let checkpoint = dir.path().join("cid-7").join("base-build-1");
        std::fs::create_dir_all(&checkpoint).unwrap();
        noise(&checkpoint.join("memory.img"), 9 << 20, 8);
        std::fs::write(checkpoint.join("vmstate.json"), br#"{"meta":{"format":1}}"#).unwrap();
        let state = br#"{"meta":{"format":1}}"#;
        let size = (9usize << 20) + state.len();
        let receivers = receivers(
            "a",
            [
                "c".to_string(),
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
            ],
        );
        assert_eq!(
            receivers,
            ["b", "c"],
            "the build node and repeats are left out"
        );

        let route = "/v1/runs/run-9/vms/0/checkpoints/base-build-1/manifest".to_string();
        let mut send = stand_in(&f, vec![(route, checkpoint)]);
        let answers = distribute_built_with(
            &f.hub,
            "p",
            &f.sender("a"),
            ("run-9", "base-build-1"),
            &receivers,
            None,
            &mut send,
        );
        assert_eq!(answers.len(), 2);
        for answer in &answers {
            assert_eq!(answer["via"], "direct", "{answer}");
            assert_eq!(
                answer["fetched"]["bytes"].as_u64().unwrap(),
                size as u64,
                "{answer}"
            );
        }
        // Both forks' nodes hold the base; the build node sent it twice and received nothing.
        assert_eq!(f.node("a").1.served_bytes(), 2 * size as u64);
        assert!(f.node("b").1.blobs().stats().1 >= size as u64);
        assert!(f.node("c").1.blobs().stats().1 >= size as u64);
    }

    /// A wire between the hub's mux and a node's: in-memory frames both ways.
    struct Wire {
        tx: std::sync::mpsc::Sender<Vec<u8>>,
        rx: std::sync::mpsc::Receiver<Vec<u8>>,
    }

    impl apz_http::mux::Carrier for Wire {
        fn send(&mut self, frame: &[u8]) -> io::Result<()> {
            self.tx
                .send(frame.to_vec())
                .map_err(|_| io::ErrorKind::BrokenPipe.into())
        }

        fn recv(&mut self, wait: std::time::Duration) -> io::Result<Option<Vec<u8>>> {
            match self.rx.recv_timeout(wait) {
                Ok(frame) => Ok(Some(frame)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
                Err(_) => Err(io::ErrorKind::BrokenPipe.into()),
            }
        }
    }

    /// The hub's tunnel to a node: the node answers each stream of kind `transfer` by piping it to the sender's transfer
    /// address, as apz-link does for its noded.
    fn tunnel_to(sender_addr: String) -> Arc<apz_http::mux::Mux> {
        use apz_http::mux::{Mux, Side};
        let (to_node, to_hub) = {
            let (a_tx, b_rx) = std::sync::mpsc::channel();
            let (b_tx, a_rx) = std::sync::mpsc::channel();
            (Wire { tx: a_tx, rx: a_rx }, Wire { tx: b_tx, rx: b_rx })
        };
        let hub = Mux::new(to_node, Side::Dialer);
        let node = Mux::new(to_hub, Side::Acceptor);
        std::thread::spawn(move || {
            while let Ok(accepted) = node.accept(std::time::Duration::from_millis(200)) {
                let Some(stream) = accepted else { continue };
                let header: Value = serde_json::from_slice(stream.header()).unwrap_or_default();
                if header["kind"] != "transfer" {
                    stream.reset("unknown stream kind");
                    continue;
                }
                let to = sender_addr.clone();
                std::thread::spawn(move || {
                    if let Ok(conn) = std::net::TcpStream::connect(&to) {
                        broker::pipe(stream, conn, Arc::new(|_| {}));
                    }
                });
            }
        });
        hub
    }

    #[test]
    fn a_blocked_direct_dial_is_brokered_and_the_record_names_the_bytes() {
        let f = fleet();
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.bin");
        noise(&base, 6 << 20, 17);
        let files = json!([{"name": "base.bin", "path": base}]);
        // Node a's direct address refuses: a port nothing listens on.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut sender = f.sender("a");
        sender.addr = closed.local_addr().unwrap().to_string();
        drop(closed);
        // The hub's broker, and node a's tunnel through it.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let broker_addr = listener.local_addr().unwrap().to_string();
        let hub_mux = tunnel_to(f.sender("a").addr);
        let open: broker::Open =
            Arc::new(move |_node: &str| hub_mux.open(br#"{"kind":"transfer"}"#));
        broker::serve(listener, Arc::clone(tickets()), open);

        let before = meter();
        let mut send = stand_in(&f, Vec::new());
        let done = distribute_with(
            &f.hub,
            "base:p:build",
            &sender,
            "b",
            Source::Files(&files),
            Some(&broker_addr),
            &mut send,
        )
        .unwrap();
        let after = meter();
        assert_eq!(done["via"], "brokered", "{done}");
        let moved = done["fetched"]["bytes"].as_u64().unwrap();
        assert_eq!(moved, 6 << 20, "the whole base came: {done}");
        // The record names the bytes the hub relayed for this transfer, TLS and HTTP included, and the meter counts them.
        let brokered = done["brokered_bytes"].as_u64().unwrap();
        assert!(brokered >= moved, "{done}");
        assert_eq!(after.brokered - before.brokered, brokered);
        assert_eq!(after.payload, before.payload, "the hub read no chunk bytes");
        assert_eq!(f.node("a").1.served_bytes(), moved, "a sent them to b");
        let bytes = std::fs::read(&base).unwrap();
        assert_eq!(
            f.node("b")
                .1
                .blobs()
                .get(&apz_blobs::Hash::of(&bytes))
                .unwrap(),
            bytes
        );
    }
}
