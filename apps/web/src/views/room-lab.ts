// The room lab: a developer screen for the procedural rooms (floor3d/rooms/gen.ts). It builds one room, the
// twelve seeds of one role side by side, or every role at one size, under the robot lab's light.
// Route: #/room-lab?role=architect&w=4&d=3.5&door=s&seed=7&debug=1&grid=1&cam=top
//   role: a role id, or all (every role at the same size). Default: orchestrator.
//   w, d: the inner size in units (default 4 and 3.5). door: n | e | s | w (default s). seed: a u32 (default 7).
//   grid=1: the seeds seed .. seed+11 side by side, each labelled. hue: 0-360 (default 200).
//   debug=1: the grid (0.25), blocking cells (red), walk cells (amber), reservations (clear zones yellow,
//   door and spine blue), spots (dots coloured by clip), and the violations as a list and red markers.
//   cam: obl (default, an oblique view) or top.
// The page exposes window.__roomLab = { roles, layouts } for headless checks.

import * as THREE from 'three';
import { h } from '../ui/dom';
import type { Cleanup } from './index';
import { buildRoom } from './floor3d/rooms';
import type { KitPalette } from './floor3d/rooms/kit';
import { fnv1a32, type Catalog, type Door, type Input, type Layout, type Roles } from './floor3d/rooms/gen';
import { verify } from './floor3d/rooms/verify';
import catalogFile from '../../../../assets/rooms/catalog.json';
import rolesFile from '../../../../assets/rooms/roles.json';
import { startRoomLabEngine, type RoomLabOptions } from './engine-mount';

const CATALOG = catalogFile as unknown as Catalog;
const ROLES = rolesFile as unknown as Roles;
const ROLE_IDS = Object.keys(ROLES.roles);
const DOORS: Door[] = ['n', 'e', 's', 'w'];
const CELL = CATALOG.cell ?? 0.25;
const EPS = 1e-9;
const GAP_X = 2.2;
const GAP_Z = 2.4;
const ALL_COLS = 6;
const SEED_COLS = 4;
const SEED_COUNT = 12;
const COLOUR = { solid: 0xff3b3b, walk: 0xffb020, door: 0x3b82ff, clear: 0xffe14d, marker: 0xff1744 };

interface LabState {
  role: string;
  w: number;
  d: number;
  door: Door;
  seed: number;
  hue: number;
  debug: boolean;
  grid: boolean;
  cam: 'obl' | 'top';
}

/** One room on screen: its role, seed and where it stands. */
interface Placement {
  key: string;
  role: string;
  seed: number;
  x: number;
  z: number;
  label: string;
}

interface Box {
  x0: number;
  x1: number;
  z0: number;
  z1: number;
}

function num(q: URLSearchParams, key: string, fallback: number, lo: number, hi: number): number {
  const raw = q.get(key);
  const v = raw === null || raw.trim() === '' ? NaN : Number(raw);
  return Number.isFinite(v) && v >= lo && v <= hi ? v : fallback;
}

function readState(hash: string): LabState {
  const q = new URLSearchParams(hash.split('?')[1] ?? '');
  const role = q.get('role') ?? 'orchestrator';
  const door = q.get('door') as Door | null;
  return {
    role: role === 'all' || ROLE_IDS.includes(role) ? role : 'orchestrator',
    w: num(q, 'w', 4, 1, 12),
    d: num(q, 'd', 3.5, 1, 12),
    door: door !== null && DOORS.includes(door) ? door : 's',
    seed: Math.floor(num(q, 'seed', 7, 0, 4294967295)),
    hue: num(q, 'hue', 200, 0, 360) % 360,
    debug: q.get('debug') === '1',
    grid: q.get('grid') === '1',
    cam: q.get('cam') === 'top' ? 'top' : 'obl',
  };
}

