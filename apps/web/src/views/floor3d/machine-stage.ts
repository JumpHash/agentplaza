// The machine's 3D picture: one shared offscreen renderer draws each machine's thumbnail (a PNG) and the live orbit
// view on the list's stage canvas, with the same framing, lights and ink, so a morph starts pixel-identical. The model
// is the floor's exterior model (hardware.ts), configured from the machine: its look, its GPU cards (count, and a
// vendor tint on a cloned material), its memory sticks and its storage devices, where the model has them.

import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { buildModel, type Built, type Model, type Palette, type Params, type Part } from './hardware';
import { MODELS, PALETTE } from './hardware-index';
import { EXTERIOR, lookOf, type Look, type MachineKind } from './machine-model';

/** CSS px of a list thumbnail. */
export const THUMB_W = 160;
export const THUMB_H = 120;

export interface PictureSpec {
  kind: MachineKind;
  /** Lower-case maker as the fleet reports it ('apple', 'dell', ...). */
  vendor: string | null;
  /** Total RAM in MiB. */
  memory_mib: number;
  /** One entry per accelerator card, vendor as reported ('NVIDIA', 'AMD', 'Intel', 'Apple', ...). */
  gpus: { vendor: string }[];
  /** Storage device count: only the workstation's M.2 slot draws them. */
  drives?: number;
}

export interface Stage {
  readonly canvas: HTMLCanvasElement;
  place(spec: PictureSpec, css: { w: number; h: number }, pr: number): void;
  pose(t: number): void;
  orbit(on: boolean): void;
  tick(dtMs: number): void;
  resize(css: { w: number; h: number }, pr: number): void;
  prepare(spec: PictureSpec): void;
  release(): void;
}

// ---- The picture's fields ---------------------------------------------------------------------

/** Bumped when the same spec would draw differently, so cached pictures are not reused. */
const PICTURE_VERSION = 1;
const FOV = 30;
const MAX_CARDS = 4;
/**
 * The GPU cards a look draws at most (the laptop has one chip). Looks not listed have none (Apple SoCs, edge, phone).
 * Workstation: capped at 2, the two the model has. A third card has no free place inside the case: between the side
 * glass (inner face x -2.45) and the board (x -0.08) the space is 2.37 wide, and the two cards (2.3 wide) take most of it;
 * the CPU socket, cooler and DIMMs fill the rest of that half (a search over every part found no free place for a third).
 * Cards 3 and 4 are not drawn.
 * Server: capped at 4, the four card parts its model has (a 2x2 inside the chassis, see server.json).
 */
const CARD_LOOKS = new Map<Look, number>([
  ['workstation', 2],
  ['server', MAX_CARDS],
  ['laptop', 1],
]);
/** The DIMM slots each look's model has. Its `dimms` param is the stick count. */
const MEMORY_SLOTS = new Map<Look, number>([
  ['workstation', 4],
  ['laptop', 4],
  ['server', 16],
]);
/** The GPU shroud colour per vendor. Other vendors keep the palette's shroud. */
const CARD_TINT = new Map<string, number>([
  ['nvidia', 0x76b900],
  ['amd', 0xc8102e],
  ['intel', 0x0068b5],
]);

const clamp = (v: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, v));
const clampPr = (pr: number): number => clamp(Number.isFinite(pr) ? pr : 1, 1, 2);

interface Picture {
  look: Look;
  /** The tint per visible card, in card order (null: the palette's shroud). */
  tints: (number | null)[];
  sticks: number;
  drives: number;
}

/** The sticks for `mib` of RAM: a stick is a power of two of at least 8 GiB, sized to fill the slots evenly. */
function sticksFor(mib: number, slots: number): number {
  const gib = Math.max(0, Number.isFinite(mib) ? mib : 0) / 1024;
  let p = 1;
  while (p < Math.ceil(gib / slots)) p *= 2;
  const stick = Math.max(8, p);
  return clamp(Math.round(gib / stick), 1, slots);
}

