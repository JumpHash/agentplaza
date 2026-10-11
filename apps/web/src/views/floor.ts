/* The floor: the plaza as a 3D real-time-strategy scene — the view that
   replaces the register-plot 3D view. Left-click and the arena answers: every
   connected agent is a small robot standing on its role's plinth, the ground
   is ruled into the role clusters, and two agents talking walk adjacent.

   The presentation is deliberately RTS, and nothing here is decoration:
   - a FIXED camera — 57° of tilt above the horizon, 12° of yaw, orthographic
     so the plaza's parallel rulings stay parallel on screen — panned by
     dragging and zoomed by the wheel. No orbit at v0.
   - the ground is the plaza's plan: a ruled quadrant per role cluster, each
     with a plate carrying the role's monogram and name, and a plinth per
     station. Every position derives from the role's index alone
     (models/floor.ts states the layout rule).
   - agents are composed primitives — a capsule body, an eye, an antenna — in
     the theme's own ink pairs, never sprites, with a status halo on the
     ground: a ticked ring while working, an ink ring and a speech balloon
     while talking, a plain ring while idle, a dashed crit ring struck
     through when the hub, which can be read, does not list them.
   - a conversation walks the pair adjacent over the floor, eased, and turns
     them to face their travel; a hiring (the board's award note, the ledger's
     contractor) walks them together under a joint mark. Reduced motion never
     walks: the pair stands adjacent at once.
   - the robots are drawn with INSTANCED meshes: one geometry per part, one
     instance per agent, a per-instance tint, and no allocation in the frame
     path — a robot that walks costs one matrix write per part, which is the
     reason a plaza of hundreds stays at frame rate.

   Picking is a raycast against the robot instances; hover, click, the arrow
   keys and the ruled table all move the selection, and the card below reads
   the selected robot's own facts, its contacts, the threads it holds and the
   last thing it posted. Driving stays in the roster: the card links there.
   The canvas is one image to a screen reader (role=img) with a live label
   naming the fleet and the conversations, and the table beneath carries the
   same data. */

import type {
  BufferGeometry,
  Color,
  Group,
  InstancedMesh,
  Line,
  Matrix4,
  Mesh,
  MeshBasicMaterial,
  OrthographicCamera,
  Quaternion,
  Raycaster,
  Scene,
  Sprite,
  Vector2,
  Vector3,
  WebGLRenderer,
} from 'three';
import { announce, byId, el } from '../dom';
import { fmtNum, fmtStamp } from '../format';
import { vmKey } from '../models/activity';
import type { FloorRead, FloorSource, GatherKind, HireLink, Robot, RobotState } from '../models/floor';
import {
  ROBOT_STATE_LABEL,
  WORKING_LPS,
  clampPan,
  clampZoom,
  floorCountLine,
  floorLabel,
  hireLinks,
  lastPostBy,
  rtsFraming,
  rtsPanOffset,
  rtsViewH,
  readFloor,
  roleMonogram,
} from '../models/floor';
import { roleOf } from '../models/run';
import type { BoardPost, ContactKind } from '../models/social';
import { activePairs, deliveryText } from '../models/social';
import type { ContactsRead, ThreadRead } from '../store';
import { store } from '../store';
import type { Palette } from './plan';
import { palette } from './plan';

/* The runtime module arrives by a DYNAMIC import on purpose: three.js is the
   console's one heavy dependency, and a static import would carry it into the
   eager bundle every view pays for. `await import('three')` makes esbuild
   emit it as a lazy chunk that only the floor's route ever fetches. */
type ThreeModule = typeof import('three');

const REDUCED = typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;

/* world units — the plan is 1:1 with floor units and centred on the origin */
const PAD_H = 2.6; // a station plinth's height: the robots stand on it
const BODY_R = 5.4;
const BODY_LEN = 6;
const BODY_Y = PAD_H + BODY_R + BODY_LEN / 2; // the capsule's centre
const RING_IN = 12.4;
const RING_OUT = 14.6;
const RING_Y = PAD_H + 0.16;
const HEAD_Y = BODY_Y + BODY_LEN / 2 + BODY_R;
const TICKS_PER_WORKING = 6;
const DASHES_PER_LOST = 12;
const MARK_CAPACITY = 12; // the most marks one robot ever draws (a lost ring's dashes)
const WALK_MS_PER_UNIT = 9;
const WALK_MS_MIN = 260;
const WALK_MS_MAX = 900;
const THREAD_TAIL = 20; // exchanges the card's thread shows
const PLATE_SCALE = 0.5; // world units per plate pixel

/* The role tints: the theme's ink pairs, cycled by station index, so a role
   always wears the same pair and no tint is a pastel fill or the owner's
   orange — which stays the owner's hand alone. */
const TINTS = [
  { body: 'ink', eye: 'sheet' },
  { body: 'ink2', eye: 'sheet' },
  { body: 'ink3', eye: 'paper' },
] as const satisfies ReadonlyArray<{ body: keyof Palette; eye: keyof Palette }>;

/** The ink pair a station's index wears: the cycle, always defined. */
function tintOf(index: number): { body: keyof Palette; eye: keyof Palette } {
  return TINTS[((index % TINTS.length) + TINTS.length) % TINTS.length] ?? TINTS[0];
}

/** The instanced parts every robot is drawn from, plus the station plinths.
    One geometry per part, one instance per agent: the shared geometry and the
    per-instance tint are what keep the frame path allocation-free. */
interface Parts {
  bodies: InstancedMesh;
  eyes: InstancedMesh;
  stems: InstancedMesh;
  tips: InstancedMesh;
  rings: InstancedMesh;
  marks: InstancedMesh;
  bars: InstancedMesh;
  balloons: InstancedMesh;
  tails: InstancedMesh;
  pads: InstancedMesh;
  robots: number;
  stations: number;
}

/** The scratch objects the frame path reuses. Nothing in a frame allocates. */
interface Scratch {
  v1: Vector3;
  v2: Vector3;
  uv: Vector2;
  q1: Quaternion;
  q2: Quaternion;
  m1: Matrix4;
  c1: Color;
  c2: Color;
}

/** The ground wire one gathering draws along, with the mark over its meeting
    point: rebuilt only when the gathering set changes, rewritten in place
    while its members walk. */
interface LinkView {
  kind: GatherKind;
  cids: number[];
  line: Line;
  joint: Mesh;
  positions: Float32Array;
}

/** A robot's drawing instance: its place in the plan, its live position on the
    ground, the walk in flight, and the instance slot its meshes live in. */
interface RobotView {
  key: string;
  index: number;
  role: string;
  cid: number | null;
  state: RobotState;
  tint: number;
  /** Where it stands and faces now (eased while walking). */
  x: number;
  z: number;
  yaw: number;
  /** Where the plan says it belongs, and the walk in flight toward it. */
  tx: number;
  tz: number;
  walk: { fx: number; fz: number; t0: number; dur: number } | null;
  /** Whether its instance matrices are out of date. */
  dirty: boolean;
}

interface Scene3 {
  three: ThreeModule;
  renderer: WebGLRenderer;
  scene: Scene;
  camera: OrthographicCamera;
  raycaster: Raycaster;
  parts: Parts;
  scratch: Scratch;
  ground: Group;
  links: Group;
  cursor: Group;
  cursorSel: InstancedMesh;
  cursorFocus: InstancedMesh;
  label: Sprite;
}

let scene3: Scene3 | null = null;
let starting: Promise<void> | null = null;
let failed: string | null = null;

let plan: FloorRead | null = null;
const robots = new Map<string, RobotView>();
let order: Robot[] = [];
let groundKey = '';
let linkKey = '';
let links: LinkView[] = [];
let linkCapacity = 0;
let pulse = 0;
let working = false;
let selected: number | null = null;
let focusIdx = 0;
let hoverKey: string | null = null;
let zoom = 1;
const pan = { x: 0, z: 0 };
let dragging: { x: number; y: number; moved: number } | null = null;
let suppressClick = false;
let raf: number | null = null;
/** Frames the scene actually drew — proof, for the harness, that the GPU ran
    (and stays idle when the tab is hidden). */
let frames = 0;

/* DOM rebuild signatures — the register's rule: rebuild only when the words
   change, so focus and scroll survive a poll. */
let tableSig: string | null = null;
const tableRows = new Map<string, { state: HTMLTableCellElement; lines: HTMLTableCellElement; tr: HTMLTableRowElement }>();
let cardSig: string | null = null;
let talkSig: string | null = null;

