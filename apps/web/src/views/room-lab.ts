// The room lab: a developer screen for the procedural rooms. It shows one room, the twelve seeds of one role side
// by side, or every role at one size, drawn by the wasm engine (startRoomLabEngine in ./engine-mount).
// Route: #/room-lab?role=architect&w=4&d=3.5&door=s&seed=7&grid=1&cam=top&debug=1
//   role: a role id, or all (every role at the same size). Default: orchestrator.
//   w, d: the inner size in units (default 4 and 3.5). door: n | e | s | w (default s). seed: a u32 (default 7).
//   grid=1: the seeds seed .. seed+11 side by side, each labelled.
//   cam: obl (default, an oblique view) | top (looking straight down). debug=1: the debug overlay (the grid, the solid
//   and walk cells, the reservations, the spots and the violations' markers), and each room's line in the readout.
// A control change writes the route and restarts the engine with the new options. The readout (the violations, the
// size and the door) is the engine's own: it comes from the running engine's lab_info.

import { field } from '../ui/components';
import { h } from '../ui/dom';
import type { Cleanup } from './index';
import { loadLabCatalog, mountLabEngine, startRoomLabEngine, type RoomLabOptions, type WebEngine } from './engine-mount';

const DOORS = ['n', 'e', 's', 'w'] as const;
type Door = (typeof DOORS)[number];

interface LabState {
  role: string;
  w: number;
  d: number;
  door: Door;
  seed: number;
  grid: boolean;
  cam: 'obl' | 'top';
  debug: boolean;
}

function num(q: URLSearchParams, key: string, fallback: number, lo: number, hi: number): number {
  const raw = q.get(key);
  const v = raw === null || raw.trim() === '' ? NaN : Number(raw);
  return Number.isFinite(v) && v >= lo && v <= hi ? v : fallback;
}

function readState(hash: string, roles: string[]): LabState {
  const q = new URLSearchParams(hash.split('?')[1] ?? '');
  const role = q.get('role') ?? 'orchestrator';
  const door = q.get('door') as Door | null;
  return {
    role: role === 'all' || roles.includes(role) ? role : 'orchestrator',
    w: num(q, 'w', 4, 1, 12),
    d: num(q, 'd', 3.5, 1, 12),
    door: door !== null && DOORS.includes(door) ? door : 's',
    seed: Math.floor(num(q, 'seed', 7, 0, 4294967295)),
    grid: q.get('grid') === '1',
    cam: q.get('cam') === 'top' ? 'top' : 'obl',
    debug: q.get('debug') === '1',
  };
}

function writeRoute(s: LabState): void {
  const q = new URLSearchParams();
  q.set('role', s.role);
  q.set('w', String(s.w));
  q.set('d', String(s.d));
  q.set('door', s.door);
  q.set('seed', String(s.seed));
  if (s.grid) q.set('grid', '1');
  if (s.cam === 'top') q.set('cam', 'top');
  if (s.debug) q.set('debug', '1');
  history.replaceState(null, '', `#/room-lab?${q.toString()}`);
}

function toOptions(s: LabState): RoomLabOptions {
  return { role: s.role, size: `${s.w}x${s.d}`, seed: s.seed, door: s.door, grid: s.grid, cam: s.cam, debug: s.debug };
}

/** A number from an input when it is in range; otherwise the fallback. */
function readNum(el: HTMLInputElement, fallback: number, lo: number, hi: number): number {
  const v = el.valueAsNumber;
  return Number.isFinite(v) && v >= lo && v <= hi ? v : fallback;
}

/** The engine's readout (`lab_info`): the summary line, and each room's line. */
interface RoomInfo {
  summary?: string;
  lines?: string[];
  roles?: string[];
  layouts?: Record<string, unknown>;
}

function roomInfo(engine: WebEngine): RoomInfo {
  try {
    return JSON.parse(engine.lab_info()) as RoomInfo;
  } catch {
    return {};
  }
}

