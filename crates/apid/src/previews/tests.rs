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
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use apz_http::auth::InternalToken;
use apz_http::{Request, Response, Router, Server};
use apz_link::identity::Identity;
use apz_link::inbound::{Duplex, GuestDial, PortCheck};
use apz_link::run::Local;
use apz_link::tunnel as node_tunnel;
use apz_proto::wire::HostMessage;
use apz_trust::node::NodeKey;
use serde_json::{Value, json};

use super::life::{self, Ctx, Lane};
use super::proxy;
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

fn request(method: &str, query: &str, headers: &[(&str, &str)], body: &[u8]) -> Request {
    Request {
        method: method.to_string(),
        path: "/".to_string(),
        query: query.to_string(),
        headers: headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
        body: body.to_vec(),
        peer: None,
    }
}

#[test]
fn the_app_gets_what_it_needs_and_none_of_the_hubs_credentials() {
    let req = request(
        "POST",
        "x=1",
        &[
            ("content-type", "text/plain"),
            ("authorization", "Bearer owner"),
            ("cookie", "apz_preview=secret"),
        ],
        b"hello",
    );
    let bytes = String::from_utf8(proxy::request_bytes(&req, "/echo", "x=1")).expect("text");
    assert!(bytes.starts_with("POST /echo?x=1 HTTP/1.1\r\n"), "{bytes}");
    assert!(bytes.contains("Content-Length: 5\r\n"));
    assert!(bytes.contains("content-type: text/plain\r\n"));
    assert!(!bytes.to_ascii_lowercase().contains("authorization"));
    assert!(!bytes.to_ascii_lowercase().contains("cookie"));
    assert!(bytes.ends_with("\r\n\r\nhello"));
}

#[test]
fn an_answer_is_read_whole_and_a_chunked_one_is_de_chunked() {
    let plain = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nX-Hidden: no\r\nContent-Length: 4\r\n\r\nhi!!";
    let answer = proxy::parse(plain).expect("parses");
    assert_eq!(answer.status, 200);
    assert_eq!(
        answer.headers,
        vec![("content-type".to_string(), "text/html".to_string())]
    );
    let chunked = b"HTTP/1.1 201 Created\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n3;ext=1\r\nefg\r\n0\r\n\r\n";
    let answer = proxy::parse(chunked).expect("parses");
    assert_eq!(answer.status, 201);
    match answer.body {
        apz_http::Body::Bytes(bytes) => assert_eq!(bytes, b"abcdefg"),
        _ => panic!("a parsed answer is bytes"),
    }
    assert!(proxy::parse(b"not http").is_err());
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
            _ => (404, json!({"error": format!("no route {path}")})),
        })
    }

    fn send(&self, cid: u64, message: &HostMessage) -> Result<(), String> {
        // The base runner's exec items: each succeeds at once, as a VM that ran them would answer.
        if let HostMessage::Exec { id, .. } = message {
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

/// The lane's app: a small HTTP server in the fork. `GET /` is its page, `POST /echo` answers with the body's size.
fn fork_app() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let addr = listener.local_addr().expect("address");
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(conn) = conn else { return };
            thread::spawn(move || serve_app(conn));
        }
    });
    addr
}

fn serve_app(mut conn: TcpStream) {
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
    let standin = Arc::new(NodedStandIn::default());
    let stack = project_stack::parse(STACK).expect("the stack parses");
    let target = stack.targets[0].clone();

    // The CI recipe's preview step: the lane's app built and started in a fork, its port registered with noded.
    let host: &NodedStandIn = &standin;
    let ctx = Ctx {
        store: &store,
        nodes: &nodes,
        host,
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

    let inbound = Arc::new(Inbound {
        fork: Arc::clone(&standin),
        app: fork_app(),
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
    let mut web_router: Router<()> = Router::new();
    for method in ["GET", "POST"] {
        let (store, nodes, standin) =
            (Arc::clone(&store), Arc::clone(&nodes), Arc::clone(&standin));
        let handler = move |_: &(), req: &Request, p: &apz_http::Params<'_>| {
            let host: &NodedStandIn = &standin;
            let ctx = Ctx {
                store: &store,
                nodes: &nodes,
                host,
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
        web_router = web_router
            .route(method, "/preview/{project}/{lane}/", handler.clone())
            .route(method, "/preview/{project}/{lane}/{rest..}", handler);
    }
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

    // 5. Every opening is recorded: when, from where, what.
    let openings = store.openings(&use_link.id, 50);
    assert_eq!(openings.len(), 3, "{openings:?}");
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
