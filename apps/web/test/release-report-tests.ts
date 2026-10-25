// The qualification report's view model, over the fixture test/fixtures/release-report.json (one passing release with all
// eleven stages, one stage not in depth, a quarantined test and an acceptance). No DOM library is installed, so the report
// page is not rendered here: the model the page draws is checked instead. Bundled by test/tests.ts; it prints
// "release report tests: ok (N checks)".

import type { QualificationReport } from '../src/api-releases';
import { acceptanceRows, changeText, countText, isHubPath, metricText, reportView, stageStateText, stageTone, verdictView } from '../src/views/release-report-model';
import fixtureJson from './fixtures/release-report.json';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

const fixture = fixtureJson as unknown as QualificationReport;
const view = reportView(fixture);

// ---- The verdict and the reasons -----------------------------------------------------------------------------------------

eq(view.verdict, { text: 'Passed', tone: 'ok' }, 'a passing qualification reads Passed, in the ok tone');
eq(view.reasons.length, 2, 'the reasons are listed');
eq(verdictView('failed'), { text: 'Failed', tone: 'danger' }, 'a failed verdict is danger');
eq(verdictView('incomplete'), { text: 'Incomplete', tone: 'warn' }, 'an incomplete verdict is warn');
eq(verdictView(''), { text: 'No verdict yet', tone: 'muted' }, 'an empty verdict says so');

// ---- The stages ----------------------------------------------------------------------------------------------------------

eq(view.stages.length, 11, 'the report has eleven stage rows');
eq(view.stages.map((s) => s.number), [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11], 'the stage rows are in stage order');
const perf = view.stages.find((s) => s.key === 'perf');
eq(perf?.muted, true, 'a stage not in depth is shown muted');
eq(perf?.stateText, 'Not in depth', 'a stage not in depth says so');
eq(perf?.tone, 'muted', 'a stage not in depth has the muted tone');
eq(view.stages.filter((s) => s.muted).length, 1, 'only the stage not in depth is muted');
eq(view.stages[0]?.muted, false, 'a stage in depth is not muted');
eq(view.stages[0]?.stateText, 'Passed', 'a passed stage reads Passed');
const e2e = view.stages.find((s) => s.key === 'e2e');
eq(e2e?.tone, 'info', 'an accepted stage is in the info tone');
eq(e2e?.evidence, [{ label: 'e2e run', href: '/v1/runs/run_e1' }], 'evidence keeps its label and its hub path');
eq(e2e?.findings, [{ label: 'login step retried twice', href: '/v1/findings/f_31' }], 'a stage finding keeps its text and its hub path');
eq(view.stages[0]?.metrics, ['build_seconds 184'], 'a stage metric reads as name and value');
eq(view.stages[0]?.parts.length, 3, 'a stage with parts lists them');
eq(stageStateText('weird_new_state'), 'weird_new_state', 'an unknown stage state reads as it is');
eq(stageTone('passed'), 'ok', 'passed is ok');
eq(stageTone('pending'), 'warn', 'pending is warn');
eq(stageTone('failed'), 'danger', 'failed is danger');

// ---- The depth -----------------------------------------------------------------------------------------------------------

eq(view.depth.short, ['sbom', 'provenance'], 'the short stages are listed');
eq(view.depth.inDepth.length, 11, 'the stages in depth are listed');
eq(view.depth.excluded, [{ stage: 'perf', why: 'no hot path changed since 1.3.2' }], 'an excluded stage carries its why');

// ---- Quarantine, flaky tests, acceptances --------------------------------------------------------------------------------

eq(view.quarantine.map((q) => q.test), ['sync::tests::retries_after_reset'], 'the quarantined test is listed');
eq(view.quarantine[0]?.flaked, '4 flaked runs', 'the flaked runs are counted');
eq(view.quarantine[0]?.fixTask, 'task_904', 'the quarantine names its fix task');
eq(view.flaky, ['sync::tests::retries_after_reset'], 'the flaky tests are listed');
eq(view.acceptances.length, 1, 'the acceptance is listed');
eq(view.acceptances[0]?.reason, 'login retry is a known timing issue in the staging proxy; tracked in fix task task_904', 'the acceptance reason is shown');
eq(view.acceptances[0]?.plannerSays, 'accept: the retry passed on the same build twice', 'the planner answer is shown');
eq(view.acceptances[0]?.secondSays, 'accept: no user-visible failure in the retried runs', 'the second model answer is shown');
eq(view.acceptances[0]?.tokens, '18,420', 'the tokens of an acceptance are counted with commas');

// ---- Deltas, findings, cost, fix tasks -----------------------------------------------------------------------------------

eq(view.comparedWith, '1.3.2', 'the report says which release it was compared with');
eq(view.deltas.map((d) => d.change), ['+4.5%', '-4.1%'], 'the change in percent is signed');
eq(view.deltas[1]?.direction, 'down', 'a fall is a down delta');
eq(view.deltas[0]?.previous, '176', 'the previous value is shown');
eq(view.findings.new, ['login step retried twice under load'], 'the new findings are listed');
eq(view.findings.fixed, ['stale lockfile warning in build'], 'the fixed findings are listed');
eq(view.cost, { tokens: '412,300', labMinutes: '37.5', labRuns: '2' }, 'the cost reads tokens, lab minutes and lab runs');
eq(view.fixTasks, [{ stage: 'e2e', goal: 'make the login step deterministic in staging', task: 'task_904' }], 'the fix tasks are listed');
eq(view.labRuns, ['lab_9a1', 'lab_9a2'], 'the lab runs are listed');
eq(view.reviewRun, 'review_77c', 'the review run is named');

// ---- Empty states --------------------------------------------------------------------------------------------------------

const bare = reportView({ ...fixture, acceptances: [], quarantine: [], flaky: [], deltas: [], compared_with: null, matrices: [], fix_tasks: [] });
eq(bare.acceptances, [], 'no acceptances is an empty list (the page says so)');
eq(bare.quarantine, [], 'no quarantine is an empty list (the page says so)');
eq(bare.deltas, [], 'no deltas is an empty list');
eq(bare.comparedWith, null, 'a report with nothing to compare against says so');
eq(bare.fixTasks, [], 'no fix tasks is an empty list');
eq(acceptanceRows([]), [], 'acceptance rows of none are none');

// ---- Links from the hub --------------------------------------------------------------------------------------------------

eq(isHubPath('/v1/runs/run_b1'), true, 'a hub path is a link');
eq(isHubPath('//evil.test/x'), false, 'a protocol-relative link is not made a link');
eq(isHubPath('/\\evil.test/x'), false, 'a backslash link is not made a link');
eq(isHubPath('https://ci.example.test/run/1'), false, 'an absolute link is not made a link');
eq(isHubPath('javascript:alert(1)'), false, 'a script link is not made a link');

// ---- Numbers -------------------------------------------------------------------------------------------------------------

eq(countText(1234567), '1,234,567', 'a count has a comma every three digits');
eq(countText(12), '12', 'a short count is as it is');
eq(metricText(2.5), '2.5', 'a decimal metric keeps its places');
eq(metricText(184), '184', 'a whole metric is as it is');
eq(changeText(0), '0.0%', 'no change is 0.0%');
eq(changeText(Number.NaN), '—', 'a change the hub did not give is a dash');

console.log(`release report tests: ok (${checks} checks)`);
