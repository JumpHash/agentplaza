// The Metrics view's state: what is in the URL (`#/metrics?tab=…&scope=…&range=…&shift=…&zoom=a,b&compare=…
// &group=…&trace=…&filter=…&fixture=1`), the request it makes, the owner's saved pins and scopes, and the live lease
// that keeps frames coming while the view is visible.

import type { LiveLease, Saved, ViewQuery } from './model';

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
  void hash;
  throw new Error('parseHash: not written yet');
}

/** The route hash of a state (defaults left out), e.g. `#/metrics?tab=latency&scope=node%3An1`. */
export function toHash(s: MetricsState): string {
  void s;
  throw new Error('toHash: not written yet');
}

/** The hub request for a state at a chart width and the console's UTC offset. */
export function queryOf(s: MetricsState, width: number, tz: number): ViewQuery {
  void s;
  void width;
  void tz;
  throw new Error('queryOf: not written yet');
}

/** Live applies to the 15m and 1h ranges, ending now, when live is on. */
export function liveApplies(s: MetricsState): boolean {
  return s.live && s.shift === 0 && (s.range === '15m' || s.range === '1h');
}

/** The per-console preferences (live, annotations, compact) from localStorage (defaults when unavailable). */
export function loadPrefs(): Pick<MetricsState, 'live' | 'annotations' | 'compact'> {
  throw new Error('loadPrefs: not written yet');
}

export function savePrefs(s: Pick<MetricsState, 'live' | 'annotations' | 'compact'>): void {
  void s;
  throw new Error('savePrefs: not written yet');
}

/** Saved with a scope visited: it goes first in `recent` (at most 6, no duplicates). */
export function visit(saved: Saved, scope: string): Saved {
  void saved;
  void scope;
  throw new Error('visit: not written yet');
}

/** Saved with a scope pinned or unpinned. */
export function togglePinnedScope(saved: Saved, scope: string): Saved {
  void saved;
  void scope;
  throw new Error('togglePinnedScope: not written yet');
}

/** Saved with a panel pinned or unpinned (pins are (tab, scope, panel id)); `starter` becomes false. */
export function togglePin(saved: Saved, pin: { tab: string; scope: string; panel: string }): Saved {
  void saved;
  void pin;
  throw new Error('togglePin: not written yet');
}

/** Renews a live lease every 10 s while `active()` holds (the view visible and live applies), and stops when it
 * does not. `send` posts the lease. */
export class LeaseKeeper {
  constructor(send: (l: LiveLease) => Promise<unknown>, active: () => LiveLease | null) {
    void send;
    void active;
  }

  /** Checks now and on every 10 s tick; call after anything that changes `active()`. */
  poke(): void {
    throw new Error('LeaseKeeper.poke: not written yet');
  }

  stop(): void {
    throw new Error('LeaseKeeper.stop: not written yet');
  }
}
