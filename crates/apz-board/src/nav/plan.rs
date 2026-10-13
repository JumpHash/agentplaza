//! Path planning on the nav grid (nav-plan.ts): A* over the plannable cells, string pulling, one corner-cutting pass,
//! and the fixture hash. The TS file's RULES 1-11 are the contract.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use super::{Edge, NavPath, NavPoint};
use crate::grid::{move_kind, DIRS8, NAV_STEP};
use crate::navgrid::{NavGrid, PortalKind};

/// Rule 2: rings searched by snapWalk.
const SNAP_RINGS: i64 = 6;
/// Rule 4: heuristic scale (the cheapest step costs 0.7 per unit of cell length).
const H_SCALE: f64 = 0.7;
/// Rule 4: extra cost of a jump and of a drop edge, in cells.
const JUMP_COST: f64 = 2.0;
const DROP_COST: f64 = 1.0;

/// A waypoint on the way to smoothing (rule 5): a point, its surface height, the portal entered (or -1), its cell and
/// the edge.
#[derive(Debug, Clone, Copy)]
struct Wp {
    x: f64,
    z: f64,
    y: f64,
    portal: i32,
    cell: usize,
    edge: Option<Edge>,
}

/// Rule 7 and 8: a fixed waypoint is one entered by a portal or by a jump or drop edge.
fn is_fixed(w: &Wp) -> bool {
    w.portal >= 0 || w.edge.is_some()
}

/// Rule 6: visibility of the segment p to q for samples on plannable cells of region r.
fn visible(grid: &NavGrid, p: [f64; 2], q: [f64; 2], r: i32) -> bool {
    let (dx, dz) = (q[0] - p[0], q[1] - p[1]);
    let len = (dx * dx + dz * dz).sqrt();
    let n = (len / (grid.cell / 4.0)).ceil().max(1.0) as usize;
    let mut prev_y = 0.0;
    for k in 0..=n {
        let (x, z) = if k == n {
            (q[0], q[1])
        } else {
            let t = k as f64 / n as f64;
            (p[0] + dx * t, p[1] + dz * t)
        };
        let Some(c) = grid.cell_at(x, z) else { return false };
        if grid.walk[c] != 1 || grid.region[c] != r {
            return false;
        }
        // Rule 6: no height change between consecutive samples beyond NAV_STEP (a jump or drop is never shortcut).
        let y = grid.y[c];
        if k > 0 && (y - prev_y).abs() > NAV_STEP {
            return false;
        }
        prev_y = y;
    }
    true
}

