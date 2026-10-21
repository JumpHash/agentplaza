//! Trust between the hub and its nodes, and the owner's access to the hub.
//!
//! - [`token`]: join tokens (`apzj1.…`) — the hub's URL, its certificate pin, a 128-bit secret and
//!   an expiry; the hub keeps only the secret's hash.
//! - [`node`]: a node's Ed25519 key and the signed-request scheme every node request uses.
//! - [`tls`]: the hub's self-signed certificate, its fingerprint, a rustls server config, and a
//!   client config that accepts exactly the pinned certificate.
//! - [`owner`]: owner tokens (`apzo_…`), stored hashed.
//!
//! Everything secret is compared in constant time ([`ct_eq`]).

pub mod node;
pub mod owner;
pub mod tls;
pub mod token;

/// Constant-time equality of two byte strings (length is not secret).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// SHA-256 of `bytes`.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).into()
}

/// `n` random bytes from the operating system.
pub fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut out);
    out
}

/// URL-safe base64 without padding.
pub fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Decodes URL-safe base64 without padding.
pub fn unb64(text: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The primitives every secret rests on: fresh randomness, a hash that depends on its input, a check that
    /// admits only the token it was made from, and an equality that sees a one-bit difference in any byte. A
    /// constant RNG or an always-true check would make every token, session and nonce predictable or void.
    #[test]
    fn secrets_are_random_hashed_and_checked_bit_by_bit() {
        let (a, b) = (random::<32>(), random::<32>());
        assert_ne!(a, b, "two draws of 256 bits are equal");
        assert!(
            a.iter().any(|byte| *byte != a[0]),
            "a draw is one repeated byte"
        );
        let (first, second) = (owner::new_token(), owner::new_token());
        assert_ne!(first, second);
        assert!(first.starts_with(owner::PREFIX) && first.len() > owner::PREFIX.len() + 40);
        assert_ne!(owner::hash(&first), owner::hash(&second));
        assert!(owner::check(&first, &owner::hash(&first)));
        assert!(!owner::check(&second, &owner::hash(&first)));
        assert!(!owner::check("", &owner::hash(&first)));
        for i in 0..a.len() {
            for bit in 0..8 {
                let mut changed = a;
                changed[i] ^= 1 << bit;
                assert!(!ct_eq(&a, &changed), "byte {i} bit {bit}");
            }
        }
        // Two bytes that differ in the same bit must not cancel out.
        let mut twice = a;
        twice[0] ^= 1;
        twice[1] ^= 1;
        assert!(!ct_eq(&a, &twice));
    }

    #[test]
    fn ct_eq_compares_whole_strings() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
