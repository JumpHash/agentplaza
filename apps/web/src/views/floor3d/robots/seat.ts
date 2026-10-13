// The eye seat (scripts/robot-seat-eyes.mjs runs it): where each socket goes, under the style rules (README "Style").
// A socket is a flat disc, and the eye format has no rotation, so it faces +z. The seat puts the disc on the head's
// front: the head surface behind the disc at every rim point (plus SEAT_EPS), the centre floating at most 50% of r,
// and the least lift that leaves the pupil and rim visible from the portrait cameras in the rest, walk and wave
// poses (audit.ts, checkEyeVisibility). The lateral spot is the current one when it works; otherwise the flattest
// spot in the style's band. A mirror partner copies its twin's seat.

import * as THREE from 'three';
import { SEAT_EPS, eyeSeen, eyeViews, rayHits, sceneOf, transformPoint, type Scene } from './audit';
import type { Clip, Eye, Species, Vec3 } from './types';

/** The style rules: eyes 0.35–0.45 of head width apart (half spacing 0.175–0.225, aimed at 0.205), 45–60% up the head (aimed at 49%). */
const HALF_SPACING = { min: 0.175, aim: 0.205, max: 0.225 };
const HEIGHT = { min: 0.45, aim: 0.49, max: 0.6 };
/** A socket centre may float off the head surface by this share of its radius (the audit's eye-fit limit). */
const FLOAT_SHARE = 0.5;
/** The lift search steps (robot units), and the spots tried after the current one. */
const STEP = 0.0025;
const SPOTS = 12;
/** A socket this close to the centre line is central (x = 0). */
const CENTRE = 0.02;

export interface SeatNote {
  k: number;
  text: string;
  /** The eye stays hidden in some pose: the named parts must shrink or move (a hand fix). */
  blocked: boolean;
}

export interface Seat {
  /** Every eye: the seated ones at their new `at`, the rest copies. */
  eyes: Eye[];
  notes: SeatNote[];
}

type Poses = Partial<Record<'rest' | 'walk' | 'wave', Scene>>;

interface Spot {
  x: number;
  y: number;
  z: number;
  /** Seen in every pose (true), or only at rest (fallback), or not at all. */
  seen: 'all' | 'rest' | 'none';
  blockers: string[];
  /** The rim's highest surface minus the centre's: how far the disc sags on the head (the float rule). */
  drop: number;
}

function toLocal(inv: THREE.Matrix4, p: Vec3): Vec3 {
  const v = new THREE.Vector3(p[0], p[1], p[2]).applyMatrix4(inv);
  return [v.x, v.y, v.z];
}

/** Whether the eye is seen in every pose, and in the rest pose; and the parts that hide it in a pose that fails. */
function seenIn(poses: Poses, e: Eye): { all: boolean; rest: boolean; blockers: string[] } {
  const blockers: string[] = [];
  let all = true;
  let rest = false;
  for (const name of ['rest', 'walk', 'wave'] as const) {
    const sc = poses[name];
    if (!sc) continue;
    const views = eyeViews(e, sc.bones.get(e.bone) ?? new THREE.Matrix4(), sc.pieces);
    const ok = eyeSeen(views);
    if (name === 'rest') rest = ok;
    if (ok) continue;
    all = false;
    for (const v of views) {
      for (const h of v.hiders) {
        if (!h) continue;
        const text = `${name} ${h.path} "${h.label}"`;
        if (!blockers.includes(text)) blockers.push(text);
      }
    }
  }
  return { all, rest, blockers };
}

