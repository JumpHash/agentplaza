//! The drawn tops (drawn-tops.ts): the board's and rooms' drawn upward faces as supports, and their bake over the nav
//! grid, which the walk heights and the hover audit read.
//!
//! A support field is the upward faces of the drawn solids; top_below is the exact drawn top under a point; the walk
//! heights bake (bake_drawn_tops) turns the field into one height per sample over the nav grid, and drawn_top_at reads it.

use std::collections::BTreeMap;

use crate::grid::NAV_STEP;
use crate::navgrid::NavGrid;

/// A bottom this far off what is under it, or an overlap this deep on every axis, is a finding (ground.ts).
pub const GROUND_TOL: f64 = 0.02;
/// The support field's bucket size: each bucket lists the upward triangles over it; a point is tested exactly against them.
pub const FIELD_CELL: f64 = 0.5;

/// The upward faces of a model's solids (model units, the model's frame): the triangles, nine numbers each (a, b, c as
/// x y z), and the buckets of the grid cells they cover (keyed by key_of). A point's support is the exact height of the
/// triangle under it, as a ray would find it.
#[derive(Debug, Clone, PartialEq)]
pub struct SupportField {
    pub cell: f64,
    pub tris: Vec<f64>,
    pub cells: BTreeMap<i64, Vec<usize>>,
}

/// A support field at a board's world scale: the field is in model units, the scale maps them to the board.
#[derive(Debug, Clone, PartialEq)]
pub struct Support {
    pub field: SupportField,
    pub scale: f64,
}

const KEY_OFF: i64 = 100000;
const KEY_M: i64 = 200000;

/// The bucket key of grid cell (i, j) in a support field.
pub fn key_of(i: i64, j: i64) -> i64 {
    (i + KEY_OFF) * KEY_M + (j + KEY_OFF)
}

/// The height of the highest support under (x, z) at or below y (within the tolerance), in world units; 0 for the ground
/// or no support. y may be infinite (the highest support there is).
pub fn top_below(sup: Option<&Support>, x: f64, z: f64, y: f64) -> f64 {
    let Some(sup) = sup else {
        return 0.0;
    };
    let f = &sup.field;
    let s = sup.scale;
    let xm = x / s;
    let zm = z / s;
    let ym = y / s + GROUND_TOL / s;
    let Some(bucket) = f.cells.get(&key_of(
        (xm / f.cell).floor() as i64,
        (zm / f.cell).floor() as i64,
    )) else {
        return 0.0;
    };
    let mut best = 0.0;
    for &t in bucket {
        let o = t * 9;
        let tr = &f.tris;
        let (ax, ay, az) = (tr[o], tr[o + 1], tr[o + 2]);
        let (bx, by, bz) = (tr[o + 3], tr[o + 4], tr[o + 5]);
        let (cx, cy, cz) = (tr[o + 6], tr[o + 7], tr[o + 8]);
        let den = (bz - cz) * (ax - cx) + (cx - bx) * (az - cz);
        if den.abs() < 1e-12 {
            continue;
        }
        let w1 = ((bz - cz) * (xm - cx) + (cx - bx) * (zm - cz)) / den;
        let w2 = ((cz - az) * (xm - cx) + (ax - cx) * (zm - cz)) / den;
        let w3 = 1.0 - w1 - w2;
        if w1 < -1e-9 || w2 < -1e-9 || w3 < -1e-9 {
            continue;
        }
        let h = w1 * ay + w2 * by + w3 * cy;
        if h <= ym && h > best {
            best = h;
        }
    }
    best * s
}

/// The exact drawn top under (x, z) at or below y, over several supports (the board, its rooms): the highest of them.
pub fn drawn_top_below(sups: &[Support], x: f64, z: f64, y: f64) -> f64 {
    let mut h = 0.0_f64;
    for s in sups {
        h = h.max(top_below(Some(s), x, z, y));
    }
    h
}

/// The walk heights' sample spacing (world units): the bilinear lookup reads the four samples around a point.
pub const DRAWN_STEP: f64 = 0.05;
/// A sample's tolerance on the band (world units): a drawn top within NAV_STEP of the walk cell's height counts.
const BAND_EPS: f64 = 1e-4;

/// The walk heights over a nav grid, sampled every `step` from the grid's origin. NaN where the sample is off the walk
/// or no drawn top is in its band. Row-major: sample (i, j) is h[j * nx + i].
#[derive(Debug, Clone, PartialEq)]
pub struct DrawnTops {
    pub x0: f64,
    pub z0: f64,
    pub step: f64,
    pub nx: usize,
    pub nz: usize,
    pub h: Vec<f64>,
}

