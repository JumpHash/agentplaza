//! The previews' tests: the proxy's framing and the link tokens, and the end-to-end path (6.4's done-when): the CI
//! recipe's preview step starts a lane's app in a fork, the fork's port is registered with noded, and a link's token
//! reaches the app through the hub's route and the node's tunnel, and nothing else. A refused, expired or revoked link is
//! refused; the opening is recorded; a request after the preview sleeps wakes it; the lane's end closes it.
//!
//! The fork is a stand-in: noded's calls answer from a table, its VM's exec items succeed at once (the base runner's
//! `answer`), and the app is a small HTTP server on loopback that the node's inbound stream dials.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::Duration;

use super::route;
use apz_http::auth::InternalToken;
use apz_http::{Body, Request, Response, Router, Server};
use apz_link::identity::Identity;
use apz_link::inbound::{Duplex, GuestDial, PortCheck};
use apz_link::run::Local;
use apz_link::tunnel as node_tunnel;
use apz_proto::wire::HostMessage;
use apz_trust::node::NodeKey;
use serde_json::{Value, json};

use super::life::{self, Ctx, Frames, Lane, NoFrames};
use super::route::answer;
use super::{ASLEEP, AWAKE, CLOSED, Store, TOKEN_PARAM, USE, VIEW};
use crate::base_vm::{Answer, Host, arrive};
use crate::nodes::{self, CIDS_PER_NODE, Node, REMOTE_CID_BASE};
use crate::project_stack;
use crate::tunnel::{self, Dispatch};

const NODE: &str = "n1";
const RUN: &str = "r1";
/// The guest CID the stand-in VM answers from (no other test of the crate uses it).
const CID: u64 = 4242;
const PORT: u16 = 8080;
const STACK: &str = "[[pools]]\narch = \"aarch64\"\nhypervisor = \"kvm\"\n\n[[targets]]\nname = \"web\"\nkind = \"preview\"\nserve = \"python3 -m http.server 8080\"\nport = 8080\n";

fn now() -> u64 {
    apz_plaza::now_ms()
}

#[test]
fn a_preview_subdomain_names_its_lane_and_project_under_the_domain() {
    let domain = "preview.example.com";
    assert_eq!(
        route::subdomain_lane("t1--proj.preview.example.com", domain),
        Some(("proj".to_string(), "t1".to_string()))
    );
    assert_eq!(
        route::subdomain_lane("T1--proj.Preview.Example.com:8443", domain),
        Some(("proj".to_string(), "t1".to_string())),
        "the port and the case are ignored"
    );
    assert!(
        route::subdomain_lane("proj.preview.example.com", domain).is_none(),
        "no lane"
    );
    assert!(
        route::subdomain_lane("t1--proj.example.org", domain).is_none(),
        "another domain"
    );
    assert!(
        route::subdomain_lane("a.b--proj.preview.example.com", domain).is_none(),
        "a lane is one label"
    );
    assert!(
        route::subdomain_lane("t1--proj.preview.example.com", "").is_none(),
        "no preview domain"
    );
}

#[test]
fn a_request_on_a_preview_subdomain_moves_to_the_preview_path_and_others_stay() {
    let on = |host: &str| Request {
        method: "GET".into(),
        path: "/docs/a".into(),
        query: "x=1".into(),
        headers: vec![("host".into(), host.into())],
        body: Vec::new(),
        peer: None,
    };
    let moved =
        route::subdomain_request(&on("t1--proj.preview.example.com"), "preview.example.com")
            .expect("moved");
    assert_eq!(moved.path, "/preview/proj/t1/docs/a");
    assert_eq!(moved.query, "x=1");
    assert!(route::subdomain_request(&on("hub.example.com"), "preview.example.com").is_none());
}

