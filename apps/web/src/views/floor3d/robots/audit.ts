// The robot design audit: pure geometry over a species (no DOM). Every part is sampled from the same
// geometry the rig bakes (geometry.ts), placed by the bone chain (rest pose, and the walk and wave clips
// evaluated the way clips.ts evaluates them), and checked for fit, contact, overlap, symmetry, ground,
// tool holds and proportions. Tolerances and the style rules are in assets/robots/README.md ("Style").
//
// auditSpecies(species) -> Issue[]: every issue names its JSON path (parts[12], eyes[1], tools/x.json
// parts[3]) and the part's label (its `name`, or bone:geo:mat#index). metrics(species) -> the numbers the
// style guide is written from; medians(all) and the outlier rule flag a species against the other 30.

import * as THREE from 'three';
import { evalClip, resolve } from './clips';
import { partGeometry } from './geometry';
import { registry } from './files';
import type { Clip, Eye, Gait, Part, Species, Tool, Vec3 } from './types';

export type PoseName = 'rest' | 'walk' | 'wave';
export type IssueKind =
  | 'eye-fit' | 'eye-pupil' | 'eye-silhouette' | 'eye-overlap' | 'eye-pair' | 'eye-occluded'
  | 'feature-float' | 'feature-buried' | 'attach' | 'limb-through-body'
  | 'symmetry' | 'ground' | 'tool' | 'proportion';

export interface Issue {
  kind: IssueKind;
  severity: 'error' | 'warn';
  /** A JSON path into the species file (or the tool file), e.g. `parts[12]`, `eyes[1]`. */
  path: string;
  /** The part's name, or `bone:geo:mat#index`. */
  label: string;
  pose: PoseName;
  message: string;
  value?: number;
}

export interface Metrics {
  id: string;
  locomotion: string;
  floats: boolean;
  parts: number;
  eyes: number;
  /** Overall size (rest pose, robot units). */
  height: number;
  width: number;
  depth: number;
  minY: number;
  headHeight: number;
  headWidth: number;
  bodyHeight: number | null;
  /** head height / body height. */
  headToBody: number | null;
  /** head height / overall height. */
  headToHeight: number;
  /** mean socket radius / head width. */
  eyeRelHead: number | null;
  /** mean nearest-neighbour distance between eye sockets / head width. */
  eyeSpacing: number | null;
  /** mean socket height above the head's base, as a fraction of head height. */
  eyeHeight: number | null;
  /** Area share of the surface that carries the ink line (inked parts / all parts). */
  inkCoverage: number;
}

export type MetricKey = 'height' | 'width' | 'headToBody' | 'headToHeight' | 'eyeRelHead' | 'eyeSpacing' | 'eyeHeight' | 'inkCoverage';
export type Medians = Record<MetricKey, number>;

export interface AuditOptions {
  /** Tools for the hold check (default: the registry's valid tools). */
  tools?: Record<string, Tool>;
  /** Shared clips for the walk and wave poses (default: the registry's). */
  clips?: Record<string, Clip>;
  /** Medians for the proportion check (default: the medians of the registry's species). */
  medians?: Medians;
}

// Tolerances. Robot units: a species is about 1 high.
const SAMPLES = 48; // vertices sampled per part for contact and containment tests
const CONTACT = 0.02; // a part touches another within this gap
const GROUND = 0.01; // the lowest point of a grounded species sits at y = 0 ± this
const EYE_FIT = 0.5; // a socket centre may float off the head surface by this share of r: a flat disc sags onto a curved head at its rim
const BURIED = 0.3; // a feature buried deeper than this share of its size
const LIMB = 0.03; // a limb sunk this deep into the body
const SYM = 0.02; // mirror tolerance (centre, and size)
const MEDIAN_TOL = 0.25; // a metric this far (relative) from the median is an outlier
const PIERCE = 0.01; // a tool this deep inside the head or body
const MATTE = ['shell', 'shell2', 'panel', 'paper', 'trim', 'ink', 'signal', 'glass', 'accent'];
const RAY_DIR: Vec3 = normal([0.3714, 0.5571, 0.7428]);
const EPS = 1e-6;

function normal(v: Vec3): Vec3 {
  const l = Math.hypot(v[0], v[1], v[2]) || 1;
  return [v[0] / l, v[1] / l, v[2] / l];
}

/** A part (or a tool part) placed in the world: its triangles, sample points and bounds. */
export interface Piece {
  path: string;
  label: string;
  bone: string;
  part: Part;
  mat: string;
  /** A plane or ring: no volume at all. */
  surface: boolean;
  /** Every edge shared by two triangles: a solid, so a point can be inside it (an open dome cannot). */
  closed: boolean;
  inked: boolean;
  /** World triangles, 9 numbers each. */
  tris: Float64Array;
  /** World sample points (vertices), 3 numbers each. */
  samples: Float64Array;
  min: Vec3;
  max: Vec3;
  centre: Vec3;
  /** The longest bounding edge. */
  size: number;
  /** Half the bounding diagonal. */
  radius: number;
  area: number;
}

const geoCache = new WeakMap<Part, THREE.BufferGeometry>();
function geometryOf(p: Part): THREE.BufferGeometry {
  let g = geoCache.get(p);
  if (!g) {
    g = partGeometry(p).geometry;
    geoCache.set(p, g);
  }
  return g;
}

function worldTris(g: THREE.BufferGeometry, m: THREE.Matrix4): Float64Array {
  const pos = g.getAttribute('position');
  const e = m.elements;
  const n = pos.count;
  const out = new Float64Array(n * 3);
  for (let i = 0; i < n; i++) {
    const x = pos.getX(i);
    const y = pos.getY(i);
    const z = pos.getZ(i);
    out[i * 3] = e[0]! * x + e[4]! * y + e[8]! * z + e[12]!;
    out[i * 3 + 1] = e[1]! * x + e[5]! * y + e[9]! * z + e[13]!;
    out[i * 3 + 2] = e[2]! * x + e[6]! * y + e[10]! * z + e[14]!;
  }
  return out;
}

function labelOf(p: Part, i: number): string {
  if (p.name) return p.name;
  const mat = typeof p.mat === 'string' ? p.mat : 'inline';
  return `${p.bone ?? 'tool'}:${p.geo.type}:${mat}#${i}`;
}

