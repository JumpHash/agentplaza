// The list-like panel bodies: tables, bars, splits, stats and flags. panels.ts calls these; the alerting and tracing
// bodies are in bodies-ops.ts, charts and figures in charts.ts and figures.ts.

import { button, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { PanelCtx } from './panels';
import type { BarRow, Cell, Column, Flag, LegendItem, Row, SplitRow, StatLine, Table, Tone } from './model';
import { statusVar, toneVar } from './model';
import { spark } from './figures';

/** Where a click goes: a scope, a tab and a trace, any of them optional. */
interface Target {
  scope?: string;
  tab?: string;
  trace?: string;
}

/** The navigation a row or flag asks for, leaving out the fields it does not set. */
function targetOf(fields: Target): Target {
  const target: Target = {};
  if (fields.scope) target.scope = fields.scope;
  if (fields.tab) target.tab = fields.tab;
  if (fields.trace) target.trace = fields.trace;
  return target;
}

/** A fraction clamped to the range 0 to 1, so a bar never leaves its track. */
function clamp(frac: number): number {
  return Math.min(1, Math.max(0, frac));
}

/** A table of rows. Rows that name a scope, tab or trace open it on a click or on Enter. When `more` is set, a
 * "Load older" button under the rows fetches the next page and appends its rows. */
export function table(host: HTMLElement, t: Table, ctx: PanelCtx): void {
  if (t.rows.length === 0) {
    replace(host, h('div.mx-empty', null, t.empty || 'Nothing here yet.'));
    return;
  }
  const head = h('tr', null, t.columns.map((c) => h('th', { class: c.num ? 'mx-r' : null }, c.head)));
  const tbody = h('tbody', null, t.rows.map((row) => tableRow(row, t.columns, ctx)));
  const moreHost = h('div.mx-more');
  let current = t;

  const paintMore = (): void => {
    if (!current.more) {
      replace(moreHost, null);
      return;
    }
    const older: HTMLButtonElement = button('Load older', () => void loadOlder(older), { size: 'sm' });
    replace(moreHost, older);
  };

  const loadOlder = async (btn: HTMLButtonElement): Promise<void> => {
    const route = current.more;
    if (!route) return;
    btn.disabled = true;
    try {
      const page = await ctx.page(route);
      current = appendPage(current, page);
      for (const row of page.rows) tbody.appendChild(tableRow(row, current.columns, ctx));
      paintMore();
    } catch (err) {
      btn.disabled = false;
      toast(err instanceof Error ? err.message : 'Could not load older rows', 'error');
    }
  };

  replace(host, h('div.mx-tablewrap', null, h('table.mx-t', null, h('thead', null, head), tbody)), moreHost);
  paintMore();
}

/** The table with a page's rows under its own rows, and the page's `more` in place of the table's. `t` is not changed. */
export function appendPage(t: Table, page: Table): Table {
  return { ...t, rows: [...t.rows, ...page.rows], more: page.more ?? null };
}

function tableRow(row: Row, columns: Column[], ctx: PanelCtx): HTMLTableRowElement {
  const clickable = !!(row.scope || row.tab || row.trace);
  const classes = [row.selected ? 'mx-sel' : '', clickable ? 'mx-click' : ''].filter(Boolean).join(' ');
  const tr = h('tr', { class: classes || null, tabindex: clickable ? 0 : null });
  row.cells.forEach((cell, i) => {
    const numeric = columns[i]?.num ?? false;
    // Only free text wraps; names, ids, numbers and pills stay on one line (the desktop sizes its columns the same way).
    const wraps = !numeric && (cell.type === 'text' || cell.type === 'muted');
    const cellClasses = [numeric ? 'mx-r' : '', numeric || cell.type === 'num' ? 'mx-num' : '', wraps ? 'mx-wrap' : ''].filter(Boolean).join(' ');
    tr.appendChild(h('td', { class: cellClasses || null }, cellContent(cell)));
  });
  if (clickable) {
    const go = () => ctx.go(targetOf(row));
    tr.addEventListener('click', (e) => {
      if (e.target instanceof Element && e.target.closest('button, input')) return;
      go();
    });
    tr.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') go();
    });
  }
  return tr;
}

/** The content of one table cell, by the cell's kind. */
/** Free text wraps at spaces only: hyphens become non-breaking, so a name like "sonnet-5.5" stays whole (as on the
 * desktop). */
export function wrapsAtSpaces(text: string): string {
  return text.replace(/-/g, '\u2011');
}

