/* The floor: the plaza as a 3D real-time-strategy scene. Every connected agent
   is a small articulated robot standing on a glowing station pad, the ground is
   a dark sci-fi plaza ruled into the role clusters, and two agents talking walk
   adjacent and face each other.

   The scene, and nothing in it is decoration that lies about state:
   - a FIXED camera — 57° of tilt above the horizon, 12° of yaw, orthographic
     so the plaza's parallel rulings stay parallel on screen — panned by
     dragging and zoomed by the wheel, both eased with momentum-less damping.
     No orbit, and no idle drift: the ground is a labelled diagram, and a
     drifting frame would slide the plates and rulings under the labels. (The
     report states the same.)
   - the ground is a near-black plaza with a faint scanline texture and an
     EMISSIVE grid whose vertices darken with distance from the centre; a soft
     fog (_fog hue = the horizon glow) eats the far edges, a gradient sky dome
     and a starfield sit above, low pylons ring the plaza, and two role-tinted
     point lights drift over it.
   - each robot stands on a hex pad with an emissive role-tinted ring; its
     status ring on the ground is emissive and animated (rates below).
   - a conversation walks the pair adjacent, eased, and turns them to face each
     other; a hiring floats a contract glyph over the pair. Reduced motion never
     walks and never animates: every effect collapses to its end state.
   - the robots are drawn with INSTANCED meshes: one geometry per part, one
     instance per agent, a per-instance tint, and no allocation in the frame
     path — a robot that walks costs one matrix write per part, which is why a
     plaza of hundreds stays at frame rate.

   The rig (13 primitives per robot; ~4 more instanced effect parts):
     torso · head · visor · visor scan bar · arm L/R · leg L/R · foot L/R ·
     antenna stem · antenna tip. Effects: status ring · ticks/dashes · working
     sparks · working sweep · speech bubble · bubble tail · station pads ·
     pad rings.
   The animation maths lives in rts-anim.ts (DOM-free, unit-tested): the idle
   breath, the walk cycle, the talk gesture, the spawn/leave scaling, the ring
   pulses, the sweep, the sparks.

   Picking is a raycast against the robot instances; hover, click, the arrow
   keys and the ruled table all move the selection, and the card below reads the
   selected robot's own facts, its contacts, the threads it holds and the last
   thing it posted. Driving stays in the roster: the card links there. The
   canvas is one image to a screen reader (role=img) with a live label naming
   the fleet and the conversations, and the table beneath carries the same data. */

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
  PointLight,
  Points,
  Quaternion,
  Raycaster,
  Scene,
  Sprite,
  Texture,
  Vector2,
  Vector3,
  WebGLRenderer,
} from 'three';
import { announce, byId, el } from '../dom';
import { fmtNum, fmtStamp } from '../format';
import { vmKey } from '../models/activity';
import type { FloorRead, FloorSource, GatherKind, Gathering, HireLink, Robot, RobotState } from '../models/floor';
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
import type { ActivePair, BoardPost, ContactKind } from '../models/social';
import { activePairs, deliveryText } from '../models/social';
import type { ContactsRead, ThreadRead } from '../store';
import { store } from '../store';
import {
  BUBBLE_MS,
  damp,
  idleBob,
  antennaSway,
  bubblePop,
  leaveScale,
  pulseHz,
  ringPulse,
  sparkLife,
  spawnRing,
  spawnScale,
  sweepFrac,
  talkGesture,
  visorScan,
  walkArm,
  walkBob,
  walkSwing,
} from './rts-anim';

/* The runtime module arrives by a DYNAMIC import on purpose: three.js is the
   console's one heavy dependency, and a static import would carry it into the
   eager bundle every view pays for. `await import('three')` makes esbuild
   emit it as a lazy chunk that only the floor's route ever fetches. */
type ThreeModule = typeof import('three');

const REDUCED = typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;

/* ------------------------------------------------------------- the palette
   The floor is a viewport into a place, drawn dark in both themes; the sheet
   around it keeps the theme. The semantic colours are the NIGHT theme's own
   tokens, so the scene and the sheet agree on what green and red mean, and
   orange stays the owner's hand alone. */
const C = {
  sky: 0x05070e,
  horizon: 0x0d1f33,
  haze: 0x081019,
  ground: 0x0c1018,
  grid: 0x35b3c4,
  ink: 0xe4e8e1,
  ink3: 0x6f786f,
  ok: 0x86c995,
  crit: 0xff7c86,
  warn: 0xe2bd5a,
  signal: 0xff7a2e,
  key: 0xd7e8ff,
  accentA: 0x38bdf8,
  accentB: 0x8b5cf6,
  star: 0xbfd4e6,
} as const;

/* Role tints: a cool cycle, so a role always wears the same colour and no tint
   is the owner's orange. The pad ring, the body panels and the visor wear it. */
const ROLE_TINTS = [0x2dd4bf, 0x60a5fa, 0xa78bfa, 0x93a3b8] as const;

/* the canvas-2d labels and plates need CSS colour strings, not the hex ints */
const CSS_INK = '#e4e8e1';
const CSS_INK2 = '#a6aea5';
const CSS_GROUND = '#05070e';

function roleTint(index: number): number {
  return ROLE_TINTS[((index % ROLE_TINTS.length) + ROLE_TINTS.length) % ROLE_TINTS.length] ?? ROLE_TINTS[0];
}

/* ------------------------------------------------------------ world units
   The plan is 1:1 with floor units and centred on the origin; the feet stand
   on the ground (y=0) and every local coordinate below is in robot-local space
   with the feet at y=0. */
const PAD_TILE_H = 1.0; // the hex pad's tile height
const RING_IN = 7.6;
const RING_OUT = 9.4;
const RING_Y = PAD_TILE_H + 0.18;
const HIP_Y = 9.6;
const HIP_X = 3.4;
const LEG_LEN = 8.4;
const SHOULDER_Y = 18.6;
const SHOULDER_X = 5.0;
const ARM_LEN = 7.6;
const TORSO_Y = 14.6;
const HEAD_Y = 21.6;
const VISOR_Y = 21.9;
const VISOR_Z = 2.7;
const STEM_BASE_Y = 24.0;
const ANTENNA_LEN = 4.6;
const TICKS_PER_WORKING = 6;
const DASHES_PER_LOST = 12;
const MARK_CAPACITY = 12; // the most marks one robot ever draws (a lost ring's dashes)
const SPARK_CAPACITY = 4;
const GHOST_CAPACITY = 32; // the leaving robots still scaling out
const ARC_SEG = 14; // segments in a conversation's link arc
const STRIDE_MS = 300; // one stride of the walk cycle
const WALK_MS_PER_UNIT = 9;
const WALK_MS_MIN = 260;
const WALK_MS_MAX = 900;
const YAW_HALF_LIFE = 130; // the turn toward the travel, in ms
const FOLLOW_HALF_LIFE = 460;
const THREAD_TAIL = 20; // exchanges the card's thread shows
const PLATE_SCALE = 0.5; // world units per plate pixel

/** The instanced parts every robot and every effect is drawn from: one geometry
    per part, one instance per agent (the effects carry a per-agent sub-row).
    The shared geometry and the per-instance tint are what keep the frame path
    allocation-free. */
interface Parts {
  torso: InstancedMesh;
  head: InstancedMesh;
  visor: InstancedMesh;
  scan: InstancedMesh;
  armL: InstancedMesh;
  armR: InstancedMesh;
  legL: InstancedMesh;
  legR: InstancedMesh;
  footL: InstancedMesh;
  footR: InstancedMesh;
  stem: InstancedMesh;
  tip: InstancedMesh;
  rings: InstancedMesh;
  spawnRings: InstancedMesh;
  marks: InstancedMesh;
  sparks: InstancedMesh;
  sweeps: InstancedMesh;
  bubbles: InstancedMesh;
  bubbleTails: InstancedMesh;
  pads: InstancedMesh;
  padRings: InstancedMesh;
  /** every per-frame mesh, in flush order (preallocated: no array per frame) */
  robot: InstancedMesh[];
  /** the meshes rewritten only when the layout changes */
  ground: InstancedMesh[];
  /** live robot slots (ghosts draw past this, in `slots`) */
  capacity: number;
  slots: number;
  stations: number;
}

/** The scratch objects the frame path reuses. Nothing in a frame allocates. */
interface Scratch {
  v1: Vector3;
  v2: Vector3;
  v3: Vector3;
  uv: Vector2;
  q1: Quaternion;
  q2: Quaternion;
  q3: Quaternion;
  m1: Matrix4;
  m2: Matrix4;
  m3: Matrix4;
  c1: Color;
  c2: Color;
  /** the three axis unit vectors, so no axis literal is built in a frame */
  ax: Vector3;
  ay: Vector3;
  az: Vector3;
}

/** One gathering's wire (a conversation's arc) or glyph (a hiring's contract
    mark and its celebration ring), rebuilt only when the gathering set
    changes and rewritten in place while its members walk. */