function writeUrl(s: LabState): void {
  const q = new URLSearchParams();
  q.set('role', s.role);
  q.set('w', String(s.w));
  q.set('d', String(s.d));
  q.set('door', s.door);
  q.set('seed', String(s.seed));
  q.set('hue', String(s.hue));
  if (s.debug) q.set('debug', '1');
  if (s.grid) q.set('grid', '1');
  if (s.cam === 'top') q.set('cam', 'top');
  history.replaceState(null, '', `#/room-lab?${q.toString()}`);
}

/** The rooms on screen: every role (all), the twelve seeds of one role (grid), or the one room. */
function placements(s: LabState): Placement[] {
  if (s.role === 'all') {
    const rows = Math.ceil(ROLE_IDS.length / ALL_COLS);
    return ROLE_IDS.map((role, i) => {
      const col = i % ALL_COLS;
      const row = Math.floor(i / ALL_COLS);
      return {
        key: role,
        role,
        seed: s.seed,
        x: (col - (ALL_COLS - 1) / 2) * (s.w + GAP_X),
        z: (row - (rows - 1) / 2) * (s.d + GAP_Z),
        label: role,
      };
    });
  }
  if (s.grid) {
    const rows = Math.ceil(SEED_COUNT / SEED_COLS);
    return Array.from({ length: SEED_COUNT }, (_, i) => {
      const seed = s.seed + i;
      const col = i % SEED_COLS;
      const row = Math.floor(i / SEED_COLS);
      return {
        key: `seed:${seed}`,
        role: s.role,
        seed,
        x: (col - (SEED_COLS - 1) / 2) * (s.w + GAP_X),
        z: (row - (rows - 1) / 2) * (s.d + GAP_Z),
        label: `${s.role} seed ${seed}`,
      };
    });
  }
  return [{ key: s.role, role: s.role, seed: s.seed, x: 0, z: 0, label: `${s.role} seed ${s.seed}` }];
}

/** The footprint point of a rot-0 offset turned by rot (the same mapping as verify.ts). */
function turn(x: number, z: number, rot: number): [number, number] {
  switch (rot) {
    case 90:
      return [z, -x];
    case 180:
      return [-x, -z];
    case 270:
      return [-z, x];
    default:
      return [x, z];
  }
}

/** The axis-aligned box of a rot-0 rectangle turned by rot and moved to (cx, cz). */
function boxOf(x0: number, x1: number, z0: number, z1: number, cx: number, cz: number, rot: number): Box {
  const pts = [turn(x0, z0, rot), turn(x1, z0, rot), turn(x0, z1, rot), turn(x1, z1, rot)];
  const xs = pts.map((p) => p[0] + cx);
  const zs = pts.map((p) => p[1] + cz);
  return { x0: Math.min(...xs), x1: Math.max(...xs), z0: Math.min(...zs), z1: Math.max(...zs) };
}

const overlaps = (a: Box, b: Box): boolean => a.x0 < b.x1 - EPS && a.x1 > b.x0 + EPS && a.z0 < b.z1 - EPS && a.z1 > b.z0 + EPS;

function centreIn(a: Box, b: Box): boolean {
  const cx = (a.x0 + a.x1) / 2;
  const cz = (a.z0 + a.z1) / 2;
  return cx >= b.x0 - EPS && cx <= b.x1 + EPS && cz >= b.z0 - EPS && cz <= b.z1 + EPS;
}

/** Materials and geometry shared by every overlay of one mount. */
interface Kit {
  cell: THREE.PlaneGeometry;
  cellMat: Record<'solid' | 'walk' | 'door' | 'clear', THREE.MeshBasicMaterial>;
  spot: THREE.SphereGeometry;
  marker: THREE.SphereGeometry;
  markerMat: THREE.MeshBasicMaterial;
  gridMat: THREE.LineBasicMaterial;
  lineMat: THREE.LineBasicMaterial;
  spotMats: Map<string, THREE.MeshBasicMaterial>;
}

