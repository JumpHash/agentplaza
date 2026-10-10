/* The 3D view: the same register, raised. The machine is the ground — a grid
   ruled to its CPUs and memory, labeled on its edges; runs are platforms
   arranged by id, each named on a plate (run id, state, CID range); VMs are
   blocks whose state is their height and edge (running glows with its serial
   chatter, exited settles, killed falls flat and crit). The host every guest
   session addresses stands off the plot's back corner (cid 2), and each
   session draws an arc from its VM's block to the host — from the hello line
   to the bye, and one beat after so a just-written session stays visible —
   with a packet travelling it while the session is in flight.
   Drag orbits, scroll zooms, a click selects the VM and opens its entry;
   o frames the whole plot, r the selected run. Every object is a machine,
   a run, a VM, the host, or a session — no decoration.
   Rendering: three.js WebGLRenderer on WebGL2 (r163+ has no WebGL1 path;
   three's WebGPURenderer is still not the stable default on r186 — WebGL2 is
   the boring, correct choice). three is dynamically imported on first show so
   the register's bundle stays lean; the render loop pauses in hidden tabs. */

import type {
  Group,
  Line,
  LineBasicMaterial,
  LineSegments,
  Mesh,
  MeshBasicMaterial,
  MeshLambertMaterial,
  Object3D,
  PerspectiveCamera,
  QuadraticBezierCurve3,
  Raycaster,
  Scene,
  Sprite,
  WebGLRenderer,
} from 'three';
import type { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { byId } from '../dom';
import { fmtNum } from '../format';
import { vmKey } from '../models/activity';
import type { Run, Vm } from '../models/run';
import { isOpen, roleOf, seqOf } from '../models/run';
import { sessionOf } from '../models/session';
import { store } from '../store';
import type { Framing } from './district';
import { HOST_CID, arcAlive, arcLift, focusFraming, overviewFraming, runPlateLines } from './district';
import type { Palette } from './plan';
import { palette } from './plan';

/* The runtime module arrives by dynamic import; this named alias is its type. */
type ThreeModule = typeof import('three');

interface VmObject {
  mesh: Mesh;
  edges: LineSegments;
  label: Sprite;
  mat: MeshLambertMaterial;
  edgeMat: LineBasicMaterial;
  runId: string;
  index: number;
  state: Vm['state'];
  /** The block label last textured (vm id, cid and role) — retextured only
      when the words change, which is how a role that lands a tick late (and
      a CID that lands with it) reaches a block already on the plot. */
  labelKey: string;
}

interface RunObject {
  group: Group;
  platform: Mesh;
  pedges: LineSegments;
  label: Sprite;
  /** The plate text last textured — retexturing is a canvas draw + upload,
      so it happens only when the words change. */
  plateKey: string;
  vms: Map<number, VmObject>;
  /* the platform's block grid; reshaped when a run's VM count changes */
  cols: number;
  rows: number;
}

/** One live session arc: the wire from a sessioning VM's block to the host,
    with the packet that travels it while the session is in flight. */
interface ArcObject {
  line: Line;
  lineMat: LineBasicMaterial;
  packet: Mesh;
  packetMat: MeshBasicMaterial;
  curve: QuadraticBezierCurve3 | null;
  from: [number, number, number];
  to: [number, number, number];
  /** The packet's phase along the curve, derived from the key (stable). */
  offset: number;
}

/** An in-flight camera move between two framings (the presets); a user drag
    cancels it, and reduced motion never starts one — the jump is instant. */
interface Flight {
  fromPos: [number, number, number];
  fromTgt: [number, number, number];
  toPos: [number, number, number];
  toTgt: [number, number, number];
  t0: number;
  dur: number;
}

const REDUCED = typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;
const CELL_PITCH = 2.6; // ground units between VM blocks on a platform
const FLIGHT_MS = 700; // a camera preset's travel time

let module3: ThreeModule | null = null;
let renderer: WebGLRenderer | null = null;
let scene: Scene | null = null;
let camera: PerspectiveCamera | null = null;
let controls: OrbitControls | null = null;
let raycaster: Raycaster | null = null;
let failed: string | null = null;
let starting: Promise<void> | null = null;

const runObjects = new Map<string, RunObject>();
let groundGroup: Group | null = null;
let groundKey = '';
let autoOrbitTimer: number | undefined;
let pulse = 0;

/* the district's session state: the host marker's top (set by rebuildGround)
   and the live arcs, keyed "runId/vmId" like the serial cache */
const arcs = new Map<string, ArcObject>();
let hostAnchor: [number, number, number] | null = null;
let liveSessions = 0;
let flight: Flight | null = null;

function three(): ThreeModule {
  if (!module3) throw new Error('the 3D scene has not booted');
  return module3;
}

/* ------------------------------------------------------------ boot */
async function ensureScene(): Promise<void> {
  if (renderer || failed) return;
  starting ??= boot()
    .catch((err: unknown) => {
      failed = err instanceof Error ? err.message : String(err);
    })
    .finally(() => { starting = null; });
  await starting;
}

async function boot(): Promise<void> {
  const [THREE, controlsMod] = await Promise.all([
    import('three'),
    import('three/examples/jsm/controls/OrbitControls.js'),
  ]);
  module3 = THREE;
  const mount = byId('v3-mount');

  let r: WebGLRenderer;
  try {
    r = new THREE.WebGLRenderer({ antialias: true, powerPreference: 'low-power' });
  } catch (err) {
    throw new Error(`WebGL2 unavailable: ${err instanceof Error ? err.message : String(err)}`);
  }
  renderer = r;
  r.setPixelRatio(Math.min(2, window.devicePixelRatio || 1));
  r.domElement.tabIndex = 0;
  r.domElement.setAttribute('role', 'application');
  r.domElement.setAttribute('aria-label',
    '3D fleet view. Drag to orbit, scroll to zoom. The map view carries the same fleet with a keyboard path.');
  mount.appendChild(r.domElement);

  scene = new THREE.Scene();
  camera = new THREE.PerspectiveCamera(50, 1, 0.1, 2000);
  raycaster = new THREE.Raycaster();

  scene.add(new THREE.AmbientLight(0xffffff, 1.1));
  const sun = new THREE.DirectionalLight(0xffffff, 1.6);
  sun.position.set(30, 50, 20);
  scene.add(sun);

  const ctl = new controlsMod.OrbitControls(camera, r.domElement);
  ctl.enableDamping = true;
  ctl.dampingFactor = 0.08;
  ctl.autoRotate = !REDUCED;
  ctl.autoRotateSpeed = 0.55;
  ctl.maxPolarAngle = Math.PI * 0.49;
  ctl.addEventListener('start', () => {
    flight = null; // the owner's hand cancels any preset's travel
    ctl.autoRotate = false;
    clearTimeout(autoOrbitTimer);
    autoOrbitTimer = setTimeout(() => {
      if (!REDUCED) ctl.autoRotate = true;
    }, 5000);
  });
  controls = ctl;

  r.domElement.addEventListener('click', onClick);
  r.domElement.addEventListener('mousemove', onHover);
  /* the camera keys while the canvas itself holds focus (the sheet-wide
     key handler leaves canvases to their own devices) */
  r.domElement.addEventListener('keydown', (ev) => {
    if (ev.key === 'o' || ev.key === 'r') {
      ev.preventDefault();
      ev.stopPropagation();
      cameraPreset(ev.key === 'o' ? 'overview' : 'focus');
    }
  });

  new ResizeObserver(() => resize()).observe(mount);
  resize();
  sync3d();
  loop();
}

function resize(): void {
  const mount = byId('v3-mount');
  const w = mount.clientWidth;
  const h = mount.clientHeight;
  if (!renderer || !camera || w === 0 || h === 0) return;
  renderer.setSize(w, h);
  camera.aspect = w / h;
  camera.updateProjectionMatrix();
}

/* ------------------------------------------------------------ labels
   Labels are canvas textures — text is drawn, never inserted as markup. */
function makeLabel(text: string, color: string): Sprite {
  const THREE = three();
  const pad = 6;
  const c = document.createElement('canvas');
  const g = c.getContext('2d');
  if (!g) throw new Error('no 2d context for labels');
  const font = '11px "Martian Mono", ui-monospace, monospace';
  g.font = font;
  const metrics = g.measureText(text);
  c.width = Math.ceil(metrics.width) + pad * 2;
  c.height = 22;
  g.font = font;
  g.textBaseline = 'middle';
  g.fillStyle = color;
  g.fillText(text, pad, 11);
  const tex = new THREE.CanvasTexture(c);
  tex.colorSpace = THREE.SRGBColorSpace;
  const mat = new THREE.SpriteMaterial({ map: tex, transparent: true, depthWrite: false });
  const sprite = new THREE.Sprite(mat);
  const scale = 0.042; // world units per label pixel — labels read as marginalia, not billboards
  sprite.scale.set(c.width * scale, c.height * scale, 1);
  return sprite;
}

function retextureLabel(sprite: Sprite, text: string, color: string): void {
  replaceSprite(sprite, makeLabel(text, color));
}

/** Swap a sprite's texture for a freshly drawn one, disposing the old. */
function replaceSprite(sprite: Sprite, fresh: Sprite): void {
  const oldMat = sprite.material;
  sprite.material = fresh.material;
  sprite.scale.copy(fresh.scale);
  oldMat.map?.dispose();
  oldMat.dispose();
}

/** A run platform's nameplate: two lines (id · state over cid range · shape),
    drawn larger than the VM labels so the district reads from the overview.
    The text is real register data, drawn on a canvas texture, never markup. */
function makePlate(title: string, sub: string, titleColor: string, subColor: string): Sprite {
  const THREE = three();
  const padX = 8;
  const c = document.createElement('canvas');
  const g = c.getContext('2d');
  if (!g) throw new Error('no 2d context for plates');
  const titleFont = '600 13px "Martian Mono", ui-monospace, monospace';
  const subFont = '10.5px "Martian Mono", ui-monospace, monospace';
  g.font = titleFont;
  const wTitle = g.measureText(title).width;
  g.font = subFont;
  const wSub = g.measureText(sub).width;
  c.width = Math.ceil(Math.max(wTitle, wSub)) + padX * 2;
  c.height = 40;
  g.textBaseline = 'middle';
  g.font = titleFont;
  g.fillStyle = titleColor;
  g.fillText(title, padX, 12);
  g.font = subFont;
  g.fillStyle = subColor;
  g.fillText(sub, padX, 29);
  const tex = new THREE.CanvasTexture(c);
  tex.colorSpace = THREE.SRGBColorSpace;
  const mat = new THREE.SpriteMaterial({ map: tex, transparent: true, depthWrite: false });
  const sprite = new THREE.Sprite(mat);
  const scale = 0.05; // world units per plate pixel — a nameplate, not a billboard
  sprite.scale.set(c.width * scale, c.height * scale, 1);
  return sprite;
}

/* ------------------------------------------------------------ ground */
function rebuildGround(p: Palette): void {
  const THREE = three();
  if (!scene) return;
  if (groundGroup) {
    scene.remove(groundGroup);
    disposeDeep(groundGroup);
    groundGroup = null;
  }
  const m = store.machine;
  const group = new THREE.Group();
  if (m) {
    /* the plot: 1 unit per CPU across, 1 unit per 4 GiB deep, capped for view */
    const plotW = Math.min(48, Math.max(8, m.cpus));
    const plotD = Math.min(48, Math.max(8, Math.round(m.memory_mib / 1024 / 4)));
    const stepW = Math.max(1, Math.round(m.cpus / plotW));
    const stepD = Math.max(1, Math.round(m.memory_mib / 1024 / 4 / plotD));

    const plane = new THREE.Mesh(
      new THREE.PlaneGeometry(plotW, plotD),
      new THREE.MeshLambertMaterial({ color: p.sheet }),
    );
    plane.rotation.x = -Math.PI / 2;
    group.add(plane);

    /* the surveyor's grid: every line a real bound (CPU step / memory step) */
    const pts: number[] = [];
    for (let x = 0; x <= plotW; x += 1) pts.push(x - plotW / 2, 0.01, -plotD / 2, x - plotW / 2, 0.01, plotD / 2);
    for (let z = 0; z <= plotD; z += 1) pts.push(-plotW / 2, 0.01, z - plotD / 2, plotW / 2, 0.01, z - plotD / 2);
    const gridGeo = new THREE.BufferGeometry();
    gridGeo.setAttribute('position', new THREE.Float32BufferAttribute(pts, 3));
    group.add(new THREE.LineSegments(gridGeo, new THREE.LineBasicMaterial({ color: p.rule })));

    /* the plot's double edge: ink outside, rule inside */
    const edge = (inset: number, color: string): void => {
      const w = plotW / 2 - inset;
      const d = plotD / 2 - inset;
      const geo = new THREE.BufferGeometry();
      geo.setAttribute('position', new THREE.Float32BufferAttribute([
        -w, 0.02, -d, w, 0.02, -d, w, 0.02, -d, w, 0.02, d,
        w, 0.02, d, -w, 0.02, d, -w, 0.02, d, -w, 0.02, -d,
      ], 3));
      group.add(new THREE.LineSegments(geo, new THREE.LineBasicMaterial({ color })));
    };
    edge(0, p.ink);
    edge(0.35, p.ink3);

    const name = makeLabel(`${m.name} · ${m.class} · ${m.arch} ${m.backend}`, p.ink);
    name.position.set(-plotW / 2 + 2 + name.scale.x / 2, 1.4, -plotD / 2 - 1.6);
    group.add(name);
    const cpus = makeLabel(`${fmtNum(m.cpus)} cpus (1 line = ${stepW})`, p.ink3);
    cpus.position.set(0, 0.7, plotD / 2 + 1.8);
    group.add(cpus);
    const mem = makeLabel(`${fmtNum(m.memory_mib)} MiB (1 line = ${stepD * 4} GiB)`, p.ink3);
    mem.position.set(plotW / 2 + 1.2 + mem.scale.x / 2, 0.7, 0);
    group.add(mem);

    /* the host: the one endpoint every guest session addresses (cid 2, the
       vsock host) — a post off the plot's back corner; the session arcs
       land on its top */
    const hostH = 2.8;
    const host = new THREE.Mesh(
      new THREE.BoxGeometry(0.9, hostH, 0.9),
      new THREE.MeshLambertMaterial({ color: p.sheet }),
    );
    const hx = plotW / 2 + 2.8;
    const hz = -plotD / 2 - 2.8;
    host.position.set(hx, hostH / 2, hz);
    group.add(host);
    const hostEdges = new THREE.LineSegments(
      new THREE.EdgesGeometry(host.geometry),
      new THREE.LineBasicMaterial({ color: p.ink }),
    );
    hostEdges.position.copy(host.position);
    group.add(hostEdges);
    const hostName = makeLabel(`host · cid ${HOST_CID}`, p.ink2);
    hostName.position.set(hx, hostH + 0.9, hz);
    group.add(hostName);
    hostAnchor = [hx, hostH, hz];
  } else {
    hostAnchor = null;
  }
  scene.add(group);
  groundGroup = group;

  if (camera && controls) {
    const span = Math.max(16, Math.min(48, m ? Math.max(m.cpus, m.memory_mib / 4096) : 16));
    camera.position.set(span * 0.9, span * 0.85, span * 0.9);
    controls.target.set(0, 0, 0);
    controls.update();
  }
}

function disposeDeep(obj: Object3D): void {
  const THREE = three();
  obj.traverse((o) => {
    if (o instanceof THREE.Mesh || o instanceof THREE.LineSegments) {
      o.geometry.dispose();
      const mat = o.material;
      for (const m of Array.isArray(mat) ? mat : [mat]) m.dispose();
    }
    if (o instanceof THREE.Sprite) {
      o.material.map?.dispose();
      o.material.dispose();
    }
  });
}

/* ------------------------------------------------------------ sync */
/** Reconcile the scene with the store: create, update, and remove only what
    the register says. Runs by id; VMs by run/index. */
export function sync3d(): void {
  if (store.route.name !== 'view3d') return;
  if (!renderer || !scene) {
    render3dStatus();
    void ensureScene().then(() => {
      render3dStatus();
      if (failed) showFailure(failed);
    });
    return;
  }
  const THREE = three();
  const p = palette();
  scene.background = new THREE.Color(p.paper);

  const machineKey = store.machine ? `${store.machine.cpus}:${store.machine.memory_mib}` : 'none';
  if (machineKey !== groundKey || !groundGroup) {
    groundKey = machineKey;
    rebuildGround(p);
  }

  const seen = new Set<string>();
  const runs = store.runs.slice().sort((a, b) => seqOf(a.id) - seqOf(b.id));
  const rows = Math.max(1, runs.length);
  runs.forEach((run, rowIdx) => {
    seen.add(run.id);
    const ro = runObjects.get(run.id) ?? addRun(run, p);
    reshapePlatform(ro, run.vms.length);
    ro.group.position.set(0, 0, (rowIdx - (rows - 1) / 2) * 9);
    /* the nameplate: retexture only when the words change (a canvas draw +
       texture upload is not a per-render expense) */
    const plate = runPlateLines(run);
    const plateKey = `${plate.title}|${plate.sub}|${run.state}`;
    if (plateKey !== ro.plateKey) {
      ro.plateKey = plateKey;
      replaceSprite(ro.label, makePlate(plate.title, plate.sub, run.state === 'running' ? p.ink : p.ink3, p.ink3));
      const { len } = platformSize(ro.cols, ro.rows);
      ro.label.position.set(-len / 2 - 1.2 - ro.label.scale.x / 2, 1.3, 0);
    }
    const platMat = ro.platform.material;
    if (platMat instanceof THREE.MeshLambertMaterial) {
      platMat.color.set(run.state === 'starting' || run.state === 'running' ? p.paper : p.rule);
    }
    run.vms.forEach((vm, i) => syncVm(ro, run, vm, i, p));
  });
  for (const [id, ro] of [...runObjects]) {
    if (!seen.has(id)) {
      scene.remove(ro.group);
      disposeDeep(ro.group);
      runObjects.delete(id);
    }
  }
  syncArcs(runs, p);
  render3dStatus();
}

/* ------------------------------------------------------------ session arcs
   The arc's bounds are the frozen protocol's own, read from the transcript
   the guest writes (models/session.ts): in flight from the `[vsock->] hello`
   line to the `[vsock->] bye`. Measured against this daemon, the demo
   session completes inside ONE serial delivery frame (~300 lines land in a
   single burst), so the protocol window itself is never observed
   mid-flight — hence the arc also dwells one beat (ARC_DWELL_MS) after a
   bye observed while its run is open, the same recency idiom the activity
   EMA gives the glow and pulse. The serial-rate EMA was considered as the
   activity signal and rejected: it measures chatter (boot spew included),
   not vsock traffic, and would light arcs for VMs with no session at all.
   A session read from an already-closed run is history and draws no arc;
   a killed VM's arc dies with it. In flight the arc carries a travelling
   packet; during the dwell it stands quiet; reduced motion parks the packet. */
const sessionDoneAt = new Map<string, number>();

function syncArcs(runs: Run[], p: Palette): void {
  if (!scene) return;
  const active = new Set<string>();
  const now = Date.now();
  if (hostAnchor) {
    for (const run of runs) {
      const ro = runObjects.get(run.id);
      if (!ro) continue;
      const detail = store.details.get(run.id);
      for (const vm of run.vms) {
        const key = `${run.id}/${vm.id}`;
        const vo = ro.vms.get(vm.id);
        const serial = detail?.vms.find((v) => v.id === vm.id)?.serial;
        /* no capture yet (the detail GET lands within a tick): nothing to
           read, and linesFor must never see an empty raw — it would take
           the fresh-capture path and wipe the line cache */
        if (!vo || serial == null) continue;
        const session = sessionOf(store.linesFor(run.id, vm.id, serial));
        if (session == null) {
          sessionDoneAt.delete(key);
          continue;
        }
        if (session.phase === 'active') {
          sessionDoneAt.delete(key);
        } else if (!sessionDoneAt.has(key) && isOpen(run)) {
          /* the bye's arrival, stamped once — a completion only starts the
             dwell while its run is still open; history draws no arc */
          sessionDoneAt.set(key, now);
        }
        if (!arcAlive(session.phase, vm.state, sessionDoneAt.get(key) ?? null, now)) continue;
        active.add(key);
        ensureArc(key, [
          vo.mesh.position.x,
          0.6 + vo.mesh.scale.y + 0.15,
          ro.group.position.z + vo.mesh.position.z,
        ], p, session.phase === 'active');
      }
    }
  }
  for (const [key, arc] of [...arcs]) {
    if (!active.has(key)) {
      scene.remove(arc.line);
      scene.remove(arc.packet);
      arc.line.geometry.dispose();
      arc.lineMat.dispose();
      arc.packet.geometry.dispose();
      arc.packetMat.dispose();
      arcs.delete(key);
    }
  }
  /* stamps for runs that left the register go with them */
  for (const key of [...sessionDoneAt.keys()]) {
    const runId = key.slice(0, key.indexOf('/'));
    if (!runs.some((r) => r.id === runId)) sessionDoneAt.delete(key);
  }
  liveSessions = active.size;
}

/** Raise (or re-land) the arc from a VM block's top to the host marker:
    a quadratic bezier bowed by arcLift, redrawn only when an end moved.
    In flight it is ink with a travelling packet; in the post-bye dwell it
    stands quieter, the packet parked. */
function ensureArc(key: string, from: [number, number, number], p: Palette, inFlight: boolean): void {
  const THREE = three();
  if (!scene || !hostAnchor) return;
  const to = hostAnchor;
  let arc = arcs.get(key);
  if (!arc) {
    const lineMat = new THREE.LineBasicMaterial({ color: p.ink, transparent: true, opacity: 0.85 });
    const line = new THREE.Line(new THREE.BufferGeometry(), lineMat);
    line.frustumCulled = false;
    const packetMat = new THREE.MeshBasicMaterial({ color: p.ok });
    const packet = new THREE.Mesh(new THREE.SphereGeometry(0.17, 10, 8), packetMat);
    /* the packet's phase is a stable hash of the key — arcs never pulse in
       lockstep, and a re-created arc resumes where its key says */
    let h = 0;
    for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) | 0;
    arc = {
      line, lineMat, packet, packetMat,
      curve: null,
      from: [0, 0, 0],
      to: [0, 0, 0],
      offset: ((h % 100) + 100) % 100 / 100,
    };
    arcs.set(key, arc);
    scene.add(line);
    scene.add(packet);
  }
  arc.lineMat.color.set(inFlight ? p.ink : p.ink3);
  arc.packetMat.color.set(p.ok);
  arc.packet.visible = inFlight && !REDUCED;
  if (arc.from[0] !== from[0] || arc.from[1] !== from[1] || arc.from[2] !== from[2] || arc.to[2] !== to[2]) {
    arc.from = from;
    arc.to = to;
    const a = new THREE.Vector3(from[0], from[1], from[2]);
    const b = new THREE.Vector3(to[0], to[1], to[2]);
    const mid = a.clone().lerp(b, 0.5);
    mid.y = Math.max(a.y, b.y) + arcLift(a.distanceTo(b));
    arc.curve = new THREE.QuadraticBezierCurve3(a, mid, b);
    arc.line.geometry.dispose();
    arc.line.geometry = new THREE.BufferGeometry().setFromPoints(arc.curve.getPoints(28));
    if (!REDUCED) arc.packet.position.copy(arc.curve.getPoint(arc.offset));
  }
}

