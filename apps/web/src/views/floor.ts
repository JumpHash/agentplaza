// The Floor: districts of robots on a three.js plaza. The scene owns placement,
// wandering, socialising, packets and labels; each robot owns its own animation
// (floor3d/robot), and the furniture comes from floor3d/props.

import '../styles/floor.css';

import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';

import { api, type Agent, type Conversation, type Entry, type FleetApple, type FleetMachine, type FleetResponse, type FleetStatus } from '../api';
import { go, href } from '../router';
import { agents, on } from '../store';
import { ago, button, clip } from '../ui/components';
import { plazaPaused } from '../ui/plaza-control';
import { floorName, roleLabel } from '../roles';
import { h } from '../ui/dom';
import { makeRobot, ROBOT_PALETTE, speciesOf, type Robot, type RobotState } from './floor3d/robot';
import { buildBackdropSteps } from './floor3d/outskirts';
import { buildFleet } from './floor3d/fleet';
import { buildRoom } from './floor3d/rooms';
import { ground as groundTex, rng } from './floor3d/textures';
import { outline, sky, starfield, toonMat } from './floor3d/toon';
import { bakeStatic, routeShadowProxies } from './floor3d/bake';
import { compileSteps, disposeTree, JobQueue, layoutCached, layoutFor, navSteps, releaseBuild, takeBuild, textWidth } from './floor3d/warm';
import { createMeter, startPixelRatio } from './floor3d/perf';
import { mountFloor2d } from './floor2d';
import { boardHeightAt, scaleModel, traceGraph, type BoardDistrict, type BoardLayout, type TraceGraph } from './floor3d/board-layout';
import { roomPlan } from './floor3d/nav-scene';
import { NavWorld, navigateTo } from './floor3d/nav';
import { buildNavOverlay, type NavOverlay } from './floor3d/nav-overlay';
import { buildLift, prewarmLift, type LiftProp } from './floor3d/lift';
import { disposeRamp, rampForPortal } from './floor3d/ramp';
import { buildGangway, type Gangway } from './floor3d/gangway';
import { RobotWork, straightStep, type WorkGoal } from './floor3d/robot-work';
import type { BodyMode } from './floor3d/nav-height';
import { registry } from './floor3d/robots/files';
import { currentPulses, modelsFor, placeAtom, traceStrips, type Built, type Model, type Params, type Pulses, type Trace } from './floor3d/hardware';
import { boardScene, platePieces, roomPalette as auditPalette } from './floor3d/board-rooms';
import { groundAudit, supportField, type Support } from './floor3d/ground';
import { MODELS } from './floor3d/hardware-index';
import { prewarmFx } from './floor3d/robots/fx';
import { DIRECT_START, EntrySequence, beginEntry, ownRoomMaterials, prepareFade, setReturn, takeEntry, takeReturn, type EntryContext, type Exterior, type Handoff, type Pose } from './floor3d/entry';
import type { Cleanup } from './index';
import { engineWanted, mountEngine } from './engine-mount';

const PACKET_MS = 1500;
const MAX_PACKETS = 60;
const POLAR = 0.95; // camera angle from vertical, about 55 degrees: isometric-ish
const AZIMUTH = Math.PI / 4;
const BIG_BOARD = 72; // a board this many units across (the server's 88) is big: its default view is higher (viewDir)
const BIG_POLAR = 0.42; // a big board's default view, from vertical: about 24 degrees, near top-down (POLAR is 55)
const BOARD_DROP = 14; // the night's ground sits this far under the board, so the board floats
const GLOW_OPACITY = 0.4; // a working component's glow plate, at full strength
const DUST_N = 90;
const LABEL_CSS =
  'position:absolute;left:0;top:0;display:none;align-items:center;gap:6px;max-width:200px;padding:3px 7px;' +
  'background:var(--sheet);border:1px solid var(--ink);box-shadow:var(--print);color:var(--ink);' +
  'font:600 10.5px/1.35 var(--mono);white-space:nowrap;' +
  'pointer-events:none;will-change:transform';
const FIT_FILL = 0.85;
const DRIFT_AMP = (3 * Math.PI) / 180; // the settled plaza's idle sway: this far either way about the framed target (radians)
const DRIFT_RATE = (0.35 * Math.PI) / 180; // at about 3.5 degrees per 10 s (radians a second)
const FRAME_MIN_SHARE = 0.3; // the default view frames at least this share of the board's width and depth (fitBox)
const LOAD_BUDGET_MS = 20; // while an arrival waits for its build (the loading card): the build runs at a fuller pace
const WORLD_BUDGET_MS = 6; // the district build's time per frame (stepWorld): a few districts a frame // the plaza spans this share of the stage on its tighter axis

/** A room on its component: its centre on the board (cx, cy), its slot's top (y) and its cell (w, d). */
type District = BoardDistrict;

interface Bot {
  agent: Agent;
  district: District;
  home: [number, number]; // console position
  x: number; // current position, grid units
  y: number;
  heading: number; // radians about the vertical axis
  goalKey: string; // the goal the robot was last sent to (navigateTo runs when it changes)
  moving: boolean;
  py: number; // height of the feet
  state: RobotState | null;
  activity: string; // what the agent is doing right now, shown on its label and card
  robot: Robot;
  robotKey: string;
  /** The robot's orientation, eased toward the ground's slope and its heading. */
  tilt: THREE.Quaternion;
  pad: THREE.Mesh; // console pad under the home slot, tinted by state
  padMat: THREE.MeshBasicMaterial;
  token: THREE.Mesh; // the glowing task token, shown while working
  highlighted: boolean;
  label: HTMLDivElement; // compact: status square and name; idle agents show none unless selected or hovered
  labelKey: HTMLElement;
  labelName: HTMLElement;
}

interface Packet {
  from: number;
  to: number;
  start: number;
  core: THREE.Mesh;
  halo: THREE.Mesh;
}

/** A district's monitors and whether one of its robots is working right now. */
interface DistrictLight {
  screens: THREE.MeshToonMaterial[];
  lit: boolean;
}

interface Palette {
  paper: string;
  sheet: string;
  rule: string;
  ink: string;
  ink2: string;
  ink3: string;
  signal: string;
  ok: string;
  info: string;
  danger: string;
}

/** The district hues: a curated cartoon set (coral, tangerine, butter, lime, mint, teal, sky, periwinkle, lavender, rose). */
const HUES = [8, 28, 46, 88, 140, 172, 198, 222, 262, 340];

/** An agent's own colour: hues spread by the golden angle, so neighbours never match. */
export function agentHue(cid: number): number {
  return (cid * 137.508 + 20) % 360;
}

export function roleHue(role: string): number {
  let hash = 0;
  for (const ch of role) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  return HUES[hash % HUES.length]!;
}

/** A board's glow plate over one slot: lit while its rooms have working agents. */
interface GlowPlate {
  slot: string; // the slot id
  on: string; // the part the slot lies on
  mesh: THREE.Mesh;
  mat: THREE.MeshBasicMaterial;
  level: number; // 0..1, eased toward whether a working agent sits in the slot
}

/** One machine's board as the plaza: the built model, its walkways, its current and its glow plates. */
interface BoardScene {
  key: string; // the model and its params, so a change in either rebuilds the board
  model: Model;
  params: Params;
  group: THREE.Group;
  built: Built;
  graph: TraceGraph;
  /** Pulses per component: the traces that start on each part (`on`); traces on no slot carry `on` ''. */
  pulses: { on: string; pulses: Pulses }[];
  plates: Map<string, GlowPlate>; // by the slot id
  atom: ((t: number) => void) | null;
  atomGroup: THREE.Group | null; // the atom's object, for the ENTER sequence's glow
}

/** The board shown when no machine is known: the workstation, with its one GPU. */
const DEFAULT_BOARD: { name: string; params: Params } = { name: 'board/workstation', params: { gpus: 1, studio: 0 } };

/** The board a machine's plaza is built on, and the params its model takes (as the Machines view builds them). */
export function boardOf(m: { kind: string; vendor?: string | null; model?: string | null; apple?: FleetApple | null; gpus: unknown[] }): { name: string; params: Params } {
  const { board } = modelsFor(m);
  const params: Params = { gpus: m.gpus.length, studio: m.model?.includes('Studio') ? 1 : 0 };
  if (m.apple) {
    params.p_cores = m.apple.p_cores;
    params.e_cores = m.apple.e_cores;
    params.gpu_cores = m.apple.gpu_cores;
    params.memory_gib = m.apple.unified_memory_gib;
  }
  return { name: board, params };
}

/** The room whose cell holds the point (x, z), if any. */
export function roomAt(layout: BoardLayout, x: number, z: number): BoardDistrict | null {
  return layout.districts.find((d) => Math.abs(x - d.cx) <= d.w / 2 && Math.abs(z - d.cy) <= d.d / 2) ?? null;
}

function readPalette(): Palette {
  const cs = getComputedStyle(document.documentElement);
  const get = (name: string, fallback: string): string => cs.getPropertyValue(`--${name}`).trim() || fallback;
  return {
    paper: get('paper', '#f5f3ee'),
    sheet: get('sheet', '#ffffff'),
    rule: get('rule', '#d6d2c8'),
    ink: get('ink', '#1a1a1a'),
    ink2: get('ink-2', '#4a4a4a'),
    ink3: get('ink-3', '#8a8a8a'),
    signal: get('signal', '#d9482b'),
    ok: get('ok', '#2f8f5b'),
    info: get('info', '#2f6fb0'),
    danger: get('danger', '#c0392b'),
  };
}

let probe: CanvasRenderingContext2D | null = null;

/** Any CSS colour (including oklch) as a three.js colour: the browser resolves it to sRGB. */
function colour(css: string, fallback: string): THREE.Color {
  probe ??= document.createElement('canvas').getContext('2d');
  const c = new THREE.Color();
  if (!probe) return c.set(fallback);
  probe.fillStyle = fallback;
  probe.fillStyle = css; // invalid CSS is ignored and keeps the fallback
  probe.clearRect(0, 0, 1, 1);
  probe.fillRect(0, 0, 1, 1);
  const px = probe.getImageData(0, 0, 1, 1).data;
  return c.setRGB((px[0] ?? 0) / 255, (px[1] ?? 0) / 255, (px[2] ?? 0) / 255, THREE.SRGBColorSpace);
}

function str(v: unknown): string {
  return typeof v === 'string' ? v : '';
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}

/** A room's floor height: its slot's top. */
const topOf = (d: District): number => d.y;


/** Whether (x, y) lies within a robot's width of a prop footprint. */


/** The cid a tool call hands a message or a task to, if any. */
function recipientOf(tool: string, inp: Record<string, unknown>): number | null {
  const to = inp.to_cid ?? inp.assignee;
  const sends = tool === 'plaza_send_message';
  const assigns = tool === 'plaza_assign' || tool === 'plaza_post_task';
  return (sends || assigns) && typeof to === 'number' ? to : null;
}

/** The one line that says what a tool call is doing. */
function toolActivity(tool: string, inp: Record<string, unknown>, nameOf: (cid: unknown) => string): string {
  if (tool === 'bash') return `$ ${clip(str(inp.command).split('\n')[0] ?? '', 120)}`;
  if (tool === 'plaza_send_message') return `✉ to ${nameOf(inp.to_cid)}: ${clip(str(inp.text), 100)}`;
  if (tool === 'plaza_assign' || (tool === 'plaza_post_task' && inp.assignee != null)) {
    return `→ task for ${nameOf(inp.assignee ?? inp.to_cid)}: ${str(inp.title)}`;
  }
  if (tool === 'share_file') return `⇪ shared ${str(inp.path)}`;
  const target = str(inp.path) || str(inp.url);
  return target ? `→ ${tool} ${clip(target, 80)}` : `→ ${tool}`;
}

/** A WebGL renderer, or null when the browser refuses to make one. */
function newRenderer(antialias = true): THREE.WebGLRenderer | null {
  try {
    const r = new THREE.WebGLRenderer({ antialias });
    r.debug.checkShaderErrors = false; // the info log is read only when a shader fails: reading it per program costs ms on the select path
    return r;
  } catch {
    return null;
  }
}

/**
 * The plazas' renderer: one per page, created once (the Machines mount creates it ahead of a popup, so the popup does
 * not pay for the context). A plaza's canvas moves with it; the Machines view keeps its own, which the popup shows.
 */
let plazaGl: THREE.WebGLRenderer | null | undefined;
function plazaRenderer(): THREE.WebGLRenderer | null {
  if (plazaGl === undefined) plazaGl = newRenderer();
  return plazaGl;
}
/** Which plaza's frame loop owns the shared renderer: a torn-down plaza stops the loop only if it still owns it. */
let loopOwner: object | null = null;

