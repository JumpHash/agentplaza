// The places the behaviour engine sends robots to, in the floor's world frame: x and z on the plaza (the robot's own
// position is group.position (x, y, z) with z = the floor's y). Region 0 is the board; a room's region is its id
// (regions count from 1 in the order the rooms are built). Pure: no three.js, no DOM.
//
// A spot's face is a heading in radians in the floor's own convention: 0 faces +z (the robot's forward), +x is
// atan2's first argument, the same as the floor's robot heading. Room spots are authored in degrees in the room's frame
// (rooms.md): they are turned to the world by the room's yaw. Rules: docs/design/robots.md ("Places").

import type { SpotIn } from './behaviour';
import { boardHeightAt, type BoardDistrict, type BoardLayout, type District, type TraceGraph } from './board-layout';
import type { Model, Params } from './hardware';
import { partBoxes, type PartBox } from './zfight';

/** A robot's footprint radius (the floor's ROBOT_R): how far a station stands off a part. */
export const CLEAR = 0.55;
/** The grid of free points on the board (anywhere spots). */
const ANY_STEP = 2.5;
/** A part this far above the PCB is a step or taller (nav.md: the step height is 0.25). */
const STEP_HEIGHT = 0.25;
const DEG = Math.PI / 180;

/** A room's station, as rooms build it (LayoutSpot in rooms/gen.ts): in the room's own frame, the face in degrees. */
export interface RoomSpot {
  item: number;
  x: number;
  z: number;
  face: number;
  clip: string | null;
}

/** A furniture item of a room layout: `n` is the number the spots name, `id` the catalogue id (a prop id). */
export interface RoomItem {
  n: number;
  id: string;
}

/** A room as the engine sees it: its region, its district and its stations in the world frame. */
export interface RoomFrame {
  region: number;
  d: BoardDistrict;
  spots: SpotIn[];
}

/** The key of a district: the same across layout rebuilds that keep the district (role and place). */
export const districtKey = (d: District): string => `${d.role}|${d.cx},${d.cy}`;

/** Whether (x, z) lies in a district's cell, grown by `margin` (the cell test the floor's roomAt uses). */
function inCell(d: BoardDistrict, x: number, z: number, margin: number): boolean {
  return Math.abs(x - d.cx) <= d.w / 2 + margin && Math.abs(z - d.cy) <= d.d / 2 + margin;
}

/** The region of the point (x, z): the room whose cell holds it, else 0 (the board). */
export function regionOf(rooms: Iterable<RoomFrame>, x: number, z: number): number {
  for (const r of rooms) if (inCell(r.d, x, z, 0)) return r.region;
  return 0;
}

/** A room's stations in the world frame: `turn` is the room group's yaw (rotation.y), the room's origin is its district's centre. */
export function roomSpots(region: number, d: BoardDistrict, turn: number, spots: RoomSpot[], items: RoomItem[]): SpotIn[] {
  const idOf = new Map<number, string>();
  for (const it of items) idOf.set(it.n, it.id);
  const c = Math.cos(turn);
  const s = Math.sin(turn);
  return spots.map((sp, i) => ({
    key: `room${region}:${i}`,
    kind: `prop:${idOf.get(sp.item) ?? 'item'}`,
    region,
    x: d.cx + sp.x * c + sp.z * s,
    z: d.cy - sp.x * s + sp.z * c,
    face: sp.face * DEG + turn,
    clip: sp.clip,
  }));
}

/** One desk per agent: its console slot in its room, facing the room's centre (the desk's own face is not authored). */
export function deskSpots(homes: BoardLayout['homes'], regionFor: (d: District) => number): SpotIn[] {
  const out: SpotIn[] = [];
  for (const [cid, [d, x, z]] of homes) {
    out.push({ key: `desk:${cid}`, kind: 'desk', region: regionFor(d), x, z, face: Math.atan2(d.cx - x, d.cy - z), clip: 'type' });
  }
  return out;
}