/** The block grid a run's VMs take on its platform: near-square, so a
    50-VM run is a district (8 × 7), not a 130-unit queue off the plot. */
function gridOf(n: number): { cols: number; rows: number } {
  const cols = Math.max(1, Math.ceil(Math.sqrt(Math.max(1, n))));
  return { cols, rows: Math.max(1, Math.ceil(Math.max(1, n) / cols)) };
}

/** Platform length/depth for a grid, with the one-block margin the old
    single-row platform had. */
function platformSize(cols: number, rows: number): { len: number; dep: number } {
  return { len: cols * CELL_PITCH, dep: rows * CELL_PITCH + 1.8 };
}

/** Reshape a run's platform when its VM count changes (VM handles land
    after 'starting'); geometry is disposed, never leaked. */
function reshapePlatform(ro: RunObject, n: number): void {
  const THREE = three();
  const { cols, rows } = gridOf(n);
  if (ro.cols === cols && ro.rows === rows) return;
  ro.cols = cols;
  ro.rows = rows;
  const { len, dep } = platformSize(cols, rows);
  ro.platform.geometry.dispose();
  ro.platform.geometry = new THREE.BoxGeometry(len, 0.6, dep);
  ro.pedges.geometry.dispose();
  ro.pedges.geometry = new THREE.EdgesGeometry(ro.platform.geometry);
  /* the run's nameplate hangs off the platform's left end, clear of the blocks */
  ro.label.position.set(-len / 2 - 1.2 - ro.label.scale.x / 2, 1.3, 0);
}

