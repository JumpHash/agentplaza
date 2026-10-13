// Robot followers: goals, paths, lifts and avoidance. The contracts are in nav-types.ts; the design is in
// docs/design/navigation.md ("Following" and "Avoiding each other"). Within a step, the followers (follow, the lift code)
// and the avoidance (avoid, separate) only PROPOSE a position for a robot (r.proposal). settle() commits each robot's
// proposal once, at the end of the step, after every proposal is final. watch() then runs the stuck timer (invariant 2).
//
// Rules (the cross-app contract; a Rust port follows them exactly). Cell indices are row-major: cell (i, j) is
// j * cols + i, and cellAt() returns -1 off the grid.
//
// 1. Path cache. The key is (start cell, goal cell). An entry holds a non-null NavPath and the simulation time it was
//    made. It is reused while (now - made) < NAV_PATH_TTL, otherwise the path is replanned and the entry replaced. Null
//    plans are not cached. setGrid clears the cache. A start or goal off the grid is planned without the cache.
//    A plan is made only between two cells of one connected component (invariant 4); otherwise it is null.
// 2. setGoal(id, goal): a goal off the walk (walk 0 or off the grid) first moves to the nearest walk cell of the robot's
//    component (invariant 4); with no such cell the robot keeps no goal, stays where it is, and onUnreachable is called.
//    The (snapped) goal is stored (kept for replans) and the stuck timer restarts. The start is the robot's
//    (x, z), or for a robot on a lift its step-off point. findPath runs from the start. If it returns null the path is
//    cleared, the speed is 0 and the robot stays where it is (moving false); the first retry is due after 0.5 s.
//    Otherwise the points are copied, next = 0, the segment origin is the start, and the final face is goal.face when
//    given, else the heading of the last segment (kept as is when the path has one point). The speed is kept.
//    A new goal also ends a yield: the goal is the new one, not the saved one.
// 3. Following, per robot each step, with dt, for a robot not on a lift. It proposes; it does not move the robot.
//    a. The target is path[next]. The desired heading is atan2(tx - x, tz - z). Once next >= length, it is the final
//       face (or the current heading when there is none).
//    b. Turn first: the heading moves toward the desired heading by at most turnRate * dt along the shortest signed
//       angle, wrapped to (-pi, pi]. Then, if |desired - heading| > pi/6 (measured after the turn), the robot is turning
//       in place: the speed target is 0 and v is 0, and the robot does not translate that step.
//    c. Speed target (when walking and the path is not used up) = speed * corner * arrive. corner = 1 - 0.6 * a / pi,
//       where a is the turn angle between the current segment (origin to target) and the next (target to the point
//       after it); corner = 1 with no next segment or a zero-length segment. arrive = min(1, max(0.25, dist / 1.0))
//       when the target is the final point (dist = robot to final point), else 1.
//    d. The speed moves toward the target: up by at most 2.0 * speed * dt, down by at most 4.0 * speed * dt. Never
//       negative.
//    e. Walk budget = speed * dt. While budget remains and next < length: if the target is a lift entry (rule 4), the
//       lift decides; otherwise, if the target is within budget, move onto it, spend its distance, set origin = target
//       and next += 1; else move budget along the direction and stop. The walk gives a proposed point and the path
//       progress (next, origin) for it; settle() applies the progress only when the proposed point is taken (invariant 1).
//    f. The height of the committed point (commit): a walking robot stands on the ground under its centre, y = grid.y of
//       the cell it is in. While it heads for a jump or drop point (flagged edge), y runs along that edge:
//       origin.y + (target.y - origin.y) * u, with u the fraction of the way from origin to target, clamped to [0, 1].
//    g. moving = speed > 0.02. When the path is used up and speed < 0.02, speed = 0: the robot is done and keeps its
//       position; its heading still eases to the final face at turnRate.
// 4. Lifts. Each lift's cab is a LiftController (lift-ctrl.ts), kept in the lift record across setGrid while its id and
//    shaft (low, high) exist. nav-agents never moves a cab: it asks (request) and reads (ticket, state). An entry
//    is point k with a lift portal (the front cell, NavLift.front; the door is the other side). At an entry the robot
//    asks the cab for its landing (need 0 at the board, the low stop; need 1 at the plate, the high stop) once, and
//    boards when its ticket is 'boarding' (board()). Boarding: the robot
//    proposes the platform centre (lift = true), its y is the cab's cabY, its path becomes points k+1 .. end. Riding: it
//    stays on the cab while its ticket is 'riding', or 'arrived' with the doors still shutting (doorT < 1); its y is
//    cabY. Stepping off: once arrived and the doors are open the robot alights and steps off at the far landing (the
//    step-off point, point k+1); the walk goes on from there. A request is live until the robot has left: a robot whose
//    path no longer uses the lift cancels it (follow), and a removed robot is cancelled everywhere. Nothing times out.
//    Riders are never yielded to or pushed (fixed), as before.
// 5. step(dt): dt is clamped to [0, 0.1] (NaN and negative give 0); time += dt. Then the lifts (rule 4, motion), then
//    the robots in ascending id (a riding robot rides or steps off; any other robot follows, rule 3). Then avoid(dt),
//    then separate(). Then settle() for every robot in ascending id (the only place a position changes), then watch()
//    for every robot in ascending id (invariant 2).
// 6. A waiting robot keeps waiting true and moving false until it boards.
// 7. remove(id) releases a lift the robot occupies. add(spec) with an id already present replaces that robot.
// 8. add(spec): the robot starts at (spec.x, spec.z) when that cell is walkable; otherwise at the centre of snapWalk's
//    cell (kept as is when snapWalk finds none). The snap is a placement: committed with lift = true. Heading 0, no
//    goal, y from the cell.
// 9. setGrid(grid): caches are cleared, the components are recomputed, and lift states are kept for ids that still
//    exist (new lifts start at rest). A riding robot whose lift still exists keeps riding and replans from its step-off
//    point. A riding robot whose lift is gone steps off at once (committed at once: setGrid is not a step). Every robot
//    with a goal replans from its current position (rule 2). Other robots keep their pose; with no path, y is refreshed
//    from the grid.
// 10. Avoidance (avoid(dt)), with the look-ahead of NAV_LOOKAHEAD along each path. Of two robots that meet, the lower
//    priority (ties: the higher id) yields. A rider never yields: a robot that is not riding yields to a rider it meets. A yield steps aside to the nearest free walkable cell (rings out to 6 cells,
//    clear of the others and of the path's own cells), or holds on the spot when there is none. It ends when the
//    blocker has been clear for 0.5 s, or at the blocker's disappearance, or after YIELD_MAX seconds (invariant 3).
//    Then the saved goal is restored, and the robot does not yield to that blocker again for NO_YIELD_AFTER seconds.
// 11. A robot in a hold or a side-step proposes its position (a hold proposes where it stands; a side-step follows its
//    path). A robot with a lift proposal (stepping off) is left alone by avoidance for that step.
// 12. Separation (separate()): robots closer than NAV_SEPARATION are pushed apart by the full overlap each, or by half of
//    it when the full push is not walkable. The pushes move the proposals, and a push goes only to a walkable cell.
//    A rider, or a robot with a lift proposal, is fixed (not pushed): the other robot of its pair is pushed alone.
// 13. Segment check (rule 3, before the turn). The walk from the robot to its next point samples plannable cells of the
//    robot's region at spacing cell / 4, end included (a portal point may lead to another region); the robot's own
//    sample is skipped when its cell is not plannable. Points the robot already stands on are crossed first, without
//    movement, except a lift entry. A failed check replans from the robot's position (rule 2); with no path the robot
//    stays still.
//
// 14. Gravity and heights (nav-height.ts; docs/design/navigation.md, "Gravity and heights"). pose.y is the robot's body,
//     not the cell: step() runs the body once per robot after the commit and the stuck check (vertical). Ground eases, a
//     rise above NAV_STEP is a jump or a crouch, a drop above NAV_STEP is a fall, a floating species glides, and a fall past
//     NAV_DROP_DOWN or a body with no floor respawns the robot at home (respawn: the walk is dropped, the robot fades in).
//
// Invariants (the doc numbers them the same way):
//  1. Never enter a blocked cell (settle). The move from the current point to the proposed point must sweep only walk
//     cells (the start cell is excluded; supercover, Amanatides-Woo, the X axis first at a corner), and each height step
//     between consecutive touched cells must be level (|dy| <= NAV_STEP) or a jump or drop the path emits (the edge point
//     in the move's range, entered at its cell). Or the move lies on a portal edge (its ends within 0.05 of the two cell
//     centres), or it is a lift move (lift = true). If it fails, the x-only move and then the z-only move are tried; if
//     all fail the robot stays and blockedSince records the first failure.
//  2. Never stay stuck (watch). Progress is the distance to the current waypoint shrinking by 0.05 (or a waypoint crossed).
//     Legitimate waits stop the clock: a finished path, a lift wait, a yield, and turning in place (up to 1.2 s). After 1 s
//     without progress the robot replans from its cell with a crowd penalty (1000 for cells within 1.5 of another robot);
//     a failed plan retries after 0.5 s. After 3 s without progress its goal is cleared and onUnreachable is called.
//  3. Every yield has a timeout (avoid): 1.5 s at most, then a rejoin, then no yield to that robot for 1.5 s.
//  4. Snapping and components (setGrid, add, setGoal, findPath calls): a spawn is snapped to the nearest walkable cell
//     (snapWalk). A goal off the walk is snapped to the nearest walk cell of the robot's component (snapInComponent; the
//     goal's own component when the robot has none). A plan needs the start and goal in one connected component of the
//     walk cells (8-neighbours, diagonals only past two walkable orthogonal cells, plus portals whose two ends are
//     walkable). Otherwise the plan fails.

