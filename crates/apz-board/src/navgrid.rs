//! The nav grid (nav-grid.ts buildGrid): the board, its plates (the slot tops) and the rooms on the plates as one grid of
//! cells in world space. The web builds the same grid, so the rules in nav-grid.ts's header are the contract, applied here
//! in the same order:
//!
//! 1. Off-board cells are blocked.
//! 3. Plate cells take the plate's region and height.
//! 4. Part boxes block the cells they overlap when they stand more than NAV_STEP above the cell.
//! 5. Copper traces cost less than open PCB.
//! 6. Board landings (and 6a gangways), from the board-only walk.
//! 7. Rooms (terraces): interior, slab margin and band, doors, solids, door choice, spots (`rooms.rs`).
//! 8. Exempt cells (doors, landings, spots) and their 8-neighbours.
//! 9. Inflation and walk.
//! 10. Costs near blocked cells.
//! 11. Portals and lifts, one per plate landing.
//! 12. Components (portals included); islands are cut.
//!
//! A room's furniture comes from the room generator, which lives in the engine: the caller gives each terrace its solid
//! footprints and its spots already in the terrace's local frame (`NavTerrace::solids`, `NavTerrace::spots`).

mod rooms;

pub use rooms::furniture;

use crate::grid::{DIRS8, NAV_CELL, NAV_COST_OPEN, NAV_DROP_DOWN, NAV_JUMP_UP, NAV_LIFT_COST, NAV_NEAR_COST, NAV_STEP};

/// A plate this far above the PCB (or less) is reached by a ramp; higher, by a lift (nav-types.ts NAV_LIFT_RISE).
pub const NAV_LIFT_RISE: f64 = 1.5;
/// The cliff band around a room's slab (nav-types.ts NAV_RAMP_DEPTH).
pub const NAV_RAMP_DEPTH: f64 = 1.0;
/// The cost of a cell on a copper trace (nav-types.ts NAV_COST_TRACE).
pub const NAV_COST_TRACE: f64 = 0.7;

/// A solid part box (nav-types.ts NavBox).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavBox {
    pub lo: [f64; 3],
    pub hi: [f64; 3],
}

/// A copper trace polyline (world x, z) and its width (nav-types.ts NavTrace).
#[derive(Debug, Clone, PartialEq)]
pub struct NavTrace {
    pub path: Vec<[f64; 2]>,
    pub width: f64,
}

/// How a plate's landing leaves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingKind {
    Ramp,
    Lift,
    Gangway,
}

/// A plate's one way onto the board, set by the layout: the aisle-end plate point (ax, az), the board point (bx, bz), the
/// outward normal (nx, nz) and the kind. A gangway also has its bridge cells' centres, from the plate edge out.
#[derive(Debug, Clone, PartialEq)]
pub struct PlateLanding {
    pub ax: f64,
    pub az: f64,
    pub bx: f64,
    pub bz: f64,
    pub kind: LandingKind,
    pub nx: f64,
    pub nz: f64,
    pub bridge: Vec<[f64; 2]>,
}

/// A plate: the rectangle [x0, x0 + w) x [z0, z0 + d) at height `y`. `id` is its region id (>= 1, unique among the
/// plates and terraces).
#[derive(Debug, Clone, PartialEq)]
pub struct NavPlate {
    pub id: i32,
    pub x0: f64,
    pub z0: f64,
    pub w: f64,
    pub d: f64,
    pub y: f64,
    pub landing: Option<PlateLanding>,
}

/// The board: the PCB's size (x, y, z; centred on the origin in x and z), its top, the part boxes, traces and plates.
#[derive(Debug, Clone, PartialEq)]
pub struct NavBoard {
    pub size: [f64; 3],
    pub pcb_top: f64,
    pub boxes: Vec<NavBox>,
    pub traces: Vec<NavTrace>,
    pub plates: Vec<NavPlate>,
}

/// A wall side of a room (the door's side): n, e, s or w.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    N,
    E,
    S,
    W,
}

/// A room's door: its side, the opening's centre as an offset from its wall's middle (local units) and its full width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavDoor {
    pub side: Side,
    pub at: f64,
    pub width: f64,
}

