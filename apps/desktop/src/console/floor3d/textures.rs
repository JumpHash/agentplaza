//! `floor3d/textures.ts`: the floor's painted textures (meadow, paving, tiles, wood, metal,
//! polish, water) and the seeded random numbers everything procedural uses.

use std::sync::Arc;

use super::canvas::Canvas;
use super::scene::{Color, Mat, Tex};

/// mulberry32, as `textures.ts`'s `rng(seed)`: the same seed gives the same sequence as the web.
#[derive(Debug, Clone)]
pub struct Rng(pub u32);

impl Rng {
    pub fn new(seed: u32) -> Rng {
        Rng(seed)
    }
    /// The next number in [0, 1).
    pub fn next(&mut self) -> f32 {
        // contract: T — mulberry32 exactly as textures.ts.
        let _ = self.0;
        0.0
    }
}

/// `canvasTexture(key, w, h, repeat, draw)`: painted once per key and cached; repeat-wrapped,
/// sRGB, `repeat` × `repeat`. `draw` gets the canvas, its size and an `Rng` seeded from the key
/// (FNV-1a like `seedOf`).
pub fn canvas_texture(key: &str, w: u32, h: u32, repeat: f32, draw: impl FnOnce(&mut Canvas, f32, f32, &mut Rng)) -> Tex {
    let _ = (key, repeat, draw);
    Canvas::new(w, h).to_texture() // contract: T
}

pub fn grass() -> Tex { canvas_texture("grass", 4, 4, 1.0, |_, _, _, _| {}) } // contract: T
pub fn paving(tint: Color, repeat: f32) -> Tex { let _ = (tint, repeat); grass() } // contract: T
pub fn tiles(hue: f32, repeat: f32) -> Tex { let _ = (hue, repeat); grass() } // contract: T
pub fn wood() -> Tex { grass() } // contract: T
pub fn metal() -> Tex { grass() } // contract: T
pub fn polish(tint: Color) -> Tex { let _ = tint; grass() } // contract: T
pub fn water() -> Tex { grass() } // contract: T
/// `slabSide(hue)`: the material of a district slab's sides.
pub fn slab_side(hue: f32) -> Mat { let _ = hue; Mat::toon(Color::WHITE) } // contract: T

/// The cache `canvas_texture` keeps (exposed for tests).
pub fn cached() -> usize {
    let _ = Arc::<()>::default;
    0 // contract: T
}
