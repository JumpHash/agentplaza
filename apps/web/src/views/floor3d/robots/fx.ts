// Robot effects: small sprites, cone meshes and flat discs from one pool shared by every robot in the scene
// (LIVE_MAX at once). A robot spawns them at a bone; they move, fade, and go back to the pool. A trail's dots
// and a glow's floor spot lie on the floor, so they are re-parented to the top of the scene (the robot walks
// on): at the base bone's world height plus a hair, with a polygon offset against z-fighting.
// Nothing allocates per frame: the pieces and the items are reused.

import * as THREE from 'three';
import type { FxKind, Vec3 } from './types';

/** The live effects of the whole page at once: beyond this, new ones are dropped (the budget). */
const LIVE_MAX = 60;
/** A glow's colour when the caller gives none: the lamp's warm light. */
const WARM = 0xffc46b;
/** Metres above the floor for the floor pieces (a trail's dot, a glow's spot). */
const FLOOR_LIFT = 0.01;

/** The pieces the pool holds: the kinds, and a glow's floor spot. */
type Pooled = FxKind | 'spot';

interface Look {
  /** Seconds alive. */
  life: number;
  /** Sprite side in metres (a cone is scaled by this; a floor disc is its diameter). */
  size: number;
  /** The scale over the life, as multiples of `size`. */
  grow: readonly [number, number];
  /** Metres per second upward (negative falls). */
  rise: number;
  /** Random sideways speed, metres per second. */
  spread: number;
  /** Sideways wobble, metres, at 5 rad/s. */
  sway: number;
  /** Peak opacity. */
  peak: number;
  additive: boolean;
  /** Random tints (hex) per piece, or null for the texture's own colour. */
  tints: readonly number[] | null;
  /** Spin rate of a sprite, rad/s (confetti). */
  spin: number;
  /** The scale eases out (a pop: fast at first, then it settles). */
  pop?: boolean;
}

const LOOK: Record<FxKind, Look> = {
  sparks: { life: 0.5, size: 0.14, grow: [1, 1], rise: 0.2, spread: 0.5, sway: 0, peak: 1, additive: true, tints: null, spin: 0 },
  zzz: { life: 1.6, size: 0.24, grow: [0.7, 1.2], rise: 0.32, spread: 0.12, sway: 0, peak: 1, additive: false, tints: null, spin: 0 },
  dots: { life: 1.2, size: 0.26, grow: [1, 1], rise: 0, spread: 0.12, sway: 0, peak: 1, additive: false, tints: null, spin: 0 },
  hearts: { life: 1.5, size: 0.2, grow: [1, 1], rise: 0.45, spread: 0.12, sway: 0, peak: 1, additive: false, tints: null, spin: 0 },
  keys: { life: 0.4, size: 0.14, grow: [0.4, 1.1], rise: 0.25, spread: 0.12, sway: 0, peak: 1, additive: false, tints: null, spin: 0, pop: true },
  steam: { life: 1.2, size: 0.26, grow: [0.6, 1.9], rise: 0.3, spread: 0.08, sway: 0.05, peak: 0.85, additive: false, tints: null, spin: 0 },
  bubbles: { life: 1.4, size: 0.13, grow: [1, 1.25], rise: 0.3, spread: 0.05, sway: 0.04, peak: 1, additive: false, tints: null, spin: 0 },
  notes: { life: 1.4, size: 0.2, grow: [1, 1.1], rise: 0.4, spread: 0.1, sway: 0.12, peak: 1, additive: false, tints: [0x7a5cff, 0xff7ab6, 0x3fb6a8], spin: 0 },
  confetti: { life: 1.8, size: 0.13, grow: [1, 1], rise: -0.5, spread: 0.8, sway: 0.03, peak: 1, additive: false, tints: [0xff6b8a, 0xffc23d, 0x5bd1b5, 0x8fb8ff, 0xb08bff], spin: 6 },
  ink: { life: 1, size: 0.42, grow: [0.6, 1.3], rise: 0.04, spread: 0, sway: 0, peak: 0.8, additive: false, tints: null, spin: 0 },
  trail: { life: 6, size: 0.13, grow: [1, 1], rise: 0, spread: 0, sway: 0, peak: 1, additive: false, tints: [0x2ee6a6], spin: 0 },
  // A cone, scaled by its own size (1). The caller gives its colour (warm by default).
  glow: { life: 1, size: 1, grow: [1, 1], rise: 0, spread: 0, sway: 0, peak: 0.6, additive: true, tints: null, spin: 0 },
};