import { LiftController } from './lift-ctrl';
import {
  NAV_JUMP_UP,
  NAV_LOOKAHEAD,
  NAV_PATH_TTL,
  NAV_RADIUS,
  NAV_RESPAWN_OUT,
  NAV_SEPARATION,
  NAV_STEP,
  type LiftHandle,
  type NavGoal,
  type NavGrid,
  type NavLift,
  type NavPath,
  type NavPoint,
  type NavPose,
  type NavRobotSpec,
} from './nav-types';
import { cellAt, moveKind } from './nav-grid';
import { type Body, bodyScale, grounded, newBody, stepBody, type Surfaces } from './nav-height';
import { findPath, snapWalk } from './nav-plan';
import { bakeDrawnTops, drawnTopAt, drawnTopBelow, type DrawnTops, type Support } from './drawn-tops';

const TAU = Math.PI * 2;
/** The footprint's corner offsets (unit signs), scaled by NAV_RADIUS for the support. */
const FOOTPRINT: [number, number][] = [
  [1, 1],
  [1, -1],
  [-1, 1],
  [-1, -1],
];
/** The audit's tolerance (world units): a grounded body is floating above this, or sinking below it. */
const AUDIT_EPS = 0.02;
/** The hover audit's tolerance (world units): a grounded body whose feet are this far above the drawn top under it hovers. */
const HOVER_EPS = 0.01;
/** A robot walks only within 30 degrees of its target heading. */
const WALK_ANGLE = Math.PI / 6;
/** Corner slow-down per radian of turn, the arrival easing radius and its minimum factor. */
const CORNER_LOSS = 0.6;
const EASE_RADIUS = 1.0;
const ARRIVE_MIN = 0.25;
/** Speed change limits, in multiples of speed per second. */
const ACCEL = 2.0;
const DECEL = 4.0;
/** Speeds below this are not walking; a finished path ends at this speed. */
const WALK_SPEED = 0.02;
const MAX_DT = 0.1;
/** Invariant 1: a point within this of both cell centres of a portal edge makes the move a portal transition. */
const PORTAL_EDGE = 0.05;
/** Invariant 2: progress is the waypoint distance shrinking by PROGRESS_STEP; replan after REPLAN_AFTER s without
 *  progress, retry a failed plan after RETRY_AFTER s, give up after GIVE_UP_AFTER s. */
const PROGRESS_STEP = 0.05;
const REPLAN_AFTER = 1;
const RETRY_AFTER = 0.5;
const GIVE_UP_AFTER = 3;
/** Invariant 2: turning in place is legitimate for up to TURN_WAIT s. */
const TURN_WAIT = 1.2;
/** Invariant 2 and 3: the crowd penalty of a replan, its radius, and the longest yield (s). */
const CROWD_RADIUS = 1.5;
const CROWD_PENALTY = 1000;
const YIELD_MAX = 1.5;
/** Invariant 3: after a yield times out, no new yield to the same robot for this long (s). */
const NO_YIELD_AFTER = 1.5;
/** A yield ends once the blocker has been clear this long (s). */
const CLEAR_AFTER = 0.5;
/** Invariant 4: a goal is snapped to a walk cell of its component within this many rings, then the whole grid. */
const SNAP_COMPONENT_RINGS = 12;

interface LiftState {
  def: NavLift;
  /** The cab (lift-ctrl.ts): its queue, doors and travel. Its cabY is the riders' height; nothing else moves a cab. */
  ctrl: LiftController;
}

/** A proposed position (rule A). `walk` is the path progress of a followed proposal; it is applied only when the
 *  proposed point itself is taken. */
interface Proposal {
  x: number;
  z: number;
  lift: boolean;
  walk?: { next: number; from: NavPoint };
}

interface RobotState {
  id: number;
  x: number;
  y: number;
  z: number;
  heading: number;
  speed: number;
  turnRate: number;
  priority: number;
  /** Current walking speed. */
  v: number;
  goal: NavGoal | null;
  path: NavPoint[] | null;
  /** Index of the next point to reach. */
  next: number;
  /** The segment origin: the start, or the last point reached. */
  from: NavPoint;
  finalFace: number | null;
  riding: boolean;
  liftId: number;
  waiting: boolean;
  moving: boolean;
  /** The position proposed in this step (committed by settle), or null. */
  proposal: Proposal | null;
  /** Invariant 1: the time of the first failed commit (-1 when the last commit moved the robot). */
  blockedSince: number;
  /** Invariant 2: the heading is more than WALK_ANGLE off the target after the turn (set by follow). */
  turning: boolean;
  /** Invariant 2: seconds turning in place. */
  turnT: number;
  /** Invariant 2: the robot waits for a lift (a legitimate wait). */
  liftWait: boolean;
  /** Invariant 2: seconds without progress outside the legitimate waits, the next replan due at replanAt. */
  stuckT: number;
  replanAt: number;
  /** Invariant 2: the path, waypoint and waypoint distance at the last progress reference. */
  refPath: NavPoint[] | null;
  refNext: number;
  refDist: number;
  /** Gravity and heights (nav-height.ts): the vertical body, the home desk a respawn returns to, and the species' floats. */
  body: Body;
  homeX: number;
  homeZ: number;
  floats: boolean;
  /** A forced move (shove) this step: the commit does not project it back onto the walk, so its body falls (G4). */
  forced: boolean;
}

/** Per robot avoidance state (rule 10, invariant 3). */
interface AvoidState {
  yielding: boolean;
  /** The spot a held robot stands on (no free cell was found to step to), or null. */
  hold: { x: number; z: number } | null;
  blocker: number;
  /** Seconds the blocker has been clear (the yield ends at CLEAR_AFTER). */
  clear: number;
  /** Seconds yielding (the yield ends at YIELD_MAX). */
  yieldT: number;
  /** The goal to restore when the robot rejoins. */
  saved: NavGoal | null;
  /** Blocker id -> the simulation time until which this robot does not yield to it. */
  noYield: Map<number, number>;
}

/** Wraps an angle to (-pi, pi]. */
function wrapAngle(a: number): number {
  const t = (a + Math.PI) % TAU;
  const u = t < 0 ? t + TAU : t;
  const r = u - Math.PI;
  return r === -Math.PI ? Math.PI : r;
}

/** Turns h toward desired by at most maxStep along the shortest signed angle. */
function turnToward(h: number, desired: number, maxStep: number): number {
  const diff = wrapAngle(desired - h);
  if (Math.abs(diff) <= maxStep) return wrapAngle(desired);
  return wrapAngle(h + Math.sign(diff) * maxStep);
}

/** Heading (three.js rotation.y) of the segment a -> b, or null when it has no length. */
function segmentHeading(a: { x: number; z: number }, b: { x: number; z: number }): number | null {
  const dx = b.x - a.x;
  const dz = b.z - a.z;
  if (Math.hypot(dx, dz) < 1e-9) return null;
  return Math.atan2(dx, dz);
}

function copyPoints(points: NavPoint[]): NavPoint[] {
  return points.map((p) =>
    p.edge !== undefined ? { x: p.x, y: p.y, z: p.z, portal: p.portal, edge: p.edge } : { x: p.x, y: p.y, z: p.z, portal: p.portal },
  );
}

/** A jump (kind 1) or drop (kind 2) edge of a path: the cell its move enters. */
interface EdgeCell {
  cell: number;
  kind: number;
}

/** Rule 5: the height step from cell a to the touched cell b is level, or a jump or drop the path emits into b. */
function stepHeightOk(grid: NavGrid, a: number, b: number, edges: EdgeCell[]): boolean {
  const kind = moveKind(grid.y[b]! - grid.y[a]!);
  if (kind < 0) return false;
  if (kind === 0) return true;
  return edges.some((e) => e.cell === b && e.kind === kind);
}

/** Invariant 1: the walk cells the centre line from (x0, z0) to (x1, z1) enters, in order (the start cell is not
 *  included), or null when one of them is not walkable. Amanatides-Woo over the cell boundaries; at a corner the X axis
 *  steps first (deterministic). */
function sweepCells(grid: NavGrid, x0: number, z0: number, x1: number, z1: number): number[] | null {
  const cells: number[] = [];
  const s = grid.cell;
  let i = Math.floor((x0 - grid.x0) / s);
  let j = Math.floor((z0 - grid.z0) / s);
  const ti = Math.floor((x1 - grid.x0) / s);
  const tj = Math.floor((z1 - grid.z0) / s);
  let mx = Math.abs(ti - i);
  let mz = Math.abs(tj - j);
  const si = ti >= i ? 1 : -1;
  const sj = tj >= j ? 1 : -1;
  const dx = x1 - x0;
  const dz = z1 - z0;
  const tdx = dx !== 0 ? s / Math.abs(dx) : Infinity;
  const tdz = dz !== 0 ? s / Math.abs(dz) : Infinity;
  let tx = dx > 0 ? (grid.x0 + (i + 1) * s - x0) / dx : dx < 0 ? (grid.x0 + i * s - x0) / dx : Infinity;
  let tz = dz > 0 ? (grid.z0 + (j + 1) * s - z0) / dz : dz < 0 ? (grid.z0 + j * s - z0) / dz : Infinity;
  while (mx > 0 || mz > 0) {
    if (mz === 0 || (mx > 0 && tx <= tz)) {
      i += si;
      mx--;
      tx += tdx;
    } else {
      j += sj;
      mz--;
      tz += tdz;
    }
    if (i < 0 || j < 0 || i >= grid.cols || j >= grid.rows) return null;
    const c = j * grid.cols + i;
    if (grid.walk[c] !== 1) return null;
    cells.push(c);
  }
  return cells;
}

