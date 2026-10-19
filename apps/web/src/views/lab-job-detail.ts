// One job (`#/lab/jobs/<id>`), in the prototype's layout: the head (id, status, Cancel while the job is
// live), four stats, the status timeline beside the budget, the runs, the datasets, the plan, then the
// console's live charts (loss and learning rate per island, eval metrics), the loose-mode rounds and
// the trials, per-device utilization, a log tail, the outputs and metrics. The job view reloads on each
// `training` frame for this job (at most once a second; the last one trails) and on a 4 s poll. The
// charts draw the job's series plus the live lines the view has not absorbed yet (a line whose seq is
// above the view's seq), so the desktop draws the same numbers.
// Contract: `render(root, params)` fills `root` and returns its cleanup; `params[0]` is the job id.

import { ApiError, cancelJob, getJob, jobEvents, type JobEvent, type JobRun, type JobStatus, type JobView, type SeriesPoint } from '../api';
import { href } from '../router';
import { stackedArea, type AreaData, type ChartHandle } from '../ui/chart';
import { card, modal, stat, table as dataTable, toast, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { agoText, errorText, fmtPercent, labButton, labEmpty, onTraining, pollEvery, type TrainingFrame } from './lab-common';
import { planDiagram } from './lab-jobs-plan';

const POLL_MS = 4000;
const THROTTLE_MS = 1000;
const LIVE_KEPT = 2000;
const LOG_KEPT = 300;
const LINE_MAX = 240;
const TONE: Record<JobStatus, 'muted' | 'info' | 'ok' | 'danger' | 'warn'> = { queued: 'muted', running: 'info', done: 'ok', failed: 'danger', cancelled: 'warn' };
const PALETTE = ['var(--chart-3)', 'var(--chart-4)', 'var(--chart-5)', 'var(--chart-6)', 'var(--chart-7)', 'var(--chart-8)'];
const LOSS_COLOR = 'var(--chart-1)';
const LR_COLOR = 'var(--chart-2)';

const fix4 = (v: number): string => v.toFixed(4);
const fixExp = (v: number): string => v.toExponential(2);
const isNum = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const dash = (v: number | null | undefined, digits = 2): string => (isNum(v) ? v.toFixed(digits) : '—');
const cap = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);
const isLive = (s: JobStatus): boolean => s === 'queued' || s === 'running';

/** A trainer event as the job's series keeps it: the same mapping the apid's `absorb` makes. */
function pointOf(island: string, ev: { ev: string } & Record<string, unknown>, t: number): SeriesPoint | null {
  const num = (k: string): number | undefined => (isNum(ev[k]) ? (ev[k] as number) : undefined);
  switch (ev.ev) {
    case 'step':
      return { kind: 'step', island, t, step: num('step'), loss: num('loss'), lr: num('lr'), samples: num('samples') };
    case 'eval':
      return {
        kind: 'eval',
        island,
        t,
        step: num('step'),
        split: typeof ev.split === 'string' ? ev.split : undefined,
        metrics: ev.metrics && typeof ev.metrics === 'object' ? (ev.metrics as Record<string, number>) : undefined,
      };
    case 'round':
      return {
        kind: 'round',
        island,
        t,
        round: num('round'),
        accepted: typeof ev.accepted === 'boolean' ? ev.accepted : undefined,
        hub_round: num('hub_round'),
        step: num('step'),
      };
    case 'trial':
      return { kind: 'trial', island, t, trial: num('trial'), objective: num('objective') };
    default:
      return null;
  }
}