function addRun(run: Run, p: Palette): RunObject {
  const THREE = three();
  if (!scene) throw new Error('the 3D scene has not booted');
  const group = new THREE.Group();
  const platform = new THREE.Mesh(
    new THREE.BoxGeometry(CELL_PITCH, 0.6, CELL_PITCH + 1.8),
    new THREE.MeshLambertMaterial({ color: p.paper }),
  );
  platform.position.y = 0.3;
  group.add(platform);
  const pedges = new THREE.LineSegments(
    new THREE.EdgesGeometry(platform.geometry),
    new THREE.LineBasicMaterial({ color: p.ink3 }),
  );
  pedges.position.copy(platform.position);
  group.add(pedges);
  /* a blank plate; the sync pass textures it on the same render */
  const label = makePlate('', '', p.ink, p.ink3);
  group.add(label);
  scene.add(group);
  const ro: RunObject = { group, platform, pedges, label, plateKey: '', vms: new Map(), cols: 0, rows: 0 };
  reshapePlatform(ro, run.vms.length);
  runObjects.set(run.id, ro);
  return ro;
}

function syncVm(ro: RunObject, run: Run, vm: Vm, index: number, p: Palette): void {
  const THREE = three();
  let vo = ro.vms.get(vm.id);
  if (!vo) {
    const geo = new THREE.BoxGeometry(1.6, 1, 1.6);
    const mat = new THREE.MeshLambertMaterial({ color: p.sheet });
    const mesh = new THREE.Mesh(geo, mat);
    const edgeMat = new THREE.LineBasicMaterial({ color: p.ink2 });
    const edges = new THREE.LineSegments(new THREE.EdgesGeometry(geo), edgeMat);
    mesh.add(edges);
    mesh.userData = { runId: run.id, index };
    const label = makeLabel(vmLabelText(vm), p.ink2);
    /* the label hangs off the run group, not the mesh — a mesh-child sprite
       would stretch with the block's height scale */
    vo = { mesh, edges, label, mat, edgeMat, runId: run.id, index, state: vm.state, labelKey: vmLabelText(vm) };
    ro.vms.set(vm.id, vo);
    ro.group.add(mesh);
    ro.group.add(label);
  }
  /* place on the platform's grid by VM index */
  const { len, dep } = platformSize(ro.cols, ro.rows);
  const col = index % ro.cols;
  const row = Math.floor(index / ro.cols);
  const x = -len / 2 + CELL_PITCH / 2 + col * CELL_PITCH;
  const z = -(dep - 1.8) / 2 + CELL_PITCH / 2 + row * CELL_PITCH;
  vo.mesh.position.set(x, 0.6, z);

  /* state is height and edge: running stands and glows, exited settles,
     killed falls flat in crit ink */
  const lps = store.activity.get(vmKey(run.id, vm.id));
  const height = vm.state === 'running' ? 2.2 : vm.state === 'exited' ? 0.7 : 0.35;
  vo.mesh.scale.y += (height - vo.mesh.scale.y) * 0.5;
  vo.mesh.position.y = 0.6 + vo.mesh.scale.y / 2;
  /* the block's words — vm id, guest CID, and the role the register names —
     retextured only when they change (a canvas draw + upload) */
  const labelText = vmLabelText(vm);
  if (labelText !== vo.labelKey) {
    vo.labelKey = labelText;
    retextureLabel(vo.label, labelText, p.ink2);
  }
  vo.state = vm.state;
  vo.label.position.set(x, vo.mesh.position.y + vo.mesh.scale.y / 2 + 0.7, z);

  const glow = vm.state === 'running'
    ? Math.min(1, 0.25 + lps / 3) * (REDUCED ? 1 : 0.7 + 0.3 * Math.sin(pulse))
    : 0;
  vo.mat.color.set(vm.state === 'killed' ? p.rule : p.sheet);
  vo.mat.emissive.set(p.ok);
  vo.mat.emissiveIntensity = glow;
  vo.edgeMat.color.set(vm.state === 'running' ? p.ok : vm.state === 'killed' ? p.crit : p.ink2);

  /* selection: the owner's signal outline */
  if (store.detailId === run.id && store.vm === index) {
    vo.edgeMat.color.set(p.signal);
  }
}