/** The plaza of one machine (today's Floor). Without WebGL the 2.5D canvas floor takes over. */
function mountPlaza(root: HTMLElement, machineId: string | null, warm: { fleet: FleetResponse } | null = null): Cleanup {
  // The shared renderer (cached): null when the browser refuses WebGL, which is the 2.5D canvas floor's cue. A probe
  // canvas per mount would keep one more WebGL context alive each time (a context is released only when it is collected).
  const renderer = plazaRenderer();
  if (!renderer) return mountFloor2d(root);
  const palette = readPalette();
  // The scene's own sci-fi grade, the same in either console theme: hull slate and steel for the
  // plaza, neon mint and cyan for the status lights, and the register's signal orange.
  const C = {
    sheet: new THREE.Color(0x2c3554),
    rule: new THREE.Color(0x56638a),
    ink3: new THREE.Color(0x7d8bb5),
    signal: colour(palette.signal, '#ff7a2e'),
    ok: new THREE.Color(0x3ee08f),
    info: new THREE.Color(0x4cc9ff),
  };
  // The rooms keep their own light paper (documents, signs) under the night lighting.
  const roomPalette = { ...C, sheet: new THREE.Color(0xe9edf7), rule: new THREE.Color(0xa9b4cc) };

  // Renderer, scene, camera, controls: the night and the orbit are the same in every view.
  const canvas = viewCanvas(renderer, 'The plaza floor');
  const loopToken = {}; // this plaza's frame loop, for the teardown (loopOwner)
  const { scene, ground, backdrop, skyLayer } = nightScene();
  const { camera, controls } = orbitView(canvas);
  // The board floats on the night: no skyline, and a dark ground sheet well under it. An ENTER starts in the
  // Machines view's look (the skyline and the night ground) and turns to this one as the camera dives (see `look`).
  // A warm plaza (the popup's machine, built hidden) takes its handoff from ENTER, in adopt below.
  let handoff: Handoff | null = machineId === null || warm !== null ? null : takeEntry(machineId);
  let host = root; // the plaza's DOM: the warm container until adopt moves it to the visible root
  let warmShown = warm === null; // a warm plaza draws only once adopted
  let bodyFloor = false;
  const groundNight = ground.material as THREE.Material;
  const groundDark = toonMat({ color: 0x0b0d1f });
  backdrop.visible = handoff !== null;
  if (handoff === null) {
    groundNight.dispose();
    ground.material = groundDark;
    ground.position.y = -BOARD_DROP;
  }

  // Shared resources: the rooms of the board, and the board itself once the machine is known.
  const districtGroup = new THREE.Group();
  scene.add(districtGroup);
  let board: BoardScene | null = null;
  let layoutNow: BoardLayout | null = null;
  // Robot navigation: one world per plaza, on the grid of the board and its rooms; the lifts and the overlay (nav=1) follow the grid.
  let nav: NavWorld | null = null;
  let navLifts: { id: number; prop: LiftProp; low: number; high: number }[] = [];
  let navRamps: THREE.Group[] = [];
  let navGangways: Gangway[] = [];
  let navOverlay: NavOverlay | null = null;
  const navDebug = new URLSearchParams(location.hash.split('?')[1] ?? '').get('nav') === '1';
  const padGeo = new THREE.CircleGeometry(0.42, 28);
  const packetGeo = new THREE.SphereGeometry(0.13, 14, 10);
  const haloGeo = new THREE.SphereGeometry(0.34, 14, 10);
  const tokenGeo = new THREE.BoxGeometry(0.2, 0.2, 0.2);
  const mats = {
    accent: new THREE.MeshBasicMaterial({ color: C.signal, toneMapped: false }),
    ok: new THREE.MeshBasicMaterial({ color: C.ok, toneMapped: false }),
    accentHalo: new THREE.MeshBasicMaterial({ color: C.signal, transparent: true, opacity: 0.35, depthWrite: false, blending: THREE.AdditiveBlending, toneMapped: false }),
    okHalo: new THREE.MeshBasicMaterial({ color: C.ok, transparent: true, opacity: 0.35, depthWrite: false, blending: THREE.AdditiveBlending, toneMapped: false }),
    token: new THREE.MeshBasicMaterial({ color: C.signal, toneMapped: false }),
  };

  // Dust: sparse motes drifting over the plaza.
  const dustPos = new Float32Array(DUST_N * 3);
  const dustAttr = new THREE.BufferAttribute(dustPos, 3);
  const dustSeeds: { x: number; y: number; z: number; rise: number }[] = [];
  for (let i = 0; i < DUST_N; i++) {
    const a = Math.random() * Math.PI * 2;
    const r = Math.sqrt(Math.random()) * 9;
    dustSeeds.push({ x: Math.cos(a) * r, y: Math.random() * 4.2, z: Math.sin(a) * r, rise: 0.04 + Math.random() * 0.05 });
  }
  const dustGeo = new THREE.BufferGeometry();
  dustGeo.setAttribute('position', dustAttr);
  const dustMat = new THREE.PointsMaterial({ color: 0x7fe3ff, size: 0.08, transparent: true, opacity: 0.8, depthWrite: false, blending: THREE.AdditiveBlending });
  scene.add(new THREE.Points(dustGeo, dustMat));

  // DOM: stage with the canvas, name labels, the card, the legend.
  const card = h('div.floor-card', { hidden: true });
  const legend = h(
    'div.floor-legend',
    null,
    h('span', null, h('i.floor-key.working'), 'Working'),
    h('span', null, h('i.floor-key.idle'), 'Idle'),
    h('span', null, h('i.floor-key.offline'), 'Offline'),
  );
  const tags = document.createElement('div');
  tags.style.cssText = 'position:absolute;inset:0;overflow:hidden;pointer-events:none';
  // The loading card of an arrival that waits for its build (updateLoading): a thin progress bar and the percentage.
  const loadBar = h('i.floor-loading-bar');
  const loadName = h('span', null, machineId ?? '');
  const loadPctEl = h('span.floor-loading-pct', null, '0%');
  const loader = h('div.floor-loading', { hidden: true, role: 'status', 'aria-live': 'polite' }, h('div.floor-loading-head', null, h('span', null, 'Preparing ', loadName, '…'), loadPctEl), h('div.floor-loading-track', null, loadBar));
  const stage = h('div.floor-stage.floor-stage-3d', null, tags, card, legend, loader);
  stage.style.position = 'relative';
  stage.prepend(canvas);
  if (warmShown) {
    root.classList.add('floor-view');
    document.body.classList.add('floor-mode');
    bodyFloor = true;
  }
  // Inside a machine the header names it: "Machines › name", and Back returns to the Machines view.
  const crumbName = h('span', null, machineId ?? ''); // the fleet names it once read (applyFleet)
  root.append(
    h(
      'div.view-head',
      null,
      h('h1', null, 'Floor'),
      machineId === null ? null : h('nav.floor-crumb', { 'aria-label': 'Breadcrumb' }, h('a', { href: href('floor') }, 'Machines'), ' › ', crumbName),
      h(
        'div.actions',
        null,
        machineId === null ? null : button('Back to machines', () => leavePlaza()),
        button('Reset view', () => {
          driftStopped = false; // the Reset view button is a reset: the idle sway starts again
          resetView();
        }, { icon: 'refresh' }),
      ),
    ),
    stage,
  );

  let districtSig = '';
  let bots: Bot[] = [];
  // Debug (window.__apzLiftRide): a robot walking a ride goal in place of its work goal, until it arrives or its time is up.
  const rides = new Map<number, { goal: { x: number; z: number }; left: number }>();
  const byCid = new Map<number, Bot>();
  const packets: Packet[] = [];
  const litByRole = new Map<string, DistrictLight>();
  const propAnim: ((t: number) => void)[] = [];
  const tmpV = new THREE.Vector3();
  let width = 0;
  let height = 0;
  let hovered: Bot | null = null;
  let selected: Bot | null = null;
  let cardSig = '';
  let fitted = false;
  // The top of the rooms (set by buildWorld) and whether the user has moved the camera; until they do, the default view refits.
  let roomsTop = 2;
  let userMoved = false;
  // The settled plaza's idle sway (updateDrift): stopped by any input until the next reset or arrival.
  let driftStopped = false;
  let driftT = 0; // the sway's clock (seconds)
  let driftApplied = 0; // how far the camera has been turned so far (radians)
  // The ENTER sequence while it plays (and after it, until a Back): the view is not refit while it drives the camera.
  let entry: EntrySequence | null = null;
  // A plaza that has arrived (ENTER) waits for its build before the flight starts; the loading card shows the progress.
  let pendingFlight = false;
  let loadPct = 0;
  let directWaved = false; // a direct load plays its build wave once
  let poseOf: { board: BoardScene; layout: BoardLayout | null; pose: Pose } | null = null;
  // The machine this plaza shows: its members (the cids the fleet names; empty until a fleet read, so no robots before),
  // its status and its board. Only a successful fleet read changes the members; a failed read keeps the last known ones.
  let members = new Set<number>();
  let fleetRead = false; // a fleet read has named the machine (members and board are its)
  let machineStatus: FleetStatus = 'online';
  let lastList: Agent[] = [];
  let alive = true;
  let settingView = false; // resetView's own camera moves are not the user's
  let lastInput = -Infinity; // the user's last camera move: the plaza draws at the full rate while it lasts (see frame)
  let robotsBusy = false; // a robot walks, works or rides this frame (updateBots)
  let plazaBusy = true; // the last drawn frame's verdict: full rate while busy, the idle rate when only the sway and ambient motion remain
  controls.addEventListener('change', () => {
    if (!settingView) {
      userMoved = true;
      lastInput = performance.now();
    }
  });
  let last = performance.now();
  const ndcVec = new THREE.Vector2();
  const raycaster = new THREE.Raycaster();
  const camDir = new THREE.Vector3();
  const UP = new THREE.Vector3(0, 1, 0);
  const slopeN = new THREE.Vector3();
  const tiltQ = new THREE.Quaternion();
  const yawQ = new THREE.Quaternion();
  // Any orbit, pan or zoom gesture stops the idle sway (pointer and wheel input on the stage does too, below).
  controls.addEventListener('start', () => {
    driftStopped = true;
  });

  // Plaques and rooms.
  function plaqueTexture(text: string, orch: boolean): THREE.CanvasTexture {
    const cv = document.createElement('canvas');
    cv.width = 512;
    cv.height = 128;
    const g = cv.getContext('2d')!;
    g.beginPath();
    g.moveTo(24, 4);
    g.arcTo(508, 4, 508, 124, 20);
    g.arcTo(508, 124, 4, 124, 20);
    g.arcTo(4, 124, 4, 4, 20);
    g.arcTo(4, 4, 508, 4, 20);
    g.closePath();
    g.fillStyle = palette.sheet;
    g.fill();
    g.lineWidth = 6;
    g.strokeStyle = orch ? palette.signal : palette.rule;
    g.stroke();
    g.fillStyle = palette.ink2;
    g.textAlign = 'center';
    g.textBaseline = 'middle';
    const base = '500 48px ui-monospace, SFMono-Regular, Menlo, monospace';
    g.font = base;
    const w = textWidth(g, text);
    if (w > 460) g.font = `500 ${Math.floor((48 * 460) / w)}px ui-monospace, SFMono-Regular, Menlo, monospace`;
    g.fillText(text, 256, 66);
    const tex = new THREE.CanvasTexture(cv);
    tex.colorSpace = THREE.SRGBColorSpace;
    return tex;
  }

  /**
   * A room on its component: a thin terrace plate seated on the component's top (its top at the slot's top, nothing
   * below the component's top), the room's floor at the slot's top, and the role's plaque at its open front. The
   * ramps and lifts to the board are placed by the nav grid (applyNav). `index` is the district's place in the layout
   * (with the machine and role, it seeds the room's generator).
   */
  function districtMesh(d: District, index: number, support: Support): THREE.Group {
    const g = new THREE.Group();
    const orch = d.role === 'orchestrator';
    const round = d.shape === 'round';
    const hs = Math.min(d.w, d.d) / 2; // the cell's half-size: the terrace's flat top
    g.add(platePieces(d, support)); // the plate rests on what is under it, columns where it stands over lower ground (board-rooms.ts)
    // Neon in the district's hue along the terrace's top edge.
    const neon = new THREE.MeshBasicMaterial({ color: new THREE.Color().setHSL(d.hue / 360, 0.95, 0.62, THREE.SRGBColorSpace), toneMapped: false });
    for (const [rr, y] of [[hs, d.y + 0.01]] as const) {
      const edge = round
        ? new THREE.Mesh(new THREE.TorusGeometry(rr, 0.045, 6, 72), neon)
        : new THREE.Mesh(new THREE.TorusGeometry(rr * Math.SQRT2, 0.045, 6, 4), neon);
      edge.rotation.set(Math.PI / 2, 0, round ? 0 : Math.PI / 4);
      edge.position.set(d.cx, y, d.cy);
      edge.userData.outline = false;
      g.add(edge);
    }
    // The room's turn, door, seed and desks come from the layout (roomPlan), the same values the nav grid reads. The turn
    // faces the aisle the room opens onto (a square room) or the board's centre (a round room). The room is 2r - 0.15
    // across: the cell less the wall margin. A slot below the role's minimum is built at the slot size, not enlarged
    // (enlarging would overlap the neighbours); buildRoom warns, and the room's required items may not place there.
    const plan = roomPlan(d, index, machineId, layoutNow);
    const turn = plan.turn;
    const { seed, door, agents } = plan;
    const room = buildRoom(d.role, d.r, d.hue, roomPalette, { seed, door, agents: Math.max(1, agents), w: 2 * d.r - 0.15, d: 2 * d.r - 0.15 });
    work.addRoom(d, turn, room.spots, room.layout.items); // the room's stations (where its robots work)
    // The floor covers the cell, or the built room where that is larger (its walls stand just outside the inner size).
    const top = new THREE.Mesh(
      round ? new THREE.CircleGeometry(d.r, 48) : new THREE.PlaneGeometry(Math.max(2 * d.r, room.w + 0.15), Math.max(2 * d.r, room.d + 0.15)),
      room.floor,
    );
    top.rotation.set(-Math.PI / 2, 0, turn);
    top.position.set(d.cx, d.y + 0.004, d.cy);
    top.receiveShadow = true;
    g.add(top);
    room.group.position.set(d.cx, d.y, d.cy);
    room.group.rotation.y = turn;
    g.add(room.group);
    room.group.updateMatrixWorld(true);
    litByRole.set(d.role, { screens: room.screens, lit: false });
    propAnim.push(room.animate);
    // Role plaque lies flat on the room's open front, readable from the board.
    const holder = new THREE.Group();
    holder.position.set(d.cx, d.y + 0.012, d.cy);
    holder.rotation.y = Math.atan2(-d.cx, -d.cy);
    const plaque = new THREE.Mesh(
      new THREE.PlaneGeometry(2.6, 0.65),
      new THREE.MeshBasicMaterial({ map: plaqueTexture(d.role.toUpperCase(), orch), transparent: true, depthWrite: false }),
    );
    plaque.rotation.x = -Math.PI / 2;
    plaque.position.set(0, 0, d.r - 0.5);
    holder.add(plaque);
    g.add(holder);
    outline(g);
    return g;
  }

  /** The rooms, rebuilt whenever the layout changes; the old ones are disposed first. */
  /** The district build in progress: its districts are built and baked one group per step, across frames (stepWorld). */
  let worldJob: { ds: BoardDistrict[]; pcb: number; next: number; support: Support; drawn: Support[] } | null = null;
  /** The drawn supports of the built world (board, rooms, aisles), once it is complete: the nav's walk heights bake from them. */
  let drawnSups: Support[] | null = null;
  /** The plaza's budgeted jobs (floor3d/warm.ts): the exterior, the board, its nav grid. The frame loop runs them. */
  const jobs = new JobQueue();
  let boardPending: string | null = null; // the key of the board a job is building

  /**
   * Starts the districts of a new world (the old one is disposed here). The districts are built by stepWorld, a few per
   * frame, so no frame carries the whole plaza: the default view and the ENTER sequence's dive wait for the last one.
   */
  function buildWorld(ds: BoardDistrict[], pcb: number, support: Support): void {
    for (const child of [...districtGroup.children]) {
      districtGroup.remove(child);
      disposeTree(child);
    }
    litByRole.clear();
    propAnim.length = 0;
    work.beginRooms();
    drawnSups = null; // the nav's walk heights wait for the new world (its grid heights until then)
    nav?.setDrawn(null);
    worldJob = { ds, pcb, next: 0, support, drawn: [support] };
  }

  /** Builds and bakes districts of the world job until the budget (ms) is spent, one group each (its own bake). */
  function stepWorld(budgetMs: number): void {
    const job = worldJob;
    if (!job) return;
    const until = performance.now() + budgetMs;
    while (job.next < job.ds.length) {
      const g = districtMesh(job.ds[job.next]!, job.next, job.support);
      job.drawn.push({ field: supportField(g), scale: 1 }); // the drawn tops, read before the bake merges the meshes
      bakeStatic(g, { shadowProxies: true }); // bake this group alone: the world is not rebaked as a whole
      ownRoomMaterials(g); // the room's own material copies, for the ENTER sequence's fade
      districtGroup.add(g);
      job.next++;
      if (performance.now() >= until) break;
    }
    if (job.next >= job.ds.length) {
      worldJob = null;
      drawnSups = job.drawn;
      nav?.setDrawn(drawnSups);
      // The new rooms' shaders, a material a step (warm.ts); the night ground's material too, which ENTER swaps in.
      if (renderer) jobs.replace('compile-rooms', compileSteps(renderer, scene, camera, [new THREE.Mesh(ground.geometry, groundNight), new THREE.Mesh(ground.geometry, groundDark)]));
      const box = new THREE.Box3().setFromObject(districtGroup);
      roomsTop = box.isEmpty() ? 2 : box.max.y;
      if (!userMoved) resetView();
    }
  }

  /**
   * A machine's board as the plaza: the model at the origin with its copper traces, the current running along each
   * component's traces, the atom over its anchor, and a glow plate over each slot (lit while its rooms work).
   */
  function* buildBoard(model: Model, params: Params, key: string): Generator<void, BoardScene, void> {
    const group = new THREE.Group();
    const built = yield* takeBuild(key, model, params); // a part at a time, or the pooled model of this board
    group.add(built.group, traceStrips(model));
    yield;
    const plates = new Map<string, GlowPlate>();
    for (const slot of model.slots ?? []) {
      if (!built.parts.has(slot.on)) continue; // a slot whose part is hidden by its `when` does not exist
      const [x0, z0, w, d] = slot.rect;
      const mat = new THREE.MeshBasicMaterial({ color: C.info, transparent: true, opacity: 0, depthWrite: false, blending: THREE.AdditiveBlending, toneMapped: false });
      const mesh = new THREE.Mesh(new THREE.PlaneGeometry(w, d), mat);
      mesh.rotation.x = -Math.PI / 2;
      mesh.position.set(x0 + w / 2, slot.top + 0.012, z0 + d / 2);
      mesh.userData.outline = false;
      mesh.userData.dynamic = true; // its opacity and visibility follow the work every frame
      mesh.visible = false;
      group.add(mesh);
      plates.set(slot.id, { slot: slot.id, on: slot.on, mesh, mat, level: 0 });
    }
    // Each part's traces carry their own current, so a component's walkways light up with it.
    const walkways = new Map<string, Trace[]>();
    for (const tr of model.traces ?? []) {
      const on = tr.from ?? '';
      walkways.set(on, [...(walkways.get(on) ?? []), tr]);
    }
    const pulses = [...walkways].map(([on, traces]) => ({ on, pulses: currentPulses({ ...model, traces }) }));
    for (const p of pulses) group.add(p.pulses.group);
    const atom = placeAtom(model, group);
    prepareFade(group);
    scene.add(group);
    return { key, model, params, group, built, graph: traceGraph(model), pulses, plates, atom: atom?.animate ?? null, atomGroup: atom?.atom ?? null };
  }

  /** The nav grid of the board and its rooms, rebuilt with them: robots keep their places, the lifts and overlay follow the grid. */
  function rebuildNav(): void {
    if (!board || !layoutNow) return;
    jobs.replace('nav', applyNav(board, layoutNow));
  }

  /** The nav grid of a board and its layout, a room plan a step (navSteps), applied if the board and layout are still current. */
  function* applyNav(b: BoardScene, lay: BoardLayout): Generator<void, void, void> {
    const built = yield* navSteps(b.model, b.params, lay, machineId);
    if (board !== b || layoutNow !== lay) return;
    if (nav) nav.setGrid(built.grid);
    else {
      nav = new NavWorld(built.grid);
      nav.setDrawn(drawnSups);
      nav.onUnreachable((cid) => work.unreachable(cid));
      nav.onRespawn((cid) => work.unreachable(cid)); // a respawn at home ends the walk the same way: the next decision starts clean
    }
    for (const l of navLifts) {
      scene.remove(l.prop.group);
      l.prop.dispose();
    }
    navLifts = built.grid.lifts.map((lift) => {
      const prop = buildLift(lift, roomPalette); // the cab's look; its mechanics are the nav world's (NavWorld.lift)
      scene.add(prop.group);
      return { id: lift.id, prop, low: lift.low, high: lift.high };
    });
    // The gangways: one per plate whose landing is a gangway, bridging from the plate edge to its lift shaft.
    for (const g of navGangways) {
      scene.remove(g.group);
      g.dispose();
    }
    navGangways = lay.plates.flatMap((p) => (p.landing?.kind === 'gangway' ? [buildGangway(p.landing, p.y, lay.pcbTop, roomPalette)] : []));
    for (const g of navGangways) scene.add(g.group);
    // The ramps: one per ramp portal (a plate landing at most NAV_LIFT_RISE above the PCB), from the plate cell to the board.
    for (const r of navRamps) {
      scene.remove(r);
      disposeRamp(r);
    }
    navRamps = built.grid.portals.flatMap((p) => (p.kind === 'ramp' ? [rampForPortal(built.grid, p, roomPalette).group] : []));
    for (const r of navRamps) scene.add(r);
    if (navOverlay) navOverlay.setGrid(built.grid);
    else if (navDebug) {
      navOverlay = buildNavOverlay(built.grid);
      scene.add(navOverlay.group);
    }
  }

  // Robots, labels, pads.
  function createBot(agent: Agent, district: District, hx: number, hy: number): Bot {
    const orch = agent.tier === 'orchestrator';
    const robot = makeRobot(district.hue, orch, ROBOT_PALETTE, agentHue(agent.cid), speciesOf(agent.species, agent.cid));
    robot.group.updateMatrixWorld(true); // not in the scene yet: local is world, so the part scales below are right
    robot.group.traverse((o) => {
      o.castShadow = o instanceof THREE.Mesh && robotPartCasts(o);
      o.receiveShadow = true;
    });
    robot.group.userData.dynamic = true; // walks, turns, is picked
    scene.add(robot.group);
    const padMat = new THREE.MeshBasicMaterial({ color: C.ink3, transparent: true, opacity: 0.6, depthWrite: false });
    const pad = new THREE.Mesh(padGeo, padMat);
    pad.userData.dynamic = true; // tinted by state every frame
    pad.rotation.x = -Math.PI / 2;
    pad.position.set(hx, topOf(district) + 0.02, hy);
    scene.add(pad);
    const token = new THREE.Mesh(tokenGeo, mats.token);
    token.userData.dynamic = true; // bobs and spins while its robot works
    token.visible = false;
    scene.add(token);
    // The label: a status square in the legend's colours, then the name. Activity and role live on the card.
    const label = document.createElement('div');
    label.style.cssText = LABEL_CSS;
    const labelKey = document.createElement('i');
    labelKey.className = 'floor-key';
    const labelName = document.createElement('span');
    labelName.style.cssText = 'overflow:hidden;text-overflow:ellipsis;min-width:0';
    label.append(labelKey, labelName);
    tags.append(label);
    // The robot walks the nav grid at its species' gait speed (0.9 when the species has none).
    nav?.add({ id: agent.cid, x: hx, z: hy, speed: registry().species[robot.species]?.gait.speed ?? 0.9, turnRate: 4, priority: 0, floats: registry().species[robot.species]?.floats ?? false });
    return {
      agent,
      district,
      home: [hx, hy],
      x: hx,
      y: hy,
      heading: 0,
      goalKey: '',
      moving: false,
      py: 0,
      state: null,
      activity: 'idle',
      robot,
      robotKey: `${district.hue}|${orch}`,
      tilt: new THREE.Quaternion(),
      pad,
      padMat,
      token,
      highlighted: false,
      label,
      labelKey,
      labelName,
    };
  }

  function removeBot(b: Bot): void {
    nav?.remove(b.agent.cid);
    scene.remove(b.robot.group);
    b.robot.dispose();
    scene.remove(b.pad);
    b.padMat.dispose();
    scene.remove(b.token);
    b.label.remove();
  }

  /** Lays the agents on the board and places their robots: the rooms rebuild when the layout changes. */
  function sync(list: Agent[]): void {
    lastList = list;
    if (!board) return; // the board comes with the machine (refresh); until then there is nothing to lay out
    const mine = list.filter((a) => members.has(a.cid));
    const cached = layoutCached(board.key, mine);
    if (cached === null) {
      // A new roster or board: the layout is computed as a job (the frame budget), then this sync runs again on a hit.
      const b = board;
      jobs.replace('layout', (function* (): Generator<void, void, void> {
        layoutFor(b.key, mine, b.model, b.params);
        if (board === b) sync(lastList);
      })());
      return;
    }
    const laid = cached;
    layoutNow = laid;
    // The board's key (its model and params) is part of the signature: a new board changes the trace graph the doors use.
    const sig = `${board.key}|${JSON.stringify(laid.districts)}`;
    if (sig !== districtSig) {
      districtSig = sig;
      // The board's world scale (layout.boardScale): its group and its walkway graph follow the scaled layout.
      board.group.scale.setScalar(laid.boardScale);
      board.graph = traceGraph(scaleModel(board.model, laid.boardScale));
      buildWorld(laid.districts, laid.pcbTop, { field: board.built.support, scale: laid.boardScale });
      work.setBoard(board.model, board.params, board.graph, laid);
      rebuildNav();
    }
    work.setLayout(laid);
    const seen = new Set<number>();
    for (const agent of mine) {
      const slot = laid.homes.get(agent.cid);
      if (!slot) continue;
      const [district, hx, hy] = slot as [District, number, number];
      seen.add(agent.cid);
      let bot = byCid.get(agent.cid);
      if (bot && bot.robotKey !== `${district.hue}|${agent.tier === 'orchestrator'}`) {
        removeBot(bot);
        bot = undefined;
      }
      if (!bot) {
        // A new robot is made a budgeted step at a time (stepBots), not all in this task.
        pendingBots = pendingBots.filter((q) => q.agent.cid !== agent.cid);
        pendingBots.push({ agent, district, hx, hy });
        continue;
      }
      Object.assign(bot, { agent, district, home: [hx, hy] as [number, number] });
      nav?.setHome(agent.cid, hx, hy); // the layout moved the home (the board scale follows the agent count): a respawn returns there
      bot.pad.position.set(hx, topOf(district) + 0.02, hy);
    }
    for (const [cid, bot] of byCid) {
      if (seen.has(cid)) continue;
      removeBot(bot);
      byCid.delete(cid);
    }
    bots = [...byCid.values()];
    if (selected) selected = byCid.get(selected.agent.cid) ?? null;
    if (hovered) hovered = byCid.get(hovered.agent.cid) ?? null;
    renderCard();
  }

  /** Robots waiting to be made (sync queues them). stepBots makes them a few per frame, within the budget. */
  let pendingBots: { agent: Agent; district: District; hx: number; hy: number }[] = [];
  let robotsCompiled = false; // the robots' shaders compile once their first batch is made (their materials are new)
  function stepBots(budgetMs: number): void {
    if (pendingBots.length === 0 || jobs.has('nav')) return; // a robot joins the nav grid it is made on (createBot): the grid comes first
    const until = performance.now() + budgetMs;
    while (pendingBots.length > 0) {
      const p = pendingBots.shift()!;
      if (!byCid.has(p.agent.cid)) {
        const bot = createBot(p.agent, p.district, p.hx, p.hy);
        byCid.set(p.agent.cid, bot);
        Object.assign(bot, { agent: p.agent, district: p.district, home: [p.hx, p.hy] as [number, number] });
        bot.pad.position.set(p.hx, topOf(p.district) + 0.02, p.hy);
      }
      if (performance.now() >= until) break;
    }
    bots = [...byCid.values()];
    if (pendingBots.length === 0 && !robotsCompiled && renderer) {
      robotsCompiled = true;
      jobs.replace('compile-robots', compileSteps(renderer, scene, camera)); // the robots' programs, before ENTER pops them in
    }
    renderCard();
  }

  /** Shows the named board with its params, building it unless it is the one already shown, then lays the agents on it. */
  function showBoard(name: string, params: Params): void {
    const key = `${name}|${JSON.stringify(params)}`;
    if (board?.key === key) {
      sync(lastList);
      return;
    }
    if (boardPending === key && jobs.has('board')) return; // this board is already being built
    boardPending = key;
    jobs.replace('board', boardJob(MODELS[name] ?? MODELS[DEFAULT_BOARD.name]!, params, key));
  }

  /** The board, built a budgeted step at a time (buildBoard); it replaces the shown one, then the agents are laid on it. */
  function* boardJob(model: Model, params: Params, key: string): Generator<void, void, void> {
    const next = yield* buildBoard(model, params, key);
    boardPending = null;
    if (board) retireBoard(board);
    board = next;
    if (renderer) jobs.replace('compile-board', compileSteps(renderer, scene, camera)); // the board's programs before its fade shows them
    sync(lastList);
  }

  /** Takes a board off the scene and frees it. Its built model goes back to the pool (warm.ts), not freed: it may come back. */
  function retireBoard(b: BoardScene): void {
    scene.remove(b.group);
    b.group.remove(b.built.group); // before the tree is freed: the built model is pooled, not disposed
    disposeTree(b.group);
    releaseBuild(b.key, b.built);
  }

  /** Reads the fleet and shows its machine (the one named, else the hub) with its agents and status. */
  function applyFleet(fleet: FleetResponse): void {
    const m = fleet.machines.find((x) => x.id === machineId) ?? fleet.machines.find((x) => x.role === 'hub') ?? fleet.machines[0];
    if (!m) return;
    if (machineId !== null) crumbName.textContent = m.name;
    const { name, params } = boardOf(m);
    members = new Set(m.agents);
    fleetRead = true;
    machineStatus = m.status;
    showBoard(name, params);
  }

  /**
   * Reads the fleet every few seconds. A read that fails keeps the machine shown last (its board and its members);
   * before any fleet read, the machines list's first node (n0, the hub) shows its board, and failing that the
   * workstation board. The members are never "all agents": until a fleet read names them there are no robots.
   */
  async function refresh(): Promise<void> {
    let fleet: FleetResponse | null = null;
    try {
      fleet = await api.fleet();
    } catch {
      fleet = null; // read below: a failed read is not a rendering error (applyFleet runs outside this catch)
    }
    if (fleet !== null && fleet.machines.length > 0) {
      if (alive) applyFleet(fleet);
      return;
    }
    if (!alive || fleetRead) return; // the last known machine stays
    try {
      const nodes = await api.machines();
      const node = nodes.find((n) => n.id === 'n0') ?? nodes[0];
      if (alive && node && !fleetRead) {
        const { name, params } = boardOf(node);
        showBoard(name, params);
        return;
      }
    } catch {
      // the workstation board below
    }
    if (alive && !fleetRead) showBoard(DEFAULT_BOARD.name, DEFAULT_BOARD.params);
  }

  /** Each frame: the board's fans and lights follow the machine, glow plates ease toward their components' work, and a lit component's current runs faster. */
  function updateBoard(dt: number, t: number): void {
    if (!board) return;
    const working = new Set<string>(); // the slots whose rooms have a working agent
    let busy = 0;
    for (const b of bots) {
      if (b.agent.status !== 'working') continue;
      busy++;
      if (b.district.slot) working.add(b.district.slot);
    }
    const share = bots.length ? busy / bots.length : 0;
    board.built.update(dt, { status: machineStatus, working: share, load: share });
    const ease = 1 - Math.exp(-4 * dt);
    for (const plate of board.plates.values()) {
      plate.level += ((working.has(plate.slot) ? 1 : 0) - plate.level) * ease;
      plate.mat.opacity = GLOW_OPACITY * plate.level;
      plate.mesh.visible = plate.level > 0.01;
    }
    for (const p of board.pulses) {
      let level = 0;
      for (const plate of board.plates.values()) if (plate.on === p.on) level = Math.max(level, plate.level);
      p.pulses.update(dt, level);
    }
    board.atom?.(t);
    // Detail by distance; the cutaways toward the board's atom (its centre of interest).
    const at = board.model.anchors?.atom;
    if (at) board.built.group.localToWorld(focusPoint.set(at[0], at[1], at[2]));
    else focusPoint.copy(controls.target);
    board.built.frame(camera, focusPoint, dt, plazaArea());
  }

  /**
   * The default view: the camera distance at which the plaza fills FIT_FILL of the stage on its tighter axis, and the
   * point it is aimed at. The board's footprint (its corners, at the floor and at the rooms' top) is projected;
   * the nearest distance where every point is in front of the lens and inside that share of the frame is found by
   * bisection. Perspective puts the near side of the ring lower on screen than the far side is high, so the aim moves
   * to the ring's projected middle and the fit is run again.
   */
  /** The rooms that seat a robot: the layout's districts with a home (layout.homes), in the layout's order. */
  function occupiedRooms(): BoardDistrict[] {
    if (!layoutNow) return [];
    const seated = new Set<object>(); // the homes' districts are the layout's own objects
    for (const [district] of layoutNow.homes.values()) seated.add(district);
    return layoutNow.districts.filter((r) => seated.has(r));
  }

  /**
   * The footprint a view frames, as its eight corners and their centre. Occupied: the rooms that seat a robot, from the
   * PCB's top to the rooms' top, with a margin of 15% of the box on each side (the default view). Otherwise the whole
   * board: its layout bounds from the floor to the rooms' top (the orbit's zoom limit reaches it).
   */
  function fitBox(occupied: boolean): { centre: THREE.Vector3; ring: THREE.Vector3[] } {
    const b = layoutNow?.bounds ?? { cx: 0, cz: 0, w: 64, d: 56 };
    const top = Math.max(roomsTop, (board?.model.anchors?.atom?.[1] ?? 0) * (layoutNow?.boardScale ?? 1) + 1);
    let lo = new THREE.Vector3(b.cx - b.w / 2, 0, b.cz - b.d / 2);
    let hi = new THREE.Vector3(b.cx + b.w / 2, top, b.cz + b.d / 2);
    const rooms = occupied ? occupiedRooms() : [];
    if (layoutNow && rooms.length > 0) {
      lo = new THREE.Vector3(Infinity, layoutNow.pcbTop, Infinity);
      hi = new THREE.Vector3(-Infinity, top, -Infinity);
      for (const r of rooms) {
        // A room's footprint: its cell, or the built room where that is larger (the floor is at least 2r across).
        const hx = Math.max(r.r, r.w / 2);
        const hz = Math.max(r.r, r.d / 2);
        lo.x = Math.min(lo.x, r.cx - hx);
        hi.x = Math.max(hi.x, r.cx + hx);
        lo.z = Math.min(lo.z, r.cy - hz);
        hi.z = Math.max(hi.z, r.cy + hz);
      }
      // A few rooms on a big board still frame a share of its footprint: one room alone does not fill the stage.
      for (const axis of ['x', 'z'] as const) {
        const want = (axis === 'x' ? b.w : b.d) * FRAME_MIN_SHARE;
        const mid = (lo[axis] + hi[axis]) / 2;
        const half = Math.max(want, hi[axis] - lo[axis]) / 2;
        lo[axis] = mid - half;
        hi[axis] = mid + half;
      }
      const margin = hi.clone().sub(lo).multiplyScalar(0.15);
      lo.sub(margin);
      hi.add(margin);
    }
    const ring: THREE.Vector3[] = [];
    for (const x of [lo.x, hi.x]) for (const z of [lo.z, hi.z]) for (const y of [lo.y, hi.y]) ring.push(new THREE.Vector3(x, y, z));
    return { centre: lo.clone().add(hi).multiplyScalar(0.5), ring };
  }

  /**
   * The direction the default view looks from: the board's own camera anchor (from its look point), else the isometric
   * default. A big board (BIG_BOARD across or more) keeps the anchor's heading but is seen from higher, BIG_POLAR from
   * vertical: its tall GPU shrouds hide the plates from the anchor's oblique angle.
   */
  function viewDir(): THREE.Vector3 {
    const cam = board?.model.anchors?.camera;
    let d = new THREE.Vector3().setFromSphericalCoords(1, POLAR, AZIMUTH);
    if (cam) {
      const v = new THREE.Vector3(cam.at[0] - cam.look[0], cam.at[1] - cam.look[1], cam.at[2] - cam.look[2]);
      if (v.lengthSq() > 1e-6) d = v.normalize();
    }
    const size = board?.model.size;
    if (size && Math.max(size[0], size[2]) >= BIG_BOARD) {
      const h = Math.hypot(d.x, d.z) || 1;
      d = new THREE.Vector3((d.x / h) * Math.sin(BIG_POLAR), Math.cos(BIG_POLAR), (d.z / h) * Math.sin(BIG_POLAR));
    }
    return d;
  }

  /** The plaza the default view frames (fitBox's occupied box), for the cutaways; found once per layout. */
  let plazaBox: { layout: unknown; box: THREE.Box3 } | null = null;
  function plazaArea(): THREE.Box3 {
    if (plazaBox?.layout !== layoutNow) {
      // The framed plaza plus a third of its size on each side: the shrouds beside it also stand in the view.
      const box = new THREE.Box3().setFromPoints(fitBox(true).ring);
      const grow = box.getSize(new THREE.Vector3()).multiplyScalar(1 / 3);
      box.min.x -= grow.x; box.max.x += grow.x; box.min.z -= grow.z; box.max.z += grow.z;
      plazaBox = { layout: layoutNow, box };
    }
    return plazaBox.box;
  }

  function fitView(occupied = true): { dist: number; target: THREE.Vector3 } {
    const { centre, ring } = fitBox(occupied);
    const target = centre;
    const dir = viewDir();
    const place = (dist: number): void => {
      camera.position.copy(target).addScaledVector(dir, dist);
      camera.lookAt(target);
      camera.updateMatrixWorld();
    };
    const fits = (dist: number): boolean => {
      place(dist);
      return ring.every((p) => {
        if (p.clone().applyMatrix4(camera.matrixWorldInverse).z >= 0) return false; // behind the lens
        const q = p.clone().project(camera);
        return Math.abs(q.x) <= FIT_FILL && Math.abs(q.y) <= FIT_FILL;
      });
    };
    const bisect = (): number => {
      let near = 1; // too close: the ring spills out of the frame
      let far = 4000; // far enough that the plaza is a speck, so it fits
      for (let i = 0; i < 40; i++) {
        const mid = (near + far) / 2;
        if (fits(mid)) far = mid;
        else near = mid;
      }
      return far;
    };
    const tanHalf = Math.tan(THREE.MathUtils.degToRad(camera.fov) / 2);
    let dist = bisect();
    for (let pass = 0; pass < 2; pass++) {
      place(dist);
      let lo = Infinity;
      let hi = -Infinity;
      for (const p of ring) {
        const y = p.clone().project(camera).y;
        lo = Math.min(lo, y);
        hi = Math.max(hi, y);
      }
      // Moving the aim up the screen's own vertical moves the plaza down the frame by the same share of the view.
      const up = new THREE.Vector3(0, 1, 0).applyQuaternion(camera.quaternion);
      target.addScaledVector(up, ((lo + hi) / 2) * dist * tanHalf);
      dist = bisect();
    }
    return { dist, target };
  }

  /** Where the camera sits for the default view: the plaza framed by fitView, the orbit aimed at its centre. */
  function resetView(): void {
    if (entry?.driving || pendingFlight) return; // the ENTER sequence owns the camera (and an arrival holds it at the Machines pose)
    const { dist, target } = fitView();
    controls.maxDistance = Math.max(300, fitView(false).dist * 2); // the orbit limit still reaches the whole board
    driftT = 0; // the idle sway starts again from the framed view
    driftApplied = 0;
    if (scene.fog instanceof THREE.Fog) {
      // The haze keeps its look at the fitted distance: the plaza stays clear and the far horizon still fades.
      scene.fog.near = dist * 0.8;
      scene.fog.far = dist * 3;
    }
    controls.target.copy(target);
    camera.position.copy(target).addScaledVector(viewDir(), dist);
    settingView = true;
    controls.update();
    settingView = false;
    userMoved = false;
  }

  /**
   * The idle sway of the settled plaza: the camera turns about the board's vertical axis through the framed target, a
   * few degrees either way (a triangle wave, so the framed board stays in view). It holds while an ENTER sequence drives,
   * while the plaza loads (defaultPose is null) and after any input (driftStopped). Its turn is applied inside
   * settingView, so the sway is not the user's move; the frame's controls.update() is unchanged.
   */
  function updateDrift(dt: number): void {
    if (driftStopped || entry?.driving || pendingFlight || defaultPose() === null) return;
    driftT += dt;
    const period = (4 * DRIFT_AMP) / DRIFT_RATE;
    const u = (driftT / period) % 1;
    const next = DRIFT_AMP * (u < 0.25 ? 4 * u : u < 0.75 ? 2 - 4 * u : 4 * u - 4);
    const turn = next - driftApplied;
    if (turn === 0) return;
    driftApplied = next;
    tmpV.copy(camera.position).sub(controls.target).applyAxisAngle(UP, turn);
    camera.position.copy(controls.target).add(tmpV);
    settingView = true;
    controls.update();
    settingView = false;
  }

  // The robots' work: the behaviour engine decides what each robot does (floor3d/robot-work); the floor walks it there.
  const work = new RobotWork();
  // Reachability for the work: a point's nav component (-1 off the walk); a robot only goes to spots in its own component.
  work.setReach((x, z) => nav?.debugCell(x, z)?.component ?? -1);

  /** The surface under (x, y): a room's floor, its ramp down to the PCB, or the PCB top (boardHeightAt). */
  function surfaceAt(x: number, y: number): number {
    return layoutNow ? boardHeightAt(x, y, layoutNow) : 0;
  }

  /** The body clip of a vertical mode (gravity and heights, nav-height.ts): none on the ground, in a lift or in the respawn fade in. */
  function bodyClip(mode: BodyMode): string | null {
    switch (mode) {
      case 'crouch': return 'jump.crouch';
      case 'jump': return 'jump.air';
      case 'fall':
      case 'glide':
      case 'out': return 'fall.air';
      case 'land': return 'land';
      default: return null;
    }
  }

  /**
   * THE MOVEMENT SEAM: walks one robot toward its goal (from robot-work) for one frame. With the nav world: the goal is
   * sent with navigateTo when its key changes, and the robot's position, height and moving flag come from world.pose;
   * returns the pose's yaw. Without it (the fallback): a straight-line step (robot-work's straightStep) on the robot's
   * own x and y; returns the travel yaw, or null when it stands. navigateTo is the only call into nav; nothing else moves a robot.
   */
  function moveRobot(world: NavWorld | null, b: Bot, g: WorkGoal | null, dt: number): number | null {
    if (!world) {
      b.robot.setBody(null);
      b.robot.group.scale.setScalar(1);
      if (!g) {
        b.moving = false;
        return null;
      }
      const travel = straightStep(b, g.goal.x, g.goal.z, dt);
      b.py = surfaceAt(b.x, b.y);
      return travel;
    }
    const cid = b.agent.cid;
    // A robot made while there was no nav world (createBot's nav?.add was a no-op) joins it here, at its own place.
    if (!world.has(cid)) world.add({ id: cid, x: b.x, z: b.y, speed: registry().species[b.robot.species]?.gait.speed ?? 0.9, turnRate: 4, priority: 0, floats: registry().species[b.robot.species]?.floats ?? false });
    if (g && b.goalKey !== g.key) {
      b.goalKey = g.key;
      navigateTo(world, cid, g.goal);
    }
    const pose = world.pose(cid);
    b.x = pose.x;
    b.y = pose.z;
    b.py = pose.y;
    b.moving = pose.moving;
    b.robot.setBody(bodyClip(pose.body));
    b.robot.group.scale.setScalar(pose.scale);
    return pose.heading;
  }

  // Debug (headless runs): window.__apzShove(cid, x, z) moves a robot without the commit (a forced push). Its body then falls,
  // lands or, past NAV_DROP_DOWN or off the board, respawns at home. False for an unknown robot or no nav world.
  if (typeof window !== 'undefined') {
    (window as unknown as { __apzShove?: (cid: number, x: number, z: number) => boolean }).__apzShove = (cid, x, z) => {
      if (!nav || !nav.has(cid)) return false;
      nav.shove(cid, x, z);
      return true;
    };
    // window.__apzNavCell(x, z): the nav cell at a point (walk, blocked, region, component, and its surface height y), or null.
    (window as unknown as { __apzNavCell?: (x: number, z: number) => unknown }).__apzNavCell = (x, z) => nav?.debugCell(x, z) ?? null;
    // window.__apzGroundAudit(): the grounding audit of the live board (ground.ts): every resting object that floats or sinks.
    (window as unknown as { __apzGroundAudit?: () => unknown }).__apzGroundAudit = () =>
      board && layoutNow ? groundAudit(boardScene(board.model, board.params, layoutNow, auditPalette())) : null;
    // window.__apzNavAudit(): the ground audit so far (robot-frames, floating and sinking grounded frames). Null without nav.
    (window as unknown as { __apzNavAudit?: () => unknown }).__apzNavAudit = () => nav?.audit() ?? null;
  }
  // Debug (headless runs): window.__apzLifts() lists each lift's place and its controller (lift-ctrl.ts); window.__apzLookAt(x, y, z,
  // dist, polar, azimuth) aims the camera at a point. Both are for captures only; the lift itself is not changed by them.
  if (typeof window !== 'undefined') {
    const w = window as unknown as { __apzLifts?: () => unknown; __apzLookAt?: (x: number, y: number, z: number, dist: number, polar: number, azimuth: number) => void };
    // x, z: the shaft's world position; yLow, yHigh: the stops' world heights (the cab's floor at each stop).
    w.__apzLifts = () => navLifts.map((l) => ({ id: l.id, low: l.low, high: l.high, x: l.prop.group.position.x, z: l.prop.group.position.z, yLow: l.low, yHigh: l.high, ctrl: nav?.lift(l.id), at: l.prop.group.position.toArray(), face: l.prop.group.rotation.y, meshes: ((k) => { l.prop.group.traverse((o) => { if ((o as THREE.Mesh).isMesh) k++; }); return k; })(0) }));
    w.__apzLookAt = (x, y, z, dist, polar, azimuth) => {
      controls.target.set(x, y, z);
      camera.position.set(x, y, z).add(new THREE.Vector3().setFromSphericalCoords(dist, polar, azimuth));
      camera.lookAt(controls.target);
    };
  }
  // Debug (headless runs): window.__apzLiftRide(i, n) sends the n robots nearest lift i (off its plate) on a real trip through
  // it: a goal on the plate beyond the lift's plate edge, walked by navigateTo, so the nav world boards them. Returns the goal
  // and the robot ids. A robot's ride ends on arrival or after 120 s; its work goal then takes over again.
  if (typeof window !== 'undefined') {
    (window as unknown as { __apzLiftRide?: (i: number, n: number) => unknown }).__apzLiftRide = (index, n) => {
      const l = navLifts[index];
      if (!nav || !l) return { ok: false, reason: 'no nav or no such lift' };
      const yaw = l.prop.group.rotation.y;
      const ox = l.prop.group.position.x;
      const oz = l.prop.group.position.z;
      // The robots' level decides the way: robots on the plate ride down to a goal on the board beyond the front, robots on
      // the board ride up to a goal on the plate beyond the plate edge.
      const fromHigh = bots.filter((b) => Math.abs(b.py - l.high) < 0.05 && !rides.has(b.agent.cid)).length >= Math.max(1, Math.floor(n));
      const sg = fromHigh ? 1 : -1; // +1: out toward the walkway (the board), -1: back toward the plate
      const lvl = fromHigh ? l.low : l.high;
      let goal: { x: number; z: number } | null = null;
      for (let d = 0.3; d < 3 && !goal; d += 0.05) {
        for (const lat of [0, 0.1, -0.1, 0.2, -0.2]) {
          const x = ox + sg * Math.sin(yaw) * d + Math.cos(yaw) * lat;
          const z = oz + sg * Math.cos(yaw) * d - Math.sin(yaw) * lat;
          const c = nav.debugCell(x, z);
          if (c && c.walk && (fromHigh ? c.region === 0 : c.region !== 0) && Math.abs(c.y - lvl) < 0.03) {
            goal = { x, z };
            break;
          }
        }
      }
      if (!goal) return { ok: false, reason: 'no walk cell beyond the lift on the other level' };
      const near = bots
        .filter((b) => Math.abs(b.py - (fromHigh ? l.high : l.low)) < 0.05 && !rides.has(b.agent.cid))
        .sort((a, b) => Math.hypot(a.x - ox, a.y - oz) - Math.hypot(b.x - ox, b.y - oz))
        .slice(0, Math.max(0, Math.floor(n)));
      for (const b of near) rides.set(b.agent.cid, { goal, left: 120 });
      return { ok: true, goal, robots: near.map((b) => b.agent.cid), bots: bots.length, ofPlate: bots.map((b) => [b.agent.cid, +b.x.toFixed(1), +b.y.toFixed(1), +b.py.toFixed(2)]) };
    };
  }
  // Debug (headless runs): window.__apzBots() reports each robot's work, goal and walk, the frame count, held and whether nav exists. Read-only.
  let botFrames = 0;
  /** Debug: a point's nav cell, its walk flag and its component (-1 off the walk), from nav's debugCell. */
  const probeCell = (x: number, z: number): unknown => nav?.debugCell(x, z) ?? null;
  if (typeof window !== 'undefined') {
    (window as unknown as { __apzBots?: () => unknown }).__apzBots = () => ({
      frames: botFrames,
      nav: nav !== null,
      held: plazaPaused(),
      bots: bots.map((b) => {
        const cid = b.agent.cid;
        let navPose: unknown = null;
        let navPath: number | null = null;
        if (nav) {
          try {
            navPose = nav.pose(cid);
            navPath = nav.path(cid)?.length ?? null;
          } catch (e) {
            navPose = String(e);
          }
        }
        return {
          cid,
          role: b.agent.role,
          tier: b.agent.tier,
          status: b.agent.status,
          activity: b.activity,
          state: b.state,
          goalKey: b.goalKey,
          moving: b.moving,
          pos: { x: +b.x.toFixed(3), y: +b.y.toFixed(3), py: +b.py.toFixed(3) },
          heading: +b.heading.toFixed(3),
          work: work.debugOf(cid),
          nav: navPose,
          navPath,
          navStart: probeCell(b.x, b.y),
          navGoal: probeCell((work.debugOf(cid) as { target: { x: number; z: number } | null }).target?.x ?? b.x, (work.debugOf(cid) as { target: { x: number; z: number } | null }).target?.z ?? b.y),
        };
      }),
    });
  }

  /** Robots: their decisions and goals (floor3d/robot-work), the walk (moveRobot), then their animation and pads. */
  function updateBots(dt: number, now: number): void {
    botFrames += 1;
    robotsBusy = false;
    const t = now / 1000;
    for (const p of litByRole.values()) p.lit = false;
    const world = nav;
    world?.step(dt);
    if (world) for (const l of navLifts) { const h = world.lift(l.id); if (h) l.prop.setState(h.state()); }
    work.frame(dt, bots, plazaPaused());
    for (const b of bots) {
      const status = b.agent.status;
      const cid = b.agent.cid;
      const ride = rides.get(cid);
      const g = ride ? { key: `ride:${cid}:${ride.goal.x.toFixed(2)},${ride.goal.z.toFixed(2)}`, goal: ride.goal, activity: 'ride' } : work.goalFor(b);
      const travel = moveRobot(world, b, g, dt);
      if (ride || b.moving || status === 'working') robotsBusy = true;
      if (ride) {
        ride.left -= dt;
        if (ride.left <= 0 || (!b.moving && Math.hypot(b.x - ride.goal.x, b.y - ride.goal.z) < 0.15)) rides.delete(cid);
      }
      // Yielding: a working robot keeps its way, a walking one yields to it, an idle one yields to both.
      if (world) world.setPriority(cid, status === 'working' ? 2 : b.moving ? 1 : 0);
      work.arrived(b);
      // Local +Z is forward. A station or a partner sets the heading; the rest face the way they walk (or the nav pose points).
      const want = work.facing(b);
      if (want !== null) {
        const diff = Math.atan2(Math.sin(want - b.heading), Math.cos(want - b.heading));
        b.heading += diff * Math.min(1, dt * 8);
      } else if (travel !== null) {
        b.heading = travel;
      }
      const state: RobotState = status === 'working' ? 'working' : status === 'offline' ? 'offline' : b.moving ? 'walking' : 'idle';
      if (state !== b.state) {
        b.state = state;
        b.robot.setState(state);
      }
      const highlight = b === selected || b === hovered;
      if (highlight !== b.highlighted) {
        b.highlighted = highlight;
        b.robot.setHighlighted(highlight);
      }
      b.robot.group.position.set(b.x, b.py, b.y);
      // Lean with the ground: the slope's normal from the surface around the robot, then the heading.
      const e = 0.3;
      const sx = (surfaceAt(b.x + e, b.y) - surfaceAt(b.x - e, b.y)) / (2 * e);
      const sz = (surfaceAt(b.x, b.y + e) - surfaceAt(b.x, b.y - e)) / (2 * e);
      slopeN.set(-sx, 1, -sz).normalize();
      tiltQ.setFromUnitVectors(UP, slopeN).multiply(yawQ.setFromAxisAngle(UP, b.heading));
      b.tilt.slerp(tiltQ, 1 - Math.exp(-10 * dt));
      b.robot.group.quaternion.copy(b.tilt);
      b.robot.update(dt, t);
      b.padMat.color.copy(status === 'working' ? C.info : status === 'idle' ? C.ok : C.ink3);
      // Working robots stay at their desk, carrying a glowing task token; their district's monitors light up.
      const working = status === 'working';
      b.token.visible = working;
      if (working) {
        const lit = litByRole.get(b.district.role);
        if (lit) lit.lit = true;
        const head = b.robot.labelAnchor(tmpV);
        b.token.position.set(head.x, head.y + 0.6 + Math.sin(t * 2.5 + b.agent.cid) * 0.08, head.z);
        b.token.rotation.y += dt * 1.8;
      }
    }
    if (world) navOverlay?.update(world, bots.map((b) => b.agent.cid));
    for (const p of litByRole.values()) {
      const want = p.lit ? 1.2 : 0.05;
      for (const m of p.screens) if (m.emissiveIntensity !== want) m.emissiveIntensity = want;
    }
  }

  function updateDust(t: number): void {
    for (let i = 0; i < DUST_N; i++) {
      const d = dustSeeds[i]!;
      const k = i * 3;
      dustPos[k] = d.x + Math.sin(t * 0.3 + i) * 0.25;
      dustPos[k + 1] = 0.3 + ((d.y + t * d.rise) % 4.2);
      dustPos[k + 2] = d.z + Math.cos(t * 0.25 + i) * 0.25;
    }
    dustAttr.needsUpdate = true;
  }

  // Packets: glowing spheres on a curved arc between two robots.
  function sendPacket(from: number, to: number, tone: 'accent' | 'ok'): void {
    // A packet is a world-space sphere, so none is sent while the ENTER sequence drives the scene (its board is scaled).
    if (from === to || entry?.driving) return;
    const core = new THREE.Mesh(packetGeo, tone === 'ok' ? mats.ok : mats.accent);
    const halo = new THREE.Mesh(haloGeo, tone === 'ok' ? mats.okHalo : mats.accentHalo);
    core.userData.dynamic = true; // moved along its arc every frame
    halo.userData.dynamic = true;
    scene.add(core, halo);
    packets.push({ from, to, start: performance.now(), core, halo });
    while (packets.length > MAX_PACKETS) dropPacket(packets.shift()!);
  }

  function dropPacket(p: Packet): void {
    scene.remove(p.core, p.halo);
  }

  function updatePackets(now: number): void {
    for (let i = packets.length - 1; i >= 0; i--) {
      const p = packets[i]!;
      const t = (now - p.start) / PACKET_MS;
      const a = byCid.get(p.from);
      const b = byCid.get(p.to);
      if (t >= 1 || !a || !b) {
        dropPacket(p);
        packets.splice(i, 1);
        continue;
      }
      const p0 = a.robot.labelAnchor(new THREE.Vector3());
      const p2 = b.robot.labelAnchor(new THREE.Vector3());
      const mid = p0.clone().lerp(p2, 0.5);
      mid.y += 0.8 + p0.distanceTo(p2) * 0.3;
      const u = 1 - t;
      const pos = p0.multiplyScalar(u * u).addScaledVector(mid, 2 * u * t).addScaledVector(p2, t * t);
      p.core.position.copy(pos);
      p.halo.position.copy(pos);
    }
  }

  /** What labels and the card show: offline overrides whatever the transcript last said; a chat shows dots. */
  const shown = (b: Bot): string => (b.agent.status === 'offline' ? 'offline' : work.talking(b.agent.cid) ? '· · ·' : b.activity);

  const nameOf = (cid: unknown): string => {
    const b = typeof cid === 'number' ? byCid.get(cid) : undefined;
    return b ? floorName(b.agent) : String(cid ?? '?');
  };

  function setActivity(bot: Bot, text: string): void {
    bot.activity = text;
    if (bot === selected) renderCard();
  }

  // Projection onto the stage and the HTML labels.
  function toScreen(p: THREE.Vector3): { x: number; y: number; visible: boolean } {
    const ndc = p.clone().project(camera);
    camera.getWorldDirection(camDir);
    const inFront = p.clone().sub(camera.position).dot(camDir) > 0;
    return { x: ((ndc.x + 1) / 2) * width, y: ((1 - ndc.y) / 2) * height, visible: inFront && ndc.z <= 1 };
  }

  /** Idle agents carry no label; working and offline ones do, and so does the selected or hovered agent. */
  const labelShown = (b: Bot): boolean => b === selected || b === hovered || b.agent.status !== 'idle';

  function updateLabels(): void {
    for (const b of bots) {
      const s = labelShown(b) && b.robot.group.scale.x > 0.05 ? toScreen(b.robot.labelAnchor(new THREE.Vector3())) : null;
      if (!s?.visible) {
        if (b.label.style.display !== 'none') b.label.style.display = 'none';
        continue;
      }
      b.label.style.display = 'flex';
      b.label.style.transform = `translate(${s.x.toFixed(1)}px, ${s.y.toFixed(1)}px) translate(-50%, -100%) translateY(-6px)`;
      const name = floorName(b.agent);
      if (b.labelName.textContent !== name) b.labelName.textContent = name;
      const key = `floor-key ${b.agent.status}`;
      if (b.labelKey.className !== key) b.labelKey.className = key;
    }
  }

  /** The role as a square tag, striped in the district's colour as on the robot's label. */
  function roleTag(role: string, hue: number): HTMLElement {
    const t = document.createElement('span');
    t.className = 'floor-tag';
    t.style.borderLeftColor = `hsl(${hue} 55% 34%)`;
    t.textContent = roleLabel(role);
    return t;
  }

  function renderCard(): void {
    if (!selected) {
      card.hidden = true;
      cardSig = '';
      return;
    }
    const a = selected.agent;
    card.hidden = false;
    const line = shown(selected);
    const name = floorName(a);
    const sig = [name, line].join('|');
    if (sig === cardSig) return;
    cardSig = sig;
    card.replaceChildren(
      h('header.floor-card-head', null, h('strong', null, name), roleTag(a.role, selected.district.hue)),
      h('div.floor-card-activity', null, line),
    );
  }

  function positionCard(): void {
    if (!selected || card.hidden) return;
    const s = toScreen(selected.robot.labelAnchor(new THREE.Vector3()));
    const left = clamp(s.x + 24, 8, Math.max(8, width - 308));
    const top = clamp(s.y - 120, 8, Math.max(8, height - 260));
    card.style.left = `${left}px`;
    card.style.top = `${top}px`;
  }

  function select(bot: Bot | null): void {
    selected = bot;
    cardSig = '';
    renderCard();
  }

  /** The robot under a pointer event, found by raycasting against the robot groups. */
  function pick(ev: MouseEvent): Bot | null {
    const r = canvas.getBoundingClientRect();
    ndcVec.set(((ev.clientX - r.left) / r.width) * 2 - 1, -((ev.clientY - r.top) / r.height) * 2 + 1);
    raycaster.setFromCamera(ndcVec, camera);
    const hits = raycaster.intersectObjects(
      bots.map((b) => b.robot.group),
      true,
    );
    for (const hit of hits) {
      let o: THREE.Object3D | null = hit.object;
      while (o) {
        const found = bots.find((b) => b.robot.group === o);
        if (found) return found;
        o = o.parent;
      }
    }
    return null;
  }

  /** Live transcript activity, as the card and labels show it. */
  function toolActivityFor(tool: string, inp: Record<string, unknown>): string {
    return toolActivity(tool, inp, nameOf);
  }

  // Resize.
  const resize = (): void => {
    const box = stage.getBoundingClientRect();
    width = box.width;
    height = box.height;
    if (!width || !height) return;
    renderer.setSize(width, height, false);
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
    if (!userMoved) resetView(); // a view the user has not moved keeps the whole plaza in frame
  };
  const ro = new ResizeObserver(() => resize());
  ro.observe(stage);

  // The ENTER sequence (floor3d/entry.ts). A handoff from the Machines view builds that machine's fleet here and plays
  // the fly-in and the dive; a direct load plays only the build wave (see the frame loop).
  let exterior: Exterior | null = null;
  // The exterior is built from the handoff, or for a warm plaza from the fleet the popup showed.
  const exteriorOf: { machineId: string; fleet: FleetResponse } | null =
    handoff ?? (warm !== null && machineId !== null ? { machineId, fleet: warm.fleet } : null);
  // The flight's light: in the scene from the build on, at no intensity, so its shaders never change at ENTER (a light
  // added then would relink every program). entry.ts sets its intensity and place.
  const entryLight = new THREE.PointLight(0xffb070, 0, 40, 1.6);
  const buildExterior = (x: { machineId: string; fleet: FleetResponse }): void => {
    const fleet = buildFleet(x.fleet);
    prepareFade(fleet.group); // its fades never change a program (entry.ts): set now, while the part is built
    scene.add(entryLight);
    prewarmFx(scene); // the robot effects' programs compile in warm (their pieces wait in a hidden holder)
    prewarmLift(scene, roomPalette); // the lift's and gangway's materials, compiled here (lift.ts)
    scene.add(fleet.group);
    const m = x.fleet.machines.find((y) => y.id === x.machineId);
    if (m) exterior = { fleet, data: x.fleet, machineId: x.machineId, model: MODELS[modelsFor(m).exterior] ?? MODELS['exterior/workstation']! };
  };
  if (exteriorOf !== null) {
    // A warm plaza builds its exterior as a job (the popup is up); ENTER finishes it first if it is not done (adopt).
    if (warm !== null) {
      jobs.replace('exterior', (function* (): Generator<void, void, void> {
        buildExterior(exteriorOf);
        jobs.replace('compile-exterior', compileSteps(renderer, scene, camera, [new THREE.Mesh(ground.geometry, groundNight), new THREE.Mesh(ground.geometry, groundDark)]));
      })());
    }
    else buildExterior(exteriorOf);
  }
  // The plaza's default view (the pose resetView frames), cached per board and layout.
  const defaultPose = (): Pose | null => {
    if (!board || worldJob !== null || pendingBots.length > 0 || jobs.busy) return null; // the rooms are built before the default view (and the dive) can use them
    if (!poseOf || poseOf.board !== board || poseOf.layout !== layoutNow) {
      const { dist, target } = fitView();
      const position = target.clone().addScaledVector(viewDir(), dist);
      poseOf = { board, layout: layoutNow, pose: { position, target, fog: { near: dist * 0.8, far: dist * 3 } } };
    }
    return poseOf.pose;
  };
  // The look from the Machines view's night (k 0) to the plaza's board floor (k 1): skyline, ground and fog.
  const look = (k: number): void => {
    if (handoff === null) return;
    const night = k < 0.5;
    backdrop.visible = night;
    ground.material = night ? groundNight : groundDark;
    ground.position.y = night ? 0 : -BOARD_DROP;
    const from = handoff.from.fog;
    const to = defaultPose()?.fog;
    if (scene.fog instanceof THREE.Fog && from && to) {
      scene.fog.near = from.near + (to.near - from.near) * k;
      scene.fog.far = from.far + (to.far - from.far) * k;
    }
  };
  const entryContext = (x: Exterior | null): EntryContext => ({
    camera,
    orbit: controls,
    scene,
    exterior: x,
    light: entryLight,
    from: handoff?.from ?? null,
    board: () => (board ? { group: board.group, model: board.model, atom: board.atomGroup } : null),
    rooms: districtGroup,
    bots: () => bots.map((b) => b.robot.group),
    defaultPose,
    look,
    onLeft: () => {
      if (handoff !== null) setReturn(handoff.from, handoff.fleet);
      go('floor');
    },
  });
  // The arrival (ENTER): the camera holds at the Machines pose and the parts wait hidden; the flight starts once the plaza
  // is ready (its builds done, its default view known), and the loading card shows the progress until then.
  const arrive = (h: Handoff): void => {
    handoff = h;
    pendingFlight = true;
    driftStopped = false; // an arrival is a reset: the idle sway waits for the flight to end
    driftApplied = 0;
    driftT = 0;
    loadPct = 0;
    loadName.textContent = h.fleet.machines.find((m) => m.id === h.machineId)?.name ?? h.machineId;
    camera.position.copy(h.from.position);
    controls.target.copy(h.from.target);
    camera.lookAt(h.from.target);
    camera.updateMatrixWorld();
    controls.enabled = false;
    if (scene.fog instanceof THREE.Fog && h.from.fog) {
      scene.fog.near = h.from.fog.near;
      scene.fog.far = h.from.fog.far;
    }
    look(0);
    // The sequence exists from the arrival, held at its start: its state applies while the build finishes, and no part
    // changes its visibility (a visibility change relinks shaders) when the flight starts.
    if (exterior !== null) {
      entry = new EntrySequence(entryContext(exterior), 0);
      entry.hold(true);
    }
  };
  /** Keeps the flight's parts hidden until it starts: the rooms, the board and the robots (new ones too). */
  const holdParts = (): void => {
    districtGroup.visible = false;
    if (board) board.group.visible = false;
    for (const b of bots) b.robot.group.visible = false;
  };
  /** The loading card: the jobs' progress, never back down. */
  const updateLoading = (): void => {
    loadPct = Math.max(loadPct, jobs.progress);
    if (loader.hidden) loader.hidden = false;
    loadBar.style.transform = `scaleX(${loadPct.toFixed(3)})`;
    const pct = `${Math.floor(loadPct * 100)}%`;
    if (loadPctEl.textContent !== pct) loadPctEl.textContent = pct;
  };
  /** The flight starts: the parts show, and the sequence takes the camera. */
  const startFlight = (): void => {
    pendingFlight = false;
    loader.hidden = true;
    districtGroup.visible = true;
    if (entry === null) entry = new EntrySequence(entryContext(exterior), 0);
    else entry.hold(false);
  };
  if (handoff !== null) arrive(handoff);
  // The fleet's shaders compile before its first draw. A warm plaza compiles its exterior and rooms as budgeted jobs instead.
  if (warm === null) void renderer.compileAsync(scene, camera);
  entryTime = () => entry?.time ?? null;
  // Back (and Esc) reverse the sequence from where it stands; before it starts, they leave at once.
  // Back (and Esc): a handoff plaza flies back out from wherever it stands, its end included; a direct load (no handoff)
  // mid-sequence reverses as before, and once its own sequence has finished it navigates to the Machines view.
  const leavePlaza = (): void => {
    if (pendingFlight) go('floor'); // a Back during the loading card cancels the arrival (the plaza is torn down)
    else if (entry && (handoff !== null || !entry.isFinished)) entry.back();
    else go('floor');
  };
  plazaBack = leavePlaza;
  resize();
  resetView();

  // Pointer: drag (OrbitControls) to orbit and pan, click to select, hover to highlight.
  const ac = new AbortController();
  const listen = <K extends keyof HTMLElementEventMap>(type: K, fn: (ev: HTMLElementEventMap[K]) => void): void =>
    canvas.addEventListener(type, fn, { signal: ac.signal });
  let downAt: { x: number; y: number } | null = null;
  listen('pointerdown', (ev) => {
    downAt = { x: ev.clientX, y: ev.clientY };
  });
  listen('pointermove', (ev) => {
    if (ev.buttons !== 0) return;
    hovered = pick(ev);
    canvas.style.cursor = hovered ? 'pointer' : 'grab';
  });
  listen('pointerup', (ev) => {
    if (!downAt) return;
    const moved = Math.hypot(ev.clientX - downAt.x, ev.clientY - downAt.y);
    downAt = null;
    if (moved < 4) select(pick(ev));
  });
  listen('pointerleave', () => {
    hovered = null;
  });
  // Any pointer or wheel input on the stage stops the idle sway for good (until the next reset or arrival).
  for (const type of ['pointerdown', 'pointermove', 'wheel'] as const) stage.addEventListener(type, () => (driftStopped = true), { signal: ac.signal, passive: true });
  listen('dblclick', (ev) => {
    const bot = pick(ev);
    if (bot) go('agents', bot.agent.cid);
  });
  // Esc closes the card, else steps back out of the machine.
  document.addEventListener(
    'keydown',
    (ev) => {
      if (ev.key !== 'Escape' || !warmShown) return;
      if (selected !== null) select(null);
      else leavePlaza();
    },
    { signal: ac.signal },
  );

  // Live data: the machine's board and members first (refresh, then every few seconds), then the agents.
  void refresh();
  const poll = window.setInterval(() => void refresh(), FLEET_POLL_MS);
  const offAgents = agents.watch((list) => {
    sync(list);
    if (!fitted && list.length) {
      resetView();
      fitted = true;
    }
  });
  const offTranscript = on('transcript', (data) => {
    const e = data as Entry;
    const bot = byCid.get(e.cid);
    switch (e.kind) {
      case 'tool_request': {
        const tool = str(e.data.tool) || str(e.data.name) || 'tool';
        const inp = (e.data.input ?? {}) as Record<string, unknown>;
        work.noteTool(e.cid, tool, inp);
        if (bot) setActivity(bot, toolActivityFor(tool, inp));
        const to = recipientOf(tool, inp);
        if (to !== null) sendPacket(e.cid, to, 'accent');
        break;
      }
      case 'model_request':
        if (bot) setActivity(bot, 'thinking…');
        break;
      case 'work_end':
        if (bot) setActivity(bot, 'idle');
        break;
      case 'work_start': {
        const origin = (e.data.origin ?? {}) as { from_cid?: unknown };
        if (typeof origin.from_cid === 'number') sendPacket(origin.from_cid, e.cid, 'accent');
        else if (e.data.kind === 'task') {
          const boss = bots.find((b) => b.agent.tier === 'orchestrator');
          if (boss) sendPacket(boss.agent.cid, e.cid, 'accent');
        }
        break;
      }
      default:
        break;
    }
  });
  const offDelta = on('delta', (data) => {
    const bot = byCid.get((data as { cid: number }).cid);
    if (bot) setActivity(bot, 'thinking…');
  });
  const offConversation = on('conversation', (data) => {
    const c = data as Conversation;
    sendPacket(c.from_cid, c.to_cid, 'ok');
  });
  // The bus frames that give robots their stand-ups, deliveries and responses (floor3d/robot-work busFrame).
  const offBus = [
    on('project', (data) => work.busFrame('project', data)),
    on('file', (data) => work.busFrame('file', data)),
    on('work', (data) => work.busFrame('work', data)),
  ];

  // Loop.
  // Capped at 60 fps while the plaza is busy, and at 15 fps when only the idle sway and the ambient motion move (the
  // busy verdict is taken at the end of each drawn frame). A frame that comes too soon is skipped whole, and `last`
  // moves only on rendered frames.
  let lastRender = -Infinity;
  const meter = createMeter(renderer, 'floor', stage);
  const frame = (now: number): void => {
    if (now - lastRender < (plazaBusy ? 1000 / 60 - 1 : 1000 / 15 - 1)) return;
    if (!warmShown) {
      // A warm plaza that is not drawn still builds, on the same budget (the popup is up meanwhile).
      plazaBusy = true;
      jobs.run(WORLD_BUDGET_MS);
      stepWorld(WORLD_BUDGET_MS);
      stepBots(WORLD_BUDGET_MS);
      return;
    }
    if (!meter.ready()) return; // a hidden tab or an offscreen canvas does not draw
    lastRender = now;
    const rawDt = (now - last) / 1000; // the wall time: the sequence's clock (entry.ts smooths it and caps a hidden tab's gap)
    const dt = Math.min(plazaBusy ? 0.05 : 0.1, rawDt); // an idle frame is 1/15 s apart: the sway keeps its wall-clock speed
    last = now;
    const t = now / 1000;
    updateBots(dt, now);
    updatePackets(now);
    updateDust(t);
    const budget = pendingFlight ? LOAD_BUDGET_MS : WORLD_BUDGET_MS;
    jobs.run(budget);
    stepWorld(budget);
    stepBots(budget);
    updateBoard(dt, t);
    if (pendingFlight) {
      if (entry === null) holdParts(); // no sequence yet (no exterior): the parts wait hidden
      if (defaultPose() !== null && exterior !== null) startFlight();
      else updateLoading();
    }
    // A direct load plays the build wave once its board is built; the sequence then sets the board's and rooms' state.
    if (entry === null && handoff === null && !directWaved && board !== null && defaultPose() !== null) {
      directWaved = true;
      entry = new EntrySequence(entryContext(null), DIRECT_START);
    }
    entry?.step(rawDt);
    skyLayer.animate(t);
    for (const fn of propAnim) fn(t);
    updateDrift(dt); // the settled plaza's idle sway (no ENTER drives, nothing loads)
    if (!entry?.driving && !pendingFlight) controls.update();
    meter.render(scene, camera);
    updateLabels();
    positionCard();
    plazaBusy =
      entry?.driving === true || pendingFlight || jobs.busy || pendingBots.length > 0 || worldJob !== null ||
      robotsBusy || packets.length > 0 || selected !== null || hovered !== null || now - lastInput < 1500;
  };
  loopOwner = loopToken;
  renderer.setAnimationLoop(frame);

  // ENTER on a warm plaza: its DOM moves to the visible root, and its look and sequence start from the handoff.
  const adopt = (real: HTMLElement, h: Handoff | null): void => {
    finishBackdrop(); // the night's backdrop is part of the look ENTER starts from (built once per page: normally done already)
    host = real; // the builds are not finished here: the flight waits for them (arrive), with the loading card shown
    real.append(...Array.from(root.childNodes));
    real.classList.add('floor-view');
    document.body.classList.add('floor-mode');
    bodyFloor = true;
    warmShown = true;
    if (h !== null) arrive(h);
  };
  const teardown = (): void => {
    alive = false;
    pendingFlight = false; // a Back during the loading card cancels the arrival: the plaza is torn down
    jobs.clear(); // a build that is still queued never finishes into a torn-down plaza
    boardPending = null;
    window.clearInterval(poll);
    ac.abort();
    plazaBack = null;
    entryTime = null;
    entry?.dispose();
    entry = null;
    if (exterior) disposeTree(exterior.fleet.group);
    groundNight.dispose();
    groundDark.dispose();
    if (loopOwner === loopToken) {
      renderer.setAnimationLoop(null);
      loopOwner = null;
    }
    meter.dispose();
    ro.disconnect();
    offAgents();
    offTranscript();
    offDelta();
    offConversation();
    for (const off of offBus) off();
    controls.dispose();
    for (const b of [...bots]) removeBot(b);
    if (navOverlay) {
      scene.remove(navOverlay.group);
      navOverlay.dispose();
      navOverlay = null;
    }
    for (const l of navLifts) {
      scene.remove(l.prop.group);
      l.prop.dispose();
    }
    navLifts = [];
    for (const g of navGangways) {
      scene.remove(g.group);
      g.dispose();
    }
    navGangways = [];
    for (const r of navRamps) {
      scene.remove(r);
      disposeRamp(r);
    }
    navRamps = [];
    nav = null;
    bots = [];
    byCid.clear();
    for (const p of packets.splice(0)) dropPacket(p);
    disposeTree(districtGroup);
    if (board) retireBoard(board);
    disposeTree(ground);
    disposeTree(skyLayer.group);
    dustGeo.dispose();
    dustMat.dispose();
    padGeo.dispose();
    packetGeo.dispose();
    haloGeo.dispose();
    tokenGeo.dispose();
    for (const m of Object.values(mats)) m.dispose();
    // The renderer is shared by the plazas (plazaRenderer): it is not disposed here; its program cache serves the next one.
    host.classList.remove('floor-view');
    if (bodyFloor) document.body.classList.remove('floor-mode');
    host.replaceChildren();
  };
  if (warm !== null && machineId !== null) warmHandle = { machineId, adopt, teardown };
  return teardown;
}

