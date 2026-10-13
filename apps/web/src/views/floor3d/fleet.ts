// The Machines scene: every machine of GET /v1/fleet as its hardware model on a ring around the hub,
// each cabled to the hub's port, with status rings, activity LEDs and light packets on the cables.
// The pure parts (ring layout, cable curve, packet rate, facing) are exported for tests; buildFleet
// builds the three.js group and the query API the Machines view uses (see scenes-contract.md, section 2).

import * as THREE from 'three';
import type { FleetMachine, FleetResponse, FleetStatus } from '../../api';
import { buildModel, modelsFor, placeAtom, type Built, type Model, type Palette, type Params, type Vec3 } from './hardware';
import { MODELS, PALETTE } from './hardware-index';
import { toonMat } from './toon';

/** The space kept between two machines' footprints on the ring. */
const GAP = 4;
const RING_MIN = 14;
/** The cable's sag below the straight line between its ends, and the lowest it may hang. */
export const SAG = 1.5;
export const CABLE_SEGMENTS = 24;
const CABLE_MIN_Y = 0.1;
const CABLE_RADIUS = 0.12;
/** How far a hovered machine lifts, in units. */
export const HOVER_LIFT = 0.3;
/** Light packets per second per direction: the base, one more per working agent on either end, the cap. */
const PACKET_BASE = 0.5;
const PACKET_PER_AGENT = 1;
const PACKET_MAX = 4;
/** Fraction of a cable a packet crosses per second, and packets in flight per direction. */
const PACKET_SPEED = 0.8;
const PACKETS_PER_DIR = 6;
const TAU = Math.PI * 2;
const UP = new THREE.Vector3(0, 1, 0);

const LED: Record<FleetStatus, string> = { online: 'led-ok', draining: 'led-warn', offline: 'led-off', revoked: 'led-crit' };

// ---- Pure parts ----------------------------------------------------------------------------

export interface RingSlot {
  id: string;
  /** Its angle around the hub (radians, from +x towards +z). */
  angle: number;
  x: number;
  z: number;
}
export interface RingLayout {
  /** The ring's radius, from the hub's centre. */
  ring: number;
  slots: RingSlot[];
}

/**
 * The nodes on a ring around a hub. A node takes an arc proportional to 2r + GAP (r = its plan radius), so
 * the arcs cover the whole ring; the radius is at least RING_MIN, the arcs' length over 2π, the chord each
 * pair of neighbours needs, and the hub's radius plus the largest node plus GAP. Nodes are sorted by id.
 */
export function ringLayout(nodes: { id: string; radius: number }[], hubRadius = 0): RingLayout {
  const list = [...nodes].sort((p, q) => p.id.localeCompare(q.id, undefined, { numeric: true }));
  if (list.length === 0) return { ring: RING_MIN, slots: [] };
  const span = list.map((n) => 2 * n.radius + GAP);
  const total = span.reduce((s, v) => s + v, 0);
  const biggest = Math.max(...list.map((n) => n.radius));
  let ring = Math.max(RING_MIN, total / TAU, hubRadius + biggest + GAP);
  // Neighbours sit (span_i + span_j) / 2 apart; a chord is shorter than its arc, so the radius grows to keep the gap.
  list.forEach((_, i) => {
    const j = (i + 1) % list.length;
    if (j === i) return;
    const pair = span[i]! + span[j]!;
    ring = Math.max(ring, pair / (4 * Math.sin((Math.PI * pair) / (2 * total))));
  });
  const slots: RingSlot[] = [];
  let cum = 0;
  list.forEach((n, i) => {
    const angle = (TAU * (cum + span[i]! / 2)) / total;
    cum += span[i]!;
    slots.push({ id: n.id, angle, x: ring * Math.cos(angle), z: ring * Math.sin(angle) });
  });
  return { ring, slots };
}

/** A point rotated about the y axis, as three.js rotation.y turns it. */
export function rotateY(v: Vec3, angle: number): Vec3 {
  const c = Math.cos(angle);
  const s = Math.sin(angle);
  return [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c];
}

