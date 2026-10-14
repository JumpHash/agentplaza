import { api, type Agent, type FleetResponse } from '../api';
import { floorName } from '../roles';
import { on } from '../store';
import { button } from '../ui/components';
import { h } from '../ui/dom';
import type { Cleanup } from './index';
import { machineCard } from './floor-popup';

/** The wasm glue's file name at the dist root: esbuild replaces this at build time (null when the wasm crate is missing). */
declare const __APZ_ENGINE_JS__: string | null;

/** The glue's file name, or '' when the build has no engine (typeof is safe even if the define is absent). */
function glueName(): string {
  return typeof __APZ_ENGINE_JS__ === 'string' ? __APZ_ENGINE_JS__ : '';
}

/** One label of the shown view, as the engine reports it when its text changes (`on_labels`). */
interface LabelMeta {
  cid: number;
  name: string;
  role: string;
  line: string;
  hue: number;
}

/** The surface of the wasm engine this mount uses (the crate's WebEngine, wasm-bindgen --target web). */
export interface WebEngine {
  /** Seconds since ENTER began (the entry clock), or undefined when no sequence runs. */
  entry_time: () => number | undefined;
  backend(): string;
  set_feed(json: string): void;
  /** One event from the console's stream: its channel and its data as JSON (the desktop forwards the same). */
  event(kind: string, json: string): void;
  /** The shared board layouts the engine asked for since the last call (a JSON array of keys), for the layout worker. */
  layout_requests(): string;
  /** A shared board layout the layout worker computed: its key and the worker's answer, both JSON. */
  provide_layout(key: string, answer: string): void;
  command(name: string, arg?: string | null): void;
  on_pick(cb: (kind: 'machine' | 'robot' | 'none', id: string) => void): void;
  on_labels(cb: (labels: LabelMeta[]) => void): void;
  label_positions(): Float32Array;
  screen_of(kind: 'machine' | 'robot', id: string): Float32Array;
  busy(): boolean;
  frames_drawn(): number;
  draw_calls(): number;
  draw_breakdown(): Uint32Array;
  draw_stats(): string;
  /** The lab's readout, JSON: the room lab's violations or the hardware lab's model validation; `{}` when no lab runs. */
  lab_info(): string;
  /** The robot lab's draw counts of the last frame's main pass, JSON (`props`, `robots` with `species` and `calls`, `calls`); `{}` when no robot lab runs. */
  lab_draws(): string;
  /** The shown view: `machines` or `floor` (as the three.js build's __apzPerf named it). */
  view_name(): string;
  /** The shown view's camera, JSON: { view, eye, target, fov, roomsTop }. */
  camera_json(): string;
  /** The shown view's scene graph, JSON (`__apzScene`). */
  scene_json(): string;
  /** The grounding audit of the board, JSON: { items, findings: [{ item, kind, amount, at, against }] } or null. */
  ground_audit(): string;
  /** The floor's robots, JSON (see the engine's debug hooks). */
  bots_json(): string;
  /** The nav grid's cell at (x, z), JSON or "null". */
  nav_cell(x: number, z: number): string;
  /** The board's ground audit, JSON or "null". */
  nav_audit(): string;
  /** The board's lifts, JSON. */
  lifts(): string;
  /** A scripted lift ride (`__apzLiftRide`): the n robots nearest lift i sent through it, JSON. */
  lift_ride(index: number, n: number): string;
  /** Moves an agent's robot on the board; false without a robot for it. */
  shove(cid: number, x: number, z: number): boolean;
  look_at(x: number, y: number, z: number, dist: number, polar: number, azimuth: number): void;
  thumbnail(id: string, w: number, h: number): Promise<Uint8Array>;
  portrait(json: string, w: number, h: number): Promise<Uint8Array>;
  destroy(): void;
}

/** The glue module: its default export loads the .wasm; `start` builds the engine on a canvas. */
/** The robot lab's options, as the route names them; each maps to its APZ_ROBOT_* name in the engine. Unset ones take the lab's defaults. */
export interface LabOptions {
  species?: string;
  clip?: string;
  tool?: string;
  carry?: string;
  state?: string;
  hue?: number;
  orch?: boolean;
  cam?: string;
  t?: number;
  bg?: string;
  prop?: string;
  fx?: string;
}

/** The room lab's options, as the room lab reads its route; each maps to its APZ_ROOM_* name in the engine (`size` is WxD in units). Unset ones take the lab's defaults. */
export interface RoomLabOptions {
  role?: string;
  size?: string;
  seed?: number;
  door?: string;
  grid?: boolean;
  cam?: string;
  debug?: boolean;
}

/** The hardware lab's options, as the route names them; each maps to its APZ_HW_* name in the engine. `params` are the
 *  model's own numbers (APZ_HW_PARAMS). Unset ones take the lab's defaults. */
export interface HardwareLabOptions {
  model?: string;
  gpus?: number;
  status?: string;
  working?: number;
  open?: number;
  traces?: boolean;
  cam?: string;
  params?: Record<string, number>;
}

