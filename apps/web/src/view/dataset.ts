// The dataset body of the web console's artifact view: a toolbar (filter, row count, page controls) over a
// scrollable table with a sticky head whose columns sort on click. Inline rows are filtered, sorted and paged
// locally; rows held by a blob are fetched one page at a time through the `rows` callback.

import '../styles/view-dataset.css';
import { format } from '../metrics/format';
import type { Unit } from '../metrics/model';
import type { DatasetBody, DatasetColumn, RowsPage } from './model';

export type { DatasetBody, DatasetColumn, RowsPage } from './model';

export type RowsQuery = {
  offset: number;
  limit: number;
  sort?: string;
  desc?: boolean;
  filter?: string;
};

/** Rows per page in the table. */
export const DATASET_PAGE = 100;

/** How long the filter waits after the last keystroke before it applies. */
const FILTER_DELAY_MS = 250;

function isBlank(v: unknown): boolean {
  return v === null || v === undefined;
}

/** The number a value stands for, or null when it is not a finite number or a string that parses to one. */
function numeric(v: unknown): number | null {
  if (typeof v === 'number') return Number.isFinite(v) ? v : null;
  if (typeof v === 'string' && v.trim() !== '') {
    const n = Number(v);
    return Number.isFinite(n) ? n : null;
  }
  return null;
}

/** The plain text of a cell, used for filtering and for string sorting. */
function cellText(v: unknown): string {
  if (typeof v === 'string') return v;
  if (typeof v === 'object') return JSON.stringify(v);
  return String(v);
}

/** Keeps the rows that have a cell containing the query, case-insensitively. An empty query keeps every row. */
export function filterRows(rows: unknown[][], q: string): unknown[][] {
  const needle = q.trim().toLowerCase();
  if (needle === '') return rows;
  return rows.filter((row) => row.some((cell) => !isBlank(cell) && cellText(cell).toLowerCase().includes(needle)));
}

/**
 * Sorts rows by one column and returns a new array. The sort is numeric when every present value parses as a
 * number, and by string otherwise. Blank cells sort last in both directions.
 */