/** A block's own words: its index, the guest CID when it has one, and the
    role the register names for it — the fleet's agent made readable on the
    plot. Absent fields add nothing; no placeholder is invented. */
function vmLabelText(vm: Vm): string {
  const parts = [`vm ${vm.id}`];
  if (vm.guest_cid != null) parts.push(`cid ${vm.guest_cid}`);
  const role = roleOf(vm);
  if (role != null) parts.push(role);
  return parts.join(' · ');
}

/* ------------------------------------------------------------ picking */
function pick(ev: MouseEvent): VmObject | null {
  const THREE = three();
  if (!raycaster || !camera || !renderer) return null;
  const rect = renderer.domElement.getBoundingClientRect();
  const ndc = new THREE.Vector2(
    ((ev.clientX - rect.left) / rect.width) * 2 - 1,
    -((ev.clientY - rect.top) / rect.height) * 2 + 1,
  );
  raycaster.setFromCamera(ndc, camera);
  const meshes: Object3D[] = [];
  for (const ro of runObjects.values()) for (const vo of ro.vms.values()) meshes.push(vo.mesh);
  const hits = raycaster.intersectObjects(meshes, false);
  const hit = hits[0]?.object;
  if (!hit) return null;
  const data: Record<string, unknown> = hit.userData;
  if (typeof data['runId'] !== 'string' || typeof data['index'] !== 'number') return null;
  return runObjects.get(data['runId'])?.vms.get(data['index']) ?? null;
}

