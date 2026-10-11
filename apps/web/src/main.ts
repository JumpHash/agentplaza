/* Boot: theme, chrome, router, views, polling. The store is the single
   source of truth; every view renders from it on a coalesced microtask. */

import './styles/base.css';
import './styles/views.css';

import { startEvents } from './events';
import { KEY_ROUTES, NAV_OF, parseRoute } from './router';
import { store } from './store';
import { initColophon, cycleTheme, initTheme, renderChrome } from './views/chrome';
import { cmpFollowKey, cmpJumpKey, initCompare, renderCompare } from './views/compare';
import { initKeys, keysOpen, toggleKeys } from './views/keys';
import { initLedger, renderLedger } from './views/ledger';
import { renderMarket } from './views/market';
import { initRegister, renderRegister } from './views/register';
import { followKey, initRun, jumpKey, renderRun, resetRunView, selectVm } from './views/run';
import { renderMachine } from './views/machine';
import { renderMap, syncMapVisibility } from './views/map2d';
import { initRoster, renderRoster } from './views/roster';
import { initTelemetry, renderTelemetry } from './views/telemetry';
import { renderTreasury } from './views/treasury';
import { cameraPreset, cameraState, districtState, initView3d, sync3d } from './views/view3d';

/* ------------------------------------------------------------ routing */
function show(): void {
  const route = parseRoute(location.hash);
  store.route = route;

  document.querySelectorAll<HTMLElement>('[data-view]').forEach((v) => {
    v.hidden = v.dataset['view'] !== route.name;
  });
  document.querySelectorAll<HTMLElement>('[data-nav]').forEach((a) => {
    if (a.dataset['nav'] === NAV_OF[route.name]) a.setAttribute('aria-current', 'page');
    else a.removeAttribute('aria-current');
  });

  if (route.name === 'run') {
    if (store.detailId !== route.param) {
      store.detailId = route.param;
      store.vm = 0;
      resetRunView();
    }
  }
  if (route.name === 'ledger') store.taskDetailId = route.param;
  window.scrollTo(0, 0);
  /* the first fetch of any view fires even in a hidden tab */
  store.ensurePolled();
  store.scheduleRender();
  syncMapVisibility();
}

function go(hash: string): void {
  if (location.hash !== hash) location.hash = hash;
  else show();
}

window.addEventListener('hashchange', show);

document.addEventListener('click', (e) => {
  const t = e.target;
  if (!(t instanceof Element)) return;
  const nav = t.closest<HTMLElement>('[data-nav]');
  if (nav) {
    e.preventDefault();
    navigate(nav.dataset['nav'] ?? 'register');
    return;
  }
  const g = t.closest<HTMLElement>('[data-go]');
  if (g) {
    e.preventDefault();
    go(g.getAttribute('data-go') ?? '#register');
    return;
  }
  /* a VM row's compare link: this row's VM against the one being viewed
     (viewing that very row? compare it against a neighbour instead) */
  const cmp = t.closest<HTMLElement>('[data-cmp-vm]');
  if (cmp) {
    e.preventDefault();
    const run = store.openRun;
    if (!run || run.vms.length < 2) return;
    const right = Number(cmp.getAttribute('data-cmp-vm'));
    if (!Number.isInteger(right)) return;
    let left = store.vm;
    if (left === right) left = right === 0 ? 1 : 0;
    go(`#compare/${run.id}/${left}/${run.id}/${right}`);
    return;
  }
  const s = t.closest<HTMLElement>('tr[data-sel-vm]');
  if (s) selectVm(Number(s.getAttribute('data-sel-vm')));
});

function navigate(name: string): void {
  if (name === 'run') {
    /* the Run index entry opens the selected run, else the newest open run */
    const id = store.detailId ?? store.runs.find((r) => r.state === 'starting' || r.state === 'running')?.id ?? store.runs[0]?.id;
    if (id) go(`#run/${id}`);
    return;
  }
  go('#' + name);
}

