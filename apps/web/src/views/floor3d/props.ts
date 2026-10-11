// The props that furnish the Floor: each district's desks, plants, crates, lamp and
// role furniture; the central plaza with its benches and holographic globe; and the
// paths from the centre to each district. Everything is primitives with soft
// MeshStandardMaterials, tinted a little toward the district's role hue. Each group
// carries what the scene animates in userData: `screens` (monitor materials the scene
// lights while someone works) and `animate(t)` (LED blink, lamp flicker, globe spin).

import * as THREE from 'three';

/** The palette colours the props need, already resolved to three.js colours. */
export interface PropPalette {
  sheet: THREE.Color;
  rule: THREE.Color;
  ink3: THREE.Color;
  signal: THREE.Color;
  ok: THREE.Color;
  info: THREE.Color;
}

const TECH = new Set(['builder', 'qa', 'release', 'security']);
const LIBRARY = new Set(['researcher', 'librarian', 'planner']);
const BOARD = new Set(['architect', 'reviewer']);
const LAMP = new THREE.Color(0xffd79a);
const CLAY = new THREE.Color(0xc98b66);

function std(color: THREE.Color, roughness: number, metalness = 0.1): THREE.MeshStandardMaterial {
  return new THREE.MeshStandardMaterial({ color, roughness, metalness });
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
  const screen = new THREE.MeshStandardMaterial({
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

/** The central plaza: a round paved square, four benches and a slowly turning hologram above the dais. */
export function buildPlaza(palette: PropPalette): THREE.Group {
  const g = new THREE.Group();
  const paving = std(palette.rule.clone().lerp(palette.sheet, 0.4), 0.9, 0);
  const pave = part(g, new THREE.CylinderGeometry(4.3, 4.45, 0.06, 64), paving, 0, 0.03, 0);
  pave.receiveShadow = true;
  const inlay = part(g, new THREE.TorusGeometry(3.3, 0.025, 6, 72), std(palette.rule, 0.8, 0), 0, 0.065, 0);
  inlay.rotation.x = Math.PI / 2;

  const benchWood = std(palette.rule.clone().lerp(palette.ink3, 0.25), 0.8, 0);
  for (let i = 0; i < 4; i++) {
    const a = (i / 4) * Math.PI * 2;
    const x = Math.cos(a) * 3.6;
    const z = Math.sin(a) * 3.6;
    const bench = new THREE.Group();
    bench.position.set(x, 0, z);
    bench.rotation.y = Math.atan2(-x, -z); // the seat faces the centre, the back faces out
    part(bench, new THREE.BoxGeometry(1.3, 0.08, 0.42), benchWood, 0, 0.42, 0);
    part(bench, new THREE.BoxGeometry(0.08, 0.4, 0.36), benchWood, -0.55, 0.2, 0);
    part(bench, new THREE.BoxGeometry(0.08, 0.4, 0.36), benchWood, 0.55, 0.2, 0);
    part(bench, new THREE.BoxGeometry(1.3, 0.4, 0.07), benchWood, 0, 0.66, -0.2);
    g.add(bench);
  }

  // The globe: a transparent wireframe on a short pillar above the dais.
  const pillar = part(g, new THREE.CylinderGeometry(0.05, 0.05, 1.35, 8), std(palette.ink3, 0.45, 0.4), 0, 1.475, 0);
  pillar.castShadow = true;
  const globe = new THREE.Mesh(
    new THREE.SphereGeometry(0.75, 24, 16),
    new THREE.MeshStandardMaterial({
      color: palette.signal,
      emissive: palette.signal,
      emissiveIntensity: 0.7,
      wireframe: true,
      transparent: true,
      opacity: 0.5,
      depthWrite: false,
    }),
  );
  globe.position.set(0, 2.9, 0);
  g.add(globe);

  g.userData = {
    animate: (t: number) => {
      globe.rotation.y = t * 0.4;
      globe.position.y = 2.9 + Math.sin(t * 0.9) * 0.05;
    },
  };
  shadows(g);
  globe.castShadow = false;
  return g;
}

/** Flat paved strips from the origin out to each point (y = 0), for the paths between the centre and the districts. */
export function buildPaths(from: THREE.Vector3[], palette: PropPalette): THREE.Group {
  const g = new THREE.Group();
  const mat = std(palette.rule.clone().lerp(palette.sheet, 0.35), 0.95, 0);
  for (const to of from) {
    const len = Math.hypot(to.x, to.z);
    if (len < 0.5) continue;
    const strip = new THREE.Mesh(new THREE.BoxGeometry(len, 0.03, 0.9), mat);
    strip.position.set(to.x / 2, 0.015, to.z / 2);
    strip.rotation.y = -Math.atan2(to.z, to.x);
    strip.receiveShadow = true;
    g.add(strip);
  }
  return g;
}