function onClick(ev: MouseEvent): void {
  const vo = pick(ev);
  if (!vo) return;
  store.detailId = vo.runId;
  store.vm = vo.index;
  if (location.hash !== `#run/${vo.runId}`) location.hash = `#run/${vo.runId}`;
  else store.scheduleRender();
}

function onHover(ev: MouseEvent): void {
  if (!renderer || !module3) return;
  renderer.domElement.style.setProperty('cursor', pick(ev) ? 'pointer' : 'default');
}

/* ------------------------------------------------------------ camera presets
   Two framings, one derivation (district.ts): the overview puts the whole
   plot and every platform row in frame; the focus lands on the selected
   run's platform — the register's notion of selected (the open entry, else
   the newest open run), never a 3D-only selection. Buttons and the o/r keys
   call the same preset; reduced motion jumps, else the camera travels. */

/** The run the focus preset frames: the open entry when it is on the plot,
    else the newest open run, else the register's newest — the Run index
    entry's own rule. */
function focusRunId(): string | null {
  if (store.detailId && runObjects.has(store.detailId)) return store.detailId;
  const open = store.runs.find((r) => r.state === 'starting' || r.state === 'running');
  const id = (open ?? store.runs[0])?.id ?? null;
  return id != null && runObjects.has(id) ? id : null;
}