/** The labs' option lists, from the engine (`lab_catalog`): the robot species and their clips, tools, carried items,
 *  props and effects; the room roles; the hardware models, with each model's params (their defaults, without the GPUs). */
export interface LabCatalog {
  species: string[];
  /** The shared clip ids. */
  clips: string[];
  /** Each species' own clip ids. */
  speciesClips: Record<string, string[]>;
  tools: string[];
  /** The tools whose grip is carry. */
  carry: string[];
  props: string[];
  fx: string[];
  roles: string[];
  models: string[];
  params: Record<string, Record<string, number>>;
}

interface EngineGlue {
  default: () => Promise<unknown>;
  lab_catalog: () => string;
  start: (canvas: HTMLCanvasElement, opts: { backend?: 'auto' | 'webgl2'; nav?: boolean }) => Promise<WebEngine>;
  start_lab: (canvas: HTMLCanvasElement, opts: LabOptions & { backend?: 'auto' | 'webgl2' }) => Promise<WebEngine>;
  start_room_lab: (canvas: HTMLCanvasElement, opts: RoomLabOptions & { backend?: 'auto' | 'webgl2' }) => Promise<WebEngine>;
  start_hardware_lab: (canvas: HTMLCanvasElement, opts: HardwareLabOptions & { backend?: 'auto' | 'webgl2' }) => Promise<WebEngine>;
}

/** The same cadence as the DOM Machines view and the desktop. */
const FLEET_POLL_MS = 5000;
const AGENTS_POLL_MS = 4000;

/** True when this build has the engine's glue: the Rust engine draws every 3D view (there is no other renderer). */
export function engineWanted(): boolean {
  return glueName() !== '';
}

/** The glue, loaded and initialised. Throws without a glue (this build has no engine). */
async function loadGlue(): Promise<EngineGlue> {
  const name = glueName();
  if (name === '') throw new Error('this build has no engine');
  const glueUrl = new URL('./' + name, document.baseURI).href;
  const glue = (await import(/* @vite-ignore */ glueUrl)) as EngineGlue;
  await glue.default();
  return glue;
}

let catalog: Promise<LabCatalog> | null = null;

/** The labs' option lists, read once from the engine's glue. Rejects without a glue (this build has no engine). */
export function loadLabCatalog(): Promise<LabCatalog> {
  if (!catalog) {
    catalog = loadGlue().then((glue) => JSON.parse(glue.lab_catalog()) as LabCatalog);
    catalog.catch(() => {
      catalog = null;
    });
  }
  return catalog;
}

/** The backend the page asks for: webgl2 with ?gl=1, else auto. */
const backendOf = (): 'auto' | 'webgl2' => (new URLSearchParams(location.search).get('gl') === '1' ? 'webgl2' : 'auto');

/** The navigation overlay the page asks for: on with ?nav=1. */
const navOf = (): boolean => new URLSearchParams(location.search).get('nav') === '1';

/**
 * The page's layout worker: the same glue in a module worker, computing the shared board layouts (apz-board's layout of a
 * board for a roster, seconds of work) off the page's thread. The engine has no threads in the browser: it lists the
 * layouts it wants, the pump posts each key here, and the answer goes back to the engine that asked.
 */
let layoutWorker: Worker | null = null;
/** The open requests by key: each answer goes to every waiter (an engine's cache, or a page's promise). */
const layoutAsks = new Map<string, ((answer: string) => void)[]>();

/** Hands an answer to a key's waiters. */
function answerLayout(key: string, answer: string): void {
  const waiters = layoutAsks.get(key) ?? [];
  layoutAsks.delete(key);
  for (const w of waiters) {
    try {
      w(answer);
    } catch {
      // A destroyed engine: the next engine asks again.
    }
  }
}

/** Posts a key to the worker (started on the first request), or fails it when the build has none. */
function postLayout(key: string, waiter: (answer: string) => void): void {
  const waiters = layoutAsks.get(key);
  if (waiters) {
    waiters.push(waiter);
    return;
  }
  layoutAsks.set(key, [waiter]);
  layoutWorker ??= startLayoutWorker();
  if (layoutWorker) layoutWorker.postMessage(key);
  else failLayout(key, 'this build has no layout worker');
}

/**
 * The shared board layout (apz-board's, the one the engine lays its boards out with) of a key `{kind, gpus, agents:
 * [[cid, role], ...]}` (the engine's SharedKey; agents sorted by cid, the orchestrator's role `orchestrator`), computed
 * in the layout worker. Resolves to the layout's JSON, or null when it fails.
 */
export function requestLayout(key: { kind: string; gpus: number; agents: [number, string][] }): Promise<unknown> {
  return new Promise((resolve) => {
    postLayout(JSON.stringify(key), (answer) => {
      try {
        const parsed = JSON.parse(answer) as { ok?: unknown };
        resolve(parsed.ok ?? null);
      } catch {
        resolve(null);
      }
    });
  });
}

