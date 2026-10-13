// The kit every district room is built from: a room style says how big the room is, its shape,
// its floor and walls, and what furniture stands where; the kit gives it cartoon materials, floor
// textures and ready-made pieces. Rooms are built in a local frame: the floor is y = 0, the back
// wall runs along z = -s, the open front (z = +s) faces the plaza, and the middle
// (|x| < 1.3 and |z| < 1.3) stays clear for the agents' consoles.

import * as THREE from 'three';
import { canvasTexture, type Rand } from '../textures';
import { toonMat } from '../toon';

export type FloorKind =
  | 'planks' | 'parquet' | 'checker' | 'hazard' | 'slate' | 'carpet' | 'blueprint'
  | 'stars' | 'marble' | 'gravel' | 'tiles' | 'lawn';

export type WallKind = 'panel' | 'brick' | 'hedge' | 'glass' | 'fence' | 'stone' | 'none';

export interface RoomStyle {
  /** The room's name on its wall sign, e.g. "Workshop". */
  name: string;
  /** Half-width (square) or radius (round) of the room's floor, world units: 3 (snug) to 5 (grand). */
  size: number;
  shape: 'square' | 'round';
  floor: FloorKind;
  wall: WallKind;
  /** Furnishes the room. Add pieces with the room's helpers; push per-frame animations to `room.anim`. */
  build: (room: Room) => void;
}

/** Cartoon materials, tinted for the room's hue where it says so. */
export interface Kit {
  wood: THREE.Material; darkWood: THREE.Material; metal: THREE.Material; dark: THREE.Material;
  white: THREE.Material; paper: THREE.Material; clay: THREE.Material; leaf: THREE.Material;
  leaf2: THREE.Material; crate: THREE.Material; gold: THREE.Material; red: THREE.Material;
  blue: THREE.Material; yellow: THREE.Material; green: THREE.Material; purple: THREE.Material;
  /** The room hue, strong and soft. */
  accent: THREE.Material; accentSoft: THREE.Material;
  /** The plaza's signal orange and status colours. */
  signal: THREE.Material; ok: THREE.Material; info: THREE.Material;
  /** Monitor faces: they light up while an agent of the room works. */
  screen: THREE.MeshToonMaterial;
  /** Unlit glowing bits (LEDs, bulbs); their colour may be animated. */
  glowOk: THREE.MeshBasicMaterial; glowInfo: THREE.MeshBasicMaterial; glowWarm: THREE.MeshBasicMaterial; glowRed: THREE.MeshBasicMaterial;
  /** Any other toon colour, cached. */
  mat(color: THREE.ColorRepresentation): THREE.Material;
}

export class Room {
  readonly anim: ((t: number) => void)[] = [];
  constructor(
    readonly g: THREE.Group,
    readonly s: number,
    readonly hue: number,
    readonly k: Kit,
    readonly rand: Rand,
  ) {}

  /** A piece of furniture: a group at (x, z) turned by `rotY`. Each item gets its own collision footprint. */
  item(x: number, z: number, rotY = 0): THREE.Group {
    const it = new THREE.Group();
    it.position.set(x, 0, z);
    it.rotation.y = rotY;
    this.g.add(it);
    return it;
  }

  /** A box standing on `y` (its bottom), centred on x/z. */
  box(parent: THREE.Object3D, w: number, h: number, d: number, mat: THREE.Material, x = 0, y = 0, z = 0): THREE.Mesh {
    return put(parent, new THREE.BoxGeometry(w, h, d), mat, x, y + h / 2, z);
  }

  /** A cylinder standing on `y`. */
  cyl(parent: THREE.Object3D, rTop: number, rBottom: number, h: number, mat: THREE.Material, x = 0, y = 0, z = 0, seg = 16): THREE.Mesh {
    return put(parent, new THREE.CylinderGeometry(rTop, rBottom, h, seg), mat, x, y + h / 2, z);
  }

  /** A sphere centred at (x, y, z). */
  ball(parent: THREE.Object3D, r: number, mat: THREE.Material, x = 0, y = 0, z = 0): THREE.Mesh {
    return put(parent, new THREE.SphereGeometry(r, 16, 12), mat, x, y, z);
  }