/** The y rotation that turns a model's port (its local xz offset from the centre) to face the hub at the origin. */
export function facingAngle(x: number, z: number, port: Vec3): number {
  const toHub = Math.atan2(-x, -z);
  const local = Math.hypot(port[0], port[2]) < 1e-6 ? 0 : Math.atan2(port[0], port[2]);
  return toHub - local;
}

/**
 * A cable's centre line: a quadratic Bézier from a to b whose middle hangs `sag` below the straight line,
 * never below minY. Returns segments + 1 points, the first at a and the last at b.
 */
export function cableCurvePoints(a: Vec3, b: Vec3, segments = CABLE_SEGMENTS, sag = SAG, minY = CABLE_MIN_Y): Vec3[] {
  const mid: Vec3 = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2, (a[2] + b[2]) / 2];
  const c: Vec3 = [mid[0], mid[1] - 2 * sag, mid[2]];
  const pts: Vec3[] = [];
  for (let i = 0; i <= segments; i++) {
    const t = i / segments;
    const u = 1 - t;
    const w0 = u * u;
    const w1 = 2 * u * t;
    const w2 = t * t;
    pts.push([w0 * a[0] + w1 * c[0] + w2 * b[0], Math.max(minY, w0 * a[1] + w1 * c[1] + w2 * b[1]), w0 * a[2] + w1 * c[2] + w2 * b[2]]);
  }
  return pts;
}

/** Light packets per second in each direction of a cable: 0 while offline, else the base plus one per working agent at either end, capped. */
export function packetRate(workingA: number, workingB: number, online: boolean): number {
  if (!online) return 0;
  return Math.min(PACKET_MAX, PACKET_BASE + PACKET_PER_AGENT * (workingA + workingB));
}

// ---- The scene -----------------------------------------------------------------------------

export interface FleetMachineScene {
  /** The machine's model, lifted on hover. */
  root: THREE.Group;
  built: Built;
  /** The middle of the machine's body, before any hover lift. */
  center: Vec3;
  /** Its bounding sphere's radius. */
  radius: number;
  /** The tag anchor (anchors.label, placed). */
  label: Vec3;
  /** The port anchor (anchors.port, placed), where its cable starts. */
  port: Vec3;
}

export interface FleetTag {
  id: string;
  pos: Vec3;
  name: string;
  status: FleetStatus;
}

export interface FleetScene {
  group: THREE.Group;
  machines: Map<string, FleetMachineScene>;
  /** Per frame: applies the fleet's state; rebuilds the scene when the machines change. */
  update: (dt: number, fleet: FleetResponse) => void;
  /** The machine whose bounding sphere the ray hits first, or null. */
  pick: (ray: THREE.Ray) => string | null;
  /** Lifts the hovered machine (null lowers every machine). */
  hover: (id: string | null) => void;
  tags: () => FleetTag[];
  frame: () => { center: Vec3; radius: number };
  /** Drives a machine's ENTER opening, 0..1. */
  setOpen: (id: string, t: number) => void;
}

interface Live {
  m: FleetMachine;
  scene: FleetMachineScene;
  ringMat: THREE.MeshBasicMaterial;
  atom: { atom: THREE.Object3D; animate: (t: number) => void } | null;
  lift: number;
  open: number;
  phase: number;
}
interface Packet {
  mesh: THREE.Mesh;
  /** 0: from the node to the hub; 1: from the hub to the node. */
  dir: 0 | 1;
  /** 0..1 along the direction of travel. */
  s: number;
  live: boolean;
}
interface Cable {
  node: string;
  segs: THREE.Mesh[];
  packets: Packet[];
  timers: [number, number];
}
interface Mats {
  cable: THREE.Material;
  dim: THREE.Material;
  busy: THREE.Material;
  idle: THREE.Material;
}

const byId = (a: FleetMachine, b: FleetMachine): number => a.id.localeCompare(b.id, undefined, { numeric: true });
const clamp01 = (v: number): number => Math.min(1, Math.max(0, v));
const colour = (palette: Palette, name: string): string => palette[name]?.color ?? '#888888';

/** The fleet's shape: the machines the scene is built from. A change to it rebuilds the scene; a change of state does not. */
function fleetKey(f: FleetResponse): string {
  return f.machines
    .map((m) => [m.id, m.role, m.kind, m.vendor ?? '', m.model ?? '', m.gpus.length, m.apple ? [m.apple.p_cores, m.apple.e_cores, m.apple.gpu_cores, m.apple.unified_memory_gib].join(',') : ''].join('|'))
    .sort()
    .join(';');
}