export function cameraPreset(kind: 'overview' | 'focus'): void {
  if (!camera || !controls || !renderer) return;
  let framing: Framing | null;
  if (kind === 'overview') {
    const m = store.machine;
    framing = overviewFraming(m?.cpus ?? 16, m?.memory_mib ?? 16384, store.runs.length);
  } else {
    const id = focusRunId();
    const ro = id != null ? runObjects.get(id) : undefined;
    if (!ro) {
      framing = null;
    } else {
      const { len, dep } = platformSize(ro.cols, ro.rows);
      framing = focusFraming(len, dep, ro.group.position.z);
    }
  }
  if (!framing) return;
  if (REDUCED) {
    camera.position.set(framing.pos[0], framing.pos[1], framing.pos[2]);
    controls.target.set(framing.target[0], framing.target[1], framing.target[2]);
    controls.update();
    return;
  }
  flight = {
    fromPos: [camera.position.x, camera.position.y, camera.position.z],
    fromTgt: [controls.target.x, controls.target.y, controls.target.z],
    toPos: framing.pos,
    toTgt: framing.target,
    t0: performance.now(),
    dur: FLIGHT_MS,
  };
}

/** The 3D sheet's own wiring: the two framing buttons. */
export function initView3d(): void {
  byId('v3-overview').addEventListener('click', () => cameraPreset('overview'));
  byId('v3-focus').addEventListener('click', () => cameraPreset('focus'));
}

