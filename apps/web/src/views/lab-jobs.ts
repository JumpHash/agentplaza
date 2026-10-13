// The Lab's jobs tab (`#/lab/jobs`): every job the plaza has recorded, newest first, with filters by
// status, recipe and mode and a search by id; a table or cards view; and the New job form under it.
// The list refreshes on a poll and whenever a `training` frame arrives. Rows open `#/lab/jobs/<id>`.

import '../styles/lab-jobs.css';
import { listJobs, type FoundryMode, type JobStatus, type JobSummary } from '../api';
import { href } from '../router';
import { button, tabs } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { errorText, fmtDuration, fmtGpuHours, fmtNumber, fmtTime, labEmpty, onTraining, pollEvery } from './lab-common';
import { jobForm } from './lab-jobs-form';

type View = 'table' | 'cards';
type StatusFilter = 'all' | JobStatus;

const POLL_MS = 4000;
const DEBOUNCE_MS = 400;
const STATUSES: { key: StatusFilter; label: string }[] = [
  { key: 'all', label: 'All' },
  { key: 'queued', label: 'Queued' },
  { key: 'running', label: 'Running' },
  { key: 'done', label: 'Done' },
  { key: 'failed', label: 'Failed' },
  { key: 'cancelled', label: 'Cancelled' },
];
const MODES: { value: string; label: string }[] = [
  { value: '', label: 'All modes' },
  { value: 'single', label: 'single' },
  { value: 'tight', label: 'tight' },
  { value: 'loose', label: 'loose' },
];
const VIEWS: { key: View; label: string }[] = [
  { key: 'table', label: 'Table' },
  { key: 'cards', label: 'Cards' },
];
const TONE: Record<JobStatus, string> = { queued: 'info', running: 'accent', done: 'ok', failed: 'danger', cancelled: 'muted' };
const COLUMNS: { label: string; num?: boolean }[] = [
  { label: 'ID' },
  { label: 'Status' },
  { label: 'Mode' },
  { label: 'Recipe' },
  { label: 'Datasets', num: true },
  { label: 'Created' },
  { label: 'Duration', num: true },
  { label: 'GPU-h used', num: true },
  { label: 'Error' },
];

const isLive = (s: JobStatus): boolean => s === 'queued' || s === 'running';

/** How long a job has run: to now while it is queued or running, else to its last update. */
export function durationOf(job: JobSummary, now: number): number {
  const end = isLive(job.status) ? now : job.updated_ms;
  return Math.max(0, end - job.created_ms);
}

function statusBadge(status: JobStatus): HTMLElement {
  return h('span.lab-badge', { class: TONE[status] }, status);
}

/** The budget bar: how much of the job's budget the runs have used. Over budget turns it red. */
function budgetBar(job: JobSummary): HTMLElement {
  const ratio = job.budget_gpu_hours > 0 ? job.gpu_hours / job.budget_gpu_hours : 0;
  const pct = Math.max(0, Math.min(100, ratio * 100));
  return h(
    'span.lab-jobs-bar',
    { class: ratio > 1 ? 'over' : '', title: `${fmtGpuHours(job.gpu_hours)} of ${fmtGpuHours(job.budget_gpu_hours)}` },
    h('i', { style: `width:${pct.toFixed(1)}%` }),
  );
}

function used(job: JobSummary): HTMLElement {
  return h(
    'span.lab-jobs-used',
    null,
    h('span.lab-mono', null, `${fmtNumber(job.gpu_hours, 2)} / ${fmtNumber(job.budget_gpu_hours, 2)}`),
    budgetBar(job),
  );
}

function jobHref(id: string): string {
  return href('lab', 'jobs', id);
}

function tableOf(rows: JobSummary[], fresh: ReadonlySet<string>, now: number): HTMLElement {
  return h(
    'div.lab-table-wrap',
    null,
    h(
      'table.lab-table.lab-jobs-table',
      null,
      h('thead', null, h('tr', null, COLUMNS.map((c) => h('th', { class: c.num ? 'num' : '' }, c.label)))),
      h(
        'tbody',
        null,
        rows.map((job) => {
          const tr = h(
            'tr',
            { 'data-link': '', class: fresh.has(job.id) ? 'lab-jobs-enter' : '' },
            h('td', null, h('a.lab-mono', { href: jobHref(job.id) }, job.id)),
            h('td', null, statusBadge(job.status)),
            h('td', null, h('span.lab-mono', null, job.mode)),
            h('td', null, h('span.lab-mono', null, job.recipe)),
            h('td', { class: 'num' }, fmtNumber(job.datasets.length)),
            h('td', null, h('span.lab-mono', null, fmtTime(job.created_ms))),
            h('td', { class: 'num' }, h('span.lab-mono', null, fmtDuration(durationOf(job, now)))),
            h('td', { class: 'num' }, used(job)),
            h('td', null, h('span.lab-jobs-error', { title: job.error ?? '' }, job.error ?? '—')),
          );
          tr.addEventListener('click', () => {
            location.hash = jobHref(job.id);
          });
          return tr;
        }),
      ),
    ),
  );
}

