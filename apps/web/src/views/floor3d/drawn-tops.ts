// The drawn tops of a board, pure (no three.js). A support field is the upward faces of the drawn solids (ground.ts
// supportField reads them from a scene); topBelow is the exact drawn top under a point; the walk heights bake (bakeDrawnTops)
// turns the field into one height per sample over the nav grid, and drawnTopAt reads it (navigation.md, "Gravity and heights").
// The desktop port mirrors this file (apz-board.md).

import { NAV_STEP, type NavGrid } from './nav-types';
import { cellAt } from './nav-grid';

/** A bottom this far off what is under it, or an overlap this deep on every axis, is a finding (ground.ts). */
export const GROUND_TOL = 0.02;
/** The support field's bucket size: each bucket lists the upward triangles over it; a point is tested exactly against them. */
export const FIELD_CELL = 0.5;

/**
 * The upward faces of a model's solids (model units, the model's frame): the triangles, nine numbers each (a, b, c as x y z),
 * and the buckets of the grid cells they cover. A point's support is the exact height of the triangle under it, as a ray
 * would find it.
 */
export interface SupportField {
  cell: number;
  tris: number[];
  cells: Map<number, number[]>;
}

/** A support field at a board's world scale: the field is in model units, the scale maps them to the board. */
export interface Support {
  field: SupportField;
  scale: number;
}

const KEY_OFF = 100000;
const KEY_M = 200000;
export const keyOf = (i: number, j: number): number => (i + KEY_OFF) * KEY_M + (j + KEY_OFF);

/**
 * The height of the highest support under (x, z) at or below y (within the tolerance), in world units; 0 for the ground or no
 * support. y may be Infinity (the highest support there is).
 */
export function topBelow(sup: Support | null, x: number, z: number, y: number): number {
  if (!sup) return 0;
  const { field: f, scale: s } = sup;
  const xm = x / s;
  const zm = z / s;
  const ym = y / s + GROUND_TOL / s;
  const bucket = f.cells.get(keyOf(Math.floor(xm / f.cell), Math.floor(zm / f.cell)));
  if (!bucket) return 0;
  let best = 0;
  for (const t of bucket) {
    const o = t * 9;
    const ax = f.tris[o]!, ay = f.tris[o + 1]!, az = f.tris[o + 2]!;
    const bx = f.tris[o + 3]!, by = f.tris[o + 4]!, bz = f.tris[o + 5]!;
    const cx = f.tris[o + 6]!, cy = f.tris[o + 7]!, cz = f.tris[o + 8]!;
    const den = (bz - cz) * (ax - cx) + (cx - bx) * (az - cz);
    if (Math.abs(den) < 1e-12) continue;
    const w1 = ((bz - cz) * (xm - cx) + (cx - bx) * (zm - cz)) / den;
    const w2 = ((cz - az) * (xm - cx) + (ax - cx) * (zm - cz)) / den;
    const w3 = 1 - w1 - w2;
    if (w1 < -1e-9 || w2 < -1e-9 || w3 < -1e-9) continue;
    const h = w1 * ay + w2 * by + w3 * cy;
    if (h <= ym && h > best) best = h;
  }
  return best * s;
}

/** The exact drawn top under (x, z) at or below y, over several supports (the board, its rooms): the highest of them. */
export function drawnTopBelow(sups: Support[], x: number, z: number, y: number): number {
  let h = 0;
  for (const s of sups) h = Math.max(h, topBelow(s, x, z, y));
  return h;
}

/** The walk heights' sample spacing (world units): the bilinear lookup reads the four samples around a point. */
export const DRAWN_STEP = 0.05;
/** A sample's tolerance on the band (world units): a drawn top within NAV_STEP of the walk cell's height counts. */
const BAND_EPS = 1e-4;

/**
 * The drawn walk heights over a nav grid, sampled every `step` from the grid's origin. A sample on a walk cell takes the
 * highest drawn upward face under it whose height is within NAV_STEP of the cell's grid height (the band rule: a part taller
 * than a step is blocked on the grid, so walls, slabs and furniture never enter, and a chip top a step high does). NaN where
 * the sample is off the walk or no drawn top is in its band.
 */
export interface DrawnTops {
  x0: number;
  z0: number;
  step: number;
  nx: number;
  nz: number;
  h: Float64Array;
}

