// The z-fight lint. Two solid faces in the same plane that overlap flicker on screen: the GPU cannot say
// which one is in front. This walks every hardware model in world space (its `use` parts expanded, `when`
// and `repeat` resolved with the model's default params) and reports those pairs. Decals (`plane`, `ring`,
// silkscreen labels) are exempt from the pair rule, since they draw with a polygon offset, but a decal that
// sits exactly on a surface with no lift is reported as well. Only axis-aligned geometry is checked:
// rotations that are not multiples of 90 degrees and curved faces (sphere, torus, cylinder sides) are skipped.
// The same walk runs over any object of the Model shape, so robot species and generated rooms can be linted too.

import { MODELS } from './hardware-index';
import { evalWhen, parseWhen, repeatOffsets } from './hardware';
import type { Geo, Model, Params, Part, Vec3 } from './hardware';

/** Faces closer than this along their normal are coplanar. */
export const COPLANAR = 0.002;
/** Overlaps smaller than this (square units) do not count. */
export const MIN_OVERLAP = 1e-4;

/** A face of a solid in world space: an axis-aligned rectangle with its outward sign. */
export interface Face {
  model: string;
  /** The part's id, or its JSON path when it has none. */
  id: string;
  /** The JSON file that holds the part (a shared part's file under assets/hardware/parts/). */
  file: string;
  /** The part's JSON path inside that file, e.g. "parts[3].children[0]". */
  path: string;
  /** The material's palette name, or the JSON of an inline material. */
  mat: string;
  axis: number;
  /** +1 when the face points along +axis, -1 when along -axis. */
  sign: number;
  plane: number;
  /** The face's extent on the two other axes, in ascending axis order: (axis+1) % 3, then (axis+2) % 3. */
  lo: [number, number];
  hi: [number, number];
  decal: boolean;
  label: boolean;
  /** The face is an approximation (a circle drawn as its square, a rounded corner drawn square). */
  approx: boolean;
  /** The part's origin along the axis, in world space. */
  origin: number;
  /** The parent's world rotation, row-major. The part's `at` is in this frame. */
  frame: number[];
}

export interface Finding {
  kind: 'pair' | 'decal';
  why: string;
  a: Face;
  b: Face;
  axis: number;
  plane: number;
  overlap: number;
}

export interface LintResult {
  findings: Finding[];
  /** Parts that were skipped: rotated off the 90-degree grid. */
  skipped: string[];
  faces: number;
}

// ---- Transforms ------------------------------------------------------------------------------------

/** A 3 x 3 matrix, row-major. */
export type M3 = [number, number, number, number, number, number, number, number, number];
interface Xf { R: M3; t: Vec3 }

const cell = (R: M3, i: number, j: number): number => R[i * 3 + j] ?? 0;
/** A vector's component at a computed index. */
const comp = (v: readonly number[], i: number): number => v[i] ?? 0;
const mul3 = (a: M3, b: M3): M3 => {
  const o: number[] = [];
  for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) {
    let v = 0;
    for (let k = 0; k < 3; k++) v += cell(a, i, k) * cell(b, k, j);
    o.push(v);
  }
  return o as M3;
};
const apply = (x: Xf, p: Vec3): Vec3 => {
  const R = x.R;
  return [
    R[0] * p[0] + R[1] * p[1] + R[2] * p[2] + x.t[0],
    R[3] * p[0] + R[4] * p[1] + R[5] * p[2] + x.t[1],
    R[6] * p[0] + R[7] * p[1] + R[8] * p[2] + x.t[2],
  ];
};
const turn = (R: M3, v: Vec3): Vec3 => [
  R[0] * v[0] + R[1] * v[1] + R[2] * v[2],
  R[3] * v[0] + R[4] * v[1] + R[5] * v[2],
  R[6] * v[0] + R[7] * v[1] + R[8] * v[2],
];
const compose = (a: Xf, b: Xf): Xf => ({ R: mul3(a.R, b.R), t: apply(a, b.t) });