/* ------------------------------------------------------------ boot */
async function ensureScene(): Promise<void> {
  if (scene3 || failed) return;
  starting ??= boot()
    .catch((err: unknown) => {
      failed = err instanceof Error ? err.message : String(err);
    })
    .finally(() => {
      starting = null;
    });
  await starting;
}

async function boot(): Promise<void> {
  const THREE = await import('three');
  const mount = byId('floor-mount');
  let renderer: WebGLRenderer;
  try {
    renderer = new THREE.WebGLRenderer({ antialias: true, powerPreference: 'low-power' });
  } catch (err) {
    throw new Error(`WebGL2 unavailable: ${err instanceof Error ? err.message : String(err)}`);
  }
  renderer.setPixelRatio(Math.min(2, window.devicePixelRatio || 1));
  renderer.domElement.tabIndex = 0;
  renderer.domElement.setAttribute('role', 'img');
  renderer.domElement.setAttribute('aria-label', 'The plaza floor. The table below carries the same agents.');
  mount.appendChild(renderer.domElement);

  const scene = new THREE.Scene();
  const camera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0.1, 6000);
  scene.add(new THREE.AmbientLight(0xffffff, 1.45));
  const sun = new THREE.DirectionalLight(0xffffff, 1.5);
  sun.position.set(-140, 300, 200);
  scene.add(sun);

  const ground = new THREE.Group();
  const linksGroup = new THREE.Group();
  const cursor = new THREE.Group();
  scene.add(ground);
  scene.add(linksGroup);
  scene.add(cursor);

  const ringsMat = new THREE.MeshBasicMaterial({ color: 0xffffff, side: THREE.DoubleSide, transparent: true });
  const cursorSel = new THREE.InstancedMesh(new THREE.RingGeometry(RING_OUT + 1.4, RING_OUT + 3.4, 44), ringsMat, 1);
  const cursorFocus = new THREE.InstancedMesh(
    new THREE.RingGeometry(RING_OUT + 5.4, RING_OUT + 6.4, 44),
    new THREE.MeshBasicMaterial({ color: 0xffffff, side: THREE.DoubleSide, transparent: true }),
    1,
  );
  for (const mesh of [cursorSel, cursorFocus]) {
    mesh.frustumCulled = false;
    cursor.add(mesh);
  }

  const label = makeLabel(THREE, '', '#000');
  label.visible = false;
  scene.add(label);

  const scratch: Scratch = {
    v1: new THREE.Vector3(),
    v2: new THREE.Vector3(),
    uv: new THREE.Vector2(),
    q1: new THREE.Quaternion(),
    q2: new THREE.Quaternion(),
    m1: new THREE.Matrix4(),
    c1: new THREE.Color(),
    c2: new THREE.Color(),
  };

  scene3 = {
    three: THREE,
    renderer,
    scene,
    camera,
    raycaster: new THREE.Raycaster(),
    parts: buildParts(THREE, scene, Math.max(1, plan?.robots.length ?? 1), Math.max(1, plan?.stations.length ?? 1)),
    scratch,
    ground,
    links: linksGroup,
    cursor,
    cursorSel,
    cursorFocus,
    label,
  };

  renderer.domElement.addEventListener('click', onClick);
  renderer.domElement.addEventListener('mousemove', onHover);
  renderer.domElement.addEventListener('mousedown', onDown);
  renderer.domElement.addEventListener('wheel', onWheel, { passive: false });
  renderer.domElement.addEventListener('keydown', onKeys);
  window.addEventListener('mousemove', onDrag);
  window.addEventListener('mouseup', onUp);
  /* coming back to a hidden tab repaints the scene at once (nothing drew
     while it was hidden) */
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden && store.route.name === 'floor') syncFloor();
  });
  new ResizeObserver(() => {
    resize();
    requestFrame();
  }).observe(mount);
  resize();
  syncFloor();
  requestFrame();
}

/** Build the instanced parts for a fleet of `n` agents on `m` stations. The
    capacity is a plan's own size; a plan that outgrows it is rebuilt (see
    ensureParts), never overrun. */
function buildParts(THREE: ThreeModule, scene: Scene, n: number, m: number): Parts {
  const lit = (geo: BufferGeometry, transparent = false): InstancedMesh => {
    const mesh = new THREE.InstancedMesh(
      geo,
      new THREE.MeshLambertMaterial({ color: 0xffffff, transparent }),
      n,
    );
    mesh.instanceMatrix.setUsage(THREE.DynamicDrawUsage);
    mesh.frustumCulled = false;
    scene.add(mesh);
    return mesh;
  };
  const flat = (geo: BufferGeometry, capacity: number): InstancedMesh => {
    const mesh = new THREE.InstancedMesh(
      geo,
      new THREE.MeshBasicMaterial({ color: 0xffffff, side: THREE.DoubleSide, transparent: true }),
      capacity,
    );
    mesh.instanceMatrix.setUsage(THREE.DynamicDrawUsage);
    mesh.frustumCulled = false;
    scene.add(mesh);
    return mesh;
  };
  const pads = lit(new THREE.BoxGeometry(1, 1, 1));
  const padMesh = new THREE.InstancedMesh(pads.geometry, pads.material, m);
  scene.remove(pads);
  padMesh.instanceMatrix.setUsage(THREE.DynamicDrawUsage);
  padMesh.frustumCulled = false;
  scene.add(padMesh);
  const parts: Parts = {
    bodies: lit(new THREE.CapsuleGeometry(BODY_R, BODY_LEN, 6, 14)),
    eyes: lit(new THREE.BoxGeometry(BODY_R * 1.15, BODY_R * 0.62, 0.9)),
    stems: lit(new THREE.CylinderGeometry(0.42, 0.42, 8, 6)),
    tips: lit(new THREE.SphereGeometry(1.5, 8, 6)),
    rings: flat(new THREE.RingGeometry(RING_IN, RING_OUT, 40), n),
    marks: flat(new THREE.BoxGeometry(2.6, 0.34, 1), n * MARK_CAPACITY),
    bars: flat(new THREE.BoxGeometry(RING_OUT * 2, 0.5, 1.1), n),
    balloons: flat(new THREE.BoxGeometry(11, 7, 1), n),
    tails: flat(new THREE.ConeGeometry(2.4, 4, 4), n),
    pads: padMesh,
    robots: n,
    stations: m,
  };
  return parts;
}

/** A plan that outgrew the instance capacity gets fresh parts: the old ones
    are disposed and every robot is marked to redraw into the new slots. */
function ensureParts(THREE: ThreeModule, scene: Scene, n: number, m: number): void {
  const parts = scene3?.parts;
  if (!parts || !scene3) return;
  if (n <= parts.robots && m <= parts.stations) return;
  for (const mesh of Object.values(parts)) {
    if (mesh instanceof THREE.InstancedMesh) {
      scene.remove(mesh);
      mesh.geometry.dispose();
      const mat = mesh.material;
      for (const one of Array.isArray(mat) ? mat : [mat]) one.dispose();
    }
  }
  scene3.parts = buildParts(THREE, scene, Math.max(n, parts.robots), Math.max(m, parts.stations));
  for (const view of robots.values()) view.dirty = true;
}

function resize(): void {
  if (!scene3) return;
  const mount = byId('floor-mount');
  const w = mount.clientWidth;
  const h = mount.clientHeight;
  if (w === 0 || h === 0) return;
  scene3.renderer.setSize(w, h);
  if (plan) updateCamera(w / h);
}

/** Aim the fixed RTS camera at the plan: the frustum holds the whole plaza at
    zoom 1, and the pan moves the aim over the ground. Never an orbit. */
function updateCamera(aspect: number): void {
  if (!scene3 || !plan) return;
  const { camera } = scene3;
  const frame = rtsFraming(plan.width, plan.height, aspect, zoom);
  const view = frame.view_h;
  camera.left = (-view * aspect) / 2;
  camera.right = (view * aspect) / 2;
  camera.top = view / 2;
  camera.bottom = -view / 2;
  camera.near = 0.1;
  camera.far = view * 4 + 6000;
  camera.position.set(frame.pos[0] + pan.x, frame.pos[1], frame.pos[2] + pan.z);
  camera.lookAt(pan.x, PAD_H, pan.z);
  camera.updateProjectionMatrix();
}

/* ------------------------------------------------------------ labels
   Labels are canvas textures — text is drawn, never inserted as markup. */
