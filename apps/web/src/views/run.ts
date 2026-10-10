/* Run detail: the entry's particulars, the owner's kill, the VM table with
   guest-CID chips and exit stamps, the serial transcript (windowed, follow /
   jump), and the serial input with the guest's quick commands. */

import { deleteRun, sendInput } from '../api';
import { announce, byId, el, toast } from '../dom';
import { downloadText, runReportText, runTranscriptText, vmTranscriptText } from '../export';
import { fmtDur, fmtNum, fmtTime, pad } from '../format';
import { HISTORY_CAP } from '../models/history';
import type { Run, Vm } from '../models/run';
import { exitKind, exitKindText, isOpen, modeText, seqOf } from '../models/run';
import { vmKey } from '../models/activity';
import { QUICK_COMMANDS } from '../serial';
import { store } from '../store';
import { runStateMk, vmStateMk } from './chrome';
import type { Palette } from './plan';
import { palette } from './plan';
import { txRow } from './txrow';

const MAXROWS = 1500; // transcript render window; older lines elide

interface VmRow {
  tr: HTMLTableRowElement;
  vm: Vm;
  linesTd: HTMLTableCellElement;
  bytesTd: HTMLTableCellElement;
  lpsTd: HTMLTableCellElement;
  sparkCv: HTMLCanvasElement;
  sparkSig: string;
  exitTd: HTMLTableCellElement;
  viewTd: HTMLTableCellElement;
}

/* transcript render bookkeeping */
let vmSig: string | null = null;
let vmRows: VmRow[] = [];
let txFrom = 0;
let txTo = 0;
let txLastRow: { idx: number; pre: HTMLElement } | null = null;
let txEmpty: string | null = null;
let txDirty = true;
let announcedState: string | null = null;
let stampKey: string | null = null;

/** Route change to another run resets the detail bookkeeping. */
export function resetRunView(): void {
  vmSig = null;
  vmRows = [];
  txFrom = txTo = 0;
  txLastRow = null;
  txEmpty = null;
  txDirty = true;
  announcedState = null;
  stampKey = null;
  byId('d-stamp').textContent = '';
}

/* ------------------------------------------------------------ rendering */
export function renderRun(): void {
  const run = store.openRun;
  const notice = byId('d-notice');
  if (store.detailError != null) {
    notice.hidden = false;
    byId('d-notice-text').textContent =
      `The node daemon is not answering (${store.detailError}). Retrying; the page catches up on its own.`;
  } else {
    notice.hidden = true;
  }

  byId('d-crumb').textContent = store.detailId ?? '—';
  if (!run) {
    byId('d-folio').textContent = 'no. ' + pad(seqOf(store.detailId), 4);
    byId('d-h1').textContent = 'Run ' + (store.detailId ?? '—');
    byId('d-lede').textContent = store.detailError ? 'The entry cannot be read just now.' : 'Fetching the entry…';
    renderVms(null);
    renderSerial(null);
    setInputEnabled(false, 'The entry cannot be read just now.');
    return;
  }

  if (announcedState != null && run.state !== announcedState) {
    announce(`Run ${run.id} is now ${run.state}.`);
  }
  announcedState = run.state;

  const n = run.vms.length;
  byId('d-folio').textContent = 'no. ' + pad(seqOf(run.id), 4);
  byId('d-h1').textContent = 'Run ' + run.id;
  const st = byId('d-state');
  st.textContent = '';
  st.appendChild(runStateMk(run.state));
  byId('d-lede').textContent =
    `${n} ${n === 1 ? 'VM' : 'VMs'} × ${run.vcpus} vCPUs · ${fmtNum(run.memory_mib)} MiB each · ` +
    `${modeText(run)} · opened ${fmtTime(run.created_at)}` +
    (run.ended_at != null ? ` · closed ${fmtTime(run.ended_at)}` : '');
  const st2 = byId('d-state2');
  st2.textContent = '';
  st2.appendChild(runStateMk(run.state));
  byId('d-opened').textContent = fmtTime(run.created_at);
  byId('d-closed').textContent = run.ended_at != null ? fmtTime(run.ended_at) : '—';
  byId('d-elapsed').textContent = fmtDur((run.ended_at ?? Date.now()) - run.created_at);
  byId('d-n').textContent = String(n);
  byId('d-vcpus').textContent = String(run.vcpus);
  byId('d-mem').textContent = fmtNum(run.memory_mib) + ' MiB';
  byId('d-beats').textContent = run.beats == null ? 'steady — until killed' : String(run.beats);
  const counts: Record<string, number> = { running: 0, exited: 0, killed: 0 };
  for (const v of run.vms) counts[v.state] = (counts[v.state] ?? 0) + 1;
  byId('d-vm-running').textContent = String(counts.running ?? 0);
  byId('d-vm-exited').textContent = String(counts.exited ?? 0);
  byId('d-vm-killed').textContent = String(counts.killed ?? 0);

  /* the owner's hand: kill while open, a stamp once the register closes */
  const kill = byId<HTMLButtonElement>('d-kill');
  if (!kill.hasAttribute('data-busy')) {
    kill.disabled = !isOpen(run);
    kill.textContent = 'Kill run ' + run.id;
  }
  const stampSlot = byId('d-stamp');
  const key = isOpen(run) ? null : `${run.state}@${run.ended_at ?? 0}`;
  if (key !== stampKey) {
    stampKey = key;
    stampSlot.textContent = '';
    if (key) {
      const deny = run.state !== 'done';
      stampSlot.appendChild(
        el('span', 'stamp' + (deny ? ' deny' : ''),
          run.state.charAt(0).toUpperCase() + run.state.slice(1) + ' · ' + fmtTime(run.ended_at)),
      );
    }
  }

  renderVms(run);
  renderSerial(run);
  setInputEnabled(
    isOpen(run) && n > 0,
    n === 0 ? 'The VMs are starting — handles land here shortly.' : 'This run is closed — its VMs have left.',
  );
}

