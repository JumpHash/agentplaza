// The robot lab: a developer screen for the robot species under assets/robots/. It shows one species
// (or a line-up of all of them) under the Floor's lighting, with any clip, tool, carried item and state.
// Route: #/robot-lab?species=rover&clip=type&tool=wrench&carry=crate&state=working&hue=200&orch=1&cam=iso
//   species: a species id, or all (the line-up). clip: a shared or species clip (empty: the state's own).
//   tool: a tool id for the grip; carry: an item id for the carry bone. state: idle | working | walking | offline.
//   hue: 0–360 (the shell and the panel). orch=1: the rover as the orchestrator.
//   cam: front | iso | side | q34 (three-quarter, front-left, a little above) | top (looking down).
//   t: seconds. A frozen clock: each robot takes its pose at that time, once per change, and the animation stops.
//   bg: dark | light (the scene background; the default is a pale blue). sheet=1: the canvas alone, filling the
//   window (this view covers the console's chrome; the controls and the readout are not drawn).
//   prop: a station prop id (its spots, a robot at each playing the spot's clip), or all (the line-up of props).
// The page shows the draw calls each robot makes (the main pass, shadows off), from renderer.info.

import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { h } from '../ui/dom';
import type { Cleanup } from './index';
import { makeRobot, ROBOT_PALETTE, type Robot, type RobotState } from './floor3d/robot';
import { registry } from './floor3d/robots/files';
import { FX_KINDS, type FxKind } from './floor3d/robots/types';
import { buildProp, propRegistry, type BuiltProp } from './floor3d/robots/props';

const STATES: RobotState[] = ['idle', 'working', 'walking', 'offline'];
const CAMS = ['front', 'iso', 'side', 'q34', 'top'] as const;
type Cam = (typeof CAMS)[number];

/** The scene backgrounds of the capture sheets; the default (no bg) is the lab's pale blue. */
const BG = { dark: '#1b1a17', light: '#f3efe6' } as const;
const DEFAULT_BG = '#e9edf3';

interface LabState {
  species: string;
  clip: string;
  tool: string;
  carry: string;
  state: RobotState;
  hue: number;
  orch: boolean;
  cam: Cam;
  fx: FxKind | '';
  /** A prop id, or all (the line-up of every prop). Empty: the species screen. */
  prop: string;
  /** A frozen clock, in seconds; null for the live clock. */
  t: number | null;
  /** The scene background: dark, light, or '' for the default. */
  bg: '' | 'dark' | 'light';
  /** The canvas alone, filling the window. */
  sheet: boolean;
}

function readState(hash: string): LabState {
  const q = new URLSearchParams(hash.split('?')[1] ?? '');
  const state = q.get('state');
  const cam = q.get('cam');
  const hue = Number(q.get('hue') ?? 200);
  const tRaw = q.get('t') ?? '';
  const bg = q.get('bg');
  return {
    species: q.get('species') ?? 'all',
    clip: q.get('clip') ?? '',
    tool: q.get('tool') ?? '',
    carry: q.get('carry') ?? '',
    prop: q.get('prop') ?? '',
    state: STATES.includes(state as RobotState) ? (state as RobotState) : 'idle',
    hue: Number.isFinite(hue) ? ((hue % 360) + 360) % 360 : 200,
    orch: q.get('orch') === '1',
    cam: CAMS.includes(cam as Cam) ? (cam as Cam) : 'front',
    fx: FX_KINDS.includes(q.get('fx') as FxKind) ? (q.get('fx') as FxKind) : '',
    t: tRaw !== '' && Number.isFinite(Number(tRaw)) ? Math.max(0, Number(tRaw)) : null,
    bg: bg === 'dark' || bg === 'light' ? bg : '',
    sheet: q.get('sheet') === '1',
  };
}

