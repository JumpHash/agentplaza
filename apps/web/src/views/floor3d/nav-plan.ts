// Path planning on the nav grid: A* over the plannable cells, string pulling, one corner-cutting pass, and the
// fixture hash. These rules are the cross-app contract (desktop mirrors them, so a Rust port must match exactly).
// The contracts are in nav-types.ts; the design is in docs/design/navigation.md (Planning).
//
// RULES
// 1. Cells. Cell index n = j * cols + i (column i, row j). Its centre is (x0 + (i + 0.5) * cell, z0 + (j + 0.5) * cell).
//    cellAt(grid, x, z) is -1 off the grid, else the index of the cell containing (x, z) (floor of (x - x0) / cell and
//    (z - z0) / cell). Plannable means walk[n] == 1. Lengths are world units; cell is NAV_CELL.
// 2. snapWalk(x, z). c = cellAt(x, z). If c >= 0 and walk[c] == 1, return c. Otherwise let (ci, cj) be the floor
//    cell coordinates of (x, z) (they may lie off the grid). For r = 1 .. 6 take every cell (i, j) with
//    max(|i - ci|, |j - cj|) == r that lies inside the grid and is plannable. If the ring has any, return the one with
//    the smallest squared distance from (x, z) to its centre; ties go to the smaller index. If no ring up to 6 has one,
//    return -1.
// 3. findPath(from, to, opts). s = snapWalk(from), g = snapWalk(to). If either is -1, return null. If s == g, return
//    the path [from, to] with y = grid.y[s] on both points, portal -1, and cost 0.
// 4. A* over plannable cells, 8-connected. Neighbour order (dx, dz): (1,0), (-1,0), (0,1), (0,-1), (1,1), (-1,1),
//    (1,-1), (-1,-1). A diagonal move is allowed only when both orthogonal cells it passes, (i+dx, j) and (i, j+dz),
//    are plannable. Move cost into n: cell * m * (cost[n] + pen), where m = 1 orthogonal, m = SQRT2 diagonal (Math.SQRT2,
//    the IEEE double sqrt(2)), and pen = penalty[n] or 0 without opts.penalty.
//    Height moves (moveKind of dy = grid.y[n] - grid.y[c], nav-grid.ts): 0 level, a walk move, adds nothing; 1 a jump
//    (a rise above NAV_STEP up to NAV_JUMP_UP) adds 2.0 * cell to the cost; 2 a drop (a fall above NAV_STEP down to
//    NAV_DROP_DOWN) adds 1.0 * cell; -1 is not a move (a cliff, or a larger step: only a portal crosses it). The kind of
//    the move into a cell is kept per cell (edge: 'jump' or 'drop' on the waypoint, rule 5).
//    Portal edges are extra to the 8 moves. For a popped cell c with p = portal[c] >= 0 and portals[p] present, the other
//    end is o = b if c == a, o = a if c == b. If o is plannable, the edge costs portals[p].cost unscaled and unpenalised,
//    and the entry records portal p.
//    Heuristic from cell n to goal cell with di = |i_n - i_g|, dj = |j_n - j_g|:
//    h(n) = 0.7 * cell * (max(di, dj) + (SQRT2 - 1) * min(di, dj)).
//    Open set: a binary min-heap keyed on (f, index), comparing f first and then index (exact comparisons). A cell
//    relaxes a neighbour only when the neighbour is not closed and the new g is strictly lower; it then sets parent,
//    the via-portal (or -1 for a plain move) and pushes (g_new + h(n), n). Popped entries whose cell is closed are
//    skipped; a closed cell is never reopened. The search stops when the goal is popped. No goal: null.
//    g is a Float64Array (start 0, others Infinity), parent and via are Int32Arrays (-1), closed is a Uint8Array.
//    cost = g[goal] (the A* cost, before smoothing).
// 5. Waypoints. W0 = {from, y = grid.y[s], portal -1, cell s}. For each path cell after the start (in order, the goal
//    cell included): centre of the cell, y = grid.y[c], portal = via[c], cell c, edge = the kind of its move (none for a
//    walk move or a portal entry). Last: {to, y = grid.y[g], portal -1, cell g}. Each waypoint keeps the cell it stands
//    on (its cell is the start's cell for W0, the goal's for the last). A fixed waypoint is one with a portal or an edge.
// 6. Visibility vis(p, q, r). len = |q - p|; n = ceil(len / (cell / 4)), at least 1. For k = 0 .. n take t = k / n and
//    the point p + (q - p) * t (exactly q when k = n). Each such point's cell must be >= 0, plannable, and have
//    region == r, and the heights of consecutive samples (grid.y of their cells) must differ by at most NAV_STEP. The
//    region r is the region of the cell that p stands on (for a segment starting at a waypoint, that
//    waypoint's cell; for a corner-cut point, its own cell). The segment is visible only if every sample passes.
// 7. String pulling. Keep index 0. With anchor a (the last kept index), let lim be the smallest fixed waypoint
//    index > a, or the last index if there is none. Scan b from min(lim, last) down to a + 1 and keep the first b with
//    vis(W[a], W[b], region of W[a]). Fixed waypoints stay fixed: b may be a fixed waypoint but never jumps over one.
//    If no b passes, keep a + 1. Repeat with anchor = b until the last index is kept.
// 8. Corner cutting, one pass over the kept list K (m points). out starts as [K0]. For i = 1 .. m - 2, let P = K[i]:
//    skip (keep P) if P is a fixed waypoint or K[i-1] or K[i+1] is one. Otherwise A = the last point of out (already
//    processed), B = K[i+1]; C1 = P + 0.25 * (A - P), C2 = P + 0.25 * (B - P). Replace P by C1, C2 (portal -1,
//    cell = cellAt(C), y = grid.y[cell]) only if vis(A, C1), vis(C1, C2) and vis(C2, B) all hold, using rule 6 with the
//    region of the first point of each segment. Otherwise keep P. Replaced points are not revisited. Then append K[m - 1].
// 9. Output. points = the final list as NavPoint {x, z, y, portal} (plus edge when set); all doubles unrounded.
//    hash = pathHash(points).
// 10. pathHash(points). For each point qx = Math.floor(x * 1000 + 0.5), qz = Math.floor(z * 1000 + 0.5); the string s
//     is the concatenation of `${qx},${qz};` over all points. FNV-1a 32-bit over the char codes of s (ASCII):
//     h = 0x811c9dc5; per char code c: h ^= c; h = Math.imul(h, 0x01000193) >>> 0. Returned as h.toString(16)
//     left-padded with zeros to 8 digits.
// 11. Determinism: no randomness, no clock, no Map iteration. Typed arrays and plain arrays only.