/** A glow's floor spot: a flat disc, 0.8 m across, that lives as long as its cone. */
const SPOT: Look = { life: 1, size: 0.8, grow: [0.8, 1.1], rise: 0, spread: 0, sway: 0, peak: 0.45, additive: true, tints: null, spin: 0 };

function lookOf(kind: Pooled): Look {
  return kind === 'spot' ? SPOT : LOOK[kind];
}

/** The opacity over the life (k in 0..1), as a fraction of the peak. */
const FADE: Record<Pooled, (k: number) => number> = {
  sparks: (k) => 1 - k * k,
  zzz: (k) => 1 - k * k,
  dots: (k) => 1 - k * k,
  hearts: (k) => 1 - k * k,
  keys: (k) => 1 - k * k,
  steam: (k) => Math.min(1, k * 6) * (1 - k),
  bubbles: (k) => 1 - k * k,
  notes: (k) => 1 - k * k,
  confetti: (k) => (k < 0.7 ? 1 : (1 - k) / 0.3),
  ink: (k) => 1 - k,
  trail: (k) => 1 - k,
  glow: (k) => Math.sin(Math.PI * k),
  spot: (k) => Math.sin(Math.PI * k),
};

const textures = new Map<Pooled, THREE.Texture>();
let cone: THREE.BufferGeometry | null = null;
let flatDisc: THREE.BufferGeometry | null = null;
const free = new Map<Pooled, Piece[]>();
const spareItems: Item[] = [];
const scratch = new THREE.Vector3();
const world = new THREE.Vector3();
const probe = new THREE.Vector3();
let live = 0;

/** One small canvas picture per kind, drawn once. Without a DOM (tests), an empty texture. */
function texture(kind: Pooled): THREE.Texture {
  const hit = textures.get(kind);
  if (hit) return hit;
  if (typeof document === 'undefined') return new THREE.Texture();
  const cv = document.createElement('canvas');
  cv.width = cv.height = 64;
  const g = cv.getContext('2d');
  if (g) paint(g, kind);
  const tex = new THREE.CanvasTexture(cv);
  tex.colorSpace = THREE.SRGBColorSpace;
  textures.set(kind, tex);
  return tex;
}

function disc(g: CanvasRenderingContext2D, x: number, y: number, r: number): void {
  g.beginPath();
  g.arc(x, y, r, 0, Math.PI * 2);
  g.fill();
}

/** A rounded rectangle path (x, y, w, h, corner radius r); the caller fills it. */
function rounded(g: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number): void {
  g.beginPath();
  g.moveTo(x + r, y);
  g.arcTo(x + w, y, x + w, y + h, r);
  g.arcTo(x + w, y + h, x, y + h, r);
  g.arcTo(x, y + h, x, y, r);
  g.arcTo(x, y, x + w, y, r);
  g.closePath();
}

function glyph(g: CanvasRenderingContext2D, ch: string, font: string, color: string): void {
  g.font = font;
  g.fillStyle = color;
  g.fillText(ch, 32, 34);
}

