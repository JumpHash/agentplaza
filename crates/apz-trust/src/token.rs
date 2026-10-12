//! Join tokens: `apzj1.<base64url(json)>`. The JSON is `{"u": hub URL, "f": base64url SHA-256 of
//! the hub's TLS certificate (DER), "s": base64url 16-byte secret, "x": expiry, unix seconds}`.

use serde::{Deserialize, Serialize};

pub const PREFIX: &str = "apzj1.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinToken {
    pub hub_url: String,
    pub cert_sha256: [u8; 32],
    pub secret: [u8; 16],
    pub expires: u64,
}

impl JoinToken {
    /// A fresh token for `hub_url` pinned to `cert_sha256`, valid `ttl_secs` from `now`.
    pub fn issue(_hub_url: &str, _cert_sha256: [u8; 32], _now: u64, _ttl_secs: u64) -> JoinToken {
        unimplemented!("contract: T1")
    }
    /// The token as text (`apzj1.…`).
    pub fn encode(&self) -> String {
        unimplemented!("contract: T1")
    }
    /// Parses a token; refuses a wrong prefix, bad base64/JSON, a non-https URL (unless the host is
    /// loopback), wrong field lengths.
    pub fn decode(_text: &str) -> Result<JoinToken, String> {
        Err("contract: T1".into())
    }
    /// What the hub stores: SHA-256 of the secret.
    pub fn secret_hash(&self) -> [u8; 32] {
        crate::sha256(&self.secret)
    }
}