/** A robot part casts a shadow only when its world bounding radius reaches this (units): smaller parts do not read in the shadow map. */
const ROBOT_SHADOW_MIN_RADIUS = 0.6;

/** Whether a robot part casts: the body and limbs do, the bolts and eyes do not (each caster is one more draw in the shadow pass). */
function robotPartCasts(m: THREE.Mesh): boolean {
  const geo = m.geometry;
  if (!geo.boundingSphere) geo.computeBoundingSphere();
  const s = m.getWorldScale(new THREE.Vector3());
  return (geo.boundingSphere?.radius ?? 0) * Math.max(s.x, s.y, s.z) >= ROBOT_SHADOW_MIN_RADIUS;
}

/** The point the board's cutaways look toward, in world space (reused every frame). */
const focusPoint = new THREE.Vector3();

/** The canvas every view draws into: pixel ratio, soft cartoon shadows, filling its stage. */
function viewCanvas(renderer: THREE.WebGLRenderer, label: string, maxRatio?: number): HTMLCanvasElement {
  renderer.setPixelRatio(startPixelRatio(maxRatio)); // maxRatio: a view's own cap (the default is the plaza's)
  renderer.shadowMap.enabled = true;
  renderer.shadowMap.type = THREE.PCFShadowMap; // crisp, cartoon shadows
  routeShadowProxies(renderer); // static casters cast from their merged proxies (bake.ts)
  const canvas = renderer.domElement;
  canvas.classList.add('floor-canvas');
  canvas.setAttribute('aria-label', label);
  canvas.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;display:block;cursor:grab';
  return canvas;
}

