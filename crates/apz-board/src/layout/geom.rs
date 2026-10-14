//! Plan geometry for the layout: axis-aligned rectangles in (x, z), the plate frame (u along the long axis, v across
//! it, as board-layout.ts frameOf), guillotine subtraction into convex pieces, the polygon orientation test and the
//! segment-and-box test the occlusion check uses.

/// An axis-aligned rectangle in the plan: the min and max corners.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub z0: f64,
    pub x1: f64,
    pub z1: f64,
}

impl Rect {
    /// From the slot form [x, z, w, d].
    pub fn from_xywh(r: [f64; 4]) -> Rect {
        Rect {
            x0: r[0],
            z0: r[1],
            x1: r[0] + r[2],
            z1: r[1] + r[3],
        }
    }

    /// The slot form [x, z, w, d]. (Used by the tests; the second pass reads rects in this form.)
    #[allow(dead_code)]
    pub fn xywh(&self) -> [f64; 4] {
        [self.x0, self.z0, self.x1 - self.x0, self.z1 - self.z0]
    }

    /// The polygon in (x, z), counter-clockwise seen from above (see [`shoelace`]).
    pub fn poly(&self) -> Vec<[f64; 2]> {
        vec![
            [self.x0, self.z0],
            [self.x0, self.z1],
            [self.x1, self.z1],
            [self.x1, self.z0],
        ]
    }
}

/// Subtracts each hole from `outer`, guillotine-style: a piece a hole cuts becomes up to four rectangles around the
/// overlap (the strips below and above it, then the left and right parts beside it). The pieces are convex, do not
/// overlap and cover `outer` less the holes. Pieces with no area are dropped. Deterministic in the order of `holes`.
pub fn subtract(outer: Rect, holes: &[Rect]) -> Vec<Rect> {
    let mut pieces = vec![outer];
    for h in holes {
        let mut next = Vec::with_capacity(pieces.len() + 4);
        for p in pieces {
            let ix0 = p.x0.max(h.x0);
            let ix1 = p.x1.min(h.x1);
            let iz0 = p.z0.max(h.z0);
            let iz1 = p.z1.min(h.z1);
            if ix1 - ix0 <= 1e-9 || iz1 - iz0 <= 1e-9 {
                next.push(p);
                continue;
            }
            let cut = [
                Rect {
                    x0: p.x0,
                    z0: p.z0,
                    x1: p.x1,
                    z1: iz0,
                },
                Rect {
                    x0: p.x0,
                    z0: iz1,
                    x1: p.x1,
                    z1: p.z1,
                },
                Rect {
                    x0: p.x0,
                    z0: iz0,
                    x1: ix0,
                    z1: iz1,
                },
                Rect {
                    x0: ix1,
                    z0: iz0,
                    x1: p.x1,
                    z1: iz1,
                },
            ];
            next.extend(
                cut.into_iter()
                    .filter(|r| r.x1 - r.x0 > 1e-9 && r.z1 - r.z0 > 1e-9),
            );
        }
        pieces = next;
    }
    pieces
}

/// Twice the signed area of a polygon in (x, z): `sum(x_i z_{i+1} - x_{i+1} z_i)`. Its sign is the orientation seen
/// from above: a counter-clockwise polygon (seen from +y, the normal of a surface) has a negative value here, because
/// the plan's normal is `-(x_i z_j - x_j z_i)` in y.
#[allow(dead_code)] // Orientation check for the tests; the second pass checks its own polygons with it.
pub fn shoelace(poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let [x0, z0] = poly[i];
            let [x1, z1] = poly[(i + 1) % n];
            x0 * z1 - x1 * z0
        })
        .sum()
}

/// The frame of a plate (board-layout.ts frameOf): `u` runs along the long side (length `l`), `v` across it (width `s`).
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub x0: f64,
    pub z0: f64,
    pub along_x: bool,
    pub l: f64,
    pub s: f64,
}

impl Frame {
    /// The frame of a slot rect [x, z, w, d]; along x when w >= d.
    pub fn of(rect: [f64; 4]) -> Frame {
        let [x0, z0, w, d] = rect;
        let along_x = w >= d;
        Frame {
            x0,
            z0,
            along_x,
            l: if along_x { w } else { d },
            s: if along_x { d } else { w },
        }
    }

    /// The world rect [x, z, w, d] of the frame box [u0, u1] x [v0, v1].
    pub fn rect(&self, u0: f64, u1: f64, v0: f64, v1: f64) -> [f64; 4] {
        if self.along_x {
            [self.x0 + u0, self.z0 + v0, u1 - u0, v1 - v0]
        } else {
            [self.x0 + v0, self.z0 + u0, v1 - v0, u1 - u0]
        }
    }

    /// The world (x, z) of a frame point.
    #[allow(dead_code)] // Used by the tests and the second pass's landing search.
    pub fn point(&self, u: f64, v: f64) -> [f64; 2] {
        if self.along_x {
            [self.x0 + u, self.z0 + v]
        } else {
            [self.x0 + v, self.z0 + u]
        }
    }

    /// The world (x, z) of a frame direction.
    pub fn vec(&self, du: f64, dv: f64) -> [f64; 2] {
        if self.along_x { [du, dv] } else { [dv, du] }
    }
}

/// Is the segment from `from` to `to` blocked by the box (lo, hi) strictly between its ends? A slab test: a segment that
/// only touches a face does not count (board-layout.ts segmentBlocked).
pub fn segment_blocked(from: [f64; 3], to: [f64; 3], lo: [f64; 3], hi: [f64; 3]) -> bool {
    let mut t0 = 0.0_f64;
    let mut t1 = 1.0_f64;
    for k in 0..3 {
        let o = from[k];
        let d = to[k] - o;
        if d.abs() < 1e-12 {
            if o < lo[k] || o > hi[k] {
                return false;
            }
            continue;
        }
        let mut a = (lo[k] - o) / d;
        let mut b = (hi[k] - o) / d;
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        if a > t0 {
            t0 = a;
        }
        if b < t1 {
            t1 = b;
        }
        if t0 > t1 {
            return false;
        }
    }
    t1 - t0 > 1e-9 && t0 < 1.0 - 1e-9 && t1 > 1e-9
}

/// Does a part name belong to a slot's part? Its id, or a copy ("id#i"), or a path under it ("id/x", "id.x").
pub fn belongs(name: &str, part: &str) -> bool {
    name == part
        || name.strip_prefix(part).is_some_and(|rest| {
            rest.starts_with('#') || rest.starts_with('/') || rest.starts_with('.')
        })
}