/** The layout worker's file name at the dist root (build.mjs writes it next to the glue); '' without an engine. */
declare const __APZ_LAYOUT_WORKER__: string | null;
const layoutWorkerName = (): string => (typeof __APZ_LAYOUT_WORKER__ === 'string' ? __APZ_LAYOUT_WORKER__ : '');

/** Answers a request with an error: the engine keeps its own layout for that board rather than waiting on it. */
function failLayout(key: string, reason: string): void {
  answerLayout(key, JSON.stringify({ err: reason }));
}

function startLayoutWorker(): Worker | null {
  const name = layoutWorkerName();
  if (name === '') return null;
  const worker = new Worker(new URL('./' + name, document.baseURI).href, { type: 'module' });
  worker.onmessage = (e: MessageEvent<{ key: string; answer: string }>) => answerLayout(e.data.key, e.data.answer);
  // A worker that cannot start (or dies) answers every open request with an error, and the next request starts another.
  worker.onerror = (e) => {
    e.preventDefault();
    for (const key of [...layoutAsks.keys()]) failLayout(key, `the layout worker failed: ${e.message || 'no message'}`);
    worker.terminate();
    if (layoutWorker === worker) layoutWorker = null;
  };
  return worker;
}

/** Posts an engine's layout requests to the layout worker every LAYOUT_PUMP_MS until the returned stop is called. */
const LAYOUT_PUMP_MS = 200;
function pumpLayouts(engine: WebEngine): () => void {
  let stopped = false;
  const timer = window.setInterval(() => {
    let keys: unknown[];
    try {
      keys = JSON.parse(engine.layout_requests()) as unknown[];
    } catch {
      return;
    }
    for (const k of keys) {
      const key = JSON.stringify(k);
      postLayout(key, (answer) => {
        if (!stopped) engine.provide_layout(key, answer);
      });
    }
  }, LAYOUT_PUMP_MS);
  return () => {
    window.clearInterval(timer);
    stopped = true;
  };
}

/** Loads the glue and starts an engine on the canvas: the Floor's mount and the Machines rows share it. Throws without a glue. */
export async function startEngine(canvas: HTMLCanvasElement): Promise<WebEngine> {
  const engine = await (await loadGlue()).start(canvas, { backend: backendOf(), nav: navOf() });
  const stop = pumpLayouts(engine);
  const destroy = engine.destroy.bind(engine);
  engine.destroy = () => {
    stop();
    destroy();
  };
  return engine;
}

/** The robot lab on the wasm engine: the route's options under the engine's APZ_ROBOT_* names. Throws without a glue. */
export async function startLabEngine(canvas: HTMLCanvasElement, lab: LabOptions): Promise<WebEngine> {
  return (await loadGlue()).start_lab(canvas, { ...lab, backend: backendOf() });
}

/** The room lab on the wasm engine: the route's options under the engine's APZ_ROOM_* names. Throws without a glue. */
export async function startRoomLabEngine(canvas: HTMLCanvasElement, lab: RoomLabOptions): Promise<WebEngine> {
  return (await loadGlue()).start_room_lab(canvas, { ...lab, backend: backendOf() });
}

/** The hardware lab on the wasm engine: the route's options under the engine's APZ_HW_* names. Throws without a glue. */
export async function startHardwareLabEngine(canvas: HTMLCanvasElement, lab: HardwareLabOptions): Promise<WebEngine> {
  return (await loadGlue()).start_hardware_lab(canvas, { ...lab, backend: backendOf() });
}

/** A lab's engine in a box under its controls. `restart` destroys the engine and starts one with the new options on a
 *  fresh canvas in the same box; `onStarted` runs for each engine and returns its cleanup (the lab's debug hooks). The
 *  starts run one after another (the engine reads its options while it starts), and a start that a restart overtook
 *  is destroyed as it lands. */
export interface LabHost<O> {
  restart(opts: O): void;
  /** The live engine, or null while one starts. */
  engine(): WebEngine | null;
  close(): void;
}

/** A lab canvas's default size: most of the window's height, never under 420 pixels. */
export const LAB_CANVAS = 'display:block;width:100%;height:70vh;min-height:420px;touch-action:none';

