// The eval reports' words and numbers (PLAN 25.2): the cost and success of the runs per step type and model as a grid,
// the money and the times as text, and a reviewer's line. No DOM here, so node can test it (test/evals-tests.ts).

import type { CostBy, EvalScore, ReviewerScore } from '../api-evals';
import { dollarsText } from './budget-model';

/** The groupings the cost report offers, in the order its selector lists them. */
export const COST_GROUPS: { value: CostBy; label: string }[] = [
  { value: 'step', label: 'Step type' },
  { value: 'model', label: 'Model' },
  { value: 'task', label: 'Task' },
  { value: 'project', label: 'Project' },
];

/** A share as a whole percentage: `0.667` is "67%"; no share is a dash. */
export function percentText(share: number | null | undefined): string {
  return share == null || !Number.isFinite(share) ? '—' : `${Math.round(share * 100)}%`;
}

/** Successes of runs: "2 of 3"; no runs reads "No runs". */
export function successText(successes: number, runs: number): string {
  return runs > 0 ? `${successes} of ${runs}` : 'No runs';
}

/** Micro-dollars per success as dollars, or "no success" without one. */
export function costPerSuccessText(micros: number | null | undefined): string {
  return micros == null ? 'no success' : `${dollarsText(micros)} per success`;
}

/** Milliseconds as a short duration: "850 ms", "4.2 s", "3 min"; none is a dash. */
export function durationText(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return '—';
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.round(ms / 60_000)} min`;
}

/** One cell of the grid: a step type under a model, summed over its seats and engines. */
export interface GridCell {
  step: string;
  model: string;
  runs: number;
  successes: number;
  success_rate: number;
  cost_micros: number;
  cost_per_success_micros: number | null;
  time_per_success_ms: number | null;
  quality: number;
}

/** The scores as a grid: the step types down, the models across. */
export interface Grid {
  steps: string[];
  models: string[];
  cells: Map<string, GridCell>;
}

/** The key of a grid cell. */
export function gridKey(step: string, model: string): string {
  return `${step}\u0000${model}`;
}

/**
 * The scores as a grid. Seats and engines under one step and model add up: runs and successes add, the cost and the
 * time are rebuilt from their per-success figures (per success times successes), and the quality is weighted by runs.
 */
export function grid(scores: EvalScore[]): Grid {
  const sums = new Map<string, { step: string; model: string; runs: number; successes: number; cost: number; wall: number; clean: number }>();
  for (const s of scores) {
    const model = s.model ?? '(no model)';
    const key = gridKey(s.step, model);
    const sum = sums.get(key) ?? { step: s.step, model, runs: 0, successes: 0, cost: 0, wall: 0, clean: 0 };
    sum.runs += s.runs;
    sum.successes += s.successes;
    if (s.cost_per_success_micros != null) sum.cost += s.cost_per_success_micros * s.successes;
    if (s.time_per_success_ms != null) sum.wall += s.time_per_success_ms * s.successes;
    sum.clean += s.quality * s.runs;
    sums.set(key, sum);
  }
  const cells = new Map<string, GridCell>();
  for (const [key, s] of sums) {
    cells.set(key, {
      step: s.step,
      model: s.model,
      runs: s.runs,
      successes: s.successes,
      success_rate: s.runs > 0 ? s.successes / s.runs : 0,
      cost_micros: s.cost,
      cost_per_success_micros: s.successes > 0 ? s.cost / s.successes : null,
      time_per_success_ms: s.successes > 0 ? s.wall / s.successes : null,
      quality: s.runs > 0 ? s.clean / s.runs : 0,
    });
  }
  const unique = (values: string[]) => [...new Set(values)].sort();
  const all = [...cells.values()];
  return {
    steps: unique(all.map((c) => c.step)),
    models: unique(all.map((c) => c.model)),
    cells,
  };
}

/** A reviewer's line: its findings, how they were ruled, its precision and its misses. */
export function reviewerLine(r: ReviewerScore): string {
  const precision = r.precision == null ? 'no findings yet' : `${percentText(r.precision)} precision`;
  return `${r.findings} findings, ${r.confirmed} confirmed, ${r.dropped} dropped · ${precision} · ${r.misses} misses`;
}

/** A benchmark replay's verdict in words: the check's result against the outcome the task had. */
export function benchVerdict(outcome: string, known: string, matched: boolean): string {
  const check = outcome === 'passed' ? 'the check passed' : 'the check failed';
  const agrees = matched ? 'and matches' : 'and does not match';
  return `${check}, ${agrees} the known outcome (${known})`;
}