/// A room solid's footprint in the room's local frame: the centre (x, z) and the size (sx, sz) after its turn (the
/// size is swapped when the item's rotation is not a multiple of 180). Only the items rule 7c counts: placed neither on a
/// surface nor on a wall, not on the floor layer, and not walkable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavSolid {
    pub x: f64,
    pub z: f64,
    pub sx: f64,
    pub sz: f64,
}

/// A room spot in the room's local frame: (x, z) and its face in degrees (the layout's spot.face).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavLocalSpot {
    pub x: f64,
    pub z: f64,
    pub face: f64,
}

/// A room on a terrace (nav-types.ts NavTerrace). The terrace is the district's slab: a square of half-side `half` (or the
/// circle of radius `half` when round) centred on (cx, cz) at height `y`. The room's frame is turned by `turn` about y;
/// its inner size is w x d (local x by z).
#[derive(Debug, Clone, PartialEq)]
pub struct NavTerrace {
    pub id: i32,
    pub cx: f64,
    pub cz: f64,
    pub half: f64,
    pub hw: f64,
    pub hd: f64,
    pub round: bool,
    pub y: f64,
    pub turn: f64,
    pub w: f64,
    pub d: f64,
    pub door: NavDoor,
    pub solids: Vec<NavSolid>,
    pub spots: Vec<NavLocalSpot>,
}

/// A portal's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalKind {
    Ramp,
    Lift,
}

/// A plate landing joining two cells: a is the plate cell, b the board cell (nav-types.ts NavPortal).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavPortal {
    pub a: usize,
    pub b: usize,
    pub kind: PortalKind,
    pub cost: f64,
    /// The lift index for a lift portal.
    pub lift: Option<usize>,
}

/// A lift at a tall plate's landing (nav-types.ts NavLift): the platform centre (x, z) on the front cell, its outward
/// normal's yaw, its heights, and its cells (the platform's front cell, the queue cell beyond it, the plate's edge cell).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavLift {
    pub id: usize,
    pub terrace: i32,
    pub x: f64,
    pub z: f64,
    pub face: f64,
    pub low: f64,
    pub high: f64,
    pub front: usize,
    pub queue: usize,
    pub door: usize,
}

/// A room spot in world space (nav-types.ts NavSpot): `face` is the world yaw; `cell` is None off the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavSpot {
    pub terrace: i32,
    pub index: usize,
    pub x: f64,
    pub z: f64,
    pub face: f64,
    pub cell: Option<usize>,
}

/// A one-way drop between two 8-adjacent walk cells: from the high cell to the low one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavDrop {
    pub from: usize,
    pub to: usize,
}

/// The built grid (nav-types.ts NavGrid). Index c = j * cols + i.
#[derive(Debug, Clone, PartialEq)]
pub struct NavGrid {
    pub cell: f64,
    pub cols: usize,
    pub rows: usize,
    pub x0: f64,
    pub z0: f64,
    pub walk: Vec<u8>,
    pub blocked: Vec<u8>,
    pub inflated: Vec<u8>,
    /// 0 for the PCB, else the plate's or terrace's id.
    pub region: Vec<i32>,
    pub y: Vec<f64>,
    /// 1 where the cell is off the board (no floor).
    pub off: Vec<u8>,
    pub cost: Vec<f64>,
    /// The first portal at each cell, or -1.
    pub portal: Vec<i32>,
    pub portals: Vec<NavPortal>,
    pub lifts: Vec<NavLift>,
    pub spots: Vec<NavSpot>,
    /// The plate ids with no usable landing.
    pub unreached: Vec<i32>,
    /// Per terrace (in order): its door cell and the cell outside its wall beyond the door.
    pub door_cells: Vec<(Option<usize>, Option<usize>)>,
    /// The component of each walk cell, -1 otherwise.
    pub component: Vec<i32>,
    pub drops: Vec<NavDrop>,
    /// The terrace ids, in order (for the connectivity report).
    pub terrace_ids: Vec<i32>,
    /// The plate ids, in order.
    pub plate_ids: Vec<i32>,
}

impl NavGrid {
    /// The centre of column i (x) and row j (z).
    pub fn centre(&self, c: usize) -> [f64; 2] {
        [self.x0 + ((c % self.cols) as f64 + 0.5) * self.cell, self.z0 + ((c / self.cols) as f64 + 0.5) * self.cell]
    }