/** A part's own transform: rotation in degrees (Euler XYZ, as three.js reads it), then scale, then `at`. */
function localXf(at: Vec3, rot: Vec3 | undefined, scale: number | Vec3 | undefined): Xf {
  const deg: Vec3 = rot ?? [0, 0, 0];
  const rx = (deg[0] * Math.PI) / 180;
  const ry = (deg[1] * Math.PI) / 180;
  const rz = (deg[2] * Math.PI) / 180;
  const Rx: M3 = [1, 0, 0, 0, Math.cos(rx), -Math.sin(rx), 0, Math.sin(rx), Math.cos(rx)];
  const Ry: M3 = [Math.cos(ry), 0, Math.sin(ry), 0, 1, 0, -Math.sin(ry), 0, Math.cos(ry)];
  const Rz: M3 = [Math.cos(rz), -Math.sin(rz), 0, Math.sin(rz), Math.cos(rz), 0, 0, 0, 1];
  const s: Vec3 = typeof scale === 'number' ? [scale, scale, scale] : (scale ?? [1, 1, 1]);
  const S: M3 = [s[0], 0, 0, 0, s[1], 0, 0, 0, s[2]];
  return { R: mul3(mul3(mul3(Rx, Ry), Rz), S), t: at };
}

/** True when every column of the rotation has one non-zero entry, so every axis maps onto an axis. */
const axisAligned = (R: M3): boolean => [0, 1, 2].every((j) => [0, 1, 2].filter((i) => Math.abs(cell(R, i, j)) > 1e-6).length === 1);

// ---- Local faces -----------------------------------------------------------------------------------

interface LFace { n: Vec3; pts: Vec3[]; decal: boolean; approx: boolean }

/** The four corners of a rectangle lying on the plane where coordinate `axis` equals `value`. */
function rect(axis: number, value: number, half: Vec3): Vec3[] {
  const b = (axis + 1) % 3;
  const c = (axis + 2) % 3;
  const out: Vec3[] = [];
  for (const sb of [-1, 1]) for (const sc of [-1, 1]) {
    const p: Vec3 = [0, 0, 0];
    p[axis] = value;
    p[b] = sb * comp(half, b);
    p[c] = sc * comp(half, c);
    out.push(p);
  }
  return out;
}

function boxFaces(half: Vec3, approx: boolean): LFace[] {
  const out: LFace[] = [];
  for (let a = 0; a < 3; a++) for (const s of [-1, 1]) {
    const n: Vec3 = [0, 0, 0];
    n[a] = s;
    out.push({ n, pts: rect(a, s * comp(half, a), half), decal: false, approx });
  }
  return out;
}

/** The planar faces of a geometry, in its own frame. Curved faces are left out. */
function geoFaces(g: Geo): LFace[] {
  switch (g.type) {
    case 'box':
    case 'rbox':
      return boxFaces([g.w / 2, g.h / 2, g.d / 2], false);
    case 'cyl':
      return [
        { n: [0, 1, 0], pts: rect(1, g.h / 2, [g.rt, g.h / 2, g.rt]), decal: false, approx: true },
        { n: [0, -1, 0], pts: rect(1, -g.h / 2, [g.rb, g.h / 2, g.rb]), decal: false, approx: true },
      ];
    case 'cone':
      return [{ n: [0, -1, 0], pts: rect(1, -g.h / 2, [g.r, g.h / 2, g.r]), decal: false, approx: true }];
    case 'sphere':
    case 'torus':
      return [];
    case 'plane':
      return [{ n: [0, 1, 0], pts: rect(1, 0, [g.w / 2, 0, g.d / 2]), decal: true, approx: false }];
    case 'ring':
      return [{ n: [0, 1, 0], pts: rect(1, 0, [g.ro, 0, g.ro]), decal: true, approx: true }];
    case 'shape': {
      // An outline in (x, z), extruded along +y by h: two caps and one side per axis-aligned edge.
      const P = g.points;
      const h = g.h;
      const out: LFace[] = [
        { n: [0, -1, 0], pts: P.map(([x, z]): Vec3 => [x, 0, z]), decal: false, approx: true },
        { n: [0, 1, 0], pts: P.map(([x, z]): Vec3 => [x, h, z]), decal: false, approx: true },
      ];
      const at2 = (i: number): [number, number] => P[i % P.length] ?? [0, 0];
      let area = 0;
      for (let i = 0; i < P.length; i++) {
        const [xi, zi] = at2(i);
        const [xj, zj] = at2(i + 1);
        area += xi * zj - xj * zi;
      }
      const flip = area > 0 ? 1 : -1; // CCW in (x, z): the outward normal of edge (ex, ez) is (ez, -ex)
      for (let i = 0; i < P.length; i++) {
        const [x0, z0] = at2(i);
        const [x1, z1] = at2(i + 1);
        const ex = x1 - x0;
        const ez = z1 - z0;
        if (Math.abs(ex) > 1e-9 && Math.abs(ez) > 1e-9) continue;
        if (Math.abs(ex) < 1e-9 && Math.abs(ez) < 1e-9) continue;
        if (Math.abs(ex) < 1e-9) {
          const s = Math.sign(ez * flip) || 1;
          out.push({ n: [s, 0, 0], pts: [[x0, 0, z0], [x0, 0, z1], [x0, h, z1], [x0, h, z0]], decal: false, approx: false });
        } else {
          const s = Math.sign(-ex * flip) || 1;
          out.push({ n: [0, 0, s], pts: [[x0, 0, z0], [x1, 0, z0], [x1, h, z0], [x0, h, z0]], decal: false, approx: false });
        }
      }
      return out;
    }
  }
}

