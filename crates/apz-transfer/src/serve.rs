//! The sending side: a node's chunk server. It offers the manifests it published, and serves a chunk of one to a
//! request that carries a grant the hub signed for this node and that receiver.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use apz_blobs::{Blobs, Hash};
use apz_http::{Handle, Request, Response, Server};
use ed25519_dalek::VerifyingKey;

use crate::{GRANT_HEADER, Grant, Manifest, RECEIVER_HEADER, now_ms};

/// A node's transfer state: its store, the hub's grant key, the manifests it offers, and its certificate (served over TLS;
/// a receiver pins its pin, which the hub puts in the grant).
pub struct Node {
    tls: Arc<rustls::ServerConfig>,
    pin: [u8; 32],
    /// This node's id in the hub; the hub names it with each call (`set_id`).
    id: Mutex<String>,
    blobs: Arc<Blobs>,
    hub: Mutex<Option<VerifyingKey>>,
    published: Mutex<BTreeMap<String, BTreeSet<Hash>>>,
    served: AtomicU64,
}

impl Node {
    pub fn new(
        id: &str,
        blobs: Arc<Blobs>,
        identity: &apz_trust::tls::CertPair,
    ) -> Result<Arc<Node>, String> {
        Ok(Arc::new(Node {
            tls: apz_trust::tls::server_config(identity)?,
            pin: apz_trust::tls::fingerprint(&identity.cert_pem)?,
            id: Mutex::new(id.to_string()),
            blobs,
            hub: Mutex::new(None),
            published: Mutex::new(BTreeMap::new()),
            served: AtomicU64::new(0),
        }))
    }

    /// The SHA-256 of this node's certificate: what a grant pins.
    pub fn pin(&self) -> [u8; 32] {
        self.pin
    }

    pub fn id(&self) -> String {
        self.id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Names this node (the hub's id for it), as the hub's call says.
    pub fn set_id(&self, id: &str) {
        *self.id.lock().unwrap_or_else(PoisonError::into_inner) = id.to_string();
    }

    pub fn blobs(&self) -> &Arc<Blobs> {
        &self.blobs
    }

    /// The hub's grant key, as the hub gave it over its authenticated link.
    pub fn trust(&self, hub: VerifyingKey) {
        *self.hub.lock().unwrap_or_else(PoisonError::into_inner) = Some(hub);
    }

    /// Offers a manifest: every chunk it names must already be held here. Returns its id.
    pub fn publish(&self, manifest: &Manifest) -> Result<String, String> {
        let leaves = manifest.leaves();
        for leaf in &leaves {
            if !self.blobs.has(leaf) {
                return Err(format!("chunk {} is not held here", leaf.short()));
            }
        }
        let id = manifest.id();
        self.published
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone(), leaves.into_iter().collect());
        Ok(id)
    }

    /// The chunk bytes this node has sent.
    pub fn served_bytes(&self) -> u64 {
        self.served.load(Ordering::Relaxed)
    }

    /// Answers `GET /v1/transfer/chunks/{hash}` with a grant in [`GRANT_HEADER`] and the asking node in
    /// [`RECEIVER_HEADER`]. Anything else is refused.
    pub fn handle(&self, req: &Request, now: u64) -> Response {
        let Some(hex) = req.path.strip_prefix("/v1/transfer/chunks/") else {
            return Response::error(404, "no such transfer route");
        };
        let Some(hub) = *self.hub.lock().unwrap_or_else(PoisonError::into_inner) else {
            return Response::error(503, "this node has no hub grant key yet");
        };
        let Some(text) = req.header(GRANT_HEADER) else {
            return Response::error(401, "a transfer needs a grant");
        };
        let grant = match Grant::decode(text) {
            Ok(grant) => grant,
            Err(err) => return Response::error(400, &err),
        };
        if let Err(refusal) = grant.check(&hub, &self.id(), req.header(RECEIVER_HEADER), now) {
            return Response::error(403, refusal.text());
        }
        let Ok(hash) = hex.parse::<Hash>() else {
            return Response::error(400, "the chunk is not a hash");
        };
        let offered = self
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&grant.manifest)
            .is_some_and(|leaves| leaves.contains(&hash));
        if !offered {
            return Response::error(403, crate::Refusal::NotPublished.text());
        }
        match self.blobs.get(&hash) {
            Ok(bytes) => {
                self.served.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                Response::new(200, "application/octet-stream", bytes)
            }
            Err(err) => Response::error(404, &err),
        }
    }

    /// Serves [`Node::handle`] over TLS on `addr` until the handle is dropped.
    pub fn spawn(self: &Arc<Self>, addr: SocketAddr) -> std::io::Result<Handle> {
        let node = Arc::clone(self);
        Server::bind_addr(addr)?
            .tls(Arc::clone(&self.tls))
            .spawn(move |req| node.handle(req, now_ms()))
    }
}
