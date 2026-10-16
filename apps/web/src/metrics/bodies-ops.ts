// The operational panel bodies: firing alerts (with mute, acknowledge, open), the rules table (on/off, new rule),
// kept traces (filter, list, waterfall, span details) and the events feed (kind filter).

import { button } from '../ui/components';
import { h } from '../ui/dom';
import { format } from './format';
import { spark } from './figures';
import type { PanelCtx } from './panels';
import { statusVar, toneVar, type AlertRow, type EventRow, type LegendItem, type RuleRow, type SpanRow, type TraceDetail, type TracesBody } from './model';

const TRACE_FILTERS = ['all', 'error', 'slow', 'sampled', 'open'];

/** The firing rules, each with its severity bar, its current value, a spark line and the Open, Mute and Acknowledge actions. */
export function alerts(host: HTMLElement, rows: AlertRow[], ctx: PanelCtx): void {
  if (!rows.length) {
    host.append(h('div.mx-empty', null, 'Nothing firing. Muted rules are listed below.'));
    return;
  }
  for (const r of rows) {
    const bar = h('i.mx-sev');
    bar.style.background = statusVar(r.severity);
    const text = h('div', null,
      h('b', null, r.name),
      h('div.mx-mute', null, `${r.scope_label} · ${r.cond} for ${r.for_text} · since ${r.since}`));
    const value = h('div.mx-val', null, h('b.num', null, r.now), ' ', h('span.mx-mute', null, 'now'));
    const trend = h('div.mx-spk', null, spark(r.spark, 'crit', 120, 24));
    const actions = h('div.mx-acts', null,
      button('Open', () => ctx.go({ scope: r.scope, tab: r.tab }), { size: 'sm' }),
      button('Mute 1 h', () => ctx.mute(r.rule), { size: 'sm' }),
      button(r.acked ? 'Acknowledged' : 'Acknowledge', () => ctx.ack(r.rule, r.scope), { size: 'sm', disabled: r.acked }));
    host.append(h('div.mx-alert-row', null, bar, text, value, trend, actions));
  }
}

/** The rules table, with a New rule button above it. Turning a rule on or off reports the new state. */
export function rules(host: HTMLElement, rows: RuleRow[], ctx: PanelCtx): void {
  host.append(button('New rule', () => ctx.newRule(), { kind: 'primary', size: 'sm' }));
  const body = rows.map((r) => {
    const on = h('input', { type: 'checkbox', checked: r.on, 'aria-label': `Enable ${r.name}` });
    on.addEventListener('change', () => ctx.toggleRule(r.id, on.checked));
    const severity = h('span.mx-pill', { class: `mx-${r.severity}` }, r.severity);
    const stateClass = r.state === 'firing' ? 'mx-crit' : r.state === 'ok' ? 'mx-ok' : 'mx-mute';
    return h('tr', null,
      h('td', null, on),
      h('td', null, r.name),
      h('td', null, r.scope_label),
      h('td', null, h('span.mono', null, r.cond), ` for ${r.for_text}`),
      h('td', null, severity),
      h('td', { class: stateClass }, r.state),
      h('td', null, r.last || '—'));
  });
  host.append(makeTable(['On', 'Rule', 'Scope', 'Condition', 'Severity', 'State', 'Last fired'], body));
}

/** The kept traces: a filter, a list of turns (left) and the selected turn's waterfall and span details (right). */
export function traces(host: HTMLElement, t: TracesBody, ctx: PanelCtx): void {
  const filter = segmented(TRACE_FILTERS, t.filter, (k) => ctx.filter(k));
  if (!t.list.length) {
    host.append(filter, h('div.mx-empty', null, 'No kept traces match.'));
    return;
  }
  const selectedId = t.selected?.id;
  const listRows = t.list.map((r) => {
    const row = h('tr', { class: r.id === selectedId ? 'mx-sel' : null, tabindex: '0' },
      h('td.mono', null, r.time),
      h('td', null, r.agent),
      h('td.num', null, r.total),
      h('td', { class: r.error ? 'mx-crit' : 'mx-mute' }, r.kept_text));
    const open = () => ctx.go({ tab: 'traces', trace: r.id });
    row.addEventListener('click', open);
    row.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') open();
    });
    return row;
  });
  const list = h('div.mx-traces-list', null, makeTable(['Time', 'Agent', 'Total', 'Kept because'], listRows));
  host.append(filter, h('div.mx-traces', null, list, t.selected ? traceDetail(t.selected) : null));
}

