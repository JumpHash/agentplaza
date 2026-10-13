// Gravity and heights: a robot's vertical body (its feet height y, its ballistic anchor and its mode). Pure: no three.js and
// no nav state. NavWorld (nav-agents.ts) owns one Body per robot, steps it after the commit, and performs the respawn
// when a body reports the end of its fade out. The rules are in docs/design/navigation.md ("Gravity and heights"); the
// desktop port mirrors them in world/motion.rs.
//
// Modes:
//   ground  on a walk cell: y is the surface under the footprint, exactly (NavWorld gives walkTop as the highest non-blocked
//           surface under the four corners of the robot's footprint, so no corner sinks into an edge).
//   crouch  a planned rise (above NAV_STEP, up to NAV_JUMP_UP) starts: the body holds its height for NAV_CROUCH_TIME.
//   jump    the flight of that rise: launch speed chosen so the arc lands on the new surface after NAV_JUMP_TIME.
//   fall    a fall (a drop above NAV_STEP, or no walk cell under the centre): ballistic under NAV_GRAVITY.
//   glide   a floating species over a gap (or no walk cell): it sinks at NAV_GLIDE_SPEED, it does not fall.
//   land    the squash after a landing (NAV_LAND_TIME), on the walk surface.
//   lift    riding a lift: y is the platform's (NavWorld sets it).
//   out     the respawn fade out: the body keeps falling (ballistic) for NAV_RESPAWN_OUT; then NavWorld respawns it.
//   in      the respawn fade in, at home, for NAV_RESPAWN_IN; then ground.
//
// Air modes (crouch is not one) use the anchor (y0, v0, t): y = y0 + v0 t - g t^2 / 2, v = v0 - g t, t counted from the
// anchor. A jump's landing is at t = NAV_JUMP_TIME when its surface is where it was launched; a fall lands on the first
// walk surface its anchor reaches. A fall longer than NAV_DROP_DOWN (measured from its anchor), or a body with no floor
// under it at all (off the board), goes to out.

import { NAV_CROUCH_TIME, NAV_DROP_DOWN, NAV_GLIDE_SPEED, NAV_GRAVITY, NAV_JUMP_TIME, NAV_JUMP_UP, NAV_LAND_TIME, NAV_RESPAWN_IN, NAV_RESPAWN_OUT, NAV_STEP } from './nav-types';

export type BodyMode = 'ground' | 'crouch' | 'jump' | 'fall' | 'glide' | 'land' | 'lift' | 'out' | 'in';

export interface Body {
  mode: BodyMode;
  /** The feet height (world y). */
  y: number;
  /** Air modes: the height at the anchor (t = 0). */
  y0: number;
  /** Air modes: the vertical velocity at the anchor. */
  v0: number;
  /** Seconds in the current mode (air modes: since the anchor). */
  t: number;
}

/** What is under a body's centre: `walkTop` is the top of a walk cell (null when the cell is not walkable), `floorTop`
 *  the top of any on-board cell (null off the board). */
export interface Surfaces {
  walkTop: number | null;
  floorTop: number | null;
}

/** Shares the epsilon of the landing test (a height within this of the surface has landed). */
const EPS = 1e-4;
/** The smallest scale of the respawn fade (not 0: the group keeps a finite matrix). */
const FADE_MIN = 0.02;
/** The launch speed of a planned jump: the arc rises by NAV_JUMP_TIME's ballistic shape to the new surface. */
const JUMP_LIFT = 0.5 * NAV_GRAVITY * NAV_JUMP_TIME * NAV_JUMP_TIME;

/** A body standing at height y, on the ground. */
export function newBody(y: number): Body {
  return { mode: 'ground', y, y0: y, v0: 0, t: 0 };
}

/** The height of an air body at its anchor's time t. */
export function airY(b: Body): number {
  return b.y0 + b.v0 * b.t - 0.5 * NAV_GRAVITY * b.t * b.t;
}

/** The vertical velocity of an air body now. */
export function airV(b: Body): number {
  return b.v0 - NAV_GRAVITY * b.t;
}