function drivesFor(drives: number | undefined): number {
  return drives === undefined || !Number.isFinite(drives) ? 1 : clamp(Math.round(drives), 0, 2);
}

function pictureOf(spec: PictureSpec): Picture {
  const look = lookOf(spec.kind, spec.vendor);
  const max = CARD_LOOKS.get(look) ?? 0;
  const tints = spec.gpus.slice(0, max).map((g) => CARD_TINT.get(String(g.vendor ?? '').toLowerCase()) ?? null);
  const slots = MEMORY_SLOTS.get(look) ?? 0;
  return {
    look,
    tints,
    sticks: slots > 0 ? sticksFor(spec.memory_mib, slots) : 0,
    drives: look === 'workstation' ? drivesFor(spec.drives) : 0,
  };
}

/** FNV-1a twice, with two offsets: a 64-bit hex digest of the text. */
function digest(text: string): string {
  let a = 0x811c9dc5;
  let b = 0x9dc5811c;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    a = Math.imul(a ^ c, 0x01000193);
    b = Math.imul(b ^ c, 0x01000193);
  }
  return (a >>> 0).toString(16).padStart(8, '0') + (b >>> 0).toString(16).padStart(8, '0');
}

/** A deterministic key of exactly the fields that change the picture: the look, the cards and their tints, the sticks, the drives. */
export function specKey(spec: PictureSpec): string {
  const p = pictureOf(spec);
  const tints = p.tints.map((t) => (t === null ? '-' : t.toString(16))).join(',');
  return `m${digest([PICTURE_VERSION, p.look, tints, p.sticks, p.drives].join('|'))}`;
}

// ---- The model ----------------------------------------------------------------------------------

const hasShroud = (p: Part): boolean => p.mat === 'gpu-shroud' || (p.children ?? []).some(hasShroud);
/** A top-level part that is one GPU card: gated by the gpus param, with a shroud (the laptop's chip is bare). */
const isCard = (p: Part): boolean => p.when !== undefined && /\bgpus\b/.test(p.when) && (hasShroud(p) || p.mat === 'ic');

/** Points a card's shroud at a tinted material; the top part is the laptop's bare chip. */
function retint(p: Part, key: string, top: boolean): void {
  if (p.mat === 'gpu-shroud' || (top && p.mat === 'ic')) p.mat = key;
  for (const c of p.children ?? []) retint(c, key, false);
}

interface Assembly {
  built: Built;
  model: Model;
}

/**
 * The machine's model, built with its params and palette. The model file is never changed: the tints are new palette
 * entries on its card parts, and the gpus param shows that many cards. Defaults build the floor's model.
 */
function assemble(spec: PictureSpec): Assembly | null {
  const pic = pictureOf(spec);
  const base = MODELS[EXTERIOR[pic.look]];
  if (!base) return null;
  const parts: Part[] = JSON.parse(JSON.stringify(base.parts));
  const palette: Palette = { ...PALETTE };
  const params: Params = {};
  const cards = parts.filter(isCard);
  if (cards.length > 0) {
    // Every shown card is a part of the model (the server's four in a 2x2, the workstation's two), so the tints go on
    // the model's own cards; pictureOf caps them at the look's card count (CARD_LOOKS), which is the model's card count.
    params.gpus = pic.tints.length;
    pic.tints.forEach((t, i) => {
      const card = cards[i];
      if (t === null || !card) return;
      const key = `gpu-tint-${t.toString(16)}`;
      palette[key] = { color: `#${t.toString(16).padStart(6, '0')}` };
      retint(card, key, true);
    });
  }
  if (MEMORY_SLOTS.has(pic.look)) params.dimms = pic.sticks;
  if (pic.look === 'workstation') params.drives = pic.drives;
  const model: Model = { ...base, parts };
  const built = buildModel(model, params, { palette });
  // The ENTER state: the case opened or cut away, so the internals show.
  built.update(0, { status: 'online', working: 0, open: 1 });
  built.group.traverse((o) => {
    o.castShadow = true;
    o.receiveShadow = true;
  });
  return { built, model };
}