function makeLabel(THREE: ThreeModule, text: string, color: string): Sprite {
  const c = document.createElement('canvas');
  const g = c.getContext('2d');
  if (!g) throw new Error('no 2d context for labels');
  const font = '11px "Martian Mono", ui-monospace, monospace';
  g.font = font;
  c.width = Math.ceil(g.measureText(text).width) + 14;
  c.height = 24;
  g.font = font;
  g.textBaseline = 'middle';
  g.fillStyle = color;
  g.fillText(text, 7, 12);
  const tex = new THREE.CanvasTexture(c);
  tex.colorSpace = THREE.SRGBColorSpace;
  const sprite = new THREE.Sprite(new THREE.SpriteMaterial({ map: tex, transparent: true, depthWrite: false }));
  sprite.scale.set(c.width * 0.5, c.height * 0.5, 1);
  sprite.userData['text'] = text;
  return sprite;
}

function retextureLabel(sprite: Sprite, text: string, color: string): void {
  if (!scene3) return;
  const fresh = makeLabel(scene3.three, text, color);
  const old = sprite.material;
  sprite.material = fresh.material;
  sprite.scale.copy(fresh.scale);
  sprite.userData['text'] = text;
  old.map?.dispose();
  old.dispose();
}

/** A station's plate: its monogram in an ink square, then the role's name —
    drawn on a canvas texture and laid flat on the ground like a map label. */
function makePlate(role: string, tint: Color, p: Palette): Mesh {
  const THREE = scene3?.three;
  if (!THREE) throw new Error('the floor has not booted');
  const c = document.createElement('canvas');
  const g = c.getContext('2d');
  if (!g) throw new Error('no 2d context for plates');
  const font = '10px "Martian Mono", ui-monospace, monospace';
  g.font = font;
  c.width = Math.ceil(g.measureText(role).width) + 36;
  c.height = 20;
  g.textBaseline = 'middle';
  g.fillStyle = tint.getStyle();
  g.fillRect(3, 4, 12, 12);
  g.font = '9px "Martian Mono", ui-monospace, monospace';
  g.textAlign = 'center';
  g.fillStyle = p.sheet;
  g.fillText(roleMonogram(role), 9, 10);
  g.textAlign = 'left';
  g.font = font;
  g.fillStyle = p.ink2;
  g.fillText(role, 21, 10);
  const tex = new THREE.CanvasTexture(c);
  tex.colorSpace = THREE.SRGBColorSpace;
  const mesh = new THREE.Mesh(
    new THREE.PlaneGeometry(c.width * PLATE_SCALE, c.height * PLATE_SCALE),
    new THREE.MeshBasicMaterial({ map: tex, transparent: true, depthWrite: false, side: THREE.DoubleSide }),
  );
  mesh.rotation.x = -Math.PI / 2;
  mesh.userData['width'] = c.width * PLATE_SCALE;
  return mesh;
}

function disposeDeep(obj: unknown): void {
  const THREE = scene3?.three;
  if (!THREE) return;
  const visit = (node: unknown): void => {
    if (!(node instanceof THREE.Object3D)) return;
    if (node instanceof THREE.Mesh || node instanceof THREE.Line || node instanceof THREE.LineSegments) {
      node.geometry.dispose();
      const mat = node.material;
      for (const one of Array.isArray(mat) ? mat : [mat]) one.dispose();
    }
    if (node instanceof THREE.Sprite) {
      node.material.map?.dispose();
      node.material.dispose();
    }
    for (const child of [...node.children]) visit(child);
  };
  visit(obj);
}

/* ------------------------------------------------------------ the ground */
/** Rebuild the ground for a plan: the plane, its ruling, a ruled quadrant per
    role cluster in that cluster's tint, a plinth per station and a plate per
    role. Rebuilt only when the layout itself changes — the ground does not
    move when a robot does. */
function rebuildGround(p: Palette): void {
  const s = scene3;
  if (!s || !plan) return;
  const THREE = s.three;
  for (const child of [...s.ground.children]) {
    s.ground.remove(child);
    disposeDeep(child);
  }
  const { width, height } = plan;
  const stations = plan.stations;

  const plane = new THREE.Mesh(new THREE.PlaneGeometry(width, height), new THREE.MeshLambertMaterial({ color: p.sheet }));
  plane.rotation.x = -Math.PI / 2;
  s.ground.add(plane);

  const grid: number[] = [];
  for (let x = 0; x <= width; x += 40) grid.push(x - width / 2, 0.02, -height / 2, x - width / 2, 0.02, height / 2);
  for (let z = 0; z <= height; z += 40) grid.push(-width / 2, 0.02, z - height / 2, width / 2, 0.02, z - height / 2);
  const gridGeo = new THREE.BufferGeometry();
  gridGeo.setAttribute('position', new THREE.Float32BufferAttribute(grid, 3));
  s.ground.add(
    new THREE.LineSegments(gridGeo, new THREE.LineBasicMaterial({ color: p.rule, transparent: true, opacity: 0.45 })),
  );

  const byTint = new Map<number, number[]>();
  for (const station of stations) {
    const x0 = station.x - width / 2;
    const x1 = x0 + station.width;
    const z0 = station.y - height / 2;
    const z1 = z0 + station.height;
    const list = byTint.get(station.index % TINTS.length) ?? [];
    list.push(
      x0, 0.05, z0, x1, 0.05, z0,
      x1, 0.05, z0, x1, 0.05, z1,
      x1, 0.05, z1, x0, 0.05, z1,
      x0, 0.05, z1, x0, 0.05, z0,
    );
    byTint.set(station.index % TINTS.length, list);
  }
  for (const [tint, pts] of byTint) {
    const geo = new THREE.BufferGeometry();
    geo.setAttribute('position', new THREE.Float32BufferAttribute(pts, 3));
    s.ground.add(
      new THREE.LineSegments(
        geo,
        new THREE.LineBasicMaterial({ color: p[tintOf(tint).body], transparent: true, opacity: 0.8 }),
      ),
    );
  }

  const { m1, v1, v2, q1, c1, c2 } = s.scratch;
  stations.forEach((station, i) => {
    const ink = p[tintOf(station.index).body];
    m1.compose(
      v1.set(station.x - width / 2 + station.width / 2, PAD_H / 2, station.y - height / 2 + station.height / 2),
      q1.identity(),
      v2.set(station.width * 0.94, PAD_H, station.height * 0.94),
    );
    s.parts.pads.setMatrixAt(i, m1);
    s.parts.pads.setColorAt(i, c1.set(ink).lerp(c2.set(p.sheet), 0.74));
    const plate = makePlate(station.role, c1.set(ink), p);
    /* the plate is anchored by its own left edge; its geometry is the canvas'
       size in world units, so nothing here guesses a pixel measurement */
    const plateW = Number(plate.userData['width'] ?? 0);
    plate.position.set(station.x - width / 2 + 7 + plateW / 2, 0.07, station.y - height / 2 + 8);
    s.ground.add(plate);
  });
  s.parts.pads.instanceMatrix.needsUpdate = true;
  if (s.parts.pads.instanceColor) s.parts.pads.instanceColor.needsUpdate = true;
}

/* ------------------------------------------------------------- the robots */
/** The world x/z a plan position stands at: the plan is centred on the origin
    and its y grows toward +z, so the camera's rake reads north as far. */
function worldX(fx: number): number {
  return plan ? fx - plan.width / 2 : fx;
}

function worldZ(fy: number): number {
  return plan ? fy - plan.height / 2 : fy;
}

function walkMs(dist: number): number {
  return Math.max(WALK_MS_MIN, Math.min(WALK_MS_MAX, dist * WALK_MS_PER_UNIT));
}

/** Reconcile the drawing's roster with the plan: a robot that appears gets an
    instance slot, one that leaves gives it up, and one whose target moved
    walks there from where it stands — never from where it was supposed to be.
    Everything else is left exactly as it was. */