#[test]
fn a_subdomain_is_gated_as_its_preview_path_so_an_apps_v1_is_not_the_hubs() {
    let on = |host: &str| Request {
        method: "GET".into(),
        path: "/v1/items".into(),
        query: String::new(),
        headers: vec![("host".into(), host.into())],
        body: Vec::new(),
        peer: None,
    };
    let domain = "preview.example.com";
    // The app's own `/v1/` on its subdomain is the preview's path: the gate passes it to the preview's own check.
    let app = on("t1--proj.preview.example.com");
    let routed = route::as_routed(&app, domain);
    assert_eq!(routed.path, "/preview/proj/t1/v1/items");
    assert!(crate::hub_auth::is_public(&routed.method, &routed.path));
    // The same path on the hub is the owner's route, and the hub's gate refuses it without the owner.
    assert!(!crate::hub_auth::is_public("GET", "/v1/items"));
    let hub = on("hub.example.com");
    let kept = route::as_routed(&hub, domain);
    assert!(matches!(kept, std::borrow::Cow::Borrowed(_)));
    assert_eq!(kept.path, "/v1/items");
}

#[test]
fn a_token_is_its_link_id_and_a_mac_only_the_store_makes() {
    let store = Store::for_tests();
    let token = store.mint("0123456789abcdef0123456789abcdef");
    assert_eq!(
        store.verified_id(&token).as_deref(),
        Some("0123456789abcdef0123456789abcdef")
    );
    let forged = format!("0123456789abcdef0123456789abcdef.{}", "0".repeat(64));
    assert!(
        store.verified_id(&forged).is_none(),
        "a wrong MAC is refused"
    );
    assert!(
        store.verified_id("short.abc").is_none(),
        "a malformed id is refused"
    );
    assert!(store.verified_id(&token.replace('.', "x")).is_none());
}

#[test]
fn the_token_leaves_the_query_and_the_rest_stays() {
    let token = "a".repeat(32);
    let (found, rest) = super::route::take_token(&format!("a=1&{TOKEN_PARAM}={token}&b=2"));
    assert_eq!(found.as_deref(), Some(token.as_str()));
    assert_eq!(rest, "a=1&b=2");
}

/// The stand-in for one fork and its node's noded: the run, the VM's sleep, the ports noded serves, and every call.
#[derive(Default)]
struct Fork {
    asleep: bool,
    ports: BTreeSet<(u64, u16)>,
    calls: Vec<String>,
}

#[derive(Default)]
struct NodedStandIn {
    fork: Mutex<Fork>,
    /// Whether the VM announces its port when it listens (the guest's registration, 6.4), or stays silent so the hub
    /// registers it (the fallback).
    announce: bool,
}

impl NodedStandIn {
    fn fork(&self) -> std::sync::MutexGuard<'_, Fork> {
        self.fork.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether noded would let an inbound stream reach `port` of guest `cid`: registered, and the VM awake.
    fn serves(&self, cid: u64, port: u16) -> bool {
        let fork = self.fork();
        !fork.asleep && fork.ports.contains(&(cid, port))
    }

    fn calls(&self) -> Vec<String> {
        self.fork().calls.clone()
    }
}

impl Host for NodedStandIn {
    fn noded(
        &self,
        _node: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value), String> {
        let mut fork = self.fork();
        fork.calls.push(format!("{method} {path}"));
        Ok(match (method, path) {
            ("POST", "/v1/runs") => {
                arrive(CID);
                (200, json!({"id": RUN, "vms": [{"guest_cid": CID}]}))
            }
            ("POST", "/v1/runs/r1/ports") => {
                let port = body.and_then(|b| b["port"].as_u64()).unwrap_or(0) as u16;
                let cid = body.and_then(|b| b["cid"].as_u64()).unwrap_or(0);
                fork.ports.insert((cid, port));
                (200, json!({"registered": true}))
            }
            ("DELETE", "/v1/runs/r1/ports/4242/8080") => {
                fork.ports.remove(&(CID, PORT));
                (200, json!({"removed": true}))
            }
            ("POST", "/v1/runs/r1/vms/0/sleep") => {
                fork.asleep = true;
                (200, json!({}))
            }
            ("POST", "/v1/runs/r1/vms/0/wake") => {
                fork.asleep = false;
                (200, json!({}))
            }
            ("POST", "/v1/runs/r1/vms/0/stop") => (200, json!({})),
            ("POST", "/v1/forks/4242/ports") => {
                let port = body.and_then(|b| b["port"].as_u64()).unwrap_or(0) as u16;
                fork.ports.insert((CID, port));
                (200, json!({"registered": true}))
            }
            ("GET", "/v1/forks/4242/ports/8080") => {
                if fork.ports.contains(&(CID, PORT)) {
                    (200, json!({"served": true}))
                } else {
                    (404, json!({"served": false}))
                }
            }
            _ => (404, json!({"error": format!("no route {path}")})),
        })
    }

