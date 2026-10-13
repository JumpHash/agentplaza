// The props that furnish the plaza outside the district rooms (those are in rooms/, the outskirts
// and the horizon in outskirts.ts): the central plaza's ring and its xenon atom; the holo fountain
// and market stalls around it; the paths with their light lines.
// Everything is toon-shaded primitives; each group carries what the scene animates in
// `userData.animate(t)`, and every solid piece reports its ground footprint so robots collide with it.

import * as THREE from 'three';
import { outline, toonMat } from './toon';
import { canvasTexture, metal as metalTex, polish, rng, water } from './textures';

/** The palette colours the props need, already resolved to three.js colours. */
export interface PropPalette {
  sheet: THREE.Color;
  rule: THREE.Color;
  ink3: THREE.Color;
  signal: THREE.Color;
  ok: THREE.Color;
  info: THREE.Color;
}

/** A prop's ground footprint, in grid units: the box it occupies on the floor. */
export interface Box2 {
  x0: number;
  x1: number;
  z0: number;
  z1: number;
}

/** A district as the scene lays it out: the centre on the floor and the half-width of its slab. */
export interface Site {
  cx: number;
  cy: number;
  r: number;
}

/** Plaza furniture: its group, its footprints and its per-frame animation. */
export interface Furniture {
  group: THREE.Group;
  boxes: Box2[];
  animate: (t: number) => void;
}

export const LAMP = new THREE.Color(0x7fe8ff);
/** Dark hull metal, for benches, stalls and planters. */
const HULL = new THREE.Color(0x56607e);

/** A toon colour that also glows a little: the plaza's bioluminescence and crystals. */
export function glowing(color: number, intensity: number): THREE.MeshToonMaterial {
  return toonMat({ color, emissive: color, emissiveIntensity: intensity });
}

function std(color: THREE.Color, roughness: number, metalness = 0.1): THREE.MeshToonMaterial {
  return toonMat({ color, roughness, metalness });
}

export function part(parent: THREE.Object3D, geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number): THREE.Mesh {
  const m = new THREE.Mesh(geo, mat);
  m.position.set(x, y, z);
  parent.add(m);
  return m;
}

export function shadows(root: THREE.Object3D): void {
  root.traverse((o) => {
    if (o instanceof THREE.Mesh) {
      o.castShadow = true;
      o.receiveShadow = true;
    }
  });
}

/** The world-space ground footprint of an object, from its bounding box. */
export function footprint(o: THREE.Object3D): Box2 {
  o.updateWorldMatrix(true, false);
  const b = new THREE.Box3().setFromObject(o);
  return { x0: b.min.x, x1: b.max.x, z0: b.min.z, z1: b.max.z };
}

/** The smallest angle between two bearings, in radians. */
function angDist(a: number, b: number): number {
  return Math.abs(Math.atan2(Math.sin(a - b), Math.cos(a - b)));
}

/** Distance from (px, pz) to the segment from the origin to (bx, bz). */
export function segDist(px: number, pz: number, bx: number, bz: number): number {
  const len2 = Math.max(bx * bx + bz * bz, 1e-6);
  const t = Math.max(0, Math.min(1, (px * bx + pz * bz) / len2));
  return Math.hypot(px - bx * t, pz - bz * t);
}

/** A bench at the origin, its seat facing +z and its back at -z. */
function bench(mat: THREE.Material): THREE.Group {
  const b = new THREE.Group();
  part(b, new THREE.BoxGeometry(1.3, 0.08, 0.42), mat, 0, 0.42, 0);
  part(b, new THREE.BoxGeometry(0.08, 0.4, 0.36), mat, -0.55, 0.2, 0);
  part(b, new THREE.BoxGeometry(0.08, 0.4, 0.36), mat, 0.55, 0.2, 0);
  part(b, new THREE.BoxGeometry(1.3, 0.4, 0.07), mat, 0, 0.66, -0.2);
  return b;
}