function syncRobots(now: number): void {
  if (!scene3 || !plan) return;
  const seen = new Set<string>();
  plan.robots.forEach((robot, index) => {
    seen.add(robot.key);
    const at = robots.get(robot.key);
    const view: RobotView =
      at ??
      {
        key: robot.key,
        index,
        role: robot.source.role,
        cid: robot.source.cid,
        state: robot.state,
        tint: robot.station.index % TINTS.length,
        x: worldX(robot.x),
        z: worldZ(robot.y),
        yaw: 0,
        tx: worldX(robot.x),
        tz: worldZ(robot.y),
        walk: null,
        dirty: true,
      };
    if (!at) robots.set(robot.key, view);
    view.index = index;
    view.state = robot.state;
    view.tint = robot.station.index % TINTS.length;
    const tx = worldX(robot.x);
    const tz = worldZ(robot.y);
    const retarget = Math.abs(tx - view.tx) > 0.5 || Math.abs(tz - view.tz) > 0.5;
    const away = Math.abs(tx - view.x) > 0.5 || Math.abs(tz - view.z) > 0.5;
    if (REDUCED) {
      if (retarget || away) {
        view.walk = null;
        view.x = tx;
        view.z = tz;
        view.dirty = true;
      }
    } else if (retarget || (away && !view.walk)) {
      view.walk = { fx: view.x, fz: view.z, t0: now, dur: walkMs(Math.hypot(tx - view.x, tz - view.z)) };
    }
    view.tx = tx;
    view.tz = tz;
  });
  for (const [key, view] of [...robots]) {
    if (seen.has(key)) continue;
    robots.delete(key);
    hideRobot(view);
  }
  order = plan.robots;
}

/** The position a robot stands at this instant: its walk interpolated and
    eased, or its own target when it is not walking. A finished walk is
    dropped; reduced motion never started one. */
function stoodAt(view: RobotView, now: number): { x: number; z: number; moving: boolean } {
  const walk = view.walk;
  if (!walk) return { x: view.tx, z: view.tz, moving: false };
  if (walk.dur <= 0) {
    view.walk = null;
    return { x: view.tx, z: view.tz, moving: false };
  }
  const t = Math.min(1, (now - walk.t0) / walk.dur);
  const e = t * t * (3 - 2 * t);
  if (t >= 1) view.walk = null;
  return { x: walk.fx + (view.tx - walk.fx) * e, z: walk.fz + (view.tz - walk.fz) * e, moving: t < 1 };
}

const RING_FLAT = Math.PI / 2;
const HIDDEN_Y = -1000;

/** Write one robot's instance matrices: its body, eye, antenna, and the halo
    and mark its state draws. Idle slots collapse to a hidden instance rather
    than a drawn one, so no part is ever billed for a robot that lacks it. */
function drawRobot(view: RobotView, p: Palette, phase: number): void {
  const s = scene3;
  if (!s) return;
  const { m1, v1, v2, q1, q2, c1 } = s.scratch;
  const parts = s.parts;
  const i = view.index;
  if (i >= parts.robots) return;
  const tint = tintOf(view.tint);
  const stateColour =
    view.state === 'working' ? p.ok : view.state === 'unreachable' ? p.crit : view.state === 'talking' ? p.ink : p.ink3;
  q1.setFromAxisAngle(v2.set(0, 1, 0), view.yaw);
  const flat = q2.setFromAxisAngle(v2.set(1, 0, 0), -RING_FLAT);

  m1.compose(v1.set(view.x, BODY_Y, view.z), q1, v2.set(1, 1, 1));
  parts.bodies.setMatrixAt(i, m1);
  parts.bodies.setColorAt(i, c1.set(p[tint.body]));

  m1.compose(v1.set(0, 1.6, BODY_R + 0.2).applyQuaternion(q1).add(v2.set(view.x, BODY_Y, view.z)), q1, v2.set(1, 1, 1));
  parts.eyes.setMatrixAt(i, m1);
  parts.eyes.setColorAt(i, c1.set(p[tint.eye]));

  m1.compose(v1.set(view.x, HEAD_Y + 2.4, view.z), q1, v2.set(1, 1, 1));
  parts.stems.setMatrixAt(i, m1);
  parts.stems.setColorAt(i, c1.set(p[tint.body]));
  m1.compose(v1.set(view.x, HEAD_Y + 6, view.z), q1, v2.set(1, 1, 1));
  parts.tips.setMatrixAt(i, m1);
  parts.tips.setColorAt(i, c1.set(p.ink));

  const ringScale = view.state === 'idle' ? 0.86 : 1;
  m1.compose(v1.set(view.x, RING_Y, view.z), flat, v2.set(ringScale, ringScale, 1));
  parts.rings.setMatrixAt(i, m1);
  parts.rings.setColorAt(i, c1.set(stateColour));

  /* the marks: a ticked ring while working, a dashed one when the hub has lost
     the agent — the same instanced boxes, a different count and layout */
  const marks = view.state === 'working' ? TICKS_PER_WORKING : view.state === 'unreachable' ? DASHES_PER_LOST : 0;
  for (let k = 0; k < MARK_CAPACITY; k++) {
    const slot = i * MARK_CAPACITY + k;
    if (k >= marks) {
      m1.compose(v1.set(0, HIDDEN_Y, 0), q1.identity(), v2.set(0, 0, 0));
      parts.marks.setMatrixAt(slot, m1);
      continue;
    }
    const span = (Math.PI * 2) / marks;
    const angle = k * span + (view.state === 'working' && !REDUCED ? phase : 0);
    const radius = (RING_IN + RING_OUT) / 2;
    m1.compose(
      v1.set(view.x + Math.cos(angle) * radius, RING_Y + 0.12, view.z + Math.sin(angle) * radius),
      q2.setFromAxisAngle(v2.set(1, 0, 0), -RING_FLAT).multiply(q1.setFromAxisAngle(v2.set(0, 0, 1), -angle)),
      v2.set(view.state === 'working' ? 1 : 1.45, 1, 1),
    );
    parts.marks.setMatrixAt(slot, m1);
    parts.marks.setColorAt(slot, c1.set(stateColour));
  }

  /* the strike through a lost robot's halo */
  m1.compose(
    view.state === 'unreachable' ? v1.set(view.x, RING_Y + 0.14, view.z) : v1.set(0, HIDDEN_Y, 0),
    q2.setFromAxisAngle(v2.set(1, 0, 0), -RING_FLAT).multiply(q1.setFromAxisAngle(v2.set(0, 0, 1), Math.PI / 4)),
    view.state === 'unreachable' ? v2.set(1, 1, 1) : v2.set(0, 0, 0),
  );
  parts.bars.setMatrixAt(i, m1);
  parts.bars.setColorAt(i, c1.set(p.crit));

  /* a talking robot's speech mark, above its head */
  const speaking = view.state === 'talking';
  m1.compose(
    speaking ? v1.set(view.x, HEAD_Y + 12, view.z) : v1.set(0, HIDDEN_Y, 0),
    q1,
    speaking ? v2.set(1, 1, 1) : v2.set(0, 0, 0),
  );
  parts.balloons.setMatrixAt(i, m1);
  parts.balloons.setColorAt(i, c1.set(p.sheet));
  m1.compose(
    speaking ? v1.set(view.x, HEAD_Y + 7, view.z) : v1.set(0, HIDDEN_Y, 0),
    q2.setFromAxisAngle(v2.set(1, 0, 0), Math.PI),
    speaking ? v2.set(1, 1, 1) : v2.set(0, 0, 0),
  );
  parts.tails.setMatrixAt(i, m1);
  parts.tails.setColorAt(i, c1.set(p.sheet));
}

/** Push the instance buffers the frame wrote; the flags are set once per
    frame, not once per robot. */
function flushParts(): void {
  const parts = scene3?.parts;
  if (!parts) return;
  for (const mesh of [
    parts.bodies,
    parts.eyes,
    parts.stems,
    parts.tips,
    parts.rings,
    parts.marks,
    parts.bars,
    parts.balloons,
    parts.tails,
  ]) {
    mesh.instanceMatrix.needsUpdate = true;
    if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
  }
}

/** Collapse a departed robot's instances: nothing is drawn for it again, and
    its slot stays free for the next arrival. */
function hideRobot(view: RobotView): void {
  const s = scene3;
  if (!s) return;
  const { m1, v1, q1, v2 } = s.scratch;
  m1.compose(v1.set(0, HIDDEN_Y, 0), q1.identity(), v2.set(0, 0, 0));
  for (const mesh of [s.parts.bodies, s.parts.eyes, s.parts.stems, s.parts.tips, s.parts.rings, s.parts.bars, s.parts.balloons, s.parts.tails]) {
    mesh.setMatrixAt(view.index, m1);
    mesh.instanceMatrix.needsUpdate = true;
  }
  for (let k = 0; k < MARK_CAPACITY; k++) {
    s.parts.marks.setMatrixAt(view.index * MARK_CAPACITY + k, m1);
  }
  s.parts.marks.instanceMatrix.needsUpdate = true;
}