  /** A cone standing on `y`. */
  cone(parent: THREE.Object3D, r: number, h: number, mat: THREE.Material, x = 0, y = 0, z = 0, seg = 16): THREE.Mesh {
    return put(parent, new THREE.ConeGeometry(r, h, seg), mat, x, y + h / 2, z);
  }

  /** A flat rug or floor marking: walkable, never blocks a robot. */
  rug(w: number, d: number, mat: THREE.Material, x = 0, z = 0, round = false): THREE.Mesh {
    const geo = round ? new THREE.CircleGeometry(w / 2, 40) : new THREE.PlaneGeometry(w, d);
    const m = new THREE.Mesh(geo, mat);
    m.rotation.x = -Math.PI / 2;
    m.position.set(x, 0.012, z);
    m.receiveShadow = true;
    m.userData.walk = true;
    m.userData.outline = false;
    this.g.add(m);
    return m;
  }

  /** A desk with a monitor (it lights up while the room's agents work) and a chair. */
  desk(x: number, z: number, rotY = 0): THREE.Group {
    const it = this.item(x, z, rotY);
    const { wood, metal, screen } = this.k;
    this.box(it, 1.0, 0.06, 0.55, wood, 0, 0.5, 0);
    this.box(it, 0.08, 0.5, 0.5, wood, -0.42, 0, 0);
    this.box(it, 0.08, 0.5, 0.5, wood, 0.42, 0, 0);
    this.box(it, 0.3, 0.02, 0.18, metal, 0, 0.56, -0.12);
    this.box(it, 0.07, 0.18, 0.05, metal, 0, 0.58, -0.12);
    // The monitor lights up while agents work (its emissive is animated).
    this.box(it, 0.62, 0.38, 0.04, screen, 0, 0.74, -0.14).userData.dynamic = true;
    this.chair(0, 0.55, Math.PI, it);
    return it;
  }

  /** A chair, its seat facing +z (turn it with `rotY`). Pass `parent` to make it part of another item. */
  chair(x: number, z: number, rotY = 0, parent?: THREE.Object3D): THREE.Group {
    const it = parent ? new THREE.Group() : this.item(x, z, rotY);
    if (parent) {
      it.position.set(x, 0, z);
      it.rotation.y = rotY;
      parent.add(it);
    }
    this.box(it, 0.42, 0.06, 0.42, this.k.accent, 0, 0.36, 0);
    this.box(it, 0.42, 0.42, 0.06, this.k.accent, 0, 0.42, -0.18);
    for (const [cx, cz] of [[-0.17, -0.17], [0.17, -0.17], [-0.17, 0.17], [0.17, 0.17]] as const) {
      this.cyl(it, 0.025, 0.025, 0.36, this.k.metal, cx, 0, cz, 6);
    }
    return it;
  }

  /** A bookshelf against a wall, its books facing +z. */
  shelf(x: number, z: number, rotY = 0, w = 1.2, h = 1.6): THREE.Group {
    const it = this.item(x, z, rotY);
    const { darkWood, red, blue, yellow, green, purple } = this.k;
    const spines = [red, blue, yellow, green, purple];
    this.box(it, w, h, 0.4, darkWood, 0, 0, 0);
    const rows = Math.max(2, Math.round(h / 0.42));
    for (let r = 0; r < rows; r++) {
      const y = 0.1 + (r * (h - 0.2)) / rows;
      let bx = -w / 2 + 0.1;
      while (bx < w / 2 - 0.14) {
        const bw = 0.06 + this.rand() * 0.06;
        const bh = 0.2 + this.rand() * 0.1;
        this.box(it, bw, bh, 0.26, spines[Math.floor(this.rand() * spines.length)]!, bx + bw / 2, y, 0.04);
        bx += bw + 0.012;
      }
    }
    return it;
  }

  /** A potted plant. */
  plant(x: number, z: number, scale = 1): THREE.Group {
    const it = this.item(x, z);
    const { clay, leaf, leaf2 } = this.k;
    this.cyl(it, 0.22 * scale, 0.16 * scale, 0.3 * scale, clay, 0, 0, 0, 14);
    this.ball(it, 0.34 * scale, leaf, 0, 0.5 * scale, 0);
    this.cone(it, 0.22 * scale, 0.45 * scale, leaf2, 0, 0.62 * scale, 0, 10);
    return it;
  }

