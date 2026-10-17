import { api, type Agent, type FleetResponse } from '../api';
import { floorName } from '../roles';
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

/** The surface of the wasm engine this mount uses (the crate's WebEngine, wasm-bindgen --target web). */
/** A machine's view: the iso orbit, or one of the four flat orthographic drawings. */
export type MachineView = 'iso' | 'front' | 'side' | 'back' | 'top';

/** A machine's facts from the engine: its model, and its size in cm as [width, height, depth]. */
export interface MachineInfo {
  model: string;
  size_cm: [number, number, number];
}

/** The engine's machine_info JSON as a MachineInfo; null for `null`, or for JSON of another shape. */
export function parseMachineInfo(json: string): MachineInfo | null {
  try {
    const v: unknown = JSON.parse(json);
    if (typeof v !== 'object' || v === null) return null;
    const { model, size_cm } = v as Partial<MachineInfo>;
    return typeof model === 'string' && Array.isArray(size_cm) && size_cm.length === 3 ? (v as MachineInfo) : null;
  } catch {
    return null;
  }
}

export interface WebEngine {
  backend(): string;
  set_feed(json: string): void;
  /** The engine's commands: reset the view, open (or with no id, close) the popup, show a machine, leave a machine. */
  command(name: 'reset' | 'popup' | 'show_machine' | 'leave_machine', arg?: string | null): void;
  /** Draws the Machines view in a style (`ink`, `blueprint` or `night`) and the console theme (dark or light). */
  set_style(style: string, dark: boolean): void;
  /** The four status lamps' colours, JSON: CSS hex for Working, Idle, Strained and Offline, in the stage's palette. */
  lamps(): string;
  /** A pick on the stage: only a machine is picked. */
  on_pick(cb: (kind: 'machine' | 'none', id: string) => void): void;
  screen_of(kind: 'machine', id: string): Float32Array;
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
  look_at(x: number, y: number, z: number, dist: number, polar: number, azimuth: number): void;
  thumbnail(id: string, w: number, h: number): Promise<Uint8Array>;
  portrait(json: string, w: number, h: number): Promise<Uint8Array>;
  /** One of the machine's views, RGBA8 w by h with a transparent background (each side at most 512). */
  view(id: string, view: string, w: number, h: number): Promise<Uint8Array>;
  /** The machine's facts, JSON `{"model", "size_cm"}`, or `null`. */
  machine_info(id: string): string;
  destroy(): void;
}

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

/** The hardware lab's options, as the route names them; each maps to its APZ_HW_* name in the engine. `model` is a kit
 *  model id or `all`; `lod` is 0 (full) to 2 (block); `style` is ink, blueprint or night; `status` is working, idle,
 *  strained or offline; `cam` is iso, front, side, back or top; `ortho` draws with the orthographic camera. Unset ones
 *  take the engine's defaults. */
export interface HardwareLabOptions {
  model?: string;
  lod?: number;
  style?: string;
  theme?: string;
  status?: string;
  cam?: string;
  ortho?: boolean;
}

/** The labs' option lists, from the engine (`lab_catalog`): the robot species and their clips, tools, carried items,
 *  props and effects; the room roles; the hardware kit's model ids, render styles and levels of detail. */
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
  /** The hardware lab's styles: ink, blueprint, night. */
  styles: string[];
  /** The hardware lab's levels of detail, 0 (full) to lods - 1 (block). */
  lods: number;
}

