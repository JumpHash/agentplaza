// The board plaza's layout (pure, no rendering): where each role's room sits on a board model's slots.
// It returns the Floor's own layout shape (districts and homes, as layoutDistricts does) plus each
// slot's height and its plate (the aisle, the landing), so the rest of the Floor and the nav read the same values.
// Desktop mirrors it in floor3d/world/board_layout.rs.

import type { Agent } from '../../api';
import { partBoxes, type PartBox } from './zfight';
import { roleFamily, type Family } from '../../roles';
import { roomStyle } from './rooms';
import type { Door } from './rooms/gen';
import { buildModel, evalWhen, parseWhen, type Model, type Params, type Part, type Slot, type SlotType, type Vec3 } from './hardware';
import { PLATE, supportField, topBelow } from './ground';
import { buildGrid, cellAt, connectivity, landingClearAt, type Slab } from './nav-grid';
import { navScene } from './nav-scene';
import { NAV_CELL, NAV_LIFT_RISE, NAV_RAMP_DEPTH, type NavBoard, type NavGrid, type NavPlate } from './nav-types';
import minSizeTable from '../../../../../assets/rooms/minsize.json';

/** The room minimums (assets/rooms/minsize.json): role -> the smallest square side for 1..6 agents, null = none fits. */
export type MinSizeTable = Record<string, (number | null)[]>;

/** A district as the Floor draws it: the plaza x (cx), z (cy), the room's radius, hue and shape. */
export interface District {
  role: string;
  cx: number;
  cy: number;
  r: number;
  hue: number;
  shape: 'square' | 'round';
}

/** A district on a board: its slot's top height, the slot's id, the room's cell size and its turn and door. */
export interface BoardDistrict extends District {
  y: number;
  slot: string;
  /** The room's cell along the world x and z axes (before the room turn): the cell's half widths are w / 2 and d / 2. */
  w: number;
  d: number;
  /** The room's turn about y (three.js rotation.y) and the side of its room frame that is the door. */
  turn: number;
  door: Door;
}

/** A slot the layout leaves out, and why. */
export interface UnusableSlot {
  slot: string;
  reason: string;
}

export interface BoardLayout {
  districts: BoardDistrict[];
  /** Each agent's console slot inside its district: [district, gx, gy] (gy is the plaza z). */
  homes: Map<number, [District, number, number]>;
  atom: [number, number, number];
  /** The board's footprint, centred on the origin, for camera framing. */
  bounds: { cx: number; cz: number; w: number; d: number };
  /** The PCB's top: the surface height away from every room. */
  pcbTop: number;
  /** The usable slots as plates (region ids after the districts'), each with its landing on the board. */
  plates: NavPlate[];
  /** The slots the aisle rules leave out, each with its reason. */
  unusable: UnusableSlot[];
  /** The board's world scale: its slots, tops, anchors and part boxes are this many times the model's (rooms and desks are not). */
  boardScale: number;
  /** Each usable slot's aisle: double (aisle down the middle, rooms both sides) or single, and whether it leaves by a spur. */
  aisles: { slot: string; double: boolean; spur: boolean; gangway: boolean }[];
}

/**
 * Where each family's rooms go, in order; each type is one stage, then any slot with room. This order is
 * also the move order for a room that does not fit its slot: the next slot of its family's types, then any.
 */
const FAMILY_TYPES: Record<Family, SlotType[]> = {
  command: ['cpu', 'soc'],
  build: ['gpu', 'npu', 'cpu', 'soc'],
  knowledge: ['memory', 'storage'],
  operations: ['network', 'io', 'bmc', 'modem'],
  money: ['power', 'io'],
  other: ['storage'],
};

/** The ramp band around a district's cell, in grid units, sloping down to the PCB top. */
const RAMP = 1;
/** The orchestrator's room look, as the Floor's dais has it. */
const DAIS_HUE = 25;
/** The Floor's curated district hues and role hash (floor.ts roleHue): the same colour per role. */
const HUES = [8, 28, 46, 88, 140, 172, 198, 222, 262, 340];
/** The aisle's width: the lane the rooms open onto (single: along one long side; double: down the middle). */
const AISLE = 1;
/** A spur is a gap of this width between two rooms of a row, from the aisle to the plate's perimeter. */
const GAP = 1;
/** The most desks a room seats: the minimum table's entries run for 1 to 6 agents (assets/rooms/minsize.json). */
const DESKS_MAX = 6;
/** A part whose top is more than this above the plate top blocks an aisle strip or a spur gap. */
const TALL = 0.25;
/** A landing is searched within this many cells (BFS steps) of an aisle end or a spur's perimeter point. */
const LANDING_REACH = 40;
/** A gangway bridges at most this many units of blocked cells from the plate edge to its lift cell. Deviates from the 2.5 limit (3.25) pending the coordinator's decision. */
export const GANGWAY_REACH = 3.25;
/** How far a perimeter sample sits outside the plate (half a nav cell: the cell beside the edge). */
const EDGE = NAV_CELL / 2;
/** The spacing of spur columns and perimeter samples along a side. */
const SAMPLE = 0.5;
/** The eight neighbour steps of the landing search. */
const NEIGHBOURS: [number, number][] = [
  [1, 0],
  [-1, 0],
  [0, 1],
  [0, -1],
  [1, 1],
  [-1, 1],
  [1, -1],
  [-1, -1],
];

/** The reasons a slot is unusable. */
const REASON_ENCLOSED = 'enclosed: no walk path leaves the plate on any side';
const REASON_PERIMETER = `no perimeter point within ${LANDING_REACH} cells of the main component`;
const REASON_TALL = 'a part stands over the aisle strip of every configuration';
const REASON_ROOM = 'a room cannot reach its door through this plate with its furniture in place';

function roleHue(role: string): number {
  let hash = 0;
  for (const ch of role) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  return HUES[hash % HUES.length]!;
}

interface Cell {
  cx: number;
  cz: number;
  w: number;
  d: number;
  /** The world direction from the room to its aisle: the room's door faces it. */
  face: [number, number];
}

/** The parts hidden by their own or an ancestor's `when`, as buildModel draws them (an unparseable one hides). */
function hiddenIds(parts: Part[], params: Params): Set<string> {
  const hidden = new Set<string>();
  const walk = (ps: Part[], dark: boolean): void => {
    for (const p of ps) {
      let off = dark;
      if (p.when !== undefined) {
        const w = parseWhen(p.when);
        if (!w || !evalWhen(w, params)) off = true;
      }
      if (p.id !== undefined && off) hidden.add(p.id);
      if (p.children) walk(p.children, off);
    }
  };
  walk(parts, false);
  return hidden;
}

/** The distance from a point to a slot's rect [x, z, w, d] (0 inside it). */
function rectDistance(rect: Slot['rect'], x: number, z: number): number {
  const [x0, z0, w, d] = rect;
  const dx = Math.max(x0 - x, 0, x - (x0 + w));
  const dz = Math.max(z0 - z, 0, z - (z0 + d));
  return Math.hypot(dx, dz);
}

/**
 * The smallest square side a room of `role` needs with `agents` agents: the table's entry at index
 * agents-1 (above 6 agents, the 6-agent entry). 0 for a role the table does not list; null when no size fits.
 */
function minimumSide(role: string, agents: number, table: MinSizeTable): number | null {
  const row = table[role];
  if (!row || row.length === 0) return 0;
  return row[Math.min(Math.max(agents, 1), row.length) - 1] ?? null;
}

/**
 * The most desks a room of `role` seats on `cell`: desk counts 1, 2, ... in order, while the cell's width reaches the table's
 * minimum for that count. The table's entries bound the count (no clamping: a role's row ends at its last entry).
 */
function deskCapOf(role: string, cell: Cell, table: MinSizeTable): number {
  const inner = innerOf(Math.min(cell.w, cell.d));
  const row = table[role];
  const cap = row ? Math.min(DESKS_MAX, row.length) : DESKS_MAX;
  let n = 0;
  for (let k = 1; k <= cap; k++) {
    const m = minimumSide(role, k, table);
    if (m === null || m > inner + 1e-9) break;
    n = k;
  }
  return n;
}

