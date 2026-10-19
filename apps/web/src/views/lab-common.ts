// Shared helpers for the Lab sections: a fetch wrapper whose errors read as sentences, the
// formatters every section uses (numbers, sizes, durations, percents, times), the live
// `training` subscription on the console socket, a poll helper, and the empty state.

import { ApiError, type JobEvent } from '../api';
import { on } from '../store';
import { h, lu } from '../ui/dom';
import type { LucideName } from '../ui/icons.gen';

// ---- Requests --------------------------------------------------------------

/**
 * One apid call under `/v1/...` that throws readable errors: the hub's `error` text when it gave
 * one, "apid is unreachable" when the call never got an answer. A 401 raises the sign-in screen,
 * as the api client does.
 */
export async function labRequest<T>(method: string, path: string, body?: unknown): Promise<T> {
  let res: Response;
  try {
    res = await fetch(path, {
      method,
      headers: body === undefined ? {} : { 'content-type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new ApiError(0, `${method} ${path}: apid is unreachable`);
  }
  const text = await res.text();
  let data: unknown = null;
  try {
    data = text ? (JSON.parse(text) as unknown) : null;
  } catch {
    data = null;
  }
  if (!res.ok) {
    if (res.status === 401) window.dispatchEvent(new CustomEvent('apz-auth-required'));
    const hub = (data as { error?: unknown } | null)?.error;
    throw new ApiError(res.status, typeof hub === 'string' && hub ? hub : `${method} ${path} failed: HTTP ${res.status}`);
  }
  return data as T;
}

/** The message to show for anything a failed call threw. */
export function errorText(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}

// ---- Formatters ------------------------------------------------------------

const DASH = '—';

/** A count or a quantity with grouping, `digits` decimals; a dash for no value. */
export function fmtNumber(n: number | null | undefined, digits = 0): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return DASH;
  return n.toLocaleString(undefined, { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

/** A byte count in binary units (B, KB, MB, GB, TB). */
export function fmtSize(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes)) return DASH;
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  return `${value.toFixed(digits)} ${units[unit]}`;
}

/** A duration from milliseconds: `840 ms`, `12.5 s`, `4m 07s`, `2h 13m`. */
export function fmtDuration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return DASH;
  const sign = ms < 0 ? '-' : '';
  const abs = Math.abs(ms);
  if (abs < 1000) return `${sign}${Math.round(abs)} ms`;
  const s = abs / 1000;
  if (s < 60) return `${sign}${s.toFixed(1)} s`;
  const totalSeconds = Math.round(s);
  if (totalSeconds < 3600) {
    return `${sign}${Math.floor(totalSeconds / 60)}m ${String(totalSeconds % 60).padStart(2, '0')}s`;
  }
  return `${sign}${Math.floor(totalSeconds / 3600)}h ${String(Math.floor((totalSeconds % 3600) / 60)).padStart(2, '0')}m`;
}

/** A ratio (0 to 1) as a percent: `0.42` is `42%`. Pass `util_pct / 100` for the device figures. */
export function fmtPercent(ratio: number | null | undefined, digits = 0): string {
  if (ratio === null || ratio === undefined || !Number.isFinite(ratio)) return DASH;
  return `${(ratio * 100).toFixed(digits)}%`;
}

/** Device-hours: the GPU time a job or run has used. */
export function fmtGpuHours(hours: number | null | undefined): string {
  if (hours === null || hours === undefined || !Number.isFinite(hours)) return DASH;
  return `${hours.toFixed(2)} GPU-h`;
}

/** An absolute local time from unix milliseconds: `2026-10-13 14:05`. The desktop's `time()` prints the same. */
export function fmtTime(ms: number | null | undefined): string {
  if (!ms) return DASH;
  const d = new Date(ms);
  const p = (n: number): string => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

// ---- Live and polling ------------------------------------------------------

/** One `training` frame from the console socket: the job and its stored event line. */
export interface TrainingFrame { job: string; line: JobEvent }

/**
 * Subscribes to the trainer's live lines (the console socket's `training` frames), for every job or
 * for one `job`. Returns the unsubscribe function; call it from the section's cleanup.
 */
export function onTraining(fn: (frame: TrainingFrame) => void, job?: string): () => void {
  return on('training', (data) => {
    const frame = data as TrainingFrame | null;
    if (!frame || typeof frame.job !== 'string' || !frame.line) return;
    if (job !== undefined && frame.job !== job) return;
    fn(frame);
  });
}

/** Runs `load` now and then every `ms`; returns the stop function. Errors are the caller's to show. */
export function pollEvery(ms: number, load: () => void): () => void {
  load();
  const timer = setInterval(load, ms);
  return () => clearInterval(timer);
}

// ---- Empty state -----------------------------------------------------------

/** The empty (or not-yet-built) state: the prototype's `h.empty` (an icon, a bold title, an optional sentence). */
export function labEmpty(iconName: LucideName, title: string, text?: string, action?: Node): HTMLElement {
  return h('div.empty', null, lu(iconName, 22), h('b', null, title), text ? h('span', null, text) : null, action ?? null);
}

// ---- The prototype's small text and control helpers -------------------------

/** A relative time as the prototype reads it: `just now`, `30 min ago`, `2 h ago`, `3 d ago`. */
export function agoText(ms: number | null | undefined, now = Date.now()): string {
  if (!ms) return DASH;
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return 'just now';
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}

/** A job's run time as the prototype prints it: `40 min`, `2.0 h`. */
export function runTimeText(ms: number): string {
  return ms < 3600000 ? `${Math.round(ms / 60000)} min` : `${(ms / 3600000).toFixed(1)} h`;
}

/** GPU-hours used of a budget, as the prototype's budget cell: the figure, then a meter that turns red over budget. */
export function budgetCell(used: number, budget: number): HTMLElement {
  const over = used > budget;
  const pct = budget > 0 ? Math.min(100, (used / budget) * 100) : 0;
  return h(
    'div.lab-budget',
    { class: over ? 'is-over' : null },
    h('span.num', null, `${fmtNumber(used, 2)} of ${fmtNumber(budget)}`),
    h('div.lab-meter', { class: over ? 'is-over' : null }, h('i', { style: `width:${pct.toFixed(1)}%` })),
  );
}

/** The prototype's `h.btn`: `button.btn` (`.primary`, `.ghost`, `.danger`, `.sm`) with a lucide icon and a label. */
export function labButton(
  label: string,
  onClick: (ev: MouseEvent) => void,
  opts: { kind?: 'primary' | 'ghost' | 'danger'; sm?: boolean; icon?: LucideName; title?: string; disabled?: boolean } = {},
): HTMLButtonElement {
  const classes = ['btn', opts.kind ?? '', opts.sm ? 'sm' : '', label ? '' : 'icon-only'].filter(Boolean).join(' ');
  const b = h(
    'button',
    { class: classes, type: 'button', title: opts.title ?? null, 'aria-label': label ? null : (opts.title ?? null), disabled: !!opts.disabled },
    opts.icon ? lu(opts.icon, opts.sm ? 14 : 15) : null,
    label ? h('span', null, label) : null,
  );
  b.addEventListener('click', onClick);
  return b;
}

/** A segmented control (`div.seg`): each option is a button, the chosen one is `aria-pressed`. */
export function seg(options: [string, string][], current: string, onPick: (value: string) => void): HTMLElement {
  const group = h(
    'div.seg',
    { role: 'group' },
    options.map(([value, label]) => {
      const b = h('button', { type: 'button', 'aria-pressed': value === current ? 'true' : 'false' }, label);
      b.addEventListener('click', () => onPick(value));
      return b;
    }),
  );
  return group;
}
