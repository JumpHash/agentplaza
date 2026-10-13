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
import { floorName, roleLabel } from '../roles';
import { h } from '../ui/dom';
import { makeRobot, ROBOT_PALETTE, robotVariant, type Robot, type RobotState } from './floor3d/robot';
import { buildBackdrop } from './floor3d/outskirts';
import { buildFleet } from './floor3d/fleet';
import { buildRoom } from './floor3d/rooms';
import { ground as groundTex, rng, slabSide } from './floor3d/textures';
import { outline, sky, starfield, toonMat } from './floor3d/toon';
import { bakeStatic } from './floor3d/bake';
import { mountFloor2d } from './floor2d';
import { boardHeightAt, layoutOnBoard, pathOnTraces, traceGraph, type BoardDistrict, type BoardLayout, type TraceGraph } from './floor3d/board-layout';
import { buildModel, currentPulses, modelsFor, placeAtom, traceMeshes, type Built, type Model, type Params, type Pulses, type Trace } from './floor3d/hardware';
import { MODELS } from './floor3d/hardware-index';
import type { Cleanup } from './index';

const PACKET_MS = 1500;
const SPEED = 1.6; // grid units per second; one grid cell is one world unit
const MAX_PACKETS = 60;
const RAMP = 1; // the sloped band round a room: the same band boardHeightAt slopes over
const GRAVITY = 9.8; // world units per second squared
const POLAR = 0.95; // camera angle from vertical, about 55 degrees: isometric-ish
const AZIMUTH = Math.PI / 4;
const SOCIAL_NEAR = 9.5; // districts this close count as neighbours for a visit or a chat
const BOARD_DROP = 14; // the night's ground sits this far under the board, so the board floats
const GLOW_OPACITY = 0.4; // a working component's glow plate, at full strength
const ROBOT_R = 0.55; // robot footprint radius for collisions
const STEER = [0, 30, -30, 60, -60].map((d) => (d * Math.PI) / 180); // headings tried when a step is blocked
const DUST_N = 90;
const LABEL_CSS =
  'position:absolute;left:0;top:0;display:none;align-items:center;gap:6px;max-width:200px;padding:3px 7px;' +
  'background:var(--sheet);border:1px solid var(--ink);box-shadow:var(--print);color:var(--ink);' +
  'font:600 10.5px/1.35 var(--mono);white-space:nowrap;' +
  'pointer-events:none;will-change:transform';
const FIT_FILL = 0.85; // the plaza spans this share of the stage on its tighter axis

/** A room on its component: its centre on the board (cx, cy), its slot's top (y) and its cell (w, d). */
type District = BoardDistrict;

/** A robot's visit or chat. In a `talk` the partner stands still, facing it. */
interface Social {
  phase: 'go' | 'talk';
  partner: Bot | null;
  gx: number; // where it walks to, grid units
  gy: number;
  until: number; // when the social ends, ms; Infinity until the walk is done
}

