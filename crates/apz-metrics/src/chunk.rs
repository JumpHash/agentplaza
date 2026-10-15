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
    let mut out = Vec::new();
    write_varint(points.len() as u64, &mut out);
    let Some(&(first_time, first_value)) = points.first() else {
        return out;
    };
    let mut bits = BitWriter::default();
    bits.write_bits(first_time, 64);
    bits.write_bits(first_value.to_bits(), 64);
    let mut previous_time = first_time;
    let mut previous_delta = 0i64;
    let mut previous_value = first_value.to_bits();
    let mut window = None;
    for &(time, value) in &points[1..] {
        let delta = time.wrapping_sub(previous_time) as i64;
        write_delta_of_delta(&mut bits, delta.wrapping_sub(previous_delta));
        let value_bits = value.to_bits();
        write_value_xor(&mut bits, value_bits ^ previous_value, &mut window);
        previous_time = time;
        previous_delta = delta;
        previous_value = value_bits;
    }
    out.extend_from_slice(&bits.bytes);
    out
}

/// The points of an [`encode`]d chunk, exactly (values bit for bit).
pub fn decode(bytes: &[u8]) -> Result<Vec<Point>, ChunkError> {
    let (count, header_length) = read_varint(bytes)?;
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut bits = BitReader::new(&bytes[header_length..]);
    let first_time = bits.read_bits(64)?;
    let first_value = bits.read_bits(64)?;
    // Every later point takes at least two bits, so a count beyond that is a lie; do not reserve for it.
    let capacity = count.min(bytes.len() as u64 * 8) as usize;
    let mut points = Vec::with_capacity(capacity);
    points.push((first_time, f64::from_bits(first_value)));
    let mut previous_time = first_time;
    let mut previous_delta = 0i64;
    let mut previous_value = first_value;
    let mut window = None;
    for _ in 1..count {
        let delta = previous_delta.wrapping_add(read_delta_of_delta(&mut bits)?);
        let time = previous_time.wrapping_add(delta as u64);
        let value = previous_value ^ read_value_xor(&mut bits, &mut window)?;
        points.push((time, f64::from_bits(value)));
        previous_time = time;
        previous_delta = delta;
        previous_value = value;
    }
    Ok(points)
}

/// Appends `value` as an unsigned LEB128 varint.
fn write_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Reads a varint from the front of `bytes`, returning the value and how many bytes it took.
fn read_varint(bytes: &[u8]) -> Result<(u64, usize), ChunkError> {
    let mut value = 0u64;
    for (index, &byte) in bytes.iter().enumerate() {
        let shift = 7 * index as u32;
        if shift >= 64 {
            return Err(ChunkError::BadHeader);
        }
        value |= u64::from(byte & 0x7f) << shift;
        if (byte & 0x80) == 0 {
            return Ok((value, index + 1));
        }
    }
    Err(ChunkError::Truncated)
}

/// Packs bits into bytes, most significant bit first.
#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bit_length: usize,
}

impl BitWriter {
    fn write_bit(&mut self, bit: bool) {
        if self.bit_length % 8 == 0 {
            self.bytes.push(0);
        }
        if bit {
            let last = self.bytes.len() - 1;
            self.bytes[last] |= 0x80 >> (self.bit_length % 8);
        }
        self.bit_length += 1;
    }

    /// Writes the low `count` bits of `value`, most significant first. `count` is at most 64.
    fn write_bits(&mut self, value: u64, count: u32) {
        for shift in (0..count).rev() {
            self.write_bit(((value >> shift) & 1) == 1);
        }
    }
}

/// Reads bits from a byte slice, most significant bit first. Running past the end is `Truncated`.
struct BitReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        BitReader { bytes, position: 0 }
    }

    fn read_bit(&mut self) -> Result<bool, ChunkError> {
        let byte = *self
            .bytes
            .get(self.position / 8)
            .ok_or(ChunkError::Truncated)?;
        let bit = ((byte >> (7 - self.position % 8)) & 1) == 1;
        self.position += 1;
        Ok(bit)
    }

    /// Reads `count` bits (at most 64) as an unsigned number, most significant first.
    fn read_bits(&mut self, count: u32) -> Result<u64, ChunkError> {
        let mut value = 0u64;
        for _ in 0..count {
            value = (value << 1) | u64::from(self.read_bit()?);
        }
        Ok(value)
    }
}

/// Writes a delta-of-delta in the smallest class that holds it.
fn write_delta_of_delta(bits: &mut BitWriter, delta_of_delta: i64) {
    if delta_of_delta == 0 {
        bits.write_bit(false);
    } else if (-63..=64).contains(&delta_of_delta) {
        bits.write_bits(0b10, 2);
        bits.write_bits(delta_of_delta as u64, 7);
    } else if (-255..=256).contains(&delta_of_delta) {
        bits.write_bits(0b110, 3);
        bits.write_bits(delta_of_delta as u64, 9);
    } else if (-2047..=2048).contains(&delta_of_delta) {
        bits.write_bits(0b1110, 4);
        bits.write_bits(delta_of_delta as u64, 12);
    } else {
        bits.write_bits(0b1111, 4);
        bits.write_bits(delta_of_delta as u64, 64);
    }
}

fn read_delta_of_delta(bits: &mut BitReader<'_>) -> Result<i64, ChunkError> {
    if !bits.read_bit()? {
        return Ok(0);
    }
    if !bits.read_bit()? {
        return Ok(signed_from_bits(bits.read_bits(7)?, 7));
    }
    if !bits.read_bit()? {
        return Ok(signed_from_bits(bits.read_bits(9)?, 9));
    }
    if !bits.read_bit()? {
        return Ok(signed_from_bits(bits.read_bits(12)?, 12));
    }
    Ok(bits.read_bits(64)? as i64)
}

