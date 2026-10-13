//! The aisle exits (board-layout.ts outwardSources, perimeterOpen, reachMain, exitOf and mainComponent): where a plate's
//! aisle meets the main component of the board's own walk. Planning runs on `crate::grid::build` over the board's walk
//! surfaces (the PCB pieces) and the candidate plates. The grid joins surfaces only through links, so the PCB pieces that
//! meet at one height are joined here, for planning only; the layout's own links (lifts, ramps, doors) are the later pass's.
//!
//! An exit is a Plate join: the plate's edge at the aisle end reaches the board cell where the search first meets the
//! main component. Its kind is a Ramp when the plate stands within NAV_LIFT_RISE of the PCB, else a Lift (the lift's cab
//! and landings are the later pass's, so only the join point is planned here).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::geom::{Frame, Rect};
use super::rooms::{Config, AISLE};
use crate::grid::{build, Grid, NAV_CELL};
use crate::walk::{LinkKind, Plane, Side, SurfaceKind, WalkLink, WalkSet, WalkSurface};

/// How far an exit's search may walk, in grid cells (board-layout.ts LANDING_REACH).
pub const LANDING_REACH: usize = 40;
/// A plate this far above the PCB (or less) exits by a ramp; higher, by a lift (nav-types.ts NAV_LIFT_RISE).
pub const NAV_LIFT_RISE: f64 = 1.5;
/// Half a grid cell: the distance from an edge to its first outward sample (board-layout.ts EDGE).
const EDGE: f64 = NAV_CELL / 2.0;
/// The spacing of the samples along a perimeter (board-layout.ts SAMPLE).
const SAMPLE: f64 = 0.5;
/// A gangway candidate: its bridge, its landing cell, its length, its direction, its aisle end and its candidate index.
type GangCand = (Vec<[f64; 2]>, usize, f64, [f64; 2], usize, usize);

/// A spur's gap through a row: its width (board-layout.ts GAP).
const GAP: f64 = 1.0;
/// The farthest a gangway bridges past a plate edge (board-layout.ts GANGWAY_REACH).
pub const GANGWAY_REACH: f64 = 3.25;

/// What joins the aisle to the main component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExitKind {
    Ramp,
    Lift,
    /// A straight gangway over blocked cells at plate height (the last resort): its bridge is in `Exit::bridge`.
    Gangway,
}

/// A gangway: a plate's bridge at plate height over blocked cells, onto a main-component cell (board-layout.ts gangwayOf).
/// `plate` indexes `Layout::plates`; `bridge` is the bridge's cell centres from the plate edge; `landing` is the cell it
/// lands on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gangway {
    pub plate: usize,
    pub dir: [f64; 2],
    pub bridge: Vec<[f64; 2]>,
    pub landing: [f64; 2],
}

/// A plate's exit. `plate_at` is the plate's point at the exit (for a gangway, the first bridge cell) and `board_at` the
/// main-component cell it reaches. `dir` is the outward direction. `end` is the aisle end (0: the -u end, 1: the +u end,
/// 2: a single aisle's long side). `bridge` is the gangway's cells (empty for a ramp or a lift).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exit {
    pub kind: ExitKind,
    pub plate_at: [f64; 2],
    pub board_at: [f64; 2],
    pub dir: [f64; 2],
    /// 0: the -u end, 1: the +u end, 2: a spur (see `spur`) or a single aisle's long side (a gangway).
    pub end: usize,
    pub bridge: Vec<[f64; 2]>,
    /// The candidate's key: a rejected key is excluded from the next plan (layout.rs).
    pub key: String,
    /// The row gap a spur cuts through a row, when the exit is a spur.
    pub spur: Option<Gap>,
}

/// A spur's gap: the row it cuts (index into the configuration's rows) and its span along u.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gap {
    pub row: usize,
    pub span: [f64; 2],
}

