// The props that furnish the Floor: each district's desks, plants, crates, lamp, hedges, signpost and
// role furniture; the central plaza's paved ring; the fountain and market stalls around it; the paths
// and the street lamps along them; and the trees, flower beds, benches and rocks scattered outside the
// districts. Everything is primitives with textured MeshStandardMaterials: the built things in the
// plaza's palette, the greenery in natural colours. Each group carries what the scene animates in
// userData (`screens`, `animate(t)`), and every solid piece reports its ground footprint so that the
// robots collide with it.

import * as THREE from 'three';
import { outline, toonMat } from './toon';
import { canvasTexture, metal as metalTex, paving, polish, rng, water, wood as woodTex, type Rand } from './textures';

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

const TECH = new Set(['builder', 'qa', 'release', 'security']);
const LIBRARY = new Set(['researcher', 'librarian', 'planner']);
const BOARD = new Set(['architect', 'reviewer']);
const LAMP = new THREE.Color(0xffd79a);
const CLAY = new THREE.Color(0xc98b66);

function std(color: THREE.Color, roughness: number, metalness = 0.1): THREE.MeshToonMaterial {
  return toonMat({ color, roughness, metalness });
}

/** `base` mixed a little toward the role hue, so each district carries its own tint. */
function tint(base: THREE.Color, hue: number, amount: number): THREE.Color {
  const hued = new THREE.Color().setHSL(hue / 360, 0.45, 0.6, THREE.SRGBColorSpace);
  return base.clone().lerp(hued, amount);
}

function part(parent: THREE.Object3D, geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number): THREE.Mesh {
  const m = new THREE.Mesh(geo, mat);
  m.position.set(x, y, z);
  parent.add(m);
  return m;
}

function shadows(root: THREE.Object3D): void {
  root.traverse((o) => {
    if (o instanceof THREE.Mesh) {
      o.castShadow = true;
      o.receiveShadow = true;
    }
  });
}

/** The world-space ground footprint of an object, from its bounding box. */
function footprint(o: THREE.Object3D): Box2 {
  o.updateWorldMatrix(true, false);
  const b = new THREE.Box3().setFromObject(o);
  return { x0: b.min.x, x1: b.max.x, z0: b.min.z, z1: b.max.z };
}

function pick<T>(items: T[], r: Rand): T {
  return items[Math.floor(r() * items.length)] as T;
}

/** The smallest angle between two bearings, in radians. */
function angDist(a: number, b: number): number {
  return Math.abs(Math.atan2(Math.sin(a - b), Math.cos(a - b)));
}

/** Distance from (px, pz) to the segment from the origin to (bx, bz). */
function segDist(px: number, pz: number, bx: number, bz: number): number {
  const len2 = Math.max(bx * bx + bz * bz, 1e-6);
  const t = Math.max(0, Math.min(1, (px * bx + pz * bz) / len2));
  return Math.hypot(px - bx * t, pz - bz * t);
}

/** A wooden bench at the origin, its seat facing +z and its back at -z. */
function bench(mat: THREE.Material): THREE.Group {
  const b = new THREE.Group();
  part(b, new THREE.BoxGeometry(1.3, 0.08, 0.42), mat, 0, 0.42, 0);
  part(b, new THREE.BoxGeometry(0.08, 0.4, 0.36), mat, -0.55, 0.2, 0);
  part(b, new THREE.BoxGeometry(0.08, 0.4, 0.36), mat, 0.55, 0.2, 0);
  part(b, new THREE.BoxGeometry(1.3, 0.4, 0.07), mat, 0, 0.66, -0.2);
  return b;
}