/** The night every view shares: sky and stars, moonlight, the ground sheet and the outskirts. */
/**
 * The night backdrop: built once per page, in idle steps from the first mount, and shared. Every scene gets its own
 * copies of the baked sectors, which share their geometry and materials; no teardown disposes them (they outlive a view).
 */
let backdropMaster: THREE.Group | null = null;
let backdropSteps: Generator<void, void, void> | null = null;
const backdropHolders: THREE.Group[] = [];
function fillBackdrop(holder: THREE.Group): void {
  for (const s of backdropMaster!.children) holder.add(s.clone(true)); // clone(true) shares the geometry and materials
}
/** One step of the page's build; true once it is built (and every holder waiting for it is filled). */
function backdropStep(): boolean {
  if (backdropSteps === null) return true;
  if (!backdropSteps.next().done) return false;
  backdropSteps = null;
  for (const h of backdropHolders.splice(0)) fillBackdrop(h);
  return true;
}
/** The idle callback: steps of the build for about 6 ms, then the next idle time, until the build is done. */
function idleBackdrop(): void {
  const until = performance.now() + 6;
  while (backdropSteps !== null && performance.now() < until) backdropStep();
  if (backdropSteps !== null) scheduleIdleBackdrop();
}
function scheduleIdleBackdrop(): void {
  if (typeof window.requestIdleCallback === 'function') window.requestIdleCallback(idleBackdrop, { timeout: 500 });
  else window.setTimeout(idleBackdrop, 16);
}
/** A scene's backdrop holder: filled now if the page's build is done, else when it is (the idle steps build it). */
function requestBackdrop(holder: THREE.Group): void {
  if (backdropMaster !== null && backdropSteps === null) {
    fillBackdrop(holder);
    return;
  }
  backdropHolders.push(holder);
  if (backdropMaster === null) {
    backdropMaster = new THREE.Group();
    backdropSteps = buildBackdropSteps(backdropMaster);
    scheduleIdleBackdrop();
  }
}
/** Finishes the page's build now: ENTER needs the backdrop, so a build the idle steps have not reached is done here. */
function finishBackdrop(): void {
  while (!backdropStep()) {
    // run to the end
  }
}