import { NAV_STEP } from './nav-types';
import type { NavGrid, NavPath, NavPoint } from './nav-types';
import { cellAt, moveKind } from './nav-grid';

/** Rings searched by snapWalk (rule 2). */
const SNAP_RINGS = 6;
/** Neighbour order (rule 4). */
const DX = [1, -1, 0, 0, 1, -1, 1, -1];
const DZ = [0, 0, 1, -1, 1, 1, -1, -1];
/** Heuristic scale (rule 4): the cheapest step costs 0.7 per unit of cell length. */
const H_SCALE = 0.7;
/** Extra cost of a jump and of a drop edge, in cells (rule 4). */
const JUMP_COST = 2.0;
const DROP_COST = 1.0;
/** The edge name of a move kind (moveKind 1 jump, 2 drop; 0 walk has none). */
const EDGE_OF = [undefined, 'jump', 'drop'] as const;

/** A waypoint on the way to smoothing: a point, its surface height, the portal entered (or -1), its cell, and the edge. */
interface Wp {
  x: number;
  z: number;
  y: number;
  portal: number;
  cell: number;
  edge?: 'jump' | 'drop';
}

/** Rule 7 and 8: a fixed waypoint is one entered by a portal or by a jump or drop edge. */
function isFixed(w: Wp): boolean {
  return w.portal >= 0 || w.edge !== undefined;
}

