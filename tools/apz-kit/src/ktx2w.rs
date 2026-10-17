//! RGBA8 images, their mip chains, and KTX2 files with zstd supercompression (what the engine loads with the `ktx2`
//! and `ruzstd` crates).

use ruzstd::encoding::{compress_to_vec, CompressionLevel};

/// An RGBA8 image, rows top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// `base` and every level below it down to 1×1: each level halves width and height (at least 1) and averages 2×2
/// texels. With `srgb` the colour channels are averaged in linear light (decode, average, encode); alpha always
/// linearly.
pub fn mip_chain(base: &Image, srgb: bool) -> Vec<Image> {
    let mut chain = vec![base.clone()];
    while let Some(prev) = chain.last() {
        if prev.width == 1 && prev.height == 1 {
            break;
        }
        let next = halve(prev, srgb);
        chain.push(next);
    }
    chain
}

/// One level down: each output texel averages the 2×2 source texels at its position. A dimension that is already 1
/// repeats its only row or column, so the average stays correct.
fn halve(src: &Image, srgb: bool) -> Image {
    let width = (src.width / 2).max(1);
    let height = (src.height / 2).max(1);
    let texel = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y * src.width + x) * 4) as usize;
        [src.rgba[i], src.rgba[i + 1], src.rgba[i + 2], src.rgba[i + 3]]
    };
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let (x0, x1) = (2 * x, (2 * x + 1).min(src.width - 1));
            let (y0, y1) = (2 * y, (2 * y + 1).min(src.height - 1));
            let taps = [texel(x0, y0), texel(x1, y0), texel(x0, y1), texel(x1, y1)];
            for c in 0..4 {
                let v = if srgb && c < 3 {
                    let mean = taps.iter().map(|t| srgb_to_linear(t[c])).sum::<f32>() / 4.0;
                    linear_to_srgb(mean)
                } else {
                    let sum: u32 = taps.iter().map(|t| u32::from(t[c])).sum();
                    ((sum + 2) / 4) as u8
                };
                rgba.push(v);
            }
        }
    }
    Image { width, height, rgba }
}

