//! Content hashes: BLAKE3, 32 bytes, written as lower-case hex.

pub use apz_blobs::{Hash, hex, unhex};

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