function nightScene(): { scene: THREE.Scene; ground: THREE.Mesh; backdrop: THREE.Group; skyLayer: ReturnType<typeof starfield> } {
  const scene = new THREE.Scene();
  // Night: a deep blue zenith banded down to a violet horizon glow, stars and a ringed planet.
  const SKY_TOP = new THREE.Color(0x05071a);
  const SKY_HORIZON = new THREE.Color(0x3a2275);
  scene.add(sky(SKY_TOP, SKY_HORIZON, new THREE.Color(0x090c18)));
  scene.fog = new THREE.Fog(0x161236, 60, 230);
  const skyLayer = starfield(900, 330, rng(31));
  skyLayer.group.userData.dynamic = true; // turns every frame
  scene.add(skyLayer.group);

  // Cool moonlight from above and a violet bounce from the deck; the neon does the rest.
  scene.add(new THREE.HemisphereLight(0x9fb0ff, 0x2a1d52, 1.15));
  const sun = new THREE.DirectionalLight(0xd2ddff, 2.0);
  sun.position.set(22, 40, 16);
  sun.castShadow = true;
  sun.shadow.mapSize.set(2048, 2048);
  sun.shadow.bias = -0.0008;
  sun.shadow.normalBias = 0.03;
  const sc = sun.shadow.camera;
  Object.assign(sc, { left: -40, right: 40, top: 40, bottom: -40, near: 1, far: 140 });
  sc.updateProjectionMatrix();
  scene.add(sun);

  // Ground: one smooth sheet out to the horizon, lighter round the plaza.
  const ground = new THREE.Mesh(new THREE.PlaneGeometry(500, 500), toonMat({ map: groundTex() }));
  ground.rotation.x = -Math.PI / 2;
  ground.position.y = -0.01;
  ground.receiveShadow = true;
  scene.add(ground);
  const backdrop = new THREE.Group(); // the page's shared backdrop, copied in (requestBackdrop)
  scene.add(backdrop);
  requestBackdrop(backdrop);
  return { scene, ground, backdrop, skyLayer };
}

