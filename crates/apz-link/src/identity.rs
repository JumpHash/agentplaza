//! The node's identity on disk, under its state directory (mode 0700):
//! - `node.json` (0600): [`Identity`] — hub URL, certificate pin, node id, CID range, hub name;
//! - `node.key` (0600): the Ed25519 seed (32 raw bytes). It never leaves this machine.
//! - `internal-token` (0600): the secret the local vsockd presents to the forwarder.

use std::path::Path;

use apz_http::client::Target;
use apz_trust::node::NodeKey;
use serde::{Deserialize, Serialize};

/// What the hub gave this node when it joined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// `https://host:port` (no trailing slash).
    pub hub_url: String,
    /// SHA-256 of the hub's TLS certificate, base64url (as in the join token).
    pub pin: String,
    pub node_id: String,
    pub cid_base: u64,
    pub cid_count: u64,
    #[serde(default)]
    pub hub_name: String,
}

impl Identity {
    /// The pin as bytes; Err when it is not 32 bytes of base64url.
    pub fn pin_bytes(&self) -> Result<[u8; 32], String> {
        Err("contract: L1".into())
    }

    /// The hub as an apz-http target (pinned, no bearer, no extra headers).
    pub fn target(&self) -> Result<Target, String> {
        Err("contract: L1".into())
    }

    /// [`Identity::target`] with this node's signature headers for `method path` (path with its
    /// query) and `body`.
    pub fn signed(&self, key: &NodeKey, method: &str, path: &str, body: &[u8]) -> Result<Target, String> {
        let _ = (key, method, path, body);
        Err("contract: L1".into())
    }

    /// The inclusive guest-CID range as `noded --run-cids` takes it (and people read it): `A-B`.
    pub fn cid_range(&self) -> String {
        format!("{}-{}", self.cid_base, self.cid_base + self.cid_count - 1)
    }

    /// The same CIDs as `vsockd --cids` takes them: `A,A+1,…,B`.
    pub fn cid_list(&self) -> String {
        (self.cid_base..self.cid_base + self.cid_count).map(|cid| cid.to_string()).collect::<Vec<_>>().join(",")
    }
}

/// Writes the identity and key (creating `dir` 0700, the files 0600, atomically).
pub fn save(dir: &Path, identity: &Identity, key: &NodeKey) -> Result<(), String> {
    let _ = (dir, identity, key);
    Err("contract: L1".into())
}

/// Reads them back; Err naming the file when one is missing or malformed.
pub fn load(dir: &Path) -> Result<(Identity, NodeKey), String> {
    let _ = dir;
    Err("contract: L1".into())
}

/// The local forwarder's secret: read `internal-token`, or create it (32 random bytes,
/// base64url, 0600) when absent. Returns the token text.
pub fn internal_token(dir: &Path) -> Result<String, String> {
    let _ = dir;
    Err("contract: L1".into())
}