/** Invariant 1: the move is a portal transition (both ends within PORTAL_EDGE of the two cell centres of one portal). */
function onPortalEdge(grid: NavGrid, x0: number, z0: number, x1: number, z1: number): boolean {
  const near = (px: number, pz: number, c: { x: number; z: number }): boolean => Math.hypot(px - c.x, pz - c.z) <= PORTAL_EDGE;
  for (const p of grid.portals) {
    const a = cellCentre(grid, p.a);
    const b = cellCentre(grid, p.b);
    if ((near(x0, z0, a) && near(x1, z1, b)) || (near(x0, z0, b) && near(x1, z1, a))) return true;
  }
  return false;
}

/** Invariant 4: the connected component of every walk cell (-1 off the walk). Diagonals need both orthogonal cells
 *  walkable (as the planner); a portal links its two ends when both are walkable. */
function componentsOf(grid: NavGrid): Int32Array {
  const n = grid.walk.length;
  const comp = new Int32Array(n).fill(-1);
  const links = new Map<number, number[]>();
  for (const p of grid.portals) {
    if (grid.walk[p.a] === 1 && grid.walk[p.b] === 1) {
      links.set(p.a, [...(links.get(p.a) ?? []), p.b]);
      links.set(p.b, [...(links.get(p.b) ?? []), p.a]);
    }
  }
  const steps: [number, number][] = [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [-1, 1], [1, -1], [-1, -1]];
  let next = 0;
  const stack: number[] = [];
  for (let s = 0; s < n; s++) {
    if (grid.walk[s] !== 1 || comp[s]! >= 0) continue;
    comp[s] = next;
    stack.push(s);
    while (stack.length > 0) {
      const c = stack.pop()!;
      const i = c % grid.cols;
      const j = Math.floor(c / grid.cols);
      const nbrs = [...(links.get(c) ?? [])];
      for (const [di, dj] of steps) {
        const ni = i + di;
        const nj = j + dj;
        if (ni < 0 || nj < 0 || ni >= grid.cols || nj >= grid.rows) continue;
        if (di !== 0 && dj !== 0 && (grid.walk[j * grid.cols + ni] !== 1 || grid.walk[nj * grid.cols + i] !== 1)) continue;
        nbrs.push(nj * grid.cols + ni);
      }
      for (const m of nbrs) {
        if (grid.walk[m] === 1 && comp[m]! < 0) {
          comp[m] = next;
          stack.push(m);
        }
      }
    }
    next++;
  }
  return comp;
}

/** Rule 3f: the height at (x, z) on the segment from `from` to point `next`; the last point's y once the path is used up.
 *  Null when there is no path. */
function heightAt(pts: NavPoint[], next: number, from: { x: number; y: number; z: number }, x: number, z: number): number | null {
  const last = pts[pts.length - 1];
  if (!last) return null;
  const t = pts[next];
  if (!t) return last.y;
  const dx = t.x - from.x;
  const dz = t.z - from.z;
  const len2 = dx * dx + dz * dz;
  let u = 0;
  if (len2 > 1e-12) u = Math.min(1, Math.max(0, ((x - from.x) * dx + (z - from.z) * dz) / len2));
  return from.y + (t.y - from.y) * u;
}

/** Invariant 2: the stuck timer back to its start (no goal, a new goal, a finished path, a riding robot). */
function clearStuck(r: RobotState): void {
  r.stuckT = 0;
  r.turnT = 0;
  r.replanAt = REPLAN_AFTER;
  r.refPath = null;
  r.refNext = -1;
  r.refDist = Infinity;
}

/** Invariant 2: progress (the timer restarts, the next replan is REPLAN_AFTER s away). */
function progressMade(r: RobotState): void {
  r.stuckT = 0;
  r.replanAt = REPLAN_AFTER;
}

export class NavWorld {
  private grid: NavGrid;
  /** Invariant 4: the connected component of each walk cell. */
  private comp: Int32Array;
  private readonly avoidance = new Map<number, AvoidState>();
  private readonly robots = new Map<number, RobotState>();
  private lifts = new Map<number, LiftState>();
  private readonly cache = new Map<string, { path: NavPath; time: number }>();
  private simTime = 0;
  private unreachableHandler: (id: number) => void = () => {};
  private respawnHandler: (id: number) => void = () => {};
  /** The ground audit (Gravity and heights, invariant G5): robot-frames counted, floating and sinking grounded frames. */
  private auditFrames = 0;
  private auditFloating = 0;
  private auditSinking = 0;
  /** Grounded frames with the centre off a walk cell, or the feet more than AUDIT_EPS above the grid's walk surface there. */
  private auditOnWall = 0;
  /** Grounded frames whose feet hover more than HOVER_EPS over the drawn top under the centre (not for a floating species). */
  private auditHover = 0;
  /** The hover frames whose centre cell has no drawn top at its grid height (an undrawn walk cell, not a drawn one). */
  private auditHoverUndrawn = 0;
  /** Grounded frames whose drawn height was NaN (no drawn top in a sample's band), so the grid's height was used. */
  private auditFallback = 0;
  /** Robots moved onto the nearest walk cell because their centre was off one (the commit point or a grid change). */
  private projections = 0;
  /** The drawn supports the walk heights are baked from (setDrawn), and the bake of the current grid. */
  private sups: Support[] | null = null;
  private drawn: DrawnTops | null = null;

  constructor(grid: NavGrid) {
    this.grid = grid;
    this.comp = componentsOf(grid);
    this.syncLifts();
  }

  /**
   * The drawn supports (the board and its rooms, in world units): the walk heights bake from them over the grid (and again on
   * every setGrid), and the hover audit reads them exactly. Null: no drawn tops (the grid's heights, no hover audit).
   */
  setDrawn(sups: Support[] | null): void {
    this.sups = sups;
    this.drawn = sups ? bakeDrawnTops(this.grid, sups) : null;
  }

  /** Simulation time in seconds. */
  get time(): number {
    return this.simTime;
  }

  /** Invariant 2: called with the robot id when it has made no progress for GIVE_UP_AFTER s; its goal is cleared first. */
  onUnreachable(handler: (id: number) => void): void {
    this.unreachableHandler = handler;
  }

  /**
   * The ground audit: robot-frames so far, and the grounded frames whose feet are more than AUDIT_EPS above the surface
   * (floating) or below it (sinking). A lift rider is not audited (its y is the cab floor).
   */
  audit(): {
    frames: number;
    floating: number;
    sinking: number;
    onWall: number;
    hover: number;
    hoverUndrawn: number;
    fallback: number;
    projections: number;
  } {
    return {
      frames: this.auditFrames,
      floating: this.auditFloating,
      sinking: this.auditSinking,
      onWall: this.auditOnWall,
      hover: this.auditHover,
      hoverUndrawn: this.auditHoverUndrawn,
      fallback: this.auditFallback,
      projections: this.projections,
    };
  }

  /** The lift handle for the lift `id` (LiftHandle in nav-types.ts; navigation.md, "Lifts"). Null for an unknown id. */
  lift(id: number): LiftHandle | null {
    const L = this.lifts.get(id);
    if (!L) return null;
    const c = L.ctrl;
    return {
      request: (fromY, toY, rid) => c.request(rid, fromY, toY),
      ticket: (rid) => c.ticket(rid),
      board: (rid) => c.board(rid),
      alight: (rid) => c.alight(rid),
      cancel: (rid) => c.cancel(rid),
      state: () => c.state(),
    };
  }

  /** Gravity and heights: called with the robot id when its body respawns it at home (after a fall it cannot survive). */
  onRespawn(handler: (id: number) => void): void {
    this.respawnHandler = handler;
  }

  /**
   * A forced move (a test or a debug shove): the robot is put at (x, z) without the commit, so its body sees the ground
   * there at the next step. Unknown ids are a no-op.
   */
  /**
   * The robot's home (where a respawn returns it): the layout's home, which moves when the layout changes (the agent count
   * scales the board). Unknown ids are a no-op; the robot is not moved (a respawn snaps the home to the walk).
   */
  setHome(id: number, x: number, z: number): void {
    const r = this.robots.get(id);
    if (!r) return;
    r.homeX = x;
    r.homeZ = z;
  }

  shove(id: number, x: number, z: number): void {
    const r = this.robots.get(id);
    if (!r) return;
    r.x = x;
    r.z = z;
    r.forced = true;
  }

  setGrid(grid: NavGrid): void {
    this.grid = grid;
    this.comp = componentsOf(grid);
    this.drawn = this.sups ? bakeDrawnTops(grid, this.sups) : null;
    this.cache.clear();
    this.syncLifts();
    for (const r of this.robotList()) {
      if (r.riding) {
        if (this.lifts.has(r.liftId)) {
          const exit = this.exitOf(r);
          this.plan(r, exit);
          if (!r.path) this.restHeight(r);
          continue;
        }
        this.stepOff(r);
        this.settle(r);
      }
      // A robot standing off a walk cell of the new grid goes to the nearest one, and its body follows the surface there.
      this.projectWalk(r);
      if (r.goal) this.plan(r, { x: r.x, y: r.y, z: r.z });
      if (!r.path) this.restHeight(r);
      this.reground(r);
    }
  }

  /**
   * The walk invariant (the commit point and every grid change): a robot's centre is on a walk cell of the current grid. A
   * centre that is not (a rebuilt grid, a push that landed off the walk) moves to the nearest walk cell at once.
   */
  private projectWalk(r: RobotState): void {
    if (r.forced) return;
    const c = cellAt(this.grid, r.x, r.z);
    if (c >= 0 && this.grid.walk[c] === 1) return;
    const snapped = snapWalk(this.grid, r.x, r.z);
    if (snapped < 0) return;
    const p = cellCentre(this.grid, snapped);
    r.x = p.x;
    r.z = p.z;
    this.projections++;
  }

