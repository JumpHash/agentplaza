// The operational panel bodies: firing alerts (with mute, acknowledge, open), the rules table (on/off, new rule),
// kept traces (filter, list, waterfall, span details) and the events feed (kind filter).

import { badge, button } from '../ui/components';
import { h } from '../ui/dom';
import { format } from './format';
import { spark } from './figures';
import type { PanelCtx } from './panels';
import { toneVar, type AlertRow, type EventRow, type LegendItem, type RuleRow, type SpanRow, type Status, type TraceDetail, type TracesBody } from './model';

const TRACE_FILTERS = ['all', 'error', 'slow', 'sampled', 'open'];

const STATUS_TONE: Record<Status, 'ok' | 'warn' | 'danger' | 'info' | 'muted'> = { ok: 'ok', warn: 'warn', crit: 'danger', info: 'info', mute: 'muted' };
const STATUS_WORD: Record<Status, string> = { ok: 'OK', warn: 'Warning', crit: 'Critical', info: 'Info', mute: 'Muted' };

/** A status chip with its dot; the text defaults to the status's word. */
function statusBadge(status: Status, text: string = STATUS_WORD[status]): HTMLElement {
  return badge(text, STATUS_TONE[status], true);
}

/** The firing rules: the rule (a link to its scope), its scope, condition, trend, severity and the Mute and Acknowledge actions. */
export function alerts(host: HTMLElement, rows: AlertRow[], ctx: PanelCtx): void {
  if (!rows.length) {
    host.append(h('div.mt-empty', null, 'Nothing is firing.'));
    return;
  }
  const trs = rows.map((r) =>
    h('tr', null,
      h('td', null, h('button.mt-link', { type: 'button', onclick: () => ctx.go({ scope: r.scope, tab: r.tab }) }, r.name)),
      h('td', null, r.scope_label),
      h('td', null, r.cond),
      h('td', null, r.for_text),
      h('td', null, r.since),
      h('td', { class: 'num' }, r.now),
      h('td', null, spark(r.spark, 'chart-1', 96, 22, 'mt-spark mt-spk')),
      h('td', null, statusBadge(r.severity), r.acked ? ' ' : null, r.acked ? badge('acknowledged', 'muted') : null),
      h('td', { class: 'mt-acts' },
        r.acked
          ? null
          : [
              button('Mute', () => ctx.mute(r.rule), { kind: 'ghost', size: 'sm', title: 'Mute for 1 h' }),
              ' ',
              button('Acknowledge', () => ctx.ack(r.rule, r.scope), { kind: 'ghost', size: 'sm' }),
            ])));
  host.append(table(['Alert', 'Scope', 'Condition', 'For', 'Since', ['Now', 'num'], 'Trend', 'Severity', ''], trs));
}

/** The rules table, with a New alert rule button above it. Turning a rule on or off reports the new state. */
export function rules(host: HTMLElement, rows: RuleRow[], ctx: PanelCtx): void {
  host.append(button('New alert rule', () => ctx.newRule(), { kind: 'primary', size: 'sm' }));
  if (!rows.length) {
    host.append(h('div.mt-empty', null, 'No alert rules.'));
    return;
  }
  const trs = rows.map((r) =>
    h('tr', null,
      h('td', null, toggle(r, ctx)),
      h('td', null, r.name),
      h('td', null, r.scope_label),
      h('td', null, r.cond),
      h('td', null, r.for_text),
      h('td', null, statusBadge(r.severity)),
      h('td', null, r.state),
      h('td', { class: 'muted' }, r.last || '—'),
      h('td', null, r.starter ? badge('starter', 'accent') : null)));
  host.append(table(['On', 'Rule', 'Scope', 'Condition', 'For', 'Severity', 'State', 'Last', ''], trs));
}

/** The on/off switch of a rule. */
function toggle(r: RuleRow, ctx: PanelCtx): HTMLElement {
  const sw = h('button.mt-toggle', { type: 'button', role: 'switch', 'aria-checked': String(r.on), 'aria-label': r.name }, h('i'));
  sw.addEventListener('click', () => ctx.toggleRule(r.id, !r.on));
  return sw;
}

/** The kept traces: a kind filter, the list of turns, then the selected turn's waterfall and span details. */
export function traces(host: HTMLElement, t: TracesBody, ctx: PanelCtx): void {
  const filter = segmented(TRACE_FILTERS, t.filter, (k) => ctx.filter(k), 'Trace kind');
  filter.classList.add('mt-kinds');
  host.append(filter);
  const selectedId = t.selected?.id;
  const trs = t.list.length
    ? t.list.map((r) => {
        const open = () => ctx.go({ tab: 'traces', trace: r.id });
        const row = h('tr', { class: r.id === selectedId ? 'mt-click mt-sel' : 'mt-click', tabindex: '0' },
          h('td', null, r.time),
          h('td.mt-mono', null, r.id),
          h('td', null, r.agent),
          h('td.num', null, r.total),
          h('td', null, statusBadge(r.error ? 'crit' : 'info', r.kept_text)));
        row.addEventListener('click', open);
        row.addEventListener('keydown', (e) => {
          if (e.key === 'Enter') open();
        });
        return row;
      })
    : [h('tr', null, h('td', { colspan: '5', class: 'muted' }, 'No kept traces match.'))];
  host.append(table(['Time', 'Trace', 'Agent', ['Total', 'num'], 'Kept'], trs));
  if (t.selected) host.append(traceDetail(t.selected));
}

