/* The 3D view: the same register, raised. The machine is the ground — a grid
   ruled to its CPUs and memory, labeled on its edges; runs are platforms
   arranged by id; VMs are blocks whose state is their height and edge (running
   glows with its serial chatter, exited settles, killed falls flat and crit).
   Guest CIDs float as labels. Drag orbits, scroll zooms, a click selects the
   VM and opens its entry. Every object is a machine, a run, or a VM — no
   decoration.
   Rendering: three.js WebGLRenderer on WebGL2 (r163+ has no WebGL1 path;
   three's WebGPURenderer is still not the stable default on r186 — WebGL2 is
   the boring, correct choice). three is dynamically imported on first show so
   the register's bundle stays lean; the render loop pauses in hidden tabs. */

import type {
  Group,
  LineBasicMaterial,
  LineSegments,
  Mesh,
  MeshLambertMaterial,
  Object3D,
  PerspectiveCamera,
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
import { seqOf } from '../models/run';
import { store } from '../store';

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
}

interface RunObject {
  group: Group;
  platform: Mesh;
  label: Sprite;
  vms: Map<number, VmObject>;
}

interface Palette {
  paper: string; sheet: string; ink: string; ink2: string; ink3: string;
  rule: string; signal: string; ok: string; crit: string;
}

const REDUCED = typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;
const CELL_PITCH = 2.6; // ground units between VM blocks on a platform

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

function three(): ThreeModule {
  if (!module3) throw new Error('the 3D scene has not booted');
  return module3;
}

function palette(): Palette {
  const cs = getComputedStyle(document.documentElement);
  const g = (name: string): string => cs.getPropertyValue(name).trim();
  return {
    paper: g('--paper'), sheet: g('--sheet'), ink: g('--ink'), ink2: g('--ink-2'), ink3: g('--ink-3'),
    rule: g('--rule'), signal: g('--signal'), ok: g('--ok'), crit: g('--crit'),
  };
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
    ctl.autoRotate = false;
    clearTimeout(autoOrbitTimer);
    autoOrbitTimer = setTimeout(() => {
      if (!REDUCED) ctl.autoRotate = true;
    }, 5000);
  });
  controls = ctl;

  r.domElement.addEventListener('click', onClick);
  r.domElement.addEventListener('mousemove', onHover);

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
  const fresh = makeLabel(text, color);
  const oldMat = sprite.material;
  sprite.material = fresh.material;
  sprite.scale.copy(fresh.scale);
  oldMat.map?.dispose();
  oldMat.dispose();
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
    ro.group.position.set(0, 0, (rowIdx - (rows - 1) / 2) * 9);
    retextureLabel(ro.label, runLabelText(run), run.state === 'running' ? p.ink : p.ink3);
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
  render3dStatus();
}

function runLabelText(run: Run): string {
  return `${run.id} · ${run.vms.length} VMs × ${run.vcpus} vCPU · ${fmtNum(run.memory_mib)} MiB · ${run.state}`;
}

function addRun(run: Run, p: Palette): RunObject {
  const THREE = three();
  if (!scene) throw new Error('the 3D scene has not booted');
  const group = new THREE.Group();
  const len = Math.max(CELL_PITCH, run.vms.length * CELL_PITCH);
  const platform = new THREE.Mesh(
    new THREE.BoxGeometry(len, 0.6, 4.4),
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
  const label = makeLabel(runLabelText(run), p.ink);
  /* left of the platform, clear of the blocks: right edge at the platform end */
  label.position.set(-len / 2 - 1.2 - label.scale.x / 2, 1.0, 0);
  group.add(label);
  scene.add(group);
  const ro: RunObject = { group, platform, label, vms: new Map() };
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
    vo = { mesh, edges, label, mat, edgeMat, runId: run.id, index, state: vm.state };
    ro.vms.set(vm.id, vo);
    ro.group.add(mesh);
    ro.group.add(label);
  }
  /* place on the platform by VM index */
  const len = Math.max(CELL_PITCH, run.vms.length * CELL_PITCH);
  const x = -len / 2 + CELL_PITCH / 2 + index * CELL_PITCH;
  vo.mesh.position.set(x, 0.6, 0);

  /* state is height and edge: running stands and glows, exited settles,
     killed falls flat in crit ink */
  const lps = store.activity.get(vmKey(run.id, vm.id));
  const height = vm.state === 'running' ? 2.2 : vm.state === 'exited' ? 0.7 : 0.35;
  vo.mesh.scale.y += (height - vo.mesh.scale.y) * 0.5;
  vo.mesh.position.y = 0.6 + vo.mesh.scale.y / 2;
  if (vo.state !== vm.state) {
    vo.state = vm.state;
    retextureLabel(vo.label, vmLabelText(vm), p.ink2);
  }
  vo.label.position.set(x, vo.mesh.position.y + vo.mesh.scale.y / 2 + 0.7, 0);

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

function vmLabelText(vm: Vm): string {
  return vm.guest_cid != null ? `vm ${vm.id} · cid ${vm.guest_cid}` : `vm ${vm.id}`;
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
    : `${store.runs.length} ${store.runs.length === 1 ? 'run' : 'runs'} · ${vms} VMs on the plot.`;
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
    controls?.update();
    if (renderer && scene && camera) renderer.render(scene, camera);
  };
  requestAnimationFrame(step);
}
