//! Path planning on the nav grid (grid.rs). Two planners share the grid, the cell moves and the output shape:
//!
//! - `plan`: lazy Theta* (the default). The path is a chain of waypoints; a segment between two waypoints is a straight
//!   line over walk cells (`sight`). The start and the goal are the exact points when they lie on their own walk cell.
//! - `plan_grid`: A* over the cells with the costs of nav-plan.ts. Kept for the tests and the golden fixtures.
//!
//! Shared rules (nav-plan.ts): a point snaps to the nearest walk cell (rings up to 6, ties to the lower index); moves are
//! 8-connected and come from `edge_cost` (a diagonal only past two walk cells, a height step within NAV_JUMP_UP); a jump
//! costs 2 cells extra and a drop 1 cell extra; a portal (a lift) is crossed at its own cost, and only the first portal at
//! a cell is used. A waypoint keeps its y, the portal it was entered by (or -1) and its edge (a jump up or a drop down;
//! a walk step and a portal entry have none).
//!
//! Theta* differences, and why:
//! - The length is Euclidean: a segment is a straight line. So the per-cell costs (NAV_COST_OPEN and NAV_NEAR_COST, the
//!   grid's `cost` array) and the sqrt 2 factor of a diagonal are dropped. A straight segment has no cell cost to pay.
//!   The clearance NAV_NEAR_COST bought is kept by the walk flags instead: an inflated cell is not walk, so a sight line
//!   never runs next to a wall. The jump, drop and portal costs are kept, since they are not segment lengths.
//! - The heuristic is the octile distance between the two points, at scale 1.0 (not 0.7, which explores far more cells
//!   for no gain). Octile is at least the Euclidean distance, so it can overestimate the remaining length by up to about
//!   8 percent. The search then may return a path somewhat longer than the best one; the tests bound it.
//! - Line of sight is checked lazily (Nash's lazy Theta*, one level): when a node is expanded, if its grandparent sees
//!   it, the grandparent becomes its parent. A node entered by a jump, a drop or a portal is never skipped, and neither is
//!   a node whose parent was entered so: those are hard waypoints, and the path keeps them.
//! - The sight line (`sight`) is the supercover of the segment: every cell it touches is a walk cell, every step between
//!   touched cells is within NAV_STEP, a change of surface happens only over a Step or Ramp seam, and a portal cell is
//!   only an endpoint of the line. A jump and a drop are height steps above NAV_STEP, so a sight line never crosses one.
//!
//! Plans are deterministic: the open set orders by (f, cell index) and a closed cell is never reopened.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::f64::consts::SQRT_2;

use serde::{Deserialize, Serialize};

use crate::grid::{move_kind, Grid, DIRS8, NAV_STEP};

/// Rings searched around a point that is off the walk (nav-plan.ts rule 2).
const SNAP_RINGS: i64 = 6;
/// The heuristic's scale for plan_grid: the cheapest step costs 0.7 per unit of cell length.
const H_SCALE: f64 = 0.7;
/// The extra cost of a jump and of a drop, in cells.
const JUMP_COST: f64 = 2.0;
const DROP_COST: f64 = 1.0;

/// The edge a waypoint is entered by. A walk move and a portal entry have none.
///
/// - `Jump`: a rise above NAV_STEP (up to NAV_JUMP_UP), on a two-way join.
/// - `StepDown`: a descent above NAV_STEP over a two-way join (a Step, Ramp or Door link, or two-way on one surface).
/// - `Drop`: a one-way fall, through a Drop link (the reverse move is not allowed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Edge {
    Jump,
    StepDown,
    Drop,
}

/// A waypoint: its (x, z), the height of the surface it stands on, the portal entered (or -1), and its edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathPoint {
    pub x: f64,
    pub z: f64,
    pub y: f64,
    pub portal: i32,
    pub edge: Option<Edge>,
}

/// A planned path: its waypoints and its cost (the A* cost for plan_grid; the Euclidean length plus the jump, drop and
/// portal costs for plan).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Path {
    pub points: Vec<PathPoint>,
    pub cost: f64,
}