function cellContent(cell: Cell): Node | string {
  switch (cell.type) {
    case 'text':
      return wrapsAtSpaces(cell.text);
    case 'num':
      return cell.text;
    case 'mono':
      return h('span.mx-mono', null, cell.text);
    case 'named':
      return h('span', null, cell.name, ' ', h('span.mx-mono.mx-mute', null, cell.id));
    case 'status':
      return h('span', { class: `mx-${cell.status}` }, cell.text);
    case 'pill':
      return h('span.mx-pill', { class: `mx-${cell.status}` }, cell.text);
    case 'muted':
      return h('span.mx-mute', null, wrapsAtSpaces(cell.text));
    case 'spark':
      return spark(cell.values, cell.tone);
    case 'bar':
      return miniBar(cell.parts);
  }
}

/** A small bar in a table cell: one coloured part per (percent, tone, label), each titled with its label and share. */
function miniBar(parts: [number, Tone, string][]): HTMLSpanElement {
  return h(
    'span.mx-mbar',
    null,
    parts.map(([pct, tone, label]) =>
      h('i', { title: `${label} ${Math.round(pct)}%`, style: `width: ${pct}%; background: ${toneVar(tone)}` }),
    ),
  );
}

/** A column of flags. Each one is a button that opens its tab or scope. */
export function flags(host: HTMLElement, list: Flag[], ctx: PanelCtx): void {
  if (list.length === 0) {
    replace(host, h('div.mx-empty', null, 'Nothing here yet.'));
    return;
  }
  const buttons = list.map((f) => {
    const button = h('button.mx-flag', { type: 'button', class: `mx-${f.status}` }, f.text, h('span.mx-when', null, f.when));
    button.addEventListener('click', () => ctx.go(targetOf(f)));
    return button;
  });
  replace(host, h('div.mx-flags', null, buttons));
}

/** Budget rows: each has its label and value, a bar filled by its fraction and a status colour, and a muted note. */
export function bars(host: HTMLElement, rows: BarRow[], note: string): void {
  replace(host, h('div.mx-budget', null, rows.map(barRow)), note ? h('div.mx-sub', null, note) : null);
}

function barRow(row: BarRow): HTMLDivElement {
  const fill = h('i', { style: `width: ${clamp(row.frac) * 100}%; background: ${statusVar(row.status)}` });
  return h(
    'div.mx-brow',
    null,
    h('span', null, row.label),
    h('span.mx-num', null, row.value),
    h('div.mx-bar', null, fill),
    row.note ? h('div.mx-mute', null, row.note) : null,
  );
}

/**
 * Stacked rows on one scale. The longest row fills the track, and each part's width is its value over that longest
 * total, so the rows compare with each other.
 */
export function split(host: HTMLElement, rows: SplitRow[], legend: LegendItem[]): void {
  const largest = Math.max(0, ...rows.map(splitTotal));
  replace(
    host,
    h('div.mx-split', null, rows.map((row) => splitRow(row, largest))),
    legend.length
      ? h(
          'div.mx-legend',
          null,
          legend.map((item) => h('span', null, h('i', { style: `background: ${toneVar(item.tone)}` }), item.label)),
        )
      : null,
  );
}

function splitTotal(row: SplitRow): number {
  return row.parts.reduce((sum, [value]) => sum + value, 0);
}

function splitRow(row: SplitRow, largest: number): HTMLDivElement {
  const track = h(
    'div',
    { style: 'display: flex; height: 10px; background: var(--rule-faint)' },
    row.parts.map(([value, tone, label]) =>
      h('i', {
        title: label,
        style: `width: ${largest > 0 ? (value / largest) * 100 : 0}%; height: 100%; background: ${toneVar(tone)}`,
      }),
    ),
  );
  return h(
    'div.mx-split-row',
    { style: 'display: grid; grid-template-columns: 80px 1fr auto; gap: 10px; align-items: center; margin-bottom: 6px' },
    h('span.mx-mute', null, row.label),
    track,
    h('span.mx-num', null, row.total),
  );
}

/** A vertical list of figures: a small muted label, then the big value with its note beside it. */
export function stats(host: HTMLElement, list: StatLine[]): void {
  replace(
    host,
    h(
      'div.mx-stats',
      { style: 'display: grid; gap: 10px; margin-top: 6px' },
      list.map((s) =>
        h(
          'div',
          null,
          h('div.mx-mute', { style: 'font-size: 11px' }, s.label),
          h(
            'div',
            { style: 'font: 600 22px var(--font)' },
            s.value,
            s.note ? [' ', h('span.mx-mute', { style: 'font: 400 12px var(--font)' }, s.note)] : null,
          ),
        ),
      ),
    ),
  );
}