    /// The cell containing (x, z), or None outside the grid (nav-grid.ts cellAt).
    pub fn cell_at(&self, x: f64, z: f64) -> Option<usize> {
        let i = ((x - self.x0) / self.cell).floor();
        let j = ((z - self.z0) / self.cell).floor();
        if i < 0.0 || j < 0.0 || i >= self.cols as f64 || j >= self.rows as f64 {
            return None;
        }
        Some(j as usize * self.cols + i as usize)
    }

    /// The cell at column i, row j, or None off the grid.
    pub fn at(&self, i: i64, j: i64) -> Option<usize> {
        (i >= 0 && j >= 0 && (i as usize) < self.cols && (j as usize) < self.rows).then(|| j as usize * self.cols + i as usize)
    }
}

/// The connectivity report (nav-grid.ts NavConnectivity).
#[derive(Debug, Clone, PartialEq)]
pub struct Connectivity {
    pub main: i32,
    pub components: usize,
    pub one_way: usize,
    pub doors_ok: usize,
    pub doors: usize,
    pub unconnected: Vec<i32>,
    pub unreached: Vec<i32>,
}

/// A terrace's slab footprint for the slab tests (nav-grid.ts Slab).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slab {
    pub cx: f64,
    pub cz: f64,
    pub half: f64,
    pub round: bool,
}

impl Slab {
    pub fn of(t: &NavTerrace) -> Slab {
        Slab { cx: t.cx, cz: t.cz, half: t.half, round: t.round }
    }

    /// Is (x, z) inside the slab (nav-grid.ts inSlabAt).
    pub fn contains(&self, x: f64, z: f64) -> bool {
        let (dx, dz) = (x - self.cx, z - self.cz);
        if self.round {
            dx * dx + dz * dz < self.half * self.half
        } else {
            dx.abs() < self.half && dz.abs() < self.half
        }
    }

    /// The distance from (x, z) to the slab (nav-grid.ts slabGapAt).
    pub fn gap(&self, x: f64, z: f64) -> f64 {
        ((x - self.cx).abs() - self.half).max(0.0).hypot(((z - self.cz).abs() - self.half).max(0.0))
    }
}

/// Is (x, z) landing-clear (nav-grid.ts landingClearAt): outside every slab and, off every plate, at least
/// NAV_RAMP_DEPTH from every slab.
pub fn landing_clear_at(slabs: &[Slab], x: f64, z: f64, on_plate: bool) -> bool {
    slabs.iter().all(|t| !t.contains(x, z) && (on_plate || t.gap(x, z) >= NAV_RAMP_DEPTH))
}

/// A plate's landing (rule 6): the plate cell p, the path p .. q, the board cell q, the lift queue, the step p to q.
struct Landing {
    p: usize,
    path: Vec<usize>,
    q: usize,
    queue: Option<usize>,
    sx: i64,
    sz: i64,
    gang: bool,
}