/// The board-only walk for aisle planning: the PCB at `pcb_top` less the holes (the plate footprints and the tall parts),
/// its pieces joined where they share an edge, and the candidate plates `(rect, top)`, walled on every side.
pub fn aisle_walk(pcb: Rect, pcb_top: f64, tall: &[Rect], plates: &[([f64; 4], f64)]) -> WalkSet {
    let mut holes: Vec<Rect> = plates.iter().map(|(r, _)| Rect::from_xywh(*r)).collect();
    holes.extend(tall.iter().copied());
    let pieces = super::geom::subtract(pcb, &holes);
    let mut surfaces: Vec<WalkSurface> = Vec::new();
    let mut links: Vec<WalkLink> = Vec::new();
    // Two pieces that share a boundary segment of positive length are joined (a partial edge joins too, as the cells do).
    // A side against a plate is free (Drop); against a tall part or the board's edge it is a wall.
    let mut sides_of: Vec<Vec<Side>> = pieces.iter().map(|r| (0..4).map(|k| super::surfaces::pcb_free_side(r, k, &pcb, tall)).collect()).collect();

    for a in 0..pieces.len() {
        for b in a + 1..pieces.len() {
            if let Some((sa, sb, at)) = super::surfaces::shared_side(&pieces[a], &pieces[b]) {
                sides_of[a][sa] = Side::Open;
                sides_of[b][sb] = Side::Open;
                links.push(WalkLink { from: a as u32, to: b as u32, kind: LinkKind::Step, via: None, at });
            }
        }
    }
    for (i, (r, sides)) in pieces.iter().zip(sides_of).enumerate() {
        surfaces.push(WalkSurface {
            id: i as u32,
            kind: SurfaceKind::Board,
            poly: r.poly(),
            height: Plane::flat(pcb_top),
            owner: "board".to_string(),
            sides,
        });
    }
    for (p, (r, top)) in plates.iter().enumerate() {
        let id = surfaces.len() as u32;
        surfaces.push(WalkSurface {
            id,
            kind: SurfaceKind::Plate,
            poly: Rect::from_xywh(*r).poly(),
            height: Plane::flat(*top),
            owner: format!("plate:{p}"),
            sides: vec![Side::Walled; 4],
        });
    }
    WalkSet { surfaces, links }
}

/// The grid of the aisle walk (the planning grid of the layout).
pub fn plan_grid(walk: &WalkSet) -> Grid {
    build(walk)
}

/// The outward samples from an edge: for each edge point, the free cells 1..=8 edges out along `dir` (board-layout.ts
/// outwardSources). The plate's own inside cell is not tested: its walled side already blocks the plate's edge cells, so
/// the aisle end is taken as the plate edge.
fn outward_sources(grid: &Grid, edge: &[[f64; 2]], dir: [f64; 2], label: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for &[x, z] in edge {
        for k in 1..=8 {
            if let Some(c) = grid.cell_at(x + dir[0] * k as f64 * EDGE, z + dir[1] * k as f64 * EDGE).filter(|&c| grid.blocked[c] == 0) {
                out.push((c, label));
            }
        }
    }
    out
}

/// Does any perimeter edge of the plate open onto unblocked ground? (false: the plate is enclosed on every side)
pub fn perimeter_open(grid: &Grid, f: &Frame) -> bool {
    let mut u = 0.0;
    while u <= f.l + 1e-9 {
        if !outward_sources(grid, &[f.point(u, 0.0)], f.vec(0.0, -1.0), 0).is_empty() {
            return true;
        }
        if !outward_sources(grid, &[f.point(u, f.s)], f.vec(0.0, 1.0), 0).is_empty() {
            return true;
        }
        u += SAMPLE;
    }
    let mut v = 0.0;
    while v <= f.s + 1e-9 {
        if !outward_sources(grid, &[f.point(0.0, v)], f.vec(-1.0, 0.0), 0).is_empty() {
            return true;
        }
        if !outward_sources(grid, &[f.point(f.l, v)], f.vec(1.0, 0.0), 0).is_empty() {
            return true;
        }
        v += SAMPLE;
    }
    false
}

/// The multi-source search from the sources (open cells only) over the grid's walk cells, within LANDING_REACH steps, to
/// the first cell of the main component. The plate's own footprint is not a way off. Returns (label, cell) of the first
/// hit, or None (board-layout.ts reachMain).
fn reach_main(grid: &Grid, rect: [f64; 4], sources: &[(usize, usize)], main: i32) -> Option<(usize, usize)> {
    if main < 0 {
        return None;
    }
    let n = grid.cols * grid.rows;
    let mut dist = vec![-1i64; n];
    let mut label = vec![0usize; n];
    let mut queue: Vec<usize> = Vec::new();
    for &(c, l) in sources {
        if c < n && grid.walk[c] == 1 && dist[c] < 0 {
            dist[c] = 0;
            label[c] = l;
            queue.push(c);
        }
    }
    let [px, pz, pw, pd] = rect;
    let mut head = 0;
    while head < queue.len() {
        let u = queue[head];
        head += 1;
        if grid.component_of(u) == main {
            return Some((label[u], u));
        }
        if dist[u] >= LANDING_REACH as i64 {
            continue;
        }
        for k in 0..8 {
            if grid.moves[u] & (1 << k) == 0 {
                continue;
            }
            let Some(v) = grid.neighbour(u, k) else { continue };
            if dist[v] >= 0 || grid.walk[v] != 1 {
                continue;
            }
            let [x, z] = grid.cell_centre(v);
            if x >= px && x < px + pw && z >= pz && z < pz + pd {
                continue;
            }
            dist[v] = dist[u] + 1;
            label[v] = label[u];
            queue.push(v);
        }
    }
    None
}