/** Whether the triangles form a closed surface (each edge used twice). Degenerate edges (a pole) are skipped. */
function isClosed(tris: Float64Array): boolean {
  const key = (o: number): string => `${Math.round(tris[o]! * 1e4)},${Math.round(tris[o + 1]! * 1e4)},${Math.round(tris[o + 2]! * 1e4)}`;
  const edges = new Map<string, number>();
  for (let t = 0; t < tris.length; t += 9) {
    const k = [key(t), key(t + 3), key(t + 6)];
    for (let i = 0; i < 3; i++) {
      const a = k[i]!, b = k[(i + 1) % 3]!;
      if (a === b) continue;
      const e = a < b ? `${a}|${b}` : `${b}|${a}`;
      edges.set(e, (edges.get(e) ?? 0) + 1);
    }
  }
  if (edges.size === 0) return false;
  for (const n of edges.values()) if (n !== 2) return false;
  return true;
}

function makePiece(path: string, label: string, bone: string, p: Part, tris: Float64Array): Piece {
  const n = tris.length / 3;
  const min: Vec3 = [Infinity, Infinity, Infinity];
  const max: Vec3 = [-Infinity, -Infinity, -Infinity];
  for (let v = 0; v < n; v++) {
    for (let k = 0; k < 3; k++) {
      const c = tris[v * 3 + k]!;
      if (c < min[k]!) min[k] = c;
      if (c > max[k]!) max[k] = c;
    }
  }
  const stride = Math.max(1, Math.floor(n / SAMPLES));
  const picks: number[] = [];
  for (let v = 0; v < n; v += stride) picks.push(tris[v * 3]!, tris[v * 3 + 1]!, tris[v * 3 + 2]!);
  let area = 0;
  for (let t = 0; t + 8 < tris.length; t += 9) {
    const ux = tris[t + 3]! - tris[t]!, uy = tris[t + 4]! - tris[t + 1]!, uz = tris[t + 5]! - tris[t + 2]!;
    const vx = tris[t + 6]! - tris[t]!, vy = tris[t + 7]! - tris[t + 1]!, vz = tris[t + 8]! - tris[t + 2]!;
    area += 0.5 * Math.hypot(uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx);
  }
  const mat = typeof p.mat === 'string' ? p.mat : 'inline';
  const size = Math.max(max[0]! - min[0]!, max[1]! - min[1]!, max[2]! - min[2]!);
  const diag = Math.hypot(max[0]! - min[0]!, max[1]! - min[1]!, max[2]! - min[2]!);
  const inked = MATTE.includes(mat) && (p.outline ?? mat !== 'ink') && bboxRadius(p) >= 0.03;
  return {
    path, label, bone, part: p, mat,
    surface: p.geo.type === 'plane' || p.geo.type === 'ring',
    closed: isClosed(tris),
    inked,
    tris,
    samples: Float64Array.from(picks),
    min, max,
    centre: [(min[0]! + max[0]!) / 2, (min[1]! + max[1]!) / 2, (min[2]! + max[2]!) / 2],
    size,
    radius: diag / 2,
    area,
  };
}

/** The part's own bounding radius (the same test the rig uses to ink a part). */
function bboxRadius(p: Part): number {
  const g = geometryOf(p);
  g.computeBoundingSphere();
  return g.boundingSphere?.radius ?? 0;
}

// ---- Bones: the rest pose and the posed bone matrices (rig: position = rest + pos, rotation XYZ, uniform scale) ----

interface BonePose {
  pos: Vec3;
  rot: Vec3;
  scale: number;
}

function boneMatrices(s: Species, pose?: Map<string, BonePose>): Map<string, THREE.Matrix4> {
  const out = new Map<string, THREE.Matrix4>();
  const identity = new THREE.Matrix4();
  const world = (name: string, depth = 0): THREE.Matrix4 => {
    const hit = out.get(name);
    if (hit) return hit;
    const b = s.bones[name];
    if (!b || name === 'root' || depth > 32) return identity;
    const p = pose?.get(name);
    const pos = new THREE.Vector3(b.at[0] + (p?.pos[0] ?? 0), b.at[1] + (p?.pos[1] ?? 0), b.at[2] + (p?.pos[2] ?? 0));
    const rot = p?.rot ?? [0, 0, 0];
    const q = new THREE.Quaternion().setFromEuler(new THREE.Euler(rot[0], rot[1], rot[2], 'XYZ'));
    const sc = p?.scale ?? 1;
    const local = new THREE.Matrix4().compose(pos, q, new THREE.Vector3(sc, sc, sc));
    const m = world(b.parent, depth + 1).clone().multiply(local);
    out.set(name, m);
    return m;
  };
  for (const name of Object.keys(s.bones)) world(name);
  return out;
}

function clipFor(s: Species, id: string, shared: Record<string, Clip>): Clip | undefined {
  const own = s.overrides?.[id];
  if (own && s.clips?.[own]) return s.clips[own];
  return s.clips?.[id] ?? shared[id];
}

/** The pose of a clip at its mid-cycle (the first looping channel at half a period); one-shots at their peak. */
function poseOf(clip: Clip, gait: Gait): Map<string, BonePose> {
  const ch = clip.channels.find((c) => c.rate === undefined && resolve(c.freq, gait, 0) > 0);
  const f = ch ? resolve(ch.freq, gait, 0) : 0;
  const t = f > 0 ? 0.5 / f : 0;
  const elapsed = clip.loop === false && clip.dur ? clip.dur / 2 : 0;
  const out = new Map<string, BonePose>();
  for (const [key, { value }] of evalClip(clip, t, gait, elapsed)) {
    const [bone, prop] = key.split('|');
    if (!bone || !prop) continue;
    const p = out.get(bone) ?? { pos: [0, 0, 0] as Vec3, rot: [0, 0, 0] as Vec3, scale: 1 };
    const axis = { x: 0, y: 1, z: 2 }[prop.slice(-1)] ?? 0;
    if (prop.startsWith('rot.')) p.rot[axis] = value;
    else if (prop.startsWith('pos.')) p.pos[axis] = value;
    else if (prop === 'scale') p.scale = value;
    out.set(bone, p);
  }
  return out;
}

function piecesOf(parts: Part[], prefix: string, matOf: (p: Part) => THREE.Matrix4): Piece[] {
  return parts.map((p, i) => makePiece(`${prefix}[${i}]`, labelOf(p, i), p.bone ?? 'root', p, worldTris(geometryOf(p), matOf(p))));
}

