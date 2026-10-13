// Navigation contracts (pure types and constants; no three.js). The grid, the planner and the followers are
// split over nav-grid.ts, nav-plan.ts and nav-agents.ts, and nav.ts re-exports them. The rules are in
// docs/design/navigation.md; this file is the shape they share. Desktop mirrors every number here.

import type { Catalog, Door, Layout } from './rooms/gen';
import type { BodyMode } from './nav-height';

/** Grid cell size (world units). Cell (i, j) has its centre at (x0 + (i + 0.5) * cell, z0 + (j + 0.5) * cell). */
export const NAV_CELL = 0.25;
/** A walk cell may step to a neighbour whose height differs by at most this (walked normally). */
export const NAV_STEP = 0.3;
/** A jump edge: a rise above NAV_STEP up to this (cost + 2.0 cells). Higher needs a ramp or lift portal. */
export const NAV_JUMP_UP = 0.8;
/** A drop edge: a fall above NAV_STEP down to this (cost + 1.0 cells). */
export const NAV_DROP_DOWN = 2.0;
/** Robots are discs of this radius. */
export const NAV_RADIUS = 0.3;
/** A plate whose top is at most this above the PCB top gets a ramp portal at its landing; higher, a lift. */
export const NAV_LIFT_RISE = 1.5;
/** The band round a room's slab: off its plate it is a blocked cliff buffer (matches board-layout's RAMP). */
export const NAV_RAMP_DEPTH = 1;
/** Cell costs: open PCB, copper trace, and +NAV_NEAR_COST for cells two cells from a blocked cell. */
export const NAV_COST_OPEN = 1.0;
export const NAV_COST_TRACE = 0.7;
export const NAV_NEAR_COST = 0.5;
/** A lift portal costs this much plus its rise. */
export const NAV_LIFT_COST = 4;
/** Lift travel speed (units per second) and the dwell at each end (seconds). */
export const NAV_LIFT_SPEED = 1.2;
export const NAV_LIFT_DWELL = 0.5;
/** Cached paths live this long (simulation seconds). */
export const NAV_PATH_TTL = 2;
/** Robots this close look ahead (units along the path) for others, and separate within SEPARATION. */
export const NAV_LOOKAHEAD = 1.2;
export const NAV_SEPARATION = 0.7;
/**
 * Gravity and heights (docs/design/navigation.md, "Gravity and heights"): the gravity of a fall or a jump, the flight
 * time of a planned jump (launch to landing), the crouch before a launch, the landing squash, the ground smoothing
 * the glide of a floating species over a gap, and the respawn
 * fade (out, then in).
 */
export const NAV_GRAVITY = 20;
export const NAV_JUMP_TIME = 0.45;
export const NAV_CROUCH_TIME = 0.1;
export const NAV_LAND_TIME = 0.3;
export const NAV_GLIDE_SPEED = 1.5;
export const NAV_RESPAWN_OUT = 0.25;
export const NAV_RESPAWN_IN = 0.35;

/** The lift's phases (lift-ctrl.ts): idle at a stop; the doors opening, unloading, boarding, closing; or travelling. */
export type LiftPhase = 'idle' | 'opening' | 'unloading' | 'boarding' | 'closing' | 'travel';
/** A robot's place with a lift: none; queued at its stop; admitted and walking in (boarding); aboard; aboard at its stop (arrived). */
export type LiftTicket = 'none' | 'queued' | 'boarding' | 'riding' | 'arrived';

/**
 * The lift as its cab and doors show it (docs/design/navigation.md, "Lifts"). `cabY` is the cab floor's height (the riders'
 * y), `doorsOpen` is true while the doors are at least partly open, and `boarding` lists the robots aboard (riding or
 * arrived). `doorT` is the doors' opening (0 shut, 1 open), `stop` the stop the cab is at (0 board, 1 plate; going to, while
 * travelling), and `calls` the robots waiting at each stop.
 */
export interface LiftState {
  cabY: number;
  doorsOpen: boolean;
  boarding: number[];
  doorT: number;
  phase: LiftPhase;
  stop: 0 | 1;
  calls: [number, number];
}

/**
 * A lift's handle (NavWorld.lift). `request(fromY, toY, id)` queues robot `id` for a ride from the stop nearest fromY to the
 * stop nearest toY (false for a duplicate or the same stop). The request stays live until the robot alights or `cancel`s it
 * (it has left); nothing times it out. `ticket` is its place; `board` and `alight` are its own steps into and out of the
 * cab; `cancel` drops it. `state` reads the cab.
 */