  /** The body follows the surface under the robot now: the body rules with no time passed (a step, not a fade or a fall). */
  private reground(r: RobotState): void {
    if (r.riding) return;
    if (r.body.mode === 'lift') {
      r.body.mode = 'ground';
      r.body.t = 0;
    }
    stepBody(r.body, this.surfacesAt(r.x, r.z), 0, r.floats);
  }

  add(spec: NavRobotSpec): void {
    if (this.robots.has(spec.id)) this.remove(spec.id);
    const grid = this.grid;
    let x = spec.x;
    let z = spec.z;
    let cell = cellAt(grid, x, z);
    const r: RobotState = {
      id: spec.id,
      x: spec.x,
      y: 0,
      z: spec.z,
      heading: 0,
      speed: spec.speed,
      turnRate: spec.turnRate,
      priority: spec.priority,
      v: 0,
      goal: null,
      path: null,
      next: 0,
      from: { x: spec.x, y: 0, z: spec.z, portal: -1 },
      finalFace: null,
      riding: false,
      liftId: -1,
      waiting: false,
      moving: false,
      proposal: null,
      blockedSince: -1,
      turning: false,
      turnT: 0,
      liftWait: false,
      stuckT: 0,
      replanAt: REPLAN_AFTER,
      refPath: null,
      refNext: -1,
      refDist: Infinity,
      body: newBody(0),
      homeX: spec.x,
      homeZ: spec.z,
      floats: spec.floats ?? false,
      forced: false,
    };
    this.robots.set(spec.id, r);
    if (cell < 0 || grid.walk[cell] !== 1) {
      const snapped = snapWalk(grid, x, z);
      if (snapped >= 0) {
        const c = cellCentre(grid, snapped);
        x = c.x;
        z = c.z;
        cell = snapped;
        // A spawn is a placement: it lands on the snapped point at once (lift = true: no sweep).
        this.commit(r, x, z, true);
      }
    }
    const y = cell >= 0 ? (grid.y[cell] ?? 0) : 0;
    r.y = y;
    r.body = newBody(y);
    r.from = { x: r.x, y, z: r.z, portal: -1 };
  }

  remove(id: number): void {
    const r = this.robots.get(id);
    if (!r) return;
    if (r.riding) {
      for (const cab of this.lifts.values()) cab.ctrl.cancel(id);
    }
    this.robots.delete(id);
  }

  has(id: number): boolean {
    return this.robots.has(id);
  }

  /** Unknown ids are a no-op. A goal with no walk cell in reach is given up at once (onUnreachable), the robot stays. */
  setGoal(id: number, goal: NavGoal): void {
    const r = this.robots.get(id);
    if (!r) return;
    const st = this.avoidance.get(id);
    if (st && st.yielding) {
      st.yielding = false;
      st.hold = null;
      st.saved = null;
      st.blocker = -1;
      st.yieldT = 0;
      st.clear = 0;
    }
    const start = r.riding ? this.exitOf(r) : { x: r.x, y: r.y, z: r.z };
    const placed = this.goalInComponent(goal, start);
    if (!placed) {
      r.goal = null;
      clearStuck(r);
      this.plan(r, start);
      r.waiting = false;
      if (!r.riding) this.restHeight(r);
      this.unreachableHandler(id);
      return;
    }
    r.goal = placed;
    clearStuck(r);
    this.plan(r, start);
    if (!r.path) {
      r.replanAt = RETRY_AFTER;
      this.restHeight(r);
    }
  }

  setPriority(id: number, priority: number): void {
    const r = this.robots.get(id);
    if (r) r.priority = priority;
  }

  step(dt: number): void {
    const h = dt > 0 ? Math.min(MAX_DT, dt) : 0;
    this.simTime += h;
    for (const L of this.liftList()) this.updateLift(L, h);
    const robots = this.robotList();
    for (const r of robots) {
      if (r.riding) {
        const L = this.lifts.get(r.liftId);
        if (L && this.aboard(L, r.id)) this.ride(r, h);
        else {
          if (L) L.ctrl.alight(r.id);
          this.stepOff(r);
        }
        continue;
      }
      this.follow(r, h);
    }
    this.avoid(h);
    this.separate();
    for (const r of robots) this.settle(r);
    for (const r of robots) this.watch(r, h);
    for (const r of robots) this.vertical(r, h);
    for (const r of robots) this.audit1(r);
  }

  /**
   * The pose of robot `id`. An unknown id returns a STUB, not a robot: the origin, not moving, not on a lift. Check has(id)
   * before reading a pose that must be real.
   */
  pose(id: number): NavPose {
    const r = this.robots.get(id);
    if (!r) return { x: 0, y: 0, z: 0, heading: 0, moving: false, waiting: false, onLift: false, body: 'ground', scale: 1, yielding: false };
    return {
      x: r.x,
      y: r.body.y,
      z: r.z,
      heading: r.heading,
      moving: r.moving,
      waiting: r.waiting,
      onLift: r.riding,
      body: r.body.mode,
      scale: bodyScale(r.body),
      yielding: this.avoidance.get(id)?.yielding ?? false,
    };
  }

  path(id: number): NavPoint[] | null {
    const r = this.robots.get(id);
    if (!r || !r.path) return null;
    return copyPoints(r.path.slice(r.next));
  }

  liftState(liftId: number): { level: number; occupant: number } {
    const L = this.lifts.get(liftId);
    if (!L) return { level: 0, occupant: -1 };
    const s = L.ctrl.state();
    return { level: (s.cabY - L.def.low) / Math.max(1e-6, L.def.high - L.def.low), occupant: s.boarding[0] ?? -1 };
  }

  /** Read-only debug view of the cell at (x, z): its grid flags, region and connected component (-1 off the walk). Null off the grid. */
  debugCell(x: number, z: number): { cell: number; walk: boolean; blocked: boolean; inflated: boolean; region: number; component: number; y: number } | null {
    const c = cellAt(this.grid, x, z);
    if (c < 0) return null;
    return {
      cell: c,
      walk: this.grid.walk[c] === 1,
      blocked: this.grid.blocked[c] === 1,
      inflated: this.grid.inflated[c] === 1,
      region: this.grid.region[c] ?? 0,
      component: this.comp[c] ?? -1,
      y: this.grid.y[c] ?? 0,
    };
  }

  /**
   * Gravity and heights (docs/design/navigation.md): steps the robot's body on the surface under its centre, after the
   * commit. A riding robot's body is its platform (NavWorld sets y); a body that reaches the end of its fade out respawns.
   */
  private vertical(r: RobotState, dt: number): void {
    const b = r.body;
    if (r.riding) {
      b.mode = 'lift';
      b.y = r.y;
      b.t = 0;
      return;
    }
    if (b.mode === 'lift') {
      b.mode = 'ground';
      b.t = 0;
    }
    stepBody(b, this.surfacesAt(r.x, r.z), dt, r.floats);
    r.forced = false;
    if (b.mode === 'out') {
      this.dropGoal(r);
      if (b.t >= NAV_RESPAWN_OUT) this.respawn(r);
    }
  }

  /**
   * The surfaces under (x, z). The footprint's support is the highest surface under its four corners (NAV_RADIUS), taken
   * over the cells that are not blocked, off the board or cut off (a cliff or a part is not a floor for a corner); the
   * centre must be a walk cell for the ground (walkTop), else the robot is airborne (walkTop null). Off the board: none.
   */
  private surfacesAt(x: number, z: number): Surfaces {
    const s = this.gridSurfacesAt(x, z);
    if (!this.drawn || s.walkTop === null) return s;
    // The feet stand on the drawn top under the centre (the walk heights bake, drawn-tops.ts); the grid's height where none is drawn.
    const h = drawnTopAt(this.drawn, x, z);
    return Number.isNaN(h) ? s : { walkTop: h, floorTop: s.floorTop };
  }

  /** The surfaces from the grid's heights alone (the centre cell and the footprint corners). */
  private gridSurfacesAt(x: number, z: number): Surfaces {
    const grid = this.grid;
    const c = cellAt(grid, x, z);
    if (c < 0 || grid.off?.[c] === 1) return { walkTop: null, floorTop: null };
    const centre = grid.y[c]!;
    let top = centre;
    for (const [dx, dz] of FOOTPRINT) {
      const k = cellAt(grid, x + dx * NAV_RADIUS, z + dz * NAV_RADIUS);
      if (k < 0 || grid.off?.[k] === 1 || grid.blocked[k] === 1) continue;
      // A corner over a surface more than NAV_STEP above the centre is a cliff beside the walk, not a step: it is no floor.
      const y = grid.y[k]!;
      if (y - centre <= NAV_STEP) top = Math.max(top, y);
    }
    return { walkTop: grid.walk[c] === 1 ? top : null, floorTop: top };
  }

  /** The ground audit of one robot (after its body step): a grounded body's feet are on its support, exactly. */
  private audit1(r: RobotState): void {
    if (r.riding) return;
    this.auditFrames++;
    if (!grounded(r.body)) return;
    // onWall: the centre is off a walk cell, or the feet are above the grid's walk surface there (the grid's own heights).
    const gridTop = this.gridSurfacesAt(r.x, r.z).walkTop;
    if (gridTop === null || r.body.y - gridTop > AUDIT_EPS) this.auditOnWall++;
    if (this.drawn && gridTop !== null && Number.isNaN(drawnTopAt(this.drawn, r.x, r.z))) this.auditFallback++;
    const sup = this.surfacesAt(r.x, r.z).walkTop;
    if (sup === null || r.body.y - sup > AUDIT_EPS) this.auditFloating++;
    else if (sup - r.body.y > AUDIT_EPS) this.auditSinking++;
    // hover: the feet are more than HOVER_EPS over the exact drawn top under the centre (a floating species flies: exempt).
    if (!this.sups || r.floats) return;
    const exact = drawnTopBelow(this.sups, r.x, r.z, r.body.y + AUDIT_EPS);
    if (r.body.y - exact <= HOVER_EPS) return;
    this.auditHover++;
    const under = gridTop === null ? 0 : drawnTopBelow(this.sups, r.x, r.z, gridTop);
    if (gridTop === null || Math.abs(under - gridTop) > HOVER_EPS) this.auditHoverUndrawn++;
  }

