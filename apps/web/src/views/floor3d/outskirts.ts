// The land round the districts: the lamps along the paths, two neat rings of toy sci-fi pieces
// (habitat domes, comm towers, glowing groves, landing pads with a shuttle, cargo stacks, solar
// arrays), and the backdrop on the horizon (mesas, lit city towers, mounds in the middle distance).
// Toon-shaded primitives like the rest of the Floor; every solid piece reports its ground footprint.

import * as THREE from 'three';
import { RoundedBoxGeometry } from 'three/examples/jsm/geometries/RoundedBoxGeometry.js';
import { footprint, glowing, LAMP, part, segDist, shadows, type Box2, type PropPalette, type Site } from './props';
import { rng } from './textures';
import { outline, toonMat } from './toon';

/**
 * The outskirts: a lamp along every path, then two neat rings of toy sci-fi pieces round the
 * districts (habitat domes, comm towers, glowing groves in planters, landing pads with a shuttle,
 * cargo stacks, solar arrays), each turned to face the plaza and kept clear of the plaza, the
 * slabs and their ramps, the paths and each other.
 */
export function buildScatter(sites: Site[], _palette: PropPalette): { group: THREE.Group; boxes: Box2[] } {
  const g = new THREE.Group();
  const boxes: Box2[] = [];
  let far = 10;
  for (const s of sites) far = Math.max(far, Math.hypot(s.cx, s.cy) + s.r);

  // The toy kit: white shells, a dark trim, five accents, and their glowing twins.
  const shell = toonMat({ color: 0xe4e8f6 });
  const trim = toonMat({ color: 0x2a3050 });
  const steel = toonMat({ color: 0x8792b8 });
  const soil = toonMat({ color: 0x1d3a46 });
  const ACCENTS = [0xff7a6b, 0x2fd6c4, 0x5fb8ff, 0x9a7dff, 0xffc75f];
  const accent = ACCENTS.map((c) => toonMat({ color: c }));
  const neon = ACCENTS.map((c) => new THREE.MeshBasicMaterial({ color: c, toneMapped: false }));
  const crowns = [0x2fd6c4, 0x5fe0a0, 0xff8fd8, 0x7fc8ff].map((c) => glowing(c, 0.35));
  const warm = new THREE.MeshBasicMaterial({ color: 0xffd27a, toneMapped: false });
  const glass = toonMat({ color: 0x7fe3ff, emissive: 0x2ab8ff, emissiveIntensity: 0.6 });
  const panel = toonMat({ color: 0x2b3f9a, emissive: 0x1a2a80, emissiveIntensity: 0.4 });
  const bulb = new THREE.MeshBasicMaterial({ color: LAMP, toneMapped: false });

  /** Whether a piece of radius `rad` at (x, z) clears the plaza, the districts, the paths and the boxes so far. */
  const clear = (x: number, z: number, rad: number): boolean => {
    if (Math.hypot(x, z) < 6.4 + rad) return false;
    for (const s of sites) {
      if (Math.abs(x - s.cx) < s.r + 3 + rad && Math.abs(z - s.cy) < s.r + 3 + rad) return false;
      if (segDist(x, z, s.cx, s.cy) < 1.4 + rad) return false;
    }
    for (const b of boxes) {
      if (x + rad > b.x0 - 0.3 && x - rad < b.x1 + 0.3 && z + rad > b.z0 - 0.3 && z - rad < b.z1 + 0.3) return false;
    }
    return true;
  };

  /** A piece's node at (x, z), facing the plaza (its front is +z). */
  const place = (x: number, z: number): THREE.Group => {
    const node = new THREE.Group();
    node.position.set(x, 0, z);
    node.rotation.y = Math.atan2(-x, -z);
    g.add(node);
    return node;
  };
  const flat = (r: number, tube: number, seg = 48): THREE.TorusGeometry => {
    const geo = new THREE.TorusGeometry(r, tube, 8, seg);
    geo.rotateX(Math.PI / 2);
    return geo;
  };

  // A lamp: a slim pole with a round glowing head.
  const lamps: [number, number][] = [];
  const addLamp = (x: number, z: number): void => {
    // Paths meet at the centre, so lamps of neighbouring paths can land on one spot: keep one.
    if (lamps.some(([lx, lz]) => Math.hypot(lx - x, lz - z) < 3)) return;
    lamps.push([x, z]);
    const l = place(x, z);
    part(l, new THREE.CylinderGeometry(0.16, 0.2, 0.14, 16), trim, 0, 0.07, 0);
    const pole = part(l, new THREE.CylinderGeometry(0.04, 0.05, 2.1, 10), steel, 0, 1.1, 0);
    part(l, new THREE.CylinderGeometry(0.17, 0.12, 0.08, 16), trim, 0, 2.18, 0);
    part(l, new THREE.SphereGeometry(0.16, 16, 12), bulb, 0, 2.34, 0);
    boxes.push(footprint(pole));
  };

  // The pieces, each built round its node's origin; `r` is its footprint radius.
  const dome = (n: THREE.Group, k: number): void => {
    part(n, new THREE.CylinderGeometry(1.75, 1.85, 0.24, 40), trim, 0, 0.12, 0);
    part(n, new THREE.SphereGeometry(1.6, 36, 18, 0, Math.PI * 2, 0, Math.PI / 2), shell, 0, 0.24, 0);
    part(n, flat(1.61, 0.07), neon[k]!, 0, 0.42, 0);
    part(n, new RoundedBoxGeometry(0.72, 0.95, 0.6, 3, 0.16), accent[k]!, 0, 0.6, 1.38);
    part(n, new RoundedBoxGeometry(0.4, 0.5, 0.05, 2, 0.08), trim, 0, 0.62, 1.69);
    for (const side of [-1, 1]) part(n, new THREE.SphereGeometry(0.17, 14, 10), warm, side * 0.8, 1.05, 1.08);
    part(n, new THREE.CylinderGeometry(0.03, 0.03, 0.6, 6), steel, 0.55, 1.95, -0.3);
    part(n, new THREE.SphereGeometry(0.09, 10, 8), neon[k]!, 0.55, 2.28, -0.3);
  };
  const tower = (n: THREE.Group, k: number): void => {
    part(n, new RoundedBoxGeometry(1.3, 0.4, 1.3, 3, 0.14), trim, 0, 0.2, 0);
    part(n, new THREE.CylinderGeometry(0.42, 0.56, 2.4, 24), shell, 0, 1.6, 0);
    part(n, flat(0.5, 0.06, 32), neon[k]!, 0, 1.1, 0);
    part(n, flat(0.46, 0.06, 32), neon[k]!, 0, 2.2, 0);
    part(n, new THREE.CylinderGeometry(0.3, 0.42, 1.2, 24), shell, 0, 3.4, 0);
    part(n, new THREE.SphereGeometry(0.42, 20, 14), accent[k]!, 0, 4.2, 0);
    const dish = part(n, new THREE.SphereGeometry(0.55, 20, 10, 0, Math.PI * 2, 0, Math.PI * 0.32), steel, 0, 4.25, 0.3);
    dish.rotation.x = 1.0;
    part(n, new THREE.CylinderGeometry(0.025, 0.025, 0.7, 6), steel, 0, 4.9, 0);
    part(n, new THREE.SphereGeometry(0.08, 10, 8), warm, 0, 5.28, 0);
  };
  const grove = (n: THREE.Group, k: number): void => {
    part(n, new THREE.CylinderGeometry(1.25, 1.3, 0.36, 36), shell, 0, 0.18, 0);
    part(n, flat(1.27, 0.05), neon[k]!, 0, 0.3, 0);
    part(n, new THREE.CylinderGeometry(1.12, 1.12, 0.04, 36), soil, 0, 0.37, 0);
    for (let i = 0; i < 3; i++) {
      const a = (i / 3) * Math.PI * 2 + k;
      const h = 1.0 + ((i + k) % 3) * 0.3;
      const x = Math.cos(a) * 0.55;
      const z = Math.sin(a) * 0.55;
      part(n, new THREE.CylinderGeometry(0.06, 0.08, h, 8), trim, x, 0.37 + h / 2, z);
      part(n, new THREE.SphereGeometry(0.42 + (i % 2) * 0.12, 18, 14), crowns[(i + k) % crowns.length]!, x, 0.37 + h + 0.3, z);
    }
  };
  const pad = (n: THREE.Group, k: number): void => {
    part(n, new THREE.CylinderGeometry(2.0, 2.1, 0.16, 48), trim, 0, 0.08, 0);
    part(n, flat(1.62, 0.05, 64), neon[k]!, 0, 0.17, 0);
    for (const rot of [0.6, -0.6]) {
      const stripe = part(n, new THREE.BoxGeometry(2.2, 0.02, 0.12), neon[k]!, 0, 0.17, 0);
      stripe.rotation.y = rot;
    }
    // A little shuttle: a round-ended body along z, wings, a fin, a glass canopy, three legs.
    const ship = new THREE.Group();
    ship.position.set(0, 0.72, 0);
    ship.rotation.y = 0.5;
    n.add(ship);
    const body = new THREE.CylinderGeometry(0.42, 0.42, 1.3, 24);
    body.rotateX(Math.PI / 2);
    part(ship, body, shell, 0, 0, 0);
    part(ship, new THREE.SphereGeometry(0.42, 24, 16), shell, 0, 0, 0.65);
    part(ship, new THREE.SphereGeometry(0.4, 24, 16), accent[k]!, 0, 0, -0.65);
    part(ship, new THREE.SphereGeometry(0.26, 18, 12), glass, 0, 0.22, 0.6);
    part(ship, new RoundedBoxGeometry(1.9, 0.08, 0.55, 2, 0.04), accent[k]!, 0, -0.08, -0.1);
    part(ship, new RoundedBoxGeometry(0.08, 0.5, 0.45, 2, 0.04), accent[k]!, 0, 0.42, -0.55);
    for (const [lx, lz] of [[-0.35, -0.3], [0.35, -0.3], [0, 0.45]] as const) {
      part(ship, new THREE.CylinderGeometry(0.04, 0.05, 0.5, 6), steel, lx, -0.42, lz);
    }
  };
  const crates = (n: THREE.Group, k: number): void => {
    const box = new RoundedBoxGeometry(0.8, 0.8, 0.8, 3, 0.1);
    const band = new THREE.BoxGeometry(0.84, 0.12, 0.84);
    const stack: [number, number, number, number][] = [[-0.45, 0.4, 0, 0], [0.45, 0.4, 0.05, 1], [0, 1.2, 0, 2], [0.2, 0.4, -0.9, 3]];
    for (const [x, y, z, i] of stack) {
      const c = part(n, box, accent[(k + i) % accent.length]!, x, y, z);
      c.rotation.y = i * 0.25;
      const b = part(n, band, trim, x, y, z);
      b.rotation.y = i * 0.25;
    }
  };
  const solar = (n: THREE.Group, k: number): void => {
    part(n, new RoundedBoxGeometry(2.6, 0.2, 0.9, 2, 0.08), trim, 0, 0.1, 0);
    for (const x of [-0.9, 0, 0.9]) {
      part(n, new THREE.CylinderGeometry(0.04, 0.05, 0.7, 6), steel, x, 0.5, 0);
      const p = part(n, new RoundedBoxGeometry(0.8, 0.05, 1.0, 2, 0.03), panel, x, 0.9, 0);
      p.rotation.x = -0.55;
    }
    part(n, new THREE.SphereGeometry(0.08, 10, 8), neon[k]!, 1.2, 0.3, 0.38);
  };
  const PIECES: Record<string, { r: number; build: (n: THREE.Group, k: number) => void }> = {
    dome: { r: 1.9, build: dome }, tower: { r: 0.9, build: tower }, grove: { r: 1.35, build: grove },
    pad: { r: 2.1, build: pad }, crates: { r: 1.1, build: crates }, solar: { r: 1.4, build: solar },
  };

  // One lamp per path, just off its edge where it reaches the district, sides alternating.
  sites.forEach((s, i) => {
    const dist = Math.hypot(s.cx, s.cy);
    const d = dist - s.r - 2.6;
    if (dist < 1 || d < 7) return;
    const ux = s.cx / dist;
    const uz = s.cy / dist;
    const side = i % 2 === 0 ? 1 : -1;
    addLamp(ux * d - uz * side * 0.95, uz * d + ux * side * 0.95);
  });

  // Two rings of pieces round the districts, the outer one offset by half a step; a spot that
  // clashes with something is left empty.
  const rings: { radius: number; step: number; kinds: string[] }[] = [
    { radius: far + 5.5, step: 6.5, kinds: ['grove', 'dome', 'crates', 'tower', 'grove', 'solar', 'pad'] },
    { radius: far + 14, step: 8.5, kinds: ['dome', 'tower', 'grove', 'pad', 'dome', 'solar', 'grove', 'tower'] },
  ];
  rings.forEach(({ radius, step, kinds }, ring) => {
    const n = Math.max(8, Math.floor((2 * Math.PI * radius) / step));
    for (let i = 0; i < n; i++) {
      const a = ((i + ring * 0.5) / n) * Math.PI * 2;
      const x = Math.cos(a) * radius;
      const z = Math.sin(a) * radius;
      const kind = kinds[i % kinds.length]!;
      const piece = PIECES[kind]!;
      if (!clear(x, z, piece.r)) continue;
      const node = place(x, z);
      piece.build(node, (i + ring * 2) % ACCENTS.length);
      shadows(node);
      boxes.push({ x0: x - piece.r, x1: x + piece.r, z0: z - piece.r, z1: z + piece.r });
    }
  });
  outline(g, 0.1);
  return { group: g, boxes };
}

