// The eval reports' pure logic (PLAN 25.2): the grid of cost and success per step type and model (seats and engines
// under one step and model add up), the words for money, times, shares and reviewers, and the verdict of a replay. Over a
// fixture of scores, as the hub answers them. Bundled with esbuild; it prints "evals tests: ok".

import { COST_GROUPS, benchVerdict, costPerSuccessText, durationText, grid, gridKey, percentText, reviewerLine, successText } from '../src/views/evals-model';
import type { EvalScore, ReviewerScore } from '../src/api-evals';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks += 1;
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) throw new Error(`evals tests: ${what}: expected ${e}, got ${a}`);
}

/** The scores of a fixture: two engines run the build step on one model, and the review step runs on another. */
const scores: EvalScore[] = [
  { step: 'build', seat: 'builder', model: 'mock/agent', engine: 'harness', runs: 3, successes: 2, success_rate: 2 / 3, cost_per_success_micros: 125, time_per_success_ms: 1250, quality: 2 / 3 },
  { step: 'build', seat: 'builder', model: 'mock/agent', engine: 'acp', runs: 1, successes: 0, success_rate: 0, cost_per_success_micros: null, time_per_success_ms: null, quality: 0 },
  { step: 'review', seat: 'reviewer', model: 'mock/echo', engine: 'harness', runs: 2, successes: 2, success_rate: 1, cost_per_success_micros: 10, time_per_success_ms: 200, quality: 1 },
];

// ---- Words and numbers -------------------------------------------------------------------------------------------

eq(percentText(2 / 3), '67%', 'two thirds reads as 67%');
eq(percentText(null), '—', 'no share is a dash');
eq(successText(2, 3), '2 of 3', 'successes of runs');
eq(successText(0, 0), 'No runs', 'no runs');
eq(costPerSuccessText(125), '$0.00 per success', 'micro-dollars per success, as dollars');
eq(costPerSuccessText(2_500_000), '$2.50 per success', 'a dollar figure');
eq(costPerSuccessText(null), 'no success', 'no success, no cost per success');
eq(durationText(850), '850 ms', 'under a second is milliseconds');
eq(durationText(4200), '4.2 s', 'seconds to one decimal');
eq(durationText(180_000), '3 min', 'minutes past a minute');
eq(durationText(null), '—', 'no time is a dash');
eq(COST_GROUPS[0].value, 'step', 'the cost report groups by step type first');
eq(COST_GROUPS.map((g) => g.value), ['step', 'model', 'task', 'project'], 'every grouping the report offers');

// ---- The grid ----------------------------------------------------------------------------------------------------

const g = grid(scores);
eq(g.steps, ['build', 'review'], 'the step types, sorted');
eq(g.models, ['mock/agent', 'mock/echo'], 'the models, sorted');
const build = g.cells.get(gridKey('build', 'mock/agent'));
eq(build?.runs, 4, 'the two engines of a step and model add their runs');
eq(build?.successes, 2, 'and their successes');
eq(build?.success_rate, 0.5, 'the success rate of the sum');
eq(build?.cost_micros, 250, 'the cost is rebuilt from the per-success figures');
eq(build?.cost_per_success_micros, 125, 'the cost per success of the sum');
eq(build?.time_per_success_ms, 1250, 'the time per success of the sum');
eq(build?.quality, 0.5, 'quality is weighted by runs: two clean of four');
const review = g.cells.get(gridKey('review', 'mock/echo'));
eq([review?.success_rate, review?.cost_micros, review?.quality], [1, 20, 1], 'a step with every run successful and clean');
eq(g.cells.get(gridKey('build', 'mock/echo')), undefined, 'no run of the build step on the review model');
eq(grid([]).steps, [], 'no scores, no grid');

// ---- Reviewers and replays ---------------------------------------------------------------------------------------

const reviewer: ReviewerScore = { model: 'mock/x', findings: 4, confirmed: 3, dropped: 1, precision: 0.75, changes_passed: 2, missed_findings: 1, late_failures: 0, misses: 1 };
eq(reviewerLine(reviewer), '4 findings, 3 confirmed, 1 dropped · 75% precision · 1 misses', 'a reviewer in a line');
eq(reviewerLine({ ...reviewer, findings: 0, confirmed: 0, dropped: 0, precision: null, misses: 0 }), '0 findings, 0 confirmed, 0 dropped · no findings yet · 0 misses', 'a reviewer with no finding has no precision yet');
eq(benchVerdict('failed', 'landed', false), 'the check failed, and does not match the known outcome (landed)', 'a replay that disagrees with a landed task');
eq(benchVerdict('passed', 'landed', true), 'the check passed, and matches the known outcome (landed)', 'a replay that agrees');

console.log(`evals tests: ok (${checks} checks)`);
