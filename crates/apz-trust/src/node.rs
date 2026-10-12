//! A node's identity and the signed-request scheme.
//!
//! Every node request carries headers [`H_NODE`], [`H_TS`], [`H_NONCE`] and [`H_SIG`], where the
//! signature is Ed25519 over `"{method}\n{path_and_query}\n{ts}\n{nonce}\n{hex(sha256(body))}"`.
//! The hub accepts a timestamp within [`MAX_SKEW_SECS`] and remembers nonces for [`NONCE_TTL_SECS`].

pub const H_NODE: &str = "x-apz-node";
pub const H_TS: &str = "x-apz-ts";
pub const H_NONCE: &str = "x-apz-nonce";
pub const H_SIG: &str = "x-apz-sig";
pub const MAX_SKEW_SECS: u64 = 60;
pub const NONCE_TTL_SECS: u64 = 300;

/// A node's signing key (kept on the node, 0600).
pub struct NodeKey {
    pub(crate) key: ed25519_dalek::SigningKey,
}

impl NodeKey {
    pub fn generate() -> NodeKey {
        unimplemented!("contract: T1")
    }
    /// The 32-byte secret seed, for storage.
    pub fn to_bytes(&self) -> [u8; 32] {
        unimplemented!("contract: T1")
    }
    pub fn from_bytes(_seed: &[u8]) -> Result<NodeKey, String> {
        Err("contract: T1".into())
    }
    /// The public key, base64url (what the hub stores).
    pub fn public_b64(&self) -> String {
        unimplemented!("contract: T1")
    }
    /// The four headers for one request, `nonce` 16 random bytes base64url, `ts` unix seconds.
    pub fn sign(&self, _node_id: &str, _method: &str, _path: &str, _ts: u64, _body: &[u8]) -> Vec<(&'static str, String)> {
        unimplemented!("contract: T1")
    }
}

/// The message a signature covers.
pub fn signed_message(method: &str, path: &str, ts: u64, nonce: &str, body: &[u8]) -> String {
    let digest = crate::sha256(body);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{method}\n{path}\n{ts}\n{nonce}\n{hex}")
}

/// Verifies a signature made by [`NodeKey::sign`] against the node's stored public key.
/// Skew and replay are the caller's checks (it knows the clock and the nonce cache).
pub fn verify(_public_b64: &str, _method: &str, _path: &str, _ts: u64, _nonce: &str, _body: &[u8], _sig_b64: &str) -> bool {
    false // contract: T1
}
