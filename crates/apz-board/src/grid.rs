//! The nav grid (docs/design/walk-surfaces.md): the walk surfaces rasterised into square cells, with the height moves
//! between cells, the lift portals, the one-way drops and the components. It follows the rules of
//! apps/web/src/views/floor3d/nav-grid.ts, read from the walk surfaces instead of layout numbers.
//!
//! How it matches nav-grid.ts (same numbers, same order):
//! - Cell size NAV_CELL. Cell c = j * cols + i, centre (x0 + (i + 0.5) * cell, z0 + (j + 0.5) * cell).
//! - Height moves are moveKind: |dy| <= NAV_STEP is a walk, a rise above NAV_STEP up to NAV_JUMP_UP is a jump, a
//!   fall above NAV_STEP down to NAV_DROP_DOWN is a drop, anything else is no move.
//! - A diagonal move needs both orthogonal cells to be walk.
//! - Inflation: a supported cell that is not blocked, not exempt, and has a blocked or off-grid 8-neighbour is not
//!   walk. Exempt cells are the cells at each link's join point and their 8-neighbours, and the lift landing cells.
//! - Cost: NAV_COST_OPEN, plus NAV_NEAR_COST at Chebyshev distance exactly 2 from a blocked cell.
//! - Portals: a lift joins its bottom and top landing cells, two-way, cost NAV_LIFT_COST + rise. portal[c] is the
//!   first portal at c.
//! - Components: union-find over two-way moves and portals, labelled in order of lowest cell index (-1 off walk).
//!   Drops are the one-way moves, kept only when both cells are in the main component.
//!
//! Differences, and why:
//! - There are no plates, rooms, traces, boxes or landing searches: the walk surfaces are the only support. A cell is
//!   support when its centre and its four corners lie on the highest surface under the centre, or on a surface joined
//!   to it by a Step, Ramp or Door link whose height is within NAV_STEP. Walls and anything outside the walk list
//!   never hold a robot up.
//! - A cell is blocked when its closed square touches a Walled side. Open and Drop sides block nothing.
//! - Moves between two surfaces need a link (Step, Ramp, Door: two-way within NAV_JUMP_UP; Drop: one-way, downhill).
//!   Adjacency across two surfaces with no link is not an edge.
//! - Islands are not cut from the walk. Their cells keep their own component ids, so the layout can see them. Drops
//!   stay inside the main component, so no robot falls into a place it cannot leave.
//! - There is no trace cost (surfaces carry no traces) and no penalty array.
//! - The fine height field (NAV_FINE = 0.05) answers height_at exactly. It keeps the surfaces whose boxes reach each
//!   fine sample, and evaluates the highest surface that contains the query point with that surface's plane.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::walk::{LinkKind, Side, WalkSet, WalkSurface};

/// Grid cell size (world units).
pub const NAV_CELL: f64 = 0.25;
/// A walk cell may step to a neighbour whose height differs by at most this (walked normally).
pub const NAV_STEP: f64 = 0.3;
/// A jump: a rise above NAV_STEP up to this. A two-way link is only two-way within this height.
pub const NAV_JUMP_UP: f64 = 0.8;
/// A drop: a fall above NAV_STEP down to this.
pub const NAV_DROP_DOWN: f64 = 2.0;
/// A lift portal costs this much plus its rise.
pub const NAV_LIFT_COST: f64 = 4.0;
/// The base cost of a walk cell.
pub const NAV_COST_OPEN: f64 = 1.0;
/// The extra cost of a walk cell at Chebyshev distance exactly 2 from a blocked cell.
pub const NAV_NEAR_COST: f64 = 0.5;
/// The spacing of the fine height field (world units). Each cell holds NAV_CELL / NAV_FINE fine samples per side.
pub const NAV_FINE: f64 = 0.05;

const EPS: f64 = 1e-9;

/// The eight neighbour steps (di, dj), in the order the planner uses them (nav-plan.ts rule 4).
pub const DIRS8: [(i64, i64); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, 1), (1, -1), (-1, -1)];
/// The index in DIRS8 of the opposite step.
const OPP: [usize; 8] = [1, 0, 3, 2, 7, 6, 5, 4];

/// The height move for a height difference `dy` (nav-grid.ts moveKind): Some(0) a walk, Some(1) a jump, Some(2) a drop,
/// None when the step is not a move.
pub fn move_kind(dy: f64) -> Option<u8> {
    let a = dy.abs();
    if a <= NAV_STEP {
        Some(0)
    } else if dy > 0.0 {
        (dy <= NAV_JUMP_UP).then_some(1)
    } else {
        (a <= NAV_DROP_DOWN).then_some(2)
    }
}

/// A one-way fall: from the high cell to the low one. Both cells are in the main component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Drop {
    pub from: usize,
    pub to: usize,
}

/// A two-way crossing between cells `a` and `b` (a lift), costing `cost`. `link` indexes `WalkSet::links`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Portal {
    pub a: usize,
    pub b: usize,
    pub kind: LinkKind,
    pub cost: f64,
    pub link: usize,
}

