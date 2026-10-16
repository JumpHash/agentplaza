//! Robot part geometry, as `robots/geometry.ts`. The geo types are the hardware parts' (the same
//! builders, `geometry/` and the hardware's shape extrusion), with the optional segment counts the robots
//! need, so the five ported designs keep their mesh resolution. Parts are baked into their bone's frame.

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::Arc;

use glam::{EulerRot, Mat4, Quat, Vec3};

use super::super::geometry::{
    box_geo, circle, cone, cylinder_full, from_triangles, plane, ring, rounded_box, sphere_part,
    torus, transformed, weld_smooth,
};
use super::super::hardware::shape_geometry;
use super::super::scene::{Geo as Mesh, Geometry};
use super::types::{Eye, Geo, Part, Scale};

/// One part's geometry with the vertex colour and material slot it draws with (see `join`).
pub struct Piece {
    pub geo: Mesh,
    /// Linear rgb, multiplied into the material's colour.
    pub colour: [f32; 3],
    /// 0 fixed, 1 shell, 2 shell2, 3 panel, 4 glass (see `scene::RobotHues`).
    pub slot: u8,
}

impl Piece {
    /// A piece with no colour change (white, slot 0).
    pub fn plain(geo: Mesh) -> Piece {
        Piece {
            geo,
            colour: [1.0; 3],
            slot: 0,
        }
    }
}

/// Lays a geometry flat, facing up (three's `rotateX(-PI/2)`), as the hardware lays planes and rings.
fn flat() -> Mat4 {
    Mat4::from_rotation_x(-FRAC_PI_2)
}

/// A geo as its geometry: the hardware's builders, with the robots' segment defaults (a validated geo
/// has its required numbers, so the zeros are never read).
fn make(g: &Geo) -> Mesh {
    let v = |k: Option<f32>| k.unwrap_or(0.0);
    let seg = |i: usize, default: u32| {
        g.seg
            .as_ref()
            .and_then(|s| s.get(i).copied())
            .unwrap_or(default)
    };
    let (w, h, d, r) = (v(g.w), v(g.h), v(g.d), v(g.r));
    match g.kind.as_str() {
        "box" => box_geo(w, h, d),
        "rbox" => rounded_box(w, h, d, seg(0, 3), r),
        "cyl" => cylinder_full(v(g.rt), v(g.rb), h, seg(0, 24), 1, false, 0.0, TAU),
        "cone" => cone(r, h, seg(0, 24)),
        "sphere" => {
            let theta = if g.half.as_deref() == Some("top") {
                FRAC_PI_2
            } else {
                PI
            };
            sphere_part(r, seg(0, 24), seg(1, 16), 0.0, TAU, 0.0, theta)
        }
        "torus" => torus(
            r,
            v(g.tube),
            seg(0, 10),
            seg(1, 32),
            g.arc.unwrap_or(360.0).to_radians(),
        ),
        "plane" => transformed(&plane(w, d, 1, 1), flat()),
        "ring" => transformed(&ring(v(g.ri), v(g.ro), 48, 1, 0.0, TAU), flat()),
        "shape" => shape_geometry(g.points.as_deref().unwrap_or(&[]), h),
        _ => box_geo(w, h, d), // a type validate.rs has refused
    }
}

/// A part's geometry in its bone's frame: its geo moved by `at`, `rot` (degrees, XYZ Euler, as
/// Object3D) and `scale`. Also the source's bounding radius, which the ink line reads.
pub fn part_geometry(p: &Part) -> (Mesh, f32) {
    let src = make(&p.geo);
    let radius = src.radius;
    let at = p.at.unwrap_or([0.0; 3]);
    let rot = p.rot.unwrap_or([0.0; 3]);
    let scale = match p.scale.as_ref() {
        None => [1.0; 3],
        Some(Scale::Uniform(s)) => [*s; 3],
        Some(Scale::Xyz(v)) => *v,
    };
    let rotation = Quat::from_euler(
        EulerRot::XYZ,
        rot[0].to_radians(),
        rot[1].to_radians(),
        rot[2].to_radians(),
    );
    let m = Mat4::from_scale_rotation_translation(Vec3::from(scale), rotation, Vec3::from(at));
    (transformed(&src, m), radius)
}

fn scale_y(s: f32) -> Mat4 {
    Mat4::from_scale(Vec3::new(1.0, s, 1.0))
}

