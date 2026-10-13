// Procedural room generator, v2 (groups with a purpose). Pure TypeScript: no three.js, no DOM beyond TextEncoder.
// Algorithm: docs/design/rooms.md (v2 and its appendix "v2 algorithm details"), with the binding resolutions
// in that appendix. Both apps must produce identical layouts, so every float expression is written in the order
// it is meant to run.

import minSizeTable from '../../../../../../assets/rooms/minsize.json';

export type Door = 'n' | 'e' | 's' | 'w';
export type Anchor = 'wall' | 'corner' | 'center' | 'door' | 'wallmount';

export interface Input {
  role: string;
  /** Inner size in units (x by z). */
  w: number;
  d: number;
  /** The side facing the nearest walkway: n = -z, e = +x, s = +z, w = -x. */
  door: Door;
  seed: number;
  /** Desk count for per-agent entries; default cap(w, d) = clamp(floor(w * d / 6), 1, 4). */
  agents?: number;
}

export interface CatSpot {
  at: [number, number];
  face: number;
  clip?: string;
}

export interface CatItem {
  model: string;
  /** Footprint w (x) by d (z) at rot 0, and height h. */
  size: [number, number, number];
  place: string;
  face?: string;
  clear?: { front: number; side: number };
  spots?: CatSpot[];
  walk?: boolean;
  layer?: string;
  mount?: number;
}

export interface CatGroupItem {
  /** A catalogue item id, or "$station" (the template's station param). */
  id: string;
  /** Relation to the anchor (the group's first item). Absent for the first item. */
  rel?: string;
  gap?: number;
  /** "anchor": the item faces the anchor. Absent: rot 0 (or the side's rot for "around"). */
  face?: string;
  /** [lo, hi]: the number of copies, drawn once per group instance. */
  count?: [number, number];
  optional?: boolean;
}

export interface CatGroup {
  anchor: Anchor;
  items: CatGroupItem[];
}

export interface Catalog {
  schema?: number;
  cell?: number;
  items: Record<string, CatItem>;
  groups: Record<string, CatGroup>;
}

export interface RoleGroupRef {
  group: string;
  /** Replaces "$station" in the group's items. */
  station?: string;
  /** "agent": repeated cap times (the room's desk count). */
  per?: string;
}

export interface RolePool {
  group: string;
  w?: number;
  max?: number;
}

export interface RoleTemplate {
  family?: string;
  /** "wall": desk-sets anchor at a wall (the user faces the wall). "room": desk-sets prefer a corner (the user faces the room), else a wall. */
  desk_facing?: 'wall' | 'room';
  shell: { floor: string[]; wall: string[]; open: number };
  /** Placed first; a failure is a drop (a violation). */
  required: RoleGroupRef[];
  /** Placed after required, in order, if they fit; a failure is a skip (never a violation). */
  preferred?: RoleGroupRef[];
  pool: RolePool[];
  density: [number, number];
}

export interface Roles {
  schema?: number;
  roles: Record<string, RoleTemplate>;
  families?: Record<string, RoleTemplate>;
}

export interface LayoutShell {
  floor: string;
  wall: string;
  walls: Door[];
  door: { side: Door; at: number; width: number };
}

export interface LayoutItem {
  n: number;
  id: string;
  x: number;
  y: number;
  z: number;
  rot: number;
  host: number | null;
  /** The group instance index (into Layout.groups). */
  g: number;
}

export interface LayoutSpot {
  item: number;
  x: number;
  z: number;
  face: number;
  clip: string | null;
}

export interface Layout {
  shell: LayoutShell;
  items: LayoutItem[];
  spots: LayoutSpot[];
  /** groups[g] = the group id of instance g, in creation order. */
  groups: string[];
  /** Required group ids that could not be placed (a verifier violation). */
  dropped: string[];
  /** Preferred group ids that did not fit (not a violation). */
  skipped: string[];
}

const SIDES: Door[] = ['n', 'e', 's', 'w'];
/** Rotation that turns a wall item's front (+z) to the room centre, per wall. Also the rot of a front-side item. */
const SIDE_ROT: Record<Door, number> = { n: 0, e: 270, s: 180, w: 90 };
const INSET = 0.05;
const WALLMOUNT_DEPTH = 0.1;
/** Metres in front of a wall-mounted item that must stay clear of tall solids. */
const MOUNT_VIEW = 0.6;
/** Half-width margin of the view strip, on each side of the mount. */
const MOUNT_VIEW_SIDE = 0.1;
const CLEAR_MIN_ACCESS = 0.25;
const EPS = 1e-9;
const COPLANAR_TOL = 1e-3;
const MAX_FAILED_TRIES = 20;
const ROW_GAP_DEFAULT = 0;
const ON_SURFACE_OFFSET = 0.05;
const DOOR_NEAR = 0.6;

type Kind = 'solid' | 'surface' | 'floor' | 'wall';

interface Box {
  x0: number;
  x1: number;
  z0: number;
  z1: number;
}

/** Round to 3 decimals as Math.round(v * 1000) / 1000, with -0 mapped to 0. */
export function roundNum(v: number): number {
  const r = Math.round(v * 1000) / 1000;
  return r === 0 ? 0 : r;
}