function speciesPieces(s: Species, bones: Map<string, THREE.Matrix4>): Piece[] {
  return piecesOf(s.parts, 'parts', (p) => bones.get(p.bone ?? 'root') ?? new THREE.Matrix4());
}

// ---- Geometry queries: distance to a triangle, ray hits, containment (closed pieces only) ----

/** Squared distance from p to triangle t (Ericson, real-time collision detection). */
function triDistSq(t: Float64Array, o: number, px: number, py: number, pz: number): number {
  const ax = t[o]!, ay = t[o + 1]!, az = t[o + 2]!;
  const bx = t[o + 3]!, by = t[o + 4]!, bz = t[o + 5]!;
  const cx = t[o + 6]!, cy = t[o + 7]!, cz = t[o + 8]!;
  const abx = bx - ax, aby = by - ay, abz = bz - az;
  const acx = cx - ax, acy = cy - ay, acz = cz - az;
  const apx = px - ax, apy = py - ay, apz = pz - az;
  let qx = ax, qy = ay, qz = az;
  const d1 = abx * apx + aby * apy + abz * apz;
  const d2 = acx * apx + acy * apy + acz * apz;
  const bpx = px - bx, bpy = py - by, bpz = pz - bz;
  const d3 = abx * bpx + aby * bpy + abz * bpz;
  const d4 = acx * bpx + acy * bpy + acz * bpz;
  const cpx = px - cx, cpy = py - cy, cpz = pz - cz;
  const d5 = abx * cpx + aby * cpy + abz * cpz;
  const d6 = acx * cpx + acy * cpy + acz * cpz;
  const vc = d1 * d4 - d3 * d2;
  const vb = d5 * d2 - d1 * d6;
  const va = d3 * d6 - d5 * d4;
  if (d1 <= 0 && d2 <= 0) {
    qx = ax; qy = ay; qz = az;
  } else if (d3 >= 0 && d4 <= d3) {
    qx = bx; qy = by; qz = bz;
  } else if (vc <= 0 && d1 >= 0 && d3 <= 0) {
    const v = d1 / (d1 - d3);
    qx = ax + v * abx; qy = ay + v * aby; qz = az + v * abz;
  } else if (d6 >= 0 && d5 <= d6) {
    qx = cx; qy = cy; qz = cz;
  } else if (vb <= 0 && d2 >= 0 && d6 <= 0) {
    const w = d2 / (d2 - d6);
    qx = ax + w * acx; qy = ay + w * acy; qz = az + w * acz;
  } else if (va <= 0 && d4 - d3 >= 0 && d5 - d6 >= 0) {
    const w = (d4 - d3) / (d4 - d3 + (d5 - d6));
    qx = bx + w * (cx - bx); qy = by + w * (cy - by); qz = bz + w * (cz - bz);
  } else {
    const den = 1 / (va + vb + vc);
    const v = vb * den, w = vc * den;
    qx = ax + abx * v + acx * w; qy = ay + aby * v + acy * w; qz = az + abz * v + acz * w;
  }
  const dx = px - qx, dy = py - qy, dz = pz - qz;
  return dx * dx + dy * dy + dz * dz;
}

/** The distance from p to the piece's surface. */
function distTo(pc: Piece, px: number, py: number, pz: number): number {
  let best = Infinity;
  for (let o = 0; o < pc.tris.length; o += 9) best = Math.min(best, triDistSq(pc.tris, o, px, py, pz));
  return Math.sqrt(best);
}

/** Möller–Trumbore, both faces: the ray parameter of a hit, or null. */
function rayTri(t: Float64Array, o: number, ox: number, oy: number, oz: number, dx: number, dy: number, dz: number): number | null {
  const ax = t[o]!, ay = t[o + 1]!, az = t[o + 2]!;
  const e1x = t[o + 3]! - ax, e1y = t[o + 4]! - ay, e1z = t[o + 5]! - az;
  const e2x = t[o + 6]! - ax, e2y = t[o + 7]! - ay, e2z = t[o + 8]! - az;
  const hx = dy * e2z - dz * e2y, hy = dz * e2x - dx * e2z, hz = dx * e2y - dy * e2x;
  const det = e1x * hx + e1y * hy + e1z * hz;
  if (Math.abs(det) < 1e-12) return null;
  const f = 1 / det;
  const sx = ox - ax, sy = oy - ay, sz = oz - az;
  const u = f * (sx * hx + sy * hy + sz * hz);
  if (u < 0 || u > 1) return null;
  const qx = sy * e1z - sz * e1y, qy = sz * e1x - sx * e1z, qz = sx * e1y - sy * e1x;
  const v = f * (dx * qx + dy * qy + dz * qz);
  if (v < 0 || u + v > 1) return null;
  return f * (e2x * qx + e2y * qy + e2z * qz);
}

/** Every hit (t ≥ 0) of a ray on the pieces' surfaces, sorted. */
export function rayHits(list: Piece[], o: Vec3, d: Vec3): number[] {
  const out: number[] = [];
  for (const pc of list) {
    for (let t = 0; t < pc.tris.length; t += 9) {
      const hit = rayTri(pc.tris, t, o[0], o[1], o[2], d[0], d[1], d[2]);
      if (hit !== null && hit >= -EPS) out.push(hit);
    }
  }
  return out.sort((a, b) => a - b);
}

/** Inside a closed piece (parity of crossings along a fixed direction). Surfaces are never inside. */
function insideOf(pc: Piece, p: Vec3): boolean {
  if (!pc.closed) return false;
  let n = 0;
  for (let t = 0; t < pc.tris.length; t += 9) {
    const hit = rayTri(pc.tris, t, p[0], p[1], p[2], RAY_DIR[0], RAY_DIR[1], RAY_DIR[2]);
    if (hit !== null && hit > EPS) n++;
  }
  return n % 2 === 1;
}

/** Whether a point is inside any closed piece of the list. */
function insideAny(list: Piece[], p: Vec3): Piece | null {
  for (const pc of list) if (insideOf(pc, p)) return pc;
  return null;
}