/** The central plaza: a round paved square with a darker ring, four benches and a slowly turning hologram above the dais. */
export function buildPlaza(palette: PropPalette): THREE.Group {
  const g = new THREE.Group();
  const stone = palette.rule.clone().lerp(palette.sheet, 0.4);
  const pave = part(g, new THREE.CylinderGeometry(4.3, 4.45, 0.06, 64), std(stone, 0.9), 0, 0.03, 0);
  pave.receiveShadow = true;
  const ring = part(g, new THREE.RingGeometry(3.7, 3.95, 72), std(stone.clone().multiplyScalar(0.72), 0.9, 0), 0, 0.066, 0);
  ring.rotation.x = -Math.PI / 2;
  const inlay = part(g, new THREE.TorusGeometry(3.3, 0.025, 6, 72), std(palette.rule, 0.8, 0), 0, 0.065, 0);
  inlay.rotation.x = Math.PI / 2;

  const benchWood = toonMat({ map: metalTex(), color: HULL });
  for (let i = 0; i < 4; i++) {
    const a = (i / 4) * Math.PI * 2;
    const x = Math.cos(a) * 3.6;
    const z = Math.sin(a) * 3.6;
    const b = bench(benchWood);
    b.position.set(x, 0, z);
    b.rotation.y = Math.atan2(-x, -z); // the seat faces the centre, the back faces out
    g.add(b);
  }

  const atom = xenonAtom(palette, 4.2);
  g.add(atom.group);
  g.userData = { animate: (t: number) => atom.animate(t) };
  shadows(g);
  atom.group.traverse((o) => (o.castShadow = false));
  outline(g);
  return g;
}

/** The fountain and market stalls on the plaza ring, placed in the gaps between the paths. */
export function buildPlazaFurniture(palette: PropPalette, bearings: number[]): Furniture {
  const g = new THREE.Group();
  const boxes: Box2[] = [];
  const RING = 4.0;

  // Eight candidate spots around the ring, kept off the paths and spread at least two apart.
  const chosen: number[] = [];
  for (let k = 0; k < 8; k++) {
    const a = Math.PI / 8 + (k * Math.PI) / 4;
    if (bearings.some((b) => angDist(a, b) < 0.3)) continue;
    if (chosen.every((c) => angDist(a, c) > 1.0)) chosen.push(a);
  }

  const waterTex = water();
  const stoneMat = toonMat({ map: polish(palette.sheet), roughness: 0.4 });
  const waterMat = toonMat({ map: waterTex, emissive: 0x2ad0ff, emissiveIntensity: 0.55, transparent: true, opacity: 0.9 });
  const counterMat = toonMat({ map: metalTex(), color: HULL });
  const poleMat = toonMat({ map: metalTex(), roughness: 0.4, metalness: 0.5 });
  const awningTex = canvasTexture(`awning:${palette.signal.getStyle()}:${palette.sheet.getStyle()}`, 256, 64, 1, (cg, w, h) => {
    const n = 8;
    for (let i = 0; i < n; i++) {
      cg.fillStyle = i % 2 === 1 ? palette.signal.getStyle() : '#d8deef';
      cg.fillRect((i * w) / n, 0, w / n, h);
    }
  });
  const awningMat = toonMat({ map: awningTex, roughness: 0.9, side: THREE.DoubleSide });

  chosen.forEach((a, i) => {
    const x = Math.cos(a) * RING;
    const z = Math.sin(a) * RING;
    const node = new THREE.Group();
    node.position.set(x, 0, z);
    if (i === 0) {
      // The fountain: a basin with a glowing pool and a spout of light.
      const basin = part(node, new THREE.CylinderGeometry(0.62, 0.7, 0.26, 32), stoneMat, 0, 0.13, 0);
      const pool = part(node, new THREE.CircleGeometry(0.55, 32), waterMat, 0, 0.27, 0);
      pool.rotation.x = -Math.PI / 2;
      part(node, new THREE.CylinderGeometry(0.05, 0.07, 0.6, 8), stoneMat, 0, 0.56, 0);
      part(node, new THREE.SphereGeometry(0.11, 12, 8), waterMat, 0, 0.92, 0);
      boxes.push(footprint(basin));
    } else {
      // A market stall: a metal counter under a striped awning, turned to face the plaza's centre.
      node.rotation.y = Math.atan2(-x, -z);
      const counter = part(node, new THREE.BoxGeometry(1.1, 0.72, 0.5), counterMat, 0, 0.36, 0);
      part(node, new THREE.BoxGeometry(0.26, 0.18, 0.26), std(palette.ok, 0.7, 0), 0.25, 0.81, 0);
      part(node, new THREE.BoxGeometry(0.24, 0.16, 0.24), std(palette.signal, 0.7, 0), -0.3, 0.8, 0.05);
      for (const px of [-0.5, 0.5]) part(node, new THREE.CylinderGeometry(0.03, 0.03, 2.0, 6), poleMat, px, 1.0, 0.42);
      const awning = part(node, new THREE.BoxGeometry(1.4, 0.04, 0.95), awningMat, 0, 2.0, 0.1);
      awning.rotation.x = 0.2; // the front edge drops toward the customers
      boxes.push(footprint(counter));
    }
    shadows(node);
    g.add(node);
  });

  const animate = (t: number): void => {
    waterTex.offset.y = (t * 0.04) % 1;
  };
  outline(g);
  return { group: g, boxes, animate };
}