  /** A wooden crate, `size` on a side. */
  crate(x: number, z: number, size = 0.5, rotY = 0): THREE.Group {
    const it = this.item(x, z, rotY);
    this.box(it, size, size, size, this.k.crate, 0, 0, 0);
    this.box(it, size * 1.02, size * 0.12, size * 1.02, this.k.darkWood, 0, size * 0.44, 0);
    return it;
  }

  /** A floor lamp with a warm, gently flickering light. */
  lamp(x: number, z: number): THREE.Group {
    const it = this.item(x, z);
    this.cyl(it, 0.035, 0.05, 1.7, this.k.metal, 0, 0, 0, 8);
    this.cone(it, 0.24, 0.26, this.k.accentSoft, 0, 1.62, 0, 16);
    this.ball(it, 0.1, this.k.glowWarm, 0, 1.66, 0);
    const light = new THREE.PointLight(0xffd79a, 0.5, 5, 2);
    light.position.set(0, 1.55, 0);
    it.add(light);
    const phase = this.rand() * 10;
    this.anim.push((t) => (light.intensity = 0.5 * (1 + 0.04 * Math.sin(t * 7.1 + phase))));
    return it;
  }

  // ---- Builders for the room generator (rooms/gen.ts). Each item's origin is its floor centre
  // (y = 0); a builder never changes the old pieces above.

  /** A desk's table only: top surface at y = 0.56, footprint 1.0 x 0.55, no chair and no monitor. */
  deskTop(x: number, z: number, rotY = 0): THREE.Group {
    const it = this.item(x, z, rotY);
    const { wood } = this.k;
    this.box(it, 1.0, 0.06, 0.55, wood, 0, 0.5, 0);
    this.box(it, 0.08, 0.5, 0.5, wood, -0.42, 0, 0);
    this.box(it, 0.08, 0.5, 0.5, wood, 0.42, 0, 0);
    return it;
  }

  /**
   * A monitor standing on a desk. Its origin is the desk top: `y` is the host's top (0.56 on a deskTop, the
   * default 0 leaves it on the floor). Relative to that origin: stand base y 0 to 0.02, neck 0.02 to 0.20,
   * screen 0.18 to 0.56. The origin sits at desk-local z -0.12, so the screen is at local z -0.02.
   * Footprint 0.62 x 0.18, height 0.56.
   */
  monitor(x: number, z: number, rotY = 0, y = 0): THREE.Group {
    const it = this.item(x, z, rotY);
    it.position.y = y;
    const { metal, screen } = this.k;
    this.box(it, 0.3, 0.02, 0.18, metal, 0, 0, 0);
    this.box(it, 0.07, 0.18, 0.05, metal, 0, 0.02, 0);
    // The screen lights up while agents work (its emissive is animated).
    this.box(it, 0.62, 0.38, 0.04, screen, 0, 0.18, -0.02).userData.dynamic = true;
    return it;
  }

  /** A framed picture on a wall: 0.5 wide, 0.6 high, 0.1 deep. Its bottom is y = 0 of the group; set `position.y` to the mount height. */
  poster(x: number, z: number, rotY = 0): THREE.Group {
    const it = this.item(x, z, rotY);
    const { darkWood, paper, accent, signal, dark } = this.k;
    this.box(it, 0.5, 0.04, 0.1, darkWood, 0, 0, 0);
    this.box(it, 0.5, 0.04, 0.1, darkWood, 0, 0.56, 0);
    this.box(it, 0.04, 0.52, 0.1, darkWood, -0.23, 0.04, 0);
    this.box(it, 0.04, 0.52, 0.1, darkWood, 0.23, 0.04, 0);
    this.box(it, 0.42, 0.52, 0.02, paper, 0, 0.04, 0);
    this.box(it, 0.2, 0.24, 0.01, accent, -0.07, 0.2, 0.015);
    const sun = this.cyl(it, 0.07, 0.07, 0.01, signal, 0.1, 0, 0, 16);
    sun.rotation.x = Math.PI / 2;
    sun.position.set(0.1, 0.38, 0.015);
    this.box(it, 0.3, 0.03, 0.01, dark, 0, 0.14, 0.015);
    this.box(it, 0.2, 0.03, 0.01, dark, -0.05, 0.08, 0.015);
    return it;
  }

