// The navigation scene of a board: each district's room plan (the values floor.ts districtMesh builds its room with),
// the terraces, and the nav grid over the board and them. Pure TypeScript (no three.js). The layout decides the rooms'
// turns, doors and plates (board-layout.ts); this file reads them, so the grid and the rendered room agree. The rules of
// the grid are in nav-grid.ts and docs/design/navigation.md.

import { ROOM_CATALOG, roomLayout } from './rooms';
import { fnv1a32, type Door } from './rooms/gen';
import { navBoardOf, traceGraph, type BoardDistrict, type BoardLayout, type TraceGraph } from './board-layout';
import type { Model, Params } from './hardware';
import type { NavGrid, NavPlate, NavTerrace } from './nav-types';
import { buildGrid } from './nav-grid';

/** The room plan of one district: the same values floor.ts uses to build the room, so the nav grid and the room agree. */
export interface RoomPlan {
  turn: number;
  door: Door;
  seed: number;
  agents: number;
  w: number;
  d: number;
}

export interface NavScene {
  grid: NavGrid;
  terraces: NavTerrace[];
  plans: RoomPlan[];
}

/**
 * The room plan of district `d` (its position `index` in layout.districts). The turn and the door are the layout's
 * (BoardDistrict.turn and .door, set by layoutOnBoard from the aisle), so the nav and the floor read the same values.
 * `_graph` is unused: it stays so callers that pass the board's trace graph still compile.
 */
export function roomPlan(
  d: BoardDistrict,
  index: number,
  machineId: string | null,
  layout: Pick<BoardLayout, 'homes'> | null,
  _graph?: TraceGraph,
): RoomPlan {
  const seed = fnv1a32(`${machineId ?? ''}/${d.role}/${index}`);
  let agents = 0;
  for (const [home] of layout?.homes.values() ?? []) if (home === d) agents++;
  const inner = 2 * d.r - 0.15;
  return { turn: d.turn, door: d.door, seed, agents: Math.max(1, agents), w: inner, d: inner };
}

/** A terrace's region id: district i is region i + 1 (0 is the PCB), the same ids on the floor. */
export function terraceId(index: number): number {
  return index + 1;
}

/**
 * The board's plates: the layout's (BoardLayout.plates), one per usable slot, each with its landing. The layout already
 * dropped hidden, occluded and unusable slots. Plate region ids come after the terrace ids (1 .. districts).
 */
export function plateList(_board: Model, layout: BoardLayout): NavPlate[] {
  return layout.plates;
}

/** The board's nav grid with one terrace per district, and each district's room plan. */
export function navScene(board: Model, params: Params, layout: BoardLayout, machineId: string | null): NavScene {
  const graph = traceGraph(board);
  const navBoard = navBoardOf(board, params, layout.pcbTop, plateList(board, layout), layout.boardScale);
  const plans: RoomPlan[] = [];
  const terraces: NavTerrace[] = layout.districts.map((d, i) => {
    const plan = roomPlan(d, i, machineId, layout, graph);
    plans.push(plan);
    return {
      id: terraceId(i),
      cx: d.cx,
      cz: d.cy,
      half: Math.min(d.w, d.d) / 2,
      hw: d.w / 2,
      hd: d.d / 2,
      round: d.shape === 'round',
      y: d.y,
      turn: plan.turn,
      w: plan.w,
      d: plan.d,
      door: plan.door,
      layout: roomLayout(d.role, { w: plan.w, d: plan.d, door: plan.door, seed: plan.seed, agents: plan.agents }, ROOM_CATALOG),
      catalog: ROOM_CATALOG,
    };
  });
  return { grid: buildGrid(navBoard, terraces), terraces, plans };
}
