/* Run comparison: two transcript windows side by side, rendered from the same
   store as everything else. The hash carries the pair — #compare/{a}/{avm}/
   {b}/{bvm} — so a comparison can be bookmarked or dictated; the picker form
   just edits the hash. Each pane gets the run view's stamps (VM, guest CID,
   state, exit kind — the same exitKind derivation), one toggle follows new
   lines in both windows, and a diff-lite summary names the first line where
   the two transcripts part. */

import { byId, el, toast } from '../dom';
import { fmtNum } from '../format';
import type { Run, Vm } from '../models/run';
import { exitKind, exitKindText, roleOf } from '../models/run';
import type { CompareTarget } from '../store';
import { store } from '../store';
import { vmStateMk, roleChip } from './chrome';
import { txRow } from './txrow';

const MAXROWS = 800; // per-pane render window; older lines elide

/* ------------------------------------------------------------ pane state */
interface PaneState {
  /** "runId/vmIndex" the pane is bound to; a change resets the window. */
  key: string;
  from: number;
  to: number;
  lastRow: { idx: number; pre: HTMLElement } | null;
  empty: string | null;
  dirty: boolean;
}

interface Side {
  runId: string;
  vmIdx: number;
  st: PaneState;
  head: HTMLElement;
  wrap: HTMLElement;
  tx: HTMLElement;
  open: HTMLAnchorElement;
}

const sides: Side[] = [];
let followBoth = true;
let pickerKey: string | null = null;
let runListSig: string | null = null;

function paneOf(which: 'a' | 'b'): Side {
  const side = sides.find((s) => s.head.id === `cmp-head-${which}`);
  if (!side) throw new Error(`compare pane ${which} not initialised`);
  return side;
}

function blankState(): PaneState {
  return { key: '', from: 0, to: 0, lastRow: null, empty: null, dirty: true };
}

/* ------------------------------------------------------------ transcript
   The rows come from ./txrow — the run entry and these panes read alike. */

/** Windowed append, the same convention as the run view's transcript —
    including the last-line-grows case (a partial line on the wire). */
function renderPane(side: Side, lines: string[], raw: string): void {
  const { st, tx, wrap } = side;
  if (lines.length === 0) {
    if (st.empty !== 'boot') {
      tx.textContent = '';
      tx.appendChild(txRow(0, '— the guest is booting; serial output lands here.', 'elide'));
      st.from = st.to = 0;
      st.lastRow = null;
      st.empty = 'boot';
      st.dirty = false;
    }
    return;
  }
  if (st.empty != null) {
    st.empty = null;
    st.dirty = true;
  }
  if (st.dirty) {
    tx.textContent = '';
    st.from = Math.max(0, lines.length - MAXROWS);
    st.to = st.from;
    st.lastRow = null;
    st.dirty = false;
    if (st.from > 0) tx.appendChild(txRow(-1, `… ${fmtNum(st.from)} earlier lines elided …`, 'elide'));
  } else if (lines.length - st.from > MAXROWS) {
    st.from = lines.length - MAXROWS;
    tx.textContent = '';
    tx.appendChild(txRow(-1, `… ${fmtNum(st.from)} earlier lines elided …`, 'elide'));
    st.to = st.from;
    st.lastRow = null;
  }

  /* a previously rendered last line may have grown (partial line on the wire) */
  if (st.lastRow && st.lastRow.idx < lines.length) {
    st.lastRow.pre.textContent = lines[st.lastRow.idx] ?? '';
  }
  st.lastRow = null;

  let lastPre: HTMLElement | null = null;
  for (let i = st.to; i < lines.length; i++) {
    const row = txRow(i, lines[i] ?? '');
    lastPre = row.querySelector('pre');
    tx.appendChild(row);
  }
  st.to = lines.length;
  if (lines.length && raw !== '' && !raw.endsWith('\n') && lastPre) {
    st.lastRow = { idx: lines.length - 1, pre: lastPre };
  }

  if (followBoth) wrap.scrollTop = wrap.scrollHeight;
}

