/* Chrome: the index nav, the running header, the colophon, the theme switch,
   and the state marks every view shares. */

import { byId, el } from '../dom';
import { fmtNum } from '../format';
import type { RunState, VmState } from '../models/run';
import { seqOf } from '../models/run';
import { store } from '../store';

/* The register's marks: one filled colour (the owner's orange); every other
   state is ink or an outlined shape. The shapes pair with the map and 3D. */
const RUN_MARK: Record<RunState, string> = {
  starting: 'mk warn',
  running: 'mk ok',
  done: 'mk',
  failed: 'mk crit',
  killed: 'mk',
};
const VM_MARK: Record<VmState, string> = { running: 'mk ok', exited: 'mk', killed: 'mk' };

export function runStateMk(state: RunState | undefined): HTMLElement {
  return el('span', RUN_MARK[state ?? 'done'] ?? 'mk', state ?? '—');
}

export function vmStateMk(state: VmState | undefined): HTMLElement {
  return el('span', VM_MARK[state ?? 'exited'] ?? 'mk', state ?? '—');
}

/* ------------------------------------------------------------ theme */
function applyTheme(pref: string): void {
  const root = document.documentElement;
  if (pref === 'system') root.removeAttribute('data-theme');
  else root.setAttribute('data-theme', pref);
  document.querySelectorAll<HTMLElement>('[data-theme-set]').forEach((b) => {
    b.setAttribute('aria-pressed', String(b.dataset.themeSet === pref));
  });
  try {
    localStorage.setItem('apz-theme', pref);
  } catch {
    /* private windows may refuse storage; the theme still applies */
  }
  store.scheduleRender(); // canvases re-read the palette on the next render
}

export function initTheme(): void {
  let pref = 'system';
  try {
    pref = localStorage.getItem('apz-theme') ?? 'system';
  } catch {
    /* storage refused */
  }
  store.theme = pref === 'light' || pref === 'dark' ? pref : 'system';
  document.querySelectorAll<HTMLElement>('[data-theme-set]').forEach((b) => {
    b.addEventListener('click', () => {
      const p = b.dataset.themeSet ?? 'system';
      store.theme = p === 'light' || p === 'dark' ? p : 'system';
      applyTheme(store.theme);
    });
  });
  applyTheme(store.theme);
}

/* ------------------------------------------------------------ colophon */
export function initColophon(): void {
  byId('colo-origin').textContent = 'Node ' + location.host;
  /* The bundle hash is stamped into the served HTML at build time (sha256
     over the exact js+css bytes); show it the way the register would. */
  const hash = document.body.dataset.bundleHash ?? '';
  byId('colo-hash').textContent =
    hash ? `sha256:${hash.slice(0, 8)}…${hash.slice(-4)}` : 'unstamped';
}

/* ------------------------------------------------------------ header */
const HEALTH_MARK: Record<string, string> = { live: 'mk ok', stale: 'mk warn', down: 'mk crit' };

export function renderChrome(): void {
  byId('nav-count').textContent = store.runsError != null ? '—' : fmtNum(store.runs.length);
  byId('nav-run-id').textContent = store.detailId ?? '—';
  byId('nav-machine').textContent = store.machine?.class ?? '—';
  const vms = store.runs.reduce((n, r) => n + r.vms.length, 0);
  byId('nav-map').textContent = store.runsError != null ? '—' : fmtNum(vms);
  byId('nav-3d').textContent = byId('nav-map').textContent;

  const date = new Date().toLocaleDateString('en-GB', {
    weekday: 'long', day: 'numeric', month: 'long', year: 'numeric',
  });
  let left: string;
  if (store.runsError != null && store.runs.length === 0) {
    left = date + ' · node unreachable';
  } else if (store.runs.length === 0) {
    left = date + ' · the register is blank';
  } else {
    const first = store.runs[0];
    left =
      date + ' · folio ' + fmtNum(first ? seqOf(first.id) : 0) + ' · ' +
      fmtNum(store.runs.length) + (store.runs.length === 1 ? ' entry' : ' entries');
  }
  byId('running-left').textContent = left;

  const health = byId('health-mk');
  health.hidden = false;
  health.className = HEALTH_MARK[store.health] ?? 'mk';
  health.textContent = store.health;

  const cmd =
    store.route.name === 'run' && store.detailId
      ? `apz run ${store.detailId} --follow`
      : `apz ${store.route.name === 'register' ? 'register' : store.route.name} --follow`;
  const prompt = byId('prompt');
  const firstNode = prompt.firstChild;
  if (firstNode) firstNode.nodeValue = '$ ' + cmd;
}