/** A sign plate: the role name in ink on the sheet colour. */
function signMaterial(text: string, bg: THREE.Color, ink: THREE.Color): THREE.MeshToonMaterial {
  const tex = canvasTexture(`sign:${text}:${bg.getStyle()}:${ink.getStyle()}`, 256, 96, 1, (g, w, h) => {
    g.fillStyle = bg.getStyle();
    g.fillRect(0, 0, w, h);
    g.strokeStyle = ink.getStyle();
    g.lineWidth = 6;
    g.strokeRect(6, 6, w - 12, h - 12);
    g.fillStyle = ink.getStyle();
    g.font = '600 30px ui-monospace, SFMono-Regular, Menlo, monospace';
    g.textAlign = 'center';
    g.textBaseline = 'middle';
    g.fillText(text, w / 2, h / 2 + 2);
  });
  return toonMat({ map: tex, roughness: 0.6 });
}

function desk(parent: THREE.Object3D, x: number, z: number, wood: THREE.Material, metal: THREE.Material, screen: THREE.Material): void {
  const d = new THREE.Group();
  d.position.set(x, 0, z);
  part(d, new THREE.BoxGeometry(1.0, 0.06, 0.55), wood, 0, 0.55, 0);
  part(d, new THREE.BoxGeometry(0.08, 0.55, 0.5), wood, -0.42, 0.275, 0);
  part(d, new THREE.BoxGeometry(0.08, 0.55, 0.5), wood, 0.42, 0.275, 0);
  part(d, new THREE.BoxGeometry(0.3, 0.02, 0.18), metal, 0, 0.59, -0.12);
  part(d, new THREE.BoxGeometry(0.07, 0.2, 0.05), metal, 0, 0.7, -0.12);
  part(d, new THREE.BoxGeometry(0.62, 0.38, 0.04), screen, 0, 0.95, -0.14);
  parent.add(d);
}

function plant(parent: THREE.Object3D, x: number, z: number, scale: number, pot: THREE.Material, leaf: THREE.Material, leaf2: THREE.Material): void {
  part(parent, new THREE.CylinderGeometry(0.22 * scale, 0.16 * scale, 0.3 * scale, 14), pot, x, 0.15 * scale, z);
  part(parent, new THREE.SphereGeometry(0.34 * scale, 14, 10), leaf, x, 0.5 * scale, z);
  part(parent, new THREE.ConeGeometry(0.22 * scale, 0.45 * scale, 10), leaf2, x, 0.85 * scale, z);
}