/// The nav grid of a board and its terraces (nav-grid.ts buildGrid).
pub fn build(board: &NavBoard, terraces: &[NavTerrace]) -> NavGrid {
    let cell = NAV_CELL;
    let (size0, size2) = (board.size[0], board.size[2]);
    let x0 = -size0 / 2.0;
    let z0 = -size2 / 2.0;
    let cols = (size0 / cell).ceil() as usize;
    let rows = (size2 / cell).ceil() as usize;
    let n = cols * rows;
    let pcb = board.pcb_top;
    let mut g = NavGrid {
        cell,
        cols,
        rows,
        x0,
        z0,
        walk: vec![0; n],
        blocked: vec![0; n],
        inflated: vec![0; n],
        region: vec![0; n],
        y: vec![pcb; n],
        off: vec![0; n],
        cost: vec![NAV_COST_OPEN; n],
        portal: vec![-1; n],
        portals: Vec::new(),
        lifts: Vec::new(),
        spots: Vec::new(),
        unreached: Vec::new(),
        door_cells: Vec::new(),
        component: Vec::new(),
        drops: Vec::new(),
        terrace_ids: terraces.iter().map(|t| t.id).collect(),
        plate_ids: board.plates.iter().map(|p| p.id).collect(),
    };
    let cx = |i: usize| x0 + (i as f64 + 0.5) * cell;
    let cz = |j: usize| z0 + (j as f64 + 0.5) * cell;
    let mut plate_of = vec![0i32; n];

    // Rule 1: off-board cells.
    for j in 0..rows {
        for i in 0..cols {
            if cx(i).abs() > size0 / 2.0 || cz(j).abs() > size2 / 2.0 {
                g.off[j * cols + i] = 1;
                g.blocked[j * cols + i] = 1;
            }
        }
    }

    // Rule 3: plates, in order (a later plate wins an overlap).
    for pl in &board.plates {
        for j in 0..rows {
            for i in 0..cols {
                let c = j * cols + i;
                if g.off[c] == 1 {
                    continue;
                }
                let (px, pz) = (cx(i), cz(j));
                if px >= pl.x0 && px < pl.x0 + pl.w && pz >= pl.z0 && pz < pl.z0 + pl.d {
                    plate_of[c] = pl.id;
                    g.region[c] = pl.id;
                    g.y[c] = pl.y;
                }
            }
        }
    }

    // Rule 4: boxes block their footprints where they stand more than NAV_STEP above the cell.
    for b in &board.boxes {
        for j in span(b.lo[2], b.hi[2], z0, rows) {
            for i in span(b.lo[0], b.hi[0], x0, cols) {
                let (ax0, ax1) = (x0 + i as f64 * cell, x0 + (i + 1) as f64 * cell);
                let (az0, az1) = (z0 + j as f64 * cell, z0 + (j + 1) as f64 * cell);
                let c = j * cols + i;
                if ax0 < b.hi[0] && ax1 > b.lo[0] && az0 < b.hi[2] && az1 > b.lo[2] && b.hi[1] - g.y[c] > NAV_STEP {
                    g.blocked[c] = 1;
                }
            }
        }
    }

    // Rule 5: copper traces cost less than open PCB (PCB cells still open).
    for tr in &board.traces {
        let r = tr.width / 2.0;
        let r2 = r * r;
        for k in 0..tr.path.len().saturating_sub(1) {
            let ([ax, az], [bx, bz]) = (tr.path[k], tr.path[k + 1]);
            for j in span(az.min(bz) - r, az.max(bz) + r, z0, rows) {
                for i in span(ax.min(bx) - r, ax.max(bx) + r, x0, cols) {
                    let c = j * cols + i;
                    if g.blocked[c] == 0 && plate_of[c] == 0 && dist_sq(cx(i), cz(j), ax, az, bx, bz) <= r2 {
                        g.cost[c] = NAV_COST_TRACE;
                    }
                }
            }
        }
    }

    // Rule 6: board landings, from the board-only walk (no rooms, no exemptions).
    let mut b_walk = vec![0u8; n];
    for j in 0..rows {
        for i in 0..cols {
            let c = j * cols + i;
            if g.blocked[c] == 0 && !neighbour_blocked(&g, i, j, &g.blocked) {
                b_walk[c] = 1;
            }
        }
    }
    let b_comp = components(cols, rows, &b_walk, &g.y, &[]);
    let main_board = largest_at_board(&b_comp, &g.y, pcb);
    let slabs: Vec<Slab> = terraces.iter().map(Slab::of).collect();
    let clear_cell = |c: usize, on_plate: bool| landing_clear_at(&slabs, cx(c % cols), cz(c / cols), on_plate);
    let landed: Vec<Option<Landing>> = board
        .plates
        .iter()
        .map(|pl| {
            let gang = match &pl.landing {
                Some(l) if l.kind == LandingKind::Gangway && !l.bridge.is_empty() => gangway_landing(&g, l, &b_walk, &b_comp, main_board),
                _ => None,
            };
            gang.or_else(|| landing_of(&g, pl, pcb, &plate_of, &b_walk, &b_comp, main_board, &clear_cell))
        })
        .collect();

    // Rule 7: rooms, in order.
    let doors = rooms::apply(&mut g, terraces, &plate_of);

    // Rule 6a (after the rooms): a gangway's bridge cells walk at the plate's height, over the parts and the cliff bands
    // they cross; a bridge cell inside a slab stays blocked, so a bridge never walks through a wall.
    for (pl, l) in board.plates.iter().zip(&landed) {
        if let Some(l) = l.as_ref().filter(|l| l.gang) {
            for &c in &l.path {
                let [x, z] = g.centre(c);
                if slabs.iter().any(|t| t.contains(x, z)) {
                    continue;
                }
                g.blocked[c] = 0;
                g.region[c] = pl.id;
                g.y[c] = pl.y;
            }
        }
    }

    // Rule 8: exempt cells (door cells, landing cells, spot cells) and their 8-neighbours.
    let mut exempt = vec![false; n];
    let mut mark = |c: usize| {
        let (i, j) = ((c % cols) as i64, (c / cols) as i64);
        for dj in -1..=1 {
            for di in -1..=1 {
                if let Some(v) = g.at(i + di, j + dj) {
                    exempt[v] = true;
                }
            }
        }
    };
    for &d in doors.iter().flatten() {
        mark(d);
    }
    for l in landed.iter().flatten() {
        for &c in &l.path {
            mark(c);
        }
        mark(l.q);
        if let Some(q) = l.queue {
            mark(q);
        }
    }
    for c in g.spots.iter().filter_map(|s| s.cell).collect::<Vec<_>>() {
        mark(c);
    }

    // Rule 9: inflation and walk.
    for j in 0..rows {
        for i in 0..cols {
            let c = j * cols + i;
            if g.blocked[c] == 1 {
                g.walk[c] = 0;
                continue;
            }
            let inf = !exempt[c] && neighbour_blocked(&g, i, j, &g.blocked);
            g.inflated[c] = inf as u8;
            g.walk[c] = (!inf) as u8;
        }
    }

    // Rule 10: blocked cells cost OPEN; cells at Chebyshev distance exactly 2 from a blocked cell gain NEAR.
    let mut near = vec![false; n];
    let mut ring = vec![false; n];
    for j in 0..rows as i64 {
        for i in 0..cols as i64 {
            if g.blocked[j as usize * cols + i as usize] != 1 {
                continue;
            }
            for dj in -2..=2i64 {
                for di in -2..=2i64 {
                    if let Some(v) = g.at(i + di, j + dj) {
                        if di.abs().max(dj.abs()) <= 1 {
                            near[v] = true;
                        } else {
                            ring[v] = true;
                        }
                    }
                }
            }
        }
    }
    for c in 0..n {
        if g.blocked[c] == 1 {
            g.cost[c] = NAV_COST_OPEN;
        } else if ring[c] && !near[c] {
            g.cost[c] += NAV_NEAR_COST;
        }
    }

    // Rule 11: portals and lifts, one per plate landing.
    for (pl, l) in board.plates.iter().zip(&landed) {
        let Some(l) = l else {
            g.unreached.push(pl.id);
            continue;
        };
        let rise = pl.y - pcb;
        let ([px, pz], [qx, qz]) = (g.centre(l.p), g.centre(l.q));
        if rise <= NAV_LIFT_RISE && !l.gang {
            if g.walk[l.p] != 1 || g.walk[l.q] != 1 {
                g.unreached.push(pl.id);
                continue;
            }
            let dist = (qx - px).hypot(qz - pz);
            add_portal(&mut g, NavPortal { a: l.p, b: l.q, kind: PortalKind::Ramp, cost: dist + 2.0 * rise, lift: None });
            continue;
        }
        let Some(queue) = l.queue.filter(|&q| g.walk[l.p] == 1 && g.walk[l.q] == 1 && g.walk[q] == 1) else {
            g.unreached.push(pl.id);
            continue;
        };
        let id = g.lifts.len();
        g.lifts.push(NavLift {
            id,
            terrace: pl.id,
            x: qx,
            z: qz,
            face: (l.sx as f64).atan2(l.sz as f64),
            low: pcb,
            high: pl.y,
            front: l.q,
            queue,
            door: l.p,
        });
        add_portal(&mut g, NavPortal { a: l.p, b: l.q, kind: PortalKind::Lift, cost: NAV_LIFT_COST + rise, lift: Some(id) });
    }

    // Rule 12: components of the finished walk (portals included); islands are cut.
    let joins: Vec<(usize, usize)> = g.portals.iter().map(|p| (p.a, p.b)).collect();
    let first = components(cols, rows, &g.walk, &g.y, &joins);
    let main_first = largest_at_board(&first, &g.y, pcb);
    for (w, &f) in g.walk.iter_mut().zip(&first) {
        if *w == 1 && f != main_first {
            *w = 0;
        }
    }
    g.component = components(cols, rows, &g.walk, &g.y, &joins);
    g.drops = drop_edges(cols, rows, &g.walk, &g.y);
    g
}

