// Layout verifier. Recomputes footprints, grid cells, clear zones and reachability from the layout and
// the catalogue alone (it does not use gen.ts internals), with a 1e-3 tolerance on float comparisons.
// Returns human-readable violations; an empty list means the layout is valid.

import type { Catalog, CatGroup, CatItem, Door, Input, Layout, LayoutItem, Roles, RoleTemplate } from './gen';

const TOL = 1e-3;
const WALLMOUNT_DEPTH = 0.1;
const MIN_ACCESS = 0.25;
const SPOT_TOL = 0.0011;
/** v2: a group anchor (or a wall-mounted item's back) is within this distance of the wall or edge it stands on. */
const ANCHOR_TOUCH = 0.08;
/** v2: the strip in front of a wall-mounted item that must stay clear (metres). */
const MOUNT_VIEW = 0.6;
/** v2: seats face their anchor within this many degrees; spots face their item within the second limit. */
const SEAT_FACE_TOL = 10;
const SPOT_FACE_TOL = 30;
const SEAT_IDS = ['chair', 'armchair', 'stool'];
const SEAT_GROUPS = ['desk-set', 'meeting', 'station', 'reading-nook'];
/** Rotation that turns a wall item's front to the room centre, per wall (as gen.ts). */
const SIDE_ROT: Record<Door, number> = { n: 0, e: 270, s: 180, w: 90 };

type Kind = 'solid' | 'surface' | 'floor' | 'wall';

type GroupedCatalog = Catalog & { groups?: Record<string, CatGroup> };
/** v2 layout: `groups[g]` is the group id of instance g; each item carries its instance index `g`. */
type GroupedLayout = Layout & { groups?: string[] };
/** A template's required entry: v2 names a group, and `per: "agent"` scales it by the desk count. */
interface ReqEntry {
  group?: string;
  per?: string;
}

/** Front of an item (or spot) at a rotation in degrees: (sin r, cos r) in (x, z). */
function frontOf(deg: number): [number, number] {
  const r = (deg * Math.PI) / 180;
  return [Math.sin(r), Math.cos(r)];
}

/** Angle in degrees between two planar vectors; 0 when either has no length. */
function angleDeg(a: [number, number], b: [number, number]): number {
  const la = Math.hypot(a[0], a[1]);
  const lb = Math.hypot(b[0], b[1]);
  if (la === 0 || lb === 0) return 0;
  const c = Math.max(-1, Math.min(1, (a[0] * b[0] + a[1] * b[1]) / (la * lb)));
  return (Math.acos(c) * 180) / Math.PI;
}

/** The wall side an item's back is against, from its rotation (the inverse of SIDE_ROT). */
function sideOfRot(rot: number): Door | undefined {
  return (['n', 'e', 's', 'w'] as Door[]).find((s) => SIDE_ROT[s] === rot);
}

interface VItem {
  k: number;
  id: string;
  cat: CatItem;
  g: number;
  kind: Kind;
  x: number;
  y: number;
  z: number;
  rot: number;
  host: number | null;
  fw: number;
  fd: number;
  x0: number;
  x1: number;
  z0: number;
  z1: number;
  top: number;
}

function has(o: object, k: string): boolean {
  return Object.prototype.hasOwnProperty.call(o, k);
}

function kindOfCat(cat: CatItem): Kind {
  if (cat.place === 'surface') return 'surface';
  if (cat.layer === 'floor') return 'floor';
  if (cat.place === 'wallmount') return 'wall';
  return 'solid';
}

function rotated(x: number, z: number, rot: number): [number, number] {
  const r = ((rot % 360) + 360) % 360;
  if (r === 90) return [z, -x];
  if (r === 180) return [-x, -z];
  if (r === 270) return [-z, x];
  return [x, z];
}

function findTemplate(role: string, roles: Roles): RoleTemplate | undefined {
  if (has(roles.roles, role)) return roles.roles[role]!;
  return roles.families?.['other'];
}

