// Formatting shared with the desktop: the same rules as crates/apz-metrics-view/src/format.rs, checked against the
// same golden file (crates/apz-metrics-view/tests/format_golden.json) by the test suite.

import type { Unit } from './model';

const KIB = 1024;
const MIB = KIB * 1024;
const GIB = MIB * 1024;

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** A value as text (`—` for none or not finite). */
export function format(unit: Unit, v: number | null | undefined): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return '—';
  if (v < 0) return `-${format(unit, -v)}`;
  switch (unit) {
    case 'usd': {
      if (v >= 1000) return `$${groupThousands(fixed(v, 0))}`;
      const decimals = settled(v, (x) => (x >= 100 ? 0 : x >= 10 ? 1 : 2));
      if (roundTo(v, decimals) >= 1000) return `$${groupThousands(fixed(v, 0))}`;
      return `$${fixed(v, decimals)}`;
    }
    case 'tokens':
      return shortNumber(v);
    case 'count': {
      if (v >= 1000) return shortNumber(v);
      if (v < 10 && roundTo(v, 1) % 1 !== 0) return fixed(v, 1);
      return fixed(v, 0);
    }
    case 'ms': {
      if (v >= 1000) {
        const seconds = v / 1000;
        const decimals = settled(seconds, (x) => (x >= 10 ? 0 : 1));
        return `${fixed(seconds, decimals)} s`;
      }
      if (roundTo(v, 0) >= 1000) return '1.0 s';
      return `${fixed(v, 0)} ms`;
    }
    case 'percent': {
      const decimals = settled(v, (x) => (x < 10 ? 1 : 0));
      return `${fixed(v, decimals)}%`;
    }
    case 'mib':
      if (v >= 1024) return `${fixed(v / 1024, 1)} GiB`;
      return `${fixed(v, 0)} MiB`;
    case 'bytes':
      return bytes(v);
    case 'bytes_per_sec':
      if (v >= GIB) return `${fixed(v / GIB, 1)} GiB/s`;
      if (v >= MIB) return `${fixed(v / MIB, 1)} MiB/s`;
      if (v >= KIB) return `${fixed(v / KIB, 0)} KiB/s`;
      return `${fixed(v, 0)} B/s`;
    case 'tokens_per_sec':
      return `${fixed(v, 0)} tok/s`;
    case 'ratio':
      return fixed(v, 2);
  }
}

/** The axis ceiling: the smallest of 1, 2, 2.5, 5, 10 × 10^k at or above `max` (1 for `max <= 0`). */
export function nice(max: number): number {
  if (!Number.isFinite(max) || max <= 0) return 1;
  const scale = pow10(Math.floor(Math.log10(max)));
  const ratio = max / scale;
  const step = [1, 2, 2.5, 5].find((candidate) => candidate >= ratio) ?? 10;
  return step * scale;
}

/** Five ticks from 0 to nice(max · 1.05). */
export function ticks(max: number): number[] {
  const ym = nice(max * 1.05);
  return [0, ym / 4, ym / 2, (ym * 3) / 4, ym];
}

/**
 * The label of a point at `t` (ms) on an axis stepping `step` (ms), at `offsetMin` minutes from UTC: step ≥ 1 day
 * gives `Oct 16`; step ≥ 1 h gives `Oct 16 14:00`; step < 1 min gives `14:20:05`; otherwise `14:20`.
 */
export function timeLabel(t: number, step: number, offsetMin: number): string {
  const MINUTE = 60_000;
  const HOUR = 60 * MINUTE;
  const DAY = 24 * HOUR;

  const local = t + offsetMin * MINUTE;
  const days = Math.floor(local / DAY);
  const inDay = local - days * DAY;
  const [month, day] = civilFromDays(days);
  const hour = Math.floor(inDay / HOUR);
  const minute = Math.floor((inDay % HOUR) / MINUTE);
  const second = Math.floor((inDay % MINUTE) / 1000);

  const clock = `${pad2(hour)}:${pad2(minute)}`;
  const date = `${MONTHS[month - 1]} ${day}`;
  if (step >= DAY) return date;
  if (step >= HOUR) return `${date} ${clock}`;
  if (step < MINUTE) return `${clock}:${pad2(second)}`;
  return clock;
}