/** Samples per side of a slot's grid (3 by 3), the height above a slot's top where they sit, and the blocked share that hides it. */
export const OCCLUSION_GRID = 3;
export const OCCLUSION_HEIGHT = 1;
/**
 * A slot is occluded when MORE than this share of its samples is blocked: more than a third (4 of the 9 samples).
 * Measured on the workstation: the RAM slot is 1/3 blocked (its near row sits behind the GPU card) and stays usable;
 * m2_2 is 4/9 blocked and is hidden. A 25% share would hide the RAM too (see the note in hardware-plazas.md).
 */
export const OCCLUSION_SHARE = 1 / 3;

/**
 * Is the segment from `from` to `to` blocked by the axis-aligned box (lo, hi) strictly between its ends?
 * Slab test; a segment that only touches a face does not count.
 */
export function segmentBlocked(from: [number, number, number], to: [number, number, number], lo: [number, number, number], hi: [number, number, number]): boolean {
  let t0 = 0;
  let t1 = 1;
  for (let k = 0; k < 3; k++) {
    const o = from[k]!;
    const d = to[k]! - o;
    if (Math.abs(d) < 1e-12) {
      if (o < lo[k]! || o > hi[k]!) return false;
      continue;
    }
    let a = (lo[k]! - o) / d;
    let b = (hi[k]! - o) / d;
    if (a > b) [a, b] = [b, a];
    if (a > t0) t0 = a;
    if (b < t1) t1 = b;
    if (t0 > t1) return false;
  }
  return t1 - t0 > 1e-9 && t0 < 1 - 1e-9 && t1 > 1e-9;
}

/** The part a box belongs to: its id, or its path under the id (a part of a used model: "gpu_1/pcb" or "gpu_1.pcb"). */
function boxOfPart(box: PartBox, part: string): boolean {
  return box.id === part || box.path === part || box.path.startsWith(part + '/') || box.path.startsWith(part + '.');
}

/**
 * The fraction of a slot's samples whose segment from the camera is blocked by a part other than the slot's own.
 * Samples: a grid over the slot's rect on its top, OCCLUSION_HEIGHT above the top. Pure: the boxes are an input.
 */
export function slotBlockedShare(slot: { on: string; top: number; rect: [number, number, number, number] }, camera: [number, number, number], boxes: PartBox[]): number {
  const [x, z, w, d] = slot.rect;
  const n = OCCLUSION_GRID;
  const others = boxes.filter((b) => !boxOfPart(b, slot.on));
  let blocked = 0;
  for (let i = 0; i < n; i++) {
    for (let j = 0; j < n; j++) {
      const to: [number, number, number] = [x + (w * (i + 0.5)) / n, slot.top + OCCLUSION_HEIGHT, z + (d * (j + 0.5)) / n];
      if (others.some((b) => segmentBlocked(camera, to, b.lo, b.hi))) blocked++;
    }
  }
  return blocked / (n * n);
}

/**
 * The ids of the slots a board's default camera cannot see: more than OCCLUSION_SHARE of a slot's samples are blocked.
 * A model without a camera anchor occludes nothing. `boxes` are the board's part boxes (zfight.ts partBoxes).
 */
export function occludedSlotIds(board: Model, boxes: PartBox[]): Set<string> {
  const out = new Set<string>();
  const at = board.anchors?.camera?.at;
  if (!at) return out;
  const camera: [number, number, number] = [at[0]!, at[1]!, at[2]!];
  for (const slot of board.slots ?? []) {
    if (slotBlockedShare(slot, camera, boxes) > OCCLUSION_SHARE) out.add(slot.id);
  }
  return out;
}

/**
 * A room's square side is this share of its cell's (the ramp band sits round the cell): the room's radius is set from it.
 * The width minimums (assets/rooms/minsize.json) are compared with the cell's width, not the room's: placement and joins
 * use the same measure as the board-join check, so a room is never placed or joined below its minimum.
 */
const ROOM_SHARE = 0.9;
/** The wall margin: a room's inner width is its cell's share less this (roomPlan: 2r - 0.15). */
const ROOM_MARGIN = 0.15;
/** A room's inner width from its cell's width: the width the table's minimum is checked against. */
const innerOf = (cellW: number): number => ROOM_SHARE * cellW - ROOM_MARGIN;
/** The cell width a room needs for an inner width of at least `m` (the inverse of innerOf); 0 when there is no minimum. */
const cellNeedOf = (m: number): number => (m > 0 ? (m + ROOM_MARGIN) / ROOM_SHARE : 0);

/**
 * The nav board of a board model: its part boxes (partBoxes with the same params navScene uses), its traces, the PCB top
 * and the given plates. Layout and nav build their board-only grid from it, so the landings are checked on the nav's own walk.
 */
export function navBoardOf(board: Model, params: Params, pcbTop: number, plates: NavPlate[], scale = 1): NavBoard {
  return {
    size: [board.size[0] * scale, board.size[1] * scale, board.size[2] * scale],
    pcbTop,
    boxes: scaleBoxes(partBoxes(board, params), scale).map((b) => ({ lo: b.lo, hi: b.hi })),
    traces: (board.traces ?? []).map((tr) => ({ path: tr.path.map(([x, z]) => [x * scale, z * scale] as [number, number]), width: tr.width * scale })),
    plates,
  };
}

// ---- The board's world scale ------------------------------------------------------------------------

/** The world scale steps and cap: a board grows by BOARD_SCALE_STEP until its plates seat every agent (BOARD_SCALE_CAP at most). */
export const BOARD_SCALE_STEP = 1.1;
export const BOARD_SCALE_CAP = 3;
/** The most landing fallbacks a scale tries (each excludes the rejected landings and plans again) before it grows. */
const LANDING_ROUNDS = 64;

/**
 * The board model at a world scale: its size, slot rects and tops, traces, and anchors scaled about the origin. Part
 * geometry is left as it is (the floor scales the board's group; scaleBoxes gives the part boxes the nav and layout use).
 */
export function scaleModel(model: Model, s: number): Model {
  if (s === 1) return model;
  const v = (p: Vec3): Vec3 => [p[0] * s, p[1] * s, p[2] * s];
  const a = model.anchors;
  return {
    ...model,
    size: v(model.size),
    slots: (model.slots ?? []).map((sl) => ({ ...sl, top: sl.top * s, rect: sl.rect.map((x) => x * s) as Slot['rect'] })),
    traces: (model.traces ?? []).map((tr) => ({ ...tr, path: tr.path.map(([x, z]) => [x * s, z * s] as [number, number]), y: tr.y * s, width: tr.width * s })),
    anchors: a
      ? {
          ...a,
          label: a.label ? v(a.label) : undefined,
          port: a.port ? v(a.port) : undefined,
          atom: a.atom ? v(a.atom) : undefined,
          camera: a.camera ? { at: v(a.camera.at), look: v(a.camera.look) } : undefined,
        }
      : undefined,
  };
}

/** The part boxes at a world scale (a uniform scale about the origin scales each box's corners). */
export function scaleBoxes(boxes: PartBox[], s: number): PartBox[] {
  if (s === 1) return boxes;
  return boxes.map((b) => ({ ...b, lo: [b.lo[0] * s, b.lo[1] * s, b.lo[2] * s], hi: [b.hi[0] * s, b.hi[1] * s, b.hi[2] * s] }));
}

// ---- Plate frames -------------------------------------------------------------------------------------

/**
 * A plate in its own frame: u runs along its long axis (length L), v across it (width S). A plate along x has
 * world (x0 + u, z0 + v); along z, world (x0 + v, z0 + u).
 */
interface Frame {
  x0: number;
  z0: number;
  alongX: boolean;
  L: number;
  S: number;
}

function frameOf(rect: Slot['rect']): Frame {
  const [x0, z0, W, D] = rect;
  const alongX = W >= D;
  return { x0, z0, alongX, L: alongX ? W : D, S: alongX ? D : W };
}

/** The world rect [x, z, w, d] of the frame box [u0, u1] x [v0, v1]. */
function worldRect(f: Frame, u0: number, u1: number, v0: number, v1: number): Slot['rect'] {
  return f.alongX ? [f.x0 + u0, f.z0 + v0, u1 - u0, v1 - v0] : [f.x0 + v0, f.z0 + u0, v1 - v0, u1 - u0];
}

