//! `floor3d/toon.ts`: toon materials, the ink outline, the painted sky, the drifting clouds and
//! the wind-swayed grass tufts.

use super::scene::{Color, Mat, Material, Obj};
use super::textures::Rng;

/// `toonMat({ color, ... })`: a toon material with these values (roughness/metalness dropped).
pub fn toon_mat(material: Material) -> Mat {
    Mat::new(material)
}

/// `outline(root, minSize)`: gives every opaque, lit (toon) mesh under `root` whose bounding
/// radius is at least `min_size`, and that is not `no_outline`, its ink hull
/// (`geometry::weld_smooth(geo, 1e-3)`), set on the node's `hull`.
pub fn outline(root: &Obj, min_size: f32) {
    let _ = (root, min_size); // contract: TOON
}

/// `sky(top, horizon, ground)`: the dome (sphere r 400, 32 × 16) with `Shading::Sky`, back side,
/// no depth write, render order -1.
pub fn sky(top: Color, horizon: Color, ground: Color) -> Obj {
    let _ = (top, horizon, ground);
    Obj::group() // contract: TOON
}

/// `clouds(n, radius, rand)`: the cloud ring; the caller turns it (`rotation.y = t * 0.006`).
pub fn clouds(n: u32, radius: f32, rand: &mut Rng) -> Obj {
    let _ = (n, radius, rand);
    Obj::group() // contract: TOON
}

/// `grassTufts(count, reach, keep, rand)`: the instanced tufts (`Shading::Grass`, double-sided,
/// receiving shadows, no outline). The wind runs in the shader from the frame's time.
pub fn grass_tufts(count: u32, reach: f32, keep: &dyn Fn(f32, f32) -> bool, rand: &mut Rng) -> Obj {
    let _ = (count, reach, keep, rand);
    Obj::group() // contract: TOON
}