/* ------------------------------------------------------------ panes */
function renderSide(side: Side): { run: Run; vm: Vm; lines: string[] } | null {
  const key = `${side.runId}/${side.vmIdx}`;
  if (side.st.key !== key) {
    const fresh = blankState();
    fresh.key = key;
    Object.assign(side.st, fresh);
    side.tx.textContent = '';
  }

  const run = store.details.get(side.runId) ?? null;
  const head = side.head;
  head.textContent = '';
  if (!run) {
    const known = store.runs.some((r) => r.id === side.runId);
    const msg = store.runsError != null
      ? `run ${side.runId} — the node is not answering`
      : known || store.runs.length > 0
        ? `run ${side.runId} — not on the register`
        : `run ${side.runId} — fetching the entry…`;
    head.appendChild(el('span', 'm', msg));
    renderPane(side, [], '');
    return null;
  }
  const vm = run.vms[side.vmIdx];
  if (!vm) {
    head.appendChild(
      el('span', 'm', `run ${run.id} has ${run.vms.length} ${run.vms.length === 1 ? 'VM' : 'VMs'} — vm ${side.vmIdx} is not among them`),
    );
    renderPane(side, [], '');
    return null;
  }

  head.appendChild(el('span', 'm', `run ${run.id} · vm ${vm.id}`));
  const role = roleChip(roleOf(vm));
  if (role) head.appendChild(role);
  if (vm.guest_cid != null) head.appendChild(el('span', 'chip', 'cid ' + vm.guest_cid));
  head.appendChild(vmStateMk(vm.state));
  if (vm.state === 'exited') {
    const kind = exitKind(store.linesFor(run.id, vm.id, vm.serial));
    head.appendChild(el('span', 'stamp sm', exitKindText(kind)));
  } else if (vm.state === 'killed') {
    head.appendChild(el('span', 'stamp sm deny', 'Killed'));
  }

  const lines = store.linesFor(run.id, vm.id, vm.serial);
  head.appendChild(el('span', 'cmplines', `${fmtNum(lines.length)} ${lines.length === 1 ? 'line' : 'lines'}`));
  side.open.setAttribute('data-go', `#run/${run.id}`);
  renderPane(side, lines, vm.serial ?? '');
  return { run, vm, lines };
}

/* ------------------------------------------------------------ diff-lite */
function renderDiff(a: { run: Run; vm: Vm; lines: string[] } | null,
                    b: { run: Run; vm: Vm; lines: string[] } | null): void {
  const body = byId('cmp-diff');
  body.textContent = '';
  if (!a || !b) {
    body.appendChild(el('p', 'example',
      'Resolve both sides and the first line where the two transcripts part is named here.'));
    return;
  }
  const la = a.lines;
  const lb = b.lines;
  const n = Math.min(la.length, lb.length);
  let i = 0;
  while (i < n && la[i] === lb[i]) i++;

  const nameA = `${a.run.id} vm ${a.vm.id}`;
  const nameB = `${b.run.id} vm ${b.vm.id}`;
  if (la.length === 0 && lb.length === 0) {
    body.appendChild(el('p', 'example', 'Both transcripts are empty so far — the guests are booting.'));
    return;
  }
  if (i === la.length && i === lb.length) {
    body.appendChild(el('p', 'example',
      `The two transcripts are identical — ${fmtNum(la.length)} ${la.length === 1 ? 'line' : 'lines'} each, not a byte between them.`));
    return;
  }
  if (i === n) {
    const longer = la.length > lb.length ? nameA : nameB;
    body.appendChild(el('p', 'example',
      `Identical for the first ${fmtNum(i)} ${i === 1 ? 'line' : 'lines'}; ` +
      `${longer} continues for ${fmtNum(Math.abs(la.length - lb.length))} more.`));
    return;
  }
  body.appendChild(el('p', 'example',
    `Identical for ${fmtNum(i)} ${i === 1 ? 'line' : 'lines'}, then the first divergence at line ${fmtNum(i + 1)}:`));
  const pair = el('div', 'diffpair');
  pair.appendChild(el('div', 'm', `${nameA} · line ${fmtNum(i + 1)}`));
  pair.appendChild(el('pre', null, la[i] ?? ''));
  pair.appendChild(el('div', 'm', `${nameB} · line ${fmtNum(i + 1)}`));
  pair.appendChild(el('pre', null, lb[i] ?? ''));
  body.appendChild(pair);
}