/** Whether a point is free for a robot to stand on the board: off every room's cell, on the PCB top, off the tall parts. */
function standable(layout: BoardLayout, boxes: PartBox[], x: number, z: number): boolean {
  for (const d of layout.districts) if (inCell(d, x, z, CLEAR)) return false;
  if (Math.abs(boardHeightAt(x, z, layout) - layout.pcbTop) > 1e-6) return false;
  for (const b of boxes) {
    if (b.hi[1] <= layout.pcbTop + STEP_HEIGHT) continue; // decals and flat parts: walkable
    if (x > b.lo[0] - CLEAR && x < b.hi[0] + CLEAR && z > b.lo[2] - CLEAR && z < b.hi[2] + CLEAR) return false;
  }
  return true;
}

/**
 * The board's own spots: components (on the edges of each slot, facing it; a slot under a room has none, since the
 * room stands there), commons (a ring round the atom), docks (one per room, at its rim), traces (the walkway points) and
 * anywhere (a grid of free points). All on region 0.
 */
export function boardSpots(model: Model, params: Params, graph: TraceGraph, layout: BoardLayout): SpotIn[] {
  const out: SpotIn[] = [];
  const boxes = partBoxes(model, params);
  // Components: four stations per slot, one off each edge, facing the slot's centre.
  for (const slot of model.slots ?? []) {
    const [x0, z0, w, d] = slot.rect;
    const cx = x0 + w / 2;
    const cz = z0 + d / 2;
    const edge: [number, number][] = [
      [cx, z0 - CLEAR],
      [cx, z0 + d + CLEAR],
      [x0 - CLEAR, cz],
      [x0 + w + CLEAR, cz],
    ];
    edge.forEach(([x, z], k) => {
      if (layout.districts.some((r) => inCell(r, x, z, 0))) return;
      out.push({ key: `slot:${slot.id}:${k}`, kind: `component:${slot.type}`, region: 0, x, z, face: Math.atan2(cx - x, cz - z), clip: null });
    });
  }
  // Commons: two rings round the atom (its benches and the meeting table stand on them).
  const [ax, , az] = layout.atom;
  for (let i = 0; i < 8; i++) {
    for (const r of [3, 4.5]) {
      const a = (i * Math.PI) / 4;
      const x = ax + Math.cos(a) * r;
      const z = az + Math.sin(a) * r;
      if (!standable(layout, boxes, x, z)) continue;
      out.push({ key: `commons:${i}:${r}`, kind: 'commons', region: 0, x, z, face: Math.atan2(ax - x, az - z), clip: null });
    }
  }
  // Docks: the first free point round each room's rim (the charging pad).
  layout.districts.forEach((d, i) => {
    for (let k = 0; k < 8; k++) {
      const a = (k * Math.PI) / 4;
      const x = d.cx + Math.cos(a) * (d.r + 0.6);
      const z = d.cy + Math.sin(a) * (d.r + 0.6);
      if (!standable(layout, boxes, x, z)) continue;
      out.push({ key: `dock:${i}`, kind: 'dock', region: 0, x, z, face: Math.atan2(d.cx - x, d.cy - z), clip: null });
      return;
    }
  });
  // Traces: the walkway points, facing the next point of the walkway.
  graph.nodes.forEach(([x, z], i) => {
    if (layout.districts.some((r) => inCell(r, x, z, 0.3))) return;
    const next = graph.edges[i]?.[0];
    const face = next ? Math.atan2(graph.nodes[next.to]![0] - x, graph.nodes[next.to]![1] - z) : 0;
    out.push({ key: `trace:${i}`, kind: 'trace', region: 0, x, z, face, clip: null });
  });
  // Anywhere: a grid of free points over the board's footprint.
  const b = layout.bounds;
  for (let ix = 0; ix * ANY_STEP < b.w; ix++) {
    for (let iz = 0; iz * ANY_STEP < b.d; iz++) {
      const x = b.cx - b.w / 2 + ANY_STEP / 2 + ix * ANY_STEP;
      const z = b.cz - b.d / 2 + ANY_STEP / 2 + iz * ANY_STEP;
      if (!standable(layout, boxes, x, z)) continue;
      out.push({ key: `any:${ix},${iz}`, kind: 'anywhere', region: 0, x, z, face: 0, clip: null });
    }
  }
  return out;
}