  /** A round wall clock: a face 0.36 across, 0.08 deep. Its bottom is y = 0 of the group; set `position.y` to the mount height. */
  clock(x: number, z: number, rotY = 0): THREE.Group {
    const it = this.item(x, z, rotY);
    const { wood, white, dark } = this.k;
    const rim = this.cyl(it, 0.18, 0.18, 0.08, wood, 0, 0, 0, 24);
    rim.rotation.x = Math.PI / 2;
    rim.position.set(0, 0.18, 0);
    const face = this.cyl(it, 0.14, 0.14, 0.012, white, 0, 0, 0, 24);
    face.rotation.x = Math.PI / 2;
    face.position.set(0, 0.18, 0.035);
    this.box(it, 0.012, 0.13, 0.003, dark, 0, 0.18, 0.0425);
    this.box(it, 0.09, 0.012, 0.003, dark, 0.045, 0.174, 0.0425);
    return it;
  }

  /** A two-seat sofa: 1.6 wide, 0.8 high, 0.7 deep; the seat faces +z. Footprint centred on the origin. */
  sofa(x: number, z: number, rotY = 0): THREE.Group {
    const it = this.item(x, z, rotY);
    const { accent, accentSoft } = this.k;
    this.box(it, 1.2, 0.22, 0.7, accent, 0, 0, 0);
    this.box(it, 1.6, 0.58, 0.18, accent, 0, 0.22, -0.26);
    for (const sx of [-0.71, 0.71]) this.box(it, 0.18, 0.44, 0.52, accent, sx, 0, 0.09);
    for (const sx of [-0.31, 0.31]) this.box(it, 0.6, 0.14, 0.52, accentSoft, sx, 0.22, 0.09);
    return it;
  }

  /** A waste bin: 0.3 x 0.6 x 0.3, a body with a lid-coloured band on top. */
  bin(x: number, z: number): THREE.Group {
    const it = this.item(x, z);
    this.box(it, 0.3, 0.54, 0.3, this.k.metal, 0, 0, 0);
    this.box(it, 0.3, 0.06, 0.3, this.k.accent, 0, 0.54, 0);
    return it;
  }

  /** A coat stand: a round foot 0.4 across, a pole and three hooks; 1.8 high. */
  coatStand(x: number, z: number): THREE.Group {
    const it = this.item(x, z);
    const { darkWood, dark } = this.k;
    this.cyl(it, 0.2, 0.2, 0.04, dark, 0, 0, 0, 20);
    this.cyl(it, 0.025, 0.025, 1.68, darkWood, 0, 0.04, 0, 8);
    this.ball(it, 0.06, dark, 0, 1.74, 0);
    for (let i = 0; i < 3; i++) {
      const a = (i * 2 * Math.PI) / 3;
      const hook = this.box(it, 0.18, 0.03, 0.03, darkWood, 0, 0, 0);
      hook.position.set(Math.sin(a) * 0.09, 1.5, Math.cos(a) * 0.09);
      hook.rotation.y = a - Math.PI / 2;
    }
    return it;
  }

  /** A wall safe: dark metal 0.7 x 0.9 x 0.6, a gold dial and a handle on the front (+z). */
  safe(x: number, z: number, rotY = 0): THREE.Group {
    const it = this.item(x, z, rotY);
    const { dark, gold, metal } = this.k;
    this.box(it, 0.7, 0.9, 0.58, dark, 0, 0, 0);
    const dial = this.cyl(it, 0.1, 0.1, 0.02, gold, 0, 0, 0, 20);
    dial.rotation.x = Math.PI / 2;
    dial.position.set(0, 0.5, 0.3);
    this.box(it, 0.04, 0.12, 0.03, metal, 0.22, 0.4, 0.29);
    return it;
  }

