// The hardware lab: a developer screen for the hardware models under assets/hardware/. It shows one
// model under the Floor's lighting, with its state controls and its validation errors in red.
// Route: #/hardware-lab?model=exterior/workstation&gpus=1&status=online&working=0&open=0&traces=1&cam=iso
// (cam: iso | top | front | side | anchor). Any other numeric key is a model param, overriding its default:
// &p_cores=16&e_cores=8&gpu_cores=76. The model's params are number inputs in the controls. The URL is kept in step.
// model=fleet shows the Machines scene (fleet.ts) with GET /v1/fleet from the live hub; &fake=N adds N made-up
// machines of mixed kinds, vendors and statuses to it (for judging the ring, cables and statuses).
// &zoom=8 divides the camera's distance by 8 (close-ups of small parts).

import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { h } from '../ui/dom';
import { boardWithRooms } from './floor3d/board-rooms';
import type { Cleanup } from './index';
import { api, type FleetMachine, type FleetResponse, type FleetStatus } from '../api';
import { MODELS, PALETTE } from './floor3d/hardware-index';
import { buildModel, currentPulses, placeAtom, traceStrips, validate, type Built, type Model, type Pulses, type Status } from './floor3d/hardware';
import { buildFleet, type FleetScene } from './floor3d/fleet';
import { sky, toonMat } from './floor3d/toon';
import { createMeter, startPixelRatio } from './floor3d/perf';
import { engineWanted, startHardwareLabEngine, type HardwareLabOptions } from './engine-mount';

const STATUSES: string[] = ['online', 'draining', 'offline', 'revoked'];
const CAMS = ['iso', 'top', 'front', 'side', 'anchor'] as const;
type Cam = (typeof CAMS)[number];

interface LabState {
  model: string;
  /** Board models: n synthetic agents seated (one per roster role) with their rooms (&rooms=n). */
  rooms: number;
  gpus: number;
  status: Status;
  working: number;
  open: number;
  traces: boolean;
  cam: Cam;
  /** Made-up fleet machines added to model=fleet (&fake=N). */
  fake: number;
  /** Camera distance divisor (&zoom=N): above 1 comes closer. */
  zoom: number;
  /** Model params from the URL (every numeric key the lab does not use itself); they override the model's defaults. */
  params: Record<string, number>;
}

const DEFAULT_STATE: LabState = { model: 'exterior/workstation', rooms: 0, gpus: 0, status: 'online', working: 0, open: 0, traces: true, cam: 'iso', fake: 0, zoom: 1, params: {} };
/** The query keys the lab reads for itself; any other key that holds a number is a model param. */
const LAB_KEYS = new Set(['model', 'gpus', 'status', 'working', 'open', 'traces', 'cam', 'fake', 'zoom', 'rooms']);
const PARAM_KEY = /^[a-z_]\w*$/;

const NO_FLEET: FleetResponse = { machines: [], links: [] };
type FakeTemplate = Omit<FleetMachine, 'id' | 'name' | 'role' | 'status' | 'agents' | 'working' | 'stats' | 'last_seen_ms'>;
/** The kinds the fake machines cycle through: a laptop, an edge device, a phone, a server, an Apple laptop, a workstation with a GPU. */
const FAKE_TEMPLATES: FakeTemplate[] = [
  { kind: 'laptop', vendor: 'lenovo', model: 'ThinkPad X1 Carbon', cpu: { model: 'Intel Core Ultra 7 165U', cores: 10, threads: 12 }, memory_mib: 32768, gpus: [], storage_gib: 512, arch: 'x86_64', os: 'linux', backend: 'kvm', apple: null },
  { kind: 'edge', vendor: 'raspberrypi', model: 'Raspberry Pi 5', cpu: { model: 'Cortex-A76', cores: 4, threads: 4 }, memory_mib: 8192, gpus: [], storage_gib: 256, arch: 'aarch64', os: 'linux', backend: 'kvm', apple: null },
  { kind: 'phone', vendor: 'samsung', model: 'Galaxy S24', cpu: { model: 'Snapdragon 8 Gen 3', cores: 8, threads: 8 }, memory_mib: 8192, gpus: [], storage_gib: 256, arch: 'aarch64', os: 'android', backend: 'none', apple: null },
  { kind: 'server', vendor: 'dell', model: 'PowerEdge R750', cpu: { model: 'Intel Xeon Gold 6338', cores: 32, threads: 64 }, memory_mib: 262144, gpus: [{ name: 'NVIDIA A40', vendor: 'NVIDIA', memory_mib: 46080 }], storage_gib: 7680, arch: 'x86_64', os: 'linux', backend: 'kvm', apple: null },
  { kind: 'laptop', vendor: 'apple', model: 'MacBook Pro (M3 Max)', cpu: { model: 'Apple M3 Max', cores: 16, threads: 16 }, memory_mib: 65536, gpus: [], storage_gib: 1024, arch: 'arm64', os: 'macos', backend: 'hvf', apple: { chip: 'Apple M3 Max', p_cores: 12, e_cores: 4, gpu_cores: 40, neural_cores: 16, unified_memory_gib: 64 } },
  { kind: 'workstation', vendor: 'dell', model: 'Precision 7865', cpu: { model: 'AMD Ryzen Threadripper PRO 7965WX', cores: 24, threads: 48 }, memory_mib: 131072, gpus: [{ name: 'NVIDIA GeForce RTX 4090', vendor: 'NVIDIA', memory_mib: 24576 }], storage_gib: 2048, arch: 'x86_64', os: 'linux', backend: 'kvm', apple: null },
];

