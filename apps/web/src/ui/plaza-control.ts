// The top bar's master switch: STOP ALL (cancel every agent's running work and hold the plaza)
// and RESUME, with the plaza's held state as a banner.

import { api, type PlazaState } from '../api';
import { on, onConnection } from '../store';
import { ago, attempt, badge, button, field, input, modal } from './components';
import { h, lu, replace } from './dom';

// ---- Shared state: one copy, painted into every control and banner on screen ----

type Sub = { el: HTMLElement; paint: () => void; seen: boolean };
const subs = new Set<Sub>();
let state: PlazaState | null = null;
let started = false;

function notify(): void {
  for (const s of subs) {
    if (s.el.isConnected) s.seen = true;
    // The top bar is rebuilt on navigation; drop elements that were on screen and are gone.
    else if (s.seen) {
      subs.delete(s);
      continue;
    }
    s.paint();
  }
}

const heldListeners = new Set<(held: boolean) => void>();

function setState(next: PlazaState | null): void {
  state = next;
  notify();
  for (const fn of heldListeners) fn(next?.paused ?? false);
}

/** Whether the plaza is held (STOP ALL, not yet resumed). Starts the shared state's one poll on first use. */
export function plazaPaused(): boolean {
  start();
  return state?.paused ?? false;
}

/** Calls `fn` with whether the plaza is held now and on each change (for a screen that is not the top bar). */
export function onPlazaHeld(fn: (held: boolean) => void): () => void {
  start();
  heldListeners.add(fn);
  fn(state?.paused ?? false);
  return () => {
    heldListeners.delete(fn);
  };
}

function subscribe(el: HTMLElement, paint: () => void): HTMLElement {
  start();
  subs.add({ el, paint, seen: false });
  paint();
  return el;
}

function start(): void {
  if (started) return;
  started = true;
  on('plaza', (frame) => setState(frame as PlazaState));
  const load = () => api.plazaState().then(setState).catch(() => undefined);
  void load();
  onConnection((up) => {
    if (up) void load();
  });
}

// ---- Confirmations ----------------------------------------------------------

function confirmStop(): void {
  const reason = input({ placeholder: 'Why (optional)', maxlength: 200 });
  let close: () => void = () => undefined;
  const stop = button(
    'Stop all',
    async () => {
      stop.disabled = true;
      const res = await attempt(() => api.stopAll(reason.value.trim()), 'All agents stopped');
      if (res) {
        setState(res);
        close();
      } else stop.disabled = false;
    },
    { kind: 'danger' },
  );
  const cancel = button('Cancel', () => close(), { kind: 'ghost' });
  close = modal(
    'Stop all agents?',
    h(
      'div.plaza-confirm',
      null,
      h('p', null, "Every agent's running work is cancelled and nothing new starts until you resume. Queued work, VMs and workspaces are kept."),
      field('Reason', reason, 'Optional. Shown on the held banner.'),
      h('div.plaza-actions', null, cancel, stop),
    ),
  );
}

function confirmResume(): void {
  let close: () => void = () => undefined;
  const go = button(
    'Resume',
    async () => {
      go.disabled = true;
      const res = await attempt(() => api.resumeAll(), 'Plaza resumed');
      if (res) {
        setState(res);
        close();
      } else go.disabled = false;
    },
    { kind: 'primary' },
  );
  go.classList.add('plaza-resume');
  const cancel = button('Cancel', () => close(), { kind: 'ghost' });
  close = modal(
    'Resume the plaza?',
    h(
      'div.plaza-confirm',
      null,
      h('p', null, 'New work can start again. The items cancelled by the stop stay cancelled.'),
      h('div.plaza-actions', null, cancel, go),
    ),
  );
}

// ---- Controls ---------------------------------------------------------------

function stopButton(): HTMLButtonElement {
  const b = button('STOP ALL', () => confirmStop(), { icon: 'stop', title: "Cancel every agent's running work and hold the plaza" });
  b.classList.add('plaza-stop');
  return b;
}

function resumeButton(): HTMLButtonElement {
  const b = button('RESUME', () => confirmResume(), { kind: 'primary', icon: 'play', title: 'Let the plaza start new work again' });
  b.classList.add('plaza-resume');
  return b;
}

/** The top bar control; keeps itself current from the `plaza` SSE kind and /v1/plaza/state. */
export function plazaControl(): HTMLElement {
  const el = h('span.plaza-control', null);
  return subscribe(el, () => {
    if (state?.paused) replace(el, [badge('held', 'danger', true), resumeButton()]);
    else replace(el, [stopButton()]);
  });
}

/** The sidebar foot's STOP ALL (RESUME while the plaza is held), as the Loom and Graphite consoles draw it. */
export function stopAll(): HTMLElement {
  const el = h('span.stopall-slot', null);
  return subscribe(el, () => {
    const held = !!state?.paused;
    const b = h(
      'button.stopall',
      { type: 'button', class: held ? 'resume' : '', title: held ? 'Let the plaza start new work again' : "Cancel every agent's running work and hold the plaza" },
      held ? 'RESUME' : 'STOP ALL',
    );
    b.addEventListener('click', () => (held ? confirmResume() : confirmStop()));
    replace(el, b);
  });
}

/** The banner shown under the top bar while the plaza is held (empty otherwise). */
export function heldBanner(): HTMLElement {
  const el = h('div.held-banner', { role: 'status' });
  return subscribe(el, () => {
    const s = state;
    el.hidden = !s?.paused;
    if (!s?.paused) {
      replace(el, []);
      return;
    }
    const by = s.by ? ` by ${s.by}` : '';
    const reason = s.reason ? `: ${s.reason}` : '';
    replace(
      el,
      lu('octagon-pause', 15),
      h('span', null, `The plaza is stopped — ${s.cancelled} running items were cancelled ${ago(s.since_ms)}${by}${reason}. Nothing new starts until you resume.`),
      bannerResume(),
    );
  });
}

function bannerResume(): HTMLButtonElement {
  return button('RESUME', () => confirmResume(), { kind: 'primary', icon: 'play', size: 'sm', title: 'Let the plaza start new work again' });
}