/** The camera and the orbit every view uses: damped, pan and zoom, held above the ground. */
function orbitView(canvas: HTMLCanvasElement): { camera: THREE.PerspectiveCamera; controls: OrbitControls } {
  const camera = new THREE.PerspectiveCamera(40, 1, 0.5, 1000);
  const controls = new OrbitControls(camera, canvas);
  controls.enableDamping = true;
  controls.dampingFactor = 0.08;
  controls.enablePan = true;
  controls.enableZoom = true;
  controls.minPolarAngle = 0.2;
  controls.maxPolarAngle = 1.3; // keeps the floor in view
  controls.minDistance = 6;
  controls.maxDistance = 300;
  return { camera, controls };
}

/** The Machines view polls the fleet this often, ms. */
const FLEET_POLL_MS = 5000;
/** The Machines view repaints this often while idle, for the ambient animation (LED pulses, cable packets). */
const AMBIENT_FPS = 12;
/** A change (hover, select, resize, camera, fleet data) keeps the Machines view rendering at full rate this long, ms. */
const BUSY_MS = 700;
/** The retry gap for a frame the meter could not draw (a hidden tab, or a canvas off the screen), ms. */
const RETRY_MS = 250;
/** The Machines view's pixel-ratio cap (its canvas is not the plaza's): one device pixel per CSS pixel. */
const MACHINES_MAX_RATIO = 1;
/** The Machines view's antialiasing (MSAA). Off for now: the on/off screenshot comparison was not finished (see the report). */
const MACHINES_ANTIALIAS = false;
/** The fleet's bounding sphere fills this share of the frame on its tighter axis. */
const MACHINE_FILL = 0.8;
/** How many agent names the popup lists before "+N more". */
const FLEET_CARD_AGENTS = 6;
const FLEET_KEYS = [
  ['online', 'Online'],
  ['draining', 'Draining'],
  ['offline', 'Offline'],
  ['revoked', 'Revoked'],
] as const;

