// The treasury (`#/treasury`): the plaza budget, where it sits and where it went,
// the agents' wallets and the journal. The owner mints and fines here; the rest is
// read. Balances and flows chart over a range (or table the same series), the money
// flow runs in proportion to the range's flows, wallets sort, and the journal pages
// through every entry and takes new ones live.

import '../styles/ops.css';
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
import { countTo, easeOut, niceScale, stackedArea, tweenArray, type AreaData } from '../ui/chart';
import { ago, badge, button, card, empty, field, input, modal, select, tabs, toast, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

export type Range = '7d' | '30d' | '90d' | 'all';
export type FlowKey = 'minted' | 'escrowed' | 'paid' | 'refunded' | 'fined' | 'spent';
export type Flows = Record<FlowKey | 'entries', number>;
export interface Sort { key: string; dir: 1 | -1 }
export interface BarLayer { key: FlowKey; label: string; color: string; values: number[] }
export interface BarData { labels: string[]; layers: BarLayer[] }
type View = 'chart' | 'table';

const DAY = 86_400_000;
const PAGE = 50;
const DURATION = 400;
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
const KINDS = ['mint', 'budget_move', 'payment', 'refund', 'fine', 'fee', 'clawback', 'spend', 'subcontract', 'transfer'];
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
export function balanceData(points: TreasuryPoint[], labelOf: (at: number) => string): AreaData {
  return {
    labels: points.map((p) => labelOf(p.at_ms)),
    layers: [
      { key: 'plaza', label: 'Plaza budget', color: 'var(--chart-1)', values: points.map((p) => p.plaza_budget_end) },
      { key: 'circulating', label: 'In circulation', color: 'var(--chart-2)', values: points.map((p) => p.circulating_end) },
      { key: 'treasury', label: 'Treasury', color: 'var(--chart-3)', values: points.map((p) => p.treasury_end) },
    ],
  };
}

/** The flows per bucket, one layer per kind. */
export function barData(points: TreasuryPoint[], labelOf: (at: number) => string): BarData {
  return {
    labels: points.map((p) => labelOf(p.at_ms)),
    layers: FLOWS.map((f) => ({ key: f.key, label: f.label, color: f.color, values: points.map((p) => p[f.key]) })),
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
const zeros = (n: number): number[] => new Array<number>(n).fill(0);
const fit = (v: number[], n: number): number[] => Array.from({ length: n }, (_, i) => v[i] ?? 0);

function bucketLabel(at: number, bucketMs: number): string {
  const d = new Date(at);
  return bucketMs < DAY
    ? d.toLocaleString('en-GB', { weekday: 'short', hour: '2-digit', minute: '2-digit' })
    : d.toLocaleDateString('en-GB', { day: 'numeric', month: 'short' });
}

const reduced = (): boolean =>
  typeof window !== 'undefined' && typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches;

/** Calls step with eased progress each frame until done. Returns a cancel function. */
function tween(step: (k: number) => void): () => void {
  if (reduced()) {
    step(1);
    return () => {};
  }
  let id = 0;
  const t0 = performance.now();
  const tick = (now: number): void => {
    const t = Math.min(1, (now - t0) / DURATION);
    step(easeOut(t));
    if (t < 1) id = requestAnimationFrame(tick);
  };
  id = requestAnimationFrame(tick);
  return () => cancelAnimationFrame(id);
}

const tipOf = (head: string, rows: [string, string][]): HTMLElement =>
  h('div.tr-tip', null, h('b', null, head), ...rows.map(([k, v]) => h('div.tr-tip-row', null, h('span', null, k), h('span.tr-tip-v', null, v))));

/** A sortable table. Clicking a header sorts by it; `foot` is an optional totals row aligned to the columns. */
interface Col<T> { key: string; label: string; num?: boolean; value: (r: T) => number | string; cell: (r: T) => Node | string }
function sortable<T>(cols: Col<T>[], rows: T[], sort: Sort, onSort: (s: Sort) => void, foot?: string[]): HTMLElement {
  const col = cols.find((c) => c.key === sort.key) ?? cols[0];
  const sorted = col ? sortRows(rows, (r) => col.value(r), sort.dir) : rows;
  const heads = cols.map((c) => {
    const active = c.key === sort.key;
    const th = h('th', { 'aria-sort': active ? (sort.dir > 0 ? 'ascending' : 'descending') : 'none' });
    const b = button(`${c.label}${active ? (sort.dir > 0 ? ' ↑' : ' ↓') : ''}`, () =>
      onSort({ key: c.key, dir: active ? (sort.dir === 1 ? -1 : 1) : c.num ? -1 : 1 }),
    );
    b.classList.add('tr-sort');
    th.append(b);
    th.classList.toggle('num', !!c.num);
    return th;
  });
  const body = sorted.map((r) => h('tr', null, ...cols.map((c) => h('td', { class: c.num ? 'num' : null }, c.cell(r)))));
  const tfoot = foot ? [h('tfoot', null, h('tr', null, ...foot.map((v, i) => h('td', { class: cols[i]?.num ? 'num' : null }, v))))] : [];
  return h('table.tr-table', null, h('thead', null, h('tr', null, ...heads)), h('tbody', null, ...body), ...tfoot);
}

function shareBar(v: number): HTMLElement {
  const fill = h('i');
  fill.style.width = `${(v * 100).toFixed(1)}%`;
  return h('span.tr-share-wrap', null, h('span.tr-share', null, fill), h('span.tr-pct', null, `${(v * 100).toFixed(1)}%`));
}

function journalRow(e: JournalEntry, names: ReadonlyMap<number, string>, enter: boolean): HTMLElement {
  const { from, to } = movesOf(e, names);
  const task = e.task_id ?? e.root_task;
  const row = h(
    'div.tr-row',
    { 'data-key': `j-${e.id}` },
    h('span.tr-at', { title: new Date(e.created_at).toLocaleString('en-GB') }, ago(e.created_at)),
    badge(e.kind, KIND_TONE[e.kind] ?? 'muted'),
    h('span.tr-amt', null, money(e.amount)),
    h('span.tr-move', null, `${from || '—'} → ${to || '—'}`),
    task ? h('a.tr-task', { href: href('tasks', task) }, task) : h('span.tr-task'),
    h('span.tr-memo', null, e.memo || e.kind),
  );
  if (enter) row.classList.add('tr-enter');
  return row;
}

// ---- Stacked bars (local: chart.ts has no stacked bars) ---------------------

interface BarsOptions { format: (v: number) => string; tooltip: (i: number) => Node; height: number; ariaLabel: string }
interface BarsHandle { update(d: BarData, hidden: ReadonlySet<string>): void; destroy(): void }

function stackedBars(el: HTMLElement, opts: BarsOptions): BarsHandle {
  const PL = 48;
  const PR = 12;
  const PT = 12;
  const PB = 26;
  const root = h('div.tr-bars');
  const tip = h('div.chart-tip');
  Object.assign(tip.style, { position: 'absolute', display: 'none', top: '8px', left: '0px', pointerEvents: 'none' });
  el.append(root, tip);

  let data: BarData = { labels: [], layers: [] };
  let shown: number[][] = [];
  let shownMax = 0;
  let hover = -1;
  let stop = (): void => {};

  function draw(): void {
    const W = root.clientWidth || 640;
    const H = opts.height;
    const n = data.labels.length;
    const py1 = H - PB;
    const ph = py1 - PT;
    const pw = W - PL - PR;
    const ticks = niceScale(shownMax);
    const top = ticks[ticks.length - 1] ?? 1;
    const slot = n ? pw / n : pw;
    const bw = Math.max(1, Math.min(30, slot * 0.68));
    const y = (v: number): number => py1 - (v / top) * ph;
    const every = Math.max(1, Math.ceil(n / Math.max(1, Math.floor(pw / 64))));
    const parts: string[] = [];
    for (const v of ticks) {
      parts.push(`<line class="${v === 0 ? 'tr-axis' : 'tr-grid'}" x1="${PL}" x2="${W - PR}" y1="${y(v)}" y2="${y(v)}"/>`);
      parts.push(`<text class="tr-tick" x="${PL - 8}" y="${y(v) + 4}" text-anchor="end">${opts.format(v)}</text>`);
    }
    if (hover >= 0 && hover < n) parts.push(`<rect class="tr-hover" x="${PL + slot * hover}" y="${PT}" width="${slot}" height="${ph}"/>`);
    for (let i = 0; i < n; i++) {
      const cx = PL + slot * (i + 0.5);
      let base = 0;
      data.layers.forEach((l, j) => {
        const v = shown[j]?.[i] ?? 0;
        if (v <= 0) return;
        parts.push(`<rect style="fill:${l.color}" x="${cx - bw / 2}" y="${y(base + v)}" width="${bw}" height="${(v / top) * ph}" rx="1.5"/>`);
        base += v;
      });
      if (i % every === 0) parts.push(`<text class="tr-tick" x="${cx}" y="${H - 8}" text-anchor="middle">${data.labels[i] ?? ''}</text>`);
    }
    root.innerHTML = `<svg viewBox="0 0 ${W} ${H}" width="100%" height="${H}" role="img" aria-label="${opts.ariaLabel}">${parts.join('')}</svg>`;
  }

  function onLeave(): void {
    hover = -1;
    draw();
    tip.style.display = 'none';
  }

  root.addEventListener('pointermove', (e) => {
    const n = data.labels.length;
    const W = root.clientWidth || 640;
    const r = root.getBoundingClientRect();
    const x = ((e.clientX - r.left) / (r.width || W)) * W;
    if (!n || x < PL || x > W - PR) {
      onLeave();
      return;
    }
    hover = Math.min(n - 1, Math.max(0, Math.floor((x - PL) / ((W - PL - PR) / n))));
    draw();
    tip.replaceChildren(opts.tooltip(hover));
    tip.style.display = 'block';
    const boxW = el.clientWidth || W;
    const tw = tip.offsetWidth;
    const cx = e.clientX - el.getBoundingClientRect().left;
    let left = cx + 12;
    if (left + tw > boxW) left = cx - tw - 12;
    tip.style.left = `${Math.max(0, Math.min(left, boxW - tw))}px`;
  });
  root.addEventListener('pointerleave', onLeave);

  const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(() => draw());
  ro?.observe(root);

  return {
    update(d: BarData, hidden: ReadonlySet<string>): void {
      stop();
      data = d;
      const n = d.labels.length;
      const to = d.layers.map((l) => (hidden.has(l.key) ? zeros(n) : fit(l.values, n)));
      const toMax = Math.max(0, ...Array.from({ length: n }, (_, i) => to.reduce((s, row) => s + (row[i] ?? 0), 0)));
      const from = shown.map((v) => fit(v, n));
      const fromMax = shownMax;
      stop = tween((k) => {
        shown = to.map((row, j) => tweenArray(from[j] ?? [], row, k));
        shownMax = fromMax + (toMax - fromMax) * k;
        draw();
      });
    },
    destroy(): void {
      stop();
      ro?.disconnect();
      root.remove();
      tip.remove();
    },
  };
}

// ---- The view ---------------------------------------------------------------

interface Tile { el: HTMLElement; fig: HTMLElement; sub: HTMLElement }
interface WalletRow { cid: number; name: string; role: string; balance: number; earned: number | null; spent: number | null; share: number }

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let gen = 0;
  let range: Range = '30d';
  let view: View = 'chart';
  const hidden = new Set<string>();
  let balSort: Sort = { key: 'at', dir: -1 };
  let flowSort: Sort = { key: 'at', dir: -1 };
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
  let fresh = new Set<number>();
  let rosterKey = '';

  const bucketOf = (): number => series?.bucket_ms ?? DAY;
  const labelOf = (at: number): string => bucketLabel(at, bucketOf());
  const pointAt = (i: number): TreasuryPoint | undefined => series?.points[i];

  // ---- controls ----
  const rangeHost = h('div.tr-seg');
  const viewHost = h('div.tr-seg');
  const fineBtn = button('Fine an agent', () => openFine());
  const mintBtn = button('Mint budget', () => openMint(), { kind: 'primary' });
  const errBox = h('div');

  // ---- tiles ----
  const tile = (label: string): Tile => {
    const fig = h('span.tr-figure');
    const sub = h('span.tr-sub');
    return { el: h('div.tr-tile', null, h('span.tr-label', null, label), fig, sub), fig, sub };
  };
  const tiles = {
    plaza: tile('Plaza budget'),
    circulating: tile('In circulation'),
    paid: tile('Paid out'),
    cost: tile('Real model cost'),
    treasury: tile('Treasury'),
  };
  const tilesEl = h('div.grid.tr-tallies', null, ...Object.values(tiles).map((x) => x.el));

  // ---- panes and cards ----
  const balanceHost = h('div.tr-chart');
  const balanceChart = stackedArea(balanceHost, { labels: [], layers: [] }, {
    format: money,
    height: 240,
    ariaLabel: 'Balances over time',
    tooltip: (i) => {
      const p = pointAt(i);
      if (!p) return h('span');
      return tipOf(labelOf(p.at_ms), [
        ['Plaza budget', money(p.plaza_budget_end)],
        ['In circulation', money(p.circulating_end)],
        ['Treasury', money(p.treasury_end)],
      ]);
    },
  });
  const balanceChartPane = h('div.tr-pane', null, balanceHost);
  const balanceTable = h('div.tr-tablewrap');
  const balanceTablePane = h('div.tr-pane', null, balanceTable);

  const flowHost = h('div.tr-chart');
  const flowChart = stackedBars(flowHost, {
    format: money,
    height: 240,
    ariaLabel: 'Flows per bucket',
    tooltip: (i) => {
      const p = pointAt(i);
      if (!p) return h('span');
      return tipOf(labelOf(p.at_ms), FLOWS.filter((f) => !hidden.has(f.key)).map((f) => [f.label, money(p[f.key])]));
    },
  });
  const flowChartPane = h('div.tr-pane', null, flowHost);
  const flowTable = h('div.tr-tablewrap');
  const flowTablePane = h('div.tr-pane', null, flowTable);
  const legendKeys = FLOWS.map((f) => {
    const sw = h('i.tr-swatch');
    sw.style.background = f.color;
    const b = h(
      'button.tr-key',
      {
        type: 'button',
        'aria-pressed': 'true',
        onclick: () => {
          if (hidden.has(f.key)) hidden.delete(f.key);
          else hidden.add(f.key);
          renderFlows();
        },
      },
      sw,
      f.label,
    );
    return { key: f.key, el: b };
  });
  const legend = h('div.tr-legend', null, ...legendKeys.map((x) => x.el));

  const diagram = h('div.tr-flow-wrap');
  const walletsHost = h('div');

  const journalList = h('div.tr-ledger');
  const countEl = h('span.tr-count');
  const olderBtn = button('Load older', () => void loadOlder());
  const journalFoot = h('div.tr-foot', null, countEl, olderBtn);

  const kindSel = select([{ value: '', label: 'All kinds' }, ...KINDS.map((k) => ({ value: k, label: k }))], '');
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

  const panes: [HTMLElement, HTMLElement][] = [
    [balanceChartPane, balanceTablePane],
    [flowChartPane, flowTablePane],
  ];
  const balanceCard = card('Balances over time', [balanceChartPane, balanceTablePane]);
  const flowCard = card('Flows per bucket', [legend, flowChartPane, flowTablePane]);
  const diagramCard = card('Where the money goes', diagram);
  const walletsCard = card('Wallets', walletsHost, { flush: true });
  const journalCard = card('Journal', [journalList, journalFoot], { flush: true, actions: [kindSel, agentSel, taskIn] });
  const body = h('div.stack', null, tilesEl, balanceCard, flowCard, diagramCard, walletsCard, journalCard);

  root.append(
    h('div.view-head', null, h('h1', null, 'Treasury'), h('div.actions', null, fineBtn, mintBtn)),
    h('div.tr-bar', null, rangeHost, viewHost),
    errBox,
    body,
  );

  // ---- modals (unchanged behaviour) ----
  const openMint = (): void => {
    const amount = input({ type: 'number', min: 1, step: 1, required: true });
    let close = (): void => {};
    const go = button('Mint', () => {
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
    });
    go.classList.add('primary');
    close = modal('Mint budget', h('div.stack', null, field('Amount', amount, 'Whole units added to the plaza budget.'), h('div.row', null, go)));
  };

  const openFine = (): void => {
    if (roster.length === 0) {
      toast('There are no agents to fine.', 'error');
      return;
    }
    const who = select(roster.map((a) => ({ value: String(a.cid), label: a.name })));
    const amount = input({ type: 'number', min: 1, step: 1, required: true });
    const memo = input({ type: 'text', required: true, placeholder: 'What for' });
    let close = (): void => {};
    const go = button('Fine', () => {
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
    });
    go.classList.add('danger');
    close = modal(
      'Fine an agent',
      h(
        'div.stack',
        null,
        field('Agent', who),
        field('Amount', amount, 'Taken from the agent’s wallet.'),
        field('Memo', memo, 'Shown in the journal.'),
        h('div.row', null, go),
      ),
    );
  };

  // ---- renderers ----
  const renderControls = (): void => {
    replace(
      rangeHost,
      tabs(RANGES, range, (k) => {
        range = k as Range;
        gen++;
        renderControls();
        void refresh();
      }),
    );
    replace(
      viewHost,
      tabs(VIEWS, view, (k) => {
        view = k as View;
        renderControls();
        applyView();
      }),
    );
  };

  const applyView = (): void => {
    for (const [chart, table] of panes) {
      chart.hidden = view !== 'chart';
      table.hidden = view === 'chart';
    }
  };

  const renderTiles = (): void => {
    if (!t) return;
    const escrow = t.accounts.filter((a) => a.kind === 'task_escrow').reduce((s, a) => s + a.balance, 0);
    const walletSum = wallets.reduce((s, w) => s + w.balance, 0);
    const f = flowsOf(series?.points ?? []);
    const last = series?.points.at(-1);
    countTo(tiles.plaza.fig, t.plaza_budget, money);
    tiles.plaza.sub.textContent = `≈ ${usd(creditsUsd(t.plaza_budget))} at $0.001 a credit`;
    countTo(tiles.circulating.fig, escrow + walletSum, money);
    tiles.circulating.sub.textContent = `${money(escrow)} in escrow · ${money(walletSum)} in wallets`;
    countTo(tiles.paid.fig, f.paid, money);
    tiles.paid.sub.textContent = `≈ ${usd(creditsUsd(f.paid))} · ${money(f.spent)} spent`;
    if (usage) {
      countTo(tiles.cost.fig, usage.totals.usd, usd);
      tiles.cost.sub.replaceChildren(`≈ ${money(creditsOf(usage.totals.usd))} credits metered · `, h('a', { href: href('usage') }, 'see usage'));
    } else {
      tiles.cost.fig.textContent = '—';
      tiles.cost.sub.textContent = 'usage unavailable';
    }
    countTo(tiles.treasury.fig, last?.treasury_end ?? 0, money);
    tiles.treasury.sub.textContent = `${money(f.fined)} fined in range`;
  };

  const renderBalances = (): void => {
    const s = series;
    if (!s) return;
    balanceChart.update(balanceData(s.points, (at) => bucketLabel(at, s.bucket_ms)));
  };

  const renderBalanceTable = (): void => {
    const s = series;
    if (!s) return;
    const cols: Col<TreasuryPoint>[] = [
      { key: 'at', label: 'Bucket', value: (p) => p.at_ms, cell: (p) => bucketLabel(p.at_ms, s.bucket_ms) },
      { key: 'plaza', label: 'Plaza budget', num: true, value: (p) => p.plaza_budget_end, cell: (p) => money(p.plaza_budget_end) },
      { key: 'circulating', label: 'In circulation', num: true, value: (p) => p.circulating_end, cell: (p) => money(p.circulating_end) },
      { key: 'treasury', label: 'Treasury', num: true, value: (p) => p.treasury_end, cell: (p) => money(p.treasury_end) },
    ];
    replace(balanceTable, sortable(cols, s.points, balSort, (next) => {
      balSort = next;
      renderBalanceTable();
    }));
  };

  const renderFlows = (): void => {
    const s = series;
    if (s) flowChart.update(barData(s.points, (at) => bucketLabel(at, s.bucket_ms)), hidden);
    for (const k of legendKeys) k.el.setAttribute('aria-pressed', String(!hidden.has(k.key)));
  };

  const renderFlowTable = (): void => {
    const s = series;
    if (!s) return;
    const f = flowsOf(s.points);
    const cols: Col<TreasuryPoint>[] = [
      { key: 'at', label: 'Bucket', value: (p) => p.at_ms, cell: (p) => bucketLabel(p.at_ms, s.bucket_ms) },
      { key: 'entries', label: 'Entries', num: true, value: (p) => p.entries, cell: (p) => money(p.entries) },
      ...FLOWS.map((x) => ({ key: x.key, label: x.label, num: true, value: (p: TreasuryPoint) => p[x.key], cell: (p: TreasuryPoint) => money(p[x.key]) })),
    ];
    const foot = ['Total', money(f.entries), ...FLOWS.map((x) => money(f[x.key]))];
    replace(flowTable, sortable(cols, s.points, flowSort, (next) => {
      flowSort = next;
      renderFlowTable();
    }, foot));
  };

  const renderDiagram = (): void => {
    const s = series;
    const tt = t;
    if (!s || !tt) return;
    const f = flowsOf(s.points);
    const escrow = tt.accounts.filter((a) => a.kind === 'task_escrow').reduce((x, a) => x + a.balance, 0);
    const walletSum = wallets.reduce((x, w) => x + w.balance, 0);
    const boxes: [number, number, string, number][] = [
      [0, 55, 'Budget · now', tt.plaza_budget],
      [220, 55, 'Escrow · now', escrow],
      [440, 10, 'Spend · range', f.spent],
      [440, 100, 'Payment · range', f.paid],
      [660, 55, 'Wallets · now', walletSum],
    ];
    // Arrows: forward and refund in the budget-escrow gap, the rest out of escrow and back into wallets.
    const arrows: [number, number, number, number, number][] = [
      [170, 78, 220, 78, f.escrowed],
      [220, 102, 170, 102, f.refunded],
      [390, 70, 440, 45, f.spent],
      [390, 110, 440, 135, f.paid],
      [610, 135, 660, 100, f.paid],
    ];
    const max = Math.max(1, ...arrows.map((a) => a[4]));
    const parts: string[] = [
      `<defs><marker id="tr-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path class="tr-head" d="M0 0L10 5L0 10z"/></marker></defs>`,
    ];
    for (const [x, y, label, value] of boxes) {
      parts.push(
        `<rect class="tr-box" x="${x}" y="${y}" width="170" height="70"/>`,
        `<text class="tr-label" x="${x + 12}" y="${y + 22}">${label}</text>`,
        `<text class="tr-fig" x="${x + 12}" y="${y + 54}">${money(value)}</text>`,
      );
    }
    for (const [x1, y1, x2, y2, v] of arrows) {
      const share = v / max;
      parts.push(`<line class="tr-line" style="stroke-width:${(1 + 3 * share).toFixed(2)}px" x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}" marker-end="url(#tr-arrow)"/>`);
      if (v > 0) {
        parts.push(`<line class="tr-run" style="--tr-dur:${(4 - 3 * share).toFixed(2)}s;stroke-width:${(1.5 + 2.5 * share).toFixed(2)}px" x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}"/>`);
        parts.push(`<text class="tr-flowv" x="${(x1 + x2) / 2}" y="${(y1 + y2) / 2 - 6}" text-anchor="middle">${money(v)}</text>`);
      }
    }
    diagram.innerHTML = `<svg class="tr-flow" viewBox="0 0 830 180" role="img" aria-label="Where the plaza budget goes">${parts.join('')}</svg>`;
  };

  const renderWallets = (): void => {
    if (wallets.length === 0) {
      replace(walletsHost, empty('user', 'No agent wallets yet.'));
      return;
    }
    const total = wallets.reduce((s, w) => s + Math.max(0, w.balance), 0);
    const rows: WalletRow[] = wallets.map((w) => {
      const p = profiles.get(w.cid);
      return {
        cid: w.cid,
        name: w.name,
        role: roles.get(w.cid) ?? '',
        balance: w.balance,
        earned: p ? p.earned : null,
        spent: p ? p.spent : null,
        share: shareOf(w.balance, total),
      };
    });
    const cols: Col<WalletRow>[] = [
      { key: 'name', label: 'Agent', value: (r) => r.name, cell: (r) => h('a', { href: href('agents', r.cid) }, r.name) },
      { key: 'role', label: 'Role', value: (r) => r.role, cell: (r) => r.role || '—' },
      { key: 'balance', label: 'Balance', num: true, value: (r) => r.balance, cell: (r) => money(r.balance) },
      { key: 'earned', label: 'Earned', num: true, value: (r) => r.earned ?? 0, cell: (r) => (r.earned === null ? '—' : money(r.earned)) },
      { key: 'spent', label: 'Spent', num: true, value: (r) => r.spent ?? 0, cell: (r) => (r.spent === null ? '—' : money(r.spent)) },
      { key: 'share', label: 'Share', num: true, value: (r) => r.share, cell: (r) => shareBar(r.share) },
    ];
    const sum = (f: (r: WalletRow) => number): number => rows.reduce((s, r) => s + f(r), 0);
    const foot = ['Total', '', money(sum((r) => r.balance)), money(sum((r) => r.earned ?? 0)), money(sum((r) => r.spent ?? 0)), ''];
    replace(walletsHost, sortable(cols, rows, wallSort, (next) => {
      wallSort = next;
      renderWallets();
    }, foot));
  };

  const renderFilters = (): void => {
    const key = roster.map((a) => `${a.cid}:${a.name}`).join('|');
    if (key === rosterKey) return;
    rosterKey = key;
    agentSel.replaceChildren(new Option('All agents', '0'), ...roster.map((a) => new Option(a.name, String(a.cid))));
    agentSel.value = String(filters.wallet);
  };

  const renderJournal = (): void => {
    const filtered = filters.kind !== '' || filters.wallet !== 0 || filters.task !== '';
    replace(
      journalList,
      ...(entries.length
        ? entries.map((e) => journalRow(e, names, fresh.has(e.id)))
        : [empty('clock', filtered ? 'No matching entries.' : 'No entries.')]),
    );
    countEl.textContent = `${entries.length} of ${total} shown`;
    olderBtn.hidden = nextBefore === null;
  };

  const renderAll = (): void => {
    renderTiles();
    renderBalances();
    renderBalanceTable();
    renderFlows();
    renderFlowTable();
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
      const have = new Set(entries.map((e) => e.id));
      fresh = have.size ? new Set(page.entries.filter((e) => !have.has(e.id)).map((e) => e.id)) : new Set();
      entries = mergeJournal(entries, page.entries);
      total = page.total;
      if (!olderLoaded) nextBefore = page.next_before;
      replace(errBox);
      renderAll();
    } catch (err) {
      if (alive && mine === gen) replace(errBox, h('div.tr-error', null, `Treasury unreachable: ${msg(err)}`));
    }
  };

  const loadOlder = async (): Promise<void> => {
    if (nextBefore === null) return;
    const mine = gen;
    olderBtn.disabled = true;
    try {
      const page = await api.treasuryJournal(queryOf(nextBefore));
      if (!alive || mine !== gen) return;
      entries = mergeJournal(entries, page.entries);
      total = page.total;
      nextBefore = page.next_before;
      olderLoaded = true;
      fresh = new Set();
      renderJournal();
    } catch (err) {
      toast(msg(err), 'error');
    } finally {
      olderBtn.disabled = false;
    }
  };

  const resetJournal = (): void => {
    gen++;
    entries = [];
    total = 0;
    nextBefore = null;
    olderLoaded = false;
    fresh = new Set();
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
  applyView();
  void refresh();
  return () => {
    alive = false;
    off();
    if (timer !== undefined) clearTimeout(timer);
    balanceChart.destroy();
    flowChart.destroy();
  };
}
