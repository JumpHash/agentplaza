//! Content hashes: BLAKE3, 32 bytes, written as lower-case hex.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A BLAKE3 hash.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Hash(pub [u8; 32]);

impl Hash {
    pub const ZERO: Hash = Hash([0; 32]);

    /// The hash of `bytes`.
    pub fn of(bytes: &[u8]) -> Hash {
        Hash(*blake3::hash(bytes).as_bytes())
    }

    /// The hash of several parts, each length-prefixed so the parts cannot run into each other.
    pub fn of_parts(parts: &[&[u8]]) -> Hash {
        let mut hasher = blake3::Hasher::new();
        for part in parts {
            hasher.update(&(part.len() as u64).to_le_bytes());
            hasher.update(part);
        }
        Hash(*hasher.finalize().as_bytes())
    }

    pub fn hex(&self) -> String {
        hex(&self.0)
    }

    /// The first 12 hex digits, for people.
    pub fn short(&self) -> String {
        self.hex()[..12].to_string()
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0; 32]
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({})", self.short())
    }
}

impl FromStr for Hash {
    type Err = String;
    fn from_str(s: &str) -> Result<Hash, String> {
        let bytes = unhex(s)?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| format!("a hash is 64 hex digits, not {}", s.len()))?;
        Ok(Hash(arr))
    }
}

impl Serialize for Hash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for Hash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Hash, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// A hash is stored as its 32 bytes.
impl apz_store::Column for Hash {
    const TYPE: apz_store::SqlType = apz_store::SqlType::Blob;
    fn check(quoted: &str) -> Option<String> {
        Some(format!("length({quoted}) = 32"))
    }
    fn to_value(&self) -> Result<apz_store::table::Value, apz_store::StoreError> {
        Ok(apz_store::table::Value::Blob(self.0.to_vec()))
    }
    fn from_value(value: apz_store::table::Value) -> Result<Self, apz_store::StoreError> {
        match value {
            apz_store::table::Value::Blob(b) => b
                .try_into()
                .map(Hash)
                .map_err(|_| apz_store::StoreError::invalid("a hash is 32 bytes")),
            other => Err(apz_store::StoreError::invalid(format!(
                "expected a hash, found {other:?}"
            ))),
        }
    }
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}

pub fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd number of hex digits".into());
    }
    let digit = |c: u8| -> Result<u8, String> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => Err(format!("not a hex digit: {}", c as char)),
        }
    };
    s.as_bytes()
        .chunks(2)
        .map(|pair| Ok(digit(pair[0])? << 4 | digit(pair[1])?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_round_trips_through_hex_and_json() {
        let h = Hash::of(b"weft");
        assert_eq!(h.hex().parse::<Hash>().unwrap(), h);
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(serde_json::from_str::<Hash>(&json).unwrap(), h);
        assert_eq!(h.short().len(), 12);
    }

    #[test]
    fn parts_are_length_prefixed() {
        assert_ne!(
            Hash::of_parts(&[b"ab", b"c"]),
            Hash::of_parts(&[b"a", b"bc"])
        );
    }
}
