// The Floor: districts of robots on a three.js plaza. The scene owns placement,
// wandering, packets and labels; each robot owns its own animation (floor3d/robot).

import '../styles/floor.css';

import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';

import { type Agent, type Conversation, type Entry } from '../api';
import { go } from '../router';
import { agents, on } from '../store';
import { button, clip } from '../ui/components';
import { h } from '../ui/dom';
import { makeRobot, type Palette as RobotPalette, type Robot, type RobotState } from './floor3d/robot';
import type { Cleanup } from './index';

const PACKET_MS = 1500;
const SPEED = 1.6; // grid units per second; one grid cell is one world unit
const MAX_PACKETS = 60;
const SLAB_H = 0.5; // district slab height
const DAIS_H = 0.8; // orchestrator dais height
const DAIS_R = 2.3;
const POLAR = 0.95; // camera angle from vertical, about 55 degrees: isometric-ish
const AZIMUTH = Math.PI / 4;
const LABEL_CSS =
  'position:absolute;left:0;top:0;display:none;max-width:220px;padding:3px 6px;' +
  'background:var(--sheet);border:1px solid var(--ink);color:var(--ink);' +
  'font:11px/1.35 ui-monospace,SFMono-Regular,Menlo,monospace;white-space:nowrap;' +
  'pointer-events:none;will-change:transform';

interface District {
  role: string;
  cx: number; // centre, grid units
  cy: number;
  r: number; // half-size, grid units
  hue: number;
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
  state: RobotState | null;
  nextWander: number;
  activity: string; // what the agent is doing right now, shown on its label and card
  robot: Robot;
  robotKey: string;
  pad: THREE.Mesh; // console pad under the home slot, tinted by state
  padMat: THREE.MeshBasicMaterial;
  highlighted: boolean;
  label: HTMLDivElement;
  labelName: HTMLElement;
  labelLine: HTMLElement;
}