/** The world (x, z) of a frame point. */
function worldPoint(f: Frame, u: number, v: number): [number, number] {
  return f.alongX ? [f.x0 + u, f.z0 + v] : [f.x0 + v, f.z0 + u];
}

/** The world (x, z) of a frame direction. */
function worldVec(f: Frame, du: number, dv: number): [number, number] {
  return f.alongX ? [du, dv] : [dv, du];
}

/** The share of walk cells in a strip of the board-only grid (sampled at the grid's cell centres). */
function openShare(grid: NavGrid, strip: Slot['rect']): number {
  const [x, z, w, d] = strip;
  const nx = Math.max(1, Math.round(w / NAV_CELL));
  const nz = Math.max(1, Math.round(d / NAV_CELL));
  let open = 0;
  for (let i = 0; i < nx; i++) {
    for (let j = 0; j < nz; j++) {
      const c = cellAt(grid, x + ((i + 0.5) * w) / nx, z + ((j + 0.5) * d) / nz);
      if (c >= 0 && grid.walk[c] === 1) open++;
    }
  }
  return open / (nx * nz);
}

/** Does a part stand over the strip higher than the plate top plus TALL? (a box overlapping the strip's footprint) */
function tallPartOn(boxes: PartBox[], strip: Slot['rect'], top: number): boolean {
  const [x, z, w, d] = strip;
  return boxes.some((b) => b.hi[1] > top + TALL && b.lo[0] < x + w && b.hi[0] > x && b.lo[2] < z + d && b.hi[2] > z);
}

/** The component id of the board-only walk grid's largest component (ties: the lowest id); -1 when it has no walk cell. */
function mainComponent(grid: NavGrid): number {
  const comp = grid.component;
  if (!comp) return -1;
  const sizes = new Map<number, number>();
  for (const c of comp) if (c >= 0) sizes.set(c, (sizes.get(c) ?? 0) + 1);
  let best = -1;
  let bestSize = 0;
  for (const [id, size] of sizes) {
    if (size > bestSize || (size === bestSize && id < best)) {
      best = id;
      bestSize = size;
    }
  }
  return best;
}

/** A board cell the landing search may walk: not blocked and not inflated (rule 6 of nav-grid.ts). */
const open = (grid: NavGrid, c: number): boolean => grid.blocked[c] !== 1 && grid.inflated[c] !== 1;

/**
 * The multi-source search for the landing: from the sources (open cells just outside the plate, each with a label), over the
 * board-only grid's open cells off the plate, within LANDING_REACH steps, to the first walk cell of the main component. Returns
 * the first hit (the nearest source's label), or null. "Open" is the grid's own walk test before the component cut (rule 6 of
 * nav-grid.ts: not blocked and not inflated), so the search never plans an exit the grid cannot walk.
 */
function reachMain(grid: NavGrid, rect: Slot['rect'], sources: { cell: number; label: number }[], main: number): { label: number; cell: number } | null {
  const comp = grid.component;
  if (!comp || main < 0) return null;
  const { cols, rows, cell } = grid;
  const [px0, pz0, W, D] = rect;
  const dist = new Int32Array(cols * rows).fill(-1);
  const label = new Int32Array(cols * rows).fill(-1);
  const queue: number[] = [];
  for (const s of sources) {
    if (s.cell < 0 || !open(grid, s.cell) || dist[s.cell]! >= 0) continue;
    dist[s.cell] = 0;
    label[s.cell] = s.label;
    queue.push(s.cell);
  }
  for (let head = 0; head < queue.length; head++) {
    const u = queue[head]!;
    if (comp[u] === main) return { label: label[u]!, cell: u };
    if (dist[u]! >= LANDING_REACH) continue;
    const i = u % cols;
    const j = (u - i) / cols;
    for (const [di, dj] of NEIGHBOURS) {
      const ni = i + di;
      const nj = j + dj;
      if (ni < 0 || nj < 0 || ni >= cols || nj >= rows) continue;
      const v = nj * cols + ni;
      if (dist[v]! >= 0 || !open(grid, v)) continue;
      if (di !== 0 && dj !== 0 && (!open(grid, j * cols + ni) || !open(grid, nj * cols + i))) continue;
      const x = grid.x0 + (ni + 0.5) * cell;
      const z = grid.z0 + (nj + 0.5) * cell;
      if (x >= px0 && x < px0 + W && z >= pz0 && z < pz0 + D) continue; // the plate itself is not a way off
      dist[v] = dist[u]! + 1;
      label[v] = label[u]!;
      queue.push(v);
    }
  }
  return null;
}

/** A plate's landing (rule C): the plate cell at the exit (ax, az), the board cell reached (bx, bz), the outward normal (nx, nz). */
type Landing = NonNullable<NavPlate['landing']>;

function landingOf(grid: NavGrid, plate: [number, number], normal: [number, number], cell: number, top: number, pcbTop: number): Landing {
  const i = cell % grid.cols;
  const j = (cell - i) / grid.cols;
  return {
    ax: plate[0],
    az: plate[1],
    bx: grid.x0 + (i + 0.5) * grid.cell,
    bz: grid.z0 + (j + 0.5) * grid.cell,
    kind: top - pcbTop <= NAV_LIFT_RISE ? 'ramp' : 'lift',
    nx: normal[0],
    nz: normal[1],
  };
}

// ---- Aisles -------------------------------------------------------------------------------------------

/** A room row: its frame span [v0, v1], the direction (du, dv) from the row to its aisle, and an optional spur gap [u0, u1]. */
interface Row {
  v0: number;
  v1: number;
  face: [number, number];
  gap?: [number, number];
}

/** A usable slot's plan: its rows of rooms, its aisle (double or single), whether it leaves by a spur, and its landing. */
interface Aisle {
  slot: Slot;
  f: Frame;
  rows: Row[];
  double: boolean;
  spur: boolean;
  /** The exit is a gangway over blocked cells (used only where no plain perimeter landing exists). */
  gangway: boolean;
  landing: Landing;
  /** The candidate's key (slot, configuration, exit): a rejected key is excluded from the next plan (landingClear). */
  key: string;
}

/** The candidate exits a layout pass must not use again: the keys of the landings the room check rejected. */
type Excluded = Set<string>;

/** A configuration's key: its orientation and aisle start (the candidate keys are per slot and configuration). */
const configKey = (slot: Slot, cfg: Config): string => `${slot.id}|${cfg.double ? 'D' : 'S'}${cfg.aisle[0].toFixed(3)}`;

/**
 * The parts of a row that hold rooms: the row's length less its spur gap (one or two spans). A room keeps NAV_RAMP_DEPTH off
 * a spur gap and off the aisle end the exit leaves by, so no room slab sits in a cliff band beside the landing (rule 7a): the
 * band of a slab reaches NAV_RAMP_DEPTH past it, and the landing cell is just beyond the edge.
 */
function rowSpans(row: Row, a: Aisle): [number, number][] {
  const L = a.f.L;
  const un = worldVec(a.f, 1, 0);
  const side = a.spur ? 0 : a.landing.nx * un[0] + a.landing.nz * un[1];
  const lo = side < -0.5 ? NAV_RAMP_DEPTH : 0;
  const hi = side > 0.5 ? L - NAV_RAMP_DEPTH : L;
  const cut: [number, number][] = row.gap ? [[0, row.gap[0] - NAV_RAMP_DEPTH], [row.gap[1] + NAV_RAMP_DEPTH, L]] : [[0, L]];
  return cut.map(([u0, u1]): [number, number] => [Math.max(u0, lo), Math.min(u1, hi)]).filter(([u0, u1]) => u1 - u0 > 1e-9);
}

/**
 * The room spans of an aisle at a minimum side `need`: per span, how many cells of at least `need` it holds. A row
 * shallower than `need` holds none. The count is capped by the slot's capacity in roomCells / holds.
 */