function cardsOf(rows: JobSummary[], fresh: ReadonlySet<string>, now: number): HTMLElement {
  return h(
    'div.lab-grid.auto.lab-jobs-cards',
    null,
    rows.map((job) =>
      h(
        'a.lab-card.lab-jobs-card',
        { href: jobHref(job.id), class: fresh.has(job.id) ? 'lab-jobs-enter' : '' },
        h('div.lab-card-head', null, h('h3.lab-mono', null, job.id), statusBadge(job.status)),
        h(
          'div.lab-card-body',
          null,
          h(
            'dl.lab-jobs-kv',
            null,
            h('dt', null, 'Recipe'),
            h('dd.lab-mono', null, job.recipe),
            h('dt', null, 'Mode'),
            h('dd.lab-mono', null, job.mode),
            h('dt', null, 'Datasets'),
            h('dd', null, fmtNumber(job.datasets.length)),
            h('dt', null, 'Created'),
            h('dd.lab-mono', null, fmtTime(job.created_ms)),
            h('dt', null, 'Duration'),
            h('dd.lab-mono', null, fmtDuration(durationOf(job, now))),
            h('dt', null, 'GPU-h used'),
            h('dd', null, used(job)),
          ),
          job.error ? h('p.lab-error', null, job.error) : null,
        ),
      ),
    ),
  );
}