function paint(g: CanvasRenderingContext2D, kind: Pooled): void {
  g.textBaseline = 'middle';
  g.textAlign = 'center';
  if (kind === 'sparks') {
    const grd = g.createRadialGradient(32, 32, 2, 32, 32, 30);
    grd.addColorStop(0, '#fff6c2');
    grd.addColorStop(0.4, '#ffc23d');
    grd.addColorStop(1, 'rgba(255,194,61,0)');
    g.fillStyle = grd;
    g.fillRect(0, 0, 64, 64);
  } else if (kind === 'steam') {
    // Three soft puffs in light grey-blue, each with a slightly darker rim: a cloud of overlapping discs.
    for (const [x, y, r] of [[24, 38, 14], [41, 32, 16], [33, 44, 12]] as const) {
      const grd = g.createRadialGradient(x, y, 0, x, y, r);
      grd.addColorStop(0, 'rgba(204,217,231,0.95)');
      grd.addColorStop(0.7, 'rgba(180,197,216,0.9)');
      grd.addColorStop(0.9, 'rgba(130,152,178,0.8)');
      grd.addColorStop(1, 'rgba(130,152,178,0)');
      g.fillStyle = grd;
      disc(g, x, y, r);
    }
  } else if (kind === 'keys') {
    // A keycap: a cyan top inside a dark ink outline, with a small glint.
    rounded(g, 8, 10, 48, 44, 10);
    g.fillStyle = '#17141f';
    g.fill();
    rounded(g, 12, 14, 40, 36, 7);
    g.fillStyle = '#33c2ea';
    g.fill();
    rounded(g, 17, 19, 13, 6, 3);
    g.fillStyle = 'rgba(255,255,255,0.9)';
    g.fill();
  } else if (kind === 'dots') {
    g.fillStyle = '#5b6475';
    for (const x of [14, 32, 50]) disc(g, x, 32, 6);
  } else if (kind === 'bubbles') {
    g.fillStyle = 'rgba(190,230,255,0.25)';
    disc(g, 32, 32, 22);
    g.lineWidth = 5;
    g.strokeStyle = '#9fd8ff';
    g.beginPath();
    g.arc(32, 32, 22, 0, Math.PI * 2);
    g.stroke();
    g.fillStyle = '#ffffff';
    disc(g, 24, 24, 5);
  } else if (kind === 'ink') {
    // A navy-purple cloud: six overlapping spheres, lighter at the top-left, darker at the rim.
    for (const [x, y, r] of [[32, 32, 13], [22, 27, 11], [42, 27, 11], [21, 40, 10], [43, 40, 10], [32, 44, 11]] as const) {
      const grd = g.createRadialGradient(x - r * 0.35, y - r * 0.35, r * 0.1, x, y, r);
      grd.addColorStop(0, '#5b4d9c');
      grd.addColorStop(0.6, '#2c2462');
      grd.addColorStop(1, '#17123a');
      g.fillStyle = grd;
      disc(g, x, y, r);
    }
  } else if (kind === 'confetti') {
    g.fillStyle = '#ffffff';
    g.fillRect(10, 22, 44, 20);
  } else if (kind === 'glow') {
    // A cone's texture: clear at the apex (the canvas top, uv v = 1), bright at the base (the canvas bottom).
    const grd = g.createLinearGradient(0, 0, 0, 64);
    grd.addColorStop(0, 'rgba(255,255,255,0)');
    grd.addColorStop(0.5, 'rgba(255,255,255,0.3)');
    grd.addColorStop(1, 'rgba(255,255,255,0.9)');
    g.fillStyle = grd;
    g.fillRect(0, 0, 64, 64);
  } else if (kind === 'spot') {
    // A floor spot: bright at its centre, clear at its rim.
    const grd = g.createRadialGradient(32, 32, 0, 32, 32, 32);
    grd.addColorStop(0, 'rgba(255,255,255,1)');
    grd.addColorStop(0.45, 'rgba(255,255,255,0.55)');
    grd.addColorStop(1, 'rgba(255,255,255,0)');
    g.fillStyle = grd;
    g.fillRect(0, 0, 64, 64);
  } else if (kind === 'trail') {
    // A trail dot: a small bright core, soft at its rim (tinted mint by its tints).
    const grd = g.createRadialGradient(32, 32, 0, 32, 32, 32);
    grd.addColorStop(0, '#ffffff');
    grd.addColorStop(0.6, 'rgba(255,255,255,0.95)');
    grd.addColorStop(1, 'rgba(255,255,255,0)');
    g.fillStyle = grd;
    g.fillRect(0, 0, 64, 64);
  } else if (kind === 'notes') {
    glyph(g, '♫', 'bold 44px sans-serif', '#ffffff');
  } else if (kind === 'hearts') {
    glyph(g, '♥', 'bold 46px sans-serif', '#ff6b8a');
  } else {
    glyph(g, 'z', 'bold 44px sans-serif', '#8fb8ff');
  }
}

/** The glow's cone: apex at the origin, opening downward, so it hangs from its bone (shared). */
function coneGeometry(): THREE.BufferGeometry {
  if (!cone) {
    cone = new THREE.ConeGeometry(0.22, 1.1, 14, 1, true);
    cone.translate(0, -0.55, 0);
  }
  return cone;
}

/** A flat disc lying on the floor (centred on its origin, a metre across), shared. */
function discGeometry(): THREE.BufferGeometry {
  if (!flatDisc) {
    flatDisc = new THREE.PlaneGeometry(1, 1);
    flatDisc.rotateX(-Math.PI / 2);
  }
  return flatDisc;
}