/// The fine height field over the grid bounds, in samples of NAV_FINE. Sample k = j * cols + i holds the walk surfaces
/// whose boxes reach it, in ascending surface index: items[start[k]..start[k + 1]].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FineField {
    pub cols: usize,
    pub rows: usize,
    pub x0: f64,
    pub z0: f64,
    pub start: Vec<u32>,
    pub items: Vec<u32>,
}

/// The rasterised walk surfaces. Every per-cell vector has `cols * rows` entries, in cell index order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    pub cell: f64,
    pub cols: usize,
    pub rows: usize,
    pub x0: f64,
    pub z0: f64,
    /// 1 where a robot may stand (support, not blocked, not inflated), else 0.
    pub walk: Vec<u8>,
    /// 1 where the closed cell touches a Walled side, else 0.
    pub blocked: Vec<u8>,
    /// 1 where a supported cell is pushed off the plan by a blocked or off-grid neighbour, else 0.
    pub inflated: Vec<u8>,
    /// For a walk cell, the index into `surfaces` of the surface it stands on; -1 otherwise.
    pub surface: Vec<i32>,
    /// For a walk cell, the surface height at its centre; 0 otherwise.
    pub y: Vec<f64>,
    /// The cell cost (NAV_COST_OPEN, plus NAV_NEAR_COST at distance 2 from a blocked cell).
    pub cost: Vec<f64>,
    /// The component of each walk cell (labelled by lowest cell index); -1 otherwise.
    pub component: Vec<i32>,
    /// The first portal at each cell, or -1.
    pub portal: Vec<i32>,
    /// Per cell, bit k is set when a move to DIRS8[k] is allowed (both cells walk, a legal height move, and the surfaces
    /// are joined in that direction).
    pub moves: Vec<u8>,
    /// Per walk cell, bit k is set when the move to DIRS8[k] crosses from one surface to another over a Step or Ramp
    /// link (never a Door, a Drop or a Lift). A sight line may change surface only through such a seam (plan.rs).
    pub seam: Vec<u8>,
    /// The one-way moves inside the main component, in cell index order.
    pub drops: Vec<Drop>,
    /// The lift portals, in link order (only those whose two landings exist).
    pub portals: Vec<Portal>,
    /// The main component's id (the largest; ties go to the lowest id), or -1 when there is no walk cell.
    pub main: i32,
    /// The walk surfaces the grid was built from (WalkSet::surfaces, same order).
    pub surfaces: Vec<WalkSurface>,
    /// The fine height field.
    pub fine: FineField,
}

impl Grid {
    /// The cell containing (x, z), or None outside the grid.
    pub fn cell_at(&self, x: f64, z: f64) -> Option<usize> {
        index_of(self.x0, self.z0, self.cols, self.rows, x, z)
    }

    /// The centre (x, z) of cell `c`.
    pub fn cell_centre(&self, c: usize) -> [f64; 2] {
        centre_of(self.x0, self.z0, self.cols, c)
    }

    /// The neighbour of cell `c` in direction `k` (an index into DIRS8), or None off the grid.
    pub fn neighbour(&self, c: usize, k: usize) -> Option<usize> {
        step(c, k, self.cols, self.rows)
    }

    /// The walk surface height at (x, z) from the fine field, or None where no walk surface lies under the point.
    pub fn height_at(&self, x: f64, z: f64) -> Option<f64> {
        let f = &self.fine;
        if f.cols == 0 || x < f.x0 || z < f.z0 {
            return None;
        }
        let i = ((x - f.x0) / NAV_FINE).floor() as usize;
        let j = ((z - f.z0) / NAV_FINE).floor() as usize;
        if i >= f.cols || j >= f.rows {
            return None;
        }
        let k = j * f.cols + i;
        let mut best: Option<f64> = None;
        for &s in &f.items[f.start[k] as usize..f.start[k + 1] as usize] {
            let surf = &self.surfaces[s as usize];
            if !Shape::new(&surf.poly).contains(x, z) {
                continue;
            }
            let h = surf.height.at(x, z);
            best = match best {
                Some(b) if b >= h => Some(b),
                _ => Some(h),
            };
        }
        best
    }

    /// The component of cell `c` (-1 when the cell is not walk, or is out of range).
    pub fn component_of(&self, c: usize) -> i32 {
        self.component.get(c).copied().unwrap_or(-1)
    }

    /// The main component's id (-1 when there is none).
    pub fn main_component(&self) -> i32 {
        self.main
    }
}

/// Rasterises a walk set into a grid. Deterministic: the same set gives an equal grid.
pub fn build(set: &WalkSet) -> Grid {
    build_with(set, true)
}

