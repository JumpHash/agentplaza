// The hardware lab: a developer screen for the hardware models under assets/hardware/. It shows one
// model under the Floor's lighting, with its state controls and its validation errors in red.
// Route: #/hardware-lab?model=exterior/workstation&gpus=1&status=online&working=0&open=0&traces=1&cam=iso
// (cam: iso | top | front | side | anchor). The URL is kept in step with the controls.

import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { h } from '../ui/dom';
import type { Cleanup } from './index';
import { MODELS } from './floor3d/hardware-index';
import { buildModel, currentPulses, placeAtom, traceMeshes, validate, type Built, type Model, type Pulses, type Status } from './floor3d/hardware';
import { sky, toonMat } from './floor3d/toon';

const STATUSES: string[] = ['online', 'draining', 'offline', 'revoked'];
const CAMS = ['iso', 'top', 'front', 'side', 'anchor'] as const;
type Cam = (typeof CAMS)[number];

interface LabState {
  model: string;
  gpus: number;
  status: Status;
  working: number;
  open: number;
  traces: boolean;
  cam: Cam;
}

const DEFAULT_STATE: LabState = { model: 'exterior/workstation', gpus: 0, status: 'online', working: 0, open: 0, traces: true, cam: 'iso' };

/** The lab's state from the route's query; a missing or bad value takes its default. */
function readState(hash: string): LabState {
  const q = new URLSearchParams(hash.split('?')[1] ?? '');
  const num = (key: string, lo: number, hi: number, fb: number): number => {
    const v = Number(q.get(key));
    return q.has(key) && Number.isFinite(v) ? Math.min(hi, Math.max(lo, v)) : fb;
  };
  const status = q.get('status') ?? '';
  const cam = q.get('cam') ?? '';
  return {
    model: q.get('model') ?? DEFAULT_STATE.model,
    gpus: Math.round(num('gpus', 0, 2, DEFAULT_STATE.gpus)),
    status: (STATUSES.includes(status) ? status : DEFAULT_STATE.status) as Status,
    working: Math.round(num('working', 0, 6, DEFAULT_STATE.working)),
    open: num('open', 0, 1, DEFAULT_STATE.open),
    traces: q.has('traces') ? q.get('traces') !== '0' : DEFAULT_STATE.traces,
    cam: (CAMS as readonly string[]).includes(cam) ? (cam as Cam) : DEFAULT_STATE.cam,
  };
}

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
    return new THREE.WebGLRenderer({ antialias: true });
  } catch {
    return null;
  }
}

export function mount(root: HTMLElement): Cleanup {
  let state = readState(location.hash);

  // ---- Controls -------------------------------------------------------------------------------
  const bar = { style: 'display:flex;flex-wrap:wrap;gap:12px 18px;align-items:center;padding:8px 0' };
  const field = (label: string, control: HTMLElement): HTMLElement => h('label', { style: 'display:flex;gap:6px;align-items:center' }, h('span', { style: 'font-size:12px;color:var(--ink-2)' }, label), control);
  const modelSel = h(
    'select',
    { 'aria-label': 'Model', onchange: () => { state.model = modelSel.value; rebuild(true); writeUrl(); sync(); } },
    ...Object.keys(MODELS).sort().map((k) => h('option', { value: k }, k)),
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
  const toolbar = h(
    'div',
    { style: 'display:flex;flex-direction:column;border-bottom:1px solid var(--rule);padding:0 12px' },
    h('div', { style: bar.style }, field('model', modelSel), h('span', { style: 'display:flex;gap:4px' }, ...gpuBtns), field('status', statusSel)),
    h('div', { style: bar.style }, field('working', workingIn), workingOut, field('open', openIn), openOut, field('traces (boards)', tracesIn)),
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
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
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
    errorsEl.textContent = errors.length ? errors.join('\n') : `${state.model}: no errors`;
    errorsEl.style.color = errors.length ? 'var(--crit)' : 'var(--ink-2)';
  }
  let lastErrors: string[] = [];

  /** Frames the model: the camera's preset (iso by default), at a distance that fits its size. */
  function frame(model: Model | undefined, cam: Cam): void {
    const [w, hgt, d] = model?.size ?? [4, 6, 4];
    const centre = new THREE.Vector3(0, hgt / 2, 0);
    if (cam === 'anchor' && model?.anchors?.camera) {
      camera.position.set(...model.anchors.camera.at);
      controls.target.set(...model.anchors.camera.look);
    } else {
      const radius = Math.max(Math.hypot(w, hgt, d) / 2, 1);
      const dist = (radius / Math.sin(THREE.MathUtils.degToRad(camera.fov / 2))) * 0.9;
      const dir = { iso: [0.8, 0.6, 1], top: [0, 1, 0.0001], front: [0, 0, 1], side: [1, 0, 0], anchor: [0.8, 0.6, 1] }[cam];
      camera.position.copy(centre).add(new THREE.Vector3(...dir).normalize().multiplyScalar(dist));
      controls.target.copy(centre);
    }
    controls.update();
  }

  /** Builds the model in `state`. `reframe` also moves the camera to frame it. */
  function rebuild(reframe: boolean): void {
    if (current) {
      scene.remove(current.built.group);
      disposeTree(current.built.group);
      current = null;
    }
    const model = MODELS[state.model];
    lastErrors = model ? validate(model, state.model) : [`no model named "${state.model}" (see the Model menu)`];
    if (model) {
      const built = buildModel(model, { gpus: state.gpus });
      let traces: THREE.Group | null = null;
      let pulses: Pulses | null = null;
      if (model.view === 'board') {
        traces = traceMeshes(model);
        built.group.add(traces);
        pulses = currentPulses(model);
        built.group.add(pulses.group);
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
    const changed = next.model !== state.model || next.gpus !== state.gpus;
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
  renderer.setAnimationLoop(() => {
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
    controls.update();
    renderer.render(scene, camera);
  });

  return () => {
    window.removeEventListener('hashchange', onHash);
    renderer.setAnimationLoop(null);
    ro.disconnect();
    controls.dispose();
    if (current) {
      scene.remove(current.built.group);
      disposeTree(current.built.group);
    }
    renderer.dispose();
  };
}
