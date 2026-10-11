/* The RTS floor's animation maths — DOM-free, three-free, and allocation-free:
   every function returns a number (or writes to nothing), so the frame path
   never allocates and the harness can assert the motion rules directly.

   Everything here is delta/time driven: a function takes the scene clock in
   milliseconds and a per-robot phase in [0,1) so twenty-one robots never
   breathe, blink or walk in lockstep. The pulse rates below are the contract
   the legend and the report state; the reduced-motion path never calls any of
   this at all — the view passes a frozen clock instead (see floor.ts). */

const TAU = Math.PI * 2;

function clamp01(t: number): number {
  return t < 0 ? 0 : t > 1 ? 1 : t;
}

export function easeOutCubic(t: number): number {
  const x = clamp01(t);
  const y = 1 - x;
  return 1 - y * y * y;
}

/** Overshooting ease (a small pop past the target before it settles). Not
    clamped above 1 — the overshoot is the point; it lands exactly on 1 at t=1. */
export function easeOutBack(t: number, overshoot = 1.70158): number {
  const x = clamp01(t);
  const y = x - 1;
  return 1 + (overshoot + 1) * y * y * y + overshoot * y * y;
}

/** Exponential damping toward a target: framerate-independent (the same
    half-life at any frame time) and momentum-less — a pure approach, never an
    overshoot. The camera's pan, zoom and selection-follow all use it. */
export function damp(current: number, target: number, halfLifeMs: number, dtMs: number): number {
  if (halfLifeMs <= 0) return target;
  const k = 1 - Math.pow(2, -dtMs / halfLifeMs);
  return current + (target - current) * k;
}

/* ------------------------------------------------------------- spawn/leave */
export const SPAWN_MS = 520;
export const LEAVE_MS = 420;

/** The scale-in of a robot that has just appeared: from nothing, with a small
    overshoot, to full size. Reduced motion skips this (scale is 1 at once). */
export function spawnScale(elapsedMs: number): number {
  return Math.max(0, easeOutBack(elapsedMs / SPAWN_MS, 1.4));
}

/** The teleport ring's progress in [0,1) while it expands once, or -1 once it
    is spent (also -1 before it starts). */
export function spawnRing(elapsedMs: number): number {
  const d = elapsedMs / SPAWN_MS;
  return d < 0 || d >= 1 ? -1 : d;
}

/** The scale-out of a robot that is leaving: 1 down to 0. */
export function leaveScale(elapsedMs: number): number {
  return 1 - easeOutCubic(elapsedMs / LEAVE_MS);
}

/* -------------------------------------------------------------- status ring */
/** The rings these animations answer for — the four states, bare, so this file
    needs no import from the model. */
export type RingState = 'working' | 'talking' | 'idle' | 'unreachable';

/** The status ring's pulse rate, in Hz, per state: working is brisk, talking is
    calm, idle is a slow breath, and an unreachable robot's ring flickers like an
    alarm. These are the stated rates. */
export function pulseHz(state: RingState): number {
  switch (state) {
    case 'working':
      return 1.2;
    case 'talking':
      return 0.5;
    case 'unreachable':
      return 2.0;
    case 'idle':
      return 0.25;
  }
}

/** The ring's glow in [0,1] at this instant: a raised cosine at the state's
    rate, so the ring reads as a slow breath, a brisk pulse or a flicker. */
export function ringPulse(state: RingState, nowMs: number, phase: number): number {
  return 0.5 - 0.5 * Math.cos(TAU * (nowMs * 0.001 * pulseHz(state) + phase));
}

/* ------------------------------------------------------------------- idle */
/** The idle breath: the torso's rise and fall, in world units (0.5 Hz). */
export function idleBob(nowMs: number, phase: number): number {
  return 0.55 * Math.sin(TAU * (nowMs * 0.0005 + phase));
}

/** The antenna's sway, in radians. */
export function antennaSway(nowMs: number, phase: number): number {
  return 0.16 * Math.sin(TAU * (nowMs * 0.00037 + phase * 1.7));
}

/** The visor's scan bar across the face, in [-1,1]. */
export function visorScan(nowMs: number, phase: number): number {
  return Math.sin(TAU * (nowMs * 0.0007 + phase));
}

/* ------------------------------------------------------------------- walk */
/** One leg's swing at the walk cycle phase `cycle` in [0,1], in radians. The
    two legs call this half a cycle apart, so they swing opposite. */
export function walkSwing(cycle: number): number {
  return 0.6 * Math.sin(TAU * cycle);
}

/** The arm's counter-swing (opposite the leg in front). */
export function walkArm(cycle: number): number {
  return -0.42 * Math.sin(TAU * cycle);
}

/** The torso's bob, in world units, at twice the stride: two beats per cycle. */
export function walkBob(cycle: number): number {
  return 0.5 * Math.abs(Math.sin(TAU * cycle));
}

/* ------------------------------------------------------------------- talk */
/** The talking gesture: a raised hand's swing, in radians. */
export function talkGesture(nowMs: number, phase: number): number {
  return 0.55 * Math.sin(TAU * (nowMs * 0.0011 + phase));
}

export const BUBBLE_MS = 3000;
const BUBBLE_POP_MS = 260;
const BUBBLE_FADE_MS = 420;

/** The speech bubble's scale over one showing: it pops in (with a small
    overshoot), holds, then fades out — repeating every BUBBLE_MS while the pair
    keeps talking. 0 while hidden. */
export function bubblePop(elapsedMs: number): number {
  if (elapsedMs < 0) return 0;
  const t = elapsedMs % BUBBLE_MS;
  if (t < BUBBLE_POP_MS) return easeOutBack(t / BUBBLE_POP_MS, 1.6);
  const fadeStart = BUBBLE_MS - BUBBLE_FADE_MS;
  if (t >= fadeStart) return Math.max(0, 1 - (t - fadeStart) / BUBBLE_FADE_MS);
  return 1;
}

/* ----------------------------------------------------------------- working */
export const SWEEP_MS = 2400;

/** The working pad's scan arc: its angle fraction in [0,1), sweeping once every
    SWEEP_MS. */
export function sweepFrac(nowMs: number, phase: number): number {
  return (((nowMs / SWEEP_MS + phase) % 1) + 1) % 1;
}

export const SPARK_PERIOD_MS = 900;
export const SPARK_RISE_MS = 620;

/** One spark's life fraction in [0,1) while it is in flight, or -1 while this
    index's spark is not airborne. The stagger is deterministic (index-strided),
    so a working robot's sparks are spread, not simultaneous. */
export function sparkLife(nowMs: number, phase: number, index: number): number {
  const stagger = index * 137;
  const t = (((nowMs + phase * SPARK_PERIOD_MS + stagger) % SPARK_PERIOD_MS) + SPARK_PERIOD_MS) % SPARK_PERIOD_MS;
  const d = t / SPARK_RISE_MS;
  return d < 1 ? d : -1;
}
