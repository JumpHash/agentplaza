// The hardware lab: a developer screen for the hardware models under assets/hardware/. The Rust engine draws one
// model (a board, a part or an exterior) with its state controls from the route. Without the engine in the build,
// the lab says so instead.
// Route: #/hardware-lab?model=exterior/workstation&gpus=1&status=online&working=0&open=0&traces=1
// model is a board/…, parts/… or exterior/… id; an unknown one shows the engine's fallback (board/workstation).
// The state is read once, when the lab mounts; a route change after that is not followed.

import { h } from '../ui/dom';
import type { Cleanup } from './index';
import { engineWanted, startHardwareLabEngine, type HardwareLabOptions } from './engine-mount';

type Status = HardwareLabOptions['status'];

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

/** The model on the Rust engine, its canvas filling the root. The state is the one read at mount. */
function mountEngineLab(root: HTMLElement, state: LabState): Cleanup {
  const canvas = document.createElement('canvas');
  canvas.style.cssText = 'display:block;width:100%;height:70vh;min-height:420px;touch-action:none';
  root.append(canvas);
  const lab: HardwareLabOptions = { model: state.model, gpus: state.gpus, status: state.status, working: state.working, open: state.open, traces: state.traces };
  let engine: { destroy: () => void } | null = null;
  let closed = false;
  startHardwareLabEngine(canvas, lab).then(
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

export function mount(root: HTMLElement): Cleanup {
  if (!engineWanted()) {
    root.replaceChildren(h('p', { style: 'padding:16px;color:var(--ink-2)' }, 'The hardware lab needs the engine, which this build does not have.'));
    return () => {};
  }
  return mountEngineLab(root, readState(location.hash));
}
