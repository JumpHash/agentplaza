//! The kit's procedural textures.

use crate::ktx2w::Image;

/// Hole colour #05070b and metal colour #1a1f2b, as sRGB bytes.
const HOLE: [u8; 3] = [0x05, 0x07, 0x0b];
const METAL: [u8; 3] = [0x1a, 0x1f, 0x2b];
/// Holes per row, and rows per tile. The row count is even, so the half-pitch offset alternates and wraps.
const COLUMNS: u32 = 32;
const ROWS: u32 = 36;

/// The grille: a hexagonal field of round perforations, `size` by `size` texels, tiling seamlessly. Holes are near
/// black (#05070b), the metal between them a dark grey (#1a1f2b); the hole edges are antialiased. sRGB colour.
pub fn grille(size: u32) -> Image {
    let s = size as f32;
    let pitch = s / COLUMNS as f32;
    let row_gap = s / ROWS as f32;
    let radius = 0.33 * pitch;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        let py = y as f32 + 0.5;
        let ry = (py / row_gap).floor() as i64;
        for x in 0..size {
            let px = x as f32 + 0.5;
            // Distance to the nearest hole centre, measured the short way round the tile.
            let mut nearest = f32::INFINITY;
            for row in (ry - 2)..=(ry + 2) {
                let cy = row as f32 * row_gap;
                let offset = if row.rem_euclid(2) == 1 {
                    pitch / 2.0
                } else {
                    0.0
                };
                let column0 = ((px - offset) / pitch).round() as i64;
                for column in (column0 - 2)..=(column0 + 2) {
                    let cx = column as f32 * pitch + offset;
                    let dx = wrap(px - cx, s);
                    let dy = wrap(py - cy, s);
                    nearest = nearest.min((dx * dx + dy * dy).sqrt());
                }
            }
            // Coverage of the hole over one texel around its edge.
            let hole = (0.5 + radius - nearest).clamp(0.0, 1.0);
            for c in 0..3 {
                let v = METAL[c] as f32 + (HOLE[c] as f32 - METAL[c] as f32) * hole;
                rgba.push(v.round() as u8);
            }
            rgba.push(255);
        }
    }
    Image {
        width: size,
        height: size,
        rgba,
    }
}

/// Maps a difference into the range -period/2..period/2.
fn wrap(d: f32, period: f32) -> f32 {
    d - period * (d / period).round()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texels(img: &Image) -> impl Iterator<Item = &[u8; 4]> {
        img.rgba.as_chunks::<4>().0.iter()
    }

    #[test]
    fn grille_is_square_with_both_colours() {
        let img = grille(256);
        assert_eq!((img.width, img.height), (256, 256));
        assert_eq!(img.rgba.len(), 256 * 256 * 4);
        assert!(texels(&img).any(|p| p[..3] == HOLE), "no hole texel");
        assert!(texels(&img).any(|p| p[..3] == METAL), "no metal texel");
        assert!(texels(&img).all(|p| p[3] == 255));
    }

    #[test]
    fn grille_tiles_horizontally() {
        // The pitch is 256 / 32 = 8 texels, so shifting by one pitch maps the pattern onto itself, including across
        // the wrap at the right edge.
        let size = 256u32;
        let img = grille(size);
        let at = |x: u32, y: u32| {
            let i = ((y * size + x) * 4) as usize;
            &img.rgba[i..i + 3]
        };
        for y in 0..size {
            for x in 0..size {
                assert_eq!(at(x, y), at((x + 8) % size, y), "texel ({x}, {y})");
            }
        }
    }
}
