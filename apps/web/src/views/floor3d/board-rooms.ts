// Rooms on a board, for the hardware lab's test hook (#/hardware-lab?model=board/<id>&rooms=<n>). The layout is
// layoutOnBoard (its occlusion and its joins), and each district's room is built the way the Floor builds it
// (floor.ts districtMesh): buildRoom at the district's cell, turned to face the board's centre (square rooms in
// quarter turns), its door on the side nearest a trace point, its seed from the district. The Floor's plates, walkways
// and lighting are not drawn here.

import * as THREE from 'three';
import type { Agent } from '../../api';
import { layoutOnBoard, traceGraph, type BoardDistrict, type BoardLayout, type TraceGraph } from './board-layout';
import { buildModel, traceStrips, type Model, type Params } from './hardware';
import { GROUND_TOL, PLATE, topBelow, type Support } from './ground';
import { buildRoom } from './rooms';
import { fnv1a32 } from './rooms/gen';
import { slabSide } from './textures';
import { outline } from './toon';
import type { KitPalette } from './rooms/kit';

/** The roster of docs/agents/initial-roles.json in roster order (its slot numbers 0..16). */
export const ROSTER: readonly string[] = [
  'architect', 'auditor', 'builder', 'edge-operator', 'finance', 'incident', 'librarian', 'mayor', 'mayor-office',
  'orchestrator', 'planner', 'qa', 'release', 'researcher', 'reviewer', 'security-reviewer', 'breeder',
];

/** n synthetic agents: one per roster role in roster order, then the roster again. The orchestrator has its own tier. */
export function syntheticAgents(n: number): Agent[] {
  return Array.from({ length: Math.max(0, n) }, (_, i) => {
    const role = ROSTER[i % ROSTER.length]!;
    return { cid: 900 + i, name: '', role, tier: role === 'orchestrator' ? 'orchestrator' : 'worker' } as unknown as Agent;
  });
}

/** The page's palette from its CSS variables, as floor.ts readPalette reads it. */
interface PagePalette { sheet: string; rule: string; ink2: string; signal: string }
function readPagePalette(): PagePalette {
  const cs = getComputedStyle(document.documentElement);
  const get = (name: string, fallback: string): string => cs.getPropertyValue(`--${name}`).trim() || fallback;
  return { sheet: get('sheet', '#ffffff'), rule: get('rule', '#d6d2c8'), ink2: get('ink-2', '#4a4a4a'), signal: get('signal', '#d9482b') };
}

/** The room palette of the Floor: its rooms keep their light paper under the night lighting (floor.ts roomPalette). */
export function roomPalette(page: PagePalette = readPagePalette()): KitPalette & { ink2: THREE.Color } {
  return {
    sheet: new THREE.Color(0xe9edf7),
    rule: new THREE.Color(0xa9b4cc),
    ink2: new THREE.Color(page.ink2),
    ink3: new THREE.Color(0x7d8bb5),
    signal: new THREE.Color(page.signal),
    ok: new THREE.Color(0x3ee08f),
    info: new THREE.Color(0x4cc9ff),
  };
}

/** The floor's plaque: a rounded sheet with the role name (floor.ts plaqueTexture). */
function plaqueTexture(text: string, orch: boolean, p: KitPalette & { ink2: THREE.Color }): THREE.CanvasTexture {
  if (typeof document === 'undefined') return new THREE.CanvasTexture(undefined as unknown as HTMLCanvasElement); // no page (the tests): never drawn
  const cv = document.createElement('canvas');
  cv.width = 512;
  cv.height = 128;
  const g = cv.getContext('2d')!;
  g.beginPath();
  g.moveTo(24, 4);
  g.arcTo(508, 4, 508, 124, 20);
  g.arcTo(508, 124, 4, 124, 20);
  g.arcTo(4, 124, 4, 4, 20);
  g.arcTo(4, 4, 508, 4, 20);
  g.closePath();
  g.fillStyle = `#${p.sheet.getHexString()}`;
  g.fill();
  g.lineWidth = 6;
  g.strokeStyle = orch ? `#${p.signal.getHexString()}` : `#${p.rule.getHexString()}`;
  g.stroke();
  g.fillStyle = `#${p.ink2.getHexString()}`;
  g.textAlign = 'center';
  g.textBaseline = 'middle';
  const base = '500 48px ui-monospace, SFMono-Regular, Menlo, monospace';
  g.font = base;
  const w = g.measureText(text).width;
  if (w > 460) g.font = `500 ${Math.floor((48 * 460) / w)}px ui-monospace, SFMono-Regular, Menlo, monospace`;
  g.fillText(text, 256, 66);
  const tex = new THREE.CanvasTexture(cv);
  tex.colorSpace = THREE.SRGBColorSpace;
  return tex;
}