  /**
   * A robot queued for a lift is waiting for the lift, not stalled: its path reaches a lift portal within a few points,
   * and the robot is within 2 units of that point (the queue at the landing).
   */
  private inLiftQueue(r: RobotState): boolean {
    const pts = r.path;
    if (!pts || r.riding) return false;
    for (let k = r.next; k < pts.length && k < r.next + 5; k++) {
      const p = pts[k]!;
      if (Math.hypot(p.x - r.x, p.z - r.z) > 2) continue;
      const here = p.portal >= 0 ? this.grid.portals[p.portal] : undefined;
      const ahead = pts[k + 1]?.portal;
      const next = ahead !== undefined && ahead >= 0 ? this.grid.portals[ahead] : undefined;
      if (here?.kind === 'lift' || next?.kind === 'lift') return true;
    }
    return false;
  }

  /** A falling robot's walk is over: its goal, path and proposal are dropped and its avoidance is forgotten. */
  private dropGoal(r: RobotState): void {
    if (!r.goal && !r.path) return;
    r.goal = null;
    r.path = null;
    r.next = 0;
    r.proposal = null;
    r.waiting = false;
    r.moving = false;
    r.v = 0;
    clearStuck(r);
    this.avoidance.delete(r.id);
  }

  /** Gravity and heights: the robot goes back to its home (its add position, snapped to a walk cell) with a fresh body that fades in. */
  private respawn(r: RobotState): void {
    this.dropGoal(r);
    if (r.riding) {
      const L = this.lifts.get(r.liftId);
      if (L) L.ctrl.cancel(r.id);
      r.riding = false;
      r.liftId = -1;
    }
    r.x = r.homeX;
    r.z = r.homeZ;
    const c = cellAt(this.grid, r.x, r.z);
    if (c < 0 || this.grid.walk[c] !== 1) {
      const snapped = snapWalk(this.grid, r.x, r.z);
      if (snapped >= 0) {
        const p = cellCentre(this.grid, snapped);
        r.x = p.x;
        r.z = p.z;
      }
    }
    r.heading = 0;
    r.v = 0;
    r.blockedSince = -1;
    r.liftWait = false;
    r.turning = false;
    r.turnT = 0;
    r.from = { x: r.x, y: 0, z: r.z, portal: -1 };
    const s = this.surfacesAt(r.x, r.z);
    r.body = newBody(s.walkTop ?? s.floorTop ?? 0);
    r.body.mode = 'in';
    this.respawnHandler(r.id);
  }

  /** Rule A: records the position a robot proposes for this step (the lift flag and the walk progress are optional). */
  private propose(r: RobotState, x: number, z: number, lift = false, walk?: { next: number; from: NavPoint }): void {
    r.proposal = { x, z, lift, walk };
  }

  /** Rule A: moves the proposal (a push keeps the path progress of a followed proposal), or makes one. */
  private moveProposal(r: RobotState, x: number, z: number): void {
    if (r.proposal) {
      r.proposal.x = x;
      r.proposal.z = z;
    } else {
      r.proposal = { x, z, lift: false };
    }
  }

  /** The position a robot will have after this step's commit (its proposal, else where it stands). */
  private at(r: RobotState): { x: number; z: number } {
    return r.proposal ? { x: r.proposal.x, z: r.proposal.z } : { x: r.x, z: r.z };
  }

  private avoidState(id: number): AvoidState {
    let st = this.avoidance.get(id);
    if (!st) {
      st = { yielding: false, hold: null, blocker: -1, clear: 0, yieldT: 0, saved: null, noYield: new Map() };
      this.avoidance.set(id, st);
    }
    return st;
  }

  /**
   * Rule 10 and invariant 3: look ahead along each path; of two robots that meet, the lower priority (ties: the higher
   * id) yields. A yield ends when the blocker has been clear for CLEAR_AFTER s, or at YIELD_MAX s (then the saved goal
   * is restored and the robot does not yield to that blocker for NO_YIELD_AFTER s).
   */
  private avoid(dt: number): void {
    const robots = this.robotList();
    for (const r of robots) {
      const st = this.avoidState(r.id);
      if (r.riding) {
        st.hold = null;
        continue;
      }
      if (r.proposal?.lift) continue;
      // A rider is a fixed obstacle (it never yields), so a robot that is not riding yields to it.
      const others = robots.filter((o) => o.id !== r.id);
      if (st.yielding) {
        const b = others.find((o) => o.id === st.blocker);
        st.yieldT += dt;
        const far = !b || Math.hypot(b.x - r.x, b.z - r.z) >= 2 * NAV_RADIUS;
        st.clear = far ? st.clear + dt : 0;
        // Two robots yielding to each other: only the higher id keeps yielding, so the lower id proceeds.
        const bothYield = b !== undefined && this.avoidance.get(b.id)?.yielding === true && r.id < b.id;
        const timeUp = st.yieldT >= YIELD_MAX;
        if (st.clear >= CLEAR_AFTER || !b || bothYield || timeUp) {
          if (timeUp && b) st.noYield.set(b.id, this.simTime + NO_YIELD_AFTER);
          this.rejoin(r, st);
          continue;
        }
        if (st.hold) {
          this.propose(r, st.hold.x, st.hold.z);
          r.v = 0;
          r.waiting = true;
          r.moving = false;
        }
        continue;
      }
      const ahead = this.lookAhead(r);
      let rival: RobotState | null = null;
      for (const o of others) {
        if ((st.noYield.get(o.id) ?? -Infinity) > this.simTime) continue;
        if (Math.hypot(o.x - ahead.x, o.z - ahead.z) < 2 * NAV_RADIUS || Math.hypot(o.x - r.x, o.z - r.z) < 2 * NAV_RADIUS) {
          rival = o;
          break;
        }
      }
      if (rival && (rival.riding || r.priority < rival.priority || (r.priority === rival.priority && r.id > rival.id))) this.yieldTo(r, rival, others, st);
    }
  }

  /** Starts a yield: a side-step (its path is planned to the side cell) or a hold on the spot when there is none. */
  private yieldTo(r: RobotState, rival: RobotState, others: RobotState[], st: AvoidState): void {
    const remaining = r.path ? r.path.slice(r.next) : [];
    const side = this.sideCell(r, others, remaining);
    st.yielding = true;
    st.blocker = rival.id;
    st.clear = 0;
    st.yieldT = 0;
    st.saved = r.goal;
    if (side) {
      st.hold = null;
      r.goal = { x: side.x, z: side.z };
      this.plan(r, { x: r.x, y: r.y, z: r.z });
    } else {
      st.hold = { x: r.x, z: r.z };
      this.propose(r, r.x, r.z);
      r.v = 0;
      r.waiting = true;
      r.moving = false;
    }
  }

  /** Ends a yield: the saved goal is restored and the plan is made from here. */
  private rejoin(r: RobotState, st: AvoidState): void {
    st.yielding = false;
    st.hold = null;
    st.clear = 0;
    st.yieldT = 0;
    st.blocker = -1;
    r.goal = st.saved;
    st.saved = null;
    this.plan(r, { x: r.x, y: r.y, z: r.z });
  }

  /** Rule 10: the nearest walkable cell (rings out to 6 cells) clear of the others and of the path's own cells. */
  private sideCell(r: RobotState, others: RobotState[], remaining: NavPoint[]): { x: number; z: number } | null {
    const grid = this.grid;
    const c0 = cellAt(grid, r.x, r.z);
    if (c0 < 0) return null;
    const ci = c0 % grid.cols;
    const cj = (c0 - ci) / grid.cols;
    const pathCells = remaining.map((p) => cellAt(grid, p.x, p.z)).filter((c) => c >= 0);
    for (let R = 0; R <= 6; R++) {
      let best: { x: number; z: number } | null = null;
      let bestD = Infinity;
      let bestIdx = -1;
      for (let dj = -R; dj <= R; dj++) {
        for (let di = -R; di <= R; di++) {
          if (Math.max(Math.abs(di), Math.abs(dj)) !== R) continue;
          const i = ci + di;
          const j = cj + dj;
          if (i < 0 || j < 0 || i >= grid.cols || j >= grid.rows) continue;
          const idx = j * grid.cols + i;
          if (grid.walk[idx] !== 1) continue;
          if (pathCells.some((pc) => Math.abs((pc % grid.cols) - i) <= 1 && Math.abs(Math.floor(pc / grid.cols) - j) <= 1)) continue;
          const x = grid.x0 + (i + 0.5) * grid.cell;
          const z = grid.z0 + (j + 0.5) * grid.cell;
          if (others.some((o) => Math.hypot(o.x - x, o.z - z) < 2 * NAV_RADIUS)) continue;
          const d2 = (x - r.x) * (x - r.x) + (z - r.z) * (z - r.z);
          if (d2 < bestD || (d2 === bestD && idx < bestIdx)) {
            best = { x, z };
            bestD = d2;
            bestIdx = idx;
          }
        }
      }
      if (best) return best;
    }
    return null;
  }