/**
 * The backdrop on the horizon: a ring of toy mesas and city towers far out (the fog softens them
 * into the violet haze), the towers lit by rows of windows and a warm light on top; and low
 * mounds in the middle distance.
 */
export function buildBackdrop(): THREE.Group {
  const g = new THREE.Group();
  const rand = rng(4242);
  const rock = toonMat({ color: 0x3b3572 });
  const cap = toonMat({ color: 0x4d4794 });
  const block = toonMat({ color: 0x323a72 });
  const roof = toonMat({ color: 0x434c8e });
  const lit = [0xffd27a, 0x7fe8ff, 0xff9ad8].map((c) => new THREE.MeshBasicMaterial({ color: c, toneMapped: false }));
  const N = 34;
  for (let i = 0; i < N; i++) {
    const a = (i / N) * Math.PI * 2 + rand() * 0.08;
    const radius = 120 + rand() * 40;
    const node = new THREE.Group();
    node.position.set(Math.cos(a) * radius, 0, Math.sin(a) * radius);
    node.rotation.y = Math.atan2(-node.position.x, -node.position.z);
    if (i % 3 !== 1) {
      // A mesa: a wide rounded hill with a lighter cap, sometimes a second, smaller one beside it.
      const w = 8 + rand() * 9;
      const h = 4 + rand() * 7;
      part(node, new THREE.CylinderGeometry(w * 0.8, w, h, 28), rock, 0, h / 2, 0);
      part(node, new THREE.SphereGeometry(w * 0.8, 28, 10, 0, Math.PI * 2, 0, Math.PI / 2).scale(1, 0.25, 1), cap, 0, h, 0);
      if (rand() < 0.6) {
        const w2 = w * 0.55;
        const h2 = h * (0.5 + rand() * 0.4);
        part(node, new THREE.CylinderGeometry(w2 * 0.8, w2, h2, 24), rock, w * 1.1, h2 / 2, 2);
        part(node, new THREE.SphereGeometry(w2 * 0.8, 24, 8, 0, Math.PI * 2, 0, Math.PI / 2).scale(1, 0.25, 1), cap, w * 1.1, h2, 2);
      }
    } else {
      // A cluster of city towers, rounded, with lit window rows on their fronts.
      const count = 2 + Math.floor(rand() * 3);
      for (let k = 0; k < count; k++) {
        const w = 3 + rand() * 3;
        const h = 12 + rand() * 22;
        const x = (k - (count - 1) / 2) * (w + 1.5);
        part(node, new RoundedBoxGeometry(w, h, w, 3, 0.9), block, x, h / 2, 0);
        part(node, new RoundedBoxGeometry(w * 0.7, 1.4, w * 0.7, 3, 0.5), roof, x, h + 0.7, 0);
        const light = lit[(i + k) % lit.length]!;
        for (let y = 3; y < h - 2; y += 2.6) {
          if (rand() < 0.25) continue;
          part(node, new THREE.BoxGeometry(w * 0.62, 0.45, 0.1), light, x, y, w / 2 + 0.02);
        }
        part(node, new THREE.CylinderGeometry(0.12, 0.12, 3, 6), roof, x, h + 2.9, 0);
        part(node, new THREE.SphereGeometry(0.45, 10, 8), lit[0]!, x, h + 4.5, 0);
      }
    }
    g.add(node);
  }
  // The middle distance: low rolling mounds, so the land between the outskirts and the horizon
  // is not bare.
  const mound = toonMat({ color: 0x2c3366 });
  const moundTop = toonMat({ color: 0x353d76 });
  for (let i = 0; i < 40; i++) {
    const a = rand() * Math.PI * 2;
    const radius = 55 + rand() * 45;
    const w = 3 + rand() * 5;
    const m = part(g, new THREE.SphereGeometry(w, 24, 12, 0, Math.PI * 2, 0, Math.PI / 2).scale(1.4, 0.4, 1), i % 2 ? mound : moundTop, Math.cos(a) * radius, 0, Math.sin(a) * radius);
    m.rotation.y = rand() * Math.PI;
  }
  outline(g, 2);
  return g;
}