    fn send(&self, cid: u64, message: &HostMessage) -> Result<(), String> {
        // The base runner's exec items: each succeeds at once, as a VM that ran them would answer. A background command
        // that names its port is announced when it listens: apid takes the announcement, registers the port with noded by
        // CID, and records it, which this stand-in does the same way.
        if let HostMessage::Exec { id, listen, .. } = message {
            if let (true, Some(port)) = (self.announce, listen) {
                let registered = self.noded(
                    NODE,
                    "POST",
                    &format!("/v1/forks/{CID}/ports"),
                    Some(&json!({"port": port})),
                );
                if let Ok((200, _)) = registered {
                    crate::base_vm::record_listening(CID, *port);
                }
            }
            crate::base_vm::answer(
                cid,
                *id,
                Answer {
                    ok: true,
                    exit_code: Some(0),
                    output: String::new(),
                },
            );
        }
        Ok(())
    }
}

/// The node link's inbound side (vsockd's check and dial): a port is reached when the stand-in serves it, and the dial
/// goes to the fork's app.
struct Inbound {
    fork: Arc<NodedStandIn>,
    app: SocketAddr,
}

impl PortCheck for Inbound {
    fn served(&self, cid: u64, port: u16) -> bool {
        self.fork.serves(cid, port)
    }
}

impl GuestDial for Inbound {
    fn dial(&self, _cid: u64, _port: u16) -> std::io::Result<Box<dyn Duplex>> {
        Ok(Box::new(TcpStream::connect(self.app)?))
    }
}

/// The app's gate for its stream: the test holds the sender, and the app waits on the receiver (taken from the slot by
/// the stream request) between its first event and its second. `seen` says whether the gate opened in time.
#[derive(Clone)]
struct Gate {
    slot: Arc<Mutex<Option<mpsc::Receiver<()>>>>,
    seen: Arc<std::sync::atomic::AtomicBool>,
}

/// The lane's app: a small HTTP server in the fork. `GET /` is its page, `POST /echo` answers with the body's size, and
/// `GET /stream` sends one server-sent event, waits for the gate, and sends the second.
fn fork_app(gate: Gate) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let addr = listener.local_addr().expect("address");
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { return };
            let gate = gate.clone();
            thread::spawn(move || serve_app(conn, gate));
        }
    });
    addr
}

fn serve_app(mut conn: TcpStream, gate: Gate) {
    let Ok(clone) = conn.try_clone() else { return };
    let mut lines = BufReader::new(clone);
    let mut first = String::new();
    if lines.read_line(&mut first).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = first.split_whitespace();
    let (method, target) = (
        parts.next().unwrap_or("").to_string(),
        parts.next().unwrap_or("/").to_string(),
    );
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if lines.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    let _ = lines.read_exact(&mut body);
    if (method.as_str(), target.as_str()) == ("GET", "/stream") {
        serve_stream(&mut conn, &gate);
        return;
    }
    let (status, text) = match (method.as_str(), target.as_str()) {
        ("GET", "/") => (200, "<h1>lane preview</h1>".to_string()),
        ("POST", "/echo") => (200, format!("got {} bytes", body.len())),
        _ => (404, format!("no {target}")),
    };
    let reply = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    let _ = conn.write_all(reply.as_bytes());
}

/// The stream: an event, then the gate, then the second event; the connection's end frames the body.
fn serve_stream(conn: &mut TcpStream, gate: &Gate) {
    let _ = conn.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
    );
    let _ = conn.write_all(b"data: 1\n\n");
    let _ = conn.flush();
    let receiver = gate
        .slot
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    let opened =
        receiver.is_some_and(|receiver| receiver.recv_timeout(Duration::from_secs(10)).is_ok());
    gate.seen.store(opened, std::sync::atomic::Ordering::SeqCst);
    let _ = conn.write_all(b"data: 2\n\n");
}

/// Opens a request to a plain server and returns the connection, unread (for a stream).
fn open_stream(addr: SocketAddr, path: &str, headers: &[(&str, &str)]) -> TcpStream {
    let mut stream = TcpStream::connect(addr).expect("connects");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let mut head = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).expect("writes");
    stream
}

