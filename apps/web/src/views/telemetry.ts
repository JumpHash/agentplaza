/* Telemetry: the session's own ledger of figures — guest RAM committed, VMs
   live, serial chatter — drawn on canvas 2D from the store's rolling sample
   buffer. The charts follow the sample console's figures: ruled gridlines,
   an ink baseline, mono axis labels, the series in ink; the selected run's
   band is the one orange stroke. An account book leaves unobserved time
   blank: where polling paused (a hidden tab, an unreachable node) the line
   breaks and a dotted margin rule marks the gap. Fewer than three
   observations draw no figure — a quiet note instead. Nothing guest-derived
   reaches these canvases: the only labels are run ids and computed numbers. */

import { byId, el, toast } from '../dom';
import { downloadText, telemetryCsv } from '../export';
import { fmtDur, fmtNum, fmtTime, pad } from '../format';
import type { FleetSample, RunSlice } from '../models/history';
import { seqOf } from '../models/run';
import { store } from '../store';
import { MONO_SM, palette } from './plan';

const GAP_MS = 5000; // a longer silence than this breaks the line — unobserved time stays blank
const MIN_SAMPLES = 3; // fewer draws no figure, a quiet note instead
const TABLE_ROWS = 10;

interface ChartSpec {
  canvasId: string;
  noteId: string;
  capId: string; // the group header's count slot
  label: string;
  unit: string;
  fmt: (v: number) => string;
  value: (s: FleetSample) => number;
  slice: (r: RunSlice | undefined) => number;
  /** The fleet value is the sum of its run slices — the chart can stack per-run bands. */
  additive: boolean;
}

const fmtInt = (v: number): string => fmtNum(Math.round(v));
const fmtRate = (v: number): string => (v >= 10 ? String(Math.round(v)) : v.toFixed(1));

const CHARTS: readonly ChartSpec[] = [
  {
    canvasId: 'tl-ram', noteId: 'tl-ram-note', capId: 'tl-ram-c',
    label: 'Committed guest RAM', unit: 'MiB', fmt: fmtInt,
    value: (s) => s.ramMib, slice: (r) => r?.ramMib ?? 0, additive: true,
  },
  {
    canvasId: 'tl-vms', noteId: 'tl-vms-note', capId: 'tl-vms-c',
    label: 'Live VMs', unit: 'VMs', fmt: fmtInt,
    value: (s) => s.vms, slice: (r) => r?.vms ?? 0, additive: true,
  },
  {
    canvasId: 'tl-lps', noteId: 'tl-lps-note', capId: 'tl-lps-c',
    label: 'Serial throughput', unit: 'lines/s', fmt: fmtRate,
    value: (s) => s.lps, slice: (r) => r?.lps ?? 0, additive: false,
  },
];

const LEFT = 56; // y-axis label gutter
const RIGHT = 10;
const TOP = 8;
const BOT = 22; // x-axis label row

/* ------------------------------------------------------------ rendering */
export function renderTelemetry(): void {
  if (store.route.name !== 'telemetry') return;

  const notice = byId('tl-notice');
  if (store.runsError != null && store.runs.length === 0) {
    notice.hidden = false;
    byId('tl-notice-text').textContent =
      `The node daemon is not answering at ${location.origin} (${store.runsError}). Retrying.`;
  } else {
    notice.hidden = true;
  }

  /* a picked run dies with its register entry — fall back to the fleet */
  if (store.telemetryRun != null && !store.runs.some((r) => r.id === store.telemetryRun)) {
    store.telemetryRun = null;
  }

  const live = byId('tl-live');
  live.hidden = store.health !== 'live';
  live.textContent = store.health;

  const latest = store.history.latest;
  byId('tl-t-mem').textContent = latest ? fmtNum(latest.ramMib) : '—';
  byId('tl-t-vms').textContent = latest ? fmtNum(latest.vms) : '—';
  byId('tl-t-lps').textContent = latest ? fmtRate(latest.lps) : '—';
  byId('tl-samples').textContent = fmtNum(store.history.fleet.length);

  renderScope();
  for (const spec of CHARTS) drawChart(spec);
  renderTable();
}

/* ------------------------------------------------------------ the run pick */
let scopeRunSig: string | null = null;
/* 'fleet' or a run id — kept so a re-pick updates aria-pressed in place and
   the focused button is never rebuilt out from under the owner's finger */
const scopeBtns = new Map<string, HTMLButtonElement>();

