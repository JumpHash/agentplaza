// The Lab's jobs tab (`#/lab/jobs`): every job the plaza has recorded, newest first, with filters by
// status, recipe and mode and a search by id; a table or cards view; and the New job form under it.
// The list refreshes on a poll and whenever a `training` frame arrives. Rows open `#/lab/jobs/<id>`.

import { listJobs, type FoundryMode, type JobStatus, type JobSummary } from '../api';
import { href } from '../router';
import { badge, table as dataTable, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import {
  agoText,
  budgetCell,
  errorText,
  labButton,
  labEmpty,
  onTraining,
  pollEvery,
  runTimeText,
  seg,
} from './lab-common';
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
const MODES: [string, string][] = [
  ['all', 'All'],
  ['single', 'single'],
  ['tight', 'tight'],
  ['loose', 'loose'],
];
const VIEWS: [View, string][] = [
  ['table', 'Table'],
  ['cards', 'Cards'],
];
const TONE: Record<JobStatus, 'muted' | 'info' | 'ok' | 'danger' | 'warn'> = { queued: 'muted', running: 'info', done: 'ok', failed: 'danger', cancelled: 'warn' };

const isLive = (s: JobStatus): boolean => s === 'queued' || s === 'running';
const cap = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);

/** How long a job has run: to now while it is queued or running, else to its last update. */
export function durationOf(job: JobSummary, now: number): number {
  const end = isLive(job.status) ? now : job.updated_ms;
  return Math.max(0, end - job.created_ms);
}

const jobHref = (id: string): string => href('lab', 'jobs', id);
const statusBadge = (status: JobStatus): HTMLElement => badge(cap(status), TONE[status]);

/** The job's error, in the red the prototype uses, or a dash. */
function errorCell(job: JobSummary): HTMLElement | string {
  return job.error ? h('span.lab-err', null, job.error) : '—';
}

function tableOf(rows: JobSummary[], fresh: ReadonlySet<string>, now: number): HTMLElement {
  const cols: Column<JobSummary>[] = [
    { label: 'ID', cell: (j) => h('span.mono', null, j.id) },
    { label: 'Status', cell: (j) => statusBadge(j.status) },
    { label: 'Mode', cell: (j) => j.mode },
    { label: 'Recipe', cell: (j) => h('span.mono', null, j.recipe) },
    { label: 'Datasets', num: true, cell: (j) => String(j.datasets.length) },
    { label: 'Created', cell: (j) => agoText(j.created_ms, now) },
    { label: 'Duration', num: true, cell: (j) => runTimeText(durationOf(j, now)) },
    { label: 'GPU-h', cell: (j) => budgetCell(j.gpu_hours, j.budget_gpu_hours) },
    { label: 'Error', cell: (j) => errorCell(j) },
  ];
  const el = h('div.tbl-wrap', null, dataTable(cols, rows, (j) => (location.hash = jobHref(j.id)), (j) => j.id));
  for (const tr of el.querySelectorAll<HTMLTableRowElement>('tbody tr')) {
    if (fresh.has(tr.dataset.key ?? '')) tr.classList.add('lab-flash');
  }
  return el;
}

function cardsOf(rows: JobSummary[], fresh: ReadonlySet<string>, now: number): HTMLElement {
  return h(
    'div.lab-cards',
    null,
    rows.map((job) => {
      const card = h(
        'article.card.lab-jcard.clickable',
        { 'data-id': job.id, tabindex: '0', role: 'link' },
        h('header.card-h', null, h('h3.mono', null, job.id), h('span.sp'), statusBadge(job.status)),
        h(
          'div.card-b.lab-jcard-b',
          null,
          h('div', null, h('span.lab-note', null, 'Recipe'), ' ', h('span.mono', null, job.recipe), ` · ${job.mode}`),
          h('div.lab-note', null, `${job.datasets.length} dataset${job.datasets.length === 1 ? '' : 's'} · created ${agoText(job.created_ms, now)} · ${runTimeText(durationOf(job, now))}`),
          budgetCell(job.gpu_hours, job.budget_gpu_hours),
          job.error ? h('div.lab-err', null, job.error) : null,
        ),
      );
      card.addEventListener('click', () => {
        location.hash = jobHref(job.id);
      });
      card.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') location.hash = jobHref(job.id);
      });
      if (fresh.has(job.id)) card.classList.add('lab-flash');
      return card;
    }),
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
  let mode = 'all';
  let query = '';
  let view: View = 'table';
  let timer: ReturnType<typeof setTimeout> | undefined;

  // Controls are built once and updated in place, so a poll never takes the focus away.
  const chipCounts = new Map<StatusFilter, HTMLElement>();
  const chips = STATUSES.map((s) => {
    const count = h('span.num', null, '0');
    chipCounts.set(s.key, count);
    const b = h('button.lab-chip', { type: 'button', 'aria-pressed': s.key === status ? 'true' : 'false' }, s.label, ' ', count);
    b.addEventListener('click', () => {
      status = s.key;
      updateChips();
      renderList();
    });
    return { key: s.key, el: b };
  });
  const updateChips = (): void => {
    for (const c of chips) {
      const on = c.key === status;
      c.el.classList.toggle('is-on', on);
      c.el.setAttribute('aria-pressed', on ? 'true' : 'false');
    }
  };
  const recipeSel = h('select.select', { 'aria-label': 'Recipe' });
  recipeSel.addEventListener('change', () => {
    recipe = recipeSel.value;
    renderList();
  });
  const modeSel = h(
    'select.select',
    { 'aria-label': 'Mode' },
    MODES.map(([value, label]) => h('option', { value }, label)),
  );
  modeSel.addEventListener('change', () => {
    mode = modeSel.value;
    renderList();
  });
  const search = h('input.input.lab-q', { type: 'search', placeholder: 'Search by job id', 'aria-label': 'Search by job id', spellcheck: 'false' });
  search.addEventListener('input', () => {
    query = search.value;
    renderList();
  });
  const viewSlot = h('div');
  const listHost = h('div');
  const countLine = h('p.lab-note');
  const errorLine = h('p.lab-err');

  const renderViewSeg = (): void => {
    replace(
      viewSlot,
      seg(VIEWS, view, (v) => {
        view = v as View;
        renderViewSeg();
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
        (mode === 'all' || j.mode === (mode as FoundryMode)) &&
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
      body = h('p.lab-note', null, loadError ? '' : 'Loading jobs…');
    } else if (total === 0) {
      body = labEmpty('flask-conical', 'No jobs yet', undefined, labButton('New job', () => scrollToForm(), { kind: 'primary', sm: true, icon: 'plus' }));
    } else if (rows.length === 0) {
      body = labEmpty('search', 'No jobs match', undefined, labButton('Clear filters', () => clearFilters(), { kind: 'ghost', sm: true }));
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
    mode = 'all';
    query = '';
    search.value = '';
    modeSel.value = 'all';
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

  const tools = h(
    'div.lab-tools',
    null,
    h('div.lab-chips', { role: 'group', 'aria-label': 'Status' }, chips.map((c) => c.el)),
    h('div.lab-tools-r', null, recipeSel, modeSel, search, viewSlot),
  );

  replace(root, h('div.lab-stack', null, tools, h('div', null, countLine, errorLine, listHost), form.el));
  updateChips();
  renderViewSeg();
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