const modelOf = (m: FleetMachine): Model => MODELS[modelsFor(m).exterior] ?? MODELS['exterior/workstation']!;
const portOf = (model: Model): Vec3 => model.anchors?.port ?? [0, model.size[1] / 2, 0];

function paramsOf(m: FleetMachine): Params {
  const p: Params = { gpus: m.gpus.length, studio: m.model?.includes('Studio') ? 1 : 0 };
  if (m.apple) {
    p.p_cores = m.apple.p_cores;
    p.e_cores = m.apple.e_cores;
    p.gpu_cores = m.apple.gpu_cores;
    p.memory_gib = m.apple.unified_memory_gib;
  }
  return p;
}

/** A point of a machine at (x, z) turned by rot, in the scene. */
const placed = (p: Vec3, x: number, z: number, rot: number): Vec3 => {
  const r = rotateY(p, rot);
  return [x + r[0], r[1], z + r[2]];
};
const lifted = (p: Vec3, lift: number): Vec3 => [p[0], p[1] + lift, p[2]];
const loadOf = (m: FleetMachine): number => (m.stats ? clamp01(m.stats.cpu_pct / 100) : Math.min(1, m.working / 4));

/** Sets a cylinder of unit height from a to b. */
function placeSegment(mesh: THREE.Mesh, a: Vec3, b: Vec3): void {
  const d = new THREE.Vector3(b[0] - a[0], b[1] - a[1], b[2] - a[2]);
  const len = d.length();
  mesh.visible = len > 1e-6;
  if (!mesh.visible) return;
  mesh.position.set((a[0] + b[0]) / 2, (a[1] + b[1]) / 2, (a[2] + b[2]) / 2);
  mesh.quaternion.setFromUnitVectors(UP, d.multiplyScalar(1 / len));
  mesh.scale.set(1, len, 1);
}

/** The point at fraction t along a polyline. */
function pointOn(pts: Vec3[], t: number): Vec3 {
  const f = t * (pts.length - 1);
  const i = Math.min(pts.length - 2, Math.floor(f));
  const u = f - i;
  const a = pts[i]!;
  const b = pts[i + 1]!;
  return [a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u, a[2] + (b[2] - a[2]) * u];
}

/** Frees a built tree's geometry and materials (the shared outline material stays). */
function disposeTree(root: THREE.Object3D): void {
  root.traverse((o) => {
    const m = o as THREE.Mesh;
    if (!m.isMesh || m.userData.isOutline) return;
    m.geometry.dispose();
    for (const mat of Array.isArray(m.material) ? m.material : [m.material]) {
      (mat as THREE.MeshBasicMaterial).map?.dispose();
      mat.dispose();
    }
  });
}

/** Moves the packets of one cable along its curve (from the node's end when dir 0). */
function stepPackets(c: Cable, pts: Vec3[], dt: number, online: boolean, rate: number, busy: boolean, mats: Mats): void {
  if (!online || rate <= 0) {
    for (const p of c.packets) {
      p.live = false;
      p.mesh.visible = false;
    }
    c.timers = [0, 0];
    return;
  }
  for (const dir of [0, 1] as const) {
    c.timers[dir] += dt;
    const period = 1 / rate;
    while (c.timers[dir] >= period) {
      c.timers[dir] -= period;
      const p = c.packets.find((q) => !q.live && q.dir === dir);
      if (!p) break;
      p.live = true;
      p.s = 0;
      p.mesh.material = busy ? mats.busy : mats.idle;
    }
  }
  for (const p of c.packets) {
    if (!p.live) continue;
    p.s += PACKET_SPEED * dt;
    if (p.s >= 1) {
      p.live = false;
      p.mesh.visible = false;
      continue;
    }
    const [x, y, z] = pointOn(pts, p.dir === 0 ? p.s : 1 - p.s);
    p.mesh.visible = true;
    p.mesh.position.set(x, y, z);
  }
}