export function mountLabEngine<O>(
  root: HTMLElement,
  start: (canvas: HTMLCanvasElement, opts: O) => Promise<WebEngine>,
  initial: O,
  onStarted: (engine: WebEngine) => () => void = () => () => {},
  canvasStyle: (opts: O) => string = () => LAB_CANVAS,
): LabHost<O> {
  const box = document.createElement('div');
  root.append(box);
  let live: WebEngine | null = null;
  let unhook: () => void = () => {};
  let turn = 0;
  let closed = false;
  let chain: Promise<void> = Promise.resolve();

  const drop = (): void => {
    unhook();
    unhook = () => {};
    const old = live;
    live = null;
    old?.destroy();
  };
  const boot = (opts: O): void => {
    const mine = ++turn;
    chain = chain.then(async () => {
      if (closed || mine !== turn) return;
      const canvas = document.createElement('canvas');
      canvas.style.cssText = canvasStyle(opts);
      box.replaceChildren(canvas);
      try {
        const started = await start(canvas, opts);
        if (closed || mine !== turn) {
          started.destroy();
          return;
        }
        live = started;
        // The running engine, for developers (as the Floor's): its `lab_info()` is the lab's readout.
        (window as unknown as { apzEngine?: WebEngine }).apzEngine = started;
        unhook = onStarted(started);
      } catch (err) {
        if (closed || mine !== turn) return;
        const msg = err instanceof Error ? err.message : String(err);
        box.replaceChildren(h('p', { style: 'color:var(--ink-2)' }, `The engine did not start: ${msg}`));
      }
    });
  };
  boot(initial);
  return {
    restart(opts: O): void {
      if (closed) return;
      drop();
      boot(opts);
    },
    engine: () => live,
    close(): void {
      closed = true;
      drop();
      box.remove();
    },
  };
}

/** A thumbnail request a destroyed engine never answered: the caller asks again on the engine that replaces it. */
export const ENGINE_STOPPED = 'engine stopped';
export const isEngineStopped = (e: unknown): boolean => e instanceof Error && e.message === ENGINE_STOPPED;

/** One engine on a page: a machine's live orbit for a Machines row, or the thumbnails alone (no machine on show). */
export interface EngineStage {
  /** An agent's robot portrait from the engine: the JSON spec, w by h, RGBA8. Rejects ENGINE_STOPPED once destroyed. */
  portrait(json: string, w: number, h: number): Promise<Uint8Array>;
  /** The engine's RGBA8 thumbnail of a machine, w by h (each side at most 512). Rejects ENGINE_STOPPED once destroyed. */
  thumbnail(id: string, w: number, h: number): Promise<Uint8Array>;
  /** Stops the engine and the fleet poll. The caller removes the canvas. */
  destroy(): void;
}

/**
 * Starts the engine on `canvas`, fed the fleet like the Floor. With a `machineId`, shows that machine (`show_machine`)
 * once the fleet lists it; `onFirstFrame` (optional) runs once, when the engine has drawn its first frame. One engine
 * per page: the caller destroys it before it starts another.
 */
export function startEngineStage(canvas: HTMLCanvasElement, machineId: string | null, onFirstFrame?: () => void): EngineStage {
  let closed = false;
  let engine: WebEngine | null = null;
  let fleet: FleetResponse | null = null;
  let fleetError: string | null = null;
  let agents: Agent[] | null = null;
  let shown = false;
  let raf = 0;
  const timers: number[] = [];
  let markFleet: () => void = () => undefined;
  const fleetSeen = new Promise<void>((resolve) => {
    markFleet = () => resolve();
  });
  let stopNow: () => void = () => undefined;
  const stopped = new Promise<never>((_, reject) => {
    stopNow = () => reject(new Error(ENGINE_STOPPED));
  });
  stopped.catch(() => undefined);

  const pushFeed = (): void => {
    engine?.set_feed(JSON.stringify({ fleet, agents, fleet_error: fleetError }));
  };
  const tryShow = (): void => {
    if (machineId === null || shown || engine === null || fleet === null) return;
    if (!fleet.machines.some((x) => x.id === machineId)) return; // show_machine finds nothing until the fleet lists it
    shown = true;
    engine.command('show_machine', machineId);
  };
  const poll = (): void => {
    api.fleet().then(
      (f) => {
        fleet = f;
        fleetError = null;
        pushFeed();
        tryShow();
        markFleet();
      },
      (e: unknown) => {
        fleetError = e instanceof Error ? e.message : String(e); // keep the last fleet
        pushFeed();
      },
    );
    api.agents().then(
      (list) => {
        agents = list;
        pushFeed();
      },
      () => undefined, // keep the last list until the next poll
    );
  };
  const watch = (): void => {
    raf = 0;
    if (closed || engine === null || onFirstFrame === undefined) return;
    if (engine.frames_drawn() > 0) {
      onFirstFrame();
      return;
    }
    raf = requestAnimationFrame(watch);
  };

  const boot = async (): Promise<void> => {
    const started = await startEngine(canvas);
    if (closed) {
      started.destroy();
      return;
    }
    engine = started;
    pushFeed();
    poll();
    timers.push(window.setInterval(poll, FLEET_POLL_MS));
    if (onFirstFrame !== undefined) raf = requestAnimationFrame(watch);
  };
  boot().catch((e: unknown) => console.error('engine stage did not start', e));

  return {
    thumbnail(id, w, h) {
      return Promise.race([fleetSeen, stopped])
        .then(() => {
          if (closed || engine === null) throw new Error(closed ? ENGINE_STOPPED : 'the engine did not start');
          return engine.thumbnail(id, w, h);
        })
        .catch((e: unknown) => {
          throw closed ? new Error(ENGINE_STOPPED) : e;
        });
    },
    portrait(json, w, h) {
      return Promise.race([fleetSeen, stopped])
        .then(() => {
          if (closed || engine === null) throw new Error(closed ? ENGINE_STOPPED : 'the engine did not start');
          return engine.portrait(json, w, h);
        })
        .catch((e: unknown) => {
          throw closed ? new Error(ENGINE_STOPPED) : e;
        });
    },
    destroy() {
      closed = true;
      stopNow();
      for (const t of timers) window.clearInterval(t);
      if (raf !== 0) cancelAnimationFrame(raf);
      raf = 0;
      const e = engine;
      engine = null;
      e?.destroy();
    },
  };
}

