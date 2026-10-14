// Usage: what the plaza's model calls consumed and what they cost (`#/usage`).
// The chart shows the window over time by group, the table ranks the groups,
// the call log pages through every call, and the prices panel shows the rates
// behind the estimates. `#/usage?fixture=1` renders generated data for design work.

import '../styles/usage.css';
import { api, isContributor, type PriceRates, type PricesView, type Settings, type UsageBucket, type UsageCall, type UsageCallsPage, type UsageGroup, type UsageKeyed, type UsageSeries, type UsageSummary, type UsageTotals } from '../api';
import { agentName, roleLabel } from '../roles';
import { href } from '../router';
import { agents, on } from '../store';
import { bars, countTo, sparkline, stackedArea, type AreaData, type BarItem, type ChartHandle, type ChartLayer } from '../ui/chart';
import { ago, button, card, compact, empty, select, stat, table, tabs, toast, trainsBadge, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

export type Range = '24h' | '7d' | '30d' | '90d';
export type Metric = 'usd' | 'tokens';
type View = 'chart' | 'table';
type SortKey = 'key' | 'calls' | 'input' | 'output' | 'cache_read' | 'cache_write' | 'cache_hit_rate' | 'est_usd' | 'cost_usd' | 'usd';

const HOUR = 3_600_000;
const DAY = 24 * HOUR;
const RANGE: Record<Range, { label: string; ms: number; bucket: UsageBucket }> = {
  '24h': { label: '24 h', ms: DAY, bucket: 'hour' },
  '7d': { label: '7 d', ms: 7 * DAY, bucket: 'hour' },
  '30d': { label: '30 d', ms: 30 * DAY, bucket: 'day' },
  '90d': { label: '90 d', ms: 90 * DAY, bucket: 'day' },
};
const RANGE_KEYS: Range[] = ['24h', '7d', '30d', '90d'];
const GROUPS: { key: UsageGroup; label: string }[] = [
  { key: 'model', label: 'Model' },
  { key: 'agent', label: 'Agent' },
  { key: 'role', label: 'Role' },
  { key: 'provider', label: 'Provider' },
];
const METRICS: { key: Metric; label: string }[] = [
  { key: 'usd', label: 'USD' },
  { key: 'tokens', label: 'Tokens' },
];
const VIEWS: { key: View; label: string }[] = [
  { key: 'chart', label: 'Chart' },
  { key: 'table', label: 'Table' },
];
const TOP = 6;
const PAGE = 50;
const OTHER = '~other';
const PALETTE = ['var(--chart-1)', 'var(--chart-2)', 'var(--chart-3)', 'var(--chart-4)', 'var(--chart-5)', 'var(--chart-6)', 'var(--chart-7)', 'var(--chart-8)'];

export const ZERO: UsageTotals = { calls: 0, input: 0, output: 0, cache_read: 0, cache_write: 0, cost_usd: 0, est_usd: 0, usd: 0, cache_hit_rate: 0 };

const msg = (err: unknown): string => (err instanceof Error ? err.message : String(err));
export const usd = (v: number): string =>
  v === 0 ? '$0.00' : v < 0.01 ? `$${v.toFixed(4)}` : `$${v.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
const pct = (v: number): string => `${v.toFixed(1)}%`;
const fmtOf = (m: Metric): ((v: number) => string) => (m === 'usd' ? usd : (v: number) => `${compact(v)} tok`);
export const colorOf = (i: number): string => (i < 0 ? 'var(--chart-other)' : (PALETTE[i % PALETTE.length] ?? 'var(--chart-other)'));
const metricOf = (t: UsageTotals, m: Metric): number => (m === 'usd' ? t.usd : t.input + t.output);
const rowsOf = (s: UsageSummary, g: UsageGroup): UsageKeyed[] => ({ model: s.by_model, agent: s.by_agent, role: s.by_role, provider: s.by_provider })[g];
const keyOfGroup = (g: UsageGroup, c: UsageCall): string => (g === 'model' ? c.model : g === 'provider' ? c.provider : g === 'role' ? c.role : String(c.cid));

// ---- Pure helpers (tested) -------------------------------------------------

/** The window ending now, and the bucket size its chart uses. */
export function windowOf(range: Range, now: number): { from: number; to: number; bucket: UsageBucket } {
  return { from: now - RANGE[range].ms, to: now, bucket: RANGE[range].bucket };
}

/** Totals over a set of calls. `usd` is the reported cost where known, else the estimate. */
export function totalsOf(list: UsageCall[]): UsageTotals {
  const t: UsageTotals = { ...ZERO };
  for (const c of list) {
    t.calls++;
    t.input += c.input;
    t.output += c.output;
    t.cache_read += c.cache_read;
    t.cache_write += c.cache_write;
    if (c.cost_usd !== null) t.cost_usd += c.cost_usd;
    t.est_usd += c.est_usd;
    t.usd += c.cost_usd ?? c.est_usd;
  }
  const seen = t.input + t.cache_read;
  t.cache_hit_rate = seen > 0 ? t.cache_read / seen : 0;
  return t;
}

function groupBy(list: UsageCall[], keyOf: (c: UsageCall) => string): Record<string, UsageTotals> {
  const groups = new Map<string, UsageCall[]>();
  for (const c of list) {
    const k = keyOf(c);
    const g = groups.get(k);
    if (g) g.push(c);
    else groups.set(k, [c]);
  }
  return Object.fromEntries([...groups].map(([k, v]) => [k, totalsOf(v)]));
}

/** The summary the API returns, computed from calls (used by the fixture). Groups sort by spend. */
export function ledgerSummary(calls: UsageCall[], from: number, to: number): UsageSummary {
  const keyed = (g: UsageGroup): UsageKeyed[] =>
    Object.entries(groupBy(calls, (c) => keyOfGroup(g, c)))
      .map(([key, totals]) => ({ key, totals }))
      .sort((a, b) => b.totals.usd - a.totals.usd);
  const unpriced = [...new Set(calls.filter((c) => c.cost_usd === null && c.est_usd === 0).map((c) => c.model))];
  return { from_ms: from, to_ms: to, totals: totalsOf(calls), by_model: keyed('model'), by_agent: keyed('agent'), by_role: keyed('role'), by_provider: keyed('provider'), unpriced };
}

/** Buckets of [from, to), empty ones included, each grouped by `by`. */
export function ledgerSeries(calls: UsageCall[], from: number, to: number, bucket: UsageBucket, by: UsageGroup): UsageSeries {
  const bucketMs = bucket === 'hour' ? HOUR : DAY;
  const n = Math.max(1, Math.ceil((to - from) / bucketMs));
  const slots: UsageCall[][] = Array.from({ length: n }, () => []);
  for (const c of calls) {
    const i = Math.floor((c.at_ms - from) / bucketMs);
    if (i >= 0 && i < n) slots[i]?.push(c);
  }
  const points = slots.map((list, i) => ({ at_ms: from + i * bucketMs, groups: groupBy(list, (c) => keyOfGroup(by, c)), totals: totalsOf(list) }));
  return { from_ms: from, to_ms: to, bucket_ms: bucketMs, by, points };
}

/** Groups of the series, largest first by the metric. Ranks are stable across hiding a group. */
export function rankGroups(series: UsageSeries, metric: Metric): string[] {
  const sums = new Map<string, number>();
  for (const p of series.points) for (const [k, t] of Object.entries(p.groups)) sums.set(k, (sums.get(k) ?? 0) + metricOf(t, metric));
  return [...sums.keys()].sort((a, b) => (sums.get(b) ?? 0) - (sums.get(a) ?? 0) || a.localeCompare(b));
}

const bucketLabel = (at: number, bucketMs: number): string => {
  const d = new Date(at);
  return bucketMs < DAY ? d.toLocaleString('en-GB', { weekday: 'short', hour: '2-digit', minute: '2-digit' }) : d.toLocaleDateString('en-GB', { day: 'numeric', month: 'short' });
};

/** The stacked-area data: the top groups, the rest folded into "Other", hidden groups left out. */
export function areaData(series: UsageSeries, ranked: string[], hidden: ReadonlySet<string>, metric: Metric, labelOf: (k: string) => string): AreaData {
  const visible = ranked.filter((k) => !hidden.has(k));
  const sumOf = (keys: string[]): number[] => series.points.map((p) => keys.reduce((s, k) => s + metricOf(p.groups[k] ?? ZERO, metric), 0));
  const layers: ChartLayer[] = visible.slice(0, TOP).map((k) => ({ key: k, label: labelOf(k), color: colorOf(ranked.indexOf(k)), values: sumOf([k]) }));
  const rest = visible.slice(TOP);
  if (rest.length) layers.push({ key: OTHER, label: 'Other', color: 'var(--chart-other)', values: sumOf(rest) });
  return { labels: series.points.map((p) => bucketLabel(p.at_ms, series.bucket_ms)), layers };
}

/** Dollars the cache reads saved against the full input price, over the models with known prices. */
export function cacheSaving(byModel: UsageKeyed[], prices: ReadonlyMap<string, PriceRates | null>): { usd: number; complete: boolean } {
  let saved = 0;
  let complete = true;
  for (const m of byModel) {
    if (m.totals.cache_read === 0) continue;
    const p = prices.get(m.key);
    if (!p || p.cache_read === null) {
      complete = false;
      continue;
    }
    saved += (m.totals.cache_read * Math.max(0, p.input - p.cache_read)) / 1e6;
  }
  return { usd: saved, complete };
}

/** Calls newest first, without repeats: a page merged into what is shown. Repeats are told apart by the server id; the inputs are not changed. */
export function mergeCalls(a: UsageCall[], b: UsageCall[]): UsageCall[] {
  const seen = new Set<number>();
  const out: UsageCall[] = [];
  for (const c of [...a, ...b]) {
    if (seen.has(c.id)) continue;
    seen.add(c.id);
    out.push(c);
  }
  return out.sort((x, y) => y.at_ms - x.at_ms || y.id - x.id);
}

export interface CallQuery { model?: string; cid?: number; before?: number; limit: number }
/** One page of calls over a full list, with the API's paging rule (`before` is an exclusive call-id bound). */
export function pageCalls(all: UsageCall[], q: CallQuery): UsageCallsPage {
  const matching = all.filter((c) => (!q.model || c.model === q.model) && (q.cid === undefined || c.cid === q.cid));
  const hits = matching.filter((c) => q.before === undefined || c.id < q.before);
  const calls = hits.slice(0, q.limit);
  const last = calls[calls.length - 1];
  return { calls, total: matching.length, next_before: hits.length > calls.length && last ? last.id : null };
}

// ---- Fixture (design preview only: `#/usage?fixture=1`) --------------------

const FIXTURE_MODELS = [
  { id: 'anthropic/claude-opus-5-5', provider: 'anthropic', input: 15, output: 75, cr: 1.5, cw: 18.75, weight: 2 },
  { id: 'anthropic/claude-haiku-5-5', provider: 'anthropic', input: 0.1, output: 0.5, cr: 0.01, cw: 0.125, weight: 5 },
  { id: 'openai/gpt-6-astra', provider: 'openai', input: 2.5, output: 10, cr: 1.25, cw: 0, weight: 2 },
  { id: 'deepseek/deepseek-flash', provider: 'deepseek', input: 0.14, output: 0.28, cr: 0.014, cw: 0, weight: 4 },
  { id: 'xai/grok-4.7', provider: 'xai', input: 3, output: 15, cr: 0.75, cw: 0, weight: 1 },
  { id: 'meta/muse-wonder-1.0', provider: 'meta', input: 4, output: 20, cr: 0.2, cw: 0, weight: 1 },
  { id: 'meta/muse-spark-1.3', provider: 'meta', input: 1.25, output: 4.25, cr: 0.15, cw: 0, weight: 2 },
  { id: 'meta/muse-spark-1.3-contributor', provider: 'meta', input: 0.1, output: 0.2, cr: 0.002, cw: 0, weight: 1 },
];
const FIXTURE_TRAINERS = new Set(['meta/muse-spark-1.3-contributor']);
const FIXTURE_AGENTS = [
  { cid: 101, name: 'Ada', role: 'orchestrator' },
  { cid: 102, name: 'Brin', role: 'builder' },
  { cid: 103, name: 'Cato', role: 'researcher' },
  { cid: 104, name: 'Dana', role: 'reviewer' },
  { cid: 105, name: 'Enzo', role: 'writer' },
];
const fixturePrice = (model: string): PriceRates | null => {
  const m = FIXTURE_MODELS.find((x) => x.id === model);
  return m ? { input: m.input, output: m.output, cache_read: m.cr, cache_write: m.cw, source: 'litellm' } : null;
};

/** Deterministic calls over the last `days` days, newest first. */
export function fixtureCalls(now: number, days: number): UsageCall[] {
  let seed = 7;
  const rand = (): number => {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    return seed / 0x7fffffff;
  };
  const pick = <T,>(xs: readonly T[], r: number): T => xs[Math.min(xs.length - 1, Math.floor(r * xs.length))] as T;
  const weighted = FIXTURE_MODELS.flatMap((m) => Array.from({ length: m.weight }, () => m));
  const out: UsageCall[] = [];
  for (let hr = 0; hr < days * 24; hr++) {
    const start = now - (days * 24 - hr) * HOUR;
    const count = Math.round(rand() * 5);
    for (let k = 0; k < count; k++) {
      const m = pick(weighted, rand());
      const a = pick(FIXTURE_AGENTS, rand());
      const input = Math.round(300 + rand() * 4000);
      const output = Math.round(50 + rand() * 900);
      const cacheRead = rand() < 0.7 ? Math.round(input * rand() * 2) : 0;
      const cacheWrite = rand() < 0.15 ? Math.round(rand() * 2000) : 0;
      const est = (input * m.input + output * m.output + cacheRead * m.cr + cacheWrite * m.cw) / 1e6;
      out.push({
        id: 0, at_ms: start + Math.floor(rand() * HOUR), cid: a.cid, role: a.role, model: m.id, provider: m.provider,
        input, output, cache_read: cacheRead, cache_write: cacheWrite,
        cost_usd: rand() < 0.4 ? est * (0.98 + 0.04 * rand()) : null, est_usd: est,
        task: rand() < 0.6 ? `t-${(hr * 7 + k) % 40 + 1}` : null, via: pick(['task', 'chat', 'message', 'motion'], rand()),
      });
    }
  }
  out.sort((x, y) => y.at_ms - x.at_ms);
  // Server ids rise with time, as the ledger assigns them: the newest call has the largest id.
  return out.map((c, i) => ({ ...c, id: out.length - i }));
}

// ---- The screen ------------------------------------------------------------

const SORT_COLS: { key: Exclude<SortKey, 'key' | 'usd'>; label: string; text: (t: UsageTotals) => string }[] = [
  { key: 'calls', label: 'Calls', text: (t) => t.calls.toLocaleString('en-US') },
  { key: 'input', label: 'Input', text: (t) => compact(t.input) },
  { key: 'output', label: 'Output', text: (t) => compact(t.output) },
  { key: 'cache_read', label: 'Cache read', text: (t) => compact(t.cache_read) },
  { key: 'cache_write', label: 'Cache write', text: (t) => compact(t.cache_write) },
  { key: 'cache_hit_rate', label: 'Hit rate', text: (t) => pct(t.cache_hit_rate * 100) },
  { key: 'est_usd', label: 'Estimated', text: (t) => usd(t.est_usd) },
  { key: 'cost_usd', label: 'Reported', text: (t) => usd(t.cost_usd) },
];

/** Display names for provider ids; an unknown id is shown as it is. */
const PROVIDER_LABEL: Record<string, string> = { openrouter: 'OpenRouter', opencode: 'OpenCode Zen' };
const SOURCE_LABEL: Record<string, string> = { ...PROVIDER_LABEL, litellm: 'LiteLLM', manual: 'manual' };
/** Splits `provider/model` on the first slash only: a model id can hold slashes (`openrouter/anthropic/x`). */
const splitKey = (key: string): { provider: string; model: string } => {
  const at = key.indexOf('/');
  return at < 0 ? { provider: '', model: key } : { provider: key.slice(0, at), model: key.slice(at + 1) };
};

const seg = <T extends string>(items: { key: T; label: string }[], current: T, onPick: (k: T) => void, label: string): HTMLElement => {
  const group = h('div.us-seg', { role: 'group', 'aria-label': label });
  for (const it of items) {
    const b = h('button', { type: 'button', 'aria-pressed': String(it.key === current) }, it.label);
    b.addEventListener('click', () => onPick(it.key));
    group.append(b);
  }
  return group;
};

const shareBar = (share: number): HTMLElement => {
  const bar = h('div.us-share');
  bar.innerHTML = `<svg viewBox="0 0 100 6" preserveAspectRatio="none" aria-hidden="true"><rect class="us-share-bg" width="100" height="6"/><rect class="us-share-fg" width="${Math.max(0, Math.min(100, share * 100)).toFixed(2)}" height="6"/></svg>`;
  return bar;
};

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  const fixture = location.hash.includes('fixture=1');
  let alive = true;
  let range: Range = '30d';
  let group: UsageGroup = 'model';
  let metric: Metric = 'usd';
  let view: View = 'chart';
  let sortKey: SortKey = 'usd';
  let sortDesc = true;
  const hidden = new Set<string>();
  let summary: UsageSummary | null = null;
  let series: UsageSeries | null = null;
  let loaded = false;
  let loadError: string | null = null;
  let seq = 0;
  let calls: UsageCall[] = [];
  let callsNext: number | null = null;
  let callsTotal = 0;
  let callModel = '';
  let callAgent = '';
  let busyMore = false;
  let settings: Settings | null = null;
  let pricesMeta: PricesView | null = null;
  let trainers = new Set<string>();
  const isTrainer = (model: string): boolean => (fixture ? FIXTURE_TRAINERS.has(model) : trainers.has(model.replace(/@.*$/, '')));
  const prices = new Map<string, PriceRates | null>();
  let plan: AreaData | null = null;
  let ranked: string[] = [];
  let fixtureAll: UsageCall[] | null = null;
  let areaHandle: ChartHandle<AreaData> | null = null;
  let barHandle: ChartHandle<BarItem[]> | null = null;
  let spark: ChartHandle<number[]> | null = null;
  let liveTimer: ReturnType<typeof setTimeout> | undefined;

  const knownAgents = (): { cid: number; name: string }[] =>
    fixture ? FIXTURE_AGENTS.map((a) => ({ cid: a.cid, name: a.name })) : agents.get().map((a) => ({ cid: a.cid, name: agentName(a) }));
  const nameOf = (cid: number): string => knownAgents().find((a) => a.cid === cid)?.name ?? `agent ${cid}`;
  const labelOf = (key: string): string => {
    if (group === 'agent') {
      const n = Number(key);
      return Number.isFinite(n) ? nameOf(n) : key;
    }
    if (group === 'role') return roleLabel(key);
    if (group === 'provider') return PROVIDER_LABEL[key] ?? key;
    return key;
  };
  const allCalls = (): UsageCall[] => (fixtureAll ??= fixtureCalls(Date.now(), 90));

  // Controls and the head
  const rangeSlot = h('div.us-slot');
  const groupSlot = h('div.us-slot');
  const metricSlot = h('div.us-slot');
  const viewSlot = h('div.us-slot');
  const priceLine = h('div.us-prices');

  // Stat tiles: figure first, the label, then an italic note.
  const spendTile = stat('Spend', '—', '—');
  const tokTile = stat('Tokens in / out', '—', '—');
  const hitTile = stat('Cache hit rate', '—', '—');
  const callTile = stat('Calls', '—', '—');
  const tokIn = h('span');
  const tokOut = h('span');
  tokTile.querySelector('.value')?.replaceChildren(tokIn, ' / ', tokOut);
  tokTile.querySelector('.value')?.classList.add('us-two');
  const sparkEl = h('div.us-spark');
  spendTile.append(sparkEl);
  const valueOf = (t: HTMLElement): HTMLElement => t.querySelector<HTMLElement>('.value') ?? t;
  const subOf = (t: HTMLElement): HTMLElement => t.querySelector<HTMLElement>('.sub') ?? t;
  const tiles = h('div.grid.cols-4.us-tiles', null, spendTile, tokTile, hitTile, callTile);

  // Over time: legend, stacked area, and the totals bars; the table is the other pane.
  const titleEl = h('h2', null, 'Spend by model');
  const legendEl = h('div.us-legend');
  const chartEl = h('div.us-chart');
  const chartEmpty = h('div.us-pane-empty');
  const barsEl = h('div.us-bars');
  const barsTitle = h('h4.us-sub', null, 'Totals in this window');
  const chartPane = h('div.us-pane', null, legendEl, chartEmpty, chartEl, barsTitle, barsEl);
  const tablePane = h('div.us-pane');
  const stage = h('div.us-stage', null, chartPane, tablePane);
  const chartCard = card(titleEl, stage, { actions: [metricSlot, viewSlot] });

  // The call log
  const callFilters = h('div.us-filters');
  const callBody = h('div.us-calls');
  const callFoot = h('div.us-foot');
  const callsCard = card('Calls', h('div.stack', null, callFilters, callBody, callFoot));

  // Prices in use
  const priceBody = h('div');
  const pricesCard = card('Prices in use', priceBody);

  root.append(
    h('div.view-head', null, h('h1', null, 'Usage'), h('div.actions', null, rangeSlot)),
    h('div.us-bar', null, groupSlot, priceLine),
    tiles,
    chartCard,
    callsCard,
    pricesCard,
  );

  // ---- Loading ----

  const fetchLedger = async (): Promise<{ summary: UsageSummary; series: UsageSeries }> => {
    const w = windowOf(range, Date.now());
    if (fixture) {
      const all = allCalls().filter((c) => c.at_ms >= w.from);
      return { summary: ledgerSummary(all, w.from, w.to), series: ledgerSeries(all, w.from, w.to, w.bucket, group) };
    }
    const [s, ts] = await Promise.all([api.usageSummary(w.from, w.to), api.usageSeries(w.from, w.to, w.bucket, group)]);
    return { summary: s, series: ts };
  };

  // A model's price; the `@low`-style effort suffix is tried off when the exact id has no price.
  const priceOf = async (model: string): Promise<PriceRates | null> => {
    if (fixture) return fixturePrice(model);
    for (const id of new Set([model, model.replace(/@.*$/, '')])) {
      try {
        return (await api.price(id)).price;
      } catch {
        /* no price under this id */
      }
    }
    return null;
  };
  const ensurePrices = async (models: string[]): Promise<void> => {
    const todo = [...new Set(models)].filter((m) => !prices.has(m));
    await Promise.all(todo.map(async (m) => prices.set(m, await priceOf(m))));
  };
  const settingsModels = (): string[] => {
    if (fixture) return FIXTURE_MODELS.map((m) => m.id);
    if (!settings) return [];
    return [settings.default_model, settings.orchestrator_model, ...Object.values(settings.role_models)].filter(Boolean);
  };
  // Where each model is used, listed as the desktop lists it: the default, the orchestrator, then each role.
  const settingsUses = (): Map<string, string[]> => {
    const uses = new Map<string, string[]>();
    if (fixture || !settings) return uses;
    const add = (model: string, label: string): void => {
      if (model) uses.set(model, [...(uses.get(model) ?? []), label]);
    };
    add(settings.default_model, 'default');
    add(settings.orchestrator_model, 'orchestrator');
    for (const [role, model] of Object.entries(settings.role_models)) add(model, `role ${role}`);
    return uses;
  };
  const loadPricesMeta = async (): Promise<void> => {
    if (fixture) {
      pricesMeta = { fetched_at_ms: Date.now() - 3 * HOUR, sources: [{ name: 'openrouter', url: '', models: 3400, ok: true }, { name: 'litellm', url: '', models: 3300, ok: true }], models: [] };
      return;
    }
    pricesMeta = await api.prices().catch(() => null);
  };

  const reload = async (): Promise<void> => {
    const my = ++seq;
    try {
      const got = await fetchLedger();
      await ensurePrices([...got.summary.by_model.map((m) => m.key), ...settingsModels()]);
      if (!alive || my !== seq) return;
      summary = got.summary;
      series = got.series;
      loadError = null;
    } catch (err) {
      if (!alive || my !== seq) return;
      loadError = msg(err);
    }
    loaded = true;
    render();
  };

  const callFilter = (): { model?: string; cid?: number } => ({ model: callModel || undefined, cid: callAgent ? Number(callAgent) : undefined });
  const fetchCalls = async (before?: number): Promise<UsageCallsPage> => {
    const f = callFilter();
    if (fixture) return pageCalls(allCalls(), { ...f, before, limit: PAGE });
    return api.usageCalls({ limit: PAGE, before, model: f.model, cid: f.cid });
  };
  const loadCalls = async (): Promise<void> => {
    try {
      const page = await fetchCalls();
      calls = page.calls;
      callsNext = page.next_before;
      callsTotal = page.total;
    } catch (err) {
      toast(msg(err), 'error');
    }
    renderCalls();
  };
  const loadOlder = async (): Promise<void> => {
    if (callsNext === null || busyMore) return;
    busyMore = true;
    renderCalls();
    try {
      const page = await fetchCalls(callsNext);
      calls = mergeCalls(calls, page.calls);
      callsNext = page.next_before;
      callsTotal = page.total;
    } catch (err) {
      toast(msg(err), 'error');
    }
    busyMore = false;
    renderCalls();
  };

  // Live: the event socket's usage frames (and work frames, which end every call) reload the window; new calls join the top of the log.
  const liveRefresh = (): void => {
    clearTimeout(liveTimer);
    liveTimer = setTimeout(() => {
      if (fixture) return;
      void reload();
      void fetchCalls()
        .then((page) => {
          if (!alive) return;
          calls = mergeCalls(page.calls, calls);
          if (calls.length <= PAGE) callsNext = page.next_before;
          callsTotal = page.total;
          renderCalls();
        })
        .catch(() => undefined);
    }, 700);
  };

  const refreshPrices = async (): Promise<void> => {
    try {
      if (!fixture) await api.refreshPrices();
      prices.clear();
      await loadPricesMeta();
      await reload();
      toast('Prices refreshed.');
    } catch (err) {
      toast(msg(err), 'error');
    }
  };

  // ---- Rendering ----

  const sortedRows = (): UsageKeyed[] => {
    if (!summary) return [];
    const rows = [...rowsOf(summary, group)];
    const value = (r: UsageKeyed): number | string => (sortKey === 'key' ? labelOf(r.key) : r.totals[sortKey]);
    rows.sort((a, b) => {
      const x = value(a);
      const y = value(b);
      const c = typeof x === 'string' || typeof y === 'string' ? String(x).localeCompare(String(y)) : Number(x) - Number(y);
      return sortDesc ? -c : c;
    });
    return rows;
  };

  function renderControls(): void {
    replace(rangeSlot, seg(RANGE_KEYS.map((k) => ({ key: k, label: RANGE[k].label })), range, (k) => { range = k; void reload(); }, 'Range'));
    replace(groupSlot, tabs(GROUPS, group, (k) => {
      group = k as UsageGroup;
      hidden.clear();
      sortKey = 'usd';
      void reload();
    }));
    replace(metricSlot, seg(METRICS, metric, (k) => { metric = k; render(); }, 'Metric'));
    replace(viewSlot, seg(VIEWS, view, (k) => { view = k; render(); }, 'View'));
    titleEl.textContent = `${view === 'chart' ? 'Over time' : 'Ranked'} by ${GROUPS.find((g) => g.key === group)?.label.toLowerCase() ?? 'model'}`;
  }

  function renderTiles(): void {
    const t = summary?.totals ?? ZERO;
    countTo(valueOf(spendTile), t.usd, usd);
    subOf(spendTile).textContent = summary ? `reported ${usd(t.cost_usd)} · estimated ${usd(t.est_usd)}` : '—';
    countTo(tokIn, t.input, compact);
    countTo(tokOut, t.output, compact);
    subOf(tokTile).textContent = summary ? `cache read ${compact(t.cache_read)} · write ${compact(t.cache_write)}` : '—';
    countTo(valueOf(hitTile), t.cache_hit_rate * 100, pct);
    const save = cacheSaving(summary?.by_model ?? [], prices);
    subOf(hitTile).textContent = !summary ? '—' : t.cache_read === 0 ? 'no cache reads yet' : `${compact(t.cache_read)} cached tokens saved ~${usd(save.usd)}${save.complete ? '' : ' (partly priced)'}`;
    countTo(valueOf(callTile), t.calls, (v) => Math.round(v).toLocaleString('en-US'));
    subOf(callTile).textContent = summary ? `${summary.by_model.length} models · ${summary.unpriced.length} unpriced` : '—';
    const values = series?.points.map((p) => p.totals.usd) ?? [];
    if (spark) spark.update(values);
    else spark = sparkline(sparkEl, values, { color: 'var(--chart-1)', height: 38, ariaLabel: 'Spend per bucket' });
  }

  function renderChart(): void {
    const msgNode = !loaded
      ? h('p.faint', null, 'Reading the ledger…')
      : loadError !== null
        ? empty('alert', 'The ledger could not be read', loadError)
        : !summary || summary.totals.calls === 0
          ? empty('zap', 'No model calls in this window', 'Every call an agent makes is recorded here with its tokens and cost. Nothing has been called in this window yet.')
          : null;
    replace(chartEmpty, ...(msgNode ? [msgNode] : []));
    chartEmpty.hidden = !msgNode;
    legendEl.hidden = chartEl.hidden = barsTitle.hidden = barsEl.hidden = msgNode !== null;
    if (msgNode || !series) return;

    const fmt = fmtOf(metric);
    ranked = rankGroups(series, metric);
    plan = areaData(series, ranked, hidden, metric, labelOf);
    const current = plan;
    const tip = (i: number): Node => {
      const total = current.layers.reduce((s, l) => s + (l.values[i] ?? 0), 0);
      return h(
        'div.us-tip',
        null,
        h('strong', null, current.labels[i] ?? ''),
        current.layers.map((l) => h('div.us-tip-row', null, h('i', { style: `background:${l.color}` }), h('span', null, l.label), h('b', null, fmt(l.values[i] ?? 0)))),
        h('div.us-tip-total', null, h('span', null, 'Total'), h('b', null, fmt(total))),
      );
    };
    if (areaHandle) areaHandle.update(plan, { format: fmt, tooltip: tip });
    else areaHandle = stackedArea(chartEl, plan, { format: fmt, tooltip: tip, height: 260, ariaLabel: 'Spend over time by group' });

    replace(
      legendEl,
      ranked.slice(0, 12).map((k, i) => {
        const on = !hidden.has(k);
        const chip = h('button.us-chip', { type: 'button', class: on ? '' : 'off', 'aria-pressed': String(on) }, h('i', { style: `background:${colorOf(i)}` }), labelOf(k));
        chip.addEventListener('click', () => {
          if (on) hidden.add(k);
          else hidden.delete(k);
          render();
        });
        return chip;
      }),
    );

    const rows = summary ? rowsOf(summary, group) : [];
    const items: BarItem[] = ranked
      .filter((k) => !hidden.has(k))
      .slice(0, 8)
      .map((k) => ({ key: k, label: labelOf(k), value: metricOf(rows.find((r) => r.key === k)?.totals ?? ZERO, metric), color: colorOf(ranked.indexOf(k)) }));
    if (barHandle) barHandle.update(items);
    else barHandle = bars(barsEl, items, { format: fmt, height: 150, ariaLabel: 'Totals by group' });
  }

  function renderTable(): void {
    if (!summary) {
      replace(tablePane, h('p.faint', null, loadError ?? (loaded ? 'No data.' : 'Reading the ledger…')));
      return;
    }
    const rows = sortedRows();
    const max = Math.max(0, ...rows.map((r) => r.totals.usd));
    const head = (key: SortKey, label: string, num: boolean): HTMLElement => {
      const active = sortKey === key;
      const b = h('button.us-sort', { type: 'button' }, `${label}${active ? (sortDesc ? ' ↓' : ' ↑') : ''}`);
      b.addEventListener('click', () => {
        if (active) sortDesc = !sortDesc;
        else {
          sortKey = key;
          sortDesc = true;
        }
        renderTable();
      });
      return h('th', { class: `${num ? 'num' : ''} ${active ? 'sorted' : ''}` }, b);
    };
    const body = rows.map((r) =>
      h(
        'tr',
        { 'data-key': r.key },
        h('td', null, h('strong', null, labelOf(r.key)), isTrainer(r.key) ? trainsBadge() : null),
        SORT_COLS.map((c) => h('td', { class: 'num' }, c.text(r.totals))),
        h('td', { class: 'num us-shr' }, shareBar(max > 0 ? r.totals.usd / max : 0), h('span', null, pct(summary && summary.totals.usd > 0 ? (r.totals.usd / summary.totals.usd) * 100 : 0))),
      ),
    );
    replace(
      tablePane,
      h(
        'div.us-table-wrap',
        null,
        h(
          'table.table.us-table',
          null,
          h('thead', null, h('tr', null, h('th', null, labelOf(group)), SORT_COLS.map((c) => head(c.key, c.label, true)), head('usd', 'Share', true))),
          h('tbody', null, body),
        ),
      ),
    );
    if (!rows.length) replace(tablePane, empty('zap', 'Nothing to rank yet', 'The table ranks each group once calls are recorded.'));
  }

  function renderPrices(): void {
    const models = [...new Set([...(summary?.by_model.map((m) => m.key) ?? []), ...settingsModels()])];
    const sources = pricesMeta?.sources ?? [];
    const sourceNames = sources.filter((s) => s.ok).map((s) => SOURCE_LABEL[s.name] ?? s.name).join(' and ');
    const refresh = button('Refresh', () => void refreshPrices(), { icon: 'refresh', size: 'sm' });
    replace(
      priceLine,
      pricesMeta
        ? `Prices from ${sourceNames || 'no source'} · updated ${ago(pricesMeta.fetched_at_ms)} · `
        : 'Prices unavailable · ',
      refresh,
    );
    const uses = settingsUses();
    const columns: Column<string>[] = [
      {
        label: 'Model',
        cell: (m) => {
          const { provider, model } = splitKey(m);
          return h('span.truncate', { title: m }, h('strong', null, model), ' ', h('span.faint', null, provider), isTrainer(m) ? [' ', trainsBadge()] : null);
        },
      },
      { label: 'Used by', cell: (m) => (uses.get(m) ?? []).join(', ') || '—' },
      { label: 'Source', cell: (m) => (prices.get(m)?.source ? SOURCE_LABEL[prices.get(m)?.source ?? ''] ?? prices.get(m)?.source ?? '—' : '—') },
      { label: 'Input', num: true, cell: (m) => (prices.get(m) ? usd(prices.get(m)?.input ?? 0) : '—') },
      { label: 'Output', num: true, cell: (m) => (prices.get(m) ? usd(prices.get(m)?.output ?? 0) : '—') },
      { label: 'Cache read', num: true, cell: (m) => (prices.get(m)?.cache_read != null ? usd(prices.get(m)?.cache_read ?? 0) : '—') },
      { label: 'Cache write', num: true, cell: (m) => (prices.get(m)?.cache_write != null ? usd(prices.get(m)?.cache_write ?? 0) : '—') },
      {
        label: 'Status',
        cell: (m) =>
          prices.get(m)
            ? h('span.badge.ok', null, 'priced')
            : h('span', null, h('span.badge.warn', null, 'unpriced'), ' ', h('a', { href: href('models', 'policy') }, 'set a price under Models')),
      },
    ];
    replace(
      priceBody,
      models.length
        ? h('div.us-table-wrap', null, table(columns, models, undefined, (m) => m))
        : empty('models', 'No models in use', 'Models appear here once a model is called or set as a default.'),
      h('p.faint.us-note', null, 'USD per million tokens. The estimate uses these rates; the reported cost is what the provider charged.'),
    );
  }

  function renderCalls(): void {
    const modelOpts = [{ value: '', label: 'All models' }, ...(summary?.by_model ?? []).map((m) => ({ value: m.key, label: m.key }))];
    const agentOpts = [{ value: '', label: 'All agents' }, ...knownAgents().map((a) => ({ value: String(a.cid), label: a.name }))];
    const modelSel = select(modelOpts, callModel);
    const agentSel = select(agentOpts, callAgent);
    modelSel.addEventListener('change', () => { callModel = modelSel.value; void loadCalls(); });
    agentSel.addEventListener('change', () => { callAgent = agentSel.value; void loadCalls(); });
    replace(callFilters, h('label.us-filter', null, h('span', null, 'Model'), modelSel), h('label.us-filter', null, h('span', null, 'Agent'), agentSel));

    const columns: Column<UsageCall>[] = [
      { label: 'Time', cell: (c) => h('span.num', { title: new Date(c.at_ms).toLocaleString() }, new Date(c.at_ms).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', second: '2-digit' })) },
      { label: 'Agent', cell: (c) => h('a', { href: href('agents', c.cid) }, nameOf(c.cid)) },
      { label: 'Model', cell: (c) => h('span.truncate', { title: c.model }, c.model, isTrainer(c.model) ? [' ', trainsBadge()] : null) },
      { label: 'In', num: true, cell: (c) => compact(c.input) },
      { label: 'Out', num: true, cell: (c) => compact(c.output) },
      { label: 'Cache r / w', num: true, cell: (c) => `${compact(c.cache_read)} / ${compact(c.cache_write)}` },
      {
        label: 'Cost',
        num: true,
        cell: (c) => (c.cost_usd !== null ? h('span', { title: 'Reported by the provider' }, usd(c.cost_usd)) : h('span.faint', { title: 'Estimated from the price' }, `~${usd(c.est_usd)}`)),
      },
      { label: 'Task', cell: (c) => (c.task ? h('a', { href: href('tasks', c.task) }, c.task) : h('span.faint', null, '—')) },
      { label: 'Via', cell: (c) => h('span.faint', null, c.via) },
    ];
    replace(
      callBody,
      calls.length ? table(columns, calls, undefined, (c) => String(c.id)) : empty('zap', 'No calls match', callModel || callAgent ? 'Clear the filters to see every call.' : 'Calls appear here as agents work.'),
    );
    const more = button(busyMore ? 'Loading…' : callsNext === null ? 'All calls loaded' : 'Load older', () => void loadOlder(), { disabled: callsNext === null || busyMore });
    replace(callFoot, h('span.faint', null, `Showing ${calls.length.toLocaleString('en-US')} of ${callsTotal.toLocaleString('en-US')} · newest first`), h('span', { style: 'flex:1' }), more);
  }

  function render(): void {
    renderControls();
    renderTiles();
    renderChart();
    renderTable();
    renderCalls();
    renderPrices();
    chartPane.classList.toggle('on', view === 'chart');
    tablePane.classList.toggle('on', view === 'table');
  }

  const offs = [on('usage', liveRefresh), on('work', liveRefresh), agents.watch(() => { if (summary) renderCalls(); })];
  render();
  const boot = async (): Promise<void> => {
    if (!fixture) settings = await api.settings().catch(() => null);
    if (!fixture) trainers = await api.models().then((v) => new Set(v.models.filter(isContributor).map((m) => `${m.provider}/${m.id}`))).catch(() => new Set<string>());
    await loadPricesMeta();
    await loadCalls();
    await reload();
  };
  void boot();

  return () => {
    alive = false;
    clearTimeout(liveTimer);
    offs.forEach((off) => off());
    areaHandle?.destroy();
    barHandle?.destroy();
    spark?.destroy();
  };
}