/// Reads from `stream` until `needle` has arrived, and returns everything read so far.
fn read_until(stream: &mut TcpStream, needle: &str) -> String {
    let mut got = Vec::new();
    let mut buf = [0u8; 256];
    while !String::from_utf8_lossy(&got).contains(needle) {
        let n = stream.read(&mut buf).expect("the stream answers in time");
        assert!(n > 0, "the stream ended before {needle}");
        got.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&got).to_string()
}

/// One HTTP exchange with a plain server on `addr`: the status, the head (lower-cased) and the body.
fn exchange(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (u16, String, String) {
    let mut stream = TcpStream::connect(addr).expect("connects");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("timeout");
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).expect("writes");
    stream.write_all(body).expect("writes");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("reads to the end");
    let raw = String::from_utf8_lossy(&raw).to_string();
    let (head, text) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, head.to_ascii_lowercase(), text.to_string())
}

/// The hub's preview routes on a plain listener: the same handler the hub serves `/preview/…` with, owner false.
fn preview_router(
    store: Arc<Store>,
    nodes: Arc<nodes::Store>,
    standin: Arc<NodedStandIn>,
) -> Router<()> {
    let mut router: Router<()> = Router::new();
    for method in ["GET", "POST"] {
        let (store, nodes, standin) =
            (Arc::clone(&store), Arc::clone(&nodes), Arc::clone(&standin));
        let handler = move |_: &(), req: &Request, p: &apz_http::Params<'_>| {
            let host: &NodedStandIn = &standin;
            let ctx = Ctx {
                store: &store,
                nodes: &nodes,
                host,
                frames: &NoFrames,
            };
            answer(
                &ctx,
                req,
                false,
                p.get("project"),
                p.get("lane"),
                p.get("rest"),
                now(),
            )
        };
        router = router
            .route(method, "/preview/{project}/{lane}/", handler.clone())
            .route(method, "/preview/{project}/{lane}/{rest..}", handler);
    }
    router
}

/// A workstation's stream for the native test: two frames, the second after a pause.
struct ScriptFrames;

impl Frames for ScriptFrames {
    fn open(&self, station: &str) -> Result<super::life::StreamBody, String> {
        if station != "ws-1" {
            return Err(format!("no workstation {station}"));
        }
        Ok(Box::new(|out: &mut dyn Write| -> std::io::Result<()> {
            out.write_all(b"event: frame\ndata: ZjE=\n\n")?;
            out.flush()?;
            thread::sleep(Duration::from_millis(100));
            out.write_all(b"event: frame\ndata: ZjI=\n\n")
        }))
    }
}

