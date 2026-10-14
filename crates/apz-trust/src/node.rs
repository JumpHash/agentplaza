//! A node's identity and the signed-request scheme.
//!
//! Every node request carries headers [`H_NODE`], [`H_TS`], [`H_NONCE`] and [`H_SIG`], where the
//! signature is Ed25519 over `"{method}\n{path_and_query}\n{ts}\n{nonce}\n{hex(sha256(body))}"`.
//! The hub accepts a timestamp within [`MAX_SKEW_SECS`] and remembers nonces for [`NONCE_TTL_SECS`].

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

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
        NodeKey {
            key: SigningKey::generate(&mut rand_core::OsRng),
        }
    }
    /// The 32-byte secret seed, for storage.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.key.to_bytes()
    }
    pub fn from_bytes(seed: &[u8]) -> Result<NodeKey, String> {
        let seed: [u8; 32] = seed
            .try_into()
            .map_err(|_| "node key must be 32 bytes".to_string())?;
        Ok(NodeKey {
            key: SigningKey::from_bytes(&seed),
        })
    }
    /// The public key, base64url (what the hub stores).
    pub fn public_b64(&self) -> String {
        crate::b64(self.key.verifying_key().as_bytes())
    }
    /// The four headers for one request, `nonce` 16 random bytes base64url, `ts` unix seconds.
    pub fn sign(
        &self,
        node_id: &str,
        method: &str,
        path: &str,
        ts: u64,
        body: &[u8],
    ) -> Vec<(&'static str, String)> {
        let nonce = crate::b64(&crate::random::<16>());
        let message = signed_message(method, path, ts, &nonce, body);
        let signature = self.key.sign(message.as_bytes());
        vec![
            (H_NODE, node_id.to_string()),
            (H_TS, ts.to_string()),
            (H_NONCE, nonce),
            (H_SIG, crate::b64(&signature.to_bytes())),
        ]
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
pub fn verify(
    public_b64: &str,
    method: &str,
    path: &str,
    ts: u64,
    nonce: &str,
    body: &[u8],
    sig_b64: &str,
) -> bool {
    let check = || -> Option<bool> {
        let public: [u8; 32] = crate::unb64(public_b64).ok()?.try_into().ok()?;
        let key = VerifyingKey::from_bytes(&public).ok()?;
        let sig: [u8; 64] = crate::unb64(sig_b64).ok()?.try_into().ok()?;
        let signature = Signature::from_bytes(&sig);
        let message = signed_message(method, path, ts, nonce, body);
        Some(key.verify_strict(message.as_bytes(), &signature).is_ok())
    };
    check().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Signed {
        key: NodeKey,
        headers: Vec<(&'static str, String)>,
    }

    fn signed(body: &[u8]) -> Signed {
        let key = NodeKey::generate();
        let headers = key.sign("node-1", "POST", "/v1/node/heartbeat", 1_700_000_000, body);
        Signed { key, headers }
    }

    fn header(headers: &[(&'static str, String)], name: &str) -> String {
        headers.iter().find(|(n, _)| *n == name).unwrap().1.clone()
    }

    fn check(s: &Signed, method: &str, path: &str, ts: u64, body: &[u8]) -> bool {
        verify(
            &s.key.public_b64(),
            method,
            path,
            ts,
            &header(&s.headers, H_NONCE),
            body,
            &header(&s.headers, H_SIG),
        )
    }

    #[test]
    fn signed_request_verifies() {
        let s = signed(b"{}");
        assert_eq!(header(&s.headers, H_NODE), "node-1");
        assert_eq!(header(&s.headers, H_TS), "1700000000");
        assert!(check(
            &s,
            "POST",
            "/v1/node/heartbeat",
            1_700_000_000,
            b"{}"
        ));
    }

    #[test]
    fn changed_parts_fail() {
        let s = signed(b"{}");
        let ts = 1_700_000_000;
        assert!(!check(&s, "POST", "/v1/node/heartbeat", ts, b"{ }"));
        assert!(!check(&s, "POST", "/v1/other", ts, b"{}"));
        assert!(!check(&s, "POST", "/v1/node/heartbeat", ts + 1, b"{}"));
        assert!(!verify(
            &s.key.public_b64(),
            "POST",
            "/v1/node/heartbeat",
            ts,
            &crate::b64(&[9u8; 16]),
            b"{}",
            &header(&s.headers, H_SIG),
        ));
    }

    #[test]
    fn other_key_fails() {
        let s = signed(b"{}");
        let other = NodeKey::generate();
        assert!(!verify(
            &other.public_b64(),
            "POST",
            "/v1/node/heartbeat",
            1_700_000_000,
            &header(&s.headers, H_NONCE),
            b"{}",
            &header(&s.headers, H_SIG),
        ));
    }

    #[test]
    fn seed_round_trip_keeps_public_key() {
        let key = NodeKey::generate();
        let restored = NodeKey::from_bytes(&key.to_bytes()).unwrap();
        assert_eq!(key.public_b64(), restored.public_b64());
        assert!(NodeKey::from_bytes(&[0u8; 5]).is_err());
    }
}
