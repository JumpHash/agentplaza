// The board plaza's layout (pure, no rendering): where each role's room sits on a board model's slots.
// It returns the Floor's own layout shape (districts and homes, as layoutDistricts does) plus each
// slot's height, so the rest of the Floor works unchanged. Desktop mirrors it in floor3d/world/board_layout.rs.

import type { Agent } from '../../api';
import { roleFamily, type Family } from '../../roles';
import { roomStyle } from './rooms';
import { evalWhen, parseWhen, type Model, type Params, type Part, type Slot, type SlotType } from './hardware';

/** A district as the Floor draws it: the plaza x (cx), z (cy), the room's radius, hue and shape. */
export interface District {
  role: string;
  cx: number;
  cy: number;
  r: number;
  hue: number;
  shape: 'square' | 'round';
}

/** A district on a board: its slot's top height, the slot's id and the room's cell size. */
export interface BoardDistrict extends District {
  y: number;
  slot: string;
  w: number;
  d: number;
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
}

/** Where each family's rooms go, in order; each type is one stage, then any slot with room. */
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
 * Splits a rect into n cells, columns along its longer side: the factor pair of n whose cells are
 * nearest square. Cell i is at (i % along, floor(i / along)), counted along the longer side first.
 */
function gridCells(rect: Slot['rect'], n: number): Cell[] {
  if (n <= 0) return [];
  const [x0, z0, W, D] = rect;
  const alongX = W >= D;
  const long = Math.max(1e-6, alongX ? W : D);
  const short = Math.max(1e-6, alongX ? D : W);
  let along = 1;
  let best = Infinity;
  for (let k = 1; k <= n; k++) {
    if (n % k !== 0) continue;
    const score = Math.abs(Math.log((long / k) / (short / (n / k))));
    if (score <= best) {
      best = score;
      along = k;
    }
  }
  const across = n / along;
  const cw = alongX ? W / along : W / across;
  const cd = alongX ? D / across : D / along;
  return Array.from({ length: n }, (_, i) => {
    const a = i % along;
    const b = Math.floor(i / along);
    return alongX
      ? { cx: x0 + (a + 0.5) * cw, cz: z0 + (b + 0.5) * cd, w: cw, d: cd }
      : { cx: x0 + (b + 0.5) * cw, cz: z0 + (a + 0.5) * cd, w: cw, d: cd };
  });
}

/**
 * Lays the roles out on a board model's slots. The orchestrator takes the cpu slot nearest the atom
 * (then soc), in the cell nearest the atom; the other roles follow sorted by name, each in the first
 * slot of its family's type with room, then the fallbacks, then any slot with room. When every slot is
 * full, a role goes to the slot whose rooms stay largest with one more, and the slot's cells shrink.
 */