export function sortRows(rows: unknown[][], col: number, desc: boolean): unknown[][] {
  const present = rows.map((row) => row[col]).filter((v) => !isBlank(v));
  const asNumbers = present.every((v) => numeric(v) !== null);
  const sign = desc ? -1 : 1;
  return rows.slice().sort((a, b) => {
    const x = a[col];
    const y = b[col];
    const xBlank = isBlank(x);
    const yBlank = isBlank(y);
    if (xBlank || yBlank) return xBlank === yBlank ? 0 : xBlank ? 1 : -1;
    if (asNumbers) return ((numeric(x) ?? 0) - (numeric(y) ?? 0)) * sign;
    const sx = cellText(x);
    const sy = cellText(y);
    return (sx < sy ? -1 : sx > sy ? 1 : 0) * sign;
  });
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** A date from an epoch-millisecond number, a numeric string, or an ISO-like string; null when none applies. */
function toDate(v: unknown): Date | null {
  const n = numeric(v);
  if (n !== null) return new Date(n);
  if (typeof v === 'string' && v.trim() !== '') {
    const d = new Date(v);
    return Number.isNaN(d.getTime()) ? null : d;
  }
  return null;
}

/** A local date and time as YYYY-MM-DD HH:MM. */
function stamp(d: Date): string {
  const p = (n: number): string => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** A number as text, formatted by its unit when the column has one. */
function numberText(n: number, unit: string | null | undefined): string {
  if (unit) {
    const text = format(unit as Unit, n);
    if (typeof text === 'string' && text !== '') return text;
  }
  return n.toLocaleString(undefined, { maximumFractionDigits: 6 });
}

/** One table cell for a value in a column, styled by the column's kind. */
function cellNode(col: DatasetColumn, v: unknown): HTMLTableCellElement {
  const td = el('td');
  if (isBlank(v)) {
    td.append(el('span', 'muted view-dataset-null', '—'));
    return td;
  }
  const flag = typeof v === 'boolean' ? v : col.kind === 'bool' && (v === 'true' || v === 'false') ? v === 'true' : null;
  if (flag !== null) {
    td.textContent = flag ? 'yes' : 'no';
    return td;
  }
  if (col.kind === 'time') {
    const date = toDate(v);
    if (date !== null) {
      td.classList.add('is-time', 'mono');
      td.textContent = stamp(date);
      return td;
    }
  }
  if (col.kind === 'number' || (col.kind === undefined && typeof v === 'number')) {
    const n = numeric(v);
    if (n !== null) {
      td.classList.add('is-num', 'num');
      td.textContent = numberText(n, col.unit);
      return td;
    }
  }
  const text = cellText(v);
  td.textContent = text;
  td.title = text;
  return td;
}

/**
 * The dataset view. `body.rows` is shown at once; with a blob, the first page is then fetched through `rows`
 * so the table reflects the stored rows, and every later page, sort and filter goes through `rows` too.
 */
export function renderDataset(
  panelId: string,
  body: DatasetBody,
  rows: (panelId: string, q: RowsQuery) => Promise<RowsPage>,
): HTMLElement {
  const columns = body.columns;
  const blob = body.blob != null;
  const state = { offset: 0, sort: -1, desc: false, filter: '' };
  let shown: { rows: unknown[][]; total: number } = { rows: body.rows, total: body.total };
  let loading = false;
  let failure: string | null = null;
  let seq = 0;
  let filterTimer: number | undefined;

  const root = el('div', 'view-dataset');
  const bar = el('div', 'cluster view-dataset-bar');

  const filter = el('input', 'input view-dataset-filter');
  filter.type = 'search';
  filter.placeholder = 'Filter rows';
  filter.setAttribute('aria-label', 'Filter rows');

  const count = el('span', 'muted num view-dataset-count');
  const status = el('span', 'hint view-dataset-status');

  const pager = el('div', 'cluster view-dataset-pager');
  const prevButton = el('button', 'btn ghost sm icon-only view-dataset-prev', '‹');
  prevButton.type = 'button';
  prevButton.setAttribute('aria-label', 'Previous page');
  const range = el('span', 'muted num view-dataset-range');
  const nextButton = el('button', 'btn ghost sm icon-only view-dataset-next', '›');
  nextButton.type = 'button';
  nextButton.setAttribute('aria-label', 'Next page');
  pager.append(prevButton, range, nextButton);

  bar.append(filter, count, status, pager);

  const error = el('div', 'notice view-dataset-error');
  error.setAttribute('role', 'alert');
  error.hidden = true;

  const scroll = el('div', 'tbl-wrap view-dataset-scroll');
  const table = el('table', 'tbl view-dataset-table');
  const thead = el('thead');
  const headRow = el('tr');
  const heads: { th: HTMLTableCellElement; arrow: HTMLSpanElement }[] = [];
  columns.forEach((col, i) => {
    const th = el('th');
    th.scope = 'col';
    if (col.kind === 'number') th.classList.add('num');
    const button = el('button', 'view-dataset-sort');
    button.type = 'button';
    const arrow = el('span', 'view-dataset-arrow');
    arrow.setAttribute('aria-hidden', 'true');
    button.append(el('span', '', col.label), arrow);
    button.addEventListener('click', () => cycleSort(i));
    th.append(button);
    headRow.append(th);
    heads.push({ th, arrow });
  });
  thead.append(headRow);
  const tbody = el('tbody');
  table.append(thead, tbody);
  scroll.append(table);
  root.append(bar, error, scroll);

  /** Keeps the offset on a real page for the given total. */
  function clampOffset(total: number): void {
    const last = total > 0 ? Math.floor((total - 1) / DATASET_PAGE) * DATASET_PAGE : 0;
    if (state.offset > last) state.offset = last;
  }

  /** Filters, sorts and pages the inline rows. */
  function localPage(): { rows: unknown[][]; total: number } {
    const matched = filterRows(body.rows, state.filter);
    const ordered = state.sort >= 0 ? sortRows(matched, state.sort, state.desc) : matched;
    clampOffset(ordered.length);
    return { rows: ordered.slice(state.offset, state.offset + DATASET_PAGE), total: ordered.length };
  }

  /** Fetches the current page, sort and filter from the blob. The previous rows stay until the new page arrives. */
  function load(): void {
    const mine = ++seq;
    loading = true;
    failure = null;
    paint();
    const q: RowsQuery = { offset: state.offset, limit: DATASET_PAGE };
    const sorted = state.sort >= 0 ? columns[state.sort] : undefined;
    if (sorted) {
      q.sort = sorted.key;
      q.desc = state.desc;
    }
    if (state.filter !== '') q.filter = state.filter;
    rows(panelId, q).then(
      (page) => {
        if (mine !== seq) return;
        shown = { rows: page.rows, total: page.total };
        loading = false;
        paint();
      },
      (err: unknown) => {
        if (mine !== seq) return;
        loading = false;
        failure = err instanceof Error ? err.message : String(err);
        paint();
      },
    );
  }

  /** Brings the rows up to date with the state: a fetch for a blob, a local page otherwise. */
  function refresh(): void {
    if (blob) {
      load();
      return;
    }
    shown = localPage();
    paint();
  }

  /** Cycles a column's sort: ascending, then descending, then none. */
  function cycleSort(col: number): void {
    if (state.sort !== col) {
      state.sort = col;
      state.desc = false;
    } else if (!state.desc) {
      state.desc = true;
    } else {
      state.sort = -1;
      state.desc = false;
    }
    state.offset = 0;
    refresh();
  }

  /** Writes the toolbar, the table body, the sort arrows and the status lines from the current state. */
  function paint(): void {
    const total = shown.total;
    count.textContent = `${total.toLocaleString()} ${total === 1 ? 'row' : 'rows'}`;
    status.textContent = loading ? 'Loading' : '';
    range.textContent =
      total === 0
        ? '0 of 0'
        : `${state.offset + 1}–${Math.min(state.offset + DATASET_PAGE, total)} of ${total.toLocaleString()}`;
    prevButton.disabled = loading || state.offset === 0;
    nextButton.disabled = loading || state.offset + DATASET_PAGE >= total;

    scroll.classList.toggle('is-loading', loading);
    scroll.setAttribute('aria-busy', String(loading));
    error.hidden = failure === null;
    error.textContent = failure === null ? '' : `Could not load rows. ${failure}`;

    heads.forEach((head, i) => {
      const active = state.sort === i;
      head.th.setAttribute('aria-sort', active ? (state.desc ? 'descending' : 'ascending') : 'none');
      head.arrow.textContent = active ? (state.desc ? '↓' : '↑') : '';
    });

    const nodes = shown.rows.map((row) => {
      const tr = el('tr');
      columns.forEach((col, i) => tr.append(cellNode(col, row[i])));
      return tr;
    });
    if (nodes.length === 0 && !loading) {
      const tr = el('tr', 'view-dataset-empty');
      const td = el('td', 'muted', state.filter !== '' ? 'No rows match this filter.' : 'No rows.');
      td.colSpan = Math.max(1, columns.length);
      tr.append(td);
      nodes.push(tr);
    }
    tbody.replaceChildren(...nodes);
  }

  filter.addEventListener('input', () => {
    window.clearTimeout(filterTimer);
    filterTimer = window.setTimeout(() => {
      const value = filter.value.trim();
      if (value === state.filter) return;
      state.filter = value;
      state.offset = 0;
      refresh();
    }, FILTER_DELAY_MS);
  });

  prevButton.addEventListener('click', () => {
    state.offset = Math.max(0, state.offset - DATASET_PAGE);
    refresh();
  });

  nextButton.addEventListener('click', () => {
    if (state.offset + DATASET_PAGE >= shown.total) return;
    state.offset += DATASET_PAGE;
    refresh();
  });

  if (blob) {
    paint();
    load();
  } else {
    refresh();
  }
  return root;
}
