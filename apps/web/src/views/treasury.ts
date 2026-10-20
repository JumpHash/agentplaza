// The treasury (`#/treasury`), as the Loom console draws it (the tr-* rules in styles/proto/social.css): the plaza
// budget, where it sits and where it went, the agents' wallets and the journal. The owner mints and fines here;
// the rest is read. Balances and flows chart over a range (or table the same series), the money flow runs in
// proportion to the range's flows, wallets sort, and the journal pages through every entry and takes new ones live.

import '../styles/proto/social.css';
import {
  api,
  type Agent,
  type AgentProfile,
  type JournalEntry,
  type JournalQuery,
  type Posting,
  type Treasury,
  type TreasuryPoint,
  type TreasurySeries,
  type UsageSummary,
  type Wallet,
} from '../api';
import { href } from '../router';
import { on } from '../store';
import { badge, button, card, field, input, modal, select, tabs, toast, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

export type Range = '7d' | '30d' | '90d' | 'all';
export type FlowKey = 'minted' | 'escrowed' | 'paid' | 'refunded' | 'fined' | 'spent';
export type Flows = Record<FlowKey | 'entries', number>;
export interface Sort { key: string; dir: 1 | -1 }
export interface BarLayer { key: string; label: string; color: string; values: number[] }
/** One stacked layer: its value per bucket. */
export interface LayerSpec { key: string; label: string; color: string; value: (p: TreasuryPoint) => number }
export interface BarData { labels: string[]; layers: BarLayer[] }
type View = 'chart' | 'table';

const DAY = 86_400_000;
const PAGE = 50;
/** One credit is a thousandth of a dollar: the plaza's metering unit. */
export const CREDIT_USD = 0.001;

const RANGES: { key: Range; label: string }[] = [
  { key: '7d', label: '7 d' },
  { key: '30d', label: '30 d' },
  { key: '90d', label: '90 d' },
  { key: 'all', label: 'All' },
];
const VIEWS: { key: View; label: string }[] = [
  { key: 'chart', label: 'Chart' },
  { key: 'table', label: 'Table' },
];
export const FLOWS: { key: FlowKey; label: string; color: string }[] = [
  { key: 'minted', label: 'Minted', color: 'var(--chart-1)' },
  { key: 'escrowed', label: 'Escrowed', color: 'var(--chart-2)' },
  { key: 'paid', label: 'Paid', color: 'var(--chart-3)' },
  { key: 'refunded', label: 'Refunded', color: 'var(--chart-4)' },
  { key: 'fined', label: 'Fined', color: 'var(--chart-5)' },
  { key: 'spent', label: 'Spent', color: 'var(--chart-6)' },
];
const KINDS = ['mint', 'budget_move', 'payment', 'refund', 'fine', 'fee', 'clawback', 'spend', 'subcontract', 'transfer', 'model'];
const KIND_TONE: Record<string, Tone> = {
  mint: 'ok',
  spend: 'warn',
  payment: 'accent',
  refund: 'info',
  transfer: 'info',
  fine: 'danger',
  clawback: 'danger',
  fee: 'warn',
  subcontract: 'accent',
  budget_move: 'muted',
  model: 'warn',
};
/** The journal's kinds in human words: the filter, the badges and the flows legend. */
const KIND_LABEL: Record<string, string> = {
  mint: 'Mint',
  budget_move: 'Budget move',
  payment: 'Payment',
  refund: 'Refund',
  fine: 'Fine',
  fee: 'Fee',
  clawback: 'Clawback',
  spend: 'Spend',
  subcontract: 'Subcontract',
  transfer: 'Transfer',
  model: 'Model call',
};
const KIND_COLOR: Record<string, string> = {
  mint: 'var(--chart-1)',
  payment: 'var(--chart-2)',
  fee: 'var(--chart-3)',
  refund: 'var(--chart-4)',
  clawback: 'var(--chart-5)',
  spend: 'var(--chart-6)',
  budget_move: 'var(--chart-7)',
  subcontract: 'var(--chart-8)',
  fine: 'var(--crit)',
  transfer: 'var(--ink-2)',
  model: 'var(--ink-3)',
};

// ---- Pure helpers (tested) -------------------------------------------------

export const money = (n: number): string => n.toLocaleString('en-US', { maximumFractionDigits: 0 });
export const usd = (v: number): string =>
  v === 0 ? '$0.00' : Math.abs(v) < 0.01 ? `$${v.toFixed(4)}` : `$${v.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
/** Credits as their dollar equivalent at the metering rate. */
export const creditsUsd = (c: number): number => c * CREDIT_USD;
/** Credits metered for a dollar amount of real model spend. */
export const creditsOf = (dollars: number): number => Math.round(dollars / CREDIT_USD);

/** The window ending now and its bucket. 7 d is hourly; the rest are daily. `all` starts at the first account. */
export function windowOf(range: Range, now: number, firstAt: number): { from: number; to: number; bucket: 'hour' | 'day' } {
  if (range === '7d') return { from: now - 7 * DAY, to: now, bucket: 'hour' };
  const span = range === 'all' ? Math.ceil((now - firstAt) / DAY) : range === '30d' ? 30 : 90;
  return { from: now - Math.min(1999, Math.max(1, span)) * DAY, to: now, bucket: 'day' };
}

/** Flow totals over a set of buckets. */
export function flowsOf(points: TreasuryPoint[]): Flows {
  const out: Flows = { entries: 0, minted: 0, escrowed: 0, paid: 0, refunded: 0, fined: 0, spent: 0 };
  for (const p of points) {
    out.entries += p.entries;
    out.minted += p.minted;
    out.escrowed += p.escrowed;
    out.paid += p.paid;
    out.refunded += p.refunded;
    out.fined += p.fined;
    out.spent += p.spent;
  }
  return out;
}

/** The balances over time: plaza budget, circulating, treasury (stacked). */
export function balanceData(points: TreasuryPoint[], labelOf: (at: number) => string): BarData {
  return {
    labels: points.map((p) => labelOf(p.at_ms)),
    layers: [
      { key: 'plaza', label: 'Plaza budget', color: 'var(--chart-1)', values: points.map((p) => p.plaza_budget_end) },
      { key: 'circulating', label: 'In circulation', color: 'var(--chart-2)', values: points.map((p) => p.circulating_end) },
      { key: 'treasury', label: 'Treasury', color: 'var(--chart-3)', values: points.map((p) => p.treasury_end) },
    ],
  };
}

/** The six flow totals as layers (the default), or any layers the caller names. */
const FLOW_LAYERS: LayerSpec[] = FLOWS.map((f) => ({ key: f.key, label: f.label, color: f.color, value: (p) => p[f.key] }));
/** One layer per journal kind, read from `by_kind`: every kind the plaza moves, not just the six totals. */
export const kindLayers = (): LayerSpec[] =>
  KINDS.map((k) => ({ key: k, label: KIND_LABEL[k] ?? k, color: KIND_COLOR[k] ?? 'var(--ink-3)', value: (p) => p.by_kind[k] ?? 0 }));
/** The kinds that moved in the range, in journal order. Kinds at zero in every bucket are left out. */
export const activeKinds = (points: TreasuryPoint[]): string[] =>
  KINDS.filter((k) => points.some((p) => (p.by_kind[k] ?? 0) > 0));

/** The flows per bucket, one layer per layer spec (the six flow totals unless told otherwise). */
export function barData(points: TreasuryPoint[], labelOf: (at: number) => string, layers: LayerSpec[] = FLOW_LAYERS): BarData {
  return {
    labels: points.map((p) => labelOf(p.at_ms)),
    layers: layers.map((l) => ({ key: l.key, label: l.label, color: l.color, values: points.map((p) => l.value(p)) })),
  };
}

/** Newest first by id, one entry per id. `page` holds the fresher copy when both have an id. */
export function mergeJournal(loaded: JournalEntry[], page: JournalEntry[]): JournalEntry[] {
  const seen = new Map<number, JournalEntry>();
  for (const e of [...page, ...loaded]) if (!seen.has(e.id)) seen.set(e.id, e);
  return [...seen.values()].sort((a, b) => b.id - a.id);
}

export function sortRows<T>(rows: T[], key: (r: T) => number | string, dir: 1 | -1): T[] {
  return rows.slice().sort((a, b) => {
    const x = key(a);
    const y = key(b);
    const c = typeof x === 'number' && typeof y === 'number' ? x - y : String(x).localeCompare(String(y));
    return c * dir;
  });
}

/** Who a posting touches: an agent's name for its cid, else the account's role in words. */
export function partyOf(p: Posting, names: ReadonlyMap<number, string>): string {
  const cid = Number(p.owner_ref);
  if (p.owner_ref !== '' && Number.isInteger(cid) && names.has(cid)) return names.get(cid) ?? '';
  switch (p.kind) {
    case 'town_budget':
      return 'Plaza budget';
    case 'external_spend':
      return 'Providers';
    case 'mint':
      return 'Mint';
    case 'task_escrow':
      return `Escrow ${p.owner_ref}`;
    default:
      return p.owner_ref ? `${p.kind} ${p.owner_ref}` : p.kind;
  }
}

/** Where an entry's money came from and went: debits (negative legs) to credits (positive legs). */
export function movesOf(e: JournalEntry, names: ReadonlyMap<number, string>): { from: string; to: string } {
  const side = (sign: 1 | -1): string => e.postings.filter((p) => Math.sign(p.amount) === sign).map((p) => partyOf(p, names)).join(', ');
  return { from: side(-1), to: side(1) };
}

export const shareOf = (balance: number, total: number): number => (total > 0 ? Math.max(0, balance) / total : 0);

// ---- Local helpers ----------------------------------------------------------

const msg = (err: unknown): string => (err instanceof Error ? err.message : String(err));
const esc = (s: string): string => s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
const escrowOf = (x: Treasury): number => x.accounts.filter((a) => a.kind === 'task_escrow').reduce((s, a) => s + a.balance, 0);
const walletTotal = (ws: Wallet[]): number => ws.reduce((s, w) => s + w.balance, 0);

/** A bucket's label, as the prototype writes it: the hour for a sub-day bucket, else the day. */
function bucketLabel(at: number, bucketMs: number): string {
  const opts: Intl.DateTimeFormatOptions = bucketMs < DAY ? { month: 'short', day: 'numeric', hour: 'numeric' } : { month: 'short', day: 'numeric' };
  return new Date(at).toLocaleString('en-US', opts);
}

/** The time since `ms` in the prototype's words: "just now", "4 min ago", "3 h ago", "2 d ago". */
function agoOf(ms: number, now = Date.now()): string {
  if (!ms) return '—';
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return 'just now';
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}

/** Markup from a chart's string, as one element. Every text in it is escaped. */
function svgOf(markup: string): Element {
  const t = document.createElement('template');
  t.innerHTML = markup;
  return t.content.firstElementChild as Element;
}

/** A tile's note: its parts joined by a middle dot. */
function noteParts(el: HTMLElement, parts: (string | Node)[]): void {
  replace(el, ...parts.flatMap((p, i) => (i ? [' · ', p] : [p])));
}

/** Rows per page of a series table, as the desktop's. */
const SERIES_PAGE = 25;

/** Paging for a sortable table: the page shown (0-based), the rows per page, the noun for the count, and the Prev/Next handler. */
interface Paging { page: number; size: number; unit: string; onPage: (page: number) => void }

/**
 * A sortable column. `num` right-aligns the header and sorts it high-first on the first click; `cellNum` aligns its
 * cells (default: as `num`).
 */
interface Col<T> { key: string; label: string; num?: boolean; cellNum?: boolean; value: (r: T) => number | string; cell: (r: T) => Node | string }

/**
 * A sortable table, as the prototype draws it. Clicking a header sorts by it; `foot` is an optional totals row
 * aligned to the columns; `paging` pages the sorted rows under it (the pager follows the table).
 */
function sortable<T>(cols: Col<T>[], rows: T[], sort: Sort, onSort: (s: Sort) => void, foot?: string[], paging?: Paging): Node[] {
  const col = cols.find((c) => c.key === sort.key) ?? cols[0];
  const sorted = col ? sortRows(rows, (r) => col.value(r), sort.dir) : rows;
  let shown = sorted;
  const pager: Node[] = [];
  if (paging) {
    const pages = Math.max(1, Math.ceil(sorted.length / paging.size));
    const page = Math.min(Math.max(0, paging.page), pages - 1);
    shown = sorted.slice(page * paging.size, (page + 1) * paging.size);
    pager.push(
      h(
        'div.tr-pager',
        null,
        h('span', null, `${sorted.length} ${paging.unit} · page ${page + 1} of ${pages}`),
        h(
          'span.cluster',
          null,
          button('Prev', () => paging.onPage(page - 1), { kind: 'ghost', size: 'sm', disabled: page === 0 }),
          button('Next', () => paging.onPage(page + 1), { kind: 'ghost', size: 'sm', disabled: page + 1 >= pages }),
        ),
      ),
    );
  }
  const heads = cols.map((c) => {
    const active = c.key === sort.key;
    const dir: 1 | -1 = active ? (sort.dir === 1 ? -1 : 1) : c.num ? -1 : 1;
    const label = `${c.label}${active ? (sort.dir < 0 ? ' ↓' : ' ↑') : ''}`;
    return h('th', { class: c.num ? 'num' : null }, button(label, () => onSort({ key: c.key, dir }), { kind: 'ghost', size: 'sm' }));
  });
  const numCell = (c: Col<T> | undefined): string | null => ((c?.cellNum ?? c?.num) ? 'num' : null);
  const body = shown.map((r) => h('tr', null, ...cols.map((c) => h('td', { class: numCell(c) }, c.cell(r)))));
  const tfoot = foot
    ? [h('tfoot', null, h('tr', null, ...foot.map((v, i) => (i === 0 ? h('th', null, v) : h('td', { class: numCell(cols[i]) }, v)))))]
    : [];
  const table = h('table.tbl.tr-table', null, h('thead', null, h('tr', null, ...heads)), h('tbody', null, ...body), ...tfoot);
  return [h('div.tbl-wrap', null, table), ...pager];
}

function shareBar(v: number): HTMLElement {
  const fill = h('i');
  fill.style.width = `${(v * 100).toFixed(1)}%`;
  return h('span', null, h('span.tr-share', null, fill), ' ', h('span.num', null, `${(v * 100).toFixed(1)}%`));
}

function journalRow(e: JournalEntry, names: ReadonlyMap<number, string>): HTMLElement {
  const { from, to } = movesOf(e, names);
  const task = e.task_id ?? e.root_task;
  return h(
    'div.tr-jrow',
    { 'data-key': `j-${e.id}` },
    h('span.muted', { title: new Date(e.created_at).toLocaleString('en-US') }, agoOf(e.created_at)),
    h('span', null, badge(KIND_LABEL[e.kind] ?? e.kind, KIND_TONE[e.kind] ?? 'muted')),
    h('span.num', null, money(e.amount)),
    h('span', null, `${from || '—'} → ${to || '—'}`, h('br'), h('span.hint', null, e.memo || e.kind)),
    h('span', null, task ? h('a', { href: href('tasks', task) }, task) : null),
  );
}

// ---- Charts (the prototype's inline SVGs) -----------------------------------

/** The chart frame, in the prototype's viewBox units. */
const CH = { W: 720, H: 260, x0: 56, x1: 712, y0: 12, y1: 220 };
const X = (i: number, n: number): number => (n <= 1 ? CH.x0 : CH.x0 + (i / (n - 1)) * (CH.x1 - CH.x0));

/** The top of a y-axis: the first of 1, 2, 2.5, 5, 10 (times a power of ten) at or above v. */
function niceMax(v: number): number {
  if (v <= 0) return 1;
  const p = 10 ** Math.floor(Math.log10(v));
  const m = v / p;
  return (m <= 1 ? 1 : m <= 2 ? 2 : m <= 2.5 ? 2.5 : m <= 5 ? 5 : 10) * p;
}

/** Five gridlines from zero to `top`, each with its figure. */
function ticksOf(top: number): string {
  return [0, 1, 2, 3, 4]
    .map((j) => {
      const y = CH.y1 - (j / 4) * (CH.y1 - CH.y0);
      return `<line class="tr-grid" x1="${CH.x0}" x2="${CH.x1}" y1="${y}" y2="${y}"/><text class="tr-txt" x="${CH.x0 - 8}" y="${y + 4}" text-anchor="end">${money((top * j) / 4)}</text>`;
    })
    .join('');
}

/** The first and last bucket's labels under the frame. */
function xLabelsOf(labels: string[]): string {
  if (!labels.length) return '';
  const first = esc(labels[0] ?? '');
  const last = esc(labels[labels.length - 1] ?? '');
  return `<text class="tr-txt" x="${CH.x0}" y="${CH.y1 + 18}">${first}</text><text class="tr-txt" x="${CH.x1}" y="${CH.y1 + 18}" text-anchor="end">${last}</text>`;
}

/** The balances as stacked areas on one scale, from the largest bucket total. */
function balanceSvg(s: TreasurySeries): Element {
  const { labels, layers } = balanceData(s.points, (at) => bucketLabel(at, s.bucket_ms));
  const n = s.points.length;
  const totals = s.points.map((_, i) => layers.reduce((sum, l) => sum + Math.max(0, l.values[i] ?? 0), 0));
  const top = niceMax(Math.max(1, ...totals));
  const Y = (v: number): number => CH.y1 - (v / top) * (CH.y1 - CH.y0);
  let base = s.points.map(() => 0);
  let areas = '';
  for (const l of layers) {
    const cur = base.map((b, i) => b + Math.max(0, l.values[i] ?? 0));
    const up = cur.map((v, i) => `${i ? 'L' : 'M'}${X(i, n).toFixed(1)},${Y(v).toFixed(1)}`).join('');
    const down = base.map((b, i) => `L${X(i, n).toFixed(1)},${Y(b).toFixed(1)}`).reverse().join('');
    areas += `<path d="${up}${down}Z" fill="${l.color}" fill-opacity=".6"><title>${esc(l.label)}</title></path>`;
    base = cur;
  }
  const bw = (CH.x1 - CH.x0) / Math.max(n - 1, 1);
  const hits = s.points
    .map((_, i) => {
      const lines = layers.map((l) => `\n${esc(l.label)}: ${money(l.values[i] ?? 0)}`).join('');
      return `<rect x="${(X(i, n) - bw / 2).toFixed(1)}" y="${CH.y0}" width="${bw.toFixed(1)}" height="${CH.y1 - CH.y0}" fill="transparent"><title>${esc(labels[i] ?? '')}${lines}</title></rect>`;
    })
    .join('');
  return svgOf(`<svg class="tr-chart" viewBox="0 0 ${CH.W} ${CH.H}" role="img" aria-label="Balances over time">${ticksOf(top)}${areas}${hits}${xLabelsOf(labels)}</svg>`);
}

/** The flows per bucket as stacked bars, one segment per shown kind. */
function flowSvg(s: TreasurySeries, shown: LayerSpec[]): Element {
  const data = barData(s.points, (at) => bucketLabel(at, s.bucket_ms), shown);
  const n = s.points.length;
  const bw = (CH.x1 - CH.x0) / Math.max(n, 1);
  const totals = s.points.map((_, i) => data.layers.reduce((sum, l) => sum + (l.values[i] ?? 0), 0));
  const top = niceMax(Math.max(1, ...totals));
  const Y = (v: number): number => CH.y1 - (v / top) * (CH.y1 - CH.y0);
  const bars = s.points
    .map((_, i) => {
      let acc = 0;
      let segs = '';
      for (const l of data.layers) {
        const v = l.values[i] ?? 0;
        if (!v) continue;
        segs += `<rect x="${(CH.x0 + i * bw + bw * 0.15).toFixed(1)}" y="${Y(acc + v).toFixed(1)}" width="${(bw * 0.7).toFixed(1)}" height="${Math.max(0.5, Y(acc) - Y(acc + v)).toFixed(1)}" fill="${l.color}"/>`;
        acc += v;
      }
      const lines = data.layers.map((l) => `\n${esc(l.label)}: ${money(l.values[i] ?? 0)}`).join('');
      return `<g><title>${esc(data.labels[i] ?? '')}${lines}</title>${segs}</g>`;
    })
    .join('');
  return svgOf(`<svg class="tr-chart" viewBox="0 0 ${CH.W} ${CH.H}" role="img" aria-label="Flows per bucket">${ticksOf(top)}${bars}${xLabelsOf(data.labels)}</svg>`);
}

/** Where the plaza budget goes: boxes for the stock and the range's flows, arrows sized by their value. */
function diagramSvg(budget: number, escrow: number, wallets: number, f: Flows): Element {
  const boxes: [string, number, number, number][] = [
    ['Budget · now', budget, 110, 130],
    ['Escrow · now', escrow, 360, 130],
    ['Spend · range', f.spent, 610, 50],
    ['Payment · range', f.paid, 610, 130],
    ['Wallets · now', wallets, 610, 210],
  ];
  const arrows: [number, string, number, number][] = [
    [f.escrowed, 'M185,130 L285,130', 235, 122],
    [f.refunded, 'M285,146 Q235,192 185,146', 235, 180],
    [f.spent, 'M435,112 C500,112 480,50 535,50', 485, 74],
    [f.paid, 'M435,130 L535,130', 485, 122],
    [f.paid, 'M610,158 L610,182', 630, 174],
  ];
  const max = Math.max(1, ...arrows.map((a) => a[0]));
  const paths = arrows
    .map(([v, d, lx, ly]) =>
      v
        ? `<path class="tr-arrow tr-flow" d="${d}" style="stroke-width:${(1 + (9 * v) / max).toFixed(1)};animation-duration:${(2.4 - (1.6 * v) / max).toFixed(2)}s"/><text class="tr-txt" x="${lx}" y="${ly}" text-anchor="middle">${money(v)}</text>`
        : `<path class="tr-arrow" d="${d}" style="stroke-width:1"/>`,
    )
    .join('');
  const bx = boxes
    .map(
      ([t, v, cx, cy]) =>
        `<rect class="tr-box" x="${cx - 75}" y="${cy - 28}" width="150" height="56" rx="10"/><text class="tr-txt" x="${cx}" y="${cy - 6}" text-anchor="middle">${esc(t)}</text><text class="tr-txt tr-val" x="${cx}" y="${cy + 14}" text-anchor="middle">${money(v)}</text>`,
    )
    .join('');
  return svgOf(`<svg class="tr-diag" viewBox="0 0 720 260" role="img" aria-label="Where the plaza budget goes">${paths}${bx}</svg>`);
}

// ---- The view ---------------------------------------------------------------

interface Tile { el: HTMLElement; fig: HTMLElement; note: HTMLElement }
interface WalletRow { cid: number; name: string; role: string; balance: number; earned: number | null; spent: number | null; share: number }

const tile = (label: string): Tile => {
  const fig = h('b.num');
  const note = h('span.hint');
  return { el: h('div.stat', null, h('span.muted', null, label), fig, note), fig, note };
};

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let gen = 0;
  let range: Range = '30d';
  let view: View = 'chart';
  const hidden = new Set<string>();
  let balSort: Sort = { key: 'at', dir: -1 };
  let flowSort: Sort = { key: 'at', dir: -1 };
  let balPage = 0;
  let flowPage = 0;
  let wallSort: Sort = { key: 'balance', dir: -1 };
  const filters = { kind: '', wallet: 0, task: '' };

  let t: Treasury | null = null;
  let series: TreasurySeries | null = null;
  let usage: UsageSummary | null = null;
  let roster: Agent[] = [];
  let names = new Map<number, string>();
  let roles = new Map<number, string>();
  let wallets: Wallet[] = [];
  let profiles = new Map<number, AgentProfile>();
  let entries: JournalEntry[] = [];
  let total = 0;
  let nextBefore: number | null = null;
  let olderLoaded = false;
  let rosterKey = '';
  let olderBtn = button('Load older', () => void loadOlder(), { kind: 'ghost', size: 'sm' });

  const bar = h('div.tr-bar');
  const fineBtn = button('Fine an agent', () => openFine());
  const mintBtn = button('Mint budget', () => openMint(), { kind: 'primary' });
  // Hidden until there is an error: an empty child of the page would still take a flex gap.
  const errBox = h('div');
  errBox.hidden = true;

  // ---- tiles ----
  const tiles = {
    plaza: tile('Plaza budget'),
    circulating: tile('In circulation'),
    paid: tile('Paid out'),
    cost: tile('Real model cost'),
    treasury: tile('Treasury'),
  };
  const tilesEl = h('div.tr-tiles', null, ...Object.values(tiles).map((x) => x.el));

  // ---- cards ----
  const balanceBody = h('div');
  const flowBody = h('div');
  const diagramBody = h('div');
  const walletsBody = h('div');
  const journalBody = h('div');

  const kindSel = select([{ value: '', label: 'All kinds' }, ...KINDS.map((k) => ({ value: k, label: KIND_LABEL[k] ?? k }))], '');
  kindSel.setAttribute('aria-label', 'Kind');
  kindSel.addEventListener('change', () => {
    filters.kind = kindSel.value;
    resetJournal();
  });
  const agentSel = select([{ value: '0', label: 'All agents' }], '0');
  agentSel.setAttribute('aria-label', 'Agent');
  agentSel.addEventListener('change', () => {
    filters.wallet = Number(agentSel.value);
    resetJournal();
  });
  const taskIn = input({ type: 'search', placeholder: 'Task id', 'aria-label': 'Task id' });
  taskIn.addEventListener('change', () => {
    filters.task = taskIn.value.trim();
    resetJournal();
  });

  const balanceCard = card('Balances over time', balanceBody);
  const flowCard = card('Flows per bucket', flowBody);
  const diagramCard = card('Where the money goes', diagramBody);
  const walletsCard = card('Wallets', walletsBody);
  const journalCard = card('Journal', journalBody, { actions: [h('div.tr-bar', null, kindSel, agentSel, taskIn)] });

  root.append(
    h('header.page-head', null, h('div.page-head-row', null, h('h1', null, 'Treasury'), h('span.sp'), h('div.page-actions', null, fineBtn, mintBtn))),
    bar,
    errBox,
    tilesEl,
    h('div.tr-cards', null, balanceCard, flowCard, diagramCard, walletsCard, journalCard),
  );

  // ---- modals ----
  const openMint = (): void => {
    const amount = input({ type: 'number', min: 1, step: 1, required: true, 'aria-label': 'Amount' });
    let close = (): void => {};
    const go = button(
      'Mint',
      () => {
        const n = Number(amount.value);
        if (!Number.isInteger(n) || n <= 0) {
          toast('Enter a whole amount above zero.', 'error');
          return;
        }
        go.disabled = true;
        api
          .mintBudget(n)
          .then(() => {
            toast(`Minted ${money(n)} into the plaza budget.`);
            close();
            void refresh();
          })
          .catch((err: unknown) => {
            go.disabled = false;
            toast(msg(err), 'error');
          });
      },
      { kind: 'primary' },
    );
    close = modal(
      'Mint budget',
      h('div', null, field('Amount', amount, 'Whole units added to the plaza budget.'), h('footer.sheet-f', null, button('Cancel', () => close(), { kind: 'ghost' }), go)),
    );
  };

  const openFine = (): void => {
    if (roster.length === 0) {
      toast('There are no agents to fine.', 'error');
      return;
    }
    const who = select([...roster].sort((a, b) => a.name.localeCompare(b.name)).map((a) => ({ value: String(a.cid), label: a.name })));
    who.setAttribute('aria-label', 'Agent');
    const amount = input({ type: 'number', min: 1, step: 1, required: true, 'aria-label': 'Amount' });
    const memo = input({ type: 'text', required: true, placeholder: 'What for', 'aria-label': 'Memo' });
    let close = (): void => {};
    const go = button(
      'Fine',
      () => {
        const n = Number(amount.value);
        const text = memo.value.trim();
        if (!Number.isInteger(n) || n <= 0) {
          toast('Enter a whole amount above zero.', 'error');
          return;
        }
        if (!text) {
          toast('A fine needs a memo.', 'error');
          return;
        }
        go.disabled = true;
        api
          .fineAgent(Number(who.value), n, text)
          .then(() => {
            toast(`Fined ${money(n)}.`);
            close();
            void refresh();
          })
          .catch((err: unknown) => {
            go.disabled = false;
            toast(msg(err), 'error');
          });
      },
      { kind: 'danger' },
    );
    close = modal(
      'Fine an agent',
      h(
        'div',
        null,
        field('Agent', who),
        field('Amount', amount, 'Taken from the agent’s wallet.'),
        field('Memo', memo, 'Shown in the journal.'),
        h('footer.sheet-f', null, button('Cancel', () => close(), { kind: 'ghost' }), go),
      ),
    );
  };

  // ---- renderers ----
  const renderControls = (): void => {
    replace(
      bar,
      tabs(RANGES, range, (k) => {
        range = k as Range;
        gen++;
        renderControls();
        void refresh();
      }),
      tabs(VIEWS, view, (k) => {
        view = k as View;
        renderControls();
        renderBalances();
        renderFlows();
      }),
    );
  };

  const renderTiles = (): void => {
    if (!t) return;
    const escrow = escrowOf(t);
    const walletSum = walletTotal(wallets);
    const f = flowsOf(series?.points ?? []);
    const last = series?.points.at(-1);
    tiles.plaza.fig.textContent = money(t.plaza_budget);
    noteParts(tiles.plaza.note, [`≈ ${usd(creditsUsd(t.plaza_budget))}`, '$0.001 a credit']);
    tiles.circulating.fig.textContent = money(escrow + walletSum);
    noteParts(tiles.circulating.note, [`${money(escrow)} in escrow`, `${money(walletSum)} in wallets`]);
    tiles.paid.fig.textContent = money(f.paid);
    noteParts(tiles.paid.note, [`≈ ${usd(creditsUsd(f.paid))}`, `${money(f.spent)} spent`]);
    if (usage) {
      tiles.cost.fig.textContent = usd(usage.totals.usd);
      noteParts(tiles.cost.note, [`≈ ${money(creditsOf(usage.totals.usd))} credits metered`, h('a', { href: '#/metrics?tab=tokens' }, 'see usage')]);
    } else {
      tiles.cost.fig.textContent = '—';
      noteParts(tiles.cost.note, ['usage unavailable']);
    }
    tiles.treasury.fig.textContent = money(last?.treasury_end ?? 0);
    noteParts(tiles.treasury.note, [`${money(f.fined)} fined in range`]);
  };

  const renderBalances = (): void => {
    const s = series;
    if (!s) return;
    if (view === 'chart') {
      replace(balanceBody, balanceSvg(s));
      return;
    }
    const cols: Col<TreasuryPoint>[] = [
      { key: 'at', label: 'Bucket', num: true, cellNum: false, value: (p) => p.at_ms, cell: (p) => bucketLabel(p.at_ms, s.bucket_ms) },
      { key: 'plaza', label: 'Plaza budget', num: true, value: (p) => p.plaza_budget_end, cell: (p) => money(p.plaza_budget_end) },
      { key: 'circulating', label: 'In circulation', num: true, value: (p) => p.circulating_end, cell: (p) => money(p.circulating_end) },
      { key: 'treasury', label: 'Treasury', num: true, value: (p) => p.treasury_end, cell: (p) => money(p.treasury_end) },
    ];
    replace(
      balanceBody,
      sortable(
        cols,
        s.points,
        balSort,
        (next) => {
          balSort = next;
          balPage = 0;
          renderBalances();
        },
        undefined,
        {
          page: balPage,
          size: SERIES_PAGE,
          unit: 'buckets',
          onPage: (p) => {
            balPage = Math.max(0, p);
            renderBalances();
          },
        },
      ),
    );
  };

  const renderFlows = (): void => {
    const s = series;
    if (!s) return;
    const active = activeKinds(s.points);
    const legend = h(
      'div.tr-leg',
      null,
      ...active.map((k) => {
        const sw = h('i.tr-sw');
        sw.style.background = KIND_COLOR[k] ?? 'var(--ink-3)';
        return h(
          'button.btn.ghost.sm',
          {
            type: 'button',
            'aria-pressed': String(!hidden.has(k)),
            onclick: () => {
              if (hidden.has(k)) hidden.delete(k);
              else hidden.add(k);
              renderFlows();
            },
          },
          sw,
          KIND_LABEL[k] ?? k,
        );
      }),
    );
    if (view === 'chart') {
      const shown = kindLayers().filter((l) => active.includes(l.key) && !hidden.has(l.key));
      replace(flowBody, legend, flowSvg(s, shown));
      return;
    }
    const cols: Col<TreasuryPoint>[] = [
      { key: 'at', label: 'Bucket', num: true, cellNum: false, value: (p) => p.at_ms, cell: (p) => bucketLabel(p.at_ms, s.bucket_ms) },
      { key: 'entries', label: 'Entries', num: true, value: (p) => p.entries, cell: (p) => money(p.entries) },
      ...active.map((k) => ({
        key: k,
        label: KIND_LABEL[k] ?? k,
        num: true,
        value: (p: TreasuryPoint) => p.by_kind[k] ?? 0,
        cell: (p: TreasuryPoint) => money(p.by_kind[k] ?? 0),
      })),
    ];
    const foot = ['Total', money(flowsOf(s.points).entries), ...active.map((k) => money(s.points.reduce((sum, p) => sum + (p.by_kind[k] ?? 0), 0)))];
    replace(
      flowBody,
      legend,
      sortable(
        cols,
        s.points,
        flowSort,
        (next) => {
          flowSort = next;
          flowPage = 0;
          renderFlows();
        },
        foot,
        {
          page: flowPage,
          size: SERIES_PAGE,
          unit: 'buckets',
          onPage: (p) => {
            flowPage = Math.max(0, p);
            renderFlows();
          },
        },
      ),
    );
  };

  const renderDiagram = (): void => {
    const s = series;
    const tt = t;
    if (!s || !tt) return;
    replace(diagramBody, diagramSvg(tt.plaza_budget, escrowOf(tt), walletTotal(wallets), flowsOf(s.points)));
  };

  const renderWallets = (): void => {
    if (wallets.length === 0) {
      replace(walletsBody, h('p.muted', null, 'No agent wallets yet.'));
      return;
    }
    const posTotal = wallets.reduce((s, w) => s + Math.max(0, w.balance), 0);
    const rows: WalletRow[] = wallets.map((w) => {
      const p = profiles.get(w.cid);
      return {
        cid: w.cid,
        name: w.name,
        role: roles.get(w.cid) ?? '',
        balance: w.balance,
        earned: p ? p.earned : null,
        spent: p ? p.spent : null,
        share: shareOf(w.balance, posTotal),
      };
    });
    const cols: Col<WalletRow>[] = [
      { key: 'name', label: 'Agent', value: (r) => r.name, cell: (r) => h('a', { href: href('agents', r.cid) }, r.name) },
      { key: 'role', label: 'Role', value: (r) => r.role, cell: (r) => r.role },
      { key: 'balance', label: 'Balance', num: true, value: (r) => r.balance, cell: (r) => money(r.balance) },
      { key: 'earned', label: 'Earned', num: true, value: (r) => r.earned ?? 0, cell: (r) => (r.earned === null ? '—' : money(r.earned)) },
      { key: 'spent', label: 'Spent', num: true, value: (r) => r.spent ?? 0, cell: (r) => (r.spent === null ? '—' : money(r.spent)) },
      { key: 'share', label: 'Share', num: true, cellNum: false, value: (r) => r.share, cell: (r) => shareBar(r.share) },
    ];
    const sum = (f: (r: WalletRow) => number): number => rows.reduce((s, r) => s + f(r), 0);
    const foot = ['Total', '', money(sum((r) => r.balance)), money(sum((r) => r.earned ?? 0)), money(sum((r) => r.spent ?? 0)), ''];
    replace(
      walletsBody,
      sortable(cols, rows, wallSort, (next) => {
        wallSort = next;
        renderWallets();
      }, foot),
    );
  };

  const renderFilters = (): void => {
    const sorted = [...roster].sort((a, b) => a.name.localeCompare(b.name));
    const key = sorted.map((a) => `${a.cid}:${a.name}`).join('|');
    if (key === rosterKey) return;
    rosterKey = key;
    agentSel.replaceChildren(new Option('All agents', '0'), ...sorted.map((a) => new Option(a.name, String(a.cid))));
    agentSel.value = String(filters.wallet);
  };

  const renderJournal = (): void => {
    const filtered = filters.kind !== '' || filters.wallet !== 0 || filters.task !== '';
    olderBtn = button('Load older', () => void loadOlder(), { kind: 'ghost', size: 'sm' });
    replace(
      journalBody,
      entries.length ? entries.map((e) => journalRow(e, names)) : h('p.muted', null, filtered ? 'No matching entries.' : 'No entries.'),
      h(
        'div.tr-pager',
        null,
        h('span', null, `${entries.length} of ${total} shown`),
        nextBefore !== null ? olderBtn : null,
      ),
    );
  };

  const renderAll = (): void => {
    renderTiles();
    renderBalances();
    renderFlows();
    renderDiagram();
    renderWallets();
    renderFilters();
    renderJournal();
  };

  // ---- loading ----
  const queryOf = (before?: number): JournalQuery => ({
    before,
    limit: PAGE,
    kind: filters.kind || undefined,
    wallet: filters.wallet || undefined,
    task: filters.task || undefined,
  });

  const refresh = async (): Promise<void> => {
    const mine = gen;
    try {
      const now = Date.now();
      const [overview, ws, ros, page] = await Promise.all([
        api.treasury(),
        api.treasuryWallets(),
        api.agents(),
        api.treasuryJournal(queryOf()),
      ]);
      const firstAt = overview.accounts.length ? Math.min(...overview.accounts.map((a) => a.created_at)) : now;
      const win = windowOf(range, now, firstAt);
      const [s, u, profs] = await Promise.all([
        api.treasurySeries(win.from, win.to, win.bucket),
        api.usageSummary(win.from, win.to).catch(() => null),
        Promise.all(ws.map((w) => api.profile(w.cid).catch(() => null))),
      ]);
      if (!alive || mine !== gen) return;
      t = overview;
      series = s;
      balPage = 0;
      flowPage = 0;
      usage = u;
      wallets = ws;
      roster = ros;
      names = new Map(ros.map((a) => [a.cid, a.name]));
      roles = new Map(ros.map((a) => [a.cid, a.role_label]));
      profiles = new Map();
      ws.forEach((w, i) => {
        const p = profs[i];
        if (p) profiles.set(w.cid, p);
      });
      entries = mergeJournal(entries, page.entries);
      total = page.total;
      if (!olderLoaded) nextBefore = page.next_before;
      replace(errBox);
      errBox.hidden = true;
      renderAll();
    } catch (err) {
      if (alive && mine === gen) {
        replace(errBox, h('div.tr-error', null, `Treasury unreachable: ${msg(err)}`));
        errBox.hidden = false;
      }
    }
  };

  const loadOlder = async (): Promise<void> => {
    if (nextBefore === null) return;
    const mine = gen;
    const btn = olderBtn;
    btn.disabled = true;
    try {
      const page = await api.treasuryJournal(queryOf(nextBefore));
      if (!alive || mine !== gen) return;
      entries = mergeJournal(entries, page.entries);
      total = page.total;
      nextBefore = page.next_before;
      olderLoaded = true;
      renderJournal();
    } catch (err) {
      toast(msg(err), 'error');
    } finally {
      btn.disabled = false;
    }
  };

  const resetJournal = (): void => {
    gen++;
    entries = [];
    total = 0;
    nextBefore = null;
    olderLoaded = false;
    renderJournal();
    void refresh();
  };

  const schedule = (): void => {
    if (timer !== undefined) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = undefined;
      void refresh();
    }, 400);
  };
  const off = on('treasury', schedule);

  renderControls();
  void refresh();
  return () => {
    alive = false;
    off();
    if (timer !== undefined) clearTimeout(timer);
  };
}
