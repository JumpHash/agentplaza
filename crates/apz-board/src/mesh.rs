//! The slab mesh of a walk surface, and the ray helper the audits read it with (docs/design/walk-surfaces.md).
//!
//! The top of a slab is the walk height by construction: each top vertex sits at `surface.height.at(x, z)`, rounded
//! to f32 as three.js stores it. So the drawn top and the walk height are the same numbers, and [`ray_down`] reads the
//! drawn height back at any point.
//!
//! Winding: a top triangle's normal (by the right-hand rule, the one `support::is_upward` reads) points up, a bottom
//! triangle's points down, and a side triangle's points out of the polygon. A polygon that is counter-clockwise in
//! (x, z) has a downward right-hand normal, so the top fan lists its corners in reverse order.

use crate::support::is_upward;
use crate::walk::WalkSurface;

/// Barycentric slack at the edges, as in support.rs (topBelow).
const EDGE_TOL: f64 = 1e-9;

/// Degenerate triangles (area below this) are skipped, as in support.rs.
const DEGENERATE: f64 = 1e-12;

/// A triangle mesh: vertex positions and triangles as index triples.
#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    /// Vertex positions, each coordinate rounded to f32 (three.js keeps them in a Float32Array).
    pub positions: Vec<[f32; 3]>,
    /// Triangles: three indices into `positions` each. The normal by the right-hand rule points out of the solid.
    pub indices: Vec<u32>,
}

/// The slab of a walk surface: its top at the walk height, its sides down to `top - thickness`, and its bottom.
///
/// The top vertex over each polygon corner is `surface.height.at(x, z)` rounded to f32 (round to nearest). The bottom
/// lies `thickness` below the top at every corner, so the slab has a constant vertical thickness and follows a ramp's
/// slope. Every polygon side gets a side face, whatever its `Side` kind. `thickness` should be positive. The polygon
/// should pass [`surface_mesh_check`]; one with fewer than three points gives an empty mesh.
pub fn slab(surface: &WalkSurface, thickness: f64) -> Mesh {
    let poly = &surface.poly;
    let n = poly.len();
    if n < 3 {
        return Mesh {
            positions: Vec::new(),
            indices: Vec::new(),
        };
    }
    let tops: Vec<f64> = poly.iter().map(|p| surface.height.at(p[0], p[1])).collect();
    let mut positions = Vec::with_capacity(2 * n);
    positions.extend(
        poly.iter()
            .zip(&tops)
            .map(|(p, &y)| [p[0] as f32, y as f32, p[1] as f32]),
    );
    positions.extend(
        poly.iter()
            .zip(&tops)
            .map(|(p, &y)| [p[0] as f32, (y - thickness) as f32, p[1] as f32]),
    );

    let bottom = |i: usize| (n + i) as u32;
    let mut indices = Vec::with_capacity(12 * n);
    // Top: a fan from corner 0, listed as (0, i + 1, i) so the normal points up.
    for i in 1..n - 1 {
        indices.extend([0, (i + 1) as u32, i as u32]);
    }
    // Bottom: the same fan on the bottom corners, listed as (0, i, i + 1) so the normal points down.
    for i in 1..n - 1 {
        indices.extend([bottom(0), bottom(i), bottom(i + 1)]);
    }
    // Sides: for the edge from corner i to corner j, the two triangles face out of the polygon.
    for i in 0..n {
        let j = (i + 1) % n;
        let (ti, tj) = (i as u32, j as u32);
        let (bi, bj) = (bottom(i), bottom(j));
        indices.extend([ti, bj, bi]);
        indices.extend([ti, tj, bj]);
    }
    Mesh { positions, indices }
}

/// The highest upward-facing triangle top at (x, z) that lies at or below `above`, or None when no upward face is
/// there. Faces are upward when `support::is_upward` says so (normal y above half its length), so downward, vertical
/// and degenerate triangles never count. A point on a shared edge is hit, within the same EDGE_TOL slack as the
/// support field. The result is the drawn height at that point: the walk height, read back from the mesh.
pub fn ray_down(mesh: &Mesh, x: f64, z: f64, above: f64) -> Option<f64> {
    let mut best: Option<f64> = None;
    for t in mesh.indices.chunks_exact(3) {
        let Some([a, b, c]) = corners(mesh, t) else {
            continue;
        };
        if !is_upward(&[a, b, c]) {
            continue;
        }
        let den = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
        if den.abs() < DEGENERATE {
            continue;
        }
        let w1 = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / den;
        let w2 = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / den;
        let w3 = 1.0 - w1 - w2;
        if w1 < -EDGE_TOL || w2 < -EDGE_TOL || w3 < -EDGE_TOL {
            continue;
        }
        let h = w1 * a[1] + w2 * b[1] + w3 * c[1];
        if h <= above {
            best = match best {
                Some(prev) if prev >= h => Some(prev),
                _ => Some(h),
            };
        }
    }
    best
}