/// The body of an answer, read whole (a stream is run to its end).
fn body_text(answer: Response) -> String {
    match answer.body {
        Body::Bytes(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Body::Stream(write) => {
            let mut out = Vec::new();
            write(&mut out).expect("the stream runs to its end");
            String::from_utf8_lossy(&out).into_owned()
        }
        Body::Upgrade(_) => panic!("a preview is not upgraded"),
    }
}

/// A native lane's preview: its workstation's stream reaches a link, and nothing else of the lane or of another lane.
#[test]
fn a_native_lane_shows_its_stations_stream_to_a_link_and_nothing_else() {
    let store = Store::for_tests();
    let nodes = nodes::Store::holding(Vec::new());
    let host = NodedStandIn::default();
    let frames = ScriptFrames;
    let ctx = Ctx {
        store: &store,
        nodes: &nodes,
        host: &host,
        frames: &frames,
    };
    let native = store
        .add_native_preview("p", "nat", "ws-1", now())
        .expect("the native preview");
    assert_eq!(native.kind, super::NATIVE);
    let web = store
        .add_preview("p", "web", NODE, RUN, CID, u32::from(PORT), now())
        .expect("a web preview");
    let watch = store
        .add_link(
            &native.id,
            VIEW,
            "watch",
            Some(now() + 3_600_000),
            "owner",
            now(),
        )
        .expect("the view link");
    let watch_cookie = format!("apz_preview={}", store.mint(&watch.id));
    let drive = store
        .add_link(
            &native.id,
            USE,
            "drive",
            Some(now() + 3_600_000),
            "owner",
            now(),
        )
        .expect("the use link");
    let drive_cookie = format!("apz_preview={}", store.mint(&drive.id));
    let web_link = store
        .add_link(&web.id, USE, "web", Some(now() + 3_600_000), "owner", now())
        .expect("the web link");
    let web_cookie = format!("apz_preview={}", store.mint(&web_link.id));
    let request = |method: &str, cookie: Option<&str>| {
        let mut headers = Vec::new();
        if let Some(cookie) = cookie {
            headers.push(("cookie".to_string(), cookie.to_string()));
        }
        Request {
            method: method.to_string(),
            path: "/".to_string(),
            query: String::new(),
            headers,
            body: Vec::new(),
            peer: None,
        }
    };

    // Without a link the stream is refused.
    let refused = answer(&ctx, &request("GET", None), false, "p", "nat", "", now());
    assert_eq!(refused.status, 401);

    // A view link reaches the stream: the events come as server-sent events.
    let watched = answer(
        &ctx,
        &request("GET", Some(&watch_cookie)),
        false,
        "p",
        "nat",
        "",
        now(),
    );
    assert_eq!(watched.status, 200);
    assert!(
        watched
            .headers
            .iter()
            .any(|(name, value)| name == "Content-Type" && value == "text/event-stream"),
        "{:?}",
        watched.headers
    );
    let text = body_text(watched);
    assert!(
        text.contains("data: ZjE=") && text.contains("data: ZjI="),
        "{text}"
    );

    // The stream is all of the lane: any other path of it is refused.
    let other = answer(
        &ctx,
        &request("GET", Some(&watch_cookie)),
        false,
        "p",
        "nat",
        "docs",
        now(),
    );
    assert_eq!(other.status, 404);

    // A view link reads only: a write is refused by the link's scope, and a use link has no controls yet.
    let write = answer(
        &ctx,
        &request("POST", Some(&watch_cookie)),
        false,
        "p",
        "nat",
        "",
        now(),
    );
    assert_eq!(write.status, 403);
    let drove = answer(
        &ctx,
        &request("POST", Some(&drive_cookie)),
        false,
        "p",
        "nat",
        "",
        now(),
    );
    assert_eq!(drove.status, 405);

    // Another lane's link does not reach this stream, and this stream's link does not reach a web lane.
    let crossed = answer(
        &ctx,
        &request("GET", Some(&web_cookie)),
        false,
        "p",
        "nat",
        "",
        now(),
    );
    assert_eq!(crossed.status, 401);
    let web_refused = answer(
        &ctx,
        &request("GET", Some(&watch_cookie)),
        false,
        "p",
        "web",
        "",
        now(),
    );
    assert_eq!(web_refused.status, 401);
}

fn stand_in_node(key: &NodeKey) -> Node {
    Node {
        id: NODE.into(),
        name: "box".into(),
        public_key: key.public_b64(),
        cid_base: REMOTE_CID_BASE,
        cid_count: CIDS_PER_NODE,
        joined_ms: 0,
        last_seen_ms: 0,
        machine: Default::default(),
        draining: false,
        revoked: false,
        address: String::new(),
    }
}

/// The lane's preview through a booted hub: the tunnel's route over loopback TLS (the node dials it), the preview route
/// on a plain listener, the CI recipe's preview step starting the app, and a node link that carries the streams.
#[test]
fn a_lane_preview_opens_from_a_link_through_the_booted_hub() {
    let store = Arc::new(Store::for_tests());
    let node_key = NodeKey::generate();
    let nodes = Arc::new(nodes::Store::holding(vec![stand_in_node(&node_key)]));
    let standin = Arc::new(NodedStandIn {
        announce: true,
        ..Default::default()
    });
    let stack = project_stack::parse(STACK).expect("the stack parses");
    let target = stack.targets[0].clone();

    // The fallback first, on its own store: a guest that stays silent gets its port registered by the hub itself.
    let silent_store = Store::for_tests();
    let silent = NodedStandIn::default();
    let silent_ctx = Ctx {
        store: &silent_store,
        nodes: &nodes,
        host: &silent,
        frames: &NoFrames,
    };
    let fallback_lane = Lane {
        project: "p",
        lane: "silent",
        rev: "rev0",
        repo: "repo",
    };
    life::start_on(&silent_ctx, NODE, &fallback_lane, &stack, &target, now())
        .expect("the preview starts");
    let calls = silent.calls();
    assert!(
        calls.iter().any(|c| c == "POST /v1/runs/r1/ports"),
        "the hub registered it: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.starts_with("POST /v1/forks/")),
        "no announcement came: {calls:?}"
    );

    // The CI recipe's preview step: the lane's app built and started in a fork, its port registered with noded.
    let host: &NodedStandIn = &standin;
    let ctx = Ctx {
        store: &store,
        nodes: &nodes,
        host,
        frames: &NoFrames,
    };
    let lane = Lane {
        project: "p",
        lane: "lane",
        rev: "rev1",
        repo: "repo",
    };
    let summary =
        life::start_on(&ctx, NODE, &lane, &stack, &target, now()).expect("the preview starts");
    assert!(summary.contains("serves port 8080"), "{summary}");
    // The guest announced its port when it listened, so the hub did not poll noded to register it: no GET at all.
    let calls = standin.calls();
    assert!(
        calls.iter().any(|c| c == "POST /v1/forks/4242/ports"),
        "the guest announced it: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.starts_with("GET ")),
        "the hub asks noded nothing: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c == "POST /v1/runs/r1/ports"),
        "the hub did not register it: {calls:?}"
    );
    let preview = store
        .lane_preview("p", "lane")
        .expect("the lane has a preview");
    assert_eq!(preview.status, AWAKE);
    assert!(
        standin.serves(CID, PORT),
        "noded serves the registered port"
    );

    // A second lane's preview, with no VM behind it, for the isolation check.
    store
        .add_preview("p", "other", NODE, RUN, CID, u32::from(PORT), now())
        .expect("the other lane's row");

    // The node link: the hub's tunnel route over TLS, pinned as a join pins it, and vsockd's inbound listener.
    let pair = apz_trust::tls::self_signed(&["localhost".to_string()]).expect("a certificate");
    let pin = apz_trust::tls::fingerprint(&pair.cert_pem).expect("a pin");
    let tls = apz_trust::tls::server_config(&pair).expect("a server config");
    let dispatch: Dispatch =
        Arc::new(|_: &Request| Response::error(404, "the hub's routes are not this test's"));
    let tunnel_router: Router<()> = {
        let (nodes_for, dispatch) = (Arc::clone(&nodes), Arc::clone(&dispatch));
        Router::new().get("/v1/nodes/n1/tunnel", move |_, req, _| {
            match nodes_for.authenticate(req, now()) {
                Ok(node) => {
                    tunnel::open(req, node.id, Arc::clone(&nodes_for), Arc::clone(&dispatch))
                }
                Err(reason) => Response::error(401, &reason),
            }
        })
    };
    let hub = Server::bind(0)
        .expect("an ephemeral port")
        .tls(tls)
        .spawn(move |request| tunnel_router.handle(&(), request))
        .expect("serves");

    let gate = Gate {
        slot: Arc::new(Mutex::new(None)),
        seen: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let inbound = Arc::new(Inbound {
        fork: Arc::clone(&standin),
        app: fork_app(gate.clone()),
    });
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let inbound_port = listener.local_addr().expect("address").port();
    {
        let (check, dial) = (Arc::clone(&inbound), inbound);
        thread::spawn(move || {
            apz_link::inbound::serve(
                listener,
                check as Arc<dyn PortCheck>,
                dial as Arc<dyn GuestDial>,
            )
        });
    }
    let identity = Identity {
        hub_url: format!("https://localhost:{}", hub.port()),
        pin: apz_trust::b64(&pin),
        node_id: NODE.into(),
        cid_base: REMOTE_CID_BASE,
        cid_count: CIDS_PER_NODE,
        hub_name: "test".into(),
    };
    let local = Local {
        noded: 0,
        vsockd: 0,
        forward: 0,
        token: InternalToken::none(),
        inbound: inbound_port,
    };
    let mux = node_tunnel::connect(&identity, &node_key).expect("the tunnel opens over TLS");
    let _serving = thread::spawn(move || node_tunnel::serve(mux, local));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while nodes.tunnel(NODE).is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the hub never saw the tunnel"
        );
        thread::sleep(Duration::from_millis(10));
    }

    // The preview route on the hub. It is outside `/v1/`, so the owner gate lets it through to its own check.
    assert!(crate::hub_auth::is_public("GET", "/preview/p/lane/"));
    let web_router = preview_router(Arc::clone(&store), Arc::clone(&nodes), Arc::clone(&standin));
    let web_server = Server::bind(0)
        .expect("an ephemeral port")
        .spawn(move |request| web_router.handle(&(), request))
        .expect("serves");
    let web = SocketAddr::from(([127, 0, 0, 1], web_server.port()));

    // The owner's link to the lane, for use; a view-only link; an expired one.
    let use_link = store
        .add_link(
            &preview.id,
            USE,
            "for the review",
            Some(now() + 3_600_000),
            "owner",
            now(),
        )
        .expect("the link");
    let use_token = store.mint(&use_link.id);
    let cookie = format!("apz_preview={use_token}");

    // 1. Without a link the preview asks for sign-in: the app is not reached.
    let (status, head, body) = exchange(web, "GET", "/preview/p/lane/", &[], b"");
    assert_eq!(status, 401, "{head} {body}");

    // 2. A link's first request sets its cookie and drops the token from the address.
    let (status, head, _) = exchange(
        web,
        "GET",
        &format!("/preview/p/lane/?{TOKEN_PARAM}={use_token}"),
        &[],
        b"",
    );
    assert_eq!(status, 303, "{head}");
    assert!(
        head.contains(&format!("set-cookie: apz_preview={use_token}")),
        "{head}"
    );
    assert!(
        head.contains("location: /preview/p/lane/") && !head.contains(TOKEN_PARAM),
        "{head}"
    );

    // 3. With the cookie, the app's page arrives through the hub, over the node's tunnel.
    let (status, _, body) = exchange(web, "GET", "/preview/p/lane/", &[("Cookie", &cookie)], b"");
    assert_eq!(status, 200);
    assert!(body.contains("lane preview"), "{body}");

    // 4. A POST reaches the app with its body.
    let (status, _, body) = exchange(
        web,
        "POST",
        "/preview/p/lane/echo",
        &[("Cookie", &cookie)],
        b"hello",
    );
    assert_eq!((status, body.as_str()), (200, "got 5 bytes"));

    // 4b. A server-sent stream reaches the viewer as the app sends it: the first event arrives while the app still
    // waits at the gate, before it sends the second.
    let (gate_tx, gate_rx) = mpsc::channel();
    *gate.slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(gate_rx);
    let mut events = open_stream(web, "/preview/p/lane/stream", &[("Cookie", &cookie)]);
    let first = read_until(&mut events, "data: 1");
    assert!(
        first.contains("200 OK") && first.contains("text/event-stream"),
        "{first}"
    );
    gate_tx.send(()).expect("the app is waiting at the gate");
    let mut rest = String::new();
    events.read_to_string(&mut rest).expect("the stream ends");
    assert!(
        first.contains("data: 2") || rest.contains("data: 2"),
        "{first}{rest}"
    );
    assert!(
        gate.seen.load(std::sync::atomic::Ordering::SeqCst),
        "the first event reached the viewer before the app sent the second"
    );

    // 5. Every opening is recorded: when, from where, what.
    let openings = store.openings(&use_link.id, 50);
    assert_eq!(openings.len(), 4, "{openings:?}");
    assert!(openings.iter().all(|o| o.from.starts_with("127.0.0.1")));
    assert!(
        openings
            .iter()
            .any(|o| o.method == "POST" && o.path == "/echo")
    );

    // 6. A view link reads the pages and refuses a write.
    let view = store
        .add_link(
            &preview.id,
            VIEW,
            "watch",
            Some(now() + 3_600_000),
            "owner",
            now(),
        )
        .expect("the view link");
    let view_cookie = format!("apz_preview={}", store.mint(&view.id));
    let (status, _, _) = exchange(
        web,
        "GET",
        "/preview/p/lane/",
        &[("Cookie", &view_cookie)],
        b"",
    );
    assert_eq!(status, 200);
    let (status, _, _) = exchange(
        web,
        "POST",
        "/preview/p/lane/echo",
        &[("Cookie", &view_cookie)],
        b"x",
    );
    assert_eq!(status, 403);

    // 7. A link grants its own preview only: on the other lane it is refused before any VM is woken.
    let (status, _, body) = exchange(web, "GET", "/preview/p/other/", &[("Cookie", &cookie)], b"");
    assert_eq!(status, 401, "{body}");
    assert!(body.contains("another preview"), "{body}");

    // 8. An expired link is refused.
    let expired = store
        .add_link(&preview.id, USE, "old", Some(now() - 1), "owner", now())
        .expect("the expired link");
    let expired_cookie = format!("apz_preview={}", store.mint(&expired.id));
    let (status, _, body) = exchange(
        web,
        "GET",
        "/preview/p/lane/",
        &[("Cookie", &expired_cookie)],
        b"",
    );
    assert_eq!(status, 401);
    assert!(body.contains("expired"), "{body}");

    // 9. Idle for 15 minutes, the lifecycle pass sleeps the preview; the next request through the link wakes it.
    let mut idle = store.preview(&preview.id).expect("the preview");
    idle.used_ms = now() - life::IDLE_SLEEP_MS - 1;
    store.save_preview(&idle).expect("saves the idle clock");
    let notes = life::settle(&ctx, now(), &|_: &str, _: &str| false, &|_: &str| 24);
    assert!(
        notes.iter().any(|n| n.contains("asleep when idle")),
        "{notes:?}"
    );
    assert_eq!(
        store.preview(&preview.id).expect("the preview").status,
        ASLEEP
    );
    assert!(!standin.serves(CID, PORT), "a sleeping VM is not served");
    let (status, _, body) = exchange(web, "GET", "/preview/p/lane/", &[("Cookie", &cookie)], b"");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("lane preview"));
    assert!(
        standin
            .calls()
            .iter()
            .any(|c| c == "POST /v1/runs/r1/vms/0/wake")
    );
    assert_eq!(
        store.preview(&preview.id).expect("the preview").status,
        AWAKE
    );

    // 9b. The lane's subdomain (`<lane>--<project>.<domain>`) is the same preview: the request is moved to its path, and
    // the link's cookie carries over.
    let direct = preview_router(Arc::clone(&store), Arc::clone(&nodes), Arc::clone(&standin));
    let host_request = Request {
        method: "GET".into(),
        path: "/stream".into(),
        query: String::new(),
        headers: vec![
            ("host".into(), "lane--p.preview.test".into()),
            ("cookie".into(), cookie.clone()),
        ],
        body: Vec::new(),
        peer: None,
    };
    let moved =
        route::subdomain_request(&host_request, "preview.test").expect("the Host names a preview");
    assert_eq!(moved.path, "/preview/p/lane/stream");
    let mut answered = Vec::new();
    match direct.handle(&(), &moved).body {
        Body::Stream(write) => write(&mut answered).expect("relays"),
        Body::Bytes(bytes) => answered = bytes,
        Body::Upgrade(_) => panic!("a preview is not upgraded"),
    }
    assert!(
        String::from_utf8_lossy(&answered).contains("data: 1"),
        "{answered:?}"
    );
    let without_cookie = Request {
        headers: vec![("host".into(), "lane--p.preview.test".into())],
        ..host_request
    };
    let refused = route::subdomain_request(&without_cookie, "preview.test").expect("moved");
    assert_eq!(
        direct.handle(&(), &refused).status,
        401,
        "a subdomain needs the same link"
    );

    // 10. A revoked link never works again.
    store.revoke(&use_link.id, now()).expect("revokes");
    let (status, _, body) = exchange(web, "GET", "/preview/p/lane/", &[("Cookie", &cookie)], b"");
    assert_eq!(status, 401);
    assert!(body.contains("revoked"), "{body}");

    // 11. The lane lands: its preview ends with the grace (none here), and the app is closed for good.
    let ended = |_: &str, _: &str| true;
    let grace = |_: &str| 0u64;
    let notes = life::settle(&ctx, now(), &ended, &grace);
    assert!(notes.iter().any(|n| n.contains("closed")), "{notes:?}");
    assert_eq!(
        store.preview(&preview.id).expect("the preview").status,
        CLOSED
    );
    let (status, _, _) = exchange(
        web,
        "GET",
        "/preview/p/lane/",
        &[("Cookie", &view_cookie)],
        b"",
    );
    assert_eq!(status, 410);
    assert!(
        standin
            .calls()
            .iter()
            .any(|c| c == "POST /v1/runs/r1/vms/0/stop")
    );

    drop(web_server);
    drop(hub);
}