function makeKit(): Kit {
  const cellMat = (color: number): THREE.MeshBasicMaterial =>
    new THREE.MeshBasicMaterial({ color, transparent: true, opacity: 0.5, depthWrite: false });
  return {
    cell: new THREE.PlaneGeometry(CELL * 0.94, CELL * 0.94),
    cellMat: { solid: cellMat(COLOUR.solid), walk: cellMat(COLOUR.walk), door: cellMat(COLOUR.door), clear: cellMat(COLOUR.clear) },
    spot: new THREE.SphereGeometry(0.04, 8, 6),
    marker: new THREE.SphereGeometry(0.09, 10, 8),
    markerMat: new THREE.MeshBasicMaterial({ color: COLOUR.marker }),
    gridMat: new THREE.LineBasicMaterial({ color: 0x1f2937, transparent: true, opacity: 0.25 }),
    lineMat: new THREE.LineBasicMaterial({ color: COLOUR.marker }),
    spotMats: new Map(),
  };
}

function spotMaterial(kit: Kit, clip: string | null): THREE.MeshBasicMaterial {
  const key = clip ?? '';
  let m = kit.spotMats.get(key);
  if (!m) {
    const colour = clip === null ? new THREE.Color(0xffffff) : new THREE.Color().setHSL((fnv1a32(clip) % 360) / 360, 0.85, 0.5);
    kit.spotMats.set(key, (m = new THREE.MeshBasicMaterial({ color: colour })));
  }
  return m;
}

/**
 * The debug overlay of one room, in its own group at (cx, cz): the grid, the cells held by a solid (red) or a
 * walk item (amber), the reservations (clear zones yellow; door and spine blue), the spots and the violations.
 */