  /** A 0.5 cube crate (the room's crate, the small size). */
  smallCrate(x: number, z: number, rotY = 0): THREE.Group {
    return this.crate(x, z, 0.5, rotY);
  }

  /** An armchair: 0.8 x 0.8, 0.85 high; a sage fabric back, two arms and a seat cushion on a base, with four short feet. Its seat faces +z. */
  armchair(x: number, z: number, rotY = 0): THREE.Group {
    const it = this.item(x, z, rotY);
    const fabric = this.k.mat(0x6f9e8c);
    const cushion = this.k.mat(0x8fbba9);
    for (const [fx, fz] of [[-0.34, -0.34], [0.34, -0.34], [-0.34, 0.34], [0.34, 0.34]] as const) {
      this.cyl(it, 0.03, 0.03, 0.06, this.k.dark, fx, 0, fz, 6);
    }
    this.box(it, 0.8, 0.2, 0.8, fabric, 0, 0.06, 0);
    this.box(it, 0.8, 0.59, 0.1, fabric, 0, 0.26, -0.35);
    for (const sx of [-0.34, 0.34]) this.box(it, 0.12, 0.3, 0.8, fabric, sx, 0.26, 0);
    this.box(it, 0.56, 0.12, 0.6, cushion, 0, 0.26, 0);
    return it;
  }

  /** A coffee table: 0.8 x 0.5, 0.42 high; a wooden top slab on four short legs. */
  coffeeTable(x: number, z: number, rotY = 0): THREE.Group {
    const it = this.item(x, z, rotY);
    const { wood, darkWood } = this.k;
    this.box(it, 0.8, 0.05, 0.5, wood, 0, 0.37, 0);
    for (const [lx, lz] of [[-0.35, -0.2], [0.35, -0.2], [-0.35, 0.2], [0.35, 0.2]] as const) {
      this.box(it, 0.05, 0.37, 0.05, darkWood, lx, 0, lz);
    }
    return it;
  }

  /** A stool: 0.36 across, 0.5 high; a round seat on one pedestal and a round foot. No back. */
  stool(x: number, z: number, rotY = 0): THREE.Group {
    const it = this.item(x, z, rotY);
    this.cyl(it, 0.16, 0.16, 0.03, this.k.dark, 0, 0, 0, 16);
    this.cyl(it, 0.035, 0.035, 0.41, this.k.metal, 0, 0.03, 0, 8);
    this.cyl(it, 0.18, 0.18, 0.06, this.k.accent, 0, 0.44, 0, 20);
    return it;
  }

  /** A low two-shelf unit: 0.8 wide, 1.0 high, 0.3 deep; its back at -z (against the wall), its front at +z. Books stand on the two lower shelves. */
  smallShelf(x: number, z: number, rotY = 0): THREE.Group {
    const it = this.item(x, z, rotY);
    const { darkWood } = this.k;
    const spines = [this.k.red, this.k.blue, this.k.yellow, this.k.green, this.k.purple];
    this.box(it, 0.8, 1.0, 0.02, darkWood, 0, 0, -0.14);
    for (const sx of [-0.385, 0.385]) this.box(it, 0.03, 1.0, 0.3, darkWood, sx, 0, 0);
    for (const y of [0, 0.485, 0.97]) this.box(it, 0.8, 0.03, 0.3, darkWood, 0, y, 0);
    for (const y of [0.03, 0.515]) {
      for (let i = 0; i < 5; i++) {
        const bh = 0.2 + this.rand() * 0.1;
        this.box(it, 0.06, bh, 0.2, spines[Math.floor(this.rand() * spines.length)]!, (i - 2) * 0.11, y, 0.02);
      }
    }
    return it;
  }

  /** A small round table: 0.5 across, 0.55 high; a round top on one pedestal and a round foot. */
  sideTable(x: number, z: number, rotY = 0): THREE.Group {
    const it = this.item(x, z, rotY);
    const { wood, darkWood } = this.k;
    this.cyl(it, 0.18, 0.18, 0.03, darkWood, 0, 0, 0, 16);
    this.cyl(it, 0.04, 0.04, 0.48, darkWood, 0, 0.03, 0, 8);
    this.cyl(it, 0.25, 0.25, 0.04, wood, 0, 0.51, 0, 24);
    return it;
  }