/** One district's furniture, built around its slab: the origin is the slab's centre at its top. */
export function buildDistrictProps(roleHue: number, size: number, role: string, palette: PropPalette): THREE.Group {
  const g = new THREE.Group();
  const s = size;
  const wood = std(tint(palette.rule, roleHue, 0.3), 0.8, 0);
  const dark = std(tint(palette.ink3, roleHue, 0.2), 0.6, 0.2);
  const metal = std(tint(palette.ink3, roleHue, 0.1), 0.45, 0.4);
  const clay = std(tint(CLAY, roleHue, 0.1), 0.9, 0);
  const leaf = std(tint(palette.ok, roleHue, 0.2), 0.85, 0);
  const leaf2 = std(tint(palette.ok, roleHue, 0.45), 0.85, 0);
  const crate = std(tint(palette.rule, roleHue, 0.55), 0.9, 0);
  const screen = toonMat({
    color: tint(palette.ink3, roleHue, 0.3),
    emissive: palette.info,
    emissiveIntensity: 0.05,
    roughness: 0.35,
  });
  const ledOk = new THREE.MeshBasicMaterial({ color: palette.ok, toneMapped: false });
  const ledInfo = new THREE.MeshBasicMaterial({ color: palette.info, toneMapped: false });

  // Two or three desks along the front edge, each with a small monitor.
  const deskCount = s >= 3 ? 3 : 2;
  for (let i = 0; i < deskCount; i++) {
    desk(g, -s + 1.2 + (i * (2 * s - 2.4)) / (deskCount - 1), s - 0.9, wood, metal, screen);
  }

  // A potted plant or two at the back corners.
  plant(g, s - 0.5, -s + 0.55, 1, clay, leaf, leaf2);
  if (s >= 3) plant(g, -s + 0.5, -s + 0.55, 0.9, clay, leaf, leaf2);

  // A couple of crates on the right wall.
  part(g, new THREE.BoxGeometry(0.5, 0.5, 0.5), crate, s - 0.5, 0.25, 0.05);
  part(g, new THREE.BoxGeometry(0.45, 0.45, 0.45), crate, s - 0.45, 0.725, 0.1);

  // Low hedges along both side walls, kept off the ramp band.
  for (const x of [s - 1.6, -s + 1.6]) part(g, new THREE.BoxGeometry(0.3, 0.4, 1.2), leaf, x, 0.2, 0);

  // A lamp post at the front left, with a warm, low, shadowless point light.
  const lampX = -s + 0.4;
  const lampZ = s - 0.4;
  part(g, new THREE.CylinderGeometry(0.035, 0.035, 1.9, 8), metal, lampX, 0.95, lampZ);
  const bulbMat = new THREE.MeshBasicMaterial({ color: LAMP, toneMapped: false });
  part(g, new THREE.SphereGeometry(0.13, 12, 8), bulbMat, lampX, 1.95, lampZ);
  const light = new THREE.PointLight(LAMP, 0.35, 4.5, 2);
  light.position.set(lampX, 1.8, lampZ);
  g.add(light);

  // Role flavour on the left wall.
  const fx = -s + 0.5;
  if (TECH.has(role)) {
    // Server racks with blinking LED dots on their faces.
    for (const z of [-0.55, 0.2]) {
      part(g, new THREE.BoxGeometry(0.5, 1.5, 0.45), dark, fx, 0.75, z);
      for (const y of [0.3, 0.6, 0.9, 1.2]) {
        for (const dz of [-0.12, 0.12]) {
          part(g, new THREE.BoxGeometry(0.02, 0.05, 0.05), y + dz > 0.7 ? ledOk : ledInfo, fx + 0.26, y, z + dz);
        }
      }
    }
  } else if (LIBRARY.has(role)) {
    // A bookshelf: three shelves with spines on the room-facing side.
    const books = [tint(palette.signal, roleHue, 0.3), tint(palette.info, roleHue, 0.2), tint(palette.ok, roleHue, 0.1)].map((c) => std(c, 0.7, 0));
    part(g, new THREE.BoxGeometry(0.5, 1.5, 1.0), dark, fx, 0.75, -0.2);
    for (const y of [0.35, 0.75, 1.15]) {
      part(g, new THREE.BoxGeometry(0.56, 0.04, 1.0), wood, fx, y, -0.2);
      for (let k = 0; k < 4; k++) {
        part(g, new THREE.BoxGeometry(0.02, 0.22, 0.12), books[k % books.length] ?? wood, fx + 0.265, y + 0.13, -0.6 + k * 0.28);
      }
    }
  } else if (BOARD.has(role)) {
    // A whiteboard on the left wall, with two marker strokes.
    const white = std(palette.sheet, 0.5, 0);
    part(g, new THREE.BoxGeometry(0.08, 1.0, 1.5), metal, -s + 0.1, 1.05, 0);
    part(g, new THREE.BoxGeometry(0.02, 0.8, 1.3), white, -s + 0.15, 1.05, 0);
    part(g, new THREE.BoxGeometry(0.02, 0.12, 0.4), std(palette.signal, 0.6, 0), -s + 0.17, 1.2, -0.25);
    part(g, new THREE.BoxGeometry(0.02, 0.1, 0.5), std(palette.info, 0.6, 0), -s + 0.17, 0.9, 0.3);
  } else {
    // A long planter with foliage.
    part(g, new THREE.BoxGeometry(0.4, 0.35, 1.3), clay, fx, 0.175, -0.1);
    part(g, new THREE.SphereGeometry(0.3, 12, 8), leaf, fx, 0.55, -0.5);
    part(g, new THREE.SphereGeometry(0.3, 12, 8), leaf2, fx, 0.55, 0.3);
  }

  // A signpost at the back of the slab, naming the district on its plate.
  const signMat = signMaterial(role.toUpperCase(), palette.sheet, palette.ink3);
  const sign = new THREE.Group();
  sign.position.set(0, 0, -s + 1.5);
  part(sign, new THREE.CylinderGeometry(0.035, 0.035, 1.2, 8), metal, 0, 0.6, 0);
  const plate = new THREE.Mesh(new THREE.BoxGeometry(0.9, 0.34, 0.04), [wood, wood, wood, wood, signMat, signMat]);
  plate.position.set(0, 1.05, 0);
  sign.add(plate);
  g.add(sign);

  g.userData = {
    screens: [screen],
    animate: (t: number) => {
      ledOk.color.copy(palette.ok).multiplyScalar(0.35 + 0.65 * Math.max(0, Math.sin(t * 2.7)));
      ledInfo.color.copy(palette.info).multiplyScalar(0.35 + 0.65 * Math.max(0, Math.sin(t * 3.9 + 1)));
      light.intensity = 0.35 * (1 + 0.03 * Math.sin(t * 7.1) + 0.02 * Math.sin(t * 2.3));
    },
  };
  shadows(g);
  return g;
}

