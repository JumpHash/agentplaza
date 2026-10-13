import * as THREE from 'three';
import type { KitPalette } from './rooms/kit';
import { CAB_BACK, liftMats } from './lift';

/** A gangway landing of a plate: the plate edge (ax, az), the shaft's board cell (bx, bz), the outward normal (nx, nz). */
export interface GangwayLanding {
  ax: number; az: number; bx: number; bz: number; nx: number; nz: number;
  bridge?: [number, number][];
}
export interface GangwayOptions {
  /** The top of the blocked solids under the bridge at a world point; a support never reaches below it. Default: the PCB top. */
  topAt?: (x: number, z: number) => number;
}
export interface Gangway { group: THREE.Group; dispose(): void }

const WIDTH = 0.5;     // deck width
const THICK = 0.06;    // deck thickness under its top surface
const RAIL = 0.04;     // rail section
const RAIL_H = 0.3;    // rail height above the deck
const POST = 0.06;     // support post section
const SPAN = 1.0;      // distance between supports

/**
 * A gangway: a slim bridge at plate height from the plate edge out along the normal, ending flush at the lift cab's plate
 * side (lift.ts: the cab's back plane sits CAB_BACK from the shaft's centre, so the bridge is CAB_BACK shorter than the
 * distance to the shaft). Its deck top is plateY, the cab's stop-1 height, so the two meet flush. The shaft, the cab and
 * its gates are buildLift's. Everything is in world coordinates (y absolute).
 */
export function buildGangway(
  landing: GangwayLanding,
  plateY: number,
  pcbTop: number,
  palette: Pick<KitPalette, 'info'>,
  opts: GangwayOptions = {},
): Gangway {
  const { deck, steel } = liftMats(palette);
  const topAt = opts.topAt ?? (() => pcbTop);
  const group = new THREE.Group();
  group.userData.gangway = true;
  const geos: THREE.BufferGeometry[] = [];

  const part = (parent: THREE.Object3D, w: number, h: number, d: number, mat: THREE.Material,
    x: number, y: number, z: number): THREE.Mesh => {
    const geo = new THREE.BoxGeometry(w, h, d);
    geos.push(geo);
    const m = new THREE.Mesh(geo, mat);
    m.position.set(x, y, z);
    m.castShadow = true;
    m.receiveShadow = true;
    parent.add(m);
    return m;
  };

  // The bridge runs from its first cell (the plate edge) along the normal to the cab's plate side. Local +z is the normal.
  const [sx, sz] = landing.bridge?.[0] ?? [landing.ax, landing.az];
  const { nx, nz } = landing;
  const len = Math.max(0.01, (landing.bx - sx) * nx + (landing.bz - sz) * nz + CAB_BACK);
  const face = Math.atan2(nx, nz);
  const bridge = new THREE.Group();
  bridge.position.set(sx, 0, sz);
  bridge.rotation.y = face;
  group.add(bridge);

  part(bridge, WIDTH, THICK, len, deck, 0, plateY - THICK / 2, len / 2);
  for (const side of [-1, 1]) {
    part(bridge, RAIL, RAIL_H, len, steel, side * (WIDTH / 2 - RAIL / 2), plateY + RAIL_H / 2, len / 2);
  }
  // A support every SPAN, from the deck's underside down to the solid it stands on (never below it).
  const bottom = plateY - THICK;
  for (let s = SPAN / 2; s < len; s += SPAN) {
    const base = topAt(sx + nx * s, sz + nz * s);
    if (bottom - base <= 0.01) continue;
    part(bridge, POST, bottom - base, POST, steel, 0, (bottom + base) / 2, s);
  }

  const dispose = (): void => {
    for (const g of geos) g.dispose();
  };

  return { group, dispose };
}