/** Starts a new mode from the current height: its anchor is here, with vertical velocity v0. */
function enter(b: Body, mode: BodyMode, v0: number): void {
  b.mode = mode;
  b.y0 = b.y;
  b.v0 = v0;
  b.t = 0;
}

/** Launches a planned rise from the current height to `target`: v0 such that the arc lands on target at NAV_JUMP_TIME. */
function launch(b: Body, target: number): void {
  const v0 = (target - b.y + JUMP_LIFT) / NAV_JUMP_TIME;
  enter(b, 'jump', v0);
}

/** Advances a body by dt seconds (`floats`: the species floats). Non-lift bodies only; NavWorld sets y while riding. */
export function stepBody(b: Body, s: Surfaces, dt: number, floats: boolean): void {
  switch (b.mode) {
    case 'lift':
      return;
    case 'ground': {
      b.t += dt;
      if (s.walkTop === null) return enter(b, floats && s.floorTop !== null ? 'glide' : 'fall', 0);
      const d = s.walkTop - b.y;
      if (d > NAV_STEP + EPS) {
        // A rise over NAV_JUMP_UP is never planned: it is snapped up (the feet stay on the surface).
        if (d > NAV_JUMP_UP + EPS) {
          b.y = s.walkTop;
          return;
        }
        if (floats) return launch(b, s.walkTop);
        return enter(b, 'crouch', 0);
      }
      if (d < -NAV_STEP - EPS) return enter(b, floats ? 'glide' : 'fall', 0);
      // Exact contact: a grounded body's feet are on the surface every step (the audit counts any gap).
      b.y = s.walkTop;
      return;
    }
    case 'crouch': {
      b.t += dt;
      if (s.walkTop === null) return enter(b, 'fall', 0);
      if (b.t >= NAV_CROUCH_TIME) launch(b, s.walkTop);
      return;
    }
    case 'jump':
    case 'fall':
    case 'out': {
      b.t += dt;
      const y = airY(b);
      const v = airV(b);
      b.y = y;
      if (b.mode === 'out') return;
      if (s.floorTop === null || b.y0 - y > NAV_DROP_DOWN) return enter(b, 'out', v);
      if (s.walkTop !== null && v <= 0 && y <= s.walkTop + EPS) {
        b.y = s.walkTop;
        return enter(b, 'land', 0);
      }
      return;
    }
    case 'glide': {
      b.t += dt;
      if (s.walkTop !== null && s.walkTop - b.y >= -NAV_STEP) {
        b.mode = 'ground';
        b.t = 0;
        return;
      }
      const target = s.walkTop ?? s.floorTop;
      if (target === null) return enter(b, 'out', 0);
      b.y = Math.max(target, b.y - NAV_GLIDE_SPEED * dt);
      return;
    }
    case 'land': {
      b.t += dt;
      if (s.walkTop === null) return enter(b, 'fall', 0);
      b.y = s.walkTop;
      if (b.t >= NAV_LAND_TIME) {
        b.mode = 'ground';
        b.t = 0;
      }
      return;
    }
    case 'in': {
      b.t += dt;
      if (b.t >= NAV_RESPAWN_IN) {
        b.mode = 'ground';
        b.t = 0;
      }
      return;
    }
  }
}

/** The respawn fade's scale now: 1 except in out (it shrinks, ease in) and in (it grows back, ease out with a small overshoot). */
export function bodyScale(b: Body): number {
  if (b.mode === 'out') {
    const u = Math.min(1, b.t / NAV_RESPAWN_OUT);
    return 1 - (1 - FADE_MIN) * u * u;
  }
  if (b.mode === 'in') {
    const u = Math.min(1, b.t / NAV_RESPAWN_IN);
    const c1 = 1.70158;
    const c3 = c1 + 1;
    const e = 1 + c3 * (u - 1) ** 3 + c1 * (u - 1) ** 2;
    return FADE_MIN + (1 - FADE_MIN) * e;
  }
  return 1;
}

/** Whether a body is on the ground (its feet are on a walk surface and it is not in the air). */
export function grounded(b: Body): boolean {
  return b.mode === 'ground' || b.mode === 'land' || b.mode === 'in';
}
