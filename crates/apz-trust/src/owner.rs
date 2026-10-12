//! Owner tokens: `apzo_<base64url(32 random bytes)>`; the hub stores SHA-256 of the whole token.

pub const PREFIX: &str = "apzo_";

/// A fresh owner token.
pub fn new_token() -> String {
    format!("{PREFIX}{}", crate::b64(&crate::random::<32>()))
}

/// What the hub stores.
pub fn hash(token: &str) -> [u8; 32] {
    crate::sha256(token.trim().as_bytes())
}

/// Whether `presented` matches the stored hash (constant time).
pub fn check(presented: &str, stored: &[u8; 32]) -> bool {
    crate::ct_eq(&hash(presented), stored)
}
