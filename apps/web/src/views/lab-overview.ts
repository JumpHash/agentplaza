// The Lab overview (`#/lab`, `#/lab/overview`): the GPUs across the fleet (per machine and per
// device: memory, live utilization, the job that holds each one), the queue (queued and running
// jobs), the recent models, and the training GPU-hours of the last seven days with a daily bar
// chart. It polls every 4 s and reloads, debounced, on each training line, so the queue and the
// utilization move with the trainers. A failed reload keeps the last figures and says why.

import '../styles/lab-overview.css';
import {
  listDevices,
  listJobs,
  listModels,
  type Device,
  type DeviceMachine,
  type JobStatus,
  type JobSummary,
  type ModelState,
  type ModelView,
} from '../api';
import { href } from '../router';
import { bars, countTo, type BarItem, type ChartHandle } from '../ui/chart';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { errorText, fmtGpuHours, fmtNumber, fmtPercent, fmtSize, fmtTime, labEmpty, onTraining, pollEvery } from './lab-common';

const POLL_MS = 4000;
/** Training lines arrive in bursts: one reload follows a burst, this long after its first line. */
const LINE_DEBOUNCE_MS = 800;
/** The GPU-hours window: the last seven local days, today included. */
const WEEK_DAYS = 7;
const MODELS_SHOWN = 6;
const MIB = 1024 * 1024;

const JOB_TONE: Record<JobStatus, string> = { queued: 'info', running: 'accent', done: 'ok', failed: 'danger', cancelled: 'muted' };
const MODEL_TONE: Record<ModelState, string> = { trained: 'info', shadow: 'warn', promoted: 'ok', retired: 'muted' };

interface Panel { el: HTMLElement; meta: HTMLElement; body: HTMLElement }