/// The walk heights of `grid` from the drawn supports (world units). Pure; the bake costs one pass per support triangle.
/// A sample on a walk cell takes the highest drawn upward face under it whose height is within NAV_STEP of the cell's grid
/// height (the band rule: a part taller than a step is blocked on the grid, so walls, slabs and furniture never enter, and
/// a chip top a step high does).
pub fn bake_drawn_tops(grid: &NavGrid, sups: &[Support], step: f64) -> DrawnTops {
    let nx = ((grid.cols as f64 * grid.cell) / step).ceil() as usize + 1;
    let nz = ((grid.rows as f64 * grid.cell) / step).ceil() as usize + 1;
    let mut band = vec![f64::NAN; nx * nz];
    let mut best = vec![f64::NEG_INFINITY; nx * nz];
    for j in 0..nz {
        for i in 0..nx {
            let x = grid.x0 + i as f64 * step;
            let z = grid.z0 + j as f64 * step;
            band[j * nx + i] = match grid.cell_at(x, z) {
                Some(c) if grid.walk[c] == 1 => grid.y[c],
                _ => f64::NAN,
            };
        }
    }
    for sup in sups {
        let tr = &sup.field.tris;
        let s = sup.scale;
        for o in (0..).step_by(9).take_while(|&o| o + 9 <= tr.len()) {
            let (ax, ay, az) = (tr[o] * s, tr[o + 1] * s, tr[o + 2] * s);
            let (bx, by, bz) = (tr[o + 3] * s, tr[o + 4] * s, tr[o + 5] * s);
            let (cx, cy, cz) = (tr[o + 6] * s, tr[o + 7] * s, tr[o + 8] * s);
            let den = (bz - cz) * (ax - cx) + (cx - bx) * (az - cz);
            if den.abs() < 1e-12 {
                continue;
            }
            let i0 = (((ax.min(bx).min(cx) - grid.x0) / step).ceil().max(0.0)) as i64;
            let i1 = (((ax.max(bx).max(cx) - grid.x0) / step)
                .floor()
                .min(nx as f64 - 1.0)) as i64;
            let j0 = (((az.min(bz).min(cz) - grid.z0) / step).ceil().max(0.0)) as i64;
            let j1 = (((az.max(bz).max(cz) - grid.z0) / step)
                .floor()
                .min(nz as f64 - 1.0)) as i64;
            // The barycentric weights are linear in x: each row starts at its first sample and steps by dw1 and dw2.
            let dw1 = ((bz - cz) * step) / den;
            let dw2 = ((cz - az) * step) / den;
            for j in j0..=j1 {
                let z = grid.z0 + j as f64 * step;
                let x_start = grid.x0 + i0 as f64 * step;
                let mut w1 = ((bz - cz) * (x_start - cx) + (cx - bx) * (z - cz)) / den;
                let mut w2 = ((cz - az) * (x_start - cx) + (ax - cx) * (z - cz)) / den;
                for i in i0..=i1 {
                    let (cw1, cw2) = (w1, w2);
                    w1 += dw1;
                    w2 += dw2;
                    let k = j as usize * nx + i as usize;
                    let b = band[k];
                    if b.is_nan() {
                        continue; // off the walk: no walk height here
                    }
                    let w3 = 1.0 - cw1 - cw2;
                    if cw1 < -1e-9 || cw2 < -1e-9 || w3 < -1e-9 {
                        continue;
                    }
                    let h = cw1 * ay + cw2 * by + w3 * cy;
                    if (h - b).abs() <= NAV_STEP + BAND_EPS && h > best[k] {
                        best[k] = h;
                    }
                }
            }
        }
    }
    let h = best
        .iter()
        .map(|&b| if b == f64::NEG_INFINITY { f64::NAN } else { b })
        .collect();
    DrawnTops {
        x0: grid.x0,
        z0: grid.z0,
        step,
        nx,
        nz,
        h,
    }
}

/// The walk height under (x, z): bilinear over the four samples around the point. NaN when one of them has no drawn top
/// (the caller falls back to the grid's height) or the point is off the field.
pub fn drawn_top_at(d: &DrawnTops, x: f64, z: f64) -> f64 {
    let u = (x - d.x0) / d.step;
    let v = (z - d.z0) / d.step;
    let i = u.floor() as i64;
    let j = v.floor() as i64;
    if i < 0 || j < 0 || i + 1 >= d.nx as i64 || j + 1 >= d.nz as i64 {
        return f64::NAN;
    }
    let (i, j) = (i as usize, j as usize);
    let a = d.h[j * d.nx + i];
    let b = d.h[j * d.nx + i + 1];
    let c = d.h[(j + 1) * d.nx + i];
    let e = d.h[(j + 1) * d.nx + i + 1];
    if a.is_nan() || b.is_nan() || c.is_nan() || e.is_nan() {
        return f64::NAN;
    }
    let fu = u - i as f64;
    let fv = v - j as f64;
    (a * (1.0 - fu) + b * fu) * (1.0 - fv) + (c * (1.0 - fu) + e * fu) * fv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_below_reads_a_one_cell_support() {
        // One upward triangle at height 0.5 inside cell (0, 0).
        let field = SupportField {
            cell: FIELD_CELL,
            tris: vec![0.0, 0.5, 0.0, 0.5, 0.5, 0.0, 0.0, 0.5, 0.5],
            cells: BTreeMap::from([(key_of(0, 0), vec![0])]),
        };
        let sup = Support { field, scale: 1.0 };
        let top = top_below(Some(&sup), 0.1, 0.1, f64::INFINITY);
        assert!((top - 0.5).abs() < 1e-9, "top {top}");
        assert_eq!(top_below(None, 0.1, 0.1, f64::INFINITY), 0.0);
    }
}