/// The heart of the eye shapes: the web's two cubic curves (eight steps each), fanned from the centre.
fn heart(k: f32, stretch: f32) -> Mesh {
    let curve = |p: [[f32; 2]; 4], out: &mut Vec<[f32; 2]>| {
        for i in 1..=8 {
            let t = i as f32 / 8.0;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            out.push(
                [0, 1]
                    .map(|axis| a * p[0][axis] + b * p[1][axis] + c * p[2][axis] + d * p[3][axis]),
            );
        }
    };
    let mut outline: Vec<[f32; 2]> = vec![[0.0, -0.8 * k]];
    curve(
        [
            [0.0, -0.8 * k],
            [-1.2 * k, -0.1 * k],
            [-0.6 * k, 1.0 * k],
            [0.0, 0.5 * k],
        ],
        &mut outline,
    );
    curve(
        [
            [0.0, 0.5 * k],
            [0.6 * k, 1.0 * k],
            [1.2 * k, -0.1 * k],
            [0.0, -0.8 * k],
        ],
        &mut outline,
    );
    outline.pop(); // the last point is the first
    let n = outline.len();
    let mut tris: Vec<[f32; 3]> = Vec::with_capacity(n * 3);
    for i in 0..n {
        let (b, c) = (outline[i], outline[(i + 1) % n]);
        // Counter-clockwise seen from +z (the eye faces +z), so the normals point at the viewer.
        let cross = b[0] * c[1] - b[1] * c[0];
        let (b, c) = if cross < 0.0 { (c, b) } else { (b, c) };
        tris.extend_from_slice(&[
            [0.0, 0.0, 0.0],
            [b[0], b[1] * stretch, 0.0],
            [c[0], c[1] * stretch, 0.0],
        ]);
    }
    from_triangles(tris)
}

/// An eye's shape, facing +z at the origin, in the pupil's size (`r`).
fn eye_shape(shape: &str, r: f32, stretch: f32) -> Mesh {
    match shape {
        "bar" => plane(r * 2.0, r * 0.5 * stretch, 1, 1),
        "visor" => rounded_box(r * 3.0, r * 0.9 * stretch, r * 0.2, 2, r * 0.3),
        "ring" => transformed(&ring(r * 0.55, r, 28, 1, 0.0, TAU), scale_y(stretch)),
        "heart" => heart(r, stretch),
        // A closed, smiling eye: the lower half of a ring.
        "arc" => transformed(&torus(r, r * 0.22, 6, 16, PI), Mat4::from_rotation_z(PI)),
        _ => transformed(&circle(r, 24, 0.0, TAU), scale_y(stretch)),
    }
}

/// An eye's pupil (at its own position, a hair in front of the socket) and its glass socket, if any.
pub fn eye_geometries(e: &Eye) -> (Mesh, Option<Mesh>) {
    let at = Vec3::from(e.at);
    let pupil = transformed(
        &eye_shape(&e.shape, e.pupil, e.stretch.unwrap_or(1.0)),
        Mat4::from_translation(at + Vec3::Z * 0.004),
    );
    let glass = if e.glass == Some(false) {
        None
    } else {
        Some(transformed(
            &circle(e.r.unwrap_or(0.1), 24, 0.0, TAU),
            Mat4::from_translation(at),
        ))
    };
    (pupil, glass)
}

/// Joins pieces into one geometry, with a per-vertex colour and slot (the matte's, for the renderer's
/// robot hues). None when there are no pieces.
pub fn join(pieces: &[Piece]) -> Option<Mesh> {
    if pieces.is_empty() {
        return None;
    }
    let (mut positions, mut normals, mut indices) = (Vec::new(), Vec::new(), Vec::new());
    let (mut colors, mut slots) = (Vec::new(), Vec::new());
    for p in pieces {
        let g = &p.geo;
        let n = g.positions.len();
        let base = positions.len() as u32;
        positions.extend_from_slice(&g.positions);
        normals.extend((0..n).map(|i| g.normals.get(i).copied().unwrap_or([0.0; 3])));
        if g.indices.is_empty() {
            indices.extend((0..n as u32).map(|i| base + i));
        } else {
            indices.extend(g.indices.iter().map(|i| base + i));
        }
        colors.extend(std::iter::repeat_n(p.colour, n));
        slots.extend(std::iter::repeat_n(p.slot, n));
    }
    let mut mesh = Geometry::new(positions, normals, Vec::new(), indices);
    let g = Arc::get_mut(&mut mesh).expect("a fresh geometry has one owner");
    g.colors = colors;
    g.slots = slots;
    Some(mesh)
}

/// A bone's ink line: the welded hull of its inked parts, with smooth normals (as `toon.ts`).
pub fn hull_of(pieces: &[Piece]) -> Option<Mesh> {
    join(pieces).map(|all| weld_smooth(&all, 1e-3))
}
