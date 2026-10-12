//! three.js's round geometries, vertex for vertex (same order, normals and uvs).

use super::super::scene::{Geo, Geometry};

/// `SphereGeometry(r, widthSegments, heightSegments)`.
pub fn sphere(r: f32, width_segments: u32, height_segments: u32) -> Geo {
    sphere_part(r, width_segments, height_segments, 0.0, std::f32::consts::TAU, 0.0, std::f32::consts::PI)
}

/// `SphereGeometry(r, ws, hs, phiStart, phiLength, thetaStart, thetaLength)`.
pub fn sphere_part(r: f32, ws: u32, hs: u32, phi_start: f32, phi_length: f32, theta_start: f32, theta_length: f32) -> Geo {
    let _ = (r, ws, hs, phi_start, phi_length, theta_start, theta_length);
    Geometry::new(Vec::new(), Vec::new(), Vec::new(), Vec::new()) // contract: G2
}

/// `CylinderGeometry(rTop, rBottom, h, radialSegments)` (one height segment, capped).
pub fn cylinder(r_top: f32, r_bottom: f32, h: f32, radial: u32) -> Geo {
    cylinder_full(r_top, r_bottom, h, radial, 1, false, 0.0, std::f32::consts::TAU)
}

/// `CylinderGeometry` with every parameter.
#[allow(clippy::too_many_arguments)]
pub fn cylinder_full(r_top: f32, r_bottom: f32, h: f32, radial: u32, height_segments: u32, open_ended: bool, theta_start: f32, theta_length: f32) -> Geo {
    let _ = (r_top, r_bottom, h, radial, height_segments, open_ended, theta_start, theta_length);
    Geometry::new(Vec::new(), Vec::new(), Vec::new(), Vec::new()) // contract: G2
}

/// `ConeGeometry(r, h, radialSegments)`: a cylinder with a zero top radius.
pub fn cone(r: f32, h: f32, radial: u32) -> Geo {
    cylinder_full(0.0, r, h, radial, 1, false, 0.0, std::f32::consts::TAU)
}

/// `TorusGeometry(r, tube, radialSegments, tubularSegments, arc)`.
pub fn torus(r: f32, tube: f32, radial: u32, tubular: u32, arc: f32) -> Geo {
    let _ = (r, tube, radial, tubular, arc);
    Geometry::new(Vec::new(), Vec::new(), Vec::new(), Vec::new()) // contract: G2
}

/// `DodecahedronGeometry(r, detail)` (a PolyhedronGeometry: flat normals, non-indexed).
pub fn dodecahedron(r: f32, detail: u32) -> Geo {
    let _ = (r, detail);
    Geometry::new(Vec::new(), Vec::new(), Vec::new(), Vec::new()) // contract: G2
}