function spansAt(a: Aisle, need: number): { row: Row; u0: number; u1: number; k: number }[] {
  const cap = Math.floor(a.slot.capacity);
  const out: { row: Row; u0: number; u1: number; k: number }[] = [];
  for (const row of a.rows) {
    if (row.v1 - row.v0 < need - 1e-9) continue;
    for (const [u0, u1] of rowSpans(row, a)) {
      const k = need <= 0 ? cap : Math.floor((u1 - u0) / need + 1e-9);
      if (k > 0) out.push({ row, u0, u1, k });
    }
  }
  return out;
}

/** The rooms an aisle holds with cells at least `need` on their short side: its capacity, lowered as its spans allow. */
function holds(a: Aisle, need: number): number {
  const cap = Math.floor(a.slot.capacity);
  if (cap < 1) return 0;
  return Math.min(cap, spansAt(a, need).reduce((s, g) => s + g.k, 0));
}

/** The cells of an aisle at `need`: `holds` of them, each span split into equal cells across the row's depth. */
function roomCells(a: Aisle, need: number): Cell[] {
  let left = holds(a, need);
  const cells: Cell[] = [];
  for (const g of spansAt(a, need)) {
    const n = Math.min(g.k, left);
    if (n <= 0) continue;
    left -= n;
    const len = (g.u1 - g.u0) / n;
    for (let j = 0; j < n; j++) {
      const [x, z, w, d] = worldRect(a.f, g.u0 + j * len, g.u0 + (j + 1) * len, g.row.v0, g.row.v1);
      cells.push({ cx: x + w / 2, cz: z + d / 2, w, d, face: worldVec(a.f, g.row.face[0], g.row.face[1]) });
    }
  }
  return cells;
}

/**
 * The source cells outside a plate edge: every unblocked cell along the rays from the edge points in `dir` (a unit world
 * direction), up to one unit beyond the edge, each labelled. A cell straddling the edge may be blocked by the plate's own
 * part, and a single ray can end in a pocket walled off from the board, so all unblocked cells in the band are sources.
 */
function outwardSources(grid: NavGrid, edge: [number, number][], dir: [number, number], label: number): { cell: number; label: number }[] {
  const out: { cell: number; label: number }[] = [];
  for (const [x, z] of edge) {
    // The plate cell just inside the edge is the landing's plate end (rule 6 walks from an open plate cell): it must be open.
    const inside = cellAt(grid, x - dir[0] * EDGE, z - dir[1] * EDGE);
    if (inside < 0 || !open(grid, inside)) continue;
    for (let k = 1; k <= 8; k++) {
      const c = cellAt(grid, x + dir[0] * k * EDGE, z + dir[1] * k * EDGE);
      if (c >= 0 && grid.blocked[c] !== 1) out.push({ cell: c, label });
    }
  }
  return out;
}

/** Does any perimeter edge open onto unblocked ground? (false: the plate is enclosed on every side) */
function perimeterOpen(grid: NavGrid, f: Frame): boolean {
  for (let u = 0; u <= f.L + 1e-9; u += SAMPLE) {
    if (outwardSources(grid, [worldPoint(f, u, 0)], worldVec(f, 0, -1), 0).length > 0) return true;
    if (outwardSources(grid, [worldPoint(f, u, f.S)], worldVec(f, 0, 1), 0).length > 0) return true;
  }
  for (let v = 0; v <= f.S + 1e-9; v += SAMPLE) {
    if (outwardSources(grid, [worldPoint(f, 0, v)], worldVec(f, -1, 0), 0).length > 0) return true;
    if (outwardSources(grid, [worldPoint(f, f.L, v)], worldVec(f, 1, 0), 0).length > 0) return true;
  }
  return false;
}

/** An aisle configuration before its exit is chosen: the aisle's frame span, its rows and whether it is double. */
interface Config {
  double: boolean;
  aisle: [number, number];
  rows: Row[];
  open: number;
}

/**
 * The exit of a configuration (rules e and 2): the nearer aisle end (both ends searched together, the first hit wins);
 * else a spur, a 1.0 gap through a row from an aisle cell to the row's outer perimeter, searched for the same way. Null
 * when neither reaches the main component within LANDING_REACH cells.
 */
function exitOf(slot: Slot, cfg: Config, grid: NavGrid, boxes: PartBox[], main: number, pcbTop: number, excluded: Excluded): Aisle | null {
  const f = frameOf(slot.rect);
  const ck = configKey(slot, cfg);
  const vm = (cfg.aisle[0] + cfg.aisle[1]) / 2;
  // Both ends at once: the sources are the band beyond each end across the aisle's width; the first hit names the end.
  const across = (u: number, v0: number, v1: number): [number, number][] => {
    const pts: [number, number][] = [];
    for (let v = v0 + EDGE; v <= v1 - EDGE + 1e-9; v += 2 * EDGE) pts.push(worldPoint(f, u, v));
    return pts;
  };
  const ends = [-1, 1].map((nu) => ({ nu, normal: worldVec(f, nu, 0), plate: worldPoint(f, (nu < 0 ? 0 : f.L) - nu * EDGE, vm) }));
  const endSources = ends.flatMap((e, k) =>
    excluded.has(`${ck}|end${k}`) ? [] : outwardSources(grid, across(e.nu < 0 ? 0 : f.L, cfg.aisle[0], cfg.aisle[1]), e.normal, k),
  );
  const endHit = reachMain(grid, slot.rect, endSources, main);
  if (endHit) {
    const e = ends[endHit.label]!;
    return { slot, f, rows: cfg.rows, double: cfg.double, spur: false, gangway: false, landing: landingOf(grid, e.plate, e.normal, endHit.cell, slot.top, pcbTop), key: `${ck}|end${endHit.label}` };
  }

  // Spur candidates: a 1.0 gap in each row, centred on each sample column, clear of tall parts; its perimeter point is the
  // row's outer edge (the edge away from the aisle), and its source is the cell just outside that edge.
  const spurs: { r: number; gap: [number, number]; plate: [number, number]; normal: [number, number]; key: string }[] = [];
  const spurSources: { cell: number; label: number }[] = [];
  cfg.rows.forEach((row, r) => {
    const outerV = row.face[1] > 0 ? row.v0 : row.v1;
    const nv = -Math.sign(row.face[1]);
    const normal = worldVec(f, 0, nv);
    for (let c = GAP / 2; c <= f.L - GAP / 2 + 1e-9; c += SAMPLE) {
      const gap: [number, number] = [c - GAP / 2, c + GAP / 2];
      if (tallPartOn(boxes, worldRect(f, gap[0], gap[1], row.v0, row.v1), slot.top)) continue;
      // A spur never leaves a room span narrower than one aisle width (a sliver cannot hold a room): an end span is 0 or >= AISLE.
      const spanOk = (w: number): boolean => w < 1e-9 || w >= AISLE - 1e-9;
      if (!spanOk(gap[0]) || !spanOk(f.L - gap[1])) continue;
      const k = spurs.length;
      const key = `${ck}|spur${r}:${gap[0].toFixed(3)}`;
      spurs.push({ r, gap, plate: worldPoint(f, c, outerV - nv * EDGE), normal, key });
      if (excluded.has(key)) continue;
      const edge: [number, number][] = [];
      for (let u = gap[0] + EDGE; u <= gap[1] - EDGE + 1e-9; u += 2 * EDGE) edge.push(worldPoint(f, u, outerV));
      for (const s of outwardSources(grid, edge, normal, k)) spurSources.push(s);
    }
  });
  const spurHit = reachMain(grid, slot.rect, spurSources, main);
  if (!spurHit) return null;
  const s = spurs[spurHit.label]!;
  const rows = cfg.rows.map((row, r) => (r === s.r ? { ...row, gap: s.gap } : row));
  return { slot, f, rows, double: cfg.double, spur: true, gangway: false, landing: landingOf(grid, s.plate, s.normal, spurHit.cell, slot.top, pcbTop), key: s.key };
}

/** The centre of a grid cell in world (x, z). */
function cellCentre(grid: NavGrid, c: number): [number, number] {
  const i = c % grid.cols;
  const j = (c - i) / grid.cols;
  return [grid.x0 + (i + 0.5) * grid.cell, grid.z0 + (j + 0.5) * grid.cell];
}

