// Nav grid: the board, its plates (the slot tops) and the rooms on the plates as one grid of cells in world space. Pure
// TypeScript (no three.js). Both apps build the same grid, so the rules below are the contract; a Rust port follows them
// in this order, and each float expression is evaluated as written. Rules are in docs/design/navigation.md ("The nav grid").
//
// Notation
//  - cell = NAV_CELL; size = board.size; x0 = -size[0]/2, z0 = -size[2]/2; cols = ceil(size[0]/cell),
//    rows = ceil(size[2]/cell). Index c = j*cols + i; centre (x0 + (i+0.5)*cell, z0 + (j+0.5)*cell).
//  - cellAt(x, z) = (floor((x-x0)/cell), floor((z-z0)/cell)) as (i, j), or -1 outside [0,cols) x [0,rows).
//  - Plate membership: cell c is on plate P when its centre is in [P.x0, P.x0 + P.w) x [P.z0, P.z0 + P.d). plateOf[c] is
//    P.id for such a cell (later plates win an overlap), else 0.
//  - Terrace frame (turn, cosT = cos turn, sinT = sin turn). A world centre (px, pz) has dx = px-cx, dz = pz-cz and
//    local (lx, lz) = (dx*cosT - dz*sinT, dx*sinT + dz*cosT). A local point (x, z) is world
//    (cx + x*cosT + z*sinT, cz - x*sinT + z*cosT).
//  - Slab (inSlab): square |dx| < half && |dz| < half; round dx*dx + dz*dz < half*half.
//    gap = hypot(max(|dx|-half, 0), max(|dz|-half, 0)). interior = inSlab && |lx| < w/2 && |lz| < d/2.
//    band = !inSlab && gap < NAV_RAMP_DEPTH.
//  - Door side = layout.shell.door.side, with local outward normal N and lateral axis T:
//    n: N=(0,-1) T=(1,0); e: N=(1,0) T=(0,1); s: N=(0,1) T=(1,0); w: N=(-1,0) T=(0,1).
//    H = d/2 for n and s, w/2 for e and w (the inner half-size along N). out = lx*Nx + lz*Nz, lat = lx*Tx + lz*Tz.
//    layout.shell.door.at is the opening's centre, as an offset from the middle of its wall along T (local units);
//    width is the opening's full span. inSpan: |lat - at| <= width/2 + 1e-9. gen.ts centres its door zone, so at = 0.
//    Nw (outward normal, world) = (Nx*cosT + Nz*sinT, -Nx*sinT + Nz*cosT).
//  - outward(c, k) = cellAt(centre(c) + k*Nw*cell): the cell k steps outward of c (k = 1 is the cell in front of c).
//  - Height moves (moveKind, dy = y[to] - y[from]): 0 level when |dy| <= NAV_STEP (walked); 1 a jump when NAV_STEP < dy <=
//    NAV_JUMP_UP; 2 a drop when NAV_STEP < -dy <= NAV_DROP_DOWN; -1 otherwise (not a planner edge). Walking crosses a
//    height change only by a jump or a drop edge, or by a portal (rule 11). Plate cliffs follow the same limits.
//  - Ground: a walk cell's height is the ground under it (y). Part tops are NOT walkable: a part blocks its footprint
//    (rule 4) and no walk cell takes a part's top as its height.
//
// Rules
//  1. Off-board: a cell whose centre lies outside [-size0/2, size0/2] x [-size2/2, size2/2] is blocked, region 0,
//     y = pcbTop, cost OPEN. Off-board cells stay blocked whatever later rules say.
//  2. Every other cell starts: region 0, y = pcbTop, cost OPEN, not blocked.
//  3. Plates, in the given order: a cell on plate P (on board) becomes region = P.id, y = P.y, plateOf = P.id. The plate
//     is walkable at P.y unless a box (rule 4) or a room (rule 7) blocks it.
//  4. Boxes: a box blocks every cell whose open square overlaps its footprint when hi.y - y[c] > NAV_STEP, where y[c] is
//     the plate top for a plate cell and pcbTop otherwise (a part under a plate is covered by it). Candidate
//     i in [max(0, floor((lo.x-x0)/cell) - 1), min(cols-1, ceil((hi.x-x0)/cell))] (j likewise with z), then the strict
//     test x0+i*cell < hi.x && x0+(i+1)*cell > lo.x && z0+j*cell < hi.z && z0+(j+1)*cell > lo.z.
//  5. Traces: each cell with blocked = 0 and plateOf = 0 whose centre is within width/2 of a trace segment (squared
//     distance <= (width/2)^2, min over all segments) gets cost NAV_COST_TRACE. Segment distance: t = clamp(dot(p-a, b-a) /
//     |b-a|^2, 0, 1) (t = 0 for a zero-length segment); candidates from the segment's bbox grown by width/2. Plate cells
//     keep cost OPEN.
//  6. Board landings, one per plate, in plate order (computed on the board only: plates, PCB, parts and traces; no
//     rooms, no exemptions). The board walk bW = !blocked && !inflated. Let M be the main component of bW (rule 12 without
//     portals: the largest; ties: the lowest id). For plate P, the sources are the cells of P with bW = 1 that have an
//     8-neighbour off P (plateOf = 0, on the grid). Multi-source BFS from all sources, in index order, expanding into
//     8-neighbours n with bW = 1, plateOf = 0 (diagonal only when both orthogonal cells have bW = 1). When n is in M, the
//     landing is found: p = the source the path starts from, q = n. Its kind is 'ramp' when P.y - pcbTop <= NAV_LIFT_RISE,
//     else 'lift', which also needs queue = the cell two beyond q in the direction p to q (the major axis of the step,
//     two cells; the cell between them too) to have bW = 1; without it q is skipped. The path p .. q is kept (its cells have bW = 1). No landing: the plate
//     is unreached (rule 11).
//  7. Rooms (terraces), in the given order. For each terrace:
//     7a. On-board cells: an interior cell becomes region = id, y = terrace y, blocked = 0, cost OPEN. A slab cell that is
//         not interior is blocked (region and y kept). A band cell that is not on a plate is blocked (region and y kept): a
//         cliff buffer. A band cell on a plate keeps its plate (walkable at P.y).
//     7b. Door cells, set region = id, y = terrace y, blocked = 0: interior slab cells with out >= H - cell, and
//         non-interior slab cells with out >= H (the margin on the door side); both must lie in the span.
//     7c. Room solids: each layout item whose catalogue entry has place not 'surface' or 'wallmount', layer not
//         'floor', and no walk flag. Footprint in local space: sx = size[0], sz = size[2] when rot % 180 === 0, else
//         swapped; x in [x-sx/2, x+sx/2], z in [z-sz/2, z+sz/2]. A slab cell is blocked when the local bbox of its four
//         corners overlaps the footprint strictly. (Blocked cells keep region and y.)
//     7d. Door: the door cells that are not blocked, nearest the middle first (smallest |lat - at|, ties: lower index).
//         The outside cell of door d is the first outward(d, k), k = 1 .. 4, that is outside the slab (the margin cells
//         of the opening are walked over). The door is the first whose outside cell is a plate cell with blocked = 0;
//         if none, the first. doorCells[terrace] = [door, outside cell] (-1 when missing).
//     7e. Spots: each layout spot (in layout order) gets world (x, z) as in the frame above; face = spot.face * PI / 180
//         + turn (radians, not wrapped); cell = cellAt(x, z), -1 off the grid.
//  8. Exempt cells: every door cell, every landing cell (p, the path, q and the queue), every spot cell, plus their
//     8-neighbours. Exempt cells are never inflated.
//  9. Inflated: not blocked, not exempt, and at least one of its 8 neighbours is blocked or outside the grid.
//     walk = !blocked && !inflated.
// 10. Cost: blocked cells are NAV_COST_OPEN. Every other cell keeps its base cost (trace or open) and gains
//     NAV_NEAR_COST when its Chebyshev distance to the nearest blocked grid cell is exactly 2 (computed by marking the
//     3x3 block (distance <= 1) and the 5x5 ring (distance 2) around every blocked cell).
// 11. Portals, in plate order, at most one per plate (its landing). Ramp (rise <= NAV_LIFT_RISE): when walk[p] and walk[q]:
//     {a: p, b: q, kind 'ramp', cost: dist(centres) + 2*rise, lift: -1}. Lift: when walk[p], walk[q] and walk[queue]: a
//     lift record {id, terrace: plate id, x, z = q centre, face = atan2(sx, sz) of the step p to q, low = pcbTop, high =
//     plate y, front = q, queue, door = p} and {a: p, b: q, kind 'lift', cost: NAV_LIFT_COST + rise, lift: id}. A plate
//     without a usable landing (no landing in rule 6, or an end not walk here) goes to unreached.
//     portal[c] = index of the first portal whose a or b is c, else -1.
// 12. Components: walk cells are joined when 8-adjacent (a diagonal only when both orthogonal cells are walk) and a height
//     move (moveKind) runs between them in either direction, and each portal joins its two ends. Component ids are 0, 1, ... in order of their lowest
//     cell index; -1 for a cell that is not walk. Stored as grid.component. connectivity() (below) reads it.
// 13. Output: index j*cols + i. Iteration is in index order and terrace/plate/item/spot array order; no randomness, no maps.
// buildGridSteps (a generator) is the same build in slices; buildGrid drains it and returns the same grid.
//
// Connectivity (rule 12, read by connectivity()): a room is ok when its door cell is in the main component and its
// outside cell (rule 7d) is a walk plate cell. unconnected lists the terrace ids that are not ok; unreached lists the plate ids
// without a usable landing.