  /** Stands in for a station prop that has no JSON yet: a plain box of its footprint and height, in a neutral colour. */
  propPlaceholder(x: number, z: number, rotY: number, w: number, h: number, d: number, label: string): THREE.Group {
    const it = this.item(x, z, rotY);
    it.name = `prop:${label}`;
    it.userData.placeholder = label;
    this.box(it, w, h, d, this.k.mat(0xb9bfc9), 0, 0, 0);
    return it;
  }
}

function put(parent: THREE.Object3D, geo: THREE.BufferGeometry, mat: THREE.Material, x: number, y: number, z: number): THREE.Mesh {
  const m = new THREE.Mesh(geo, mat);
  m.position.set(x, y, z);
  m.castShadow = true;
  m.receiveShadow = true;
  parent.add(m);
  return m;
}

// ---- Materials ------------------------------------------------------------------------------

export interface KitPalette { sheet: THREE.Color; rule: THREE.Color; ink3: THREE.Color; signal: THREE.Color; ok: THREE.Color; info: THREE.Color }

const cache = new Map<string, THREE.Material>();
function cached(key: string, make: () => THREE.Material): THREE.Material {
  let m = cache.get(key);
  if (!m) cache.set(key, (m = make()));
  return m;
}

/** The kit for a room of hue `hue`. Monitor and glow materials are the room's own (they animate). */
export function makeKit(hue: number, p: KitPalette): Kit {
  const hsl = (s: number, l: number) => new THREE.Color().setHSL(hue / 360, s, l, THREE.SRGBColorSpace);
  const mat = (c: THREE.ColorRepresentation): THREE.Material => {
    const col = new THREE.Color(c);
    return cached(`toon:${col.getHexString()}`, () => toonMat({ color: col }));
  };
  return {
    wood: mat(0xd99a5b), darkWood: mat(0x8a5634), metal: mat(0x8f9bab), dark: mat(0x46505e),
    white: mat(0xfbf8f1), paper: mat(p.sheet), clay: mat(0xd27f55), leaf: mat(0x5fae4e), leaf2: mat(0x3f8a43),
    crate: mat(0xe0b36c), gold: mat(0xf4c542), red: mat(0xe5533d), blue: mat(0x4c86d9), yellow: mat(0xf7d154),
    green: mat(0x56b870), purple: mat(0x9a72d6),
    accent: mat(hsl(0.55, 0.55)), accentSoft: mat(hsl(0.6, 0.78)),
    signal: mat(p.signal), ok: mat(p.ok), info: mat(p.info),
    screen: live(toonMat({ color: 0x3a4658, emissive: p.info, emissiveIntensity: 0.05 })),
    glowOk: live(new THREE.MeshBasicMaterial({ color: p.ok, toneMapped: false })),
    glowInfo: live(new THREE.MeshBasicMaterial({ color: p.info, toneMapped: false })),
    glowWarm: new THREE.MeshBasicMaterial({ color: 0xffd79a, toneMapped: false }),
    glowRed: new THREE.MeshBasicMaterial({ color: 0xff4a3a, toneMapped: false }),
    mat,
  };
}

/** Marks a material whose properties change at runtime, so static batching never shares it. */
function live<M extends THREE.Material>(m: M): M {
  m.userData.live = true;
  return m;
}

// ---- Floors and walls -----------------------------------------------------------------------

function rrect(g: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number): void {
  g.beginPath();
  g.moveTo(x + r, y);
  g.arcTo(x + w, y, x + w, y + h, r);
  g.arcTo(x + w, y + h, x, y + h, r);
  g.arcTo(x, y + h, x, y, r);
  g.arcTo(x, y, x + w, y, r);
  g.closePath();
}