/**
 * A straight gangway from a plate edge point out along `dir`: the cells it passes must be blocked (it bridges over a
 * part at plate height), up to GANGWAY_REACH units past the edge. The first unblocked cell is the lift's cell b: it must
 * be a walk cell of the main component, and the cells one and two more out (the queue) too. Null when the ray meets no such cell.
 */
function gangwayFrom(grid: NavGrid, edge: [number, number], dir: [number, number], main: number): { bridge: [number, number][]; b: number; length: number } | null {
  const comp = grid.component;
  if (!comp || main < 0) return null;
  // The plate's own edge cell is over the part under the plate, so it is blocked on the board-only grid: the bridge starts there anyway.
  const inside = cellAt(grid, edge[0] - dir[0] * EDGE, edge[1] - dir[1] * EDGE);
  if (inside < 0) return null;
  const bridgeCells: number[] = [inside];
  const steps = Math.floor((GANGWAY_REACH + EDGE) / NAV_CELL);
  for (let k = 1; k <= steps; k++) {
    const t = k * NAV_CELL - EDGE;
    const c = cellAt(grid, edge[0] + dir[0] * t, edge[1] + dir[1] * t);
    if (c < 0) return null;
    if (grid.walk[c] !== 1) {
      // Blocked, or inflated (clear of solids but beside one): the bridge passes over either; the lift stands on a walk cell.
      if (bridgeCells[bridgeCells.length - 1] !== c) bridgeCells.push(c);
      continue;
    }
    if (comp[c] !== main) return null;
    // The lift's queue is two cells out (the cab's deck covers the first cell beyond its cell): that one must be walk too.
    const mid = cellAt(grid, edge[0] + dir[0] * (t + NAV_CELL), edge[1] + dir[1] * (t + NAV_CELL));
    if (mid < 0 || grid.walk[mid] !== 1 || comp[mid] !== main) return null;
    const q = cellAt(grid, edge[0] + dir[0] * (t + 2 * NAV_CELL), edge[1] + dir[1] * (t + 2 * NAV_CELL));
    if (q < 0 || grid.walk[q] !== 1 || comp[q] !== main) return null;
    return { bridge: bridgeCells.map((x) => cellCentre(grid, x)), b: c, length: t };
  }
  return null;
}

/**
 * A slot's gangway landing for one configuration (the last resort, after every plain landing failed): from each cell of the
 * aisle's perimeter (an aisle end, or a single aisle's outer long side) a straight gangway over blocked cells to the
 * nearest walkable main-component cell within GANGWAY_REACH. The shortest bridge wins. Null when none exists.
 */
function gangwayOf(slot: Slot, cfg: Config, grid: NavGrid, main: number, excluded: Excluded): Aisle | null {
  const f = frameOf(slot.rect);
  const { L, S } = f;
  const ck = configKey(slot, cfg);
  const cellsAcross = (u: number, v0: number, v1: number): [number, number][] => {
    const pts: [number, number][] = [];
    for (let v = v0 + NAV_CELL / 2; v < v1 - 1e-9; v += NAV_CELL) pts.push(worldPoint(f, u, v));
    return pts;
  };
  const cands: { edge: [number, number]; dir: [number, number]; key: string }[] = [];
  for (const nu of [-1, 1]) {
    const dir = worldVec(f, nu, 0);
    for (const e of cellsAcross(nu < 0 ? 0 : L, cfg.aisle[0], cfg.aisle[1])) cands.push({ edge: e, dir, key: `${ck}|gang${nu}:${e[0].toFixed(3)}:${e[1].toFixed(3)}` });
  }
  if (!cfg.double) {
    // A single aisle runs along one outer long side of the plate: its cells there are perimeter cells too.
    const sv = cfg.aisle[0] <= 1e-9 ? -1 : 1;
    const dir = worldVec(f, 0, sv);
    for (let u = NAV_CELL / 2; u < L - 1e-9; u += NAV_CELL) {
      const edge = worldPoint(f, u, sv < 0 ? 0 : S);
      cands.push({ edge, dir, key: `${ck}|gangs${sv}:${edge[0].toFixed(3)}:${edge[1].toFixed(3)}` });
    }
  }
  let best: { bridge: [number, number][]; b: number; length: number; dir: [number, number]; key: string } | null = null;
  for (const c of cands) {
    if (excluded.has(c.key)) continue;
    const g = gangwayFrom(grid, c.edge, c.dir, main);
    if (g && (!best || g.length < best.length - 1e-9)) best = { ...g, dir: c.dir, key: c.key };
  }
  if (!best) return null;
  const b = cellCentre(grid, best.b);
  const landing: Landing = {
    ax: best.bridge[0]![0],
    az: best.bridge[0]![1],
    bx: b[0],
    bz: b[1],
    kind: 'gangway',
    nx: best.dir[0],
    nz: best.dir[1],
    bridge: best.bridge,
  };
  return { slot, f, rows: cfg.rows, double: cfg.double, spur: false, gangway: true, landing, key: best.key };
}

/**
 * A slot's aisle plan (rules a-e): the double aisle down the middle when the plate is deep enough for two rows of the
 * smallest room (rowMin), else the single rows, the long side with the most open ground first (ties: +v, the camera side
 * for a plate along x). A configuration needs its aisle clear of tall parts and an exit. A slot is unusable with the reason
 * when no configuration works: enclosed (no perimeter sample has an unblocked outside cell), no exit, or every aisle blocked.
 */
function planAisle(slot: Slot, grid: NavGrid, boxes: PartBox[], main: number, pcbTop: number, rowMin: number, excluded: Excluded): Aisle | string {
  if (excluded.has(`slot|${slot.id}`)) return REASON_ROOM;
  const f = frameOf(slot.rect);
  const { L, S } = f;
  const half = (S - AISLE) / 2;
  const configs: Config[] = [];
  if (half > 0 && half >= rowMin - 1e-9) {
    configs.push({
      double: true,
      aisle: [half, half + AISLE],
      rows: [
        { v0: 0, v1: half, face: [0, 1] },
        { v0: half + AISLE, v1: S, face: [0, -1] },
      ],
      open: 0,
    });
  }
  const singles: Config[] = [
    // +v first: on a tie the camera side wins (+z for a plate along x; +x for one along z).
    {
      double: false,
      aisle: [S - AISLE, S],
      rows: [{ v0: 0, v1: S - AISLE, face: [0, 1] }],
      open: openShare(grid, worldRect(f, 0, L, S, S + AISLE)),
    },
    {
      double: false,
      aisle: [0, AISLE],
      rows: [{ v0: AISLE, v1: S, face: [0, -1] }],
      open: openShare(grid, worldRect(f, 0, L, -AISLE, 0)),
    },
  ];
  singles.sort((a, b) => (Math.abs(b.open - a.open) < 1e-9 ? 0 : b.open - a.open));
  configs.push(...singles);

  // The plain perimeter landings (an aisle end, or a spur) of every configuration come first; a gangway only where none exists.
  const clearConfigs = configs.filter((cfg) => !tallPartOn(boxes, worldRect(f, 0, L, cfg.aisle[0], cfg.aisle[1]), slot.top));
  for (const cfg of clearConfigs) {
    const found = exitOf(slot, cfg, grid, boxes, main, pcbTop, excluded);
    if (found) return found;
  }
  for (const cfg of clearConfigs) {
    const found = gangwayOf(slot, cfg, grid, main, excluded);
    if (found) return found;
  }
  if (clearConfigs.length === 0) return REASON_TALL;
  return perimeterOpen(grid, f) ? REASON_PERIMETER : REASON_ENCLOSED;
}

/** The rooms as the nav grid reads them (rule 7a): each district's slab, the square of half-side min(w, d) / 2 (the dais: a circle). */
function slabsOf(districts: BoardDistrict[]): Slab[] {
  return districts.map((d) => ({ cx: d.cx, cz: d.cy, half: Math.min(d.w, d.d) / 2, round: d.shape === 'round' }));
}

/**
 * Is an aisle's landing usable with the rooms in place (rule 6 of nav-grid.ts, the same test): the board cell it reaches (and a
 * lift's queue) is outside every room slab and cliff band, the plate cell is outside every slab, and a gangway's bridge is
 * outside every wall (a bridge may cross a band, not a wall). A landing that fails is excluded and the layout is planned again.
 */