import type { CatItem, Door } from './rooms/gen';
import type { NavDrop, NavLift, NavPlate, NavPortal, NavTerrace, NavBoard, NavGrid } from './nav-types';
import {
  NAV_CELL,
  NAV_COST_OPEN,
  NAV_COST_TRACE,
  NAV_DROP_DOWN,
  NAV_JUMP_UP,
  NAV_LIFT_COST,
  NAV_LIFT_RISE,
  NAV_NEAR_COST,
  NAV_RAMP_DEPTH,
  NAV_STEP,
} from './nav-types';

const EPS = 1e-9;

/**
 * The height move from a cell to a neighbour, by the height difference dy = y[to] - y[from] (rules in the header):
 * 0 level (walked), 1 jump (a rise above NAV_STEP up to NAV_JUMP_UP), 2 drop (a fall above NAV_STEP down to
 * NAV_DROP_DOWN), -1 none (not a move).
 */
export function moveKind(dy: number): number {
  const a = Math.abs(dy);
  if (a <= NAV_STEP) return 0;
  if (dy > 0) return dy <= NAV_JUMP_UP ? 1 : -1;
  return a <= NAV_DROP_DOWN ? 2 : -1;
}

/** Outward local normal and lateral axis per door side. */
const SIDE_OUT: Record<Door, [number, number]> = { n: [0, -1], e: [1, 0], s: [0, 1], w: [-1, 0] };
const SIDE_LAT: Record<Door, [number, number]> = { n: [1, 0], e: [0, 1], s: [1, 0], w: [0, 1] };

/** The eight neighbour steps (di, dj), in the order the planner uses them. */
const DIRS8: [number, number][] = [
  [1, 0],
  [-1, 0],
  [0, 1],
  [0, -1],
  [1, 1],
  [-1, 1],
  [1, -1],
  [-1, -1],
];

/** Per-terrace frame: the door side, the turn and the inner half-size along the normal. */
interface Frame {
  t: NavTerrace;
  cosT: number;
  sinT: number;
  nx: number;
  nz: number;
  tx: number;
  tz: number;
  /** Outward world normal (Nw). */
  nwx: number;
  nwz: number;
  H: number;
  at: number;
  hw: number;
}

/** Scratch result of probe(). */
interface Probe {
  slab: boolean;
  interior: boolean;
  gap: number;
  out: number;
  lat: number;
  inSpan: boolean;
}

