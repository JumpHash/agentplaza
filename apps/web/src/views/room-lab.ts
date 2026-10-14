// The room lab: a developer screen for the procedural rooms. It shows one room, the twelve seeds of one role side
// by side, or every role at one size, drawn by the wasm engine (startRoomLabEngine in ./engine-mount).
// Route: #/room-lab?role=architect&w=4&d=3.5&door=s&seed=7&grid=1
//   role: a role id, or all (every role at the same size). Default: orchestrator.
//   w, d: the inner size in units (default 4 and 3.5). door: n | e | s | w (default s). seed: a u32 (default 7).
//   grid=1: the seeds seed .. seed+11 side by side, each labelled.
// A control change writes the route and restarts the engine with the new options. The engine has no option for the
// debug overlay, the camera or the violation count, so the lab has no controls or readout for them.

import { field } from '../ui/components';
import { h } from '../ui/dom';
import type { Cleanup } from './index';
import rolesFile from '../../../../assets/rooms/roles.json';
import { mountLabEngine, startRoomLabEngine, type RoomLabOptions } from './engine-mount';

const DOORS = ['n', 'e', 's', 'w'] as const;
type Door = (typeof DOORS)[number];
const ROLE_IDS = Object.keys(rolesFile.roles);

interface LabState {
  role: string;
  w: number;
  d: number;
  door: Door;
  seed: number;
  grid: boolean;
}

function num(q: URLSearchParams, key: string, fallback: number, lo: number, hi: number): number {
  const raw = q.get(key);
  const v = raw === null || raw.trim() === '' ? NaN : Number(raw);
  return Number.isFinite(v) && v >= lo && v <= hi ? v : fallback;
}

function readState(hash: string): LabState {
  const q = new URLSearchParams(hash.split('?')[1] ?? '');
  const role = q.get('role') ?? 'orchestrator';
  const door = q.get('door') as Door | null;
  return {
    role: role === 'all' || ROLE_IDS.includes(role) ? role : 'orchestrator',
    w: num(q, 'w', 4, 1, 12),
    d: num(q, 'd', 3.5, 1, 12),
    door: door !== null && DOORS.includes(door) ? door : 's',
    seed: Math.floor(num(q, 'seed', 7, 0, 4294967295)),
    grid: q.get('grid') === '1',
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
  history.replaceState(null, '', `#/room-lab?${q.toString()}`);
}

function toOptions(s: LabState): RoomLabOptions {
  return { role: s.role, size: `${s.w}x${s.d}`, seed: s.seed, door: s.door, grid: s.grid };
}

/** A number from an input when it is in range; otherwise the fallback. */
function readNum(el: HTMLInputElement, fallback: number, lo: number, hi: number): number {
  const v = el.valueAsNumber;
  return Number.isFinite(v) && v >= lo && v <= hi ? v : fallback;
}

/** The lab: the controls over the engine's canvas. The route is read once, when the lab mounts. */
export function mount(root: HTMLElement): Cleanup {
  const s = readState(location.hash);
  const bar = 'display:flex;flex-wrap:wrap;gap:10px 16px;align-items:center;padding:8px 0';

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
  };

  const roleSel = h(
    'select.select',
    { onchange: () => { s.role = roleSel.value; apply(); } },
    ...['all', ...ROLE_IDS].map((r) => h('option', { value: r }, r)),
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
    ),
  );
  root.append(panel);
  paint();

  const host = mountLabEngine(root, startRoomLabEngine, toOptions(s));

  return () => {
    host.close();
    panel.remove();
  };
}