/** Read-only camera state for the verification harness; the app never reads it. */
export function cameraState(): { pos: [number, number, number]; target: [number, number, number] } | null {
  if (!camera || !controls) return null;
  return {
    pos: [camera.position.x, camera.position.y, camera.position.z],
    target: [controls.target.x, controls.target.y, controls.target.z],
  };
}

/** Read-only district state for the verification harness; the app never reads
    it. What the plot actually holds — the platforms, blocks and live arcs —
    and the words on the first run's plate and first block, so the labels can
    be read back as drawn, not just as derived. */
export function districtState(): {
  runs: number;
  vms: number;
  arcs: number;
  sessions: number;
  plate: string | null;
  label: string | null;
} | null {
  if (!scene) return null;
  let vms = 0;
  let label: string | null = null;
  for (const ro of runObjects.values()) {
    vms += ro.vms.size;
    if (label == null) label = [...ro.vms.values()][0]?.labelKey ?? null;
  }
  const first = store.runs.slice().sort((a, b) => seqOf(a.id) - seqOf(b.id))[0];
  let plate: string | null = null;
  if (first) {
    const lines = runPlateLines(first);
    plate = `${lines.title} · ${lines.sub}`;
  }
  return { runs: runObjects.size, vms, arcs: arcs.size, sessions: liveSessions, plate, label };
}