/**
 * The fleet with n fake nodes added (ids f1..fn, linked to the hub). Statuses: the third is draining, the sixth
 * offline. Without a hub in the fleet, a fake one (the last template) is added first.
 */
export function withFake(view: FleetResponse, n: number): FleetResponse {
  if (n <= 0) return view;
  const machines = [...view.machines];
  const links = [...view.links];
  const make = (t: FakeTemplate, id: string, role: 'hub' | 'node', status: FleetStatus, working: number): FleetMachine => ({
    ...t,
    id,
    name: `fake ${t.model ?? t.kind}`,
    role,
    status,
    agents: [],
    working,
    stats: status === 'offline' ? null : { cpu_pct: 8 + ((working * 23 + id.length * 7) % 60), mem_used_mib: 2048 },
    last_seen_ms: status === 'offline' ? 0 : Date.now(),
  });
  let hub = machines.find((m) => m.role === 'hub');
  if (!hub) {
    hub = make(FAKE_TEMPLATES[5]!, 'fake-hub', 'hub', 'online', 1);
    machines.push(hub);
  }
  for (let i = 0; i < n; i++) {
    const status: FleetStatus = i === 2 ? 'draining' : i === 5 ? 'offline' : 'online';
    const id = `f${i + 1}`;
    machines.push(make(FAKE_TEMPLATES[i % FAKE_TEMPLATES.length]!, id, 'node', status, status === 'offline' ? 0 : i % 3));
    links.push({ a: hub.id, b: id, online: status !== 'offline' });
  }
  return { machines, links };
}

/** The lab's state from the route's query; a missing or bad value takes its default. */
function readState(hash: string): LabState {
  const q = new URLSearchParams(hash.split('?')[1] ?? '');
  const num = (key: string, lo: number, hi: number, fb: number): number => {
    const v = Number(q.get(key));
    return q.has(key) && Number.isFinite(v) ? Math.min(hi, Math.max(lo, v)) : fb;
  };
  const status = q.get('status') ?? '';
  const cam = q.get('cam') ?? '';
  const params: Record<string, number> = {};
  q.forEach((value, key) => {
    if (!LAB_KEYS.has(key) && PARAM_KEY.test(key) && value.trim() !== '' && Number.isFinite(Number(value))) params[key] = Number(value);
  });
  return {
    model: q.get('model') ?? DEFAULT_STATE.model,
    rooms: Math.round(num('rooms', 0, 40, DEFAULT_STATE.rooms)),
    gpus: Math.round(num('gpus', 0, 2, DEFAULT_STATE.gpus)),
    status: (STATUSES.includes(status) ? status : DEFAULT_STATE.status) as Status,
    working: Math.round(num('working', 0, 6, DEFAULT_STATE.working)),
    open: num('open', 0, 1, DEFAULT_STATE.open),
    traces: q.has('traces') ? q.get('traces') !== '0' : DEFAULT_STATE.traces,
    cam: (CAMS as readonly string[]).includes(cam) ? (cam as Cam) : DEFAULT_STATE.cam,
    fake: Math.round(num('fake', 0, 12, DEFAULT_STATE.fake)),
    zoom: num('zoom', 0.1, 50, DEFAULT_STATE.zoom),
    params,
  };
}

