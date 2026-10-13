import { api, type Agent, type FleetResponse } from '../api';
import { floorName } from '../roles';
import { button } from '../ui/components';
import type { Cleanup } from './index';
import { LABEL_CSS, machineCard } from './floor-popup';

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
interface WebEngine {
  backend(): string;
  set_feed(json: string): void;
  command(name: string, arg?: string | null): void;
  on_pick(cb: (kind: 'machine' | 'robot' | 'none', id: string) => void): void;
  on_labels(cb: (labels: LabelMeta[]) => void): void;
  label_positions(): Float32Array;
  busy(): boolean;
  frames_drawn(): number;
  draw_calls(): number;
  destroy(): void;
}

/** The glue module: its default export loads the .wasm; `start` builds the engine on a canvas. */
interface EngineGlue {
  default: () => Promise<unknown>;
  start: (canvas: HTMLCanvasElement, opts: { backend?: 'auto' | 'webgl2' }) => Promise<WebEngine>;
}

/** One reusable DOM label: the pool grows to the most labels a view has, and is never rebuilt per frame. */
interface LabelEntry {
  el: HTMLDivElement;
  key: HTMLElement;
  name: HTMLElement;
  x: number;
  y: number;
  shown: boolean;
}

/** The same cadence as the DOM Machines view and the desktop. */
const FLEET_POLL_MS = 5000;
const AGENTS_POLL_MS = 4000;

/** True when the page's query (not the hash) asks for the wasm engine (`/?engine=wasm#/floor`) and this build has the glue. */
export function engineWanted(): boolean {
  return new URLSearchParams(location.search).get('engine') === 'wasm' && glueName() !== '';
}

/**
 * The Floor on the wasm engine: the Machines view, or with `initialId` (`#/floor/<id>`) that machine's board, entered
 * on mount. A pick opens the machine popup (the three.js Floor's card); its ENTER flies the engine into the board.
 */