const matKey = (m: Part['mat']): string => (m === undefined ? 'default' : typeof m === 'string' ? m : JSON.stringify(m));

// ---- The walk --------------------------------------------------------------------------------------

interface Ctx { model: string; params: Params; faces: Face[]; skipped: string[]; models: Record<string, Model> }

/** Adds the faces of one geometry, placed by the world transform M. */
type Meta = Pick<Face, 'id' | 'file' | 'path' | 'mat'>;

function emit(ctx: Ctx, lf: LFace[], M: Xf, parentFrame: M3, meta: Meta): Face[] {
  const origin = apply(M, [0, 0, 0]);
  const added: Face[] = [];
  for (const f of lf) {
    const pts = f.pts.map((p) => apply(M, p));
    const nw = turn(M.R, f.n);
    let axis = 0;
    for (let k = 1; k < 3; k++) if (Math.abs(comp(nw, k)) > Math.abs(comp(nw, axis))) axis = k;
    const plane = pts.reduce((s, p) => s + comp(p, axis), 0) / pts.length;
    if (pts.some((p) => Math.abs(comp(p, axis) - plane) > 1e-4)) continue;
    const b = (axis + 1) % 3;
    const c = (axis + 2) % 3;
    const face: Face = {
      ...meta,
      model: ctx.model,
      axis,
      sign: Math.sign(comp(nw, axis)),
      plane,
      lo: [Math.min(...pts.map((p) => comp(p, b))), Math.min(...pts.map((p) => comp(p, c)))],
      hi: [Math.max(...pts.map((p) => comp(p, b))), Math.max(...pts.map((p) => comp(p, c)))],
      decal: f.decal,
      approx: f.approx,
      origin: comp(origin, axis),
      frame: parentFrame,
      label: false,
    };
    ctx.faces.push(face);
    added.push(face);
  }
  return added;
}

/** Walks a part list under the parent transform. `file` and `prefix` give each part its JSON path. */
function walk(ctx: Ctx, parts: Part[], parent: Xf, file: string, prefix: string): void {
  parts.forEach((p, i) => {
    const path = `${prefix}[${i}]`;
    if (p.when !== undefined) {
      const w = parseWhen(p.when);
      if (!w || !evalWhen(w, ctx.params)) return;
    }
    const id = p.id ?? path;
    const copies: Vec3[] = p.repeat ? repeatOffsets(p.repeat, ctx.params) : [[0, 0, 0]];
    for (const o of copies) {
      const at = (p.at ?? [0, 0, 0]) as Vec3;
      const local = localXf([at[0] + o[0], at[1] + o[1], at[2] + o[2]], p.rot, p.scale);
      const M = compose(parent, local);
      if (!axisAligned(M.R)) {
        ctx.skipped.push(`${ctx.model} ${file}:${path}`);
        continue;
      }
      const frame = parent.R;
      const meta: Meta = { id, file, path, mat: matKey(p.mat) };
      if (p.geo) {
        const lf = geoFaces(p.geo);
        emit(ctx, lf, M, frame, meta);
        const top = p.label ? lf.find((f) => !f.decal && f.n[1] === 1) : undefined;
        if (p.label && top) {
          // A label is a decal on the part's top face, drawn with no lift.
          const added = emit(ctx, [{ n: top.n, pts: top.pts, decal: true, approx: true }], M, frame, { ...meta, mat: `label:${p.label.text}` });
          for (const f of added) f.label = true;
        }
      }
      if (p.use) {
        const ref = ctx.models[p.use];
        if (ref) walk(ctx, ref.parts, M, `assets/hardware/${p.use}.json`, 'parts');
      }
      if (p.children) walk(ctx, p.children, M, file, `${path}.children`);
    }
  });
}