/** The walk heights of `grid` from the drawn supports (world units). Pure; the bake costs one pass per support triangle. */
export function bakeDrawnTops(grid: NavGrid, sups: Support[], step = DRAWN_STEP): DrawnTops {
  const nx = Math.ceil((grid.cols * grid.cell) / step) + 1;
  const nz = Math.ceil((grid.rows * grid.cell) / step) + 1;
  const band = new Float64Array(nx * nz);
  const best = new Float64Array(nx * nz).fill(-Infinity);
  for (let j = 0; j < nz; j++) {
    for (let i = 0; i < nx; i++) {
      const c = cellAt(grid, grid.x0 + i * step, grid.z0 + j * step);
      band[j * nx + i] = c >= 0 && grid.walk[c] === 1 ? grid.y[c]! : NaN;
    }
  }
  for (const sup of sups) {
    const { tris } = sup.field;
    const s = sup.scale;
    for (let t = 0; t + 8 < tris.length; t += 9) {
      const ax = tris[t]! * s, ay = tris[t + 1]! * s, az = tris[t + 2]! * s;
      const bx = tris[t + 3]! * s, by = tris[t + 4]! * s, bz = tris[t + 5]! * s;
      const cx = tris[t + 6]! * s, cy = tris[t + 7]! * s, cz = tris[t + 8]! * s;
      const den = (bz - cz) * (ax - cx) + (cx - bx) * (az - cz);
      if (Math.abs(den) < 1e-12) continue;
      const i0 = Math.max(0, Math.ceil((Math.min(ax, bx, cx) - grid.x0) / step));
      const i1 = Math.min(nx - 1, Math.floor((Math.max(ax, bx, cx) - grid.x0) / step));
      const j0 = Math.max(0, Math.ceil((Math.min(az, bz, cz) - grid.z0) / step));
      const j1 = Math.min(nz - 1, Math.floor((Math.max(az, bz, cz) - grid.z0) / step));
      // The barycentric weights are linear in x: each row starts at its first sample and steps by dw1 and dw2 per sample.
      const dw1 = ((bz - cz) * step) / den;
      const dw2 = ((cz - az) * step) / den;
      for (let j = j0; j <= j1; j++) {
        const z = grid.z0 + j * step;
        const x0 = grid.x0 + i0 * step;
        let w1 = ((bz - cz) * (x0 - cx) + (cx - bx) * (z - cz)) / den;
        let w2 = ((cz - az) * (x0 - cx) + (ax - cx) * (z - cz)) / den;
        for (let i = i0; i <= i1; i++, w1 += dw1, w2 += dw2) {
          const k = j * nx + i;
          const b = band[k]!;
          if (Number.isNaN(b)) continue; // off the walk: no walk height here
          const w3 = 1 - w1 - w2;
          if (w1 < -1e-9 || w2 < -1e-9 || w3 < -1e-9) continue;
          const h = w1 * ay + w2 * by + w3 * cy;
          if (Math.abs(h - b) <= NAV_STEP + BAND_EPS && h > best[k]!) best[k] = h;
        }
      }
    }
  }
  const h = new Float64Array(nx * nz);
  for (let k = 0; k < h.length; k++) h[k] = best[k] === -Infinity ? NaN : best[k]!;
  return { x0: grid.x0, z0: grid.z0, step, nx, nz, h };
}

/**
 * The walk height under (x, z): bilinear over the four samples around the point. NaN when one of them has no drawn top (the
 * caller falls back to the grid's height) or the point is off the field.
 */
export function drawnTopAt(d: DrawnTops, x: number, z: number): number {
  const u = (x - d.x0) / d.step;
  const v = (z - d.z0) / d.step;
  const i = Math.floor(u);
  const j = Math.floor(v);
  if (i < 0 || j < 0 || i + 1 >= d.nx || j + 1 >= d.nz) return NaN;
  const a = d.h[j * d.nx + i]!;
  const b = d.h[j * d.nx + i + 1]!;
  const c = d.h[(j + 1) * d.nx + i]!;
  const e = d.h[(j + 1) * d.nx + i + 1]!;
  if (Number.isNaN(a) || Number.isNaN(b) || Number.isNaN(c) || Number.isNaN(e)) return NaN;
  const fu = u - i;
  const fv = v - j;
  return (a * (1 - fu) + b * fu) * (1 - fv) + (c * (1 - fu) + e * fu) * fv;
}
