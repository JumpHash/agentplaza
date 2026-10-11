/* Chrome: the index nav, the running header, the colophon, the theme switch,
   and the state marks every view shares. */

import { byId, el, toast } from '../dom';
import { fmtNum } from '../format';
import { hubLine } from '../models/hub';
import type { RunState, VmState } from '../models/run';
import type { Theme } from '../store';
import { roleOf, seqOf } from '../models/run';
import { isOpenTask } from '../models/task';
import { store } from '../store';

/* The register's marks: one filled colour (the owner's orange); every other
   state is ink or an outlined shape. The shapes pair with the map and the floor. */
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
  return el('span', VM_MARK[state ?? 'exited'] ?? 'mk', state ?? 'exited');
}

/** The role tag a VM carries when the register names one: the same small,
    square, ink-outlined chip as the guest CID, so the run table, the compare
    panes and the roster read alike. A VM with no role draws nothing — the
    field is the fleet's alone, and no placeholder is ever invented for it. */
export function roleChip(role: string | null): HTMLElement | null {
  return role == null ? null : el('span', 'chip role', role);
}

/** The hub line, into whichever element asks. The register's fleet-status
    marginalia and the roster both read the one builder, and both turn amber
    when the hub is unreachable — a state distinct from an empty hub, which
    still names its zeroes. */
export function renderHubLine(id: string): void {
  const node = byId(id);
  node.textContent = hubLine(store.hub, store.hubError);
  node.classList.toggle('warn', store.hubError != null);
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

/** The `t` key: system, then light, then dark, then system again. */
export function cycleTheme(): void {
  const order: readonly Theme[] = ['system', 'light', 'dark'];
  const next = order[(order.indexOf(store.theme) + 1) % order.length] ?? 'system';
  store.theme = next;
  applyTheme(next);
  toast('Theme: ' + next);
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

/* The wire as marginalia: the event stream's own liveness and what it has
   carried, kept honest about being transport — daemon health is the polls'. */
function renderWire(): void {
  const w = store.wire;
  const total = w.run + w.vm + w.serial + w.task + w.treasury + w.agora + w.board + w.conversation;
  byId('colo-wire').textContent =
    store.stream === 'live'
      ? `The wire · live — ${fmtNum(total)} frames carried (${fmtNum(w.run)} run · ${fmtNum(w.vm)} vm · ${fmtNum(w.serial)} serial · ${fmtNum(w.task)} task · ${fmtNum(w.treasury)} treasury · ${fmtNum(w.agora)} agora · ${fmtNum(w.board)} board · ${fmtNum(w.conversation)} talk)`
      : store.stream === 'reconnecting'
        ? 'The wire · down — reconnecting; the polls carry the register'
        : 'The wire · opening…';
}

export function renderChrome(): void {
  renderWire();
  byId('nav-count').textContent = store.runsError != null ? '—' : fmtNum(store.runs.length);
  byId('nav-run-id').textContent = store.detailId ?? '—';
  byId('nav-machine').textContent = store.machine?.class ?? '—';
  const vms = store.runs.reduce((n, r) => n + r.vms.length, 0);
  byId('nav-map').textContent = store.runsError != null ? '—' : fmtNum(vms);
  /* the floor answers one question — how many agents stand on it (the
     register's role VMs, joined to the hub's connected agents by CID, the
     same join the floor draws) */
  const connectedCids = store.hubError == null && store.hub != null ? new Set(store.hub.agents.map((a) => a.cid)) : null;
  let onFloor = 0;
  const seenCids = new Set<number>();
  for (const run of store.runs) {
    for (const vm of run.vms) {
      if (roleOf(vm) == null) continue;
      if (vm.guest_cid != null) {
        if (seenCids.has(vm.guest_cid)) continue;
        seenCids.add(vm.guest_cid);
      }
      onFloor++;
    }
  }
  if (connectedCids != null) for (const cid of connectedCids) if (!seenCids.has(cid)) onFloor++;
  byId('nav-floor').textContent = store.runsError != null && store.hubError != null ? '—' : fmtNum(onFloor);
  /* the roster answers one question — how many role agents are running now */
  let roleAgents = 0;
  let roleRunning = 0;
  for (const run of store.runs) {
    for (const vm of run.vms) {
      if (roleOf(vm) == null) continue;
      roleAgents++;
      if (vm.state === 'running') roleRunning++;
    }
  }
  byId('nav-roster').textContent = roleAgents === 0 ? '—' : fmtNum(roleRunning);
  /* the ledger answers one question — how many tasks still need an agent's
     hand (open, unclaimed, or claimed and not yet settled) */
  let tasksInFlight = 0;
  for (const task of store.tasks) if (isOpenTask(task)) tasksInFlight++;
  byId('nav-ledger').textContent = store.tasksError != null ? '—' : fmtNum(tasksInFlight);
  /* the treasury answers one question — how many entries the account-book
     journal holds; a journal the console cannot read reads '—' */
  byId('nav-treasury').textContent = store.journalError != null ? '—' : fmtNum(store.journalTotal);
  /* the market answers one question — how many calls still offer (a roled
     task whose offer stands, no award yet); an unreadable board reads '—' */
  byId('nav-market').textContent =
    store.callsError != null ? '—' : fmtNum(store.calls.filter((c) => c.state === 'open').length);
  /* the board answers one question — how many posts the public board holds; a
     board this node does not serve reads '—' */
  byId('nav-board').textContent = store.boardError != null ? '—' : fmtNum(store.boardTotal);
  /* session data, not daemon state — the count stands even while unreachable */
  byId('nav-telemetry').textContent = fmtNum(store.history.fleet.length);

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

  const cmp = store.route.compare;
  const cmd =
    store.route.name === 'run' && store.detailId
      ? `apz run ${store.detailId} --follow`
      : store.route.name === 'compare' && cmp
        ? `apz compare ${cmp.a}/${cmp.avm} ${cmp.b}/${cmp.bvm} --follow`
        : `apz ${store.route.name === 'register' ? 'register' : store.route.name} --follow`;
  const prompt = byId('prompt');
  const firstNode = prompt.firstChild;
  if (firstNode) firstNode.nodeValue = '$ ' + cmd;
}