export function render(root: HTMLElement, _params: string[]): Cleanup {
  const errorLine = h('div');
  const fleet = panel('GPUs across the fleet');
  const queue = panel('Queue');
  const models = panel('Recent models');
  const hours = panel('Training GPU-hours this week');

  const hoursFig = h('div.lab-stat-value', null, '—');
  const hoursSub = h('div.lab-stat-sub', null, '');
  const hoursChartEl = h('div.lab-hours-chart');
  let hoursChart: ChartHandle<BarItem[]> | undefined;
  replace(
    hours.body,
    h('div.lab-hours', null, h('div.lab-stat', null, h('span.lab-stat-label', null, 'GPU-hours, last 7 days'), hoursFig, hoursSub), hoursChartEl),
  );

  replace(root, h('div.lab-overview', null, errorLine, fleet.el, queue.el, models.el, hours.el));
  for (const p of [fleet, queue, models]) replace(p.body, h('p.lab-muted', null, 'Reading the lab…'));

  let seq = 0;
  let gone = false;

  async function load(): Promise<void> {
    const mine = ++seq;
    try {
      const [devices, jobs, recent] = await Promise.all([listDevices(), listJobs(), listModels()]);
      if (gone || mine !== seq) return;
      replace(errorLine);
      showFleet(devices.machines);
      showQueue(jobs);
      showModels(recent);
      showHours(jobs);
    } catch (err) {
      if (gone || mine !== seq) return;
      replace(errorLine, h('p.lab-error', null, errorText(err)));
    }
  }

  function showFleet(machines: DeviceMachine[]): void {
    const devices = machines.flatMap((m) => m.devices);
    const busy = devices.filter((d) => d.allocated_to !== null || d.busy).length;
    fleet.meta.textContent = `${plural(machines.length, 'machine')} · ${plural(devices.length, 'GPU')} · ${fmtNumber(busy)} busy`;
    replace(
      fleet.body,
      devices.length === 0
        ? labEmpty('machine', 'No GPUs reported', "Check the machine's noded trainer: a machine reports its GPUs once its trainer is up.")
        : h('div.lab-machines', null, ...machines.map(machineCard)),
    );
  }

  function showQueue(jobs: JobSummary[]): void {
    const active = jobs
      .filter((j) => j.status === 'queued' || j.status === 'running')
      .sort((a, b) => (a.status === b.status ? a.created_ms - b.created_ms : a.status === 'running' ? -1 : 1));
    const running = active.filter((j) => j.status === 'running').length;
    queue.meta.textContent = `${fmtNumber(active.length - running)} queued · ${fmtNumber(running)} running`;
    replace(
      queue.body,
      active.length > 0
        ? table(
            ['Job', 'Status', 'Recipe', 'Mode', 'Runs', 'GPU-h used / budget', 'Created'],
            active.map((j) => ({
              href: href('lab', 'jobs', j.id),
              cells: [
                h('a.lab-mono.lab-overview-link', { href: href('lab', 'jobs', j.id) }, j.id),
                badge(j.status, JOB_TONE[j.status]),
                h('span.lab-mono', null, j.recipe),
                j.mode,
                fmtNumber(j.runs),
                `${fmtNumber(j.gpu_hours, 2)} / ${fmtNumber(j.budget_gpu_hours, 2)}`,
                fmtTime(j.created_ms),
              ],
            })),
            ['', '', '', '', 'num', 'num', ''],
          )
        : jobs.length === 0
          ? labEmpty('tasks', 'No jobs yet', 'Submit one from Jobs to queue a training run.')
          : labEmpty('tasks', 'Nothing queued', 'Every job has finished. Queued and running jobs show here.'),
    );
  }

  function showModels(list: ModelView[]): void {
    const newest = [...list].sort((a, b) => b.created_ms - a.created_ms).slice(0, MODELS_SHOWN);
    models.meta.textContent = `${fmtNumber(list.length)} registered`;
    replace(
      models.body,
      newest.length > 0
        ? table(
            ['Model', 'State', 'Parameters', 'Size', 'Created'],
            newest.map((m) => ({
              href: href('lab', 'models', m.id),
              cells: [
                h('a.lab-overview-link', { href: href('lab', 'models', m.id) }, m.card.version ? `${m.card.name} ${m.card.version}` : m.card.name),
                badge(m.state, MODEL_TONE[m.state]),
                fmtNumber(m.parameters),
                fmtSize(m.size_bytes),
                fmtTime(m.created_ms),
              ],
            })),
            ['', '', 'num', 'num', ''],
          )
        : labEmpty('models', 'No models yet', "Import a finished job's model in Models to see it here."),
    );
  }

  function showHours(jobs: JobSummary[]): void {
    const now = Date.now();
    const days = Array.from({ length: WEEK_DAYS }, (_, i) => ({ start: dayStart(now, i - WEEK_DAYS + 1), end: dayStart(now, i - WEEK_DAYS + 2) }));
    const sums = days.map(() => 0);
    let created = 0;
    for (const job of jobs) {
      const i = days.findIndex((d) => job.created_ms >= d.start && job.created_ms < d.end);
      if (i < 0) continue;
      sums[i] = (sums[i] ?? 0) + job.gpu_hours;
      created += 1;
    }
    countTo(hoursFig, sums.reduce((a, b) => a + b, 0), fmtGpuHours);
    hoursSub.textContent = `${fmtNumber(created)} ${created === 1 ? 'job' : 'jobs'} created in the last 7 days`;
    const items: BarItem[] = days.map((d, i) => ({ key: dayLabel(d.start), label: dayLabel(d.start), value: sums[i] ?? 0, color: 'var(--chart-1)' }));
    if (hoursChart) hoursChart.update(items);
    else hoursChart = bars(hoursChartEl, items, { format: (v) => fmtNumber(v, 2), height: 150, ariaLabel: 'Training GPU-hours per day' });
  }

  let timer: number | undefined;
  const offLines = onTraining(() => {
    if (timer !== undefined) return;
    timer = window.setTimeout(() => {
      timer = undefined;
      void load();
    }, LINE_DEBOUNCE_MS);
  });
  const stopPoll = pollEvery(POLL_MS, () => void load());

  return () => {
    gone = true;
    stopPoll();
    offLines();
    if (timer !== undefined) window.clearTimeout(timer);
  };
}

// ---- Parts -----------------------------------------------------------------