/// Plans a path from `from` to `to` (world x, z) with lazy Theta* (see the module docs). None when either end cannot be
/// snapped to a walk cell or no path exists. Deterministic: the same grid and ends give the same path.
pub fn plan(grid: &Grid, from: [f64; 2], to: [f64; 2]) -> Option<Path> {
    let s = snap(grid, from[0], from[1])?;
    let g = snap(grid, to[0], to[1])?;
    if s == g {
        let points = vec![
            PathPoint { x: from[0], z: from[1], y: grid.y[s], portal: -1, edge: None },
            PathPoint { x: to[0], z: to[1], y: grid.y[g], portal: -1, edge: None },
        ];
        return Some(Path { points, cost: dist(from, to) });
    }

    let n = grid.cols * grid.rows;
    let from_in = grid.cell_at(from[0], from[1]) == Some(s);
    let to_in = grid.cell_at(to[0], to[1]) == Some(g);
    // A node's position: its cell centre, or the exact end point when the end lies on its own walk cell.
    let pos = |c: usize| -> [f64; 2] {
        if c == s && from_in {
            from
        } else if c == g && to_in {
            to
        } else {
            grid.cell_centre(c)
        }
    };
    let goal = pos(g);
    let h = |c: usize| -> f64 {
        let p = pos(c);
        let (dx, dz) = ((p[0] - goal[0]).abs(), (p[1] - goal[1]).abs());
        dx.max(dz) + (SQRT_2 - 1.0) * dx.min(dz)
    };

    let mut gcost = vec![f64::INFINITY; n];
    let mut parent = vec![usize::MAX; n];
    let mut via = vec![-1i32; n];
    let mut kind: Vec<Option<Edge>> = vec![None; n];
    let mut closed = vec![false; n];
    let mut heap = BinaryHeap::new();
    gcost[s] = 0.0;
    heap.push(Node { f: h(s), cell: s });

    let mut found = false;
    while let Some(Node { cell: c, .. }) = heap.pop() {
        if closed[c] {
            continue;
        }
        // Lazy line of sight: if the grandparent sees c, c is entered straight from it. Neither c nor its parent may be a
        // hard waypoint (a jump, a drop or a portal entry), since the path must keep those.
        let p = parent[c];
        let gp = if p == usize::MAX { usize::MAX } else { parent[p] };
        if gp != usize::MAX
            && is_walk_entry(kind[c], via[c])
            && is_walk_entry(kind[p], via[p])
            && sight(grid, pos(gp), pos(c))
        {
            // An equal-cost shortcut is taken too: on a straight run it drops the cells in between.
            let through = gcost[gp] + dist(pos(gp), pos(c));
            if through <= gcost[c] + 1e-9 {
                gcost[c] = through;
                parent[c] = gp;
            }
        }

        closed[c] = true;
        if c == g {
            found = true;
            break;
        }
        let gc = gcost[c];
        for k in 0..DIRS8.len() {
            let Some((d, _, edge)) = edge_cost(grid, c, k) else {
                continue;
            };
            if closed[d] {
                continue;
            }
            let extra = match edge {
                Some(Edge::Jump) => JUMP_COST * grid.cell,
                Some(Edge::Drop | Edge::StepDown) => DROP_COST * grid.cell,
                None => 0.0,
            };
            let gn = gc + dist(pos(c), pos(d)) + extra;
            if gn < gcost[d] {
                gcost[d] = gn;
                parent[d] = c;
                via[d] = -1;
                kind[d] = edge;
                heap.push(Node { f: gn + h(d), cell: d });
            }
        }
        let p = grid.portal[c];
        if p >= 0 {
            let pt = &grid.portals[p as usize];
            let o = if c == pt.a {
                pt.b
            } else if c == pt.b {
                pt.a
            } else {
                usize::MAX
            };
            if o < n && grid.walk[o] == 1 && !closed[o] {
                let gn = gc + pt.cost;
                if gn < gcost[o] {
                    gcost[o] = gn;
                    parent[o] = c;
                    via[o] = p;
                    kind[o] = None;
                    heap.push(Node { f: gn + h(o), cell: o });
                }
            }
        }
    }
    if !found {
        return None;
    }

    let mut cells = vec![g];
    let mut c = g;
    while c != s {
        c = parent[c];
        cells.push(c);
    }
    cells.reverse();

    let mut points = Vec::with_capacity(cells.len() + 1);
    points.push(PathPoint { x: from[0], z: from[1], y: grid.y[s], portal: -1, edge: None });
    for &c in &cells[1..] {
        let [x, z] = pos(c);
        points.push(PathPoint { x, z, y: grid.y[c], portal: via[c], edge: kind[c] });
    }
    if !to_in {
        points.push(PathPoint { x: to[0], z: to[1], y: grid.y[g], portal: -1, edge: None });
    }
    let mut cost = gcost[g];
    if !from_in {
        cost += dist(from, pos(s));
    }
    if !to_in {
        cost += dist(pos(g), to);
    }
    Some(Path { points, cost })
}