interface LinkView {
  cids: number[];
  line: Line | null;
  positions: Float32Array | null;
  glyph: Mesh | null;
  ring: Mesh | null;
  t0: number;
}

/** A robot's drawing instance: its facts, its live position and facing, the
    walk in flight, the instance slot it draws into, and its animation clocks. */
interface RobotView {
  key: string;
  index: number;
  role: string;
  cid: number | null;
  state: RobotState;
  tint: number;
  /** where it stands and faces now (eased while walking) */
  x: number;
  z: number;
  yaw: number;
  /** where the plan says it belongs, and the walk in flight toward it */
  tx: number;
  tz: number;
  walk: { fx: number; fz: number; t0: number; dur: number } | null;
  /** the animation clocks: per-robot phase, spawn, leave, talk */
  phase: number;
  spawnT0: number;
  leftT0: number | null;
  talkT0: number | null;
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
  sky: Mesh;
  stars: PointsHandle;
  links: Group;
  cursor: Group;
  cursorSel: InstancedMesh;
  cursorFocus: InstancedMesh;
  label: Sprite;
  lights: { a: PointLight; b: PointLight };
}

/** The starfield's handle, kept so the frame path can scale the dome with the
    camera without touching the Points' geometry. */
interface PointsHandle {
  points: Points;
  scale: number;
}

let scene3: Scene3 | null = null;
let starting: Promise<void> | null = null;
let failed: string | null = null;

let plan: FloorRead | null = null;
const robots = new Map<string, RobotView>();
const byCid = new Map<number, RobotView>();
/** the drawn slots by instance index (index i < capacity is the plan's robot i;
    past that, the ghost pool). Rebuilt whenever the parts are. */
let slots: Array<RobotView | null> = [];
let order: Robot[] = [];
let groundKey = '';
let linkKey = '';
let links: LinkView[] = [];
let ghostAt = 0;
let selected: number | null = null;
let follow = false;
let focusIdx = 0;
let hoverKey: string | null = null;
let zoom = 1;
let zoomTarget = 1;
const pan = { x: 0, z: 0 };
const panTarget = { x: 0, z: 0 };
let dragging: { x: number; y: number; moved: number } | null = null;
let suppressClick = false;
let raf: number | null = null;
let lastNow = 0;
let fpsShown = 0;
let msShown = 0;
let fpsAt = 0;
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
  scene.background = new THREE.Color(C.sky);
  scene.fog = new THREE.Fog(C.haze, 800, 2400);
  const camera = new THREE.OrthographicCamera(-1, 1, 1, -1, 0.1, 6000);

  /* Lighting: a hemisphere fill, one directional key light, and two role-tinted
     point lights that drift over the plaza (their motion is the scene's own
     ambient life; it is off under reduced motion). The point lights are
     physically-correct (intensity in candela, decay 2), so their intensities are
     large and their reach short — they tint the pads and robots near them. */
  scene.add(new THREE.HemisphereLight(0x3d5a7a, 0x0a1018, 1.05));
  const key = new THREE.DirectionalLight(C.key, 2.0);
  key.position.set(-180, 380, 240);
  scene.add(key);
  const accentA = new THREE.PointLight(C.accentA, 16000, 1600, 2);
  const accentB = new THREE.PointLight(C.accentB, 14000, 1600, 2);
  scene.add(accentA);
  scene.add(accentB);

  const sky = makeSky(THREE);
  scene.add(sky);
  const starPts = makeStars(THREE);
  scene.add(starPts);

  const ground = new THREE.Group();
  const linksGroup = new THREE.Group();
  const cursor = new THREE.Group();
  scene.add(ground);
  scene.add(linksGroup);
  scene.add(cursor);

  const ringsMat = new THREE.MeshBasicMaterial({ color: 0xffffff, side: THREE.DoubleSide, transparent: true });
  const cursorSel = new THREE.InstancedMesh(new THREE.RingGeometry(RING_OUT + 2.6, RING_OUT + 4.6, 48), ringsMat, 1);
  const cursorFocus = new THREE.InstancedMesh(
    new THREE.RingGeometry(RING_OUT + 6.8, RING_OUT + 7.8, 48),
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
    v3: new THREE.Vector3(),
    uv: new THREE.Vector2(),
    q1: new THREE.Quaternion(),
    q2: new THREE.Quaternion(),
    q3: new THREE.Quaternion(),
    m1: new THREE.Matrix4(),
    m2: new THREE.Matrix4(),
    m3: new THREE.Matrix4(),
    c1: new THREE.Color(),
    c2: new THREE.Color(),
    ax: new THREE.Vector3(1, 0, 0),
    ay: new THREE.Vector3(0, 1, 0),
    az: new THREE.Vector3(0, 0, 1),
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
    sky,
    stars: { points: starPts, scale: 1 },
    links: linksGroup,
    cursor,
    cursorSel,
    cursorFocus,
    label,
    lights: { a: accentA, b: accentB },
  };

  renderer.domElement.addEventListener('click', onClick);
  renderer.domElement.addEventListener('mousemove', onHover);
  renderer.domElement.addEventListener('mouseleave', onLeave);
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

/** The gradient sky dome: a large back-side sphere whose vertices carry the
    theme's night colours, brightest in a band at the horizon (the glow). */
function makeSky(THREE: ThreeModule): Mesh {
  const geo = new THREE.SphereGeometry(1, 32, 20);
  const pos = geo.getAttribute('position');
  const colors = new Float32Array(pos.count * 3);
  const top = new THREE.Color(C.sky);
  const horizon = new THREE.Color(C.horizon);
  const tmp = new THREE.Color();
  for (let i = 0; i < pos.count; i++) {
    const y = pos.getY(i);
    const glow = Math.exp(-(y * y) / 0.05);
    tmp.copy(top).lerp(horizon, glow);
    colors[i * 3] = tmp.r;
    colors[i * 3 + 1] = tmp.g;
    colors[i * 3 + 2] = tmp.b;
  }
  geo.setAttribute('color', new THREE.BufferAttribute(colors, 3));
  const sky = new THREE.Mesh(
    geo,
    new THREE.MeshBasicMaterial({ vertexColors: true, side: THREE.BackSide, fog: false, depthWrite: false }),
  );
  sky.renderOrder = -2;
  sky.frustumCulled = false;
  return sky;
}

/** A static starfield on the upper dome, drawn unlit and unfogged. */
function makeStars(THREE: ThreeModule): Points {
  const COUNT = 700;
  const pts = new Float32Array(COUNT * 3);
  /* a small deterministic hash so the field is the same on every load */
  let seed = 0x9e3779b9;
  const rnd = (): number => {
    seed = (seed * 1664525 + 1013904223) >>> 0;
    return seed / 0xffffffff;
  };
  for (let i = 0; i < COUNT; i++) {
    const theta = rnd() * Math.PI * 2;
    const y = 0.06 + rnd() * 0.94;
    const r = Math.sqrt(1 - y * y);
    pts[i * 3] = Math.cos(theta) * r;
    pts[i * 3 + 1] = y;
    pts[i * 3 + 2] = Math.sin(theta) * r;
  }
  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.BufferAttribute(pts, 3));
  const stars = new THREE.Points(
    geo,
    new THREE.PointsMaterial({ color: C.star, size: 2.4, sizeAttenuation: false, transparent: true, opacity: 0.8, fog: false }),
  );
  stars.renderOrder = -1;
  stars.frustumCulled = false;
  return stars;
}

/** Build the instanced parts for a fleet of `n` agents on `m` stations: the
    thirteen rig primitives plus the effect rows, one instance per agent (or per
    agent per row). A plan that outgrows the capacity is rebuilt (ensureParts),
    never overrun. */
