//! Node stores on loopback: a second node fetches a manifest's chunks from the first over TLS, pinned to the first's
//! certificate, presenting the hub's grant. Also the brokered path: the direct dial is refused, and the bytes go through
//! the hub's broker and the sender's tunnel.

use std::io::Cursor;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::Duration;

use apz_blobs::Blobs;
use apz_http::mux::{Carrier, Mux, Side};
use apz_transfer::broker::{self, Tickets};
use apz_transfer::{Grant, Manifest, Node, Route, Via, fetch, now_ms};
use ed25519_dalek::SigningKey;

/// Bytes that do not repeat within a chunk, so a file this size is cut into several chunks.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn store(dir: &tempfile::TempDir) -> Arc<Blobs> {
    Arc::new(Blobs::open(dir.path()).expect("a store"))
}

/// A node's own certificate, for the test's node.
fn identity() -> apz_trust::tls::CertPair {
    apz_trust::tls::self_signed(&["localhost".to_string()]).expect("a certificate")
}

/// A node's files as a manifest, stored in its store.
fn manifest_of(blobs: &Blobs, files: &[(&str, &[u8])]) -> Manifest {
    Manifest {
        files: files
            .iter()
            .map(|(name, bytes)| Manifest::store(blobs, name, Cursor::new(*bytes)).expect("stored"))
            .collect(),
    }
}