/// Plans a path from `from` to `to` (world x, z) by A* over the cells with the TS costs of nav-plan.ts: a walk cell costs
/// its cell length times its cell cost (times sqrt 2 on a diagonal), a jump and a drop add their cells, and the heuristic
/// is 0.7 of the octile distance. None when either end cannot be snapped to a walk cell or no path exists. Deterministic.
pub fn plan_grid(grid: &Grid, from: [f64; 2], to: [f64; 2]) -> Option<Path> {
    let s = snap(grid, from[0], from[1])?;
    let g = snap(grid, to[0], to[1])?;
    if s == g {
        let points = vec![
            PathPoint { x: from[0], z: from[1], y: grid.y[s], portal: -1, edge: None },
            PathPoint { x: to[0], z: to[1], y: grid.y[g], portal: -1, edge: None },
        ];
        return Some(Path { points, cost: 0.0 });
    }

    let n = grid.cols * grid.rows;
    let cell = grid.cell;
    let (gi, gj) = ((g % grid.cols) as i64, (g / grid.cols) as i64);
    let h = |c: usize| -> f64 {
        let di = ((c % grid.cols) as i64 - gi).abs() as f64;
        let dj = ((c / grid.cols) as i64 - gj).abs() as f64;
        H_SCALE * cell * (di.max(dj) + (SQRT_2 - 1.0) * di.min(dj))
    };

    let mut gcost = vec![f64::INFINITY; n];
    let mut parent = vec![usize::MAX; n];
    let mut via = vec![-1i32; n];
    let mut kind: Vec<Option<Edge>> = vec![None; n];
    let mut closed = vec![false; n];
    let mut heap = BinaryHeap::new();
    gcost[s] = 0.0;
    heap.push(Node { f: h(s), cell: s });

    let mut found = false;
    while let Some(Node { cell: c, .. }) = heap.pop() {
        if closed[c] {
            continue;
        }
        closed[c] = true;
        if c == g {
            found = true;
            break;
        }
        let gc = gcost[c];
        for k in 0..DIRS8.len() {
            let Some((d, step, edge)) = edge_cost(grid, c, k) else {
                continue;
            };
            if closed[d] {
                continue;
            }
            let gn = gc + step;
            if gn < gcost[d] {
                gcost[d] = gn;
                parent[d] = c;
                via[d] = -1;
                kind[d] = edge;
                heap.push(Node { f: gn + h(d), cell: d });
            }
        }
        let p = grid.portal[c];
        if p >= 0 {
            let pt = &grid.portals[p as usize];
            let o = if c == pt.a {
                pt.b
            } else if c == pt.b {
                pt.a
            } else {
                usize::MAX
            };
            if o < n && grid.walk[o] == 1 && !closed[o] {
                let gn = gc + pt.cost;
                if gn < gcost[o] {
                    gcost[o] = gn;
                    parent[o] = c;
                    via[o] = p;
                    kind[o] = None;
                    heap.push(Node { f: gn + h(o), cell: o });
                }
            }
        }
    }
    if !found {
        return None;
    }

    let mut cells = vec![g];
    let mut c = g;
    while c != s {
        c = parent[c];
        cells.push(c);
    }
    cells.reverse();

    let mut points = Vec::with_capacity(cells.len() + 1);
    points.push(PathPoint { x: from[0], z: from[1], y: grid.y[s], portal: -1, edge: None });
    for &c in &cells[1..] {
        let [x, z] = grid.cell_centre(c);
        points.push(PathPoint { x, z, y: grid.y[c], portal: via[c], edge: kind[c] });
    }
    points.push(PathPoint { x: to[0], z: to[1], y: grid.y[g], portal: -1, edge: None });
    Some(Path { points, cost: gcost[g] })
}

/// The 8-connected move from cell `c` in direction `k` (an index into DIRS8): the neighbour cell, the cost of the move
/// (the cell length, times the diagonal factor, times the neighbour's cell cost, plus the jump or drop cost) and the edge
/// it is entered by. None when the move is not allowed: no move bit, off the grid, or a height difference that is no move.
pub(crate) fn edge_cost(grid: &Grid, c: usize, k: usize) -> Option<(usize, f64, Option<Edge>)> {
    if grid.moves[c] & (1 << k) == 0 {
        return None;
    }
    let d = grid.neighbour(c, k)?;
    let (di, dj) = DIRS8[k];
    let mk = move_kind(grid.y[d] - grid.y[c])?;
    let m = if di != 0 && dj != 0 { SQRT_2 } else { 1.0 };
    // A fall is one way when the reverse move is not allowed (the Drop link); otherwise it is a two-way step down.
    let rev = DIRS8.iter().position(|&(a, b)| (a, b) == (-di, -dj)).unwrap_or(k);
    let (edge, extra) = match mk {
        1 => (Some(Edge::Jump), JUMP_COST * grid.cell),
        2 if grid.moves[d] & (1 << rev) == 0 => (Some(Edge::Drop), DROP_COST * grid.cell),
        2 => (Some(Edge::StepDown), DROP_COST * grid.cell),
        _ => (None, 0.0),
    };
    Some((d, grid.cell * m * grid.cost[d] + extra, edge))
}

