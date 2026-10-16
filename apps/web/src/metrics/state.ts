// The Metrics view's state: what is in the URL (`#/metrics?tab=…&scope=…&range=…&shift=…&zoom=a,b&compare=…
// &group=…&trace=…&filter=…&fixture=1`), the request it makes, the owner's saved pins and scopes, and the live lease
// that keeps frames coming while the view is visible.

import { RANGES, type LiveLease, type Pin, type Saved, type ViewQuery } from './model';

export interface MetricsState {
  tab: string;
  scope: string;
  range: string;
  /** Whole ranges back from now: 0 or negative. */
  shift: number;
  zoom: [number, number] | null;
  /** `prev`, `week`, `none`, `scope:<id>`. */
  compare: string;
  group: string;
  trace: string | null;
  filter: string | null;
  fixture: boolean;
  live: boolean;
  annotations: boolean;
  compact: boolean;
}

export const DEFAULT_STATE: MetricsState = {
  tab: 'overview',
  scope: 'global',
  range: '24h',
  shift: 0,
  zoom: null,
  compare: 'prev',
  group: 'provider',
  trace: null,
  filter: null,
  fixture: false,
  live: true,
  annotations: true,
  compact: false,
};

/** The state in a route hash (`#/metrics?…`); unknown or bad values fall back to the defaults. `live`,
 * `annotations` and `compact` are not in the URL (they are per-console preferences, see `loadPrefs`). */
export function parseHash(hash: string): MetricsState {
  const at = hash.indexOf('?');
  const params = new URLSearchParams(at < 0 ? '' : hash.slice(at + 1));
  const range = params.get('range');
  const shift = params.get('shift');
  return {
    tab: params.get('tab') || DEFAULT_STATE.tab,
    scope: params.get('scope') || DEFAULT_STATE.scope,
    range: range !== null && RANGES.some((r) => r.key === range) ? range : DEFAULT_STATE.range,
    shift: shift !== null && /^-?\d+$/.test(shift) && Number(shift) <= 0 ? Number(shift) : DEFAULT_STATE.shift,
    zoom: zoomOf(params.get('zoom')),
    compare: params.get('compare') || DEFAULT_STATE.compare,
    group: params.get('group') || DEFAULT_STATE.group,
    trace: params.get('trace') || null,
    filter: params.get('filter') || null,
    fixture: params.get('fixture') === '1',
    live: DEFAULT_STATE.live,
    annotations: DEFAULT_STATE.annotations,
    compact: DEFAULT_STATE.compact,
  };
}

/** `a,b` with 0 ≤ a < b ≤ 1, or null for anything else (including a missing value). */
function zoomOf(text: string | null): [number, number] | null {
  if (text === null) return null;
  const parts = text.split(',');
  if (parts.length !== 2 || parts.some((p) => p.trim() === '')) return null;
  const a = Number(parts[0]);
  const b = Number(parts[1]);
  return a >= 0 && a < b && b <= 1 ? [a, b] : null;
}

/** The route hash of a state (defaults left out), e.g. `#/metrics?tab=latency&scope=node%3An1`. */
export function toHash(s: MetricsState): string {
  const params = new URLSearchParams();
  if (s.tab !== DEFAULT_STATE.tab) params.set('tab', s.tab);
  if (s.scope !== DEFAULT_STATE.scope) params.set('scope', s.scope);
  if (s.range !== DEFAULT_STATE.range) params.set('range', s.range);
  if (s.shift !== DEFAULT_STATE.shift) params.set('shift', String(s.shift));
  if (s.zoom) params.set('zoom', `${s.zoom[0].toFixed(3)},${s.zoom[1].toFixed(3)}`);
  if (s.compare !== DEFAULT_STATE.compare) params.set('compare', s.compare);
  if (s.group !== DEFAULT_STATE.group) params.set('group', s.group);
  if (s.trace) params.set('trace', s.trace);
  if (s.filter) params.set('filter', s.filter);
  if (s.fixture) params.set('fixture', '1');
  const query = params.toString();
  return query ? `#/metrics?${query}` : '#/metrics';
}

/** The hub request for a state at a chart width and the console's UTC offset. */
export function queryOf(s: MetricsState, width: number, tz: number): ViewQuery {
  const query: ViewQuery = {
    tab: s.tab,
    scope: s.scope,
    range: s.range,
    shift: s.shift,
    compare: s.compare,
    group: s.group,
    width,
    tz,
    fixture: s.fixture,
  };
  if (s.zoom) query.zoom = [s.zoom[0], s.zoom[1]];
  if (s.trace) query.trace = s.trace;
  if (s.filter) query.filter = s.filter;
  return query;
}