/** The glue module: its default export loads the .wasm; `start` builds the engine on a canvas. */
interface EngineGlue {
  default: () => Promise<unknown>;
  lab_catalog: () => string;
  start: (canvas: HTMLCanvasElement, opts: { backend?: 'auto' | 'webgl2' }) => Promise<WebEngine>;
  start_lab: (canvas: HTMLCanvasElement, opts: LabOptions & { backend?: 'auto' | 'webgl2' }) => Promise<WebEngine>;
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


/** Loads the glue and starts an engine on the canvas: the Machines view and the stages share it. Throws without a glue. */
export async function startEngine(canvas: HTMLCanvasElement): Promise<WebEngine> {
  return (await loadGlue()).start(canvas, { backend: backendOf() });
}

/** The robot lab on the wasm engine: the route's options under the engine's APZ_ROBOT_* names. Throws without a glue. */
export async function startLabEngine(canvas: HTMLCanvasElement, lab: LabOptions): Promise<WebEngine> {
  return (await loadGlue()).start_lab(canvas, { ...lab, backend: backendOf() });
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
  /** One of the machine's views (iso, front, side, back, top), RGBA8 w by h. Rejects ENGINE_STOPPED once destroyed. */
  view(id: string, view: MachineView, w: number, h: number): Promise<Uint8Array>;
  /** The machine's facts from the engine, or null. Rejects ENGINE_STOPPED once destroyed. */
  machineInfo(id: string): Promise<MachineInfo | null>;
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

  // Runs a call on the engine once the fleet has shown it; a stopped engine rejects ENGINE_STOPPED.
  const whenReady = <T>(run: (e: WebEngine) => T | Promise<T>): Promise<T> =>
    Promise.race([fleetSeen, stopped])
      .then(() => {
        if (closed || engine === null) throw new Error(closed ? ENGINE_STOPPED : 'the engine did not start');
        return run(engine);
      })
      .catch((e: unknown) => {
        throw closed ? new Error(ENGINE_STOPPED) : e;
      });

  return {
    thumbnail: (id, w, h) => whenReady((e) => e.thumbnail(id, w, h)),
    portrait: (json, w, h) => whenReady((e) => e.portrait(json, w, h)),
    view: (id, view, w, h) => whenReady((e) => e.view(id, view, w, h)),
    machineInfo: (id) => whenReady((e) => parseMachineInfo(e.machine_info(id))),
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
    view: (id, view, w, h) => stage.view(id, view, w, h),
    machineInfo: (id) => stage.machineInfo(id),
    destroy() {
      stage.destroy();
      canvas.remove();
    },
  };
}

/** The legend's keys: the Machines view's lamps, in the engine's lamp order (WebEngine.lamps). */
const FLEET_KEYS: [string, string][] = [
  ['working', 'Working'],
  ['idle', 'Idle'],
  ['strained', 'Draining'],
  ['offline', 'Offline'],
];

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

/** A PerfSnapshot of a running engine (the three.js build's window.__apzPerf shape), plus the engine's own counts. */
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
 * The Machines view on the wasm engine (the Floor's page): the fleet's machines, drawn by the engine. A pick opens the
 * machine popup (floor-popup.ts's card), anchored to the machine's point on the stage.
 */
export function mountEngine(root: HTMLElement, onFail?: (reason: string) => void, bindReset?: (reset: (() => void) | null) => void): Cleanup {
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
  // The legend, bottom-left of the stage: the machines' lamps. The swatches take the engine's lamp colours (paintLamps).
  const swatches: HTMLElement[] = [];
  const legend = h(
    'div.floor-legend',
    null,
    ...FLEET_KEYS.map(([key, label]) => {
      const swatch = document.createElement('i');
      swatch.className = `floor-key ${key}`;
      swatches.push(swatch);
      return h('span', null, swatch, label);
    }),
  );
  wrap.append(canvas, status, legend, card);
  // The Machines screen's Reset view runs the engine's reset (null once the engine is gone).
  bindReset?.(() => engine?.command('reset'));
  root.append(wrap);
  // The grid fills the window (floor.css): the view and the page take the floor mode.
  root.classList.add('floor-view');
  document.body.classList.add('floor-mode');

  let closed = false;
  let engine: WebEngine | null = null;
  let fleet: FleetResponse | null = null;
  let fleetError: string | null = null;
  let agents: Agent[] | null = null;
  let agentNames = new Map<number, string>();
  let popupId: string | null = null; // the machine the popup shows, or null
  let cardKey = ''; // what the popup last rendered, so a poll that changes nothing does not rebuild it
  let raf = 0;
  const timers: number[] = [];

  const pushFeed = (): void => {
    engine?.set_feed(JSON.stringify({ fleet, agents, fleet_error: fleetError }));
  };

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
    const built = machineCard(m, agentName, Date.now());
    card.replaceChildren(bar, ...built.children);
  };

  /** The popup beside its machine's point on the stage, kept on the stage. */
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

  const pollFleet = (): void => {
    api.fleet().then(
      (f) => {
        fleet = f;
        fleetError = null;
        pushFeed();
        if (popupId !== null) renderPopup();
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

  /** One display frame: the popup follows its anchor (the engine paints the labels itself). */
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
      __apzTrace: () => ({ frames: started.frames_drawn(), draws: started.draw_calls() }),
    };
    for (const [name, fn] of Object.entries(hooks)) w[name] = fn;
    const unscene = installSceneHook(started);
    return () => {
      for (const name of Object.keys(hooks)) delete w[name];
      unscene();
    };
  };
  let removeDebug: (() => void) | null = null;

  // The legend's swatches take the engine's lamp colours in its current palette (WebEngine.lamps, FLEET_KEYS order).
  const paintLamps = (started: WebEngine): void => {
    try {
      const lamps = JSON.parse(started.lamps()) as string[];
      swatches.forEach((swatch, i) => {
        const colour = lamps[i];
        if (colour !== undefined) swatch.style.background = colour;
      });
    } catch {
      // the swatches keep their floor.css colours
    }
  };
  // The console is dark unless the page says light: the engine draws in that theme and the legend follows it.
  const applyTheme = (started: WebEngine): void => {
    started.set_style('ink', document.documentElement.dataset.theme !== 'light');
    paintLamps(started);
  };
  // Watches the page's theme (data-theme on the root) while the engine runs; disconnected in the cleanup.
  let themeObserver: MutationObserver | null = null;

  // Escape closes the popup here, before the engine's own handler.
  const onKey = (ev: KeyboardEvent): void => {
    if (ev.key !== 'Escape' || closed || popupId === null) return;
    ev.stopImmediatePropagation();
    closePopup();
  };
  window.addEventListener('keydown', onKey, true);

  const boot = async (): Promise<void> => {
    const started = await startEngine(canvas);
    if (closed) {
      started.destroy();
      return;
    }
    // A machine's pick opens the popup on its card (the engine has already selected it); a click on nothing closes it.
    started.on_pick((kind, id) => {
      if (closed) return;
      popupId = kind === 'machine' ? id : null;
      renderPopup();
    });
    engine = started;
    applyTheme(started);
    themeObserver = new MutationObserver(() => {
      if (engine !== null) applyTheme(engine);
    });
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    // The engine's counters for the browser checks (this mount only: the flag is on the page's URL).
    (window as unknown as { apzEngine?: WebEngine }).apzEngine = started;
    removeDebug = installDebugHooks(started);
    status.textContent = '';
    pushFeed();
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

  return () => {
    closed = true;
    for (const t of timers) window.clearInterval(t);
    if (raf !== 0) cancelAnimationFrame(raf);
    root.classList.remove('floor-view');
    document.body.classList.remove('floor-mode');
    window.removeEventListener('keydown', onKey, true);
    const e = engine;
    engine = null;
    removeDebug?.();
    removeDebug = null;
    themeObserver?.disconnect();
    themeObserver = null;
    e?.destroy();
    root.innerHTML = '';
  };
}
