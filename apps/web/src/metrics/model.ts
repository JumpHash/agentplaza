// The Metrics view model, mirroring crates/apz-metrics-view/src/model.rs (the hub composes every tab as these
// panels; this console and the desktop only render them). Field names are the JSON ones (snake_case); enums are
// tagged with `type`. Keep this file in step with model.rs.

import type { ArtifactBody } from '../view/model';

export type Unit = 'count'| 'percent' | 'ms' | 'bytes' | 'bytes_per_sec' | 'mib' | 'usd' | 'tokens' | 'tokens_per_sec' | 'ratio';

/** A colour token: `chart-1` … `chart-8`, `chart-other`, `ink-3`, `rule`, `ok`, `warn`, `crit`, `info`, `signal`. */
export type Tone =
  | 'chart-1' | 'chart-2' | 'chart-3' | 'chart-4' | 'chart-5' | 'chart-6' | 'chart-7' | 'chart-8'
  | 'chart-other' | 'ink-3' | 'rule' | 'ok' | 'warn' | 'crit' | 'info' | 'signal';

export type Status = 'ok' | 'warn' | 'crit' | 'info' | 'mute';

export interface ViewQuery {
  tab: string;
  scope: string;
  range: string;
  shift: number;
  zoom?: [number, number];
  compare: string;
  group: string;
  trace?: string;
  filter?: string;
  width: number;
  tz: number;
  fixture: boolean;
}

export const RANGES: { key: string; ms: number }[] = [
  { key: '15m', ms: 15 * 60_000 },
  { key: '1h', ms: 3_600_000 },
  { key: '6h', ms: 6 * 3_600_000 },
  { key: '24h', ms: 24 * 3_600_000 },
  { key: '7d', ms: 7 * 86_400_000 },
  { key: '30d', ms: 30 * 86_400_000 },
  { key: '90d', ms: 90 * 86_400_000 },
];

export interface View {
  tab: string;
  scope: ScopeInfo;
  tabs: TabInfo[];
  panels: Panel[];
  from_ms: number;
  to_ms: number;
  step_ms: number;
  tier: string;
  points: number;
  level: string;
  stepped_down: boolean;
  live: boolean;
  firing: number;
  compare_label: string;
  annotations: Annotation[];
  fixture: boolean;
}

export interface ScopeInfo {
  id: string;
  kind: string;
  label: string;
  /** (label, id), Global first. */
  path: [string, string][];
  siblings: [string, string][];
}

export interface TabInfo {
  key: string;
  label: string;
  group: string;
  badge?: number;
}

export interface Panel {
  id: string;
  title: string;
  sub: string;
  right: string;
  span: number;
  metric?: string;
  live: boolean;
  pinned?: PinRef;
  /** Buttons in the panel's head. Omitted when there are none. */
  actions?: PanelAction[];
  body: Body;
}

/** A head button: the console posts to `post` with no body, toasts `done`, then loads the view again. */
export interface PanelAction {
  label: string;
  post: string;
  done: string;
}

export interface PinRef {
  tab: string;
  scope: string;
  panel: string;
  from: string;
}

export type Body =
  | { type: 'health'; items: HealthItem[] }
  | { type: 'tiles'; tiles: Tile[] }
  | ({ type: 'chart' } & Chart)
  | ({ type: 'heatmap' } & Heatmap)
  | { type: 'treemap'; unit: Unit; items: TreeItem[] }
  | { type: 'gauges'; gauges: GaugeItem[] }
  | ({ type: 'table' } & Table)
  | { type: 'flags'; flags: Flag[] }
  | { type: 'bars'; rows: BarRow[]; note: string }
  | { type: 'split'; rows: SplitRow[]; legend: LegendItem[] }
  | { type: 'stats'; stats: StatLine[] }
  | ({ type: 'swimlanes' } & Swimlanes)
  | { type: 'alerts'; rows: AlertRow[] }
  | { type: 'rules'; rules: RuleRow[] }
  | ({ type: 'traces' } & TracesBody)
  | { type: 'events'; kinds: string[]; rows: EventRow[] }
  | { type: 'empty'; text: string }
  | ArtifactBody;

export interface HealthItem { area: string; status: Status; text: string; tab: string }

export interface Tile {
  label: string;
  value: string;
  change_pct?: number;
  up_bad: boolean;
  note: string;
  spark: number[];
  tone: Tone;
  tab?: string;
}

export type XAxis = { type: 'time'; start_ms: number; step_ms: number } | { type: 'labels'; labels: string[] };

export interface Chart {
  unit: Unit;
  x: XAxis;
  series: Series[];
  stacked: boolean;
  area: boolean;
  y_max?: number;
  band?: Band;
  ghosts: Series[];
  markers: Marker[];
  limit?: Limit;
  height: number;
  annotate: boolean;
  point_tab?: string;
}

export interface Series {
  name: string;
  tone: Tone;
  dash: boolean;
  values: (number | null)[];
  gap_reason?: string;
  scope?: string;
}

export interface Band { name: string; tone: Tone; lo: number[]; hi: number[] }
export interface Marker { index: number; label: string }
export interface Limit { value: number; label: string }
export interface Annotation { t_ms: number; kind: string; label: string }

export interface Heatmap {
  start_ms: number;
  step_ms: number;
  unit: Unit;
  lo: number;
  hi: number;
  /** counts[col][row], row 0 the lowest bucket. */
  counts: number[][];
  height: number;
}