/** Region for a cell, or -2 (never a region) when the cell is off the grid. */
function regionOf(grid: NavGrid, cell: number): number {
  return cell >= 0 ? grid.region[cell]! : -2;
}

/** Rule 6: visibility of the segment p to q for samples on plannable cells of region r. */
function visible(grid: NavGrid, px: number, pz: number, qx: number, qz: number, r: number): boolean {
  const dx = qx - px;
  const dz = qz - pz;
  const len = Math.sqrt(dx * dx + dz * dz);
  let n = Math.ceil(len / (grid.cell / 4));
  if (n < 1) n = 1;
  let prevY = 0;
  for (let k = 0; k <= n; k++) {
    let x: number;
    let z: number;
    if (k === n) {
      x = qx;
      z = qz;
    } else {
      const t = k / n;
      x = px + dx * t;
      z = pz + dz * t;
    }
    const c = cellAt(grid, x, z);
    if (c < 0 || grid.walk[c] !== 1 || grid.region[c] !== r) return false;
    // Rule 6: no height change between consecutive samples beyond NAV_STEP (a jump or drop is never shortcut).
    const y = grid.y[c]!;
    if (k > 0 && Math.abs(y - prevY) > NAV_STEP) return false;
    prevY = y;
  }
  return true;
}

function less(ka: number, va: number, kb: number, vb: number): boolean {
  return ka < kb || (ka === kb && va < vb);
}

/** Binary min-heap of (key, value), ordered by key then value (rule 4). */
class MinHeap {
  private keys: number[] = [];
  private vals: number[] = [];

  get size(): number {
    return this.vals.length;
  }

  push(key: number, val: number): void {
    const keys = this.keys;
    const vals = this.vals;
    let i = vals.length;
    keys.push(key);
    vals.push(val);
    while (i > 0) {
      const p = (i - 1) >> 1;
      if (!less(key, val, keys[p]!, vals[p]!)) break;
      keys[i] = keys[p]!;
      vals[i] = vals[p]!;
      i = p;
    }
    keys[i] = key;
    vals[i] = val;
  }

  /** Removes the minimum and returns its value. The heap must not be empty. */
  pop(): number {
    const keys = this.keys;
    const vals = this.vals;
    const top = vals[0]!;
    const lastKey = keys.pop()!;
    const lastVal = vals.pop()!;
    const n = vals.length;
    if (n > 0) {
      let i = 0;
      for (;;) {
        let c = 2 * i + 1;
        if (c >= n) break;
        if (c + 1 < n && less(keys[c + 1]!, vals[c + 1]!, keys[c]!, vals[c]!)) c++;
        if (!less(keys[c]!, vals[c]!, lastKey, lastVal)) break;
        keys[i] = keys[c]!;
        vals[i] = vals[c]!;
        i = c;
      }
      keys[i] = lastKey;
      vals[i] = lastVal;
    }
    return top;
  }
}

/** Rule 2: the plannable cell nearest to (x, z), or -1. */
export function snapWalk(grid: NavGrid, x: number, z: number): number {
  const c = cellAt(grid, x, z);
  if (c >= 0 && grid.walk[c] === 1) return c;
  const ci = Math.floor((x - grid.x0) / grid.cell);
  const cj = Math.floor((z - grid.z0) / grid.cell);
  for (let r = 1; r <= SNAP_RINGS; r++) {
    let best = -1;
    let bestD = Infinity;
    for (let j = cj - r; j <= cj + r; j++) {
      if (j < 0 || j >= grid.rows) continue;
      for (let i = ci - r; i <= ci + r; i++) {
        if (i < 0 || i >= grid.cols) continue;
        if (Math.max(Math.abs(i - ci), Math.abs(j - cj)) !== r) continue;
        const n = j * grid.cols + i;
        if (grid.walk[n] !== 1) continue;
        const dx = grid.x0 + (i + 0.5) * grid.cell - x;
        const dz = grid.z0 + (j + 0.5) * grid.cell - z;
        const d = dx * dx + dz * dz;
        if (d < bestD || (d === bestD && n < best)) {
          best = n;
          bestD = d;
        }
      }
    }
    if (best >= 0) return best;
  }
  return -1;
}