/** A room's floor texture; one texture tile covers two world units. */
export function floorTexture(kind: FloorKind, hue: number, size: number): THREE.Texture {
  const c = (s: number, l: number) => new THREE.Color().setHSL(hue / 360, s, l, THREE.SRGBColorSpace).getStyle();
  const tex = canvasTexture(`floor:${kind}:${Math.round(hue)}`, 256, 256, 1, (g, w, h, r) => {
    switch (kind) {
      case 'planks':
      case 'parquet': {
        const n = kind === 'planks' ? 5 : 8;
        const ph = h / n;
        for (let i = 0; i < n; i++) {
          for (let j = 0; j < (kind === 'planks' ? 2 : 4); j++) {
            const pw = w / (kind === 'planks' ? 2 : 4);
            const off = kind === 'planks' ? (i % 2) * pw * 0.5 : 0;
            g.fillStyle = `hsl(${28 + r() * 8}, ${50 + r() * 12}%, ${kind === 'planks' ? 56 + r() * 8 : 44 + r() * 10}%)`;
            g.fillRect(j * pw + off - (off ? pw : 0), i * ph, pw, ph);
            g.fillRect(j * pw + off, i * ph, pw, ph);
            g.fillStyle = 'rgba(70,38,16,0.7)';
            g.fillRect(j * pw + off, i * ph, 2, ph);
          }
          g.fillStyle = 'rgba(70,38,16,0.75)';
          g.fillRect(0, i * ph, w, 2);
          g.fillStyle = 'rgba(255,255,255,0.16)';
          g.fillRect(0, i * ph + 4, w, 4);
        }
        break;
      }
      case 'checker': {
        const n = 4;
        const t = w / n;
        for (let i = 0; i < n; i++) for (let j = 0; j < n; j++) {
          g.fillStyle = (i + j) % 2 ? '#f6f3ec' : c(0.45, 0.62);
          g.fillRect(i * t, j * t, t, t);
        }
        break;
      }
      case 'hazard': {
        g.fillStyle = '#9aa3ae';
        g.fillRect(0, 0, w, h);
        for (let i = 0; i < 6; i++) for (let j = 0; j < 6; j++) {
          g.fillStyle = 'rgba(60,66,76,0.35)';
          g.beginPath();
          g.arc(i * 44 + 20, j * 44 + 20, 4, 0, Math.PI * 2);
          g.fill();
        }
        g.fillStyle = '#f2c230';
        g.fillRect(0, 0, w, 26);
        g.fillStyle = '#2b2420';
        for (let x = -26; x < w; x += 26) {
          g.beginPath();
          g.moveTo(x, 26);
          g.lineTo(x + 13, 0);
          g.lineTo(x + 26, 0);
          g.lineTo(x + 13, 26);
          g.fill();
        }
        break;
      }
      case 'slate':
      case 'marble': {
        g.fillStyle = kind === 'slate' ? '#4d5866' : '#f3efe6';
        g.fillRect(0, 0, w, h);
        const n = 2;
        const t = w / n;
        for (let i = 0; i < n; i++) for (let j = 0; j < n; j++) {
          rrect(g, i * t + 3, j * t + 3, t - 6, t - 6, 4);
          g.fillStyle = kind === 'slate' ? `hsl(214, 14%, ${30 + r() * 8}%)` : `hsl(40, 30%, ${92 + r() * 4}%)`;
          g.fill();
          g.strokeStyle = kind === 'slate' ? 'rgba(20,24,30,0.6)' : 'rgba(150,130,100,0.5)';
          g.lineWidth = 2;
          g.stroke();
          for (let v = 0; v < 3; v++) {
            g.strokeStyle = kind === 'slate' ? 'rgba(255,255,255,0.08)' : c(0.25, 0.75);
            g.lineWidth = 1.5;
            g.beginPath();
            g.moveTo(i * t + r() * t, j * t + r() * t);
            g.quadraticCurveTo(i * t + r() * t, j * t + r() * t, i * t + r() * t, j * t + r() * t);
            g.stroke();
          }
        }
        break;
      }
      case 'carpet': {
        g.fillStyle = c(0.45, 0.5);
        g.fillRect(0, 0, w, h);
        g.strokeStyle = c(0.5, 0.62);
        g.lineWidth = 6;
        for (let i = 0; i < 4; i++) for (let j = 0; j < 4; j++) {
          g.beginPath();
          g.arc(i * 64 + 32, j * 64 + 32, 16, 0, Math.PI * 2);
          g.stroke();
        }
        break;
      }
      case 'blueprint': {
        g.fillStyle = '#2f6db5';
        g.fillRect(0, 0, w, h);
        g.strokeStyle = 'rgba(255,255,255,0.25)';
        g.lineWidth = 1;
        for (let x = 0; x <= w; x += 16) {
          g.beginPath(); g.moveTo(x, 0); g.lineTo(x, h); g.stroke();
          g.beginPath(); g.moveTo(0, x); g.lineTo(w, x); g.stroke();
        }
        g.strokeStyle = 'rgba(255,255,255,0.75)';
        g.lineWidth = 3;
        g.strokeRect(40, 40, 110, 70);
        g.beginPath(); g.arc(190, 180, 36, 0, Math.PI * 2); g.stroke();
        g.beginPath(); g.moveTo(40, 200); g.lineTo(130, 200); g.lineTo(130, 150); g.stroke();
        break;
      }
      case 'stars': {
        g.fillStyle = '#26305e';
        g.fillRect(0, 0, w, h);
        for (let i = 0; i < 70; i++) {
          g.fillStyle = r() < 0.2 ? '#ffe9a8' : 'rgba(255,255,255,0.85)';
          const x = r() * w;
          const y = r() * h;
          const s = 1 + r() * 2.5;
          g.beginPath();
          g.moveTo(x, y - s * 2); g.lineTo(x + s * 0.6, y); g.lineTo(x, y + s * 2); g.lineTo(x - s * 0.6, y); g.closePath();
          g.fill();
        }
        break;
      }
      case 'gravel':
      case 'lawn': {
        g.fillStyle = kind === 'gravel' ? '#c9b99a' : '#7fc05a';
        g.fillRect(0, 0, w, h);
        for (let i = 0; i < 260; i++) {
          g.fillStyle = kind === 'gravel' ? `hsla(35, 18%, ${55 + r() * 30}%, 0.9)` : `hsla(${90 + r() * 20}, 45%, ${40 + r() * 20}%, 0.6)`;
          g.beginPath();
          g.ellipse(r() * w, r() * h, 2 + r() * 4, 1.5 + r() * 3, r() * 3, 0, Math.PI * 2);
          g.fill();
        }
        break;
      }
      case 'tiles':
      default: {
        g.fillStyle = c(0.28, 0.66);
        g.fillRect(0, 0, w, h);
        const n = 4;
        const t = w / n;
        for (let i = 0; i < n; i++) for (let j = 0; j < n; j++) {
          rrect(g, i * t + 3, j * t + 3, t - 6, t - 6, 6);
          g.fillStyle = c(0.5, (i + j) % 2 ? 0.84 : 0.8);
          g.fill();
        }
      }
    }
  });
  const t = tex.clone();
  t.repeat.set(size, size);
  t.needsUpdate = true;
  return t;
}