/** Live applies to the 15m and 1h ranges, ending now, when live is on and the view is not the fixture (live frames
 * are composed from recorded data). */
export function liveApplies(s: MetricsState): boolean {
  return s.live && !s.fixture && s.shift === 0 && (s.range === '15m' || s.range === '1h');
}

/** The per-console preferences (live, annotations, compact) from localStorage (defaults when unavailable). */
const PREFS_KEY = 'apz.metrics.prefs';

type Prefs = Pick<MetricsState, 'live' | 'annotations' | 'compact'>;

const PREF_DEFAULTS: Prefs = {
  live: DEFAULT_STATE.live,
  annotations: DEFAULT_STATE.annotations,
  compact: DEFAULT_STATE.compact,
};

export function loadPrefs(): Prefs {
  try {
    const raw = localStorage.getItem(PREFS_KEY);
    if (raw === null) return { ...PREF_DEFAULTS };
    const stored = JSON.parse(raw);
    return {
      live: typeof stored.live === 'boolean' ? stored.live : PREF_DEFAULTS.live,
      annotations: typeof stored.annotations === 'boolean' ? stored.annotations : PREF_DEFAULTS.annotations,
      compact: typeof stored.compact === 'boolean' ? stored.compact : PREF_DEFAULTS.compact,
    };
  } catch {
    return { ...PREF_DEFAULTS };
  }
}

export function savePrefs(s: Prefs): void {
  try {
    localStorage.setItem(PREFS_KEY, JSON.stringify({ live: s.live, annotations: s.annotations, compact: s.compact }));
  } catch {
    // Storage is unavailable (private window, blocked site data): the preferences last for this view only.
  }
}

/** How many scopes `recent` keeps. */
const RECENT_MAX = 6;

/** Saved with a scope visited: it goes first in `recent` (at most 6, no duplicates). */
export function visit(saved: Saved, scope: string): Saved {
  const recent = [scope, ...saved.recent.filter((r) => r !== scope)].slice(0, RECENT_MAX);
  return { ...saved, recent };
}

/** Saved with a scope pinned or unpinned. */
export function togglePinnedScope(saved: Saved, scope: string): Saved {
  const scopes = saved.scopes.includes(scope) ? saved.scopes.filter((s) => s !== scope) : [...saved.scopes, scope];
  return { ...saved, scopes };
}

/** Saved with a panel pinned or unpinned (pins are (tab, scope, panel id)); `starter` becomes false. */
export function togglePin(saved: Saved, pin: { tab: string; scope: string; panel: string }): Saved {
  const same = (p: Pin): boolean => p.tab === pin.tab && p.scope === pin.scope && p.panel === pin.panel;
  const pins = saved.pins.some(same)
    ? saved.pins.filter((p) => !same(p))
    : [...saved.pins, { tab: pin.tab, scope: pin.scope, panel: pin.panel }];
  return { ...saved, pins, starter: false };
}

/** Renews a live lease every 10 s while `active()` holds (the view visible and live applies), and stops when it
 * does not. `send` posts the lease. */
export class LeaseKeeper {
  private readonly send: (l: LiveLease) => Promise<unknown>;
  private readonly active: () => LiveLease | null;
  /** The lease last sent (a copy), or null when nothing is held. */
  private last: LiveLease | null = null;
  private sentAt = 0;
  private timer: ReturnType<typeof setInterval> | undefined;

  constructor(send: (l: LiveLease) => Promise<unknown>, active: () => LiveLease | null) {
    this.send = send;
    this.active = active;
  }

  /** Checks now and on every 10 s tick; call after anything that changes `active()`. */
  poke(): void {
    if (this.timer === undefined) this.timer = setInterval(() => this.poke(), LEASE_RENEW_MS);
    const lease = this.active();
    if (lease === null) {
      // The hub lets an unrenewed lease lapse; the next lease is sent fresh.
      this.last = null;
      return;
    }
    const now = Date.now();
    const same =
      this.last !== null &&
      this.last.scope === lease.scope &&
      this.last.tab === lease.tab &&
      this.last.group === lease.group &&
      this.last.range === lease.range;
    if (same && now - this.sentAt < LEASE_RENEW_MS) return;
    this.last = { ...lease };
    this.sentAt = now;
    // A failed renewal is not reported here; the next tick tries again.
    this.send(lease).catch(() => undefined);
  }

  stop(): void {
    if (this.timer !== undefined) clearInterval(this.timer);
    this.timer = undefined;
    this.last = null;
  }
}

/** How often a live lease is renewed while the view keeps it. */
const LEASE_RENEW_MS = 10_000;
