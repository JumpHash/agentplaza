//! The vault's primitives and nothing else: XChaCha20-Poly1305 (values, data keys, wrapped keys),
//! HKDF-SHA256 (subkeys and the recovery key-encryption key), Argon2id (passphrases), HMAC-SHA256
//! (fingerprints, the index MAC and the audit chain), base32 (recovery codes), and the canonical
//! JSON that every MAC and AAD is computed over. Keys are `Zeroizing`; no key type is `Debug`.

use std::collections::BTreeMap;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use zeroize::Zeroizing;

/// A 32-byte key, wiped when dropped.
pub type Key = Zeroizing<[u8; 32]>;

pub const WRAP_INFO: &str = "apz-vault/v1/wrap";
pub const INDEX_INFO: &str = "apz-vault/v1/index";
pub const AUDIT_INFO: &str = "apz-vault/v1/audit";
pub const RECOVERY_INFO: &str = "apz-vault/v1/recovery";

/// Argon2id for passphrase slots: 256 MiB, three passes, one lane. Stored in each slot.
pub const KDF_MEMORY_KIB: u32 = 256 * 1024;
pub const KDF_PASSES: u32 = 3;
pub const KDF_LANES: u32 = 1;

/// The one failure a decryption reports. It says nothing about why, so it is no oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opaque;

/// `n` bytes from the operating system's CSPRNG.
pub fn random_vec(n: usize) -> Result<Vec<u8>, String> {
    let mut bytes = vec![0u8; n];
    apz_os::random::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// A fresh random 32-byte key.
pub fn random_key() -> Result<Key, String> {
    let mut key = Zeroizing::new([0u8; 32]);
    apz_os::random::fill(key.as_mut()).map_err(|e| e.to_string())?;
    Ok(key)
}

/// A 32-byte key from `bytes`, when it is exactly 32 bytes long.
pub fn key_from(bytes: &[u8]) -> Option<Key> {
    let array: [u8; 32] = bytes.try_into().ok()?;
    Some(Zeroizing::new(array))
}

/// A subkey of the root key: HKDF-SHA256 with the vault id as salt and `info` as the label.
pub fn subkey(root: &[u8; 32], vault_id: &str, info: &str) -> Key {
    let hk = Hkdf::<Sha256>::new(Some(vault_id.as_bytes()), root);
    let mut out = Zeroizing::new([0u8; 32]);
    // 32 bytes is far below HKDF-SHA256's limit, so this cannot fail.
    let _ = hk.expand(info.as_bytes(), out.as_mut());
    out
}

/// Argon2id of a passphrase under `salt` with the given cost.
pub fn argon2id(
    passphrase: &[u8],
    salt: &[u8],
    memory_kib: u32,
    passes: u32,
    lanes: u32,
) -> Result<Key, String> {
    let params = Params::new(memory_kib, passes, lanes, Some(32)).map_err(|e| e.to_string())?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(passphrase, salt, out.as_mut())
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn cipher(key: &[u8; 32]) -> Result<XChaCha20Poly1305, String> {
    XChaCha20Poly1305::new_from_slice(key).map_err(|e| e.to_string())
}

/// Encrypts `plaintext` under `key` with a fresh 24-byte nonce. Returns `(nonce, ciphertext)`.
pub fn seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let nonce = random_vec(24)?;
    let ct = cipher(key)?
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| "encryption failed".to_string())?;
    Ok((nonce, ct))
}

/// Decrypts and authenticates. Any change to the key, nonce, AAD or ciphertext fails.
pub fn open(
    key: &[u8; 32],
    nonce: &[u8],
    aad: &[u8],
    ct: &[u8],
) -> Result<Zeroizing<Vec<u8>>, Opaque> {
    if nonce.len() != 24 {
        return Err(Opaque);
    }
    let cipher = cipher(key).map_err(|_| Opaque)?;
    cipher
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
        .map(Zeroizing::new)
        .map_err(|_| Opaque)
}

/// HMAC-SHA256 over the concatenation of `parts`.
pub fn mac(key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    // HMAC accepts keys of any length, so this cannot fail.
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC takes a key of any length");
    for part in parts {
        m.update(part);
    }
    m.finalize().into_bytes().to_vec()
}

/// `mac` as lowercase hex.
pub fn mac_hex(key: &[u8], parts: &[&[u8]]) -> String {
    hex(&mac(key, parts))
}

/// Whether `expected_hex` is the MAC of `parts` (compared in constant time).
pub fn mac_matches(key: &[u8], parts: &[&[u8]], expected_hex: &str) -> bool {
    let Some(expected) = unhex(expected_hex) else {
        return false;
    };
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC takes a key of any length");
    for part in parts {
        m.update(part);
    }
    m.verify_slice(&expected).is_ok()
}