/**
 * Plated strips from the origin out to each point (y = 0), for the paths between the centre and the
 * districts, each with a light line down either edge.
 */
export function buildPaths(from: THREE.Vector3[], palette: PropPalette): THREE.Group {
  const g = new THREE.Group();
  const mat = std(palette.rule.clone().lerp(palette.sheet, 0.35), 0.95);
  const light = new THREE.MeshBasicMaterial({ color: palette.info, toneMapped: false });
  for (const to of from) {
    const len = Math.hypot(to.x, to.z);
    if (len < 0.5) continue;
    const geo = new THREE.BoxGeometry(len, 0.03, 0.9);
    // Repeat the stones along the strip instead of stretching one tile over it.
    const uv = geo.getAttribute('uv') as THREE.BufferAttribute;
    for (let i = 0; i < uv.count; i++) uv.setX(i, uv.getX(i) * (len / 0.9));
    const strip = new THREE.Mesh(geo, mat);
    strip.position.set(to.x / 2, 0.015, to.z / 2);
    strip.rotation.y = -Math.atan2(to.z, to.x);
    strip.receiveShadow = true;
    g.add(strip);
    for (const side of [-0.41, 0.41]) {
      const line = new THREE.Mesh(new THREE.BoxGeometry(len, 0.02, 0.05), light);
      line.position.set(0, 0.02, side);
      strip.add(line);
    }
  }
  return g;
}