export interface TreeItem { name: string; value: number; tone: Tone }
export interface GaugeItem { frac: number; label: string; sub: string; status: Status }

/** `more` is the GET route that answers the next page as a Table; omitted or null when there are no older rows. */
export interface Table { columns: Column[]; rows: Row[]; empty: string; more?: string | null }
export interface Column { head: string; num: boolean }
export interface Row { cells: Cell[]; scope?: string; tab?: string; trace?: string; selected: boolean }

export type Cell =
  | { type: 'text'; text: string }
  | { type: 'mono'; text: string }
  | { type: 'named'; name: string; id: string }
  | { type: 'num'; text: string }
  | { type: 'status'; text: string; status: Status }
  | { type: 'pill'; text: string; status: Status }
  | { type: 'muted'; text: string }
  | { type: 'spark'; values: number[]; tone: Tone }
  | { type: 'bar'; parts: [number, Tone, string][] };

export interface Flag { status: Status; text: string; when: string; tab?: string; scope?: string }
export interface BarRow { label: string; value: string; frac: number; status: Status; note: string }
export interface SplitRow { label: string; parts: [number, Tone, string][]; total: string }
export interface LegendItem { label: string; tone: Tone }
export interface StatLine { label: string; value: string; note: string }

export interface Swimlanes { start_ms: number; end_ms: number; lanes: Lane[]; legend: LegendItem[] }
export interface Lane { name: string; scope: string | null; segs: Seg[] }
export interface Seg { start_ms: number; end_ms: number; state: string; tone: Tone; detail: string }

export interface AlertRow {
  rule: string;
  name: string;
  scope: string;
  scope_label: string;
  cond: string;
  for_text: string;
  since: string;
  now: string;
  severity: Status;
  spark: number[];
  tab: string;
  acked: boolean;
}

export interface RuleRow {
  id: string;
  on: boolean;
  name: string;
  scope_label: string;
  cond: string;
  for_text: string;
  severity: Status;
  state: string;
  last: string;
  starter: boolean;
}

export interface TracesBody { filter: string; list: TraceRow[]; selected?: TraceDetail }
export interface TraceRow { id: string; time: string; agent: string; total: string; kept: string; kept_text: string; error: boolean }
export interface TraceDetail {
  id: string;
  title: string;
  sub: string;
  error: boolean;
  total_ms: number;
  spans: SpanRow[];
  legend: LegendItem[];
}
export interface SpanRow {
  name: string;
  depth: number;
  tone: Tone;
  start_ms: number;
  dur_ms: number;
  first_token_ms?: number;
  error: boolean;
  attrs: [string, string][];
}

export interface EventRow { t_ms: number; time: string; kind: string; status: Status; subject: string; scope?: string; detail: string }

export interface Around {
  label: string;
  sub: string;
  events: EventRow[];
  movers: MoverRow[];
  traces: { total: number; errors: number; slow: number };
  zoom: [number, number];
}
export interface MoverRow { name: string; change: string; status: Status; spark: number[]; tab: string; scope?: string }

export interface ScopeNode { id: string; label: string; kind: string; group: string; depth: number; tag: string; status?: Status }

export interface Saved { scopes: string[]; pins: Pin[]; recent: string[]; starter: boolean }
export interface Pin { tab: string; scope: string; panel: string }

export interface LiveFrame { scope: string; tab: string; group: string; range: string; t_ms: number; panels: Panel[] }
export interface LiveLease { scope: string; tab: string; group: string; range: string }

/** The catalog (`GET /v1/metrics/catalog`), for "About this metric". */
export interface MetricDef {
  id: string;
  title: string;
  unit: Unit;
  kind: 'counter' | 'gauge' | 'histogram';
  level: string;
  entity: string;
  labels: string[];
  source: string;
  cost: string;
  description: string;
}

/** `GET`/`PUT /v1/metrics/settings`. */
export interface MetricsSettings {
  level: 'off' | 'basic' | 'standard' | 'full';
  retention: { chunks_hours: number; minute_days: number; hour_days: number; calls_days: number; traces_days: number; traces_cap_mb: number; events_days: number };
  budgets: { scope: string; cap_usd: number }[];
  monthly_budget_usd: number | null;
  basis_days: number;
  outside: { openmetrics: boolean; otlp: boolean; otlp_url: string; otlp_confirmed: boolean };
}

/** An alert rule as `PUT /v1/metrics/alerts/rules` takes it. */
export interface Rule {
  id: string;
  name: string;
  metric: string;
  agg: string;
  scope: string;
  cmp: 'gt' | 'lt';
  threshold: number;
  for_ms: number;
  severity: 'warning' | 'critical';
  on: boolean;
  notify: { console: boolean; desktop: boolean; board: boolean };
  starter: boolean;
}

/** The CSS colour of a tone token. */
export const toneVar = (t: Tone): string => (t === 'ok' ? 'var(--ok)' : t === 'warn' ? 'var(--warn)' : t === 'crit' ? 'var(--crit)' : t === 'info' ? 'var(--info)' : t === 'signal' ? 'var(--signal)' : `var(--${t})`);

/** The CSS colour of a status. */
export const statusVar = (s: Status): string => (s === 'ok' ? 'var(--ok)' : s === 'warn' ? 'var(--warn)' : s === 'crit' ? 'var(--crit)' : s === 'info' ? 'var(--info)' : 'var(--ink-3)');
