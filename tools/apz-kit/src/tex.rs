//! The kit's procedural textures.

use crate::ktx2w::Image;

/// The grille: a hexagonal field of round perforations, `size` by `size` texels, tiling seamlessly. Holes are near
/// black (#05070b), the metal between them a dark grey (#1a1f2b); the hole edges are antialiased. sRGB colour.
pub fn grille(size: u32) -> Image {
    let _ = size;
    todo!()
}