/** The lint of one model (or any Model-shaped object, keyed by its name). */
function faceList(name: string, model: Model, models: Record<string, Model>, folder: string): { faces: Face[]; skipped: string[] } {
  const params: Params = { ...(model.params ?? {}) };
  const ctx: Ctx = { model: name, params, faces: [], skipped: [], models };
  walk(ctx, model.parts, { R: [1, 0, 0, 0, 1, 0, 0, 0, 1], t: [0, 0, 0] }, `${folder}/${name}.json`, 'parts');
  return { faces: ctx.faces, skipped: ctx.skipped };
}

const overlap = (a: Face, b: Face): number => {
  const w = Math.min(a.hi[0], b.hi[0]) - Math.max(a.lo[0], b.lo[0]);
  const h = Math.min(a.hi[1], b.hi[1]) - Math.max(a.lo[1], b.lo[1]);
  return w > 0 && h > 0 ? w * h : 0;
};

/** Pairs of solid faces that are coplanar, overlap, and either differ in material or face the same way. */
function pairs(faces: Face[]): Finding[] {
  const solids = faces.filter((f) => !f.decal);
  const out: Finding[] = [];
  for (let axis = 0; axis < 3; axis++) {
    const list = solids.filter((f) => f.axis === axis).sort((x, y) => x.plane - y.plane);
    for (let i = 0; i < list.length; i++) {
      for (let j = i + 1; j < list.length; j++) {
        const a = list[i];
        const b = list[j];
        if (!a || !b || b.plane - a.plane >= COPLANAR) break;
        if (a.sign !== b.sign && a.mat === b.mat) continue; // face to face, one material: a contact, never seen
        const area = overlap(a, b);
        if (area <= MIN_OVERLAP) continue;
        const why = a.mat !== b.mat ? 'coplanar, different materials' : a.sign === b.sign ? 'coplanar, same material, same way' : 'contact';
        out.push({ kind: 'pair', why, a, b, axis, plane: a.plane, overlap: area });
      }
    }
  }
  return out;
}

/** Decals that sit exactly on a solid face. A decal is never a pair partner; it is reported on its own. */
function decalsOnSurface(faces: Face[]): Finding[] {
  const out: Finding[] = [];
  for (const d of faces.filter((f) => f.decal)) {
    for (const s of faces) {
      if (s.decal || s.axis !== d.axis || Math.abs(s.plane - d.plane) >= COPLANAR) continue;
      const area = overlap(d, s);
      if (area <= MIN_OVERLAP) continue;
      out.push({ kind: 'decal', why: d.label ? 'label on a surface, no lift' : 'decal on a surface, no lift', a: d, b: s, axis: d.axis, plane: d.plane, overlap: area });
    }
  }
  return out;
}

/** Lints every model in `models`: the hardware models by default, or any Model-shaped objects passed in. `folder` is the path the files are labelled with (assets/robots for the robot species). */
export function lintModels(models: Record<string, Model> = MODELS, names?: string[], folder = 'assets/hardware'): LintResult {
  const findings: Finding[] = [];
  const skipped: string[] = [];
  let faces = 0;
  for (const name of names ?? Object.keys(models)) {
    const model = models[name];
    if (!model || !Array.isArray(model.parts)) continue;
    const r = faceList(name, model, models, folder);
    faces += r.faces.length;
    skipped.push(...r.skipped);
    findings.push(...pairs(r.faces), ...decalsOnSurface(r.faces));
  }
  return { findings, skipped, faces };
}

/** One line per finding: the model, both parts with their JSON paths, the axis and the plane value. */
export function describe(f: Finding): string {
  const ax = 'xyz'.charAt(f.axis);
  const side = (x: Face): string => `${x.id} [${x.file}:${x.path}] ${x.mat}${x.sign > 0 ? '+' : '-'}${ax}${x.approx ? ' ~' : ''}`;
  return `${f.a.model} | ${f.why} | ${side(f.a)} | ${side(f.b)} | axis ${ax} | plane ${f.plane.toFixed(4)} | overlap ${f.overlap.toFixed(3)}`;
}

// ---- Part boxes ------------------------------------------------------------------------------------

/** The world box of one emitted copy of a solid part: its id (or JSON path), its file and path as the lint labels them, and its AABB. */
export interface PartBox { id: string; path: string; file: string; lo: [number, number, number]; hi: [number, number, number] }

interface BoxCtx { params: Params; models: Record<string, Model> }

