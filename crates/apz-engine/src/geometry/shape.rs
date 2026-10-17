//! Extruded outlines: a flat polygon given in plan, raised into a solid (the robots' `shape` parts).

use super::from_triangles;
use crate::scene;

/// A flat outline extruded up by `h`: the top and bottom faces (ear-clipped) and the side walls.
pub fn shape_geometry(points: &[[f32; 2]], h: f32) -> scene::Geo {
    // Work counter-clockwise in (x, w) with w = -z: seen from above, x right and w up.
    let mut pts: Vec<[f32; 2]> = points.to_vec();
    if pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    let area: f32 = (0..pts.len())
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            a[0] * -b[1] - b[0] * -a[1]
        })
        .sum();
    if area < 0.0 {
        pts.reverse();
    }
    let mut tris: Vec<[f32; 3]> = Vec::new();
    let v = |p: [f32; 2], y: f32| [p[0], y, p[1]];
    // Emits a triangle facing `want` (flips the winding if it doesn't), as from_triangles takes them.
    let mut push = |a: [f32; 3], b: [f32; 3], c: [f32; 3], want: [f32; 3]| {
        let (u, w) = (
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
            [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
        );
        let n = [
            u[1] * w[2] - u[2] * w[1],
            u[2] * w[0] - u[0] * w[2],
            u[0] * w[1] - u[1] * w[0],
        ];
        let facing = n[0] * want[0] + n[1] * want[1] + n[2] * want[2];
        if facing >= 0.0 {
            tris.extend_from_slice(&[a, b, c]);
        } else {
            tris.extend_from_slice(&[a, c, b]);
        }
    };
    for [i, j, k] in ear_clip(&pts) {
        push(v(pts[i], h), v(pts[j], h), v(pts[k], h), [0.0, 1.0, 0.0]);
        push(
            v(pts[i], 0.0),
            v(pts[j], 0.0),
            v(pts[k], 0.0),
            [0.0, -1.0, 0.0],
        );
    }
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        // Outward for a counter-clockwise outline in (x, w): (dw, -dx) in (x, w), i.e. (-dz, dx) in (x, z).
        let out = [-(b[1] - a[1]), 0.0, b[0] - a[0]];
        push(v(a, 0.0), v(b, 0.0), v(b, h), out);
        push(v(a, 0.0), v(b, h), v(a, h), out);
    }
    from_triangles(tris)
}

/// Ear clipping of a simple polygon given counter-clockwise in (x, -z); returns index triples.
fn ear_clip(pts: &[[f32; 2]]) -> Vec<[usize; 3]> {
    let w = |p: [f32; 2]| [p[0], -p[1]];
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| {
        let (o, a, b) = (w(o), w(a), w(b));
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let inside = |p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
        cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
    };
    let mut idx: Vec<usize> = (0..pts.len()).collect();
    let mut out = Vec::new();
    let mut guard = 0;
    while idx.len() > 3 && guard < 10_000 {
        guard += 1;
        let n = idx.len();
        let mut clipped = false;
        for k in 0..n {
            let (i, j, l) = (idx[(k + n - 1) % n], idx[k], idx[(k + 1) % n]);
            let (a, b, c) = (pts[i], pts[j], pts[l]);
            if cross(a, b, c) <= 0.0 {
                continue; // reflex or flat corner: not an ear
            }
            let blocked = idx
                .iter()
                .any(|&m| m != i && m != j && m != l && inside(pts[m], a, b, c));
            if blocked {
                continue;
            }
            out.push([i, j, l]);
            idx.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            // Degenerate input: fan the rest rather than loop forever.
            for k in 1..idx.len() - 1 {
                out.push([idx[0], idx[k], idx[k + 1]]);
            }
            return out;
        }
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    }
    out
}