/** A sprite, a cone or a floor disc with its own material (pooled, so its opacity and tint are its own). */
interface Piece {
  kind: Pooled;
  obj: THREE.Sprite | THREE.Mesh;
  mat: THREE.SpriteMaterial | THREE.MeshBasicMaterial;
  /** A disc on the floor (a trail dot, a glow's spot): scaled in x and z only, placed once. */
  flat: boolean;
}

function make(kind: Pooled): Piece {
  const look = lookOf(kind);
  const blending = look.additive ? THREE.AdditiveBlending : THREE.NormalBlending;
  if (kind === 'glow' || kind === 'spot' || kind === 'trail') {
    const flat = kind !== 'glow';
    const mat = new THREE.MeshBasicMaterial({ map: texture(kind), transparent: true, opacity: 0, depthWrite: false, blending, side: THREE.DoubleSide });
    if (flat) {
      // Pulled a little toward the viewer, so the floor never shows through (z-fighting).
      mat.polygonOffset = true;
      mat.polygonOffsetFactor = -4;
      mat.polygonOffsetUnits = -4;
    }
    const mesh = new THREE.Mesh(flat ? discGeometry() : coneGeometry(), mat);
    mesh.castShadow = false;
    mesh.receiveShadow = false;
    return { kind, obj: mesh, mat, flat };
  }
  const mat = new THREE.SpriteMaterial({ map: texture(kind), transparent: true, depthWrite: false, opacity: 0, blending });
  return { kind, obj: new THREE.Sprite(mat), mat, flat: false };
}

function take(kind: Pooled): Piece {
  return free.get(kind)?.pop() ?? make(kind);
}

/**
 * Makes one piece of every kind and parks it in an invisible holder in the scene, so each kind's program (a sprite's, a
 * cone's, a disc's) compiles in warm (the plaza's compile pass shows hidden objects), not at the first effect of a flight.
 * The pieces are pooled as they are: the holder is only where they wait.
 */
export function prewarmFx(scene: THREE.Scene): void {
  const holder = new THREE.Group();
  holder.name = 'fx-warm';
  holder.visible = false;
  const kinds: Pooled[] = [...(Object.keys(LOOK) as FxKind[]), 'spot'];
  for (const kind of kinds) {
    const piece = make(kind);
    holder.add(piece.obj);
    let pool = free.get(kind);
    if (!pool) free.set(kind, (pool = []));
    pool.push(piece);
  }
  scene.add(holder);
}

function giveBack(piece: Piece): void {
  piece.obj.removeFromParent();
  let pool = free.get(piece.kind);
  if (!pool) free.set(piece.kind, (pool = []));
  pool.push(piece);
}

/** The top of the tree a robot stands in (the scene): a trail and a spot live there, since the robot walks on. */
function top(o: THREE.Object3D): THREE.Object3D {
  let t = o;
  while (t.parent) t = t.parent;
  return t;
}

/** The world height of the robot's base bone (the floor it stands on), walking up from `o`; the top when there is none. */
function floorY(o: THREE.Object3D): number {
  let b: THREE.Object3D | null = o;
  while (b && b.name !== 'base') b = b.parent;
  const base = b ?? top(o);
  base.updateWorldMatrix(true, false);
  return base.getWorldPosition(probe).y;
}

interface Item {
  kind: FxKind;
  piece: Piece;
  /** A glow's floor spot (else null). */
  spot: Piece | null;
  age: number;
  life: number;
  x0: number;
  y0: number;
  z0: number;
  vx: number;
  vy: number;
  sway: number;
  phase: number;
  spin: number;
  s0: number;
  s1: number;
  size: number;
}

/** A robot's live effects. */
export class Fx {
  private items: Item[] = [];