/// The plate a candidate exit serves: its rect, its frame, the rise of its top above the PCB, its configuration's key, and
/// the tall-part test over a rect (a part higher than the plate top plus TALL stands there).
pub struct Site<'a> {
    pub rect: [f64; 4],
    pub f: Frame,
    pub rise: f64,
    pub ck: String,
    pub tall: &'a dyn Fn([f64; 4]) -> bool,
}

/// The exit of an aisle configuration (board-layout.ts exitOf): the nearer aisle end (both ends searched together, the
/// first hit wins); else a spur, a 1.0 gap through a row from an aisle cell to the row's outer perimeter, searched the same
/// way. Keys in `excluded` are not used. The spurs are tried in row, then cut order, so the first hit is the TS's.
pub fn exit_of(site: &Site, cfg: &Config, grid: &Grid, main: i32, excluded: &BTreeSet<String>) -> Option<Exit> {
    let f = site.f;
    let kind = if site.rise <= NAV_LIFT_RISE { ExitKind::Ramp } else { ExitKind::Lift };
    let vm = (cfg.aisle[0] + cfg.aisle[1]) / 2.0;
    let mut sources = Vec::new();
    let mut ends: Vec<([f64; 2], [f64; 2])> = Vec::new();
    for (k, nu) in [-1.0_f64, 1.0].into_iter().enumerate() {
        let u_edge = if nu < 0.0 { 0.0 } else { f.l };
        let normal = f.vec(nu, 0.0);
        ends.push((f.point(u_edge - nu * EDGE, vm), normal));
        if excluded.contains(&format!("{}|end{k}", site.ck)) {
            continue;
        }
        let mut edge = Vec::new();
        let mut v = cfg.aisle[0] + EDGE;
        while v <= cfg.aisle[1] - EDGE + 1e-9 {
            edge.push(f.point(u_edge, v));
            v += 2.0 * EDGE;
        }
        sources.extend(outward_sources(grid, &edge, normal, k));
    }
    if let Some((end, cell)) = reach_main(grid, site.rect, &sources, main) {
        let (plate_at, dir) = ends[end];
        let key = format!("{}|end{end}", site.ck);
        return Some(Exit { kind, plate_at, board_at: grid.cell_centre(cell), dir, end, bridge: Vec::new(), key, spur: None });
    }

    // Spur candidates: a 1.0 gap in each row, centred on each sample column, clear of tall parts. Its perimeter point is the
    // row's outer edge (the edge away from the aisle); its source is the band beyond that edge across the gap.
    struct Spur {
        row: usize,
        span: [f64; 2],
        plate_at: [f64; 2],
        normal: [f64; 2],
        key: String,
    }
    let mut spurs: Vec<Spur> = Vec::new();
    let mut spur_sources = Vec::new();
    let spur_ok = |w: f64| !(1e-9..AISLE - 1e-9).contains(&w);
    for (r, row) in cfg.rows.iter().enumerate() {
        let outer_v = if row.face[1] > 0.0 { row.v0 } else { row.v1 };
        let nv = -row.face[1].signum();
        let normal = f.vec(0.0, nv);
        let mut c = GAP / 2.0;
        while c <= f.l - GAP / 2.0 + 1e-9 {
            let span = [c - GAP / 2.0, c + GAP / 2.0];
            let centre = c;
            c += SAMPLE;
            if (site.tall)(f.rect(span[0], span[1], row.v0, row.v1)) || !spur_ok(span[0]) || !spur_ok(f.l - span[1]) {
                continue;
            }
            let k = spurs.len();
            let key = format!("{}|spur{r}:{:.3}", site.ck, span[0]);
            spurs.push(Spur { row: r, span, plate_at: f.point(centre, outer_v - nv * EDGE), normal, key: key.clone() });
            if excluded.contains(&key) {
                continue;
            }
            let mut edge = Vec::new();
            let mut u = span[0] + EDGE;
            while u <= span[1] - EDGE + 1e-9 {
                edge.push(f.point(u, outer_v));
                u += 2.0 * EDGE;
            }
            for s in outward_sources(grid, &edge, normal, k) {
                spur_sources.push(s);
            }
        }
    }
    let (label, cell) = reach_main(grid, site.rect, &spur_sources, main)?;
    let s = &spurs[label];
    Some(Exit {
        kind,
        plate_at: s.plate_at,
        board_at: grid.cell_centre(cell),
        dir: s.normal,
        end: 2,
        bridge: Vec::new(),
        key: s.key.clone(),
        spur: Some(Gap { row: s.row, span: s.span }),
    })
}