/**
 * The Floor's entry. `#/floor/<id>` is that machine's plaza (today's Floor). `#/floor` is the Machines view once the
 * fleet loads, and the plaza itself when it does not.
 */
export function mount(root: HTMLElement, params: string[] = []): Cleanup {
  exposeHooks();
  // The plazas' renderer, ahead of a popup (not in its frame). Not in engine mode: one GPU context per page.
  if (!engineWanted()) window.setTimeout(() => plazaRenderer(), 0);
  const machineId = params[0];
  // A Back from a machine returns with the fleet it showed: the Machines view mounts at once, with no refetch.
  const back = takeReturn();
  if (machineId === undefined && engineWanted()) return mountEngine(root); // ?engine=wasm: the Machines view on the wasm canvas
  if (back) return mountMachines(root, back.fleet, back.pose);
  if (machineId !== undefined) {
    const w = warmHandle;
    if (w !== null && w.machineId === machineId) {
      // ENTER on the popup's machine: its warm plaza is attached now, so no build runs at the click.
      warmHandle = null;
      window.clearTimeout(warmTimer);
      w.adopt(root, takeEntry(machineId));
      return w.teardown;
    }
    return mountPlaza(root, machineId);
  }
  let stop: Cleanup | undefined;
  let alive = true;
  api.fleet().then(
    (fleet) => {
      if (alive) stop = mountMachines(root, fleet);
    },
    () => {
      if (alive) stop = mountPlaza(root, null);
    },
  );
  return () => {
    alive = false;
    stop?.();
  };
}

/** The ENTER hooks: the mounted Machines view's enter, and the mounted plaza's Back. window.__apzEnter / __apzBack call them (headless recording). */
let enterHook: ((id: string) => void) | null = null;
let openHook: ((id: string) => void) | null = null; // the popup of a machine, for headless runs (__apzOpen)
let plazaBack: (() => void) | null = null;
/** The warm plaza (the popup's machine, built hidden): ENTER adopts it; it is dropped WARM_MS after the popup closes. */
let warmHandle: { machineId: string; adopt: (real: HTMLElement, h: Handoff | null) => void; teardown: () => void } | null = null;
let warmTimer = 0;
const WARM_MS = 30000;
function dropWarm(): void {
  const w = warmHandle;
  warmHandle = null;
  window.clearTimeout(warmTimer);
  w?.teardown();
}
/** The popup opened on a machine: its plaza is built hidden now (or kept, if it is already warm). Another machine's is dropped. */
function warmPlaza(machineId: string, fleet: FleetResponse): void {
  if (warmHandle?.machineId !== machineId) {
    dropWarm();
    mountPlaza(document.createElement('div'), machineId, { fleet });
  }
  window.clearTimeout(warmTimer);
  warmTimer = window.setTimeout(dropWarm, WARM_MS);
}
let entryTime: (() => number | null) | null = null; // the ENTER sequence's time, seconds (headless checks)
/** Exposes the headless hooks on window, from mount (never at load: the tests run under Node, which has no window). */
function exposeHooks(): void {
  if (typeof window === 'undefined') return;
  Object.assign(window, {
    __apzEnter: (id: string): void => (enterHook ? enterHook(id) : go('floor', id)),
    __apzBack: (): void => plazaBack?.(),
    __apzOpen: (id: string): void => openHook?.(id),
    __apzEntryTime: (): number | null => entryTime?.() ?? null,
    // Headless audit: the plaza renderer's programs (name and cache key), to name the ones an ENTER links.
    __apzPrograms: (): { name: string; key: string }[] => (plazaGl ? plazaGl.info.programs ?? [] : []).map((p) => ({ name: p.name, key: String((p as unknown as { cacheKey: string }).cacheKey) })),
  });
}

/** A Machines view kept alive through an ENTER: its renderer, scene and DOM wait for the Back, and are released KEPT_MS after. */
interface KeptMachines {
  resume: (root: HTMLElement, fleet: FleetResponse, pose: Pose) => Cleanup;
  release: () => void;
}
let kept: KeptMachines | null = null;
let keptTimer = 0;
const KEPT_MS = 30000;
/** Set by ENTER just before its navigation: the Machines view it leaves is kept (parked), not disposed. */
let keepMachines = false;
function releaseKept(): void {
  const k = kept;
  kept = null;
  window.clearTimeout(keptTimer);
  k?.release();
}
function keepView(k: KeptMachines): void {
  releaseKept();
  kept = k;
  keptTimer = window.setTimeout(releaseKept, KEPT_MS);
}