/** The wall material and height for a wall kind. */
export function wallLook(kind: WallKind, hue: number): { mat: THREE.Material; h: number; thick: number } | null {
  const hsl = (s: number, l: number) => new THREE.Color().setHSL(hue / 360, s, l, THREE.SRGBColorSpace);
  switch (kind) {
    case 'panel': return { mat: cached(`wall:panel:${hue}`, () => toonMat({ color: hsl(0.4, 0.8) })), h: 1.1, thick: 0.14 };
    case 'brick': return { mat: cached('wall:brick', () => toonMat({ color: 0xc8684a })), h: 1.0, thick: 0.2 };
    case 'stone': return { mat: cached('wall:stone', () => toonMat({ color: 0xd9d1c2 })), h: 0.9, thick: 0.22 };
    case 'hedge': return { mat: cached('wall:hedge', () => toonMat({ color: 0x4f9a45 })), h: 0.8, thick: 0.35 };
    case 'glass': return { mat: cached('wall:glass', () => toonMat({ color: 0xbfe6f2, transparent: true, opacity: 0.55 })), h: 1.2, thick: 0.06 };
    case 'fence': return { mat: cached('wall:fence', () => toonMat({ color: 0xf2e6cf })), h: 0.6, thick: 0.08 };
    case 'none': return null;
  }
}
