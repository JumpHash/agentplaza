// The hardware lab: a developer screen for the hardware models under assets/hardware/. The Rust engine draws one
// model (a board, a part or an exterior) with its state controls from the route. Without the engine in the build,
// the lab says so instead.
// Route: #/hardware-lab?model=exterior/workstation&gpus=1&status=online&working=0&open=0&traces=1&cam=iso
// model is a board/…, parts/… or exterior/… id; an unknown one shows the engine's fallback (board/workstation).
// cam: iso | top | front | side | anchor (the model's own anchor view). Any other numeric key is a model param, as the
// engine reads it: &amd=1&pcb_top=0.3. The model's params are number inputs in the controls.
// A control change writes the route and restarts the engine with the new options. The readout is the model's
// validation: its errors in red, or "<model>: no errors", as the running engine reports them.
// The debug hooks __apzScene (the scene graph, as three's Scene was) and __apzPerf read the engine while it runs.

import { button, field } from '../ui/components';
import { h } from '../ui/dom';
import type { Cleanup } from './index';
import {
  engineWanted,
  loadLabCatalog,
  mountLabEngine,
  perfProbe,
  startHardwareLabEngine,
  type HardwareLabOptions,
  type LabCatalog,
  installSceneHook,
  type WebEngine,
} from './engine-mount';

type Status = NonNullable<HardwareLabOptions['status']>;

const STATUSES: string[] = ['online', 'draining', 'offline', 'revoked'];
const CAMS = ['iso', 'top', 'front', 'side', 'anchor'] as const;
type Cam = (typeof CAMS)[number];
/** The query keys the lab reads for itself; any other key that holds a number is a model param. */
const LAB_KEYS = new Set(['model', 'gpus', 'status', 'working', 'open', 'traces', 'cam']);
const PARAM_KEY = /^[a-z_]\w*$/;

interface LabState {
  model: string;
  gpus: number;
  status: Status;
  working: number;
  open: number;
  traces: boolean;
  cam: Cam;
  /** The model's own params from the route (overriding the model's defaults). */
  params: Record<string, number>;
}

const DEFAULT_STATE: LabState = { model: 'exterior/workstation', gpus: 0, status: 'online', working: 0, open: 0, traces: true, cam: 'iso', params: {} };

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
  for (const [key, value] of q.entries()) {
    if (!LAB_KEYS.has(key) && PARAM_KEY.test(key) && value.trim() !== '' && Number.isFinite(Number(value))) params[key] = Number(value);
  }
  return {
    model: q.get('model') ?? DEFAULT_STATE.model,
    gpus: Math.round(num('gpus', 0, 2, DEFAULT_STATE.gpus)),
    status: (STATUSES.includes(status) ? status : DEFAULT_STATE.status) as Status,
    working: Math.round(num('working', 0, 6, DEFAULT_STATE.working)),
    open: num('open', 0, 1, DEFAULT_STATE.open),
    traces: q.has('traces') ? q.get('traces') !== '0' : DEFAULT_STATE.traces,
    cam: (CAMS as readonly string[]).includes(cam) ? (cam as Cam) : DEFAULT_STATE.cam,
    params,
  };
}

function writeRoute(s: LabState): void {
  const q = new URLSearchParams({
    model: s.model,
    gpus: String(s.gpus),
    status: s.status,
    working: String(s.working),
    open: String(s.open),
    traces: s.traces ? '1' : '0',
    cam: s.cam,
  });
  for (const [k, v] of Object.entries(s.params)) q.set(k, String(v));
  history.replaceState(null, '', `#/hardware-lab?${q.toString()}`);
}

function toOptions(s: LabState): HardwareLabOptions {
  return { model: s.model, gpus: s.gpus, status: s.status, working: s.working, open: s.open, traces: s.traces, cam: s.cam, params: s.params };
}

/** The options of a select, one per value. */
function choices(values: string[]): HTMLOptionElement[] {
  return values.map((v) => h('option', { value: v }, v));
}

/** The debug hooks for the engine that is running: __apzScene (its scene graph) and __apzPerf (the PerfSnapshot). */
function installHooks(started: WebEngine): () => void {
  const w = window as unknown as Record<string, unknown>;
  const hooks: Record<string, unknown> = {
    __apzPerf: perfProbe(started, () => 'hardware-lab'),
  };
  for (const [name, fn] of Object.entries(hooks)) w[name] = fn;
  const unscene = installSceneHook(started);
  return () => {
    for (const name of Object.keys(hooks)) delete w[name];
    unscene();
  };
}