/** The gap between two pieces: 0 when one samples inside the other, else the nearest sample-to-surface distance. */
function gapOf(a: Piece, b: Piece): number {
  const reach = a.radius + b.radius + 0.5;
  if (Math.hypot(a.centre[0] - b.centre[0], a.centre[1] - b.centre[1], a.centre[2] - b.centre[2]) > reach) return Infinity;
  let best = Infinity;
  for (const [x, y] of [[a, b], [b, a]] as const) {
    for (const p of samplesOf(x)) {
      if (insideOf(y, p)) return 0;
      best = Math.min(best, distTo(y, p[0], p[1], p[2]));
    }
  }
  return best;
}

function samplesOf(pc: Piece): Vec3[] {
  const out: Vec3[] = [];
  for (let i = 0; i < pc.samples.length; i += 3) out.push([pc.samples[i]!, pc.samples[i + 1]!, pc.samples[i + 2]!]);
  return out;
}

function volumeOf(pc: Piece): number {
  return (pc.max[0] - pc.min[0]) * (pc.max[1] - pc.min[1]) * (pc.max[2] - pc.min[2]);
}

export function transformPoint(m: THREE.Matrix4, p: Vec3): Vec3 {
  const v = new THREE.Vector3(p[0], p[1], p[2]).applyMatrix4(m);
  return [v.x, v.y, v.z];
}

function transformDir(m: THREE.Matrix4, d: Vec3): Vec3 {
  const e = m.elements;
  return normal([
    e[0]! * d[0] + e[4]! * d[1] + e[8]! * d[2],
    e[1]! * d[0] + e[5]! * d[1] + e[9]! * d[2],
    e[2]! * d[0] + e[6]! * d[1] + e[10]! * d[2],
  ]);
}

// ---- The checks ----

/** The eyes: socket on the head surface, pupil inside the socket, socket inside the silhouette, eye vs eye. */
function checkEyes(s: Species, pieces: Piece[], bones: Map<string, THREE.Matrix4>, issues: Issue[]): void {
  const sockets: { e: Eye; k: number; c: Vec3; r: number }[] = [];
  s.eyes.forEach((e, k) => {
    const bone = bones.get(e.bone) ?? new THREE.Matrix4();
    const c = transformPoint(bone, e.at);
    const dir = transformDir(bone, [0, 0, 1]);
    const r = e.r ?? 0.1;
    const path = `eyes[${k}]`;
    const label = `eye ${e.bone} ${e.shape} #${k}`;
    sockets.push({ e, k, c, r });
    const onBone = pieces.filter((p) => p.bone === e.bone);
    const primary = largestSolid(onBone);
    if (!primary) {
      issues.push({ kind: 'eye-fit', severity: 'error', path, label, pose: 'rest', message: `no solid head on bone "${e.bone}" to seat the eye` });
      return;
    }
    // Fit: a socket is a flat disc, and the eye format has no rotation, so it faces +z (bones carry no rotation).
    // The head surface must lie behind the disc at its centre and at every rim point (else it clips the rim), and
    // the centre may float off the surface by at most 25% of r.
    // The surface is the union of every solid on the eye's bone (a disc on a bone with a head box is part of the head).
    const solids = onBone.filter((p) => !p.surface && p.mat !== 'clear' && p.mat !== 'glass');
    const surfaceZ = (x: number, y: number): number | undefined => {
      const h = rayHits(solids, [x, y, c[2] + 10], [0, 0, -1])[0];
      return h === undefined ? undefined : c[2] + 10 - h;
    };
    const samples: [number, number][] = [[c[0], c[1]]];
    for (let i = 0; i < 8; i++) samples.push([c[0] + r * Math.cos((i / 8) * Math.PI * 2), c[1] + r * Math.sin((i / 8) * Math.PI * 2)]);
    const centreZ = surfaceZ(c[0], c[1]);
    const clips = samples.map(([x, y]) => surfaceZ(x, y)).filter((z): z is number => z !== undefined && z > c[2] + SEAT_SLACK);
    if (centreZ === undefined) {
      issues.push({ kind: 'eye-fit', severity: 'error', path, label, pose: 'rest', message: 'no head surface in front of the socket centre (the eye floats in the air)' });
    } else if (clips.length > 0) {
      const worst = Math.max(...clips) - c[2];
      issues.push({
        kind: 'eye-fit', severity: 'error', path, label, pose: 'rest', value: worst,
        message: `the head surface clips the socket: ${clips.length} of ${samples.length} points lie up to ${worst.toFixed(3)} in front of the disc (seat the socket at the rim's highest surface plus ${SEAT_EPS})`,
      });
    } else if (c[2] - centreZ > EYE_FIT * r) {
      issues.push({
        kind: 'eye-fit', severity: 'error', path, label, pose: 'rest', value: c[2] - centreZ,
        message: `socket floats ${(c[2] - centreZ).toFixed(3)} off the head (limit ${(EYE_FIT * r).toFixed(3)} = 50% of r ${r})`,
      });
    }
    // Pupil: it must sit in its socket (its footprint inside the glass radius).
    const hw = (e.shape === 'visor' ? 1.5 : 1) * e.pupil;
    const hh = (e.shape === 'bar' ? 0.25 : e.shape === 'visor' ? 0.45 : 1) * e.pupil * (e.stretch ?? 1);
    if (e.glass !== false && (hw > r * 1.02 || hh > r * 1.02)) {
      issues.push({ kind: 'eye-pupil', severity: 'warn', path, label, pose: 'rest', value: Math.max(hw, hh) / r, message: `pupil ${e.pupil} (${e.shape}) spills out of the socket r ${r}` });
    }
    // Silhouette: the socket's rim must be over the head, seen from the front.
    const up = Math.abs(dir[0]) < 0.9 ? normal(cross(dir, [1, 0, 0])) : normal(cross(dir, [0, 1, 0]));
    const side = cross(dir, up);
    let missing = 0;
    for (let i = 0; i < 8; i++) {
      const a = (i / 8) * Math.PI * 2;
      const p: Vec3 = [c[0] + r * (Math.cos(a) * up[0] + Math.sin(a) * side[0]), c[1] + r * (Math.cos(a) * up[1] + Math.sin(a) * side[1]), c[2] + r * (Math.cos(a) * up[2] + Math.sin(a) * side[2])];
      const from: Vec3 = [p[0] + dir[0] * 10, p[1] + dir[1] * 10, p[2] + dir[2] * 10];
      if (rayHits([primary], from, [-dir[0], -dir[1], -dir[2]]).length === 0) missing++;
    }
    if (missing >= 2) {
      issues.push({ kind: 'eye-silhouette', severity: 'error', path, label, pose: 'rest', value: missing, message: `${missing} of 8 rim points of the socket are off the head's silhouette` });
    }
    // Overlap: a face feature (not a plate) lying under the socket's footprint.
    const features = onBone.filter((p) => p.mat !== 'clear' && p !== primary && p.mat !== 'paper');
    for (const f of features) {
      let hits = 0;
      for (const p of samplesOf(f)) {
        const lx = p[0] - c[0], ly = p[1] - c[1], lz = p[2] - c[2];
        const a = lx * dir[0] + ly * dir[1] + lz * dir[2];
        const lat = Math.sqrt(Math.max(0, lx * lx + ly * ly + lz * lz - a * a));
        if (lat <= 0.9 * r && a >= -0.3 * r && a <= 0.5 * r) hits++;
      }
      if (hits > 0) {
        issues.push({ kind: 'eye-overlap', severity: 'error', path, label, pose: 'rest', value: hits, message: `overlaps ${f.label} (${hits} sample points inside its footprint)` });
      }
    }
  });
  // Eye vs eye: sockets on the same face must not overlap.
  for (let i = 0; i < sockets.length; i++) {
    for (let j = i + 1; j < sockets.length; j++) {
      const a = sockets[i]!, b = sockets[j]!;
      const d = Math.hypot(a.c[0] - b.c[0], a.c[1] - b.c[1], a.c[2] - b.c[2]);
      if (d < 0.9 * (a.r + b.r)) {
        issues.push({ kind: 'eye-pair', severity: 'error', path: `eyes[${a.k}]`, label: `eye #${a.k} / eye #${b.k}`, pose: 'rest', value: d, message: `eye #${a.k} and eye #${b.k} overlap (centres ${d.toFixed(3)} apart, radii ${a.r} and ${b.r})` });
      }
    }
  }
}