function renderVms(run: Run | null): void {
  const vms = run?.vms ?? [];
  byId('d-vm-count').textContent = vms.length
    ? fmtNum(vms.length) + (vms.length === 1 ? ' handle' : ' handles')
    : run ? 'starting' : '—';
  if (store.vm > vms.length - 1) store.vm = Math.max(0, vms.length - 1);

  /* Rebuild rows when the shape changes (ids, states, CIDs, exit kinds);
     counts tick in place below. */
  const sig = vms
    .map((v) => {
      const kind = v.state === 'exited' ? (exitKind(store.linesFor(run?.id ?? '', v.id, v.serial)) ?? 'x') : '';
      return `${v.id}:${v.state}:${v.guest_cid ?? ''}:${kind}`;
    })
    .join('|');
  const tb = byId('vm-body');
  if (sig !== vmSig) {
    vmSig = sig;
    vmRows = [];
    tb.textContent = '';
    vms.forEach((v, i) => {
      const tr = el('tr', 'link');
      tr.tabIndex = 0;
      tr.setAttribute('data-sel-vm', String(i));
      tr.setAttribute('aria-label',
        `Show transcript of vm ${v.id}, ${v.state}${v.guest_cid != null ? `, cid ${v.guest_cid}` : ''}`);
      tr.appendChild(el('td', null, pad(i + 1, 2)));
      const idTd = el('td', 'm', 'vm ' + v.id);
      if (v.guest_cid != null) idTd.appendChild(el('span', 'chip', 'cid ' + v.guest_cid));
      tr.appendChild(idTd);
      const stTd = el('td');
      stTd.appendChild(vmStateMk(v.state));
      tr.appendChild(stTd);
      const exitTd = el('td');
      if (v.state === 'exited') {
        const kind = exitKind(store.linesFor(run?.id ?? '', v.id, v.serial));
        exitTd.appendChild(el('span', 'stamp sm', exitKindText(kind)));
      } else if (v.state === 'killed') {
        exitTd.appendChild(el('span', 'stamp sm deny', 'Killed'));
      }
      tr.appendChild(exitTd);
      const linesTd = el('td', 'num', '0');
      const bytesTd = el('td', 'num', '0');
      const lpsTd = el('td', 'num', '0');
      const sparkTd = el('td', 'sparkcell');
      const sparkCv = el('canvas', 'spark');
      sparkCv.setAttribute('role', 'img');
      sparkTd.appendChild(sparkCv);
      const viewTd = el('td', 'm');
      tr.appendChild(linesTd);
      tr.appendChild(bytesTd);
      tr.appendChild(lpsTd);
      tr.appendChild(sparkTd);
      tr.appendChild(viewTd);
      /* compare: this VM against the viewed one — the pair is resolved at
         click time (main.ts), so a re-selected view never stale-dates it */
      const cmpTd = el('td', 'm');
      if (vms.length >= 2) {
        const cmpBtn = el('button', 'tlink', 'compare');
        cmpBtn.type = 'button';
        cmpBtn.setAttribute('data-cmp-vm', String(i));
        cmpBtn.setAttribute('aria-label', `Compare vm ${v.id} against the viewed vm`);
        cmpTd.appendChild(cmpBtn);
      }
      tr.appendChild(cmpTd);
      tb.appendChild(tr);
      vmRows.push({ tr, vm: v, linesTd, bytesTd, lpsTd, sparkCv, sparkSig: '', exitTd, viewTd });
    });
    txDirty = true;
  }

  /* runs are replaced on every poll: rebind rows to the fresh vm objects
     before reading serial, or the counts freeze at first paint */
  const pal = palette();
  const freshById = new Map(vms.map((v) => [v.id, v]));
  vmRows.forEach((row, i) => {
    const fresh = freshById.get(row.vm.id);
    if (fresh) row.vm = fresh;
    const lines = store.linesFor(run?.id ?? '', row.vm.id, row.vm.serial);
    row.linesTd.textContent = fmtNum(lines.length);
    row.bytesTd.textContent = fmtNum(row.vm.serial?.length ?? 0);
    const lps = run ? store.activity.get(vmKey(run.id, row.vm.id)) : 0;
    row.lpsTd.textContent = lps > 0.05 ? lps.toFixed(1) : '—';
    /* the rate's shape beside its number — redrawn only when a new
       observation landed for this VM */
    const series = run ? store.history.vmSeries(vmKey(run.id, row.vm.id)) : [];
    const sparkSig = series.length + ':' + (series[series.length - 1] ?? -1).toFixed(3);
    if (sparkSig !== row.sparkSig) {
      row.sparkSig = sparkSig;
      drawSpark(row.sparkCv, series, pal);
    }
    const sel = i === store.vm;
    row.tr.classList.toggle('sel', sel);
    if (sel) row.tr.setAttribute('aria-current', 'true');
    else row.tr.removeAttribute('aria-current');
    row.viewTd.textContent = sel ? 'viewing' : '';
  });
}