  /**
   * Spawns one effect under `parent` at `at`. A trail is left on the floor under the bone; a glow hangs from
   * the bone in `color` (warm by default) and casts a spot on the floor. False when the page is full.
   */
  spawn(kind: FxKind, parent: THREE.Object3D, at: Vec3, color: number = WARM): boolean {
    const look = LOOK[kind];
    if (!look || live >= LIVE_MAX) return false;
    const piece = take(kind);
    const rnd = Math.random;
    parent.updateWorldMatrix(true, false);
    let host: THREE.Object3D = parent;
    scratch.set(at[0], at[1], at[2]);
    if (kind === 'trail') {
      parent.localToWorld(scratch);
      scratch.y = floorY(parent) + FLOOR_LIFT;
      host = top(parent);
      host.worldToLocal(scratch);
    }
    host.add(piece.obj);
    piece.obj.position.copy(scratch);
    let spot: Piece | null = null;
    if (kind === 'glow') {
      // The spot lies on the floor under the bone, in the top of the tree (it stays put as the robot moves on).
      spot = take('spot');
      const root = top(parent);
      world.set(at[0], at[1], at[2]);
      parent.localToWorld(world);
      world.y = floorY(parent) + FLOOR_LIFT;
      root.worldToLocal(world);
      root.add(spot.obj);
      spot.obj.position.copy(world);
      spot.mat.color.setHex(color);
    }
    const size = look.size;
    const s0 = look.grow[0];
    if (piece.obj instanceof THREE.Sprite) piece.obj.scale.set(size * s0, size * s0, 1);
    if (kind === 'glow') piece.mat.color.setHex(color);
    else if (look.tints) piece.mat.color.setHex(look.tints[Math.floor(rnd() * look.tints.length)]!);
    else piece.mat.color.setHex(0xffffff);
    piece.mat.opacity = 0;
    if (piece.mat instanceof THREE.SpriteMaterial) piece.mat.rotation = rnd() * Math.PI * 2;
    const it = spareItems.pop() ?? ({} as Item);
    it.kind = kind;
    it.piece = piece;
    it.spot = spot;
    it.age = 0;
    it.life = look.life;
    it.x0 = scratch.x;
    it.y0 = scratch.y;
    it.z0 = scratch.z;
    it.vx = (rnd() - 0.5) * look.spread;
    it.vy = look.rise * (0.6 + 0.8 * rnd());
    it.sway = look.sway * (0.5 + rnd());
    it.phase = rnd() * Math.PI * 2;
    it.spin = (rnd() < 0.5 ? -1 : 1) * look.spin * (0.5 + rnd());
    it.s0 = s0;
    it.s1 = look.grow[1];
    it.size = size;
    this.items.push(it);
    live++;
    return true;
  }

  /** Advances every live effect: move, grow (a pop eases out), fade; and back to the pool when done. */
  update(dt: number): void {
    for (let i = this.items.length - 1; i >= 0; i--) {
      const it = this.items[i]!;
      it.age += dt;
      if (it.age >= it.life) {
        this.retire(i);
        continue;
      }
      const k = it.age / it.life;
      const look = LOOK[it.kind];
      const e = look.pop ? 1 - (1 - k) * (1 - k) * (1 - k) : k;
      const obj = it.piece.obj;
      const s = it.size * (it.s0 + (it.s1 - it.s0) * e);
      if (it.piece.flat) {
        obj.scale.set(s, 1, s);
      } else {
        obj.position.set(it.x0 + it.vx * it.age + it.sway * Math.sin(5 * it.age + it.phase), it.y0 + it.vy * it.age, it.z0);
        if (obj instanceof THREE.Sprite) {
          obj.scale.set(s, s, 1);
        } else {
          obj.scale.set(1 + 0.08 * Math.sin(9 * it.age + it.phase), 1, 1 + 0.08 * Math.sin(9 * it.age + it.phase));
        }
      }
      it.piece.mat.opacity = look.peak * FADE[it.kind](k);
      if (it.piece.mat instanceof THREE.SpriteMaterial && it.spin !== 0) it.piece.mat.rotation = it.spin * it.age + it.phase;
      if (it.spot) {
        const ss = SPOT.size * (SPOT.grow[0] + (SPOT.grow[1] - SPOT.grow[0]) * k);
        it.spot.obj.scale.set(ss, 1, ss);
        it.spot.mat.opacity = SPOT.peak * FADE.spot(k);
      }
    }
  }

  private retire(i: number): void {
    const it = this.items[i]!;
    giveBack(it.piece);
    if (it.spot) giveBack(it.spot);
    it.spot = null;
    spareItems.push(it);
    this.items.splice(i, 1);
    live--;
  }

  /** Every live effect goes back to the pool (a robot's dispose, or a change of species). */
  clear(): void {
    for (let i = this.items.length - 1; i >= 0; i--) this.retire(i);
  }

  get count(): number {
    return this.items.length;
  }
}