document.addEventListener('keydown', (e) => {
  const t = e.target;
  if (e.altKey || e.ctrlKey || e.metaKey) return;
  if (t instanceof HTMLElement && /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName)) return;
  if (t instanceof HTMLCanvasElement) return; // canvases own their arrows/enter
  if (keysOpen()) return; // the sheet owns Escape, Tab and ? while open
  if (e.key === '?') {
    e.preventDefault();
    toggleKeys();
    return;
  }
  if (e.key === 't') {
    cycleTheme();
    return;
  }
  if (e.key === 'f' || e.key === 'j') {
    const on = e.key === 'f';
    if (store.route.name === 'run') (on ? followKey : jumpKey)();
    else if (store.route.name === 'compare') (on ? cmpFollowKey : cmpJumpKey)();
    return;
  }
  /* the 3D view's camera keys (the canvas itself handles them when focused) */
  if ((e.key === 'o' || e.key === 'r') && store.route.name === 'view3d') {
    cameraPreset(e.key === 'o' ? 'overview' : 'focus');
    return;
  }
  if (e.key === 'Enter' && t instanceof HTMLElement && t.matches('tr[data-sel-vm]')) {
    selectVm(Number(t.getAttribute('data-sel-vm')));
    return;
  }
  /* a focused ruled row (the ledger's, the roster's) opens like a click */
  if (e.key === 'Enter' && t instanceof HTMLElement && t.matches('tr[data-go]')) {
    const dest = t.getAttribute('data-go');
    if (dest) {
      e.preventDefault();
      go(dest);
      return;
    }
  }
  const dest = KEY_ROUTES[e.key];
  if (dest) navigate(dest);
});

/* ------------------------------------------------------------ boot */
initTheme();
initColophon();
initRegister();
initRun();
initCompare();
initKeys();
initTelemetry();
initView3d();
initRoster();
initLedger();

store.onRender(renderChrome);
store.onRender(() => {
  switch (store.route.name) {
    case 'register': renderRegister(); break;
    case 'run': renderRun(); break;
    case 'machine': renderMachine(); break;
    case 'map': renderMap(); break;
    case 'view3d': sync3d(); break;
    case 'compare': renderCompare(); break;
    case 'telemetry': renderTelemetry(); break;
    case 'roster': renderRoster(); break;
    case 'ledger': renderLedger(); break;
    case 'treasury': renderTreasury(); break;
    case 'market': renderMarket(); break;
  }
});

show();
store.startPolling();
startEvents();

/* The verification harness's read-only window into the live data layer:
   render passes flushed, wire frames carried, stream state. The app itself
   never reads these. */
window.__PLAZA_DIAG__ = {
  get renders() { return store.renders; },
  get wire() { return { ...store.wire }; },
  get stream() { return store.stream; },
  get tasks() {
    return {
      count: store.tasks.length,
      open: store.tasks.filter((t) => t.state === 'open' || t.state === 'claimed').length,
      detail: store.taskDetailId,
      error: store.tasksError,
    };
  },
  get artifact() {
    const a = store.artifact;
    return a == null ? null : { task: a.task, path: a.path, state: a.state };
  },
  get treasury() {
    return {
      accounts: store.treasury?.accounts.length ?? 0,
      entries: store.journal.length,
      total: store.journalTotal,
      credits: store.treasury?.totals.credits ?? 0,
      error: store.treasuryError,
      journalError: store.journalError,
    };
  },
  get market() {
    return {
      calls: store.calls.length,
      open: store.calls.filter((c) => c.state === 'open').length,
      bids: store.calls.reduce((n, c) => n + c.bids.length, 0),
      awarded: store.calls.filter((c) => c.award != null).length,
      total: store.callsTotal,
      error: store.callsError,
    };
  },
  get tell() {
    const t = store.lastTell;
    return t == null ? null : { cid: t.cid, name: t.name, state: t.state, status: t.status };
  },
  get hub() {
    const readable = store.hub != null && store.hubError == null;
    return {
      reachable: readable,
      agents: readable ? (store.hub?.agents.length ?? 0) : 0,
      sessions: readable ? (store.hub?.sessions_active ?? 0) : 0,
      error: store.hubError,
    };
  },
  get camera3d() { return cameraState(); },
  get district3d() { return districtState(); },
};
