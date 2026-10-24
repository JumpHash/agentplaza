// The project trends' pure rules: the metric and target columns (the union over the releases, in the order they first
// appear), a release's value in each column, the start time and the flake rate as text. The DOM is not touched here; the
// tab (project-trends.ts) draws these, and test/project-trends-tests.ts checks them.

import type { TrendRelease } from '../api-trends';
import { metricText } from './release-report-model';
import { sizeText } from './releases-model';

/** The metric names of the trends, in the order they first appear across the releases (oldest first). */
export function metricNames(rows: TrendRelease[]): string[] {
  return columnNames(rows, (r) => r.metrics.map((m) => m.name));
}

/** The artifact targets of the trends, in the order they first appear across the releases. */
export function targetNames(rows: TrendRelease[]): string[] {
  return columnNames(rows, (r) => r.sizes.map((s) => s.target));
}

function columnNames(rows: TrendRelease[], of: (r: TrendRelease) => string[]): string[] {
  const seen = new Set<string>();
  for (const r of rows) for (const name of of(r)) seen.add(name);
  return [...seen];
}

/** A release's value of one metric, as text; a dash when the release did not measure it. */
export function metricCell(row: TrendRelease, name: string): string {
  const m = row.metrics.find((x) => x.name === name);
  return m ? metricText(m.value) : '—';
}

/** A release's artifact size for one target, as text; a dash when it has no artifact for that target. */
export function sizeCell(row: TrendRelease, target: string): string {
  const s = row.sizes.find((x) => x.target === target);
  return s ? sizeText(s.size) : '—';
}

/** A start time as UTC to the minute (`2026-10-18 09:30 UTC`); a dash when the release has not started. */
export function startText(ms: number | null): string {
  if (ms === null || !Number.isFinite(ms)) return '—';
  const iso = new Date(ms).toISOString();
  return `${iso.slice(0, 10)} ${iso.slice(11, 16)} UTC`;
}

/** A flake rate (a fraction of the runs that flaked) as a percentage with one decimal place; a dash when none was measured. */
export function flakeText(rate: number | null): string {
  if (rate === null || !Number.isFinite(rate)) return '—';
  return `${(rate * 100).toFixed(1)}%`;
}