/* ------------------------------------------------------------ render */
export function renderCompare(): void {
  const notice = byId('cmp-notice');
  if (store.runsError != null) {
    notice.hidden = false;
    byId('cmp-notice-text').textContent =
      `The node daemon is not answering at ${location.origin} (${store.runsError}). ` +
      'Retrying; both windows catch up on their own.';
  } else {
    notice.hidden = true;
  }
  byId('cmp-live').hidden = store.health !== 'live';
  byId('cmp-live').textContent = store.health;

  /* the picker's run-id suggestions follow the register */
  const ids = store.runs.map((r) => r.id);
  const sig = ids.join('|');
  if (sig !== runListSig) {
    runListSig = sig;
    const dl = byId('cmp-runs');
    dl.textContent = '';
    for (const id of ids) dl.appendChild(el('option', null, id));
  }

  const target = store.route.compare;
  /* the hash is the truth; the inputs echo it (without clobbering typing) */
  const key = target ? `${target.a}/${target.avm}/${target.b}/${target.bvm}` : '';
  if (key !== pickerKey) {
    pickerKey = key;
    if (target) {
      byId<HTMLInputElement>('cmp-a').value = target.a;
      byId<HTMLInputElement>('cmp-avm').value = String(target.avm);
      byId<HTMLInputElement>('cmp-b').value = target.b;
      byId<HTMLInputElement>('cmp-bvm').value = String(target.bvm);
    }
  }

  if (!target) {
    const a = paneOf('a');
    const b = paneOf('b');
    for (const side of [a, b]) {
      side.runId = '';
      side.st.key = '';
      side.head.textContent = '';
      side.head.appendChild(el('span', 'm', 'no run named — use the pair form above'));
      renderPane(side, [], '');
    }
    byId('cmp-diff').textContent = '';
    byId('cmp-diff').appendChild(el('p', 'example',
      'Name two runs above — or open a run entry and use a VM row’s compare link. ' +
      'The address #compare/r1/0/r2/0 means run r1’s vm 0 against run r2’s vm 0.'));
    return;
  }

  paneOf('a').runId = target.a;
  paneOf('a').vmIdx = target.avm;
  paneOf('b').runId = target.b;
  paneOf('b').vmIdx = target.bvm;
  const ra = renderSide(paneOf('a'));
  const rb = renderSide(paneOf('b'));
  renderDiff(ra, rb);
}

/** `f` on the compare route: one follow toggle drives both windows. */
export function cmpFollowKey(): void {
  followBoth = !followBoth;
  byId<HTMLInputElement>('cmp-follow').checked = followBoth;
  if (followBoth) for (const side of sides) side.wrap.scrollTop = side.wrap.scrollHeight;
  toast(followBoth ? 'Following new lines in both windows' : 'Follow paused');
}

/** `j` on the compare route: jump both windows to their latest lines. */
export function cmpJumpKey(): void {
  byId<HTMLInputElement>('cmp-follow').checked = true;
  followBoth = true;
  for (const side of sides) side.wrap.scrollTop = side.wrap.scrollHeight;
}

/* ------------------------------------------------------------ wiring */
function goToPair(target: CompareTarget): void {
  const hash = `#compare/${target.a}/${target.avm}/${target.b}/${target.bvm}`;
  if (location.hash !== hash) location.hash = hash;
  else store.scheduleRender();
}

export function initCompare(): void {
  for (const which of ['a', 'b'] as const) {
    sides.push({
      runId: '',
      vmIdx: 0,
      st: blankState(),
      head: byId(`cmp-head-${which}`),
      wrap: byId(`cmp-wrap-${which}`),
      tx: byId(`cmp-tx-${which}`),
      open: byId<HTMLAnchorElement>(`cmp-open-${which}`),
    });
  }

  byId('cmp-form').addEventListener('submit', (e) => {
    e.preventDefault();
    const errEl = byId('cmp-err');
    errEl.textContent = '';
    const a = byId<HTMLInputElement>('cmp-a').value.trim();
    const b = byId<HTMLInputElement>('cmp-b').value.trim();
    const avm = Number(byId<HTMLInputElement>('cmp-avm').value);
    const bvm = Number(byId<HTMLInputElement>('cmp-bvm').value);
    if (a === '' || b === '') {
      errEl.textContent = 'Both sides need a run id (r1, r2, …).';
      return;
    }
    if (!Number.isInteger(avm) || avm < 0 || !Number.isInteger(bvm) || bvm < 0) {
      errEl.textContent = 'VM numbers are whole numbers from 0 up.';
      return;
    }
    goToPair({ a, avm, b, bvm });
  });

  byId('cmp-swap').addEventListener('click', () => {
    const target = store.route.compare;
    if (!target) {
      /* nothing named yet: swap the raw inputs */
      const aIn = byId<HTMLInputElement>('cmp-a');
      const bIn = byId<HTMLInputElement>('cmp-b');
      const avmIn = byId<HTMLInputElement>('cmp-avm');
      const bvmIn = byId<HTMLInputElement>('cmp-bvm');
      [aIn.value, bIn.value] = [bIn.value, aIn.value];
      [avmIn.value, bvmIn.value] = [bvmIn.value, avmIn.value];
      return;
    }
    goToPair({ a: target.b, avm: target.bvm, b: target.a, bvm: target.avm });
  });

  byId('cmp-follow').addEventListener('change', function (this: HTMLInputElement) {
    followBoth = this.checked;
    if (followBoth) for (const side of sides) side.wrap.scrollTop = side.wrap.scrollHeight;
  });
}