/// The three corners of a triangle, widened to f64, or None when an index is out of range.
fn corners(mesh: &Mesh, t: &[u32]) -> Option<[[f64; 3]; 3]> {
    let mut out = [[0.0; 3]; 3];
    for (slot, &idx) in out.iter_mut().zip(t) {
        let p = mesh.positions.get(idx as usize)?;
        *slot = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
    }
    Some(out)
}

/// Whether `poly` is a convex polygon of at least three distinct points, wound counter-clockwise in (x, z): every point
/// lies on the left of (or on) every edge, and the polygon has positive area. Collinear points are allowed.
pub fn is_convex_ccw(poly: &[[f64; 2]]) -> bool {
    polygon_fault(poly).is_none()
}

/// Checks a walk surface before its mesh is built: the polygon is convex and counter-clockwise, `sides` has one entry
/// per polygon side, and the height is finite. The error names the surface and what failed.
pub fn surface_mesh_check(surface: &WalkSurface) -> Result<(), String> {
    let who = || format!("walk surface {} ({})", surface.id, surface.owner);
    if let Some(fault) = polygon_fault(&surface.poly) {
        return Err(format!("{}: polygon {fault}", who()));
    }
    if surface.sides.len() != surface.poly.len() {
        return Err(format!(
            "{}: {} sides for {} polygon points",
            who(),
            surface.sides.len(),
            surface.poly.len()
        ));
    }
    let h = surface.height;
    if !(h.c.is_finite() && h.dx.is_finite() && h.dz.is_finite()) {
        return Err(format!(
            "{}: height is not finite (c {}, dx {}, dz {})",
            who(),
            h.c,
            h.dx,
            h.dz
        ));
    }
    Ok(())
}