/** A plate's landing (rule 6): the plate cell p, the board path p .. q, the board cell q, and the lift queue (or -1). */
interface Landing {
  plate: NavPlate;
  p: number;
  path: number[];
  q: number;
  queue: number;
  /** The step from p to q in cells (a unit vector on the major axis). */
  sx: number;
  sz: number;
  /** A gangway (rule 6a): the path is its bridge cells, at plate height, and the portal is always a lift. */
  gang?: boolean;
}

interface Work {
  f: Frame;
  slab: number[];
  doors: number[];
}

function centreX(g: NavGrid, i: number): number {
  return g.x0 + (i + 0.5) * g.cell;
}

function centreZ(g: NavGrid, j: number): number {
  return g.z0 + (j + 0.5) * g.cell;
}

/** The cell containing (x, z), or -1 outside the grid. */
export function cellAt(grid: NavGrid, x: number, z: number): number {
  const i = Math.floor((x - grid.x0) / grid.cell);
  const j = Math.floor((z - grid.z0) / grid.cell);
  if (i < 0 || j < 0 || i >= grid.cols || j >= grid.rows) return -1;
  return j * grid.cols + i;
}

/** The cell k steps outward of cell c along the outward world normal (nwx, nwz): k = 1 is the cell in front of c. */
function outwardIdx(g: NavGrid, c: number, nwx: number, nwz: number, k: number): number {
  const i = c % g.cols;
  const j = (c - i) / g.cols;
  return cellAt(g, centreX(g, i) + nwx * g.cell * k, centreZ(g, j) + nwz * g.cell * k);
}

/** Index range [a, b] (inclusive) of cells whose open squares may overlap [lo, hi] along one axis. */
function span(lo: number, hi: number, origin: number, n: number): [number, number] {
  return [
    Math.max(0, Math.floor((lo - origin) / NAV_CELL) - 1),
    Math.min(n - 1, Math.ceil((hi - origin) / NAV_CELL)),
  ];
}

/** Squared distance from (px, pz) to the segment a-b. */
function distSq(px: number, pz: number, ax: number, az: number, bx: number, bz: number): number {
  const vx = bx - ax;
  const vz = bz - az;
  const len2 = vx * vx + vz * vz;
  let t = len2 > 0 ? ((px - ax) * vx + (pz - az) * vz) / len2 : 0;
  if (t < 0) t = 0;
  else if (t > 1) t = 1;
  const qx = ax + t * vx - px;
  const qz = az + t * vz - pz;
  return qx * qx + qz * qz;
}

function catOf(items: Record<string, CatItem>, id: string): CatItem | undefined {
  return Object.prototype.hasOwnProperty.call(items, id) ? items[id] : undefined;
}

/** A terrace's footprint for the slab tests: its centre, its slab half-side, and whether it is the round dais. */
export interface Slab {
  cx: number;
  cz: number;
  half: number;
  round: boolean;
}

/** Is (x, z) inside a terrace's slab (rule 7a): the square of half-side `half`, or the dais's circle. */
export function inSlabAt(t: Slab, x: number, z: number): boolean {
  const dx = x - t.cx;
  const dz = z - t.cz;
  return t.round ? dx * dx + dz * dz < t.half * t.half : Math.abs(dx) < t.half && Math.abs(dz) < t.half;
}

/** The distance from (x, z) to a terrace's slab (the probe's gap, rule 7a). */
export function slabGapAt(t: Slab, x: number, z: number): number {
  return Math.hypot(Math.max(Math.abs(x - t.cx) - t.half, 0), Math.max(Math.abs(z - t.cz) - t.half, 0));
}

/**
 * Is (x, z) landing-clear (rule 6): outside every terrace's slab and, when it is off every plate, at least NAV_RAMP_DEPTH
 * from every slab (the cliff band, rule 7a). A plate cell is only checked against the slabs. board-layout.ts checks its
 * landings with this same test, with the rooms in place.
 */
export function landingClearAt(terraces: Slab[], x: number, z: number, onPlate: boolean): boolean {
  for (const t of terraces) {
    if (inSlabAt(t, x, z)) return false;
    if (!onPlate && slabGapAt(t, x, z) < NAV_RAMP_DEPTH) return false;
  }
  return true;
}

/** Classify centre (px, pz) against a terrace frame into P. */
function probe(f: Frame, px: number, pz: number, P: Probe): void {
  const t = f.t;
  const dx = px - t.cx;
  const dz = pz - t.cz;
  const ax = Math.abs(dx);
  const az = Math.abs(dz);
  P.slab = t.round ? dx * dx + dz * dz < t.half * t.half : ax < t.half && az < t.half;
  P.gap = Math.hypot(Math.max(ax - t.half, 0), Math.max(az - t.half, 0));
  const lx = dx * f.cosT - dz * f.sinT;
  const lz = dx * f.sinT + dz * f.cosT;
  P.interior = P.slab && Math.abs(lx) < t.w / 2 && Math.abs(lz) < t.d / 2;
  P.out = lx * f.nx + lz * f.nz;
  P.lat = lx * f.tx + lz * f.tz;
  P.inSpan = Math.abs(P.lat - f.at) <= f.hw + EPS;
}

/** True when any of the 8 neighbours of (i, j) is outside the grid or blocked. */
function neighbourBlocked(g: NavGrid, i: number, j: number, blocked: Uint8Array): boolean {
  for (let dj = -1; dj <= 1; dj++) {
    for (let di = -1; di <= 1; di++) {
      if (di === 0 && dj === 0) continue;
      const ni = i + di;
      const nj = j + dj;
      if (ni < 0 || nj < 0 || ni >= g.cols || nj >= g.rows) return true;
      if (blocked[nj * g.cols + ni] === 1) return true;
    }
  }
  return false;
}

/** Union-find root of a with path halving. */
function find(parent: Int32Array, a: number): number {
  let x = a;
  while (parent[x] !== x) {
    const g = parent[parent[x]!]!;
    parent[x] = g;
    x = g;
  }
  return x;
}