function landingClear(a: Aisle, slabs: Slab[], plates: Slot['rect'][]): boolean {
  const onPlate = (x: number, z: number): boolean => plates.some(([px, pz, w, d]) => x >= px && x < px + w && z >= pz && z < pz + d);
  const L = a.landing;
  if (!landingClearAt(slabs, L.bx, L.bz, onPlate(L.bx, L.bz))) return false;
  if (L.kind === 'gangway') return (L.bridge ?? []).every(([x, z]) => landingClearAt(slabs, x, z, true));
  if (!landingClearAt(slabs, L.ax, L.az, true)) return false;
  if (L.kind === 'lift') {
    const qx = L.bx + L.nx * NAV_CELL;
    const qz = L.bz + L.nz * NAV_CELL;
    if (!landingClearAt(slabs, qx, qz, onPlate(qx, qz))) return false;
  }
  return true;
}

/**
 * The room depth a double aisle's rows must reach: the median of the present roles' minimums at one agent (the typical
 * room), with ROOM_SHARE. A plate takes the double aisle only when each of its two rows is this deep.
 */
function typicalRowDepth(list: Agent[], table: MinSizeTable): number {
  const sides: number[] = [];
  for (const a of list) {
    const v = minimumSide(a.role ? a.role : 'other', 1, table);
    if (v !== null && v > 0) sides.push(v);
  }
  if (sides.length === 0) return 0;
  sides.sort((x, y) => x - y);
  let m = sides[Math.floor((sides.length - 1) / 2)]!;
  // The orchestrator's room is bigger than the typical one and sits nearest the atom: its rows must hold it too.
  if (list.some((a) => a.tier === 'orchestrator')) {
    const orch = minimumSide('orchestrator', 1, table);
    if (orch !== null && orch > 0) m = Math.max(m, orch);
  }
  return cellNeedOf(m);
}

/**
 * The turn and door of a room on its cell (rule c). A square room turns so its local +z side (door 's') faces its aisle;
 * a round room keeps the facing rule (its turn points at the board's centre) and its door is the side nearest its aisle.
 */
function turnAndDoor(cell: Cell, round: boolean): { turn: number; door: Door } {
  const [ux, uz] = cell.face;
  const turn = round ? Math.atan2(-cell.cx, -cell.cz) : Math.atan2(ux, uz);
  const c = Math.cos(turn);
  const s = Math.sin(turn);
  const lx = ux * c - uz * s;
  const lz = ux * s + uz * c;
  const door: Door = Math.abs(lx) > Math.abs(lz) ? (lx > 0 ? 'e' : 'w') : lz > 0 ? 's' : 'n';
  return { turn, door };
}

interface Seat {
  role: string;
  family: Family;
  members: Agent[];
  centre: boolean;
  /** The slot index, or -1 when no slot holds the room at its minimum: the seat joins a built room. */
  slot: number;
  /** The room's minimum side (0 when no size is known for it). */
  minSide: number;
}

/**
 * Lays the roles out on a board model's slots, aisle first. Each usable slot is a plate with a double aisle down its middle
 * (rooms on both sides, doors to the aisle) when it is deep enough, else a single row along one long side; its aisle leaves
 * the plate by an end or a 1.0 spur to a landing on the main component. The orchestrator takes the cell nearest the atom;
 * the other roles follow sorted by name, each in the first slot of its family's type with room, then the fallbacks, then any
 * slot with room. A slot holds only as many rooms as its cells keep at their role's minimum side (from `table`, default
 * assets/rooms/minsize.json); a room that does not fit a slot moves on in that order. A role with no slot at its minimum
 * joins the nearest built room of its family (or the largest built room if none reaches the minimum); joining adds desks,
 * not cells. Slots that are hidden, occluded or unusable hold nothing.
 */
/**
 * The board's layout at its world scale: the board's slots, tops, anchors and part boxes all scaled by `s`, while the
 * rooms, desks and minimums stay in absolute units. Returns the layout with boardScale set to `s`.
 */
/** One layout pass: its layout, the rejected landing keys (the room check), and each plate's candidate key (by plate index). */
interface Pass {
  layout: BoardLayout;
  rejected: string[];
  plateKeys: string[];
  plateSlots: string[];
}

/** The candidate keys of the plates the nav grid cannot reach with the rooms in place (its own landing search, rule 6). */
function unreachedKeys(board: Model, params: Params, pass: Pass): string[] {
  const grid = navScene(board, params, pass.layout, null).grid;
  const first = pass.layout.districts.length + 1;
  const keys = grid.unreached.map((id) => pass.plateKeys[id - first]).filter((k): k is string => k !== undefined);
  // A room whose door cannot reach the main component (rule 12, connectivity) takes its whole plate out: the furniture
  // enclosed its desks or door, which an exit change does not fix. Its plate's rooms move to another plate, or the board grows.
  for (const tid of connectivity(grid).unconnected) {
    const d = pass.layout.districts[tid - 1];
    if (!d) continue;
    const k = pass.layout.plates.findIndex((p) => d.cx >= p.x0 && d.cx <= p.x0 + p.w && d.cy >= p.z0 && d.cy <= p.z0 + p.d);
    if (k >= 0) keys.push(`slot|${pass.plateSlots[k]}`);
  }
  return keys;
}

/**
 * One layout pass at a scale. The exits are planned on the board-only grid; the rooms are then built, and each plate's
 * landing is checked with the rooms in place (landingClear). The keys of the landings that fail come back in `rejected`:
 * the caller excludes them and plans again (a fallback through the candidates), before it grows the board.
 */
