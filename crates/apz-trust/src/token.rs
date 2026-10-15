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

/// The JSON wire form, with the short field names.
#[derive(Serialize, Deserialize)]
struct Wire {
    u: String,
    f: String,
    s: String,
    x: u64,
}

impl JoinToken {
    /// A fresh token for `hub_url` pinned to `cert_sha256`, valid `ttl_secs` from `now`.
    pub fn issue(hub_url: &str, cert_sha256: [u8; 32], now: u64, ttl_secs: u64) -> JoinToken {
        JoinToken {
            hub_url: hub_url.to_string(),
            cert_sha256,
            secret: crate::random::<16>(),
            expires: now + ttl_secs,
        }
    }
    /// The token as text (`apzj1.…`).
    pub fn encode(&self) -> String {
        let wire = Wire {
            u: self.hub_url.clone(),
            f: crate::b64(&self.cert_sha256),
            s: crate::b64(&self.secret),
            x: self.expires,
        };
        let json = serde_json::to_vec(&wire).expect("a token always serializes");
        format!("{PREFIX}{}", crate::b64(&json))
    }
    /// Parses a token; refuses a wrong prefix, bad base64/JSON, a non-https URL (unless the host is
    /// loopback), wrong field lengths.
    pub fn decode(text: &str) -> Result<JoinToken, String> {
        let body = text
            .strip_prefix(PREFIX)
            .ok_or_else(|| format!("not a join token (expected the {PREFIX} prefix)"))?;
        let json = crate::unb64(body).map_err(|e| format!("join token is not base64url: {e}"))?;
        let wire: Wire = serde_json::from_slice(&json)
            .map_err(|e| format!("join token JSON is malformed: {e}"))?;
        check_hub_url(&wire.u)?;
        let cert =
            crate::unb64(&wire.f).map_err(|e| format!("certificate pin is not base64url: {e}"))?;
        let cert_sha256: [u8; 32] = cert
            .try_into()
            .map_err(|_| "certificate pin must be 32 bytes (SHA-256)".to_string())?;
        let secret = crate::unb64(&wire.s).map_err(|e| format!("secret is not base64url: {e}"))?;
        let secret: [u8; 16] = secret
            .try_into()
            .map_err(|_| "secret must be 16 bytes".to_string())?;
        Ok(JoinToken {
            hub_url: wire.u,
            cert_sha256,
            secret,
            expires: wire.x,
        })
    }
    /// What the hub stores: SHA-256 of the secret.
    pub fn secret_hash(&self) -> [u8; 32] {
        crate::sha256(&self.secret)
    }
}

/// https is required, except that http is allowed for a loopback host.
fn check_hub_url(url: &str) -> Result<(), String> {
    if url.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let host_port = authority.rsplit('@').next().unwrap_or("");
        let host = if host_port.starts_with('[') {
            host_port
                .split_once(']')
                .map(|(h, _)| format!("{h}]"))
                .unwrap_or_default()
        } else {
            host_port.split(':').next().unwrap_or("").to_string()
        };
        if matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]") {
            return Ok(());
        }
        return Err("hub URL must be https (http is allowed only for loopback hosts)".into());
    }
    Err("hub URL must start with https://".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> JoinToken {
        JoinToken::issue("https://hub.example:7443", [7u8; 32], 1_000, 900)
    }

    #[test]
    fn round_trip() {
        let token = sample();
        let text = token.encode();
        assert!(text.starts_with(PREFIX));
        assert_eq!(JoinToken::decode(&text).unwrap(), token);
        assert_eq!(token.expires, 1_900);
    }

    #[test]
    fn decode_refuses_malformed_tokens() {
        // The original decode_refuses_bad_prefix: a wrong prefix is refused.
        {
            let text = sample().encode().replacen(PREFIX, "apzj2.", 1);
            assert!(JoinToken::decode(&text).unwrap_err().contains("prefix"));
        }
        // The original decode_refuses_http_unless_loopback: plain http only for loopback hubs.
        {
            let mut token = sample();
            token.hub_url = "http://hub.example:7443".into();
            assert!(JoinToken::decode(&token.encode()).is_err());
            token.hub_url = "http://127.0.0.1:7443".into();
            assert!(JoinToken::decode(&token.encode()).is_ok());
            token.hub_url = "http://[::1]:7443/x".into();
            assert!(JoinToken::decode(&token.encode()).is_ok());
        }
        // The original decode_refuses_short_secret: a secret under 16 bytes is refused.
        {
            let wire = Wire {
                u: "https://hub.example".into(),
                f: crate::b64(&[1u8; 32]),
                s: crate::b64(&[1u8; 8]),
                x: 1,
            };
            let text = format!(
                "{PREFIX}{}",
                crate::b64(&serde_json::to_vec(&wire).unwrap())
            );
            assert!(JoinToken::decode(&text).unwrap_err().contains("16 bytes"));
        }
    }
}