/** The page's thumbnail engine: a hidden 1 by 1 canvas, fed the fleet, no machine on show. The caller destroys it. */
export function startThumbEngine(): EngineStage {
  const canvas = document.createElement('canvas');
  canvas.width = 1;
  canvas.height = 1;
  canvas.style.cssText = 'position:fixed;left:0;top:0;width:1px;height:1px;opacity:0;pointer-events:none';
  document.body.append(canvas);
  const stage = startEngineStage(canvas, null);
  return {
    thumbnail: (id, w, h) => stage.thumbnail(id, w, h),
    portrait: (json, w, h) => stage.portrait(json, w, h),
    destroy() {
      stage.destroy();
      canvas.remove();
    },
  };
}

/** The legend's keys: the Machines view's statuses, and a board's robot states (the floor.css swatch colours). */
const FLEET_KEYS: [string, string][] = [
  ['online', 'Online'],
  ['draining', 'Draining'],
  ['offline', 'Offline'],
  ['revoked', 'Revoked'],
];
const BOARD_KEYS: [string, string][] = [
  ['working', 'Working'],
  ['idle', 'Idle'],
  ['offline', 'Offline'],
];

/** A PerfSnapshot of a running engine (the three.js build's window.__apzPerf shape), plus the engine's own counts. */
/** A node of `window.__apzScene`: three's Object3D fields the retired build's scene was read by, and its walk. */
export interface SceneNode {
  type: string;
  name: string;
  visible: boolean;
  position: { x: number; y: number; z: number; toArray(): number[] };
  rotation: { x: number; y: number; z: number; toArray(): number[] };
  scale: { x: number; y: number; z: number; toArray(): number[] };
  castShadow: boolean;
  receiveShadow: boolean;
  renderOrder: number;
  userData: Record<string, unknown>;
  children: SceneNode[];
  parent: SceneNode | null;
  traverse(fn: (o: SceneNode) => void): void;
  getObjectByName(name: string): SceneNode | undefined;
}

function sceneNode(json: Record<string, unknown>, parent: SceneNode | null): SceneNode {
  const vec = (v: unknown): SceneNode['position'] => {
    const [x = 0, y = 0, z = 0] = (v as number[] | undefined) ?? [];
    return { x, y, z, toArray: () => [x, y, z] };
  };
  const node = {
    visible: true,
    castShadow: false,
    receiveShadow: false,
    renderOrder: 0,
    ...json,
    userData: (json.userData as Record<string, unknown> | undefined) ?? {},
    position: vec(json.position),
    rotation: vec(json.rotation),
    scale: vec(json.scale),
    parent,
    children: [] as SceneNode[],
    traverse(fn: (o: SceneNode) => void): void {
      fn(node);
      for (const c of node.children) c.traverse(fn);
    },
    getObjectByName(name: string): SceneNode | undefined {
      let hit: SceneNode | undefined;
      node.traverse((o) => {
        if (!hit && o.name === name) hit = o;
      });
      return hit;
    },
  } as unknown as SceneNode;
  node.children = ((json.children as Record<string, unknown>[] | undefined) ?? []).map((c) => sceneNode(c, node));
  return node;
}

/**
 * `window.__apzScene` as the three.js build had it: a property holding the shown view's scene (an object with `type`,
 * `children`, `traverse` …, not a function), read from the engine's scene graph each time it is read. Returns the
 * uninstall.
 */
export function installSceneHook(engine: WebEngine): () => void {
  Object.defineProperty(window, '__apzScene', {
    configurable: true,
    enumerable: true, // listed with the other hooks, as the three.js build's assignment was
    get: () => {
      try {
        return sceneNode(JSON.parse(engine.scene_json()) as Record<string, unknown>, null);
      } catch {
        return null;
      }
    },
  });
  return () => {
    delete (window as unknown as Record<string, unknown>).__apzScene;
  };
}