/** The Machines view: every machine of the plaza as its model (floor3d/fleet.ts), polled every few seconds. */
function mountMachines(root: HTMLElement, first: FleetResponse, back: Pose | null = null): Cleanup {
  if (back !== null && kept !== null) {
    // A Back from the plaza: the kept view comes back as it left (no renderer, no build, no shader compile).
    const k = kept;
    kept = null;
    window.clearTimeout(keptTimer);
    return k.resume(root, first, back);
  }
  releaseKept(); // a Machines mount that is not a Back: a kept view is released first
  const renderer = newRenderer(MACHINES_ANTIALIAS);
  if (!renderer) return mountFloor2d(root);
  const canvas = viewCanvas(renderer, 'The machines', MACHINES_MAX_RATIO);
  const { scene, ground, skyLayer } = nightScene(); // the backdrop is the page's shared one (requestBackdrop)
  const { camera, controls } = orbitView(canvas);
  const fleetScene = buildFleet(first);
  scene.add(fleetScene.group);
  fleetScene.update(0, first); // builds the machines now, so the first fit frames them
  let latest = first;
  let fitSig = fleetSig(first);

  // DOM: the stage with the canvas, the machine tags, the popup card and the legend.
  const card = h('div.floor-card.machine-card', { hidden: true }); // fits its stage: it scrolls inside itself (floor.css)
  const legend = h(
    'div.floor-legend',
    null,
    ...FLEET_KEYS.map(([key, label]) => h('span', null, keyDot(key), label)),
  );
  const tags = document.createElement('div');
  tags.style.cssText = 'position:absolute;inset:0;overflow:hidden;pointer-events:none';
  const stage = h('div.floor-stage.floor-stage-3d', null, tags, card, legend);
  stage.style.position = 'relative';
  stage.prepend(canvas);
  root.classList.add('floor-view');
  document.body.classList.add('floor-mode');
  const viewHead = h('div.view-head', null, h('h1', null, 'Floor'), h('div.actions', null, button('Reset view', () => resetView(), { icon: 'refresh' })));
  const viewNodes: HTMLElement[] = [viewHead, stage]; // the view's DOM: a kept view moves these into the next Floor's root
  let host = root; // the element the view is in now
  root.append(...viewNodes);

  let width = 0;
  let height = 0;
  let hoveredId: string | null = null;
  let selectedId: string | null = null;
  let cardSig = '';
  let seenEl: HTMLElement | null = null; // the card's last-seen value, updated in place each frame
  // A Back from a machine returns here at the pose it left (floor3d/entry.ts): the view is not refit then.
  let userMoved = back !== null; // until the user moves the camera, the view refits on resize and when the machines change
  let settingView = false; // resetView's own camera moves are not the user's

  // Rendering is on demand. A frame runs when something changed (wake() moves busyUntil on, kick() asks for a frame);
  // a frame that finds nothing moving schedules the next one from a timer at AMBIENT_FPS, so the LED pulses and the
  // cable packets keep going. The popup open draws nothing: its canvas keeps the last image.
  let running = true; // false while parked (stopLive) and after free
  let rafId = 0; // the pending animation frame, 0 when none
  let timer = 0; // the pending ambient or retry timer, 0 when none
  let busyUntil = 0; // performance.now() until which frames run at full rate
  let shadowsDue = true; // the next frame redraws the shadow map (the first frame, a data change, a resize, a resume)
  let paintOnce = false; // a resize clears the canvas: one frame draws it even with the popup open
  let lastRender = -Infinity;
  const wake = (): void => {
    busyUntil = performance.now() + BUSY_MS;
  };
  const cancelFrame = (): void => {
    if (rafId !== 0) window.cancelAnimationFrame(rafId);
    window.clearTimeout(timer);
    rafId = 0;
    timer = 0;
  };
  /** Asks for a frame, unless one is pending or the view draws nothing now (parked, or the popup is open). */
  const kick = (): void => {
    if (!alive || !running || rafId !== 0) return;
    if (selectedId !== null && !paintOnce) return;
    window.clearTimeout(timer);
    timer = 0;
    rafId = window.requestAnimationFrame(onFrame);
  };
  /** The timer's callback: a frame again (the ambient step, or a retry after a frame the meter did not draw). */
  const later = (ms: number): void => {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => {
      timer = 0;
      kick();
    }, ms);
  };
  controls.addEventListener('change', () => {
    if (!settingView) userMoved = true;
    wake();
    kick();
  });
  let last = performance.now();
  const ndcVec = new THREE.Vector2();
  const raycaster = new THREE.Raycaster();
  const camDir = new THREE.Vector3();
  const tagEls = new Map<string, { el: HTMLDivElement; key: HTMLElement; name: HTMLElement }>();
  const ac = new AbortController();
  let alive = true;

  /** Where a point of the scene sits on the stage, and whether the camera sees it. */
  const toScreen = (p: THREE.Vector3): { x: number; y: number; visible: boolean } => {
    const ndc = p.clone().project(camera);
    camera.getWorldDirection(camDir);
    const inFront = p.clone().sub(camera.position).dot(camDir) > 0;
    return { x: ((ndc.x + 1) / 2) * width, y: ((1 - ndc.y) / 2) * height, visible: inFront && ndc.z <= 1 };
  };

  /** The machine under a pointer event: the fleet's ray test. */
  const pick = (ev: MouseEvent): string | null => {
    const r = canvas.getBoundingClientRect();
    ndcVec.set(((ev.clientX - r.left) / r.width) * 2 - 1, -((ev.clientY - r.top) / r.height) * 2 + 1);
    raycaster.setFromCamera(ndcVec, camera);
    return fleetScene.pick(raycaster.ray);
  };

  /** A machine's tag, made on first sight: a status square, then the name. */
  function tagFor(id: string): { el: HTMLDivElement; key: HTMLElement; name: HTMLElement } {
    let t = tagEls.get(id);
    if (!t) {
      const el = document.createElement('div');
      el.style.cssText = LABEL_CSS;
      const key = keyDot('');
      const name = document.createElement('span');
      name.style.cssText = 'overflow:hidden;text-overflow:ellipsis;min-width:0';
      el.append(key, name);
      tags.append(el);
      t = { el, key, name };
      tagEls.set(id, t);
    }
    return t;
  }

  function updateTags(): void {
    const seen = new Set<string>();
    for (const tg of fleetScene.tags()) {
      seen.add(tg.id);
      const t = tagFor(tg.id);
      const s = toScreen(new THREE.Vector3(...tg.pos));
      t.el.style.display = s.visible ? 'flex' : 'none';
      if (!s.visible) continue;
      t.el.style.transform = `translate(${s.x.toFixed(1)}px, ${s.y.toFixed(1)}px) translate(-50%, -100%) translateY(-6px)`;
      const key = `floor-key ${tg.status}`;
      if (t.key.className !== key) t.key.className = key;
      if (t.name.textContent !== tg.name) t.name.textContent = tg.name;
    }
    for (const [id, t] of tagEls) {
      if (seen.has(id)) continue;
      t.el.remove();
      tagEls.delete(id);
    }
  }

  /** An agent's name on the card: its floor name, or its cid when the agent list does not hold it. */
  const nameOfCid = (cid: number): string => {
    const a = agents.get().find((x) => x.cid === cid);
    return a ? floorName(a) : `agent ${cid}`;
  };

  /** The popup: the selected machine's name and kind, its facts, its agents and ENTER. */
  function renderCard(now: number): void {
    const m: FleetMachine | undefined = selectedId === null ? undefined : latest.machines.find((x) => x.id === selectedId);
    if (!m) {
      selectedId = null;
      card.hidden = true;
      cardSig = '';
      return;
    }
    card.hidden = false;
    const facts = machineFacts(m);
    const names = m.agents.map((cid) => ({ cid, name: nameOfCid(cid) }));
    const tally = agentTally(m.agents.length, m.working, m.status);
    const sig = JSON.stringify([m.id, m.name, m.status, machineSubtitle(m), facts, names, tally]);
    if (sig !== cardSig) {
      cardSig = sig;
      seenEl = h('dd');
      card.replaceChildren(
        h('header.floor-card-head', null, h('strong', null, m.name), h('span.floor-tag', null, m.status)),
        h('div.floor-card-activity', null, machineSubtitle(m)),
        h(
          'dl.machine-facts',
          null,
          ...facts.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)]),
          h('dt', null, 'Last seen'),
          seenEl,
        ),
        h(
          'div.machine-agents',
          null,
          h('div.floor-card-activity', null, tally),
          h(
            'div.machine-agent-names',
            null,
            ...names.slice(0, FLEET_CARD_AGENTS).map(({ cid, name }) => h('a', { href: href('agents', cid) }, name)),
            names.length > FLEET_CARD_AGENTS ? h('span', null, `+${names.length - FLEET_CARD_AGENTS} more`) : null,
          ),
        ),
        h('footer.machine-card-foot', null, button('ENTER', () => enterMachine(m), { kind: 'primary' })),
      );
    }
    const seen = ago(m.last_seen_ms, now);
    if (seenEl && seenEl.textContent !== seen) seenEl.textContent = seen;
  }

  /** ENTER: the sequence starts from this camera and fleet (floor3d/entry.ts), and the machine's plaza plays it. */
  function enterMachine(m: FleetMachine): void {
    const fog = scene.fog instanceof THREE.Fog ? { near: scene.fog.near, far: scene.fog.far } : undefined;
    beginEntry({ machineId: m.id, fleet: latest, from: { position: camera.position.clone(), target: controls.target.clone(), fog } });
    select(null);
    keepMachines = true; // this view is parked for a Back (its cleanup), not disposed
    go('floor', m.id);
  }
  const setHooks = (): void => {
    openHook = (id: string): void => select(id);
    enterHook = (id: string): void => {
      const m = latest.machines.find((x) => x.id === id);
      if (m) enterMachine(m);
    };
  };
  setHooks();

  /** The card sits beside its machine's tag, kept on the stage. */
  function positionCard(): void {
    if (selectedId === null || card.hidden) return;
    const node = fleetScene.machines.get(selectedId);
    if (!node) return;
    const s = toScreen(new THREE.Vector3(...node.label));
    const left = clamp(s.x + 24, 8, Math.max(8, width - 308));
    const top = clamp(s.y - 120, 8, Math.max(8, height - card.offsetHeight - 8));
    card.style.left = `${left}px`;
    card.style.top = `${top}px`;
  }

  function select(id: string | null): void {
    selectedId = id;
    controls.enabled = id === null; // the popup open: the camera is still (the canvas keeps its image)
    if (id !== null) cancelFrame(); // no frame is pending while the popup is open
    const opened = id === null ? undefined : latest.machines.find((x) => x.id === id);
    // The popup's machine builds hidden, ready for ENTER: after the card has painted (a frame, then a task).
    if (opened) window.requestAnimationFrame(() => window.setTimeout(() => warmPlaza(opened.id, latest), 0));
    cardSig = '';
    renderCard(Date.now());
    positionCard();
    wake();
    kick(); // on a close, the next frame runs updateTags and positionCard; on an open, kick does nothing
  }

  /**
   * The default view: the fleet's sphere fills MACHINE_FILL of the stage on its tighter axis, aimed at its centre
   * (floor3d/fleet.ts frames it; fitDistance sizes the distance).
   */
  function resetView(): void {
    const { center, radius } = fleetScene.frame();
    const target = new THREE.Vector3(...center);
    const dist = fitDistance(radius, camera.fov, camera.aspect, MACHINE_FILL);
    controls.maxDistance = Math.max(300, dist * 2);
    if (scene.fog instanceof THREE.Fog) {
      // The haze keeps its look at the fitted distance, as the plaza's does.
      scene.fog.near = dist * 0.8;
      scene.fog.far = dist * 3;
    }
    controls.target.copy(target);
    camera.position.copy(target).add(new THREE.Vector3().setFromSphericalCoords(dist, POLAR, AZIMUTH));
    settingView = true;
    controls.update();
    settingView = false;
    userMoved = false;
    wake();
    kick();
  }

  // Resize.
  const resize = (): void => {
    const box = stage.getBoundingClientRect();
    width = box.width;
    height = box.height;
    if (!width || !height) return;
    renderer.setSize(width, height, false);
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
    if (!userMoved) resetView(); // a view the user has not moved keeps the whole fleet in frame
    shadowsDue = true; // a new size clears the canvas: the next frame draws it, even with the popup open
    paintOnce = true;
    wake();
    kick();
  };
  const ro = new ResizeObserver(() => resize());
  ro.observe(stage);
  resize();
  /** A Back's pose: the camera, the orbit and the haze as the plaza left them (a fresh view and a resumed one alike). */
  const setPose = (pose: Pose): void => {
    camera.position.copy(pose.position);
    controls.target.copy(pose.target);
    camera.lookAt(pose.target);
    if (pose.fog && scene.fog instanceof THREE.Fog) {
      scene.fog.near = pose.fog.near;
      scene.fog.far = pose.fog.far;
    }
  };
  if (back) setPose(back);
  else resetView();
  if (back) {
    renderer.shadowMap.needsUpdate = true;
    renderer.render(scene, camera);
  }

  // Pointer: drag to orbit and pan, hover lifts a machine and shows the pointer, a click opens its popup.
  const listen = <K extends keyof HTMLElementEventMap>(type: K, fn: (ev: HTMLElementEventMap[K]) => void): void =>
    canvas.addEventListener(type, fn, { signal: ac.signal });
  let downAt: { x: number; y: number } | null = null;
  listen('pointerdown', (ev) => {
    downAt = { x: ev.clientX, y: ev.clientY };
  });
  listen('pointermove', (ev) => {
    if (ev.buttons !== 0) return;
    const id = pick(ev);
    if (id !== hoveredId) {
      hoveredId = id;
      fleetScene.hover(id);
      wake();
      kick();
    }
    canvas.style.cursor = id ? 'pointer' : 'grab';
  });
  listen('pointerup', (ev) => {
    if (!downAt) return;
    const moved = Math.hypot(ev.clientX - downAt.x, ev.clientY - downAt.y);
    downAt = null;
    if (moved < 4) select(pick(ev)); // an empty click closes the popup
  });
  listen('pointerleave', () => {
    hoveredId = null;
    fleetScene.hover(null);
    wake();
    kick();
  });
  // A click outside the stage, or Esc, closes the popup.
  document.addEventListener(
    'pointerdown',
    (ev) => {
      if (selectedId !== null && !stage.contains(ev.target as Node)) select(null);
    },
    { signal: ac.signal },
  );
  document.addEventListener(
    'keydown',
    (ev) => {
      if (ev.key === 'Escape' && selectedId !== null) select(null);
    },
    { signal: ac.signal },
  );

  // Live data: the fleet is polled; the popup's agent names follow the agent list.
  /** The card's text and place while the popup is open (its frames are not drawn): the agents and the poll update it. */
  const refreshOpenCard = (): void => {
    if (selectedId === null) return;
    renderCard(Date.now());
    positionCard();
  };
  const offAgents = agents.watch(() => {
    cardSig = '';
    refreshOpenCard();
  });
  let poll = 0;
  const startPoll = (): void => {
    poll = window.setInterval(() => {
      api.fleet().then(
        (f) => {
          if (!alive) return;
          if (JSON.stringify(f) !== JSON.stringify(latest)) {
            // The fleet's data changed: the view draws it (a fresh shadow map too), and an open popup shows it.
            latest = f;
            shadowsDue = true;
            wake();
            kick();
            refreshOpenCard();
          }
        },
        () => undefined, // keep the last fleet until the next poll
      );
    }, FLEET_POLL_MS);
  };
  startPoll();

  // A hidden tab stops animation frames: the view draws again when it is shown.
  document.addEventListener(
    'visibilitychange',
    () => {
      if (!document.hidden) {
        wake();
        kick();
      }
    },
    { signal: ac.signal },
  );

  // Loop: capped at 60 fps, as the plaza's.
  const meter = createMeter(renderer, 'machines', stage);
  /** One animation frame: the updates and the draw, then the next frame (busy) or the ambient timer (idle). */
  function onFrame(now: number): void {
    rafId = 0;
    if (!alive || !running) return;
    if (selectedId !== null && !paintOnce) return; // the popup open: no frame
    if (now - lastRender < 1000 / 60 - 1) {
      rafId = window.requestAnimationFrame(onFrame); // a 120 Hz display: at most 60 frames a second
      return;
    }
    if (!meter.ready()) {
      later(RETRY_MS); // a hidden or offscreen canvas: try again shortly
      return;
    }
    lastRender = now;
    const dt = Math.min(0.1, (now - last) / 1000); // the ambient step (12 fps) is longer than the old cap
    last = now;
    fleetScene.update(dt, latest);
    const sig = fleetSig(latest);
    if (sig !== fitSig) {
      fitSig = sig; // the machines changed: the default view refits
      if (!userMoved) resetView();
    }
    skyLayer.animate(now / 1000);
    if (controls.update()) wake(); // the orbit moved (the damping too): keep drawing at full rate
    fleetScene.lod(camera, controls.target, dt);
    const shadows = shadowsDue;
    shadowsDue = false;
    meter.render(scene, camera, shadows);
    paintOnce = false;
    updateTags();
    renderCard(Date.now());
    positionCard();
    if (rafId !== 0 || selectedId !== null) return; // a change in this frame asked for the next one, or the popup is open
    if (performance.now() < busyUntil) rafId = window.requestAnimationFrame(onFrame);
    else later(1000 / AMBIENT_FPS);
  }
  wake();
  kick(); // the first frame

  /** Stops the loop and the live data, and takes the view's DOM off the page: a view that is away draws and polls nothing. */
  const stopLive = (): void => {
    running = false;
    cancelFrame();
    window.clearInterval(poll);
    ro.disconnect();
    hoveredId = null;
    fleetScene.hover(null);
    enterHook = null;
    openHook = null;
    for (const n of viewNodes) n.remove();
    host.classList.remove('floor-view');
    document.body.classList.remove('floor-mode');
  };
  /** Frees the view's geometry, materials, renderer and listeners: the view is not used again. */
  const free = (): void => {
    alive = false;
    cancelFrame();
    ac.abort();
    meter.dispose();
    offAgents();
    controls.dispose();
    for (const t of tagEls.values()) t.el.remove();
    tagEls.clear();
    disposeTree(fleetScene.group);
    disposeTree(ground);
    disposeTree(skyLayer.group);
    renderer.dispose();
  };
  /** A kept view comes back in a new Floor root, at the Back's pose, with the fleet the Back returned with. No renderer, build or shader compile. */
  const resume = (next: HTMLElement, fleet: FleetResponse, pose: Pose): Cleanup => {
    host = next;
    host.classList.add('floor-view');
    document.body.classList.add('floor-mode');
    host.append(...viewNodes);
    setHooks();
    latest = fleet;
    fitSig = fleetSig(fleet);
    userMoved = true; // the Back's pose holds, as a fresh Back mount's does
    ro.observe(stage);
    resize();
    setPose(pose);
    fleetScene.update(0, fleet);
    renderer.shadowMap.needsUpdate = true;
    renderer.render(scene, camera);
    startPoll();
    last = performance.now();
    lastRender = -Infinity;
    shadowsDue = true;
    running = true;
    wake();
    kick();
    return leave;
  };
  /** An ENTER parks the view: stopped and detached, kept for a Back for KEPT_MS. */
  const park = (): void => {
    stopLive();
    keepView({
      resume,
      release: () => {
        free();
        renderer.forceContextLoss(); // the context goes now, not at a garbage collection
      },
    });
  };
  /** Leaving the view: an ENTER parks it (kept); any other navigation disposes it, as before. */
  const leave = (): void => {
    if (keepMachines) {
      keepMachines = false;
      park();
      return;
    }
    stopLive();
    free();
    host.replaceChildren();
  };
  return leave;
}

// ---- Pure helpers (the Machines view's text and camera distance; tested) ----------------------

/** A status square in the floor's key colour for the status. */
function keyDot(status: string): HTMLElement {
  const i = document.createElement('i');
  i.className = status ? `floor-key ${status}` : 'floor-key';
  return i;
}

/** The fleet's identity for refit decisions: the machines' ids in order. */
export function fleetSig(f: FleetResponse): string {
  return f.machines.map((m) => m.id).join(',');
}

/** The camera distance at which a bounding sphere of `radius`, aimed at its centre, fills `fill` of the frame on its tighter axis. */
export function fitDistance(radius: number, fovDeg: number, aspect: number, fill: number): number {
  const tanHalf = Math.tan(THREE.MathUtils.degToRad(fovDeg) / 2) * Math.min(1, aspect); // the tighter half-field
  // The sphere's angular radius a has tan a = fill * tanHalf, and sin a = radius / distance.
  return radius * Math.sqrt(1 + 1 / (fill * tanHalf) ** 2);
}

/** A memory size in GiB: whole, or one decimal; a size the fleet does not know (null) reads as a dash. */
export function gibText(mib: number | null | undefined): string {
  if (mib == null) return '—';
  return `${Math.round(mib / 102.4) / 10} GiB`;
}

/** The popup's subtitle: the kind and the model, "workstation · ASUS ROG …". */
export function machineSubtitle(m: Pick<FleetMachine, 'kind' | 'model'>): string {
  return m.model ? `${m.kind} · ${m.model}` : m.kind;
}

/**
 * The GPUs as one line per kind: identical cards grouped, "8× NVIDIA H100 · 80 GiB". A GPU's memory is null when the
 * fleet does not know it: its name stands alone. The kinds keep the order their first card has in the fleet.
 */
export function gpuLines(gpus: FleetMachine['gpus']): string {
  if (gpus.length === 0) return 'none';
  const kinds = new Map<string, number>();
  for (const g of gpus) {
    const label = g.memory_mib == null ? g.name : `${g.name} · ${gibText(g.memory_mib)}`;
    kinds.set(label, (kinds.get(label) ?? 0) + 1);
  }
  return [...kinds].map(([label, n]) => (n > 1 ? `${n}× ${label}` : label)).join('\n');
}

/** The facts the popup lists, in order. The last-seen time is live, so the card adds it itself. */
export function machineFacts(m: FleetMachine): [string, string][] {
  return [
    ['CPU', m.cpu.model || '—'],
    ['Cores', `${m.cpu.cores} cores · ${m.cpu.threads} threads`],
    ['Memory', gibText(m.memory_mib)],
    ['GPUs', gpuLines(m.gpus)],
    ['Arch', m.arch || '—'],
    ['Backend', m.backend || '—'],
    ['OS', m.os || '—'],
    ['Role', m.role],
    ['Status', m.status],
  ];
}

/** The agents line: the working and idle split while the machine runs, otherwise its state. */
export function agentTally(total: number, working: number, status: FleetMachine['status']): string {
  if (total === 0) return 'No agents';
  const noun = total === 1 ? 'agent' : 'agents';
  if (status === 'online' || status === 'draining') return `${total} ${noun}: ${working} working, ${total - working} idle`;
  return `${total} ${noun} on a machine that is ${status}`;
}