/** The model on the Rust engine: the controls over its canvas. The state is the one read at mount. */
function mountEngineLab(root: HTMLElement, s: LabState, cat: LabCatalog): Cleanup {
  const bar = 'display:flex;flex-wrap:wrap;gap:10px 16px;align-items:center;padding:8px 0';

  const apply = (): void => {
    writeRoute(s);
    host.restart(toOptions(s));
    paint();
  };
  const paint = (): void => {
    modelSel.value = s.model;
    gpuBtns.forEach((b, i) => (b.style.outline = i === s.gpus ? '2px solid var(--accent)' : ''));
    statusSel.value = s.status;
    workingIn.value = String(s.working);
    workingOut.textContent = String(s.working);
    openIn.value = String(s.open);
    openOut.textContent = s.open.toFixed(2);
    tracesIn.checked = s.traces;
    camBtns.forEach((b, i) => (b.style.outline = CAMS[i] === s.cam ? '2px solid var(--accent)' : ''));
    paramIns.forEach((input, k) => (input.value = String(s.params[k] ?? cat.params[s.model]?.[k] ?? 0)));
  };

  // The model list is the engine's (board, exterior and part ids); a route's model that the list does not name is added.
  const models = cat.models.includes(s.model) ? cat.models : [s.model, ...cat.models];
  const modelSel = h(
    'select.select',
    {
      onchange: () => {
        s.model = modelSel.value;
        s.params = {};
        rebuildParams();
        apply();
      },
    },
    ...choices(models),
  );
  const gpuBtns = [0, 1, 2].map((n) => button(`${n} GPU${n === 1 ? '' : 's'}`, () => { s.gpus = n; apply(); }));
  const statusSel = h('select.select', { onchange: () => { s.status = statusSel.value as Status; apply(); } }, ...choices(STATUSES));
  // The sliders apply when let go: a restart per step of a drag would start the engine many times.
  const workingOut = h('span', { style: 'font-family:var(--sans);font-size:12px;min-width:2ch' });
  const workingIn = h('input', { type: 'range', min: 0, max: 6, step: 1, onchange: () => { s.working = Number(workingIn.value); apply(); } });
  workingIn.addEventListener('input', () => (workingOut.textContent = workingIn.value));
  const openOut = h('span', { style: 'font-family:var(--sans);font-size:12px;min-width:4ch' });
  const openIn = h('input', { type: 'range', min: 0, max: 1, step: 0.01, onchange: () => { s.open = Number(openIn.value); apply(); } });
  openIn.addEventListener('input', () => (openOut.textContent = Number(openIn.value).toFixed(2)));
  const tracesIn = h('input', { type: 'checkbox', onchange: () => { s.traces = tracesIn.checked; apply(); } });
  const camBtns = CAMS.map((c) => button(c.toUpperCase(), () => { s.cam = c; apply(); }));

  // The model's own params (gpus has its buttons): number inputs, one per param the model declares, in name order.
  const paramIns = new Map<string, HTMLInputElement>();
  const paramsBar = h('div', { style: bar });
  const rebuildParams = (): void => {
    paramIns.clear();
    const keys = Object.keys(cat.params[s.model] ?? {}).sort();
    paramsBar.replaceChildren(
      ...keys.map((k) => {
        const input = h('input', {
          type: 'number',
          step: 'any',
          'aria-label': `param ${k}`,
          style: 'width:9ch;font-family:var(--sans);font-size:12px',
          // A half-typed value (empty, or "-") is not a param yet; the input keeps what was typed until it is one.
          onchange: () => {
            if (!Number.isFinite(input.valueAsNumber)) return;
            s.params[k] = input.valueAsNumber;
            apply();
          },
        });
        paramIns.set(k, input);
        return field(k, input);
      }),
    );
    paramsBar.style.display = keys.length ? 'flex' : 'none';
  };

  const readout = h('div', { style: 'font-family:var(--sans);font-size:12px;padding:4px 0', 'aria-live': 'polite' });
  const panel = h(
    'div',
    { style: 'padding:12px 16px' },
    h(
      'div',
      { style: bar },
      field('model', modelSel),
      h('span', { style: 'display:flex;gap:4px' }, ...gpuBtns),
      field('status', statusSel),
      field('working', workingIn),
      workingOut,
      field('open', openIn),
      openOut,
      field('traces (boards)', tracesIn),
      h('span', { style: 'display:flex;gap:4px' }, ...camBtns),
    ),
    paramsBar,
    readout,
  );
  root.append(panel);
  rebuildParams();
  paint();

  /** The readout of a started engine: the model's validation, its errors in red. */
  const showInfo = (engine: WebEngine): void => {
    let info: { summary?: string; errors?: string[] } = {};
    try {
      info = JSON.parse(engine.lab_info()) as typeof info;
    } catch {
      info = {};
    }
    readout.textContent = info.summary ?? '';
    readout.style.color = info.errors?.length ? 'var(--danger, #c0392b)' : 'var(--ink-2)';
  };
  // The canvas is 420 pixels high, as the three.js lab's stage was (its flex never grew it).
  const host = mountLabEngine(root, startHardwareLabEngine, toOptions(s), (engine) => {
    const hooks = installHooks(engine);
    showInfo(engine);
    return hooks;
  }, () => 'display:block;width:100%;height:420px;touch-action:none;cursor:grab');
  return () => {
    host.close();
    panel.remove();
  };
}

export function mount(root: HTMLElement): Cleanup {
  if (!engineWanted()) {
    root.replaceChildren(h('p', { style: 'padding:16px;color:var(--ink-2)' }, 'The hardware lab needs the engine, which this build does not have.'));
    return () => {};
  }
  const loading = h('p', { style: 'padding:12px 16px;color:var(--ink-2)' }, 'Loading the lab…');
  root.append(loading);
  let close: Cleanup = () => {};
  let closed = false;
  loadLabCatalog().then(
    (cat) => {
      if (closed) return;
      loading.remove();
      close = mountEngineLab(root, readState(location.hash), cat);
    },
    (err: unknown) => {
      loading.textContent = `The lab did not load: ${err instanceof Error ? err.message : String(err)}`;
    },
  );
  return () => {
    closed = true;
    close();
    loading.remove();
  };
}