/** The local bounds of a solid geometry, in its own frame. Decals (plane, ring) and empty outlines have none. */
function geoBounds(g: Geo): { lo: Vec3; hi: Vec3 } | null {
  switch (g.type) {
    case 'box':
    case 'rbox':
      return { lo: [-g.w / 2, -g.h / 2, -g.d / 2], hi: [g.w / 2, g.h / 2, g.d / 2] };
    case 'cyl': {
      const r = Math.max(g.rt, g.rb);
      return { lo: [-r, -g.h / 2, -r], hi: [r, g.h / 2, r] };
    }
    case 'cone':
      return { lo: [-g.r, -g.h / 2, -g.r], hi: [g.r, g.h / 2, g.r] };
    case 'sphere':
      return { lo: [-g.r, -g.r, -g.r], hi: [g.r, g.r, g.r] };
    case 'torus': {
      const R = g.r + g.tube;
      return { lo: [-R, -g.tube, -R], hi: [R, g.tube, R] };
    }
    case 'shape': {
      if (g.points.length === 0) return null;
      const xs = g.points.map(([x]) => x);
      const zs = g.points.map(([, z]) => z);
      return { lo: [Math.min(...xs), 0, Math.min(...zs)], hi: [Math.max(...xs), g.h, Math.max(...zs)] };
    }
    case 'plane':
    case 'ring':
      return null;
  }
}

/** The AABB of a local box after the world transform M: the min and max of its eight transformed corners. */
function worldBox(M: Xf, lo: Vec3, hi: Vec3): { lo: Vec3; hi: Vec3 } {
  const outLo: Vec3 = [Infinity, Infinity, Infinity];
  const outHi: Vec3 = [-Infinity, -Infinity, -Infinity];
  for (const x of [lo[0], hi[0]]) for (const y of [lo[1], hi[1]]) for (const z of [lo[2], hi[2]]) {
    const p = apply(M, [x, y, z]);
    for (let k = 0; k < 3; k++) {
      outLo[k] = Math.min(comp(outLo, k), comp(p, k));
      outHi[k] = Math.max(comp(outHi, k), comp(p, k));
    }
  }
  return { lo: outLo, hi: outHi };
}

/**
 * The walk of lintModels, with the same `when`, `repeat`, `use`, `children` and transforms, but it keeps
 * non-axis-aligned copies and records a box per copy instead of faces. A label is a decal on the part's top, so
 * the part's own geometry still gives its box; decals (plane, ring) have no bounds and give none.
 */
function boxWalk(out: PartBox[], ctx: BoxCtx, parts: Part[], parent: Xf, file: string, prefix: string): void {
  parts.forEach((p, i) => {
    const path = `${prefix}[${i}]`;
    if (p.when !== undefined) {
      const w = parseWhen(p.when);
      if (!w || !evalWhen(w, ctx.params)) return;
    }
    const id = p.id ?? path;
    const copies: Vec3[] = p.repeat ? repeatOffsets(p.repeat, ctx.params) : [[0, 0, 0]];
    for (const o of copies) {
      const at = (p.at ?? [0, 0, 0]) as Vec3;
      const local = localXf([at[0] + o[0], at[1] + o[1], at[2] + o[2]], p.rot, p.scale);
      const M = compose(parent, local);
      if (p.geo) {
        const b = geoBounds(p.geo);
        if (b) {
          const w = worldBox(M, b.lo, b.hi);
          out.push({ id, path, file, lo: w.lo, hi: w.hi });
        }
      }
      if (p.use) {
        const ref = ctx.models[p.use];
        if (ref && Array.isArray(ref.parts)) boxWalk(out, ctx, ref.parts, M, `assets/hardware/${p.use}.json`, 'parts');
      }
      if (p.children) boxWalk(out, ctx, p.children, M, file, `${path}.children`);
    }
  });
}

/**
 * The world AABB of every emitted copy of a model's solid parts, one box per copy. `params` are merged over
 * the model's own params, as lintModels does. The model is labelled by its key in `models` (its file is
 * `folder/key.json`); a model that is not in `models` is labelled by its kind.
 */
export function partBoxes(model: Model, params: Params = {}, models: Record<string, Model> = MODELS, folder = 'assets/hardware'): PartBox[] {
  if (!Array.isArray(model.parts)) return [];
  const name = Object.keys(models).find((k) => models[k] === model) ?? model.kind;
  const ctx: BoxCtx = { params: { ...(model.params ?? {}), ...params }, models };
  const out: PartBox[] = [];
  boxWalk(out, ctx, model.parts, { R: [1, 0, 0, 0, 1, 0, 0, 0, 1], t: [0, 0, 0] }, `${folder}/${name}.json`, 'parts');
  return out;
}