/* ------------------------------------------------------------- the wires */
/** The ground wire a gathered group stands along, with the mark over its
    meeting point. The views are rebuilt only when the gathering set changes;
    a walk rewrites their positions in place, allocating nothing. */
function syncLinks(p: Palette): void {
  const s = scene3;
  if (!s || !plan) return;
  const THREE = s.three;
  const key = plan.gatherings.map((g) => `${g.kind}:${g.cids.join('+')}`).join('|');
  const want = plan.gatherings.filter((g) => g.cids.length >= 2);
  const capacity = Math.max(2, ...plan.robots.map((_, i) => i + 2));
  if (key !== linkKey || capacity > linkCapacity) {
    linkKey = key;
    linkCapacity = Math.max(capacity, linkCapacity);
    for (const view of links) {
      s.links.remove(view.line);
      disposeDeep(view.line);
      s.links.remove(view.joint);
      disposeDeep(view.joint);
    }
    links = want.map((gathering) => {
      const positions = new Float32Array(linkCapacity * 3);
      const geo = new THREE.BufferGeometry();
      const attr = new THREE.BufferAttribute(positions, 3);
      attr.setUsage(THREE.DynamicDrawUsage);
      geo.setAttribute('position', attr);
      geo.setDrawRange(0, gathering.cids.length);
      const line = new THREE.Line(
        geo,
        new THREE.LineBasicMaterial({
          color: gathering.kind === 'hire' ? p.ink2 : p.ink3,
          transparent: true,
          opacity: 0.9,
        }),
      );
      line.frustumCulled = false;
      const joint = new THREE.Mesh(
        gathering.kind === 'hire' ? new THREE.OctahedronGeometry(4.6) : new THREE.BoxGeometry(10, 7, 1),
        new THREE.MeshBasicMaterial({ color: gathering.kind === 'hire' ? p.ink : p.ink2, side: THREE.DoubleSide }),
      );
      s.links.add(line);
      s.links.add(joint);
      return { kind: gathering.kind, cids: gathering.cids, line, joint, positions };
    });
  }
  /* the stands, rewritten in place: a pair's wire follows the walk */
  links.forEach((view, index) => {
    const gathering = want[index];
    if (!gathering) return;
    view.cids.forEach((cid, i) => {
      const stand = [...robots.values()].find((r) => r.cid === cid);
      const x = stand ? stand.x : worldX(gathering.x);
      const z = stand ? stand.z : worldZ(gathering.y);
      view.positions[i * 3] = x;
      view.positions[i * 3 + 1] = RING_Y + 0.06;
      view.positions[i * 3 + 2] = z;
    });
    const attr = view.line.geometry.getAttribute('position');
    attr.needsUpdate = true;
    view.line.geometry.setDrawRange(0, gathering.cids.length);
    view.joint.position.set(worldX(gathering.x), PAD_H + 15, worldZ(gathering.y));
    view.joint.rotation.y = REDUCED ? 0 : pulse * 0.6;
  });
}

/** The two ground halos that mark the cursor: the card's selection (the
    owner's signal) and the keyboard's focus (rule ink). */
function syncCursor(p: Palette): void {
  const s = scene3;
  if (!s || !plan) return;
  const { m1, v1, q1, v2 } = s.scratch;
  const selectedView = selected == null ? null : ([...robots.values()].find((r) => r.cid === selected) ?? null);
  const focusKey = document.activeElement === s.renderer.domElement ? order[focusIdx]?.key : undefined;
  const focusedView = focusKey == null ? null : (robots.get(focusKey) ?? null);
  (s.cursorSel.material as MeshBasicMaterial).color.set(p.signal);
  (s.cursorFocus.material as MeshBasicMaterial).color.set(p.ink3);
  const flat = q1.setFromAxisAngle(v2.set(1, 0, 0), -RING_FLAT);
  for (const [view, mesh] of [
    [selectedView, s.cursorSel],
    [focusedView, s.cursorFocus],
  ] as Array<[RobotView | null, InstancedMesh]>) {
    m1.compose(view ? v1.set(view.x, RING_Y + 0.2, view.z) : v1.set(0, HIDDEN_Y, 0), flat, v2.set(1, 1, 1));
    mesh.setMatrixAt(0, m1);
    mesh.instanceMatrix.needsUpdate = true;
  }

  /* the name, over whichever robot the cursor is on */
  const hovered = hoverKey == null ? null : (robots.get(hoverKey) ?? null);
  const named = hovered ?? focusedView ?? selectedView;
  s.label.visible = named != null;
  if (named) {
    const text = named.cid == null ? named.role : `${named.role} · cid ${named.cid}`;
    if (s.label.userData['text'] !== text) retextureLabel(s.label, text, p.ink);
    s.label.position.set(named.x, HEAD_Y + 20, named.z);
  }
}

/* ------------------------------------------------------------- picking */
function pickAt(clientX: number, clientY: number): RobotView | null {
  const s = scene3;
  if (!s) return null;
  const rect = s.renderer.domElement.getBoundingClientRect();
  s.raycaster.setFromCamera(
    s.scratch.uv.set(((clientX - rect.left) / rect.width) * 2 - 1, -((clientY - rect.top) / rect.height) * 2 + 1),
    s.camera,
  );
  const hits = s.raycaster.intersectObjects(
    [s.parts.bodies, s.parts.eyes, s.parts.stems, s.parts.tips],
    false,
  );
  const hit = hits[0];
  if (!hit || hit.instanceId == null) return null;
  const robot = order[hit.instanceId];
  return robot ? (robots.get(robot.key) ?? null) : null;
}

function onClick(ev: MouseEvent): void {
  if (suppressClick) return;
  const hit = pickAt(ev.clientX, ev.clientY);
  if (!hit) return;
  focusIdx = hit.index;
  selectCid(hit.cid);
}

function onHover(ev: MouseEvent): void {
  const s = scene3;
  if (!s) return;
  const hit = pickAt(ev.clientX, ev.clientY);
  s.renderer.domElement.style.setProperty('cursor', dragging ? 'grabbing' : hit ? 'pointer' : 'default');
  const key = hit?.key ?? null;
  if (key === hoverKey) return;
  hoverKey = key;
  requestFrame();
}

function onDown(ev: MouseEvent): void {
  if (ev.button !== 0) return;
  dragging = { x: ev.clientX, y: ev.clientY, moved: 0 };
  suppressClick = false;
}

/** Dragging pans the plaza: the aim moves against the pointer (the ground
    follows the hand), clamped so the plaza is never lost off-screen. */
function onDrag(ev: MouseEvent): void {
  const s = scene3;
  if (!dragging || !s || !plan) return;
  const dx = ev.clientX - dragging.x;
  const dy = ev.clientY - dragging.y;
  dragging = { x: ev.clientX, y: ev.clientY, moved: dragging.moved + Math.abs(dx) + Math.abs(dy) };
  const mount = byId('floor-mount');
  const aspect = mount.clientWidth / Math.max(1, mount.clientHeight);
  const perPx = rtsViewH(plan.width, plan.height, aspect) / zoom / Math.max(1, mount.clientHeight);
  const [ox, oz] = rtsPanOffset(dx, dy, perPx);
  const next = clampPan({ x: pan.x + ox, z: pan.z + oz }, plan.width, plan.height);
  pan.x = next.x;
  pan.z = next.z;
  updateCamera(aspect);
  requestFrame();
}

function onUp(): void {
  if (!dragging) return;
  suppressClick = dragging.moved > 4;
  dragging = null;
}

/** The wheel zooms the fixed camera: the frustum narrows, the ground's ruling
    stays exactly where it is. */
function onWheel(ev: WheelEvent): void {
  const s = scene3;
  if (!s || !plan) return;
  ev.preventDefault();
  zoom = clampZoom(zoom * Math.exp(-ev.deltaY * 0.0012));
  const mount = byId('floor-mount');
  updateCamera(mount.clientWidth / Math.max(1, mount.clientHeight));
  requestFrame();
}

/** The keys while the canvas itself holds focus: the arrows walk the robots
    (the sheet-wide handler leaves canvases to their own devices). */
