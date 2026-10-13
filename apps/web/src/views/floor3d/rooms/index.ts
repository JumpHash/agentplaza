// District rooms. A room is generated (gen.ts) from the catalogue and its role's template, then built (build.ts)
// into a group in the room's local frame. The generator is deterministic: the same role, size, door and seed give
// the same layout in the desktop app too (docs/design/rooms.md).

import * as THREE from 'three';
import catalogFile from '../../../../../../assets/rooms/catalog.json';
import rolesFile from '../../../../../../assets/rooms/roles.json';
import { roleLabel } from '../../../roles';
import { buildLayout, type RoomSpot } from './build';
import { generate, type Catalog, type Door, type Layout, type Roles } from './gen';
import minSizeTable from '../../../../../../assets/rooms/minsize.json';
import type { KitPalette } from './kit';

const CATALOG = catalogFile as unknown as Catalog;
export const ROOM_CATALOG = CATALOG;
const ROLES = rolesFile as unknown as Roles;
/** The largest side a room is built at when its role needs more space than the slot gives. */
const MAX_SIDE = 8;
/** Walls take this much off the cell's side: the default inner size is 2s - WALL_MARGIN. */
const WALL_MARGIN = 0.15;

export type { RoomSpot } from './build';

/** The room's shape as the board sees it: rooms are square (only a central seat is round). */
export function roomStyle(_role: string): { shape: 'square' } {
  return { shape: 'square' };
}

export interface BuiltRoom {
  /** Furniture, walls and sign, in the room's local frame (rotate it to face the plaza). */
  group: THREE.Group;
  /** The floor's top surface material. */
  floor: THREE.Material;
  /** Monitor materials that light up while the room's agents work. */
  screens: THREE.MeshToonMaterial[];
  animate: (t: number) => void;
}

export interface RoomInput {
  /** Inner size (x by z) in units. */
  w: number;
  d: number;
  /** The side facing the walkway: n = -z, e = +x, s = +z, w = -x. */
  door: Door;
  seed: number;
  /** Desk count; default cap(w, d) = clamp(floor(w * d / 6), 1, 4). */
  agents?: number;
}

/** The desk count a room of this size holds (the generator's default). */
function capFor(w: number, d: number): number {
  return Math.min(4, Math.max(1, Math.floor((w * d) / 6)));
}

/** The room's layout: the generator with the catalogue and roles, the desk count defaulting to the room's cap. */
export function roomLayout(role: string, input: RoomInput, catalog: Catalog = CATALOG, roles: Roles = ROLES): Layout {
  const agents = input.agents ?? capFor(input.w, input.d);
  const cached = catalog === CATALOG && roles === ROLES; // the default catalog only: one layout per room, shared (navSteps, districtMesh)
  const key = `${role}|${input.w}|${input.d}|${input.door}|${input.seed}|${agents}`;
  if (cached) {
    const hit = layoutMemo.get(key);
    if (hit) return hit;
  }
  const layout = generate({ role, w: input.w, d: input.d, door: input.door, seed: input.seed, agents }, catalog, roles);
  if (cached) {
    layoutMemo.set(key, layout);
    if (layoutMemo.size > LAYOUT_MEMO_MAX) layoutMemo.delete(layoutMemo.keys().next().value!);
  }
  return layout;
}

const LAYOUT_MEMO_MAX = 96;
const layoutMemo = new Map<string, Layout>();

/**
 * The minimum side of a role and desk count, from the table (assets/rooms/minsize.json). A role the table does not
 * list has no known minimum (0): computing one generates thousands of rooms and would stall the page, so it is not done here.
 */
function minSideFor(role: string, agents: number): number {
  const row = (minSizeTable as Record<string, (number | null)[]>)[role];
  if (!row) return 0;
  const v = row[Math.min(agents, row.length) - 1];
  if (typeof v === 'number') return v;
  return v === null ? Infinity : 0;
}

export interface RoomOptions {
  /** The layout seed (the floor passes fnv1a32 of machine, role and room index). Default 1. */
  seed?: number;
  /** The walkway side. Default 's'. */
  door?: Door;
  /** The inner size; default 2s - 0.15 (the slot minus the wall margin). */
  w?: number;
  d?: number;
  /** The desks the room houses: the board's agent count for it. Default cap of the slot's size. */
  agents?: number;
}

export type BuiltRoomLayout = BuiltRoom & {
  spots: RoomSpot[];
  layout: Layout;
  /** True when the slot was smaller than the role's minimum (the room is built at the slot size anyway). */
  warned: boolean;
  /** The size the room was built at: the slot's size. */
  w: number;
  d: number;
};

/**
 * Builds a role's room in a slot of half-size `s`, at the slot's size (w and d, default the cell). A slot smaller than
 * the role's minimum (minSize for the board's desk count) is NOT enlarged: enlarging would overlap the neighbouring
 * slots. Such a room is built at its slot size anyway, its required items may not place, and a warning names the
 * role, the slot and the minimum so the board's slot sizes can be raised.
 */
export function buildRoom(role: string, s: number, hue: number, palette: KitPalette, opts: RoomOptions = {}): BuiltRoomLayout {
  const cell = 2 * s - WALL_MARGIN;
  const w0 = opts.w ?? cell;
  const d0 = opts.d ?? cell;
  const door = opts.door ?? 's';
  const seed = opts.seed ?? 1;
  // The desk count is the board's agent count for this room (not the enlarged size); it drives both the layout and minSize.
  const agents = Math.max(1, Math.floor(opts.agents ?? capFor(w0, d0)));
  const need = Math.min(minSideFor(role, agents), MAX_SIDE);
  // The minimum is a cell size: compare the cell (2 * s), not the room inside its wall margin.
  const warned = 2 * s < need - 1e-9;
  if (warned) {
    console.warn(`rooms: ${role} slot ${s.toFixed(2)} (${w0.toFixed(2)} x ${d0.toFixed(2)}) is below its minimum ${need} (${agents} desks); built at the slot size, not enlarged`);
  }
  const w = w0;
  const d = d0;
  const layout = roomLayout(role, { w, d, door, seed, agents }, CATALOG, ROLES);
  const built = buildLayout(layout, CATALOG, hue, palette, w, d, s, roleLabel(role));
  return { group: built.group, floor: built.floor, screens: built.screens, animate: built.animate, spots: built.spots, layout, warned, w, d };
}