/// The build with the shape lookup chosen: `indexed` tests each cell against the shapes of its bucket (ShapeIndex), and
/// otherwise against every shape. Both give the same grid; the second is the reference the tests compare with.
fn build_with(set: &WalkSet, indexed: bool) -> Grid {
    let surfaces = &set.surfaces;
    let shapes: Vec<Shape> = surfaces.iter().map(|s| Shape::new(&s.poly)).collect();
    let by_id: BTreeMap<u32, usize> = surfaces.iter().enumerate().map(|(k, s)| (s.id, k)).collect();
    let (x0, z0, cols, rows) = bounds(&shapes);
    let n = cols * rows;
    let index = ShapeIndex::new(&shapes, x0, z0, cols, rows);
    let every: Vec<usize> = (0..shapes.len()).collect();

    // The joins (two-way: Step, Ramp, Door), the falls (one-way: Drop) and the lifts (link, bottom, top, join point).
    let mut joins: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut seams: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut falls: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut lifts: Vec<(usize, usize, usize, [f64; 2])> = Vec::new();
    for (li, link) in set.links.iter().enumerate() {
        let (Some(&a), Some(&b)) = (by_id.get(&link.from), by_id.get(&link.to)) else {
            continue;
        };
        match link.kind {
            LinkKind::Step | LinkKind::Ramp | LinkKind::Door => {
                joins.insert((a, b));
                joins.insert((b, a));
                if link.kind != LinkKind::Door {
                    seams.insert((a, b));
                    seams.insert((b, a));
                }
            }
            LinkKind::Drop => {
                falls.insert((a, b));
            }
            LinkKind::Lift => lifts.push((li, a, b, link.at)),
        }
    }

    // Support: the highest surface under the centre, with all four corners on it or on a surface joined to it.
    let support: Vec<Option<(usize, f64)>> = (0..n)
        .map(|c| {
            let [cx, cz] = centre_of(x0, z0, cols, c);
            let cands = if indexed { index.candidates(c, cols) } else { &every[..] };
            let top = highest(&shapes, surfaces, cands, cx, cz)?;
            let ys = surfaces[top].height.at(cx, cz);
            let h = 0.5 * NAV_CELL;
            let corners = [[cx - h, cz - h], [cx + h, cz - h], [cx - h, cz + h], [cx + h, cz + h]];
            supports(top, &corners, ys, &shapes, surfaces, &joins).then_some((top, ys))
        })
        .collect();
    let sup: Vec<i32> = support.iter().map(|s| s.map_or(-1, |(t, _)| t as i32)).collect();

    // Blocked: the cells whose closed square touches a Walled side.
    let mut blocked = vec![0u8; n];
    for s in surfaces {
        for (k, (a, b)) in edges(&s.poly).enumerate() {
            if s.sides.get(k) == Some(&Side::Walled) {
                mark_touched(&mut blocked, x0, z0, cols, rows, a, b);
            }
        }
    }

    // Lift landings: the supported, unblocked cell of each landing nearest to the join point.
    // The supported, unblocked cells of each surface, in cell order: the nearest is the same cell as a scan of all cells.
    let mut landing: Vec<Vec<usize>> = vec![Vec::new(); surfaces.len()];
    for c in (0..n).filter(|&c| sup[c] >= 0 && blocked[c] == 0) {
        landing[sup[c] as usize].push(c);
    }
    let nearest = |s: usize, at: [f64; 2]| -> Option<usize> {
        landing[s].iter().copied().min_by(|&p, &q| {
            dist2(centre_of(x0, z0, cols, p), at).total_cmp(&dist2(centre_of(x0, z0, cols, q), at))
        })
    };
    let lift_ends: Vec<Option<(usize, usize)>> =
        lifts.iter().map(|&(_, a, b, at)| Some((nearest(a, at)?, nearest(b, at)?))).collect();

    // Exempt: the cell at each link's join point and its 8-neighbours, and the lift landing cells and their neighbours.
    let mut exempt = vec![false; n];
    for link in &set.links {
        if let Some(c) = index_of(x0, z0, cols, rows, link.at[0], link.at[1]) {
            mark_block(&mut exempt, cols, rows, c, 1);
        }
    }
    for &(a, b) in lift_ends.iter().flatten() {
        mark_block(&mut exempt, cols, rows, a, 1);
        mark_block(&mut exempt, cols, rows, b, 1);
    }

    // Inflation (nav-grid.ts rule 9) and the walk flag.
    let touches_blocked = |c: usize| -> bool {
        let (i, j) = ((c % cols) as i64, (c / cols) as i64);
        DIRS8.iter().any(|&(di, dj)| match index_at(i + di, j + dj, cols, rows) {
            Some(d) => blocked[d] == 1,
            None => true,
        })
    };
    let inflated: Vec<u8> = (0..n)
        .map(|c| u8::from(sup[c] >= 0 && blocked[c] == 0 && !exempt[c] && touches_blocked(c)))
        .collect();
    let walk: Vec<u8> = (0..n).map(|c| u8::from(sup[c] >= 0 && blocked[c] == 0 && inflated[c] == 0)).collect();
    let surface: Vec<i32> = (0..n).map(|c| if walk[c] == 1 { sup[c] } else { -1 }).collect();
    let y: Vec<f64> = (0..n).map(|c| support[c].filter(|_| walk[c] == 1).map_or(0.0, |(_, ys)| ys)).collect();

    // Costs (nav-grid.ts rule 10): the distance to the nearest blocked cell, from the 3x3 and 5x5 marks.
    let mut near1 = vec![false; n];
    let mut near2 = vec![false; n];
    for c in (0..n).filter(|&c| blocked[c] == 1) {
        mark_block(&mut near1, cols, rows, c, 1);
        mark_block(&mut near2, cols, rows, c, 2);
    }
    let cost: Vec<f64> = (0..n)
        .map(|c| if near2[c] && !near1[c] { NAV_COST_OPEN + NAV_NEAR_COST } else { NAV_COST_OPEN })
        .collect();

    // Portals (rule 11): one per lift whose two landings are both walk cells (a supported, unblocked, walk landing cell
    // on each side). A lift with a landing over air or behind a wall gets no portal, so it joins nothing.
    // portal[c] is the first portal at c.
    let portals: Vec<Portal> = lifts
        .iter()
        .zip(&lift_ends)
        .filter_map(|(&(li, _, _, _), ends)| {
            let (a, b) = (*ends)?;
            let landed = walk[a] == 1 && walk[b] == 1;
            landed.then(|| Portal { a, b, kind: LinkKind::Lift, cost: NAV_LIFT_COST + (y[b] - y[a]).abs(), link: li })
        })
        .collect();
    let mut portal = vec![-1i32; n];
    for (p, pt) in portals.iter().enumerate() {
        for c in [pt.a, pt.b] {
            if portal[c] < 0 {
                portal[c] = p as i32;
            }
        }
    }

    // The allowed moves out of each walk cell (rule 4 and rule 12).
    let moves: Vec<u8> = (0..n)
        .map(|c| {
            if walk[c] != 1 {
                return 0;
            }
            let (i, j) = ((c % cols) as i64, (c / cols) as i64);
            let mut mask = 0u8;
            for (k, &(di, dj)) in DIRS8.iter().enumerate() {
                let Some(d) = index_at(i + di, j + dj, cols, rows) else {
                    continue;
                };
                if walk[d] != 1 {
                    continue;
                }
                if di != 0 && dj != 0 {
                    let (Some(h), Some(v)) = (index_at(i + di, j, cols, rows), index_at(i, j + dj, cols, rows))
                    else {
                        continue;
                    };
                    if walk[h] != 1 || walk[v] != 1 {
                        continue;
                    }
                }
                let (sc, sd) = (surface[c] as usize, surface[d] as usize);
                let dy = y[d] - y[c];
                if move_kind(dy).is_none() {
                    continue;
                }
                let ok = if sc == sd || joins.contains(&(sc, sd)) {
                    dy.abs() <= NAV_JUMP_UP
                } else {
                    dy <= 0.0 && falls.contains(&(sc, sd))
                };
                if ok {
                    mask |= 1 << k;
                }
            }
            mask
        })
        .collect();

    // Seams: the Step and Ramp crossings between surfaces, per cell and direction (see Grid::seam).
    let seam: Vec<u8> = (0..n)
        .map(|c| {
            if walk[c] != 1 {
                return 0;
            }
            let mut mask = 0u8;
            for k in 0..DIRS8.len() {
                let Some(d) = step(c, k, cols, rows) else {
                    continue;
                };
                if walk[d] == 1 && surface[c] != surface[d] && seams.contains(&(surface[c] as usize, surface[d] as usize))
                {
                    mask |= 1 << k;
                }
            }
            mask
        })
        .collect();

    // Components (rule 12): two-way moves and portals join cells. One-way drops do not.
    let mut parent: Vec<usize> = (0..n).collect();
    for (c, &m) in moves.iter().enumerate() {
        for (k, &opp) in OPP.iter().enumerate() {
            if m & (1 << k) == 0 {
                continue;
            }
            if let Some(d) = step(c, k, cols, rows).filter(|&d| moves[d] & (1 << opp) != 0) {
                union(&mut parent, c, d);
            }
        }
    }
    for pt in &portals {
        union(&mut parent, pt.a, pt.b);
    }
    let mut root_label = vec![-1i32; n];
    let mut component = vec![-1i32; n];
    let mut next = 0i32;
    for (c, &w) in walk.iter().enumerate() {
        if w != 1 {
            continue;
        }
        let r = find(&mut parent, c);
        if root_label[r] < 0 {
            root_label[r] = next;
            next += 1;
        }
        component[c] = root_label[r];
    }
    let mut sizes = vec![0usize; next as usize];
    for &k in component.iter().filter(|&&k| k >= 0) {
        sizes[k as usize] += 1;
    }
    let mut main = -1i32;
    let mut best = 0usize;
    for (k, &s) in sizes.iter().enumerate() {
        if s > best {
            best = s;
            main = k as i32;
        }
    }

    // The one-way moves inside the main component (rule 12).
    let mut drops = Vec::new();
    for (c, &m) in moves.iter().enumerate() {
        if component[c] != main {
            continue;
        }
        for (k, &opp) in OPP.iter().enumerate() {
            if m & (1 << k) == 0 {
                continue;
            }
            if let Some(d) = step(c, k, cols, rows).filter(|&d| moves[d] & (1 << opp) == 0 && component[d] == main) {
                drops.push(Drop { from: c, to: d });
            }
        }
    }

    let fine = fine_field(&shapes, x0, z0, cols, rows);
    Grid {
        cell: NAV_CELL,
        cols,
        rows,
        x0,
        z0,
        walk,
        blocked,
        inflated,
        surface,
        y,
        cost,
        component,
        portal,
        moves,
        seam,
        drops,
        portals,
        main,
        surfaces: surfaces.clone(),
        fine,
    }
}

