// The Floor: districts of robots on a three.js plaza. The scene owns placement,
// wandering, socialising, packets and labels; each robot owns its own animation
// (floor3d/robot), and the furniture comes from floor3d/props.

import '../styles/floor.css';

import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';

import { type Agent, type Conversation, type Entry } from '../api';
import { go } from '../router';
import { agents, on } from '../store';
import { button, clip } from '../ui/components';
import { floorName, roleLabel } from '../roles';
import { h } from '../ui/dom';
import { makeRobot, ROBOT_PALETTE, robotVariant, type Robot, type RobotState } from './floor3d/robot';
import { buildPaths, buildPlaza, buildPlazaFurniture } from './floor3d/props';
import { buildBackdrop, buildScatter } from './floor3d/outskirts';
import { buildRoom, roomStyle } from './floor3d/rooms';
import { ground as groundTex, rng, slabSide } from './floor3d/textures';
import { outline, sky, starfield, toonMat } from './floor3d/toon';
import { bakeStatic } from './floor3d/bake';
import { mountFloor2d } from './floor2d';
import type { Cleanup } from './index';

const PACKET_MS = 1500;
const SPEED = 1.6; // grid units per second; one grid cell is one world unit
const MAX_PACKETS = 60;
const SLAB_H = 0.5; // district slab height
const DAIS_H = 0.8; // orchestrator dais height
const DAIS_R = 2.3;
const RAMP = 1.5; // width of the sloped band around a district slab
const DAIS_RAMP = 2.2; // width of the dais's sloped skirt
const GRAVITY = 9.8; // world units per second squared
const POLAR = 0.95; // camera angle from vertical, about 55 degrees: isometric-ish
const AZIMUTH = Math.PI / 4;
const SOCIAL_NEAR = 9.5; // districts this close count as neighbours for a visit or a chat
const STROLL_R = 3.1; // the stroll ring just outside the dais
const ROBOT_R = 0.55; // robot footprint radius for collisions
const STEER = [0, 30, -30, 60, -60].map((d) => (d * Math.PI) / 180); // headings tried when a step is blocked
const DUST_N = 90;
const LABEL_CSS =
  'position:absolute;left:0;top:0;display:none;align-items:center;gap:6px;max-width:200px;padding:3px 7px;' +
  'background:var(--sheet);border:1px solid var(--ink);box-shadow:var(--print);color:var(--ink);' +
  'font:600 10.5px/1.35 var(--mono);white-space:nowrap;' +
  'pointer-events:none;will-change:transform';
const FIT_MARGIN = 1.12; // the whole plaza in frame with a little air around it

interface District {
  role: string;
  cx: number; // centre, grid units
  cy: number;
  r: number; // half-size (square) or radius (round), grid units
  hue: number;
  shape: 'square' | 'round';
}