fn srgb_to_linear(v: u8) -> f32 {
    let c = f32::from(v) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(l: f32) -> u8 {
    let c = if l <= 0.003_130_8 {
        12.92 * l
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The KTX2 file identifier.
const IDENTIFIER: [u8; 12] = [0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A];
/// Identifier, header (9 u32), index (32 bytes): where the level index starts.
const HEADER_LEN: usize = 12 + 9 * 4 + 32;
/// One level index entry: byteOffset, byteLength, uncompressedByteLength (u64 each).
const LEVEL_INDEX_ENTRY_LEN: usize = 24;
/// The DFD total-size word plus one basic descriptor block (8-byte header, 16-byte basic part, four 16-byte samples).
const DFD_LEN: usize = 4 + 88;

/// A KTX2 file of `levels` (level 0 first, as from [`mip_chain`]): format VK_FORMAT_R8G8B8A8_SRGB (43) with `srgb`,
/// else VK_FORMAT_R8G8B8A8_UNORM (37); supercompression scheme 2 (zstd), each level compressed on its own; a basic
/// data format descriptor for RGBA8; no key/value data. Level data is stored smallest level first, as KTX2 requires,
/// with the level index giving each one's byte offset, compressed length and uncompressed length.
pub fn write_ktx2(levels: &[Image], srgb: bool) -> Vec<u8> {
    let base = levels.first().expect("write_ktx2: no levels");
    let count = levels.len();
    let compressed: Vec<Vec<u8>> = levels
        .iter()
        .map(|level| compress_to_vec(level.rgba.as_slice(), CompressionLevel::Fastest))
        .collect();

    // Level data follows the DFD; the smallest level (the last index) comes first.
    let dfd_offset = HEADER_LEN + LEVEL_INDEX_ENTRY_LEN * count;
    let mut offsets = vec![0u64; count];
    let mut next = (dfd_offset + DFD_LEN) as u64;
    for (offset, data) in offsets.iter_mut().zip(&compressed).rev() {
        *offset = next;
        next += data.len() as u64;
    }

    let format: u32 = if srgb { 43 } else { 37 };
    let mut out = Vec::with_capacity(next as usize);
    out.extend_from_slice(&IDENTIFIER);
    let fields = [format, 1, base.width, base.height, 0, 0, 1, count as u32, 2];
    for field in fields {
        out.extend_from_slice(&field.to_le_bytes());
    }
    out.extend_from_slice(&(dfd_offset as u32).to_le_bytes());
    out.extend_from_slice(&(DFD_LEN as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 8]); // key/value data: offset and length, both none
    out.extend_from_slice(&[0u8; 16]); // supercompression global data: offset and length, both none
    for ((level, data), offset) in levels.iter().zip(&compressed).zip(&offsets) {
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        out.extend_from_slice(&(level.rgba.len() as u64).to_le_bytes());
    }
    out.extend_from_slice(&(DFD_LEN as u32).to_le_bytes());
    out.extend_from_slice(&data_format_descriptor(srgb));
    for data in compressed.iter().rev() {
        out.extend_from_slice(data);
    }
    out
}

/// The basic data format descriptor block for RGBA8 (88 bytes): colour model RGBSDA, BT.709 primaries, sRGB or linear
/// transfer, four samples R, G, B, A.
fn data_format_descriptor(srgb: bool) -> Vec<u8> {
    let mut b = Vec::with_capacity(88);
    b.extend_from_slice(&0u32.to_le_bytes()); // vendorId 0, descriptorType 0
    b.extend_from_slice(&2u16.to_le_bytes()); // versionNumber
    b.extend_from_slice(&88u16.to_le_bytes()); // descriptorBlockSize
    let transfer = if srgb { 2 } else { 1 };
    b.extend_from_slice(&[1, 1, transfer, 0]); // colorModel RGBSDA, colorPrimaries BT709, transferFunction, flags
    b.extend_from_slice(&[0, 0, 0, 0]); // texelBlockDimension (minus one): 1×1×1×1
    b.extend_from_slice(&[4, 0, 0, 0, 0, 0, 0, 0]); // bytesPlane: four bytes in plane 0
    let alpha_type = if srgb { 15 | 0x40 } else { 15 };
    for (i, channel_type) in [0u8, 1, 2, alpha_type].into_iter().enumerate() {
        b.extend_from_slice(&((8 * i) as u16).to_le_bytes()); // bitOffset
        b.push(7); // bitLength minus one
        b.push(channel_type);
        b.extend_from_slice(&[0, 0, 0, 0]); // samplePosition
        b.extend_from_slice(&0u32.to_le_bytes()); // sampleLower
        b.extend_from_slice(&255u32.to_le_bytes()); // sampleUpper
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn image(width: u32, height: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Image {
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&f(x, y));
            }
        }
        Image { width, height, rgba }
    }

    #[test]
    fn mip_chain_halves_to_one_texel_and_averages() {
        // texel R = x + 10y: level 1 averages 2×2 blocks, level 2 averages level 1
        let base = image(4, 2, |x, y| [(x + 10 * y) as u8, 0, 0, 255]);
        let chain = mip_chain(&base, false);
        let sizes: Vec<(u32, u32)> = chain.iter().map(|i| (i.width, i.height)).collect();
        assert_eq!(sizes, vec![(4, 2), (2, 1), (1, 1)]);
        assert_eq!(chain[1].rgba, vec![6, 0, 0, 255, 8, 0, 0, 255]);
        assert_eq!(chain[2].rgba, vec![7, 0, 0, 255]);
    }

    #[test]
    fn srgb_mips_average_in_linear_light() {
        // half black, half white: linear mean 0.5 encodes to about 188; a plain average would give 128
        let base = image(2, 2, |x, _| {
            let v = if x == 0 { 0 } else { 255 };
            [v, 0, 0, v]
        });
        let srgb = mip_chain(&base, true);
        assert!((186..=189).contains(&srgb[1].rgba[0]), "got {}", srgb[1].rgba[0]);
        assert_eq!(srgb[1].rgba[3], 128, "alpha is averaged linearly");
        let plain = mip_chain(&base, false);
        assert_eq!(plain[1].rgba[0], 128);
    }

    #[test]
    fn ktx2_round_trips_level_zero() {
        let base = image(4, 4, |x, y| [(x * 60) as u8, (y * 60) as u8, 128, 255]);
        let chain = mip_chain(&base, true);
        let bytes = write_ktx2(&chain, true);

        let reader = ktx2::Reader::new(&bytes).expect("valid KTX2 file");
        let header = reader.header();
        assert_eq!(header.format, Some(ktx2::Format::R8G8B8A8_SRGB));
        assert_eq!(header.level_count as usize, chain.len());
        assert_eq!((header.pixel_width, header.pixel_height), (4, 4));

        let level0 = reader.levels().next().expect("level 0");
        let mut decoded = Vec::new();
        ruzstd::decoding::StreamingDecoder::new(level0.data)
            .expect("zstd frame")
            .read_to_end(&mut decoded)
            .expect("zstd decode");
        assert_eq!(decoded, base.rgba);
    }

    #[test]
    fn ktx2_linear_format() {
        let base = image(2, 2, |_, _| [1, 2, 3, 4]);
        let bytes = write_ktx2(&mip_chain(&base, false), false);
        let reader = ktx2::Reader::new(&bytes).expect("valid KTX2 file");
        assert_eq!(reader.header().format, Some(ktx2::Format::R8G8B8A8_UNORM));
    }
}