export function perfProbe(engine: WebEngine, view: () => string): () => Record<string, unknown> {
  const round2 = (v: number): number => Math.round(v * 100) / 100;
  let sample = { at: performance.now(), frames: engine.frames_drawn() }; // the last fps sample
  let seen = sample.frames; // the frames drawn at the previous call
  let fps = 0;
  return () => {
    const now = performance.now();
    const frames = engine.frames_drawn();
    const drawing = frames > seen;
    seen = frames;
    const elapsed = now - sample.at;
    if (elapsed >= 250) {
      fps = (frames - sample.frames) / (elapsed / 1000); // under 250 ms the previous fps stands
      sample = { at: now, frames };
    }
    const frameMs = fps > 0 ? 1000 / fps : 0;
    let s: Record<string, unknown>;
    try {
      s = JSON.parse(engine.draw_stats()) as Record<string, unknown>;
    } catch {
      s = {};
    }
    return {
      view: view(),
      fps: round2(fps),
      frameMs: round2(frameMs),
      calls: engine.draw_calls(),
      triangles: (s.triangles as number | undefined) ?? 0,
      geometries: (s.geometries as number | undefined) ?? 0,
      textures: (s.textures as number | undefined) ?? 0,
      programs: (s.pipelines as number | undefined) ?? 0,
      pixelRatio: window.devicePixelRatio || 1,
      drawing,
      frames,
      draws: engine.draw_calls(),
      backend: engine.backend(),
      breakdown: Array.from(engine.draw_breakdown()),
      stats: s,
    };
  };
}

/**
 * The Floor on the wasm engine: the Machines view, or with `initialId` (`#/floor/<id>`) that machine's board, entered
 * on mount. A pick opens the machine popup (floor-popup.ts's card); its ENTER flies the engine into the board.
 */