export function buildFleet(fleet: FleetResponse, palette: Palette = PALETTE): FleetScene {
  const group = new THREE.Group();
  const machines = new Map<string, FleetMachineScene>();
  const live = new Map<string, Live>();
  const ledHex = Object.fromEntries((Object.keys(LED) as FleetStatus[]).map((s) => [s, colour(palette, LED[s])])) as Record<FleetStatus, string>;
  let cables: Cable[] = [];
  /** The cable and packet materials of the current build (made in rebuild). */
  let mats!: Mats;
  let hubId: string | null = null;
  let key = '';
  let built = false;
  let frameInfo: { center: Vec3; radius: number } = { center: [0, 0, 0], radius: 10 };
  let hoverId: string | null = null;
  let time = 0;

  function addMachine(m: FleetMachine, x: number, z: number, rot: number): void {
    const model = modelOf(m);
    const b = buildModel(model, paramsOf(m), { palette });
    const root = new THREE.Group();
    root.add(b.group);
    root.position.set(x, 0, z);
    root.rotation.y = rot;
    group.add(root);
    const atom = placeAtom(model, b.group);
    const [w, h, d] = model.size;
    const foot = Math.hypot(w, d) / 2;
    const scene: FleetMachineScene = {
      root,
      built: b,
      center: [x, h / 2, z],
      radius: Math.hypot(w, h, d) / 2,
      label: placed(model.anchors?.label ?? [0, h, 0], x, z, rot),
      port: placed(portOf(model), x, z, rot),
    };
    const ringMat = new THREE.MeshBasicMaterial({ color: ledHex[m.status], transparent: true, opacity: 0.9, depthWrite: false, toneMapped: false });
    const ring = new THREE.Mesh(new THREE.RingGeometry(foot + 0.8, foot + 1.2, 64), ringMat);
    ring.rotation.x = -Math.PI / 2;
    ring.position.set(x, 0.04, z);
    group.add(ring);
    machines.set(m.id, scene);
    live.set(m.id, { m, scene, ringMat, atom, lift: 0, open: 0, phase: [...m.id].reduce((s, c) => s + c.charCodeAt(0), 0) * 0.7 });
  }

  function rebuild(f: FleetResponse): void {
    for (const child of [...group.children]) disposeTree(child);
    group.clear();
    machines.clear();
    live.clear();
    cables = [];
    const ordered = [...f.machines].sort(byId);
    const hub = ordered.find((m) => m.role === 'hub') ?? ordered[0];
    hubId = hub?.id ?? null;
    if (!hub) {
      frameInfo = { center: [0, 0, 0], radius: 10 };
      return;
    }
    const nodes = ordered.filter((m) => m.id !== hub.id);
    const footOf = (m: FleetMachine): number => {
      const [w, , d] = modelOf(m).size;
      return Math.hypot(w, d) / 2;
    };
    const layout = ringLayout(
      nodes.map((m) => ({ id: m.id, radius: footOf(m) })),
      footOf(hub),
    );
    const slot = new Map(layout.slots.map((s) => [s.id, s]));
    mats = {
      cable: toonMat({ color: colour(palette, 'cable') }),
      dim: toonMat({ color: colour(palette, 'cable'), transparent: true, opacity: 0.3, depthWrite: false }),
      busy: new THREE.MeshBasicMaterial({ color: colour(palette, 'copper-live'), transparent: true, blending: THREE.AdditiveBlending, depthWrite: false, toneMapped: false }),
      idle: new THREE.MeshBasicMaterial({ color: colour(palette, 'led-info'), transparent: true, blending: THREE.AdditiveBlending, depthWrite: false, toneMapped: false }),
    };
    addMachine(hub, 0, 0, 0);
    for (const n of nodes) {
      const s = slot.get(n.id)!;
      addMachine(n, s.x, s.z, facingAngle(s.x, s.z, portOf(modelOf(n))));
    }
    const cylinder = new THREE.CylinderGeometry(CABLE_RADIUS, CABLE_RADIUS, 1, 6);
    const sphere = new THREE.SphereGeometry(0.22, 8, 6);
    for (const n of nodes) {
      const segs: THREE.Mesh[] = [];
      for (let k = 0; k < CABLE_SEGMENTS; k++) {
        const seg = new THREE.Mesh(cylinder, mats.cable);
        group.add(seg);
        segs.push(seg);
      }
      const packets: Packet[] = [];
      for (const dir of [0, 1] as const) {
        for (let k = 0; k < PACKETS_PER_DIR; k++) {
          const mesh = new THREE.Mesh(sphere, mats.idle);
          mesh.visible = false;
          group.add(mesh);
          packets.push({ mesh, dir, s: 0, live: false });
        }
      }
      cables.push({ node: n.id, segs, packets, timers: [0, 0] });
    }
    let maxR = 0;
    let maxH = 0;
    for (const s of live.values()) {
      maxR = Math.max(maxR, s.scene.radius);
      maxH = Math.max(maxH, s.scene.center[1] * 2);
    }
    frameInfo = { center: [0, maxH / 2, 0], radius: layout.ring + maxR };
  }

  function update(dt: number, f: FleetResponse): void {
    const k = fleetKey(f);
    if (!built || k !== key) {
      built = true;
      key = k;
      rebuild(f);
    }
    time += dt;
    const byMachine = new Map(f.machines.map((m) => [m.id, m]));
    const linkOnline = new Map<string, boolean>();
    for (const l of f.links) {
      if (l.a === hubId) linkOnline.set(l.b, l.online);
      else if (l.b === hubId) linkOnline.set(l.a, l.online);
    }
    for (const [id, L] of live) {
      const m = byMachine.get(id);
      if (!m) continue;
      L.m = m;
      L.lift += ((hoverId === id ? HOVER_LIFT : 0) - L.lift) * Math.min(1, dt * 10);
      L.scene.root.position.y = L.lift;
      L.scene.built.update(dt, { status: m.status, working: m.working, load: loadOf(m), open: L.open });
      L.ringMat.color.set(ledHex[m.status]);
      L.ringMat.opacity = m.status === 'online' ? 0.6 + 0.3 * (0.5 + 0.5 * Math.sin(time * 2 + L.phase)) : 0.85;
      if (L.atom) {
        L.atom.atom.visible = m.status === 'online';
        L.atom.animate(time);
      }
    }
    const hub = hubId !== null ? live.get(hubId) : undefined;
    for (const c of cables) {
      const node = live.get(c.node);
      if (!node || !hub) continue;
      const online = linkOnline.get(c.node) ?? node.m.status === 'online';
      const pts = cableCurvePoints(lifted(node.scene.port, node.lift), lifted(hub.scene.port, hub.lift));
      c.segs.forEach((seg, i) => {
        placeSegment(seg, pts[i]!, pts[i + 1]!);
        seg.material = online ? mats.cable : mats.dim;
      });
      const busy = hub.m.working + node.m.working > 0;
      stepPackets(c, pts, dt, online, packetRate(hub.m.working, node.m.working, online), busy, mats);
    }
  }

  rebuild(fleet);
  built = true;
  key = fleetKey(fleet);

  return {
    group,
    machines,
    update,
    pick(ray: THREE.Ray): string | null {
      const hit = new THREE.Vector3();
      const sphere = new THREE.Sphere();
      let best: string | null = null;
      let bestD = Infinity;
      for (const [id, mc] of machines) {
        const L = live.get(id);
        sphere.center.set(mc.center[0], mc.center[1] + (L?.lift ?? 0), mc.center[2]);
        sphere.radius = mc.radius;
        if (!ray.intersectSphere(sphere, hit)) continue;
        const d = ray.origin.distanceTo(hit);
        if (d < bestD) {
          bestD = d;
          best = id;
        }
      }
      return best;
    },
    hover(id: string | null): void {
      hoverId = id;
    },
    tags(): FleetTag[] {
      return [...live].map(([id, L]) => ({
        id,
        pos: [L.scene.label[0], L.scene.label[1] + L.lift, L.scene.label[2]],
        name: L.m.name,
        status: L.m.status,
      }));
    },
    frame: () => frameInfo,
    setOpen(id: string, t: number): void {
      const L = live.get(id);
      if (!L) return;
      L.open = clamp01(t);
      L.scene.built.update(0, { status: L.m.status, working: L.m.working, load: loadOf(L.m), open: L.open });
    },
  };
}