/// The connectivity report of a built grid (nav-grid.ts connectivity): a room is ok when its door cell is in the main
/// component and its outside cell is a walk plate cell.
pub fn connectivity(g: &NavGrid) -> Connectivity {
    let main = main_component(g);
    let components = g.component.iter().copied().max().map_or(0, |m| (m + 1).max(0) as usize);
    let mut unconnected = Vec::new();
    let mut doors_ok = 0;
    for (k, &id) in g.terrace_ids.iter().enumerate() {
        let (door, out) = g.door_cells.get(k).copied().unwrap_or((None, None));
        let ok = main >= 0
            && door.is_some_and(|d| g.component[d] == main)
            && out.is_some_and(|o| g.walk[o] == 1 && g.plate_ids.contains(&g.region[o]));
        if ok {
            doors_ok += 1;
        } else {
            unconnected.push(id);
        }
    }
    Connectivity { main, components, one_way: g.drops.len(), doors_ok, doors: g.terrace_ids.len(), unconnected, unreached: g.unreached.clone() }
}

/// The main component's id: the largest at board level (see [`largest_at_board`], the board level being the lowest
/// walk), ties to the lowest id; -1 when no cell walks.
pub fn main_component(g: &NavGrid) -> i32 {
    let level = g.component.iter().zip(&g.y).filter(|(k, _)| **k >= 0).map(|(_, y)| *y).fold(f64::INFINITY, f64::min);
    largest_at_board(&g.component, &g.y, level)
}