interface Frame {
  target: THREE.Vector3;
  pos0: THREE.Vector3;
  pos1: THREE.Vector3;
  dist: number;
}

/** The thumbnail pose (pos0) fits the bounds from the model's camera anchor; the orbit pose (pos1) is closer and a little higher. */
function frameOf(group: THREE.Group, model: Model, aspect: number): Frame {
  const box = new THREE.Box3().setFromObject(group);
  const center = box.getCenter(new THREE.Vector3());
  const size = box.getSize(new THREE.Vector3());
  const tan = Math.tan(THREE.MathUtils.degToRad(FOV) / 2);
  const dist = Math.max(size.y / 2 / tan, size.x / 2 / (tan * aspect)) * 1.12 + size.z / 2;
  const a = model.anchors?.camera;
  const dir = a ? new THREE.Vector3(...a.at).sub(new THREE.Vector3(...a.look)).normalize() : new THREE.Vector3(0, 0, 1);
  const high = dir.clone().add(new THREE.Vector3(0, 0.12, 0)).normalize();
  return {
    target: center,
    pos0: center.clone().addScaledVector(dir, dist),
    pos1: center.clone().addScaledVector(high, dist * 0.8),
    dist,
  };
}

/** The studio lights and the shadow catcher, the same in the thumbnail and the stage scene. */
function makeScene(): THREE.Scene {
  const scene = new THREE.Scene();
  scene.add(new THREE.HemisphereLight(0xfff8ec, 0x9c8f75, 1.4));
  const key = new THREE.DirectionalLight(0xfff1d6, 2.4);
  key.position.set(2.5, 4, 3);
  key.castShadow = true;
  key.shadow.mapSize.set(512, 512);
  scene.add(key);
  // A transparent floor just above the dots, so the model's shadow falls on them.
  const catcher = new THREE.Mesh(new THREE.PlaneGeometry(14, 14), new THREE.ShadowMaterial({ opacity: 0.2 }));
  catcher.rotation.x = -Math.PI / 2;
  catcher.position.y = 0.002;
  catcher.receiveShadow = true;
  scene.add(catcher);
  return scene;
}

function makeCamera(): THREE.PerspectiveCamera {
  return new THREE.PerspectiveCamera(FOV, THUMB_W / THUMB_H, 0.1, 50);
}

// Thumbnails share their geometries and toon materials for the whole session: the first mesh of each key is kept, and
// later thumbnails swap their own copies for it before rendering, so a shader compiles once per key. Kept objects are
// never disposed. Keys: a geometry by its type and parameters (the primitives only; hardware.ts does not transform
// these after making them), a toon material by every property that changes how it draws.
const kept = new WeakSet<object>();
const sharedGeos = new Map<string, THREE.BufferGeometry>();
const sharedMats = new Map<string, THREE.MeshToonMaterial>();
const SHARED_GEOS = new Set(['BoxGeometry', 'RoundedBoxGeometry', 'CylinderGeometry', 'ConeGeometry', 'SphereGeometry', 'TorusGeometry']);

function toonKey(m: THREE.MeshToonMaterial): string {
  return [m.color.getHex(), m.emissive.getHex(), m.emissiveIntensity, m.opacity, m.transparent, m.depthWrite, m.blending, m.side, m.polygonOffset, m.polygonOffsetFactor, m.polygonOffsetUnits].join('|');
}