/** The terrace's sloped band round a room (floor.ts RAMP). */
const RAMP = 1;

/** The terrace plate's thickness at least: its top is the slot's top, its underside rests on the highest part under it. */
export { PLATE };
/** The plate's columns stand on a grid of this side; a cell gets a column when some of it stands over lower ground. */
const COLUMN_CELL = 0.5;

/**
 * A resting slab: a square or round plate (half sizes hw, hd about (cx, cz)) with its top at `top` in the hue `hue`. Its
 * underside is the highest support under it, never less than PLATE under the top, and every cell that stands over lower
 * ground gets a column down to what is under its own footprint (the points a part's bottom is tested at: the centre and
 * four inset corners). With no support (no board), the slab is solid down to its PLATE depth. `role` names the meshes.
 */
export interface Slab {
  cx: number;
  cz: number;
  hw: number;
  hd: number;
  round: boolean;
  top: number;
  hue: number;
  role: string;
}

export function restingSlab(s: Slab, support: Support | null): THREE.Group {
  const g = new THREE.Group();
  const { cx, cz, hw, hd, round, top } = s;
  // The columns: a cell is split into four while the support heights under it differ by more than GROUND_TOL, down to
  // COLUMN_CELL (a finer split was measured on every board, and it made more floats than it removed).
  const COLUMN_MIN = COLUMN_CELL;
  const nx = Math.max(1, Math.ceil((2 * hw) / COLUMN_CELL));
  const nz = Math.max(1, Math.ceil((2 * hd) / COLUMN_CELL));
  const csx = (2 * hw) / nx;
  const csz = (2 * hd) / nz;
  const leaves: { x: number; z: number; sx: number; sz: number; sup: number }[] = [];
  const split = (x: number, z: number, sx: number, sz: number): void => {
    if (round && Math.hypot(x - cx, z - cz) > hw) return;
    const k = (Math.min(sx, sz) - 0.04) * 0.4; // the column's own footprint, inset as the audit's sample points are
    const pts: [number, number][] = [[x, z], [x - k, z - k], [x + k, z - k], [x - k, z + k], [x + k, z + k]];
    const heights = pts.map(([px, pz]) => topBelow(support, px, pz, top));
    const hi = Math.max(...heights);
    const lo = Math.min(...heights);
    if (hi - lo > GROUND_TOL && Math.max(sx, sz) > COLUMN_MIN + 1e-9) {
      const qx = sx / 4;
      const qz = sz / 4;
      for (const [dx, dz] of [[-qx, -qz], [qx, -qz], [-qx, qz], [qx, qz]] as const) split(x + dx, z + dz, sx / 2, sz / 2);
      return;
    }
    leaves.push({ x, z, sx, sz, sup: hi });
  };
  for (let i = 0; i < nx; i++) {
    for (let j = 0; j < nz; j++) split(cx - hw + (i + 0.5) * csx, cz - hd + (j + 0.5) * csz, csx, csz);
  }
  const highest = leaves.reduce((m, c) => Math.max(m, c.sup), 0);
  const under = Math.min(highest, top - PLATE);
  const thick = top - under;
  const mat = slabSide(s.hue);
  const body = round ? new THREE.CylinderGeometry(hw, hw, thick, 48) : new THREE.BoxGeometry(2 * hw, thick, 2 * hd);
  const slab = new THREE.Mesh(body, mat);
  slab.position.set(cx, under + thick / 2, cz);
  slab.castShadow = true;
  slab.receiveShadow = true;
  slab.userData.rest = true;
  slab.name = `plate ${s.role}`;
  g.add(slab);
  for (const c of leaves) {
    if (c.sup >= under - GROUND_TOL) continue;
    const col = new THREE.Mesh(new THREE.BoxGeometry(c.sx - 0.04, under - c.sup, c.sz - 0.04), mat);
    col.position.set(c.x, (under + c.sup) / 2, c.z);
    col.castShadow = true;
    col.receiveShadow = true;
    col.userData.rest = true;
    col.name = `column ${s.role}`;
    g.add(col);
  }
  return g;
}

/** A terrace plate: the district's slab, its cell's half-size (restingSlab). */
export function platePieces(d: BoardDistrict, support: Support | null): THREE.Group {
  const hs = Math.min(d.w, d.d) / 2;
  return restingSlab({ cx: d.cx, cz: d.cy, hw: hs, hd: hs, round: d.shape === 'round', top: d.y, hue: d.hue, role: d.role }, support);
}