/**
 * A two-way link (rule 12): the height step between two cells is one a robot can make in both directions, that is at most
 * NAV_JUMP_UP (a walk or a jump up, and a walk or a drop down). A bigger step (up to NAV_DROP_DOWN) is one-way: only a
 * drop, never a climb back.
 */
function twoWay(ya: number, yb: number): boolean {
  return Math.abs(yb - ya) <= NAV_JUMP_UP;
}

/**
 * Rule 12: the component of each walk cell (-1 otherwise). Walk cells join when 8-adjacent (a diagonal only when both
 * orthogonal cells are walk) and the step between them is two-way (twoWay); each portal joins its two ends. One-way drops
 * do not join (see dropEdges).
 */
export function walkComponents(
  cols: number,
  rows: number,
  walk: Uint8Array,
  y: Float64Array,
  portals: NavPortal[],
): Int32Array {
  const N = cols * rows;
  const parent = new Int32Array(N);
  for (let c = 0; c < N; c++) parent[c] = c;
  const link = (a: number, b: number): void => {
    const ra = find(parent, a);
    const rb = find(parent, b);
    if (ra < rb) parent[rb] = ra;
    else if (rb < ra) parent[ra] = rb;
  };
  const linked = (a: number, b: number): boolean => twoWay(y[a]!, y[b]!);
  for (let j = 0; j < rows; j++) {
    for (let i = 0; i < cols; i++) {
      const c = j * cols + i;
      if (walk[c] !== 1) continue;
      if (i + 1 < cols && walk[c + 1] === 1 && linked(c, c + 1)) link(c, c + 1);
      if (j + 1 < rows) {
        const s = c + cols;
        if (walk[s] === 1 && linked(c, s)) link(c, s);
        if (i + 1 < cols && walk[s + 1] === 1 && walk[c + 1] === 1 && linked(c, s + 1)) link(c, s + 1);
        if (i > 0 && walk[s - 1] === 1 && walk[c - 1] === 1 && linked(c, s - 1)) link(c, s - 1);
      }
    }
  }
  for (const p of portals) link(p.a, p.b);
  const comp = new Int32Array(N).fill(-1);
  const idOf = new Int32Array(N).fill(-1);
  let next = 0;
  for (let c = 0; c < N; c++) {
    if (walk[c] !== 1) continue;
    const r = find(parent, c);
    if (idOf[r]! < 0) idOf[r] = next++;
    comp[c] = idOf[r]!;
  }
  return comp;
}

/**
 * Rule 12: the one-way drops. Every 8-adjacent pair of walk cells (the same neighbours as walkComponents) whose step is
 * not two-way (NAV_JUMP_UP < step <= NAV_DROP_DOWN) becomes one directed drop, from the high cell to the low one, in index
 * order. The planner takes it downhill only (moveKind 2).
 */
export function dropEdges(cols: number, rows: number, walk: Uint8Array, y: Float64Array): NavDrop[] {
  const out: NavDrop[] = [];
  const pair = (a: number, b: number): void => {
    const dy = y[b]! - y[a]!;
    const ad = Math.abs(dy);
    if (ad <= NAV_JUMP_UP || ad > NAV_DROP_DOWN) return;
    out.push(dy < 0 ? { from: a, to: b } : { from: b, to: a });
  };
  for (let j = 0; j < rows; j++) {
    for (let i = 0; i < cols; i++) {
      const c = j * cols + i;
      if (walk[c] !== 1) continue;
      if (i + 1 < cols && walk[c + 1] === 1) pair(c, c + 1);
      if (j + 1 < rows) {
        const s = c + cols;
        if (walk[s] === 1) pair(c, s);
        if (i + 1 < cols && walk[s + 1] === 1 && walk[c + 1] === 1) pair(c, s + 1);
        if (i > 0 && walk[s - 1] === 1 && walk[c - 1] === 1) pair(c, s - 1);
      }
    }
  }
  return out;
}

/** The number of cells in each component id (ids run 0 .. max). */
function sizesOf(comp: Int32Array): number[] {
  const sizes: number[] = [];
  for (let c = 0; c < comp.length; c++) {
    const k = comp[c]!;
    if (k < 0) continue;
    while (sizes.length <= k) sizes.push(0);
    sizes[k] = sizes[k]! + 1;
  }
  return sizes;
}

/** The largest component's id (ties: the lowest id); -1 when there is none. */
function largestOf(sizes: number[]): number {
  let best = -1;
  for (let k = 0; k < sizes.length; k++) if (best < 0 || sizes[k]! > sizes[best]!) best = k;
  return best;
}

/**
 * The nav grid built in slices. The steps are the base cells and plates, the boxes (`chunk` per step), the traces, the
 * board landings, each terrace, then inflation, costs, portals and components. Each step yields its progress in [0, 1]
 * (the last yield is 1), and the generator returns the grid. The output does not depend on `chunk` or on how the caller
 * paces the generator.
 */
