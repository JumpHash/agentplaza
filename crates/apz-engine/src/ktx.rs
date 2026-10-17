//! KTX2 textures (the kit's, written by `tools/apz-kit`): RGBA8 levels, each compressed with zstd, decoded into a
//! [`Texture`] that carries its own mip chain.

use std::io::Read;
use std::sync::Arc;

use ktx2::{Reader, SupercompressionScheme};

use crate::scene::Texture;

/// Decodes a KTX2 file of format R8G8B8A8_SRGB (43) or R8G8B8A8_UNORM (37), with supercompression scheme 2 (zstd) or
/// none (0), into a repeat-wrapped, linearly filtered texture: `rgba` is level 0, `mips` the levels below it in order,
/// `srgb` follows the format. Other formats, a bad header, a level whose decoded length is not width × height × 4, or a
/// zstd error are an `Err` with the reason.
pub fn texture_from_ktx2(bytes: &[u8]) -> Result<Texture, String> {
    let reader = Reader::new(bytes).map_err(|e| format!("KTX2 header: {e:?}"))?;
    let header = reader.header();
    let srgb = match header.format.map(|f| f.value()) {
        Some(43) => true,
        Some(37) => false,
        other => return Err(format!("KTX2 format {other:?} is not R8G8B8A8 (43 or 37)")),
    };
    let zstd = match header.supercompression_scheme {
        None => false,
        Some(SupercompressionScheme::Zstandard) => true,
        Some(other) => {
            return Err(format!(
                "KTX2 supercompression {other:?} is not zstd (2) or none (0)"
            ));
        }
    };
    let (width, height) = (header.pixel_width, header.pixel_height);
    if width == 0 || height == 0 {
        return Err(format!("KTX2 size {width}x{height} is not a 2D texture"));
    }

    let mut levels = Vec::new();
    for (i, level) in reader.levels().enumerate() {
        let level_index = i as u32;
        let (w, h) = level_size(width, height, level_index);
        let expected = w as usize * h as usize * 4;
        let data = if zstd {
            decode_zstd(level.data, expected).map_err(|e| format!("KTX2 level {i}: {e}"))?
        } else {
            level.data.to_vec()
        };
        if data.len() != expected {
            return Err(format!(
                "KTX2 level {i} is {} bytes, expected {expected} ({w}x{h} RGBA8)",
                data.len()
            ));
        }
        levels.push(data);
    }

    let mut levels = levels.into_iter();
    let level0 = levels.next().ok_or("KTX2 has no levels")?;
    let mut texture = Texture::new(width, height, level0);
    texture.wrap = true;
    texture.srgb = srgb;
    texture.mips = levels.map(Arc::new).collect();
    Ok(texture)
}

/// The size of mip `level` of a `width` × `height` texture: each side halves, down to 1.
fn level_size(width: u32, height: u32, level: u32) -> (u32, u32) {
    (
        width.checked_shr(level).unwrap_or(0).max(1),
        height.checked_shr(level).unwrap_or(0).max(1),
    )
}

/// Decompresses one zstd frame. The output is capped at `expected` + 1 bytes, so a corrupt level cannot grow past the
/// size the caller checks for.
fn decode_zstd(data: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    let mut source = data;
    let decoder =
        ruzstd::decoding::StreamingDecoder::new(&mut source).map_err(|e| format!("zstd: {e:?}"))?;
    let mut out = Vec::with_capacity(expected);
    decoder
        .take(expected as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("zstd: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRILLE: &[u8] = include_bytes!("../../../assets/hardware/kit/grille.ktx2");

    #[test]
    fn decodes_the_kit_grille_with_its_mip_chain() {
        let t = texture_from_ktx2(GRILLE).expect("grille.ktx2 decodes");
        assert_eq!((t.width, t.height), (1024, 1024));
        assert!(t.srgb);
        assert!(t.wrap);
        assert_eq!(t.mips.len(), 10);
        for (i, mip) in t.mips.iter().enumerate() {
            let side = (1024usize >> (i + 1)).max(1);
            assert_eq!(mip.len(), side * side * 4, "mip level {}", i + 1);
        }
        assert!(
            t.rgba.as_chunks::<4>().0.iter().any(|p| p[0] < 16),
            "a dark texel in level 0"
        );
        assert!(
            t.rgba.as_chunks::<4>().0.iter().any(|p| p[0] > 24),
            "a lighter texel in level 0"
        );
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(texture_from_ktx2(&GRILLE[..40]).is_err());
        assert!(texture_from_ktx2(b"definitely not a ktx2 file, just some text").is_err());
    }
}