/** A robot's visit, chat or stroll. In a `talk` the partner stands still, facing it. */
interface Social {
  phase: 'go' | 'talk' | 'stroll';
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

/** Districts by role around a central dais; each agent gets a console slot. */
export function layoutDistricts(list: Agent[]): { districts: District[]; homes: Map<number, [District, number, number]> } {
  const groups = new Map<string, Agent[]>();
  for (const a of list) {
    const key = a.tier === 'orchestrator' ? '' : a.role;
    groups.set(key, [...(groups.get(key) ?? []), a]);
  }
  const districts: District[] = [];
  const homes = new Map<number, [District, number, number]>();
  const place = (d: District, members: Agent[]) => {
    districts.push(d);
    const side = Math.max(1, Math.ceil(Math.sqrt(members.length)));
    members.sort((a, b) => a.cid - b.cid).forEach((a, i) => {
      const span = 1.5 * (side - 1); // homes sit 3 apart
      const gx = d.cx - span + (i % side) * 3;
      const gy = d.cy - span + Math.floor(i / side) * 3;
      homes.set(a.cid, [d, gx, gy]);
    });
  };
  const centre = groups.get('');
  if (centre) place({ role: 'orchestrator', cx: 0, cy: 0, r: 2, hue: 25, shape: 'round' }, centre);
  const roles = [...groups.keys()].filter((k) => k).sort();
  // Each room is its style's size (bigger when crowded). Rooms share the ring by width: each gets
  // an arc as wide as itself plus a gap, and the ring is as large as the arcs need.
  const sizeOf = (role: string): number => {
    const n = groups.get(role)!.length;
    return Math.max(roomStyle(role).size, 1.5 * Math.max(1, Math.ceil(Math.sqrt(n))) + 1.5);
  };
  const GAP = 2.4;
  const widths = roles.map((role) => 2 * (sizeOf(role) + RAMP) + GAP);
  const total = widths.reduce((a, b) => a + b, 0);
  const ringR = Math.max(10 + RAMP, total / (2 * Math.PI));
  let arc = 0;
  roles.forEach((role, i) => {
    const members = groups.get(role)!;
    const angle = ((arc + widths[i]! / 2) / total) * Math.PI * 2 - Math.PI / 2;
    arc += widths[i]!;
    const r = sizeOf(role);
    // A square room's corners reach further out than its side: push it out so it clears its neighbours.
    const out = roomStyle(role).shape === 'square' ? ringR + r * 0.3 : ringR;
    place(
      { role, cx: Math.cos(angle) * out, cy: Math.sin(angle) * out, r, hue: roleHue(role), shape: roomStyle(role).shape },
      members,
    );
  });
  return { districts, homes };
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

const topOf = (d: District): number => (d.role === 'orchestrator' ? DAIS_H : SLAB_H);

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

export function mount(root: HTMLElement): Cleanup {
  // Without WebGL the 2.5D canvas floor takes over.
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

  // Renderer, scene, camera, controls.
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  renderer.shadowMap.enabled = true;
  renderer.shadowMap.type = THREE.PCFShadowMap; // crisp, cartoon shadows
  const canvas = renderer.domElement;
  canvas.classList.add('floor-canvas');
  canvas.setAttribute('aria-label', 'The plaza floor');
  canvas.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;display:block;cursor:grab';

  const scene = new THREE.Scene();
  // Night: a deep blue zenith banded down to a violet horizon glow, stars and a ringed planet.
  const SKY_TOP = new THREE.Color(0x05071a);
  const SKY_HORIZON = new THREE.Color(0x3a2275);
  scene.add(sky(SKY_TOP, SKY_HORIZON, new THREE.Color(0x090c18)));
  scene.fog = new THREE.Fog(0x161236, 60, 230);
  const skyLayer = starfield(900, 330, rng(31));
  skyLayer.group.userData.dynamic = true; // turns every frame
  scene.add(skyLayer.group);
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

  // Shared resources.
  const districtGroup = new THREE.Group();
  scene.add(districtGroup);
  const pathGroup = new THREE.Group();
  scene.add(pathGroup);
  const plaza = buildPlaza(C);
  plaza.userData.dynamic = true; // the xenon atom and its rings animate
  scene.add(plaza);
  const plazaAnim = plaza.userData.animate as (t: number) => void;
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
  root.append(
    h('div.view-head', null, h('h1', null, 'Floor'), h('div.actions', null, button('Reset view', () => resetView(), { icon: 'refresh' }))),
    stage,
  );

  let districts: District[] = [];
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
  // The plaza's bounds (set by buildWorld) and whether the user has moved the camera; until they do, the default view refits.
  const plazaBounds = new THREE.Sphere(new THREE.Vector3(0, 0.4, 0), 8);
  let userMoved = false;
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

  // Plaques, slabs and the dais.
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

  function districtMesh(d: District): THREE.Group {
    const g = new THREE.Group();
    const orch = d.role === 'orchestrator';
    if (orch) {
      const dais = new THREE.Mesh(
        new THREE.CylinderGeometry(DAIS_R, DAIS_R + DAIS_RAMP, DAIS_H, 48),
        toonMat({ color: 0x4a5690 }),
      );
      dais.position.set(d.cx, DAIS_H / 2, d.cy);
      dais.castShadow = true;
      dais.receiveShadow = true;
      const rim = new THREE.Mesh(new THREE.TorusGeometry(DAIS_R, 0.05, 8, 64), new THREE.MeshBasicMaterial({ color: C.signal, toneMapped: false }));
      rim.rotation.x = Math.PI / 2;
      rim.position.set(d.cx, DAIS_H + 0.01, d.cy);
      // A cyan ring of light round the foot of the skirt.
      const foot = new THREE.Mesh(new THREE.TorusGeometry(DAIS_R + DAIS_RAMP, 0.06, 8, 96), new THREE.MeshBasicMaterial({ color: C.info, toneMapped: false }));
      foot.rotation.x = Math.PI / 2;
      foot.position.set(d.cx, 0.04, d.cy);
      foot.userData.outline = false;
      g.add(dais, rim, foot);
    } else {
      // A terrace: the flat top plus a sloped band all round, so robots can walk on and off; its
      // floor is the room's own, laid as a separate surface so the texture runs square to the room.
      const round = d.shape === 'round';
      const body = round
        ? new THREE.CylinderGeometry(d.r, d.r + RAMP, SLAB_H, 48, 1)
        : new THREE.CylinderGeometry(d.r * Math.SQRT2, (d.r + RAMP) * Math.SQRT2, SLAB_H, 4, 1).rotateY(Math.PI / 4);
      const slab = new THREE.Mesh(body, slabSide(d.hue));
      slab.position.set(d.cx, SLAB_H / 2, d.cy);
      slab.castShadow = true;
      slab.receiveShadow = true;
      g.add(slab);
      // Neon in the district's hue along the slab's top edge and round its foot.
      const neon = new THREE.MeshBasicMaterial({ color: new THREE.Color().setHSL(d.hue / 360, 0.95, 0.62, THREE.SRGBColorSpace), toneMapped: false });
      for (const [rr, y] of [[d.r, SLAB_H + 0.01], [d.r + RAMP, 0.03]] as const) {
        const edge = round
          ? new THREE.Mesh(new THREE.TorusGeometry(rr, 0.045, 6, 72), neon)
          : new THREE.Mesh(new THREE.TorusGeometry(rr * Math.SQRT2, 0.045, 6, 4), neon);
        edge.rotation.set(Math.PI / 2, 0, round ? 0 : Math.PI / 4);
        edge.position.set(d.cx, y, d.cy);
        edge.userData.outline = false;
        g.add(edge);
      }
      // The room faces the plaza: square rooms turn in quarter turns so they stay square to their slab.
      const facing = Math.atan2(-d.cx, -d.cy);
      const turn = round ? facing : Math.round(facing / (Math.PI / 2)) * (Math.PI / 2);
      const room = buildRoom(d.role, d.r, d.hue, roomPalette);
      const top = new THREE.Mesh(round ? new THREE.CircleGeometry(d.r, 48) : new THREE.PlaneGeometry(2 * d.r, 2 * d.r), room.floor);
      top.rotation.set(-Math.PI / 2, 0, turn);
      top.position.set(d.cx, SLAB_H + 0.004, d.cy);
      top.receiveShadow = true;
      g.add(top);
      room.group.position.set(d.cx, SLAB_H, d.cy);
      room.group.rotation.y = turn;
      g.add(room.group);
      room.group.updateMatrixWorld(true);
      for (const part of room.group.children) {
        if (part.userData.walk) continue; // rugs and floor markings
        const box = new THREE.Box3().setFromObject(part);
        if (box.isEmpty() || box.min.y > SLAB_H + 2.2) continue; // overhead pieces do not block the floor
        propBoxes.push({ x0: box.min.x, x1: box.max.x, z0: box.min.z, z1: box.max.z });
      }
      litByRole.set(d.role, { screens: room.screens, lit: false });
      propAnim.push(room.animate);
    }
    // Role plaque lies flat on the back edge of the slab (the dais for the orchestrator).
    const plaque = new THREE.Mesh(
      new THREE.PlaneGeometry(2.6, 0.65),
      new THREE.MeshBasicMaterial({ map: plaqueTexture(d.role.toUpperCase(), orch), transparent: true, depthWrite: false }),
    );
    if (orch) {
      plaque.rotation.x = -Math.PI / 2;
      plaque.position.set(d.cx, topOf(d) + 0.01, d.cy - 1.6);
      g.add(plaque);
    } else {
      // Flat on the floor at the room's open front, readable from the plaza.
      const holder = new THREE.Group();
      holder.position.set(d.cx, topOf(d) + 0.012, d.cy);
      holder.rotation.y = Math.atan2(-d.cx, -d.cy);
      plaque.rotation.x = -Math.PI / 2;
      plaque.position.set(0, 0, d.r - 0.5);
      holder.add(plaque);
      g.add(holder);
    }
    outline(g);
    return g;
  }

  /** Districts and their paths, rebuilt whenever the layout changes. */
  function buildWorld(ds: District[]): void {
    for (const child of [...districtGroup.children]) {
      districtGroup.remove(child);
      disposeTree(child);
    }
    litByRole.clear();
    propAnim.length = 0;
    propBoxes.length = 0;
    for (const d of ds) districtGroup.add(districtMesh(d));
    disposeTree(pathGroup);
    pathGroup.clear();
    pathGroup.add(buildPaths(ds.map((d) => new THREE.Vector3(d.cx, 0, d.cy)), C));
    const fountain = buildPlazaFurniture(C, ds.map((d) => Math.atan2(d.cy, d.cx)));
    fountain.group.userData.dynamic = true; // the water animates
    pathGroup.add(fountain.group);
    propAnim.push(fountain.animate);
    for (const b of fountain.boxes) propBoxes.push(b);
    const outside = buildScatter(ds, C);
    pathGroup.add(outside.group);
    for (const b of outside.boxes) propBoxes.push(b);
    // The world was just rebuilt (the old one is disposed above), so bake only this fresh static subtree.
    const stats = { districts: bakeStatic(districtGroup), paths: bakeStatic(pathGroup) };
    console.debug('floor: baked', stats);
    // Every district, path and outside prop: the bounds the default view frames. Each mesh contributes its own
    // sphere, so the outside ring costs its radius rather than the corners of a box around it.
    const bounds = new THREE.Sphere().makeEmpty();
    for (const root of [districtGroup, pathGroup]) {
      root.traverse((o) => {
        const geo = (o as THREE.Mesh).geometry;
        if (!geo) return;
        geo.computeBoundingSphere();
        bounds.union(geo.boundingSphere!.clone().applyMatrix4(o.matrixWorld));
      });
    }
    if (!bounds.isEmpty()) plazaBounds.copy(bounds);
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

  function sync(list: Agent[]): void {
    const laid = layoutDistricts(list);
    districts = laid.districts;
    const sig = JSON.stringify(districts);
    if (sig !== districtSig) {
      districtSig = sig;
      buildWorld(districts);
      if (!userMoved) resetView();
    }
    const seen = new Set<number>();
    for (const agent of list) {
      const slot = laid.homes.get(agent.cid);
      if (!slot) continue;
      const [district, hx, hy] = slot;
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

  /**
   * Where the camera sits for the default view: the plaza's bounding sphere in frame. The tighter of the vertical and
   * horizontal half-angles decides the distance, so the outer ring is never cropped on any canvas aspect.
   */
  function resetView(): void {
    const vHalf = THREE.MathUtils.degToRad(camera.fov) / 2;
    const hHalf = Math.atan(Math.tan(vHalf) * camera.aspect);
    const dist = (plazaBounds.radius * FIT_MARGIN) / Math.sin(Math.min(vHalf, hHalf));
    controls.maxDistance = Math.max(300, dist * 2); // the orbit limit never pulls a large plaza in
    if (scene.fog instanceof THREE.Fog) {
      // The haze keeps its look at the fitted distance: the plaza stays clear and the far horizon still fades.
      scene.fog.near = dist * 0.8;
      scene.fog.far = dist * 3;
    }
    controls.target.copy(plazaBounds.center);
    camera.position.copy(plazaBounds.center).add(new THREE.Vector3().setFromSphericalCoords(dist, POLAR, AZIMUTH));
    settingView = true;
    controls.update();
    settingView = false;
    userMoved = false;
  }

  // Social behaviour: an idle robot may visit a neighbour, chat, or stroll to the ring.
  const nearby = (a: District, c: District): boolean => a.role === c.role || Math.hypot(a.cx - c.cx, a.cy - c.cy) <= SOCIAL_NEAR;

  function startSocial(b: Bot, now: number): void {
    b.nextSocial = now + 8000 + Math.random() * 12000;
    if (Math.random() < 0.3) {
      const ang = Math.atan2(b.y, b.x);
      b.social = { phase: 'stroll', partner: null, gx: Math.cos(ang) * STROLL_R, gy: Math.sin(ang) * STROLL_R, until: Infinity };
      return;
    }
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

  /** The top of whatever lies under (x, y): a dais, a district slab, or the bare floor. */
  function surfaceAt(x: number, y: number): number {
    let top = 0;
    for (const d of districts) {
      const orch = d.role === 'orchestrator';
      // Distance out from the walkable top: round for the dais and round rooms, square for the rest.
      const out = orch
        ? Math.hypot(x - d.cx, y - d.cy) - DAIS_R
        : d.shape === 'round'
          ? Math.hypot(x - d.cx, y - d.cy) - d.r
          : Math.max(Math.abs(x - d.cx), Math.abs(y - d.cy)) - d.r;
      const band = orch ? DAIS_RAMP : RAMP;
      const h = out <= 0 ? topOf(d) : out < band ? topOf(d) * (1 - out / band) : 0;
      top = Math.max(top, h);
    }
    return top;
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
      const dx = gx - b.x;
      const dy = gy - b.y;
      const dist = Math.hypot(dx, dy);
      b.moving = dist > 0.02;
      // Step toward the goal. A blocked step tries nearby headings, else the robot waits this frame.
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
      // Social arrivals: a visitor that has reached its partner starts the chat; a stroller waits briefly at the ring.
      if (s && roaming) {
        if (s.phase === 'go' && !b.moving) {
          const until = now + 3000 + Math.random() * 2000;
          s.phase = 'talk';
          s.until = until;
          if (s.partner?.social) s.partner.social.until = until;
        } else if (s.phase === 'stroll' && !b.moving && s.until === Infinity) {
          s.until = now + 2000;
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

  // Live data.
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
    plazaAnim(t);
    skyLayer.animate(t);
    for (const fn of propAnim) fn(t);
    controls.update();
    renderer.render(scene, camera);
    updateLabels();
    positionCard();
  };
  renderer.setAnimationLoop(frame);

  return () => {
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
    disposeTree(pathGroup);
    disposeTree(plaza);
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