export function mountEngine(root: HTMLElement, initialId: string | null = null): Cleanup {
  const name = glueName();
  if (name === '') return () => undefined; // no engine in this build: show nothing
  const wrap = document.createElement('div');
  wrap.style.cssText = 'position:relative;width:100%;height:100%;overflow:hidden';
  const canvas = document.createElement('canvas');
  // touch-action none: the pointer drives the orbit on touch screens too, so the page does not pan or zoom under it.
  canvas.style.cssText = 'display:block;width:100%;height:100%;touch-action:none';
  const labelLayer = document.createElement('div');
  labelLayer.style.cssText = 'position:absolute;inset:0;overflow:hidden;pointer-events:none';
  const status = document.createElement('div');
  status.style.cssText = 'position:absolute;left:16px;bottom:16px;font:13px/1.4 system-ui,sans-serif;color:#ddd;pointer-events:none';
  status.textContent = 'Starting the engine';
  const crumb = document.createElement('div');
  crumb.className = 'floor-crumb';
  crumb.style.cssText = 'position:absolute;left:16px;top:16px;display:none;align-items:center;gap:10px';
  const crumbName = document.createElement('span');
  crumb.append(button('Back to machines', () => backToMachines()), crumbName);
  const card = document.createElement('div');
  card.className = 'floor-card machine-card';
  card.style.cssText = 'right:16px;top:16px;left:auto';
  card.hidden = true;
  wrap.append(canvas, labelLayer, status, crumb, card);
  root.append(wrap);

  let closed = false;
  let engine: WebEngine | null = null;
  let fleet: FleetResponse | null = null;
  let fleetError: string | null = null;
  let agents: Agent[] | null = null;
  let agentNames = new Map<number, string>();
  let popupId: string | null = null; // the machine the popup shows, or null
  let cardKey = ''; // what the popup last rendered, so a poll that changes nothing does not rebuild it
  let entered: string | null = null; // the machine whose board is shown, or null for the Machines view
  let pendingEnter: string | null = initialId; // a direct load's machine, entered once the fleet is in the engine
  let labels: LabelMeta[] = [];
  const pool: LabelEntry[] = [];
  let lastDrawn = -1;
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

  const enterMachine = (id: string): void => {
    if (engine === null) return;
    closePopup();
    engine.command('enter', id);
    setEntered(id);
  };

  const backToMachines = (): void => {
    if (entered === null) return;
    hideCard();
    engine?.command('back');
    setEntered(null);
  };

  /** The board's crumb, and the hash: `#/floor/<id>` while a board is shown, `#/floor` otherwise. */
  const setEntered = (id: string | null): void => {
    entered = id;
    crumb.style.display = id === null ? 'none' : 'flex';
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

  /** The label at index i of the pool, made the first time it is needed. */
  const labelAt = (i: number): LabelEntry => {
    let entry = pool[i];
    if (entry === undefined) {
      const el = document.createElement('div');
      el.style.cssText = LABEL_CSS;
      const key = document.createElement('i');
      key.className = 'floor-key';
      const text = document.createElement('span');
      text.style.cssText = 'overflow:hidden;text-overflow:ellipsis;min-width:0';
      el.append(key, text);
      labelLayer.append(el);
      entry = { el, key, name: text, x: NaN, y: NaN, shown: false };
      pool.push(entry);
    }
    return entry;
  };

  /** The engine's label text changed (`on_labels`): the names and the status squares, in the engine's order. */
  const setLabels = (list: LabelMeta[]): void => {
    labels = list;
    list.forEach((meta, i) => {
      const p = labelAt(i);
      if (p.name.textContent !== meta.name) p.name.textContent = meta.name;
      p.key.style.background = `hsl(${meta.hue} 60% 50%)`;
    });
  };

  /** Places the labels from the last drawn frame (`label_positions`: flat [cid, x, y, visible] per label). */
  const placeLabels = (): void => {
    if (engine === null) return;
    const flat = engine.label_positions();
    const count = Math.min(labels.length, flat.length >> 2);
    for (let i = 0; i < pool.length; i++) {
      const p = pool[i]!;
      const visible = i < count && flat[i * 4 + 3]! > 0.5;
      if (!visible) {
        if (p.shown) {
          p.el.style.display = 'none';
          p.shown = false;
        }
        continue;
      }
      if (!p.shown) {
        p.el.style.display = 'flex';
        p.shown = true;
      }
      const x = flat[i * 4 + 1]!;
      const y = flat[i * 4 + 2]!;
      if (x !== p.x || y !== p.y) {
        p.x = x;
        p.y = y;
        p.el.style.transform = `translate(${x.toFixed(1)}px, ${y.toFixed(1)}px) translate(-50%, -100%) translateY(-6px)`;
      }
    }
  };

  /** One display frame: the labels follow the engine's draws (a frame counter, so an idle engine costs nothing here). */
  const frame = (): void => {
    raf = 0;
    if (closed) return;
    if (engine !== null) {
      const drawn = engine.frames_drawn();
      if (drawn !== lastDrawn) {
        lastDrawn = drawn;
        placeLabels();
      }
    }
    raf = requestAnimationFrame(frame);
  };

  // Escape is answered here, before the engine's own handler: its popup closes first, then the board goes Back.
  const onKey = (ev: KeyboardEvent): void => {
    if (ev.key !== 'Escape' || closed) return;
    ev.stopImmediatePropagation();
    if (popupId !== null) closePopup();
    else if (entered !== null) backToMachines();
  };
  window.addEventListener('keydown', onKey, true);

  const glueUrl = new URL('./' + name, document.baseURI).href;
  const backend: 'auto' | 'webgl2' = new URLSearchParams(location.search).get('gl') === '1' ? 'webgl2' : 'auto';
  const boot = async (): Promise<void> => {
    const glue = (await import(/* @vite-ignore */ glueUrl)) as EngineGlue;
    await glue.default();
    const started = await glue.start(canvas, { backend });
    if (closed) {
      started.destroy();
      return;
    }
    started.on_pick((kind, id) => {
      if (closed) return;
      if (kind === 'machine') {
        if (entered === null) {
          popupId = id;
          renderPopup();
        }
      } else if (kind === 'none') hideCard();
    });
    started.on_labels((list) => {
      if (!closed) setLabels(list);
    });
    engine = started;
    // The engine's counters for the browser checks (this mount only: the flag is on the page's URL).
    (window as unknown as { apzEngine?: WebEngine }).apzEngine = started;
    status.textContent = '';
    pushFeed();
    tryPendingEnter();
    raf = requestAnimationFrame(frame);
  };
  boot().catch((e: unknown) => {
    if (!closed) status.textContent = e instanceof Error ? e.message : String(e);
  });

  pollFleet();
  pollAgents();
  timers.push(window.setInterval(pollFleet, FLEET_POLL_MS), window.setInterval(pollAgents, AGENTS_POLL_MS));

  return () => {
    closed = true;
    for (const t of timers) window.clearInterval(t);
    if (raf !== 0) cancelAnimationFrame(raf);
    window.removeEventListener('keydown', onKey, true);
    const e = engine;
    engine = null;
    e?.destroy();
    root.innerHTML = '';
  };
}