/// A straight gangway from a plate edge point out along `dir` (board-layout.ts gangwayFrom): the cells it passes may be
/// blocked (it bridges over them); the first walk cell must be in the main component, and the two cells beyond it too.
/// Returns the bridge's cell centres and the landing cell, or None.
fn gangway_from(grid: &Grid, edge: [f64; 2], dir: [f64; 2], main: i32) -> Option<(Vec<[f64; 2]>, usize)> {
    if main < 0 {
        return None;
    }
    let inside = grid.cell_at(edge[0] - dir[0] * EDGE, edge[1] - dir[1] * EDGE)?;
    let mut bridge = vec![grid.cell_centre(inside)];
    let mut last = inside;
    let steps = ((GANGWAY_REACH + EDGE) / NAV_CELL).floor() as usize;
    let at = |t: f64| grid.cell_at(edge[0] + dir[0] * t, edge[1] + dir[1] * t);
    for k in 1..=steps {
        let t = k as f64 * NAV_CELL - EDGE;
        let c = at(t)?;
        if grid.walk[c] != 1 {
            if last != c {
                bridge.push(grid.cell_centre(c));
                last = c;
            }
            continue;
        }
        if grid.component_of(c) != main {
            return None;
        }
        // The lift's queue is two cells out: the cells one and two beyond the landing must be walk in the main component.
        for beyond in [t + NAV_CELL, t + 2.0 * NAV_CELL] {
            let q = at(beyond)?;
            if grid.walk[q] != 1 || grid.component_of(q) != main {
                return None;
            }
        }
        return Some((bridge, c));
    }
    None
}

/// A slot's gangway exit for one configuration (board-layout.ts gangwayOf): from each perimeter cell of the aisle's ends
/// (and a single aisle's outer long side), the shortest gangway that lands in the main component. Null when none does.
/// Candidates are keyed `<configuration>|gang<index>` in order; a key in `excluded` is skipped.
pub fn gangway_of(site: &Site, cfg: &Config, grid: &Grid, main: i32, excluded: &BTreeSet<String>) -> Option<Exit> {
    let f = site.f;
    let mut cands: Vec<([f64; 2], [f64; 2], usize)> = Vec::new();
    let across = |u: f64, v0: f64, v1: f64| {
        let mut pts = Vec::new();
        let mut v = v0 + NAV_CELL / 2.0;
        while v < v1 - 1e-9 {
            pts.push(f.point(u, v));
            v += NAV_CELL;
        }
        pts
    };
    for (k, nu) in [-1.0_f64, 1.0].into_iter().enumerate() {
        let dir = f.vec(nu, 0.0);
        let u = if nu < 0.0 { 0.0 } else { f.l };
        for e in across(u, cfg.aisle[0], cfg.aisle[1]) {
            cands.push((e, dir, k));
        }
    }
    if !cfg.double {
        let sv = if cfg.aisle[0] <= 1e-9 { -1.0 } else { 1.0 };
        let dir = f.vec(0.0, sv);
        let mut u = NAV_CELL / 2.0;
        while u < f.l - 1e-9 {
            cands.push((f.point(u, if sv < 0.0 { 0.0 } else { f.s }), dir, 2));
            u += NAV_CELL;
        }
    }
    let mut best: Option<GangCand> = None;
    for (idx, (edge, dir, end)) in cands.into_iter().enumerate() {
        if excluded.contains(&format!("{}|gang{idx}", site.ck)) {
            continue;
        }
        let Some((bridge, cell)) = gangway_from(grid, edge, dir, main) else { continue };
        let len = ((grid.cell_centre(cell)[0] - edge[0]).powi(2) + (grid.cell_centre(cell)[1] - edge[1]).powi(2)).sqrt();
        if best.as_ref().is_none_or(|b| len < b.2 - 1e-9) {
            best = Some((bridge, cell, len, dir, end, idx));
        }
    }
    let (bridge, cell, _, dir, end, idx) = best?;
    let key = format!("{}|gang{idx}", site.ck);
    Some(Exit { kind: ExitKind::Gangway, plate_at: bridge[0], board_at: grid.cell_centre(cell), dir, end, bridge, key, spur: None })
}

/// The share of walk cells in a strip of the board-only grid (board-layout.ts openShare), sampled at cell centres.
pub fn open_share(grid: &Grid, strip: [f64; 4]) -> f64 {
    let [x, z, w, d] = strip;
    let nx = ((w / NAV_CELL).round() as usize).max(1);
    let nz = ((d / NAV_CELL).round() as usize).max(1);
    let mut open = 0usize;
    for i in 0..nx {
        for j in 0..nz {
            let px = x + (i as f64 + 0.5) * w / nx as f64;
            let pz = z + (j as f64 + 0.5) * d / nz as f64;
            if grid.cell_at(px, pz).is_some_and(|c| grid.walk[c] == 1) {
                open += 1;
            }
        }
    }
    open as f64 / (nx * nz) as f64
}
