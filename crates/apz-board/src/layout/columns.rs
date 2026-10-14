//! The support lattice under a plate and the columns it needs (board-rooms.ts restingSlab, reworked for the plate rule).
//!
//! The plate rests on the HIGHEST support sample under its footprint, plus PLATE: its underside is that highest sample.
//! Every sample is a centre of a lattice cell (SUPPORT_CELL). The lattice is split into blocks while the samples in a
//! block differ by more than GROUND_TOL. A block's column stands on the highest sample inside its own footprint (its foot),
//! up to the underside: the foot is at or above every sample in the footprint and at or below the highest of them, so it
//! never cuts into a part and no column rests on the block's lowest sample.

/// The lattice cell's size: a support sample is the centre of each cell (the test uses the same samples).
pub const SUPPORT_CELL: f64 = 0.125;
/// Supports that differ by less than this are one height (drawn-tops.ts GROUND_TOL): a block of this spread is one column.
pub const GROUND_TOL: f64 = 0.02;

/// A column's footprint [x, z, w, d] and its foot: the highest support sample inside the footprint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Leaf {
    pub rect: [f64; 4],
    pub bottom: f64,
}

/// The support samples over a plate footprint: `nx * nz` cells in row-major order (x fastest), each with its centre's
/// support (0 over the ground).
#[derive(Debug, Clone, PartialEq)]
pub struct Lattice {
    pub x0: f64,
    pub z0: f64,
    pub cw: f64,
    pub cd: f64,
    pub nx: usize,
    pub nz: usize,
    pub sup: Vec<f64>,
}

impl Lattice {
    /// Samples the footprint [x, z, w, d] with `sup_at(x, z)`, the highest support at (x, z).
    pub fn of(rect: [f64; 4], sup_at: &dyn Fn(f64, f64) -> f64) -> Lattice {
        let [x0, z0, w, d] = rect;
        let nx = ((w / SUPPORT_CELL).ceil().max(1.0)) as usize;
        let nz = ((d / SUPPORT_CELL).ceil().max(1.0)) as usize;
        let (cw, cd) = (w / nx as f64, d / nz as f64);
        let mut sup = Vec::with_capacity(nx * nz);
        for j in 0..nz {
            for i in 0..nx {
                sup.push(sup_at(
                    x0 + (i as f64 + 0.5) * cw,
                    z0 + (j as f64 + 0.5) * cd,
                ));
            }
        }
        Lattice {
            x0,
            z0,
            cw,
            cd,
            nx,
            nz,
            sup,
        }
    }

    /// The highest sample: the plate's underside (0 over the ground).
    pub fn highest(&self) -> f64 {
        self.sup.iter().fold(0.0_f64, |m, &s| m.max(s))
    }

    /// The columns under a plate whose underside is `under`: one per block whose lowest sample is below it.
    pub fn columns(&self, under: f64) -> Vec<Leaf> {
        let mut out = Vec::new();
        self.split(0, self.nx, 0, self.nz, under, &mut out);
        out
    }

    fn at(&self, i: usize, j: usize) -> f64 {
        self.sup[j * self.nx + i]
    }

    fn split(&self, i0: usize, i1: usize, j0: usize, j1: usize, under: f64, out: &mut Vec<Leaf>) {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for j in j0..j1 {
            for i in i0..i1 {
                let s = self.at(i, j);
                lo = lo.min(s);
                hi = hi.max(s);
            }
        }
        let single = i1 - i0 == 1 && j1 - j0 == 1;
        // The tolerance has a float epsilon so a block at exactly GROUND_TOL spread is one height on both sides (the test's skip uses it too).
        if single || hi - lo <= GROUND_TOL + 1e-9 {
            if hi < under - 1e-9 {
                let rect = [
                    self.x0 + i0 as f64 * self.cw,
                    self.z0 + j0 as f64 * self.cd,
                    (i1 - i0) as f64 * self.cw,
                    (j1 - j0) as f64 * self.cd,
                ];
                out.push(Leaf { rect, bottom: hi });
            }
            return;
        }
        let im = if i1 - i0 > 1 { (i0 + i1) / 2 } else { i1 };
        let jm = if j1 - j0 > 1 { (j0 + j1) / 2 } else { j1 };
        for (a0, a1) in [(i0, im), (im, i1)] {
            for (b0, b1) in [(j0, jm), (jm, j1)] {
                if a1 > a0 && b1 > b0 {
                    self.split(a0, a1, b0, b1, under, out);
                }
            }
        }
    }
}