export function* buildGridSteps(board: NavBoard, terraces: NavTerrace[], chunk = 64): Generator<number, NavGrid, void> {
  const cell = NAV_CELL;
  const size0 = board.size[0];
  const size2 = board.size[2];
  const x0 = -size0 / 2;
  const z0 = -size2 / 2;
  const cols = Math.ceil(size0 / cell);
  const rows = Math.ceil(size2 / cell);
  const N = cols * rows;
  const pcb = board.pcbTop;
  const boxes = board.boxes;
  const plates = board.plates ?? [];

  // Progress: the base cells and plates (1), one per box chunk, the traces (1), the board landings (1), one per terrace,
  // then inflation, costs, portals and components (4). A chunk below 1 counts as 1; the chunk never exceeds the box count.
  const per = Math.max(1, Math.min(Math.floor(chunk) || 1, boxes.length));
  const total = 1 + (boxes.length === 0 ? 0 : Math.ceil(boxes.length / per)) + 1 + 1 + terraces.length + 4;
  let done = 0;
  const advance = (): number => {
    done += 1;
    return done / total;
  };

  const grid: NavGrid = {
    cell,
    cols,
    rows,
    x0,
    z0,
    walk: new Uint8Array(N),
    blocked: new Uint8Array(N),
    inflated: new Uint8Array(N),
    region: new Int32Array(N),
    y: new Float64Array(N).fill(pcb),
    cost: new Float64Array(N).fill(NAV_COST_OPEN),
    portal: new Int32Array(N).fill(-1),
    portals: [],
    lifts: [],
    spots: [],
    terraces,
    plates,
    unreached: [],
    doorCells: [],
  };
  const { walk, blocked, inflated, region, y, cost, portal } = grid;
  const off = new Uint8Array(N);
  grid.off = off; // exported for the vertical body (nav-height.ts): an off-board cell has no floor. No rule reads it.
  const plateOf = new Int32Array(N);

  // Rule 1: off-board cells.
  for (let j = 0; j < rows; j++) {
    for (let i = 0; i < cols; i++) {
      if (Math.abs(centreX(grid, i)) > size0 / 2 || Math.abs(centreZ(grid, j)) > size2 / 2) {
        const c = j * cols + i;
        off[c] = 1;
        blocked[c] = 1;
      }
    }
  }

  // Rule 3: plates (their cells take the plate's region and height).
  for (const pl of plates) {
    for (let j = 0; j < rows; j++) {
      for (let i = 0; i < cols; i++) {
        const c = j * cols + i;
        if (off[c] === 1) continue;
        const px = centreX(grid, i);
        const pz = centreZ(grid, j);
        if (px >= pl.x0 && px < pl.x0 + pl.w && pz >= pl.z0 && pz < pl.z0 + pl.d) {
          plateOf[c] = pl.id;
          region[c] = pl.id;
          y[c] = pl.y;
        }
      }
    }
  }

  yield advance();

  // Rule 4: tall boxes block their footprints, `per` boxes per step (a cell's threshold is its own height, y[c]).
  for (let k0 = 0; k0 < boxes.length; k0 += per) {
    const k1 = Math.min(boxes.length, k0 + per);
    for (let k = k0; k < k1; k++) {
      const b = boxes[k]!;
      const [i0, i1] = span(b.lo[0], b.hi[0], x0, cols);
      const [j0, j1] = span(b.lo[2], b.hi[2], z0, rows);
      for (let j = j0; j <= j1; j++) {
        for (let i = i0; i <= i1; i++) {
          const ax0 = x0 + i * cell;
          const ax1 = x0 + (i + 1) * cell;
          const az0 = z0 + j * cell;
          const az1 = z0 + (j + 1) * cell;
          const c = j * cols + i;
          if (ax0 < b.hi[0] && ax1 > b.lo[0] && az0 < b.hi[2] && az1 > b.lo[2] && b.hi[1] - y[c]! > NAV_STEP) {
            blocked[c] = 1;
          }
        }
      }
    }
    yield advance();
  }

  // Rule 5: copper traces cost less than open PCB (only PCB cells still open at this point).
  for (const tr of board.traces) {
    const r = tr.width / 2;
    const r2 = r * r;
    for (let k = 0; k + 1 < tr.path.length; k++) {
      const [ax, az] = tr.path[k]!;
      const [bx, bz] = tr.path[k + 1]!;
      const [i0, i1] = span(Math.min(ax, bx) - r, Math.max(ax, bx) + r, x0, cols);
      const [j0, j1] = span(Math.min(az, bz) - r, Math.max(az, bz) + r, z0, rows);
      for (let j = j0; j <= j1; j++) {
        for (let i = i0; i <= i1; i++) {
          const c = j * cols + i;
          if (blocked[c] === 0 && plateOf[c] === 0 && distSq(centreX(grid, i), centreZ(grid, j), ax, az, bx, bz) <= r2) {
            cost[c] = NAV_COST_TRACE;
          }
        }
      }
    }
  }

  yield advance();

  // Rule 6: board landings, from the board-only walk (no rooms, no exemptions).
  const bWalk = new Uint8Array(N);
  for (let j = 0; j < rows; j++) {
    for (let i = 0; i < cols; i++) {
      const c = j * cols + i;
      if (blocked[c] === 1) continue;
      bWalk[c] = neighbourBlocked(grid, i, j, blocked) ? 0 : 1;
    }
  }
  const bComp = walkComponents(cols, rows, bWalk, y, []);
  const mainBoard = largestOf(sizesOf(bComp));

  const hasOpenNeighbour = (i: number, j: number): boolean => {
    for (const [di, dj] of DIRS8) {
      const ni = i + di;
      const nj = j + dj;
      if (ni < 0 || nj < 0 || ni >= cols || nj >= rows) continue;
      if (plateOf[nj * cols + ni] === 0) return true;
    }
    return false;
  };
  // Landing clearance (rule 6, with the terraces' geometry, before any room cell is set): a plate source is outside every
  // slab, and the board cell q and a lift's queue are outside every slab and cliff band. Without it a landing could sit in a
  // room's band, where rule 7a blocks it and the plate would be cut off (the layout checks its landings the same way).
  const cellXZ = (c: number): [number, number] => [centreX(grid, c % cols), centreZ(grid, Math.floor(c / cols))];
  const clearCell = (c: number, onPlate: boolean): boolean => {
    const [x, z] = cellXZ(c);
    return landingClearAt(terraces, x, z, onPlate);
  };
  const landingOf = (pl: NavPlate): Landing | null => {
    const ramp = pl.y - pcb <= NAV_LIFT_RISE;
    const parent = new Int32Array(N).fill(-1);
    const root = new Int32Array(N).fill(-1);
    const seen = new Uint8Array(N);
    const queue: number[] = [];
    for (let c = 0; c < N; c++) {
      if (plateOf[c] !== pl.id || bWalk[c] !== 1) continue;
      const i = c % cols;
      if (!hasOpenNeighbour(i, (c - i) / cols)) continue;
      if (!clearCell(c, true)) continue;
      seen[c] = 1;
      root[c] = c;
      queue.push(c);
    }
    for (let h = 0; h < queue.length; h++) {
      const c = queue[h]!;
      const i = c % cols;
      const j = (c - i) / cols;
      for (const [di, dj] of DIRS8) {
        const ni = i + di;
        const nj = j + dj;
        if (ni < 0 || nj < 0 || ni >= cols || nj >= rows) continue;
        if (di !== 0 && dj !== 0 && (bWalk[j * cols + ni] !== 1 || bWalk[nj * cols + i] !== 1)) continue;
        const n = nj * cols + ni;
        if (plateOf[n] !== 0 || bWalk[n] !== 1) continue;
        if (bComp[n] === mainBoard && clearCell(n, false)) {
          const p = root[c]!;
          const pi = p % cols;
          const pj = (p - pi) / cols;
          const ex = ni - pi;
          const ez = nj - pj;
          const sx = Math.abs(ex) >= Math.abs(ez) ? Math.sign(ex) : 0;
          const sz = Math.abs(ex) >= Math.abs(ez) ? 0 : Math.sign(ez);
          // The queue is two cells out from the board cell (the cab's deck covers the first); the cell between must be walk.
          const mi = ni + sx;
          const mj = nj + sz;
          const midOk = mi >= 0 && mj >= 0 && mi < cols && mj < rows && bWalk[mj * cols + mi] === 1;
          const ki = ni + 2 * sx;
          const kj = nj + 2 * sz;
          const qc = ki >= 0 && kj >= 0 && ki < cols && kj < rows ? kj * cols + ki : -1;
          const queueOk = midOk && qc >= 0 && bWalk[qc] === 1 && clearCell(qc, plateOf[qc] !== 0);
          if (!ramp && !queueOk) continue;
          const path: number[] = [];
          for (let k = c; k !== -1; k = parent[k]!) path.push(k);
          path.reverse();
          return { plate: pl, p, path, q: n, queue: queueOk ? qc : -1, sx, sz };
        }
        if (seen[n] === 1) continue;
        seen[n] = 1;
        root[n] = root[c]!;
        parent[n] = c;
        queue.push(n);
      }
    }
    return null;
  };
  // Rule 6a: a gangway (the layout's landing where no plain one exists). Its bridge cells run from the plate edge out over
  // the blocked cells to the lift's door, the last bridge cell p; q is the board cell it lands on (walk, main component),
  // with a walk queue beyond q. Null when any of that fails, and the plate keeps its plain landing search.
  const gangwayLanding = (pl: NavPlate): Landing | null => {
    const land = pl.landing;
    if (!land || land.kind !== 'gangway' || !land.bridge || land.bridge.length === 0) return null;
    const path: number[] = [];
    for (const [x, z] of land.bridge) {
      const c = cellAt(grid, x, z);
      if (c < 0) return null;
      path.push(c);
    }
    const p = path[path.length - 1]!;
    const q = cellAt(grid, land.bx, land.bz);
    if (q < 0 || bWalk[q] !== 1 || bComp[q] !== mainBoard) return null;
    const pi = p % cols;
    const pj = (p - pi) / cols;
    const qi = q % cols;
    const qj = (q - qi) / cols;
    const ex = qi - pi;
    const ez = qj - pj;
    const sx = Math.abs(ex) >= Math.abs(ez) ? Math.sign(ex) : 0;
    const sz = Math.abs(ex) >= Math.abs(ez) ? 0 : Math.sign(ez);
    // The queue is two cells out from the lift's cell q; the cell between them must be walk too.
    const mi = qi + sx;
    const mj = qj + sz;
    if (mi < 0 || mj < 0 || mi >= cols || mj >= rows || bWalk[mj * cols + mi] !== 1) return null;
    const ki = qi + 2 * sx;
    const kj = qj + 2 * sz;
    const qc = ki >= 0 && kj >= 0 && ki < cols && kj < rows ? kj * cols + ki : -1;
    if (qc < 0 || bWalk[qc] !== 1) return null;
    return { plate: pl, p, path, q, queue: qc, sx, sz, gang: true };
  };
  const landed: (Landing | null)[] = plates.map((pl) => (pl.landing?.kind === 'gangway' ? gangwayLanding(pl) : null) ?? landingOf(pl));

  yield advance();

  // Rule 7: rooms, in order.
  const P: Probe = { slab: false, interior: false, gap: 0, out: 0, lat: 0, inSpan: false };
  const works: Work[] = [];
  for (const t of terraces) {
    const door = t.layout.shell.door;
    const [nx, nz] = SIDE_OUT[door.side];
    const [tx, tz] = SIDE_LAT[door.side];
    const cosT = Math.cos(t.turn);
    const sinT = Math.sin(t.turn);
    const f: Frame = {
      t,
      cosT,
      sinT,
      nx,
      nz,
      tx,
      tz,
      nwx: nx * cosT + nz * sinT,
      nwz: -nx * sinT + nz * cosT,
      H: Math.abs(nx) * (t.w / 2) + Math.abs(nz) * (t.d / 2),
      at: door.at,
      hw: door.width / 2,
    };
    const w: Work = { f, slab: [], doors: [] };
    works.push(w);

    // 7a: interior, slab margin and band.
    for (let j = 0; j < rows; j++) {
      for (let i = 0; i < cols; i++) {
        const c = j * cols + i;
        if (off[c] === 1) continue;
        probe(f, centreX(grid, i), centreZ(grid, j), P);
        if (P.slab) {
          w.slab.push(c);
          if (P.interior) {
            region[c] = t.id;
            y[c] = t.y;
            blocked[c] = 0;
            cost[c] = NAV_COST_OPEN;
          } else {
            blocked[c] = 1;
          }
        } else if (P.gap < NAV_RAMP_DEPTH && plateOf[c] === 0) {
          blocked[c] = 1;
        }
      }
    }

    // 7b: door cells (innermost interior row and the margin on the door side, within the span).
    for (const c of w.slab) {
      const i = c % cols;
      const j = (c - i) / cols;
      probe(f, centreX(grid, i), centreZ(grid, j), P);
      if (!P.inSpan) continue;
      const isDoor = P.interior ? P.out >= f.H - cell : P.out >= f.H;
      if (!isDoor) continue;
      blocked[c] = 0;
      region[c] = t.id;
      y[c] = t.y;
      w.doors.push(c);
    }

    // 7c: room solids block the slab cells their footprints overlap.
    for (const it of t.layout.items) {
      const cat = catOf(t.catalog.items, it.id);
      if (!cat || cat.place === 'surface' || cat.place === 'wallmount' || cat.layer === 'floor' || cat.walk) continue;
      const along = it.rot % 180 === 0;
      const sx = along ? cat.size[0] : cat.size[2];
      const sz = along ? cat.size[2] : cat.size[0];
      const fx0 = it.x - sx / 2;
      const fx1 = it.x + sx / 2;
      const fz0 = it.z - sz / 2;
      const fz1 = it.z + sz / 2;
      for (const c of w.slab) {
        const i = c % cols;
        const j = (c - i) / cols;
        const xa = x0 + i * cell;
        const xb = x0 + (i + 1) * cell;
        const za = z0 + j * cell;
        const zb = z0 + (j + 1) * cell;
        let mnx = Infinity;
        let mxx = -Infinity;
        let mnz = Infinity;
        let mxz = -Infinity;
        for (const X of [xa, xb]) {
          for (const Z of [za, zb]) {
            const dx = X - t.cx;
            const dz = Z - t.cz;
            const lx = dx * cosT - dz * sinT;
            const lz = dx * sinT + dz * cosT;
            if (lx < mnx) mnx = lx;
            if (lx > mxx) mxx = lx;
            if (lz < mnz) mnz = lz;
            if (lz > mxz) mxz = lz;
          }
        }
        if (mnx < fx1 && mxx > fx0 && mnz < fz1 && mxz > fz0) blocked[c] = 1;
      }
    }

    // 7d: the door: the open door cells, nearest the middle first. The outside cell of a door is the first cell outward
    // of it (within 4 cells) that lies outside the slab, so the door's margin cells are walked over. The door is the
    // first whose outside cell is an open plate cell; if none, the first.
    const outsideOf = (d: number): number => {
      for (let k = 1; k <= 4; k++) {
        const o = outwardIdx(grid, d, f.nwx, f.nwz, k);
        if (o < 0) return -1;
        const oi = o % cols;
        probe(f, centreX(grid, oi), centreZ(grid, (o - oi) / cols), P);
        if (!P.slab) return o;
        if (blocked[o] !== 0) return -1;
      }
      return -1;
    };
    const order: [number, number][] = [];
    for (const d of w.doors) {
      if (blocked[d] !== 0) continue;
      const di = d % cols;
      probe(f, centreX(grid, di), centreZ(grid, (d - di) / cols), P);
      order.push([Math.abs(P.lat - f.at), d]);
    }
    order.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
    let doorCell = -1;
    let outCell = -1;
    for (const [, d] of order) {
      const o = outsideOf(d);
      if (o >= 0 && blocked[o] === 0 && plateOf[o] !== 0) {
        doorCell = d;
        outCell = o;
        break;
      }
    }
    if (doorCell < 0 && order.length > 0) {
      doorCell = order[0]![1];
      outCell = outsideOf(doorCell);
    }
    grid.doorCells.push([doorCell, outCell]);

    // 7e: spots, in world space.
    for (let k = 0; k < t.layout.spots.length; k++) {
      const sp = t.layout.spots[k]!;
      const wx = t.cx + sp.x * cosT + sp.z * sinT;
      const wz = t.cz - sp.x * sinT + sp.z * cosT;
      grid.spots.push({
        terrace: t.id,
        index: k,
        x: wx,
        z: wz,
        face: (sp.face * Math.PI) / 180 + t.turn,
        cell: cellAt(grid, wx, wz),
      });
    }
    yield advance();
  }

  // Rule 6a (after the rooms): a gangway's bridge cells become walk at the plate's height, over the parts and the terrace
  // cliff band they cross. A bridge cell inside a room's slab (its walls) stays blocked, so a bridge never walks through a
  // wall; a gangway whose bridge meets a wall is then not usable, and its plate is reported as unreached.
  const inSlab = (x: number, z: number): boolean =>
    terraces.some((t) => {
      const dx = x - t.cx;
      const dz = z - t.cz;
      return t.round ? dx * dx + dz * dz < t.half * t.half : Math.abs(dx) < t.half && Math.abs(dz) < t.half;
    });
  for (const L of landed) {
    if (!L?.gang) continue;
    for (const c of L.path) {
      const i = c % cols;
      const j = (c - i) / cols;
      if (inSlab(centreX(grid, i), centreZ(grid, j))) continue;
      blocked[c] = 0;
      region[c] = L.plate.id;
      y[c] = L.plate.y;
    }
  }

  // Rule 8: exempt cells (door cells, landing cells, spot cells) and their 8-neighbours.
  const exempt = new Uint8Array(N);
  const markExempt = (c: number): void => {
    const i = c % cols;
    const j = (c - i) / cols;
    for (let dj = -1; dj <= 1; dj++) {
      for (let di = -1; di <= 1; di++) {
        const ni = i + di;
        const nj = j + dj;
        if (ni < 0 || nj < 0 || ni >= cols || nj >= rows) continue;
        exempt[nj * cols + ni] = 1;
      }
    }
  };
  for (const w of works) for (const d of w.doors) markExempt(d);
  for (const L of landed) {
    if (L === null) continue;
    for (const c of L.path) markExempt(c);
    markExempt(L.q);
    if (L.queue >= 0) markExempt(L.queue);
  }
  for (const sp of grid.spots) {
    if (sp.cell >= 0) markExempt(sp.cell);
  }

  // Rule 9: inflation and walkability.
  for (let j = 0; j < rows; j++) {
    for (let i = 0; i < cols; i++) {
      const c = j * cols + i;
      if (blocked[c] === 1) {
        walk[c] = 0;
        continue;
      }
      const inf = exempt[c] === 0 && neighbourBlocked(grid, i, j, blocked);
      inflated[c] = inf ? 1 : 0;
      walk[c] = inf ? 0 : 1;
    }
  }

  yield advance();

  // Rule 10: cost. Blocked cells are OPEN; cells at Chebyshev distance exactly 2 from a blocked cell gain NEAR.
  const near = new Uint8Array(N);
  const ring = new Uint8Array(N);
  for (let j = 0; j < rows; j++) {
    for (let i = 0; i < cols; i++) {
      if (blocked[j * cols + i] !== 1) continue;
      for (let dj = -2; dj <= 2; dj++) {
        for (let di = -2; di <= 2; di++) {
          const ni = i + di;
          const nj = j + dj;
          if (ni < 0 || nj < 0 || ni >= cols || nj >= rows) continue;
          const d = Math.max(Math.abs(di), Math.abs(dj));
          if (d <= 1) near[nj * cols + ni] = 1;
          else ring[nj * cols + ni] = 1;
        }
      }
    }
  }
  for (let c = 0; c < N; c++) {
    if (blocked[c] === 1) {
      cost[c] = NAV_COST_OPEN;
    } else if (ring[c] === 1 && near[c] === 0) {
      cost[c] = cost[c]! + NAV_NEAR_COST;
    }
  }

  yield advance();

  // Rule 11: portals and lifts, one per plate landing.
  const addPortal = (p: NavPortal): void => {
    const id = grid.portals.length;
    grid.portals.push(p);
    if (portal[p.a] === -1) portal[p.a] = id;
    if (portal[p.b] === -1) portal[p.b] = id;
  };
  for (let k = 0; k < plates.length; k++) {
    const pl = plates[k]!;
    const L = landed[k] ?? null;
    if (L === null) {
      grid.unreached.push(pl.id);
      continue;
    }
    const rise = pl.y - pcb;
    const pi = L.p % cols;
    const pj = (L.p - pi) / cols;
    const qi = L.q % cols;
    const qj = (L.q - qi) / cols;
    if (rise <= NAV_LIFT_RISE && !L.gang) {
      if (walk[L.p] !== 1 || walk[L.q] !== 1) {
        grid.unreached.push(pl.id);
        continue;
      }
      const dist = Math.hypot(centreX(grid, qi) - centreX(grid, pi), centreZ(grid, qj) - centreZ(grid, pj));
      addPortal({ a: L.p, b: L.q, kind: 'ramp', cost: dist + 2 * rise, lift: -1 });
      continue;
    }
    if (walk[L.p] !== 1 || walk[L.q] !== 1 || L.queue < 0 || walk[L.queue] !== 1) {
      grid.unreached.push(pl.id);
      continue;
    }
    const rec: NavLift = {
      id: grid.lifts.length,
      terrace: pl.id,
      x: centreX(grid, qi),
      z: centreZ(grid, qj),
      face: Math.atan2(L.sx, L.sz),
      low: pcb,
      high: pl.y,
      front: L.q,
      queue: L.queue,
      door: L.p,
    };
    grid.lifts.push(rec);
    addPortal({ a: L.p, b: L.q, kind: 'lift', cost: NAV_LIFT_COST + rise, lift: rec.id });
  }

  yield advance();

  // Rule 12: components of the finished walk (portals included).
  // Islands are cut: a walk cell outside the main component is not walk, so no robot can fall into a place it cannot
  // leave (a seat in one fails the seat test in test/tests.ts). Cutting an island removes no link of the main.
  const first = walkComponents(cols, rows, walk, y, grid.portals);
  const mainId = largestOf(sizesOf(first));
  for (let c = 0; c < N; c++) {
    if (walk[c] === 1 && first[c] !== mainId) walk[c] = 0;
  }
  grid.component = walkComponents(cols, rows, walk, y, grid.portals);
  grid.drops = dropEdges(cols, rows, walk, y);

  yield advance();
  return grid;
}

