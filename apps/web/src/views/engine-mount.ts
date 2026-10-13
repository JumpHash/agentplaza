import { api, type Agent, type FleetMachine, type FleetResponse } from '../api';
import type { Cleanup } from './index';

/** The wasm glue's file name at the dist root: esbuild replaces this at build time (null when the wasm crate is missing). */
declare const __APZ_ENGINE_JS__: string | null;

/** The glue's file name, or '' when the build has no engine (typeof is safe even if the define is absent). */
function glueName(): string {
  return typeof __APZ_ENGINE_JS__ === 'string' ? __APZ_ENGINE_JS__ : '';
}

/** The surface of the wasm engine this mount uses (the crate's WebEngine, wasm-bindgen --target web). */
interface WebEngine {
  backend(): string;
  set_feed(json: string): void;
  command(name: string, arg?: string): void;
  on_pick(cb: (kind: 'machine' | 'robot' | 'none', id: string) => void): void;
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

/** The same cadence as the DOM Machines view and the desktop. */
const FLEET_POLL_MS = 5000;
const AGENTS_POLL_MS = 4000;

/** True when the page's query (not the hash) asks for the wasm engine (`/?engine=wasm#/floor`) and this build has the glue. */
export function engineWanted(): boolean {
  return new URLSearchParams(location.search).get('engine') === 'wasm' && glueName() !== '';
}

/** The Machines view drawn by the wasm engine on a canvas. The DOM popup opens on a machine pick. No three.js here. */
export function mountEngine(root: HTMLElement): Cleanup {
  const name = glueName();
  if (name === '') return () => undefined; // no engine in this build: show nothing
  const wrap = document.createElement('div');
  wrap.style.cssText = 'position:relative;width:100%;height:100%;overflow:hidden';
  const canvas = document.createElement('canvas');
  // touch-action none: the pointer drives the orbit on touch screens too, so the page does not pan or zoom under it.
  canvas.style.cssText = 'display:block;width:100%;height:100%;touch-action:none';
  const status = document.createElement('div');
  status.style.cssText = 'position:absolute;left:16px;bottom:16px;font:13px/1.4 system-ui,sans-serif;color:#ddd;pointer-events:none';
  status.textContent = 'Starting the engine';
  const popup = document.createElement('div');
  popup.style.cssText = 'position:absolute;right:16px;top:16px;display:none;min-width:240px;max-width:320px;padding:14px 16px;border-radius:8px;background:rgba(20,22,28,0.94);color:#eee;font:13px/1.5 system-ui,sans-serif';
  wrap.append(canvas, status, popup);
  root.append(wrap);

  let closed = false;
  let engine: WebEngine | null = null;
  let fleet: FleetResponse | null = null;
  let fleetError: string | null = null;
  let agents: Agent[] | null = null;
  const timers: number[] = [];

  const pushFeed = (): void => {
    engine?.set_feed(JSON.stringify({ fleet, agents, fleet_error: fleetError }));
  };

  const pollFleet = (): void => {
    api.fleet().then(
      (f) => {
        fleet = f;
        fleetError = null;
        pushFeed();
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
        pushFeed();
      },
      () => undefined, // keep the last list until the next poll
    );
  };

  const hidePopup = (): void => {
    popup.style.display = 'none';
    popup.textContent = '';
  };

  const row = (text: string, bold = false): HTMLDivElement => {
    const el = document.createElement('div');
    el.textContent = text;
    if (bold) el.style.fontWeight = '600';
    return el;
  };

  const showMachine = (id: string): void => {
    const m: FleetMachine | undefined = fleet?.machines.find((x) => x.id === id);
    const close = document.createElement('button');
    close.textContent = 'Close';
    close.style.cssText = 'float:right;margin-left:12px;cursor:pointer';
    close.addEventListener('click', hidePopup);
    popup.textContent = '';
    popup.append(close);
    if (m === undefined) {
      popup.append(row(id, true), row('Not in the fleet yet'));
    } else {
      popup.append(
        row(m.name, true),
        row(`${m.kind} · ${m.status}`),
        row(m.cpu.model),
        row(`${m.cpu.cores} cores · ${Math.round(m.memory_mib / 1024)} GiB`),
        row(`${m.agents.length} agents, ${m.working} working`),
      );
    }
    popup.style.display = 'block';
  };

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
      if (kind === 'machine') showMachine(id);
      else if (kind === 'none') hidePopup();
    });
    engine = started;
    // The engine's counters for the browser checks (this mount only: the flag is on the page's URL).
    (window as unknown as { apzEngine?: WebEngine }).apzEngine = started;
    status.textContent = '';
    pushFeed();
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
    const e = engine;
    engine = null;
    e?.destroy();
    root.innerHTML = '';
  };
}