export function layoutOnBoard(list: Agent[], board: Model, params: Params = {}): BoardLayout {
  const p: Params = { ...(board.params ?? {}), ...params };
  const pcbTop = p.pcb_top ?? 0.4;
  const atom: [number, number, number] = board.anchors?.atom ?? [0, 0, 0];
  const [ax, , az] = atom;

  const hidden = hiddenIds(board.parts, p);
  const visible = (board.slots ?? []).filter((s) => !hidden.has(s.on));
  const slots: Slot[] = visible.length
    ? visible
    : [{ id: '', on: '', type: 'soc', top: pcbTop, rect: [-board.size[0] / 2, -board.size[2] / 2, board.size[0], board.size[2]], capacity: 1 }];
  const load = slots.map(() => 0);
  const hasRoom = (i: number): boolean => load[i]! < slots[i]!.capacity;

  const nearestTo = (idx: number[]): number =>
    idx.reduce((best, i) => (rectDistance(slots[i]!.rect, ax, az) < rectDistance(slots[best]!.rect, ax, az) ? i : best));

  const place = (family: Family, centre: boolean): number => {
    const stages: (SlotType | null)[] = [...FAMILY_TYPES[family], null];
    for (const type of stages) {
      const cands = slots.map((_, i) => i).filter((i) => hasRoom(i) && (type === null || slots[i]!.type === type));
      if (cands.length) return centre ? nearestTo(cands) : cands[0]!;
    }
    // Every slot is full: the one whose rooms are largest with one more.
    let pick = 0;
    let bestArea = -Infinity;
    slots.forEach((s, i) => {
      const area = (s.rect[2] * s.rect[3]) / (load[i]! + 1);
      if (area > bestArea) {
        bestArea = area;
        pick = i;
      }
    });
    return pick;
  };

  interface Seat {
    role: string;
    members: Agent[];
    centre: boolean;
    slot: number;
  }
  const seats: Seat[] = [];
  const centre = list.filter((a) => a.tier === 'orchestrator');
  const groups = new Map<string, Agent[]>();
  for (const a of list) {
    if (a.tier === 'orchestrator') continue;
    groups.set(a.role, [...(groups.get(a.role) ?? []), a]);
  }
  if (centre.length) {
    const slot = place(roleFamily('orchestrator', 'orchestrator'), true);
    load[slot]!++;
    seats.push({ role: 'orchestrator', members: centre, centre: true, slot });
  }
  for (const role of [...groups.keys()].sort()) {
    const slot = place(roleFamily(role), false);
    load[slot]!++;
    seats.push({ role, members: groups.get(role)!, centre: false, slot });
  }

  // Each slot's cells: its capacity, or one per room when it overflows. The orchestrator takes the cell
  // nearest the atom; the other rooms fill the free cells in order.
  const cellsOf = slots.map((s, i) => gridCells(s.rect, Math.max(s.capacity, load[i]!)));
  const taken = slots.map(() => new Set<number>());
  const districts: BoardDistrict[] = [];
  const homes = new Map<number, [District, number, number]>();
  for (const seat of seats) {
    const slot = slots[seat.slot]!;
    const cells = cellsOf[seat.slot]!;
    const free = cells.map((_, i) => i).filter((i) => !taken[seat.slot]!.has(i));
    const pick = seat.centre
      ? free.reduce((best, i) => (Math.hypot(cells[i]!.cx - ax, cells[i]!.cz - az) < Math.hypot(cells[best]!.cx - ax, cells[best]!.cz - az) ? i : best))
      : free[0]!;
    taken[seat.slot]!.add(pick);
    const c = cells[pick]!;
    const d: BoardDistrict = {
      role: seat.role,
      cx: c.cx,
      cy: c.cz,
      r: (Math.min(c.w, c.d) / 2) * 0.9,
      hue: seat.centre ? DAIS_HUE : roleHue(seat.role),
      shape: seat.centre ? 'round' : roomStyle(seat.role).shape,
      y: slot.top,
      slot: slot.id,
      w: c.w,
      d: c.d,
    };
    districts.push(d);
    // Homes: 3 apart in a square grid like the ring layout, closer when the cell is too small for that.
    const members = [...seat.members].sort((a, b) => a.cid - b.cid);
    const side = Math.max(1, Math.ceil(Math.sqrt(members.length)));
    const step = side > 1 ? Math.min(3, (0.8 * Math.min(c.w, c.d)) / (side - 1)) : 0;
    const span = (step * (side - 1)) / 2;
    members.forEach((a, i) => {
      homes.set(a.cid, [d, c.cx - span + (i % side) * step, c.cz - span + Math.floor(i / side) * step]);
    });
  }

  return {
    districts,
    homes,
    atom,
    bounds: { cx: 0, cz: 0, w: board.size[0], d: board.size[2] },
    pcbTop,
  };
}

/**
 * The surface height at a plaza point: a district's slot top inside its cell, a ramp of RAMP units
 * around the cell sloping down to the PCB top, and the PCB top everywhere else.
 */
export function boardHeightAt(x: number, z: number, layout: Pick<BoardLayout, 'districts' | 'pcbTop'>): number {
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