// ---- Eye visibility: from the portrait cameras (the contact sheets' front and q34), no part may hide the pupil,
// and at least EYE_CLEAR_MIN of the EYE_PROBES probes (the pupil centre and four rim points) must be clear. The rays
// run against the real triangles of every part, posed by the clips, so no bounding box stands in for a part.

/** The portrait cameras of the contact sheets: robot-lab.ts CAM_POS, the `one` line-up, looking at LAB_TARGET. Keep in step. */
export const PORTRAIT_CAMS: Record<'front' | 'q34', Vec3> = { front: [0, 1.1, 4.4], q34: [-2.44, 1.84, 3.48] };
export const LAB_TARGET: Vec3 = [0, 0.7, 0];
/** The pupil sits this far in front of its socket (geometry.ts eyeGeometries). */
export const PUPIL_LIFT = 0.004;
/** The probes of an eye: the pupil centre, then four rim points at 85% of the socket radius (an inset of 15%). */
export const EYE_PROBES = 5;
/** Of the probes, at least this many must be clear from each portrait camera (the pupil always must be). */
export const EYE_CLEAR_MIN = 4;
const RIM_INSET = 0.85;
/** A part hides a point only when it is at least this much nearer the camera than the point. */
const HIDE_GAP = 0.002;
/** A rim point this far in front of the socket disc counts as clipped (the rounding of the seat). */
const SEAT_SLACK = 0.001;
/** The seat tool lifts a socket this far clear of the highest rim point (eyes: the rim is flush, the disc never behind it). */
export const SEAT_EPS = 0.005;

export interface Scene {
  bones: Map<string, THREE.Matrix4>;
  pieces: Piece[];
}

export type Cam = keyof typeof PORTRAIT_CAMS;

export interface EyeView {
  cam: Cam;
  /** Per probe: the piece that hides it from this camera, or null when it is clear. */
  hiders: (Piece | null)[];
}

/** The rest scene, and the walk and wave scenes (the clips at mid-cycle, as the audit poses them). */
export function sceneOf(s: Species, clips: Record<string, Clip>): Partial<Record<PoseName, Scene>> {
  const out: Partial<Record<PoseName, Scene>> = {};
  const rest = boneMatrices(s);
  out.rest = { bones: rest, pieces: speciesPieces(s, rest) };
  const walk = clipFor(s, `walk.${s.locomotion}`, clips) ?? clipFor(s, 'walk.treads', clips);
  const wave = clipFor(s, 'wave', clips);
  const posed = (clip: Clip): Scene => {
    const bones = boneMatrices(s, poseOf(clip, s.gait));
    return { bones, pieces: speciesPieces(s, bones) };
  };
  if (walk) out.walk = posed(walk);
  if (wave) out.wave = posed(wave);
  return out;
}

/** The nearest piece that hides `to` from `from` (its surface is nearer than `to` by HIDE_GAP or more), or null. Clear parts do not hide. */
export function hiderOf(list: Piece[], from: Vec3, to: Vec3): Piece | null {
  const d: Vec3 = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
  const limit = 1 - HIDE_GAP / (Math.hypot(d[0], d[1], d[2]) || 1);
  let best: Piece | null = null;
  let bestT = limit;
  for (const pc of list) {
    if (pc.mat === 'clear') continue;
    for (let t = 0; t < pc.tris.length; t += 9) {
      const hit = rayTri(pc.tris, t, from[0], from[1], from[2], d[0], d[1], d[2]);
      if (hit !== null && hit > EPS && hit < bestT) {
        bestT = hit;
        best = pc;
      }
    }
  }
  return best;
}

/** An eye's probes in world space: the pupil centre, then the rim (the socket radius, or the pupil's without a socket). */
export function eyeProbes(e: Eye, bone: THREE.Matrix4): Vec3[] {
  const base = e.glass === false ? e.pupil : (e.r ?? 0.1);
  const rr = base * RIM_INSET;
  const pts: Vec3[] = [transformPoint(bone, [e.at[0], e.at[1], e.at[2] + PUPIL_LIFT])];
  for (let i = 0; i < EYE_PROBES - 1; i++) {
    const a = (i * Math.PI) / 2;
    pts.push(transformPoint(bone, [e.at[0] + rr * Math.cos(a), e.at[1] + rr * Math.sin(a), e.at[2]]));
  }
  return pts;
}