/** The events feed: a kind filter over the rows, then a table. Subjects with a scope open that scope. */
export function events(host: HTMLElement, kinds: string[], rows: EventRow[], ctx: PanelCtx): void {
  const body = h('div');
  const draw = (kind: string): void => {
    body.replaceChildren();
    const shown = rows.filter((r) => kind === 'all' || r.kind === kind);
    if (!shown.length) {
      body.append(h('div.mt-empty', null, 'No events of this kind in range.'));
      return;
    }
    const trs = shown.map((r) =>
      h('tr', null,
        h('td.mt-mono', null, r.time),
        h('td', null, statusBadge(r.status, r.kind)),
        h('td', null, subject(r.subject, r.scope, ctx)),
        h('td.muted', null, r.detail)));
    body.append(table(['Time', 'Kind', 'Subject', 'Detail'], trs));
  };
  host.append(segmented(['all', ...kinds], 'all', draw, 'Event kind'), body);
  draw('all');
}

/** The selected trace: its title, its sub line, the waterfall, the legend and the selected span's details. */
function traceDetail(d: TraceDetail): HTMLElement {
  const total = Math.max(d.total_ms, 1, ...d.spans.map((s) => s.start_ms + s.dur_ms));
  const start = d.spans.length ? defaultSpan(d.spans) : -1;
  let details: HTMLElement = h('dl.mt-dl');
  const wfRows = d.spans.map((s, i) => waterfallRow(s, total, () => select(i)));
  const select = (i: number): void => {
    const span = d.spans[i];
    if (!span) return;
    wfRows.forEach((row, k) => {
      row.classList.toggle('mt-sel', k === i);
      row.setAttribute('aria-pressed', String(k === i));
    });
    const next = spanDetails(span);
    details.replaceWith(next);
    details = next;
  };
  if (start >= 0) select(start);
  return h('div.mt-td', null,
    h('div.mt-ttl', null, d.title),
    h('div.mt-sub', null, d.sub),
    wfRows,
    details,
    legend(d.legend));
}

/** One span: its indented name, its bar on the trace's time axis and its duration. A button that selects the span. */
function waterfallRow(s: SpanRow, total: number, onSelect: () => void): HTMLElement {
  const bar = h('i');
  bar.style.left = `${((s.start_ms / total) * 100).toFixed(2)}%`;
  bar.style.width = `${Math.max(0.6, (s.dur_ms / total) * 100).toFixed(2)}%`;
  bar.style.background = toneVar(s.error ? 'crit' : s.tone);
  const row = h('button.mt-span.mt-wf', { type: 'button', 'aria-pressed': 'false' },
    h('span.mt-sn', { class: s.error ? 'mt-err' : null, title: s.name }, s.name),
    h('span.mt-track.mt-sp', null, bar),
    h('span.num', null, format('ms', s.dur_ms)));
  row.style.paddingLeft = `${8 + s.depth * 16}px`;
  row.addEventListener('click', onSelect);
  return row;
}

/** The span's attributes as a definition list, with its name, start and duration first. */
function spanDetails(s: SpanRow): HTMLElement {
  const entries: [string, string][] = [
    ['Span', s.name],
    ['Start', `+${format('ms', s.start_ms)}`],
    ['Duration', format('ms', s.dur_ms)],
    ...s.attrs,
  ];
  return h('dl.mt-dl', null, entries.flatMap(([k, v]) => [
    h('dt', null, k),
    h('dd', { class: k === 'outcome' && v !== 'ok' ? 'mt-err' : null }, v),
  ]));
}

/** The first error span, or the fourth when nothing failed. */
function defaultSpan(spans: SpanRow[]): number {
  const err = spans.findIndex((s) => s.error);
  return err >= 0 ? err : Math.min(3, spans.length - 1);
}

/** The colour key of a trace or a chart: a swatch and its label. */
function legend(items: LegendItem[]): HTMLElement {
  return h('div.mt-legend', null, items.map((item) => {
    const swatch = h('i.mt-sw');
    swatch.style.background = toneVar(item.tone);
    return h('span.mt-leg', null, swatch, item.label);
  }));
}

/** A segmented control: one button per key, the current one pressed, and `pick` called with the key clicked. */
function segmented(keys: string[], current: string, pick: (key: string) => void, label: string): HTMLElement {
  const seg = h('div.seg', { role: 'group', 'aria-label': label });
  for (const k of keys) {
    const b = h('button', { type: 'button', 'aria-pressed': String(k === current) }, k);
    b.addEventListener('click', () => {
      seg.querySelectorAll('button').forEach((other) => other.setAttribute('aria-pressed', String(other === b)));
      pick(k);
    });
    seg.append(b);
  }
  return seg;
}

/** A link-like button that opens the subject's scope, or plain text when there is no scope. */
function subject(text: string, scope: string | undefined, ctx: PanelCtx): HTMLElement {
  if (!scope) return h('span', null, text);
  return button(text, () => ctx.go({ scope }), { kind: 'ghost', size: 'sm' });
}

type Col = string | [string, string];

/** The prototype's table: a wrapped `tbl`, with the numeric heads (given as `[label, 'num']`) right-aligned. */
function table(cols: Col[], rows: Node[]): HTMLElement {
  return h('div.tbl-wrap', null,
    h('table.tbl', null,
      h('thead', null, h('tr', null, cols.map((c) => (typeof c === 'string' ? h('th', null, c) : h('th', { class: c[1] }, c[0]))))),
      h('tbody', null, rows)));
}