/// A convex polygon with its orientation and box, for point tests (points within EPS of an edge count as inside).
struct Shape<'a> {
    poly: &'a [[f64; 2]],
    sign: f64,
    min: [f64; 2],
    max: [f64; 2],
}

impl<'a> Shape<'a> {
    fn new(poly: &'a [[f64; 2]]) -> Shape<'a> {
        let area: f64 = edges(poly).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum();
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        for p in poly {
            min = [min[0].min(p[0]), min[1].min(p[1])];
            max = [max[0].max(p[0]), max[1].max(p[1])];
        }
        Shape { poly, sign: if area < 0.0 { -1.0 } else { 1.0 }, min, max }
    }

    fn contains(&self, x: f64, z: f64) -> bool {
        if self.poly.len() < 3
            || x < self.min[0] - EPS
            || x > self.max[0] + EPS
            || z < self.min[1] - EPS
            || z > self.max[1] + EPS
        {
            return false;
        }
        edges(self.poly).all(|(a, b)| {
            let ex = b[0] - a[0];
            let ez = b[1] - a[1];
            let len = (ex * ex + ez * ez).sqrt();
            len == 0.0 || self.sign * (ex * (z - a[1]) - ez * (x - a[0])) / len >= -EPS
        })
    }
}

/// The edges (a, b) of a polygon, in order: side i runs from poly[i] to poly[(i + 1) % n].
fn edges(poly: &[[f64; 2]]) -> impl Iterator<Item = ([f64; 2], [f64; 2])> + '_ {
    poly.iter().copied().zip(poly.iter().copied().cycle().skip(1))
}