/** Swaps a thumbnail's meshes onto the session's shared geometries and toon materials (keeping the first of each key). */
function shareTree(group: THREE.Object3D): void {
  group.traverse((o) => {
    const m = o as THREE.Mesh;
    if (!m.isMesh || m.userData.isOutline) return;
    const g = m.geometry;
    if (SHARED_GEOS.has(g.type)) {
      const key = `${g.type}|${JSON.stringify((g as THREE.BufferGeometry & { parameters?: object }).parameters)}`;
      let hit = sharedGeos.get(key);
      if (!hit) {
        hit = g;
        sharedGeos.set(key, g);
        kept.add(g);
      }
      m.geometry = hit;
    }
    const mat = m.material;
    if (!Array.isArray(mat) && mat instanceof THREE.MeshToonMaterial) {
      const key = toonKey(mat);
      let hit = sharedMats.get(key);
      if (!hit) {
        hit = mat;
        sharedMats.set(key, mat);
        kept.add(mat);
      }
      m.material = hit;
    }
  });
}

/**
 * Frees what a model built for one picture owns that is not kept for the session. The ink's material and the labels'
 * atlas are shared: they stay. An ink hull is made for its build's mesh, so it is this build's own and goes.
 */
function disposeGroup(group: THREE.Object3D): void {
  group.traverse((o) => {
    const m = o as THREE.Mesh;
    if (!m.isMesh) return;
    if (m.userData.isOutline) {
      m.geometry?.dispose();
      return;
    }
    if (!kept.has(m.geometry)) m.geometry?.dispose();
    for (const mat of Array.isArray(m.material) ? m.material : [m.material]) {
      if (mat instanceof THREE.MeshToonMaterial && !kept.has(mat)) mat.dispose();
    }
  });
}

// ---- The shared renderer --------------------------------------------------------------------------

interface View {
  w: number;
  h: number;
  pr: number;
}

interface Live {
  asm: Assembly;
  frame: Frame;
}

interface Shared {
  canvas: HTMLCanvasElement;
  renderer: THREE.WebGLRenderer;
  scene: THREE.Scene;
  camera: THREE.PerspectiveCamera;
  thumbScene: THREE.Scene;
  thumbCam: THREE.PerspectiveCamera;
  /** The model on the stage. */
  live: Live | null;
  /** The stage's buffer and CSS size, restored after a thumbnail render. */
  view: View | null;
  /** The pose: 0 the thumbnail, 1 the orbit. */
  t: number;
  controls: OrbitControls | null;
  /** The user has dragged or zoomed this model: auto-rotate stops for good. */
  touched: boolean;
  /** A model built and compiled ahead of its place(). */
  prepared: { key: string; asm: Assembly } | null;
}

let shared: Shared | null | undefined;
let stageApi: Stage | null = null;

function setBuffer(sh: Shared, w: number, h: number, pr: number): void {
  sh.renderer.setPixelRatio(1);
  sh.renderer.setSize(Math.round(w * pr), Math.round(h * pr), false);
}

function setAspect(cam: THREE.PerspectiveCamera, w: number, h: number): void {
  cam.aspect = w / Math.max(1, h);
  cam.updateProjectionMatrix();
}

function renderLive(sh: Shared): void {
  if (sh.live) sh.renderer.render(sh.scene, sh.camera);
}

function applyPose(sh: Shared): void {
  const f = sh.live?.frame;
  if (!f) return;
  sh.camera.position.lerpVectors(f.pos0, f.pos1, sh.t);
  sh.camera.lookAt(f.target);
}

function syncControls(sh: Shared): void {
  const c = sh.controls;
  const f = sh.live?.frame;
  if (!c || !f) return;
  c.target.copy(f.target);
  c.minDistance = f.dist * 0.5;
  c.maxDistance = f.dist * 1.6;
}

function dropLive(sh: Shared): void {
  if (!sh.live) return;
  sh.scene.remove(sh.live.asm.built.group);
  disposeGroup(sh.live.asm.built.group);
  sh.live = null;
}

/** Puts the stage's buffer and camera back after a thumbnail render, and draws the stage's current pose again. */
function restoreView(sh: Shared): void {
  if (!sh.view) return;
  setBuffer(sh, sh.view.w, sh.view.h, sh.view.pr);
  setAspect(sh.camera, sh.view.w, sh.view.h);
  renderLive(sh);
}