/** The two-minute rate shape beside a VM's number: one ink brush, no axes —
    the numeric column beside it carries the value; right-aligned on the
    HISTORY_CAP window so a young series grows in from the right, oldest left. */
function drawSpark(cv: HTMLCanvasElement, series: readonly number[], p: Palette): void {
  const g = cv.getContext('2d');
  if (!g) return;
  const cw = 96;
  const ch = 20;
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  if (cv.width !== Math.round(cw * dpr) || cv.height !== Math.round(ch * dpr)) {
    cv.width = Math.round(cw * dpr);
    cv.height = Math.round(ch * dpr);
  }
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, cw, ch);
  if (series.length < 2) {
    cv.setAttribute('aria-label', 'lines per second over the last two minutes: not enough observations yet');
    return;
  }
  /* the baseline, ruled faint */
  g.strokeStyle = p.rule;
  g.lineWidth = 1;
  g.beginPath();
  g.moveTo(0, ch - 1.5);
  g.lineTo(cw, ch - 1.5);
  g.stroke();
  const max = Math.max(...series);
  const head = max > 0 ? max * 1.15 : 1;
  const xStep = cw / (HISTORY_CAP - 1);
  const n = series.length;
  g.strokeStyle = p.ink;
  g.lineWidth = 1;
  g.beginPath();
  series.forEach((v, i) => {
    const x = cw - (n - 1 - i) * xStep;
    const y = ch - 2 - (v / head) * (ch - 4);
    if (i === 0) g.moveTo(x, y);
    else g.lineTo(x, y);
  });
  g.stroke();
  const last = series[n - 1] ?? 0;
  if (last > 0.05) {
    g.fillStyle = p.ink;
    g.fillRect(cw - 2, ch - 2 - (last / head) * (ch - 4) - 1.5, 3, 3);
  }
  cv.setAttribute(
    'aria-label',
    `lines per second over the last two minutes: now ${last.toFixed(1)}, peak ${max.toFixed(1)}`,
  );
}

/* The transcript rows come from ./txrow — the entry and the compare panes
   read alike: seq in the margin, kind, the line as text, marginalia. */