  /** The point NAV_LOOKAHEAD units ahead along the robot's remaining path (the last point if the path is shorter). */
  private lookAhead(r: RobotState): { x: number; z: number } {
    if (!r.path) return { x: r.x, z: r.z };
    let px = r.x;
    let pz = r.z;
    let acc = 0;
    for (let k = r.next; k < r.path.length; k++) {
      const p = r.path[k];
      if (!p) break;
      const seg = Math.hypot(p.x - px, p.z - pz);
      if (seg > 0 && acc + seg >= NAV_LOOKAHEAD) {
        const t = (NAV_LOOKAHEAD - acc) / seg;
        return { x: px + (p.x - px) * t, z: pz + (p.z - pz) * t };
      }
      acc += seg;
      px = p.x;
      pz = p.z;
    }
    return { x: px, z: pz };
  }

  /**
   * Rule 12: robots closer than NAV_SEPARATION are pushed apart by the full overlap each, or by half of it when the full
   * push lands off walkable cells. A push moves the proposal only onto a walkable point. The pushes only move proposals;
   * settle (the commit point) decides which move is taken.
   */
  private separate(): void {
    const robots = this.robotList();
    // A rider, or a robot on a lift proposal, is fixed: only the other robot of its pair is pushed.
    const fixed = (r: RobotState): boolean => r.riding || r.proposal?.lift === true;
    for (let i = 0; i < robots.length; i++) {
      for (let j = i + 1; j < robots.length; j++) {
        const a = robots[i];
        const b = robots[j];
        if (!a || !b) continue;
        if (fixed(a) && fixed(b)) continue;
        const pa = this.at(a);
        const pb = this.at(b);
        const d = Math.hypot(pb.x - pa.x, pb.z - pa.z);
        if (d >= NAV_SEPARATION) continue;
        let nx = 1;
        let nz = 0;
        if (d > 1e-9) {
          nx = (pb.x - pa.x) / d;
          nz = (pb.z - pa.z) / d;
        }
        const push = NAV_SEPARATION - d;
        const pushA = fixed(a) ? null : this.pushPoint(pa, -nx, -nz, push);
        const pushB = fixed(b) ? null : this.pushPoint(pb, nx, nz, push);
        if (pushA) this.moveProposal(a, pushA.x, pushA.z);
        if (pushB) this.moveProposal(b, pushB.x, pushB.z);
      }
    }
  }

  /** Rule 12: the full push from p along (dx, dz) when its point is walkable, else half of it, else none. */
  private pushPoint(p: { x: number; z: number }, dx: number, dz: number, amount: number): { x: number; z: number } | null {
    for (const f of [1, 0.5]) {
      const x = p.x + dx * amount * f;
      const z = p.z + dz * amount * f;
      if (this.walkableAt(x, z)) return { x, z };
    }
    return null;
  }

  /**
   * Invariant 4: the goal as it is stored. A goal on a walk cell is kept. Any other goal (walk 0 or off the grid) moves to
   * the nearest walk cell of the robot's component (the start's; the goal's own nearest walk cell's component when the
   * start has none). Null when no such cell exists.
   */
  private goalInComponent(goal: NavGoal, start: { x: number; z: number }): NavGoal | null {
    const grid = this.grid;
    const gc = cellAt(grid, goal.x, goal.z);
    if (gc >= 0 && grid.walk[gc] === 1) return { ...goal };
    const sc = cellAt(grid, start.x, start.z);
    let comp = sc >= 0 ? (this.comp[sc] ?? -1) : -1;
    if (comp < 0) {
      const near = snapWalk(grid, goal.x, goal.z);
      comp = near >= 0 ? (this.comp[near] ?? -1) : -1;
    }
    if (comp < 0) return null;
    const cell = this.snapInComponent(grid, goal.x, goal.z, comp);
    if (cell < 0) return null;
    const c = cellCentre(grid, cell);
    return { ...goal, x: c.x, z: c.z };
  }

  /**
   * Invariant 4: the walk cell of the given component nearest to (x, z). Square rings out to SNAP_COMPONENT_RINGS cells
   * are searched first, then the whole grid; ties go to the smaller index. Returns -1 when the component has no cell.
   */
  private snapInComponent(grid: NavGrid, x: number, z: number, component: number): number {
    const ci = Math.floor((x - grid.x0) / grid.cell);
    const cj = Math.floor((z - grid.z0) / grid.cell);
    const inComp = (n: number): boolean => grid.walk[n] === 1 && this.comp[n] === component;
    const d2 = (n: number): number => {
      const dx = grid.x0 + ((n % grid.cols) + 0.5) * grid.cell - x;
      const dz = grid.z0 + (Math.floor(n / grid.cols) + 0.5) * grid.cell - z;
      return dx * dx + dz * dz;
    };
    for (let R = 0; R <= SNAP_COMPONENT_RINGS; R++) {
      let best = -1;
      let bestD = Infinity;
      for (let j = cj - R; j <= cj + R; j++) {
        if (j < 0 || j >= grid.rows) continue;
        for (let i = ci - R; i <= ci + R; i++) {
          if (i < 0 || i >= grid.cols) continue;
          if (Math.max(Math.abs(i - ci), Math.abs(j - cj)) !== R) continue;
          const n = j * grid.cols + i;
          if (!inComp(n)) continue;
          const d = d2(n);
          if (d < bestD || (d === bestD && n < best)) {
            best = n;
            bestD = d;
          }
        }
      }
      if (best >= 0) return best;
    }
    let best = -1;
    let bestD = Infinity;
    for (let n = 0; n < grid.walk.length; n++) {
      if (!inComp(n)) continue;
      const d = d2(n);
      if (d < bestD || (d === bestD && n < best)) {
        best = n;
        bestD = d;
      }
    }
    return best;
  }

  /** Invariant 4: the path between two points, or null when either cannot be snapped or they are in two components. */
  private search(from: { x: number; z: number }, to: { x: number; z: number }, penalty?: Float64Array): NavPath | null {
    const grid = this.grid;
    const s = snapWalk(grid, from.x, from.z);
    const g = snapWalk(grid, to.x, to.z);
    if (s < 0 || g < 0 || this.comp[s] !== this.comp[g]) return null;
    // A lift is taken only for a change of level: between two points within NAV_JUMP_UP of each other, the plan first
    // avoids lifts (a ramp or a walk), and only when that finds no way does it allow them.
    if (Math.abs(grid.y[g]! - grid.y[s]!) <= NAV_JUMP_UP) {
      const level = findPath(grid, from, to, { penalty, noLifts: true });
      if (level) return level;
    }
    return findPath(grid, from, to, penalty ? { penalty } : undefined);
  }

  /** Invariant 2: the crowd penalty of a replan: cells within CROWD_RADIUS of another robot cost CROWD_PENALTY more. */
  private crowdPenalty(r: RobotState): Float64Array {
    const grid = this.grid;
    const pen = new Float64Array(grid.cost.length);
    const k = Math.ceil(CROWD_RADIUS / grid.cell);
    for (const o of this.robotList()) {
      if (o.id === r.id) continue;
      const c = cellAt(grid, o.x, o.z);
      if (c < 0) continue;
      const ci = c % grid.cols;
      const cj = Math.floor(c / grid.cols);
      for (let dj = -k; dj <= k; dj++) {
        for (let di = -k; di <= k; di++) {
          const i = ci + di;
          const j = cj + dj;
          if (i < 0 || j < 0 || i >= grid.cols || j >= grid.rows) continue;
          const cx = grid.x0 + (i + 0.5) * grid.cell;
          const cz = grid.z0 + (j + 0.5) * grid.cell;
          if (Math.hypot(cx - o.x, cz - o.z) > CROWD_RADIUS) continue;
          const idx = j * grid.cols + i;
          pen[idx] = (pen[idx] ?? 0) + CROWD_PENALTY;
        }
      }
    }
    return pen;
  }

  /** Invariant 2: a replan from the robot's cell to its goal, with the crowd penalty. False when there is no path. */
  private replanStuck(r: RobotState): boolean {
    if (!r.goal) return false;
    const p = this.search({ x: r.x, z: r.z }, r.goal, this.crowdPenalty(r));
    if (!p) return false;
    r.path = copyPoints(p.points);
    r.next = 0;
    r.from = { x: r.x, y: r.y, z: r.z, portal: -1 };
    r.finalFace = this.finalFaceOf(r.goal, p.points);
    r.proposal = null;
    return true;
  }

  /** Invariant 2: the goal is cleared, the robot stops, and the handler is told. */
  private giveUp(r: RobotState): void {
    r.goal = null;
    r.path = null;
    r.finalFace = null;
    r.proposal = null;
    r.v = 0;
    r.moving = false;
    r.waiting = false;
    clearStuck(r);
    this.unreachableHandler(r.id);
  }

  /**
   * Invariant 2: the stuck timer, run after the commit of each step. Progress is the distance to the current waypoint
   * shrinking by PROGRESS_STEP, or a waypoint crossed. Legitimate waits stop the clock: a finished path (not counted at
   * all), a lift wait, a yield (invariant 3) and turning in place (up to TURN_WAIT s). Otherwise the robot replans after
   * REPLAN_AFTER s without progress, retries a failed plan after RETRY_AFTER s, and gives up after GIVE_UP_AFTER s.
   */
  private watch(r: RobotState, dt: number): void {
    const pts = r.path;
    if (r.riding || !r.goal || (pts !== null && r.next >= pts.length)) {
      clearStuck(r);
      return;
    }
    if (this.avoidance.get(r.id)?.yielding === true || r.liftWait || this.inLiftQueue(r)) return;
    if (r.turning && r.v === 0) {
      r.turnT += dt;
      if (r.turnT <= TURN_WAIT) return;
    } else {
      r.turnT = 0;
    }
    const target = pts ? pts[r.next] : undefined;
    if (pts && target) {
      const dist = Math.hypot(target.x - r.x, target.z - r.z);
      if (r.refPath !== pts || r.refNext !== r.next) {
        // A new path, or a waypoint reached (its distance has shrunk on the way: that was the progress). Crossing a
        // point the robot already stands on is not progress: a replan starts on the robot's own cell.
        r.refPath = pts;
        r.refNext = r.next;
        r.refDist = dist;
        return;
      }
      if (dist <= r.refDist - PROGRESS_STEP) {
        r.refDist = dist;
        progressMade(r);
        return;
      }
    }
    r.stuckT += dt;
    if (r.stuckT >= r.replanAt) r.replanAt = r.stuckT + (this.replanStuck(r) ? REPLAN_AFTER : RETRY_AFTER);
    if (r.stuckT >= GIVE_UP_AFTER) this.giveUp(r);
  }