struct Pair {
    hub: SigningKey,
    sender: Arc<Node>,
    receiver_blobs: Arc<Blobs>,
    _handle: apz_http::Handle,
    addr: String,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

/// Node "a" holds the files and serves them over TLS on loopback; node "b" has an empty store.
fn pair() -> Pair {
    let (a_dir, b_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let hub = SigningKey::from_bytes(&[7; 32]);
    let sender = Node::new("a", store(&a_dir), &identity()).expect("a node");
    sender.trust(hub.verifying_key());
    let handle = sender
        .spawn("127.0.0.1:0".parse().unwrap())
        .expect("a listener");
    let addr = format!("127.0.0.1:{}", handle.port());
    Pair {
        hub,
        sender,
        receiver_blobs: store(&b_dir),
        _handle: handle,
        addr,
        _dirs: (a_dir, b_dir),
    }
}

fn pin_of(node: &Node) -> String {
    apz_transfer::pin_text(&node.pin())
}

fn grant(hub: &SigningKey, manifest: &Manifest, to: &str, pin: &str) -> Grant {
    Grant::issue(
        hub,
        "base:p:build",
        &manifest.id(),
        "a",
        to,
        pin,
        now_ms() + apz_transfer::grant::TTL_MS,
    )
}

fn direct(p: &Pair) -> Vec<Route> {
    vec![Route::Direct(p.addr.clone())]
}

#[test]
fn a_base_reaches_the_second_node_and_a_changed_chunk_moves_alone() {
    let p = pair();
    let pin = pin_of(&p.sender);
    let base = noise(24 << 20, 11);
    let small = b"a small file, one blob".to_vec();
    let m1 = manifest_of(
        p.sender.blobs(),
        &[("base.bin", &base), ("small.txt", &small)],
    );
    assert!(
        p.sender
            .blobs()
            .chunks(&m1.files[0].root)
            .unwrap()
            .is_some(),
        "the base is chunked"
    );
    p.sender.publish(&m1).expect("published");

    let first = fetch(
        &p.receiver_blobs,
        "b",
        &direct(&p),
        &grant(&p.hub, &m1, "b", &pin),
        &m1,
    )
    .expect("fetched");
    assert_eq!(first.bytes, m1.size(), "every byte of the base came");
    assert_eq!(first.via, Via::Direct);
    assert_eq!(p.receiver_blobs.get(&m1.files[0].root).unwrap(), base);
    assert_eq!(p.receiver_blobs.get(&m1.files[1].root).unwrap(), small);
    assert_eq!(p.sender.served_bytes(), first.bytes, "the sender sent them");

    // A base that differs in one byte: only the chunk that holds it moves.
    let mut changed = base.clone();
    changed[12 << 20] ^= 0xff;
    let m2 = manifest_of(
        p.sender.blobs(),
        &[("base.bin", &changed), ("small.txt", &small)],
    );
    p.sender.publish(&m2).expect("published");
    let before = p.sender.served_bytes();
    let second = fetch(
        &p.receiver_blobs,
        "b",
        &direct(&p),
        &grant(&p.hub, &m2, "b", &pin),
        &m2,
    )
    .expect("fetched");
    assert_eq!(
        second.fetched, 1,
        "a one-byte change moves one chunk: {second:?}"
    );
    assert_eq!(p.sender.served_bytes() - before, second.bytes);
    assert_eq!(p.receiver_blobs.get(&m2.files[0].root).unwrap(), changed);
}

#[test]
fn a_forged_expired_or_misdirected_grant_is_refused() {
    let p = pair();
    let pin = pin_of(&p.sender);
    let bytes = noise(1 << 20, 3);
    let m = manifest_of(p.sender.blobs(), &[("f", &bytes)]);
    p.sender.publish(&m).unwrap();

    let forged = Grant::issue(
        &SigningKey::from_bytes(&[9; 32]),
        "base:p:build",
        &m.id(),
        "a",
        "b",
        &pin,
        now_ms() + 60_000,
    );
    let err = fetch(&p.receiver_blobs, "b", &direct(&p), &forged, &m).unwrap_err();
    assert!(err.contains("not signed by the hub"), "{err}");

    let expired = Grant::issue(
        &p.hub,
        "base:p:build",
        &m.id(),
        "a",
        "b",
        &pin,
        now_ms() - 1,
    );
    let err = fetch(&p.receiver_blobs, "b", &direct(&p), &expired, &m).unwrap_err();
    assert!(err.contains("expired"), "{err}");

    let other = grant(&p.hub, &m, "c", &pin);
    let err = fetch(&p.receiver_blobs, "b", &direct(&p), &other, &m).unwrap_err();
    assert!(err.contains("another node to receive"), "{err}");

    // A manifest the sender never published is refused even with a good grant.
    let unpublished = manifest_of(p.sender.blobs(), &[("g", &noise(1 << 20, 5))]);
    let err = fetch(
        &p.receiver_blobs,
        "b",
        &direct(&p),
        &grant(&p.hub, &unpublished, "b", &pin),
        &unpublished,
    )
    .unwrap_err();
    assert!(err.contains("refused"), "{err}");
    assert_eq!(p.sender.served_bytes(), 0, "no refused grant sent a byte");
}

#[test]
fn a_wrong_pin_is_refused_before_any_byte_is_sent() {
    let p = pair();
    let bytes = noise(1 << 20, 4);
    let m = manifest_of(p.sender.blobs(), &[("f", &bytes)]);
    p.sender.publish(&m).unwrap();
    // A pin of some other certificate: the TLS session does not form, so the sender serves nothing.
    let other = Node::new("x", store(&tempfile::tempdir().unwrap()), &identity()).unwrap();
    let err = fetch(
        &p.receiver_blobs,
        "b",
        &direct(&p),
        &grant(&p.hub, &m, "b", &pin_of(&other)),
        &m,
    )
    .unwrap_err();
    assert!(!err.is_empty(), "the wrong pin is refused");
    assert_eq!(p.sender.served_bytes(), 0);
    assert!(
        !p.receiver_blobs.has(&m.files[0].root),
        "nothing was stored"
    );
}

/// An in-memory carrier: one end of a tunnel between the hub and a node.
struct Wire {
    tx: Sender<Vec<u8>>,
    rx: Receiver<Vec<u8>>,
}

impl Carrier for Wire {
    fn send(&mut self, frame: &[u8]) -> std::io::Result<()> {
        self.tx
            .send(frame.to_vec())
            .map_err(|_| std::io::ErrorKind::BrokenPipe.into())
    }

    fn recv(&mut self, wait: Duration) -> std::io::Result<Option<Vec<u8>>> {
        match self.rx.recv_timeout(wait) {
            Ok(frame) => Ok(Some(frame)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(std::io::ErrorKind::BrokenPipe.into()),
        }
    }
}

/// A tunnel between the hub (dialer) and a node (acceptor). The node answers each stream of kind `transfer` by piping it
/// to its sender's transfer address, as apz-link does for its node.
fn tunnel(sender_addr: String) -> Arc<Mux> {
    let (to_node, to_hub) = {
        let (a_tx, b_rx) = channel();
        let (b_tx, a_rx) = channel();
        (Wire { tx: a_tx, rx: a_rx }, Wire { tx: b_tx, rx: b_rx })
    };
    let hub = Mux::new(to_node, Side::Dialer);
    let node = Mux::new(to_hub, Side::Acceptor);
    thread::spawn(move || {
        loop {
            match node.accept(Duration::from_millis(200)) {
                Ok(Some(stream)) => {
                    let header: serde_json::Value =
                        serde_json::from_slice(stream.header()).unwrap_or_default();
                    if header["kind"] != "transfer" {
                        stream.reset("unknown stream kind");
                        continue;
                    }
                    let to = sender_addr.clone();
                    thread::spawn(move || {
                        if let Ok(conn) = TcpStream::connect(&to) {
                            broker::pipe(stream, conn, Arc::new(|_| {}));
                        }
                    });
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
    });
    hub
}

#[test]
fn a_blocked_direct_dial_is_brokered_through_the_hub_and_the_hub_counts_the_bytes() {
    let p = pair();
    let pin = pin_of(&p.sender);
    let bytes = noise(6 << 20, 21);
    let m = manifest_of(p.sender.blobs(), &[("base.bin", &bytes)]);
    p.sender.publish(&m).unwrap();

    // The direct dial is refused: a port nothing listens on.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_addr = closed.local_addr().unwrap().to_string();
    drop(closed);

    // The hub's broker, and the sender's tunnel through it.
    let total = Arc::new(AtomicU64::new(0));
    let tickets = Arc::new(Tickets::new(Arc::clone(&total)));
    let ticket = tickets.issue("a", now_ms() + 60_000);
    let hub_mux = tunnel(p.addr.clone());
    let open: broker::Open = Arc::new(move |_node: &str| hub_mux.open(br#"{"kind":"transfer"}"#));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let broker_addr = listener.local_addr().unwrap().to_string();
    broker::serve(listener, Arc::clone(&tickets), open);

    let routes = vec![
        Route::Direct(closed_addr),
        Route::Brokered {
            broker: broker_addr,
            ticket: ticket.clone(),
        },
    ];
    let report = fetch(
        &p.receiver_blobs,
        "b",
        &routes,
        &grant(&p.hub, &m, "b", &pin),
        &m,
    )
    .expect("fetched through the broker");
    assert_eq!(report.via, Via::Brokered, "{report:?}");
    assert_eq!(p.receiver_blobs.get(&m.files[0].root).unwrap(), bytes);
    assert_eq!(
        p.sender.served_bytes(),
        report.bytes,
        "the sender sent them"
    );
    // The hub relayed every byte of the transfer, the TLS and the HTTP included, and counted them on the ticket.
    let brokered = tickets.bytes(&ticket);
    assert!(brokered >= report.bytes, "{brokered} < {}", report.bytes);
    assert_eq!(total.load(std::sync::atomic::Ordering::Relaxed), brokered);
}
