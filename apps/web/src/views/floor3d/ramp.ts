import * as THREE from 'three';
import { toonMat } from './toon';
import type { KitPalette } from './rooms/kit';
import type { NavGrid, NavPortal } from './nav-types';

export interface RampOptions {
  x: number; z: number; face: number; low: number; high: number; width?: number; length?: number;
}

const THICK = 0.06;   // the wedge's thickness under its top surface
const RAIL_W = 0.03;  // side rail width
const RAIL_H = 0.12;  // side rail height above the top surface
const GLOW_H = 0.012; // the light strip on each rail's cap

interface RampMats { deck: THREE.Material; steel: THREE.Material; glow: THREE.Material }
const matsByPalette = new WeakMap<KitPalette, RampMats>();

/** The ramp's materials, created once per palette and shared by every ramp (never disposed with a ramp). */
function matsFor(palette: KitPalette): RampMats {
  let m = matsByPalette.get(palette);
  if (!m) {
    m = {
      deck: toonMat({ color: 0x46505e }),
      steel: toonMat({ color: 0x8f9bab }),
      glow: new THREE.MeshBasicMaterial({ color: palette.info, toneMapped: false }),
    };
    matsByPalette.set(palette, m);
  }
  return m;
}

/**
 * A ramp: a sloped wedge from a terrace edge (high) down to the PCB (low), with a side rail each way. The group sits at
 * (x, 0, z) and rotation.y = face, so local +z points out from the terrace. The slope starts at the origin and runs
 * `length` along +z (horizontally).
 */
export function buildRamp(opts: RampOptions, palette: KitPalette): { group: THREE.Group } {
  const width = opts.width ?? 0.6;
  const length = Math.max(0.01, opts.length ?? 0.5);
  const rise = opts.high - opts.low;
  const slopeLen = Math.hypot(length, rise); // the wedge's length along its own slope
  const { deck, steel, glow } = matsFor(palette);
  const group = new THREE.Group();
  group.position.set(opts.x, 0, opts.z);
  group.rotation.y = opts.face;
  group.userData.ramp = true;
  // The slope frame: its origin on the terrace-level top line, tilted about x so +z drops to the PCB.
  const slope = new THREE.Group();
  slope.position.y = opts.high;
  slope.rotation.x = Math.atan2(rise, length);
  group.add(slope);

  const part = (w: number, h: number, d: number, mat: THREE.Material, x: number, y: number, z: number): void => {
    const m = new THREE.Mesh(new THREE.BoxGeometry(w, h, d), mat);
    m.position.set(x, y, z);
    m.castShadow = true;
    m.receiveShadow = true;
    slope.add(m);
  };
  part(width, THICK, slopeLen, deck, 0, -THICK / 2, slopeLen / 2);
  const railX = width / 2 - RAIL_W / 2;
  for (const sx of [-1, 1]) {
    part(RAIL_W, RAIL_H, slopeLen, steel, sx * railX, RAIL_H / 2, slopeLen / 2);
    part(RAIL_W, GLOW_H, slopeLen, glow, sx * railX, RAIL_H + GLOW_H / 2, slopeLen / 2);
  }
  return { group };
}

/** Frees a ramp's own geometries (its materials are shared and stay). */
export function disposeRamp(group: THREE.Object3D): void {
  group.traverse((o) => {
    if (o instanceof THREE.Mesh) o.geometry.dispose();
  });
}

/**
 * The ramp of a nav grid's ramp portal: from the plate cell (a, at the plate's top) to the board cell (b, at the PCB top),
 * the same two cell centres the nav cost is measured between.
 */
export function rampForPortal(grid: NavGrid, p: NavPortal, palette: KitPalette): { group: THREE.Group } {
  const centre = (c: number): [number, number] => {
    const i = c % grid.cols;
    const j = (c - i) / grid.cols;
    return [grid.x0 + (i + 0.5) * grid.cell, grid.z0 + (j + 0.5) * grid.cell];
  };
  const [xa, za] = centre(p.a);
  const [xb, zb] = centre(p.b);
  return buildRamp({
    x: xa,
    z: za,
    face: Math.atan2(xb - xa, zb - za),
    low: grid.y[p.b]!,
    high: grid.y[p.a]!,
    length: Math.hypot(xb - xa, zb - za),
  }, palette);
}