/** A ruled card whose meta (a line of counts) and body the loads replace. */
function panel(title: string): Panel {
  const meta = h('span.lab-muted');
  const body = h('div');
  const el = h('section.lab-card', null, h('div.lab-card-head', null, h('h3', null, title), meta), h('div.lab-card-body', null, body));
  return { el, meta, body };
}

function badge(text: string, tone: string): HTMLElement {
  const el = h('span.lab-badge', null, text);
  el.classList.add(tone);
  return el;
}

/** A card per machine: its state and trainer, then each device as a row. */
function machineCard(m: DeviceMachine): HTMLElement {
  const busy = m.devices.filter((d) => d.allocated_to !== null || d.busy).length;
  const el = h(
    'section.lab-card.lab-machine',
    null,
    h(
      'div.lab-card-head',
      null,
      h('h3', null, m.id),
      badge(m.online ? 'online' : 'offline', m.online ? 'ok' : 'muted'),
      badge(m.trainer_ok ? 'trainer up' : 'trainer down', m.trainer_ok ? 'info' : 'warn'),
      h('span.lab-muted', null, `${plural(m.devices.length, 'GPU')} · ${fmtNumber(busy)} busy`),
    ),
    h(
      'div.lab-card-body',
      null,
      m.devices.length === 0 ? h('p.lab-muted', null, 'No devices reported.') : h('div.lab-dev-list', null, ...m.devices.map(deviceRow)),
    ),
  );
  if (!m.online) el.classList.add('is-offline');
  return el;
}

/** One device: its name, backend, memory, utilization bar and the job that holds it. */
function deviceRow(d: Device): HTMLElement {
  const pct = Math.max(0, Math.min(100, d.util_pct));
  const meter = h(
    'div.lab-util',
    { role: 'meter', 'aria-label': `${d.name} utilization`, 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': pct },
    h('i', { style: `width:${pct}%` }),
  );
  const run = d.allocated_to
    ? h('a.lab-mono.lab-overview-link', { href: href('lab', 'jobs', d.allocated_to) }, `running ${d.allocated_to}`)
    : h('span.lab-muted', null, d.busy ? 'busy' : 'idle');
  return h(
    'div.lab-dev',
    null,
    h('div.lab-dev-top', null, h('span.lab-dev-name', null, d.name), h('span.lab-muted', null, d.vendor), badge(d.backend, 'info')),
    h('div.lab-dev-mem', null, `${fmtSize(d.mem_used_mib * MIB)} of ${fmtSize(d.memory_mib * MIB)} memory used`),
    h('div.lab-dev-util', null, meter, h('span.lab-mono', null, fmtPercent(pct / 100))),
    h('div.lab-dev-run', null, run),
  );
}

/** A table whose rows open their detail (`href`); `aligns` gives each column's class (`num` or none). */
function table(heads: string[], rows: { href: string; cells: (Node | string)[] }[], aligns: string[]): HTMLElement {
  const body = rows.map((row) => {
    const tr = h('tr', { 'data-link': 'true' }, ...row.cells.map((cell, i) => h('td', { class: aligns[i] ?? '' }, cell)));
    tr.addEventListener('click', () => {
      location.hash = row.href;
    });
    return tr;
  });
  return h(
    'div.lab-table-wrap',
    null,
    h(
      'table.lab-table',
      null,
      h('thead', null, h('tr', null, ...heads.map((head, i) => h('th', { class: aligns[i] ?? '' }, head)))),
      h('tbody', null, ...body),
    ),
  );
}

// ---- Dates -----------------------------------------------------------------

/** Local midnight of the day `offset` days from the day of `ms`. */
function dayStart(ms: number, offset: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  d.setDate(d.getDate() + offset);
  return d.getTime();
}

/** A count with its noun, singular for one: `1 GPU`, `2 GPUs`. */
function plural(n: number, noun: string): string {
  return `${fmtNumber(n)} ${n === 1 ? noun : `${noun}s`}`;
}

/** `MM-DD` of a local day: the bar labels, the same on the desktop. */
function dayLabel(ms: number): string {
  const d = new Date(ms);
  const p = (n: number): string => String(n).padStart(2, '0');
  return `${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}