function onKeys(ev: KeyboardEvent): void {
  switch (ev.key) {
    case 'ArrowLeft':
      moveFocus(-1, 0);
      ev.preventDefault();
      break;
    case 'ArrowRight':
      moveFocus(1, 0);
      ev.preventDefault();
      break;
    case 'ArrowUp':
      moveFocus(0, -1);
      ev.preventDefault();
      break;
    case 'ArrowDown':
      moveFocus(0, 1);
      ev.preventDefault();
      break;
    case 'Enter':
    case ' ': {
      const robot = order[focusIdx];
      if (robot) {
        selectCid(robot.source.cid);
        ev.preventDefault();
      }
      break;
    }
    default:
      break;
  }
}

/* --------------------------------------------------------- the frame loop */
function requestFrame(): void {
  if (raf != null) return;
  raf = requestAnimationFrame(() => {
    raf = null;
    frame();
  });
}

/** Step every robot's walk to `now`: the eased position on the way to its
    target, and the facing turned toward the travel. Returns whether anything
    moved. Both the frame loop (for smoothness) and the store's paint path
    (so a walk still advances where animation frames are starved — the
    register's rule that the paint path never depends on frames) call it. */
function stepRobots(now: number, p: Palette): boolean {
  let moved = false;
  for (const view of robots.values()) {
    const at = stoodAt(view, now);
    const turned = Math.abs(at.x - view.x) > 0.001 || Math.abs(at.z - view.z) > 0.001;
    if (turned) {
      const wanted = Math.atan2(at.x - view.x, at.z - view.z);
      let delta = wanted - view.yaw;
      while (delta > Math.PI) delta -= Math.PI * 2;
      while (delta < -Math.PI) delta += Math.PI * 2;
      view.yaw += delta * 0.25;
      view.x = at.x;
      view.z = at.z;
    }
    if (turned || view.dirty) {
      view.dirty = false;
      drawRobot(view, p, pulse);
      moved = true;
    }
  }
  return moved;
}

/** One frame: step the walks, rewrite only the instances that moved, and
    draw — but only while the floor is up, the tab is visible, and something
    is actually moving (a walk, or the ticks of a working agent's ring). A
    plaza where everybody stands still spends no frames at all. */
function frame(): void {
  const s = scene3;
  if (!s || !plan) return;
  if (store.route.name !== 'floor' || document.hidden) return;
  const p = palette();
  const now = Date.now();
  working = plan.robots.some((r) => r.state === 'working');
  if (!REDUCED && working) pulse += 0.05;

  const moved = stepRobots(now, p);
  if (moved) flushParts();
  if (moved) syncLinks(p);
  syncCursor(p);
  s.renderer.render(s.scene, s.camera);
  frames++;
  if (moved || working || dragging) requestFrame();
}

/* ------------------------------------------------------------- the store */
function readFromStore(): FloorRead {
  const sources: FloorSource[] = [];
  const seen = new Set<number>();
  for (const run of store.runs) {
    const detail = store.detailOf(run);
    for (const vm of detail.vms) {
      const role = roleOf(vm);
      if (role == null) continue;
      const cid = vm.guest_cid ?? null;
      if (cid != null) seen.add(cid);
      sources.push({
        cid,
        role,
        runId: run.id,
        vmId: vm.id,
        lines: store.linesFor(run.id, vm.id, vm.serial).length,
        lps: store.activity.get(vmKey(run.id, vm.id)),
        vmState: vm.state,
      });
    }
  }
  if (store.hubError == null && store.hub != null) {
    for (const agent of store.hub.agents) {
      if (seen.has(agent.cid)) continue;
      seen.add(agent.cid);
      sources.push({ cid: agent.cid, role: agent.role, runId: null, vmId: null, lines: 0, lps: 0, vmState: null });
    }
  }
  const connected = store.hubError == null && store.hub != null ? new Set(store.hub.agents.map((a) => a.cid)) : null;
  const hires: HireLink[] = hireLinks(store.board, store.tasks);
  const social = { conversations: store.conversations, pairs: store.convPairs, active_secs: store.convActiveSecs };
  return readFloor(sources, connected, social, hires, Date.now());
}

/** The name a CID answers to in the floor's own prose: its role, else the CID. */
function nameOf(cid: number): string {
  const robot = plan?.robots.find((r) => r.source.cid === cid);
  return robot ? robot.source.role : `cid ${cid}`;
}

/** Reconcile the whole view with the store: the plan, the ground, the robots,
    the ground wires, the camera's frame, and the three DOM surfaces — the
    status line, the ruled table and the card. */
export function syncFloor(): void {
  if (store.route.name !== 'floor') return;
  const s = scene3;
  if (!s) {
    renderStatus();
    void ensureScene().then(() => {
      renderStatus();
      if (failed) showFailure(failed);
    });
    return;
  }
  const p = palette();
  s.scene.background = new s.three.Color(p.paper);
  const next = readFromStore();
  plan = next;
  ensureParts(s.three, s.scene, next.robots.length, next.stations.length);
  const wantGround = `${next.width}x${next.height}:${next.stations.map((st) => `${st.index}/${st.role}`).join(',')}`;
  if (wantGround !== groundKey) {
    groundKey = wantGround;
    rebuildGround(p);
  }
  const now = Date.now();
  syncRobots(now);
  const mount = byId('floor-mount');
  const aspect = mount.clientWidth / Math.max(1, mount.clientHeight);
  if (mount.clientWidth > 0) updateCamera(aspect);
  stepRobots(now, p);
  syncLinks(p);
  flushParts();
  syncCursor(p);
  /* the scene is not drawn while the tab is hidden: the store still paints
     every DOM surface the floor has (the status, the table, the card, the
     live label), and the last drawn frame stays on the canvas until the tab
     comes back — when the visibility listener repaints it. */
  if (mount.clientWidth > 0 && !document.hidden) {
    s.renderer.render(s.scene, s.camera);
    frames++;
  }

  const named: Record<number, string> = {};
  for (const robot of next.robots) if (robot.source.cid != null) named[robot.source.cid] = robot.source.role;
  const label = floorLabel(next, named);
  if (s.renderer.domElement.getAttribute('aria-label') !== label) {
    s.renderer.domElement.setAttribute('aria-label', label);
  }

  renderStatus();
  renderTable(next);
  renderCard(next);
  requestFrame();

  /* one announcement per shape, never per poll: the live label carries the
     fleet, and a screen reader is not made to re-read it every second */
  const shape = next.gatherings.map((g) => `${g.kind}:${g.cids.join('+')}`).join(',');
  if (shape !== talkSig) {
    talkSig = shape;
    if (next.gatherings.length > 0) announce(label);
  }
}

/* ---------------------------------------------------------------- status */
/** The honest line when the plaza's social layer is not on this node yet: the
    board and the conversations answer 404, and the floor says so rather than
    drawing conversations that are not there. */
function socialMissing(): string | null {
  const boardGone = store.boardError != null && store.boardErrorStatus === 404;
  const convGone = store.conversationsError != null && store.conversationsErrorStatus === 404;
  if (!boardGone && !convGone) return null;
  return (
    'The plaza’s social layer — the board, the contacts and the conversations — is not on this node yet ' +
    '(the console’s reads answer 404). The robots stand at their stations from the register and the hub alone.'
  );
}

function renderStatus(): void {
  if (store.route.name !== 'floor') return;
  const notice = byId('floor-notice');
  if (store.runsError != null && store.runs.length === 0) {
    notice.hidden = false;
    byId('floor-notice-text').textContent =
      `The node daemon is not answering at ${location.origin} (${store.runsError}). Retrying; the floor catches up on its own.`;
  } else {
    notice.hidden = true;
  }
  const missing = socialMissing();
  byId('floor-social').hidden = missing == null;
  if (missing != null) byId('floor-social-text').textContent = missing;
  const live = byId('floor-live');
  live.hidden = store.health !== 'live';
  live.textContent = store.health;
  if (plan) byId('floor-count').textContent = floorCountLine(plan);

  const status = byId('floor-status');
  if (!plan) {
    status.textContent = 'Reading the register and the hub…';
    return;
  }
  const focused =
    document.activeElement === scene3?.renderer.domElement ? plan.robots[focusIdx] : undefined;
  if (focused) {
    status.textContent =
      `${focused.source.role} · cid ${focused.source.cid ?? '—'} · ${ROBOT_STATE_LABEL[focused.state]}` +
      ` · station ${focused.station.index + 1} · Enter opens its card`;
    return;
  }
  const where = !plan.hubReadable
    ? 'the hub could not be read, so no robot reads unreachable'
    : plan.counts.unreachable > 0
      ? `${plan.counts.unreachable} not connected to the hub`
      : 'every robot is connected to the hub';
  const talks = plan.gatherings.length;
  status.textContent =
    `${plan.robots.length} robots on ${plan.stations.length} stations · ${where}` +
    (talks > 0 ? ` · ${talks} ${talks === 1 ? 'gathering' : 'gatherings'}` : '') +
    ' · drag to pan, wheel to zoom, arrow keys walk the robots';
}