/// The largest component among the cells at board level (within NAV_LIFT_RISE of `pcb`), else the largest of all.
/// The main walk is the board's: two GPU cards side by side make one plate-top component larger than any part of a
/// board their parts divide, and taking that as the main one cut the whole board as an island (a 2-GPU workstation had
/// no walk on its PCB, so no landing and no lift).
fn largest_at_board(comp: &[i32], y: &[f64], pcb: f64) -> i32 {
    let low: Vec<i32> = comp.iter().zip(y).map(|(&k, &h)| if h <= pcb + NAV_LIFT_RISE { k } else { -1 }).collect();
    match largest(&low) {
        -1 => largest(comp),
        k => k,
    }
}

fn add_portal(g: &mut NavGrid, p: NavPortal) {
    let id = g.portals.len() as i32;
    if g.portal[p.a] == -1 {
        g.portal[p.a] = id;
    }
    if g.portal[p.b] == -1 {
        g.portal[p.b] = id;
    }
    g.portals.push(p);
}

/// nav-grid.ts span: the candidate cells whose open squares may overlap [lo, hi] along one axis.
fn span(lo: f64, hi: f64, origin: f64, len: usize) -> std::ops::RangeInclusive<usize> {
    let a = (((lo - origin) / NAV_CELL).floor() as i64 - 1).max(0);
    let b = (((hi - origin) / NAV_CELL).ceil() as i64).min(len as i64 - 1);
    if b < a {
        #[allow(clippy::reversed_empty_ranges)]
        return 1..=0;
    }
    a as usize..=b as usize
}