function renderThumbnail(sh: Shared, spec: PictureSpec, pr: number): string | null {
  const asm = assemble(spec);
  if (!asm) return null;
  shareTree(asm.built.group);
  const frame = frameOf(asm.built.group, asm.model, THUMB_W / THUMB_H);
  sh.thumbScene.add(asm.built.group);
  setBuffer(sh, THUMB_W, THUMB_H, pr);
  sh.thumbCam.position.copy(frame.pos0);
  sh.thumbCam.lookAt(frame.target);
  sh.renderer.render(sh.thumbScene, sh.thumbCam);
  const url = sh.canvas.toDataURL('image/png');
  sh.thumbScene.remove(asm.built.group);
  disposeGroup(asm.built.group);
  restoreView(sh);
  return url;
}

function stageOf(sh: Shared): Stage {
  return {
    canvas: sh.canvas,
    place(spec, css, pr) {
      dropLive(sh);
      const key = specKey(spec);
      let asm: Assembly | null;
      if (sh.prepared && sh.prepared.key === key) {
        asm = sh.prepared.asm;
        sh.prepared = null;
      } else {
        asm = assemble(spec);
      }
      if (!asm) return;
      const p = clampPr(pr);
      sh.canvas.style.width = `${css.w}px`;
      sh.canvas.style.height = `${css.h}px`;
      sh.view = { w: css.w, h: css.h, pr: p };
      setBuffer(sh, css.w, css.h, p);
      setAspect(sh.camera, css.w, css.h);
      sh.scene.add(asm.built.group);
      sh.live = { asm, frame: frameOf(asm.built.group, asm.model, css.w / Math.max(1, css.h)) };
      sh.t = 0;
      sh.touched = false;
      applyPose(sh);
      syncControls(sh);
      renderLive(sh);
    },
    pose(t) {
      if (!sh.live) return;
      sh.t = clamp(t, 0, 1);
      applyPose(sh);
      renderLive(sh);
    },
    orbit(on) {
      if (!on) {
        sh.controls?.dispose();
        sh.controls = null;
        return;
      }
      if (!sh.controls) {
        const c = new OrbitControls(sh.camera, sh.canvas);
        c.enablePan = false;
        c.enableRotate = true;
        c.enableZoom = true;
        c.enableDamping = true;
        c.dampingFactor = 0.08;
        c.autoRotateSpeed = 0.6;
        c.addEventListener('start', () => {
          sh.touched = true;
          c.autoRotate = false;
        });
        sh.controls = c;
      }
      sh.controls.autoRotate = !sh.touched;
      syncControls(sh);
    },
    tick(dtMs) {
      const c = sh.controls;
      if (!c || !sh.live) return;
      // update() returns true when the view moved (damping, auto-rotate): only then is a frame drawn.
      if (c.update(Math.max(0, dtMs) / 1000)) renderLive(sh);
    },
    resize(css, pr) {
      const p = clampPr(pr);
      sh.canvas.style.width = `${css.w}px`;
      sh.canvas.style.height = `${css.h}px`;
      sh.view = { w: css.w, h: css.h, pr: p };
      setBuffer(sh, css.w, css.h, p);
      setAspect(sh.camera, css.w, css.h);
      if (sh.live) {
        sh.live.frame = frameOf(sh.live.asm.built.group, sh.live.asm.model, css.w / Math.max(1, css.h));
        applyPose(sh);
        syncControls(sh);
      }
      renderLive(sh);
    },
    prepare(spec) {
      const key = specKey(spec);
      if (sh.prepared?.key === key) return;
      const asm = assemble(spec);
      if (!asm) return;
      // Compiled while in the scene, then kept (not disposed): the programs stay with its materials.
      sh.scene.add(asm.built.group);
      sh.renderer.compile(sh.scene, sh.camera);
      sh.scene.remove(asm.built.group);
      if (sh.prepared) disposeGroup(sh.prepared.asm.built.group);
      sh.prepared = { key, asm };
    },
    release() {
      dropLive(sh);
      sh.controls?.dispose();
      sh.controls = null;
    },
  };
}