/// The grid bounds: the surfaces' box, padded by one cell on the low side. Returns (x0, z0, cols, rows); empty for no
/// surface.
fn bounds(shapes: &[Shape]) -> (f64, f64, usize, usize) {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    for sh in shapes.iter().filter(|s| s.poly.len() >= 3) {
        lo = [lo[0].min(sh.min[0]), lo[1].min(sh.min[1])];
        hi = [hi[0].max(sh.max[0]), hi[1].max(sh.max[1])];
    }
    if lo[0] > hi[0] || lo[1] > hi[1] {
        return (0.0, 0.0, 0, 0);
    }
    let x0 = (lo[0] / NAV_CELL).floor() * NAV_CELL - NAV_CELL;
    let z0 = (lo[1] / NAV_CELL).floor() * NAV_CELL - NAV_CELL;
    let cols = ((hi[0] - x0) / NAV_CELL).ceil() as usize + 1;
    let rows = ((hi[1] - z0) / NAV_CELL).ceil() as usize + 1;
    (x0, z0, cols, rows)
}

/// The shapes bucketed by the cells whose centres they can hold: BUCKET cells square. A cell's candidates are the shapes
/// of its bucket, in ascending index order, so `highest` sees them in the same order as a scan of every shape.
const BUCKET: usize = 8;

struct ShapeIndex {
    bcols: usize,
    buckets: Vec<Vec<usize>>,
}

impl ShapeIndex {
    fn new(shapes: &[Shape], x0: f64, z0: f64, cols: usize, rows: usize) -> ShapeIndex {
        if cols == 0 || rows == 0 {
            return ShapeIndex { bcols: 0, buckets: Vec::new() };
        }
        let bcols = cols.div_ceil(BUCKET);
        let brows = rows.div_ceil(BUCKET);
        let mut buckets = vec![Vec::new(); bcols * brows];
        // The cell range whose centres can lie in lo..hi, padded by a cell so rounding never drops one.
        let span = |lo: f64, hi: f64, o: f64, count: usize| -> (usize, usize) {
            let a = (((lo - o) / NAV_CELL) - 0.5).floor() as i64 - 1;
            let b = (((hi - o) / NAV_CELL) - 0.5).ceil() as i64 + 1;
            let last = count as i64 - 1;
            (a.clamp(0, last) as usize, b.clamp(0, last) as usize)
        };
        for (s, sh) in shapes.iter().enumerate().filter(|(_, sh)| sh.poly.len() >= 3) {
            let (i0, i1) = span(sh.min[0] - EPS, sh.max[0] + EPS, x0, cols);
            let (j0, j1) = span(sh.min[1] - EPS, sh.max[1] + EPS, z0, rows);
            for bj in j0 / BUCKET..=j1 / BUCKET {
                for bi in i0 / BUCKET..=i1 / BUCKET {
                    buckets[bj * bcols + bi].push(s);
                }
            }
        }
        ShapeIndex { bcols, buckets }
    }