/// The squared distance from (px, pz) to the segment a-b.
fn dist_sq(px: f64, pz: f64, ax: f64, az: f64, bx: f64, bz: f64) -> f64 {
    let (vx, vz) = (bx - ax, bz - az);
    let len2 = vx * vx + vz * vz;
    let t = if len2 > 0.0 { (((px - ax) * vx + (pz - az) * vz) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let (qx, qz) = (ax + t * vx - px, az + t * vz - pz);
    qx * qx + qz * qz
}

/// True when any of the 8 neighbours of (i, j) is off the grid or blocked.
fn neighbour_blocked(g: &NavGrid, i: usize, j: usize, blocked: &[u8]) -> bool {
    DIRS8.iter().any(|&(di, dj)| g.at(i as i64 + di, j as i64 + dj).is_none_or(|v| blocked[v] == 1))
}

/// Rule 6: the plate's landing, a multi-source search from its edge cells over the board walk to the main board
/// component. A lift also needs its queue: the cell two beyond q, with the cell between, both board walk.
#[allow(clippy::too_many_arguments)]
fn landing_of(g: &NavGrid, pl: &NavPlate, pcb: f64, plate_of: &[i32], b_walk: &[u8], b_comp: &[i32], main_board: i32, clear_cell: &dyn Fn(usize, bool) -> bool) -> Option<Landing> {
    let (cols, n) = (g.cols, g.cols * g.rows);
    let ramp = pl.y - pcb <= NAV_LIFT_RISE;
    let mut parent = vec![usize::MAX; n];
    let mut root = vec![usize::MAX; n];
    let mut seen = vec![false; n];
    let mut queue = Vec::new();
    let has_open_neighbour = |c: usize| DIRS8.iter().any(|&(di, dj)| g.at((c % cols) as i64 + di, (c / cols) as i64 + dj).is_some_and(|v| plate_of[v] == 0));
    for c in 0..n {
        if plate_of[c] != pl.id || b_walk[c] != 1 || !has_open_neighbour(c) || !clear_cell(c, true) {
            continue;
        }
        seen[c] = true;
        root[c] = c;
        queue.push(c);
    }
    let walk_at = |i: i64, j: i64| g.at(i, j).is_some_and(|u| b_walk[u] == 1);
    let mut h = 0;
    while h < queue.len() {
        let c = queue[h];
        h += 1;
        let (i, j) = ((c % cols) as i64, (c / cols) as i64);
        for &(di, dj) in &DIRS8 {
            let Some(v) = g.at(i + di, j + dj) else { continue };
            if di != 0 && dj != 0 && (!walk_at(i + di, j) || !walk_at(i, j + dj)) {
                continue;
            }
            if plate_of[v] != 0 || b_walk[v] != 1 {
                continue;
            }
            if b_comp[v] == main_board && clear_cell(v, false) {
                let p = root[c];
                let (ni, nj) = (i + di, j + dj);
                let (ex, ez) = (ni - (p % cols) as i64, nj - (p / cols) as i64);
                let (sx, sz) = if ex.abs() >= ez.abs() { (ex.signum(), 0) } else { (0, ez.signum()) };
                let mid_ok = walk_at(ni + sx, nj + sz);
                let qc = g.at(ni + 2 * sx, nj + 2 * sz).filter(|&q| mid_ok && b_walk[q] == 1 && clear_cell(q, plate_of[q] != 0));
                if !ramp && qc.is_none() {
                    continue;
                }
                let mut path = Vec::new();
                let mut u = c;
                while u != usize::MAX {
                    path.push(u);
                    u = parent[u];
                }
                path.reverse();
                return Some(Landing { p, path, q: v, queue: qc, sx, sz, gang: false });
            }
            if seen[v] {
                continue;
            }
            seen[v] = true;
            root[v] = root[c];
            parent[v] = c;
            queue.push(v);
        }
    }
    None
}

/// Rule 6a: a gangway's landing: its bridge cells (the last is p), the board cell q (walk, main board component) and a
/// walk queue two cells beyond q with the cell between walk too. None when any of that fails.
fn gangway_landing(g: &NavGrid, l: &PlateLanding, b_walk: &[u8], b_comp: &[i32], main_board: i32) -> Option<Landing> {
    let cols = g.cols;
    let path: Vec<usize> = l.bridge.iter().map(|&[x, z]| g.cell_at(x, z)).collect::<Option<_>>()?;
    let p = *path.last()?;
    let q = g.cell_at(l.bx, l.bz)?;
    if b_walk[q] != 1 || b_comp[q] != main_board {
        return None;
    }
    let (pi, pj, qi, qj) = ((p % cols) as i64, (p / cols) as i64, (q % cols) as i64, (q / cols) as i64);
    let (ex, ez) = (qi - pi, qj - pj);
    let (sx, sz) = if ex.abs() >= ez.abs() { (ex.signum(), 0) } else { (0, ez.signum()) };
    g.at(qi + sx, qj + sz).filter(|&m| b_walk[m] == 1)?;
    let qc = g.at(qi + 2 * sx, qj + 2 * sz).filter(|&k| b_walk[k] == 1)?;
    Some(Landing { p, path, q, queue: Some(qc), sx, sz, gang: true })
}

/// Rule 12: the components of the walk cells (nav-grid.ts walkComponents): 8-adjacent cells with a two-way step join (a
/// diagonal only when both orthogonal cells walk), and so does each pair in `joins`. Ids run 0, 1, ... in order of each
/// component's lowest cell; -1 for a cell that does not walk.
pub fn components(cols: usize, rows: usize, walk: &[u8], y: &[f64], joins: &[(usize, usize)]) -> Vec<i32> {
    let n = cols * rows;
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let link = |parent: &mut Vec<usize>, a: usize, b: usize| {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra < rb {
            parent[rb] = ra;
        } else if rb < ra {
            parent[ra] = rb;
        }
    };
    let two_way = |a: usize, b: usize| (y[b] - y[a]).abs() <= NAV_JUMP_UP;
    for j in 0..rows {
        for i in 0..cols {
            let c = j * cols + i;
            if walk[c] != 1 {
                continue;
            }
            if i + 1 < cols && walk[c + 1] == 1 && two_way(c, c + 1) {
                link(&mut parent, c, c + 1);
            }
            if j + 1 < rows {
                let s = c + cols;
                if walk[s] == 1 && two_way(c, s) {
                    link(&mut parent, c, s);
                }
                if i + 1 < cols && walk[s + 1] == 1 && walk[c + 1] == 1 && two_way(c, s + 1) {
                    link(&mut parent, c, s + 1);
                }
                if i > 0 && walk[s - 1] == 1 && walk[c - 1] == 1 && two_way(c, s - 1) {
                    link(&mut parent, c, s - 1);
                }
            }
        }
    }
    for &(a, b) in joins {
        link(&mut parent, a, b);
    }
    let mut comp = vec![-1i32; n];
    let mut id_of = vec![-1i32; n];
    let mut next = 0;
    for c in 0..n {
        if walk[c] != 1 {
            continue;
        }
        let r = find(&mut parent, c);
        if id_of[r] < 0 {
            id_of[r] = next;
            next += 1;
        }
        comp[c] = id_of[r];
    }
    comp
}

/// Rule 12: the one-way drops (nav-grid.ts dropEdges): each 8-adjacent pair of walk cells whose step is above
/// NAV_JUMP_UP and at most NAV_DROP_DOWN, from the high cell to the low one, in index order.
fn drop_edges(cols: usize, rows: usize, walk: &[u8], y: &[f64]) -> Vec<NavDrop> {
    let mut out = Vec::new();
    let mut pair = |a: usize, b: usize| {
        let dy = y[b] - y[a];
        if dy.abs() <= NAV_JUMP_UP || dy.abs() > NAV_DROP_DOWN {
            return;
        }
        out.push(if dy < 0.0 { NavDrop { from: a, to: b } } else { NavDrop { from: b, to: a } });
    };
    for j in 0..rows {
        for i in 0..cols {
            let c = j * cols + i;
            if walk[c] != 1 {
                continue;
            }
            if i + 1 < cols && walk[c + 1] == 1 {
                pair(c, c + 1);
            }
            if j + 1 < rows {
                let s = c + cols;
                if walk[s] == 1 {
                    pair(c, s);
                }
                if i + 1 < cols && walk[s + 1] == 1 && walk[c + 1] == 1 {
                    pair(c, s + 1);
                }
                if i > 0 && walk[s - 1] == 1 && walk[c - 1] == 1 {
                    pair(c, s - 1);
                }
            }
        }
    }
    out
}

/// The largest component's id (ties: the lowest id), or -1 when there is none.
fn largest(comp: &[i32]) -> i32 {
    let mut sizes: Vec<usize> = Vec::new();
    for &k in comp.iter().filter(|&&k| k >= 0) {
        if sizes.len() <= k as usize {
            sizes.resize(k as usize + 1, 0);
        }
        sizes[k as usize] += 1;
    }
    let mut best = -1i32;
    for (k, &s) in sizes.iter().enumerate() {
        if best < 0 || s > sizes[best as usize] {
            best = k as i32;
        }
    }
    best
}
