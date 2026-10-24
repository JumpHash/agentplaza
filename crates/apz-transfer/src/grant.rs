//! A grant: the hub's signed permission for one node to fetch one manifest from another, for a short time.
//!
//! The hub signs (Ed25519) the scope, the manifest id, the sending and receiving nodes and the expiry. A sender checks
//! the signature with the hub's public key, the expiry, that it is the grant's sender, and that the request comes from
//! the grant's receiver.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

/// How long a grant works once issued.
pub const TTL_MS: u64 = 5 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// What the grant is for: `move:<cid>:<export>`, `base:<project>:<kind>`.
    pub scope: String,
    /// The manifest the receiver may fetch from (its id, see [`crate::Manifest::id`]).
    pub manifest: String,
    /// The node that holds the bytes.
    pub from: String,
    /// The node that may fetch them.
    pub to: String,
    /// The sender's certificate pin (SHA-256 of its certificate, hex): the receiver's TLS pins it.
    pub pin: String,
    /// When the grant stops working (unix ms).
    pub expires_ms: u64,
    /// The hub's signature over the fields above, base64url.
    #[serde(default)]
    pub sig: String,
}

/// Why a sender refuses a grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Forged,
    Expired,
    NotThisSender,
    NotTheReceiver,
    NotPublished,
}

impl Refusal {
    pub fn text(self) -> &'static str {
        match self {
            Refusal::Forged => "the grant is not signed by the hub",
            Refusal::Expired => "the grant has expired",
            Refusal::NotThisSender => "the grant is for another node to send",
            Refusal::NotTheReceiver => "the grant is for another node to receive",
            Refusal::NotPublished => "the chunk is not in the grant's manifest",
        }
    }
}

impl Grant {
    /// The hub's grant, signed with its key.
    pub fn issue(
        key: &SigningKey,
        scope: &str,
        manifest: &str,
        from: &str,
        to: &str,
        pin: &str,
        expires_ms: u64,
    ) -> Grant {
        let mut grant = Grant {
            scope: scope.to_string(),
            manifest: manifest.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            pin: pin.to_string(),
            expires_ms,
            sig: String::new(),
        };
        grant.sig = apz_trust::b64(&key.sign(&grant.message()).to_bytes());
        grant
    }

    /// The bytes the signature covers: every field but the signature, unambiguously encoded.
    fn message(&self) -> Vec<u8> {
        serde_json::to_vec(&(
            "apz-grant-1",
            &self.scope,
            &self.manifest,
            &self.from,
            &self.to,
            &self.pin,
            self.expires_ms,
        ))
        .unwrap_or_default()
    }

    /// Checks the grant for node `sender` (which holds the bytes) and a request from `receiver` at `now_ms`.
    pub fn check(
        &self,
        hub: &VerifyingKey,
        sender: &str,
        receiver: Option<&str>,
        now_ms: u64,
    ) -> Result<(), Refusal> {
        let signature = apz_trust::unb64(&self.sig)
            .ok()
            .and_then(|bytes| <[u8; 64]>::try_from(bytes.as_slice()).ok())
            .map(|bytes| Signature::from_bytes(&bytes))
            .ok_or(Refusal::Forged)?;
        hub.verify_strict(&self.message(), &signature)
            .map_err(|_| Refusal::Forged)?;
        if now_ms >= self.expires_ms {
            return Err(Refusal::Expired);
        }
        if self.from != sender {
            return Err(Refusal::NotThisSender);
        }
        if receiver != Some(self.to.as_str()) {
            return Err(Refusal::NotTheReceiver);
        }
        Ok(())
    }

    /// The grant as a request header value: its JSON, base64url.
    pub fn encode(&self) -> String {
        apz_trust::b64(&serde_json::to_vec(self).unwrap_or_default())
    }

    /// A grant from its header value.
    pub fn decode(text: &str) -> Result<Grant, String> {
        let bytes = apz_trust::unb64(text).map_err(|e| format!("the grant is not base64: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("the grant is not JSON: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIN: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn grant(key: &SigningKey, expires_ms: u64) -> Grant {
        Grant::issue(key, "base:p:build", "m1", "n1", "n2", PIN, expires_ms)
    }

    #[test]
    fn a_signed_grant_checks_for_its_sender_and_receiver() {
        let hub = key(1);
        let g = grant(&hub, 1000);
        assert_eq!(g.check(&hub.verifying_key(), "n1", Some("n2"), 999), Ok(()));
    }

    #[test]
    fn a_forged_grant_is_refused() {
        let g = grant(&key(1), 1000);
        // Another key's signature, a changed field, and a missing signature.
        assert_eq!(
            g.check(&key(2).verifying_key(), "n1", Some("n2"), 0),
            Err(Refusal::Forged)
        );
        let mut tampered = grant(&key(1), 1000);
        tampered.to = "n3".into();
        assert_eq!(
            tampered.check(&key(1).verifying_key(), "n1", Some("n3"), 0),
            Err(Refusal::Forged)
        );
        let mut bare = grant(&key(1), 1000);
        bare.sig.clear();
        assert_eq!(
            bare.check(&key(1).verifying_key(), "n1", Some("n2"), 0),
            Err(Refusal::Forged)
        );
    }

    #[test]
    fn an_expired_grant_is_refused() {
        let hub = key(1);
        let g = grant(&hub, 1000);
        assert_eq!(
            g.check(&hub.verifying_key(), "n1", Some("n2"), 1000),
            Err(Refusal::Expired)
        );
    }

    #[test]
    fn a_grant_serves_only_its_sender_and_receiver() {
        let hub = key(1);
        let g = grant(&hub, 1000);
        assert_eq!(
            g.check(&hub.verifying_key(), "n9", Some("n2"), 0),
            Err(Refusal::NotThisSender)
        );
        assert_eq!(
            g.check(&hub.verifying_key(), "n1", Some("n3"), 0),
            Err(Refusal::NotTheReceiver)
        );
        assert_eq!(
            g.check(&hub.verifying_key(), "n1", None, 0),
            Err(Refusal::NotTheReceiver)
        );
    }

    #[test]
    fn a_grant_survives_its_header_encoding() {
        let hub = key(1);
        let g = grant(&hub, 1000);
        assert_eq!(Grant::decode(&g.encode()), Ok(g));
        assert!(Grant::decode("%%%").is_err());
    }
}
