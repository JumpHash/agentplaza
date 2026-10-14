// The hardware lab: a developer screen for the hardware models under assets/hardware/. The Rust engine draws one
// model (a board, a part or an exterior) with its state controls from the route. Without the engine in the build,
// the lab says so instead.
// Route: #/hardware-lab?model=exterior/workstation&gpus=1&status=online&working=0&open=0&traces=1
// model is a board/…, parts/… or exterior/… id; an unknown one shows the engine's fallback (board/workstation).
// A control change writes the route and restarts the engine with the new options. The engine has no option for the
// camera, the model's own parameters or the validation errors, so the lab has no controls or readout for them.
// The debug hooks __apzScene and __apzPerf read the engine while it runs (for tests and developers).

import { button, field } from '../ui/components';
import { h } from '../ui/dom';
import type { Cleanup } from './index';
import { HARDWARE_MODELS } from './lab-assets';
import { engineWanted, mountLabEngine, startHardwareLabEngine, type HardwareLabOptions, type WebEngine } from './engine-mount';

type Status = NonNullable<HardwareLabOptions['status']>;

const STATUSES: string[] = ['online', 'draining', 'offline', 'revoked'];

interface LabState {
  model: string;
  gpus: number;
  status: Status;
  working: number;
  open: number;
  traces: boolean;
}

const DEFAULT_STATE: LabState = { model: 'exterior/workstation', gpus: 0, status: 'online', working: 0, open: 0, traces: true };

/** The lab's state from the route's query; a missing or bad value takes its default. */
function readState(hash: string): LabState {
  const q = new URLSearchParams(hash.split('?')[1] ?? '');
  const num = (key: string, lo: number, hi: number, fb: number): number => {
    const v = Number(q.get(key));
    return q.has(key) && Number.isFinite(v) ? Math.min(hi, Math.max(lo, v)) : fb;
  };
  const status = q.get('status') ?? '';
  return {
    model: q.get('model') ?? DEFAULT_STATE.model,
    gpus: Math.round(num('gpus', 0, 2, DEFAULT_STATE.gpus)),
    status: (STATUSES.includes(status) ? status : DEFAULT_STATE.status) as Status,
    working: Math.round(num('working', 0, 6, DEFAULT_STATE.working)),
    open: num('open', 0, 1, DEFAULT_STATE.open),
    traces: q.has('traces') ? q.get('traces') !== '0' : DEFAULT_STATE.traces,
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
  });
  history.replaceState(null, '', `#/hardware-lab?${q.toString()}`);
}

function toOptions(s: LabState): HardwareLabOptions {
  return { model: s.model, gpus: s.gpus, status: s.status, working: s.working, open: s.open, traces: s.traces };
}

/** The options of a select, one per value. */
function choices(values: string[]): HTMLOptionElement[] {
  return values.map((v) => h('option', { value: v }, v));
}

/** The debug hooks for the engine that is running: __apzScene (the frame and draw counts) and __apzPerf (with the backend). */
function installHooks(started: WebEngine): () => void {
  const w = window as unknown as Record<string, unknown>;
  const stats = (): Record<string, unknown> => {
    try {
      return JSON.parse(started.draw_stats()) as Record<string, unknown>;
    } catch {
      return {};
    }
  };
  const hooks: Record<string, unknown> = {
    __apzScene: () => ({ frames: started.frames_drawn(), draws: started.draw_calls(), stats: stats() }),
    __apzPerf: () => ({
      frames: started.frames_drawn(),
      draws: started.draw_calls(),
      backend: started.backend(),
      breakdown: Array.from(started.draw_breakdown()),
      stats: stats(),
    }),
  };
  for (const [name, fn] of Object.entries(hooks)) w[name] = fn;
  return () => {
    for (const name of Object.keys(hooks)) delete w[name];
  };
}

/** The model on the Rust engine: the controls over its canvas. The state is the one read at mount. */
function mountEngineLab(root: HTMLElement, s: LabState): Cleanup {
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
  };

  // The model list is the assets' (board, exterior and part ids); a route's model that the list does not name is added.
  const models = HARDWARE_MODELS.includes(s.model) ? HARDWARE_MODELS : [s.model, ...HARDWARE_MODELS];
  const modelSel = h('select.select', { onchange: () => { s.model = modelSel.value; apply(); } }, ...choices(models));
  const gpuBtns = [0, 1, 2].map((n) => button(`${n} GPU${n === 1 ? '' : 's'}`, () => { s.gpus = n; apply(); }));
  const statusSel = h('select.select', { onchange: () => { s.status = statusSel.value as Status; apply(); } }, ...choices(STATUSES));
  // The sliders apply when let go: a restart per step of a drag would start the engine many times.
  const workingOut = h('span', { style: 'font-family:var(--mono);font-size:12px;min-width:2ch' });
  const workingIn = h('input', { type: 'range', min: 0, max: 6, step: 1, onchange: () => { s.working = Number(workingIn.value); apply(); } });
  workingIn.addEventListener('input', () => (workingOut.textContent = workingIn.value));
  const openOut = h('span', { style: 'font-family:var(--mono);font-size:12px;min-width:4ch' });
  const openIn = h('input', { type: 'range', min: 0, max: 1, step: 0.01, onchange: () => { s.open = Number(openIn.value); apply(); } });
  openIn.addEventListener('input', () => (openOut.textContent = Number(openIn.value).toFixed(2)));
  const tracesIn = h('input', { type: 'checkbox', onchange: () => { s.traces = tracesIn.checked; apply(); } });

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
    ),
  );
  root.append(panel);
  paint();

  const host = mountLabEngine(root, startHardwareLabEngine, toOptions(s), installHooks);
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
  return mountEngineLab(root, readState(location.hash));
}