/** A time of day, local, `HH:MM:SS`; a dash for no time (the desktop's `clock`). */
function clock(ms: number): string {
  if (!ms) return '—';
  const d = new Date(ms);
  const p = (n: number): string => String(n).padStart(2, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** A live `training` frame with the time it arrived: the time of a live point the view has not absorbed yet. */
type LiveFrame = TrainingFrame & { at: number };

/** The job's series, then the live lines the view has not absorbed (seq above the view's seq), in seq order. */
function pointsOf(view: JobView, frames: LiveFrame[]): SeriesPoint[] {
  const fresh = new Map<number, { line: JobEvent; at: number }>();
  for (const f of frames) if (f.line.seq > view.seq && !fresh.has(f.line.seq)) fresh.set(f.line.seq, { line: f.line, at: f.at });
  const extra = [...fresh.values()]
    .sort((a, b) => a.line.seq - b.line.seq)
    .map((l) => pointOf(l.line.island, l.line.event, l.at))
    .filter((p): p is SeriesPoint => p !== null);
  return [...view.series, ...extra];
}

interface ChartSpec {
  key: string;
  short: string;
  title: string;
  labels: string[];
  heads: string[];
  values: number[];
  format: (v: number) => string;
  summary: string;
  color: string;
}

/** Loss and learning rate per island, then one chart per eval metric (by name). */
function chartSpecs(points: SeriesPoint[]): ChartSpec[] {
  const specs: ChartSpec[] = [];
  const build = (
    pts: SeriesPoint[],
    pick: (p: SeriesPoint) => number,
    key: string,
    title: string,
    short: string,
    format: (v: number) => string,
    color: string,
    best: boolean,
  ): void => {
    if (!pts.length) return;
    const values = pts.map(pick);
    const last = pts[pts.length - 1] as SeriesPoint;
    const parts = [`last ${format(pick(last))} at step ${last.step ?? '?'}`];
    if (best) parts.push(`best ${format(Math.min(...values))}`);
    if (values.length < 2) parts.push('one point so far');
    specs.push({
      key,
      short,
      title,
      labels: pts.map((p) => String(p.step ?? '?')),
      heads: pts.map((p) => `step ${p.step ?? '?'} · ${clock(p.t)}`),
      values,
      format,
      summary: `${title} · ${parts.join(' · ')}`,
      color,
    });
  };
  const steps = points.filter((p) => p.kind === 'step');
  for (const isl of [...new Set(steps.map((p) => p.island))]) {
    build(steps.filter((p) => p.island === isl && isNum(p.loss)), (p) => p.loss as number, `loss:${isl}`, `Loss · island ${isl}`, 'loss', fix4, LOSS_COLOR, true);
    build(steps.filter((p) => p.island === isl && isNum(p.lr)), (p) => p.lr as number, `lr:${isl}`, `Learning rate · island ${isl}`, 'lr', fixExp, LR_COLOR, false);
  }
  const evals = points.filter((p) => p.kind === 'eval' && p.metrics);
  [...new Set(evals.flatMap((p) => Object.keys(p.metrics ?? {})))].sort().forEach((name, i) => {
    const pts = evals.filter((p) => isNum(p.metrics?.[name]));
    build(pts, (p) => p.metrics?.[name] as number, `eval:${name}`, `Eval · ${name}`, name, fix4, PALETTE[i % PALETTE.length] ?? LOSS_COLOR, false);
  });
  return specs;
}

/** A value with its object keys sorted, so a nested object prints the same on both consoles. */
function canon(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canon).join(',')}]`;
  if (v && typeof v === 'object') {
    const rec = v as Record<string, unknown>;
    return `{${Object.keys(rec).sort().map((k) => `${JSON.stringify(k)}:${canon(rec[k])}`).join(',')}}`;
  }
  return JSON.stringify(v) ?? 'null';
}

/** One log line: the event's name, then its other fields by name. */
function logText(ev: { ev: string } & Record<string, unknown>): string {
  const show = (v: unknown): string => (typeof v === 'string' ? v : canon(v));
  const fields = Object.keys(ev)
    .filter((k) => k !== 'ev')
    .sort()
    .map((k) => `${k}=${show(ev[k])}`);
  const line = [ev.ev, ...fields].join(' ');
  return line.length > LINE_MAX ? `${line.slice(0, LINE_MAX - 1)}…` : line;
}

interface Stage { label: string; at: number | null; sub: string; tone: 'muted' | 'info' | 'ok' | 'danger' | 'warn' }

/** The timeline: queued, running (from the first run's start), and the end (the status, its time and any error). */
function stagesOf(v: JobView): Stage[] {
  const terminal = !isLive(v.status);
  const starts = v.runs.map((r) => r.started_ms).filter((t) => t > 0);
  const runSub = v.runs.length ? `${v.runs.length} run${v.runs.length === 1 ? '' : 's'}` : terminal ? 'Never started.' : 'Not started yet.';
  const endLabel = terminal ? cap(v.status) : 'Finished';
  return [
    { label: 'Queued', at: v.created_ms, sub: v.status === 'queued' ? 'Waits for a free device.' : '', tone: 'muted' },
    { label: `Running${v.runs.length ? ` (${v.runs.length} run${v.runs.length === 1 ? '' : 's'})` : ''}`, at: starts.length ? Math.min(...starts) : null, sub: v.runs.length ? '' : runSub, tone: 'info' },
    { label: endLabel, at: terminal ? v.updated_ms : null, sub: v.status === 'failed' && v.error ? v.error : '', tone: TONE[v.status] },
  ];
}

/** Each run's devices (a CPU run has none) with the device-hours they used and their share of the job's total. */
function deviceRows(runs: JobRun[]): { node: string; device: string; island: string; state: string; hours: number; share: number }[] {
  const total = runs.reduce((sum, r) => sum + r.gpu_hours, 0);
  return runs.flatMap((run) => {
    const slots: (number | null)[] = run.devices.length ? run.devices : [null];
    const hours = run.gpu_hours / slots.length;
    return slots.map((d) => ({
      node: run.node,
      device: d === null ? 'cpu' : `dev ${d}`,
      island: run.island,
      state: run.state,
      hours,
      share: total > 0 ? hours / total : 0,
    }));
  });
}

function metricText(v: unknown): string {
  if (typeof v === 'number') return Number.isInteger(v) ? String(v) : fix4(v);
  if (typeof v === 'string') return v;
  return canon(v);
}

/** A plain table of text cells (numbers right-aligned), in the prototype's `div.tbl-wrap > table.tbl`. */
function plainTable(head: string[], rows: (Node | string)[][], numeric: number[] = []): HTMLElement {
  const cols: Column<(Node | string)[]>[] = head.map((label, i) => ({ label, num: numeric.includes(i), cell: (row) => row[i] ?? null }));
  return h('div.tbl-wrap', null, dataTable(cols, rows));
}

/** A labelled key-value list of the prototype's `dl.kv`. */
function kvList(pairs: [string, Node | string][]): HTMLElement {
  return h('dl.kv', null, pairs.map(([k, v]) => h('div', null, h('dt', null, k), h('dd', null, v))));
}

/** The status timeline: `ol.lab-steps`, one dot per stage. */
function timelineOf(stages: Stage[]): HTMLElement {
  return h(
    'ol.lab-steps',
    null,
    stages.map((s) =>
      h(
        'li',
        null,
        h('i.dot', { class: `t-${s.tone}` }),
        h('div', null, h('b', null, s.label), ' ', h('span.lab-note', null, s.at ? agoText(s.at) : '—'), s.sub ? h('div.lab-err', null, s.sub) : null),
      ),
    ),
  );
}

interface Slot {
  wrap: HTMLElement;
  host: HTMLElement;
  sum: HTMLElement;
  chart: ChartHandle<AreaData>;
  spec: ChartSpec | null;
}

export function render(root: HTMLElement, params: string[]): Cleanup {
  const id = params[0] ?? '';
  let alive = true;
  let view: JobView | null = null;
  let notFound = false;
  let loadError: string | null = null;
  let logs: JobEvent[] = [];
  let logsNext = 0;
  let frames: LiveFrame[] = [];
  let inflight = false;
  let dirty = false;
  let lastAt = 0;
  let trailing: ReturnType<typeof setTimeout> | undefined;
  let busy = false;

  // The charts keep their elements and handles across redraws, so each one animates from its last values.
  const slots = new Map<string, Slot>();
  const chartGrid = h('div.lab-job-charts');

  const slotFor = (key: string): Slot => {
    const have = slots.get(key);
    if (have) return have;
    const host = h('div.lab-job-chart-host');
    const sum = h('div.lab-job-chart-sum');
    const wrap = h('div.lab-job-chart', null, host, sum);
    const slot: Slot = { wrap, host, sum, chart: undefined as unknown as ChartHandle<AreaData>, spec: null };
    slot.chart = stackedArea(host, { labels: [], layers: [] }, {
      format: (v) => (slot.spec ? slot.spec.format(v) : fix4(v)),
      height: 220,
      ariaLabel: key,
      tooltip: (i) => {
        const s = slot.spec;
        if (!s) return h('span');
        const v = s.values[i];
        return h('div.lab-job-tip', null, h('b', null, s.heads[i] ?? ''), h('span', null, v === undefined ? '—' : `${s.short}  ${s.format(v)}`));
      },
    });
    slots.set(key, slot);
    return slot;
  };

  const pageOf = (v: JobView): HTMLElement => {
    const points = pointsOf(v, frames);
    const specs = chartSpecs(points);
    const wanted: HTMLElement[] = [];
    for (const spec of specs) {
      const slot = slotFor(spec.key);
      slot.spec = spec;
      slot.host.hidden = spec.values.length < 2;
      if (!slot.host.hidden) slot.chart.update({ labels: spec.labels, layers: [{ key: spec.key, label: spec.title, color: spec.color, values: spec.values }] });
      slot.sum.textContent = spec.summary;
      wanted.push(slot.wrap);
    }
    chartGrid.replaceChildren(...wanted);

    const live = isLive(v.status);
    const over = v.gpu_hours > v.budget_gpu_hours;
    const pct = v.budget_gpu_hours > 0 ? Math.min(100, (v.gpu_hours / v.budget_gpu_hours) * 100) : 0;

    const cancelBtn = labButton('Cancel job', () => confirmCancel(v), { kind: 'danger', sm: true, title: 'Cancel this job' });
    cancelBtn.disabled = busy;
    const head = h(
      'div.lab-dhead',
      null,
      h('h2.mono', null, v.id),
      h('span.chip', { class: `t-${TONE[v.status]}` }, cap(v.status)),
      h('span.sp'),
      live ? cancelBtn : null,
    );
    const stats = h(
      'div.lab-stats',
      null,
      stat('Recipe', v.recipe),
      stat('Mode', v.mode),
      stat('Created', agoText(v.created_ms)),
      stat('Updated', agoText(v.updated_ms)),
    );

    const budget = card(
      'Budget',
      [
        kvList([
          ['GPU-h used', `${dash(v.gpu_hours)} of ${dash(v.budget_gpu_hours)}`],
          ['Over budget', over ? 'yes' : 'no'],
        ]),
        h('div.lab-meter.lab-meter-lg', { class: over ? 'is-over' : null }, h('i', { style: `width:${pct.toFixed(1)}%` })),
      ],
    );
    budget.classList.toggle('lab-over', over);

    const runs = v.runs.length
      ? plainTable(
          ['Run', 'Node', 'Island', 'Devices', 'State', 'Started', 'Ended', 'GPU-h', 'Result'],
          v.runs.map((r) => [
            h('span.mono', null, r.node_job),
            r.node,
            r.island,
            r.devices.length ? r.devices.join(', ') : 'cpu',
            cap(r.state),
            agoText(r.started_ms),
            r.ended_ms ? agoText(r.ended_ms) : '—',
            dash(r.gpu_hours),
            r.exit_code === null ? '—' : `exit ${r.exit_code}`,
          ]),
          [7],
        )
      : h('p.lab-note', null, 'No runs yet');
    const datasetsCard = card('Datasets', h('p', null, v.datasets.length ? v.datasets.join(', ') : 'none'));
    const planCard = card('Plan', v.plan ? planDiagram(v.plan) : h('p.lab-note', null, 'No plan is recorded for this job.'));

    const chartsBody: Node[] = [];
    if (!points.length) chartsBody.push(h('p.lab-note', null, "Waiting for the trainer's first step. Queued jobs wait for a free device."));
    if (specs.length) chartsBody.push(chartGrid);
    const rounds = points.filter((p) => p.kind === 'round');
    if (rounds.length) {
      const accepted = rounds.filter((p) => p.accepted === true).length;
      chartsBody.push(
        h('p.lab-note', null, `Rounds: ${rounds.length}, accepted ${accepted}.`),
        plainTable(
          ['Round', 'Island', 'Accepted', 'Hub round'],
          rounds.map((p) => [String(p.round ?? '—'), p.island, p.accepted === undefined ? '—' : p.accepted ? 'yes' : 'no', String(p.hub_round ?? '—')]),
          [0, 3],
        ),
      );
    }
    const trials = points.filter((p) => p.kind === 'trial' && isNum(p.objective));
    if (trials.length) {
      chartsBody.push(h('p.lab-note', null, 'Trials'), plainTable(['Trial', 'Objective'], trials.map((p) => [String(p.trial ?? '—'), fix4(p.objective as number)]), [0, 1]));
    }
    const charts = card('Charts', chartsBody.length ? chartsBody : h('p.lab-note', null, 'No charts yet.'));

    const devRows = deviceRows(v.runs);
    const devices = card(
      'Devices',
      devRows.length
        ? plainTable(
            ['Node', 'Device', 'Island', 'State', 'GPU-h', 'Share of job'],
            devRows.map((r) => [
              r.node,
              r.device,
              r.island,
              cap(r.state),
              dash(r.hours),
              h('span.lab-job-share', null, h('span.lab-meter', null, h('i', { style: `width:${(r.share * 100).toFixed(1)}%` })), h('span.num', null, fmtPercent(r.share))),
            ]),
            [4],
          )
        : h('p.lab-note', null, 'No runs yet. A job gets its devices when it is dispatched.'),
    );

    const log = card(
      'Log tail',
      h(
        'div.lab-log',
        null,
        logs.length ? logs.map((e) => h('div', null, `#${e.seq} ${e.island} ${logText(e.event)}`)) : h('div.lab-note', null, 'No events yet.'),
      ),
      { actions: [h('span.lab-note', null, `${logs.length} events`)] },
    );

    const outputBody: Node[] = [
      v.metrics && Object.keys(v.metrics).length
        ? plainTable(
            ['Metric', 'Value'],
            Object.entries(v.metrics)
              .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
              .map(([k, val]) => [k, metricText(val)]),
            [1],
          )
        : h('p.lab-note', null, 'No metrics yet.'),
      h('p.lab-note', null, 'Outputs'),
      v.outputs.length ? h('ul', null, v.outputs.map((o) => h('li.mono', null, o))) : h('p.lab-note', null, 'No outputs yet.'),
    ];
    if (v.outputs_remote) outputBody.push(h('p.lab-note', null, 'The outputs stay on the node that trained them.'));
    const outputs = card('Outputs and metrics', outputBody);

    return h(
      'div.lab-stack.lab-job',
      null,
      head,
      stats,
      loadError ? h('p.lab-err', { role: 'alert' }, `The job view is out of date: ${loadError}`) : null,
      h('div.lab-grid2', null, card('Status', timelineOf(stagesOf(v)), {}), budget),
      h('div.lab-grid2', null, card('Runs', runs), charts),
      h('div.lab-grid2', null, datasetsCard, planCard),
      devices,
      log,
      outputs,
      live ? h('p.lab-note', null, 'This page follows the job live.') : null,
    );
  };

  /** The page for the current state: the job, the unknown-id empty state, or a loading or error line. */
  const paint = (): void => {
    if (!alive) return;
    if (notFound) {
      replace(root, labEmpty('search', 'No job with this id', undefined, h('a.btn', { href: href('lab', 'jobs') }, 'Back to Jobs')));
    } else if (view) {
      replace(root, pageOf(view));
    } else if (loadError) {
      replace(root, h('p.lab-err', { role: 'alert' }, `Could not load the job: ${loadError}`));
    } else {
      replace(root, h('p.lab-note', null, 'Loading the job…'));
    }
  };

  const confirmCancel = (v: JobView): void => {
    let close: () => void = () => undefined;
    const stop = async (): Promise<void> => {
      close();
      busy = true;
      paint();
      try {
        await cancelJob(v.id);
        toast(`${v.id} is cancelled`);
      } catch (err) {
        toast(errorText(err), 'error');
      } finally {
        busy = false;
        void refresh();
      }
    };
    const body = h(
      'div',
      null,
      h('p', null, 'Its runs stop; the GPU time used so far stays on the job.'),
      h('div.lab-actions', null, labButton('Keep running', () => close(), { kind: 'ghost' }), labButton('Cancel job', () => void stop(), { kind: 'danger' })),
    );
    close = modal(`Cancel ${v.id}?`, body);
  };

  /** One load: the job view, then the events after the last one kept. A request while one is in flight waits for it. */
  const refresh = async (): Promise<void> => {
    if (!alive) return;
    if (inflight) {
      dirty = true;
      return;
    }
    inflight = true;
    lastAt = Date.now();
    try {
      view = await getJob(id);
      notFound = false;
      loadError = null;
      try {
        const page = await jobEvents(id, logsNext);
        logs = [...logs, ...page.events].slice(-LOG_KEPT);
        logsNext = Math.max(logsNext, page.next);
      } catch {
        // The log tail keeps what it had; the next load asks again.
      }
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) {
        notFound = true;
        view = null;
      } else {
        loadError = errorText(err);
      }
    } finally {
      inflight = false;
    }
    if (!alive) return;
    paint();
    if (dirty) {
      dirty = false;
      kick();
    }
  };

  /** A `training` frame or a load request: at most one load a second; the last request trails. */
  const kick = (): void => {
    if (!alive || trailing !== undefined) return;
    const wait = Math.max(0, lastAt + THROTTLE_MS - Date.now());
    if (wait === 0 && !inflight) {
      void refresh();
      return;
    }
    trailing = setTimeout(
      () => {
        trailing = undefined;
        kick();
      },
      wait || 200,
    );
  };

  const off = onTraining((frame) => {
    if (!alive) return;
    frames.push({ ...frame, at: Date.now() });
    if (frames.length > LIVE_KEPT) frames = frames.slice(frames.length - LIVE_KEPT);
    kick();
  }, id);
  const stopPoll = pollEvery(POLL_MS, () => void refresh());

  return () => {
    alive = false;
    off();
    stopPoll();
    if (trailing !== undefined) clearTimeout(trailing);
    for (const slot of slots.values()) slot.chart.destroy();
    slots.clear();
  };
}