/** The lab: the controls over the engine's canvas. The route is read once, when the lab mounts. */
export function mount(root: HTMLElement): Cleanup {
  const loading = h('p', { style: 'padding:12px 16px;color:var(--ink-2)' }, 'Loading the lab…');
  root.append(loading);
  let close: Cleanup = () => {};
  let closed = false;
  loadLabCatalog().then(
    (cat) => {
      if (closed) return;
      loading.remove();
      close = mountLab(root, cat.roles);
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

function mountLab(root: HTMLElement, roles: string[]): Cleanup {
  const s = readState(location.hash, roles);
  const bar = 'display:flex;flex-wrap:wrap;gap:10px 16px;align-items:center;padding:8px 0';
  const readout = h('div', { style: 'font-family:var(--sans);font-size:12px;color:var(--ink-2);padding:4px 0', 'aria-live': 'polite' });
  const list = h('pre', { style: 'font-family:var(--mono);font-size:12px;color:var(--ink-2);white-space:pre-wrap;margin:0' });

  const apply = (): void => {
    writeRoute(s);
    host.restart(toOptions(s));
    paint();
  };
  /** Puts the controls in step with the state (a rejected input goes back to the state's value). */
  const paint = (): void => {
    roleSel.value = s.role;
    widthIn.value = String(s.w);
    depthIn.value = String(s.d);
    doorSel.value = s.door;
    seedIn.value = String(s.seed);
    gridIn.checked = s.grid;
    camSel.value = s.cam;
    debugIn.checked = s.debug;
  };

  const roleSel = h(
    'select.select',
    { onchange: () => { s.role = roleSel.value; apply(); } },
    ...['all', ...roles].map((r) => h('option', { value: r }, r)),
  );
  const widthIn = h('input.input', { type: 'number', step: '0.5', min: 1, max: 12, style: 'width:76px', onchange: () => { s.w = readNum(widthIn, s.w, 1, 12); apply(); } });
  const depthIn = h('input.input', { type: 'number', step: '0.5', min: 1, max: 12, style: 'width:76px', onchange: () => { s.d = readNum(depthIn, s.d, 1, 12); apply(); } });
  const doorSel = h('select.select', { onchange: () => { s.door = doorSel.value as Door; apply(); } }, ...DOORS.map((v) => h('option', { value: v }, v)));
  const seedIn = h('input.input', {
    type: 'number',
    step: '1',
    min: 0,
    max: 4294967295,
    style: 'width:110px',
    onchange: () => { s.seed = Math.floor(readNum(seedIn, s.seed, 0, 4294967295)); apply(); },
  });
  const gridIn = h('input', { type: 'checkbox', onchange: () => { s.grid = gridIn.checked; apply(); } });
  const camSel = h(
    'select.select',
    { 'aria-label': 'Camera', onchange: () => { s.cam = camSel.value === 'top' ? 'top' : 'obl'; apply(); } },
    h('option', { value: 'obl' }, 'oblique'),
    h('option', { value: 'top' }, 'top'),
  );
  const debugIn = h('input', { type: 'checkbox', 'aria-label': 'Debug overlay', onchange: () => { s.debug = debugIn.checked; apply(); } });

  const panel = h(
    'div',
    { style: 'padding:12px 16px' },
    h(
      'div',
      { style: bar },
      field('Role', roleSel),
      field('Width', widthIn),
      field('Depth', depthIn),
      field('Door', doorSel),
      field('Seed', seedIn),
      field('Twelve seeds', gridIn),
      field('Camera', camSel),
      field('Debug', debugIn),
    ),
    readout,
    list,
  );
  root.append(panel);
  paint();

  // The readout is the running engine's: it is read each time an engine starts (every control change restarts it).
  // The canvas sits in the panel, 540 pixels high (760 for every role), as the three.js lab had it.
  const host = mountLabEngine(panel, startRoomLabEngine, toOptions(s), (engine) => {
    const info = roomInfo(engine);
    readout.textContent = info.summary ?? '';
    list.textContent = s.debug ? (info.lines ?? []).join('\n') : '';
    // For headless checks, as the three.js lab had it: the roles shown and each room's layout by its key.
    (window as unknown as { __roomLab?: { roles: string[]; layouts: Record<string, unknown> } }).__roomLab = { roles: info.roles ?? [], layouts: info.layouts ?? {} };
    return () => {};
  }, (o) => `display:block;width:100%;height:${o.role === 'all' ? 760 : 540}px;border-radius:12px;touch-action:none`);

  return () => {
    host.close();
    panel.remove();
  };
}