function renderScope(): void {
  const runSig = store.runs.map((r) => r.id).join('|');
  const scope = byId('tl-scope');
  if (runSig !== scopeRunSig) {
    scopeRunSig = runSig;
    scopeBtns.clear();
    scope.textContent = '';
    const mkBtn = (id: string, label: string): void => {
      const b = el('button', 'tlink', label);
      b.type = 'button';
      b.addEventListener('click', () => {
        store.telemetryRun = store.telemetryRun === id ? null : id;
        store.scheduleRender();
      });
      scope.appendChild(b);
      scopeBtns.set(id, b);
    };
    mkBtn('fleet', 'The whole fleet');
    for (const r of store.runs) mkBtn(r.id, r.id);
  }
  const picked = store.telemetryRun ?? 'fleet';
  for (const [id, b] of scopeBtns) b.setAttribute('aria-pressed', String(id === picked));

  const note = byId('tl-sel');
  if (store.telemetryRun == null) {
    note.textContent = store.runs.length === 0
      ? 'The register is blank — figures land here as runs are entered.'
      : 'The whole fleet — pick a run to split the figures into per-run bands.';
    return;
  }
  const slice = store.history.latest?.runs[store.telemetryRun];
  note.textContent = slice
    ? `${store.telemetryRun} — ${fmtNum(slice.vms)} live · ${fmtNum(slice.ramMib)} MiB committed · ` +
      `${fmtRate(slice.lps)} lines/s now. Its band is the orange one.`
    : `${store.telemetryRun} — no observation yet.`;
}

/* ------------------------------------------------------------ charts */
/** The axis ceiling: the smallest round 1 / 2 / 2.5 / 5 × 10^k that holds v. */
function niceCeil(v: number): number {
  if (!(v > 0)) return 1;
  const p = 10 ** Math.floor(Math.log10(v));
  const d = v / p;
  return (d <= 1 ? 1 : d <= 2 ? 2 : d <= 2.5 ? 2.5 : d <= 5 ? 5 : 10) * p;
}

