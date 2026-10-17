//! KTX2 textures (the kit's, written by `tools/apz-kit`): RGBA8 levels, each compressed with zstd, decoded into a
//! [`Texture`] that carries its own mip chain.

use crate::scene::Texture;

/// Decodes a KTX2 file of format R8G8B8A8_SRGB (43) or R8G8B8A8_UNORM (37), with supercompression scheme 2 (zstd) or
/// none (0), into a repeat-wrapped, linearly filtered texture: `rgba` is level 0, `mips` the levels below it in order,
/// `srgb` follows the format. Other formats, a bad header, a level whose decoded length is not width × height × 4, or a
/// zstd error are an `Err` with the reason.
pub fn texture_from_ktx2(bytes: &[u8]) -> Result<Texture, String> {
    let _ = bytes;
    todo!()
}