/// The fingerprint of a value: the first 8 hex characters of its keyed MAC. It shows that two
/// entries hold the same value without revealing it.
pub fn fingerprint(index: &[u8; 32], value: &[u8]) -> String {
    mac_hex(index, &[value])[..8].to_string()
}

/// Additional authenticated data: `label ‖ 0 ‖ f1 ‖ 0 ‖ f2 …`, as in `docs/design/vault.md`.
pub fn aad(label: &str, fields: &[&str]) -> Vec<u8> {
    let mut out = label.as_bytes().to_vec();
    out.push(0);
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.push(0);
        }
        out.extend_from_slice(field.as_bytes());
    }
    out
}

/// JSON with object keys sorted at every level and no whitespace: the form every MAC covers.
pub fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            out.push('{');
            for (i, (key, item)) in sorted.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.to_string()).to_string());
                out.push(':');
                write_canonical(item, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 base32, upper case, no padding.
pub fn base32(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// The inverse of `base32`, case-insensitive. `None` on a character outside the alphabet.
pub fn unbase32(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let value = ALPHABET.iter().position(|&a| a == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [7; 32];

    #[test]
    fn round_trip_works() {
        let aad = aad("apz-vault/v1/value", &["sec_x", "1", "ANTHROPIC_API_KEY"]);
        let (nonce, ct) = seal(&KEY, &aad, b"sk-test-value").expect("sealed");
        let plain = open(&KEY, &nonce, &aad, &ct).expect("opened");
        assert_eq!(plain.as_slice(), b"sk-test-value");
    }

    #[test]
    fn flipped_byte_in_ciphertext_nonce_dek_or_slot_fails() {
        let aad = aad("apz-vault/v1/dek", &["sec_x", "1"]);
        let (nonce, ct) = seal(&KEY, &aad, &[9u8; 32]).expect("sealed");
        for i in 0..ct.len() {
            let mut bad = ct.clone();
            bad[i] ^= 1;
            assert!(
                open(&KEY, &nonce, &aad, &bad).is_err(),
                "ciphertext byte {i}"
            );
        }
        for i in 0..nonce.len() {
            let mut bad = nonce.clone();
            bad[i] ^= 1;
            assert!(open(&KEY, &bad, &aad, &ct).is_err(), "nonce byte {i}");
        }
        // The key is the DEK here, or the wrapping key or a slot's KEK: any flipped key byte fails.
        let mut bad_key = KEY;
        bad_key[0] ^= 1;
        assert!(open(&bad_key, &nonce, &aad, &ct).is_err(), "key byte");
    }

    #[test]
    fn swapped_ciphertexts_fail() {
        // A value sealed for entry A, version 1, cannot open as entry B or as version 2.
        let a = aad(
            "apz-vault/v1/value",
            &["sec_a", "1", "A_KEY", "{\"kind\":\"plaza\"}", "[\"hf\"]"],
        );
        let b = aad(
            "apz-vault/v1/value",
            &["sec_b", "1", "A_KEY", "{\"kind\":\"plaza\"}", "[\"hf\"]"],
        );
        let v2 = aad(
            "apz-vault/v1/value",
            &["sec_a", "2", "A_KEY", "{\"kind\":\"plaza\"}", "[\"hf\"]"],
        );
        let scope = aad(
            "apz-vault/v1/value",
            &["sec_a", "1", "A_KEY", "{\"kind\":\"nodes\"}", "[\"hf\"]"],
        );
        let (nonce, ct) = seal(&KEY, &a, b"value").expect("sealed");
        assert!(open(&KEY, &nonce, &b, &ct).is_err());
        assert!(open(&KEY, &nonce, &v2, &ct).is_err());
        assert!(open(&KEY, &nonce, &scope, &ct).is_err());
        assert!(open(&KEY, &nonce, &a, &ct).is_ok());
    }

    #[test]
    fn canonical_json_sorts_keys_at_every_level() {
        let value: Value =
            serde_json::from_str(r#"{"b":[{"z":1,"a":2}],"a":{"y":true,"x":null}}"#).expect("json");
        assert_eq!(
            canonical(&value),
            r#"{"a":{"x":null,"y":true},"b":[{"a":2,"z":1}]}"#
        );
    }

    #[test]
    fn base32_round_trips_and_a_recovery_code_is_52_characters() {
        let code = [0xAB; 32];
        let text = base32(&code);
        assert_eq!(text.len(), 52);
        assert_eq!(unbase32(&text.to_lowercase()).expect("decoded")[..32], code);
        assert!(unbase32("!").is_none());
    }
}
