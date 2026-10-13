//! The support field: the upward faces of the solids, bucketed on a grid, and the height of the highest one under a
//! point (ground.ts: supportField and topBelow, with the same tolerance and cell size).

use std::collections::BTreeMap;

/// A triangle in world units: three points.
pub type Tri = [[f64; 3]; 3];

/// The bucket size, as FIELD_CELL in ground.ts.
pub const CELL: f64 = 0.5;

/// Barycentric slack at the edges, as in topBelow.
const EDGE_TOL: f64 = 1e-9;

/// Degenerate triangles (area below this) are skipped.
const DEGENERATE: f64 = 1e-12;

/// Upward faces, the ones a part can rest on: n.y > half the length of n.
pub fn is_upward(t: &Tri) -> bool {
    let [a, b, c] = *t;
    let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    len >= DEGENERATE && n[1] > 0.5 * len
}

/// The upward faces of a scene, by bucket.
#[derive(Debug, Default, Clone)]
pub struct Support {
    tris: Vec<Tri>,
    cells: BTreeMap<(i64, i64), Vec<u32>>,
}

/// Faces this far above a limit still count as at or below it (the web's GROUND_TOL, drawn-tops.ts).
const LIMIT_TOL: f64 = 0.02;

impl Support {
    /// Adds an upward triangle to every bucket its box covers.
    pub fn add(&mut self, t: Tri) {
        let id = self.tris.len() as u32;
        let xs = [t[0][0], t[1][0], t[2][0]];
        let zs = [t[0][2], t[1][2], t[2][2]];
        let fx = |v: &[f64; 3]| v.iter().fold(f64::INFINITY, |m, &x| m.min(x));
        let fxm = |v: &[f64; 3]| v.iter().fold(f64::NEG_INFINITY, |m, &x| m.max(x));
        let i0 = (fx(&xs) / CELL).floor() as i64;
        let i1 = (fxm(&xs) / CELL).floor() as i64;
        let j0 = (fx(&zs) / CELL).floor() as i64;
        let j1 = (fxm(&zs) / CELL).floor() as i64;
        for i in i0..=i1 {
            for j in j0..=j1 {
                self.cells.entry((i, j)).or_default().push(id);
            }
        }
        self.tris.push(t);
    }

    /// The height of the highest upward face at (x, z), or None when nothing is there (the ground).
    pub fn top(&self, x: f64, z: f64) -> Option<f64> {
        self.top_below(x, z, f64::INFINITY)
    }

    /// As `top`, counting only faces at or below `y` (plus LIMIT_TOL): the web's topBelow. None for nothing there.
    pub(crate) fn top_below(&self, x: f64, z: f64, y: f64) -> Option<f64> {
        let key = ((x / CELL).floor() as i64, (z / CELL).floor() as i64);
        let bucket = self.cells.get(&key)?;
        let mut best = 0.0_f64;
        for &id in bucket {
            let [a, b, c] = self.tris[id as usize];
            let den = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
            if den.abs() < 1e-12 {
                continue;
            }
            let w1 = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / den;
            let w2 = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / den;
            let w3 = 1.0 - w1 - w2;
            if w1 < -EDGE_TOL || w2 < -EDGE_TOL || w3 < -EDGE_TOL {
                continue;
            }
            let h = w1 * a[1] + w2 * b[1] + w3 * c[1];
            if h > best && h <= y + LIMIT_TOL {
                best = h;
            }
        }
        (best > 0.0).then_some(best)
    }
}