/* ------------------------------------------------------------ status */
function render3dStatus(): void {
  const notice = byId('v3-notice');
  if (failed) {
    showFailure(failed);
    return;
  }
  if (store.runsError != null && store.runs.length === 0) {
    notice.hidden = false;
    byId('v3-notice-mk').textContent = 'node unreachable';
    byId('v3-notice-text').textContent =
      `The node daemon is not answering at ${location.origin} (${store.runsError}). Retrying.`;
  } else if (!renderer) {
    notice.hidden = false;
    byId('v3-notice-mk').textContent = 'raising the plot';
    byId('v3-notice-text').textContent = 'Loading the 3D surveyor…';
  } else {
    notice.hidden = true;
  }
  const live = byId('v3-live');
  live.hidden = store.health !== 'live';
  live.textContent = store.health;
  const vms = store.runs.reduce((n, r) => n + r.vms.length, 0);
  byId('v3-status').textContent = store.runs.length === 0
    ? 'The register is blank — no runs to raise. Enter one on the register page.'
    : `${store.runs.length} ${store.runs.length === 1 ? 'run' : 'runs'} · ${vms} VMs on the plot` +
      (liveSessions > 0 ? ` · ${liveSessions} ${liveSessions === 1 ? 'session' : 'sessions'} on the wire` : '') +
      '.';
  /* the focus button names the run it frames */
  const focus = byId<HTMLButtonElement>('v3-focus');
  const fid = focusRunId();
  focus.disabled = fid == null;
  focus.textContent = fid != null ? `Frame run ${fid}` : 'Frame a run';
}

function showFailure(why: string): void {
  const notice = byId('v3-notice');
  notice.hidden = false;
  byId('v3-notice-mk').textContent = 'no webgl2';
  const text = byId('v3-notice-text');
  text.textContent = '';
  text.append(`The 3D plot needs WebGL2, which this browser refused (${why}). The `);
  const a = document.createElement('a');
  a.href = '#map';
  a.textContent = '2D map';
  text.appendChild(a);
  text.append(' carries the same fleet.');
  byId('v3-status').textContent = 'WebGL2 unavailable — fallen back to the 2D map.';
}

/* ------------------------------------------------------------ loop */
function loop(): void {
  const step = (): void => {
    requestAnimationFrame(step);
    if (document.hidden || store.route.name !== 'view3d') return; // paused while hidden or away
    pulse += 0.06;
    if (flight && camera && controls) {
      /* a preset's travel: smoothstep between the two framings */
      const t = Math.min(1, (performance.now() - flight.t0) / flight.dur);
      const e = t * t * (3 - 2 * t);
      camera.position.set(
        flight.fromPos[0] + (flight.toPos[0] - flight.fromPos[0]) * e,
        flight.fromPos[1] + (flight.toPos[1] - flight.fromPos[1]) * e,
        flight.fromPos[2] + (flight.toPos[2] - flight.fromPos[2]) * e,
      );
      controls.target.set(
        flight.fromTgt[0] + (flight.toTgt[0] - flight.fromTgt[0]) * e,
        flight.fromTgt[1] + (flight.toTgt[1] - flight.fromTgt[1]) * e,
        flight.fromTgt[2] + (flight.toTgt[2] - flight.fromTgt[2]) * e,
      );
      if (t >= 1) flight = null;
    }
    /* the packets on the live session arcs: one trip per second or so,
       each arc on its own phase */
    for (const arc of arcs.values()) {
      if (arc.packet.visible && arc.curve) {
        arc.packet.position.copy(arc.curve.getPoint((pulse * 0.3 + arc.offset) % 1));
      }
    }
    controls?.update();
    if (renderer && scene && camera) renderer.render(scene, camera);
  };
  requestAnimationFrame(step);
}
