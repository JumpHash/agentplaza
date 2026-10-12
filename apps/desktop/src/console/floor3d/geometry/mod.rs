//! three.js's geometry generators, vertex for vertex (same order, normals and uvs), so textures
//! and the toon ramp land exactly where they do on the web. Flat-shaped ones here; the round
//! ones (sphere, cylinder, cone, torus, dodecahedron) in `round`.

use glam::Mat4;

use super::scene::{Geo, Geometry};

mod round;
pub use round::{cone, cylinder, cylinder_full, dodecahedron, sphere, sphere_part, torus};

/// `BoxGeometry(w, h, d, 1, 1, 1)`.
pub fn box_geo(w: f32, h: f32, d: f32) -> Geo {
    let _ = (w, h, d);
    Geometry::new(Vec::new(), Vec::new(), Vec::new(), Vec::new()) // contract: G1
}

/// three's `RoundedBoxGeometry(w, h, d, segments, radius)` (examples/jsm/geometries).
pub fn rounded_box(w: f32, h: f32, d: f32, segments: u32, radius: f32) -> Geo {
    let _ = (w, h, d, segments, radius);
    Geometry::new(Vec::new(), Vec::new(), Vec::new(), Vec::new()) // contract: G1
}

/// `PlaneGeometry(w, h, ws, hs)` in the XY plane, facing +Z.
pub fn plane(w: f32, h: f32, ws: u32, hs: u32) -> Geo {
    let _ = (w, h, ws, hs);
    Geometry::new(Vec::new(), Vec::new(), Vec::new(), Vec::new()) // contract: G1
}

/// `CircleGeometry(r, segments, thetaStart, thetaLength)` in the XY plane, facing +Z.
pub fn circle(r: f32, segments: u32, theta_start: f32, theta_length: f32) -> Geo {
    let _ = (r, segments, theta_start, theta_length);
    Geometry::new(Vec::new(), Vec::new(), Vec::new(), Vec::new()) // contract: G1
}

/// `RingGeometry(inner, outer, thetaSegments, phiSegments, thetaStart, thetaLength)`.
pub fn ring(inner: f32, outer: f32, theta_segments: u32, phi_segments: u32, theta_start: f32, theta_length: f32) -> Geo {
    let _ = (inner, outer, theta_segments, phi_segments, theta_start, theta_length);
    Geometry::new(Vec::new(), Vec::new(), Vec::new(), Vec::new()) // contract: G1
}

/// A non-indexed triangle soup with three's `computeVertexNormals` (flat per triangle), uvs 0.
pub fn from_triangles(positions: Vec<[f32; 3]>) -> Geo {
    let _ = positions;
    Geometry::new(Vec::new(), Vec::new(), Vec::new(), Vec::new()) // contract: G1
}

/// The geometry moved by `m` (three's `geometry.applyMatrix4` / `translate` / `rotateX` …).
pub fn transformed(geo: &Geo, m: Mat4) -> Geo {
    super::scene::merge(&[(std::sync::Arc::clone(geo), m)])
}

/// The outline hull of `toon.ts`: vertices welded within `tolerance` (three's `mergeVertices`
/// on positions only), then smooth normals (`computeVertexNormals`). Cached by geometry id.
pub fn weld_smooth(geo: &Geo, tolerance: f32) -> Geo {
    let _ = tolerance;
    std::sync::Arc::clone(geo) // contract: G1
}