/// Reads a two's complement number of `width` bits. The pattern for the most negative value stands for the
/// positive bound instead: for 7 bits, `1000000` reads as 64, which is how the class reaches [-63, 64].
fn signed_from_bits(pattern: u64, width: u32) -> i64 {
    let half = 1u64 << (width - 1);
    if pattern == half {
        half as i64
    } else if pattern < half {
        pattern as i64
    } else {
        pattern as i64 - (1i64 << width)
    }
}

/// Writes one value XOR'd with its predecessor. `window` holds the leading and trailing zero counts of the
/// last explicitly written meaningful bits, so later values can reuse that window.
fn write_value_xor(bits: &mut BitWriter, xor: u64, window: &mut Option<(u32, u32)>) {
    if xor == 0 {
        bits.write_bit(false);
        return;
    }
    let leading = xor.leading_zeros();
    let trailing = xor.trailing_zeros();
    if let Some((window_leading, window_trailing)) = *window {
        if leading >= window_leading && trailing >= window_trailing {
            bits.write_bits(0b10, 2);
            bits.write_bits(
                xor >> window_trailing,
                64 - window_leading - window_trailing,
            );
            return;
        }
    }
    let stored_leading = leading.min(31);
    let significant = 64 - stored_leading - trailing;
    bits.write_bits(0b11, 2);
    bits.write_bits(u64::from(stored_leading), 5);
    // A length of 64 is stored as 0, since 64 does not fit in 6 bits.
    bits.write_bits(u64::from(significant % 64), 6);
    bits.write_bits(xor >> trailing, significant);
    *window = Some((stored_leading, trailing));
}

fn read_value_xor(
    bits: &mut BitReader<'_>,
    window: &mut Option<(u32, u32)>,
) -> Result<u64, ChunkError> {
    if !bits.read_bit()? {
        return Ok(0);
    }
    if !bits.read_bit()? {
        let (leading, trailing) = (*window).ok_or(ChunkError::BadHeader)?;
        return Ok(bits.read_bits(64 - leading - trailing)? << trailing);
    }
    let leading = bits.read_bits(5)? as u32;
    let mut significant = bits.read_bits(6)? as u32;
    if significant == 0 {
        significant = 64;
    }
    if leading + significant > 64 {
        return Err(ChunkError::BadHeader);
    }
    let trailing = 64 - leading - significant;
    *window = Some((leading, trailing));
    Ok(bits.read_bits(significant)? << trailing)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The points with values as raw bits, so NaN payloads and signed zeros compare exactly.
    fn as_bits(points: &[Point]) -> Vec<(u64, u64)> {
        points
            .iter()
            .map(|&(time, value)| (time, value.to_bits()))
            .collect()
    }

    #[test]
    fn sixty_regular_points_round_trip_in_under_two_and_a_half_bytes_each() {
        let points: Vec<Point> = (0..60u64)
            .map(|i| (1_700_000_000_000 + i * 10_000, 20.0 + (i / 4) as f64 * 0.5))
            .collect();
        let bytes = encode(&points);
        assert!(
            bytes.len() as f64 / 60.0 < 2.5,
            "{} bytes for 60 points",
            bytes.len()
        );
        assert_eq!(as_bits(&decode(&bytes).unwrap()), as_bits(&points));
    }

    #[test]
    fn irregular_times_and_special_floats_round_trip_bit_for_bit() {
        // The deltas sit on the edges of the delta-of-delta classes (64, -63, 256, -255, 2048, -2047), then a
        // repeated time, a huge jump and the largest time.
        let times = [
            1_000_000u64,
            1_000_064,
            1_000_065,
            1_000_322,
            1_000_324,
            1_002_374,
            1_002_377,
            1_004_429,
            1_004_429,
            1 << 40,
            (1 << 40) + 1,
            u64::MAX,
        ];
        let values = [
            0.0,
            -0.0,
            f64::NAN,
            f64::from_bits(0x7ff0_0000_0000_0001),
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MIN_POSITIVE,
            5e-324,
            f64::MAX,
            -1.5,
            1.5,
            -0.0,
        ];
        let points: Vec<Point> = times.iter().copied().zip(values.iter().copied()).collect();
        assert_eq!(
            as_bits(&decode(&encode(&points)).unwrap()),
            as_bits(&points)
        );

        // Pseudo-random times and values: the second kind shares its high bits, so the window path is taken.
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut time = 0u64;
        let random: Vec<Point> = (0..300)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                time += state >> 44;
                let value_bits = if state & 1 == 0 {
                    state.rotate_left(17)
                } else {
                    0x4059_0000_0000_0000 | (state >> 40)
                };
                (time, f64::from_bits(value_bits))
            })
            .collect();
        assert_eq!(
            as_bits(&decode(&encode(&random)).unwrap()),
            as_bits(&random)
        );
    }

    #[test]
    fn a_truncated_chunk_fails_with_truncated_at_every_cut() {
        let points: Vec<Point> = vec![
            (1000, 1.0),
            (1010, 1.5),
            (1020, 1.5),
            (1031, -2.25),
            (1040, 3.0),
            (1100, 3.0),
        ];
        let bytes = encode(&points);
        for length in 0..bytes.len() {
            assert_eq!(
                decode(&bytes[..length]),
                Err(ChunkError::Truncated),
                "chunk cut to {length} bytes"
            );
        }
    }
}