export interface LiftHandle {
  request(fromY: number, toY: number, id: number): boolean;
  ticket(id: number): LiftTicket;
  board(id: number): boolean;
  alight(id: number): boolean;
  cancel(id: number): void;
  state(): LiftState;
}

/** A solid part's world AABB (non-decal, non-label geometry). Model units are world units. */
export interface NavBox {
  lo: [number, number, number];
  hi: [number, number, number];
}

/** A copper trace polyline (world x, z) and its width. */
export interface NavTrace {
  path: [number, number][];
  width: number;
}

/**
 * A plate: a slot's top surface, the rectangle [x0, x0 + w) x [z0, z0 + d) at height `y` (the slot's top). Walkable at
 * `y`. `id` is the plate's region id (>= 1); it must differ from every terrace id on the board.
 * `landing` is the plate's one way onto the board, set by the layout: the aisle-end plate cell (ax, az), the board
 * cell reached by the walk (bx, bz), the plate's outward normal at that end (nx, nz), and the kind (a ramp when the
 * plate is at most NAV_LIFT_RISE above the PCB top, else a lift).
 */
export interface NavPlate {
  id: number;
  x0: number;
  z0: number;
  w: number;
  d: number;
  y: number;
  landing?: {
    ax: number;
    az: number;
    bx: number;
    bz: number;
    kind: 'ramp' | 'lift' | 'gangway';
    nx: number;
    nz: number;
    /**
     * A gangway only: the world centres of its bridge cells in order, from the plate edge cell (ax, az) out over the
     * blocked cells to the cell before the lift shaft (bx, bz). The bridge is at plate height.
     */
    bridge?: [number, number][];
  };
}

/** The board: the PCB's size (x, y, z; centred on the origin in x and z), its top, the solid part boxes, traces and plates. */
export interface NavBoard {
  size: [number, number, number];
  pcbTop: number;
  boxes: NavBox[];
  traces: NavTrace[];
  plates: NavPlate[];
}

/**
 * A room on a terrace. The terrace is the district's slab: the room's cell, a rectangle centred on (cx, cz) at height `y`
 * with half widths hw (along world x) and hd (along world z), before the room turn. A square cell has hw = hd = half.
 * `half` is the square half-size (the smaller of hw and hd) and the radius of a round terrace. The room's layout is in its
 * own frame, built with the room group at (cx, y, cz) rotated by `turn` about y (three.js rotation.y). Its inner size is
 * w x d (local x by z).
 */
export interface NavTerrace {
  /** Region id of the terrace's cells (>= 1; 0 is the PCB). Unique on the board. */
  id: number;
  cx: number;
  cz: number;
  half: number;
  /** The cell's half width along world x and half depth along world z (before the room turn). */
  hw: number;
  hd: number;
  round: boolean;
  y: number;
  turn: number;
  w: number;
  d: number;
  door: Door;
  layout: Layout;
  catalog: Catalog;
}

/** A plate landing: the two cells joined. a is the plate cell (height plate.y), b is the board cell (height pcbTop). */
export interface NavPortal {
  a: number;
  b: number;
  kind: 'ramp' | 'lift';
  /** Cost of crossing the portal (ramp: length + 2 * rise; lift: NAV_LIFT_COST + rise). */
  cost: number;
  /** The lift index for a lift portal; -1 for a ramp. */
  lift: number;
}

/** A lift at a tall plate's landing: a platform on the board beside the plate's edge, its rails from `low` to `high`. */
export interface NavLift {
  id: number;
  /** The region id of the plate the lift serves. */
  terrace: number;
  /** The platform centre (world), on the front cell. */
  x: number;
  z: number;
  /** Yaw of the platform's outward normal (three.js rotation.y: the normal is (sin face, cos face)). */
  face: number;
  /** Heights: the PCB top (low) and the plate top (high). */
  low: number;
  high: number;
  /** Cells: the platform's cell on the board, the queue cell beyond it, and the plate's edge cell. */
  front: number;
  queue: number;
  door: number;
}

/** A layout spot in world space. `face` is the world yaw (three.js rotation.y convention). */
export interface NavSpot {
  terrace: number;
  index: number;
  x: number;
  z: number;
  face: number;
  cell: number;
}