/** The events feed: a kind filter over the rows, then a table. Subjects with a scope open that scope. */
export function events(host: HTMLElement, kinds: string[], rows: EventRow[], ctx: PanelCtx): void {
  const body = h('div');
  const draw = (kind: string): void => {
    body.replaceChildren();
    const shown = rows.filter((r) => kind === 'all' || r.kind === kind);
    if (!shown.length) {
      body.append(h('div.mx-empty', null, 'No events of this kind in range.'));
      return;
    }
    const trs = shown.map((r) => h('tr', null,
      h('td.mono', null, r.time),
      h('td', null, h('span.mx-pill', { class: `mx-${r.status}` }, r.kind)),
      h('td', null, subject(r.subject, r.scope, ctx)),
      h('td', null, r.detail)));
    body.append(makeTable(['Time', 'Kind', 'Subject', 'Detail'], trs));
  };
  host.append(segmented(['all', ...kinds], 'all', draw), body);
  draw('all');
}

/** The selected trace: its title, the error note, the waterfall, the legend and the clicked span's details. */
function traceDetail(d: TraceDetail): HTMLElement {
  const total = Math.max(d.total_ms, 1, ...d.spans.map((s) => s.start_ms + s.dur_ms));
  const start = d.spans.length ? defaultSpan(d.spans) : -1;
  let details: HTMLElement = h('dl.mx-kv');
  const wfRows = d.spans.map((s, i) => waterfallRow(s, total, () => select(i)));
  const select = (i: number): void => {
    const span = d.spans[i];
    if (!span) return;
    wfRows.forEach((row, k) => row.classList.toggle('mx-sel', k === i));
    const next = spanDetails(span);
    details.replaceWith(next);
    details = next;
  };
  if (start >= 0) select(start);
  return h('div.mx-trace-detail', null,
    h('h3', null, d.title),
    h('div.mx-mute', null, d.sub),
    d.error ? h('p.mx-crit', null, 'Ended with an error.') : null,
    h('div.mx-wf', null, wfRows),
    h('div.mx-legend', null, d.legend.map(legendItem)),
    details);
}

/** One span: its indented name, the track (waiting for the first token, then the rest) and its duration. */
function waterfallRow(s: SpanRow, total: number, onSelect: () => void): HTMLElement {
  const track = h('div.mx-wf-track');
  const ft = s.first_token_ms ?? 0;
  if (ft > 0) track.append(bar((s.start_ms / total) * 100, (ft / total) * 100, toneVar('chart-1'), '.35'));
  const body = Math.max(0.4, ((s.dur_ms - ft) / total) * 100);
  track.append(bar(((s.start_ms + ft) / total) * 100, body, toneVar(s.error ? 'crit' : s.tone)));
  const name = h('div.mx-wf-name', { title: s.name }, s.name);
  name.style.paddingLeft = `${s.depth * 12}px`;
  const row = h('div.mx-wf-row', { tabindex: '0' }, name, track, h('div.mx-wf-dur', null, format('ms', s.dur_ms)));
  row.addEventListener('click', onSelect);
  row.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') onSelect();
  });
  return row;
}

/** The span's attributes as a definition list, with its name, start and duration first. */
function spanDetails(s: SpanRow): HTMLElement {
  const entries: [string, string][] = [
    ['span', s.name],
    ['start', `+${format('ms', s.start_ms)}`],
    ['duration', format('ms', s.dur_ms)],
    ...s.attrs,
  ];
  return h('dl.mx-kv', null, entries.flatMap(([k, v]) => [
    h('dt', null, k),
    h('dd', { class: k === 'outcome' && v !== 'ok' ? 'mx-crit' : null }, v),
  ]));
}

/** The first error span, or the fourth when nothing failed. */
function defaultSpan(spans: SpanRow[]): number {
  const err = spans.findIndex((s) => s.error);
  return err >= 0 ? err : Math.min(3, spans.length - 1);
}

function bar(left: number, width: number, color: string, opacity?: string): HTMLElement {
  const i = h('i');
  i.style.left = `${left}%`;
  i.style.width = `${width}%`;
  i.style.background = color;
  if (opacity) i.style.opacity = opacity;
  return i;
}

function legendItem(item: LegendItem): HTMLElement {
  const swatch = h('i');
  swatch.style.background = toneVar(item.tone);
  return h('span', null, swatch, item.label);
}

/** A segmented control: one button per key, the current one pressed, and `pick` called with the key clicked. */
function segmented(keys: string[], current: string, pick: (key: string) => void): HTMLElement {
  const seg = h('div.mx-seg');
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

function makeTable(heads: string[], rows: HTMLElement[]): HTMLElement {
  return h('div.mx-tablewrap', null,
    h('table.mx-t', null,
      h('thead', null, h('tr', null, heads.map((label) => h('th', null, label)))),
      h('tbody', null, rows)));
}
