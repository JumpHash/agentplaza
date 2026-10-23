//! Two node stores on loopback: the second fetches a manifest's chunks from the first, presenting the hub's grant.

use std::io::Cursor;
use std::sync::Arc;

use apz_blobs::Blobs;
use apz_transfer::{Grant, Manifest, Node, fetch, now_ms};
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

/// Node "a" holds the files and serves them on loopback; node "b" has an empty store.
fn pair() -> Pair {
    let (a_dir, b_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let hub = SigningKey::from_bytes(&[7; 32]);
    let sender = Node::new("a", store(&a_dir));
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

fn grant(hub: &SigningKey, manifest: &Manifest, to: &str) -> Grant {
    Grant::issue(
        hub,
        "base:p:build",
        &manifest.id(),
        "a",
        to,
        now_ms() + apz_transfer::grant::TTL_MS,
    )
}

#[test]
fn a_base_reaches_the_second_node_and_a_changed_chunk_moves_alone() {
    let p = pair();
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
        &p.addr,
        &grant(&p.hub, &m1, "b"),
        &m1,
    )
    .expect("fetched");
    assert_eq!(first.bytes, m1.size(), "every byte of the base came");
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
        &p.addr,
        &grant(&p.hub, &m2, "b"),
        &m2,
    )
    .expect("fetched");
    let chunks = m2.files[0].chunks.len();
    assert!(second.fetched >= 1, "{second:?}");
    assert_eq!(
        second.fetched, 1,
        "a one-byte change moves one chunk of {chunks}: {second:?}"
    );
    assert_eq!(p.sender.served_bytes() - before, second.bytes);
    assert_eq!(p.receiver_blobs.get(&m2.files[0].root).unwrap(), changed);
}

#[test]
fn a_forged_expired_or_misdirected_grant_is_refused() {
    let p = pair();
    let bytes = noise(1 << 20, 3);
    let m = manifest_of(p.sender.blobs(), &[("f", &bytes)]);
    p.sender.publish(&m).unwrap();

    let forged = Grant::issue(
        &SigningKey::from_bytes(&[9; 32]),
        "base:p:build",
        &m.id(),
        "a",
        "b",
        now_ms() + 60_000,
    );
    let err = fetch(&p.receiver_blobs, "b", &p.addr, &forged, &m).unwrap_err();
    assert!(err.contains("not signed by the hub"), "{err}");

    let expired = Grant::issue(&p.hub, "base:p:build", &m.id(), "a", "b", now_ms() - 1);
    let err = fetch(&p.receiver_blobs, "b", &p.addr, &expired, &m).unwrap_err();
    assert!(err.contains("expired"), "{err}");

    let other = grant(&p.hub, &m, "c");
    let err = fetch(&p.receiver_blobs, "b", &p.addr, &other, &m).unwrap_err();
    assert!(err.contains("another node to receive"), "{err}");

    // A manifest the sender never published is refused even with a good grant.
    let unpublished = manifest_of(p.sender.blobs(), &[("g", &noise(1 << 20, 5))]);
    let err = fetch(
        &p.receiver_blobs,
        "b",
        &p.addr,
        &grant(&p.hub, &unpublished, "b"),
        &unpublished,
    )
    .unwrap_err();
    assert!(err.contains("refused"), "{err}");
    assert_eq!(p.sender.served_bytes(), 0, "no refused grant sent a byte");
}