/** The console's offset from UTC in minutes (what the hub is told as `tz`). */
export const localOffset = (): number => -new Date().getTimezoneOffset();

/** Half away from zero, as Rust's `round`: JavaScript's Math.round rounds negative halves up instead. */
function roundAway(x: number): number {
  return Math.sign(x) * Math.round(Math.abs(x));
}

/** Ten to the power `n`, computed as Rust's `powi` does for negative exponents (a reciprocal of the positive power). */
function pow10(n: number): number {
  return n >= 0 ? 10 ** n : 1 / 10 ** -n;
}

/** The decimals `pick` chooses for `v`, chosen again for `v` rounded to them, so a value that rounds up across a
 * boundary (99.95 → 100) prints with the decimals of the side it lands on. */
function settled(v: number, pick: (x: number) => number): number {
  const first = pick(v);
  return pick(roundTo(v, first));
}

/** `v` rounded half away from zero to `decimals` places. */
function roundTo(v: number, decimals: number): number {
  const scale = pow10(decimals);
  return roundAway(v * scale) / scale;
}

/** The absolute value of `v` rounded half away from zero to `decimals` places. The caller prints the sign. */
function fixed(v: number, decimals: number): string {
  const scaled = roundAway(Math.abs(v) * pow10(decimals));
  const digits = scaled.toFixed(0);
  if (decimals === 0) return digits;
  const padded = digits.padStart(decimals + 1, '0');
  const whole = padded.slice(0, padded.length - decimals);
  const fraction = padded.slice(padded.length - decimals);
  return `${whole}.${fraction}`;
}

/** Thousands and above as `9.8k`, `41.7M` or `1.23B`; below a thousand, whole. */
function shortNumber(v: number): string {
  if (v >= 1e9) return `${fixed(v / 1e9, 2)}B`;
  if (v >= 1e6) return `${fixed(v / 1e6, 1)}M`;
  if (v >= 1e3) return `${fixed(v / 1e3, 1)}k`;
  return fixed(v, 0);
}

/** A byte count in the largest binary unit that keeps it at or above 1, with 1 decimal below 100. */
function bytes(v: number): string {
  if (v < KIB) return `${fixed(v, 0)} B`;
  const names = ['KiB', 'MiB', 'GiB', 'TiB'];
  let value = v / KIB;
  let index = 0;
  while (value >= KIB && index + 1 < names.length) {
    value /= KIB;
    index += 1;
  }
  const decimals = value < 100 ? 1 : 0;
  return `${fixed(value, decimals)} ${names[index]}`;
}

/** Digits between commas from the right, for a string of whole digits. */
function groupThousands(digits: string): string {
  let grouped = '';
  for (let i = 0; i < digits.length; i += 1) {
    if (i > 0 && (digits.length - i) % 3 === 0) grouped += ',';
    grouped += digits.charAt(i);
  }
  return grouped;
}

/** The month (1 to 12) and day of the month for a count of days since 1970-01-01 (Howard Hinnant's days-to-civil
 * algorithm). */
function civilFromDays(days: number): [number, number] {
  const z = days + 719_468;
  const era = Math.floor(z / 146_097);
  const dayOfEra = z - era * 146_097;
  const yearOfEra = Math.floor(
    (dayOfEra - Math.floor(dayOfEra / 1_460) + Math.floor(dayOfEra / 36_524) - Math.floor(dayOfEra / 146_096)) / 365,
  );
  const dayOfYear = dayOfEra - (365 * yearOfEra + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100));
  const monthIndex = Math.floor((5 * dayOfYear + 2) / 153);
  const day = dayOfYear - Math.floor((153 * monthIndex + 2) / 5) + 1;
  const month = monthIndex < 10 ? monthIndex + 3 : monthIndex - 9;
  return [month, day];
}

function pad2(n: number): string {
  return String(n).padStart(2, '0');
}
