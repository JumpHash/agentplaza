// One job (`#/lab/jobs/<id>`): the header, the status timeline, the plan (islands, machines, devices),
// live charts (loss and learning rate per island, eval metrics), the loose-mode rounds and the trials,
// per-device utilization, a log tail, the budget, outputs and metrics, and the error. The job view
// reloads on each `training` frame for this job (at most once a second; the last one trails) and on a
// 4 s poll. The charts draw the job's series plus the live lines the view has not absorbed yet (a
// line whose seq is above the view's seq), so the desktop draws the same numbers.
// Contract: `render(root, params)` fills `root` and returns its cleanup; `params[0]` is the job id.

import '../styles/lab-job.css';
import { ApiError, cancelJob, getJob, jobEvents, type JobEvent, type JobPlan, type JobRun, type JobStatus, type JobView, type SeriesPoint } from '../api';
import { href } from '../router';
import { stackedArea, type AreaData, type ChartHandle } from '../ui/chart';
import { modal, toast } from '../ui/components';
import { h, icon, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { errorText, fmtPercent, fmtTime, onTraining, pollEvery, type TrainingFrame } from './lab-common';

const POLL_MS = 4000;
const THROTTLE_MS = 1000;
const LIVE_KEPT = 2000;
const LOG_KEPT = 300;
const LINE_MAX = 240;
const TONE: Record<JobStatus, string> = { queued: 'muted', running: 'info', done: 'ok', failed: 'danger', cancelled: 'warn' };
const PALETTE = ['var(--chart-3)', 'var(--chart-4)', 'var(--chart-5)', 'var(--chart-6)', 'var(--chart-7)', 'var(--chart-8)'];
const LOSS_COLOR = 'var(--chart-1)';
const LR_COLOR = 'var(--chart-2)';

const fix4 = (v: number): string => v.toFixed(4);
const fixExp = (v: number): string => v.toExponential(2);
const isNum = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const dash = (v: number | null | undefined, digits = 2): string => (isNum(v) ? v.toFixed(digits) : '—');

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

type StageState = 'done' | 'current' | 'pending' | 'skipped';
interface Stage { label: string; at: number | null; sub: string; state: StageState }

/** Three stages: queued, running (from the first run's start), and the end (the status, its time and any error). */
function stagesOf(v: JobView): Stage[] {
  const terminal = v.status === 'done' || v.status === 'failed' || v.status === 'cancelled';
  const starts = v.runs.map((r) => r.started_ms).filter((t) => t > 0);
  const hadRuns = v.runs.length > 0;
  const queued = v.status === 'queued';
  const running = v.status === 'running';
  let runSub = 'Not started yet.';
  if (hadRuns) runSub = `${v.runs.length} run${v.runs.length === 1 ? '' : 's'}`;
  else if (terminal) runSub = 'Never started.';
  let endSub = terminal ? '' : 'Not finished.';
  if (v.status === 'failed' && v.error) endSub = v.error;
  const endLabel = v.status === 'done' ? 'Done' : v.status === 'failed' ? 'Failed' : v.status === 'cancelled' ? 'Cancelled' : 'Finished';
  let runState: StageState = 'pending';
  if (running) runState = 'current';
  else if (terminal) runState = hadRuns ? 'done' : 'skipped';
  return [
    { label: 'Queued', at: v.created_ms, sub: queued ? 'Waits for a free device.' : '', state: queued ? 'current' : 'done' },
    { label: 'Running', at: starts.length ? Math.min(...starts) : null, sub: runSub, state: runState },
    { label: endLabel, at: terminal ? v.updated_ms : null, sub: endSub, state: terminal ? 'current' : 'pending' },
  ];
}

/** The plan's mode, in words (the desktop page says the same). */
function modeText(plan: JobPlan): string {
  if (plan.mode === 'loose') {
    return `Loose: islands train ${plan.inner_steps} inner steps, then average through the hub. Rounds ${plan.rounds ?? '—'}, staleness ${plan.staleness}.`;
  }
  if (plan.mode === 'tight') return "Tight: the machines of each island train together and sync through the island's rendezvous.";
  return 'Single: one trainer on one machine.';
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

function table(head: string[], rows: (Node | string)[][], numeric: number[] = []): HTMLElement {
  return h(
    'div.lab-table-wrap',
    null,
    h(
      'table.lab-table',
      null,
      h('thead', null, h('tr', null, head.map((label, i) => h('th', { class: numeric.includes(i) ? 'num' : '' }, label)))),
      h('tbody', null, rows.map((cells) => h('tr', null, cells.map((cell, i) => h('td', { class: numeric.includes(i) ? 'num' : '' }, cell))))),
    ),
  );
}

function section(title: string, body: Node | Node[], extra: Node[] = []): HTMLElement {
  return h('section.lab-card', null, h('header.lab-card-head', null, h('h3', null, title), ...extra), h('div.lab-card-body', null, body));
}

function stat(label: string, value: string): HTMLElement {
  return h('div.lab-stat', null, h('span.lab-stat-label', null, label), h('span.lab-stat-value', null, value));
}

function planBlock(plan: JobPlan): HTMLElement {
  const islands = plan.islands.map((isl) =>
    h(
      'div.lab-job-island',
      null,
      h('div.lab-job-island-head', null, h('span', null, `island ${isl.id}`), h('span.lab-badge.accent', null, isl.backend || '—')),
      h(
        'div.lab-job-kv',
        null,
        h('span.lab-muted', null, 'Leader'),
        h('span.lab-mono', null, isl.leader || '—'),
        h('span.lab-muted', null, 'Rendezvous'),
        h('span.lab-mono', null, isl.rdzv || '—'),
      ),
      isl.machines.map((m) =>
        h(
          'div.lab-job-machine',
          null,
          h('span.lab-mono', null, m.id),
          m.devices.length ? m.devices.map((d) => h('span.lab-job-dev', null, `dev ${d}`)) : h('span.lab-muted', null, 'no devices'),
        ),
      ),
    ),
  );
  const datasets = Object.entries(plan.datasets);
  return h(
    'div.lab-stack',
    null,
    h('p', null, modeText(plan)),
    h('div.lab-job-islands', null, islands.length ? islands : h('p.lab-muted', null, 'No islands.')),
    h(
      'div.lab-job-plan-meta',
      null,
      h('span', null, `Devices ${plan.devices_total}`),
      h('span', null, `Inner steps ${plan.inner_steps}`),
      h('span', null, `Staleness ${plan.staleness}`),
      h('span', null, `Rounds ${plan.rounds ?? '—'}`),
    ),
    h(
      'div.lab-job-kv',
      null,
      h('span.lab-muted', null, 'Datasets'),
      h('span.lab-mono', null, datasets.length ? datasets.map(([name, id]) => `${name} ${id}`).join(', ') : '—'),
      h('span.lab-muted', null, 'Frozen'),
      h('span.lab-mono', null, plan.frozen || '—'),
      h('span.lab-muted', null, 'Output'),
      h('span.lab-mono', null, plan.output_dir || '—'),
    ),
  );
}

function unknownJob(id: string): HTMLElement {
  return h(
    'div.lab-empty',
    null,
    icon('brain'),
    h('div.lab-empty-title', null, id ? `Job ${id}` : 'Job'),
    h('div.lab-empty-text', null, 'No job with this id. Go back to Jobs to pick one.'),
    h('a.lab-job-back', { href: href('lab', 'jobs') }, 'Back to Jobs'),
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

    const terminal = v.status === 'done' || v.status === 'failed' || v.status === 'cancelled';
    const cancellable = v.status === 'queued' || v.status === 'running';
    const cancelBtn = h(
      'button.btn.danger',
      { type: 'button', disabled: !cancellable || busy, title: cancellable ? 'Cancel this job' : 'Only queued or running jobs can be cancelled' },
      'Cancel job',
    );
    cancelBtn.addEventListener('click', () => confirmCancel(v));

    const head = h(
      'div.lab-page-head',
      null,
      h('h2', null, `Job ${v.id}`),
      h('div.lab-actions', null, h('span.lab-badge', { class: TONE[v.status] }, v.status), cancelBtn),
    );
    const stats = h(
      'div.lab-grid.cols-4',
      null,
      stat('Recipe', v.recipe),
      stat('Mode', v.mode),
      stat('Created', fmtTime(v.created_ms)),
      stat('Updated', fmtTime(v.updated_ms)),
    );

    const timeline = h(
      'ol.lab-job-stages',
      null,
      stagesOf(v).map((s) => {
        const li = h('li.lab-job-stage', null, h('span.lab-job-stage-label', null, s.label), h('span.lab-mono', null, fmtTime(s.at)), s.sub ? h('span.lab-job-stage-sub', null, s.sub) : null);
        li.classList.add(s.state);
        return li;
      }),
    );

    const plan = section('Plan', v.plan ? planBlock(v.plan) : h('p.lab-muted', null, 'No plan is recorded for this job.'));

    const chartsBody: Node[] = [];
    if (!points.length) chartsBody.push(h('p.lab-muted', null, "Waiting for the trainer's first step. Queued jobs wait for a free device."));
    if (specs.length) chartsBody.push(chartGrid);
    const rounds = points.filter((p) => p.kind === 'round');
    if (rounds.length) {
      const accepted = rounds.filter((p) => p.accepted === true).length;
      chartsBody.push(
        h('p.lab-muted', null, `Rounds: ${rounds.length}, accepted ${accepted}.`),
        table(
          ['Round', 'Island', 'Accepted', 'Hub round'],
          rounds.map((p) => [String(p.round ?? '—'), p.island, p.accepted === undefined ? '—' : p.accepted ? 'yes' : 'no', String(p.hub_round ?? '—')]),
          [0, 3],
        ),
      );
    }
    const trials = points.filter((p) => p.kind === 'trial' && isNum(p.objective));
    if (trials.length) {
      chartsBody.push(h('p.lab-muted', null, 'Trials'), table(['Trial', 'Objective'], trials.map((p) => [String(p.trial ?? '—'), fix4(p.objective as number)]), [0, 1]));
    }
    const charts = section('Charts', chartsBody);

    const runs = deviceRows(v.runs);
    const devices = section(
      'Devices',
      runs.length
        ? table(
            ['Node', 'Device', 'Island', 'State', 'GPU-h', 'Share of job'],
            runs.map((r) => [
              r.node,
              r.device,
              r.island,
              r.state,
              dash(r.hours),
              h('span.lab-job-share', null, h('span.lab-job-track', null, h('span.lab-job-fill', { style: `width:${(r.share * 100).toFixed(1)}%` })), h('span.lab-mono', null, fmtPercent(r.share))),
            ]),
            [4],
          )
        : h('p.lab-muted', null, 'No runs yet. A job gets its devices when it is dispatched.'),
    );

    const log = section(
      'Log tail',
      h(
        'div.lab-job-log',
        null,
        logs.length ? logs.map((e) => h('div.lab-job-line', null, `#${e.seq} ${e.island} ${logText(e.event)}`)) : h('div.lab-muted', null, 'No events yet.'),
      ),
      [h('span.lab-muted', null, `${logs.length} events`)],
    );

    const used = v.gpu_hours;
    const budget = v.budget_gpu_hours;
    const ratio = budget > 0 ? used / budget : 0;
    const budgetCard = section(
      'Budget',
      h(
        'div.lab-job-budget',
        { class: used > budget ? 'over' : '' },
        h('div.lab-mono', null, `${dash(used)} of ${dash(budget)} GPU-h used (${fmtPercent(ratio)})`),
        h('div.lab-job-track', null, h('span.lab-job-fill', { style: `width:${Math.min(1, Math.max(0, ratio)) * 100}%` })),
      ),
    );

    const metrics = Object.entries(v.metrics ?? {}).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
    const outputBody: Node[] = [
      metrics.length ? table(['Metric', 'Value'], metrics.map(([k, val]) => [k, metricText(val)]), [1]) : h('p.lab-muted', null, 'No metrics yet.'),
      h('p.lab-muted', null, 'Outputs'),
      v.outputs.length ? h('ul.lab-job-outputs', null, v.outputs.map((o) => h('li.lab-mono', null, o))) : h('p.lab-muted', null, 'No outputs yet.'),
    ];
    if (v.outputs_remote) outputBody.push(h('p.lab-muted', null, 'The outputs stay on the node that trained them.'));
    const outputsCard = section('Outputs and metrics', outputBody);

    const error = section('Error', v.error ? h('p.lab-error', null, v.error) : h('p.lab-muted', null, 'No error.'));

    return h(
      'div.lab-stack.lab-job',
      null,
      head,
      stats,
      loadError ? h('p.lab-error', null, `The job view is out of date: ${loadError}`) : null,
      section('Status', timeline),
      plan,
      charts,
      devices,
      log,
      h('div.lab-job-pair', null, budgetCard, outputsCard),
      error,
      terminal ? null : h('p.lab-muted', null, 'This page follows the job live.'),
    );
  };

  /** The page for the current state: the job, the unknown-id empty state, or a loading or error line. */
  const paint = (): void => {
    if (!alive) return;
    if (notFound) {
      replace(root, unknownJob(id));
    } else if (view) {
      replace(root, pageOf(view));
    } else if (loadError) {
      replace(root, h('p.lab-error', null, `Could not load the job: ${loadError}`));
    } else {
      replace(root, h('p.lab-muted', null, 'Loading the job…'));
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
        toast(`Job ${v.id} is cancelled.`);
      } catch (err) {
        toast(errorText(err), 'error');
      } finally {
        busy = false;
        void refresh();
      }
    };
    const body = h(
      'div.lab-stack',
      null,
      h('p', null, `Cancel job ${v.id}? Its runs stop; the GPU time used so far stays on the job.`),
      h(
        'div.lab-row',
        null,
        h('button.btn', { type: 'button', onclick: () => close() }, 'Keep running'),
        h('button.btn.danger', { type: 'button', onclick: () => void stop() }, 'Cancel job'),
      ),
    );
    close = modal('Cancel job', body);
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