/// An open-set entry (rule 4), ordered as a min-heap on (f, index): the least entry is the greatest under this Ord, so
/// the std max-heap pops it first.
#[derive(Debug, Clone, Copy)]
struct Open {
    f: f64,
    idx: usize,
}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        other.f.partial_cmp(&self.f).unwrap_or(Ordering::Equal).then_with(|| other.idx.cmp(&self.idx))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Open {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Open {}

/// The edge of a move kind (moveKind 1 jump, 2 drop; 0 walk has none).
fn edge_of(kind: u8) -> Option<Edge> {
    match kind {
        1 => Some(Edge::Jump),
        2 => Some(Edge::Drop),
        _ => None,
    }
}

/// Rule 2: the plannable cell nearest to (x, z), or None (nav-plan.ts snapWalk).
pub fn snap_walk(grid: &NavGrid, x: f64, z: f64) -> Option<usize> {
    if let Some(c) = grid.cell_at(x, z) {
        if grid.walk[c] == 1 {
            return Some(c);
        }
    }
    let ci = ((x - grid.x0) / grid.cell).floor() as i64;
    let cj = ((z - grid.z0) / grid.cell).floor() as i64;
    let cols = grid.cols as i64;
    let rows = grid.rows as i64;
    for r in 1..=SNAP_RINGS {
        // The best (squared distance, index) in this ring; ties go to the smaller index.
        let mut best: Option<(f64, usize)> = None;
        for j in (cj - r).max(0)..=(cj + r).min(rows - 1) {
            for i in (ci - r).max(0)..=(ci + r).min(cols - 1) {
                if (i - ci).abs().max((j - cj).abs()) != r {
                    continue;
                }
                let n = j as usize * grid.cols + i as usize;
                if grid.walk[n] != 1 {
                    continue;
                }
                let [cx, cz] = grid.centre(n);
                let dx = cx - x;
                let dz = cz - z;
                let d = dx * dx + dz * dz;
                let better = match best {
                    None => true,
                    Some((bd, bn)) => d < bd || (d == bd && n < bn),
                };
                if better {
                    best = Some((d, n));
                }
            }
        }
        if let Some((_, n)) = best {
            return Some(n);
        }
    }
    None
}

/// Rules 3 to 9: the path from `from` to `to` (nav-plan.ts findPath), or None when either end cannot be snapped or no
/// path exists. `penalty` adds per cell to the move cost; `no_lifts` leaves the lift portals out.
pub fn find_path(grid: &NavGrid, from: [f64; 2], to: [f64; 2], penalty: Option<&[f64]>, no_lifts: bool) -> Option<NavPath> {
    let start = snap_walk(grid, from[0], from[1])?;
    let goal = snap_walk(grid, to[0], to[1])?;
    if start == goal {
        let points = vec![
            NavPoint { x: from[0], z: from[1], y: grid.y[start], portal: -1, edge: None },
            NavPoint { x: to[0], z: to[1], y: grid.y[goal], portal: -1, edge: None },
        ];
        let hash = path_hash(&points);
        return Some(NavPath { points, cost: 0.0, hash });
    }

    // Rule 4: A* over the plannable cells.
    let cols = grid.cols;
    let cell = grid.cell;
    let n_cells = cols * grid.rows;
    let (gi, gj) = ((goal % cols) as i64, (goal / cols) as i64);
    let h = |n: usize| -> f64 {
        let di = ((n % cols) as i64 - gi).abs();
        let dj = ((n / cols) as i64 - gj).abs();
        let mx = di.max(dj) as f64;
        let mn = di.min(dj) as f64;
        H_SCALE * cell * (mx + (std::f64::consts::SQRT_2 - 1.0) * mn)
    };

    let mut g = vec![f64::INFINITY; n_cells];
    let mut parent: Vec<Option<usize>> = vec![None; n_cells];
    let mut via = vec![-1i32; n_cells];
    // The kind of the move into each cell (rule 4): 0 walk or portal, 1 jump, 2 drop.
    let mut edge_kind = vec![0u8; n_cells];
    let mut closed = vec![false; n_cells];
    let mut open = BinaryHeap::new();

    g[start] = 0.0;
    open.push(Open { f: h(start), idx: start });
    let mut found = false;
    while let Some(Open { idx: c, .. }) = open.pop() {
        if closed[c] {
            continue;
        }
        closed[c] = true;
        if c == goal {
            found = true;
            break;
        }
        let ci = (c % cols) as i64;
        let cj = (c / cols) as i64;
        let gc = g[c];
        for &(dx, dz) in &DIRS8 {
            let ni = ci + dx;
            let nj = cj + dz;
            if ni < 0 || nj < 0 || ni >= cols as i64 || nj >= grid.rows as i64 {
                continue;
            }
            let (ni, nj) = (ni as usize, nj as usize);
            let n = nj * cols + ni;
            if grid.walk[n] != 1 || closed[n] {
                continue;
            }
            let diag = dx != 0 && dz != 0;
            // A diagonal passes both orthogonal cells: both must be plannable.
            if diag && (grid.walk[(cj as usize) * cols + ni] != 1 || grid.walk[nj * cols + ci as usize] != 1) {
                continue;
            }
            let Some(kind) = move_kind(grid.y[n] - grid.y[c]) else { continue };
            let m = if diag { std::f64::consts::SQRT_2 } else { 1.0 };
            let pn = penalty.map_or(0.0, |p| p[n]);
            let extra = match kind {
                1 => JUMP_COST * cell,
                2 => DROP_COST * cell,
                _ => 0.0,
            };
            let gn = gc + cell * m * (grid.cost[n] + pn) + extra;
            if gn < g[n] {
                g[n] = gn;
                parent[n] = Some(c);
                via[n] = -1;
                edge_kind[n] = kind;
                open.push(Open { f: gn + h(n), idx: n });
            }
        }
        // Portal edges are extra to the 8 moves.
        let p = grid.portal[c];
        if p >= 0 {
            if let Some(pt) = grid.portals.get(p as usize) {
                if !(no_lifts && pt.kind == PortalKind::Lift) {
                    let o = if c == pt.a {
                        Some(pt.b)
                    } else if c == pt.b {
                        Some(pt.a)
                    } else {
                        None
                    };
                    if let Some(o) = o {
                        if o < n_cells && grid.walk[o] == 1 && !closed[o] {
                            let gn = gc + pt.cost;
                            if gn < g[o] {
                                g[o] = gn;
                                parent[o] = Some(c);
                                via[o] = p;
                                edge_kind[o] = 0;
                                open.push(Open { f: gn + h(o), idx: o });
                            }
                        }
                    }
                }
            }
        }
    }
    if !found {
        return None;
    }

    let mut cells = Vec::new();
    let mut cur = Some(goal);
    while let Some(c) = cur {
        cells.push(c);
        cur = parent[c];
    }
    cells.reverse();

    // Rule 5: waypoints.
    let mut wps = vec![Wp { x: from[0], z: from[1], y: grid.y[start], portal: -1, cell: start, edge: None }];
    for &c in &cells[1..] {
        let [x, z] = grid.centre(c);
        wps.push(Wp { x, z, y: grid.y[c], portal: via[c], cell: c, edge: edge_of(edge_kind[c]) });
    }
    wps.push(Wp { x: to[0], z: to[1], y: grid.y[goal], portal: -1, cell: goal, edge: None });

    // Rule 7: string pulling. next_fixed[a] is the smallest fixed waypoint index > a, or the last index.
    let last = wps.len() - 1;
    let mut next_fixed = vec![last; wps.len()];
    let mut np = last;
    for a in (0..last).rev() {
        if is_fixed(&wps[a + 1]) {
            np = a + 1;
        }
        next_fixed[a] = np;
    }
    let mut kept = vec![0usize];
    let mut a = 0;
    while a < last {
        let from_w = wps[a];
        let r = grid.region[from_w.cell];
        let mut b = None;
        for k in (a + 1..=next_fixed[a]).rev() {
            let to_w = wps[k];
            if visible(grid, [from_w.x, from_w.z], [to_w.x, to_w.z], r) {
                b = Some(k);
                break;
            }
        }
        let b = b.unwrap_or(a + 1);
        kept.push(b);
        a = b;
    }

    // Rule 8: one corner-cutting pass over the kept list.
    let kk: Vec<Wp> = kept.iter().map(|&idx| wps[idx]).collect();
    let m = kk.len();
    let mut out = vec![kk[0]];
    for i in 1..m.saturating_sub(1) {
        let p = kk[i];
        let a_pt = out[out.len() - 1];
        let b_pt = kk[i + 1];
        if is_fixed(&p) || is_fixed(&kk[i - 1]) || is_fixed(&b_pt) {
            out.push(p);
            continue;
        }
        let c1 = [p.x + 0.25 * (a_pt.x - p.x), p.z + 0.25 * (a_pt.z - p.z)];
        let c2 = [p.x + 0.25 * (b_pt.x - p.x), p.z + 0.25 * (b_pt.z - p.z)];
        if !visible(grid, [a_pt.x, a_pt.z], c1, grid.region[a_pt.cell]) {
            out.push(p);
            continue;
        }
        let Some(cell1) = grid.cell_at(c1[0], c1[1]) else {
            out.push(p);
            continue;
        };
        if !visible(grid, c1, c2, grid.region[cell1]) {
            out.push(p);
            continue;
        }
        let Some(cell2) = grid.cell_at(c2[0], c2[1]) else {
            out.push(p);
            continue;
        };
        if !visible(grid, c2, [b_pt.x, b_pt.z], grid.region[cell2]) {
            out.push(p);
            continue;
        }
        out.push(Wp { x: c1[0], z: c1[1], y: grid.y[cell1], portal: -1, cell: cell1, edge: None });
        out.push(Wp { x: c2[0], z: c2[1], y: grid.y[cell2], portal: -1, cell: cell2, edge: None });
    }
    out.push(kk[m - 1]);

    // Rule 9: output.
    let points: Vec<NavPoint> = out
        .iter()
        .map(|w| NavPoint { x: w.x, z: w.z, y: w.y, portal: w.portal, edge: w.edge })
        .collect();
    let hash = path_hash(&points);
    Some(NavPath { points, cost: g[goal], hash })
}

/// Rule 10: the fixture hash of quantised points (FNV-1a, 32-bit, 8 hex digits).
pub fn path_hash(points: &[NavPoint]) -> String {
    let mut s = String::new();
    for p in points {
        let qx = (p.x * 1000.0 + 0.5).floor() as i64;
        let qz = (p.z * 1000.0 + 0.5).floor() as i64;
        s.push_str(&format!("{qx},{qz};"));
    }
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{h:08x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::navgrid::{build, NavBox, NavBoard};

    /// A 10 x 10 board with one tall box across the middle (x in [-1, 1], z in [-3, 3]).
    fn boxed_grid() -> NavGrid {
        build(
            &NavBoard {
                size: [10.0, 1.0, 10.0],
                pcb_top: 0.4,
                boxes: vec![NavBox { lo: [-1.0, 0.0, -3.0], hi: [1.0, 2.0, 3.0] }],
                traces: vec![],
                plates: vec![],
            },
            &[],
        )
    }

    #[test]
    fn path_goes_round_the_box() {
        let grid = boxed_grid();
        let path = find_path(&grid, [-3.0, 0.0], [3.0, 0.0], None, false).expect("a path");
        let first = path.points.first().expect("points");
        let last = path.points.last().expect("points");
        assert_eq!((first.x, first.z), (-3.0, 0.0));
        assert_eq!((last.x, last.z), (3.0, 0.0));
        assert!(path.points.iter().any(|p| p.z.abs() > 3.0), "the path should go round the box: {:?}", path.points);
        assert_eq!(path.hash, path_hash(&path.points));
    }

    #[test]
    fn snap_inside_the_box_gives_a_walk_cell() {
        let grid = boxed_grid();
        let c = snap_walk(&grid, 0.0, 0.0).expect("a cell");
        assert_eq!(grid.walk[c], 1);
    }

    #[test]
    fn same_cell_is_two_points_at_zero_cost() {
        let grid = boxed_grid();
        let path = find_path(&grid, [-3.0, 0.0], [-3.0, 0.0], None, false).expect("a path");
        assert_eq!(path.points.len(), 2);
        assert_eq!(path.cost, 0.0);
    }

    #[test]
    fn hash_of_no_points_is_the_fnv_basis() {
        assert_eq!(path_hash(&[]), "811c9dc5");
    }
}