/** Seats every eye of a species. Pure: the species is not changed. */
export function seatEyes(s: Species, clips: Record<string, Clip>): Seat {
  const poses = sceneOf(s, clips);
  const rest = poses.rest;
  const notes: SeatNote[] = [];
  const eyes: Eye[] = s.eyes.map((e) => ({ ...e, at: [...e.at] as Vec3 }));
  if (!rest) return { eyes, notes: [{ k: -1, text: 'no rest scene', blocked: true }] };

  const head = rest.pieces.filter((p) => p.bone === 'head');
  const width = head.length ? Math.max(...head.map((p) => p.max[0])) - Math.min(...head.map((p) => p.min[0])) : 0;
  const minY = head.length ? Math.min(...head.map((p) => p.min[1])) : 0;
  const height = head.length ? Math.max(...head.map((p) => p.max[1])) - minY : 0;

  const orig = s.eyes.map((e) => transformPoint(rest.bones.get(e.bone) ?? new THREE.Matrix4(), e.at));
  const placed: Vec3[] = [];

  s.eyes.forEach((e, k) => {
    const bone = rest.bones.get(e.bone) ?? new THREE.Matrix4();
    const inv = bone.clone().invert();
    const r = e.r ?? 0.1;
    const onHead = e.bone === 'head' && !e.asym;
    const c = orig[k]!;

    // A mirror partner (an earlier eye at the mirror position, the same shape and size) copies its seat.
    const twin = s.eyes.findIndex((o, j) => {
      if (j >= k || o.asym || e.asym || Math.abs(c[0]) <= CENTRE) return false;
      const d = orig[j]!;
      return Math.abs(d[0] + c[0]) <= 0.02 && Math.abs(d[1] - c[1]) <= 0.02 && Math.abs(d[2] - c[2]) <= 0.02 && o.shape === e.shape && (o.r ?? 0.1) === r && o.pupil === e.pupil;
    });

    // The surface the socket sits on: the eye's own bone, and the head when the eye has its own bone.
    const surface = rest.pieces.filter((p) => p.mat !== 'clear' && (p.bone === e.bone || (e.bone !== 'head' && p.bone === 'head')));
    const frontZ = (px: number, py: number): number | undefined => {
      const h = rayHits(surface, [px, py, 50], [0, 0, -1])[0];
      return h === undefined ? undefined : 50 - h;
    };

    // One spot: the lift over the rim's highest surface (or the float limit), the first lift seen in every pose wins.
    const spot = (x: number, y: number): Spot | null => {
      const ring: [number, number][] = [[x, y]];
      for (let i = 0; i < 8; i++) ring.push([x + r * Math.cos((i / 8) * Math.PI * 2), y + r * Math.sin((i / 8) * Math.PI * 2)]);
      const zs = ring.map(([px, py]) => frontZ(px, py)).filter((z): z is number => z !== undefined);
      const centreZ = frontZ(x, y);
      if (centreZ === undefined || zs.length === 0) return null;
      const base = Math.max(...zs) + SEAT_EPS;
      const drop = Math.max(...zs) - centreZ;
      if (base - centreZ > FLOAT_SHARE * r) return { x, y, z: base, seen: 'none', blockers: [], drop };
      const top = Math.max(base, centreZ + FLOAT_SHARE * r);
      let fallback: Spot | null = null;
      let last: Spot = { x, y, z: top, seen: 'none', blockers: [], drop };
      for (let z = base; z <= top + 1e-9; z += STEP) {
        const res = seenIn(poses, { ...e, at: toLocal(inv, [x, y, z]) });
        if (res.all) return { x, y, z, seen: 'all', blockers: [], drop };
        if (res.rest && !fallback) fallback = { x, y, z, seen: 'rest', blockers: res.blockers, drop };
        last = { x, y, z, seen: 'none', blockers: res.blockers, drop };
      }
      return fallback ?? last;
    };

    // Lateral and height: the current spot when it works, else the flattest spots of the style's band.
    let x = c[0];
    let y = c[1];
    if (twin >= 0) {
      x = -placed[twin]![0];
      y = placed[twin]![1];
    }
    const order: [number, number][] = [[x, y]];
    if (twin < 0 && onHead && width > 0 && height > 0 && Math.abs(x) > CENTRE) {
      const sign = Math.sign(x);
      const grid: { x: number; y: number; drop: number }[] = [];
      for (let sh = HALF_SPACING.min; sh <= HALF_SPACING.max + 1e-9; sh += 0.005) {
        for (let up = HEIGHT.min; up <= HEIGHT.max + 1e-9; up += 0.01) {
          const gx = sign * sh * width;
          const gy = minY + up * height;
          const ring: [number, number][] = [[gx, gy]];
          for (let i = 0; i < 8; i++) ring.push([gx + r * Math.cos((i / 8) * Math.PI * 2), gy + r * Math.sin((i / 8) * Math.PI * 2)]);
          const zs = ring.map(([px, py]) => frontZ(px, py)).filter((z): z is number => z !== undefined);
          const cz = frontZ(gx, gy);
          if (cz === undefined || zs.length < ring.length) continue;
          grid.push({ x: gx, y: gy, drop: Math.max(...zs) - cz });
        }
      }
      grid.sort((a, b) => a.drop - b.drop);
      for (const g of grid.slice(0, SPOTS)) order.push([g.x, g.y]);
    }
    if (twin < 0 && onHead && width > 0 && height > 0 && Math.abs(c[0]) > CENTRE) {
      const share = Math.abs(c[0]) / width;
      const up = (c[1] - minY) / height;
      if (share < HALF_SPACING.min || share > HALF_SPACING.max || up < HEIGHT.min || up > HEIGHT.max) {
        order[0] = [Math.sign(c[0]) * HALF_SPACING.aim * width, minY + HEIGHT.aim * height];
      }
    }

    let pick: Spot | null = null;
    let tried: Spot | null = null;
    for (const [px, py] of order) {
      const res = spot(px, py);
      if (!res) continue;
      tried ??= res;
      if (res.seen === 'all') {
        pick = res;
        break;
      }
      if (res.seen === 'rest' && (!pick || pick.seen === 'none')) pick = res;
    }
    pick ??= tried;
    if (!pick) {
      placed.push([c[0], c[1], c[2]]);
      notes.push({ k, text: `${e.bone} ${e.shape}: no head surface at (${x.toFixed(3)}, ${y.toFixed(3)}); left as it is`, blocked: true });
      return;
    }
    if (pick.seen === 'none') {
      // No spot is seen (or the rim sags past the float limit): the eye stays where it is, for a hand fix.
      placed.push([c[0], c[1], c[2]]);
      notes.push({ k, text: `${e.bone} ${e.shape}: no seat is clear (${pick.blockers.join(', ') || 'the rim sags past the float limit'}); left as it is`, blocked: true });
      return;
    }
    eyes[k] = { ...e, at: toLocal(inv, [pick.x, pick.y, pick.z]) };
    placed.push([pick.x, pick.y, pick.z]);
    const moved = Math.abs(pick.x - c[0]) > 1e-4 || Math.abs(pick.y - c[1]) > 1e-4;
    const text = `${e.bone} ${e.shape}: socket z ${c[2].toFixed(4)} -> ${pick.z.toFixed(4)} (rim drop ${pick.drop.toFixed(4)}), at ${pick.x.toFixed(4)}, ${pick.y.toFixed(4)}${moved ? ' (moved)' : ''}${twin >= 0 ? `, mirror of eye #${twin}` : ''}`;
    const blocked = pick.seen !== 'all';
    notes.push({ k, text: blocked ? `${text}; still hidden: ${pick.blockers.join(', ') || 'float limit'}` : text, blocked });
  });
  return { eyes, notes };
}
