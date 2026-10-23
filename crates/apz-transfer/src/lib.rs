//! Node-to-node transfers (plan 17.5): a move's files and a base's files go from the node that holds them to the node
//! that needs them. The hub only coordinates: it signs a [`Grant`] that names the receiver, the sender and the manifest,
//! and tells the receiver where to fetch. The bytes never pass through the hub.
//!
//! A file is stored in the node's [`apz_blobs::Blobs`] store, as blobs: a file of at most 4 MiB is one blob, a larger one
//! is content-defined chunks. A [`Manifest`] lists the files with their chunks, and its id is the hash of the manifest,
//! so a node fetches only the chunks it lacks ([`fetch`]).
//!
//! The sender ([`Node`]) serves the chunks of the manifests it published, to a request that carries a grant the hub
//! signed for it: the signature, the expiry, the sender and the receiver named in the grant, and the manifest.

pub mod fetch;
pub mod grant;
pub mod manifest;
pub mod serve;

pub use fetch::{Report, fetch};
pub use grant::{Grant, Refusal};
pub use manifest::{File, Manifest};
pub use serve::Node;

/// The request header that carries a grant (its JSON, base64url).
pub const GRANT_HEADER: &str = "x-apz-grant";
/// The request header that names the node asking (it must be the grant's receiver).
pub const RECEIVER_HEADER: &str = "x-apz-receiver";

/// Unix milliseconds now.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// A verifying key as the hub sends it: base64url of its 32 bytes.
pub fn key_from_text(text: &str) -> Result<ed25519_dalek::VerifyingKey, String> {
    let bytes = apz_trust::unb64(text).map_err(|e| format!("the key is not base64: {e}"))?;
    let bytes = <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| "the key is not 32 bytes".to_string())?;
    ed25519_dalek::VerifyingKey::from_bytes(&bytes)
        .map_err(|e| format!("the key is not valid: {e}"))
}

/// A verifying key as text (see [`key_from_text`]).
pub fn key_to_text(key: &ed25519_dalek::VerifyingKey) -> String {
    apz_trust::b64(key.as_bytes())
}
