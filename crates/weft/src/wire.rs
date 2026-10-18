//! The wire protocol's shared pieces: who is calling, object bundles, and moving objects both ways.
//!
//! Clients never say what they have; they ask for what they lack, by hash, so any tier (a node's cache, the hub)
//! can answer. Objects are immutable, so every tier may keep them forever.
//!
//! - `POST wire/missing {hashes}` → `{missing}`: which the server lacks (before an upload).
//! - `PUT wire/objects/{hash}`: one object, checked against its hash.
//! - `POST wire/manifest {hash, chunks}`: a large file uploaded as content-defined chunks.
//! - `POST wire/bundle {hashes}` → a bundle: `(hash 32 | len u32 | bytes)*` of objects stored whole.
//! - `GET wire/chunks/{hash}` → `{size, chunks}`: a chunked object's chunks (`null` when stored whole).
//! - `GET wire/objects/{hash}`, with `Range`: one object, resumable.

use crate::blobs::{CHUNK_OVER, chunk_plan};
use crate::hash::Hash;
use crate::record::Actor;

/// The header apid (and only apid, behind the internal token) sets to say who a request is for.
pub const CALLER: &str = "x-weft-caller";

/// The most one bundle carries.
pub const BUNDLE_BYTES: usize = 6 << 20;

/// The caller a request names (`owner` when it names none: the plaza's own daemons and the owner's CLI).
pub fn caller(header: Option<&str>) -> Result<Actor, String> {
    match header {
        None => Ok(Actor::Owner),
        Some(h) => Actor::parse(h.trim()),
    }
}

pub fn encode_bundle(objects: &[(Hash, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(objects.iter().map(|(_, b)| b.len() + 36).sum());
    for (h, b) in objects {
        out.extend_from_slice(&h.0);
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b);
    }
    out
}

pub fn decode_bundle(bytes: &[u8]) -> Result<Vec<(Hash, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if at + 36 > bytes.len() {
            return Err("the bundle is cut short".into());
        }
        let h = Hash(bytes[at..at + 32].try_into().unwrap());
        let len = u32::from_le_bytes(bytes[at + 32..at + 36].try_into().unwrap()) as usize;
        let body = bytes
            .get(at + 36..at + 36 + len)
            .ok_or("the bundle is cut short")?;
        if Hash::of(body) != h {
            return Err(format!(
                "object {} in the bundle does not match its hash",
                h.short()
            ));
        }
        out.push((h, body.to_vec()));
        at += 36 + len;
    }
    Ok(out)
}

/// What an upload of one file sends: the file whole, or its chunks and then a manifest.
pub enum Upload {
    Whole(Hash),
    Chunked {
        hash: Hash,
        chunks: Vec<(Hash, usize, usize)>,
    },
}

pub fn upload_plan(bytes: &[u8]) -> Upload {
    let hash = Hash::of(bytes);
    if bytes.len() <= CHUNK_OVER {
        return Upload::Whole(hash);
    }
    Upload::Chunked {
        hash,
        chunks: chunk_plan(bytes)
            .into_iter()
            .map(|(o, l)| (Hash::of(&bytes[o..o + l]), o, l))
            .collect(),
    }
}

/// Parses `bytes=a-b` (inclusive; `a-` to the end) against a length.
pub fn parse_range(header: &str, len: usize) -> Option<(usize, usize)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    let start: usize = a.trim().parse().ok()?;
    let end: usize = if b.trim().is_empty() {
        len.checked_sub(1)?
    } else {
        b.trim().parse::<usize>().ok()?.min(len.checked_sub(1)?)
    };
    (start <= end && start < len).then_some((start, end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_round_trip_and_catch_damage() {
        let objs = vec![
            (Hash::of(b"a"), b"a".to_vec()),
            (Hash::of(b"hello"), b"hello".to_vec()),
        ];
        let b = encode_bundle(&objs);
        assert_eq!(decode_bundle(&b).unwrap(), objs);
        let mut bad = b.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(decode_bundle(&bad).is_err());
        assert!(decode_bundle(&b[..b.len() - 2]).is_err());
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), Some((0, 10)));
        assert_eq!(parse_range("bytes=90-", 100), Some((90, 100)));
        assert_eq!(parse_range("bytes=90-500", 100), Some((90, 100)));
        assert_eq!(parse_range("bytes=100-", 100), None);
    }
}