/** Rules 3 to 9: the path from `from` to `to`, or null when either end cannot be snapped or no path exists. */
export function findPath(
  grid: NavGrid,
  from: { x: number; z: number },
  to: { x: number; z: number },
  opts?: { penalty?: Float64Array; noLifts?: boolean },
): NavPath | null {
  const start = snapWalk(grid, from.x, from.z);
  const goal = snapWalk(grid, to.x, to.z);
  if (start < 0 || goal < 0) return null;
  if (start === goal) {
    const points: NavPoint[] = [
      { x: from.x, z: from.z, y: grid.y[start]!, portal: -1 },
      { x: to.x, z: to.z, y: grid.y[goal]!, portal: -1 },
    ];
    return { points, cost: 0, hash: pathHash(points) };
  }

  const cols = grid.cols;
  const cell = grid.cell;
  const walk = grid.walk;
  const cost = grid.cost;
  const portal = grid.portal;
  const portals = grid.portals;
  const pen = opts && opts.penalty ? opts.penalty : null;
  const N = cols * grid.rows;
  const gi = goal % cols;
  const gj = Math.floor(goal / cols);

  const g = new Float64Array(N).fill(Infinity);
  const parent = new Int32Array(N).fill(-1);
  const via = new Int32Array(N).fill(-1);
  /** The kind of the move into each cell (rule 4): 0 walk or portal, 1 jump, 2 drop. */
  const edgeKind = new Uint8Array(N);
  const closed = new Uint8Array(N);
  const heap = new MinHeap();
  const h = (n: number): number => {
    const di = Math.abs((n % cols) - gi);
    const dj = Math.abs(Math.floor(n / cols) - gj);
    const mx = Math.max(di, dj);
    const mn = Math.min(di, dj);
    return H_SCALE * cell * (mx + (Math.SQRT2 - 1) * mn);
  };

  g[start] = 0;
  heap.push(h(start), start);
  let found = false;
  while (heap.size > 0) {
    const c = heap.pop();
    if (closed[c]) continue;
    closed[c] = 1;
    if (c === goal) {
      found = true;
      break;
    }
    const ci = c % cols;
    const cj = Math.floor(c / cols);
    const gc = g[c]!;
    for (let k = 0; k < 8; k++) {
      const dx = DX[k]!;
      const dz = DZ[k]!;
      const ni = ci + dx;
      const nj = cj + dz;
      if (ni < 0 || nj < 0 || ni >= cols || nj >= grid.rows) continue;
      const n = nj * cols + ni;
      if (walk[n] !== 1 || closed[n]) continue;
      const diag = dx !== 0 && dz !== 0;
      if (diag && (walk[cj * cols + ni] !== 1 || walk[nj * cols + ci] !== 1)) continue;
      const kind = moveKind(grid.y[n]! - grid.y[c]!);
      if (kind < 0) continue;
      const m = diag ? Math.SQRT2 : 1;
      const pn = pen ? pen[n]! : 0;
      const extra = kind === 1 ? JUMP_COST * cell : kind === 2 ? DROP_COST * cell : 0;
      const gn = gc + cell * m * (cost[n]! + pn) + extra;
      if (gn < g[n]!) {
        g[n] = gn;
        parent[n] = c;
        via[n] = -1;
        edgeKind[n] = kind;
        heap.push(gn + h(n), n);
      }
    }
    const p = portal[c]!;
    const pt = p >= 0 ? portals[p] : undefined;
    if (pt && !(opts?.noLifts === true && pt.kind === 'lift')) {
      const o = c === pt.a ? pt.b : c === pt.b ? pt.a : -1;
      if (o >= 0 && o < N && walk[o] === 1 && !closed[o]) {
        const gn = gc + pt.cost;
        if (gn < g[o]!) {
          g[o] = gn;
          parent[o] = c;
          via[o] = p;
          edgeKind[o] = 0;
          heap.push(gn + h(o), o);
        }
      }
    }
  }
  if (!found) return null;

  const cells: number[] = [];
  for (let c = goal; c !== -1; c = parent[c]!) cells.push(c);
  cells.reverse();

  // Rule 5: waypoints.
  const wps: Wp[] = [{ x: from.x, z: from.z, y: grid.y[start]!, portal: -1, cell: start }];
  for (let k = 1; k < cells.length; k++) {
    const c = cells[k]!;
    const i = c % cols;
    const j = Math.floor(c / cols);
    wps.push({
      x: grid.x0 + (i + 0.5) * cell,
      z: grid.z0 + (j + 0.5) * cell,
      y: grid.y[c]!,
      portal: via[c]!,
      cell: c,
      edge: EDGE_OF[edgeKind[c]!],
    });
  }
  wps.push({ x: to.x, z: to.z, y: grid.y[goal]!, portal: -1, cell: goal });

  // Rule 7: string pulling. nextFixed[a] is the smallest fixed waypoint index > a, or the last index.
  const last = wps.length - 1;
  const nextFixed = new Int32Array(wps.length);
  let np = last;
  for (let a = last - 1; a >= 0; a--) {
    if (isFixed(wps[a + 1]!)) np = a + 1;
    nextFixed[a] = np;
  }
  const kept: number[] = [0];
  let a = 0;
  while (a < last) {
    const from_ = wps[a]!;
    const r = regionOf(grid, from_.cell);
    let b = -1;
    for (let k = nextFixed[a]!; k > a; k--) {
      const to_ = wps[k]!;
      if (visible(grid, from_.x, from_.z, to_.x, to_.z, r)) {
        b = k;
        break;
      }
    }
    if (b < 0) b = a + 1;
    kept.push(b);
    a = b;
  }

  // Rule 8: one corner-cutting pass.
  const K = kept.map((idx) => wps[idx]!);
  const m = K.length;
  const out: Wp[] = [K[0]!];
  for (let i = 1; i < m - 1; i++) {
    const P = K[i]!;
    const A = out[out.length - 1]!;
    const B = K[i + 1]!;
    if (isFixed(P) || isFixed(K[i - 1]!) || isFixed(B)) {
      out.push(P);
      continue;
    }
    const c1x = P.x + 0.25 * (A.x - P.x);
    const c1z = P.z + 0.25 * (A.z - P.z);
    const c2x = P.x + 0.25 * (B.x - P.x);
    const c2z = P.z + 0.25 * (B.z - P.z);
    if (!visible(grid, A.x, A.z, c1x, c1z, regionOf(grid, A.cell))) {
      out.push(P);
      continue;
    }
    const cell1 = cellAt(grid, c1x, c1z);
    if (!visible(grid, c1x, c1z, c2x, c2z, regionOf(grid, cell1))) {
      out.push(P);
      continue;
    }
    const cell2 = cellAt(grid, c2x, c2z);
    if (!visible(grid, c2x, c2z, B.x, B.z, regionOf(grid, cell2))) {
      out.push(P);
      continue;
    }
    out.push(
      { x: c1x, z: c1z, y: grid.y[cell1]!, portal: -1, cell: cell1 },
      { x: c2x, z: c2z, y: grid.y[cell2]!, portal: -1, cell: cell2 },
    );
  }
  out.push(K[m - 1]!);

  // Rule 9: output.
  const points: NavPoint[] = out.map((w) => {
    const p: NavPoint = { x: w.x, z: w.z, y: w.y, portal: w.portal };
    if (w.edge !== undefined) p.edge = w.edge;
    return p;
  });
  return { points, cost: g[goal]!, hash: pathHash(points) };
}

/** Rule 10: the fixture hash of quantised points (FNV-1a, 32-bit). */
export function pathHash(points: NavPoint[]): string {
  let s = '';
  for (const p of points) {
    s += `${Math.floor(p.x * 1000 + 0.5)},${Math.floor(p.z * 1000 + 0.5)};`;
  }
  let h = 0x811c9dc5;
  for (let k = 0; k < s.length; k++) {
    h ^= s.charCodeAt(k);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return (h >>> 0).toString(16).padStart(8, '0');
}
