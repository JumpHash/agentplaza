//! `floor3d/props.ts`: the central plaza (paved ring, the xenon atom), its furniture (fountain,
//! market stalls), the paths with their street lamps, and the meadow's scatter (trees, flower
//! beds, benches, rocks). Every solid piece reports its ground footprint for collisions.

use glam::Vec3;

use super::scene::{Color, Obj};

/// The palette colours the props need.
#[derive(Debug, Clone, Copy)]
pub struct PropPalette {
    pub sheet: Color,
    pub rule: Color,
    pub ink3: Color,
    pub signal: Color,
    pub ok: Color,
    pub info: Color,
}

/// A ground footprint, grid units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Box2 {
    pub x0: f32,
    pub x1: f32,
    pub z0: f32,
    pub z1: f32,
}

/// A district as laid out: centre and slab half-width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Site {
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
}

/// A per-frame animation: `t` seconds since start.
pub type Anim = Box<dyn FnMut(f32) + Send>;

/// Plaza furniture: its group, footprints and animation.
pub struct Furniture {
    pub group: Obj,
    pub boxes: Vec<Box2>,
    pub animate: Anim,
}

/// `buildPlaza(palette)`: the paved ring and the atom; the group is dynamic (the atom turns).
pub fn build_plaza(palette: &PropPalette) -> (Obj, Anim) {
    let _ = palette;
    (Obj::group(), Box::new(|_| {})) // contract: P
}

/// `buildPlazaFurniture(palette, bearings)`.
pub fn build_plaza_furniture(palette: &PropPalette, bearings: &[f32]) -> Furniture {
    let _ = (palette, bearings);
    Furniture { group: Obj::group(), boxes: Vec::new(), animate: Box::new(|_| {}) } // contract: P
}

/// `buildPaths(from, palette)`: paths from the plaza to each district, with lamps.
pub fn build_paths(from: &[Vec3], palette: &PropPalette) -> Obj {
    let _ = (from, palette);
    Obj::group() // contract: P
}

/// `buildScatter(sites, palette)`: trees, beds, benches, rocks, with their footprints.
pub fn build_scatter(sites: &[Site], palette: &PropPalette) -> (Obj, Vec<Box2>) {
    let _ = (sites, palette);
    (Obj::group(), Vec::new()) // contract: P
}