function showFailure(why: string): void {
  const notice = byId('floor-notice');
  notice.hidden = false;
  byId('floor-notice-mk').textContent = 'no webgl2';
  const text = byId('floor-notice-text');
  text.textContent = '';
  text.append(`The plaza floor needs WebGL2, which this browser refused (${why}). The `);
  const a = document.createElement('a');
  a.href = '#map';
  a.textContent = '2D map';
  text.appendChild(a);
  text.append(' carries the fleet, and the table below carries the agents.');
  byId('floor-status').textContent = 'WebGL2 unavailable — the plan is a table.';
}

/* ------------------------------------------------------------------ table */
/** The floor as a ruled table: the same robots, the same states, reachable by
    keyboard — each row selects its robot. The register's own answer to a
    drawing: the plan is the shape, the table is the data. */
function renderTable(read: FloorRead): void {
  const sorted = read.robots
    .slice()
    .sort((a, b) => a.station.index - b.station.index || (a.source.cid ?? 1e9) - (b.source.cid ?? 1e9));
  const sig = sorted.map((r) => `${r.key}/${r.station.index}/${r.slot}`).join('|');
  const body = byId('floor-body');
  if (sig !== tableSig) {
    tableSig = sig;
    body.textContent = '';
    tableRows.clear();
    sorted.forEach((robot, n) => {
      const tr = el('tr', 'link');
      tr.tabIndex = 0;
      if (robot.source.cid != null) tr.setAttribute('data-robot', String(robot.source.cid));
      tr.setAttribute(
        'aria-label',
        `${robot.source.role}, cid ${robot.source.cid ?? 'none'}, ${ROBOT_STATE_LABEL[robot.state]}`,
      );
      tr.appendChild(el('td', null, String(n + 1)));
      tr.appendChild(el('td', 'm', `${robot.station.index + 1} · ${robot.station.role}`));
      const roleTd = el('td');
      roleTd.appendChild(el('span', 'chip role', robot.source.role));
      tr.appendChild(roleTd);
      tr.appendChild(el('td', 'm', robot.source.cid != null ? `cid ${robot.source.cid}` : '—'));
      const stateTd = el('td');
      tr.appendChild(stateTd);
      const linesTd = el('td', 'num');
      tr.appendChild(linesTd);
      body.appendChild(tr);
      tableRows.set(robot.key, { state: stateTd, lines: linesTd, tr });
    });
  }
  for (const robot of sorted) {
    const row = tableRows.get(robot.key);
    if (!row) continue;
    row.state.textContent = '';
    row.state.appendChild(stateMark(robot.state));
    row.lines.textContent = robot.source.lps > WORKING_LPS ? `${robot.source.lps.toFixed(1)} lines/s` : '—';
    row.tr.classList.toggle('sel', robot.source.cid != null && robot.source.cid === selected);
  }
}

/* ------------------------------------------------------------------- card */
const CONTACT_LABEL: Readonly<Record<ContactKind, string>> = {
  met: 'met',
  hired: 'hired',
  'was-hired': 'was hired by',
};

function stateMark(state: RobotState): HTMLElement {
  switch (state) {
    case 'working':
      return el('span', 'mk ok', 'working');
    case 'talking':
      return el('span', 'mk', 'talking');
    case 'unreachable':
      return el('span', 'mk crit', 'unreachable');
    default:
      return el('span', 'mk', 'idle');
  }
}

/** A `label ........ value` row for the card's ruled facts. */
function fact(dt: string, dd: Node | string): HTMLElement {
  const row = el('div', 'lead');
  row.appendChild(el('dt', null, dt));
  row.appendChild(el('i'));
  const cell = el('dd', 'm');
  if (typeof dd === 'string') cell.textContent = dd;
  else cell.appendChild(dd);
  row.appendChild(cell);
  return row;
}

function linkTo(href: string, text: string): HTMLAnchorElement {
  const a = document.createElement('a');
  a.href = href;
  a.textContent = text;
  return a;
}

/** A peer row (a contact, or a pair talking now): who, how they are linked,
    and a button that asks the pair API for the thread. */
function peerRow(kind: string, role: string, meta: string, peer: number): HTMLLIElement {
  const row = el('li', 'peer');
  row.appendChild(el('span', 'fkind', kind));
  row.appendChild(el('span', 'chip role', role));
  row.appendChild(el('span', 'm', meta));
  const btn = el('button', 'btn sm', 'Read the thread');
  btn.type = 'button';
  btn.setAttribute('data-thread', String(peer));
  row.appendChild(btn);
  return row;
}

function renderContacts(cid: number | null, held: ContactsRead | undefined): void {
  const wrap = byId('fc-contacts');
  const list = byId('fc-contacts-list');
  const note = byId('fc-contacts-note');
  list.textContent = '';
  if (cid == null) {
    wrap.hidden = true;
    return;
  }
  wrap.hidden = false;
  if (held == null || held.state === 'loading') {
    note.textContent = 'reading the agent’s contacts…';
    return;
  }
  if (held.state === 'error') {
    note.textContent = held.error;
    return;
  }
  note.textContent =
    held.contacts.length === 0
      ? 'This agent has met no one yet — the board is where it meets them.'
      : `${fmtNum(held.contacts.length)}${held.truncated ? ` of ${fmtNum(held.total)}` : ''} in its own list, oldest first.`;
  for (const contact of held.contacts) {
    list.appendChild(
      peerRow(
        CONTACT_LABEL[contact.kind],
        contact.role,
        `cid ${contact.peer_cid} · ${fmtStamp(contact.since)}`,
        contact.peer_cid,
      ),
    );
  }
}

/** The active pairs a CID is in, from the store's own view of the social
    layer: the node's pairs when it sends them, the exchanges when it does
    not. */
function pairsWith(cid: number): ReturnType<typeof activePairs> {
  return activePairs(
    { pairs: store.convPairs, conversations: store.conversations, active_secs: store.convActiveSecs },
    Date.now(),
  ).filter((p) => p.a === cid || p.b === cid);
}

function renderConversations(cid: number | null): void {
  const wrap = byId('fc-convs');
  const list = byId('fc-convs-list');
  list.textContent = '';
  if (cid == null) {
    wrap.hidden = true;
    return;
  }
  const pairs = pairsWith(cid);
  wrap.hidden = false;
  byId('fc-convs-note').textContent =
    pairs.length === 0
      ? `No conversation is active — a pair stays active for ${fmtNum(store.convActiveSecs)} s after its last exchange.`
      : `${pairs.length} active now.`;
  for (const pair of pairs) {
    const peer = pair.a === cid ? pair.b : pair.a;
    list.appendChild(
      peerRow(
        'talking',
        nameOf(peer),
        `cid ${peer} · ${fmtNum(pair.exchanges)} exchange${pair.exchanges === 1 ? '' : 's'} · last ${fmtStamp(pair.last_at)}`,
        peer,
      ),
    );
  }
}

function renderPost(post: BoardPost | null): void {
  const wrap = byId('fc-post');
  wrap.hidden = post == null;
  if (!post) return;
  byId('fc-post-subject').textContent = post.subject;
  byId('fc-post-body').textContent = post.body;
  const meta = byId('fc-post-meta');
  meta.textContent = '';
  meta.appendChild(document.createTextNode(`${post.id} · ${fmtStamp(post.created_at)}`));
  if (post.task != null) {
    meta.appendChild(document.createTextNode(' · '));
    meta.appendChild(linkTo(`#ledger/${encodeURIComponent(post.task)}`, post.task));
  }
}