function renderSerial(run: Run | null): void {
  const tx = byId('tx');
  const vms = run?.vms ?? [];
  const vm = vms.length ? vms[Math.min(store.vm, vms.length - 1)] : undefined;
  const hd = byId('tx-hd');

  if (!vm) {
    hd.textContent = run ? 'the VMs are starting' : '—';
    byId('tx-export-vm').textContent = 'Export vm — transcript';
    tx.textContent = '';
    tx.appendChild(txRow(0, run ? '— the run is starting; VM handles land here shortly.' : '— nothing to show yet.', 'elide'));
    txFrom = txTo = 0;
    txLastRow = null;
    txEmpty = 'starting';
    txDirty = false;
    return;
  }

  const lines = store.linesFor(run?.id ?? '', vm.id, vm.serial);
  hd.textContent = `vm ${vm.id} · ${fmtNum(lines.length)} ${lines.length === 1 ? 'line' : 'lines'}`;
  byId('tx-export-vm').textContent = `Export vm ${vm.id} transcript`;

  if (lines.length === 0) {
    if (txEmpty !== 'boot') {
      tx.textContent = '';
      tx.appendChild(txRow(0, '— the guest is booting; serial output lands here.', 'elide'));
      txFrom = txTo = 0;
      txLastRow = null;
      txEmpty = 'boot';
      txDirty = false;
    }
    return;
  }
  if (txEmpty != null) {
    txEmpty = null;
    txDirty = true;
  }

  if (txDirty) {
    tx.textContent = '';
    txFrom = Math.max(0, lines.length - MAXROWS);
    txTo = txFrom;
    txLastRow = null;
    txDirty = false;
    if (txFrom > 0) tx.appendChild(txRow(-1, `… ${fmtNum(txFrom)} earlier lines elided …`, 'elide'));
  } else if (lines.length - txFrom > MAXROWS) {
    txFrom = lines.length - MAXROWS;
    tx.textContent = '';
    tx.appendChild(txRow(-1, `… ${fmtNum(txFrom)} earlier lines elided …`, 'elide'));
    txTo = txFrom;
    txLastRow = null;
  }

  /* a previously rendered last line may have grown (partial line on the wire) */
  if (txLastRow && txLastRow.idx < lines.length) {
    txLastRow.pre.textContent = lines[txLastRow.idx] ?? '';
  }
  txLastRow = null;

  let lastPre: HTMLElement | null = null;
  for (let i = txTo; i < lines.length; i++) {
    const row = txRow(i, lines[i] ?? '');
    lastPre = row.querySelector('pre');
    tx.appendChild(row);
  }
  txTo = lines.length;
  const raw = vm.serial ?? '';
  if (lines.length && !raw.endsWith('\n') && lastPre) {
    txLastRow = { idx: lines.length - 1, pre: lastPre };
  }

  if (store.follow) {
    const wrap = byId('txwrap');
    wrap.scrollTop = wrap.scrollHeight;
  }
}

/* ------------------------------------------------------------ serial input */
function currentVm(): Vm | null {
  const run = store.openRun;
  if (!run || run.vms.length === 0) return null;
  return run.vms[Math.min(store.vm, run.vms.length - 1)] ?? null;
}

function sendCmd(text: string): void {
  if (text === '') return;
  const run = store.openRun;
  const vm = currentVm();
  if (!run || !vm) return;
  const data = text.endsWith('\n') ? text : text + '\n';
  sendInput(run.id, vm.id, data)
    .then(() => {
      toast('Sent to vm ' + vm.id);
      byId<HTMLInputElement>('in-cmd').value = '';
      return store.pollDetail(run.id);
    })
    .catch((err: unknown) => {
      toast('Not sent: ' + (err instanceof Error ? err.message : String(err)));
    });
}

const DEFAULT_NOTE =
  'The guest serves one command per heartbeat window (about 5 s); the reply lands in the transcript above. ' +
  'It understands: help, uname, cpu, mem, uptime, echo <text>, poweroff.';

function setInputEnabled(on: boolean, why: string): void {
  const vm = currentVm();
  byId('in-note').textContent = on ? DEFAULT_NOTE : why;
  if (on) {
    byId('in-label').textContent = 'Command for vm ' + (vm ? vm.id : '—');
    byId('in-send').textContent = 'Send to vm ' + (vm ? vm.id : '—');
  }
  byId<HTMLInputElement>('in-cmd').disabled = !on;
  byId<HTMLButtonElement>('in-send').disabled = !on;
  byId('quick').querySelectorAll('button').forEach((b) => { b.disabled = !on; });
}