function overlay(s: LabState, layout: Layout, violations: string[], cx: number, cz: number, kit: Kit): THREE.Group {
  const { w, d, door } = s;
  const g = new THREE.Group();
  g.position.set(cx, 0, cz);
  const cols = Math.floor(w / CELL);
  const rows = Math.floor(d / CELL);
  const gx = -w / 2;
  const gz = -d / 2;
  const cellBox = (i: number, j: number): Box => ({ x0: gx + i * CELL, x1: gx + (i + 1) * CELL, z0: gz + j * CELL, z1: gz + (j + 1) * CELL });
  const cells: Box[] = [];
  for (let j = 0; j < rows; j++) for (let i = 0; i < cols; i++) cells.push(cellBox(i, j));

  // Occupancy: 2 = blocking solid, 1 = walk item. Reservations: 1 = clear zone, 2 = door or spine.
  const occ = new Uint8Array(cells.length);
  const res = new Uint8Array(cells.length);
  for (const it of layout.items) {
    const cat = CATALOG.items[it.id];
    if (!cat) continue;
    const [cw, , cd] = cat.size;
    const hw = cw / 2;
    const hd = cd / 2;
    const solid = cat.place !== 'surface' && cat.layer !== 'floor' && cat.place !== 'wallmount';
    if (solid) {
      const body = boxOf(-hw, hw, -hd, hd, it.x, it.z, it.rot);
      cells.forEach((c, k) => {
        if (!overlaps(c, body)) return;
        occ[k] = cat.walk ? Math.max(occ[k]!, 1) : 2;
      });
    }
    const front = cat.clear?.front ?? 0;
    const side = cat.clear?.side ?? 0;
    if (front > 0 && cat.layer !== 'floor') {
      const zone = boxOf(-hw - side, hw + side, hd, hd + front, it.x, it.z, it.rot);
      cells.forEach((c, k) => {
        if (overlaps(c, zone) && res[k] === 0) res[k] = 1;
      });
    }
  }
  const width = Math.min(1, (door === 'n' || door === 's' ? w : d) - 0.5);
  const doorBox: Box =
    door === 'n'
      ? { x0: -width / 2, x1: width / 2, z0: -d / 2, z1: -d / 2 + 1 }
      : door === 's'
        ? { x0: -width / 2, x1: width / 2, z0: d / 2 - 1, z1: d / 2 }
        : door === 'e'
          ? { x0: w / 2 - 1, x1: w / 2, z0: -width / 2, z1: width / 2 }
          : { x0: -w / 2, x1: -w / 2 + 1, z0: -width / 2, z1: width / 2 };
  const spineBox: Box =
    door === 'n'
      ? { x0: -0.25, x1: 0.25, z0: -d / 2, z1: 0 }
      : door === 's'
        ? { x0: -0.25, x1: 0.25, z0: 0, z1: d / 2 }
        : door === 'e'
          ? { x0: 0, x1: w / 2, z0: -0.25, z1: 0.25 }
          : { x0: -w / 2, x1: 0, z0: -0.25, z1: 0.25 };
  cells.forEach((c, k) => {
    if (centreIn(c, doorBox) || centreIn(c, spineBox)) res[k] = 2;
  });

  // The grid lines (0.25) and one tinted square per cell that carries a state.
  const pts: THREE.Vector3[] = [];
  for (let i = 0; i <= cols; i++) pts.push(new THREE.Vector3(gx + i * CELL, 0.025, gz), new THREE.Vector3(gx + i * CELL, 0.025, gz + rows * CELL));
  for (let j = 0; j <= rows; j++) pts.push(new THREE.Vector3(gx, 0.025, gz + j * CELL), new THREE.Vector3(gx + cols * CELL, 0.025, gz + j * CELL));
  g.add(new THREE.LineSegments(new THREE.BufferGeometry().setFromPoints(pts), kit.gridMat));
  cells.forEach((c, k) => {
    const mat =
      occ[k] === 2 ? kit.cellMat.solid : res[k] === 2 ? kit.cellMat.door : occ[k] === 1 ? kit.cellMat.walk : res[k] === 1 ? kit.cellMat.clear : null;
    if (!mat) return;
    const tile = new THREE.Mesh(kit.cell, mat);
    tile.rotation.x = -Math.PI / 2;
    tile.position.set((c.x0 + c.x1) / 2, 0.03, (c.z0 + c.z1) / 2);
    g.add(tile);
  });

  // The spots, coloured by clip.
  for (const sp of layout.spots) {
    const dot = new THREE.Mesh(kit.spot, spotMaterial(kit, sp.clip));
    dot.position.set(sp.x, 0.05, sp.z);
    g.add(dot);
  }

  // The violations: a red marker over each item or spot they name.
  const marks = new Set<number>();
  const spotMarks = new Set<number>();
  for (const v of violations) {
    for (const m of v.matchAll(/\bitems? (\d+)/g)) marks.add(Number(m[1]));
    for (const m of v.matchAll(/ and (\d+) \(/g)) marks.add(Number(m[1]));
    for (const m of v.matchAll(/\bspot (\d+)\b/g)) spotMarks.add(Number(m[1]));
  }
  const pin = (x: number, y: number, z: number, top: number): void => {
    const ball = new THREE.Mesh(kit.marker, kit.markerMat);
    ball.position.set(x, top, z);
    g.add(ball);
    g.add(new THREE.Line(new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(x, y, z), new THREE.Vector3(x, top, z)]), kit.lineMat));
  };
  for (const n of marks) {
    const it = layout.items[n];
    if (it) pin(it.x, it.y, it.z, it.y + 1.1);
  }
  for (const n of spotMarks) {
    const sp = layout.spots[n];
    if (sp) pin(sp.x, 0, sp.z, 0.9);
  }
  return g;
}

/** A flat caption on the floor, in the gap in front of a room (+z). */
function caption(text: string, w: number, d: number, x: number, z: number): THREE.Mesh | null {
  const c = document.createElement('canvas');
  c.width = 768;
  c.height = 80;
  const ctx = c.getContext('2d');
  if (!ctx) return null;
  ctx.fillStyle = '#1f2937';
  ctx.font = '600 40px system-ui, sans-serif';
  ctx.textBaseline = 'middle';
  ctx.fillText(text, 8, 42);
  const width = Math.max(w, 2.4);
  const mat = new THREE.MeshBasicMaterial({ map: new THREE.CanvasTexture(c), transparent: true, depthWrite: false });
  const mesh = new THREE.Mesh(new THREE.PlaneGeometry(width, (width * 80) / 768), mat);
  mesh.rotation.x = -Math.PI / 2;
  // In front of the room (+z, the side the oblique camera looks from), so the back wall does not hide it.
  mesh.position.set(x, 0.02, z + d / 2 + 0.55);
  return mesh;
}