/** mulberry32: output in [0, 1). */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** 32-bit FNV-1a over the UTF-8 bytes of s. */
export function fnv1a32(s: string): number {
  const bytes = new TextEncoder().encode(s);
  let h = 0x811c9dc5;
  for (let k = 0; k < bytes.length; k++) {
    h ^= bytes[k]!;
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h >>> 0;
}

/** fnv1a32 as 8 lowercase hex digits. */
export function fnv1aHex(s: string): string {
  return fnv1a32(s).toString(16).padStart(8, '0');
}

/** Sorted keys, no spaces, numbers rounded like roundNum (so -0 is 0), JSON strings. */
export function canonicalJson(v: unknown): string {
  if (v === null || v === undefined) return 'null';
  switch (typeof v) {
    case 'number':
      return String(roundNum(v));
    case 'boolean':
      return v ? 'true' : 'false';
    case 'string':
      return JSON.stringify(v);
    case 'object': {
      if (Array.isArray(v)) return '[' + v.map((x) => canonicalJson(x)).join(',') + ']';
      const o = v as Record<string, unknown>;
      const keys = Object.keys(o)
        .filter((k) => o[k]! !== undefined)
        .sort();
      return '{' + keys.map((k) => JSON.stringify(k) + ':' + canonicalJson(o[k]!)).join(',') + '}';
    }
    default:
      return 'null';
  }
}

/** FNV-1a of the canonical JSON of { shell, items, spots, groups, dropped, skipped }, as 8 hex digits. */
export function layoutHash(layout: Layout): string {
  return fnv1aHex(
    canonicalJson({
      shell: layout.shell,
      items: layout.items,
      spots: layout.spots,
      groups: layout.groups,
      dropped: layout.dropped,
      skipped: layout.skipped,
    }),
  );
}

/** The fixture value of one case: the layout hash of generate(input). */
export function fixtureHash(input: Input, catalog: Catalog, roles: Roles): string {
  return layoutHash(generate(input, catalog, roles));
}

/** The template of a role: its own entry, else the family "other" (a role has no family without an entry). */
export function templateFor(role: string, roles: Roles): RoleTemplate {
  if (Object.prototype.hasOwnProperty.call(roles.roles, role)) return roles.roles[role]!;
  const t = roles.families?.['other'];
  if (!t) throw new Error(`rooms: no template for role "${role}" and no "other" family`);
  return t;
}

/** Grid cell (i, j) of a point, clamped to the grid; index = j * cols + i. */
export function cellOfPoint(w: number, d: number, cell: number, x: number, z: number): { i: number; j: number } {
  const cols = Math.floor(w / cell);
  const rows = Math.floor(d / cell);
  const i = Math.min(cols - 1, Math.max(0, Math.floor((x + w / 2) / cell)));
  const j = Math.min(rows - 1, Math.max(0, Math.floor((z + d / 2) / cell)));
  return { i, j };
}

/** Rotate a local (x, z) by rot degrees about y: x' = x cos + z sin, z' = -x sin + z cos (exact for 0/90/180/270). */
function rotPt(x: number, z: number, rot: number): [number, number] {
  switch (((rot % 360) + 360) % 360) {
    case 90:
      return [z, -x];
    case 180:
      return [-x, -z];
    case 270:
      return [-z, x];
    default:
      return [x, z];
  }
}

/** Rot (0/90/180/270) whose front (sin rot, cos rot) best faces the room centre; ties take the first. */
function facing(cx: number, cz: number): number {
  // dot(front, toCentre) with toCentre = (-cx, -cz), for rot 0, 90, 180, 270.
  const dots = [-cz, -cx, cz, cx];
  let best = 0;
  for (let k = 1; k < 4; k++) if (dots[k]! > dots[best]!) best = k;
  return best * 90;
}

function kindOf(cat: CatItem): Kind {
  if (cat.place === 'surface') return 'surface';
  if (cat.layer === 'floor') return 'floor';
  if (cat.place === 'wallmount') return 'wall';
  return 'solid';
}

/** Index range of cells whose open square overlaps the open interval [lo, hi] (half = room half-size). */
function idxRange(lo: number, hi: number, half: number, cell: number, n: number): [number, number] {
  const a = Math.max(0, Math.floor((lo + half) / cell + EPS));
  const b = Math.min(n - 1, Math.ceil((hi + half) / cell - EPS) - 1);
  return [a, b];
}

interface Geo extends Box {
  x: number;
  z: number;
  rot: number;
  fw: number;
  fd: number;
  i0: number;
  i1: number;
  j0: number;
  j1: number;
  hasClear: boolean;
  ci0: number;
  ci1: number;
  cj0: number;
  cj1: number;
}

interface Spot {
  x: number;
  z: number;
  face: number;
  clip: string | null;
  /** The cell containing the point, or -1 when the point is outside the grid (never clamped). */
  cell: number;
}

interface Rec {
  n: number;
  id: string;
  kind: Kind;
  cat: CatItem;
  g: Geo;
  y: number;
  top: number;
  host: number | null;
  /** Group instance index. */
  gi: number;
  spots: Spot[];
  /** Cells that must be reachable (spots and access cell), for solid and wall items only. */
  checks: number[];
  /** Wall-mounted items: the strip in front that tall solids must not block, and the height limit. */
  view: Box | null;
  limit: number;
}

interface Cand {
  geo: Geo;
  ruleFit: number;
  workFar: number;
}

interface Ctx {
  W: number;
  D: number;
  cell: number;
  cols: number;
  rows: number;
  door: Door;
  doorHalf: number;
  diag: number;
  items: Record<string, CatItem>;
  groups: Record<string, CatGroup>;
  xs: number[];
  zs: number[];
  /** Solid footprints covering each cell (saturating) and non-walk ones (blocking). */
  occ: Uint8Array;
  blk: Uint8Array;
  /** Clear-zone owner: -1 none, -3 several, otherwise the item index. */
  res: Int32Array;
  /** Door zone or spine. */
  ds: Uint8Array;
  doorCells: number[];
  blocked: number;
  mark: Int32Array;
  vis: Int32Array;
  stamp: number;
  queue: Int32Array;
  recs: Rec[];
  /** Item indices of the group being placed: their clear zones may be used by each other. */
  members: number[];
  /** Group id per instance, in creation order. */
  groupIds: string[];
  /** The sides that have a wall (the non-door sides of a closed room; none when open). */
  walls: Door[];
  rng: () => number;
  /** Robot spots must lie inside [sx0, sx1] x [sz0, sz1]: inset 0.15 from each wall that exists. */
  sx0: number;
  sx1: number;
  sz0: number;
  sz1: number;
}

function makeGeo(ctx: Ctx, cat: CatItem, x: number, z: number, rot: number): Geo {
  const kind = kindOf(cat);
  const fw = cat.size[0]!;
  const fd = kind === 'wall' ? WALLMOUNT_DEPTH : cat.size[2]!;
  const along = rot % 180 === 0;
  const sx = along ? fw : fd;
  const sz = along ? fd : fw;
  const x0 = x - sx / 2;
  const x1 = x + sx / 2;
  const z0 = z - sz / 2;
  const z1 = z + sz / 2;
  const [i0, i1] = idxRange(x0, x1, ctx.W / 2, ctx.cell, ctx.cols);
  const [j0, j1] = idxRange(z0, z1, ctx.D / 2, ctx.cell, ctx.rows);
  const g: Geo = {
    x, z, rot, fw, fd, x0, x1, z0, z1, i0, i1, j0, j1,
    hasClear: false, ci0: 0, ci1: -1, cj0: 0, cj1: -1,
  };
  const front = cat.clear?.front ?? 0;
  const side = cat.clear?.side ?? 0;
  if (front > 0 && kind !== 'floor' && kind !== 'surface') {
    const lx = fw / 2 + side;
    const corners = [
      rotPt(-lx, fd / 2, rot),
      rotPt(lx, fd / 2, rot),
      rotPt(lx, fd / 2 + front, rot),
      rotPt(-lx, fd / 2 + front, rot),
    ];
    let mnx = Infinity, mxx = -Infinity, mnz = Infinity, mxz = -Infinity;
    for (const [px, pz] of corners) {
      if (px < mnx) mnx = px;
      if (px > mxx) mxx = px;
      if (pz < mnz) mnz = pz;
      if (pz > mxz) mxz = pz;
    }
    [g.ci0, g.ci1] = idxRange(x + mnx, x + mxx, ctx.W / 2, ctx.cell, ctx.cols);
    [g.cj0, g.cj1] = idxRange(z + mnz, z + mxz, ctx.D / 2, ctx.cell, ctx.rows);
    g.hasClear = true;
  }
  return g;
}

function cellIndexOf(ctx: Ctx, x: number, z: number): number {
  const i = Math.min(ctx.cols - 1, Math.max(0, Math.floor((x + ctx.W / 2) / ctx.cell)));
  const j = Math.min(ctx.rows - 1, Math.max(0, Math.floor((z + ctx.D / 2) / ctx.cell)));
  return j * ctx.cols + i;
}

/** Strict overlap of two boxes (positive area). */
function overlaps(a: Box, b: Box): boolean {
  return Math.min(a.x1, b.x1) - Math.max(a.x0, b.x0) > 0 && Math.min(a.z1, b.z1) - Math.max(a.z0, b.z0) > 0;
}

/** The view strip in front of a wall-mounted item: its width plus MOUNT_VIEW_SIDE each side, MOUNT_VIEW deep. */
function viewOf(g: Geo, rot: number): Box {
  const lw = g.fw / 2 + MOUNT_VIEW_SIDE;
  const d0 = g.fd / 2;
  const d1 = g.fd / 2 + MOUNT_VIEW;
  const pts = [rotPt(-lw, d0, rot), rotPt(lw, d0, rot), rotPt(lw, d1, rot), rotPt(-lw, d1, rot)];
  let mnx = Infinity, mxx = -Infinity, mnz = Infinity, mxz = -Infinity;
  for (const [px, pz] of pts) {
    if (px < mnx) mnx = px;
    if (px > mxx) mxx = px;
    if (pz < mnz) mnz = pz;
    if (pz > mxz) mxz = pz;
  }
  return { x0: g.x + mnx, x1: g.x + mxx, z0: g.z + mnz, z1: g.z + mxz };
}

function coplanarClash(ctx: Ctx, top: number, g: Geo): boolean {
  for (const r of ctx.recs) {
    if (r.kind === 'floor') continue;
    if (Math.abs(r.top - top) <= COPLANAR_TOL && overlaps(g, r.g)) return true;
  }
  return false;
}

function makeRec(
  ctx: Ctx,
  id: string,
  cat: CatItem,
  g: Geo,
  kind: Kind,
  host: Rec | null,
  y: number,
  top: number,
  gi: number,
): Rec {
  const spots: Spot[] = (cat.spots ?? []).map((s) => {
    const [dx, dz] = rotPt(s.at[0]!, s.at[1]!, g.rot);
    const x = g.x + dx;
    const z = g.z + dz;
    return { x, z, face: (((s.face + g.rot) % 360) + 360) % 360, clip: s.clip ?? null, cell: exactCell(ctx, x, z) };
  });
  const checks: number[] = [];
  if (kind === 'solid' || kind === 'wall') {
    for (const s of spots) checks.push(s.cell);
    if (cat.walk) {
      checks.push(cellIndexOf(ctx, g.x, g.z));
    } else {
      const front = cat.clear?.front ?? 0;
      const [dx, dz] = rotPt(0, g.fd / 2 + Math.max(front, CLEAR_MIN_ACCESS) / 2, g.rot);
      checks.push(cellIndexOf(ctx, g.x + dx, g.z + dz));
    }
  }
  const view = kind === 'wall' ? viewOf(g, g.rot) : null;
  const limit = kind === 'wall' ? (cat.mount ?? 0) + cat.size[1]! / 2 : 0;
  return { n: ctx.recs.length, id, kind, cat, g, y, top, host: host ? host.n : null, gi, spots, checks, view, limit };
}

/** The cell containing a point, or -1 when the point is outside the grid (no clamping). */
function exactCell(ctx: Ctx, x: number, z: number): number {
  const i = Math.floor((x + ctx.W / 2) / ctx.cell);
  const j = Math.floor((z + ctx.D / 2) / ctx.cell);
  if (i < 0 || i >= ctx.cols || j < 0 || j >= ctx.rows) return -1;
  return j * ctx.cols + i;
}

/**
 * Rule for every spot of a candidate: inside the room rectangle (inset 0.15 from each wall that exists),
 * inside the grid, and not on a BLOCKING cell. The candidate's own footprint counts as blocking unless
 * the candidate is a walk item.
 */
function spotsValid(ctx: Ctx, rec: Rec, candBlocks: boolean): boolean {
  const g = rec.g;
  for (const s of rec.spots) {
    if (s.x < ctx.sx0 || s.x > ctx.sx1 || s.z < ctx.sz0 || s.z > ctx.sz1) return false;
    if (s.cell < 0) return false;
    if (ctx.blk[s.cell]! > 0) return false;
    if (candBlocks) {
      const i = s.cell % ctx.cols;
      const j = (s.cell - i) / ctx.cols;
      if (i >= g.i0 && i <= g.i1 && j >= g.j0 && j <= g.j1) return false;
    }
  }
  return true;
}

/** Every check cell reachable from the door zone over non-blocking cells, with the candidate's footprint as solid. */
function reachable(ctx: Ctx, cand: Rec, candBlocks: boolean): boolean {
  const S = ++ctx.stamp;
  const { cols, rows, mark, vis, blk, queue } = ctx;
  const g = cand.g;
  if (candBlocks) {
    for (let j = g.j0; j <= g.j1; j++) for (let i = g.i0; i <= g.i1; i++) mark[j * cols + i]! = S;
  }
  const blockedAt = (c: number): boolean => blk[c]! > 0 || (candBlocks && mark[c]! === S);
  let qh = 0;
  let qt = 0;
  for (const c of ctx.doorCells) {
    if (blockedAt(c) || vis[c]! === S) continue;
    vis[c]! = S;
    queue[qt++]! = c;
  }
  while (qh < qt) {
    const c = queue[qh++]!;
    const i = c % cols;
    const j = (c - i) / cols;
    if (i > 0 && vis[c - 1]! !== S && !blockedAt(c - 1)) { vis[c - 1]! = S; queue[qt++]! = c - 1; }
    if (i < cols - 1 && vis[c + 1]! !== S && !blockedAt(c + 1)) { vis[c + 1]! = S; queue[qt++]! = c + 1; }
    if (j > 0 && vis[c - cols]! !== S && !blockedAt(c - cols)) { vis[c - cols]! = S; queue[qt++]! = c - cols; }
    if (j < rows - 1 && vis[c + cols]! !== S && !blockedAt(c + cols)) { vis[c + cols]! = S; queue[qt++]! = c + cols; }
  }
  for (const r of ctx.recs) {
    if (r.kind !== 'solid' && r.kind !== 'wall') continue;
    for (const c of r.checks) if (vis[c]! !== S) return false;
  }
  for (const c of cand.checks) if (vis[c]! !== S) return false;
  return true;
}

/**
 * Try one placement (no random draws). Returns the record, not yet committed.
 * `ban`: the item may not stand in the middle region (non-centre groups); floor items are exempt.
 */
function tryPlace(
  ctx: Ctx,
  id: string,
  cat: CatItem,
  g: Geo,
  host: Rec | null,
  gi: number,
  ban: boolean,
): Rec | null {
  const kind = kindOf(cat);
  if (kind === 'surface') {
    if (!host) return null;
    if (!(g.x0 >= host.g.x0 && g.x1 <= host.g.x1 && g.z0 >= host.g.z0 && g.z1 <= host.g.z1)) return null;
    const top = host.top + cat.size[1]!;
    if (coplanarClash(ctx, top, g)) return null;
    const srec = makeRec(ctx, id, cat, g, kind, host, host.top, top, gi);
    return spotsValid(ctx, srec, false) ? srec : null;
  }
  if (!(g.x0 >= -ctx.W / 2 && g.x1 <= ctx.W / 2 && g.z0 >= -ctx.D / 2 && g.z1 <= ctx.D / 2)) return null;
  if (ban && kind !== 'floor' && Math.abs(g.x) <= ctx.W / 4 && Math.abs(g.z) <= ctx.D / 4) return null;
  for (let j = g.j0; j <= g.j1; j++) {
    for (let i = g.i0; i <= g.i1; i++) if (ctx.ds[j * ctx.cols + i]!) return null;
  }
  const y = kind === 'wall' ? cat.mount ?? 0 : 0;
  const top = y + cat.size[1]!;
  if (kind === 'floor') {
    const frec = makeRec(ctx, id, cat, g, kind, host, y, top, gi);
    return spotsValid(ctx, frec, false) ? frec : null;
  }
  if (kind === 'solid') {
    for (let j = g.j0; j <= g.j1; j++) {
      for (let i = g.i0; i <= g.i1; i++) {
        const c = j * ctx.cols + i;
        // Cell occupancy is coarse: a group member's footprint is tested exactly (overlap below), so it may share
        // a cell with its own anchor. The anchor itself is tested against every occupied cell.
        if (ctx.members.length === 0 && ctx.occ[c]! > 0) return null;
        const r = ctx.res[c]!;
        // A reserved cell may be used only by a clear zone of this group's own items.
        if (r !== -1 && !ctx.members.includes(r)) return null;
      }
    }
    for (const r of ctx.recs) if (r.kind === 'solid' && overlaps(g, r.g)) return null;
    // A tall solid may not stand in the view strip of a wall-mounted item.
    for (const r of ctx.recs) {
      if (r.kind === 'wall' && r.view && overlaps(r.view, g) && top > r.limit) return null;
    }
  }
  if (g.hasClear) {
    for (let j = g.cj0; j <= g.cj1; j++) {
      for (let i = g.ci0; i <= g.ci1; i++) if (ctx.occ[j * ctx.cols + i]! > 0) return null;
    }
  }
  if (coplanarClash(ctx, top, g)) return null;
  const rec = makeRec(ctx, id, cat, g, kind, host, y, top, gi);
  if (kind === 'wall') {
    // A wall-mounted item may not sit in front of a tall solid.
    for (const r of ctx.recs) {
      if (r.kind === 'solid' && rec.view && overlaps(rec.view, r.g) && r.top > rec.limit) return null;
    }
  }
  if (!spotsValid(ctx, rec, kind === 'solid' && !cat.walk)) return null;
  if ((kind === 'solid' || kind === 'wall') && !reachable(ctx, rec, kind === 'solid' && !cat.walk)) return null;
  return rec;
}

/** Record a placed item in the grid: occupancy, blocking and clear-zone reservations. */
function commit(ctx: Ctx, rec: Rec): void {
  const g = rec.g;
  if (rec.kind === 'solid') {
    const walk = !!rec.cat.walk;
    for (let j = g.j0; j <= g.j1; j++) {
      for (let i = g.i0; i <= g.i1; i++) {
        const c = j * ctx.cols + i;
        if (ctx.occ[c]! < 255) ctx.occ[c]!++;
        if (!walk) {
          if (ctx.blk[c]! === 0) ctx.blocked++;
          ctx.blk[c]!++;
        }
      }
    }
  }
  if (g.hasClear) {
    for (let j = g.cj0; j <= g.cj1; j++) {
      for (let i = g.ci0; i <= g.ci1; i++) {
        const c = j * ctx.cols + i;
        const r = ctx.res[c]!;
        ctx.res[c]! = r === -1 ? rec.n : r === rec.n ? r : -3;
      }
    }
  }
  ctx.recs.push(rec);
}

function spreadAt(ctx: Ctx, id: string, x: number, z: number): number {
  let best = 0;
  for (const r of ctx.recs) {
    if (r.id !== id) continue;
    const dx = x - r.g.x;
    const dz = z - r.g.z;
    const v = 1 - Math.sqrt(dx * dx + dz * dz) / (0.5 * ctx.diag);
    const s = v > 0 ? v : 0;
    if (s > best) best = s;
  }
  return best;
}

function doorFit(near: number): number {
  return near >= -EPS && near <= DOOR_NEAR ? 1 : 0;
}

/** True when the lateral interval [lo, hi] lies beyond the door opening (on a stub), not over it. */
function beyondOpening(ctx: Ctx, lo: number, hi: number): boolean {
  return lo >= ctx.doorHalf - EPS || hi <= -ctx.doorHalf + EPS;
}

/**
 * Candidates for a group's anchor item, in enumeration order (the order is part of the algorithm).
 * wall: sides n, e, s, w (not the door), cells along each; corner: nw, ne, se, sw (not the door's corners);
 * center: cells in the middle half; door: cells along the door's own wall, rule fit 1 when beside the door;
 * wallmount: cells along each side but the door.
 */
function anchorCandidates(ctx: Ctx, cat: CatItem, rule: Anchor): Cand[] {
  const out: Cand[] = [];
  const hasSpots = (cat.spots?.length ?? 0) > 0;
  const doorPt: [number, number] =
    ctx.door === 'n' ? [0, -ctx.D / 2] : ctx.door === 'e' ? [ctx.W / 2, 0] : ctx.door === 's' ? [0, ctx.D / 2] : [-ctx.W / 2, 0];
  const push = (x: number, z: number, rot: number, ruleFit: number): void => {
    const dx = x - doorPt[0]!;
    const dz = z - doorPt[1]!;
    const workFar = hasSpots ? Math.sqrt(dx * dx + dz * dz) / ctx.diag : 0;
    out.push({ geo: makeGeo(ctx, cat, x, z, rot), ruleFit, workFar });
  };
  const fw = cat.size[0]!;
  const fd0 = cat.size[2]!;
  if (rule === 'wall' || rule === 'wallmount' || rule === 'door') {
    const fdUse = rule === 'wallmount' ? WALLMOUNT_DEPTH : fd0;
    for (const side of SIDES) {
      if (rule === 'door' && side !== ctx.door) continue;
      // A wall-anchored item needs its wall: no candidates on a side without one.
      if (rule !== 'door' && !ctx.walls.includes(side)) continue;
      // On the door side a wall-anchored item stands on a stub: its footprint lies beyond the opening's edge.
      const onDoorSide = side === ctx.door && rule !== 'door';
      const rot = SIDE_ROT[side]!;
      const along = rot % 180 === 0;
      const hx = (along ? fw : fdUse) / 2;
      const hz = (along ? fdUse : fw) / 2;
      const base = rule === 'wall' ? 1 : 0;
      if (side === 'n' || side === 's') {
        const z = side === 'n' ? -ctx.D / 2 + hz + INSET : ctx.D / 2 - hz - INSET;
        for (let i = 0; i < ctx.cols; i++) {
          const x = ctx.xs[i]!;
          if (onDoorSide && !beyondOpening(ctx, x - hx, x + hx)) continue;
          const rf = rule === 'door' ? doorFit(Math.abs(x) - ctx.doorHalf - hx) : base;
          // A door candidate must hug the opening (within DOOR_NEAR of its edge); others are not candidates.
          if (rule === 'door' && rf === 0) continue;
          push(x, z, rot, rf);
        }
      } else {
        const x = side === 'w' ? -ctx.W / 2 + hx + INSET : ctx.W / 2 - hx - INSET;
        for (let j = 0; j < ctx.rows; j++) {
          const z = ctx.zs[j]!;
          if (onDoorSide && !beyondOpening(ctx, z - hz, z + hz)) continue;
          const rf = rule === 'door' ? doorFit(Math.abs(z) - ctx.doorHalf - hz) : base;
          if (rule === 'door' && rf === 0) continue;
          push(x, z, rot, rf);
        }
      }
    }
    return out;
  }
  if (rule === 'corner') {
    const corners: [number, number, Door, Door][] = [
      [-1, -1, 'n', 'w'],
      [1, -1, 'n', 'e'],
      [1, 1, 's', 'e'],
      [-1, 1, 's', 'w'],
    ];
    for (const [sx, sz, a, b] of corners) {
      if (a === ctx.door || b === ctx.door) continue;
      // The rotation comes from the centre computed with the rot-0 footprint; the centre then uses that rot.
      const rot = facing(sx * (ctx.W / 2 - INSET - fw / 2), sz * (ctx.D / 2 - INSET - fd0 / 2));
      const along = rot % 180 === 0;
      const hx = (along ? fw : fd0) / 2;
      const hz = (along ? fd0 : fw) / 2;
      push(sx * (ctx.W / 2 - INSET - hx), sz * (ctx.D / 2 - INSET - hz), rot, 1);
    }
    return out;
  }
  // center
  for (let j = 0; j < ctx.rows; j++) {
    for (let i = 0; i < ctx.cols; i++) {
      const x = ctx.xs[i]!;
      const z = ctx.zs[j]!;
      if (!(Math.abs(x) <= ctx.W / 4 && Math.abs(z) <= ctx.D / 4)) continue;
      push(x, z, facing(x, z), 0);
    }
  }
  return out;
}

/** One item slot of a group instance, with its relation resolved. */
interface Slot {
  id: string;
  cat: CatItem;
  rel: string;
  gap: number;
  face: string;
  required: boolean;
  side: Door | null;
}

/** Row cursor and stop flag of one attempt, for "row" slots. */
interface RowState {
  right: number;
  stopped: boolean;
}

/**
 * The geometry of one non-anchor slot, from the anchor's placed record. Offsets are in the anchor's frame
 * (rot-0 sizes), then turned by the anchor's rot. Returns null when the relation does not apply.
 */
function slotGeo(ctx: Ctx, s: Slot, a: Rec, row: RowState): Geo | null {
  const aw = a.cat.size[0]!;
  const ad = a.cat.size[2]!;
  const ahx = aw / 2;
  const ahz = ad / 2;
  const halves = (rr: number): [number, number] => {
    const hw = s.cat.size[0]! / 2;
    const hd = s.cat.size[2]! / 2;
    return rr % 180 === 0 ? [hw, hd] : [hd, hw];
  };
  let lx = 0;
  let lz = 0;
  let rr = 0;
  switch (s.rel) {
    case 'front': {
      rr = s.face === 'anchor' ? 180 : 0;
      lz = ahz + s.gap + halves(rr)[1];
      break;
    }
    case 'beside': {
      lx = -(ahx + s.gap + halves(0)[0]);
      break;
    }
    case 'beside-other': {
      lx = ahx + s.gap + halves(0)[0];
      break;
    }
    case 'row': {
      if (row.stopped) return null;
      const hx = halves(0)[0];
      lx = row.right + s.gap + hx;
      row.right = lx + hx;
      break;
    }
    case 'on': {
      lz = -ahz + halves(0)[1] + ON_SURFACE_OFFSET;
      break;
    }
    case 'around': {
      const side = s.side;
      if (!side) return null;
      rr = SIDE_ROT[side];
      const [hx, hz] = halves(rr);
      if (side === 'n') lz = -(ahz + s.gap + hz);
      else if (side === 's') lz = ahz + s.gap + hz;
      else if (side === 'e') lx = ahx + s.gap + hx;
      else lx = -(ahx + s.gap + hx);
      break;
    }
    case 'mirror': {
      // Reflect the anchor's centre across the door's axis, same rot.
      const mx = ctx.door === 'n' || ctx.door === 's' ? -a.g.x : a.g.x;
      const mz = ctx.door === 'e' || ctx.door === 'w' ? -a.g.z : a.g.z;
      return makeGeo(ctx, s.cat, mx, mz, a.g.rot);
    }
    default:
      return null;
  }
  const [dx, dz] = rotPt(lx, lz, a.g.rot);
  return makeGeo(ctx, s.cat, a.g.x + dx, a.g.z + dz, (((a.g.rot + rr) % 360) + 360) % 360);
}

/**
 * One attempt to place a whole group at a given anchor placement. Items are committed as they are placed;
 * on any required failure the grid is restored to its state before the attempt.
 */
function attemptGroup(ctx: Ctx, gi: number, anchorGeo: Geo, slots: Slot[], rule: Anchor): boolean {
  const snap = {
    occ: ctx.occ.slice(),
    blk: ctx.blk.slice(),
    res: ctx.res.slice(),
    blocked: ctx.blocked,
    nrec: ctx.recs.length,
  };
  const fail = (): false => {
    ctx.occ.set(snap.occ);
    ctx.blk.set(snap.blk);
    ctx.res.set(snap.res);
    ctx.blocked = snap.blocked;
    ctx.recs.length = snap.nrec;
    ctx.members = [];
    return false;
  };
  const ban = rule !== 'center';
  ctx.members = [];
  const first = slots[0]!;
  const ar = tryPlace(ctx, first.id, first.cat, anchorGeo, null, gi, ban);
  if (!ar) return fail();
  commit(ctx, ar);
  ctx.members.push(ar.n);
  const row: RowState = { right: first.cat.size[0]! / 2, stopped: false };
  for (let k = 1; k < slots.length; k++) {
    const s = slots[k]!;
    const geo = slotGeo(ctx, s, ar, row);
    const rec = geo ? tryPlace(ctx, s.id, s.cat, geo, s.rel === 'on' ? ar : null, gi, ban) : null;
    if (!rec) {
      if (s.rel === 'row') row.stopped = true;
      if (s.required) return fail();
      continue;
    }
    commit(ctx, rec);
    ctx.members.push(rec.n);
  }
  ctx.members = [];
  return true;
}

/**
 * The slots of one group instance, in order. Draws (once per group instance, before any candidate):
 * a count per [lo, hi] entry, then, for an "around" entry, a Fisher-Yates shuffle of the sides n, e, s, w.
 * Copy c of an entry is required when c <= lo and the entry is not optional; the anchor is always required.
 */
function buildSlots(ctx: Ctx, gid: string, station: string | null): Slot[] {
  const def = Object.prototype.hasOwnProperty.call(ctx.groups, gid) ? ctx.groups[gid] : undefined;
  if (!def) throw new Error(`rooms: unknown group "${gid}"`);
  const slots: Slot[] = [];
  def.items.forEach((e, idx) => {
    const id = e.id === '$station' ? station : e.id;
    if (!id || !Object.prototype.hasOwnProperty.call(ctx.items, id)) throw new Error(`rooms: group "${gid}" item "${e.id}" unresolved`);
    const cat = ctx.items[id]!;
    let lo = 1;
    let n = 1;
    if (e.count) {
      lo = e.count[0];
      n = lo + Math.floor(ctx.rng() * (e.count[1] - lo + 1));
    }
    let sides: Door[] = SIDES.slice();
    if (e.rel === 'around') {
      for (let i = 3; i >= 1; i--) {
        const j = Math.floor(ctx.rng() * (i + 1));
        const t = sides[i]!;
        sides[i] = sides[j]!;
        sides[j] = t;
      }
    }
    for (let c = 1; c <= n; c++) {
      const isAnchor = idx === 0 && c === 1;
      let rel = e.rel ?? (idx === 0 ? 'row' : '');
      if (isAnchor) rel = '';
      slots.push({
        id,
        cat,
        rel,
        gap: e.gap ?? ROW_GAP_DEFAULT,
        face: e.face ?? '',
        required: isAnchor || (!e.optional && c <= lo),
        side: rel === 'around' ? sides[c - 1]! : null,
      });
    }
  });
  if (slots[0]!.id === '') throw new Error(`rooms: group "${gid}" has no anchor`);
  return slots;
}

/**
 * Place one group instance: anchor candidates of each rule (the first rule's candidates get a +0.5 preference),
 * in score order; the first whole success is kept. Returns true when placed.
 */
function placeGroup(ctx: Ctx, gid: string, station: string | null, rules: Anchor[]): boolean {
  const gi = ctx.groupIds.length;
  const slots = buildSlots(ctx, gid, station);
  const cands: Cand[] = [];
  rules.forEach((rule, ri) => {
    for (const c of anchorCandidates(ctx, slots[0]!.cat, rule)) {
      cands.push(ri === 0 ? { ...c, ruleFit: c.ruleFit + 0.5 } : c);
    }
  });
  const rule = rules[0]!;
  const score = new Float64Array(cands.length);
  for (let k = 0; k < cands.length; k++) {
    const c = cands[k]!;
    const sp = spreadAt(ctx, slots[0]!.id, c.geo.x, c.geo.z);
    score[k]! = c.ruleFit + 0.5 * c.workFar - 0.3 * sp + 0.05 * ctx.rng();
  }
  const order: number[] = [];
  for (let k = 0; k < cands.length; k++) order.push(k);
  order.sort((a, b) => score[b]! - score[a]! || a - b);
  for (const k of order) {
    if (attemptGroup(ctx, gi, cands[k]!.geo, slots, rule)) {
      ctx.groupIds.push(gid);
      return true;
    }
  }
  return false;
}

function pick<T>(list: T[], rng: () => number): T | undefined {
  return list[Math.floor(rng() * list.length)]!;
}

/**
 * The anchor rules of a group for a room, in preference order. Desk-sets of "room" roles prefer a corner
 * (the user faces into the room) and fall back to a wall; every other group has its catalogue rule.
 */
function anchorRulesOf(ctx: Ctx, gid: string, deskFacing: string): Anchor[] {
  if (gid === 'desk-set' && deskFacing === 'room') return ['corner', 'wall'];
  return [ctx.groups[gid]!.anchor];
}

/** Generate the room layout for one input. Pure: same input and data, same output. */
export function generate(input: Input, catalog: Catalog, roles: Roles): Layout {
  const tpl = templateFor(input.role, roles);
  const rng = mulberry32(input.seed);
  const cell = catalog.cell ?? 0.25;
  const W = input.w;
  const D = input.d;
  const door = input.door;
  const cols = Math.floor(W / cell);
  const rows = Math.floor(D / cell);
  const xs: number[] = [];
  const zs: number[] = [];
  for (let i = 0; i < cols; i++) xs.push(-W / 2 + (i + 0.5) * cell);
  for (let j = 0; j < rows; j++) zs.push(-D / 2 + (j + 0.5) * cell);
  const N = cols * rows;
  const ctx: Ctx = {
    W, D, cell, cols, rows, door,
    doorHalf: 0,
    diag: Math.sqrt(W * W + D * D),
    items: catalog.items,
    groups: catalog.groups,
    xs, zs,
    occ: new Uint8Array(N),
    blk: new Uint8Array(N),
    res: new Int32Array(N).fill(-1),
    ds: new Uint8Array(N),
    doorCells: [],
    blocked: 0,
    mark: new Int32Array(N),
    vis: new Int32Array(N),
    stamp: 0,
    queue: new Int32Array(N),
    recs: [],
    members: [],
    groupIds: [],
    walls: [],
    rng,
    sx0: 0,
    sx1: 0,
    sz0: 0,
    sz1: 0,
  };

  // Shell draws, in order: floor, wall, open; then the density draw.
  const floor = pick(tpl.shell.floor, rng) ?? '';
  const wall = pick(tpl.shell.wall, rng) ?? '';
  const openRoom = rng() < tpl.shell.open;
  // Every side has a wall; the door side has two stubs with the opening between them (see the appendix, A0).
  const walls: Door[] = openRoom ? [] : SIDES.slice();
  ctx.walls = walls;
  // Robot spots stay 0.15 off each wall that exists (the room edge where there is none).
  ctx.sx0 = walls.includes('w') ? -W / 2 + 0.15 : -W / 2;
  ctx.sx1 = walls.includes('e') ? W / 2 - 0.15 : W / 2;
  ctx.sz0 = walls.includes('n') ? -D / 2 + 0.15 : -D / 2;
  ctx.sz1 = walls.includes('s') ? D / 2 - 0.15 : D / 2;
  const [dLo, dHi] = tpl.density;
  const density = dLo + rng() * (dHi - dLo);

  // Door zone (1.0 deep, centred, at most side - 0.5 wide) and the spine from the door to the centre.
  const sideLen = door === 'n' || door === 's' ? W : D;
  const doorWidth = Math.min(1.0, sideLen - 0.5);
  const halfWidth = doorWidth / 2;
  ctx.doorHalf = halfWidth;
  for (let j = 0; j < rows; j++) {
    for (let i = 0; i < cols; i++) {
      const x = xs[i]!;
      const z = zs[j]!;
      const c = j * cols + i;
      let depth: number, lateral: number, towardCentre: boolean;
      if (door === 'n') { depth = z + D / 2; lateral = x; towardCentre = z <= 0; }
      else if (door === 's') { depth = D / 2 - z; lateral = x; towardCentre = z >= 0; }
      else if (door === 'e') { depth = W / 2 - x; lateral = z; towardCentre = x >= 0; }
      else { depth = x + W / 2; lateral = z; towardCentre = x <= 0; }
      if (depth <= 1.0 && Math.abs(lateral) <= halfWidth) {
        ctx.ds[c]! = 1;
        ctx.doorCells.push(c);
      } else if (Math.abs(lateral) <= 0.25 && towardCentre) {
        ctx.ds[c]! = 1;
      }
    }
  }

  const dropped: string[] = [];
  const skipped: string[] = [];
  const placeTop = (ref: RoleGroupRef): boolean => {
    return placeGroup(ctx, ref.group, ref.station ?? null, anchorRulesOf(ctx, ref.group, tpl.desk_facing ?? 'wall'));
  };

  // Required groups first (the station, then the per-agent desk-sets), then preferred (skipped, never dropped),
  // then the pool. A per-agent entry repeats `agents` times (default cap(w, d)).
  const agents = input.agents ?? Math.min(4, Math.max(1, Math.floor((W * D) / 6)));
  const requiredOrdered = [
    ...tpl.required.filter((e) => e.per !== 'agent'),
    ...tpl.required.filter((e) => e.per === 'agent'),
  ];
  for (const e of requiredOrdered) {
    const count = e.per === 'agent' ? agents : 1;
    for (let k = 0; k < count; k++) if (!placeTop(e)) dropped.push(e.group);
  }
  for (const e of tpl.preferred ?? []) {
    const count = e.per === 'agent' ? agents : 1;
    for (let k = 0; k < count; k++) if (!placeTop(e)) skipped.push(e.group);
  }

  // Pool: weighted picks until the density is reached or 20 tries fail in a row.
  const pool = tpl.pool;
  const counts = pool.map(() => 0);
  const weightOf = (k: number): number => pool[k]!.w ?? 1;
  const maxOf = (k: number): number => pool[k]!.max ?? Infinity;
  let fails = 0;
  while (fails < MAX_FAILED_TRIES) {
    if (ctx.blocked / N >= density) break;
    let total = 0;
    for (let k = 0; k < pool.length; k++) if (counts[k]! < maxOf(k)) total += weightOf(k);
    if (!(total > 0)) break;
    const u = rng() * total;
    let acc = 0;
    let chosen = -1;
    for (let k = 0; k < pool.length; k++) {
      if (!(counts[k]! < maxOf(k))) continue;
      acc += weightOf(k);
      if (acc > u) { chosen = k; break; }
    }
    if (chosen < 0) {
      for (let k = pool.length - 1; k >= 0; k--) if (counts[k]! < maxOf(k)) { chosen = k; break; }
    }
    if (placeTop({ group: pool[chosen]!.group })) {
      counts[chosen]!++;
      fails = 0;
    } else {
      fails++;
    }
  }

  // Output: rounded, items in placement order, spots transformed.
  const items: LayoutItem[] = [];
  const spots: LayoutSpot[] = [];
  for (const r of ctx.recs) {
    items.push({
      n: r.n,
      id: r.id,
      x: roundNum(r.g.x),
      y: roundNum(r.y),
      z: roundNum(r.g.z),
      rot: r.g.rot,
      host: r.host,
      g: r.gi,
    });
    for (const s of r.spots) {
      spots.push({ item: r.n, x: roundNum(s.x), z: roundNum(s.z), face: s.face, clip: s.clip });
    }
  }
  return {
    shell: {
      floor,
      wall,
      walls,
      door: { side: door, at: 0, width: roundNum(doorWidth) },
    },
    items,
    spots,
    groups: ctx.groupIds.slice(),
    dropped,
    skipped,
  };
}

/**
 * The smallest square side s (0.5 steps from 2.5 to 8) at which the required groups of a role always fit,
 * for `agents` desks. This is a check, not a formula: for each candidate side, every seed 1..500 and each of
 * the four doors is generated, and s qualifies when no required group is dropped in any of them. Infinity if
 * no side up to 8 qualifies.
 */
export function minSize(role: string, agents: number, catalog: Catalog, roles: Roles): number {
  // The precomputed table (assets/rooms/minsize.json: role -> minSize for agents 1..6; null = none) when it
  // holds this role and agent count; otherwise computed.
  if (Object.prototype.hasOwnProperty.call(minSizeTable, role)) {
    const row = (minSizeTable as Record<string, (number | null)[]>)[role];
    const v = row?.[agents - 1];
    if (typeof v === 'number') return v;
    if (v === null) return Infinity;
  }
  return computeMinSize(role, agents, catalog, roles);
}

/** Computes minSize by generation (see minSize); used when the table does not hold the role and count. */
export function computeMinSize(role: string, agents: number, catalog: Catalog, roles: Roles): number {
  for (let s = 2.5; s <= 8 + EPS; s += 0.5) {
    const side = Math.round(s * 10) / 10;
    let fits = true;
    for (const door of SIDES) {
      for (let seed = 1; seed <= 500 && fits; seed++) {
        const layout = generate({ role, w: side, d: side, door, seed, agents }, catalog, roles);
        if (layout.dropped.length > 0) fits = false;
      }
      if (!fits) break;
    }
    if (fits) return side;
  }
  return Infinity;
}
