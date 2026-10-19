// The Lab overview (`#/lab`, `#/lab/overview`): the GPUs across the fleet (per machine and per
// device: memory, live utilization, the job that holds each one), the queue (queued and running
// jobs), the recent models, and the training GPU-hours of the last seven days with a daily bar
// chart. It polls every 4 s and reloads, debounced, on each training line, so the queue and the
// utilization move with the trainers. A failed reload keeps the last figures and says why.

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
import { badge, card, table as dataTable, type Column } from '../ui/components';
import { countTo } from '../ui/chart';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { errorText, fmtNumber, fmtPercent, fmtSize, fmtTime, labEmpty, onTraining, pollEvery } from './lab-common';

const POLL_MS = 4000;
/** Training lines arrive in bursts: one reload follows a burst, this long after its first line. */
const LINE_DEBOUNCE_MS = 800;
/** The GPU-hours window: the last seven local days, today included. */
const WEEK_DAYS = 7;
const MODELS_SHOWN = 6;

const JOB_TONE: Record<JobStatus, 'muted' | 'info' | 'ok' | 'danger' | 'warn'> = { queued: 'muted', running: 'info', done: 'ok', failed: 'danger', cancelled: 'warn' };
const MODEL_TONE: Record<ModelState, 'info' | 'accent' | 'ok' | 'muted'> = { trained: 'info', shadow: 'accent', promoted: 'ok', retired: 'muted' };

const cap = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);

