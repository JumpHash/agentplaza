// The hardware lab: a developer screen for the hardware kit, the glTF machine models of the line-up. The Rust engine
// draws one model, or all of them in a row (`all`), with its style, level of detail, theme, status and camera from the
// route. Without the engine in the build, the lab says so instead.
// Route: #/hardware-lab?model=all&lod=0&style=ink&theme=dark&status=working&cam=iso&ortho=1
// model is `all` or a kit id (hub, server, rack, gpu, tower, edge, mini, studio, laptop, phone); lod is 0 (full) to 2
// (block); style is ink | blueprint | night; theme is light | dark; status is working | idle | strained | offline;
// cam is iso | front | side | back | top; ortho=1 draws with the orthographic camera (unset is perspective).
// A control change writes the route and restarts the engine with the new options. The readout is the engine's summary
// of the models shown.
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

const STATUSES = ['working', 'idle', 'strained', 'offline'] as const;
type Status = (typeof STATUSES)[number];
const THEMES = ['light', 'dark'] as const;
const LODS = ['0', '1', '2'] as const;
const CAMS = ['iso', 'front', 'side', 'back', 'top'] as const;
type Cam = (typeof CAMS)[number];

interface LabState {
  /** `all` or a kit model id. */
  model: string;
  lod: number;
  style: string;
  theme: string;
  status: Status;
  cam: Cam;
  ortho: boolean;
}

const DEFAULT_STATE: LabState = { model: 'all', lod: 0, style: 'ink', theme: 'dark', status: 'working', cam: 'iso', ortho: false };

/** The lab's state from the route's query; a missing or unknown value takes its default. */
function readState(hash: string, cat: LabCatalog): LabState {
  const q = new URLSearchParams(hash.split('?')[1] ?? '');
  const pick = <T extends string>(key: string, values: readonly T[], fb: T): T => {
    const v = q.get(key) ?? '';
    return (values as readonly string[]).includes(v) ? (v as T) : fb;
  };
  const lod = Number(q.get('lod'));
  return {
    model: pick('model', ['all', ...cat.models], DEFAULT_STATE.model),
    lod: q.has('lod') && Number.isInteger(lod) && lod >= 0 && lod <= 2 ? lod : DEFAULT_STATE.lod,
    style: pick('style', cat.styles, DEFAULT_STATE.style),
    theme: pick('theme', THEMES, DEFAULT_STATE.theme),
    status: pick('status', STATUSES, DEFAULT_STATE.status),
    cam: pick('cam', CAMS, DEFAULT_STATE.cam),
    ortho: q.get('ortho') === '1',
  };
}

function writeRoute(s: LabState): void {
  const q = new URLSearchParams({
    model: s.model,
    lod: String(s.lod),
    style: s.style,
    theme: s.theme,
    status: s.status,
    cam: s.cam,
  });
  if (s.ortho) q.set('ortho', '1');
  history.replaceState(null, '', `#/hardware-lab?${q.toString()}`);
}

function toOptions(s: LabState): HardwareLabOptions {
  return { model: s.model, lod: s.lod, style: s.style, theme: s.theme, status: s.status, cam: s.cam, ortho: s.ortho };
}

/** The options of a select, one per value. */
function choices(values: readonly string[]): HTMLOptionElement[] {
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

/** The kit on the Rust engine: the controls over its canvas. The state is the one read at mount. */
function mountEngineLab(root: HTMLElement, s: LabState, cat: LabCatalog): Cleanup {
  const bar = 'display:flex;flex-wrap:wrap;gap:10px 16px;align-items:center;padding:8px 0';

  const apply = (): void => {
    writeRoute(s);
    host.restart(toOptions(s));
    paint();
  };
  const paint = (): void => {
    modelSel.value = s.model;
    lodSel.value = String(s.lod);
    styleSel.value = s.style;
    themeSel.value = s.theme;
    statusSel.value = s.status;
    camBtns.forEach((b, i) => (b.style.outline = CAMS[i] === s.cam ? '2px solid var(--accent)' : ''));
    orthoIn.checked = s.ortho;
  };

  // The model list is `all` and the engine's kit ids; the styles are the engine's too.
  const modelSel = h('select.select', { onchange: () => { s.model = modelSel.value; apply(); } }, ...choices(['all', ...cat.models]));
  const lodSel = h('select.select', { onchange: () => { s.lod = Number(lodSel.value); apply(); } }, ...choices(LODS));
  const styleSel = h('select.select', { onchange: () => { s.style = styleSel.value; apply(); } }, ...choices(cat.styles));
  const themeSel = h('select.select', { onchange: () => { s.theme = themeSel.value; apply(); } }, ...choices(THEMES));
  const statusSel = h('select.select', { onchange: () => { s.status = statusSel.value as Status; apply(); } }, ...choices(STATUSES));
  const camBtns = CAMS.map((c) => button(c.toUpperCase(), () => { s.cam = c; apply(); }));
  const orthoIn = h('input', { type: 'checkbox', onchange: () => { s.ortho = orthoIn.checked; apply(); } });

  const readout = h('div', { style: 'font-family:var(--sans);font-size:12px;padding:4px 0;color:var(--ink-2)', 'aria-live': 'polite' });
  const panel = h(
    'div',
    { style: 'padding:12px 16px' },
    h(
      'div',
      { style: bar },
      field('model', modelSel),
      field('lod', lodSel),
      field('style', styleSel),
      field('theme', themeSel),
      field('status', statusSel),
      h('span', { style: 'display:flex;gap:4px' }, ...camBtns),
      field('ortho', orthoIn),
    ),
    readout,
  );
  root.append(panel);
  paint();

  /** The readout of a started engine: the summary of the models shown (count, level of detail, style). */
  const showInfo = (engine: WebEngine): void => {
    let info: { summary?: string } = {};
    try {
      info = JSON.parse(engine.lab_info()) as typeof info;
    } catch {
      info = {};
    }
    readout.textContent = info.summary ?? '';
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
      close = mountEngineLab(root, readState(location.hash, cat), cat);
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