export interface NavGrid {
  cell: number;
  cols: number;
  rows: number;
  x0: number;
  z0: number;
  /** 1 where a robot may be planned on: not blocked, not inflated (unless exempt). */
  walk: Uint8Array;
  /** 1 where a cell is solid, wall, off the board, or under a part taller than NAV_STEP (over a plate: taller than the plate). */
  blocked: Uint8Array;
  /** 1 where a non-blocked cell is 8-adjacent to a blocked cell and not exempt. */
  inflated: Uint8Array;
  /** 0 = PCB, else the id of the plate or terrace the cell is on. */
  region: Int32Array;
  /** Surface height of the cell (plate top, terrace top, PCB top). */
  y: Float64Array;
  /** 1 where the cell is off the board (outside the PCB's rectangle): there is no floor. Set by buildGrid. */
  off?: Uint8Array;
  /** Cost of entering the cell. */
  cost: Float64Array;
  /** Portal index for a cell that is a portal end, else -1. */
  portal: Int32Array;
  portals: NavPortal[];
  lifts: NavLift[];
  spots: NavSpot[];
  terraces: NavTerrace[];
  plates: NavPlate[];
  /** Plate ids with no usable landing (the plate's rooms cannot reach the board through it). */
  unreached: number[];
  /** Per terrace (same order as terraces): its door cell and the cell outside its wall beyond the door, -1 when missing. */
  doorCells: [number, number][];
  /** Connected component of each walk cell (rule 12 in nav-grid.ts), -1 for a cell that is not walk. Filled by buildGrid. */
  component?: Int32Array;
  /** The one-way drops between components (rule 12): directed edges, downhill only. Filled by buildGrid. */
  drops?: NavDrop[];
}

/**
 * A one-way drop (rule 12): two walk cells 8-adjacent whose heights differ by more than NAV_JUMP_UP (a robot can fall
 * down it, but cannot climb back up) and at most NAV_DROP_DOWN. `from` is the high cell, `to` the low cell; the planner
 * takes it downhill only (its height rule, moveKind 2). Islands below a drop are cut (rule 12), so every drop is inside
 * the main component.
 */
export interface NavDrop {
  from: number;
  to: number;
}

/**
 * A point on a planned path. `portal` is the portal index crossed to reach it, or -1. `edge` is the height move that ENDS
 * at this point: 'jump' (a rise above NAV_STEP) or 'drop' (a fall above NAV_STEP); undefined for a walk move.
 */
export interface NavPoint {
  x: number;
  z: number;
  y: number;
  portal: number;
  edge?: 'jump' | 'drop';
}

export interface NavPath {
  points: NavPoint[];
  /** Sum of the segment costs (the planner's cost, before smoothing). */
  cost: number;
  /** The fixture hash of the points (quantised; see pathHash in nav-plan.ts). */
  hash: string;
}

/** A goal: a world point, optionally with a facing (world yaw) for the arrival. */
export interface NavGoal {
  x: number;
  z: number;
  face?: number;
}

/** A robot as the floor knows it: its id (cid), its start, its gait and its priority for yielding. */
export interface NavRobotSpec {
  id: number;
  x: number;
  z: number;
  /** Units per second while walking. */
  speed: number;
  /** Radians per second while turning. */
  turnRate: number;
  /** 0 idle, 1 walking, 2 working with a deadline. Higher yields less. Ties: the lower id keeps its way. */
  priority: number;
  /** A floating species (the species' `floats` flag): it glides down over a gap instead of falling. */
  floats?: boolean;
}

/** A robot's pose after a step. */
export interface NavPose {
  x: number;
  y: number;
  z: number;
  heading: number;
  moving: boolean;
  waiting: boolean;
  onLift: boolean;
  /** The vertical body's mode (nav-height.ts): y is the body's feet height, not the cell's. */
  body: BodyMode;
  /** The body's scale: 1, except during the respawn fade (it shrinks to about 0, then grows back). */
  scale: number;
  /** True while the robot is stepping aside for a higher-priority robot (set by the avoidance pass). */
  yielding?: boolean;
}

/** The fixture key: the board, its params, the seed, the start and the goal. */
export interface NavFixtureCase {
  board: string;
  params: Record<string, number>;
  seed: number;
  start: [number, number];
  goal: [number, number];
  hash: string;
}
