// The eval reports' API (PLAN 25): what the step sessions cost and how they did, the leaderboard of a step, each reviewer
// model's precision and misses, and the benchmark's replays of archived tasks. The same origin and the same `call` as the
// rest of the console; the types follow the hub's answers (crates/apid/src/evals.rs). Money is in micro-dollars.

import { call } from './api';

/** What a cost report groups its runs by. */
export type CostBy = 'project' | 'task' | 'step' | 'model';

/** One group of the cost report: what its runs cost and how they did. */
export interface CostRow {
  key: string;
  runs: number;
  successes: number;
  success_rate: number;
  quality: number;
  cost_micros: number;
  cost_per_success_micros: number | null;
  wall_ms: number;
  time_per_success_ms: number | null;
  tokens_in: number;
  tokens_out: number;
  tokens_cached: number;
  tool_calls: number;
}

/** How the runs of one step, seat, model and engine did. */
export interface EvalScore {
  step: string;
  seat: string;
  model: string | null;
  engine: string;
  runs: number;
  successes: number;
  success_rate: number;
  cost_per_success_micros: number | null;
  time_per_success_ms: number | null;
  quality: number;
}

/** A reviewer model's record: its findings and how they were ruled, its precision, and what it missed. */
export interface ReviewerScore {
  model: string;
  findings: number;
  confirmed: number;
  dropped: number;
  precision: number | null;
  changes_passed: number;
  missed_findings: number;
  late_failures: number;
  misses: number;
}

/** One replay of an archived task: whether its check passed, the known outcome, and whether they agree. */
export interface BenchRun {
  id: number;
  archive: number;
  outcome: string;
  known: string;
  matched: boolean;
  cost_micros: number;
  wall_ms: number;
  replayed_ms: number;
}

/** The run filter the cost report takes: a match on each field, and an absent field matches any. */
export interface EvalFilter {
  project?: string;
  model?: string;
  seat?: string;
}

/** The query string of the given parameters (absent or empty ones are left out). */
function query(params: Record<string, string | undefined>): string {
  const q = new URLSearchParams();
  for (const [name, value] of Object.entries(params)) if (value) q.set(name, value);
  const text = q.toString();
  return text ? `?${text}` : '';
}

export const evalsApi = {
  costs: (by: CostBy, filter: EvalFilter = {}) =>
    call<CostRow[]>('GET', `/v1/evals/costs${query({ by, project: filter.project, model: filter.model, seat: filter.seat })}`),
  leaderboard: (step?: string) => call<EvalScore[]>('GET', `/v1/evals/leaderboard${query({ step })}`),
  reviewers: () => call<ReviewerScore[]>('GET', '/v1/evals/reviewers'),
  bench: () => call<BenchRun[]>('GET', '/v1/evals/bench'),
};