function buildParts(THREE: ThreeModule, scene: Scene, n: number, m: number): Parts {
  const slots = n + GHOST_CAPACITY;
  /* Robot structure reads as brushed metal (metalness 0.35, roughness 0.55) and
     takes its per-instance role tint. The glow parts (visor, tip, rings, marks,
     sparks, sweep, bubbles) are UNLIT (MeshBasicMaterial) — they read as
     emissive panels without paying for lights. */
  const solid = (geo: BufferGeometry): InstancedMesh => {
    const mesh = new THREE.InstancedMesh(
      geo,
      new THREE.MeshStandardMaterial({ color: 0xffffff, metalness: 0.35, roughness: 0.55, emissive: 0x14202c, emissiveIntensity: 0.7 }),
      slots,
    );
    mesh.instanceMatrix.setUsage(THREE.DynamicDrawUsage);
    mesh.frustumCulled = false;
    scene.add(mesh);
    return mesh;
  };
  const glow = (geo: BufferGeometry, capacity: number): InstancedMesh => {
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
  const torso = solid(new THREE.BoxGeometry(8.6, 9.6, 5.6));
  const head = solid(new THREE.BoxGeometry(6.2, 4.8, 5.2));
  const visor = glow(new THREE.BoxGeometry(4.8, 1.7, 0.7), slots);
  const scan = glow(new THREE.BoxGeometry(1.0, 1.9, 0.5), slots);
  const armL = solid(new THREE.BoxGeometry(2.5, ARM_LEN, 2.5));
  const armR = solid(new THREE.BoxGeometry(2.5, ARM_LEN, 2.5));
  const legL = solid(new THREE.BoxGeometry(2.6, LEG_LEN, 2.6));
  const legR = solid(new THREE.BoxGeometry(2.6, LEG_LEN, 2.6));
  const footL = solid(new THREE.BoxGeometry(3.4, 1.5, 5.4));
  const footR = solid(new THREE.BoxGeometry(3.4, 1.5, 5.4));
  const stem = solid(new THREE.CylinderGeometry(0.36, 0.36, ANTENNA_LEN, 6));
  const tip = glow(new THREE.SphereGeometry(1.0, 8, 6), slots);
  const rings = glow(new THREE.RingGeometry(RING_IN, RING_OUT, 44), slots);
  const spawnRings = glow(new THREE.RingGeometry(RING_IN - 1, RING_OUT + 3, 44), slots);
  const marks = glow(new THREE.BoxGeometry(2.4, 0.34, 1), slots * MARK_CAPACITY);
  const sparks = glow(new THREE.BoxGeometry(1.1, 1.1, 1.1), slots * SPARK_CAPACITY);
  const sweeps = glow(new THREE.RingGeometry(RING_IN - 1.6, RING_OUT + 1.6, 18, 1, 0, 0.7), slots);
  const bubbles = glow(new THREE.SphereGeometry(1, 12, 8), slots);
  const bubbleTails = glow(new THREE.ConeGeometry(1.1, 2.2, 4), slots);

  /* the station pads: one hex tile + one emissive ring per RIG (a pad per
     agent's home, rebuilt with the ground) */
  const pads = new THREE.InstancedMesh(
    new THREE.CylinderGeometry(13, 13, PAD_TILE_H, 6),
    new THREE.MeshStandardMaterial({ color: 0xffffff, metalness: 0.5, roughness: 0.42, emissive: 0x101a26, emissiveIntensity: 0.5 }),
    Math.max(1, n),
  );
  pads.instanceMatrix.setUsage(THREE.DynamicDrawUsage);
  pads.frustumCulled = false;
  scene.add(pads);
  const padRings = glow(new THREE.RingGeometry(8.6, 9.6, 40), Math.max(1, n));

  return {
    torso, head, visor, scan, armL, armR, legL, legR, footL, footR, stem, tip,
    rings, spawnRings, marks, sparks, sweeps, bubbles, bubbleTails, pads, padRings,
    robot: [
      torso, head, visor, scan, armL, armR, legL, legR, footL, footR, stem, tip,
      rings, spawnRings, marks, sparks, sweeps, bubbles, bubbleTails,
    ],
    ground: [pads, padRings],
    capacity: n,
    slots,
    stations: m,
  };
}

/** A plan that outgrew the instance capacity gets fresh parts: the old ones are
    disposed and every robot is marked to redraw into the new slots. */
function ensureParts(THREE: ThreeModule, scene: Scene, n: number, m: number): void {
  const parts = scene3?.parts;
  if (!parts || !scene3) return;
  if (n <= parts.capacity && m <= parts.stations) return;
  for (const mesh of parts.robot) {
    scene.remove(mesh);
    mesh.geometry.dispose();
    const mat = mesh.material;
    for (const one of Array.isArray(mat) ? mat : [mat]) one.dispose();
  }
  for (const mesh of parts.ground) {
    scene.remove(mesh);
    mesh.geometry.dispose();
    const mat = mesh.material;
    for (const one of Array.isArray(mat) ? mat : [mat]) one.dispose();
  }
  scene3.parts = buildParts(THREE, scene, Math.max(n, parts.capacity), Math.max(m, parts.stations));
  slots = new Array<RobotView | null>(scene3.parts.slots).fill(null);
  ghostAt = 0;
}

function resize(): void {
  if (!scene3) return;
  const mount = byId('floor-mount');
  const w = mount.clientWidth;
  const h = mount.clientHeight;
  if (w === 0 || h === 0) return;
  /* the drawing buffer follows the mount; the CSS size stays the stylesheet's
     (`width/height: 100%`), so a late layout change can blur a frame but never
     leave the canvas short of the stage — the ResizeObserver re-syncs it */
  scene3.renderer.setSize(w, h, false);
  if (plan) updateCamera(w / h);
}

/** Aim the fixed RTS camera at the plan: the frustum holds the whole plaza at
    zoom 1, and the eased pan moves the aim over the ground. Never an orbit.
    The sky and stars scale to the frustum's own distance, so the dome always
    encloses the view whatever the plan's size. */
function updateCamera(aspect: number): void {
  const s = scene3;
  if (!s || !plan) return;
  const { camera, sky, stars } = s;
  const frame = rtsFraming(plan.width, plan.height, aspect, zoom);
  const view = frame.view_h;
  camera.left = (-view * aspect) / 2;
  camera.right = (view * aspect) / 2;
  camera.top = view / 2;
  camera.bottom = -view / 2;
  const dist = Math.hypot(frame.pos[0], frame.pos[1], frame.pos[2]);
  const skyR = dist * 2.4;
  camera.near = 0.1;
  camera.far = skyR + dist + view * 2;
  camera.position.set(frame.pos[0] + pan.x, frame.pos[1], frame.pos[2] + pan.z);
  camera.lookAt(pan.x, 0, pan.z);
  camera.updateProjectionMatrix();
  sky.scale.setScalar(skyR);
  stars.points.scale.setScalar(skyR * 0.96);
  stars.scale = skyR * 0.96;
  const fog = s.scene.fog;
  if (fog instanceof s.three.Fog) {
    fog.near = dist * 0.95;
    fog.far = dist * 2.8;
  }
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

/** A station's plate: its monogram in the role's tint square, then the role's
    name — drawn on a canvas texture and laid flat on the ground like a map
    label. */
function makePlate(role: string, tint: Color): Mesh {
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
  g.fillStyle = CSS_GROUND;
  g.fillText(roleMonogram(role), 9, 10);
  g.textAlign = 'left';
  g.font = font;
  g.fillStyle = CSS_INK2;
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

/* ------------------------------------------------------------ the ground
   Rebuild the plaza for a plan: the near-black ground with its scanline
   texture, the emissive grid (its vertices darkened with distance), the
   role-cluster ruling, a hex pad and an emissive ring per robot's station, a
   plate per role, and the perimeter pylons. Rebuilt only when the layout
   itself changes — the ground does not move when a robot does. */
function rebuildGround(): void {
  const s = scene3;
  if (!s || !plan) return;
  const THREE = s.three;
  for (const child of [...s.ground.children]) {
    s.ground.remove(child);
    disposeDeep(child);
  }
  const { width, height, stations } = plan;
  const spanW = width + 1600;
  const spanH = height + 1600;

  /* the ground: near-black, with a faint scanline texture repeated across it */
  const plane = new THREE.Mesh(
    new THREE.PlaneGeometry(spanW, spanH),
    new THREE.MeshStandardMaterial({ color: C.ground, map: scanTexture(THREE, spanW, spanH), metalness: 0.1, roughness: 0.92 }),
  );
  plane.rotation.x = -Math.PI / 2;
  plane.renderOrder = -1;
  s.ground.add(plane);

  /* the emissive grid: parallel rulings whose colour dies toward the edges, so
     the plaza glows at its heart and fades into the haze */
  const grid: number[] = [];
  const gcol: number[] = [];
  const near = new THREE.Color(C.grid);
  const far = new THREE.Color(C.ground);
  const gtmp = new THREE.Color();
  const fade = (x: number, z: number): void => {
    const d = Math.hypot(x / (width / 2 + 500), z / (height / 2 + 500));
    gtmp.copy(near).lerp(far, Math.min(1, d));
    gcol.push(gtmp.r, gtmp.g, gtmp.b, gtmp.r, gtmp.g, gtmp.b);
  };
  const STEP = 60;
  for (let x = -width / 2 - 600; x <= width / 2 + 600; x += STEP) {
    const a = x - width / 2;
    grid.push(a, 0.03, -height / 2 - 600, a, 0.03, height / 2 + 600);
    fade(a, -height / 2 - 600);
  }
  for (let z = -height / 2 - 600; z <= height / 2 + 600; z += STEP) {
    const b = z - height / 2;
    grid.push(-width / 2 - 600, 0.03, b, width / 2 + 600, 0.03, b);
    fade(-width / 2 - 600, b);
  }
  const gridGeo = new THREE.BufferGeometry();
  gridGeo.setAttribute('position', new THREE.Float32BufferAttribute(grid, 3));
  gridGeo.setAttribute('color', new THREE.Float32BufferAttribute(gcol, 3));
  s.ground.add(new THREE.LineSegments(gridGeo, new THREE.LineBasicMaterial({ vertexColors: true, transparent: true, opacity: 1 })));

  /* the role clusters: a ruled quadrant per role, in that role's tint */
  const byTint = new Map<number, number[]>();
  for (const station of stations) {
    const x0 = station.x - width / 2;
    const x1 = x0 + station.width;
    const z0 = station.y - height / 2;
    const z1 = z0 + station.height;
    const list = byTint.get(station.index % ROLE_TINTS.length) ?? [];
    list.push(
      x0, 0.06, z0, x1, 0.06, z0,
      x1, 0.06, z0, x1, 0.06, z1,
      x1, 0.06, z1, x0, 0.06, z1,
      x0, 0.06, z1, x0, 0.06, z0,
    );
    byTint.set(station.index % ROLE_TINTS.length, list);
  }
  for (const [tint, pts] of byTint) {
    const geo = new THREE.BufferGeometry();
    geo.setAttribute('position', new THREE.Float32BufferAttribute(pts, 3));
    s.ground.add(new THREE.LineSegments(geo, new THREE.LineBasicMaterial({ color: roleTint(tint), transparent: true, opacity: 0.55 })));
  }

  /* the pylons: low posts ring the plaza, each with an emissive cap */
  const pylons: number[] = [];
  const rails: number[] = [];
  const px = width / 2 + 150;
  const pz = height / 2 + 150;
  const corners = 16;
  for (let i = 0; i < corners; i++) {
    const t = i / corners;
    const angle = t * Math.PI * 2;
    const x = Math.cos(angle) * px;
    const z = Math.sin(angle) * pz;
    pylons.push(x, 0, z);
    rails.push(x, 44, z);
  }
  addPosts(s, THREE, pylons, rails, corners);

  /* the station pads: a hex tile and an emissive role-tinted ring per robot's
     home, and a plate naming the role */
  const { m1, v1, v2, q1, c1 } = s.scratch;
  const homes = plan.robots;
  const padCount = s.parts.pads.count;
  plan.robots.forEach((robot, i) => {
    if (i >= padCount) return;
    const tint = roleTint(robot.station.index);
    const hx = robot.home.x - width / 2;
    const hz = robot.home.y - height / 2;
    m1.compose(v1.set(hx, PAD_TILE_H / 2, hz), q1.identity(), v2.set(1, 1, 1));
    s.parts.pads.setMatrixAt(i, m1);
    s.parts.pads.setColorAt(i, c1.set(tint));
    s.parts.padRings.setMatrixAt(i, m1);
    s.parts.padRings.setColorAt(i, c1.set(tint));
  });
  for (let i = homes.length; i < padCount; i++) {
    m1.compose(v1.set(0, -1000, 0), q1.identity(), v2.set(0, 0, 0));
    s.parts.pads.setMatrixAt(i, m1);
    s.parts.padRings.setMatrixAt(i, m1);
  }
  for (const mesh of s.parts.ground) {
    mesh.instanceMatrix.needsUpdate = true;
    if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
  }

  const { c1: cc } = s.scratch;
  for (const station of stations) {
    const plate = makePlate(station.role, cc.set(roleTint(station.index)));
    const plateW = Number(plate.userData['width'] ?? 0);
    plate.position.set(station.x - width / 2 + 7 + plateW / 2, 0.08, station.y - height / 2 + 8);
    s.ground.add(plate);
  }
}

/** The low pylons and their emissive caps, as two instanced meshes added to
    the ground group. */
function addPosts(s: Scene3, THREE: ThreeModule, pylons: number[], rails: number[], count: number): void {
  const posts = new THREE.InstancedMesh(
    new THREE.BoxGeometry(2.6, 44, 2.6),
    new THREE.MeshStandardMaterial({ color: 0x22303f, metalness: 0.4, roughness: 0.6, emissive: 0x0c1520, emissiveIntensity: 0.6 }),
    count,
  );
  const caps = new THREE.InstancedMesh(
    new THREE.SphereGeometry(2.2, 8, 6),
    new THREE.MeshBasicMaterial({ color: C.accentA }),
    count,
  );
  const { m1, v1, q1, v2 } = s.scratch;
  for (let i = 0; i < count; i++) {
    m1.compose(v1.set(pylons[i * 3] ?? 0, 22, pylons[i * 3 + 2] ?? 0), q1.identity(), v2.set(1, 1, 1));
    posts.setMatrixAt(i, m1);
    m1.compose(v1.set(rails[i * 3] ?? 0, rails[i * 3 + 1] ?? 0, rails[i * 3 + 2] ?? 0), q1.identity(), v2.set(1, 1, 1));
    caps.setMatrixAt(i, m1);
  }
  posts.instanceMatrix.needsUpdate = true;
  caps.instanceMatrix.needsUpdate = true;
  posts.frustumCulled = false;
  caps.frustumCulled = false;
  s.ground.add(posts);
  s.ground.add(caps);
}

/** A faint scanline texture for the ground: thin horizontal lines, tiled. */
function scanTexture(THREE: ThreeModule, w: number, h: number): Texture {
  const c = document.createElement('canvas');
  c.width = 8;
  c.height = 8;
  const g = c.getContext('2d');
  if (!g) throw new Error('no 2d context for the ground texture');
  g.fillStyle = '#0a0e15';
  g.fillRect(0, 0, 8, 8);
  g.fillStyle = '#0d131c';
  g.fillRect(0, 2, 8, 1);
  g.fillRect(0, 6, 8, 1);
  const tex = new THREE.CanvasTexture(c);
  tex.wrapS = THREE.RepeatWrapping;
  tex.wrapT = THREE.RepeatWrapping;
  tex.repeat.set(Math.max(1, Math.round(w / 24)), Math.max(1, Math.round(h / 24)));
  tex.colorSpace = THREE.SRGBColorSpace;
  return tex;
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
    instance slot and spawns in; one that leaves starts its scale-out and is
    handed to the ghost pool; one whose target moved walks there from where it
    stands — never from where it was supposed to be. Everything else is left
    exactly as it was. */
function syncRobots(now: number): void {
  if (!scene3 || !plan) return;
  const parts = scene3.parts;
  if (slots.length !== parts.slots) slots = new Array<RobotView | null>(parts.slots).fill(null);
  const seen = new Set<string>();
  byCid.clear();
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
        tint: robot.station.index % ROLE_TINTS.length,
        x: worldX(robot.x),
        z: worldZ(robot.y),
        yaw: 0,
        tx: worldX(robot.x),
        tz: worldZ(robot.y),
        walk: null,
        phase: ((index * 0.618) % 1 + 1) % 1,
        spawnT0: now,
        leftT0: null,
        talkT0: null,
      };
    if (!at) robots.set(robot.key, view);
    else if (view.leftT0 != null) {
      /* it came back before its scale-out finished: resurrect it in place */
      view.leftT0 = null;
      view.spawnT0 = now;
    }
    view.index = index;
    view.state = robot.state;
    view.tint = robot.station.index % ROLE_TINTS.length;
    view.role = robot.source.role;
    view.cid = robot.source.cid;
    const tx = worldX(robot.x);
    const tz = worldZ(robot.y);
    const retarget = Math.abs(tx - view.tx) > 0.5 || Math.abs(tz - view.tz) > 0.5;
    const away = Math.abs(tx - view.x) > 0.5 || Math.abs(tz - view.z) > 0.5;
    if (REDUCED) {
      if (retarget || away) {
        view.walk = null;
        view.x = tx;
        view.z = tz;
        }
    } else if (retarget || (away && !view.walk)) {
      view.walk = { fx: view.x, fz: view.z, t0: now, dur: walkMs(Math.hypot(tx - view.x, tz - view.z)) };
    }
    view.tx = tx;
    view.tz = tz;
    view.talkT0 = robot.state === 'talking' ? (view.talkT0 ?? now) : null;
    if (view.cid != null) byCid.set(view.cid, view);
    slots[index] = view;
  });
  for (const [key, view] of [...robots]) {
    if (seen.has(key)) continue;
    if (view.leftT0 == null) {
      view.leftT0 = now;
      /* hand it to the ghost pool: the instance slot it draws into is past the
         live capacity, so a returning robot never collides with it */
      slots[view.index] = null;
      view.index = parts.capacity + ghostAt;
      ghostAt = (ghostAt + 1) % GHOST_CAPACITY;
      slots[view.index] = view;
      view.walk = null;
    }
  }
  order = plan.robots;
}

/** The position a robot stands at this instant, written into one reused
    record (the frame path allocates nothing): its walk interpolated and eased,
    or its own target when it is not walking. A finished walk is dropped;
    reduced motion never started one. */
const stood = { x: 0, z: 0, moving: false };
function stoodAt(view: RobotView, now: number): { x: number; z: number; moving: boolean } {
  const walk = view.walk;
  if (!walk) {
    stood.x = view.tx;
    stood.z = view.tz;
    stood.moving = false;
    return stood;
  }
  if (walk.dur <= 0) {
    view.walk = null;
    stood.x = view.tx;
    stood.z = view.tz;
    stood.moving = false;
    return stood;
  }
  const t = Math.min(1, (now - walk.t0) / walk.dur);
  const e = t * t * (3 - 2 * t);
  if (t >= 1) view.walk = null;
  stood.x = walk.fx + (view.tx - walk.fx) * e;
  stood.z = walk.fz + (view.tz - walk.fz) * e;
  stood.moving = t < 1;
  return stood;
}

const RING_FLAT = Math.PI / 2;
const HIDDEN_Y = -1000;

/** Compose a part's world matrix (the robot transform in `m1`, this part's
    local transform picked from the scratch) and write it into `mesh`'s slot. */
function put(mesh: InstancedMesh | null, index: number, scale: number, px: number, py: number, pz: number, q: Quaternion): void {
  const s = scene3;
  if (!s || !mesh) return;
  const { m2, m3, v1, v2, m1 } = s.scratch;
  m2.compose(v1.set(px, py, pz), q, v2.set(scale, scale, scale));
  m3.multiplyMatrices(m1, m2);
  mesh.setMatrixAt(index, m3);
}

/** A part's local offset swung about the X axis at a pivot: the limb hangs
    `len` below the pivot and rotates by `angle`. */
function swing(px: number, py: number, pz: number, len: number, angle: number): void {
  const s = scene3;
  if (!s) return;
  const { q3, v3 } = s.scratch;
  q3.setFromAxisAngle(s.scratch.ax, angle);
  v3.set(0, -len, 0).applyQuaternion(q3);
  s.scratch.v1.set(px, py, pz).add(v3);
}

/** Write one robot's instances: the rig, its status ring and marks, its
    working sweep and sparks, and — while talking — its speech bubble. Idle
    slots collapse to a hidden instance rather than a drawn one, so no part is
    ever billed for a robot that lacks it. */
function drawRobot(view: RobotView, now: number): void {
  const s = scene3;
  if (!s) return;
  const { m1, v1, v2, q1, q2, c1 } = s.scratch;
  const parts = s.parts;
  const i = view.index;
  if (i < 0 || i >= parts.slots) return;

  const tint = roleTint(view.tint);
  const stateColour =
    view.state === 'working' ? C.ok : view.state === 'unreachable' ? C.crit : view.state === 'talking' ? C.ink : C.ink3;
  const ghost = i >= parts.capacity;

  /* the robot's own transform: position, eased facing, and the spawn/leave
     scale about the feet */
  let scale = 1;
  if (!REDUCED && view.leftT0 != null) scale = leaveScale(now - view.leftT0);
  else if (!REDUCED) scale = Math.min(1.12, spawnScale(now - view.spawnT0));
  q1.setFromAxisAngle(s.scratch.ay, view.yaw);
  m1.compose(v1.set(view.x, 0, view.z), q1, v2.set(scale, scale, scale));

  /* the walk cycle and the idle breath */
  const walking = view.walk != null && !REDUCED && !ghost;
  const cycle = walking ? ((now - (view.walk?.t0 ?? now)) / STRIDE_MS) % 1 : 0;
  const bob = REDUCED || ghost ? 0 : idleBob(now, view.phase) + walkBob(cycle);
  const legL = walking ? walkSwing(cycle) : 0;
  const legR = walking ? walkSwing(cycle + 0.5) : 0;
  const gesture = view.state === 'talking' && !REDUCED && !ghost ? talkGesture(now, view.phase) : 0;
  const armL = walking ? walkArm(cycle) : 0;
  const armR = (walking ? walkArm(cycle + 0.5) : 0) + gesture;

  /* torso, head, visor, scan */
  q2.identity();
  put(parts.torso, i, 1, 0, TORSO_Y + bob, 0, q2);
  put(parts.head, i, 1, 0, HEAD_Y + bob, 0, q2);
  put(parts.visor, i, 1, 0, VISOR_Y + bob, VISOR_Z, q2);
  const scan = REDUCED || ghost ? 0 : visorScan(now, view.phase);
  put(parts.scan, i, 1, scan * 1.85, VISOR_Y + bob, VISOR_Z + 0.25, q2);
  parts.torso.setColorAt(i, c1.set(tint));
  parts.head.setColorAt(i, c1.set(tint));
  parts.visor.setColorAt(i, c1.set(tint));
  parts.scan.setColorAt(i, c1.set(C.ink));

  /* arms: swing about the shoulder */
  swing(-SHOULDER_X, SHOULDER_Y, 0, ARM_LEN / 2, armL);
  put(parts.armL, i, 1, s.scratch.v1.x, s.scratch.v1.y + bob, s.scratch.v1.z, s.scratch.q3);
  swing(SHOULDER_X, SHOULDER_Y, 0, ARM_LEN / 2, armR);
  put(parts.armR, i, 1, s.scratch.v1.x, s.scratch.v1.y + bob, s.scratch.v1.z, s.scratch.q3);
  parts.armL.setColorAt(i, c1.set(tint));
  parts.armR.setColorAt(i, c1.set(tint));

  /* legs and feet: swing about the hip */
  swing(-HIP_X, HIP_Y, 0, LEG_LEN / 2, legL);
  put(parts.legL, i, 1, s.scratch.v1.x, s.scratch.v1.y, s.scratch.v1.z, s.scratch.q3);
  swing(HIP_X, HIP_Y, 0, LEG_LEN / 2, legR);
  put(parts.legR, i, 1, s.scratch.v1.x, s.scratch.v1.y, s.scratch.v1.z, s.scratch.q3);
  swing(-HIP_X, HIP_Y, 0, LEG_LEN + 0.7, legL);
  put(parts.footL, i, 1, s.scratch.v1.x, s.scratch.v1.y, s.scratch.v1.z + 0.5, s.scratch.q3);
  swing(HIP_X, HIP_Y, 0, LEG_LEN + 0.7, legR);
  put(parts.footR, i, 1, s.scratch.v1.x, s.scratch.v1.y, s.scratch.v1.z + 0.5, s.scratch.q3);
  parts.legL.setColorAt(i, c1.set(tint));
  parts.legR.setColorAt(i, c1.set(tint));
  parts.footL.setColorAt(i, c1.set(tint));
  parts.footR.setColorAt(i, c1.set(tint));

  /* the antenna: it sways from its base, its tip aglow */
  const sway = REDUCED || ghost ? 0 : antennaSway(now, view.phase);
  const { q3, v3 } = s.scratch;
  q3.setFromAxisAngle(s.scratch.az, sway);
  v3.set(0, ANTENNA_LEN / 2, 0).applyQuaternion(q3);
  put(parts.stem, i, 1, v3.x, STEM_BASE_Y + v3.y + bob, v3.z, q3);
  v3.set(0, ANTENNA_LEN, 0).applyQuaternion(q3);
  put(parts.tip, i, 1, v3.x, STEM_BASE_Y + v3.y + bob, v3.z, q3);
  parts.stem.setColorAt(i, c1.set(tint));
  parts.tip.setColorAt(i, c1.set(view.state === 'working' ? C.ok : tint));

  /* the status ring: its glow pulses at the state's rate (pulseHz) */
  const pulse = REDUCED || ghost ? 0.6 : ringPulse(view.state, now, view.phase);
  const ringScale = (view.state === 'idle' ? 0.86 : 1) * (0.96 + pulse * 0.1);
  q2.setFromAxisAngle(s.scratch.ax, -RING_FLAT);
  put(parts.rings, i, ringScale, view.x, RING_Y, view.z, q2);
  parts.rings.setColorAt(i, c1.set(stateColour).multiplyScalar(0.55 + pulse * 0.45));

  /* the spawn/teleport-in ring: one pulse expanding off the pad as the robot
     scales in */
  const spawnProg = REDUCED || ghost ? -1 : spawnRing(now - view.spawnT0);
  if (spawnProg >= 0) {
    const grow = 0.7 + spawnProg * 2.6;
    put(parts.spawnRings, i, grow, view.x, RING_Y + 0.05, view.z, q2);
    parts.spawnRings.setColorAt(i, c1.set(tint).multiplyScalar(1 - spawnProg));
  } else {
    hideAt(parts.spawnRings, i);
  }

  /* the marks: ticks while working, a dashed ring + strike when unreachable */
  const marks = ghost ? 0 : view.state === 'working' ? TICKS_PER_WORKING : view.state === 'unreachable' ? DASHES_PER_LOST : 0;
  for (let k = 0; k < MARK_CAPACITY; k++) {
    const slot = i * MARK_CAPACITY + k;
    if (k >= marks) {
      hideAt(parts.marks, slot);
      continue;
    }
    const span = (Math.PI * 2) / marks;
    const angle = k * span + (view.state === 'working' && !REDUCED ? now * 0.0016 : 0);
    const radius = (RING_IN + RING_OUT) / 2;
    s.scratch.q2
      .setFromAxisAngle(s.scratch.ax, -RING_FLAT)
      .multiply(s.scratch.q1.setFromAxisAngle(s.scratch.az, -angle));
    put(
      parts.marks, slot, 1,
      view.x + Math.cos(angle) * radius, RING_Y + 0.12, view.z + Math.sin(angle) * radius,
      s.scratch.q2,
    );
    parts.marks.setColorAt(slot, c1.set(stateColour));
  }

  /* the working feedback: a scanning sweep on the pad and rising pixel sparks */
  if (view.state === 'working' && !ghost && !REDUCED) {
    const frac = sweepFrac(now, view.phase);
    q2.setFromAxisAngle(s.scratch.ax, -RING_FLAT);
    s.scratch.q2.multiply(s.scratch.q1.setFromAxisAngle(s.scratch.az, -frac * Math.PI * 2));
    put(parts.sweeps, i, 1.04, view.x, RING_Y + 0.08, view.z, s.scratch.q2);
    parts.sweeps.setColorAt(i, c1.set(C.ok));
  } else {
    hideAt(parts.sweeps, i);
  }
  for (let k = 0; k < SPARK_CAPACITY; k++) {
    const slot = i * SPARK_CAPACITY + k;
    const life = view.state === 'working' && !ghost && !REDUCED ? sparkLife(now, view.phase, k) : -1;
    if (life < 0) {
      hideAt(parts.sparks, slot);
      continue;
    }
    const angle = view.phase * Math.PI * 2 + k * 1.3;
    const radius = RING_IN + 2;
    const rise = life * 9;
    put(
      parts.sparks, slot, (1 - life) * 1.1,
      view.x + Math.cos(angle) * radius, RING_Y + rise, view.z + Math.sin(angle) * radius,
      q2.identity(),
    );
    parts.sparks.setColorAt(slot, c1.set(C.ok));
  }

  /* the speech bubble: pops, holds and fades while the pair keeps talking */
  const talking = view.state === 'talking' && !ghost;
  const pop = talking && !REDUCED ? bubblePop(now - (view.talkT0 ?? now)) : talking ? 1 : 0;
  if (pop > 0.01) {
    const bx = view.x;
    const by = HEAD_Y + bob + 9;
    const bz = view.z;
    put(parts.bubbles, i, 4.6 * pop * Math.min(1, scale), bx, by, bz, q2.identity());
    put(parts.bubbleTails, i, pop * Math.min(1, scale), bx, by - 4.4 * pop, bz, s.scratch.q1.setFromAxisAngle(s.scratch.ax, Math.PI));
    parts.bubbles.setColorAt(i, c1.set(C.ink));
    parts.bubbleTails.setColorAt(i, c1.set(C.ink));
  } else {
    hideAt(parts.bubbles, i);
    hideAt(parts.bubbleTails, i);
  }
}

/** Set a slot to the hidden collapse (zero scale, far below the ground). */
function hideAt(mesh: InstancedMesh, slot: number): void {
  const s = scene3;
  if (!s) return;
  const { m2, v1, v2, q2 } = s.scratch;
  m2.compose(v1.set(0, HIDDEN_Y, 0), q2.identity(), v2.set(0, 0, 0));
  mesh.setMatrixAt(slot, m2);
}

/** Push the instance buffers the frame wrote; the flags are set once per frame,
    not once per robot. */
function flushParts(): void {
  const parts = scene3?.parts;
  if (!parts) return;
  for (const mesh of parts.robot) {
    mesh.instanceMatrix.needsUpdate = true;
    if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
  }
}

/** Collapse a departed robot's instances: nothing is drawn for it again, and
    its slot stays free for the next arrival. */
function hideRobot(view: RobotView): void {
  const s = scene3;
  if (!s) return;
  const parts = s.parts;
  const i = view.index;
  for (const mesh of parts.robot) {
    if (mesh === parts.marks) continue;
    if (mesh === parts.sparks) continue;
    if (mesh === parts.sweeps) continue;
    hideAt(mesh, i);
  }
  for (let k = 0; k < MARK_CAPACITY; k++) hideAt(parts.marks, i * MARK_CAPACITY + k);
  for (let k = 0; k < SPARK_CAPACITY; k++) hideAt(parts.sparks, i * SPARK_CAPACITY + k);
  flushParts();
}

/* ------------------------------------------------------------- the wires */
/* The gatherings a plan draws, kept as its own array so the frame path never
   allocates one: it is rebuilt only when the gathering set changes. */
let linkWant: Gathering[] = [];

/** Rebuild the gathering wires when the set changes: a conversation gets an arc
    between its two robots, a hiring a floating contract glyph and a celebration
    ring. Called from the store's reconcile, never per frame. */
function ensureLinks(now: number): void {
  const s = scene3;
  if (!s || !plan) return;
  const THREE = s.three;
  let key = '';
  for (const gathering of plan.gatherings) {
    if (gathering.cids.length < 2) continue;
    key += `${gathering.kind}:${gathering.cids.join('+')}|`;
  }
  if (key === linkKey) return;
  linkKey = key;
  for (const view of links) {
    for (const mesh of [view.line, view.glyph, view.ring]) {
      if (!mesh) continue;
      s.links.remove(mesh);
      disposeDeep(mesh);
    }
  }
  linkWant = plan.gatherings.filter((g) => g.cids.length >= 2);
  links = linkWant.map((gathering) => {
    const view: LinkView = { cids: gathering.cids, line: null, positions: null, glyph: null, ring: null, t0: now };
    if (gathering.kind === 'conversation') {
      const positions = new Float32Array((ARC_SEG + 1) * 3);
      const geo = new THREE.BufferGeometry();
      const attr = new THREE.BufferAttribute(positions, 3);
      attr.setUsage(THREE.DynamicDrawUsage);
      geo.setAttribute('position', attr);
      const line = new THREE.Line(geo, new THREE.LineBasicMaterial({ color: C.ink, transparent: true, opacity: 0.5 }));
      line.frustumCulled = false;
      s.links.add(line);
      view.line = line;
      view.positions = positions;
    } else {
      const glyph = new THREE.Mesh(
        new THREE.OctahedronGeometry(5.4),
        new THREE.MeshBasicMaterial({ color: C.warn, transparent: true, opacity: 0.95 }),
      );
      glyph.frustumCulled = false;
      const ring = new THREE.Mesh(
        new THREE.RingGeometry(RING_OUT + 4, RING_OUT + 5.4, 48),
        new THREE.MeshBasicMaterial({ color: C.warn, side: THREE.DoubleSide, transparent: true, opacity: 0.8 }),
      );
      ring.rotation.x = -Math.PI / 2;
      ring.frustumCulled = false;
      s.links.add(glyph);
      s.links.add(ring);
      view.glyph = glyph;
      view.ring = ring;
    }
    return view;
  });
}

/** Update the gathering wires in place: the conversation arc follows the pair's
    walk, the hiring glyph turns and its celebration ring pulses. Called every
    frame; nothing here allocates. */
function syncLinks(now: number): void {
  for (let index = 0; index < links.length; index++) {
    const view = links[index];
    const gathering = linkWant[index];
    if (!view || !gathering) continue;
    if (view.positions != null) {
      const a = gathering.cids[0] == null ? null : (byCid.get(gathering.cids[0]) ?? null);
      const b = gathering.cids[1] == null ? null : (byCid.get(gathering.cids[1]) ?? null);
      if (!a || !b) continue;
      const midX = (a.x + b.x) / 2;
      const midZ = (a.z + b.z) / 2;
      const lift = HEAD_Y + 8 + Math.hypot(a.x - b.x, a.z - b.z) * 0.12;
      const positions = view.positions;
      for (let k = 0; k <= ARC_SEG; k++) {
        const t = k / ARC_SEG;
        const omt = 1 - t;
        positions[k * 3] = omt * omt * a.x + 2 * omt * t * midX + t * t * b.x;
        positions[k * 3 + 1] = omt * omt * (HEAD_Y + 4) + 2 * omt * t * lift + t * t * (HEAD_Y + 4);
        positions[k * 3 + 2] = omt * omt * a.z + 2 * omt * t * midZ + t * t * b.z;
      }
      const attr = view.line?.geometry.getAttribute('position');
      if (attr) attr.needsUpdate = true;
    } else {
      const cx = worldX(gathering.x);
      const cz = worldZ(gathering.y);
      if (view.glyph) {
        view.glyph.position.set(cx, HEAD_Y + 22, cz);
        view.glyph.rotation.y = REDUCED ? 0 : now * 0.0011;
        view.glyph.rotation.x = REDUCED ? 0 : Math.sin(now * 0.0009) * 0.35;
      }
      if (view.ring) {
        const age = ((now - view.t0) % 2400) / 2400;
        const grow = REDUCED ? 1 : 0.6 + age * 1.6;
        view.ring.position.set(cx, RING_Y, cz);
        view.ring.scale.set(grow, grow, 1);
        (view.ring.material as MeshBasicMaterial).opacity = REDUCED ? 0.7 : 0.7 * (1 - age);
      }
    }
  }
}

/** The two ground halos that mark the cursor: the card's selection (the owner's
    signal, orange) and the keyboard's focus (rule ink). Written as two fixed
    entries, so the frame path allocates nothing. */
function syncCursor(): void {
  const s = scene3;
  if (!s || !plan) return;
  const { m1, v1, q1, v2 } = s.scratch;
  const selectedView = selected == null ? null : (byCid.get(selected) ?? null);
  const focusKey = document.activeElement === s.renderer.domElement ? order[focusIdx]?.key : undefined;
  const focusedView = focusKey == null ? null : (robots.get(focusKey) ?? null);
  (s.cursorSel.material as MeshBasicMaterial).color.set(C.signal);
  (s.cursorFocus.material as MeshBasicMaterial).color.set(C.ink3);
  const flat = q1.setFromAxisAngle(s.scratch.ax, -RING_FLAT);
  for (let k = 0; k < 2; k++) {
    const view = k === 0 ? selectedView : focusedView;
    const mesh = k === 0 ? s.cursorSel : s.cursorFocus;
    m1.compose(
      view ? v1.set(view.x, RING_Y + 0.24, view.z) : v1.set(0, HIDDEN_Y, 0),
      flat,
      v2.set(view ? 1 : 0, view ? 1 : 0, 1),
    );
    mesh.setMatrixAt(0, m1);
    mesh.instanceMatrix.needsUpdate = true;
  }

  /* the name, over whichever robot the cursor is on */
  const hovered = hoverKey == null ? null : (robots.get(hoverKey) ?? null);
  const named = hovered ?? focusedView ?? selectedView;
  s.label.visible = named != null;
  if (named) {
    const text = named.cid == null ? named.role : `${named.role} · cid ${named.cid}`;
    if (s.label.userData['text'] !== text) retextureLabel(s.label, text, CSS_INK);
    s.label.position.set(named.x, HEAD_Y + 26, named.z);
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
  const parts = s.parts;
  const hits = s.raycaster.intersectObjects([parts.torso, parts.head, parts.armL, parts.armR, parts.legL, parts.legR, parts.stem], false);
  const hit = hits[0];
  if (!hit || hit.instanceId == null) return null;
  const view = slots[hit.instanceId] ?? null;
  return view && view.index < parts.capacity ? view : null;
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

function onLeave(): void {
  if (hoverKey == null) return;
  hoverKey = null;
  requestFrame();
}

function onDown(ev: MouseEvent): void {
  if (ev.button !== 0) return;
  dragging = { x: ev.clientX, y: ev.clientY, moved: 0 };
  suppressClick = false;
}

/** Dragging pans the plaza: the aim moves against the pointer (the ground
    follows the hand), clamped so the plaza is never lost off-screen. A drag
    cancels the camera's follow of the selection. */
function onDrag(ev: MouseEvent): void {
  const s = scene3;
  if (!dragging || !s || !plan) return;
  const dx = ev.clientX - dragging.x;
  const dy = ev.clientY - dragging.y;
  dragging = { x: ev.clientX, y: ev.clientY, moved: dragging.moved + Math.abs(dx) + Math.abs(dy) };
  follow = false;
  const mount = byId('floor-mount');
  const aspect = mount.clientWidth / Math.max(1, mount.clientHeight);
  const perPx = rtsViewH(plan.width, plan.height, aspect) / zoom / Math.max(1, mount.clientHeight);
  const [ox, oz] = rtsPanOffset(dx, dy, perPx);
  const next = clampPan({ x: panTarget.x + ox, z: panTarget.z + oz }, plan.width, plan.height);
  panTarget.x = next.x;
  panTarget.z = next.z;
  /* a drag is direct: the aim snaps to the target, no easing lag under the hand */
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
    stays exactly where it is. The zoom eases to its target. */
function onWheel(ev: WheelEvent): void {
  const s = scene3;
  if (!s || !plan) return;
  ev.preventDefault();
  follow = false;
  zoomTarget = clampZoom(zoomTarget * Math.exp(-ev.deltaY * 0.0012));
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
/** The frame callback, named once so scheduling a frame allocates no closure. */
function onAnimationFrame(): void {
  raf = null;
  frame();
}

function requestFrame(): void {
  if (raf != null) return;
  raf = requestAnimationFrame(onAnimationFrame);
}

/** Step every robot's walk to `now`: the eased position on the way to its
    target, and the facing turned toward the travel (eased). Both the frame loop
    and the store's paint path call it, so a walk still advances where animation
    frames are starved. Returns whether any walk advanced (or a ghost retired). */
function stepRobots(now: number, dt: number): boolean {
  let walked = false;
  for (let i = 0; i < slots.length; i++) {
    const view = slots[i];
    if (!view) continue;
    if (view.leftT0 != null && now - view.leftT0 >= leaveMs()) {
      robots.delete(view.key);
      slots[i] = null;
      hideRobot(view);
      walked = true;
      continue;
    }
    const at = stoodAt(view, now);
    const turning = Math.abs(at.x - view.x) > 0.001 || Math.abs(at.z - view.z) > 0.001;
    if (turning) {
      const wanted = Math.atan2(at.x - view.x, at.z - view.z);
      view.yaw = dampAngle(view.yaw, wanted, YAW_HALF_LIFE, dt);
      view.x = at.x;
      view.z = at.z;
      walked = true;
    } else if (view.state === 'talking' && view.walk == null && !REDUCED) {
      /* face the peer it is talking to */
      const peer = peerOf(view);
      if (peer) view.yaw = dampAngle(view.yaw, Math.atan2(peer.x - view.x, peer.z - view.z), FOLLOW_HALF_LIFE, dt);
    }
  }
  return walked;
}

/** Rewrite every live slot's instances for this frame. The plaza is always
    breathing (breath, sway, scan, pulse, sweep), so every robot is redrawn
    every frame — the reason a walk costs the same as standing still. */
function drawAll(now: number): void {
  for (let i = 0; i < slots.length; i++) {
    const view = slots[i];
    if (view) drawRobot(view, now);
  }
}

function leaveMs(): number {
  return REDUCED ? 0 : 420;
}

/** The eased turn toward a target yaw, taking the short way round. */
function dampAngle(current: number, target: number, halfLifeMs: number, dtMs: number): number {
  let delta = target - current;
  while (delta > Math.PI) delta -= Math.PI * 2;
  while (delta < -Math.PI) delta += Math.PI * 2;
  return current + delta * (1 - Math.pow(2, -Math.max(0.001, dtMs) / halfLifeMs));
}

/** The peer a talking robot faces, from its gathering: an index loop, so the
    frame path allocates no closure. */
function peerOf(view: RobotView): RobotView | null {
  const cid = view.cid;
  if (!plan || cid == null) return null;
  for (let g = 0; g < plan.gatherings.length; g++) {
    const cids = plan.gatherings[g]?.cids;
    if (!cids || cids.indexOf(cid) < 0) continue;
    for (let k = 0; k < cids.length; k++) {
      const other = cids[k];
      if (other != null && other !== cid) return byCid.get(other) ?? null;
    }
  }
  return null;
}

/** One frame: ease the camera, step the walks, rewrite the instances, and
    draw — but only while the floor is up and the tab is visible. The plaza's
    own life (breath, sway, pulse, sweep) keeps it drawing; under reduced motion
    nothing animates, so a still plaza spends no frames at all. */
function frame(): void {
  const s = scene3;
  if (!s || !plan) return;
  if (store.route.name !== 'floor' || document.hidden) return;
  const now = Date.now();
  const dt = lastNow === 0 ? 16.7 : Math.min(100, Math.max(1, now - lastNow));
  lastNow = now;

  /* the camera eases to its target (zoom, pan, and the selection's follow) */
  const mount = byId('floor-mount');
  const aspect = mount.clientWidth / Math.max(1, mount.clientHeight);
  let cameraMoved = false;
  if (Math.abs(zoomTarget - zoom) > 0.0005) {
    zoom = REDUCED ? zoomTarget : damp(zoom, zoomTarget, 90, dt);
    cameraMoved = true;
  }
  if (follow && selected != null && !REDUCED) {
    const target = byCid.get(selected);
    if (target) {
      const next = clampPan({ x: target.x, z: target.z }, plan.width, plan.height);
      panTarget.x = next.x;
      panTarget.z = next.z;
    }
  }
  if (Math.abs(panTarget.x - pan.x) > 0.01 || Math.abs(panTarget.z - pan.z) > 0.01) {
    pan.x = REDUCED ? panTarget.x : damp(pan.x, panTarget.x, FOLLOW_HALF_LIFE, dt);
    pan.z = REDUCED ? panTarget.z : damp(pan.z, panTarget.z, FOLLOW_HALF_LIFE, dt);
    cameraMoved = true;
  }
  if (cameraMoved && mount.clientWidth > 0) updateCamera(aspect);

  const walked = stepRobots(now, dt);
  drawAll(now);
  flushParts();
  syncLinks(now);
  syncCursor();
  /* the two accent lights drift; a subtle life of the scene's own */
  if (!REDUCED) {
    s.lights.a.position.set(Math.cos(now * 0.00021) * 520, 220, Math.sin(now * 0.00021) * 520);
    s.lights.b.position.set(Math.cos(now * 0.00013 + 2.1) * 640, 260, Math.sin(now * 0.00013 + 2.1) * 640);
  }
  s.renderer.render(s.scene, s.camera);
  frames++;
  readout(now, dt);
  /* the plaza's own life keeps the loop alive; under reduced motion nothing
     animates, so it stops as soon as the frame is spent (a still plaza spends
     no further frames) */
  if (!REDUCED || walked || cameraMoved) requestFrame();
}

/** The frame-time readout in the stage's corner: the honest proof the scene
    holds its rate. Updated a few times a second, never per frame. */
function readout(now: number, dt: number): void {
  fpsShown = fpsShown === 0 ? 1000 / dt : fpsShown * 0.9 + (1000 / dt) * 0.1;
  msShown = msShown === 0 ? dt : msShown * 0.9 + dt * 0.1;
  if (now - fpsAt < 250) return;
  fpsAt = now;
  const node = byId('floor-fps');
  node.textContent = `${Math.round(fpsShown)} fps · ${msShown.toFixed(1)} ms`;
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
    the links, the camera's frame, and the three DOM surfaces — the status line,
    the ruled table and the card. */
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
  const next = readFromStore();
  plan = next;
  ensureParts(s.three, s.scene, next.robots.length, next.stations.length);
  /* the ground is rebuilt when the layout changes — the plan's extent, the
     stations, OR the robots' own homes (a new agent in a role re-centres that
     role's sub-grid, and every pad under it) */
  const wantGround =
    `${next.width}x${next.height}:` +
    `${next.robots.map((r) => `${r.home.x},${r.home.y}`).join(';')}:` +
    next.stations.map((st) => `${st.index}/${st.role}`).join(',');
  if (wantGround !== groundKey) {
    groundKey = wantGround;
    rebuildGround();
  }
  const now = Date.now();
  syncRobots(now);
  const mount = byId('floor-mount');
  const aspect = mount.clientWidth / Math.max(1, mount.clientHeight);
  if (mount.clientWidth > 0) updateCamera(aspect);
  stepRobots(now, 16.7);
  drawAll(now);
  ensureLinks(now);
  syncLinks(now);
  flushParts();
  syncCursor();
  /* the scene is not drawn while the tab is hidden: the store still paints every
     DOM surface the floor has (the status, the table, the card, the live
     label), and the last drawn frame stays on the canvas until the tab comes
     back — when the visibility listener repaints it. */
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

  /* one announcement per shape, never per poll */
  const shape = next.gatherings.map((g) => `${g.kind}:${g.cids.join('+')}`).join(',');
  if (shape !== talkSig) {
    talkSig = shape;
    if (next.gatherings.length > 0) announce(label);
  }
}

/* ---------------------------------------------------------------- status */
/** The honest line when the plaza's social layer is not on this node yet. */
function socialMissing(): string | null {
  const boardGone = store.boardError != null && store.boardErrorStatus === 404;
  const convGone = store.conversationsError != null && store.conversationsErrorStatus === 404;
  if (!boardGone && !convGone) return null;
  return 'The social layer (board, contacts, conversations) is not on this node yet: the console’s reads answer 404.';
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
  const focused = document.activeElement === scene3?.renderer.domElement ? plan.robots[focusIdx] : undefined;
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
    (talks > 0 ? ` · ${talks} ${talks === 1 ? 'gathering' : 'gatherings'}` : '');
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
    keyboard — each row selects its robot. */
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
    note.textContent = 'reading contacts…';
    return;
  }
  if (held.state === 'error') {
    note.textContent = held.error;
    return;
  }
  note.textContent = held.contacts.length === 0 ? 'no contacts yet' : `${fmtNum(held.contacts.length)}${held.truncated ? ` of ${fmtNum(held.total)}` : ''} contacts`;
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

/** The active pairs a CID is in, from the store's own view of the social layer:
    the node's pairs when it sends them, the exchanges when it does not. */
function pairsWith(cid: number): ActivePair[] {
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
  byId('fc-convs-note').textContent = pairs.length === 0 ? 'no active conversation' : `${pairs.length} active now`;
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
      ? 'reading the thread…'
      : thread.state === 'error'
        ? thread.error
        : `${fmtNum(thread.exchanges.length)}${thread.truncated ? ` of ${fmtNum(thread.total)}` : ''} exchanges, newest first`;
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
    fact('talking with', peers.length === 0 ? 'no one now' : peers.map((peer) => `cid ${peer} (${nameOf(peer)})`).join(', ')),
  );
  facts.appendChild(fact('drive · stop', cid == null ? 'no cid to drive' : linkTo('#roster', `the roster drives cid ${cid}`)));

  const held = cid == null ? undefined : store.contacts.get(cid);
  const post = lastPostBy(store.board, cid);
  const sig = [
    cid,
    held ? `${held.state}/${held.contacts.map((c) => `${c.peer_cid}:${c.kind}:${c.since}`).join(',')}` : 'none',
    post ? `${post.id}/${post.subject}/${post.body.length}` : 'nopost',
    store.thread ? `${store.thread.a}-${store.thread.b}/${store.thread.state}/${store.thread.exchanges.length}` : 'nothread',
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
  follow = cid != null;
  if (cid != null && !store.contacts.has(cid)) void store.loadContacts(cid);
  store.scheduleRender();
}

/** Move the camera's cursor to the nearest robot in a direction, from the
    positions actually stood at. The cursor is announced; Enter opens the card. */
function moveFocus(dx: number, dy: number): void {
  if (!plan || plan.robots.length === 0) return;
  const current = robots.get(plan.robots[focusIdx]?.key ?? '');
  if (!current) return;
  let best: RobotView | null = null;
  let bestD = Infinity;
  for (let i = 0; i < slots.length; i++) {
    const view = slots[i];
    if (!view || view.key === current.key || view.leftT0 != null) continue;
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
  follow = false;
  announce(`${best.role}, cid ${best.cid ?? '—'}, ${ROBOT_STATE_LABEL[best.state]}`);
  requestFrame();
}

/* ------------------------------------------------------------ the wiring */
export function initFloor(): void {
  slots = new Array<RobotView | null>(scene3?.parts.slots ?? 1 + GHOST_CAPACITY).fill(null);
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

/** Showing the floor re-reconciles it: the mount's width reads 0 while the view
    is hidden, so the camera's frame is recomputed on every enter. */
export function syncFloorVisibility(): void {
  if (store.route.name === 'floor') syncFloor();
}

/** Read-only state for the verification harness; the app never reads it. What
    the scene actually holds — instances, parts, the camera, the frame rate — so
    the floor can be read back as drawn, not just as derived. */
export function floorState(): {
  robots: number;
  stations: number;
  gatherings: number;
  states: Record<RobotState, number>;
  selected: number | null;
  walks: number;
  leaving: number;
  positions: Array<{ cid: number | null; x: number; z: number; gathering: string | null }>;
  tint_cycle: number;
  frames: number;
  fps: number;
  frame_ms: number;
  draws: number;
  triangles: number;
  parts: number;
  ghost_capacity: number;
  webgl2: boolean;
  reduced: boolean;
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
    leaving: [...robots.values()].filter((r) => r.leftT0 != null).length,
    positions: plan.robots.map((r) => {
      const view = robots.get(r.key);
      return { cid: r.source.cid, x: view?.x ?? 0, z: view?.z ?? 0, gathering: r.gathered };
    }),
    tint_cycle: ROLE_TINTS.length,
    frames,
    fps: Math.round(fpsShown * 10) / 10,
    frame_ms: Math.round(msShown * 100) / 100,
    draws: scene3.renderer.info.render.calls,
    triangles: scene3.renderer.info.render.triangles,
    parts: scene3.parts.robot.length,
    ghost_capacity: GHOST_CAPACITY,
    webgl2: scene3.renderer.getContext() instanceof WebGL2RenderingContext,
    reduced: REDUCED,
    camera: { zoom, pan: [pan.x, pan.z], view_h: rtsViewH(plan.width, plan.height, aspect) / zoom },
    plan: { width: plan.width, height: plan.height },
    label: scene3.renderer.domElement.getAttribute('aria-label'),
  };
}

/** Advance the scene exactly one frame and return the frame's own cost in
    milliseconds. The page's loop drives this from requestAnimationFrame; a
    harness whose tab fires no animation frames (a headless one) can call it
    directly — it is the same frame function, so the cost measured is the real
    one. The app itself never calls it. */
export function floorFrame(): number {
  const t0 = performance.now();
  frame();
  return performance.now() - t0;
}

/** The gatherings as the scene drew them — the meeting point and the world
    coordinates — for the harness's own assertions. */
export function floorGatherings(): Array<{ kind: GatherKind; cids: number[]; at: [number, number] }> {
  if (!plan) return [];
  return plan.gatherings.map((g) => ({ kind: g.kind, cids: g.cids, at: [worldX(g.x), worldZ(g.y)] }));
}

/** The ring pulse rate the legend and the report state, re-exported so the
    harness can assert the scene's own numbers. */
export const FLOOR_PULSE = { working: pulseHz('working'), talking: pulseHz('talking'), idle: pulseHz('idle'), unreachable: pulseHz('unreachable') };
export const FLOOR_BUBBLE_MS = BUBBLE_MS;