function renderThread(cid: number | null, thread: ThreadRead | null): void {
  const wrap = byId('fc-thread');
  const mine = thread != null && cid != null && (thread.a === cid || thread.b === cid);
  wrap.hidden = !mine;
  if (!mine || thread == null) return;
  byId('fc-thread-who').textContent = `cid ${thread.a} ↔ cid ${thread.b}`;
  byId('fc-thread-note').textContent =
    thread.state === 'loading'
      ? 'reading the pair’s thread…'
      : thread.state === 'error'
        ? thread.error
        : `${fmtNum(thread.exchanges.length)}${thread.truncated ? ` of ${fmtNum(thread.total)}` : ''} exchanges, newest first.`;
  const list = byId('fc-thread-list');
  list.textContent = '';
  for (const exchange of thread.exchanges.slice(0, THREAD_TAIL)) {
    const li = el('li', 'fex');
    li.appendChild(el('span', 'm', `${exchange.from_cid} → ${exchange.to_cid}`));
    li.appendChild(el('span', 'fextext', exchange.text));
    const tail = deliveryText(exchange);
    if (tail != null) li.appendChild(el('span', 'mk warn', tail));
    li.appendChild(el('span', 'm', fmtStamp(exchange.created_at)));
    list.appendChild(li);
  }
}

function renderCard(read: FloorRead): void {
  const card = byId('floor-card');
  const robot = selected == null ? null : (read.robots.find((r) => r.source.cid === selected) ?? null);
  const empty = byId('floor-card-empty');
  if (!robot) {
    empty.hidden = false;
    card.hidden = true;
    return;
  }
  empty.hidden = true;
  card.hidden = false;
  const cid = robot.source.cid;

  const roleSlot = byId('fc-role');
  roleSlot.textContent = '';
  roleSlot.appendChild(el('span', 'chip role', robot.source.role));
  byId('fc-cid').textContent = cid != null ? `cid ${cid}` : 'no vsock cid';
  const stateSlot = byId('fc-state');
  stateSlot.textContent = '';
  stateSlot.appendChild(stateMark(robot.state));

  const facts = byId('fc-facts');
  facts.textContent = '';
  facts.appendChild(fact('station', `${robot.station.index + 1} of ${read.stations.length} · ${robot.station.role}`));
  facts.appendChild(fact('lines / s', robot.source.lps > WORKING_LPS ? robot.source.lps.toFixed(1) : '— quiet'));
  facts.appendChild(fact('serial lines', fmtNum(robot.source.lines)));
  if (robot.source.runId != null && robot.source.vmId != null) {
    const runId = robot.source.runId;
    facts.appendChild(
      fact('register', linkTo(`#run/${encodeURIComponent(runId)}`, `run ${runId} · vm ${robot.source.vmId}`)),
    );
    const run = store.runs.find((r) => r.id === runId);
    facts.appendChild(fact('up since', run ? fmtStamp(run.created_at) : '—'));
  } else {
    facts.appendChild(fact('register', 'connected to the hub, not on the register'));
    facts.appendChild(fact('up since', '—'));
  }
  const peers = cid == null ? [] : pairsWith(cid).map((pair) => (pair.a === cid ? pair.b : pair.a));
  facts.appendChild(
    fact(
      'talking with',
      peers.length === 0 ? 'no one now' : peers.map((peer) => `cid ${peer} (${nameOf(peer)})`).join(', '),
    ),
  );
  facts.appendChild(
    fact('drive · stop', cid == null ? 'no cid to drive' : linkTo('#roster', `the roster drives cid ${cid}`)),
  );

  const held = cid == null ? undefined : store.contacts.get(cid);
  const post = lastPostBy(store.board, cid);
  const sig = [
    cid,
    held ? `${held.state}/${held.contacts.map((c) => `${c.peer_cid}:${c.kind}:${c.since}`).join(',')}` : 'none',
    post ? `${post.id}/${post.subject}/${post.body.length}` : 'nopost',
    store.thread
      ? `${store.thread.a}-${store.thread.b}/${store.thread.state}/${store.thread.exchanges.length}`
      : 'nothread',
    peers.join(','),
  ].join('|');
  if (sig !== cardSig) {
    cardSig = sig;
    renderContacts(cid, held);
    renderConversations(cid);
    renderPost(post);
    renderThread(cid, store.thread);
  }
}

/* ------------------------------------------------------------- selection */
function selectCid(cid: number | null): void {
  selected = cid;
  if (cid != null && !store.contacts.has(cid)) void store.loadContacts(cid);
  store.scheduleRender();
}

/** Move the camera's cursor to the nearest robot in a direction, from the
    positions actually stood at. The cursor is announced; Enter opens the
    card. */
function moveFocus(dx: number, dy: number): void {
  if (!plan || plan.robots.length === 0) return;
  const current = robots.get(plan.robots[focusIdx]?.key ?? '');
  if (!current) return;
  let best: RobotView | null = null;
  let bestD = Infinity;
  for (const view of robots.values()) {
    if (view.key === current.key) continue;
    const along = dx !== 0 ? (view.x - current.x) * Math.sign(dx) : (view.z - current.z) * Math.sign(dy);
    if (along <= 1) continue;
    const across = Math.abs(dx !== 0 ? view.z - current.z : view.x - current.x);
    const d = along + across * 2.5;
    if (d < bestD) {
      bestD = d;
      best = view;
    }
  }
  if (!best) return;
  focusIdx = best.index;
  hoverKey = null;
  announce(`${best.role}, cid ${best.cid ?? '—'}, ${ROBOT_STATE_LABEL[best.state]}`);
  requestFrame();
}

/* ------------------------------------------------------------ the wiring */
export function initFloor(): void {
  byId('floor-body').addEventListener('click', (e) => {
    const tr = (e.target as HTMLElement | null)?.closest<HTMLElement>('tr[data-robot]');
    if (tr) selectCid(Number(tr.getAttribute('data-robot')));
  });
  byId('floor-body').addEventListener('keydown', (e) => {
    if (e.key !== 'Enter') return;
    const tr = (e.target as HTMLElement | null)?.closest<HTMLElement>('tr[data-robot]');
    if (!tr) return;
    e.preventDefault();
    selectCid(Number(tr.getAttribute('data-robot')));
  });
  byId('floor-card').addEventListener('click', (e) => {
    const btn = (e.target as HTMLElement | null)?.closest<HTMLElement>('[data-thread]');
    if (!btn || selected == null) return;
    const peer = Number(btn.getAttribute('data-thread'));
    if (Number.isInteger(peer)) void store.loadThread(selected, peer);
  });
}

/** Showing the floor re-reconciles it: the mount's width reads 0 while the
    view is hidden, so the camera's frame is recomputed on every enter. */
export function syncFloorVisibility(): void {
  if (store.route.name === 'floor') syncFloor();
}

/** Read-only state for the verification harness; the app never reads it. What
    the scene actually holds — instances, halos, gatherings, the camera — so
    the floor can be read back as drawn, not just as derived. */
export function floorState(): {
  robots: number;
  stations: number;
  gatherings: number;
  states: Record<RobotState, number>;
  selected: number | null;
  walks: number;
  /** Where every robot actually stands on the ground, as drawn. */
  positions: Array<{ cid: number | null; x: number; z: number; gathering: string | null }>;
  tint_cycle: number;
  frames: number;
  draws: number;
  triangles: number;
  webgl2: boolean;
  camera: { zoom: number; pan: [number, number]; view_h: number };
  plan: { width: number; height: number };
  label: string | null;
} | null {
  if (!plan || !scene3) return null;
  const mount = byId('floor-mount');
  const aspect = mount.clientWidth / Math.max(1, mount.clientHeight);
  return {
    robots: plan.robots.length,
    stations: plan.stations.length,
    gatherings: plan.gatherings.length,
    states: { ...plan.counts },
    selected,
    walks: [...robots.values()].filter((r) => r.walk != null).length,
    positions: plan.robots.map((r) => {
      const view = robots.get(r.key);
      return { cid: r.source.cid, x: view?.x ?? 0, z: view?.z ?? 0, gathering: r.gathered };
    }),
    tint_cycle: TINTS.length,
    frames,
    draws: scene3.renderer.info.render.calls,
    triangles: scene3.renderer.info.render.triangles,
    webgl2: scene3.renderer.getContext() instanceof WebGL2RenderingContext,
    camera: { zoom, pan: [pan.x, pan.z], view_h: rtsViewH(plan.width, plan.height, aspect) / zoom },
    plan: { width: plan.width, height: plan.height },
    label: scene3.renderer.domElement.getAttribute('aria-label'),
  };
}

/** The gatherings as the scene drew them — the meeting mark and the world
    point it stands at — for the harness's own assertions. */
export function floorGatherings(): Array<{ kind: GatherKind; cids: number[]; at: [number, number] }> {
  if (!plan) return [];
  return plan.gatherings.map((g) => ({ kind: g.kind, cids: g.cids, at: [worldX(g.x), worldZ(g.y)] }));
}