function writeUrl(s: LabState): void {
  const q = new URLSearchParams();
  q.set('species', s.species);
  if (s.clip) q.set('clip', s.clip);
  if (s.tool) q.set('tool', s.tool);
  if (s.carry) q.set('carry', s.carry);
  if (s.prop) q.set('prop', s.prop);
  q.set('state', s.state);
  q.set('hue', String(Math.round(s.hue)));
  if (s.orch) q.set('orch', '1');
  q.set('cam', s.cam);
  if (s.fx) q.set('fx', s.fx);
  if (s.t !== null) q.set('t', String(s.t));
  if (s.bg) q.set('bg', s.bg);
  if (s.sheet) q.set('sheet', '1');
  history.replaceState(null, '', `#/robot-lab?${q.toString()}`);
}

/** The scene's background colour for a state. */
function bgColour(s: LabState): string {
  return s.bg === '' ? DEFAULT_BG : BG[s.bg];
}

const CAM_POS: Record<Cam, { lineup: [number, number, number]; one: [number, number, number] }> = {
  front: { lineup: [0, 1.5, 9.8], one: [0, 1.1, 4.4] },
  iso: { lineup: [5.0, 3.0, 8.0], one: [2.6, 2.0, 3.6] },
  side: { lineup: [0, 1.3, 11], one: [4.6, 1.1, 0] },
  // Three-quarter: 35 degrees round to the front-left, 15 degrees up, 4.4 from the robot's centre (as front).
  q34: { lineup: [-5.4, 4.4, 8.2], one: [-2.44, 1.84, 3.48] },
  // Looking down from 4.3 above the robot's centre (the same distance as front, a little toward the viewer).
  top: { lineup: [0, 11, 1.8], one: [0, 5.0, 0.8] },
};
/** The camera for a prop: the line-up of every prop is wider than a species line-up. */
const PROP_CAM: Record<Cam, { lineup: [number, number, number]; one: [number, number, number] }> = {
  front: { lineup: [0, 8, 17], one: [0, 1.7, 5.4] },
  iso: { lineup: [11, 9, 13], one: [3.4, 2.7, 4.8] },
  side: { lineup: [17, 5, 0], one: [5.6, 1.5, 0] },
  q34: { lineup: [-8.9, 7.8, 12.8], one: [-2.5, 2.6, 3.56] },
  top: { lineup: [0, 18, 0.5], one: [0, 5.6, 0.9] },
};