/// Whether a straight segment a-b is a sight line over the walk (see the module docs). Both ends must lie on the grid.
/// The cells the segment touches are visited in order (a corner it passes through touches both cells beside it), and each
/// step between two of them must keep the rules of `sight_step`.
pub(crate) fn sight(grid: &Grid, a: [f64; 2], b: [f64; 2]) -> bool {
    let inv = 1.0 / grid.cell;
    let (u0, v0) = ((a[0] - grid.x0) * inv, (a[1] - grid.z0) * inv);
    let (u1, v1) = ((b[0] - grid.x0) * inv, (b[1] - grid.z0) * inv);
    let (cols, rows) = (grid.cols as i64, grid.rows as i64);
    let (i0, j0) = (u0.floor() as i64, v0.floor() as i64);
    let (ie, je) = (u1.floor() as i64, v1.floor() as i64);
    let inside = |i: i64, j: i64| (0..cols).contains(&i) && (0..rows).contains(&j);
    if !inside(i0, j0) || !inside(ie, je) {
        return false;
    }
    let index = |i: i64, j: i64| (j * cols + i) as usize;
    let (start, end) = (index(i0, j0), index(ie, je));
    if grid.walk[start] != 1 {
        return false;
    }
    let ends = [start, end];
    let (sx, sz) = ((ie - i0).signum(), (je - j0).signum());
    let (nx, nz) = ((ie - i0).abs(), (je - j0).abs());
    let (du, dv) = (u1 - u0, v1 - v0);
    // The parameter t at which the segment crosses the k-th vertical (x) and horizontal (z) cell boundary.
    let tx = |k: i64| -> f64 {
        let bx = if sx > 0 { i0 + k } else { i0 + 1 - k };
        (bx as f64 - u0) / du
    };
    let tz = |k: i64| -> f64 {
        let bz = if sz > 0 { j0 + k } else { j0 + 1 - k };
        (bz as f64 - v0) / dv
    };

    let (mut i, mut j, mut prev) = (i0, j0, start);
    let (mut kx, mut kz) = (0i64, 0i64);
    while kx < nx || kz < nz {
        let t_x = if kx < nx { tx(kx + 1) } else { f64::INFINITY };
        let t_z = if kz < nz { tz(kz + 1) } else { f64::INFINITY };
        if kx < nx && kz < nz && (t_x - t_z).abs() <= 1e-9 {
            // The segment passes a corner: it touches the two cells beside it and the diagonal one.
            let (bi, bj) = (i + sx, j + sz);
            for c in [index(bi, j), index(i, bj), index(bi, bj)] {
                if !sight_step(grid, prev, c, ends) {
                    return false;
                }
                prev = c;
            }
            i = bi;
            j = bj;
            kx += 1;
            kz += 1;
        } else {
            if t_x < t_z {
                i += sx;
                kx += 1;
            } else {
                j += sz;
                kz += 1;
            }
            let c = index(i, j);
            if !sight_step(grid, prev, c, ends) {
                return false;
            }
            prev = c;
        }
    }
    true
}

/// The rules for a sight line to pass from touched cell `p` to touched cell `c`: `c` is walk; a portal cell is only an end
/// of the line; a portal is never crossed; the height step is within NAV_STEP (so no jump or drop); and a change of
/// surface is a Step or Ramp seam from `p`.
fn sight_step(grid: &Grid, p: usize, c: usize, ends: [usize; 2]) -> bool {
    if grid.walk[c] != 1 || (grid.portal[c] >= 0 && !ends.contains(&c)) {
        return false;
    }
    if grid.portal[p] >= 0 && grid.portal[c] >= 0 {
        return false;
    }
    if (grid.y[c] - grid.y[p]).abs() > NAV_STEP {
        return false;
    }
    if grid.surface[c] != grid.surface[p] {
        let cols = grid.cols;
        let d = ((c % cols) as i64 - (p % cols) as i64, (c / cols) as i64 - (p / cols) as i64);
        let Some(k) = DIRS8.iter().position(|&e| e == d) else {
            return false;
        };
        if grid.seam[p] & (1 << k) == 0 {
            return false;
        }
    }
    true
}

/// Whether a node is entered by a walk step (no jump, no drop and no portal): only those may be skipped by a sight line.
fn is_walk_entry(kind: Option<Edge>, via: i32) -> bool {
    kind.is_none() && via < 0
}

/// Euclidean distance between two points.
fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// The open-set entry: ordered by f, then by cell index (the lower index first).
#[derive(Clone, Copy)]
struct Node {
    f: f64,
    cell: usize,
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Node {}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Node {
    /// Reversed, so that the binary max-heap pops the smallest (f, cell) first.
    fn cmp(&self, other: &Self) -> Ordering {
        other.f.total_cmp(&self.f).then_with(|| other.cell.cmp(&self.cell))
    }
}

/// The walk cell a point snaps to: its own cell when it is walk, else the nearest walk cell within SNAP_RINGS rings
/// (ties to the lower index). None when there is none.
fn snap(grid: &Grid, x: f64, z: f64) -> Option<usize> {
    if let Some(c) = grid.cell_at(x, z).filter(|&c| grid.walk[c] == 1) {
        return Some(c);
    }
    let ci = ((x - grid.x0) / grid.cell).floor() as i64;
    let cj = ((z - grid.z0) / grid.cell).floor() as i64;
    let (cols, rows) = (grid.cols as i64, grid.rows as i64);
    for r in 1..=SNAP_RINGS {
        let mut best: Option<(f64, usize)> = None;
        for j in (cj - r).max(0)..=(cj + r).min(rows - 1) {
            for i in (ci - r).max(0)..=(ci + r).min(cols - 1) {
                if (i - ci).abs().max((j - cj).abs()) != r {
                    continue;
                }
                let c = (j * cols + i) as usize;
                if grid.walk[c] != 1 {
                    continue;
                }
                let [cx, cz] = grid.cell_centre(c);
                let d = (cx - x).powi(2) + (cz - z).powi(2);
                let better = match best {
                    None => true,
                    Some((bd, bc)) => match d.total_cmp(&bd) {
                        Ordering::Less => true,
                        Ordering::Equal => c < bc,
                        Ordering::Greater => false,
                    },
                };
                if better {
                    best = Some((d, c));
                }
            }
        }
        if let Some((_, c)) = best {
            return Some(c);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::build;
    use crate::walk::{LinkKind, Plane, Side, SurfaceKind, WalkLink, WalkSet, WalkSurface};

    fn rect(x0: f64, z0: f64, x1: f64, z1: f64) -> Vec<[f64; 2]> {
        vec![[x0, z0], [x1, z0], [x1, z1], [x0, z1]]
    }

    fn surf(id: u32, poly: Vec<[f64; 2]>, height: Plane, sides: Vec<Side>) -> WalkSurface {
        WalkSurface { id, kind: SurfaceKind::Plate, poly, height, owner: format!("s{id}"), sides }
    }

    fn link(from: u32, to: u32, kind: LinkKind, at: [f64; 2]) -> WalkLink {
        WalkLink { from, to, kind, via: None, at }
    }

    fn open() -> Vec<Side> {
        vec![Side::Open; 4]
    }

    /// The length of a path's polyline.
    fn polyline(points: &[PathPoint]) -> f64 {
        points.windows(2).map(|w| dist([w[0].x, w[0].z], [w[1].x, w[1].z])).sum()
    }

    /// Whether every sampled point of every segment lies on a walk cell (blocked and inflated cells are not walk).
    fn all_walk(g: &Grid, points: &[PathPoint]) -> bool {
        points.windows(2).all(|w| {
            (0..=400).all(|k| {
                let t = k as f64 / 400.0;
                let (x, z) = (w[0].x + (w[1].x - w[0].x) * t, w[0].z + (w[1].z - w[0].z) * t);
                g.cell_at(x, z).is_some_and(|c| g.walk[c] == 1)
            })
        })
    }

    /// Floor H (high, y 1.0) and floor L (low, y 0.0) side by side, a Drop link from H to L; both are two-way joined to
    /// the strip X (y 0.5) at z 2 to 4, so the main component holds all three.
    fn drop_set() -> WalkSet {
        let mut h_sides = open();
        h_sides[1] = Side::Drop;
        WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(1.0), h_sides),
                surf(1, rect(2.0, 0.0, 4.0, 2.0), Plane::flat(0.0), open()),
                surf(2, rect(0.0, 2.0, 4.0, 4.0), Plane::flat(0.5), open()),
            ],
            links: vec![
                link(0, 2, LinkKind::Step, [1.0, 2.0]),
                link(2, 1, LinkKind::Step, [3.0, 2.0]),
                link(0, 1, LinkKind::Drop, [2.0, 1.0]),
            ],
        }
    }

