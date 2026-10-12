//! The hub's TLS: a self-signed certificate made on first start, its SHA-256 fingerprint (the pin
//! join tokens carry), a rustls server config, and a client config that trusts exactly the pin.

use std::sync::Arc;

/// A PEM certificate and its PEM private key.
pub struct CertPair {
    pub cert_pem: String,
    pub key_pem: String,
}

/// A self-signed certificate for `names` (DNS names or IP addresses as text).
pub fn self_signed(_names: &[String]) -> Result<CertPair, String> {
    Err("contract: T2".into())
}

/// SHA-256 of the first certificate in `cert_pem` (DER).
pub fn fingerprint(_cert_pem: &str) -> Result<[u8; 32], String> {
    Err("contract: T2".into())
}

/// A rustls server config serving `pair`.
pub fn server_config(_pair: &CertPair) -> Result<Arc<rustls::ServerConfig>, String> {
    Err("contract: T2".into())
}

/// A rustls client config that accepts a server only if its end-entity certificate's SHA-256
/// equals `pin` (no CA, no hostname check — the pin is the identity).
pub fn pinned_client_config(_pin: [u8; 32]) -> Arc<rustls::ClientConfig> {
    unimplemented!("contract: T2")
}
