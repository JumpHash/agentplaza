// Robot part geometry. `makeGeometry` mirrors hardware.ts (the same geo types, the same plane and ring
// laid flat, the same shape extrusion) and adds the optional segment counts the robots need, so the
// five ported designs keep their exact mesh resolution. Parts are baked into their bone's frame here.

import * as THREE from 'three';
import { RoundedBoxGeometry } from 'three/examples/jsm/geometries/RoundedBoxGeometry.js';
import type { Eye, EyeShape, Geo, Part, Vec3 } from './types';

const DEG = Math.PI / 180;

/** A geometry without an index (the matte merge needs one): the same geometry if it has none. */
export function nonIndexed(g: THREE.BufferGeometry): THREE.BufferGeometry {
  return g.index ? g.toNonIndexed() : g;
}

function makeGeometry(g: Geo): THREE.BufferGeometry {
  const s = g.seg ?? [];
  switch (g.type) {
    case 'box': return new THREE.BoxGeometry(g.w, g.h, g.d);
    case 'rbox': return new RoundedBoxGeometry(g.w!, g.h!, g.d!, s[0] ?? 3, g.r!);
    case 'cyl': return new THREE.CylinderGeometry(g.rt!, g.rb!, g.h!, s[0] ?? 24);
    case 'cone': return new THREE.ConeGeometry(g.r!, g.h!, s[0] ?? 24);
    case 'sphere': {
      const [w = 24, hs = 16] = s;
      return new THREE.SphereGeometry(g.r!, w, hs, 0, Math.PI * 2, 0, g.half === 'top' ? Math.PI / 2 : Math.PI);
    }
    case 'torus': return new THREE.TorusGeometry(g.r!, g.tube!, s[0] ?? 10, s[1] ?? 32, (g.arc ?? 360) * DEG);
    case 'plane': {
      const geo = new THREE.PlaneGeometry(g.w, g.d);
      geo.rotateX(-Math.PI / 2); // facing up
      return geo;
    }
    case 'ring': {
      const geo = new THREE.RingGeometry(g.ri, g.ro, 48);
      geo.rotateX(-Math.PI / 2);
      return geo;
    }
    case 'shape': {
      // As hardware.ts: the outline as (x, -z), extruded along +z, then laid flat.
      const shape = new THREE.Shape((g.points ?? []).map(([x, z]) => new THREE.Vector2(x, -z)));
      const geo = new THREE.ExtrudeGeometry(shape, { depth: g.h, bevelEnabled: false, curveSegments: 1 });
      geo.rotateX(-Math.PI / 2);
      return geo;
    }
  }
}

/** A part's geometry in its bone's frame: non-indexed, transformed by at / rot / scale. */
export function partGeometry(p: Pick<Part, 'geo' | 'at' | 'rot' | 'scale'>): { geometry: THREE.BufferGeometry; radius: number } {
  const source = makeGeometry(p.geo);
  source.computeBoundingSphere();
  const radius = source.boundingSphere?.radius ?? 0;
  const at: Vec3 = p.at ?? [0, 0, 0];
  const rot: Vec3 = p.rot ?? [0, 0, 0];
  const sc = p.scale === undefined ? [1, 1, 1] : typeof p.scale === 'number' ? [p.scale, p.scale, p.scale] : p.scale;
  const euler = new THREE.Euler(rot[0] * DEG, rot[1] * DEG, rot[2] * DEG); // XYZ, as Object3D.rotation
  const m = new THREE.Matrix4().compose(new THREE.Vector3(...at), new THREE.Quaternion().setFromEuler(euler), new THREE.Vector3(sc[0]!, sc[1]!, sc[2]!));
  const geometry = nonIndexed(source);
  geometry.applyMatrix4(m);
  return { geometry, radius };
}

/** An eye's shape, facing +z at the origin (the pupil's centre), in the pupil's own size. */
export function eyeShape(shape: EyeShape, r: number, stretch = 1): THREE.BufferGeometry {
  switch (shape) {
    case 'round': return new THREE.CircleGeometry(r, 24).scale(1, stretch, 1);
    case 'bar': return new THREE.PlaneGeometry(r * 2, r * 0.5 * stretch);
    case 'visor': return new RoundedBoxGeometry(r * 3, r * 0.9 * stretch, r * 0.2, 2, r * 0.3);
    case 'ring': return new THREE.RingGeometry(r * 0.55, r, 28).scale(1, stretch, 1);
    case 'heart': {
      const k = r;
      const heart = new THREE.Shape();
      heart.moveTo(0, -k * 0.8);
      heart.bezierCurveTo(-k * 1.2, -k * 0.1, -k * 0.6, k * 1.0, 0, k * 0.5);
      heart.bezierCurveTo(k * 0.6, k * 1.0, k * 1.2, -k * 0.1, 0, -k * 0.8);
      return new THREE.ShapeGeometry(heart, 8).scale(1, stretch, 1);
    }
    case 'arc': {
      // A closed, smiling eye: the lower half of a ring.
      const geo = new THREE.TorusGeometry(r, r * 0.22, 6, 16, Math.PI);
      geo.rotateZ(Math.PI);
      return geo;
    }
  }
}

/** A glowing eye's pupil at its own position, and its glass socket (a disc behind it, if any). */
export function eyeGeometries(e: Eye): { pupil: THREE.BufferGeometry; glass: THREE.BufferGeometry | null } {
  const pupil = eyeShape(e.shape, e.pupil, e.stretch ?? 1);
  pupil.translate(e.at[0], e.at[1], e.at[2] + 0.004);
  const glass = e.glass === false ? null : new THREE.CircleGeometry(e.r ?? 0.1, 24);
  glass?.translate(e.at[0], e.at[1], e.at[2]);
  return { pupil, glass };
}
