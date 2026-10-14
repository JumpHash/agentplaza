// The room lab: a developer screen for the procedural rooms. It shows one room, the twelve seeds of one role side
// by side, or every role at one size, drawn by the wasm engine (startRoomLabEngine in ./engine-mount).
// Route: #/room-lab?role=architect&w=4&d=3.5&door=s&seed=7&grid=1
//   role: a role id, or all (every role at the same size). Default: orchestrator.
//   w, d: the inner size in units (default 4 and 3.5). door: n | e | s | w (default s). seed: a u32 (default 7).
//   grid=1: the seeds seed .. seed+11 side by side, each labelled.

import { h } from '../ui/dom';
import type { Cleanup } from './index';
import rolesFile from '../../../../assets/rooms/roles.json';
import { startRoomLabEngine, type RoomLabOptions } from './engine-mount';

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

/** The room lab on the wasm engine: the canvas fills the root. The route is read once, when the lab mounts. */
export function mount(root: HTMLElement): Cleanup {
  const canvas = document.createElement('canvas');
  canvas.style.cssText = 'display:block;width:100%;height:70vh;min-height:420px;touch-action:none';
  root.append(canvas);
  const s = readState(location.hash);
  const lab: RoomLabOptions = { role: s.role, size: `${s.w}x${s.d}`, seed: s.seed, door: s.door, grid: s.grid };
  let engine: { destroy: () => void } | null = null;
  let closed = false;
  startRoomLabEngine(canvas, lab).then(
    (started) => {
      if (closed) started.destroy();
      else engine = started;
    },
    (err: unknown) => {
      const msg = err instanceof Error ? err.message : String(err);
      canvas.replaceWith(h('p', { style: 'color:var(--ink-2)' }, `The engine did not start: ${msg}`));
    },
  );
  return () => {
    closed = true;
    engine?.destroy();
    canvas.remove();
  };
}