  /**
   * Invariant 1 (rule B): the one place a robot's position changes. The proposed point is taken when the move from the
   * current point is clear (sweepClear or onPortalEdge; lift = true takes it as it is). Otherwise the x-only move and
   * then the z-only move are tried; if none passes, the robot stays and blockedSince records the first failure. walk,
   * when given, is the path progress of the proposal; it is applied only when the proposed point itself is taken.
   */
  private commit(r: RobotState, x: number, z: number, lift: boolean, walk?: { next: number; from: NavPoint }): void {
    const options: { x: number; z: number; full: boolean }[] = [
      { x, z, full: true },
      { x, z: r.z, full: false },
      { x: r.x, z, full: false },
    ];
    const upto = walk ? walk.next : r.next;
    const chosen = options.find((o) => lift || this.moveClear(r, o.x, o.z, upto));
    if (!chosen) {
      r.v = 0;
      r.moving = false;
      if (r.blockedSince < 0) r.blockedSince = this.simTime;
      // The robot stays on its point, but its height follows the ground (or the edge it now heads for).
      if (!lift) {
        this.projectWalk(r);
        this.groundHeight(r);
      }
      return;
    }
    r.blockedSince = -1;
    r.x = chosen.x;
    r.z = chosen.z;
    if (!lift && chosen.full && walk) {
      r.next = walk.next;
      r.from = walk.from;
    }
    if (!lift) {
      this.projectWalk(r);
      this.groundHeight(r);
    }
  }

  /**
   * Invariant 1 (rule 5): the move from the robot to (x1, z1) is clear when its centre line sweeps walk cells and every
   * height step between consecutive cells is level, or a jump or drop edge the path emits (the edges from the next point
   * to `upto`); or the move is a portal transition.
   */
  private moveClear(r: RobotState, x1: number, z1: number, upto: number): boolean {
    const cells = sweepCells(this.grid, r.x, r.z, x1, z1);
    if (cells) {
      const edges = this.edgesFrom(r, upto);
      let prev = cellAt(this.grid, r.x, r.z);
      let ok = true;
      for (const c of cells) {
        if (prev >= 0 && !stepHeightOk(this.grid, prev, c, edges)) {
          ok = false;
          break;
        }
        prev = c;
      }
      if (ok) return true;
    }
    return onPortalEdge(this.grid, r.x, r.z, x1, z1);
  }

  /** The jump and drop edges of r's path from its next point to `upto` (at least the next point): each cell entered. */
  private edgesFrom(r: RobotState, upto: number): EdgeCell[] {
    const out: EdgeCell[] = [];
    const pts = r.path;
    if (!pts) return out;
    const last = Math.min(pts.length - 1, Math.max(r.next, upto));
    for (let k = r.next; k <= last; k++) {
      const p = pts[k]!;
      if (p.edge === undefined) continue;
      out.push({ cell: cellAt(this.grid, p.x, p.z), kind: p.edge === 'jump' ? 1 : 2 });
    }
    return out;
  }

  /**
   * Rule 3f (commit): a walking robot stands on the ground under its centre (grid.y of its cell). A jump or drop changes
   * the height at the cell boundary, so the pose never sits between two levels (a robot on the top of a drop is still
   * on the top's ground). Off the grid, the height follows the path.
   */
  private groundHeight(r: RobotState): void {
    const c = cellAt(this.grid, r.x, r.z);
    if (c >= 0) {
      r.y = this.grid.y[c] ?? r.y;
      return;
    }
    const pts = r.path;
    if (pts) {
      const h = heightAt(pts, r.next, r.from, r.x, r.z);
      if (h !== null) r.y = h;
    }
  }

  /** Commits the robot's proposal (if any), as the step's one commit for the robot. */
  private settle(r: RobotState): void {
    const p = r.proposal;
    r.proposal = null;
    this.commit(r, p ? p.x : r.x, p ? p.z : r.z, p ? p.lift : false, p?.walk);
  }

  private walkableAt(x: number, z: number): boolean {
    const c = cellAt(this.grid, x, z);
    return c >= 0 && this.grid.walk[c] === 1;
  }

  private robotList(): RobotState[] {
    return Array.from(this.robots.values()).sort((a, b) => a.id - b.id);
  }

  private liftList(): LiftState[] {
    return Array.from(this.lifts.values()).sort((a, b) => a.def.id - b.def.id);
  }

  /** Rebuilds the lift table from the grid, keeping the state of lifts whose ids still exist. */
  private syncLifts(): void {
    const next = new Map<number, LiftState>();
    for (const def of this.grid.lifts) {
      const old = this.lifts.get(def.id);
      const keep = old !== undefined && old.def.low === def.low && old.def.high === def.high;
      next.set(def.id, old && keep ? { ...old, def } : { def, ctrl: new LiftController({ low: def.low, high: def.high }) });
    }
    this.lifts = next;
  }

  /** The step-off point of a riding robot (path[0]); for any other robot, its own position. */
  private exitOf(r: RobotState): NavPoint {
    const first = r.path ? r.path[0] : undefined;
    if (first) return first;
    return { x: r.x, y: r.y, z: r.z, portal: -1 };
  }

  private restHeight(r: RobotState): void {
    const c = cellAt(this.grid, r.x, r.z);
    if (c >= 0) r.y = this.grid.y[c] ?? r.y;
  }

  /**
   * Plans r's path from start to its goal (rule 2). A riding robot keeps its exit point as path[0] when planning
   * fails, so it still steps off there. A plan clears the robot's proposal: the path it refers to is gone.
   */
  private plan(r: RobotState, start: { x: number; y: number; z: number }): void {
    const exit = r.riding ? this.exitOf(r) : null;
    const goal = r.goal;
    const pts = goal ? this.findCached(start, goal) : null;
    r.next = 0;
    r.proposal = null;
    if (goal && pts && pts.length > 0) {
      r.path = pts;
      r.from = { x: start.x, y: start.y, z: start.z, portal: -1 };
      r.finalFace = this.finalFaceOf(goal, pts);
      return;
    }
    r.finalFace = null;
    if (exit) {
      r.path = [exit];
      return;
    }
    r.path = null;
    r.v = 0;
    r.moving = false;
  }

  private finalFaceOf(goal: NavGoal, pts: NavPoint[]): number | null {
    if (goal.face !== undefined) return goal.face;
    const a = pts[pts.length - 2];
    const b = pts[pts.length - 1];
    if (!a || !b) return null;
    return segmentHeading(a, b);
  }

  /** Rule 1: the cached path for the start and goal cells, replanned after NAV_PATH_TTL. */
  private findCached(start: { x: number; z: number }, goal: NavGoal): NavPoint[] | null {
    const grid = this.grid;
    const sc = cellAt(grid, start.x, start.z);
    const gc = cellAt(grid, goal.x, goal.z);
    const from = { x: start.x, z: start.z };
    const to = { x: goal.x, z: goal.z };
    if (sc < 0 || gc < 0) {
      const p = this.search(from, to);
      return p ? copyPoints(p.points) : null;
    }
    const key = `${sc}:${gc}`;
    const hit = this.cache.get(key);
    if (hit && this.simTime - hit.time < NAV_PATH_TTL) {
      // The cached path was made from another start in the same cell: its first point becomes this start.
      const pts = copyPoints(hit.path.points);
      const first = pts[0];
      if (first) {
        first.x = start.x;
        first.z = start.z;
      }
      return pts;
    }
    const p = this.search(from, to);
    if (!p) {
      this.cache.delete(key);
      return null;
    }
    this.cache.set(key, { path: p, time: this.simTime });
    return copyPoints(p.points);
  }

  private updateLift(L: LiftState, dt: number): void {
    L.ctrl.update(dt);
  }

  private surfaceY(L: LiftState): number {
    return L.ctrl.state().cabY;
  }

  /** Rule 4: does the robot's remaining path use the lift `id` (a portal of that lift, at or after its next point)? */
  private pathUsesLift(r: RobotState, id: number): boolean {
    if (!r.path) return false;
    return r.path.slice(r.next).some((p) => {
      const port = p.portal >= 0 ? this.grid.portals[p.portal] : undefined;
      return port !== undefined && port.kind === 'lift' && port.lift === id;
    });
  }

  /** Rule 4: a rider is on the cab until its ticket is arrived with the doors open (then it steps off). */
  private aboard(L: LiftState, id: number): boolean {
    const t = L.ctrl.ticket(id);
    return t === 'riding' || (t === 'arrived' && L.ctrl.state().doorT < 1);
  }