    /// The shapes that may contain the centre of cell `c` (cells are `cols` wide), ascending.
    fn candidates(&self, c: usize, cols: usize) -> &[usize] {
        let (i, j) = (c % cols, c / cols);
        &self.buckets[(j / BUCKET) * self.bcols + i / BUCKET]
    }
}

/// The highest surface whose polygon contains (x, z) among `cands` (ascending shape indices; ties go to the lowest index).
fn highest(shapes: &[Shape], surfaces: &[WalkSurface], cands: &[usize], x: f64, z: f64) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for &s in cands {
        let sh = &shapes[s];
        if !sh.contains(x, z) {
            continue;
        }
        let h = surfaces[s].height.at(x, z);
        best = match best {
            Some((_, b)) if b >= h => best,
            _ => Some((s, h)),
        };
    }
    best.map(|(s, _)| s)
}

/// Whether the cell's centre and corners are on `top` or on a surface joined to it within NAV_STEP (the centre's height
/// is `ys`).
fn supports(
    top: usize,
    corners: &[[f64; 2]; 4],
    ys: f64,
    shapes: &[Shape],
    surfaces: &[WalkSurface],
    joins: &BTreeSet<(usize, usize)>,
) -> bool {
    corners.iter().all(|p| {
        shapes[top].contains(p[0], p[1])
            || joins.range((top, 0)..=(top, usize::MAX)).any(|&(_, t)| {
                shapes[t].contains(p[0], p[1]) && (surfaces[t].height.at(p[0], p[1]) - ys).abs() <= NAV_STEP
            })
    })
}

/// Whether the segment a-b meets the closed square lo..hi (with a tolerance).
fn segment_touches(a: [f64; 2], b: [f64; 2], lo: [f64; 2], hi: [f64; 2]) -> bool {
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let tests = [
        (-dx, a[0] - (lo[0] - EPS)),
        (dx, (hi[0] + EPS) - a[0]),
        (-dz, a[1] - (lo[1] - EPS)),
        (dz, (hi[1] + EPS) - a[1]),
    ];
    let mut t0 = 0.0_f64;
    let mut t1 = 1.0_f64;
    for (p, q) in tests {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return false;
            }
            t0 = t0.max(r);
        } else {
            if r < t0 {
                return false;
            }
            t1 = t1.min(r);
        }
    }
    t0 <= t1
}

/// Marks the blocked cells whose closed square touches the segment a-b.
fn mark_touched(blocked: &mut [u8], x0: f64, z0: f64, cols: usize, rows: usize, a: [f64; 2], b: [f64; 2]) {
    let i0 = (((a[0].min(b[0]) - x0) / NAV_CELL).floor() as i64 - 1).max(0);
    let i1 = (((a[0].max(b[0]) - x0) / NAV_CELL).floor() as i64 + 1).min(cols as i64 - 1);
    let j0 = (((a[1].min(b[1]) - z0) / NAV_CELL).floor() as i64 - 1).max(0);
    let j1 = (((a[1].max(b[1]) - z0) / NAV_CELL).floor() as i64 + 1).min(rows as i64 - 1);
    for j in j0..=j1 {
        for i in i0..=i1 {
            let lo = [x0 + i as f64 * NAV_CELL, z0 + j as f64 * NAV_CELL];
            let hi = [lo[0] + NAV_CELL, lo[1] + NAV_CELL];
            if segment_touches(a, b, lo, hi) {
                blocked[j as usize * cols + i as usize] = 1;
            }
        }
    }
}

/// Marks every cell within Chebyshev distance `r` of cell `c`.
fn mark_block(flags: &mut [bool], cols: usize, rows: usize, c: usize, r: i64) {
    let (i, j) = ((c % cols) as i64, (c / cols) as i64);
    for dj in -r..=r {
        for di in -r..=r {
            if let Some(d) = index_at(i + di, j + dj, cols, rows) {
                flags[d] = true;
            }
        }
    }
}