function drawChart(spec: ChartSpec): void {
  const canvas = byId<HTMLCanvasElement>(spec.canvasId);
  const note = byId(spec.noteId);
  const samples = store.history.fleet;

  if (samples.length < MIN_SAMPLES) {
    canvas.hidden = true;
    byId(spec.capId).textContent = '';
    canvas.setAttribute('aria-label', spec.label + ': not enough observations yet');
    note.textContent = samples.length === 0
      ? 'Nothing observed yet — the figure lands as the register is polled.'
      : `Only ${samples.length} ${samples.length === 1 ? 'observation' : 'observations'} so far — ` +
        'the figure lands as the register is polled.';
    return;
  }
  canvas.hidden = false;

  const g = canvas.getContext('2d');
  if (!g) return;
  const cssWidth = canvas.clientWidth || canvas.parentElement?.clientWidth || 800;
  const cssHeight = canvas.clientHeight || 150;
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  const w = Math.round(cssWidth * dpr);
  const h = Math.round(cssHeight * dpr);
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, cssWidth, cssHeight);
  const p = palette();

  const t0 = samples[0]?.at ?? 0;
  const t1 = samples[samples.length - 1]?.at ?? t0;
  const span = Math.max(1, t1 - t0);
  const plotW = cssWidth - LEFT - RIGHT;
  const plotH = cssHeight - TOP - BOT;
  const xAt = (at: number): number => LEFT + ((at - t0) / span) * plotW;

  /* gaps: silences longer than GAP_MS — the pen lifts there */
  const gapBefore = samples.map((s, i) => i > 0 && s.at - (samples[i - 1]?.at ?? s.at) > GAP_MS);

  const max = niceCeil(Math.max(...samples.map(spec.value)));
  const yAt = (v: number): number => TOP + plotH - (v / max) * plotH;

  /* the ruling: half and full gridlines, the ink baseline, mono labels */
  g.lineWidth = 1;
  g.font = MONO_SM;
  g.textBaseline = 'middle';
  for (const frac of [0.5, 1]) {
    const y = yAt(max * frac);
    g.strokeStyle = p.rule;
    g.beginPath();
    g.moveTo(LEFT, Math.round(y) + 0.5);
    g.lineTo(LEFT + plotW, Math.round(y) + 0.5);
    g.stroke();
    g.fillStyle = p.ink3;
    g.textAlign = 'right';
    g.fillText(spec.fmt(max * frac), LEFT - 7, y);
  }
  g.strokeStyle = p.ink;
  g.beginPath();
  g.moveTo(LEFT, Math.round(yAt(0)) + 0.5);
  g.lineTo(LEFT + plotW, Math.round(yAt(0)) + 0.5);
  g.stroke();

  /* session bounds on the time axis */
  g.fillStyle = p.ink3;
  g.textAlign = 'left';
  g.fillText(fmtTime(t0), LEFT, cssHeight - 8);
  g.textAlign = 'right';
  g.fillText(fmtTime(t1), LEFT + plotW, cssHeight - 8);
  if (plotW > 260) {
    g.textAlign = 'center';
    g.fillText(fmtTime(t0 + span / 2), LEFT + plotW / 2, cssHeight - 8);
  }
  g.textAlign = 'left';

  /* gap rules: a dotted margin line wherever the register went unobserved */
  g.strokeStyle = p.margin;
  g.globalAlpha = 0.55;
  g.setLineDash([2, 3]);
  samples.forEach((s, i) => {
    if (!gapBefore[i]) return;
    const gx = (xAt(samples[i - 1]?.at ?? s.at) + xAt(s.at)) / 2;
    g.beginPath();
    g.moveTo(Math.round(gx) + 0.5, TOP);
    g.lineTo(Math.round(gx) + 0.5, TOP + plotH);
    g.stroke();
  });
  g.setLineDash([]);
  g.globalAlpha = 1;

  const sel = store.telemetryRun;
  const selSlice = sel != null;

  /* per-run stacked bands (additive charts, a run picked): internal divisions
     in faint ink, the picked run's band outlined in the signal colour */
  if (selSlice && spec.additive && sel != null) {
    const ids = new Set<string>();
    for (const s of samples) {
      for (const [id, r] of Object.entries(s.runs)) {
        if (spec.slice(r) > 0) ids.add(id);
      }
    }
    const bandIds = [...ids].sort((a, b) => seqOf(a) - seqOf(b));
    /* contiguous segments between gaps; each band is a closed outline:
       its floor is the previous band's roof (the baseline for the first) */
    let segStart = 0;
    const segments: Array<[number, number]> = [];
    for (let i = 1; i < samples.length; i++) {
      if (gapBefore[i]) {
        segments.push([segStart, i - 1]);
        segStart = i;
      }
    }
    segments.push([segStart, samples.length - 1]);

    for (const [a, b] of segments) {
      const xs: number[] = [];
      const roofs: number[][] = bandIds.map(() => []); // cumulative top per band per sample
      for (let i = a; i <= b; i++) {
        const s = samples[i];
        if (!s) continue;
        xs.push(xAt(s.at));
        let cum = 0;
        bandIds.forEach((bid, k) => {
          cum += spec.slice(s.runs[bid]);
          roofs[k]?.push(yAt(cum));
        });
      }
      bandIds.forEach((bid, k) => {
        const isSel = bid === sel;
        g.strokeStyle = isSel ? p.signal : p.ink3;
        g.lineWidth = isSel ? 1.5 : 0.75;
        const roof = roofs[k] ?? [];
        const floor = k === 0 ? xs.map(() => yAt(0)) : (roofs[k - 1] ?? []);
        g.beginPath();
        xs.forEach((x, j) => {
          if (j === 0) g.moveTo(x, floor[j] ?? 0);
          else g.lineTo(x, floor[j] ?? 0);
        });
        for (let j = xs.length - 1; j >= 0; j--) g.lineTo(xs[j] ?? 0, roof[j] ?? 0);
        g.closePath();
        g.stroke();
      });
    }
    /* the picked band's label, at its newest tall-enough point */
    const last = samples[samples.length - 1];
    if (last) {
      let cum = 0;
      let selH = 0;
      let selY = 0;
      for (const bid of bandIds) {
        const v = spec.slice(last.runs[bid]);
        if (bid === sel) {
          selH = yAt(cum) - yAt(cum + v);
          selY = (yAt(cum) + yAt(cum + v)) / 2;
        }
        cum += v;
      }
      if (selH >= 12) {
        g.font = MONO_SM;
        g.fillStyle = p.signal;
        g.textAlign = 'right';
        g.fillText(sel, LEFT + plotW - 4, selY);
        g.textAlign = 'left';
      }
    }
  }

  /* the fleet line itself — ink, lifting the pen over gaps */
  g.strokeStyle = p.ink;
  g.lineWidth = 1.5;
  g.beginPath();
  samples.forEach((s, i) => {
    const x = xAt(s.at);
    const y = yAt(spec.value(s));
    if (i === 0 || gapBefore[i]) g.moveTo(x, y);
    else g.lineTo(x, y);
  });
  g.stroke();

  /* the picked run's chatter laid over the fleet's (non-additive chart) */
  if (selSlice && !spec.additive && sel != null) {
    g.strokeStyle = p.signal;
    g.lineWidth = 1.5;
    g.beginPath();
    samples.forEach((s, i) => {
      const x = xAt(s.at);
      const y = yAt(spec.slice(s.runs[sel]));
      if (i === 0 || gapBefore[i]) g.moveTo(x, y);
      else g.lineTo(x, y);
    });
    g.stroke();
    const last = samples[samples.length - 1];
    if (last) {
      const y = yAt(spec.slice(last.runs[sel]));
      g.font = MONO_SM;
      g.fillStyle = p.signal;
      g.textAlign = 'right';
      g.fillText(sel, LEFT + plotW - 4, Math.max(TOP + 6, y - 8));
      g.textAlign = 'left';
    }
  }

  /* the newest observation: the register's blot */
  const newest = samples[samples.length - 1];
  if (newest) {
    g.fillStyle = p.ink;
    g.fillRect(xAt(newest.at) - 2, yAt(spec.value(newest)) - 2, 4, 4);
  }

  /* caption + accessible name mirror the figure in prose */
  const peak = Math.max(...samples.map(spec.value));
  const cur = newest ? spec.value(newest) : 0;
  const gaps = gapBefore.filter(Boolean).length;
  const windowText = `${fmtTime(t0)}–${fmtTime(t1)}`;
  byId(spec.capId).textContent = `now ${spec.fmt(cur)} ${spec.unit}`;
  note.textContent =
    `now ${spec.fmt(cur)} ${spec.unit} · peak ${spec.fmt(peak)} ${spec.unit} · ` +
    `${fmtNum(samples.length)} observations (${windowText})` +
    (gaps > 0 ? ' · blank stretches are time the register went unobserved' : '') +
    (selSlice ? ` · ${sel ?? ''} in orange` : '');
  canvas.setAttribute(
    'aria-label',
    `${spec.label} over the last ${fmtDur(span)}: now ${spec.fmt(cur)} ${spec.unit}, ` +
    `peak ${spec.fmt(peak)} ${spec.unit}` +
    (gaps > 0 ? `, ${gaps} unobserved ${gaps === 1 ? 'stretch' : 'stretches'}` : '') +
    (selSlice ? `, run ${sel ?? ''} singled out` : ''),
  );
}