    /// Floor A (y 0) and a surface B (y 0.5) side by side with a Step link between them: a rise of 0.5 is a jump.
    fn jump_set() -> WalkSet {
        WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.0), open()),
                surf(1, rect(2.0, 0.0, 4.0, 2.0), Plane::flat(0.5), open()),
            ],
            links: vec![link(0, 1, LinkKind::Step, [2.0, 1.0])],
        }
    }

    /// Floor (0 to 12 square) with a raised block (height 1) on x 4 to 6, z 0 to 8, walled on all four sides. The gap is
    /// at the far end (z above 8.5 after inflation), so a route from x 1 to x 11 must go round the top of the block.
    fn wall_set() -> WalkSet {
        WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 12.0, 12.0), Plane::flat(0.0), open()),
                surf(1, rect(4.0, 0.0, 6.0, 8.0), Plane::flat(1.0), vec![Side::Walled; 4]),
            ],
            links: vec![],
        }
    }

    /// A bottom landing (y 0, x 0 to 2) and a top landing (y 2) with a Lift link between them at (2, 1). The top is
    /// given by the caller.
    fn lift_set(top: Vec<[f64; 2]>, top_sides: Vec<Side>) -> WalkSet {
        WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.0), open()),
                surf(1, top, Plane::flat(2.0), top_sides),
            ],
            links: vec![link(0, 1, LinkKind::Lift, [2.0, 1.0])],
        }
    }

    #[test]
    fn flat_square_plans_a_path() {
        let set = WalkSet { surfaces: vec![surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.0), open())], links: vec![] };
        let g = build(&set);
        let p = plan_grid(&g, [0.3, 0.3], [1.7, 1.7]).expect("a path");
        assert!(p.cost > 0.0);
        assert_eq!(p.points.first().unwrap().x, 0.3);
        assert_eq!(p.points.last().unwrap().z, 1.7);
        assert!(p.points.iter().all(|pt| pt.edge.is_none() && pt.portal == -1));
    }

    #[test]
    fn step_of_0_2_across_a_step_link_is_walked() {
        let set = WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.0), open()),
                surf(1, rect(2.0, 0.0, 4.0, 2.0), Plane::flat(0.2), open()),
            ],
            links: vec![link(0, 1, LinkKind::Step, [2.0, 1.0])],
        };
        let g = build(&set);
        let p = plan_grid(&g, [1.0, 1.0], [3.0, 1.0]).expect("a walk across the step");
        assert!(p.points.iter().any(|pt| (pt.y - 0.2).abs() < 1e-9));
        assert!(p.points.iter().all(|pt| pt.edge.is_none()));
    }

    #[test]
    fn rise_of_0_5_is_a_jump_edge() {
        let g = build(&jump_set());
        let p = plan_grid(&g, [1.0, 1.0], [3.0, 1.0]).expect("a jump");
        assert!(p.points.iter().any(|pt| pt.edge == Some(Edge::Jump)));
    }

    #[test]
    fn rise_of_1_0_with_a_step_link_is_not_planned() {
        let set = WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.0), open()),
                surf(1, rect(2.0, 0.0, 4.0, 2.0), Plane::flat(1.0), open()),
            ],
            links: vec![link(0, 1, LinkKind::Step, [2.0, 1.0])],
        };
        let g = build(&set);
        assert!(plan_grid(&g, [1.0, 1.0], [3.0, 1.0]).is_none());
        assert!(plan_grid(&g, [3.0, 1.0], [1.0, 1.0]).is_none());
        assert!(plan(&g, [1.0, 1.0], [3.0, 1.0]).is_none());
    }

    #[test]
    fn drop_link_is_one_way_downhill() {
        let g = build(&drop_set());
        let hi = g.cell_at(1.875, 1.0).unwrap();
        let lo = g.cell_at(2.125, 1.0).unwrap();
        assert!(g.drops.contains(&crate::grid::Drop { from: hi, to: lo }));
        assert_eq!(g.moves[lo] & (1 << 1), 0, "no uphill move from the low cell across the drop");

        let down = plan_grid(&g, [1.0, 1.0], [3.0, 1.0]).expect("the drop is planned");
        assert!(down.points.iter().any(|pt| pt.edge == Some(Edge::Drop)));

        // Uphill from L to H must go round through the strip, never straight across the drop.
        let up = plan_grid(&g, [3.0, 1.0], [1.0, 1.0]).expect("the way round");
        assert!(up.points.windows(2).all(|w| {
            !(w[0].x > 2.0 && w[1].x < 2.0 && w[0].z < 2.0 && w[1].z < 2.0)
        }));
    }

    #[test]
    fn lift_portal_joins_two_landings() {
        let set = WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.0), open()),
                surf(1, rect(4.0, 0.0, 6.0, 2.0), Plane::flat(2.0), open()),
            ],
            links: vec![link(0, 1, LinkKind::Lift, [2.0, 1.0])],
        };
        let g = build(&set);
        assert_eq!(g.portals.len(), 1);
        let pt = &g.portals[0];
        assert_eq!(pt.kind, LinkKind::Lift);
        assert!((pt.cost - (4.0 + 2.0)).abs() < 1e-9);
        assert_eq!(g.component_of(pt.a), g.component_of(pt.b));

        let p = plan_grid(&g, [0.5, 1.0], [5.0, 1.0]).expect("a ride up");
        assert!(p.points.iter().any(|q| q.portal == 0));
        assert!(p.cost >= 6.0);
    }

    #[test]
    fn walled_block_is_planned_round() {
        // A floor with a raised plate in its middle. The plate's sides are walls, so the straight route is cut and A*
        // goes round the top of the plate (z above 4).
        let set = WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 6.0, 6.0), Plane::flat(0.0), open()),
                surf(1, rect(2.0, 0.0, 4.0, 4.0), Plane::flat(1.0), vec![Side::Walled; 4]),
            ],
            links: vec![],
        };
        let g = build(&set);
        let straight = ((5.5f64 - 0.5).powi(2)).sqrt();
        let p = plan_grid(&g, [0.5, 3.0], [5.5, 3.0]).expect("a way round");
        assert!(p.cost > straight);
        assert!(p.points.iter().all(|q| !(q.x > 2.0 && q.x < 4.0) || q.z > 4.0));
    }

    #[test]
    fn unreachable_island_has_no_path() {
        let set = WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.0), open()),
                surf(1, rect(5.0, 0.0, 7.0, 2.0), Plane::flat(0.0), open()),
            ],
            links: vec![],
        };
        let g = build(&set);
        assert!(plan_grid(&g, [1.0, 1.0], [6.0, 1.0]).is_none());
        assert!(plan(&g, [1.0, 1.0], [6.0, 1.0]).is_none());
    }

    #[test]
    fn plan_twice_is_equal() {
        let g = build(&drop_set());
        assert_eq!(plan_grid(&g, [1.0, 1.0], [3.0, 1.0]), plan_grid(&g, [1.0, 1.0], [3.0, 1.0]));
    }

    #[test]
    fn edge_cost_gives_the_jump_and_the_drop_and_refuses_the_rest() {
        let g = build(&jump_set());
        let a = g.cell_at(1.875, 1.0).unwrap();
        let b = g.cell_at(2.125, 1.0).unwrap();
        let east = DIRS8.iter().position(|&d| d == (1, 0)).unwrap();
        let west = DIRS8.iter().position(|&d| d == (-1, 0)).unwrap();
        let up = edge_cost(&g, a, east).expect("a jump up is a move");
        assert_eq!((up.0, up.2), (b, Some(Edge::Jump)));
        assert!((up.1 - (g.cell * g.cost[b] + JUMP_COST * g.cell)).abs() < 1e-12);
        let down = edge_cost(&g, b, west).expect("a two-way step down is a move");
        assert_eq!((down.0, down.2), (a, Some(Edge::StepDown)));
        let walk = edge_cost(&g, a, west).expect("a walk step on the low floor");
        assert_eq!(walk.2, None);
        // A move off the far edge of the high floor (no walk cell beyond it) gives no edge.
        let edge = g.cell_at(3.875, 1.0).unwrap();
        assert!(edge_cost(&g, edge, east).is_none());
    }

    #[test]
    fn two_way_step_down_is_stepdown_and_drop_link_fall_is_drop() {
        // jump_set joins A and B with a Step link, so the 0.5 descent from B to A is two-way: StepDown.
        let two_way = build(&jump_set());
        let (a, b) = (two_way.cell_at(1.875, 1.0).unwrap(), two_way.cell_at(2.125, 1.0).unwrap());
        let west = DIRS8.iter().position(|&d| d == (-1, 0)).unwrap();
        assert_eq!(edge_cost(&two_way, b, west).map(|e| (e.0, e.2)), Some((a, Some(Edge::StepDown))));
        let p = plan(&two_way, [2.5, 1.0], [1.5, 1.0]).expect("a step down and back");
        assert!(p.points.iter().any(|q| q.edge == Some(Edge::StepDown)));
        assert!(!p.points.iter().any(|q| q.edge == Some(Edge::Drop)));

        // drop_set's H to L fall goes through a Drop link only: the reverse move is refused, so it is Drop.
        let one_way = build(&drop_set());
        let (hi, lo) = (one_way.cell_at(1.875, 1.0).unwrap(), one_way.cell_at(2.125, 1.0).unwrap());
        let east = DIRS8.iter().position(|&d| d == (1, 0)).unwrap();
        assert_eq!(edge_cost(&one_way, hi, east).map(|e| (e.0, e.2)), Some((lo, Some(Edge::Drop))));
        assert_eq!(one_way.moves[lo] & (1 << west), 0, "no way back up across the drop");
    }

    #[test]
    fn theta_open_ground_is_straight_within_2_percent() {
        let set = WalkSet { surfaces: vec![surf(0, rect(0.0, 0.0, 10.0, 10.0), Plane::flat(0.0), open())], links: vec![] };
        let g = build(&set);
        let (from, to) = ([0.6, 0.9], [9.1, 7.3]);
        let p = plan(&g, from, to).expect("a path");
        let straight = dist(from, to);
        let len = polyline(&p.points);
        eprintln!("theta open ground: length {len:.4} straight {straight:.4} ratio {:.4}", len / straight);
        assert!(len <= 1.02 * straight, "length {len} against straight {straight}");
        assert!(all_walk(&g, &p.points));
    }

    #[test]
    fn theta_around_one_wall_is_within_5_percent_of_the_polyline() {
        // The inflated block is x 3.5 to 6.5, z 0 to 8.5 (the wall, its blocked cells and one ring of inflation), so the
        // true shortest polyline from (1, 2) to (11, 2) goes round its top corners:
        //   (1, 2) -> (3.5, 8.5) -> (6.5, 8.5) -> (11, 2)
        // with length sqrt(48.5) + 3 + sqrt(62.5) = 17.870. A sight line cannot touch the inflated corner cell itself,
        // so the planner bends at the cell centres just outside it; the bound allows that.
        let g = build(&wall_set());
        let p = plan(&g, [1.0, 2.0], [11.0, 2.0]).expect("a way round");
        let reference = (48.5f64).sqrt() + 3.0 + (62.5f64).sqrt();
        let len = polyline(&p.points);
        eprintln!("theta round one wall: length {len:.4} polyline {reference:.4} ratio {:.4}", len / reference);
        assert!(len <= 1.05 * reference, "length {len} against polyline {reference}");
        assert!(all_walk(&g, &p.points));
        let grid_len = polyline(&plan_grid(&g, [1.0, 2.0], [11.0, 2.0]).unwrap().points);
        assert!(len <= grid_len + 1e-9, "theta {len} must not be longer than the grid path {grid_len}");
    }

    #[test]
    fn theta_never_cuts_a_walled_corner() {
        // The straight line from (2, 7) to (8, 9) runs through the inflated block's top corner. The path must go round
        // the corner at (3.5, 8.5) and (6.5, 8.5): reference length sqrt(4.5) + 3 + sqrt(2.25 + 0.25) (about 6.70).
        let g = build(&wall_set());
        assert!(!sight(&g, [2.0, 7.0], [8.0, 9.0]), "the straight line is not a sight line");
        let p = plan(&g, [2.0, 7.0], [8.0, 9.0]).expect("a way round");
        assert!(all_walk(&g, &p.points), "no segment touches a blocked or inflated cell");
        let reference = (4.5f64).sqrt() + 3.0 + (2.5f64).sqrt();
        let len = polyline(&p.points);
        eprintln!("theta walled corner: length {len:.4} polyline {reference:.4} ratio {:.4}", len / reference);
        assert!(len <= 1.05 * reference, "length {len} against polyline {reference}");
        assert!(len > dist([2.0, 7.0], [8.0, 9.0]));
    }

    #[test]
    fn theta_keeps_a_jump_as_a_waypoint() {
        let g = build(&jump_set());
        assert!(!sight(&g, [1.0, 1.0], [3.0, 1.0]), "a straight line over the jump is refused");
        assert!(!sight(&g, [1.7, 1.0], [2.3, 1.0]));
        let p = plan(&g, [1.0, 1.0], [3.0, 1.0]).expect("a jump");
        let jump = p.points.iter().find(|q| q.edge == Some(Edge::Jump)).expect("the jump is kept");
        assert!(jump.x > 2.0 && (jump.y - 0.5).abs() < 1e-9);
    }

    #[test]
    fn theta_keeps_a_drop_as_a_waypoint() {
        let g = build(&drop_set());
        assert!(!sight(&g, [1.0, 1.0], [3.0, 1.0]), "a straight line over the drop is refused");
        let down = plan(&g, [1.0, 1.0], [3.0, 1.0]).expect("the drop is planned");
        let drop = down.points.iter().find(|q| q.edge == Some(Edge::Drop)).expect("the drop is kept");
        assert!(drop.x > 2.0 && drop.y.abs() < 1e-9);

        let up = plan(&g, [3.0, 1.0], [1.0, 1.0]).expect("the way round");
        assert!(up.points.windows(2).all(|w| !(w[0].x > 2.0 && w[1].x < 2.0 && w[0].z < 2.0 && w[1].z < 2.0)));
    }

    #[test]
    fn theta_keeps_a_lift_portal_as_a_waypoint() {
        // The two landings touch across x = 2: the only way between them is the lift, so a sight line is refused and the
        // portal entry stays a waypoint.
        let g = build(&lift_set(rect(2.0, 0.0, 4.0, 2.0), open()));
        assert_eq!(g.portals.len(), 1);
        assert!(!sight(&g, [1.7, 1.0], [2.3, 1.0]), "a sight line over the portal is refused");
        let p = plan(&g, [0.5, 1.0], [3.5, 1.0]).expect("a ride up");
        assert!(p.points.iter().any(|q| q.portal == 0), "the portal waypoint is kept");
        assert!(p.cost >= 6.0);
    }

    #[test]
    fn theta_lift_over_air_or_blocked_has_no_path() {
        let air = build(&lift_set(rect(4.0, 0.0, 4.1, 2.0), open()));
        let blocked = build(&lift_set(rect(4.0, 0.0, 4.2, 2.0), vec![Side::Walled; 4]));
        for g in [&air, &blocked] {
            assert!(g.portals.is_empty());
            assert!(plan(g, [0.5, 1.0], [4.05, 1.0]).is_none());
            assert!(plan_grid(g, [0.5, 1.0], [4.05, 1.0]).is_none());
            assert!(plan(g, [0.5, 1.0], [1.5, 1.0]).is_some(), "the bottom landing still plans");
        }
    }

    #[test]
    fn theta_is_deterministic() {
        let g = build(&wall_set());
        assert_eq!(plan(&g, [1.0, 2.0], [11.0, 2.0]), plan(&g, [1.0, 2.0], [11.0, 2.0]));
    }

    #[test]
    fn theta_and_grid_agree_where_theta_cannot_improve() {
        // Two cell centres on one row of open ground: the grid path is already straight, so Theta* gives the same length
        // with its two ends as the only waypoints.
        let set = WalkSet { surfaces: vec![surf(0, rect(0.0, 0.0, 10.0, 10.0), Plane::flat(0.0), open())], links: vec![] };
        let g = build(&set);
        let from = g.cell_centre(g.cell_at(1.0, 4.0).unwrap());
        let to = g.cell_centre(g.cell_at(8.0, 4.0).unwrap());
        let grid = plan_grid(&g, from, to).expect("a grid path");
        let theta = plan(&g, from, to).expect("a theta path");
        assert!((grid.cost - theta.cost).abs() < 1e-9, "costs {} and {}", grid.cost, theta.cost);
        assert!((polyline(&grid.points) - polyline(&theta.points)).abs() < 1e-9);
        assert_eq!(theta.points.len(), 2);
        assert_eq!((theta.points[0].x, theta.points[0].z), (from[0], from[1]));
        assert_eq!((theta.points[1].x, theta.points[1].z), (to[0], to[1]));
    }

    #[test]
    fn theta_follows_the_grid_rules_on_the_original_cases() {
        // Each case plans (or refuses) the same way under both planners.
        let step = build(&WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.0), open()),
                surf(1, rect(2.0, 0.0, 4.0, 2.0), Plane::flat(0.2), open()),
            ],
            links: vec![link(0, 1, LinkKind::Step, [2.0, 1.0])],
        });
        let p = plan(&step, [1.0, 1.0], [3.0, 1.0]).expect("a walk across the step");
        assert!(p.points.iter().any(|pt| (pt.y - 0.2).abs() < 1e-9));
        assert!(p.points.iter().all(|pt| pt.edge.is_none()));

        let island = build(&WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.0), open()),
                surf(1, rect(5.0, 0.0, 7.0, 2.0), Plane::flat(0.0), open()),
            ],
            links: vec![],
        });
        assert!(plan(&island, [1.0, 1.0], [6.0, 1.0]).is_none());

        let lift = build(&WalkSet {
            surfaces: vec![
                surf(0, rect(0.0, 0.0, 2.0, 2.0), Plane::flat(0.0), open()),
                surf(1, rect(4.0, 0.0, 6.0, 2.0), Plane::flat(2.0), open()),
            ],
            links: vec![link(0, 1, LinkKind::Lift, [2.0, 1.0])],
        });
        let ride = plan(&lift, [0.5, 1.0], [5.0, 1.0]).expect("a ride up");
        assert!(ride.points.iter().any(|q| q.portal == 0));
        assert!(ride.cost >= 6.0);
    }
}