export function verify(layout: Layout, input: Input, catalog: Catalog, roles: Roles): string[] {
  const out: string[] = [];
  const tpl = findTemplate(input.role, roles);
  if (!tpl) return [`no template for role "${input.role}"`];

  const cell = catalog.cell ?? 0.25;
  const W = input.w;
  const D = input.d;
  const door: Door = input.door;
  const cols = Math.floor(W / cell);
  const rows = Math.floor(D / cell);
  const N = cols * rows;
  const cellXLo = (i: number): number => -W / 2 + i * cell;
  const cellZLo = (j: number): number => -D / 2 + j * cell;

  // Shell.
  const sh = layout.shell;
  if (!sh) return ['no shell'];
  if (sh.door.side !== door) out.push(`shell: door side ${sh.door.side}, expected ${door}`);
  const sideLen = door === 'n' || door === 's' ? W : D;
  const expWidth = Math.min(1.0, sideLen - 0.5);
  if (Math.abs(sh.door.width - expWidth) > TOL) out.push(`shell: door width ${sh.door.width}, expected ${expWidth}`);
  if (Math.abs(sh.door.at) > TOL) out.push(`shell: door at ${sh.door.at}, expected 0`);
  const allSides = ['n', 'e', 's', 'w'] as Door[];
  if (sh.walls.length > 0 && sh.walls.join(',') !== allSides.join(',')) {
    out.push(`shell: walls [${sh.walls.join(',')}] are not all four sides (the door side has stubs)`);
  }
  if (layout.dropped.length > 0) out.push(`dropped required groups: ${layout.dropped.join(', ')}`);

  // v2 groups: each item's instance index must name an entry of layout.groups, and each entry a catalogue group.
  const lay = layout as GroupedLayout;
  const groups: string[] = lay.groups ?? [];
  const catGroups: Record<string, CatGroup> = (catalog as GroupedCatalog).groups ?? {};
  for (let gi = 0; gi < groups.length; gi++) {
    if (!has(catGroups, groups[gi]!)) out.push(`group ${gi}: "${groups[gi]}" is not a catalogue group`);
  }
  /** The walls that exist. A wall-anchored item needs its wall: the room edge of a wall-less side does not count. */
  const edges: Door[] = sh.walls;

  // Door zone and spine (cell centres, as the algorithm defines them).
  const ds = new Uint8Array(N);
  const doorCells: number[] = [];
  const halfWidth = expWidth / 2;
  for (let j = 0; j < rows; j++) {
    for (let i = 0; i < cols; i++) {
      const x = -W / 2 + (i + 0.5) * cell;
      const z = -D / 2 + (j + 0.5) * cell;
      const c = j * cols + i;
      let depth: number, lateral: number, towardCentre: boolean;
      if (door === 'n') { depth = z + D / 2; lateral = x; towardCentre = z <= 0; }
      else if (door === 's') { depth = D / 2 - z; lateral = x; towardCentre = z >= 0; }
      else if (door === 'e') { depth = W / 2 - x; lateral = z; towardCentre = x >= 0; }
      else { depth = x + W / 2; lateral = z; towardCentre = x <= 0; }
      if (depth <= 1.0 && Math.abs(lateral) <= halfWidth) {
        ds[c]! = 1;
        doorCells.push(c);
      } else if (Math.abs(lateral) <= 0.25 && towardCentre) {
        ds[c]! = 1;
      }
    }
  }

  /** Cells whose open square overlaps the rectangle by more than the tolerance. */
  const covered = (x0: number, x1: number, z0: number, z1: number): number[] => {
    const res: number[] = [];
    for (let j = 0; j < rows; j++) {
      const zl = cellZLo(j);
      const zh = zl + cell;
      if (Math.min(zh, z1) - Math.max(zl, z0) <= TOL) continue;
      for (let i = 0; i < cols; i++) {
        const xl = cellXLo(i);
        const xh = xl + cell;
        if (Math.min(xh, x1) - Math.max(xl, x0) > TOL) res.push(j * cols + i);
      }
    }
    return res;
  };

  /** Cells of the points' grid window (a point on a cell edge may belong to either neighbour). */
  const windowCells = (x: number, z: number): number[] => {
    const u = (x + W / 2) / cell;
    const v = (z + D / 2) / cell;
    const t = TOL / cell;
    const is = [Math.floor(u - t), Math.floor(u + t)].map((i) => Math.min(cols - 1, Math.max(0, i)));
    const js = [Math.floor(v - t), Math.floor(v + t)].map((j) => Math.min(rows - 1, Math.max(0, j)));
    const res: number[] = [];
    for (const j of js) for (const i of is) {
      const c = j * cols + i;
      if (!res.includes(c)) res.push(c);
    }
    return res;
  };

  // Items: footprints, heights and containment.
  const V: (VItem | null)[] = [];
  const cov: (number[] | null)[] = [];
  for (let k = 0; k < layout.items.length; k++) {
    const it = layout.items[k]!;
    if (it.n !== k) out.push(`item ${k}: n is ${it.n}`);
    const rawG = (it as LayoutItem & { g?: number }).g;
    const g = Number.isInteger(rawG) && rawG! >= 0 && rawG! < groups.length ? rawG! : -1;
    if (g < 0) out.push(`item ${k} (${it.id}): group ${rawG} is not an index of layout.groups`);
    if (!has(catalog.items, it.id)) {
      out.push(`item ${k}: unknown id "${it.id}"`);
      V.push(null);
      cov.push(null);
      continue;
    }
    const cat = catalog.items[it.id]!;
    const kind = kindOfCat(cat);
    const label = `item ${k} (${it.id})`;
    if (![0, 90, 180, 270].includes(it.rot)) out.push(`${label}: rot ${it.rot} is not 0/90/180/270`);
    if (it.host !== null && !(Number.isInteger(it.host) && it.host >= 0 && it.host < k)) {
      out.push(`${label}: host ${it.host} is not an earlier item`);
    }
    const fw = cat.size[0]!;
    const fd = kind === 'wall' ? WALLMOUNT_DEPTH : cat.size[2]!;
    const along = ((it.rot % 180) + 180) % 180 === 0;
    const sx = along ? fw : fd;
    const sz = along ? fd : fw;
    const v: VItem = {
      k, id: it.id, cat, kind, g, x: it.x, y: it.y, z: it.z, rot: it.rot, host: it.host,
      fw, fd,
      x0: it.x - sx / 2, x1: it.x + sx / 2, z0: it.z - sz / 2, z1: it.z + sz / 2,
      top: it.y + cat.size[1]!,
    };
    V.push(v);

    const host = it.host !== null ? V[it.host]! : null;
    if (kind === 'surface') {
      if (!host) {
        out.push(`${label}: surface item without a host`);
      } else {
        const expY = host.top;
        if (host.g !== v.g) out.push(`${label}: host item ${host.k} is in group ${host.g}, not group ${v.g}`);
        if (Math.abs(it.y - expY) > TOL) out.push(`${label}: y ${it.y}, host top ${expY}`);
        if (v.x0 < host.x0 - TOL || v.x1 > host.x1 + TOL || v.z0 < host.z0 - TOL || v.z1 > host.z1 + TOL) {
          out.push(`${label}: footprint not inside its host footprint`);
        }
      }
      cov.push(null);
      continue;
    }
    const expY = kind === 'wall' ? cat.mount ?? 0 : 0;
    if (Math.abs(it.y - expY) > TOL) out.push(`${label}: y ${it.y}, expected ${expY}`);
    if (v.x0 < -W / 2 - TOL || v.x1 > W / 2 + TOL || v.z0 < -D / 2 - TOL || v.z1 > D / 2 + TOL) {
      out.push(`${label}: footprint outside the room`);
    }
    cov.push(covered(v.x0, v.x1, v.z0, v.z1));
  }

  // Occupancy: solids (walk included) occupy; non-walk solids block.
  const blk = new Uint8Array(N);
  const solids: number[] = [];
  for (const v of V) {
    if (!v || v.kind !== 'solid') continue;
    solids.push(v.k);
    if (v.cat.walk) continue;
    for (const c of cov[v.k]! ?? []) blk[c]! = 1;
  }

  // Door zone and spine: no solid, wall or floor item may cover them.
  for (const v of V) {
    if (!v || v.kind === 'surface') continue;
    for (const c of cov[v.k]! ?? []) {
      if (ds[c]!) {
        out.push(`item ${v.k} (${v.id}): covers the door zone or spine`);
        break;
      }
    }
  }

  // Solid footprints must not overlap each other.
  for (let a = 0; a < solids.length; a++) {
    for (let b = a + 1; b < solids.length; b++) {
      const p = V[solids[a]!]!;
      const q = V[solids[b]!]!;
      if (Math.min(p.x1, q.x1) - Math.max(p.x0, q.x0) > TOL && Math.min(p.z1, q.z1) - Math.max(p.z0, q.z0) > TOL) {
        out.push(`items ${p.k} (${p.id}) and ${q.k} (${q.id}) overlap`);
      }
    }
  }

  // Clear zones stay free of solids, except the item's own attachments.
  for (const v of V) {
    if (!v || (v.kind !== 'solid' && v.kind !== 'wall')) continue;
    const front = v.cat.clear?.front ?? 0;
    const side = v.cat.clear?.side ?? 0;
    if (front <= 0) continue;
    const lx = v.fw / 2 + side;
    const pts = [
      rotated(-lx, v.fd / 2, v.rot),
      rotated(lx, v.fd / 2, v.rot),
      rotated(lx, v.fd / 2 + front, v.rot),
      rotated(-lx, v.fd / 2 + front, v.rot),
    ];
    const xs = pts.map((p) => p[0]!);
    const zs = pts.map((p) => p[1]!);
    const clear = covered(
      v.x + Math.min(...xs), v.x + Math.max(...xs),
      v.z + Math.min(...zs), v.z + Math.max(...zs),
    );
    const inClear = new Uint8Array(N);
    for (const c of clear) inClear[c]! = 1;
    for (const s of solids) {
      const o = V[s]!;
      // A group's own items may share its clear zones (a chair in front of its desk).
      if (o.k === v.k || o.host === v.k || o.g === v.g) continue;
      if ((cov[s]! ?? []).some((c) => inClear[c]!)) {
        out.push(`item ${v.k} (${v.id}): clear zone blocked by item ${o.k} (${o.id})`);
        break;
      }
    }
  }

  // Wall and corner placement is checked on each group's anchor (below).

  // Z-fighting: two non-floor items with coplanar top faces that overlap.
  for (let a = 0; a < V.length; a++) {
    const p = V[a]!;
    if (!p || p.kind === 'floor') continue;
    for (let b = a + 1; b < V.length; b++) {
      const q = V[b]!;
      if (!q || q.kind === 'floor') continue;
      if (Math.abs(p.top - q.top) > TOL) continue;
      if (Math.min(p.x1, q.x1) - Math.max(p.x0, q.x0) > TOL && Math.min(p.z1, q.z1) - Math.max(p.z0, q.z0) > TOL) {
        out.push(`items ${p.k} (${p.id}) and ${q.k} (${q.id}): coplanar tops overlap`);
      }
    }
  }

  // Reachability: BFS over non-blocking cells from the door zone.
  const vis = new Uint8Array(N);
  const queue: number[] = [];
  for (const c of doorCells) {
    if (blk[c]!) continue;
    vis[c]! = 1;
    queue.push(c);
  }
  for (let h = 0; h < queue.length; h++) {
    const c = queue[h]!;
    const i = c % cols;
    const j = (c - i) / cols;
    const nb = [i > 0 ? c - 1 : -1, i < cols - 1 ? c + 1 : -1, j > 0 ? c - cols : -1, j < rows - 1 ? c + cols : -1];
    for (const n of nb) {
      if (n < 0 || vis[n]! || blk[n]!) continue;
      vis[n]! = 1;
      queue.push(n);
    }
  }
  const reach = (x: number, z: number): boolean => windowCells(x, z).some((c) => vis[c]! === 1);

  // Access cells of solid and wall items.
  for (const v of V) {
    if (!v || (v.kind !== 'solid' && v.kind !== 'wall')) continue;
    if (v.cat.walk) {
      if (!reach(v.x, v.z)) out.push(`item ${v.k} (${v.id}): centre (walk) unreachable from the door`);
      continue;
    }
    const front = v.cat.clear?.front ?? 0;
    const [dx, dz] = rotated(0, v.fd / 2 + Math.max(front, MIN_ACCESS) / 2, v.rot);
    if (!reach(v.x + dx, v.z + dz)) out.push(`item ${v.k} (${v.id}): front access unreachable from the door`);
  }

  // Spots: inside the room, reachable when they belong to a solid or wall item, and matching the catalogue.
  const expected: { item: number; x: number; z: number; face: number }[] = [];
  for (const v of V) {
    if (!v) continue;
    for (const s of v.cat.spots ?? []) {
      const [dx, dz] = rotated(s.at[0]!, s.at[1]!, v.rot);
      expected.push({ item: v.k, x: v.x + dx, z: v.z + dz, face: ((((s.face + v.rot) % 360) + 360) % 360) });
    }
  }
  if (expected.length !== layout.spots.length) {
    out.push(`spots: ${layout.spots.length} listed, catalogue gives ${expected.length}`);
  }
  for (let q = 0; q < Math.min(expected.length, layout.spots.length); q++) {
    const e = expected[q]!;
    const s = layout.spots[q]!;
    if (s.item !== e.item || Math.abs(s.x - e.x) > SPOT_TOL || Math.abs(s.z - e.z) > SPOT_TOL || s.face !== e.face) {
      out.push(`spot ${q}: item ${s.item} at (${s.x}, ${s.z}) face ${s.face}, catalogue gives item ${e.item} at (${e.x.toFixed(3)}, ${e.z.toFixed(3)}) face ${e.face}`);
    }
  }
  // A robot spot stays 0.15 off each wall that exists (the room edge where there is none), lies inside the
  // grid, and never sits on a blocking cell (walk items may be stood on).
  const inWalls = (side: Door): boolean => sh.walls.includes(side);
  const spotX0 = (inWalls('w') ? -W / 2 + 0.15 : -W / 2) - TOL;
  const spotX1 = (inWalls('e') ? W / 2 - 0.15 : W / 2) + TOL;
  const spotZ0 = (inWalls('n') ? -D / 2 + 0.15 : -D / 2) - TOL;
  const spotZ1 = (inWalls('s') ? D / 2 - 0.15 : D / 2) + TOL;
  for (const s of layout.spots) {
    const owner = V[s.item] ?? null;
    const label = owner ? `spot of item ${s.item} (${owner.id}) at (${s.x}, ${s.z})` : `spot of item ${s.item} at (${s.x}, ${s.z})`;
    if (!(s.x >= spotX0 && s.x <= spotX1 && s.z >= spotZ0 && s.z <= spotZ1)) {
      out.push(`${label}: outside the room (robot spots keep 0.15 from each wall)`);
      continue;
    }
    const gi = Math.floor((s.x + W / 2) / cell);
    const gj = Math.floor((s.z + D / 2) / cell);
    if (gi < 0 || gi >= cols || gj < 0 || gj >= rows) {
      out.push(`${label}: outside the grid (in the strip beyond the last whole cell)`);
      continue;
    }
    const cells = windowCells(s.x, s.z);
    if (cells.every((c) => blk[c] === 1)) {
      out.push(`${label}: lies on a blocking cell (inside a solid footprint)`);
      continue;
    }
    if (!owner || (owner.kind !== 'solid' && owner.kind !== 'wall')) continue;
    if (!reach(s.x, s.z)) out.push(`${label}: unreachable from the door`);
  }

  // v2 purpose rules. The first item of each group instance is its anchor (items are in placement order).
  const anchorOf = new Map<number, number>();
  for (const v of V) if (v && v.g >= 0 && !anchorOf.has(v.g)) anchorOf.set(v.g, v.k);
  const groupOf = (v: VItem): string | undefined => (v.g >= 0 ? groups[v.g] : undefined);
  const defOf = (v: VItem): CatGroup | undefined => {
    const id = groupOf(v);
    return id !== undefined && has(catGroups, id) ? catGroups[id] : undefined;
  };
  const gapsOf = (v: VItem): Record<Door, number> => ({
    n: v.z0 + D / 2,
    s: D / 2 - v.z1,
    w: v.x0 + W / 2,
    e: W / 2 - v.x1,
  });
  const touches = (v: VItem, side: Door): boolean => gapsOf(v)[side] <= ANCHOR_TOUCH + TOL;
  /** The item's back is within reach of the wall on the side its rotation faces away from. */
  const againstOwnWall = (v: VItem): boolean => {
    const side = sideOfRot(v.rot);
    return side !== undefined && edges.includes(side) && touches(v, side);
  };

  // Anchors: a wall anchor backs onto a wall and faces into the room; a corner anchor touches two room edges;
  // a door anchor stands at the door's edge. Wall-mounted items back onto their own wall.
  for (const v of V) {
    if (!v) continue;
    const label = `item ${v.k} (${v.id})`;
    const def = defOf(v);
    if (anchorOf.get(v.g) === v.k && def) {
      if (def.anchor === 'wall' && !againstOwnWall(v)) {
        out.push(`${label}: wall anchor's back is not within ${ANCHOR_TOUCH} of the wall on its own side (rot ${v.rot})`);
      } else if (def.anchor === 'corner') {
        const ns = edges.some((s) => (s === 'n' || s === 's') && touches(v, s));
        const ew = edges.some((s) => (s === 'e' || s === 'w') && touches(v, s));
        if (!(ns && ew)) out.push(`${label}: corner anchor is not within ${ANCHOR_TOUCH} of two room edges`);
      } else if (def.anchor === 'door' && !touches(v, door)) {
        out.push(`${label}: door anchor is not within ${ANCHOR_TOUCH} of the door edge`);
      }
    }
    if (v.kind === 'wall' && !againstOwnWall(v)) {
      out.push(`${label}: wall-mounted item is not within ${ANCHOR_TOUCH} of the wall on its own side (rot ${v.rot})`);
    }
  }

  // Seats sit in a purpose group and face the group's anchor (a meeting chair faces its table).
  for (const v of V) {
    if (!v || !SEAT_IDS.includes(v.id)) continue;
    const label = `item ${v.k} (${v.id})`;
    const id = groupOf(v);
    if (id === undefined || !SEAT_GROUPS.includes(id)) {
      out.push(`${label}: seat is not in a desk-set, meeting, station or reading-nook group`);
      continue;
    }
    const a = anchorOf.get(v.g);
    if (a === undefined || a === v.k) continue;
    const A = V[a];
    if (!A) continue;
    const ang = angleDeg(frontOf(v.rot), [A.x - v.x, A.z - v.z]);
    if (ang > SEAT_FACE_TOL + TOL) {
      out.push(`${label}: faces ${ang.toFixed(1)} degrees off its anchor item ${A.k} (${A.id}), limit ${SEAT_FACE_TOL}`);
    }
  }

  // Relations sit on their side of the anchor, in the anchor's own frame (the offset is turned back by the anchor's
  // rot first): front = +z, beside = -x, beside-other = +x, row = +x along the anchor's row. Tolerance 0.1 on the
  // other axis. Items matched to their catalogue entry by id; a repeated id takes its first entry.
  const REL_TOL = 0.1;
  for (const v of V) {
    if (!v || v.g < 0 || anchorOf.get(v.g) === v.k) continue;
    const def = defOf(v);
    const A = V[anchorOf.get(v.g) ?? -1];
    if (!def || !A) continue;
    const entry = def.items.find((e) => e.id === v.id && e.rel !== undefined && e.rel !== 'around' && e.rel !== 'mirror' && e.rel !== 'on');
    if (!entry) continue;
    const [lx, lz] = rotated(v.x - A.x, v.z - A.z, (360 - A.rot) % 360);
    const rel = entry.rel;
    const ok =
      rel === 'front' ? lz > 0 && Math.abs(lx) <= REL_TOL :
      rel === 'beside' ? lx < 0 && Math.abs(lz) <= REL_TOL :
      rel === 'beside-other' || rel === 'row' ? lx > 0 && Math.abs(lz) <= REL_TOL :
      true;
    if (!ok) out.push(`item ${v.k} (${v.id}): not on the ${rel} side of its anchor ${A.k} (${A.id}) in the anchor's frame (local ${lx.toFixed(3)}, ${lz.toFixed(3)})`);
  }

  // The middle region (|x| <= W/4, |z| <= D/4) belongs to centre groups only.
  for (const v of V) {
    if (!v || v.cat.layer === 'floor') continue;
    if (Math.abs(v.x) > W / 4 + TOL || Math.abs(v.z) > D / 4 + TOL) continue;
    if (defOf(v)?.anchor !== 'center') {
      out.push(`item ${v.k} (${v.id}): in the middle of the room but its group is not a centre group`);
    }
  }

  // A wall-mounted item's view strip (from its front face out MOUNT_VIEW, its width plus 0.1 each side) holds no
  // solid that rises above the mount's middle.
  for (const wm of V) {
    if (!wm || wm.kind !== 'wall') continue;
    const m = wm.cat.mount ?? 0;
    const h = wm.cat.size[1];
    const half = wm.fw / 2 + 0.1;
    const pts = [
      rotated(-half, wm.fd / 2, wm.rot),
      rotated(half, wm.fd / 2, wm.rot),
      rotated(half, wm.fd / 2 + MOUNT_VIEW, wm.rot),
      rotated(-half, wm.fd / 2 + MOUNT_VIEW, wm.rot),
    ];
    const sx0 = wm.x + Math.min(...pts.map((p) => p[0]));
    const sx1 = wm.x + Math.max(...pts.map((p) => p[0]));
    const sz0 = wm.z + Math.min(...pts.map((p) => p[1]));
    const sz1 = wm.z + Math.max(...pts.map((p) => p[1]));
    for (const s of V) {
      if (!s || s.kind !== 'solid') continue;
      if (s.x0 < sx0 - TOL || s.x1 > sx1 + TOL || s.z0 < sz0 - TOL || s.z1 > sz1 + TOL) continue;
      if (s.top > m + h / 2 + TOL) {
        out.push(`item ${wm.k} (${wm.id}): view blocked by item ${s.k} (${s.id}), top ${s.top} above ${m + h / 2}`);
      }
    }
  }

  // Spots face their item within SPOT_FACE_TOL (a spot on the item's own centre has no direction to test).
  for (const s of layout.spots) {
    const owner = V[s.item];
    if (!owner) continue;
    const dx = owner.x - s.x;
    const dz = owner.z - s.z;
    if (Math.hypot(dx, dz) <= 0.01) continue;
    const ang = angleDeg(frontOf(s.face), [dx, dz]);
    if (ang > SPOT_FACE_TOL + TOL) {
      out.push(`spot of item ${s.item} (${owner.id}) at (${s.x}, ${s.z}): faces ${ang.toFixed(1)} degrees off its item, limit ${SPOT_FACE_TOL}`);
    }
  }

  // Required groups present, with the counts the template asks for (per "agent" entries scale with the desk count).
  const agents = input.agents ?? Math.min(4, Math.max(1, Math.floor((W * D) / 6)));
  const req: ReqEntry[] = tpl.required;
  const need = new Map<string, number>();
  for (const e of req) {
    if (!e.group) {
      out.push(`template "${input.role}": required entry without a group`);
      continue;
    }
    need.set(e.group, (need.get(e.group) ?? 0) + (e.per === 'agent' ? agents : 1));
  }
  need.forEach((asked, gid) => {
    const actual = groups.filter((x) => x === gid).length;
    if (actual < asked) out.push(`required group "${gid}": ${actual} placed, ${asked} asked for`);
  });

  return out;
}