export function render(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  let busy = false;
  let pending = false;
  let jobs: JobSummary[] | null = null;
  let loadError: string | null = null;
  let seen = new Set<string>();
  let fresh = new Set<string>();
  let status: StatusFilter = 'all';
  let recipe = '';
  let mode = '';
  let query = '';
  let view: View = 'table';
  let timer: ReturnType<typeof setTimeout> | undefined;

  // Controls are built once and updated in place, so a poll never takes the focus away.
  const chipCounts = new Map<StatusFilter, HTMLElement>();
  const chips = STATUSES.map((s) => {
    const count = h('span.lab-jobs-count', null, '0');
    chipCounts.set(s.key, count);
    const b = h('button.lab-jobs-chip', { type: 'button', 'aria-pressed': s.key === status ? 'true' : 'false' }, s.label, count);
    b.addEventListener('click', () => {
      status = s.key;
      updateChips();
      renderList();
    });
    return { key: s.key, el: b };
  });
  const updateChips = (): void => {
    for (const c of chips) c.el.setAttribute('aria-pressed', c.key === status ? 'true' : 'false');
  };
  const recipeSel = h('select.select', { 'aria-label': 'Filter by recipe' });
  recipeSel.addEventListener('change', () => {
    recipe = recipeSel.value;
    renderList();
  });
  const modeSel = h(
    'select.select',
    { 'aria-label': 'Filter by mode' },
    MODES.map((m) => h('option', { value: m.value }, m.label)),
  );
  modeSel.addEventListener('change', () => {
    mode = modeSel.value;
    renderList();
  });
  const search = h('input.input', { type: 'search', placeholder: 'Search by job id', 'aria-label': 'Search by job id', spellcheck: 'false' });
  search.addEventListener('input', () => {
    query = search.value;
    renderList();
  });
  const viewSlot = h('div');
  const listHost = h('div.lab-stack');
  const countLine = h('span.lab-muted');
  const errorLine = h('p.lab-error');

  const renderViewTabs = (): void => {
    replace(
      viewSlot,
      tabs(VIEWS, view, (key) => {
        view = key as View;
        renderViewTabs();
        renderList();
      }),
    );
  };

  const visible = (): JobSummary[] => {
    const q = query.trim().toLowerCase();
    return (jobs ?? []).filter(
      (j) =>
        (status === 'all' || j.status === status) &&
        (recipe === '' || j.recipe === recipe) &&
        (mode === '' || j.mode === (mode as FoundryMode)) &&
        (q === '' || j.id.toLowerCase().includes(q)),
    );
  };

  function updateCounts(): void {
    const all = jobs ?? [];
    chipCounts.get('all')?.replaceChildren(String(all.length));
    for (const s of STATUSES) {
      if (s.key === 'all') continue;
      chipCounts.get(s.key)?.replaceChildren(String(all.filter((j) => j.status === s.key).length));
    }
    const names = [...new Set(all.map((j) => j.recipe))].sort();
    const current = recipeSel.value;
    const options = [new Option('All recipes', ''), ...names.map((n) => new Option(n, n))];
    const same = options.length === recipeSel.options.length && options.every((o, i) => recipeSel.options[i]?.value === o.value);
    if (!same) recipeSel.replaceChildren(...options);
    recipeSel.value = names.includes(current) ? current : '';
    if (recipeSel.value !== recipe) recipe = recipeSel.value;
  }

  function renderList(): void {
    if (!alive) return;
    const rows = visible();
    const now = Date.now();
    const total = jobs?.length ?? 0;
    countLine.textContent = jobs === null ? '' : `${rows.length} of ${total} shown`;
    errorLine.textContent = loadError ?? '';
    let body: Node;
    if (jobs === null) {
      body = h('p.lab-muted', null, loadError ? '' : 'Loading jobs…');
    } else if (total === 0) {
      body = h(
        'div.lab-jobs-empty',
        null,
        labEmpty(
          'play',
          'No jobs yet',
          'Pick a vetted recipe and a dataset, set a budget, and preview the plan before you submit.',
        ),
        h('div.lab-row.lab-jobs-center', null, button('New job', () => scrollToForm(), { kind: 'primary', icon: 'plus' })),
      );
    } else if (rows.length === 0) {
      body = labEmpty('search', 'No jobs match', 'Clear the filters to see every job.');
      body.appendChild(
        h(
          'div.lab-row.lab-jobs-center',
          null,
          button('Clear filters', () => clearFilters(), { kind: 'ghost' }),
        ),
      );
    } else {
      body = view === 'table' ? tableOf(rows, fresh, now) : cardsOf(rows, fresh, now);
    }
    replace(listHost, body);
    fresh = new Set();
    updateCounts();
  }

  function clearFilters(): void {
    status = 'all';
    recipe = '';
    mode = '';
    query = '';
    search.value = '';
    modeSel.value = '';
    updateChips();
    renderList();
  }

  function scrollToForm(): void {
    form.el.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }

  async function load(): Promise<void> {
    if (busy) {
      pending = true;
      return;
    }
    busy = true;
    try {
      const next = await listJobs();
      if (!alive) return;
      const ids = new Set(next.map((j) => j.id));
      if (jobs !== null) fresh = new Set([...ids].filter((id) => !seen.has(id)));
      seen = ids;
      jobs = next;
      loadError = null;
    } catch (err) {
      if (!alive) return;
      loadError = errorText(err);
    } finally {
      busy = false;
    }
    if (!alive) return;
    renderList();
    if (pending) {
      pending = false;
      void load();
    }
  }

  const scheduleLoad = (): void => {
    if (timer !== undefined) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = undefined;
      void load();
    }, DEBOUNCE_MS);
  };
  const form = jobForm((id) => {
    location.hash = jobHref(id);
  });

  const toolbar = h(
    'div.lab-jobs-toolbar',
    null,
    h('div.lab-jobs-chips', { role: 'group', 'aria-label': 'Filter by status' }, chips.map((c) => c.el)),
    h('div.lab-row', null, recipeSel, modeSel, search),
  );
  const listCard = h(
    'section.lab-card',
    null,
    h(
      'div.lab-card-head.lab-jobs-head',
      null,
      h('h3', null, 'Job list'),
      countLine,
      viewSlot,
    ),
    h('div.lab-card-body', null, toolbar, errorLine, listHost),
  );

  replace(
    root,
    h(
      'div.lab-stack',
      null,
      h(
        'div.lab-page-head',
        null,
        h('h2', null, 'Jobs'),
        h('div.lab-actions', null, button('New job', () => scrollToForm(), { kind: 'primary', icon: 'plus' })),
      ),
      listCard,
      form.el,
    ),
  );
  renderViewTabs();
  renderList();
  const offTraining = onTraining(() => scheduleLoad());
  const stopPoll = pollEvery(POLL_MS, () => void load());

  return () => {
    alive = false;
    stopPoll();
    offTraining();
    if (timer !== undefined) clearTimeout(timer);
    form.destroy();
  };
}
