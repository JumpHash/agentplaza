// Formatting shared with the desktop: the same rules as crates/apz-metrics-view/src/format.rs, checked against the
// same golden file (crates/apz-metrics-view/tests/format_golden.json) by the test suite.

import type { Unit } from './model';

/** A value as text (`—` for none or not finite). */
export function format(unit: Unit, v: number | null | undefined): string {
  void unit;
  void v;
  throw new Error('format: not written yet');
}

/** The axis ceiling: the smallest of 1, 2, 2.5, 5, 10 × 10^k at or above `max` (1 for `max <= 0`). */
export function nice(max: number): number {
  void max;
  throw new Error('nice: not written yet');
}

/** Five ticks from 0 to nice(max · 1.05). */
export function ticks(max: number): number[] {
  const ym = nice(max * 1.05);
  return [0, ym / 4, ym / 2, (ym * 3) / 4, ym];
}

/** The label of a point at `t` on an axis stepping `step`, at `offsetMin` minutes from UTC. */
export function timeLabel(t: number, step: number, offsetMin: number): string {
  void t;
  void step;
  void offsetMin;
  throw new Error('timeLabel: not written yet');
}

/** The console's offset from UTC in minutes (what the hub is told as `tz`). */
export const localOffset = (): number => -new Date().getTimezoneOffset();