/** The districts of a layout as one group, in the board's frame: each district as floor.ts districtMesh builds it. */
export function roomsOnBoard(layout: BoardLayout, _graph: TraceGraph | undefined, palette: KitPalette & { ink2: THREE.Color }, machineId = 'lab', support: Support | null = null): THREE.Group {
  const group = new THREE.Group();
  const pcb = layout.pcbTop;
  layout.districts.forEach((d: BoardDistrict, index: number) => {
    const g = new THREE.Group();
    const orch = d.role === 'orchestrator';
    const round = d.shape === 'round';
    // The terrace plate rests on what is under it (platePieces); the cell's half-size is its flat top.
    const hs = Math.min(d.w, d.d) / 2;
    g.add(platePieces(d, support));
    // Neon in the district's hue along the terrace's top edge and round its foot.
    const neon = new THREE.MeshBasicMaterial({ color: new THREE.Color().setHSL(d.hue / 360, 0.95, 0.62, THREE.SRGBColorSpace), toneMapped: false });
    for (const [rr, y] of [[hs, d.y + 0.01], [hs + RAMP, pcb + 0.03]] as const) {
      const edge = round
        ? new THREE.Mesh(new THREE.TorusGeometry(rr, 0.045, 6, 72), neon)
        : new THREE.Mesh(new THREE.TorusGeometry(rr * Math.SQRT2, 0.045, 6, 4), neon);
      edge.rotation.set(Math.PI / 2, 0, round ? 0 : Math.PI / 4);
      edge.position.set(d.cx, y, d.cy);
      edge.userData.outline = false;
      edge.userData.decal = true;
      g.add(edge);
    }
    // The room's turn and door come from the layout (board-layout: the door faces the aisle).
    const { turn, door } = d;
    let agents = 0;
    for (const [home] of layout.homes.values()) if (home === d) agents++;
    const seed = fnv1a32(`${machineId}/${d.role}/${index}`);
    const room = buildRoom(d.role, d.r, d.hue, palette, { seed, door, agents: Math.max(1, agents), w: 2 * d.r - 0.15, d: 2 * d.r - 0.15 });
    // The room's floor at the slot's top, and its role plaque on the open front.
    const top = new THREE.Mesh(
      round ? new THREE.CircleGeometry(d.r, 48) : new THREE.PlaneGeometry(Math.max(2 * d.r, room.w + 0.15), Math.max(2 * d.r, room.d + 0.15)),
      room.floor,
    );
    top.rotation.set(-Math.PI / 2, 0, turn);
    top.position.set(d.cx, d.y + 0.004, d.cy);
    top.receiveShadow = true;
    top.userData.decal = true;
    g.add(top);
    room.group.position.set(d.cx, d.y, d.cy);
    room.group.rotation.y = turn;
    room.group.userData.rest = true;
    room.group.name = `room ${d.role}`;
    g.add(room.group);
    const holder = new THREE.Group();
    holder.position.set(d.cx, d.y + 0.012, d.cy);
    holder.rotation.y = Math.atan2(-d.cx, -d.cy);
    const plaque = new THREE.Mesh(
      new THREE.PlaneGeometry(2.6, 0.65),
      new THREE.MeshBasicMaterial({ map: plaqueTexture(d.role.toUpperCase(), orch, palette), transparent: true, depthWrite: false }),
    );
    plaque.rotation.x = -Math.PI / 2;
    plaque.position.set(0, 0, d.r - 0.5);
    plaque.userData.decal = true;
    holder.add(plaque);
    g.add(holder);
    outline(g);
    group.add(g);
  });
  return group;
}

/**
 * A board's scene as the Floor draws it (floor.ts buildBoard and districtMesh), for the grounding audit (ground.ts) and
 * the tests: the board group at the layout's world scale (its hardware, unbaked, and its copper), and beside it, at world
 * scale, the districts, each with its terrace plate resting on the board and its room. The root holds both.
 */
export function boardScene(model: Model, params: Params, layout: BoardLayout, palette: KitPalette & { ink2: THREE.Color }): THREE.Group {
  const root = new THREE.Group();
  const board = new THREE.Group();
  board.scale.setScalar(layout.boardScale);
  const built = buildModel(model, params, { unbaked: true });
  board.add(built.group, traceStrips(model));
  root.add(board);
  root.add(roomsOnBoard(layout, undefined, palette, 'lab', { field: built.support, scale: layout.boardScale }));
  return root;
}

/** The layout and the room group for a board model with n synthetic agents (the lab's hook). */
export function boardWithRooms(model: Model, n: number, params: Params): { layout: BoardLayout; group: THREE.Group } {
  const layout = layoutOnBoard(syntheticAgents(n), model, params);
  return { layout, group: roomsOnBoard(layout, traceGraph(model), roomPalette()) };
}