/** The eye's probes seen from each portrait camera, with what hides each one. */
export function eyeViews(e: Eye, bone: THREE.Matrix4, list: Piece[]): EyeView[] {
  const probes = eyeProbes(e, bone);
  return (Object.keys(PORTRAIT_CAMS) as Cam[]).map((cam) => ({
    cam,
    hiders: probes.map((p) => hiderOf(list, PORTRAIT_CAMS[cam], p)),
  }));
}

/** Whether an eye is seen: from each portrait camera its pupil is clear and at least EYE_CLEAR_MIN probes are. */
export function eyeSeen(views: EyeView[]): boolean {
  return views.every((v) => v.hiders[0] === null && clearOf(v) >= EYE_CLEAR_MIN);
}

function clearOf(v: EyeView): number {
  return v.hiders.filter((h) => h === null).length;
}

/** The visibility check (eye-occluded): every pose, both portrait cameras. Names the part that hides the pupil or the rim. */
function checkEyeVisibility(s: Species, scene: Scene, pose: PoseName, issues: Issue[]): void {
  s.eyes.forEach((e, k) => {
    const bone = scene.bones.get(e.bone) ?? new THREE.Matrix4();
    const faults: string[] = [];
    let clearMin = EYE_PROBES;
    for (const v of eyeViews(e, bone, scene.pieces)) {
      const clear = clearOf(v);
      if (v.hiders[0] === null && clear >= EYE_CLEAR_MIN) continue;
      clearMin = Math.min(clearMin, clear);
      const who = [...new Set(v.hiders.filter((h): h is Piece => h !== null).map((h) => `${h.path} '${h.label}'`))].join(', ');
      faults.push(v.hiders[0] !== null
        ? `pupil hidden from the ${v.cam} camera by ${who}`
        : `only ${clear} of ${EYE_PROBES} probes clear from the ${v.cam} camera (hidden by ${who})`);
    }
    if (faults.length) {
      issues.push({ kind: 'eye-occluded', severity: 'error', path: `eyes[${k}]`, label: `eye ${e.bone} ${e.shape} #${k}`, pose, value: clearMin, message: faults.join('; ') });
    }
  });
}

function cross(a: Vec3, b: Vec3): Vec3 {
  return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
}

/** The biggest closed piece on a bone (the head shell or the body), or null. */
function largestSolid(list: Piece[]): Piece | null {
  let best: Piece | null = null;
  for (const pc of list) {
    if (pc.surface || pc.mat === 'clear' || pc.mat === 'glass') continue;
    if (!best || volumeOf(pc) > volumeOf(best)) best = pc;
  }
  return best;
}

/** Face features (anything on the head bone other than its shell): on the surface, not buried. */
function checkFeatures(pieces: Piece[], bonesWithFace: Set<string>, issues: Issue[]): void {
  for (const bone of bonesWithFace) {
    const onBone = pieces.filter((p) => p.bone === bone);
    const primary = largestSolid(onBone);
    if (!primary) continue;
    for (const f of onBone) {
      if (f === primary || f.mat === 'clear') continue;
      const others = onBone.filter((p) => p !== f);
      let gap = Infinity;
      for (const o of others) gap = Math.min(gap, gapOf(f, o));
      if (gap > CONTACT) {
        issues.push({ kind: 'feature-float', severity: 'error', path: f.path, label: f.label, pose: 'rest', value: gap, message: `floats ${gap.toFixed(3)} off the head (nothing on the bone touches it)` });
      }
      if (!f.surface) {
        let depth = 0;
        for (const p of samplesOf(f)) {
          if (insideOf(primary, p)) depth = Math.max(depth, distTo(primary, p[0], p[1], p[2]));
        }
        const ratio = depth / Math.max(f.size, 1e-6);
        if (ratio > BURIED) {
          issues.push({ kind: 'feature-buried', severity: 'warn', path: f.path, label: f.label, pose: 'rest', value: ratio, message: `buried ${(ratio * 100).toFixed(0)}% of its size in the head (limit 30%)` });
        }
      }
    }
  }
}

/** Every part touches or intersects a part on its bone, its parent bone or its child bones. */
function checkAttach(s: Species, pieces: Piece[], pose: PoseName, issues: Issue[]): void {
  // A bone's relatives: itself, its parent, its children. Root-level bones (base, body) form the trunk and
  // are all related to each other, since the body sits on its base without being its child.
  const related = (bone: string): Set<string> => {
    const out = new Set<string>([bone]);
    const parent = s.bones[bone]?.parent;
    if (parent && parent !== 'root') out.add(parent);
    const trunk = !parent || parent === 'root';
    for (const [name, b] of Object.entries(s.bones)) {
      if (b.parent === bone) out.add(name);
      if (trunk && b.parent === 'root') out.add(name);
    }
    return out;
  };
  for (const pc of pieces) {
    const near = related(pc.bone);
    let gap = Infinity;
    for (const o of pieces) {
      if (o === pc || !near.has(o.bone)) continue;
      gap = Math.min(gap, gapOf(pc, o));
      if (gap <= CONTACT) break;
    }
    if (gap > CONTACT) {
      issues.push({ kind: 'attach', severity: 'error', path: pc.path, label: pc.label, pose, value: gap, message: `floats ${Number.isFinite(gap) ? gap.toFixed(3) : 'free'} from every part on its bone, its parent and its children` });
    }
  }
}

/** Limbs (the body's descendants other than the body's head and carry bones) must not sink into the body. */
function checkLimbs(s: Species, pieces: Piece[], pose: PoseName, issues: Issue[]): void {
  const descends = (bone: string): boolean => {
    let b: string | undefined = bone;
    for (let d = 0; b && b !== 'root' && d < 32; d++) {
      if (b === 'body') return true;
      b = s.bones[b]?.parent;
    }
    return false;
  };
  const body = pieces.filter((p) => p.bone === 'body');
  for (const pc of pieces) {
    if (pc.bone === 'body' || pc.bone === 'carry' || !descends(pc.bone)) continue;
    let deepest = 0;
    for (const p of samplesOf(pc)) {
      const inside = insideAny(body, p);
      if (inside) deepest = Math.max(deepest, distTo(inside, p[0], p[1], p[2]));
    }
    if (deepest > LIMB) {
      issues.push({ kind: 'limb-through-body', severity: 'warn', path: pc.path, label: pc.label, pose, value: deepest, message: `sinks ${deepest.toFixed(3)} into the body in the ${pose} pose` });
    }
  }
}