function layoutAtScale(list: Agent[], original: Model, params: Params, table: MinSizeTable, s: number, excluded: Excluded): Pass {
  const board = scaleModel(original, s);
  const p: Params = { ...(board.params ?? {}), ...params };
  const pcbTop = (p.pcb_top ?? 0.4) * s;
  const atom: [number, number, number] = board.anchors?.atom ?? [0, pcbTop, 0];
  const [ax, , az] = atom;

  const hidden = hiddenIds(board.parts, p);
  // A slot is used when its part shows and the board's default camera can see it. An explicit `occluded` on the slot
  // overrides the computed view (leave it unset: the camera decides).
  const boxes = scaleBoxes(partBoxes(original, params), s);
  const occluded = occludedSlotIds(board, boxes);
  const isOccluded = (s: Slot): boolean => (typeof s.occluded === 'boolean' ? s.occluded : occluded.has(s.id));
  const visible = (board.slots ?? []).filter((s) => !hidden.has(s.on) && !isOccluded(s));
  const candidates: Slot[] = visible.length
    ? visible
    : [{ id: '', on: '', type: 'soc', top: pcbTop, rect: [-board.size[0] / 2, -board.size[2] / 2, board.size[0], board.size[2]], capacity: 1 }];

  // The board-only walk grid (parts and traces; no plates, no rooms) gives each slot's exit and its main component.
  // The plates are the candidate slots (rule 6: the board-only walk has the plates, the PCB, the parts and the traces).
  const plates0: NavPlate[] = candidates.map((c, k) => ({ id: k + 1, x0: c.rect[0], z0: c.rect[1], w: c.rect[2], d: c.rect[3], y: c.top }));
  const grid0 = buildGrid(navBoardOf(original, params, pcbTop, plates0, s), []);
  const main = mainComponent(grid0);
  const rowMin = typicalRowDepth(list, table);
  const aisles: Aisle[] = [];
  const unusable: UnusableSlot[] = [];
  for (const slot of candidates) {
    const r = planAisle(slot, grid0, boxes, main, pcbTop, rowMin, excluded);
    if (typeof r === 'string') {
      unusable.push({ slot: slot.id, reason: r });
      console.warn(`board layout: slot ${slot.id || '(board)'} is unusable: ${r}`);
    } else aisles.push(r);
  }

  const slots: Slot[] = aisles.map((a) => a.slot);
  const load = slots.map(() => 0);
  /** Each slot's largest minimum side among its rooms: its cells must reach it. */
  const need = slots.map(() => 0);
  const hasRoom = (i: number, m: number): boolean => load[i]! < holds(aisles[i]!, Math.max(need[i]!, cellNeedOf(m)));

  const nearestTo = (idx: number[]): number =>
    idx.reduce((best, i) => (rectDistance(slots[i]!.rect, ax, az) < rectDistance(slots[best]!.rect, ax, az) ? i : best));

  /** A slot's fill for a room with minimum `m`: its rooms over the rooms it holds at that minimum. */
  const fillOf = (i: number, m: number): number => load[i]! / Math.max(1, holds(aisles[i]!, Math.max(need[i]!, cellNeedOf(m))));

  /**
   * The slot for a room with minimum `m` (-1 when none holds it). The orchestrator takes the open slot nearest the atom. Any
   * other room goes to the open slot with the lowest fill, so the rooms spread over the usable slots in proportion to what
   * each holds (round-robin by remaining capacity) rather than all landing on the biggest plate. The family's preferred
   * slot types only break a tie.
   */
  const place = (family: Family, centre: boolean, m: number): number => {
    const open = slots.map((_, i) => i).filter((i) => hasRoom(i, m));
    if (open.length === 0) return -1;
    if (centre) return nearestTo(open);
    const preferred = (i: number): number => (FAMILY_TYPES[family].includes(slots[i]!.type) ? 0 : 1);
    return open.reduce((best, i) => {
      const fi = fillOf(i, m);
      const fb = fillOf(best, m);
      if (Math.abs(fi - fb) > 1e-9) return fi < fb ? i : best;
      return preferred(i) < preferred(best) ? i : best;
    });
  };

  const seats: Seat[] = [];
  const centre = list.filter((a) => a.tier === 'orchestrator');
  const groups = new Map<string, Agent[]>();
  // An agent with no role gets a room of its own (the 'other' family), not one room for every unnamed agent.
  const roleOfKey = (key: string): string => (key.startsWith('unnamed:') ? 'other' : key);
  for (const a of list) {
    if (a.tier === 'orchestrator') continue;
    const key = a.role ? a.role : `unnamed:${a.cid}`;
    groups.set(key, [...(groups.get(key) ?? []), a]);
  }
  const seatRoom = (role: string, family: Family, members: Agent[], isCentre: boolean): void => {
    let m = minimumSide(role, members.length, table);
    if (m === null) {
      // No size fits: the room is placed by the rules alone and its cell is not checked against a minimum.
      console.warn(`board layout: no room size fits ${role} with ${members.length} agent(s)`);
      m = 0;
    }
    const slot = place(family, isCentre, m);
    if (slot >= 0) {
      load[slot]!++;
      need[slot] = Math.max(need[slot]!, cellNeedOf(m));
    }
    seats.push({ role, family, members, centre: isCentre, slot, minSide: m });
  };
  if (centre.length) seatRoom('orchestrator', roleFamily('orchestrator', 'orchestrator'), centre, true);
  for (const key of [...groups.keys()].sort()) {
    const role = roleOfKey(key);
    seatRoom(role, roleFamily(role), groups.get(key)!, false);
  }

  // Each slot's cells: as many as its aisle holds for its rooms' minimums. The orchestrator takes the cell nearest the atom;
  // the other rooms fill the free cells in order.
  const cellsOf = aisles.map((a, i) => roomCells(a, need[i]!));
  const taken = slots.map(() => new Set<number>());
  interface Built {
    seat: Seat;
    slotIdx: number;
    cell: Cell;
    district: BoardDistrict;
    /** The room's agents: the seat's, and any that joined it. */
    members: Agent[];
    minSide: number;
  }
  const built: Built[] = [];
  for (const seat of seats.filter((st) => st.slot >= 0)) {
    const aisle = aisles[seat.slot]!;
    const slot = aisle.slot;
    const cells = cellsOf[seat.slot]!;
    const free = cells.map((_, i) => i).filter((i) => !taken[seat.slot]!.has(i));
    const pick = seat.centre
      ? free.reduce((best, i) => (Math.hypot(cells[i]!.cx - ax, cells[i]!.cz - az) < Math.hypot(cells[best]!.cx - ax, cells[best]!.cz - az) ? i : best))
      : free[0]!;
    taken[seat.slot]!.add(pick);
    const c = cells[pick]!;
    if (innerOf(Math.min(c.w, c.d)) < seat.minSide - 1e-9) {
      console.warn(`board layout: ${seat.role}'s room is ${innerOf(Math.min(c.w, c.d)).toFixed(2)} wide inside, under its minimum ${seat.minSide}; slot ${slot.id} is full`);
    }
    const { turn, door } = turnAndDoor(c, seat.centre);
    const d: BoardDistrict = {
      role: seat.role,
      cx: c.cx,
      cy: c.cz,
      r: (Math.min(c.w, c.d) / 2) * ROOM_SHARE,
      hue: seat.centre ? DAIS_HUE : roleHue(seat.role),
      shape: seat.centre ? 'round' : roomStyle(seat.role).shape,
      y: slot.top,
      slot: slot.id,
      w: c.w,
      d: c.d,
      turn,
      door,
    };
    built.push({ seat, slotIdx: seat.slot, cell: c, district: d, members: [...seat.members], minSide: seat.minSide });
  }

  // Joins: an agent whose role has no free cell at its minimum joins the nearest built room of its family. That room's
  // minimum is recomputed for its new desk count and it must still reach it. If none fits, the largest built room on
  // the board gains the desk. Joining adds desks, not cells, so no other room's cell changes.
  for (const seat of seats.filter((st) => st.slot < 0)) {
    if (built.length === 0) {
      console.warn(`board layout: no room is built, so ${seat.role}'s agents have no desk`);
      break;
    }
    const pref = slots.find((s) => FAMILY_TYPES[seat.family].includes(s.type)) ?? null;
    const [px, pz] = pref ? [pref.rect[0] + pref.rect[2] / 2, pref.rect[1] + pref.rect[3] / 2] : [ax, az];
    const byDistance = (a: Built, b: Built): number =>
      rectDistance(slots[a.slotIdx]!.rect, px, pz) - rectDistance(slots[b.slotIdx]!.rect, px, pz) || a.seat.role.localeCompare(b.seat.role);
    // A room seats at most as many desks as the minimum table has entries for (the 6-agent entry is the last): beyond that the
    // table holds no minimum, so no room may take the desk silently.
    // A room takes the desk only while its width still reaches the minimum for the new desk count (deskCapOf).
    const fits = (b: Built): boolean => b.members.length + seat.members.length <= deskCapOf(b.seat.role, b.cell, table);
    // The nearest built room of the same family that still reaches its minimum; else the largest built room with a desk to spare.
    let host = built.filter((b) => roleFamily(b.seat.role) === seat.family).sort(byDistance).find(fits);
    if (!host) {
      const spare = built.filter(fits);
      if (spare.length === 0) {
        console.warn(`board layout: no room can seat ${seat.role}'s agents within the minimum table's desks a room; they have no desk`);
        continue;
      }
      host = spare.reduce((best, b) => (Math.min(b.cell.w, b.cell.d) > Math.min(best.cell.w, best.cell.d) ? b : best));
      console.warn(`board layout: no room reaches ${seat.role}'s minimum with one more desk; ${host.seat.role}'s room takes the desk`);
    }
    host.members.push(...seat.members);
    host.minSide = minimumSide(host.seat.role, host.members.length, table) ?? host.minSide;
  }

  // Homes: the agents of a room sit in a square grid, 3 apart, closer when the cell is too small for that.
  const districts: BoardDistrict[] = [];
  const homes = new Map<number, [District, number, number]>();
  for (const b of built) {
    districts.push(b.district);
    const c = b.cell;
    const members = [...b.members].sort((x, y) => x.cid - y.cid);
    const side = Math.max(1, Math.ceil(Math.sqrt(members.length)));
    const step = side > 1 ? Math.min(3, (0.8 * Math.min(c.w, c.d)) / (side - 1)) : 0;
    const span = (step * (side - 1)) / 2;
    members.forEach((a, i) => {
      homes.set(a.cid, [b.district, c.cx - span + (i % side) * step, c.cz - span + Math.floor(i / side) * step]);
    });
  }

  // Plates: the usable slots, in candidate order. Region ids come after the districts' (terrace i is region i + 1).
  const plates: NavPlate[] = aisles.map((a, k) => {
    const [x0, z0, w, d] = a.slot.rect;
    return { id: districts.length + k + 1, x0, z0, w, d, y: a.slot.top, landing: a.landing };
  });
  const aisleReport = aisles.map((a) => ({ slot: a.slot.id, double: a.double, spur: a.spur, gangway: a.gangway }));

  const slabs = slabsOf(districts);
  const rejected = aisles.filter((a) => !landingClear(a, slabs, slots.map((st) => st.rect))).map((a) => a.key);
  const layout: BoardLayout = {
    districts,
    homes,
    atom,
    bounds: { cx: 0, cz: 0, w: board.size[0], d: board.size[2] },
    pcbTop,
    plates,
    unusable,
    aisles: aisleReport,
    boardScale: s,
  };
  return { layout, rejected, plateKeys: aisles.map((a) => a.key), plateSlots: aisles.map((a) => a.slot.id) };
}