/** Selecting a VM row re-renders the transcript against it. */
export function selectVm(i: number): void {
  if (i === store.vm) return;
  store.vm = i;
  txDirty = true;
  store.scheduleRender();
}

export function initRun(): void {
  byId('d-kill').addEventListener('click', () => {
    const run = store.openRun;
    if (!run || !isOpen(run)) return;
    const btn = byId<HTMLButtonElement>('d-kill');
    btn.disabled = true;
    btn.setAttribute('data-busy', '1');
    btn.textContent = 'Killing…';
    deleteRun(run.id)
      .then((killed) => {
        store.mergeDetail(killed);
        toast(`Run ${run.id} killed — every VM stopped`);
        announce(`Run ${run.id} killed.`);
      })
      .catch((err: unknown) => {
        toast('Not killed: ' + (err instanceof Error ? err.message : String(err)));
      })
      .finally(() => {
        btn.removeAttribute('data-busy');
        store.scheduleRender();
      });
  });

  byId('in-form').addEventListener('submit', (e) => {
    e.preventDefault();
    sendCmd(byId<HTMLInputElement>('in-cmd').value);
  });

  const quickEl = byId('quick');
  for (const [cmd, title] of QUICK_COMMANDS) {
    const b = el('button', 'btn sm', cmd);
    b.type = 'button';
    b.title = title;
    b.addEventListener('click', () => sendCmd(cmd));
    quickEl.appendChild(b);
  }

  byId('tx-follow').addEventListener('change', function (this: HTMLInputElement) {
    store.follow = this.checked;
    if (store.follow) store.scheduleRender();
  });
  byId('tx-jump').addEventListener('click', () => {
    const wrap = byId('txwrap');
    wrap.scrollTop = wrap.scrollHeight;
    byId<HTMLInputElement>('tx-follow').checked = true;
    store.follow = true;
  });

  /* export: the viewed VM's transcript, or the whole run sectioned by VM —
     plain text, Blob download, nothing touches the DOM as markup */
  byId('tx-export-vm').addEventListener('click', () => {
    const run = store.openRun;
    const vm = currentVm();
    if (!run || !vm) return;
    const lines = store.linesFor(run.id, vm.id, vm.serial);
    if (lines.length === 0) {
      toast(`No serial captured for vm ${vm.id} yet`);
      return;
    }
    downloadText(`agentplaza-${run.id}-vm${vm.id}.txt`, vmTranscriptText(lines));
    toast(`vm ${vm.id} transcript exported — ${fmtNum(lines.length)} lines`);
  });
  byId('tx-export-run').addEventListener('click', () => {
    const run = store.openRun;
    if (!run || run.vms.length === 0) return;
    downloadText(
      `agentplaza-${run.id}.txt`,
      runTranscriptText(run, (vm) => store.linesFor(run.id, vm.id, vm.serial)),
    );
    toast(`Run ${run.id} exported — ${run.vms.length} VM ${run.vms.length === 1 ? 'section' : 'sections'}`);
  });
  /* the run's report: the register's facts header, then the sectioned
     transcripts with each completed session's summary line */
  byId('tx-export-report').addEventListener('click', () => {
    const run = store.openRun;
    if (!run || run.vms.length === 0) return;
    downloadText(
      `agentplaza-${run.id}-report.txt`,
      runReportText(run, (vm) => store.linesFor(run.id, vm.id, vm.serial)),
    );
    toast(`Run ${run.id} report exported — facts and ${run.vms.length} ${run.vms.length === 1 ? 'section' : 'sections'}`);
  });
}

/* ------------------------------------------------------------ key helpers
   `f` toggles tail-follow, `j` jumps to the latest line (main.ts routes the
   keys here); both mirror the checkbox and button exactly. */
export function followKey(): void {
  store.follow = !store.follow;
  byId<HTMLInputElement>('tx-follow').checked = store.follow;
  if (store.follow) {
    const wrap = byId('txwrap');
    wrap.scrollTop = wrap.scrollHeight;
  }
  toast(store.follow ? 'Following new lines' : 'Follow paused');
}

export function jumpKey(): void {
  byId<HTMLInputElement>('tx-follow').checked = true;
  store.follow = true;
  const wrap = byId('txwrap');
  wrap.scrollTop = wrap.scrollHeight;
}