export function mount(root: HTMLElement): Cleanup {
  const reg = registry();
  const propReg = propRegistry();
  let state = readState(location.hash);

  // ---- Controls ------------------------------------------------------------------------------
  const bar = 'display:flex;flex-wrap:wrap;gap:10px 16px;align-items:center;padding:8px 0';
  const field = (label: string, control: HTMLElement): HTMLElement =>
    h('label', { style: 'display:flex;gap:6px;align-items:center' }, h('span', { style: 'font-size:12px;color:var(--ink-2)' }, label), control);
  const options = (values: string[], none?: string): HTMLElement[] => [
    ...(none !== undefined ? [h('option', { value: '' }, none)] : []),
    ...values.map((v) => h('option', { value: v }, v)),
  ];

  const speciesSel = h('select', { 'aria-label': 'Species', onchange: () => { state.species = speciesSel.value; state.clip = ''; rebuild(); sync(); } }, ...options(['all', ...reg.index.species]));
  const clipSel = h('select', { 'aria-label': 'Clip', onchange: () => { state.clip = clipSel.value; applyAll(); sync(); } });
  const toolSel = h('select', { 'aria-label': 'Tool', onchange: () => { state.tool = toolSel.value; applyAll(); sync(); } }, ...options(Object.keys(reg.tools), '(no tool)'));
  const carrySel = h('select', { 'aria-label': 'Carry', onchange: () => { state.carry = carrySel.value; applyAll(); sync(); } }, ...options(Object.keys(reg.tools).filter((id) => reg.tools[id]!.grip === 'carry'), '(nothing)'));
  const propSel = h('select', { 'aria-label': 'Prop', onchange: () => { state.prop = propSel.value; rebuild(); sync(); } }, ...options(['all', ...propReg.order], '(no prop)'));
  const fxSel = h('select', { 'aria-label': 'Effect', onchange: () => { state.fx = fxSel.value as FxKind | ''; sync(); } }, ...options([...FX_KINDS], '(no effect)'));
  const stateSel = h('select', { 'aria-label': 'State', onchange: () => { state.state = stateSel.value as RobotState; applyAll(); sync(); } }, ...STATES.map((s) => h('option', { value: s }, s)));
  const hueIn = h('input', { type: 'range', min: 0, max: 360, step: 1, 'aria-label': 'Hue', oninput: () => { state.hue = Number(hueIn.value); rebuild(); sync(); } });
  const orchIn = h('input', { type: 'checkbox', 'aria-label': 'Orchestrator', onchange: () => { state.orch = orchIn.checked; rebuild(); sync(); } });
  const camBtns = CAMS.map((c) => h('button', { type: 'button', class: 'btn', onclick: () => { state.cam = c; frame(); sync(); } }, c));
  const stats = h('div', { style: 'font-family:var(--mono);font-size:12px;color:var(--ink-2);padding:4px 0', 'aria-live': 'polite' });

  const controls = h(
    'div',
    { style: bar },
    field('Species', speciesSel),
    field('Prop', propSel),
    field('Clip', clipSel),
    field('Tool', toolSel),
    field('Carry', carrySel),
    field('State', stateSel),
    field('Effect', fxSel),
    field('Hue', hueIn),
    field('Orchestrator', orchIn),
    ...camBtns,
  );
  const canvasBox = h('div', { style: state.sheet ? 'width:100%;height:100%' : 'width:100%;height:420px;border-radius:12px;overflow:hidden' });
  root.append(h('div', { style: state.sheet ? 'height:100%' : 'padding:12px 16px' }, controls, stats, canvasBox));
  if (state.sheet) {
    // The sheet: this view covers the console's chrome (the sidebar and the top bar sit under it).
    root.style.cssText = `position:fixed;inset:0;z-index:2147483000;margin:0;padding:0;overflow:hidden;background:${bgColour(state)}`;
    controls.style.display = 'none';
    stats.style.display = 'none';
  }

  // ---- Renderer ------------------------------------------------------------------------------
  let made: THREE.WebGLRenderer | null = null;
  try {
    made = new THREE.WebGLRenderer({ antialias: true });
    made.debug.checkShaderErrors = false; // the info log is read only when a shader fails
  } catch {
    made = null;
  }
  if (!made) {
    stats.textContent = 'WebGL is not available here.';
    return () => {};
  }
  const renderer = made;
  renderer.shadowMap.enabled = true;
  renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
  canvasBox.append(renderer.domElement);
  const scene = new THREE.Scene();
  scene.background = new THREE.Color('#e9edf3');
  scene.add(new THREE.HemisphereLight(0xeaf4ff, 0x8a7a5a, 1.3));
  const key = new THREE.DirectionalLight(0xfff1d6, 2.6);
  key.position.set(2.5, 4, 3);
  key.castShadow = true;
  scene.add(key);
  const floor = new THREE.Mesh(new THREE.CircleGeometry(4.5, 48), new THREE.ShadowMaterial({ opacity: 0.2 }));
  floor.rotation.x = -Math.PI / 2;
  floor.receiveShadow = true;
  scene.add(floor);
  const camera = new THREE.PerspectiveCamera(30, 1, 0.1, 80);
  const controlsOrbit = new OrbitControls(camera, renderer.domElement);
  controlsOrbit.enableDamping = true;

  /** The robots on screen. `clip` is a prop spot's clip (played unless the state names one). */
  let robots: { id: string; robot: Robot; clip?: string }[] = [];
  let propsOnScreen: BuiltProp[] = [];
  /** A frozen clock poses the robots once per change (set by applyAll), not every frame. */
  let frozenDirty = true;

  /** The robots on screen: one species, or every species in a line-up. */
  const speciesOnScreen = (): string[] => (state.species === 'all' ? reg.index.species : [state.species]);

  const clipOptions = (): string[] => {
    const own = reg.species[state.species === 'all' ? '' : state.species]?.clips ?? {};
    return [...new Set([...Object.keys(reg.clips), ...Object.keys(own)])].sort();
  };

  /** The background: the scene's, and the sheet's cover when the view is the sheet. */
  function paint(): void {
    const colour = bgColour(state);
    scene.background = new THREE.Color(colour);
    if (state.sheet) root.style.background = colour;
  }

  function applyAll(): void {
    paint();
    frozenDirty = true;
    for (const { robot, clip } of robots) {
      robot.setState(state.state);
      robot.play(state.clip !== '' ? state.clip : (clip ?? null), { loop: true });
      robot.hold(state.tool === '' ? null : state.tool);
      robot.carry(state.carry === '' ? null : state.carry);
    }
  }

  /** The species screen: one species, or the line-up of every species. */
  function buildSpecies(): void {
    const ids = speciesOnScreen();
    ids.forEach((id, i) => {
      const orch = state.orch && id === 'rover' && (state.species !== 'all' || i === 0);
      const robot = makeRobot(state.hue, orch, ROBOT_PALETTE, state.hue, id);
      robot.group.position.set((i - (ids.length - 1) / 2) * 2.2, 0, 0);
      robot.group.traverse((o) => (o.castShadow = true));
      scene.add(robot.group);
      robots.push({ id, robot });
    });
  }

  /** The prop screen: one prop, or a line-up of every prop. Each spot gets a robot facing it. */
  function buildProps(): void {
    const lineup = state.prop === 'all';
    const ids = lineup ? propReg.order : [state.prop];
    const cols = lineup ? 6 : 1;
    const rows = Math.ceil(ids.length / cols);
    let dealt = 0;
    ids.forEach((pid, i) => {
      const prop = propReg.props[pid];
      if (!prop) return;
      const col = i % cols;
      const row = Math.floor(i / cols);
      const ox = lineup ? (col - (cols - 1) / 2) * 2.6 : 0;
      const oz = lineup ? ((rows - 1) / 2 - row) * 2.4 : 0;
      const built = buildProp(prop, ROBOT_PALETTE, state.hue);
      built.group.position.set(ox, 0, oz);
      built.group.traverse((o) => (o.castShadow = true));
      scene.add(built.group);
      propsOnScreen.push(built);
      for (const s of built.spots) {
        // The robots are the species in turn (or the one species named): a spot's robot plays the spot's clip.
        const id = state.species === 'all' ? reg.index.species[dealt % reg.index.species.length]! : state.species;
        dealt++;
        const robot = makeRobot(state.hue, false, ROBOT_PALETTE, state.hue, id);
        robot.group.position.set(ox + s.at[0], s.at[1], oz + s.at[2]);
        robot.group.rotation.y = (s.face * Math.PI) / 180;
        robot.group.traverse((o) => (o.castShadow = true));
        scene.add(robot.group);
        robots.push({ id, robot, clip: s.clip });
      }
    });
  }

  function rebuild(): void {
    for (const { robot } of robots) robot.dispose();
    robots = [];
    for (const p of propsOnScreen) p.dispose();
    propsOnScreen = [];
    if (state.prop !== '') buildProps();
    else buildSpecies();
    // The clip list follows the species: its own clips, then the shared ones.
    const clips = clipOptions();
    clipSel.replaceChildren(...options(clips, '(state clip)'));
    applyAll();
    frame();
    measure();
  }

  /** Draw calls per robot in the main pass: the others hidden, shadows off, one frame each. */
  function measure(): void {
    if (state.prop !== '') {
      // A prop screen is one scene: the draw calls of the whole frame, the floor hidden.
      const shadows = renderer.shadowMap.enabled;
      renderer.shadowMap.enabled = false;
      floor.visible = false;
      renderer.info.reset();
      renderer.render(scene, camera);
      const calls = renderer.info.render.calls;
      renderer.shadowMap.enabled = shadows;
      floor.visible = true;
      stats.textContent = `props: ${propsOnScreen.length} · robots: ${robots.length} · draw calls (main pass, whole frame): ${calls}`;
      return;
    }
    const shadows = renderer.shadowMap.enabled;
    renderer.shadowMap.enabled = false;
    floor.visible = false;
    const parts: string[] = [];
    for (const { id, robot } of robots) {
      for (const other of robots) other.robot.group.visible = other.robot === robot;
      renderer.info.reset();
      renderer.render(scene, camera);
      parts.push(`${id} ${renderer.info.render.calls}`);
    }
    for (const { robot } of robots) robot.group.visible = true;
    renderer.shadowMap.enabled = shadows;
    floor.visible = true;
    stats.textContent = `draw calls per robot (main pass): ${parts.join(' · ')}`;
  }

  function frame(): void {
    const lineup = state.prop !== '' ? state.prop === 'all' : state.species === 'all';
    const pos = (state.prop !== '' ? PROP_CAM : CAM_POS)[state.cam][lineup ? 'lineup' : 'one'];
    camera.position.set(...pos);
    const target = new THREE.Vector3(0, state.prop !== '' ? (lineup ? 0.5 : 0.6) : lineup ? 0.75 : 0.7, 0);
    camera.lookAt(target);
    controlsOrbit.target.copy(target);
    controlsOrbit.update();
  }

  /** Mirrors the state into the controls and the URL. */
  function sync(): void {
    speciesSel.value = state.species;
    propSel.value = state.prop;
    clipSel.value = state.clip;
    toolSel.value = state.tool;
    carrySel.value = state.carry;
    stateSel.value = state.state;
    fxSel.value = state.fx;
    hueIn.value = String(Math.round(state.hue));
    orchIn.checked = state.orch;
    camBtns.forEach((b, i) => b.setAttribute('aria-pressed', String(CAMS[i] === state.cam)));
    writeUrl(state);
  }

  // The route can change while the lab is open (a typed or pasted URL): take the new state.
  const onHash = (): void => {
    if (!location.hash.startsWith('#/robot-lab')) return;
    const next = readState(location.hash);
    const rebuildNeeded = next.species !== state.species || next.orch !== state.orch || next.hue !== state.hue || next.prop !== state.prop;
    state = next;
    if (rebuildNeeded) rebuild();
    else applyAll();
    frame();
    sync();
  };
  window.addEventListener('hashchange', onHash);

  const resize = (): void => {
    const w = state.sheet ? window.innerWidth : Math.max(320, canvasBox.clientWidth);
    const height = state.sheet ? window.innerHeight : 420;
    renderer.setSize(w, height, state.sheet);
    camera.aspect = w / height;
    camera.updateProjectionMatrix();
  };
  window.addEventListener('resize', resize);
  resize();

  rebuild();
  sync();

  // ---- Frames --------------------------------------------------------------------------------
  const clock = new THREE.Clock();
  let time = 0;
  let fxTimer = 0;
  renderer.setAnimationLoop(() => {
    const frozenT = state.t;
    if (frozenT !== null) {
      // A frozen clock: the pose at that time. A blend of 1 snaps each joint to its target, and the
      // update runs once per change, so the spins and the rates do not drift from frame to frame.
      if (frozenDirty) {
        for (const { robot } of robots) robot.update(1, frozenT);
        frozenDirty = false;
      }
    } else {
      const dt = Math.min(clock.getDelta(), 0.1);
      time += dt;
      for (const { robot } of robots) robot.update(dt, time);
      if (state.fx) {
        fxTimer += dt;
        if (fxTimer > 0.7) {
          fxTimer = 0;
          for (const { robot } of robots) robot.fx(state.fx as FxKind);
        }
      }
    }
    controlsOrbit.update();
    renderer.render(scene, camera);
  });

  return () => {
    window.removeEventListener('hashchange', onHash);
    window.removeEventListener('resize', resize);
    renderer.setAnimationLoop(null);
    for (const { robot } of robots) robot.dispose();
    for (const p of propsOnScreen) p.dispose();
    controlsOrbit.dispose();
    renderer.dispose();
    renderer.domElement.remove();
  };
}