function ensureShared(): Shared | null {
  if (shared !== undefined) return shared;
  try {
    const canvas = document.createElement('canvas');
    canvas.className = 'mach-stage-canvas';
    canvas.style.display = 'block';
    // Transparent: the models and their shadows sit straight on the page.
    const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, preserveDrawingBuffer: true, alpha: true });
    renderer.debug.checkShaderErrors = false; // the info log is read only when a shader fails
    renderer.setClearColor(0x000000, 0);
    renderer.shadowMap.enabled = true;
    shared = {
      canvas,
      renderer,
      scene: makeScene(),
      camera: makeCamera(),
      thumbScene: makeScene(),
      thumbCam: makeCamera(),
      live: null,
      view: null,
      t: 0,
      controls: null,
      touched: false,
      prepared: null,
    };
  } catch {
    shared = null;
  }
  return shared;
}

// ---- The picture cache: memory first, IndexedDB behind it --------------------------------------

const pictures = new Map<string, string>();
let cacheLoaded = false;
let dbp: Promise<IDBDatabase | null> | null = null;
const DB_NAME = 'apz-machine-pictures';
const STORE = 'pictures';

function openDb(): Promise<IDBDatabase | null> {
  if (!dbp) {
    dbp = new Promise((resolve) => {
      try {
        const req = indexedDB.open(DB_NAME, 1);
        req.onupgradeneeded = () => {
          try {
            req.result.createObjectStore(STORE);
          } catch {
            // the store exists
          }
        };
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => resolve(null);
        req.onblocked = () => resolve(null);
      } catch {
        resolve(null);
      }
    });
  }
  return dbp;
}

/** Hydrates memory from the store in the background: nothing waits on it, and a failure leaves memory alone. */
function loadCache(): void {
  if (cacheLoaded) return;
  cacheLoaded = true;
  openDb()
    .then((db) => {
      if (!db) return;
      try {
        const req = db.transaction(STORE, 'readonly').objectStore(STORE).openCursor();
        req.onsuccess = () => {
          const c = req.result;
          if (!c) return;
          if (typeof c.key === 'string' && typeof c.value === 'string' && !pictures.has(c.key)) pictures.set(c.key, c.value);
          c.continue();
        };
      } catch {
        // memory only
      }
    })
    .catch(() => {});
}

function saveCache(key: string, url: string): void {
  openDb()
    .then((db) => {
      if (!db) return;
      try {
        db.transaction(STORE, 'readwrite').objectStore(STORE).put(url, key);
      } catch {
        // memory only
      }
    })
    .catch(() => {});
}

// ---- The exports ----------------------------------------------------------------------------------

/**
 * The machine's thumbnail as a PNG data URL, THUMB_W * pr by THUMB_H * pr pixels, cached by spec key and pr. Null
 * without WebGL. A miss renders synchronously on the shared renderer; the stage's view is drawn again after it.
 */
export function thumbnail(spec: PictureSpec, pr: number): string | null {
  loadCache();
  const p = clampPr(pr);
  const key = `${specKey(spec)}@${p}`;
  const hit = pictures.get(key);
  if (hit) return hit;
  const sh = ensureShared();
  if (!sh) return null;
  const url = renderThumbnail(sh, spec, p);
  if (url) {
    pictures.set(key, url);
    saveCache(key, url);
  }
  return url;
}

/** The one stage for the whole console (one renderer, one canvas); null without WebGL. */
export function getStage(): Stage | null {
  loadCache();
  const sh = ensureShared();
  if (!sh) return null;
  stageApi ??= stageOf(sh);
  return stageApi;
}