  /** Rule 4, riding: the robot proposes the platform centre (lift) and turns toward its step-off point. */
  private ride(r: RobotState, dt: number): void {
    const L = this.lifts.get(r.liftId);
    if (!L) return;
    this.propose(r, L.def.x, L.def.z, true);
    r.y = this.surfaceY(L);
    r.v = 0;
    r.waiting = false;
    r.moving = false;
    const exit = this.exitOf(r);
    r.heading = turnToward(r.heading, Math.atan2(exit.x - L.def.x, exit.z - L.def.z), r.turnRate * dt);
  }

  /**
   * Rule 4, step-off: the robot proposes the step-off point (lift), the lift is released, and the robot walks from the
   * next step. Called from setGrid, the proposal is committed at once.
   */
  private stepOff(r: RobotState): void {
    const exit = this.exitOf(r);
    const L = this.lifts.get(r.liftId);
    if (L) L.ctrl.cancel(r.id);
    r.riding = false;
    r.liftId = -1;
    r.turning = false;
    r.liftWait = false;
    r.y = exit.y;
    r.from = { x: exit.x, y: exit.y, z: exit.z, portal: -1 };
    r.next = 1;
    this.propose(r, exit.x, exit.z, true);
  }

  /** Rule 4, entry: the lift that the path enters at point k, and the level the robot needs (see the header). */
  private entryAt(pts: NavPoint[], k: number): { lift: LiftState; need: number } | null {
    const grid = this.grid;
    const p = pts[k];
    const q = pts[k + 1];
    if (!p || !q) return null;
    const front = cellAt(grid, p.x, p.z);
    if (p.portal >= 0) {
      const port = grid.portals[p.portal];
      const L = port && port.kind === 'lift' ? this.lifts.get(port.lift) : undefined;
      if (L && front === L.def.front) return { lift: L, need: 1 };
    }
    if (q.portal >= 0) {
      const port = grid.portals[q.portal];
      const L = port && port.kind === 'lift' ? this.lifts.get(port.lift) : undefined;
      if (L) {
        // The portal joins the lift's two landings: the front (the platform's cell on the board) and the door (the plate's
        // edge). A robot standing at either boards the cab at its own level: the board side needs the low end, the plate
        // side the high end. The path's next point is the step-off.
        const b = cellAt(grid, q.x, q.z);
        if (front === L.def.front && b === L.def.door) return { lift: L, need: 0 };
        if (front === L.def.door && b === L.def.front) return { lift: L, need: 1 };
      }
    }
    return null;
  }

  /** Rule 4, boarding at point k (the front cell). The robot's path continues from the step-off point. */
  private board(r: RobotState, L: LiftState, pts: NavPoint[], k: number): void {
    r.riding = true;
    r.liftId = L.def.id;
    r.path = pts.slice(k + 1);
    r.next = 0;
    r.turning = false;
    this.propose(r, L.def.x, L.def.z, true);
    r.y = this.surfaceY(L);
    r.v = 0;
    r.waiting = false;
    r.moving = false;
  }

  /**
   * Rule 3: the follower for a robot that is not on a lift. It proposes the point it walks to (invariant 1 decides in
   * settle) with the path progress that goes with it; it does not move the robot.
   */
  private follow(r: RobotState, dt: number): void {
    r.waiting = false;
    r.liftWait = false;
    r.turning = false;
    r.proposal = null;
    // A request is live until the robot has LEFT: a robot whose path no longer uses that lift (a new goal, a replan, a fall)
    // cancels it here. Waiting, holding and yielding keep it: nothing times a request out (lift-ctrl.ts).
    for (const L of this.liftList()) {
      const t = L.ctrl.ticket(r.id);
      if ((t === 'queued' || t === 'boarding') && !this.pathUsesLift(r, L.def.id)) L.ctrl.cancel(r.id);
    }
    if (this.avoidance.get(r.id)?.hold) {
      r.v = 0;
      r.moving = false;
      r.waiting = true;
      return;
    }
    if (!r.path) {
      r.v = 0;
      r.moving = false;
      return;
    }
    this.passOwnPoints(r);
    if (!this.segmentOk(r)) {
      // Rule 13: the walk to the next point is not on plannable cells of this region; replan from here.
      this.plan(r, { x: r.x, y: r.y, z: r.z });
      if (!r.path) {
        r.v = 0;
        r.moving = false;
        return;
      }
      this.passOwnPoints(r);
    }
    const pts = r.path;
    const n = pts.length;
    const used = r.next >= n;

    // 3a, 3b: turn first.
    const target = pts[r.next];
    const desired = target
      ? Math.atan2(target.x - r.x, target.z - r.z)
      : r.finalFace !== null
        ? r.finalFace
        : r.heading;
    r.heading = turnToward(r.heading, desired, r.turnRate * dt);
    r.turning = Math.abs(wrapAngle(desired - r.heading)) > WALK_ANGLE;

    // 3b, 3c, 3d: turning in place (after the turn, more than WALK_ANGLE off the target) the robot does not move this
    // step (v is 0). Otherwise the speed moves toward its target; a finished path has target 0.
    if (r.turning) {
      r.v = 0;
    } else {
      const vTarget = used ? 0 : r.speed * this.cornerFactor(r, pts) * this.arrivalFactor(r, pts);
      if (r.v < vTarget) r.v = Math.min(vTarget, r.v + ACCEL * r.speed * dt);
      else r.v = Math.max(vTarget, r.v - DECEL * r.speed * dt);
    }

    // 3e: walk along the points on local copies; the lift entries are crossed under rule 4.
    let x = r.x;
    let z = r.z;
    let next = r.next;
    let from = r.from;
    let budget = r.v * dt;
    let atLift = false;
    while (next < n) {
      const entry = this.entryAt(pts, next);
      if (entry) {
        const L = entry.lift;
        const need = entry.need;
        // LiftController.request takes (id, fromY, toY); the handle takes (fromY, toY, id). This call is the controller's.
        if (L.ctrl.ticket(r.id) === 'none') L.ctrl.request(r.id, need === 0 ? L.def.low : L.def.high, need === 0 ? L.def.high : L.def.low);
        if (L.ctrl.ticket(r.id) === 'boarding' && L.ctrl.board(r.id)) {
          this.board(r, L, pts, next);
          return;
        }
        atLift = true;
        break;
      }
      const p = pts[next];
      if (!p) break;
      const dx = p.x - x;
      const dz = p.z - z;
      const d = Math.hypot(dx, dz);
      if (d > budget) {
        if (d > 0) {
          const f = budget / d;
          x += dx * f;
          z += dz * f;
        }
        budget = 0;
        break;
      }
      x = p.x;
      z = p.z;
      budget -= d;
      from = p;
      next += 1;
    }
    if (atLift) {
      r.waiting = true;
      r.liftWait = true;
      r.v = 0;
    }
    if (next >= n && r.v < WALK_SPEED) r.v = 0;
    r.moving = r.v > WALK_SPEED;
    this.propose(r, x, z, false, { next, from });
  }

  /** Crosses the points the robot already stands on (no movement), except a lift entry, which the walk decides. */
  private passOwnPoints(r: RobotState): void {
    const pts = r.path;
    if (!pts) return;
    while (r.next < pts.length) {
      const p = pts[r.next];
      if (!p || Math.hypot(p.x - r.x, p.z - r.z) > 1e-9 || this.entryAt(pts, r.next)) break;
      r.from = p;
      r.next++;
    }
  }

  /**
   * Rule 13: the walk from the robot to its next point samples plannable cells at the planner's spacing (cell / 4, end
   * included) of the robot's region; a portal point may lead to another region. The robot's own sample is skipped when
   * its cell is not plannable (it is already off the path and may walk back onto it).
   */
  private segmentOk(r: RobotState): boolean {
    const pts = r.path;
    const q = pts ? pts[r.next] : undefined;
    if (!q) return true;
    const grid = this.grid;
    const c0 = cellAt(grid, r.x, r.z);
    const own = c0 >= 0 && grid.walk[c0] === 1;
    const region = c0 >= 0 ? (grid.region[c0] ?? -2) : -2;
    const dx = q.x - r.x;
    const dz = q.z - r.z;
    const n = Math.max(1, Math.ceil(Math.hypot(dx, dz) / (grid.cell / 4)));
    for (let k = own ? 0 : 1; k <= n; k++) {
      const x = k === n ? q.x : r.x + dx * (k / n);
      const z = k === n ? q.z : r.z + dz * (k / n);
      const c = cellAt(grid, x, z);
      if (c < 0 || grid.walk[c] !== 1) return false;
      if (q.portal < 0 && q.edge === undefined && grid.region[c] !== region) return false;
    }
    return true;
  }

  /** Rule 3c: slow-down for the turn ahead. */
  private cornerFactor(r: RobotState, pts: NavPoint[]): number {
    const t = pts[r.next];
    const after = pts[r.next + 1];
    if (!t || !after) return 1;
    const cur = segmentHeading(r.from, t);
    const nxt = segmentHeading(t, after);
    if (cur === null || nxt === null) return 1;
    const angle = Math.abs(wrapAngle(nxt - cur));
    return 1 - (CORNER_LOSS * angle) / Math.PI;
  }

  /** Rule 3c: easing into the final point. */
  private arrivalFactor(r: RobotState, pts: NavPoint[]): number {
    if (r.next !== pts.length - 1) return 1;
    const t = pts[r.next];
    if (!t) return 1;
    const dist = Math.hypot(t.x - r.x, t.z - r.z);
    return Math.min(1, Math.max(ARRIVE_MIN, dist / EASE_RADIUS));
  }
}

/** Centre of a cell by index (row-major, rule header: index = j * cols + i). */
function cellCentre(grid: NavGrid, idx: number): { x: number; z: number } {
  const i = idx % grid.cols;
  const j = Math.floor(idx / grid.cols);
  return { x: grid.x0 + (i + 0.5) * grid.cell, z: grid.z0 + (j + 0.5) * grid.cell };
}