/// The fine height field over the grid bounds (see FineField).
fn fine_field(shapes: &[Shape], x0: f64, z0: f64, cols: usize, rows: usize) -> FineField {
    let per = (NAV_CELL / NAV_FINE).round() as usize;
    let (fcols, frows) = (cols * per, rows * per);
    let mut pairs: Vec<(usize, u32)> = Vec::new();
    if fcols > 0 && frows > 0 {
        let span = |lo: f64, hi: f64, o: f64, count: usize| -> (usize, usize) {
            let a = ((lo - o) / NAV_FINE).floor().max(0.0) as usize;
            let b = (((hi - o) / NAV_FINE).floor().max(0.0) as usize).min(count - 1);
            (a, b)
        };
        for (s, sh) in shapes.iter().enumerate() {
            if sh.poly.len() < 3 {
                continue;
            }
            let (i0, i1) = span(sh.min[0], sh.max[0], x0, fcols);
            let (j0, j1) = span(sh.min[1], sh.max[1], z0, frows);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    pairs.push((j * fcols + i, s as u32));
                }
            }
        }
    }
    pairs.sort_by_key(|p| p.0);
    let total = fcols * frows;
    let mut start = vec![0u32; total + 1];
    for &(k, _) in &pairs {
        start[k + 1] += 1;
    }
    let mut acc = 0u32;
    for v in start.iter_mut() {
        acc += *v;
        *v = acc;
    }
    let items: Vec<u32> = pairs.iter().map(|p| p.1).collect();
    FineField { cols: fcols, rows: frows, x0, z0, start, items }
}

/// The cell containing (x, z) on a grid with these bounds, or None outside it.
fn index_of(x0: f64, z0: f64, cols: usize, rows: usize, x: f64, z: f64) -> Option<usize> {
    let i = ((x - x0) / NAV_CELL).floor();
    let j = ((z - z0) / NAV_CELL).floor();
    if !(0.0..cols as f64).contains(&i) || !(0.0..rows as f64).contains(&j) {
        return None;
    }
    Some(j as usize * cols + i as usize)
}

/// The cell at column i, row j, or None outside the grid.
fn index_at(i: i64, j: i64, cols: usize, rows: usize) -> Option<usize> {
    if i < 0 || j < 0 || i >= cols as i64 || j >= rows as i64 {
        None
    } else {
        Some(j as usize * cols + i as usize)
    }
}

/// The centre of cell `c`.
fn centre_of(x0: f64, z0: f64, cols: usize, c: usize) -> [f64; 2] {
    let (i, j) = (c % cols, c / cols);
    [x0 + (i as f64 + 0.5) * NAV_CELL, z0 + (j as f64 + 0.5) * NAV_CELL]
}

/// The neighbour of cell `c` in direction k (DIRS8), or None off the grid.
fn step(c: usize, k: usize, cols: usize, rows: usize) -> Option<usize> {
    let (di, dj) = DIRS8[k];
    index_at((c % cols) as i64 + di, (c / cols) as i64 + dj, cols, rows)
}

fn dist2(p: [f64; 2], q: [f64; 2]) -> f64 {
    (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)
}