/** `engine=wasm` on the route: the room lab on the wasm engine, its canvas filling the root. The options are read once. */
function mountEngineRoomLab(root: HTMLElement): Cleanup {
  const canvas = document.createElement('canvas');
  canvas.style.cssText = 'display:block;width:100%;height:70vh;min-height:420px;touch-action:none';
  root.append(canvas);
  const s = readState(location.hash);
  const lab: RoomLabOptions = { role: s.role, size: `${s.w}x${s.d}`, seed: s.seed, door: s.door, grid: s.grid };
  let engine: { destroy: () => void } | null = null;
  let closed = false;
  startRoomLabEngine(canvas, lab).then(
    (started) => {
      if (closed) started.destroy();
      else engine = started;
    },
    (err: unknown) => {
      const msg = err instanceof Error ? err.message : String(err);
      canvas.replaceWith(h('p', { style: 'color:var(--ink-2)' }, `The engine did not start: ${msg}`));
    },
  );
  return () => {
    closed = true;
    engine?.destroy();
    canvas.remove();
  };
}

export function mount(root: HTMLElement): Cleanup {
  if (new URLSearchParams(location.hash.split('?')[1] ?? '').get('engine') === 'wasm') return mountEngineRoomLab(root);
  let state = readState(location.hash);

  // ---- Controls ------------------------------------------------------------------------------
  const bar = 'display:flex;flex-wrap:wrap;gap:10px 16px;align-items:center;padding:8px 0';
  const field = (label: string, control: HTMLElement): HTMLElement =>
    h('label', { style: 'display:flex;gap:6px;align-items:center' }, h('span', { style: 'font-size:12px;color:var(--ink-2)' }, label), control);
  const numIn = (label: string, value: number, step: string, lo: number, hi: number): HTMLInputElement =>
    h('input', { type: 'number', value: String(value), step, min: lo, max: hi, 'aria-label': label, style: 'width:76px', onchange: () => apply() }) as HTMLInputElement;

  const roleSel = h('select', { 'aria-label': 'Role', onchange: () => apply() }, ...['all', ...ROLE_IDS].map((r) => h('option', { value: r }, r))) as HTMLSelectElement;
  roleSel.value = state.role;
  const wIn = numIn('Width', state.w, '0.5', 1, 12);
  const dIn = numIn('Depth', state.d, '0.5', 1, 12);
  const doorSel = h('select', { 'aria-label': 'Door', onchange: () => apply() }, ...DOORS.map((v) => h('option', { value: v }, v))) as HTMLSelectElement;
  doorSel.value = state.door;
  const seedIn = h('input', { type: 'number', value: String(state.seed), step: '1', min: 0, max: 4294967295, 'aria-label': 'Seed', style: 'width:110px', onchange: () => apply() }) as HTMLInputElement;
  const debugIn = h('input', { type: 'checkbox', 'aria-label': 'Debug overlay', onchange: () => apply() }) as HTMLInputElement;
  debugIn.checked = state.debug;
  const gridIn = h('input', { type: 'checkbox', 'aria-label': 'Twelve seeds', onchange: () => apply() }) as HTMLInputElement;
  gridIn.checked = state.grid;
  const camSel = h('select', { 'aria-label': 'Camera', onchange: () => apply() }, h('option', { value: 'obl' }, 'oblique'), h('option', { value: 'top' }, 'top')) as HTMLSelectElement;
  camSel.value = state.cam;
  const stats = h('div', { style: 'font-family:var(--mono);font-size:12px;color:var(--ink-2);padding:4px 0', 'aria-live': 'polite' });
  const list = h('pre', { style: 'font-family:var(--mono);font-size:12px;color:var(--ink-1);white-space:pre-wrap;margin:0;padding:4px 0' });
  const canvasBox = h('div', { style: 'width:100%;border-radius:12px;overflow:hidden' });

  root.append(
    h(
      'div',
      { style: 'padding:12px 16px' },
      h(
        'div',
        { style: bar },
        field('Role', roleSel),
        field('Width', wIn),
        field('Depth', dIn),
        field('Door', doorSel),
        field('Seed', seedIn),
        field('Debug', debugIn),
        field('Twelve seeds', gridIn),
        field('Camera', camSel),
      ),
      stats,
      canvasBox,
      list,
    ),
  );

  // ---- Renderer ------------------------------------------------------------------------------
  let made: THREE.WebGLRenderer | null = null;
  try {
    made = new THREE.WebGLRenderer({ antialias: true });
    made.debug.checkShaderErrors = false; // the info log is read only when a shader fails
  } catch {
    made = null;
  }
  if (!made) {
    stats.textContent = 'WebGL is not available here.';
    return () => {};
  }
  const renderer = made;
  renderer.shadowMap.enabled = true;
  renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
  canvasBox.append(renderer.domElement);
  const scene = new THREE.Scene();
  scene.background = new THREE.Color('#e9edf3');
  scene.add(new THREE.HemisphereLight(0xeaf4ff, 0x8a7a5a, 1.3));
  const key = new THREE.DirectionalLight(0xfff1d6, 2.6);
  key.position.set(2.5, 4, 3);
  key.castShadow = true;
  key.shadow.mapSize.set(2048, 2048);
  scene.add(key);
  const ground = new THREE.Mesh(new THREE.PlaneGeometry(1, 1), new THREE.ShadowMaterial({ opacity: 0.2 }));
  ground.rotation.x = -Math.PI / 2;
  ground.receiveShadow = true;
  scene.add(ground);
  const camera = new THREE.PerspectiveCamera(30, 1, 0.1, 400);
  const kit = makeKit();
  const palette: KitPalette = {
    sheet: new THREE.Color(0xe9edf7),
    rule: new THREE.Color(0xa9b4cc),
    ink3: new THREE.Color(0x7d8bb5),
    signal: new THREE.Color('#ff7a2e'),
    ok: new THREE.Color(0x3ee08f),
    info: new THREE.Color(0x4cc9ff),
  };

  let content = new THREE.Group();
  scene.add(content);
  let animators: ((t: number) => void)[] = [];
  const t0 = performance.now();
  let raf = 0;

  /** Frames the rooms: the whole layout fits the view, from the oblique or the top camera. */
  function frame(reach: number, depth: number): void {
    const span = Math.max(reach, depth);
    const dist = span * 1.45 + 4;
    key.shadow.camera.left = -reach / 2 - 2;
    key.shadow.camera.right = reach / 2 + 2;
    key.shadow.camera.top = depth / 2 + 2;
    key.shadow.camera.bottom = -depth / 2 - 2;
    key.shadow.camera.near = 1;
    key.shadow.camera.far = 80;
    key.shadow.camera.updateProjectionMatrix();
    ground.scale.set(reach + 6, depth + 6, 1);
    if (state.cam === 'top') {
      camera.up.set(0, 0, -1);
      camera.position.set(0, dist, 0.001);
    } else {
      camera.up.set(0, 1, 0);
      camera.position.set(0.55, 0.85, 0.9).normalize().multiplyScalar(dist);
    }
    camera.lookAt(0, 0, 0);
  }

  function fit(): void {
    const width = Math.max(320, canvasBox.clientWidth || 960);
    const height = state.role === 'all' ? 760 : 540;
    renderer.setSize(width, height);
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
  }

  function rebuild(): void {
    scene.remove(content);
    content.traverse((o) => {
      if (o instanceof THREE.Mesh) o.geometry.dispose();
    });
    content = new THREE.Group();
    scene.add(content);
    animators = [];

    const rooms = placements(state);
    const layouts: Record<string, Layout> = {};
    const lines: string[] = [];
    let total = 0;
    let reachX = 0;
    let reachZ = 0;
    let warnings = 0;
    for (const p of rooms) {
      // The half size of the cell the room sits in (the inner size plus the wall margin). buildRoom builds a room
      // smaller than its minimum at that minimum, and reports it (warned): the lab shows the size it built.
      const half = Math.max(state.w, state.d) / 2 + 0.075;
      const built = buildRoom(p.role, half, state.hue, palette, { seed: p.seed, door: state.door, w: state.w, d: state.d });
      const rw = built.w;
      const rd = built.d;
      built.group.position.set(p.x, 0, p.z);
      built.group.traverse((o) => {
        if (o instanceof THREE.Mesh) {
          o.castShadow = true;
          o.receiveShadow = true;
        }
      });
      content.add(built.group);
      animators.push(built.animate);
      reachX = Math.max(reachX, Math.abs(p.x) + rw / 2 + 1);
      reachZ = Math.max(reachZ, Math.abs(p.z) + rd / 2 + 1);

      // The desks are those of the requested size (buildRoom keeps them when it enlarges a room to its minimum).
      const agents = Math.min(4, Math.max(1, Math.floor((state.w * state.d) / 6)));
      const input: Input = { role: p.role, w: rw, d: rd, door: state.door, seed: p.seed, agents };
      const layout = built.layout;
      const violations = verify(layout, input, CATALOG, ROLES);
      layouts[p.key] = layout;
      total += violations.length;
      if (built.warned) warnings++;
      const dropped = layout.dropped.length ? ` dropped ${layout.dropped.join(', ')}` : '';
      const skipped = layout.skipped.length ? ` skipped ${layout.skipped.join(', ')}` : '';
      const size = rw !== state.w || rd !== state.d ? ` built at ${rw} × ${rd} (below its minimum)` : '';
      lines.push(`${p.label}: items ${layout.items.length}, spots ${layout.spots.length}${skipped}${dropped}${size}, violations ${violations.length}`);
      for (const v of violations) lines.push(`    ${v}`);

      const name = caption(p.label, rw, rd, p.x, p.z);
      if (name) content.add(name);
      if (state.debug) content.add(overlay({ ...state, w: rw, d: rd }, layout, violations, p.x, p.z, kit));
    }

    const counts = rooms.length === 1 ? '' : `${rooms.length} rooms · `;
    const small = warnings > 0 ? ` · ${warnings} below minimum` : '';
    stats.textContent = `${counts}violations ${total}${small} · ${state.w} × ${state.d} · door ${state.door}${state.debug ? ' · debug' : ''}`;
    list.textContent = state.debug ? lines.join('\n') : '';
    frame(reachX * 2, reachZ * 2);
    fit();

    const roles = [...new Set(rooms.map((p) => p.role))];
    (window as unknown as { __roomLab?: { roles: string[]; layouts: Record<string, Layout> } }).__roomLab = { roles, layouts };
  }

  /** Reads the controls into the state, writes the route and rebuilds the rooms. */
  function apply(): void {
    const readNum = (el: HTMLInputElement, fallback: number): number => {
      const v = Number(el.value);
      return Number.isFinite(v) && v > 0 ? v : fallback;
    };
    state = {
      role: roleSel.value,
      w: readNum(wIn, state.w),
      d: readNum(dIn, state.d),
      door: doorSel.value as Door,
      seed: Math.max(0, Math.floor(readNum(seedIn, state.seed))),
      hue: state.hue,
      debug: debugIn.checked,
      grid: gridIn.checked,
      cam: camSel.value === 'top' ? 'top' : 'obl',
    };
    writeUrl(state);
    rebuild();
  }

  const loop = (): void => {
    const t = (performance.now() - t0) / 1000;
    for (const a of animators) a(t);
    renderer.render(scene, camera);
    raf = requestAnimationFrame(loop);
  };
  const onResize = (): void => {
    fit();
  };
  window.addEventListener('resize', onResize);
  rebuild();
  raf = requestAnimationFrame(loop);

  return () => {
    cancelAnimationFrame(raf);
    window.removeEventListener('resize', onResize);
    renderer.dispose();
  };
}