/** The xenon atom the plaza is built around: its nucleus, halo, aura, light and five electron shells. */
/** Its group is centred on its origin, lifted to `height`; `animate(t)` runs its per-frame motion. */
export function xenonAtom(palette: PropPalette, height: number): { group: THREE.Group; animate: (t: number) => void } {
  // A xenon atom above the orchestrator: a pulsing, glowing nucleus and its 54 electrons on five
  // shells (2, 8, 18, 18, 8), each shell tilted its own way, the inner ones spinning faster.
  const XENON = new THREE.Color(0x9fa8ff);
  const globe = new THREE.Group();
  const nucleus = new THREE.Group();
  const proton = toonMat({ color: palette.signal, emissive: palette.signal, emissiveIntensity: 0.45 });
  const neutron = toonMat({ color: palette.sheet, emissive: 0xffe2b0, emissiveIntensity: 0.25 });
  const nucleonGeo = new THREE.SphereGeometry(0.13, 14, 10);
  const nr = rng(54);
  for (let i = 0; i < 24; i++) {
    const v = new THREE.Vector3(nr() - 0.5, nr() - 0.5, nr() - 0.5).normalize().multiplyScalar(Math.cbrt(nr()) * 0.24);
    part(nucleus, nucleonGeo, i % 2 ? proton : neutron, v.x, v.y, v.z);
  }
  globe.add(nucleus);
  const haloMat = new THREE.MeshBasicMaterial({ color: XENON, transparent: true, opacity: 0.35, depthWrite: false, blending: THREE.AdditiveBlending, toneMapped: false });
  const halo = new THREE.Mesh(new THREE.SphereGeometry(0.62, 24, 16), haloMat);
  globe.add(halo);
  const auraMat = new THREE.MeshBasicMaterial({ color: XENON, transparent: true, opacity: 0.1, depthWrite: false, blending: THREE.AdditiveBlending, toneMapped: false });
  const aura = new THREE.Mesh(new THREE.SphereGeometry(1.9, 24, 16), auraMat);
  globe.add(aura);
  const glow = new THREE.PointLight(XENON, 2.2, 9, 1.6);
  globe.add(glow);
  const ringMat = toonMat({ color: 0xc9ccff, emissive: XENON, emissiveIntensity: 0.6 });
  const electronMat = toonMat({ color: 0x8ff0ff, emissive: 0x5fd8ff, emissiveIntensity: 1.1 });
  const electronGeo = new THREE.SphereGeometry(0.065, 12, 8);
  const shells: { group: THREE.Group; electrons: THREE.Mesh[]; r: number; speed: number }[] = [];
  [2, 8, 18, 18, 8].forEach((n, k) => {
    const r = 0.62 + k * 0.27;
    const shell = new THREE.Group();
    shell.rotation.set(nr() * Math.PI, nr() * Math.PI, nr() * Math.PI);
    const ring = new THREE.Mesh(new THREE.TorusGeometry(r, 0.012, 6, 96), ringMat);
    ring.rotation.x = Math.PI / 2;
    ring.userData.outline = false; // a fine glowing track, not an inked hoop
    shell.add(ring);
    const electrons: THREE.Mesh[] = [];
    for (let i = 0; i < n; i++) electrons.push(part(shell, electronGeo, electronMat, 0, 0, 0));
    globe.add(shell);
    shells.push({ group: shell, electrons, r, speed: 1.9 / (1 + k * 0.45) * (k % 2 ? -1 : 1) });
  });
  outline(globe, 0.05);
  globe.scale.setScalar(1.5);
  globe.position.set(0, height, 0);
  const animate = (t: number): void => {
    globe.position.y = height + Math.sin(t * 0.9) * 0.1;
    // The nucleus beats, its halo and light breathing with it.
    const beat = Math.pow(Math.max(0, Math.sin(t * 2.4)), 6);
    nucleus.scale.setScalar(1 + 0.12 * beat);
    nucleus.rotation.y = t * 0.6;
    halo.scale.setScalar(1 + 0.35 * beat + 0.05 * Math.sin(t * 7));
    haloMat.opacity = 0.3 + 0.35 * beat;
    auraMat.opacity = 0.07 + 0.08 * beat;
    aura.scale.setScalar(1 + 0.1 * beat);
    glow.intensity = 1.6 + 2.4 * beat;
    for (const sh of shells) {
      sh.group.rotation.y += 0.002;
      const n = sh.electrons.length;
      sh.electrons.forEach((e, i) => {
        const a = t * sh.speed + (i / n) * Math.PI * 2;
        e.position.set(Math.cos(a) * sh.r, 0, Math.sin(a) * sh.r);
        e.scale.setScalar(1 + 0.25 * beat);
      });
    }
  };
  return { group: globe, animate };
}