/** The central plaza: a round paved square with a darker ring, four benches and a slowly turning hologram above the dais. */
export function buildPlaza(palette: PropPalette): THREE.Group {
  const g = new THREE.Group();
  const stone = palette.rule.clone().lerp(palette.sheet, 0.4);
  const pave = part(g, new THREE.CylinderGeometry(4.3, 4.45, 0.06, 64), toonMat({ map: paving(stone, 4), roughness: 0.9 }), 0, 0.03, 0);
  pave.receiveShadow = true;
  const ring = part(g, new THREE.RingGeometry(3.7, 3.95, 72), std(stone.clone().multiplyScalar(0.72), 0.9, 0), 0, 0.066, 0);
  ring.rotation.x = -Math.PI / 2;
  const inlay = part(g, new THREE.TorusGeometry(3.3, 0.025, 6, 72), std(palette.rule, 0.8, 0), 0, 0.065, 0);
  inlay.rotation.x = Math.PI / 2;

  const benchWood = toonMat({ map: woodTex(), color: palette.rule.clone().lerp(palette.ink3, 0.25), roughness: 0.8 });
  for (let i = 0; i < 4; i++) {
    const a = (i / 4) * Math.PI * 2;
    const x = Math.cos(a) * 3.6;
    const z = Math.sin(a) * 3.6;
    const b = bench(benchWood);
    b.position.set(x, 0, z);
    b.rotation.y = Math.atan2(-x, -z); // the seat faces the centre, the back faces out
    g.add(b);
  }

  // A chunky low-poly beacon gem with an orbiting ring: the orchestrator's landmark.
  const globe = new THREE.Group();
  const gem = new THREE.Mesh(
    new THREE.OctahedronGeometry(0.5, 0),
    toonMat({ color: palette.signal, emissive: palette.signal, emissiveIntensity: 0.25 }),
  );
  gem.scale.set(1, 1.35, 1);
  const orbit = new THREE.Mesh(new THREE.TorusGeometry(0.85, 0.05, 8, 48), toonMat({ color: palette.sheet }));
  orbit.rotation.x = Math.PI / 2.4;
  globe.add(gem, orbit);
  globe.position.set(0, 3.7, 0);
  g.add(globe);

  g.userData = {
    animate: (t: number) => {
      globe.rotation.y = t * 0.4;
      globe.position.y = 3.7 + Math.sin(t * 0.9) * 0.08;
    },
  };
  shadows(g);
  globe.castShadow = false;
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
  const waterMat = toonMat({ map: waterTex, roughness: 0.12, transparent: true, opacity: 0.9 });
  const counterMat = toonMat({ map: woodTex(), roughness: 0.8 });
  const poleMat = toonMat({ map: metalTex(), roughness: 0.4, metalness: 0.5 });
  const awningTex = canvasTexture(`awning:${palette.signal.getStyle()}:${palette.sheet.getStyle()}`, 256, 64, 1, (cg, w, h) => {
    const n = 8;
    for (let i = 0; i < n; i++) {
      cg.fillStyle = i % 2 === 1 ? palette.signal.getStyle() : palette.sheet.getStyle();
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
      // The fountain: a stone basin with a rippling pool and a spout.
      const basin = part(node, new THREE.CylinderGeometry(0.62, 0.7, 0.26, 32), stoneMat, 0, 0.13, 0);
      const pool = part(node, new THREE.CircleGeometry(0.55, 32), waterMat, 0, 0.27, 0);
      pool.rotation.x = -Math.PI / 2;
      part(node, new THREE.CylinderGeometry(0.05, 0.07, 0.6, 8), stoneMat, 0, 0.56, 0);
      part(node, new THREE.SphereGeometry(0.11, 12, 8), waterMat, 0, 0.92, 0);
      boxes.push(footprint(basin));
    } else {
      // A market stall: a wooden counter under a striped awning, turned to face the plaza's centre.
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

/** Flat paved strips from the origin out to each point (y = 0), for the paths between the centre and the districts. */
export function buildPaths(from: THREE.Vector3[], palette: PropPalette): THREE.Group {
  const g = new THREE.Group();
  const mat = toonMat({ map: paving(palette.rule.clone().lerp(palette.sheet, 0.35), 2), roughness: 0.95 });
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
  }
  return g;
}

/**
 * Greenery, lamps and small street furniture outside the districts: street lamps along every path,
 * then trees, flower beds, rocks and benches scattered where they clash with neither the plaza, the
 * slabs and their ramps, the paths, nor each other.
 */
export function buildScatter(sites: Site[], palette: PropPalette): { group: THREE.Group; boxes: Box2[] } {
  const g = new THREE.Group();
  const boxes: Box2[] = [];
  const rand = rng(1177);
  let far = 0;
  for (const s of sites) far = Math.max(far, Math.hypot(s.cx, s.cy) + s.r);
  const reach = Math.min(34, far + 6);

  // Materials shared by every piece of the scatter.
  const bark = toonMat({ map: woodTex(), roughness: 0.9 });
  const greens = [0x5f8f4a, 0x77a25a, 0x4d7a3c, 0x8fae5e].map((c) => std(new THREE.Color(c), 0.85, 0));
  const darkGreens = [0x2f5d3a, 0x3c6e47].map((c) => std(new THREE.Color(c), 0.9, 0));
  const blossoms = [0xf2b8c6, 0xf7e3ea].map((c) => std(new THREE.Color(c), 0.85, 0));
  const flowers = [0xf2c14e, 0xe98aa0, 0xf7f3e8, 0xd9573c, 0x9b7fd1].map((c) => std(new THREE.Color(c), 0.7, 0));
  const soil = std(palette.rule.clone().lerp(palette.ink3, 0.6), 1, 0);
  const clay = std(CLAY, 0.9, 0);
  const rockMat = std(palette.ink3.clone().lerp(palette.sheet, 0.45), 0.95, 0);
  const benchWood = toonMat({ map: woodTex(), roughness: 0.8 });
  const poleMat = toonMat({ map: metalTex(), roughness: 0.4, metalness: 0.5 });
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

  const lamps: [number, number][] = [];
  const addLamp = (x: number, z: number): void => {
    // Paths meet at the centre, so lamps of neighbouring paths can land on one spot: keep one.
    if (lamps.some(([lx, lz]) => Math.hypot(lx - x, lz - z) < 3)) return;
    lamps.push([x, z]);
    const l = new THREE.Group();
    l.position.set(x, 0, z);
    const pole = part(l, new THREE.CylinderGeometry(0.035, 0.045, 2.2, 8), poleMat, 0, 1.1, 0);
    part(l, new THREE.CylinderGeometry(0.06, 0.08, 0.1, 10), poleMat, 0, 2.25, 0);
    part(l, new THREE.SphereGeometry(0.16, 14, 10), bulb, 0, 2.4, 0);
    g.add(l);
    shadows(l);
    boxes.push(footprint(pole));
  };

  const addTree = (x: number, z: number): void => {
    const t = new THREE.Group();
    t.position.set(x, 0, z);
    const h = 0.9 + rand() * 0.5;
    const trunk = part(t, new THREE.CylinderGeometry(0.08, 0.12, h, 8), bark, 0, h / 2, 0);
    if (rand() < 0.35) {
      // A conifer: three stacked cones.
      const needles = pick(darkGreens, rand);
      for (let k = 0; k < 3; k++) part(t, new THREE.ConeGeometry(0.8 - k * 0.2, 0.95, 10), needles, 0, h + 0.25 + k * 0.42, 0);
    } else {
      // A broadleaf: a layered crown, now and then in blossom.
      const crown = pick(rand() < 0.2 ? blossoms : greens, rand);
      part(t, new THREE.SphereGeometry(0.72, 14, 10), crown, 0, h + 0.35, 0);
      part(t, new THREE.SphereGeometry(0.5, 12, 8), crown, 0.28, h + 0.8, 0.12);
      part(t, new THREE.SphereGeometry(0.42, 12, 8), crown, -0.22, h + 0.7, -0.18);
    }
    g.add(t);
    shadows(t);
    boxes.push(footprint(trunk));
  };

  const addBed = (x: number, z: number): void => {
    const b = new THREE.Group();
    b.position.set(x, 0, z);
    b.rotation.y = rand() * Math.PI;
    const border = part(b, new THREE.BoxGeometry(1.5, 0.2, 0.9), clay, 0, 0.1, 0);
    part(b, new THREE.BoxGeometry(1.36, 0.06, 0.76), soil, 0, 0.19, 0);
    for (let k = 0; k < 16; k++) {
      const fm = pick(flowers, rand);
      part(b, new THREE.SphereGeometry(0.06 + rand() * 0.03, 8, 6), fm, (rand() - 0.5) * 1.1, 0.3 + rand() * 0.06, (rand() - 0.5) * 0.6);
    }
    g.add(b);
    shadows(b);
    boxes.push(footprint(border));
  };

  const addRock = (x: number, z: number): void => {
    const rad = 0.3 + rand() * 0.35;
    const m = new THREE.Mesh(new THREE.DodecahedronGeometry(rad, 0), rockMat);
    m.position.set(x, rad * 0.4, z);
    m.rotation.set(rand() * 3, rand() * 3, rand() * 3);
    m.castShadow = true;
    m.receiveShadow = true;
    g.add(m);
    boxes.push(footprint(m));
  };

  const addBench = (x: number, z: number): void => {
    const b = bench(benchWood);
    b.position.set(x, 0, z);
    b.rotation.y = rand() * Math.PI * 2;
    g.add(b);
    shadows(b);
    boxes.push(footprint(b));
  };

  // One street lamp per path, just off its edge where it reaches the district, sides alternating.
  sites.forEach((s, i) => {
    const dist = Math.hypot(s.cx, s.cy);
    const d = dist - s.r - 2.6;
    if (dist < 1 || d < 7) return;
    const ux = s.cx / dist;
    const uz = s.cy / dist;
    const side = i % 2 === 0 ? 1 : -1;
    addLamp(ux * d - uz * side * 0.95, uz * d + ux * side * 0.95);
  });

  // Scatter: trees most often, then flower beds, rocks and benches, wherever the spot is clear.
  for (let i = 0; i < 320; i++) {
    const x = (rand() * 2 - 1) * reach;
    const z = (rand() * 2 - 1) * reach;
    const roll = rand();
    if (roll < 0.42) {
      if (clear(x, z, 0.7)) addTree(x, z);
    } else if (roll < 0.62) {
      if (clear(x, z, 0.85)) addBed(x, z);
    } else if (roll < 0.84) {
      if (clear(x, z, 0.4)) addRock(x, z);
    } else if (clear(x, z, 0.7)) {
      addBench(x, z);
    }
  }
  outline(g, 0.1);
  return { group: g, boxes };
}