const sameParams = (a: Record<string, number>, b: Record<string, number>): boolean => {
  const keys = Object.keys(a);
  return keys.length === Object.keys(b).length && keys.every((k) => a[k] === b[k]);
};

/** Frees a built model's own geometry and materials (the shared outline material stays). */
function disposeTree(root: THREE.Object3D): void {
  root.traverse((o) => {
    const m = o as THREE.Mesh;
    if (!m.isMesh || m.userData.isOutline) return;
    m.geometry.dispose();
    for (const mat of Array.isArray(m.material) ? m.material : [m.material]) {
      const map = (mat as THREE.MeshBasicMaterial).map;
      map?.dispose();
      mat.dispose();
    }
  });
}

function newRenderer(): THREE.WebGLRenderer | null {
  try {
    const r = new THREE.WebGLRenderer({ antialias: true });
    r.debug.checkShaderErrors = false; // the info log is read only when a shader fails
    return r;
  } catch {
    return null;
  }
}

/** `?engine=wasm` on the page's query: the same model on the wasm engine, its canvas filling the root. The state is read once. */
function mountEngineLab(root: HTMLElement, state: LabState): Cleanup {
  const canvas = document.createElement('canvas');
  canvas.style.cssText = 'display:block;width:100%;height:70vh;min-height:420px;touch-action:none';
  root.append(canvas);
  const lab: HardwareLabOptions = { model: state.model, gpus: state.gpus, status: state.status, working: state.working, open: state.open, traces: state.traces };
  let engine: { destroy: () => void } | null = null;
  let closed = false;
  startHardwareLabEngine(canvas, lab).then(
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
  let state = readState(location.hash);
  if (engineWanted()) return mountEngineLab(root, state);

  // ---- Controls -------------------------------------------------------------------------------
  const bar = { style: 'display:flex;flex-wrap:wrap;gap:12px 18px;align-items:center;padding:8px 0' };
  const field = (label: string, control: HTMLElement): HTMLElement => h('label', { style: 'display:flex;gap:6px;align-items:center' }, h('span', { style: 'font-size:12px;color:var(--ink-2)' }, label), control);
  const modelSel = h(
    'select',
    { 'aria-label': 'Model', onchange: () => { state.model = modelSel.value; state.params = {}; rebuild(true); writeUrl(); sync(); } },
    ...['fleet', ...Object.keys(MODELS).sort()].map((k) => h('option', { value: k }, k)),
  );
  const gpuBtns = [0, 1, 2].map((n) => h('button', { type: 'button', class: 'btn', onclick: () => { state.gpus = n; rebuild(false); writeUrl(); sync(); } }, `${n} GPU${n === 1 ? '' : 's'}`));
  const statusSel = h(
    'select',
    { 'aria-label': 'Status', onchange: () => { state.status = statusSel.value as Status; writeUrl(); sync(); } },
    ...STATUSES.map((s) => h('option', { value: s }, s)),
  );
  const workingIn = h('input', { type: 'range', min: 0, max: 6, step: 1, 'aria-label': 'Working agents', oninput: () => { state.working = Number(workingIn.value); writeUrl(); sync(); } });
  const workingOut = h('span', { style: 'font-family:var(--mono);font-size:12px;min-width:2ch' });
  const openIn = h('input', { type: 'range', min: 0, max: 1, step: 0.01, 'aria-label': 'Open', oninput: () => { state.open = Number(openIn.value); writeUrl(); sync(); } });
  const openOut = h('span', { style: 'font-family:var(--mono);font-size:12px;min-width:4ch' });
  const tracesIn = h('input', { type: 'checkbox', 'aria-label': 'Traces', onchange: () => { state.traces = tracesIn.checked; writeUrl(); sync(); } });
  const camBtns = CAMS.map((c) => h('button', { type: 'button', class: 'btn', onclick: () => { state.cam = c; frame(current?.model, c); writeUrl(); sync(); } }, c));
  // The model's own params (gpus has its buttons): number inputs, rebuilt on each change.
  const paramsBar = h('div', { style: bar.style });
  const paramIns = new Map<string, HTMLInputElement>();
  let paramsOf = '';
  const syncParams = (): void => {
    const model = MODELS[state.model];
    if (state.model !== paramsOf) {
      paramsOf = state.model;
      paramIns.clear();
      const keys = Object.keys(model?.params ?? {}).filter((k) => k !== 'gpus').sort();
      paramsBar.style.display = keys.length ? 'flex' : 'none';
      paramsBar.replaceChildren(
        ...keys.map((k) => {
          const input = h('input', {
            type: 'number',
            step: 'any',
            'aria-label': `param ${k}`,
            style: 'width:9ch;font-family:var(--mono);font-size:12px',
            oninput: () => {
              // A half-typed value (empty, or "-") is not a param yet; the input keeps what was typed.
              if (!Number.isFinite(input.valueAsNumber)) return;
              state.params[k] = input.valueAsNumber;
              rebuild(false);
              writeUrl();
            },
          });
          paramIns.set(k, input);
          return field(k, input);
        }),
      );
    }
    for (const [k, input] of paramIns) input.value = String(state.params[k] ?? model?.params?.[k] ?? 0);
  };
  const toolbar = h(
    'div',
    { style: 'display:flex;flex-direction:column;border-bottom:1px solid var(--rule);padding:0 12px' },
    h('div', { style: bar.style }, field('model', modelSel), h('span', { style: 'display:flex;gap:4px' }, ...gpuBtns), field('status', statusSel)),
    h('div', { style: bar.style }, field('working', workingIn), workingOut, field('open', openIn), openOut, field('traces (boards)', tracesIn)),
    paramsBar,
    h('div', { style: bar.style }, h('span', { style: 'font-size:12px;color:var(--ink-2)' }, 'camera'), h('span', { style: 'display:flex;gap:4px' }, ...camBtns)),
  );
  const errorsEl = h('pre', { style: 'margin:0;padding:8px 12px;font-family:var(--mono);font-size:12px;white-space:pre-wrap;color:var(--crit)' });
  const stage = h('div', { style: 'position:relative;flex:1;min-height:420px;overflow:hidden;background:#0b0e1a' });

  /** Puts the controls in step with `state`, and the state in step with the URL. */
  const sync = (): void => {
    modelSel.value = state.model;
    gpuBtns.forEach((b, i) => (b.style.outline = i === state.gpus ? '2px solid var(--accent)' : ''));
    statusSel.value = state.status;
    workingIn.value = String(state.working);
    workingOut.textContent = String(state.working);
    openIn.value = String(state.open);
    openOut.textContent = state.open.toFixed(2);
    tracesIn.checked = state.traces;
    camBtns.forEach((b, i) => (b.style.outline = CAMS[i] === state.cam ? '2px solid var(--accent)' : ''));
    syncParams();
  };
  const writeUrl = (): void => {
    const q = new URLSearchParams({
      model: state.model,
      gpus: String(state.gpus),
      status: state.status,
      working: String(state.working),
      open: String(state.open),
      traces: state.traces ? '1' : '0',
      cam: state.cam,
    });
    if (state.fake) q.set('fake', String(state.fake));
    if (state.zoom !== 1) q.set('zoom', String(state.zoom));
    for (const [k, v] of Object.entries(state.params)) q.set(k, String(v));
    history.replaceState(null, '', `#/hardware-lab?${q.toString().replace(/%2F/g, '/')}`);
  };

  // ---- Three.js stage (the Floor's lighting, self-contained) ----------------------------------
  const renderer = newRenderer();
  if (!renderer) stage.append(h('p', { style: 'padding:16px;color:var(--ink-2)' }, 'WebGL is not available in this browser.'));
  root.replaceChildren(toolbar, errorsEl, stage);
  if (!renderer) {
    renderErrors();
    return () => {};
  }
  renderer.setPixelRatio(startPixelRatio());
  renderer.shadowMap.enabled = true;
  renderer.shadowMap.type = THREE.PCFShadowMap;
  const canvas = renderer.domElement;
  canvas.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;display:block;cursor:grab';
  canvas.setAttribute('aria-label', 'The model');
  stage.append(canvas);

  const scene = new THREE.Scene();
  scene.add(sky(new THREE.Color(0x05071a), new THREE.Color(0x3a2275), new THREE.Color(0x090c18)));
  const camera = new THREE.PerspectiveCamera(40, 1, 0.05, 2000);
  const controls = new OrbitControls(camera, canvas);
  controls.enableDamping = true;
  controls.dampingFactor = 0.08;
  controls.minDistance = 0.5;
  controls.maxDistance = 1000;
  scene.add(new THREE.HemisphereLight(0x9fb0ff, 0x2a1d52, 1.15));
  const sun = new THREE.DirectionalLight(0xd2ddff, 2.0);
  sun.position.set(22, 40, 16);
  sun.castShadow = true;
  sun.shadow.mapSize.set(2048, 2048);
  sun.shadow.bias = -0.0008;
  sun.shadow.normalBias = 0.03;
  Object.assign(sun.shadow.camera, { left: -60, right: 60, top: 60, bottom: -60, near: 1, far: 200 });
  sun.shadow.camera.updateProjectionMatrix();
  scene.add(sun);
  const ground = new THREE.Mesh(new THREE.PlaneGeometry(800, 800), toonMat({ color: 0x1d2140 }));
  ground.rotation.x = -Math.PI / 2;
  ground.receiveShadow = true;
  scene.add(ground);
  // A one-unit grid: the scale the models are drawn to.
  const grid = new THREE.GridHelper(120, 120, 0x3b4566, 0x242a48);
  grid.position.y = 0.01;
  scene.add(grid);

  const resize = (): void => {
    const w = stage.clientWidth;
    const hgt = stage.clientHeight;
    if (!w || !hgt) return;
    renderer.setSize(w, hgt, false);
    camera.aspect = w / hgt;
    camera.updateProjectionMatrix();
  };
  const ro = new ResizeObserver(resize);
  ro.observe(stage);
  resize();

  // ---- The model -------------------------------------------------------------------------------
  let current: { model: Model; built: Built; traces: THREE.Group | null; pulses: Pulses | null; atom: { atom: THREE.Object3D; animate: (t: number) => void } | null } | null = null;

  function renderErrors(errors: string[] = lastErrors): void {
    errorsEl.textContent = errors.length ? errors.join('\n') : lastInfo || `${state.model}: no errors`;
    errorsEl.style.color = errors.length ? 'var(--crit)' : 'var(--ink-2)';
  }
  let lastErrors: string[] = [];
  let lastInfo = '';

  /** Frames a sphere: the camera's preset (iso by default), at a distance that fits it. */
  function frameSphere(at: [number, number, number], radius: number, cam: Cam): void {
    const centre = new THREE.Vector3(...at);
    const dist = ((Math.max(radius, 1) / Math.sin(THREE.MathUtils.degToRad(camera.fov / 2))) * 0.9) / state.zoom;
    const dir = { iso: [0.8, 0.6, 1], top: [0, 1, 0.0001], front: [0, 0, 1], side: [1, 0, 0], anchor: [0.8, 0.6, 1] }[cam];
    camera.position.copy(centre).add(new THREE.Vector3(...dir).normalize().multiplyScalar(dist));
    controls.target.copy(centre);
  }

  /** Frames the model: the camera's preset (iso by default), at a distance that fits its size. The fleet is framed by its own sphere. */
  function frame(model: Model | undefined, cam: Cam): void {
    if (!model && fleetScene) {
      const f = fleetScene.frame();
      frameSphere(f.center, f.radius, cam);
      controls.update();
      return;
    }
    const [w, hgt, d] = model?.size ?? [4, 6, 4];
    if (cam === 'anchor' && model?.anchors?.camera) {
      const look = new THREE.Vector3(...model.anchors.camera.look);
      camera.position.copy(look).add(new THREE.Vector3(...model.anchors.camera.at).sub(look).divideScalar(state.zoom));
      controls.target.copy(look);
    } else {
      frameSphere([0, hgt / 2, 0], Math.hypot(w, hgt, d) / 2, cam);
    }
    controls.update();
  }

  // ---- The fleet (model=fleet) ----------------------------------------------------------------
  let fleetData: FleetResponse | null = null;
  let fleetAsked = false;
  let fleetLoading = false;
  let fleetError = '';
  let fleetScene: FleetScene | null = null;
  let fleetView: FleetResponse | null = null;
  let alive = true;

  /** GET /v1/fleet once; the fleet, when it is shown, is rebuilt with the answer. */
  async function loadFleet(): Promise<void> {
    fleetLoading = true;
    try {
      fleetData = await api.fleet();
      fleetError = '';
    } catch (e) {
      fleetError = `GET /v1/fleet: ${e instanceof Error ? e.message : String(e)}`;
    }
    fleetLoading = false;
    if (alive && state.model === 'fleet') rebuild(true);
  }

  /** Builds the model in `state`. `reframe` also moves the camera to frame it. */
  function rebuild(reframe: boolean): void {
    if (current) {
      scene.remove(current.built.group);
      disposeTree(current.built.group);
      current = null;
    }
    if (fleetScene) {
      scene.remove(fleetScene.group);
      disposeTree(fleetScene.group);
      fleetScene = null;
      fleetView = null;
    }
    if (state.model === 'fleet') {
      if (!fleetAsked) {
        fleetAsked = true;
        void loadFleet();
      }
      fleetView = withFake(fleetData ?? NO_FLEET, state.fake);
      fleetScene = buildFleet(fleetView, PALETTE);
      scene.add(fleetScene.group);
      lastErrors = fleetError ? [fleetError] : [];
      lastInfo = fleetLoading ? 'fleet: loading GET /v1/fleet' : `fleet: ${fleetView.machines.length} machines, ${fleetView.links.length} links`;
      if (reframe) frame(undefined, state.cam);
      renderErrors();
      return;
    }
    const model = MODELS[state.model];
    lastInfo = '';
    lastErrors = model ? validate(model, state.model) : [`no model named "${state.model}" (see the Model menu)`];
    if (model) {
      const built = buildModel(model, { ...state.params, gpus: state.gpus });
      let traces: THREE.Group | null = null;
      let pulses: Pulses | null = null;
      if (model.view === 'board') {
        traces = traceStrips(model);
        built.group.add(traces);
        pulses = currentPulses(model);
        built.group.add(pulses.group);
        // The test hook: n synthetic agents seated on the board, with their rooms (the same layout and room build as the Floor).
        if (state.rooms > 0) built.group.add(boardWithRooms(model, state.rooms, { ...state.params, gpus: state.gpus }).group);
      }
      const atom = placeAtom(model, built.group);
      scene.add(built.group);
      current = { model, built, traces, pulses, atom };
    }
    if (reframe) frame(model, state.cam);
    renderErrors();
  }

  rebuild(true);
  sync();

  // The route can change while the lab is open (a typed or pasted URL): take the new state.
  const onHash = (): void => {
    if (!location.hash.startsWith('#/hardware-lab')) return;
    const next = readState(location.hash);
    const changed = next.model !== state.model || next.gpus !== state.gpus || next.fake !== state.fake || !sameParams(next.params, state.params);
    const reframe = next.model !== state.model || next.cam !== state.cam;
    state = next;
    if (changed || reframe) rebuild(reframe);
    if (reframe) frame(current?.model, state.cam);
    sync();
  };
  window.addEventListener('hashchange', onHash);

  // ---- Frames --------------------------------------------------------------------------------
  const clock = new THREE.Clock();
  let time = 0;
  const meter = createMeter(renderer, 'lab', stage);
  renderer.setAnimationLoop(() => {
    if (!meter.ready()) return; // a hidden tab or an offscreen canvas does not draw
    const dt = Math.min(clock.getDelta(), 0.1);
    time += dt;
    if (current) {
      const { model, built, traces, pulses, atom } = current;
      built.update(dt, { status: state.status, working: state.working, load: Math.min(1, state.working / 4), open: state.open });
      if (traces) traces.visible = state.traces;
      if (pulses) {
        pulses.group.visible = state.traces;
        pulses.update(dt, Math.min(1, state.working / 4));
      }
      if (atom) {
        atom.animate(time);
        // An exterior's atom glows through its window only while the machine is online.
        atom.atom.visible = model.view !== 'exterior' || state.status === 'online';
      }
    }
    if (fleetScene && fleetView) fleetScene.update(dt, fleetView);
    controls.update();
    if (current) current.built.frame(camera, controls.target, dt);
    if (fleetScene) fleetScene.lod(camera, controls.target, dt);
    meter.render(scene, camera);
  });

  return () => {
    alive = false;
    window.removeEventListener('hashchange', onHash);
    renderer.setAnimationLoop(null);
    meter.dispose();
    ro.disconnect();
    controls.dispose();
    if (current) {
      scene.remove(current.built.group);
      disposeTree(current.built.group);
    }
    if (fleetScene) {
      scene.remove(fleetScene.group);
      disposeTree(fleetScene.group);
    }
    renderer.dispose();
  };
}