export function render(root: HTMLElement, _params: string[]): Cleanup {
  const errorLine = h('div');
  const fleetMeta = h('span.lab-note');
  const fleetBody = h('div');
  const queueMeta = h('span.lab-note');
  const queueBody = h('div');
  const modelsBody = h('div');
  const hoursFig = h('div.lab-counter.num', null, '—');
  const hoursSub = h('p.lab-note', null, '');
  const hoursBars = h('div.lab-bars');

  const hoursCard = card('Training GPU-hours this week', [hoursFig, hoursBars, hoursSub]);
  const modelsCard = card('Recent models', modelsBody);
  const fleetHead = h('div.lab-head-row', null, h('h3.lab-h', null, 'GPUs across the fleet'), fleetMeta);

  replace(
    root,
    h(
      'div.lab-stack',
      null,
      errorLine,
      h('div', null, fleetHead, fleetBody),
      card('Queue', queueBody, { actions: [queueMeta] }),
      h('div.lab-grid2', null, modelsCard, hoursCard),
    ),
  );
  replace(fleetBody, h('p.lab-note', null, 'Reading the lab…'));
  replace(queueBody, h('p.lab-note', null, 'Reading the lab…'));
  replace(modelsBody, h('p.lab-note', null, 'Reading the lab…'));

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
      replace(errorLine, h('p.lab-err', { role: 'alert' }, errorText(err)));
    }
  }

  function showFleet(machines: DeviceMachine[]): void {
    const devices = machines.flatMap((m) => m.devices);
    const busy = devices.filter((d) => d.allocated_to !== null || d.busy).length;
    fleetMeta.textContent = `${plural(devices.length, 'GPU')} · ${fmtNumber(busy)} busy`;
    replace(
      fleetBody,
      devices.length === 0
        ? labEmpty('cpu', 'No GPUs reported', "Check the machine's noded trainer: a machine reports its GPUs once its trainer is up.")
        : h('div.lab-fleet', null, ...machines.map(machineCard)),
    );
  }

  function showQueue(jobs: JobSummary[]): void {
    const active = jobs
      .filter((j) => j.status === 'queued' || j.status === 'running')
      .sort((a, b) => (a.status === b.status ? a.created_ms - b.created_ms : a.status === 'running' ? -1 : 1));
    const running = active.filter((j) => j.status === 'running').length;
    queueMeta.textContent = `${fmtNumber(active.length - running)} queued · ${fmtNumber(running)} running`;
    replace(
      queueBody,
      active.length > 0
        ? jobTable(active)
        : h('p.lab-note', null, jobs.length === 0 ? 'No jobs yet. Submit one from Jobs to queue a training run.' : 'Nothing queued or running.'),
    );
  }

  function showModels(list: ModelView[]): void {
    const newest = [...list].sort((a, b) => b.created_ms - a.created_ms).slice(0, MODELS_SHOWN);
    replace(
      modelsBody,
      newest.length > 0
        ? modelTable(newest)
        : h('p.lab-note', null, 'No models yet. Import a finished job’s model in Models to see it here.'),
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
    const total = sums.reduce((a, b) => a + b, 0);
    countTo(hoursFig, total, (v) => fmtNumber(v, 2));
    hoursSub.textContent = `${fmtNumber(created)} ${created === 1 ? 'job' : 'jobs'} created in the last 7 days`;
    const max = Math.max(...sums, 0.0001);
    replace(
      hoursBars,
      days.map((d, i) => {
        const v = sums[i] ?? 0;
        const day = new Date(d.start);
        return h(
          'div.lab-bar',
          { title: `${day.toLocaleDateString()} · ${v.toFixed(2)} GPU-h` },
          h('i', { style: `height:${((v / max) * 100).toFixed(1)}%` }),
          h('span', null, day.toLocaleDateString(undefined, { weekday: 'short' })),
        );
      }),
    );
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

/** A machine's card: its online and trainer badges, a count line, then each device. */
function machineCard(m: DeviceMachine): HTMLElement {
  const busy = m.devices.filter((d) => d.allocated_to !== null || d.busy).length;
  const body = h(
    'div',
    null,
    h('p.lab-note', null, `${plural(m.devices.length, 'GPU')} · ${fmtNumber(busy)} busy`),
    m.devices.length === 0 ? h('p.lab-note', null, 'No GPUs reported') : m.devices.map(deviceRow),
  );
  const el = card(m.id, body, {
    actions: [badge(m.online ? 'online' : 'offline', m.online ? 'ok' : 'muted', true), badge(m.trainer_ok ? 'trainer up' : 'trainer down', m.trainer_ok ? 'ok' : 'warn')],
  });
  el.classList.add('lab-mcard');
  return el;
}

/** One device: its name and backend, the vendor and what holds it, memory, then the utilization meter. */
function deviceRow(d: Device): HTMLElement {
  const pct = Math.max(0, Math.min(100, d.util_pct));
  const state = d.allocated_to
    ? h('a', { href: href('lab', 'jobs', d.allocated_to) }, `running ${d.allocated_to}`)
    : d.busy
      ? 'busy'
      : 'idle';
  return h(
    'div.lab-dev',
    null,
    h('div.lab-dev-t', null, h('b', null, d.name), badge(d.backend, 'accent')),
    h('div.lab-note', null, `${d.vendor} · `, state),
    h(
      'div.lab-dev-m',
      null,
      h('span.num', null, `${fmtNumber(d.mem_used_mib)} of ${fmtNumber(d.memory_mib)} MiB`),
      h(
        'div.lab-meter',
        { role: 'meter', 'aria-valuemin': 0, 'aria-valuemax': 100, 'aria-valuenow': pct },
        h('i', { style: `width:${pct}%` }),
      ),
      h('span.num', null, fmtPercent(pct / 100)),
    ),
  );
}

function jobTable(active: JobSummary[]): HTMLElement {
  const cols: Column<JobSummary>[] = [
    { label: 'Job', cell: (j) => h('span.mono', null, j.id) },
    { label: 'Status', cell: (j) => badge(cap(j.status), JOB_TONE[j.status]) },
    { label: 'Recipe', cell: (j) => h('span.mono', null, j.recipe) },
    { label: 'Mode', cell: (j) => j.mode },
    { label: 'Runs', num: true, cell: (j) => fmtNumber(j.runs) },
    { label: 'GPU-h', num: true, cell: (j) => `${fmtNumber(j.gpu_hours, 2)} / ${fmtNumber(j.budget_gpu_hours, 2)}` },
    { label: 'Created', cell: (j) => fmtTime(j.created_ms) },
  ];
  return h('div.tbl-wrap', null, dataTable(cols, active, (j) => (location.hash = href('lab', 'jobs', j.id)), (j) => j.id));
}

function modelTable(models: ModelView[]): HTMLElement {
  const cols: Column<ModelView>[] = [
    { label: 'Model', cell: (m) => h('div', null, h('strong', null, m.card.name), h('div.mono.lab-note', null, m.id)) },
    { label: 'Version', cell: (m) => h('span.mono', null, m.card.version) },
    { label: 'State', cell: (m) => badge(cap(m.state), MODEL_TONE[m.state]) },
    { label: 'Params', num: true, cell: (m) => (m.parameters ? `${(m.parameters / 1e9).toFixed(1)} B` : '—') },
    { label: 'Size', num: true, cell: (m) => fmtSize(m.size_bytes) },
    { label: 'Created', cell: (m) => fmtTime(m.created_ms) },
  ];
  return h('div.tbl-wrap', null, dataTable(cols, models, (m) => (location.hash = href('lab', 'models', m.id)), (m) => m.id));
}

/** `1 GPU`, `2 GPUs`. */
function plural(n: number, noun: string): string {
  return `${fmtNumber(n)} ${n === 1 ? noun : `${noun}s`}`;
}

/** Local midnight of the day `offset` days from the day of `ms`. */
function dayStart(ms: number, offset: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  d.setDate(d.getDate() + offset);
  return d.getTime();
}