/**
 * The layout of a board: the board at the smallest world scale (1.0, then BOARD_SCALE_STEP steps) at which its plates seat
 * every agent with room inner widths at their minimums. Throws when even BOARD_SCALE_CAP does not seat them all.
 */
/**
 * The slots at the height their plates rest on: a slot's top is the top of its own part under its rect plus the plate's
 * thickness, never lower than declared. Each slot's part is read from the built board (its own upward faces, ground.ts).
 */
export function groundSlots(board: Model, params: Params): Model {
  if (!board.slots || board.slots.length === 0) return board;
  const built = buildModel(board, params, { unbaked: true });
  built.group.updateMatrixWorld(true);
  const slots = board.slots.map((sl) => {
    const part = built.parts.get(sl.on);
    if (!part) return sl; // hidden by its `when` for these params
    const sup = { field: supportField(part), scale: 1 };
    const [x0, z0, w, d] = sl.rect;
    let top = 0;
    for (let i = 0; i < 5; i++) {
      for (let j = 0; j < 5; j++) top = Math.max(top, topBelow(sup, x0 + ((i + 0.5) / 5) * w, z0 + ((j + 0.5) / 5) * d, Infinity));
    }
    const target = Math.round((top + PLATE) * 1000) / 1000;
    return target > sl.top + 0.005 ? { ...sl, top: target } : sl;
  });
  return { ...board, slots };
}

export function layoutOnBoard(
  list: Agent[],
  declared: Model,
  params: Params = {},
  table: MinSizeTable = minSizeTable,
): BoardLayout {
  const board = groundSlots(declared, params);
  let s = 1;
  // The sound pass that seated the most agents: past the scale cap it is the layout, and the agents it could not seat get
  // no robot on the board (the floor skips an agent without a home), rather than the whole board failing to open.
  let best: Pass | null = null;
  for (;;) {
    if (s > BOARD_SCALE_CAP + 1e-9) {
      if (!best) throw new Error(`board layout: nothing lays out on ${board.kind} within the world scale cap ${BOARD_SCALE_CAP}`);
      console.warn(`board layout: ${list.length - best.layout.homes.size} of ${list.length} agents do not seat on ${board.kind} within the world scale cap ${BOARD_SCALE_CAP}`);
      return best.layout;
    }
    // Fall back through the candidate exits. A pass is checked with the rooms in place: its rejected landings (the room check),
    // then the plates the nav grid cannot reach (the grid's own landing, rule 6). Each round excludes those and plans again.
    const excluded: Excluded = new Set();
    let pass = layoutAtScale(list, board, params, table, s, excluded);
    for (let round = 0; round < LANDING_ROUNDS; round++) {
      const bad = pass.rejected.length > 0 ? pass.rejected : unreachedKeys(board, params, pass);
      if (bad.length === 0) break;
      for (const key of bad) excluded.add(key);
      pass = layoutAtScale(list, board, params, table, s, excluded);
    }
    const sound = pass.rejected.length === 0 && unreachedKeys(board, params, pass).length === 0;
    if (sound && pass.layout.homes.size === list.length) return pass.layout;
    if (sound && (!best || pass.layout.homes.size > best.layout.homes.size)) best = pass;
    s *= BOARD_SCALE_STEP;
  }
}

/**
 * The surface height at a plaza point: a district's slot top inside its cell, a ramp of RAMP units
 * around the cell sloping down to the PCB top, and the PCB top everywhere else.
 */
export function boardHeightAt(x: number, z: number, layout: Pick<BoardLayout, 'districts' | 'pcbTop' | 'plates'>): number {
  // A plate is walkable at its top everywhere inside it, aisles included (the rooms only fill part of it).
  for (const p of layout.plates) {
    if (x >= p.x0 && x < p.x0 + p.w && z >= p.z0 && z < p.z0 + p.d) return p.y;
  }
  let h = layout.pcbTop;
  for (const d of layout.districts) {
    const gap = Math.hypot(Math.max(Math.abs(x - d.cx) - d.w / 2, 0), Math.max(Math.abs(z - d.cy) - d.d / 2, 0));
    if (gap === 0) return d.y;
    if (gap < RAMP) h = Math.max(h, layout.pcbTop + (d.y - layout.pcbTop) * (1 - gap / RAMP));
  }
  return h;
}

// ---- Walkways ------------------------------------------------------------------------------------

export interface TraceGraph {
  /** The distinct points of the traces, as [x, z]. */
  nodes: [number, number][];
  /** edges[i]: the points linked to node i by a trace segment, with the segment's length. */
  edges: { to: number; len: number }[][];
}

/** The trace polylines as a graph: each distinct point is a node, each segment an edge both ways. */
export function traceGraph(board: Model): TraceGraph {
  const nodes: [number, number][] = [];
  const edges: { to: number; len: number }[][] = [];
  const index = new Map<string, number>();
  const node = (x: number, z: number): number => {
    const key = `${x},${z}`;
    let i = index.get(key);
    if (i === undefined) {
      i = nodes.length;
      index.set(key, i);
      nodes.push([x, z]);
      edges.push([]);
    }
    return i;
  };
  for (const tr of board.traces ?? []) {
    for (const [x, z] of tr.path) node(x, z);
    for (let k = 0; k < tr.path.length - 1; k++) {
      const [x0, z0] = tr.path[k]!;
      const [x1, z1] = tr.path[k + 1]!;
      const a = node(x0, z0);
      const b = node(x1, z1);
      if (a === b) continue;
      const len = Math.hypot(x1 - x0, z1 - z0);
      edges[a]!.push({ to: b, len });
      edges[b]!.push({ to: a, len });
    }
  }
  return { nodes, edges };
}

/**
 * The walk from one point to another along the traces: from, the shortest path between the nodes
 * nearest each point (Dijkstra), to. Null when the graph has no walkway joining them.
 */
export function pathOnTraces(graph: TraceGraph, from: [number, number], to: [number, number]): [number, number][] | null {
  const { nodes, edges } = graph;
  if (nodes.length === 0) return null;
  const nearest = (p: [number, number]): number =>
    nodes.reduce((best, n, i) => (Math.hypot(n[0] - p[0], n[1] - p[1]) < Math.hypot(nodes[best]![0] - p[0], nodes[best]![1] - p[1]) ? i : best), 0);
  const s = nearest(from);
  const t = nearest(to);
  const dist = nodes.map((_, i) => (i === s ? 0 : Infinity));
  const prev = nodes.map(() => -1);
  const done = nodes.map(() => false);
  for (;;) {
    let u = -1;
    for (let i = 0; i < nodes.length; i++) {
      if (!done[i] && dist[i]! < Infinity && (u < 0 || dist[i]! < dist[u]!)) u = i;
    }
    if (u < 0 || u === t) break;
    done[u] = true;
    for (const { to: v, len } of edges[u]!) {
      if (dist[u]! + len < dist[v]!) {
        dist[v] = dist[u]! + len;
        prev[v] = u;
      }
    }
  }
  if (dist[t]! === Infinity) return null;
  const via: [number, number][] = [];
  for (let i = t; i !== -1; i = prev[i]!) via.unshift(nodes[i]!);
  return [from, ...via, to];
}