interface Packet {
  from: number;
  to: number;
  start: number;
  core: THREE.Mesh;
  halo: THREE.Mesh;
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

export function roleHue(role: string): number {
  let hash = 0;
  for (const ch of role) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  return hash % 360;
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
      const gx = d.cx - (side - 1) + (i % side) * 2;
      const gy = d.cy - (side - 1) + Math.floor(i / side) * 2;
      homes.set(a.cid, [d, gx, gy]);
    });
  };
  const centre = groups.get('');
  if (centre) place({ role: 'orchestrator', cx: 0, cy: 0, r: 2, hue: 25 }, centre);
  const roles = [...groups.keys()].filter((k) => k).sort();
  const ringR = 6 + Math.floor(roles.length / 6) * 2;
  roles.forEach((role, i) => {
    const members = groups.get(role)!;
    const side = Math.max(1, Math.ceil(Math.sqrt(members.length)));
    const angle = (i / roles.length) * Math.PI * 2 - Math.PI / 2;
    place(
      { role, cx: Math.round(Math.cos(angle) * ringR), cy: Math.round(Math.sin(angle) * ringR), r: side + 1, hue: roleHue(role) },
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

export function mount(root: HTMLElement): Cleanup {
  const palette = readPalette();
  const C = {
    sheet: colour(palette.sheet, '#ffffff'),
    rule: colour(palette.rule, '#d6d2c8'),
    ink3: colour(palette.ink3, '#8a8a8a'),
    signal: colour(palette.signal, '#d9482b'),
    ok: colour(palette.ok, '#2f8f5b'),
    info: colour(palette.info, '#2f6fb0'),
    paper: colour(palette.paper, '#f5f3ee'),
  };
  const robotPalette: RobotPalette = {
    ink: palette.ink,
    sheet: palette.sheet,
    paper: palette.paper,
    rule: palette.rule,
    signal: palette.signal,
    ok: palette.ok,
    info: palette.info,
    inkMuted: palette.ink3,
  };

  // Renderer, scene, camera, controls.
  const renderer = new THREE.WebGLRenderer({ antialias: true });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  renderer.shadowMap.enabled = true;
  renderer.shadowMap.type = THREE.PCFSoftShadowMap;
  const canvas = renderer.domElement;
  canvas.classList.add('floor-canvas');
  canvas.setAttribute('aria-label', 'The plaza floor');
  canvas.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;display:block;cursor:grab';

  const scene = new THREE.Scene();
  scene.background = C.paper;
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

  scene.add(new THREE.HemisphereLight(0xffffff, 0x9a9a9a, 1.6));
  const sun = new THREE.DirectionalLight(0xffffff, 1.4);
  sun.position.set(22, 40, 16);
  sun.castShadow = true;
  sun.shadow.mapSize.set(2048, 2048);
  sun.shadow.bias = -0.0008;
  const sc = sun.shadow.camera;
  Object.assign(sc, { left: -40, right: 40, top: 40, bottom: -40, near: 1, far: 140 });
  sc.updateProjectionMatrix();
  scene.add(sun);

  // Ground: a large sheet with a subtle grid.
  const ground = new THREE.Mesh(
    new THREE.PlaneGeometry(200, 200),
    new THREE.MeshStandardMaterial({ color: C.sheet, roughness: 1 }),
  );
  ground.rotation.x = -Math.PI / 2;
  ground.position.y = -0.01;
  ground.receiveShadow = true;
  scene.add(ground);
  const grid = new THREE.GridHelper(200, 100, C.rule, C.rule);
  const gridMat = grid.material as THREE.LineBasicMaterial;
  gridMat.transparent = true;
  gridMat.opacity = 0.35;
  gridMat.depthWrite = false;
  scene.add(grid);

  // Shared resources.
  const districtGroup = new THREE.Group();
  scene.add(districtGroup);
  const padGeo = new THREE.CircleGeometry(0.42, 28);
  const packetGeo = new THREE.SphereGeometry(0.13, 14, 10);
  const haloGeo = new THREE.SphereGeometry(0.34, 14, 10);
  const mats = {
    accent: new THREE.MeshBasicMaterial({ color: C.signal, toneMapped: false }),
    ok: new THREE.MeshBasicMaterial({ color: C.ok, toneMapped: false }),
    accentHalo: new THREE.MeshBasicMaterial({ color: C.signal, transparent: true, opacity: 0.35, depthWrite: false, blending: THREE.AdditiveBlending, toneMapped: false }),
    okHalo: new THREE.MeshBasicMaterial({ color: C.ok, transparent: true, opacity: 0.35, depthWrite: false, blending: THREE.AdditiveBlending, toneMapped: false }),
  };

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
  const stage = h('div.floor-stage', null, tags, card, legend);
  stage.style.position = 'relative';
  stage.prepend(canvas);
  root.classList.add('floor-view');
  document.body.classList.add('floor-mode');
  root.append(
    h('div.view-head', null, h('h1', null, 'Floor'), h('div.actions', null, button('Reset view', () => resetView(), { kind: 'ghost', icon: 'refresh' }))),
    stage,
  );

  let districts: District[] = [];
  let districtSig = '';
  let bots: Bot[] = [];
  const byCid = new Map<number, Bot>();
  const packets: Packet[] = [];
  let width = 0;
  let height = 0;
  let hovered: Bot | null = null;
  let selected: Bot | null = null;
  let cardSig = '';
  let fitted = false;
  let last = performance.now();
  const ndcVec = new THREE.Vector2();
  const raycaster = new THREE.Raycaster();
  const camDir = new THREE.Vector3();

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
        new THREE.CylinderGeometry(DAIS_R, DAIS_R + 0.2, DAIS_H, 48),
        new THREE.MeshStandardMaterial({ color: C.sheet, roughness: 0.9 }),
      );
      dais.position.set(d.cx, DAIS_H / 2, d.cy);
      dais.castShadow = true;
      dais.receiveShadow = true;
      const rim = new THREE.Mesh(new THREE.TorusGeometry(DAIS_R, 0.05, 8, 64), new THREE.MeshBasicMaterial({ color: C.signal }));
      rim.rotation.x = Math.PI / 2;
      rim.position.set(d.cx, DAIS_H + 0.01, d.cy);
      g.add(dais, rim);
    } else {
      const box = new THREE.BoxGeometry(2 * d.r, SLAB_H, 2 * d.r);
      const slab = new THREE.Mesh(
        box,
        new THREE.MeshStandardMaterial({ color: new THREE.Color().setHSL(d.hue / 360, 0.25, 0.86, THREE.SRGBColorSpace), roughness: 0.95 }),
      );
      slab.position.set(d.cx, SLAB_H / 2, d.cy);
      slab.castShadow = true;
      slab.receiveShadow = true;
      const edges = new THREE.LineSegments(new THREE.EdgesGeometry(box), new THREE.LineBasicMaterial({ color: C.rule }));
      edges.position.copy(slab.position);
      g.add(slab, edges);
    }
    // Role plaque lies flat on the back edge of the slab (the dais for the orchestrator).
    const plaque = new THREE.Mesh(
      new THREE.PlaneGeometry(2.6, 0.65),
      new THREE.MeshBasicMaterial({ map: plaqueTexture(d.role.toUpperCase(), orch), transparent: true, depthWrite: false }),
    );
    plaque.rotation.x = -Math.PI / 2;
    plaque.position.set(d.cx, topOf(d) + 0.01, orch ? d.cy - 1.6 : d.cy - d.r + 0.55);
    g.add(plaque);
    return g;
  }

  function buildDistricts(ds: District[]): void {
    for (const child of [...districtGroup.children]) {
      districtGroup.remove(child);
      disposeTree(child);
    }
    for (const d of ds) districtGroup.add(districtMesh(d));
  }

  // Robots, labels, pads.
  function createBot(agent: Agent, district: District, hx: number, hy: number): Bot {
    const orch = agent.tier === 'orchestrator';
    const robot = makeRobot(district.hue, orch, robotPalette);
    robot.group.traverse((o) => {
      o.castShadow = true;
      o.receiveShadow = true;
    });
    scene.add(robot.group);
    const padMat = new THREE.MeshBasicMaterial({ color: C.ink3, transparent: true, opacity: 0.6, depthWrite: false });
    const pad = new THREE.Mesh(padGeo, padMat);
    pad.rotation.x = -Math.PI / 2;
    pad.position.set(hx, topOf(district) + 0.02, hy);
    scene.add(pad);
    const label = document.createElement('div');
    label.style.cssText = LABEL_CSS;
    const labelName = document.createElement('strong');
    labelName.style.cssText = 'display:block;overflow:hidden;text-overflow:ellipsis';
    const labelLine = document.createElement('div');
    labelLine.style.cssText = 'color:var(--ink-2);overflow:hidden;text-overflow:ellipsis';
    label.append(labelName, labelLine);
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
      state: null,
      nextWander: performance.now() + Math.random() * 3000,
      activity: 'idle',
      robot,
      robotKey: `${district.hue}|${orch}`,
      pad,
      padMat,
      highlighted: false,
      label,
      labelName,
      labelLine,
    };
  }

  function removeBot(b: Bot): void {
    scene.remove(b.robot.group);
    b.robot.dispose();
    scene.remove(b.pad);
    b.padMat.dispose();
    b.label.remove();
  }

  function sync(list: Agent[]): void {
    const laid = layoutDistricts(list);
    districts = laid.districts;
    const sig = JSON.stringify(districts);
    if (sig !== districtSig) {
      districtSig = sig;
      buildDistricts(districts);
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

  /** Where the camera sits for the default view, fitted to the districts. */
  function resetView(): void {
    let extent = 6;
    for (const d of districts) extent = Math.max(extent, Math.abs(d.cx) + d.r, Math.abs(d.cy) + d.r);
    const dist = extent * 2.6 + 6;
    controls.target.set(0, 0.4, 0);
    camera.position.copy(controls.target).add(new THREE.Vector3().setFromSphericalCoords(dist, POLAR, AZIMUTH));
    controls.update();
  }

  /** Robots: wander when idle, head home when working or offline, animate, tint pads. */
  function updateBots(dt: number, now: number): void {
    const t = now / 1000;
    for (const b of bots) {
      const status = b.agent.status;
      const roaming = status === 'idle';
      if (roaming && now >= b.nextWander) {
        const span = Math.max(0.5, b.district.r - 0.7);
        b.tx = b.district.cx + (Math.random() * 2 - 1) * span;
        b.ty = b.district.cy + (Math.random() * 2 - 1) * span;
        b.nextWander = now + 3000 + Math.random() * 4000;
      }
      const [gx, gy] = roaming ? [b.tx, b.ty] : b.home;
      const dx = gx - b.x;
      const dy = gy - b.y;
      const dist = Math.hypot(dx, dy);
      const step = SPEED * dt;
      b.moving = dist > 0.02;
      if (dist > step) {
        b.x += (dx / dist) * step;
        b.y += (dy / dist) * step;
      } else {
        b.x = gx;
        b.y = gy;
      }
      if (b.moving) {
        // Local +Z is forward; turn toward the direction of travel.
        const want = Math.atan2(dx, dy);
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
      b.robot.group.position.set(b.x, topOf(b.district), b.y);
      b.robot.group.rotation.y = b.heading;
      b.robot.update(dt, t);
      b.padMat.color.copy(status === 'working' ? C.info : status === 'idle' ? C.ok : C.ink3);
    }
  }

  // Packets: glowing spheres on a curved arc between two robots.
  function sendPacket(from: number, to: number, tone: 'accent' | 'ok'): void {
    if (from === to) return;
    const core = new THREE.Mesh(packetGeo, tone === 'ok' ? mats.ok : mats.accent);
    const halo = new THREE.Mesh(haloGeo, tone === 'ok' ? mats.okHalo : mats.accentHalo);
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

  /** What labels and the card show: offline overrides whatever the transcript last said. */
  const shown = (b: Bot): string => (b.agent.status === 'offline' ? 'offline' : b.activity);

  const nameOf = (cid: unknown): string => {
    const b = typeof cid === 'number' ? byCid.get(cid) : undefined;
    return b ? b.agent.name : String(cid ?? '?');
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

  function updateLabels(): void {
    for (const b of bots) {
      const s = toScreen(b.robot.labelAnchor(new THREE.Vector3()));
      if (!s.visible) {
        b.label.style.display = 'none';
        continue;
      }
      b.label.style.display = 'block';
      b.label.style.transform = `translate(${s.x.toFixed(1)}px, ${s.y.toFixed(1)}px) translate(-50%, -100%) translateY(-6px)`;
      if (b.labelName.textContent !== b.agent.name) b.labelName.textContent = b.agent.name;
      const line = shown(b);
      if (b.labelLine.textContent !== line) b.labelLine.textContent = line;
    }
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
    const sig = [a.name, line].join('|');
    if (sig === cardSig) return;
    cardSig = sig;
    card.replaceChildren(h('header.floor-card-head', null, h('strong', null, a.name)), h('div.floor-card-activity', null, line));
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
  const frame = (now: number): void => {
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    updateBots(dt, now);
    updatePackets(now);
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
    disposeTree(ground);
    disposeTree(grid);
    padGeo.dispose();
    packetGeo.dispose();
    haloGeo.dispose();
    for (const m of Object.values(mats)) m.dispose();
    renderer.dispose();
    root.classList.remove('floor-view');
    document.body.classList.remove('floor-mode');
    root.replaceChildren();
  };
}
