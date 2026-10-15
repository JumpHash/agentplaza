//! Gorilla-style compression of a run of points, for the `metric_chunk_10s` table (one row per series per
//! 10 minutes). Layout, in a bit stream (most significant bit first):
//!
//! * header: point count as a LEB128 varint (byte-aligned, before the bit stream), then the first time as 64
//!   bits and the first value as 64 bits (f64 bits);
//! * each later time as a delta-of-delta: `0` for 0; `10` + 7-bit two's complement for [-63, 64];
//!   `110` + 9 bits for [-255, 256]; `1110` + 12 bits for [-2047, 2048]; `1111` + 64 bits otherwise;
//! * each later value XOR'd with the previous one: `0` for equal; `10` + meaningful bits reusing the previous
//!   leading/trailing-zero window when it fits; `11` + 5 bits leading zeros + 6 bits length (0 means 64) +
//!   the meaningful bits otherwise.
//!
//! Regular 10 s points with slowly changing values cost about 1.5 bytes each.

use crate::Point;

#[derive(Debug, PartialEq, Eq)]
pub enum ChunkError {
    Truncated,
    BadHeader,
}

/// Compresses points (times ascending; equal times allowed).
pub fn encode(points: &[Point]) -> Vec<u8> {
    let _ = points;
    todo!()
}

/// The points of an [`encode`]d chunk, exactly (values bit for bit).
pub fn decode(bytes: &[u8]) -> Result<Vec<Point>, ChunkError> {
    let _ = bytes;
    todo!()
}
