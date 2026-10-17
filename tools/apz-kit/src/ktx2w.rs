//! RGBA8 images, their mip chains, and KTX2 files with zstd supercompression (what the engine loads with the `ktx2`
//! and `ruzstd` crates).

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
    let _ = (base, srgb);
    todo!()
}

/// A KTX2 file of `levels` (level 0 first, as from [`mip_chain`]): format VK_FORMAT_R8G8B8A8_SRGB (43) with `srgb`,
/// else VK_FORMAT_R8G8B8A8_UNORM (37); supercompression scheme 2 (zstd), each level compressed on its own; a basic
/// data format descriptor for RGBA8; no key/value data. Level data is stored smallest level first, as KTX2 requires,
/// with the level index giving each one's byte offset, compressed length and uncompressed length.
pub fn write_ktx2(levels: &[Image], srgb: bool) -> Vec<u8> {
    let _ = (levels, srgb);
    todo!()
}