/** The nav grid of a board and its terraces in one call: buildGridSteps drained to its result. */
export function buildGrid(board: NavBoard, terraces: NavTerrace[]): NavGrid {
  const it = buildGridSteps(board, terraces);
  let r = it.next();
  while (!r.done) r = it.next();
  return r.value;
}

/** The connectivity report of a built grid (rule 12 and the header's Connectivity). */
export interface NavConnectivity {
  /** The component id of the largest component of walk cells (ties: the lowest id); -1 when no cell is walk. */
  main: number;
  /** The number of components of walk cells. */
  components: number;
  /** The one-way drops (directed, between components; rule 12). */
  oneWay: number;
  /** Rooms whose door cell is in the main component and whose outward cell is a walk plate cell. */
  doorsOk: number;
  /** The number of rooms (terraces). */
  doors: number;
  /** The terrace ids of the rooms that are not ok. */
  unconnected: number[];
  /** The plate ids with no usable landing. */
  unreached: number[];
}

/** The connectivity report of a grid. Computes the component ids once when the grid has none (buildGrid sets them). */
export function connectivity(grid: NavGrid): NavConnectivity {
  if (!grid.component) grid.component = walkComponents(grid.cols, grid.rows, grid.walk, grid.y, grid.portals);
  if (!grid.drops) grid.drops = dropEdges(grid.cols, grid.rows, grid.walk, grid.y);
  const comp = grid.component;
  const sizes = sizesOf(comp);
  const main = largestOf(sizes);
  const plateIds = new Set(grid.plates.map((p) => p.id));
  const unconnected: number[] = [];
  let doorsOk = 0;
  grid.terraces.forEach((t, k) => {
    const [door, out] = grid.doorCells[k] ?? [-1, -1];
    const ok =
      main >= 0 &&
      door >= 0 &&
      comp[door] === main &&
      out >= 0 &&
      grid.walk[out] === 1 &&
      plateIds.has(grid.region[out]!);
    if (ok) doorsOk++;
    else unconnected.push(t.id);
  });
  return {
    main,
    components: sizes.length,
    oneWay: grid.drops.length,
    doorsOk,
    doors: grid.terraces.length,
    unconnected,
    unreached: [...grid.unreached],
  };
}