fn find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra < rb {
        parent[rb] = ra;
    } else if rb < ra {
        parent[ra] = rb;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walk::{Plane, SurfaceKind, WalkLink};

    fn rect(x0: f64, z0: f64, x1: f64, z1: f64) -> Vec<[f64; 2]> {
        vec![[x0, z0], [x1, z0], [x1, z1], [x0, z1]]
    }

    fn surf(id: u32, poly: Vec<[f64; 2]>, height: Plane, sides: Vec<Side>) -> WalkSurface {
        WalkSurface { id, kind: SurfaceKind::Plate, poly, height, owner: format!("s{id}"), sides }
    }

    /// The reference build: every cell tested against every shape (test only).
    fn build_brute(set: &WalkSet) -> Grid {
        build_with(set, false)
    }

    #[test]
    fn indexed_build_equals_brute_force_on_real_boards() {
        for kind in ["workstation", "laptop", "edge"] {
            let set = crate::layout::real_walk(kind);
            let t = std::time::Instant::now();
            let fast = build(&set);
            let t_fast = t.elapsed().as_secs_f64();
            let t = std::time::Instant::now();
            let slow = build_brute(&set);
            let t_slow = t.elapsed().as_secs_f64();
            eprintln!("GRID {kind}: indexed build {t_fast:.4} s, brute-force build {t_slow:.4} s");
            assert_eq!(fast, slow, "{kind}: indexed build differs from brute force");
        }
    }

    fn cell_walk(g: &Grid, x: f64, z: f64) -> bool {
        g.cell_at(x, z).is_some_and(|c| g.walk[c] == 1)
    }

    #[test]
    fn flat_square_is_walkable_and_connected() {
        let set = WalkSet {
            surfaces: vec![surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.0), vec![Side::Open; 4])],
            links: vec![],
        };
        let g = build(&set);
        assert!(cell_walk(&g, 1.0, 1.0));
        assert_eq!(g.walk.iter().filter(|&&w| w == 1).count(), 64);
        let comps: BTreeSet<i32> = g.component.iter().copied().filter(|&c| c >= 0).collect();
        assert_eq!(comps.len(), 1);
        assert_eq!(g.main_component(), 0);
    }

    #[test]
    fn island_without_link_is_its_own_component() {
        let set = WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.0), vec![Side::Open; 4]),
                surf(1, rect(5.0, 0.0, 7.0, 2.0), Plane::flat(0.0), vec![Side::Open; 4]),
            ],
            links: vec![],
        };
        let g = build(&set);
        let a = g.cell_at(1.0, 1.0).unwrap();
        let b = g.cell_at(6.0, 1.0).unwrap();
        assert!(g.component_of(a) >= 0 && g.component_of(b) >= 0);
        assert_ne!(g.component_of(a), g.component_of(b));
        assert_eq!(g.component_of(g.cell_at(3.0, 1.0).unwrap()), -1);
    }

    #[test]
    fn walled_side_blocks_and_inflates_with_cost() {
        // The z = 0 side is a wall: the cells touching it are blocked, the next row is inflated, and the row beyond
        // is two cells from a blocked cell (the near cost).
        let mut sides = vec![Side::Open; 4];
        sides[0] = Side::Walled;
        let set = WalkSet { surfaces: vec![surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.0), sides)], links: vec![] };
        let g = build(&set);
        let wall = g.cell_at(1.0, 0.1).unwrap();
        assert_eq!(g.blocked[wall], 1);
        assert!(!cell_walk(&g, 1.0, 0.1));
        assert!(!cell_walk(&g, 1.0, 0.4));
        assert!(cell_walk(&g, 1.0, 0.6));
        let far = g.cell_at(1.0, 0.6).unwrap();
        assert!((g.cost[far] - (NAV_COST_OPEN + NAV_NEAR_COST)).abs() < 1e-9);
    }

    #[test]
    fn build_twice_is_equal() {
        let set = WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.3), vec![Side::Open; 4]),
                surf(1, rect(2.0, 0.0, 4.0, 2.0), Plane { c: 0.2, dx: 0.1, dz: 0.0 }, vec![Side::Open; 4]),
            ],
            links: vec![WalkLink { from: 0, to: 1, kind: LinkKind::Step, via: None, at: [2.0, 1.0] }],
        };
        assert_eq!(build(&set), build(&set));
    }

    fn open() -> Vec<Side> {
        vec![Side::Open; 4]
    }

    /// A bottom landing (y 0) and a top landing (y 2) with a Lift link between them at (2, 1). The top rectangle is
    /// given by the caller, so each test can put it over air or behind walls.
    fn lift_set(top: Vec<[f64; 2]>, top_sides: Vec<Side>) -> WalkSet {
        WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.0), open()),
                surf(1, top, Plane::flat(2.0), top_sides),
            ],
            links: vec![WalkLink { from: 0, to: 1, kind: LinkKind::Lift, via: None, at: [2.0, 1.0] }],
        }
    }

    #[test]
    fn lift_landing_over_air_makes_no_portal() {
        // A top landing 0.1 wide holds no cell centre, so nothing on it is supported: no landing cell, no portal.
        let g = build(&lift_set(rect(4.0, 0.0, 4.1, 2.0), open()));
        assert!(g.portals.is_empty());
        assert!(g.portal.iter().all(|&p| p == -1));
        assert!(!cell_walk(&g, 4.05, 1.0));
        assert_eq!(g.component_of(g.cell_at(4.05, 1.0).unwrap()), -1);
        assert!(cell_walk(&g, 1.0, 1.0));
    }

    #[test]
    fn lift_landing_blocked_makes_no_portal() {
        // A top landing 0.2 wide, walled on all four sides: its only cell centre is touched by a wall, so it is blocked.
        let g = build(&lift_set(rect(4.0, 0.0, 4.2, 2.0), vec![Side::Walled; 4]));
        assert!(g.portals.is_empty());
        assert!(g.portal.iter().all(|&p| p == -1));
        assert!(!cell_walk(&g, 4.125, 1.0));
    }

    #[test]
    fn lift_landings_on_open_ground_make_a_portal() {
        let g = build(&lift_set(rect(4.0, 0.0, 6.0, 2.0), open()));
        assert_eq!(g.portals.len(), 1);
        let pt = &g.portals[0];
        assert!(g.walk[pt.a] == 1 && g.walk[pt.b] == 1);
        assert_eq!(g.component_of(pt.a), g.component_of(pt.b));
        assert_eq!(g.portal[pt.a], 0);
        assert_eq!(g.portal[pt.b], 0);
    }

    #[test]
    fn fine_height_matches_plane() {
        let set = WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.3), vec![Side::Open; 4]),
                surf(1, rect(2.0, 0.0, 4.0, 2.0), Plane { c: 0.2, dx: 0.1, dz: 0.0 }, vec![Side::Open; 4]),
            ],
            links: vec![],
        };
        let g = build(&set);
        for [x, z] in [[0.37, 1.23], [1.0, 1.0], [2.5, 0.7], [3.99, 1.99], [0.0, 0.0]] {
            let want = if x < 2.0 { 0.3 } else { 0.2 + 0.1 * x };
            let got = g.height_at(x, z).expect("a walk surface lies under the point");
            assert!((got - want).abs() < 1e-9, "at ({x}, {z}): {got} vs {want}");
        }
        assert_eq!(g.height_at(-1.0, 0.0), None);
        assert_eq!(g.height_at(10.0, 10.0), None);
        assert_eq!(g.height_at(4.1, 1.0), None);
    }
}