export function mountEngine(root: HTMLElement, initialId: string | null = null, onFail?: (reason: string) => void): Cleanup {
  const name = glueName();
  if (name === '') return () => undefined; // no engine in this build: show nothing
  const wrap = document.createElement('div');
  // The Floor's stage (floor.css): the canvas fills it.
  wrap.className = 'floor-stage floor-stage-3d';
  wrap.style.cssText = 'position:relative;overflow:hidden';
  const canvas = document.createElement('canvas');
  canvas.className = 'floor-canvas';
  // touch-action none: the pointer drives the orbit on touch screens too, so the page does not pan or zoom under it.
  canvas.style.cssText = 'touch-action:none';
  const status = document.createElement('div');
  status.style.cssText = 'position:absolute;left:16px;bottom:16px;font:13px/1.4 var(--sans);color:#ddd;pointer-events:none';
  status.textContent = 'Starting the engine';
  const card = document.createElement('div');
  card.className = 'floor-card machine-card';
  card.style.cssText = 'right:16px;top:16px;left:auto';
  card.hidden = true;
  // The legend, bottom-left of the stage: the machines' statuses, or a board's robot states (setLegend swaps them).
  const legend = h('div.floor-legend');
  const setLegend = (machines: boolean): void => {
    legend.replaceChildren(
      ...(machines ? FLEET_KEYS : BOARD_KEYS).map(([key, label]) => {
        const swatch = document.createElement('i');
        swatch.className = `floor-key ${key}`;
        return h('span', null, swatch, label);
      }),
    );
  };
  setLegend(true);
  wrap.append(canvas, status, legend, card);
  // The page header (the old Floor's): the title, inside a board the breadcrumb "Machines › name" (Machines goes back),
  // and on the right BACK TO MACHINES (a board only) and RESET VIEW.
  const crumbName = document.createElement('span');
  const crumbLink = h('a', {
    href: '#/floor',
    onclick: (ev: Event) => {
      ev.preventDefault();
      backToMachines();
    },
  });
  crumbLink.textContent = 'Machines';
  const crumb = h('nav.floor-crumb', { 'aria-label': 'Breadcrumb' }, crumbLink, ' › ', crumbName);
  crumb.hidden = true;
  const backBtn = button('Back to machines', () => backToMachines());
  backBtn.hidden = true;
  const resetBtn = button('Reset view', () => engine?.command('reset'), { icon: 'refresh' });
  const viewHead = h('div.view-head', null, h('h1', null, 'Floor'), crumb, h('div.actions', null, backBtn, resetBtn));
  root.append(viewHead, wrap);
  // The Floor fills the window (floor.css): the view and the page take the floor mode.
  root.classList.add('floor-view');
  document.body.classList.add('floor-mode');

  let closed = false;
  let engine: WebEngine | null = null;
  let fleet: FleetResponse | null = null;
  let fleetError: string | null = null;
  let agents: Agent[] | null = null;
  let agentNames = new Map<number, string>();
  let popupId: string | null = null; // the machine the popup shows, or null
  // The robot selected in the board: the engine draws its full label (labels.rs), the same box the desktop shows, so the
  // page adds no card of its own.
  let pickedRobot: number | null = null;
  let cardKey = ''; // what the popup last rendered, so a poll that changes nothing does not rebuild it
  let entered: string | null = null; // the machine whose board is shown, or null for the Machines view
  let pendingEnter: string | null = initialId; // a direct load's machine, entered once the fleet is in the engine
  let raf = 0;
  const timers: number[] = [];

  const pushFeed = (): void => {
    engine?.set_feed(JSON.stringify({ fleet, agents, fleet_error: fleetError }));
  };

  const machineName = (id: string): string => fleet?.machines.find((x) => x.id === id)?.name ?? id;
  const agentName = (cid: number): string => agentNames.get(cid) ?? `agent ${cid}`;

  const hideCard = (): void => {
    popupId = null;
    cardKey = '';
    card.hidden = true;
    card.replaceChildren();
  };

  /** Closes the popup here and in the engine (its own popup state follows). */
  const closePopup = (): void => {
    hideCard();
    engine?.command('popup');
  };

  const renderPopup = (): void => {
    const id = popupId;
    if (id === null) {
      card.hidden = true;
      return;
    }
    card.hidden = false;
    const m = fleet?.machines.find((x) => x.id === id);
    const bar = document.createElement('div');
    bar.style.cssText = 'display:flex;justify-content:flex-end';
    bar.append(button('Close', () => closePopup()));
    if (m === undefined) {
      const note = document.createElement('div');
      note.textContent = `${id}: not in the fleet yet`;
      cardKey = `missing:${id}`;
      card.replaceChildren(bar, note);
      return;
    }
    const key = JSON.stringify([m, m.agents.map(agentName)]);
    if (key === cardKey) return;
    cardKey = key;
    const built = machineCard(m, agentName, () => enterMachine(m.id), Date.now());
    card.replaceChildren(bar, ...built.children);
  };

  /** The popup or the robot's card beside its point on the stage, kept on the stage. */
  const anchorCard = (): void => {
    if (engine === null || card.hidden) return;
    const at = popupId !== null ? engine.screen_of('machine', popupId) : null;
    if (at === null || at.length < 2) return;
    const left = Math.min(Math.max(at[0]! + 24, 8), Math.max(8, wrap.clientWidth - 308));
    const top = Math.min(Math.max(at[1]! - 120, 8), Math.max(8, wrap.clientHeight - card.offsetHeight - 8));
    card.style.right = 'auto';
    card.style.left = `${left}px`;
    card.style.top = `${top}px`;
  };

  const enterMachine = (id: string): void => {
    if (engine === null) return;
    closePopup();
    engine.command('enter', id);
    setEntered(id);
  };

  const backToMachines = (): void => {
    if (entered === null) return;
    hideCard();
    pickedRobot = null;
    engine?.command('back');
    setEntered(null);
  };

  /** The board's crumb, and the hash: `#/floor/<id>` while a board is shown, `#/floor` otherwise. */
  const setEntered = (id: string | null): void => {
    entered = id;
    crumb.hidden = id === null;
    backBtn.hidden = id === null;
    setLegend(id === null);
    crumbName.textContent = id === null ? '' : machineName(id);
    const target = id === null ? '#/floor' : `#/floor/${encodeURIComponent(id)}`;
    // A pushed entry, not a hashchange: the router would remount the Floor and its engine with it.
    if (location.hash !== target) history.pushState(null, '', target);
  };

  const tryPendingEnter = (): void => {
    if (pendingEnter === null || engine === null || fleet === null) return;
    const id = pendingEnter;
    pendingEnter = null;
    enterMachine(id);
  };

  const refreshOpen = (): void => {
    if (popupId !== null) renderPopup();
    if (entered !== null) crumbName.textContent = machineName(entered);
  };

  const pollFleet = (): void => {
    api.fleet().then(
      (f) => {
        fleet = f;
        fleetError = null;
        pushFeed();
        tryPendingEnter();
        refreshOpen();
      },
      (e: unknown) => {
        fleetError = e instanceof Error ? e.message : String(e); // keep the last fleet
        pushFeed();
      },
    );
  };

  const pollAgents = (): void => {
    api.agents().then(
      (list) => {
        agents = list;
        agentNames = new Map(list.map((a) => [a.cid, floorName(a)] as [number, string]));
        pushFeed();
      },
      () => undefined, // keep the last list until the next poll
    );
  };


  /** One display frame: the picked card follows its anchor (the engine paints the labels itself). */
  const frame = (): void => {
    raf = 0;
    if (closed) return;
    anchorCard();
    raf = requestAnimationFrame(frame);
  };

  // Debug hooks (window.__apz*) for tests and developers: the names and return shapes the Floor has always had,
  // backed by the engine. The camera is WebEngine.look_at (no hook names it yet).
  const installDebugHooks = (started: WebEngine): (() => void) => {
    const w = window as unknown as Record<string, unknown>;
    const stats = (): Record<string, unknown> => {
      try {
        return JSON.parse(started.draw_stats()) as Record<string, unknown>;
      } catch {
        return {};
      }
    };
    const perf = perfProbe(started, () => started.view_name());
    const hooks: Record<string, unknown> = {
      __apzLookAt: (x: number, y: number, z: number, dist: number, polar: number, azimuth: number): void =>
        started.look_at(x, y, z, dist, polar, azimuth),
      __apzPerf: perf,
      __apzCamera: () => JSON.parse(started.camera_json()) as Record<string, unknown>,
      __apzPrograms: () => {
        const p = stats().programs;
        if (Array.isArray(p)) return p;
        if (p && typeof p === 'object') {
          return Object.entries(p as Record<string, unknown>).map(([name, draws]) => ({ name, key: name, draws }));
        }
        return [];
      },
      __apzOpen: (id: string) => started.command('popup', id),
      __apzEnter: (id: string) => started.command('enter', id),
      __apzBack: () => started.command('back'),
      __apzEntryTime: () => started.entry_time() ?? null,
      __apzBots: () => JSON.parse(started.bots_json()) as Record<string, unknown>,
      __apzTrace: () => ({ frames: started.frames_drawn(), draws: started.draw_calls() }),
      __apzNavCell: (x: number, z: number) => JSON.parse(started.nav_cell(x, z)) as Record<string, unknown> | null,
      __apzNavAudit: () => JSON.parse(started.nav_audit()) as Record<string, unknown> | null,
      __apzLifts: () => JSON.parse(started.lifts()) as unknown[],
      __apzLiftRide: (index: number, n: number) => JSON.parse(started.lift_ride(index, n)) as Record<string, unknown>,
      __apzShove: (cid: number, x: number, z: number) => started.shove(cid, x, z),
      __apzGroundAudit: () => JSON.parse(started.ground_audit()) as Record<string, unknown> | null,
    };
    for (const [name, fn] of Object.entries(hooks)) w[name] = fn;
    const unscene = installSceneHook(started);
    return () => {
      for (const name of Object.keys(hooks)) delete w[name];
      unscene();
    };
  };
  let removeDebug: (() => void) | null = null;

  // Escape is answered here, before the engine's own handler: its popup closes first, then the board goes Back.
  const onKey = (ev: KeyboardEvent): void => {
    if (ev.key !== 'Escape' || closed) return;
    ev.stopImmediatePropagation();
    if (popupId !== null) closePopup();
    else if (pickedRobot !== null) {
      pickedRobot = null;
      engine?.command('robot');
    }
    else if (entered !== null) backToMachines();
  };
  window.addEventListener('keydown', onKey, true);

  // Browser Back from a board (#/floor/<id> to #/floor) is the engine's Back, not a remount: this capture listener on
  // the window runs before the router's hashchange listener, and stops it there.
  const onHash = (ev: HashChangeEvent): void => {
    if (closed || entered === null || location.hash !== '#/floor') return;
    ev.stopImmediatePropagation();
    backToMachines();
  };
  window.addEventListener('hashchange', onHash, true);

  const boot = async (): Promise<void> => {
    const started = await startEngine(canvas);
    if (closed) {
      started.destroy();
      return;
    }
    started.on_pick((kind, id) => {
      if (closed) return;
      if (kind === 'machine') {
        if (entered === null) {
          pickedRobot = null;
          popupId = id;
          renderPopup();
        }
      } else if (kind === 'robot') {
        // The engine selected it and draws its full label; the page shows no second card.
        if (entered !== null) pickedRobot = Number(id);
      } else if (kind === 'none') {
        pickedRobot = null;
        hideCard();
      }
    });
    engine = started;
    // The engine's counters for the browser checks (this mount only: the flag is on the page's URL).
    (window as unknown as { apzEngine?: WebEngine }).apzEngine = started;
    removeDebug = installDebugHooks(started);
    status.textContent = '';
    pushFeed();
    tryPendingEnter();
    raf = requestAnimationFrame(frame);
  };
  // An engine that cannot start (no WebGPU or WebGL2 here): the caller's fallback, else the reason in the status line.
  boot().catch((e: unknown) => {
    if (closed) return;
    const reason = e instanceof Error ? e.message : String(e);
    if (onFail) onFail(reason);
    else status.textContent = reason;
  });

  pollFleet();
  pollAgents();
  timers.push(window.setInterval(pollFleet, FLEET_POLL_MS), window.setInterval(pollAgents, AGENTS_POLL_MS));
  // The console's stream into the engine, as the desktop forwards it (Floor::on_event): tool calls and work set the
  // robots' activities and packets, and the bus frames give the robots' work its stand-ups, deliveries and responses.
  const offEvents = (['transcript', 'delta', 'conversation', 'project', 'file', 'work'] as const).map((kind) =>
    on(kind, (data) => {
      try {
        engine?.event(kind, JSON.stringify(data));
      } catch {
        // An event the engine cannot read: the floor carries on without it.
      }
    }),
  );

  return () => {
    closed = true;
    for (const off of offEvents) off();
    root.classList.remove('floor-view');
    document.body.classList.remove('floor-mode');
    for (const t of timers) window.clearInterval(t);
    if (raf !== 0) cancelAnimationFrame(raf);
    window.removeEventListener('keydown', onKey, true);
    window.removeEventListener('hashchange', onHash, true);
    const e = engine;
    engine = null;
    removeDebug?.();
    removeDebug = null;
    e?.destroy();
    root.innerHTML = '';
  };
}
