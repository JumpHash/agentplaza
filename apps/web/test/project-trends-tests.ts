// The project trends' view model: the metric and target columns across the releases, a release's value in each, the start
// time and the flake rate as text. Bundled by test/tests.ts; it prints "project trends tests: ok (N checks)".

import type { TrendRelease } from '../src/api-trends';
import { flakeText, metricCell, metricNames, sizeCell, startText, targetNames } from '../src/views/trends-model';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

const rows: TrendRelease[] = [
  {
    release: 'rel_1', version: '1.3.0', channel: 'stable', verdict: 'passed',
    metrics: [{ name: 'boot_ms', value: 820 }, { name: 'fps', value: 58.5 }],
    sizes: [{ target: 'linux-x86_64', size: 2048 }], start_ms: 0, flake_rate: 0.125,
  },
  {
    release: 'rel_2', version: '1.3.1', channel: 'stable', verdict: 'failed',
    metrics: [{ name: 'fps', value: 60 }, { name: 'frame_ms', value: 16 }],
    sizes: [{ target: 'linux-x86_64', size: 2150 }, { target: 'macos-arm64', size: 1048576 }], start_ms: null, flake_rate: null,
  },
];

// ---- Columns -------------------------------------------------------------------------------------------------------------

eq(metricNames(rows), ['boot_ms', 'fps', 'frame_ms'], 'the metric columns are the union, in first-seen order');
eq(targetNames(rows), ['linux-x86_64', 'macos-arm64'], 'the target columns are the union, in first-seen order');
eq(metricNames([]), [], 'no releases, no metric columns');

// ---- Cells ---------------------------------------------------------------------------------------------------------------

eq(metricCell(rows[0]!, 'fps'), '58.5', 'a decimal metric keeps its place');
eq(metricCell(rows[1]!, 'boot_ms'), '—', 'a metric the release did not measure is a dash');
eq(sizeCell(rows[0]!, 'linux-x86_64'), '2 KiB', 'an artifact size is in the largest unit it fills');
eq(sizeCell(rows[1]!, 'macos-arm64'), '1 MiB', 'a size of a mebibyte reads in MiB');
eq(sizeCell(rows[0]!, 'macos-arm64'), '—', 'a target the release has no artifact for is a dash');

// ---- Start times and flake rates -----------------------------------------------------------------------------------------

eq(startText(0), '1970-01-01 00:00 UTC', 'a start time is UTC to the minute');
eq(startText(null), '—', 'a release that has not started is a dash');
eq(flakeText(0.125), '12.5%', 'a flake rate is a percentage with one decimal place');
eq(flakeText(0), '0.0%', 'no flakes is 0.0%');
eq(flakeText(null), '—', 'a flake rate the hub did not measure is a dash');

console.log(`project trends tests: ok (${checks} checks)`);