interface Bot {
  agent: Agent;
  district: District;
  home: [number, number]; // console position
  x: number; // current position, grid units
  y: number;
  tx: number; // wander target
  ty: number;
  heading: number; // radians about the vertical axis
  route: [number, number][]; // the trace walkway still to walk to reach the goal (empty within a room)
  routeKey: string; // the goal the route was found for
  moving: boolean;
  py: number; // height of the feet
  vy: number; // vertical speed (falling)
  state: RobotState | null;
  nextWander: number;
  nextSocial: number; // when an idle robot next considers a visit, chat or stroll
  social: Social | null;
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

/** A prop's ground footprint, in grid units. */
interface Footprint {
  x0: number;
  x1: number;
  z0: number;
  z1: number;
}

/** Whether (x, y) lies within a robot's width of a prop footprint. */
const near = (p: Footprint, x: number, y: number): boolean =>
  x > p.x0 - ROBOT_R && x < p.x1 + ROBOT_R && y > p.z0 - ROBOT_R && y < p.z1 + ROBOT_R;

/** Free geometry, materials and textures under an object. */
function disposeTree(obj: THREE.Object3D): void {
  obj.traverse((o) => {
    const mesh = o as THREE.Mesh;
    mesh.geometry?.dispose();
    const mats = Array.isArray(mesh.material) ? mesh.material : mesh.material ? [mesh.material] : [];
    for (const m of mats) {
      (m as THREE.MeshBasicMaterial).map?.dispose();
      m.dispose();
    }
  });
}

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
function newRenderer(): THREE.WebGLRenderer | null {
  try {
    return new THREE.WebGLRenderer({ antialias: true });
  } catch {
    return null;
  }
}

/** The plaza of one machine (today's Floor). Without WebGL the 2.5D canvas floor takes over. */
function mountPlaza(root: HTMLElement, machineId: string | null): Cleanup {
  const probe = document.createElement('canvas');
  const gl = probe.getContext('webgl2') ?? probe.getContext('webgl');
  const renderer = gl ? newRenderer() : null;
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
  const { scene, ground, backdrop, skyLayer } = nightScene();
  const { camera, controls } = orbitView(canvas);
  // The board floats on the night: no skyline, and a dark ground sheet well under it.
  backdrop.visible = false;
  (ground.material as THREE.Material).dispose();
  ground.material = toonMat({ color: 0x0b0d1f });
  ground.position.y = -BOARD_DROP;

  // Shared resources: the rooms of the board, and the board itself once the machine is known.
  const districtGroup = new THREE.Group();
  scene.add(districtGroup);
  let board: BoardScene | null = null;
  let layoutNow: BoardLayout | null = null;
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
  const stage = h('div.floor-stage.floor-stage-3d', null, tags, card, legend);
  stage.style.position = 'relative';
  stage.prepend(canvas);
  root.classList.add('floor-view');
  document.body.classList.add('floor-mode');
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
        machineId === null ? null : button('Back to machines', () => go('floor')),
        button('Reset view', () => resetView(), { icon: 'refresh' }),
      ),
    ),
    stage,
  );

  let districtSig = '';
  let bots: Bot[] = [];
  const byCid = new Map<number, Bot>();
  const packets: Packet[] = [];
  const propBoxes: Footprint[] = []; // prop footprints on the ground
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
  // The machine this plaza shows: its members (null until the fleet says, meaning every agent), its status and its board.
  let members: Set<number> | null = null;
  let machineStatus: FleetStatus = 'online';
  let lastList: Agent[] = [];
  let alive = true;
  let settingView = false; // resetView's own camera moves are not the user's
  controls.addEventListener('change', () => {
    if (!settingView) userMoved = true;
  });
  let last = performance.now();
  const ndcVec = new THREE.Vector2();
  const raycaster = new THREE.Raycaster();
  const camDir = new THREE.Vector3();
  const UP = new THREE.Vector3(0, 1, 0);
  const slopeN = new THREE.Vector3();
  const tiltQ = new THREE.Quaternion();
  const yawQ = new THREE.Quaternion();

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
    const w = g.measureText(text).width;
    if (w > 460) g.font = `500 ${Math.floor((48 * 460) / w)}px ui-monospace, SFMono-Regular, Menlo, monospace`;
    g.fillText(text, 256, 66);
    const tex = new THREE.CanvasTexture(cv);
    tex.colorSpace = THREE.SRGBColorSpace;
    return tex;
  }

  /**
   * A room on its component: a terrace body from the PCB up to the slot's top (filling the cell, sloping down over
   * RAMP as boardHeightAt does), the room's floor at the slot's top, and the role's plaque at its open front.
   */
  function districtMesh(d: District, pcb: number): THREE.Group {
    const g = new THREE.Group();
    const orch = d.role === 'orchestrator';
    const round = d.shape === 'round';
    const hs = Math.min(d.w, d.d) / 2; // the cell's half-size: the terrace's flat top
    const rise = Math.max(0.05, d.y - pcb);
    const body = round
      ? new THREE.CylinderGeometry(hs, hs + RAMP, rise, 48)
      : new THREE.CylinderGeometry(hs * Math.SQRT2, (hs + RAMP) * Math.SQRT2, rise, 4, 1).rotateY(Math.PI / 4);
    const slab = new THREE.Mesh(body, slabSide(d.hue));
    slab.position.set(d.cx, pcb + rise / 2, d.cy);
    slab.castShadow = true;
    slab.receiveShadow = true;
    g.add(slab);
    // Neon in the district's hue along the terrace's top edge and round its foot.
    const neon = new THREE.MeshBasicMaterial({ color: new THREE.Color().setHSL(d.hue / 360, 0.95, 0.62, THREE.SRGBColorSpace), toneMapped: false });
    for (const [rr, y] of [[hs, d.y + 0.01], [hs + RAMP, pcb + 0.03]] as const) {
      const edge = round
        ? new THREE.Mesh(new THREE.TorusGeometry(rr, 0.045, 6, 72), neon)
        : new THREE.Mesh(new THREE.TorusGeometry(rr * Math.SQRT2, 0.045, 6, 4), neon);
      edge.rotation.set(Math.PI / 2, 0, round ? 0 : Math.PI / 4);
      edge.position.set(d.cx, y, d.cy);
      edge.userData.outline = false;
      g.add(edge);
    }
    // The room faces the board's centre: square rooms turn in quarter turns so they stay square to their cell.
    const facing = Math.atan2(-d.cx, -d.cy);
    const turn = round ? facing : Math.round(facing / (Math.PI / 2)) * (Math.PI / 2);
    const room = buildRoom(d.role, d.r, d.hue, roomPalette);
    const top = new THREE.Mesh(round ? new THREE.CircleGeometry(d.r, 48) : new THREE.PlaneGeometry(2 * d.r, 2 * d.r), room.floor);
    top.rotation.set(-Math.PI / 2, 0, turn);
    top.position.set(d.cx, d.y + 0.004, d.cy);
    top.receiveShadow = true;
    g.add(top);
    room.group.position.set(d.cx, d.y, d.cy);
    room.group.rotation.y = turn;
    g.add(room.group);
    room.group.updateMatrixWorld(true);
    for (const part of room.group.children) {
      if (part.userData.walk) continue; // rugs and floor markings
      const box = new THREE.Box3().setFromObject(part);
      if (box.isEmpty() || box.min.y > d.y + 2.2) continue; // overhead pieces do not block the floor
      propBoxes.push({ x0: box.min.x, x1: box.max.x, z0: box.min.z, z1: box.max.z });
    }
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
  function buildWorld(ds: District[], pcb: number): void {
    for (const child of [...districtGroup.children]) {
      districtGroup.remove(child);
      disposeTree(child);
    }
    litByRole.clear();
    propAnim.length = 0;
    propBoxes.length = 0;
    for (const d of ds) districtGroup.add(districtMesh(d, pcb));
    // The world was just rebuilt (the old one is disposed above), so bake only this fresh static subtree.
    const stats = { districts: bakeStatic(districtGroup) };
    console.debug('floor: baked', stats);
    const box = new THREE.Box3().setFromObject(districtGroup);
    roomsTop = box.isEmpty() ? 2 : box.max.y;
  }

  /**
   * A machine's board as the plaza: the model at the origin with its copper traces, the current running along each
   * component's traces, the atom over its anchor, and a glow plate over each slot (lit while its rooms work).
   */
  function buildBoard(model: Model, params: Params, key: string): BoardScene {
    const group = new THREE.Group();
    const built = buildModel(model, params);
    group.add(built.group, traceMeshes(model));
    const plates = new Map<string, GlowPlate>();
    for (const slot of model.slots ?? []) {
      if (!built.parts.has(slot.on)) continue; // a slot whose part is hidden by its `when` does not exist
      const [x0, z0, w, d] = slot.rect;
      const mat = new THREE.MeshBasicMaterial({ color: C.info, transparent: true, opacity: 0, depthWrite: false, blending: THREE.AdditiveBlending, toneMapped: false });
      const mesh = new THREE.Mesh(new THREE.PlaneGeometry(w, d), mat);
      mesh.rotation.x = -Math.PI / 2;
      mesh.position.set(x0 + w / 2, slot.top + 0.012, z0 + d / 2);
      mesh.userData.outline = false;
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
    scene.add(group);
    return { key, model, params, group, built, graph: traceGraph(model), pulses, plates, atom: atom?.animate ?? null };
  }

  // Robots, labels, pads.
  function createBot(agent: Agent, district: District, hx: number, hy: number): Bot {
    const orch = agent.tier === 'orchestrator';
    const robot = makeRobot(district.hue, orch, ROBOT_PALETTE, agentHue(agent.cid), robotVariant(agent.cid));
    robot.group.traverse((o) => {
      o.castShadow = true;
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
    return {
      agent,
      district,
      home: [hx, hy],
      x: hx,
      y: hy,
      tx: hx,
      ty: hy,
      heading: 0,
      route: [],
      routeKey: '',
      moving: false,
      py: 0,
      vy: 0,
      state: null,
      nextWander: performance.now() + Math.random() * 3000,
      nextSocial: performance.now() + 4000 + Math.random() * 10000,
      social: null,
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
    const mine = list.filter((a) => !members || members.has(a.cid));
    const laid = layoutOnBoard(mine, board.model, board.params);
    layoutNow = laid;
    const sig = JSON.stringify(laid.districts);
    if (sig !== districtSig) {
      districtSig = sig;
      buildWorld(laid.districts, laid.pcbTop);
      if (!userMoved) resetView();
    }
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
        bot = createBot(agent, district, hx, hy);
        byCid.set(agent.cid, bot);
      }
      Object.assign(bot, { agent, district, home: [hx, hy] as [number, number] });
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

  /** Shows the named board with its params, building it unless it is the one already shown, then lays the agents on it. */
  function showBoard(name: string, params: Params): void {
    const key = `${name}|${JSON.stringify(params)}`;
    if (board?.key !== key) {
      if (board) {
        scene.remove(board.group);
        disposeTree(board.group);
      }
      board = buildBoard(MODELS[name] ?? MODELS[DEFAULT_BOARD.name]!, params, key);
    }
    sync(lastList);
  }

  /** Reads the fleet and shows its machine (the one named, else the hub) with its agents and status. */
  function applyFleet(fleet: FleetResponse): void {
    const m = fleet.machines.find((x) => x.id === machineId) ?? fleet.machines.find((x) => x.role === 'hub') ?? fleet.machines[0];
    if (!m) return applyDefault();
    if (machineId !== null) crumbName.textContent = m.name;
    const { name, params } = boardOf(m);
    members = new Set(m.agents);
    machineStatus = m.status;
    showBoard(name, params);
  }

  /** The workstation board, every agent on it: for when neither the fleet nor the machines list can be read. */
  function applyDefault(): void {
    members = null;
    showBoard(DEFAULT_BOARD.name, DEFAULT_BOARD.params);
  }

  /**
   * Reads the fleet every few seconds. When it cannot be read, the machines list's first node (n0, the hub) shows
   * its board; when that fails too, the workstation board. The old ring is never shown.
   */
  async function refresh(): Promise<void> {
    try {
      const fleet = await api.fleet();
      if (alive) applyFleet(fleet);
      return;
    } catch {
      // the machines list below
    }
    try {
      const nodes = await api.machines();
      const node = nodes.find((n) => n.id === 'n0') ?? nodes[0];
      if (alive && node) {
        members = null;
        const { name, params } = boardOf(node);
        showBoard(name, params);
        return;
      }
    } catch {
      // the workstation board below
    }
    if (alive) applyDefault();
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
  }

  /**
   * The default view: the camera distance at which the plaza fills FIT_FILL of the stage on its tighter axis, and the
   * point it is aimed at. The board's footprint (its corners, at the floor and at the rooms' top) is projected;
   * the nearest distance where every point is in front of the lens and inside that share of the frame is found by
   * bisection. Perspective puts the near side of the ring lower on screen than the far side is high, so the aim moves
   * to the ring's projected middle and the fit is run again.
   */
  /** The board's footprint (layout bounds) from the floor to the rooms' top, as its eight corners, and its centre. */
  function fitBox(): { centre: THREE.Vector3; ring: THREE.Vector3[] } {
    const b = layoutNow?.bounds ?? { cx: 0, cz: 0, w: 64, d: 56 };
    const top = Math.max(roomsTop, (board?.model.anchors?.atom?.[1] ?? 0) + 1);
    const ring: THREE.Vector3[] = [];
    for (const x of [b.cx - b.w / 2, b.cx + b.w / 2]) {
      for (const z of [b.cz - b.d / 2, b.cz + b.d / 2]) for (const y of [0, top]) ring.push(new THREE.Vector3(x, y, z));
    }
    return { centre: new THREE.Vector3(b.cx, top / 2, b.cz), ring };
  }

  function fitView(): { dist: number; target: THREE.Vector3 } {
    const { centre, ring } = fitBox();
    const target = centre;
    const dir = new THREE.Vector3().setFromSphericalCoords(1, POLAR, AZIMUTH);
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
    const { dist, target } = fitView();
    controls.maxDistance = Math.max(300, dist * 2); // the orbit limit never pulls a large plaza in
    if (scene.fog instanceof THREE.Fog) {
      // The haze keeps its look at the fitted distance: the plaza stays clear and the far horizon still fades.
      scene.fog.near = dist * 0.8;
      scene.fog.far = dist * 3;
    }
    controls.target.copy(target);
    camera.position.copy(target).add(new THREE.Vector3().setFromSphericalCoords(dist, POLAR, AZIMUTH));
    settingView = true;
    controls.update();
    settingView = false;
    userMoved = false;
  }

  // Social behaviour: an idle robot may visit a neighbour, chat, or stroll to the ring.
  const nearby = (a: District, c: District): boolean => a.role === c.role || Math.hypot(a.cx - c.cx, a.cy - c.cy) <= SOCIAL_NEAR;

  function startSocial(b: Bot, now: number): void {
    b.nextSocial = now + 8000 + Math.random() * 12000;
    const pool = bots.filter((o) => o !== b && o.agent.status === 'idle' && !o.social && nearby(o.district, b.district));
    const partner = pool[Math.floor(Math.random() * pool.length)];
    if (!partner) return;
    b.social = { phase: 'go', partner, gx: partner.x - 1, gy: partner.y, until: Infinity };
    partner.social = { phase: 'talk', partner: b, gx: partner.x, gy: partner.y, until: Infinity };
  }

  function endSocial(b: Bot, now: number): void {
    const s = b.social;
    b.social = null;
    b.nextWander = now;
    if (s?.partner && s.partner.social?.partner === b) {
      s.partner.social = null;
      s.partner.nextWander = now;
    }
  }

  /** The surface under (x, y): a room's floor, its ramp down to the PCB, or the PCB top (boardHeightAt). */
  function surfaceAt(x: number, y: number): number {
    return layoutNow ? boardHeightAt(x, y, layoutNow) : 0;
  }

  /**
   * The point a robot walks to next on its way to (gx, gy). Within a room it is the goal itself; to another room it is
   * the walk along the traces (pathOnTraces), point by point, when the traces join the two; else the goal.
   */
  function nextStop(b: Bot, gx: number, gy: number): [number, number] {
    if (!board || !layoutNow) return [gx, gy];
    const key = `${gx},${gy}`;
    if (b.routeKey !== key) {
      b.routeKey = key;
      b.route = [];
      const to = roomAt(layoutNow, gx, gy);
      if (to && to !== roomAt(layoutNow, b.x, b.y)) {
        const path = pathOnTraces(board.graph, [b.x, b.y], [gx, gy]);
        if (path) b.route = path.slice(1);
      }
    }
    while (b.route.length && Math.hypot(b.route[0]![0] - b.x, b.route[0]![1] - b.y) < 0.05) b.route.shift();
    return b.route[0] ?? [gx, gy];
  }

  /** Whether a robot centred at (x, y) would clash with a prop, leave its slab, or overlap another robot. */
  function blocked(b: Bot, x: number, y: number, checkProps: boolean): boolean {
    if (checkProps) {
      for (const p of propBoxes) if (near(p, x, y) && !near(p, b.x, b.y)) return true;
    }
    const partner = b.social?.partner ?? null;
    for (const o of bots) {
      if (o === b || o === partner) continue;
      const d = Math.hypot(o.x - x, o.y - y);
      if (d < ROBOT_R * 2 && d < Math.hypot(o.x - b.x, o.y - b.y)) return true;
    }
    return false;
  }

  /** Robots: wander or socialise when idle, head home when working or offline, animate, tint pads. */
  function updateBots(dt: number, now: number): void {
    const t = now / 1000;
    for (const p of litByRole.values()) p.lit = false;
    for (const b of bots) {
      const status = b.agent.status;
      const roaming = status === 'idle';
      const partner = b.social?.partner ?? null;
      if (b.social && (!roaming || (partner && (byCid.get(partner.agent.cid) !== partner || partner.agent.status !== 'idle')))) {
        endSocial(b, now);
      }
      if (roaming && !b.social) {
        if (now >= b.nextWander) {
          const span = Math.max(0.5, b.district.r - 0.7);
          b.tx = b.district.cx + (Math.random() * 2 - 1) * span;
          b.ty = b.district.cy + (Math.random() * 2 - 1) * span;
          b.nextWander = now + 3000 + Math.random() * 4000;
        }
        if (now >= b.nextSocial) startSocial(b, now);
      }
      const s = b.social;
      let gx = b.home[0];
      let gy = b.home[1];
      if (roaming) {
        gx = s ? s.gx : b.tx;
        gy = s ? s.gy : b.ty;
      }
      // The next stop on the way: the goal, or the trace walkway's next point when the goal is in another room.
      const [stopX, stopY] = nextStop(b, gx, gy);
      const dx = stopX - b.x;
      const dy = stopY - b.y;
      const dist = Math.hypot(dx, dy);
      b.moving = dist > 0.02;
      // Step toward the stop. A blocked step tries nearby headings, else the robot waits this frame.
      const checkProps = Math.hypot(b.home[0] - b.x, b.home[1] - b.y) > 0.9;
      let travel: number | null = null;
      if (b.moving) {
        const step = Math.min(SPEED * dt, dist);
        const toGoal = Math.atan2(dx, dy);
        for (const off of STEER) {
          const ang = toGoal + off;
          const nx = b.x + Math.sin(ang) * step;
          const ny = b.y + Math.cos(ang) * step;
          if (blocked(b, nx, ny, checkProps)) continue;
          b.x = nx;
          b.y = ny;
          travel = ang;
          break;
        }
      }
      // Social arrivals: a visitor that has reached its partner starts the chat.
      if (s && roaming) {
        if (s.phase === 'go' && !b.moving) {
          const until = now + 3000 + Math.random() * 2000;
          s.phase = 'talk';
          s.until = until;
          if (s.partner?.social) s.partner.social.until = until;
        }
        if (now >= s.until) endSocial(b, now);
      }
      // Local +Z is forward. Talkers face each other; walkers face their direction of travel.
      const talker = s && s.phase === 'talk' ? s.partner : null;
      let want: number | null = null;
      if (talker) want = Math.atan2(talker.x - b.x, talker.y - b.y);
      else if (b.moving) want = travel ?? Math.atan2(dx, dy);
      if (want !== null) {
        const diff = Math.atan2(Math.sin(want - b.heading), Math.cos(want - b.heading));
        b.heading += diff * Math.min(1, dt * 8);
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
      // Gravity: walk up slopes, fall where the ground drops away.
      const ground = surfaceAt(b.x, b.y);
      if (b.py > ground + 0.001) {
        b.vy -= GRAVITY * dt;
        b.py = Math.max(ground, b.py + b.vy * dt);
        if (b.py === ground) b.vy = 0;
      } else {
        b.py = ground;
        b.vy = 0;
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
    if (from === to) return;
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
  const shown = (b: Bot): string => (b.agent.status === 'offline' ? 'offline' : b.social?.phase === 'talk' ? '· · ·' : b.activity);

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
      const s = labelShown(b) ? toScreen(b.robot.labelAnchor(new THREE.Vector3())) : null;
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
  listen('dblclick', (ev) => {
    const bot = pick(ev);
    if (bot) go('agents', bot.agent.cid);
  });

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

  // Loop.
  // Capped at 60 fps: a frame that comes too soon is skipped whole, and `last` moves only on rendered frames.
  let lastRender = -Infinity;
  const frame = (now: number): void => {
    if (now - lastRender < 1000 / 60 - 1) return;
    lastRender = now;
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    const t = now / 1000;
    updateBots(dt, now);
    updatePackets(now);
    updateDust(t);
    updateBoard(dt, t);
    skyLayer.animate(t);
    for (const fn of propAnim) fn(t);
    controls.update();
    renderer.render(scene, camera);
    updateLabels();
    positionCard();
  };
  renderer.setAnimationLoop(frame);

  return () => {
    alive = false;
    window.clearInterval(poll);
    ac.abort();
    renderer.setAnimationLoop(null);
    ro.disconnect();
    offAgents();
    offTranscript();
    offDelta();
    offConversation();
    controls.dispose();
    for (const b of [...bots]) removeBot(b);
    bots = [];
    byCid.clear();
    for (const p of packets.splice(0)) dropPacket(p);
    disposeTree(districtGroup);
    if (board) disposeTree(board.group);
    disposeTree(ground);
    disposeTree(skyLayer.group);
    disposeTree(backdrop);
    dustGeo.dispose();
    dustMat.dispose();
    padGeo.dispose();
    packetGeo.dispose();
    haloGeo.dispose();
    tokenGeo.dispose();
    for (const m of Object.values(mats)) m.dispose();
    renderer.dispose();
    root.classList.remove('floor-view');
    document.body.classList.remove('floor-mode');
    root.replaceChildren();
  };
}

/** The canvas every view draws into: pixel ratio, soft cartoon shadows, filling its stage. */
function viewCanvas(renderer: THREE.WebGLRenderer, label: string): HTMLCanvasElement {
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  renderer.shadowMap.enabled = true;
  renderer.shadowMap.type = THREE.PCFShadowMap; // crisp, cartoon shadows
  const canvas = renderer.domElement;
  canvas.classList.add('floor-canvas');
  canvas.setAttribute('aria-label', label);
  canvas.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;display:block;cursor:grab';
  return canvas;
}

/** The night every view shares: sky and stars, moonlight, the ground sheet and the outskirts. */
function nightScene(): { scene: THREE.Scene; ground: THREE.Mesh; backdrop: ReturnType<typeof buildBackdrop>; skyLayer: ReturnType<typeof starfield> } {
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
  const backdrop = buildBackdrop();
  scene.add(backdrop);
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
  const machineId = params[0];
  if (machineId !== undefined) return mountPlaza(root, machineId);
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

/** The Machines view: every machine of the plaza as its model (floor3d/fleet.ts), polled every few seconds. */
function mountMachines(root: HTMLElement, first: FleetResponse): Cleanup {
  const renderer = newRenderer();
  if (!renderer) return mountFloor2d(root);
  const canvas = viewCanvas(renderer, 'The machines');
  const { scene, ground, backdrop, skyLayer } = nightScene();
  const { camera, controls } = orbitView(canvas);
  const fleetScene = buildFleet(first);
  scene.add(fleetScene.group);
  fleetScene.update(0, first); // builds the machines now, so the first fit frames them
  let latest = first;
  let fitSig = fleetSig(first);

  // DOM: the stage with the canvas, the machine tags, the popup card and the legend.
  const card = h('div.floor-card', { hidden: true });
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
  root.append(
    h('div.view-head', null, h('h1', null, 'Floor'), h('div.actions', null, button('Reset view', () => resetView(), { icon: 'refresh' }))),
    stage,
  );

  let width = 0;
  let height = 0;
  let hoveredId: string | null = null;
  let selectedId: string | null = null;
  let cardSig = '';
  let seenEl: HTMLElement | null = null; // the card's last-seen value, updated in place each frame
  let userMoved = false; // until the user moves the camera, the view refits on resize and when the machines change
  let settingView = false; // resetView's own camera moves are not the user's
  controls.addEventListener('change', () => {
    if (!settingView) userMoved = true;
  });
  let last = performance.now();
  let lastRender = -Infinity;
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
        h('footer.machine-card-foot', null, button('ENTER', () => go('floor', m.id), { kind: 'primary' })),
      );
    }
    const seen = ago(m.last_seen_ms, now);
    if (seenEl && seenEl.textContent !== seen) seenEl.textContent = seen;
  }

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
    cardSig = '';
    renderCard(Date.now());
    positionCard();
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
  };
  const ro = new ResizeObserver(() => resize());
  ro.observe(stage);
  resize();
  resetView();

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
  const offAgents = agents.watch(() => {
    cardSig = '';
  });
  const poll = window.setInterval(() => {
    api.fleet().then(
      (f) => {
        if (alive) latest = f;
      },
      () => undefined, // keep the last fleet until the next poll
    );
  }, FLEET_POLL_MS);

  // Loop: capped at 60 fps, as the plaza's.
  const frame = (now: number): void => {
    if (now - lastRender < 1000 / 60 - 1) return;
    lastRender = now;
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    fleetScene.update(dt, latest);
    const sig = fleetSig(latest);
    if (sig !== fitSig) {
      fitSig = sig; // the machines changed: the default view refits
      if (!userMoved) resetView();
    }
    skyLayer.animate(now / 1000);
    controls.update();
    renderer.render(scene, camera);
    updateTags();
    renderCard(Date.now());
    positionCard();
  };
  renderer.setAnimationLoop(frame);

  return () => {
    alive = false;
    ac.abort();
    window.clearInterval(poll);
    renderer.setAnimationLoop(null);
    ro.disconnect();
    offAgents();
    controls.dispose();
    for (const t of tagEls.values()) t.el.remove();
    tagEls.clear();
    disposeTree(fleetScene.group);
    disposeTree(ground);
    disposeTree(skyLayer.group);
    disposeTree(backdrop);
    renderer.dispose();
    root.classList.remove('floor-view');
    document.body.classList.remove('floor-mode');
    root.replaceChildren();
  };
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

/** The facts the popup lists, in order. The last-seen time is live, so the card adds it itself. */
export function machineFacts(m: FleetMachine): [string, string][] {
  return [
    ['CPU', m.cpu.model || '—'],
    ['Cores', `${m.cpu.cores} cores · ${m.cpu.threads} threads`],
    ['Memory', gibText(m.memory_mib)],
    // A GPU's memory is null when the fleet does not know it: its name stands alone.
    ['GPUs', m.gpus.length ? m.gpus.map((g) => (g.memory_mib == null ? g.name : `${g.name} · ${gibText(g.memory_mib)}`)).join('\n') : 'none'],
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