/** Mirror partners: a part off the centre line needs a twin at −x (same geometry, size and material), unless `asym`. */
function checkSymmetry(pieces: Piece[], issues: Issue[]): void {
  for (const pc of pieces) {
    if (Math.abs(pc.centre[0]) <= SYM || pc.part.asym) continue;
    const twin = pieces.find((o) =>
      o !== pc && o.part.geo.type === pc.part.geo.type && o.mat === pc.mat &&
      Math.abs(o.centre[0] + pc.centre[0]) <= SYM && Math.abs(o.centre[1] - pc.centre[1]) <= SYM && Math.abs(o.centre[2] - pc.centre[2]) <= SYM &&
      Math.abs(o.size - pc.size) <= SYM + 0.01);
    if (!twin) {
      issues.push({ kind: 'symmetry', severity: 'error', path: pc.path, label: pc.label, pose: 'rest', message: `no mirror partner at x ${(-pc.centre[0]).toFixed(3)} (set "asym": true if intended)` });
    }
  }
}

/** The eyes' mirror check uses their world sockets, so it runs with the bones. */
function checkEyeSymmetry(s: Species, bones: Map<string, THREE.Matrix4>, issues: Issue[]): void {
  const socket = (e: Eye): Vec3 => transformPoint(bones.get(e.bone) ?? new THREE.Matrix4(), e.at);
  s.eyes.forEach((e, k) => {
    if (e.asym) return;
    const c = socket(e);
    if (Math.abs(c[0]) <= SYM) return;
    const twin = s.eyes.findIndex((o, j) => {
      if (j === k) return false;
      const d = socket(o);
      return Math.abs(d[0] + c[0]) <= SYM && Math.abs(d[1] - c[1]) <= SYM && Math.abs(d[2] - c[2]) <= SYM && (o.r ?? 0.1) === (e.r ?? 0.1) && o.shape === e.shape && o.pupil === e.pupil;
    });
    if (twin < 0) issues.push({ kind: 'symmetry', severity: 'error', path: `eyes[${k}]`, label: `eye ${e.bone} ${e.shape} #${k}`, pose: 'rest', message: 'no matching eye at the mirror position (or not the same size and shape); set "asym": true if intended' });
  });
}

/** The lowest point of a grounded species sits on the floor; a floating one must not sink into it. */
function checkGround(s: Species, pieces: Piece[], issues: Issue[]): void {
  const low = Math.min(...pieces.map((p) => p.min[1]));
  if (!s.floats && Math.abs(low) > GROUND) {
    issues.push({ kind: 'ground', severity: 'error', path: 'parts', label: 'lowest point', pose: 'rest', value: low, message: `lowest point at y ${low.toFixed(3)}, not 0 (feet ${low > 0 ? 'float' : 'sink'})` });
  } else if (s.floats && low < -GROUND) {
    issues.push({ kind: 'ground', severity: 'error', path: 'parts', label: 'lowest point', pose: 'rest', value: low, message: `floats but sinks to y ${low.toFixed(3)}` });
  }
}

/** A held tool (one hand, or two) must not pierce the head or body. The grip's frame is the rig's (holdTool). */
function checkTools(bones: Map<string, THREE.Matrix4>, tools: Record<string, Tool>, pieces: Piece[], issues: Issue[]): void {
  const body = pieces.filter((p) => (p.bone === 'head' || p.bone === 'body') && !p.surface);
  const gripR = bones.get('gripR') ?? new THREE.Matrix4();
  const gripL = bones.get('gripL') ?? new THREE.Matrix4();
  const originR = transformPoint(gripR, [0, 0, 0]);
  const originL = transformPoint(gripL, [0, 0, 0]);
  const byGrip = new Map<string, { n: number; worst: null | { id: string; depth: number; into: string }; names: string[] }>();
  for (const [id, tool] of Object.entries(tools)) {
    if (tool.grip !== 'one' && tool.grip !== 'two') continue;
    const m = new THREE.Matrix4();
    if (tool.grip === 'two') {
      const dir = new THREE.Vector3(originL[0] - originR[0], originL[1] - originR[1], originL[2] - originR[2]).normalize();
      const q = new THREE.Quaternion().setFromUnitVectors(new THREE.Vector3(1, 0, 0), dir);
      m.compose(new THREE.Vector3(...originR), q, new THREE.Vector3(1, 1, 1));
    } else {
      const aim = tool.aim ?? 'forward';
      const rx = aim === 'forward' ? -Math.PI / 2 : aim === 'up' ? Math.PI : 0;
      const q = new THREE.Quaternion().setFromEuler(new THREE.Euler(rx, 0, 0, 'XYZ'));
      m.compose(new THREE.Vector3(...originR), q, new THREE.Vector3(1, 1, 1));
    }
    const parts = piecesOf(tool.parts, `tools/${id}.json parts`, () => m);
    let worst: { depth: number; tool: Piece; into: Piece } | null = null;
    for (const tp of parts) {
      for (const bp of body) {
        if (tp.max[0] < bp.min[0] - 0.02 || tp.min[0] > bp.max[0] + 0.02 || tp.max[1] < bp.min[1] - 0.02 || tp.min[1] > bp.max[1] + 0.02 || tp.max[2] < bp.min[2] - 0.02 || tp.min[2] > bp.max[2] + 0.02) continue;
        for (const p of samplesOf(tp)) {
          if (insideOf(bp, p)) {
            const depth = distTo(bp, p[0], p[1], p[2]);
            if (depth > (worst?.depth ?? 0)) worst = { depth, tool: tp, into: bp };
          }
        }
      }
    }
    if (worst && worst.depth > PIERCE) {
      const hit = byGrip.get(tool.grip) ?? { n: 0, worst: null as null | { id: string; depth: number; into: string }, names: [] as string[] };
      hit.n++;
      hit.names.push(id);
      if (!hit.worst || worst.depth > hit.worst.depth) hit.worst = { id, depth: worst.depth, into: worst.into.label };
      byGrip.set(tool.grip, hit);
    }
  }
  // One issue per grip: the count of tools that pierce, and the worst of them.
  for (const [grip, hit] of byGrip) {
    const w = hit.worst!;
    issues.push({
      kind: 'tool', severity: 'error', path: 'parts', label: `${grip}-hand grip`, pose: 'rest', value: w.depth,
      message: `${hit.n} ${grip}-handed tool(s) pierce the head or body (${hit.names.join(', ')}); worst ${w.id} into ${w.into} by ${w.depth.toFixed(3)}`,
    });
  }
}