/* ------------------------------------------------------------ the table */
function renderTable(): void {
  const tb = byId('tl-body');
  tb.textContent = '';
  const samples = store.history.fleet;
  byId('tl-obs-c').textContent =
    samples.length === 0 ? '' : `${fmtNum(samples.length)} kept of ${fmtNum(store.history.pushed)}`;
  const tail = samples.slice(-TABLE_ROWS).reverse();
  if (tail.length === 0) {
    const tr = el('tr');
    const td = el('td', null, 'No observations yet — the register writes one per poll.');
    td.colSpan = 6;
    tr.appendChild(td);
    tb.appendChild(tr);
    return;
  }
  for (const s of tail) {
    const tr = el('tr');
    tr.appendChild(el('td', null, pad(s.no, 4)));
    tr.appendChild(el('td', 'm', fmtTime(s.at)));
    tr.appendChild(el('td', 'num', fmtNum(s.vms)));
    tr.appendChild(el('td', 'num', fmtNum(s.ramMib)));
    tr.appendChild(el('td', 'num', fmtRate(s.lps)));
    const ids = Object.entries(s.runs).filter(([, r]) => r.vms > 0).map(([id]) => id);
    tr.appendChild(el('td', 'm',
      ids.length === 0 ? '—' : ids.slice(0, 4).join(' ') + (ids.length > 4 ? ` +${ids.length - 4}` : '')));
    tb.appendChild(tr);
  }
}

/* ------------------------------------------------------------ export */
export function initTelemetry(): void {
  byId('tl-export').addEventListener('click', () => {
    const samples = store.history.fleet;
    if (samples.length === 0) {
      toast('Nothing observed yet — the session lands as the register is polled');
      return;
    }
    const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-');
    downloadText(`agentplaza-telemetry-${stamp}.csv`, telemetryCsv(samples), 'text/csv;charset=utf-8');
    toast(`Session telemetry exported — ${fmtNum(samples.length)} observations`);
  });
}