/// Why `poly` is not a convex counter-clockwise polygon of distinct points, or None when it is one. The text follows
/// "polygon " in the surface check's message.
fn polygon_fault(poly: &[[f64; 2]]) -> Option<String> {
    let n = poly.len();
    if n < 3 {
        return Some(format!("has {n} points, needs at least 3"));
    }
    if poly.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
        return Some("has a point that is not finite".to_string());
    }
    for (i, p) in poly.iter().enumerate() {
        if let Some(j) = poly[i + 1..].iter().position(|q| q == p) {
            return Some(format!("has points {i} and {} equal", i + 1 + j));
        }
    }
    // Cross product of (a - o) and (b - o): positive when b is left of the ray o to a.
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    for (i, &a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % n];
        if let Some(k) = poly.iter().position(|&p| cross(a, b, p) < 0.0) {
            return Some(format!(
                "is not convex and counter-clockwise: point {k} is right of edge {i}"
            ));
        }
    }
    let area2: f64 = poly
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            let q = poly[(i + 1) % n];
            p[0] * q[1] - q[0] * p[1]
        })
        .sum();
    if area2 <= 0.0 {
        return Some("has zero area".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walk::{Plane, Side, SurfaceKind};

    fn surface(poly: Vec<[f64; 2]>, height: Plane) -> WalkSurface {
        let sides = vec![Side::Walled; poly.len()];
        WalkSurface {
            id: 7,
            kind: SurfaceKind::Plate,
            poly,
            height,
            owner: "test".to_string(),
            sides,
        }
    }

    fn unit_square() -> Vec<[f64; 2]> {
        vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
    }

    fn assert_height(got: Option<f64>, want: f64) {
        let h = got.expect("a hit");
        assert!((h - want).abs() < 1e-9, "got {h}, want {want}");
    }

    fn normal(m: &Mesh, t: &[u32]) -> [f64; 3] {
        let [a, b, c] = corners(m, t).expect("valid index");
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ]
    }

    #[test]
    fn flat_slab_top_is_the_walk_height_in_f32() {
        let s = surface(unit_square(), Plane::flat(1.5));
        let m = slab(&s, 0.25);
        assert_eq!(m.positions.len(), 8);
        for (p, q) in m.positions[..4].iter().zip(unit_square()) {
            assert_eq!(p[1], 1.5_f32);
            assert_eq!([p[0] as f64, p[2] as f64], q);
        }
        for p in &m.positions[4..] {
            assert_eq!(p[1], 1.25_f32);
        }
    }

    #[test]
    fn ramp_slab_top_vertices_sit_on_the_plane() {
        let plane = Plane {
            c: 0.5,
            dx: 0.25,
            dz: -0.5,
        };
        let s = surface(unit_square(), plane);
        let m = slab(&s, 0.1);
        for (p, q) in m.positions[..4].iter().zip(unit_square()) {
            assert_eq!(p[1], plane.at(q[0], q[1]) as f32);
        }
    }

    #[test]
    fn normals_face_up_on_top_down_on_bottom_and_out_on_sides() {
        let s = surface(
            unit_square(),
            Plane {
                c: 0.5,
                dx: 0.25,
                dz: -0.5,
            },
        );
        let m = slab(&s, 0.1);
        for t in m.indices.chunks_exact(3) {
            let n = normal(&m, t);
            if t.iter().all(|&i| i < 4) {
                assert!(n[1] > 0.0, "top triangle {t:?} normal {n:?}");
            } else if t.iter().all(|&i| (4..8).contains(&i)) {
                assert!(n[1] < 0.0, "bottom triangle {t:?} normal {n:?}");
            } else {
                assert!(
                    n[1].abs() < 1e-12,
                    "side triangle {t:?} is not vertical: {n:?}"
                );
                let c = t
                    .iter()
                    .map(|&i| {
                        let p = m.positions[i as usize];
                        [f64::from(p[0]), f64::from(p[2])]
                    })
                    .fold([0.0, 0.0], |acc, p| {
                        [acc[0] + p[0] / 3.0, acc[1] + p[1] / 3.0]
                    });
                let out = (c[0] - 0.5) * n[0] + (c[1] - 0.5) * n[2];
                assert!(out > 0.0, "side triangle {t:?} faces inward: {n:?}");
            }
        }
    }

    #[test]
    fn ray_down_matches_the_plane_on_a_ramp() {
        let plane = Plane {
            c: 0.5,
            dx: 0.25,
            dz: -0.5,
        };
        let m = slab(&surface(unit_square(), plane), 0.1);
        for &x in &[0.1, 0.3, 0.5, 0.7, 0.9] {
            for &z in &[0.1, 0.4, 0.6, 0.9] {
                let h = ray_down(&m, x, z, 10.0).expect("inside the square");
                assert!(
                    (h - plane.at(x, z)).abs() < 1e-6,
                    "at ({x}, {z}): {h} vs {}",
                    plane.at(x, z)
                );
            }
        }
    }

    #[test]
    fn ray_down_ignores_tops_above_the_ray() {
        let m = slab(&surface(unit_square(), Plane::flat(1.5)), 0.25);
        assert_eq!(ray_down(&m, 0.5, 0.5, 1.0), None);
        assert_height(ray_down(&m, 0.5, 0.5, 1.6), 1.5);
    }

    #[test]
    fn ray_down_outside_the_polygon_is_none() {
        let m = slab(&surface(unit_square(), Plane::flat(1.5)), 0.25);
        assert_eq!(ray_down(&m, 2.0, 0.5, 10.0), None);
        assert_eq!(ray_down(&m, 0.5, -0.5, 10.0), None);
        assert_eq!(ray_down(&m, -1e-6, 0.5, 10.0), None);
    }

    #[test]
    fn ray_down_hits_a_point_on_a_shared_edge() {
        let m = slab(&surface(unit_square(), Plane::flat(1.5)), 0.25);
        // On the diagonal shared by the two top triangles.
        assert_height(ray_down(&m, 0.5, 0.5, 10.0), 1.5);
        // Just outside the bottom edge, inside the EDGE_TOL slack.
        assert_height(ray_down(&m, 0.5, -1e-10, 10.0), 1.5);
        // Clearly outside it.
        assert_eq!(ray_down(&m, 0.5, -1e-6, 10.0), None);
    }

    #[test]
    fn ray_down_skips_a_degenerate_sliver() {
        // Three almost collinear points, upward facing, with area far below DEGENERATE.
        let m = Mesh {
            positions: vec![[0.0, 1.0, 0.0], [1.0, 1.0, 1e-13], [2.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
        };
        assert_eq!(ray_down(&m, 1.0, 0.0, 10.0), None);
    }

    #[test]
    fn clockwise_polygon_is_not_convex_ccw() {
        assert!(is_convex_ccw(&unit_square()));
        let clockwise = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
        assert!(!is_convex_ccw(&clockwise));
    }

    #[test]
    fn star_repeats_and_short_polygons_are_rejected() {
        // A pentagram: every turn is left, but it winds twice and is not convex.
        let star: Vec<[f64; 2]> = (0..5)
            .map(|k| {
                let a = 4.0 * std::f64::consts::PI * k as f64 / 5.0;
                [a.cos(), a.sin()]
            })
            .collect();
        assert!(!is_convex_ccw(&star));
        assert!(!is_convex_ccw(&[
            [0.0, 0.0],
            [1.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0]
        ]));
        assert!(!is_convex_ccw(&[[0.0, 0.0], [1.0, 0.0]]));
        assert!(!is_convex_ccw(&[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]]));
    }

    #[test]
    fn triangle_missing_a_side_count_is_rejected() {
        let tri = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let mut s = surface(tri, Plane::flat(0.0));
        assert_eq!(surface_mesh_check(&s), Ok(()));
        s.sides.pop();
        let err = surface_mesh_check(&s).unwrap_err();
        assert!(err.contains("2 sides for 3 polygon points"), "{err}");
    }

    #[test]
    fn surface_check_rejects_a_non_finite_height() {
        let s = surface(
            unit_square(),
            Plane {
                c: f64::NAN,
                dx: 0.0,
                dz: 0.0,
            },
        );
        let err = surface_mesh_check(&s).unwrap_err();
        assert!(err.contains("height is not finite"), "{err}");
    }
}