/** The metrics: the numbers the style guide is written from. */
export function metrics(s: Species): Metrics {
  const bones = boneMatrices(s);
  const pieces = speciesPieces(s, bones);
  const span = (list: Piece[], k: 0 | 1 | 2): number => (list.length ? Math.max(...list.map((p) => p.max[k])) - Math.min(...list.map((p) => p.min[k])) : 0);
  const head = pieces.filter((p) => p.bone === 'head');
  const body = pieces.filter((p) => p.bone === 'body');
  const height = span(pieces, 1);
  const headHeight = span(head, 1);
  const headWidth = span(head, 0);
  const headMinY = head.length ? Math.min(...head.map((p) => p.min[1])) : 0;
  const sockets = s.eyes.map((e) => ({ r: e.r ?? 0.1, c: transformPoint(bones.get(e.bone) ?? new THREE.Matrix4(), e.at) }));
  const nearest = sockets.map((a, i) => Math.min(...sockets.filter((_, j) => j !== i).map((b) => Math.hypot(a.c[0] - b.c[0], a.c[1] - b.c[1], a.c[2] - b.c[2]))));
  const mean = (xs: number[]): number | null => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : null);
  let total = 0;
  let inked = 0;
  for (const p of pieces) {
    total += p.area;
    if (p.inked) inked += p.area;
  }
  return {
    id: s.id,
    locomotion: s.locomotion,
    floats: s.floats,
    parts: s.parts.length,
    eyes: s.eyes.length,
    height,
    width: span(pieces, 0),
    depth: span(pieces, 2),
    minY: pieces.length ? Math.min(...pieces.map((p) => p.min[1])) : 0,
    headHeight,
    headWidth,
    bodyHeight: body.length ? span(body, 1) : null,
    headToBody: body.length && span(body, 1) > 0 ? headHeight / span(body, 1) : null,
    headToHeight: height > 0 ? headHeight / height : 0,
    eyeRelHead: headWidth > 0 ? mean(sockets.map((e) => e.r / headWidth)) : null,
    eyeSpacing: headWidth > 0 ? mean(nearest.filter(Number.isFinite).map((d) => d / headWidth)) : null,
    eyeHeight: headHeight > 0 ? mean(sockets.map((e) => (e.c[1] - headMinY) / headHeight)) : null,
    inkCoverage: total > 0 ? inked / total : 0,
  };
}

/** The median of each proportion metric over a set of species (null values skipped). */
export function medians(list: Metrics[]): Medians {
  const keys: MetricKey[] = ['height', 'width', 'headToBody', 'headToHeight', 'eyeRelHead', 'eyeSpacing', 'eyeHeight', 'inkCoverage'];
  const out = {} as Medians;
  for (const k of keys) {
    const xs = list.map((m) => m[k]).filter((v): v is number => typeof v === 'number').sort((a, b) => a - b);
    const n = xs.length;
    out[k] = n === 0 ? 0 : n % 2 ? xs[(n - 1) / 2]! : (xs[n / 2 - 1]! + xs[n / 2]!) / 2;
  }
  return out;
}

function checkProportions(m: Metrics, med: Medians, issues: Issue[]): void {
  const keys: MetricKey[] = ['height', 'width', 'headToBody', 'headToHeight', 'eyeRelHead', 'eyeSpacing', 'eyeHeight', 'inkCoverage'];
  for (const k of keys) {
    const v = m[k];
    const x = med[k];
    if (typeof v !== 'number' || !x) continue;
    const rel = (v - x) / Math.abs(x);
    if (Math.abs(rel) > MEDIAN_TOL) {
      issues.push({ kind: 'proportion', severity: 'warn', path: 'metrics', label: k, pose: 'rest', value: v, message: `${k} ${v.toFixed(3)} is ${(rel * 100).toFixed(0)}% from the median ${x.toFixed(3)} of the species` });
    }
  }
}

/** The audit of one species. Tools and clips default to the registry's; medians to the registry's species. */
export function auditSpecies(s: Species, opts: AuditOptions = {}): Issue[] {
  const reg = registry();
  const tools = opts.tools ?? reg.tools;
  const shared = opts.clips ?? reg.clips;
  const med = opts.medians ?? medians(Object.values(reg.species).map(metrics));
  const issues: Issue[] = [];
  const bones = boneMatrices(s);
  const pieces = speciesPieces(s, bones);
  const faceBones = new Set<string>(['head', ...s.eyes.map((e) => e.bone)]);

  checkEyes(s, pieces, bones, issues);
  checkEyeSymmetry(s, bones, issues);
  for (const [pose, scene] of Object.entries(sceneOf(s, shared)) as [PoseName, Scene][]) checkEyeVisibility(s, scene, pose, issues);
  checkFeatures(pieces, faceBones, issues);
  checkAttach(s, pieces, 'rest', issues);
  checkLimbs(s, pieces, 'rest', issues);
  checkSymmetry(pieces, issues);
  checkGround(s, pieces, issues);
  checkTools(bones, tools, pieces, issues);
  checkProportions(metrics(s), med, issues);

  const gait = s.gait;
  const walk = clipFor(s, `walk.${s.locomotion}`, shared) ?? clipFor(s, 'walk.treads', shared);
  const wave = clipFor(s, 'wave', shared);
  const posed: [PoseName, Clip | undefined][] = [['walk', walk], ['wave', wave]];
  for (const [name, clip] of posed) {
    if (!clip) continue;
    const pm = boneMatrices(s, poseOf(clip, gait));
    const posedPieces = speciesPieces(s, pm);
    checkAttach(s, posedPieces, name, issues);
    checkLimbs(s, posedPieces, name, issues);
  }
  return issues;
}

/** The audit of every species, with the proportion medians taken over the whole set. */
export function auditAll(list: Species[], opts: AuditOptions = {}): Record<string, Issue[]> {
  const med = opts.medians ?? medians(list.map(metrics));
  const out: Record<string, Issue[]> = {};
  for (const s of list) out[s.id] = auditSpecies(s, { ...opts, medians: med });
  return out;
}
