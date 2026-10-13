// The Lab's datasets (`#/lab/datasets`): the training sets, exported from the plaza's records or
// uploaded as JSONL. A table (or cards) sorted newest first; a row opens its detail panel. Export and
// upload are dialogs; an export is confirmed before it is written.

import '../styles/lab-data.css';
import { createDataset, listDatasets, uploadDataset, type Dataset, type DatasetKind, type DatasetSpec } from '../api';
import { button, field, input, modal, select, toast } from '../ui/components';
import { h, keepState, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { errorText, fmtNumber, fmtSize, fmtTime, labEmpty, pollEvery } from './lab-common';

type View = 'table' | 'cards';
type Tone = 'ok' | 'warn' | 'danger' | 'info' | 'accent' | 'muted';

const VIEWS: { key: View; label: string }[] = [
  { key: 'table', label: 'Table' },
  { key: 'cards', label: 'Cards' },
];
/** The kinds an export can take, in the order the form offers them. */
const EXPORT_KINDS: { key: Exclude<DatasetKind, 'upload'>; label: string }[] = [
  { key: 'cascade', label: 'cascade' },
  { key: 'tasks', label: 'tasks' },
  { key: 'usage', label: 'usage' },
  { key: 'all', label: 'all' },
];
/** The label classes in the order the rung predictor reads them; any other class follows. */
const CLASS_ORDER = ['cheap', 'mid', 'frontier'];

const tag = (text: string, tone: Tone = 'muted'): HTMLElement => h('span.lab-badge', { class: tone }, text);
const mono = (text: string): HTMLElement => h('span.lab-mono', null, text);
const kv = (label: string, value: Node | string): HTMLElement => h('div', null, h('dt', null, label), h('dd', null, value));

/** Where a dataset came from: its provenance's `source` when it names one, else what its kind says. */
function sourceOf(d: Dataset): string {
  const source = (d.provenance as Record<string, unknown> | null)?.source;
  if (typeof source === 'string' && source) return source;
  if (d.kind === 'upload') return 'uploaded file';
  if (d.kind === 'all') return 'plaza records';
  return `plaza ${d.kind} records`;
}

/** The label balance as (class, count) pairs: the known classes first, then any others. */
function balanceEntries(balance: Record<string, number>): [string, number][] {
  const known = CLASS_ORDER.filter((k) => k in balance).map((k): [string, number] => [k, balance[k] ?? 0]);
  const rest = Object.keys(balance)
    .filter((k) => !CLASS_ORDER.includes(k))
    .sort()
    .map((k): [string, number] => [k, balance[k] ?? 0]);
  return [...known, ...rest];
}

/** The label balance: one segment per class, sized by its count, and a legend with the counts. */
function balanceBar(d: Dataset): HTMLElement {
  const entries = balanceEntries(d.label_balance);
  const total = entries.reduce((sum, [, n]) => sum + n, 0);
  if (total === 0) return h('span.lab-muted', null, 'no labels');
  const label = entries.map(([k, n]) => `${k} ${fmtNumber(n)}`).join(', ');
  return h(
    'div.lab-balance',
    null,
    h(
      'div.lab-bar',
      { role: 'img', 'aria-label': label },
      entries
        .filter(([, n]) => n > 0)
        .map(([k, n]) => h('span', { class: k, title: `${k} ${fmtNumber(n)}`, style: `flex-grow: ${n}` })),
    ),
    h(
      'div.lab-legend',
      null,
      entries.map(([k, n]) => h('span.lab-key', null, h('i', { class: k }), `${k} ${fmtNumber(n)}`)),
    ),
  );
}

/** The taint flags as warn badges, or a muted none. */
function taintsOf(d: Dataset): HTMLElement {
  if (d.taint.length === 0) return h('span.lab-muted', null, 'none');
  return h('div.lab-taints', null, d.taint.map((t) => tag(t, 'warn')));
}

/** The row count, with the frozen set under it (a padlock and its count) when there is one. */
function rowsOf(d: Dataset): HTMLElement {
  return h(
    'div',
    null,
    h('span', null, fmtNumber(d.rows)),
    d.frozen_rows > 0 ? h('div.lab-frozen', { title: `Frozen file: ${d.frozen_file}` }, h('i.lab-lock'), `${fmtNumber(d.frozen_rows)} frozen`) : null,
  );
}

/** The time span, from and to, in local time. */
function rangeOf(d: Dataset): HTMLElement {
  return h('div.lab-range', null, fmtTime(d.from_ms), h('span', null, 'to ', fmtTime(d.to_ms)));
}

/** The name over the id, so a long name still shows which dataset it is. */
function nameOf(d: Dataset): HTMLElement {
  return h('div.lab-name', null, h('strong', null, d.name), mono(d.id));
}

/** The datasets as a table; a row opens its detail. The rows keep their order (newest first). */
function tableOf(rows: Dataset[], selected: string | null, pick: (id: string) => void): HTMLElement {
  const heads = ['Name', 'Kind', 'Source', 'Time range', 'Rows', 'Label balance', 'Taint'];
  return h(
    'div.lab-table-wrap',
    null,
    h(
      'table.lab-table',
      null,
      h('thead', null, h('tr', null, heads.map((label, i) => h('th', { class: i === 4 ? 'num' : '' }, label)))),
      h(
        'tbody',
        null,
        rows.map((d) => {
          const tr = h('tr', { 'data-link': '', 'data-key': d.id, class: d.id === selected ? 'is-selected' : '', 'aria-selected': String(d.id === selected) }, [
            h('td', null, nameOf(d)),
            h('td', null, tag(d.kind, d.kind === 'upload' ? 'info' : 'muted')),
            h('td', null, sourceOf(d)),
            h('td', null, rangeOf(d)),
            h('td', { class: 'num' }, rowsOf(d)),
            h('td', null, balanceBar(d)),
            h('td', null, taintsOf(d)),
          ]);
          tr.addEventListener('click', () => pick(d.id));
          return tr;
        }),
      ),
    ),
  );
}

/** The datasets as cards: the same facts, one card each, opening its detail on a click or Enter. */
function cardsOf(rows: Dataset[], selected: string | null, pick: (id: string) => void): HTMLElement {
  return h(
    'div.lab-grid.lab-dataset-cards',
    null,
    rows.map((d) =>
      h(
        'article.lab-card.lab-dataset',
        {
          role: 'button',
          tabindex: '0',
          'aria-pressed': String(d.id === selected),
          class: d.id === selected ? 'is-selected' : '',
          onclick: () => pick(d.id),
          onkeydown: (e: Event) => {
            const key = (e as KeyboardEvent).key;
            if (key === 'Enter' || key === ' ') {
              e.preventDefault();
              pick(d.id);
            }
          },
        },
        h('header.lab-card-head', null, h('h3', null, d.name), tag(d.kind, d.kind === 'upload' ? 'info' : 'muted')),
        h(
          'div.lab-card-body',
          null,
          h(
            'dl.lab-kv',
            null,
            kv('Id', h('span.lab-mono.lab-id', null, d.id)),
            kv('Source', sourceOf(d)),
            kv('Time range', rangeOf(d)),
            kv('Rows', rowsOf(d)),
            kv('Balance', balanceBar(d)),
            kv('Taint', taintsOf(d)),
          ),
        ),
      ),
    ),
  );
}

/** The detail panel of one dataset: its facts, and its provenance folded away in a block. */
function detailOf(d: Dataset, close: () => void): HTMLElement {
  return h(
    'section.lab-card.lab-detail',
    null,
    h('header.lab-card-head', null, h('h3', null, d.name), tag(d.kind, d.kind === 'upload' ? 'info' : 'muted'), button('', close, { kind: 'ghost', icon: 'x', title: 'Close' })),
    h(
      'div.lab-card-body',
      null,
      h(
        'dl.lab-kv',
        null,
        kv('Id', h('span.lab-mono.lab-id', null, d.id)),
        kv('Source', sourceOf(d)),
        kv('Created', fmtTime(d.created_ms)),
        kv('Time range', rangeOf(d)),
        kv('Rows', fmtNumber(d.rows)),
        kv('Frozen rows', fmtNumber(d.frozen_rows)),
        kv('Label balance', balanceBar(d)),
        kv('Taint', taintsOf(d)),
        kv('SHA-256', mono(d.sha256 || 'none')),
        kv('File', mono(d.file || 'none')),
        kv('Frozen file', mono(d.frozen_file || 'none')),
      ),
      h('details.lab-json', null, h('summary', null, 'Provenance'), h('pre', null, JSON.stringify(d.provenance ?? {}, null, 2))),
    ),
  );
}

/** A datetime-local value as unix ms (local time): undefined when empty, NaN when it is not a date. */
function localMs(value: string): number | undefined {
  if (!value) return undefined;
  return new Date(value).getTime();
}

/** Why the file is not a JSONL of objects, or null when it is; `rows` counts the non-empty lines. */
function jsonlProblem(text: string): string | null {
  const lines = text.split('\n');
  let rows = 0;
  for (let i = 0; i < lines.length; i += 1) {
    const line = (lines[i] ?? '').trim();
    if (!line) continue;
    try {
      const value: unknown = JSON.parse(line);
      if (!value || typeof value !== 'object' || Array.isArray(value)) return `Line ${i + 1} is not a JSON object.`;
    } catch {
      return `Line ${i + 1} is not valid JSON.`;
    }
    rows += 1;
  }
  return rows > 0 ? null : 'The file has no rows.';
}

/**
 * The export dialog: a kind, a name and an optional time range. Review first, then confirm; the
 * dataset is written by apid and handed to `done`.
 */
function exportDialog(done: (d: Dataset) => void): void {
  const box = h('div.lab-form');
  const kind = select(EXPORT_KINDS.map((k) => ({ value: k.key, label: k.label })), 'cascade');
  const name = input({ autocomplete: 'off', placeholder: 'e.g. cascade-october' });
  const since = input({ type: 'datetime-local' });
  const until = input({ type: 'datetime-local' });
  let close: () => void = () => undefined;
  let error: string | null = null;
  let busy = false;

  const readSpec = (): { ok: true; spec: DatasetSpec } | { ok: false; problem: string } => {
    const label = name.value.trim();
    if (!label) return { ok: false, problem: 'Enter a name for the dataset.' };
    const from = localMs(since.value);
    const to = localMs(until.value);
    if (from !== undefined && Number.isNaN(from)) return { ok: false, problem: 'The start is not a valid date and time.' };
    if (to !== undefined && Number.isNaN(to)) return { ok: false, problem: 'The end is not a valid date and time.' };
    if (from !== undefined && to !== undefined && from >= to) return { ok: false, problem: 'The start must be before the end.' };
    const spec: DatasetSpec = { kind: kind.value as Exclude<DatasetKind, 'upload'>, name: label };
    if (from !== undefined) spec.since_ms = from;
    if (to !== undefined) spec.until_ms = to;
    return { ok: true, spec };
  };

  const doExport = async (spec: DatasetSpec): Promise<void> => {
    if (busy) return;
    busy = true;
    error = null;
    show('confirm', spec);
    try {
      const made = await createDataset(spec);
      toast(`Dataset ${made.name} made: ${fmtNumber(made.rows)} rows`);
      close();
      done(made);
    } catch (err) {
      busy = false;
      error = errorText(err);
      show('confirm', spec);
    }
  };

  const show = (step: 'edit' | 'confirm', spec?: DatasetSpec): void => {
    const note = error ? h('div.lab-error', null, error) : null;
    if (step === 'edit' || !spec) {
      replace(
        box,
        field('Kind', kind, 'The records to export from the plaza.'),
        field('Name', name),
        h('div.lab-row', null, field('From (optional)', since, 'Local time, inclusive.'), field('To (optional)', until, 'Local time, exclusive.')),
        note,
        h(
          'div.lab-form-actions',
          null,
          button('Cancel', () => close(), { kind: 'ghost' }),
          button(
            'Review',
            () => {
              const picked = readSpec();
              if (!picked.ok) {
                error = picked.problem;
                show('edit');
                return;
              }
              error = null;
              show('confirm', picked.spec);
            },
            { kind: 'primary' },
          ),
        ),
      );
      return;
    }
    const range = spec.since_ms !== undefined || spec.until_ms !== undefined
      ? ` from ${spec.since_ms !== undefined ? fmtTime(spec.since_ms) : 'the start'} to ${spec.until_ms !== undefined ? fmtTime(spec.until_ms) : 'now'} (local time)`
      : ' over all of the records';
    replace(
      box,
      h('p', null, 'Export ', h('span.lab-mono', null, spec.kind), ' records', range, ' as ', h('span.lab-mono', null, spec.name), '? This writes a new dataset from the plaza.'),
      note,
      h(
        'div.lab-form-actions',
        null,
        button('Back', () => show('edit'), { kind: 'ghost', disabled: busy }),
        button('Export', () => void doExport(spec), { kind: 'primary', disabled: busy }),
      ),
    );
  };

  show('edit');
  close = modal('Export from the records', box);
}

/**
 * The upload dialog: a name and a JSONL file. The file is checked line by line here (each line a JSON
 * object); apid checks the rows' fields. The new dataset is shown on the dialog and handed to `done`.
 */
function uploadDialog(done: (d: Dataset) => void): void {
  const box = h('div.lab-form');
  const name = input({ autocomplete: 'off', placeholder: 'e.g. rung-labels-october' });
  const file = input({ type: 'file', accept: '.jsonl,.ndjson,application/x-ndjson' });
  const picked = h('div.lab-file', null, 'No file chosen.');
  let close: () => void = () => undefined;
  let chosen: File | null = null;
  let error: string | null = null;
  let busy = false;
  let made: Dataset | null = null;

  file.addEventListener('change', () => {
    chosen = file.files?.[0] ?? null;
    picked.textContent = chosen ? `${chosen.name} · ${fmtSize(chosen.size)}` : 'No file chosen.';
  });

  const doUpload = async (): Promise<void> => {
    const label = name.value.trim();
    if (busy) return;
    if (!label) {
      error = 'Enter a name for the dataset.';
      show();
      return;
    }
    if (!chosen) {
      error = 'Choose a JSONL file.';
      show();
      return;
    }
    busy = true;
    error = null;
    show();
    try {
      const text = await chosen.text();
      const problem = jsonlProblem(text);
      if (problem) {
        busy = false;
        error = problem;
        show();
        return;
      }
      made = await uploadDataset(label, text);
      busy = false;
      done(made);
      show();
    } catch (err) {
      busy = false;
      error = errorText(err);
      show();
    }
  };

  const show = (): void => {
    const note = error ? h('div.lab-error', null, error) : null;
    if (made) {
      replace(
        box,
        h(
          'div.lab-result',
          null,
          h('p', null, 'Uploaded as ', h('span.lab-mono', null, made.id), '.'),
          h(
            'dl.lab-kv',
            null,
            kv('Name', made.name),
            kv('Rows', fmtNumber(made.rows)),
            kv('Frozen rows', fmtNumber(made.frozen_rows)),
            kv('Label balance', balanceBar(made)),
            kv('Taint', taintsOf(made)),
          ),
        ),
        h('div.lab-form-actions', null, button('Done', () => close(), { kind: 'primary' })),
      );
      return;
    }
    replace(
      box,
      field('Name', name),
      field('File', file, 'One JSON object per line. Each row needs ts (unix ms), label and the recipe features.'),
      picked,
      note,
      h(
        'div.lab-form-actions',
        null,
        button('Cancel', () => close(), { kind: 'ghost', disabled: busy }),
        button(busy ? 'Uploading' : 'Upload', () => void doUpload(), { kind: 'primary', disabled: busy }),
      ),
    );
  };

  show();
  close = modal('Upload a JSONL dataset', box);
}

export function render(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  let rows: Dataset[] | null = null;
  let failure: string | null = null;
  let view: View = 'table';
  let selected: string | null = null;
  const body = h('div.lab-stack');
  replace(root, body);

  const load = (): void => {
    listDatasets().then(
      (list) => {
        if (!alive) return;
        rows = list;
        failure = null;
        paint();
      },
      (err: unknown) => {
        if (!alive) return;
        failure = errorText(err);
        paint();
      },
    );
  };

  /** A new dataset (exported or uploaded): it is selected, and the list reloads to show it. */
  const created = (d: Dataset): void => {
    selected = d.id;
    load();
  };

  const pick = (id: string): void => {
    selected = selected === id ? null : id;
    paint();
  };

  const paint = (): void => {
    const restore = keepState(body);
    const sorted = rows ? [...rows].sort((a, b) => b.created_ms - a.created_ms) : null;
    const current = sorted?.find((d) => d.id === selected) ?? null;
    const toggle = h('div.lab-seg', { role: 'group', 'aria-label': 'View' }, VIEWS.map((v) => {
      const b = h('button', { type: 'button', 'aria-pressed': String(v.key === view) }, v.label);
      b.addEventListener('click', () => {
        view = v.key;
        paint();
      });
      return b;
    }));
    const actions = h('div.lab-actions', null, h('span.lab-muted', null, sorted ? `${sorted.length} ${sorted.length === 1 ? 'dataset' : 'datasets'}` : ''), toggle, button('Upload', () => uploadDialog(created), { title: 'Upload a JSONL file as a dataset' }), button('Export', () => exportDialog(created), { kind: 'primary', title: 'Export from the plaza records' }));
    const nodes: Node[] = [h('div.lab-page-head', null, h('h2', null, 'Datasets'), actions)];
    if (failure) nodes.push(h('div.lab-error', null, failure));
    if (current) nodes.push(detailOf(current, () => pick(current.id)));
    if (sorted && sorted.length === 0) {
      nodes.push(labEmpty('library', 'No datasets yet', "Export cascade outcomes or tasks from the plaza's records, or upload a JSONL file of rows."));
    } else if (sorted) {
      nodes.push(view === 'table' ? tableOf(sorted, selected, pick) : cardsOf(sorted, selected, pick));
    }
    replace(body, nodes);
    restore();
  };

  const stop = pollEvery(5000, load);
  return () => {
    alive = false;
    stop();
  };
}
