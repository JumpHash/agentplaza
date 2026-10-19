// The Lab's datasets (`#/lab/datasets`), in the prototype's layout: a bar (the count, the Table or
// Cards view, Upload, Export), the datasets newest first, and a detail card under the list for the
// one picked. Export and upload are dialogs; an export is confirmed before it is written.

import { createDataset, listDatasets, uploadDataset, type Dataset, type DatasetKind, type DatasetSpec } from '../api';
import { badge, card, field, input, modal, select, table as dataTable, toast, type Column } from '../ui/components';
import { h, keepState, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { agoText, errorText, fmtNumber, fmtTime, labButton, labEmpty, pollEvery, seg } from './lab-common';

type View = 'table' | 'cards';

const VIEWS: [View, string][] = [
  ['table', 'Table'],
  ['cards', 'Cards'],
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

const kindBadge = (d: Dataset): HTMLElement => badge(d.kind, d.kind === 'upload' ? 'info' : 'muted');
const mono = (text: string): HTMLElement => h('span.mono', null, text);
const fact = (label: string, value: Node | string): HTMLElement => h('div', null, h('dt', null, label), h('dd', null, value));

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
  if (total === 0) return h('span.lab2-note', null, 'no labels');
  const label = entries.map(([k, n]) => `${k} ${fmtNumber(n)}`).join(', ');
  return h(
    'div',
    null,
    h(
      'div.lab2-bal',
      { role: 'img', 'aria-label': label },
      entries.map(([k, n]) => h('span', { class: CLASS_ORDER.includes(k) ? `lab2-c-${k}` : 'lab2-c-other', style: `flex-grow:${n}`, title: `${k} ${fmtNumber(n)}` })),
    ),
    h('div.lab2-legend', null, entries.map(([k, n]) => h('span', null, `${k} ${fmtNumber(n)}`))),
  );
}

/** The taint flags as warn badges, or `none`. */
function taintsOf(d: Dataset): Node | string {
  if (d.taint.length === 0) return 'none';
  return h('span', null, ...d.taint.map((t) => h('span', null, badge(t, 'warn'), ' ')));
}

/** The row count, with the frozen set under it (a padlock and its count) when there is one. */
function rowsOf(d: Dataset): HTMLElement {
  return h(
    'div',
    null,
    h('span', null, fmtNumber(d.rows)),
    d.frozen_rows > 0 ? h('div.lab2-note', { title: `Frozen file: ${d.frozen_file}` }, `${fmtNumber(d.frozen_rows)} frozen`) : null,
  );
}

/** The time span, from and to, in local dates. */
const rangeOf = (d: Dataset): string => `${new Date(d.from_ms).toLocaleDateString()} to ${new Date(d.to_ms).toLocaleDateString()}`;

/** The datasets as a table; a row opens its detail. The rows keep their order (newest first). */
function tableOf(rows: Dataset[], selected: string | null, pick: (id: string) => void): HTMLElement {
  const cols: Column<Dataset>[] = [
    { label: 'Name', cell: (d) => h('div', null, h('strong', null, d.name), h('div.mono.lab2-note', null, d.id)) },
    { label: 'Kind', cell: (d) => kindBadge(d) },
    { label: 'Source', cell: (d) => sourceOf(d) },
    { label: 'Rows', num: true, cell: (d) => rowsOf(d) },
    { label: 'Time range', cell: (d) => rangeOf(d) },
    { label: 'Label balance', cell: (d) => balanceBar(d) },
    { label: 'Taint', cell: (d) => taintsOf(d) },
    { label: 'Created', cell: (d) => agoText(d.created_ms) },
  ];
  const el = h('div.tbl-wrap', null, dataTable(cols, rows, (d) => pick(d.id), (d) => d.id));
  for (const tr of el.querySelectorAll<HTMLTableRowElement>('tbody tr')) {
    if (tr.dataset.key === selected) tr.setAttribute('aria-current', 'true');
  }
  return el;
}

/** The datasets as cards: the kind, the rows, the balance, and an Open action that opens the detail. */
function cardsOf(rows: Dataset[], pick: (id: string) => void): HTMLElement {
  return h(
    'div.lab2-grid',
    null,
    rows.map((d) =>
      card(
        d.name,
        h(
          'div',
          null,
          h('div.lab2-row', { style: 'justify-content:flex-start' }, kindBadge(d), h('span.lab2-note', null, `${fmtNumber(d.rows)} rows${d.frozen_rows ? `, ${fmtNumber(d.frozen_rows)} frozen` : ''}`)),
          balanceBar(d),
        ),
        { actions: [labButton('Open', () => pick(d.id), { kind: 'ghost', sm: true })] },
      ),
    ),
  );
}

/** The detail card of one dataset: its facts, and its provenance folded away in a block. */
function detailOf(d: Dataset, close: () => void): HTMLElement {
  const body = h(
    'div',
    null,
    h(
      'dl.kv',
      null,
      fact('Id', mono(d.id)),
      fact('Source', sourceOf(d)),
      fact('Created', fmtTime(d.created_ms)),
      fact('Time range', rangeOf(d)),
      fact('Rows', fmtNumber(d.rows)),
      fact('Frozen rows', d.frozen_rows ? fmtNumber(d.frozen_rows) : 'none'),
      fact('Label balance', balanceBar(d)),
      fact('Taint', d.taint.length ? d.taint.join(', ') : 'none'),
      fact('SHA-256', mono(d.sha256 || 'none')),
      fact('File', mono(d.file || 'none')),
      fact('Frozen file', mono(d.frozen_file || 'none')),
    ),
    h('details', { style: 'margin-top:12px' }, h('summary', null, 'Provenance'), h('pre.lab2-out', null, JSON.stringify(d.provenance ?? null, null, 2))),
  );
  return h('div', null, card(d.name, body, { actions: [labButton('', close, { kind: 'ghost', icon: 'x', title: 'Close' })] }));
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
  const box = h('div.lab2-form');
  const kind = select(EXPORT_KINDS.map((k) => ({ value: k.key, label: k.label })), 'cascade');
  const name = input({ autocomplete: 'off', placeholder: 'e.g. cascade-october', 'aria-label': 'Name' });
  const since = input({ type: 'datetime-local', 'aria-label': 'From (optional)' });
  const until = input({ type: 'datetime-local', 'aria-label': 'To (optional)' });
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
    const note = h('p.lab2-err', { role: 'alert' }, error ?? '');
    if (step === 'edit' || !spec) {
      replace(
        box,
        h('label.field', null, h('span.field-l', null, 'Kind'), kind, h('small.hint', null, 'The records to export from the plaza.')),
        field('Name', name),
        h('div.lab2-row', { style: 'justify-content:flex-start' }, field('From (optional)', since, 'Local time, inclusive.'), field('To (optional)', until, 'Local time, exclusive.')),
        note,
        h(
          'div.lab-actions',
          null,
          labButton('Cancel', () => close(), { kind: 'ghost' }),
          labButton(
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
    const range =
      spec.since_ms !== undefined || spec.until_ms !== undefined
        ? ` from ${spec.since_ms !== undefined ? fmtTime(spec.since_ms) : 'the start'} to ${spec.until_ms !== undefined ? fmtTime(spec.until_ms) : 'now'} (local time)`
        : ' over all of the records';
    replace(
      box,
      h('p', null, 'Export ', h('span.mono', null, spec.kind), ' records', range, ' as ', h('span.mono', null, spec.name), '? This writes a new dataset from the plaza.'),
      note,
      h(
        'div.lab-actions',
        null,
        labButton('Back', () => show('edit'), { kind: 'ghost', disabled: busy }),
        labButton('Export', () => void doExport(spec), { kind: 'primary', disabled: busy }),
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
  const box = h('div.lab2-form');
  const name = input({ autocomplete: 'off', placeholder: 'e.g. rung-labels-october', 'aria-label': 'Name' });
  const file = input({ type: 'file', accept: '.jsonl,.ndjson,application/x-ndjson', 'aria-label': 'JSONL file' });
  let close: () => void = () => undefined;
  let chosen: File | null = null;
  let error: string | null = null;
  let busy = false;
  let made: Dataset | null = null;

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
    const note = h('p.lab2-err', { role: 'alert' }, error ?? '');
    if (made) {
      replace(
        box,
        h('p', null, 'Uploaded as ', h('span.mono', null, made.id), '.'),
        h('dl.kv', null, fact('Name', made.name), fact('Rows', fmtNumber(made.rows)), fact('Frozen rows', fmtNumber(made.frozen_rows)), fact('Label balance', balanceBar(made)), fact('Taint', taintsOf(made))),
        h('div.lab-actions', null, labButton('Done', () => close(), { kind: 'primary' })),
      );
      return;
    }
    replace(
      box,
      field('Name', name),
      h('label.field', null, h('span.field-l', null, 'File'), file, h('small.hint', null, 'One JSON object per line: .jsonl or .ndjson.')),
      note,
      h(
        'div.lab-actions',
        null,
        labButton('Cancel', () => close(), { kind: 'ghost', disabled: busy }),
        labButton(busy ? 'Uploading' : 'Upload', () => void doUpload(), { kind: 'primary', icon: 'upload', disabled: busy }),
      ),
    );
  };

  file.addEventListener('change', () => {
    chosen = file.files?.[0] ?? null;
  });
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
    const bar = h(
      'div.lab2-row',
      null,
      h('span.lab2-note', null, sorted ? `${sorted.length} ${sorted.length === 1 ? 'dataset' : 'datasets'}` : ''),
      seg(VIEWS, view, (v) => {
        view = v as View;
        paint();
      }),
      labButton('Upload', () => uploadDialog(created), { kind: 'ghost', icon: 'upload', title: 'Upload a JSONL file as a dataset' }),
      labButton('Export', () => exportDialog(created), { kind: 'primary', icon: 'download', title: 'Export from the plaza records' }),
    );
    const nodes: Node[] = [bar];
    if (failure) nodes.push(h('p.lab-err', { role: 'alert' }, failure));
    if (sorted && sorted.length === 0) {
      nodes.push(labEmpty('library', 'No datasets yet', "Export cascade outcomes or tasks from the plaza's records, or upload a JSONL file of rows."));
    } else if (sorted) {
      nodes.push(view === 'table' ? tableOf(sorted, selected, pick) : cardsOf(sorted, pick));
    }
    if (current) nodes.push(detailOf(current, () => pick(current.id)));
    replace(body, nodes);
    restore();
  };

  const stop = pollEvery(5000, load);
  return () => {
    alive = false;
    stop();
  };
}
