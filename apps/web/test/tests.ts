// Pure-logic tests: run with `npm test`.
import { parse, href } from '../src/router';
import { classify, label, type RichCtx, type Span } from '../src/ui/rich';
import richVectors from '../../../crates/apz-rich/tests/vectors.json';
import { TOUR_STEPS, WIZARD_STEPS, fill, keysSet, moveStep, readFlag, writeFlag } from '../src/onboarding/flow';
import { ago, clip, compact } from '../src/ui/components';
import { floorName, familyLabel, roleFamily } from '../src/roles';
import { builtinLaws, diagnosticText, diagnosticsByLine, errorText, isBlank, isOwnerLaw, lawsOfRule, lineSpan, matchesFilter, tierKey, withStrictPermits, type LawFilter } from '../src/views/rules-model';
import type { LawInfo, Settings } from '../src/api';
import { checkRetention, clampQuery, mergeNewest, mergeOlder, spanOf } from '../src/views/conversations';
import { level, membersOf, parentOf, trailOf } from '../src/views/society';
import type { Bond, Conversation, PriceRates, SocietyAgent, UsageCall } from '../src/api';
import { easeOut, niceScale, resample, tweenArray } from '../src/ui/chart';
import { cacheSaving, fixtureCalls, ledgerSeries, ledgerSummary, mergeCalls, pageCalls, rankGroups, areaData, totalsOf, windowOf } from '../src/views/usage';
import { balanceData, barData, creditsOf, creditsUsd, flowsOf, mergeJournal, movesOf, partyOf, sortRows, windowOf as treasuryWindow } from '../src/views/treasury';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import type { Agent, FleetApple, FleetKind, FleetMachine, FleetResponse, FleetStatus } from '../src/api';
import { agentTally, boardOf, gibText, machineFacts, machineSubtitle } from '../src/views/floor';
import { hubOrigin, setupSnippets } from '../src/views/hub-mcp-snippets';
import { ENV_NAME_RE, limitsProblem, matchingPreset, ownRowProblem, specsOfBag } from '../src/views/mcp-common';
import { wrapIde, type IdeGlue } from '../src/ide/wasm';
import { activitySplit, authorHue, buildChat, currentTask, filterPalette, formatMs, matchScore, mentionsOf, rowStatus, stepLabel, type PaletteItem } from '../src/ide/view';
import { groupByProject, projectOf, sharedNames, startAgent } from '../src/ide/groups';
import type { Entry, Project, Task } from '../src/api';
import { applyBuffer, badgeOf, diffMark, dirsOf, fuzzyScore, isViewed, languageFor, lineCounts, onRemote, openBuffer, parentDir, parseViewed, rankPaths, viewedKey, withViewed } from '../src/ide/model';
import type { FileChange } from '../src/ide/types';
import { CENTRE_MIN, clampSize, defaultLayout, fitSize, isNarrow, LEFT_SIZE, leftViewFor, originFallback, parseLayout, RIGHT_SIZE, rightTabFor, serializeLayout } from '../src/ide/layout';
import { MAX_TERMINALS, attachFrame, base64ToBytes, bytesToBase64, clearSession, inputFrame, killFrame, nextOffset, openFrame, parseServerFrame, plainText, readSession, reconnectDelay, resizeFrame, sessionKey, splitInput, storedTerminalCount, terminalSocketUrl, writeSession, type KeyValueStore } from '../src/views/ide-terminal-model';

let failures = 0;
// Each section checks its own failures and reports them, then the run goes on to the next section (so one failing
// section does not hide the others). The failed sections are listed at the end, and the run exits 1 if any failed.
const failedSections: string[] = [];
// A layout that throws is a failed check, not the end of the run: the error is reported and the caller moves on.
function laid<T>(run: () => T, name: string): T | null {
  try {
    return run();
  } catch (e) {
    eq((e as Error).message, '', `${name}: the layout throws`);
    return null;
  }
}
function sectionEnd(okLine: string): void {
  if (failures) {
    console.error(`${failures} failure(s)`);
    failedSections.push(okLine.replace(/: ok$/, ''));
    failures = 0;
  } else console.log(okLine);
}
function eq(actual: unknown, expected: unknown, name: string): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    failures++;
    console.error(`FAIL ${name}: ${JSON.stringify(actual)} !== ${JSON.stringify(expected)}`);
  }
}


eq(parse('#/agents/105'), { screen: 'agents', params: ['105'] }, 'route with a param');
eq(parse(''), { screen: 'overview', params: [] }, 'empty route is the overview');
eq(href('tasks', 't 1'), '#/tasks/t%201', 'href encodes params');
eq(ago(1000, 31_000), '30s ago', 'ago seconds');
eq(clip('abcdef', 4), 'abc…', 'clip');
eq(compact(12_345), '12k', 'compact thousands');
eq(floorName({ cid: 7, name: '' }), 'agent 7', 'floor name falls back to the cid');
eq(floorName({ cid: 7, name: '  ' }), 'agent 7', 'floor name: whitespace counts as empty');

// Rules: the screen's logic (the view itself is checked in the browser).
{
  const law = (id: string, tier: string, origin: 'built-in' | 'owner', ruleId = '', file = ''): LawInfo => ({ id, tier, origin, rule_id: ruleId, file } as LawInfo);
  const laws = [law('S-1', 'statute', 'built-in'), law('C-1', 'constitution', 'built-in'), law('O-1', 'owner', 'owner', 'R-001'), law('X-1', '', 'built-in', '', 'operations.jhrl')];
  eq(tierKey({ tier: 'statute', file: '' }), 'statute', 'rules: a statute tier');
  eq(tierKey({ tier: '', file: 'constitution.jhrl' }), 'constitution', 'rules: the tier falls back to the file');
  eq(tierKey({ tier: 'weird', file: 'misc.jhrl' }), 'other', 'rules: an unknown tier and file is other');
  eq([isOwnerLaw(laws[0]), isOwnerLaw(laws[2])], [false, true], 'rules: built-in and owner laws are told apart');
  eq(builtinLaws(laws, 'all').map((l) => l.id), ['S-1', 'C-1', 'X-1'], 'rules: built-in statutes exclude owner laws');
  eq(builtinLaws(laws, 'statute').map((l) => l.id), ['S-1'], 'rules: the statute tier filter');
  eq(builtinLaws(laws, 'operations').map((l) => l.id), ['X-1'], 'rules: the operations tier filter reads the file');
  eq(lawsOfRule(laws, 'R-001').map((l) => l.id), ['O-1'], 'rules: a rule declares its own laws');
  eq(lawsOfRule(laws, 'R-002'), [], 'rules: a rule with no laws');
  const all: LawFilter = { q: '', effect: 'all', sev: 'any', tier: 'all' };
  const sample = { ...laws[0], id: 'S-9', title: 'Board posts', statement: 'no deletes', ops: ['board.delete'], effect: 'forbid', severity: 'major' } as LawInfo;
  const pass = (over: Partial<LawFilter>): boolean => matchesFilter(sample, { ...all, ...over });
  eq(
    [pass({}), pass({ q: ' DELETE ' }), pass({ q: 'nothing' }), pass({ effect: 'permit' }), pass({ effect: 'forbid' }), pass({ sev: 'major' }), pass({ sev: 'minor' }), pass({ tier: 'statute' }), pass({ tier: 'operations' })],
    [true, true, false, false, true, true, false, true, false],
    'rules: the statute filters (search, effect, severity, tier)',
  );

  eq(lineSpan('ab\ncde\nf', 2), { start: 3, end: 6 }, 'rules: the span of line 2');
  eq(lineSpan('ab\ncde', 1), { start: 0, end: 2 }, 'rules: the span of line 1');
  eq(lineSpan('ab', 9), { start: 0, end: 2 }, 'rules: a line past the end is the last');
  eq(lineSpan('', 1), { start: 0, end: 0 }, 'rules: an empty source has an empty line');
  eq(Array.from(diagnosticsByLine([{ file: '', line: 2, column: 1, message: 'a' }, { file: '', line: 2, column: 4, message: 'b' }]).entries()), [[2, 'a\nb']], 'rules: messages per line');
  eq(diagnosticText({ file: '', line: 3, column: 5, message: 'bad' }), 'Line 3, column 5: bad', 'rules: a diagnostic reads as a line and column');

  const settings = { default_model: 'm', dev_models: true, laws: { strict_permits: false }, fetch_block: [] } as unknown as Settings;
  const on = withStrictPermits(settings, true);
  eq([on.laws?.strict_permits, 'dev_models' in on], [true, false], 'rules: strict permits on, dev_models dropped');
  eq([settings.laws?.strict_permits, 'dev_models' in settings], [false, true], 'rules: the settings given are not changed');
  eq(withStrictPermits({ default_model: 'm' } as unknown as Settings, false).laws, { strict_permits: false }, 'rules: strict permits set when the settings had none');

  eq([isBlank('  '), isBlank(' a ')], [true, false], 'rules: a blank title or reason');
  eq(errorText(new Error('nope')), 'nope', 'rules: an error reads as its message');
  eq(errorText('plain'), 'plain', 'rules: a non-error reads as text');
}
eq(floorName({ cid: 7, name: 'Ada' }), 'Ada', 'floor name keeps a chosen name');

// Conversations: paging keeps what was read, and retention and search are checked as typed.
const exchange = (n: number): Conversation => ({ id: `c${n}`, from_cid: 1, to_cid: 2, from_role: 'a', to_role: 'b', text: `m${n}`, created_at: n, delivered: true });
const ids = (s: { items: Conversation[] }): string[] => s.items.map((c) => c.id);
const exchanges = (from: number, to: number): Conversation[] => Array.from({ length: to - from + 1 }, (_, i) => exchange(from + i));

eq(ids(spanOf([exchange(3), exchange(1), exchange(2)], null)), ['c1', 'c2', 'c3'], 'spans sort oldest first');
eq(
  mergeNewest({ items: exchanges(1, 5), next: 0 }, spanOf(exchanges(4, 6), 3)),
  { items: exchanges(1, 6), next: 0 },
  'a refresh keeps the older pages already read',
);
eq(
  mergeNewest({ items: exchanges(5, 6), next: 4 }, spanOf(exchanges(4, 6), 3)),
  { items: exchanges(4, 6), next: 3 },
  'a refresh with nothing older read takes the fresh cursor',
);
eq(
  mergeOlder({ items: exchanges(4, 5), next: 3 }, spanOf(exchanges(2, 3), 1)),
  { items: exchanges(2, 5), next: 1 },
  'an older page is prepended',
);
eq(ids(mergeOlder({ items: exchanges(4, 5), next: 3 }, spanOf(exchanges(3, 4), 2))), ['c3', 'c4', 'c5'], 'an overlapping older page is not duplicated');
eq(checkRetention('4096', '30').errors, {}, 'retention: the defaults are valid');
eq(checkRetention(' 100 ', '0').errors, {}, 'retention: the lowest keep and forever are valid');
eq(checkRetention('100000', '3650').errors, {}, 'retention: the highest limits are valid');
eq(Object.keys(checkRetention('99', '30').errors), ['max'], 'retention: below the minimum keep');
eq(Object.keys(checkRetention('100001', '30').errors), ['max'], 'retention: above the maximum keep');
eq(Object.keys(checkRetention('1e3', '30').errors), ['max'], 'retention: not a whole number');
eq(Object.keys(checkRetention('4096', '3651').errors), ['days'], 'retention: days above the maximum');
eq(Object.keys(checkRetention('4096', '-1').errors), ['days'], 'retention: negative days');
eq(clampQuery('  hi  '), 'hi', 'search is trimmed');
eq(clampQuery('a'.repeat(300)).length, 200, 'search is capped at 200 bytes');
eq(clampQuery('é'.repeat(150)).length, 100, 'the cap counts bytes, on whole characters');

// Society: families, groups and their levels.
const ag = (cid: number, role: string): SocietyAgent => ({ cid, name: `a${cid}`, role, reputation: 50, confidence: 0.5, wallet: 0 });
const bond = (from: number, to: number, strength: number, e: number): Bond => ({
  from, to, kind: 'met', since_ms: 0, strength, interactions: 0, last_ms: 0, note: null, blocked: false,
  opinion: { r: 0, s: 0, b: 0, d: 0, u: 0, e, updated_ms: 0 },
});
eq(roleFamily('builder'), 'build', 'family: builder is build');
eq(roleFamily('mayor-office'), 'command', 'family: mayor office is command');
eq(roleFamily('security-reviewer'), 'operations', 'family: security reviewer is operations');
eq(roleFamily('planner'), 'knowledge', 'family: planner is knowledge');
eq(roleFamily('auditor'), 'money', 'family: auditor is money');
eq(roleFamily('my-custom-role'), 'other', 'family: a custom role is other');
eq(roleFamily('breeder'), 'other', 'family: a role outside the list is other');
eq(roleFamily('constructor'), 'other', 'family: an Object.prototype name is other');
eq(roleFamily('builder', 'orchestrator'), 'command', 'family: the orchestrator tier is command');
eq(roleFamily('worker-x', 'worker'), 'other', 'family: a worker tier keeps its role');
eq(familyLabel('money'), 'Money', 'family label');

const crew = [ag(1, 'builder'), ag(2, 'architect'), ag(3, 'finance'), ag(4, 'planner')];
const cross = [bond(1, 3, 4, 0.8), bond(2, 3, 3, 0.4), bond(3, 1, 1, 0.6), bond(1, 2, 5, 0.9)];
const top = level(crew, cross, { family: null, role: null }, true);
eq(top.marks.map((m) => m.key), ['f:build', 'f:knowledge', 'f:money'], 'top level: one box per family');
eq(top.edges.length, 1, 'edges: the one build-money edge (the build-build bond is inside a box; the planner has no contacts)');
const buildMoney = top.edges.find((e) => e.from === 'f:build' && e.to === 'f:money');
eq(buildMoney?.strength, 8, 'edge aggregation: summed strength of the cross contacts');
eq(Math.round((buildMoney?.trust ?? 0) * 100), 60, 'edge aggregation: mean trust of the cross contacts');
eq(buildMoney?.bonds.length, 3, 'edge aggregation: all three cross bonds in one edge');

const flat = level(crew, cross, { family: null, role: null }, false);
eq(flat.marks.map((m) => m.key), ['a:1', 'a:2', 'a:3', 'a:4'], 'flat level: every agent is its own mark');
eq(flat.edges.length, 4, 'flat level: one edge per bond, direction kept');

const team = [ag(1, 'builder'), ag(2, 'reviewer'), ag(3, 'reviewer'), ag(4, 'finance')];
const inBuild = level(team, [], { family: 'build', role: null }, true);
eq(inBuild.marks.map((m) => m.key), ['a:1', 'r:build/reviewer', 'f:money'], 'open family: roles, a single-agent role is its agent');
eq(inBuild.marks.find((m) => m.key === 'r:build/reviewer')?.members.length, 2, 'open family: the role box holds its agents');
const inRole = level(team, [], { family: 'build', role: 'reviewer' }, true);
eq(inRole.marks.map((m) => m.key), ['a:1', 'a:2', 'a:3', 'f:money'], 'open role: its agents are drawn one by one');
eq(membersOf(team, 'r:build/reviewer').map((a) => a.cid), [2, 3], 'membersOf: a role key');
eq(membersOf(team, 'f:build').map((a) => a.cid), [1, 2, 3], 'membersOf: a family key');

const at = { family: 'build', role: 'builder' } as const;
eq(trailOf(at).map((c) => c.label), ['All families', 'Build', 'Builder'], 'breadcrumb: all families, family, role');
eq(parentOf(at), { family: 'build', role: null }, 'back from a role goes to its family');
eq(parentOf({ family: 'build', role: null }), { family: null, role: null }, 'back from a family goes to the top');

// Charts: the scale, resampling and tweening that every chart animates through.
eq(niceScale(0), [0, 1], 'nice scale: nothing to show is a unit axis');
eq(niceScale(7), [0, 2, 4, 6, 8], 'nice scale: round steps that reach the max');
eq(resample([1, 2, 3], 5), [1, 1.5, 2, 2.5, 3], 'resample: linear between points');
eq(resample([], 3), [0, 0, 0], 'resample: no values is all zeros');
eq(resample([4], 2), [4, 4], 'resample: one value repeats');
eq(tweenArray([0, 10], [10, 0], 0.5), [5, 5], 'tween: halfway between aligned arrays');
eq(tweenArray([1], [1, 3], 1), [1, 3], 'tween: a missing entry counts as zero');
eq([easeOut(0), easeOut(1)], [0, 1], 'ease out: the ends are fixed');

// Usage: the ledger's sums, the series buckets, the paging rule and the cache saving.
const call = (id: number, at: number, over: Partial<UsageCall> = {}): UsageCall => ({
  id, at_ms: at, cid: 1, role: 'builder', model: 'm/a', provider: 'p', input: 100, output: 10, cache_read: 0, cache_write: 0,
  cost_usd: null, est_usd: 0.5, task: null, via: 'task', ...over,
});
const HR = 3_600_000;
const sample = [call(1, 0.5 * HR, { cost_usd: 0.2 }), call(2, 1.5 * HR, { model: 'm/b', cid: 2, cache_read: 300 }), call(3, 2.5 * HR, { cache_read: 100 })];
const sum = totalsOf(sample);
eq([sum.calls, sum.input, sum.usd, sum.cost_usd, sum.est_usd], [3, 300, 0.2 + 0.5 + 0.5, 0.2, 1.5], 'totals: usd is reported where known, else the estimate');
eq(totalsOf([call(1, 0, { input: 0, cache_read: 0 })]).cache_hit_rate, 0, 'hit rate: no input is zero, not NaN');
eq(windowOf('24h', 1e9), { from: 1e9 - 24 * HR, to: 1e9, bucket: 'hour' }, 'window: 24 h ends now, hourly buckets');
eq(windowOf('90d', 1e9).bucket, 'day', 'window: 90 d uses daily buckets');

const series = ledgerSeries(sample, 0, 3 * HR, 'hour', 'model');
eq(series.points.map((p) => p.totals.calls), [1, 1, 1], 'series: one bucket per hour, each call in its own');
eq(Object.keys(series.points[1]?.groups ?? {}), ['m/b'], 'series: the group key is the chosen grouping');
eq(ledgerSeries([], 0, 2 * HR, 'hour', 'agent').points.length, 2, 'series: empty buckets are kept');
eq(ledgerSummary(sample, 0, 3 * HR).by_model.map((m) => m.key), ['m/a', 'm/b'], 'summary: groups sort by spend');
eq(ledgerSummary(sample, 0, 3 * HR).unpriced, [], 'summary: priced calls are not listed as unpriced');

const ranked = rankGroups(series, 'usd');
eq(ranked.length, 2, 'ranking: every group that spent');
const many = ledgerSeries(Array.from({ length: 8 }, (_, i) => call(`g${i}`, 0.5 * HR, { model: `m/${i}`, est_usd: 8 - i })), 0, HR, 'hour', 'model');
const manyRanked = rankGroups(many, 'usd');
const area = areaData(many, manyRanked, new Set([manyRanked[0] ?? '']), 'usd', (k) => k);
eq(area.layers.map((l) => l.key), [...manyRanked.slice(1, 7), '~other'], 'area: six groups, the rest in Other, hidden groups out');
eq(area.layers.at(-1)?.values, [manyRanked.slice(7).reduce((s, k) => s + (many.points[0]?.groups[k]?.usd ?? 0), 0)], 'area: Other sums the rest');
eq(area.labels.length, 1, 'area: one label per bucket');

const prices = new Map<string, PriceRates | null>([['m/a', { input: 3, output: 15, cache_read: 0.3, cache_write: null, source: 'litellm' }]]);
const saving = cacheSaving([{ key: 'm/a', totals: totalsOf([call(1, 0, { cache_read: 1_000_000 })]) }], prices);
eq(saving, { usd: 2.7, complete: true }, 'cache saving: cache reads times the input-minus-read price gap');
eq(cacheSaving([{ key: 'm/z', totals: totalsOf([call(1, 0, { cache_read: 5 })]) }], prices).complete, false, 'cache saving: an unpriced model makes it partial');

const merged = mergeCalls([call(1, 5 * HR)], [call(1, 5 * HR), call(2, 9 * HR)]);
eq(merged.map((c) => c.id), [2, 1], 'merge: newest first, no repeats');

const many5 = [call(5, 5), call(4, 4), call(3, 3), call(2, 2), call(1, 1)];
const page1 = pageCalls(many5, { limit: 2 });
eq([page1.calls.map((c) => c.id), page1.total, page1.next_before], [[5, 4], 5, 4], 'paging: the first page and its cursor');
const page2 = pageCalls(many5, { limit: 2, before: page1.next_before ?? undefined });
eq([page2.calls.map((c) => c.id), page2.next_before], [[3, 2], 2], 'paging: the next page is strictly older');
eq(pageCalls(many5, { limit: 9, model: 'nope' }).calls, [], 'paging: a filter with no match is empty');

const fx = fixtureCalls(1e12, 2);
eq(fx.length > 0 && fx.every((c, i) => i === 0 || (fx[i - 1]?.at_ms ?? 0) >= c.at_ms), true, 'fixture: calls are newest first');
eq(JSON.stringify(fixtureCalls(1e12, 2).slice(0, 5)), JSON.stringify(fx.slice(0, 5)), 'fixture: deterministic');

// Hardware models (assets/hardware): every file validates, the index lists every file, the loader builds them.
eq(parse('#/hardware-lab?model=exterior/workstation&gpus=1'), { screen: 'hardware-lab', params: [] }, 'a query is not part of the route');

// Treasury: the range window, flow totals, chart data, journal paging merge and the party labels.
const TDAY = 86_400_000;
const NOW = 100 * TDAY;
eq(treasuryWindow('7d', NOW, 0), { from: NOW - 7 * TDAY, to: NOW, bucket: 'hour' }, 'treasury 7 d is hourly');
eq(treasuryWindow('30d', NOW, 0), { from: NOW - 30 * TDAY, to: NOW, bucket: 'day' }, 'treasury 30 d is daily');
eq(treasuryWindow('all', NOW, NOW - 10 * TDAY), { from: NOW - 10 * TDAY, to: NOW, bucket: 'day' }, 'treasury all starts at the first account');
eq(treasuryWindow('all', NOW, NOW + 5), { from: NOW - TDAY, to: NOW, bucket: 'day' }, 'treasury all with a future start spans a day');

const tp = (at: number, over: Record<string, number> = {}) => ({
  at_ms: at, end_ms: at + TDAY, entries: 2, minted: 0, escrowed: 0, paid: 0, refunded: 0, fined: 0, spent: 0,
  by_kind: {}, plaza_budget_end: 100, circulating_end: 20, treasury_end: 3, ...over,
});
const pts = [tp(0, { minted: 50, paid: 5, spent: 2 }), tp(TDAY, { minted: 10, escrowed: 7, fined: 1, refunded: 4, spent: 3, entries: 3 })];
eq(flowsOf(pts), { entries: 5, minted: 60, escrowed: 7, paid: 5, refunded: 4, fined: 1, spent: 5 }, 'flow totals sum over buckets');
eq(flowsOf([]), { entries: 0, minted: 0, escrowed: 0, paid: 0, refunded: 0, fined: 0, spent: 0 }, 'flow totals of no buckets are zero');
const bd = barData(pts, (at) => `t${at / TDAY}`);
eq(bd.labels, ['t0', 't1'], 'bars are labelled per bucket');
eq(bd.layers.map((l) => l.key), ['minted', 'escrowed', 'paid', 'refunded', 'fined', 'spent'], 'bars: one layer per kind, in order');
eq(bd.layers[0]?.values, [50, 10], 'bars: minted values per bucket');
const ad = balanceData(pts, (at) => `t${at / TDAY}`);
eq(ad.layers.map((l) => [l.key, l.values]), [['plaza', [100, 100]], ['circulating', [20, 20]], ['treasury', [3, 3]]], 'balances: plaza, circulating, treasury');

const je = (id: number, over: Record<string, unknown> = {}) => ({ id, kind: 'spend', amount: 1, created_at: id, memo: '', posted_by: 'x', postings: [], task_id: null, root_task: null, ...over });
const j1 = [je(5), je(4)];
eq(mergeJournal(j1, [je(4), je(3)]).map((e) => e.id), [5, 4, 3], 'journal: an older page extends the list without repeats');
eq(mergeJournal([je(4), je(3)], [je(6), je(5)]).map((e) => e.id), [6, 5, 4, 3], 'journal: a newer head page goes first');
eq(mergeJournal([], [je(2), je(1)]).map((e) => e.id), [2, 1], 'journal: the first page is newest first');
eq(mergeJournal([je(4, { memo: 'old' })], [je(4, { memo: 'new' })])[0]?.memo, 'new', 'journal: the fresher copy of an id wins');

eq(sortRows(['b', 'a', 'c'], (s) => s, 1), ['a', 'b', 'c'], 'sort strings ascending');
eq(sortRows([3, 10, 2], (n) => n, -1), [10, 3, 2], 'sort numbers descending');

const names = new Map([[7, 'Della']]);
eq(partyOf({ account: 1, amount: -5, kind: 'wallet', owner_ref: '7' }, names), 'Della', 'a wallet is its agent');
eq(partyOf({ account: 2, amount: 5, kind: 'town_budget', owner_ref: 'plaza' }, names), 'Plaza budget', 'the plaza budget account');
eq(partyOf({ account: 3, amount: -5, kind: 'task_escrow', owner_ref: 't9' }, names), 'Escrow t9', 'an escrow names its task');
eq(movesOf({ id: 1, kind: 'refund', amount: 500, created_at: 0, memo: '', posted_by: '', task_id: 't9', root_task: null, postings: [
  { account: 12, amount: -500, kind: 'task_escrow', owner_ref: 't9' }, { account: 2, amount: 500, kind: 'town_budget', owner_ref: 'plaza' }] }, names),
  { from: 'Escrow t9', to: 'Plaza budget' }, 'a refund moves from escrow to the plaza budget');
eq(Math.abs(creditsUsd(1000) - 1) < 1e-9, true, 'a thousand credits are a dollar');
eq(creditsOf(0.5), 500, 'half a dollar of model spend is five hundred credits');

// The board plaza's machine mapping and room lookup (floor.ts).
const wsBoard = boardOf({ kind: 'workstation', vendor: null, model: null, apple: null, gpus: [{}] });
eq(wsBoard, { name: 'board/workstation', params: { gpus: 1, studio: 0 } }, 'board plaza: a workstation with one GPU shows the workstation board');
const appleBoard = boardOf({ kind: 'laptop', vendor: 'apple', model: 'MacBook Pro', apple: { chip: 'M3', p_cores: 4, e_cores: 4, gpu_cores: 10, neural_cores: 16, unified_memory_gib: 16 }, gpus: [] });
eq(appleBoard, { name: 'board/apple-soc', params: { gpus: 0, studio: 0, p_cores: 4, e_cores: 4, gpu_cores: 10, memory_gib: 16 } }, 'board plaza: an Apple laptop shows the Apple SoC with its cores and memory');
eq(boardOf({ kind: 'workstation', vendor: 'apple', model: 'Mac Studio', apple: null, gpus: [] }).params.studio, 1, 'board plaza: a Studio is flagged');
// The Machines view's pure parts (floor-popup.ts): the popup's text.
eq(gibText(16384), '16 GiB', 'machines text: whole GiB');
eq(gibText(16000), '15.6 GiB', 'machines text: one decimal when not whole');
eq(machineSubtitle({ kind: 'laptop', model: 'MacBook Pro (M3 Max)' }), 'laptop · MacBook Pro (M3 Max)', 'machines subtitle: kind and model');
eq(machineSubtitle({ kind: 'edge', model: null }), 'edge', 'machines subtitle: no model, the kind alone');
eq(agentTally(0, 0, 'online'), 'No agents', 'machines agents: none');
eq(agentTally(3, 2, 'online'), '3 agents: 2 working, 1 idle', 'machines agents: the split while online');
eq(agentTally(1, 0, 'offline'), '1 agent on a machine that is offline', 'machines agents: offline machine states its status');
const rig: FleetMachine = {
  id: 'n0', name: 'zarathos-pc', role: 'hub', kind: 'workstation', vendor: 'asus', model: 'ASUS ROG', apple: null, status: 'online',
  arch: 'x86_64', os: 'linux', backend: 'docker', cpu: { model: 'AMD Ryzen 9 5950X', cores: 16, threads: 32 }, memory_mib: 65536,
  gpus: [{ name: 'NVIDIA GeForce RTX 3090', vendor: 'nvidia', memory_mib: 24576 }], storage_gib: 2000, agents: [1, 2], working: 1,
  stats: null, last_seen_ms: 0,
};
const facts = machineFacts(rig);
eq(facts[0], ['CPU', 'AMD Ryzen 9 5950X'], 'machines facts: the CPU model first');
eq(facts[1], ['Cores', '16 cores · 32 threads'], 'machines facts: cores and threads');
eq(facts[2], ['Memory', '64 GiB'], 'machines facts: memory in GiB');
eq(facts[3], ['GPUs', 'NVIDIA GeForce RTX 3090 · 24 GiB'], 'machines facts: GPU name and memory');
eq(machineFacts({ ...rig, gpus: [] })[3], ['GPUs', 'none'], 'machines facts: no GPU reads none');
eq(machineFacts({ ...rig, gpus: [{ name: 'NVIDIA GeForce RTX 3090', vendor: 'NVIDIA', memory_mib: null as unknown as number }] })[3], ['GPUs', 'NVIDIA GeForce RTX 3090'], 'machines facts: a GPU of unknown memory shows its name alone');
eq(gibText(null), '—', 'machines text: unknown memory is a dash');

sectionEnd('web tests: ok');

// The page's Content-Security-Policy: the engine's WebAssembly compiles under 'wasm-unsafe-eval' only, never under
// 'unsafe-eval'. The hub serves no CSP header of its own (noded and apid set none), so the page's meta tag is the policy.
{
  const html = readFileSync(join(process.cwd(), 'src/index.html'), 'utf8');
  const policy = /<meta http-equiv="Content-Security-Policy" content="([^"]*)"/.exec(html)?.[1] ?? '';
  const script = /script-src([^;]*)/.exec(policy)?.[1] ?? '';
  eq(script.includes("'wasm-unsafe-eval'"), true, "csp: script-src allows 'wasm-unsafe-eval' (the engine's wasm)");
  eq(policy.includes("'unsafe-eval'"), false, "csp: the policy never allows 'unsafe-eval'");
  eq(script.includes("'unsafe-inline'"), false, "csp: script-src allows no inline script");
}

// The hub-as-a-server tab's setup text: the Claude Code command carries the URL and the bearer header, and the
// desktop block runs the stdio bridge against the hub's origin, never with the token in it.
{
  const url = 'https://hub.example.test/mcp';
  const s = setupSnippets(url, 'apz_mcp_test');
  eq(s.claudeCode, 'claude mcp add --transport http agentplaza https://hub.example.test/mcp --header "Authorization: Bearer apz_mcp_test"', 'hub mcp: the Claude Code command carries the URL and the header');
  eq(hubOrigin(url), 'https://hub.example.test', 'hub mcp: the origin is the URL without its /mcp path');
  eq(JSON.parse(s.desktop).mcpServers.agentplaza.args, ['mcp', 'serve', '--hub', 'https://hub.example.test'], 'hub mcp: the desktop block serves the hub origin');
  eq(s.desktopLogin, 'agentplaza mcp login --hub https://hub.example.test', 'hub mcp: the login line names the hub origin');
  eq(s.desktop.includes('apz_mcp_test'), false, 'hub mcp: the desktop block never carries the token');
  eq(s.generic, 'URL: https://hub.example.test/mcp\nAuthorization: Bearer apz_mcp_test', 'hub mcp: the generic setup is the URL and the header');
}
{
  // Open mode: no token, so no header and no login line.
  const o = setupSnippets('https://hub.example.test/mcp', null);
  eq(o.claudeCode, 'claude mcp add --transport http agentplaza https://hub.example.test/mcp', 'hub mcp: open mode drops the header');
  eq(o.desktopLogin, null, 'hub mcp: open mode has no login line');
  eq(o.generic, 'URL: https://hub.example.test/mcp', 'hub mcp: open mode generic setup is the URL alone');
}
sectionEnd('hub mcp tests: ok');

// The MCP wizard's pure helpers: the limit bounds the hub checks, the own-row names, and matching a server to its
// preset by transport (not by id).
{
  const ok = { timeout_s: 60, max_output_kb: 256, idle_stop_min: 10 };
  eq(limitsProblem(ok), '', 'mcp limits: the defaults are in bounds');
  eq(limitsProblem({ ...ok, timeout_s: 0 }), 'Timeout: enter a whole number from 1 to 600.', 'mcp limits: a timeout below 1 is named');
  eq(limitsProblem({ ...ok, max_output_kb: 4097 }), 'Output size: enter a whole number from 1 to 4096.', 'mcp limits: an output size above 4096 is named');
  eq(limitsProblem({ ...ok, idle_stop_min: 1.5 }), 'Idle stop: enter a whole number from 1 to 1440.', 'mcp limits: a fraction is refused');
  eq(limitsProblem({ ...ok, timeout_s: NaN }), 'Timeout: enter a whole number from 1 to 600.', 'mcp limits: an empty number (NaN) is refused');
  eq(limitsProblem({ timeout_s: 600, max_output_kb: 4096, idle_stop_min: 1440 }), '', 'mcp limits: the bounds themselves are allowed');
  eq(ENV_NAME_RE.test('GITHUB_TOKEN'), true, 'mcp own rows: an upper-case env name is accepted');
  eq(ENV_NAME_RE.test('github_token'), false, 'mcp own rows: a lower-case env name is refused');
  eq(ownRowProblem('GITHUB_TOKEN', false), '', 'mcp own rows: a valid env name has no problem');
  eq(ownRowProblem('1TOKEN', false) !== '', true, 'mcp own rows: an env name starting with a digit is refused');
  eq(ownRowProblem('Authorization', true), '', 'mcp own rows: a header name is accepted');
  eq(ownRowProblem('Auth_Header', true) !== '', true, 'mcp own rows: an underscore in a header name is refused');
  const catalog = [
    { id: 'files', name: 'Files', description: '', transport: 'stdio' as const, image: 'ghcr.io/x/fs:1', command: [], url: null, env: [], headers: [], docs: '', status: '', note: '' },
    { id: 'search', name: 'Search', description: '', transport: 'http' as const, image: null, command: [], url: 'https://mcp.example.test/sse', env: [], headers: [], docs: '', status: '', note: '' },
  ];
  eq(matchingPreset({ kind: 'stdio', image: 'ghcr.io/x/fs:1' }, catalog)?.id, 'files', 'mcp match: a container matches its preset by image');
  eq(matchingPreset({ kind: 'http', url: 'https://mcp.example.test/sse' }, catalog)?.id, 'search', 'mcp match: a URL matches its preset by URL');
  eq(matchingPreset({ kind: 'stdio', image: 'ghcr.io/other/fs:1' }, catalog), undefined, 'mcp match: another image matches nothing');
  eq(specsOfBag({ TOKEN: { secret: 'GH_TOKEN' }, ROOT: { value: '/data' } }), [
    { name: 'TOKEN', secret: true, required: false, help: '', format: null },
    { name: 'ROOT', secret: false, required: false, help: '', format: null },
  ], 'mcp own specs: a bag becomes one spec per name, secret when it is a vault reference');
  eq(specsOfBag({ Authorization: { secret: 'WEB_KEY', format: 'Bearer {}' } })[0]?.format, 'Bearer {}', 'mcp own specs: a header format is kept');
}
sectionEnd('mcp wizard helper tests: ok');


// The first-run wizard and the tour: steps, copy filling, flags that survive a refused store.
eq(moveStep(0, -1, 4), 0, 'wizard: Back stops at the first step');
eq(moveStep(3, 1, 4), 3, 'wizard: Next stops at the last step');
eq(moveStep(1, 1, 8), 2, 'tour: Next moves one step');
eq(WIZARD_STEPS.length <= 5, true, 'wizard: at most five steps');
eq(WIZARD_STEPS[2]?.id, 'deps', 'wizard: the dependencies step is third, after the hub');
eq(TOUR_STEPS.length >= 6 && TOUR_STEPS.length <= 8, true, 'tour: six to eight steps');
eq(fill('{n} of {m} providers have a key', { n: 1, m: 3 }), '1 of 3 providers have a key', 'copy: placeholders fill');
eq(fill('{n} of {m}', { n: 2 }), '2 of {m}', 'copy: a missing value stays visible');
eq(keysSet([{ credential: 'missing' }, { credential: 'secret' }, { credential: 'env' }]), { n: 2, m: 3 }, 'keys: a missing credential does not count');
const memory = new Map<string, string>();
const fakeStore = { getItem: (k: string) => memory.get(k) ?? null, setItem: (k: string, v: string) => void memory.set(k, v) };
eq(readFlag(fakeStore, 'setup'), false, 'flags: unset at first');
writeFlag(fakeStore, 'setup');
eq(readFlag(fakeStore, 'setup'), true, 'flags: set after a write');
eq(readFlag(fakeStore, 'tour'), false, 'flags: each flag on its own key');
const refusing = { getItem: () => { throw new Error('denied'); }, setItem: () => { throw new Error('denied'); } };
eq(readFlag(refusing, 'setup'), false, 'flags: a refused read is not set');
writeFlag(refusing, 'setup'); // must not throw
eq(readFlag(null, 'tour'), false, 'flags: no store at all is not set');
sectionEnd('onboarding tests: ok');

// The IDE model's typed wrapper: the JSON in and out, over a glue that records what it is given. The harness cannot load
// the built wasm; apz-ide-web's own tests cover the JSON layer, and the wasm was checked by loading the built glue.
{
  const seen: string[] = [];
  const glue: IdeGlue = {
    default: async () => undefined,
    parse_unified: () => JSON.stringify({ files: [], stats: { files: 1, additions: 2, deletions: 0 } }),
    check_path: (p) => {
      if (p.includes('..')) throw new Error('path may not contain ..');
      return p;
    },
    check_read: (p) => p,
    check_write: (p) => p,
    check_dir: (p) => p,
    is_secret: (p) => p.endsWith('.pem'),
    initial_state: () => JSON.stringify({ workspace: null, open: [], active: null, panel: 'changes', base: 'work_start', review: false, filter_changed: false }),
    reduce: (state, action) => {
      seen.push(action);
      const next = JSON.parse(state) as { open: string[] };
      const a = JSON.parse(action) as { open?: string };
      if (a.open) next.open = [...next.open, a.open];
      return JSON.stringify(next);
    },
    children: (entries) => entries,
    changed_filter: () => '[]',
    parse_workspace_id: (s) => JSON.stringify(s),
    format_workspace_id: (json) => JSON.parse(json) as string,
  };
  const ide = wrapIde(glue);
  eq(ide.initialState().base, 'work_start', 'ide: the initial state is the default');
  eq(ide.reduce(ide.initialState(), { open: 'a.txt' }).open, ['a.txt'], 'ide: reduce parses the state it returns');
  eq(seen, ['{"open":"a.txt"}'], 'ide: reduce sends the action as JSON');
  eq(ide.parseUnified('x').stats, { files: 1, additions: 2, deletions: 0 }, 'ide: parseUnified returns the totals');
  eq(ide.isSecret('keys/a.pem'), true, 'ide: isSecret passes through');
  eq(ide.checkPath('src/a.ts'), 'src/a.ts', 'ide: checkPath returns the cleaned path');
  let refusal = '';
  try {
    ide.checkPath('../x');
  } catch (e) {
    refusal = (e as Error).message;
  }
  eq(refusal, 'path may not contain ..', 'ide: a refusal reaches the caller with its message');
  eq(ide.formatWorkspaceId(ide.parseWorkspaceId('agent:7')), 'agent:7', 'ide: a workspace id round trips');
  eq(ide.children([{ path: 'a', kind: 'file', size: 1, hash: 'h', binary: false }], '')[0]?.path, 'a', 'ide: children parses the entries');
}
sectionEnd('ide wrapper tests: ok');

// The IDE view's pure helpers (ide/model.ts): the language by extension, the buffer and conflict transitions, the
// workspace's state badge, the viewed marks' storage, the diff marks, quick open's ranking and the tree's paths.
{
  eq(languageFor('src/a.ts'), 'typescript', 'ide model: .ts is TypeScript');
  eq(languageFor('ui/App.TSX'), 'tsx', 'ide model: the extension is matched in any case');
  eq(languageFor('build.mjs'), 'javascript', 'ide model: .mjs is JavaScript');
  eq(languageFor('src/lib.rs'), 'rust', 'ide model: .rs is Rust');
  eq(languageFor('tool.py'), 'python', 'ide model: .py is Python');
  eq(languageFor('README.md'), 'markdown', 'ide model: .md is Markdown');
  eq(languageFor('x/data.json'), 'json', 'ide model: .json is JSON');
  eq(languageFor('site/index.htm'), 'html', 'ide model: .htm is HTML');
  eq(languageFor('s.css'), 'css', 'ide model: .css is CSS');
  eq(languageFor('Makefile'), 'plain', 'ide model: an extensionless file is plain');
  eq(languageFor('.gitignore'), 'plain', 'ide model: a dotfile is plain');
  eq(languageFor('dir.ts/readme'), 'plain', 'ide model: a dot in a directory name is not an extension');
  eq(languageFor('notes.txt'), 'plain', 'ide model: an unknown extension is plain');

  // Save, conflict, keep mine, reload.
  let b = openBuffer('a.txt', 'h1');
  eq([b.dirty, b.conflict], [false, null], 'ide buffer: a fresh buffer is clean');
  b = applyBuffer(b, { kind: 'edit' });
  eq(b.dirty, true, 'ide buffer: an edit makes it dirty');
  b = applyBuffer(b, { kind: 'save' });
  eq(b.saving, true, 'ide buffer: a save in flight is saving');
  b = applyBuffer(b, { kind: 'conflict', current: 'h2' });
  eq([b.base, b.conflict, b.dirty, b.saving], ['h1', 'h2', true, false], 'ide buffer: a 409 keeps the edits and records the hub hash');
  b = applyBuffer(b, { kind: 'keep_mine', current: 'h2' });
  eq([b.base, b.conflict, b.dirty], ['h2', null, true], 'ide buffer: keep mine re-bases on the hub copy and keeps the edits');
  b = applyBuffer(b, { kind: 'saved', hash: 'h3', edited: false });
  eq([b.base, b.dirty, b.conflict], ['h3', false, null], 'ide buffer: a clean save takes the new hash');

  let c = applyBuffer(openBuffer('a.txt', 'h1'), { kind: 'edit' });
  c = applyBuffer(c, { kind: 'conflict', current: 'h2' });
  c = applyBuffer(c, { kind: 'reload', hash: 'h2' });
  eq([c.base, c.dirty, c.conflict], ['h2', false, null], 'ide buffer: reload takes the hub copy and drops the edits');
  const typedDuringSave = applyBuffer(applyBuffer(openBuffer('a.txt', 'h1'), { kind: 'edit' }), { kind: 'saved', hash: 'h2', edited: true });
  eq(typedDuringSave.dirty, true, 'ide buffer: an edit made during a save stays dirty');

  // A remote change: clean reloads, edited conflicts, the same hash is nothing.
  const clean = openBuffer('a.txt', 'h1');
  eq(onRemote(clean, 'h1').action, 'none', 'ide remote: the hash already read is nothing to do');
  eq(onRemote(clean, 'h2').action, 'reload', 'ide remote: a clean buffer reloads');
  const dirty = applyBuffer(clean, { kind: 'edit' });
  const clash = onRemote(dirty, 'h2');
  eq([clash.action, clash.buffer.conflict, clash.buffer.dirty], ['conflict', 'h2', true], 'ide remote: an edited buffer goes to the conflict state');
  eq(onRemote(clash.buffer, 'h1').buffer.conflict, null, 'ide remote: the hub back at the read hash clears the conflict');

  eq([badgeOf({ conflict: true, agentWorking: true, saved: true }), badgeOf({ conflict: false, agentWorking: true, saved: true })], ['conflict', 'agent working'], 'ide badge: conflict, then agent working');
  eq([badgeOf({ conflict: false, agentWorking: false, saved: true }), badgeOf({ conflict: false, agentWorking: false, saved: false })], ['saved', 'idle'], 'ide badge: saved, then idle');

  // Viewed marks: per workspace, malformed storage reads as none, a changed diff is no longer viewed.
  eq(viewedKey('agent:7'), 'apz.ide.viewed.agent:7', 'ide viewed: the key names the workspace');
  eq([parseViewed(null), parseViewed('not json'), parseViewed('[1]')], [{}, {}, {}], 'ide viewed: missing or malformed storage reads as no marks');
  eq(parseViewed('{"a.ts":"m1","b":2}'), { 'a.ts': 'm1' }, 'ide viewed: only string marks are kept');
  const marked = withViewed({}, 'a.ts', 'm1', true);
  eq([isViewed(marked, 'a.ts', 'm1'), isViewed(marked, 'a.ts', 'm2')], [true, false], 'ide viewed: a mark counts only for its own diff');
  eq(withViewed(marked, 'a.ts', 'm1', false), {}, 'ide viewed: unmarking removes the path');

  const hunkOf = (text: string): FileChange => ({
    path: 'a.ts',
    old_path: null,
    status: 'modified',
    binary: false,
    hunks: [
      {
        old_start: 1,
        old_lines: 1,
        new_start: 1,
        new_lines: 1,
        header: '@@ -1 +1 @@',
        lines: [
          { kind: 'remove', old_no: 1, new_no: null, text: 'a' },
          { kind: 'add', old_no: null, new_no: 1, text },
        ],
      },
    ],
  });
  eq(diffMark(hunkOf('b')) === diffMark(hunkOf('b')), true, 'ide diff mark: the same diff has the same mark');
  eq(diffMark(hunkOf('b')) === diffMark(hunkOf('c')), false, 'ide diff mark: a changed diff has a new mark');
  eq(lineCounts(hunkOf('b')), { add: 1, del: 1 }, 'ide diff: the line counts of a file');

  // Quick open: every character in order; the file name and word starts rank first.
  eq([fuzzyScore('', 'x.ts'), fuzzyScore('zz', 'src/a.ts')], [0, null], 'ide quick open: the empty query matches, a missing character does not');
  eq(rankPaths(['src/app/main.ts', 'src/main.ts', 'docs/readme.md'], 'main'), ['src/main.ts', 'src/app/main.ts'], 'ide quick open: the shorter name ranks first and a miss is dropped');
  eq(rankPaths(['b.ts', 'a.ts'], ''), ['a.ts', 'b.ts'], 'ide quick open: ties sort by path');
  eq(rankPaths(['a/b/c.ts', 'abc.ts'], 'abc')[0], 'abc.ts', 'ide quick open: a run in the file name beats a scattered match');

  // The tree's paths.
  eq(dirsOf('a/b/c.txt'), ['', 'a', 'a/b'], 'ide tree: the directories above a file, root first');
  eq(dirsOf('x.ts'), [''], 'ide tree: a top-level file sits in the root');
  eq([parentDir('a/b/c.txt'), parentDir('x.ts')], ['a/b', ''], 'ide tree: a file’s directory');
}
sectionEnd('ide view model tests: ok');

{
  // The layout: sizes kept in range, the stored layout read back safely, panels fitted to the window, the narrow
  // breakpoint, the screen the Console button returns to, and the panel an openIde link names.
  eq([clampSize(900, 180, 520, 260), clampSize('x', 180, 520, 260), clampSize(10.6, 180, 520, 260)], [520, 260, 180], 'ide layout: a size is clamped, and a non-number is the default');
  eq(defaultLayout(1200), { left: 260, right: 380, leftOpen: true, rightOpen: true }, 'ide layout: both panels open on a wide window');
  eq(defaultLayout(800), { left: 260, right: 380, leftOpen: false, rightOpen: false }, 'ide layout: both panels closed on a narrow window');
  eq(parseLayout(null, 1200), defaultLayout(1200), 'ide layout: nothing stored is the default');
  eq(parseLayout('{not json', 1200), defaultLayout(1200), 'ide layout: corrupt storage is the default');
  eq(parseLayout('{"left":9999,"right":1,"leftOpen":false,"rightOpen":"yes"}', 1200), { left: LEFT_SIZE.max, right: RIGHT_SIZE.min, leftOpen: false, rightOpen: true }, 'ide layout: out-of-range sizes are clamped and a non-boolean open flag is the default');
  const kept = { left: 333, right: 401, leftOpen: false, rightOpen: true };
  eq(parseLayout(serializeLayout(kept), 1200), kept, 'ide layout: a stored layout reads back as it was saved');
  eq(
    [fitSize(900, 'left', 1000, 0), fitSize(900, 'right', 1000, 300), fitSize(900, 'right', 800, 400)],
    [LEFT_SIZE.max, 1000 - 44 - 300 - CENTRE_MIN, RIGHT_SIZE.min],
    'ide layout: a panel is fitted to the window, keeping the editor its minimum',
  );
  eq([isNarrow(899), isNarrow(900)], [true, false], 'ide layout: narrow is under 900px');
  eq(originFallback(null), '#/overview', 'ide origin: no console route is the overview');
  eq([originFallback('#/ide/agent:7'), originFallback('#/ide'), originFallback('#/ide?panel=changes')], ['#/overview', '#/overview', '#/overview'], 'ide origin: an IDE route is never the origin');
  eq(originFallback('#/agents/105'), '#/agents/105', 'ide origin: the console route the IDE was opened from');
  eq([leftViewFor('changes'), leftViewFor('agent'), rightTabFor('agent'), rightTabFor('changes')], ['changes', null, 'agent', null], 'ide layout: a link’s panel is a left view or a right tab');
}
sectionEnd('ide layout tests: ok');
// The IDE terminal's logic (views/ide-terminal-model.ts): base64 over bytes, the socket's frames, the offset a re-attach
// resumes from, the reconnect schedule and the session storage. The view itself needs a DOM and runs in the browser.
{
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  // Base64 of bytes: multi-byte UTF-8 survives, and a long buffer crosses the internal chunks.
  const text = 'héllo — 日本語 🙂 é';
  eq(decoder.decode(base64ToBytes(bytesToBase64(encoder.encode(text)))), text, 'ide terminal base64: multi-byte UTF-8 survives the round trip');
  eq(bytesToBase64(encoder.encode('é')), 'w6k=', 'ide terminal base64: the UTF-8 bytes are encoded, not the code units');
  const big = new Uint8Array(100_000).map((_, i) => (i * 31) & 0xff);
  const back = base64ToBytes(bytesToBase64(big));
  eq(back.length === big.length && back.every((b, i) => b === big[i]), true, 'ide terminal base64: a long buffer round-trips');
  eq(splitInput(new Uint8Array(40_000), 16_384).map((chunk) => chunk.length), [16_384, 16_384, 7_232], 'ide terminal input: frames of at most one chunk');

  // The client frames.
  eq(JSON.parse(openFrame(80, 24)), { t: 'open', cols: 80, rows: 24 }, 'ide terminal frame: open without a cwd');
  eq(JSON.parse(openFrame(80, 24, 'src')), { t: 'open', cols: 80, rows: 24, cwd: 'src' }, 'ide terminal frame: open with a cwd');
  eq(JSON.parse(openFrame(80, 24, '')), { t: 'open', cols: 80, rows: 24 }, 'ide terminal frame: an empty cwd is left out');
  eq(JSON.parse(attachFrame('term-1', 12)), { t: 'attach', term: 'term-1', offset: 12 }, 'ide terminal frame: attach');
  eq(JSON.parse(inputFrame(encoder.encode('é'))), { t: 'in', data: 'w6k=' }, 'ide terminal frame: input is base64 of its bytes');
  eq(JSON.parse(resizeFrame(100, 30)), { t: 'resize', cols: 100, rows: 30 }, 'ide terminal frame: resize');
  eq(JSON.parse(killFrame()), { t: 'kill' }, 'ide terminal frame: kill');

  // The hub's frames.
  const out = parseServerFrame(JSON.stringify({ t: 'out', data: bytesToBase64(encoder.encode('日本')), offset: 5 }));
  eq(out && out.t === 'out' ? [decoder.decode(out.data), out.offset] : null, ['日本', 5], 'ide terminal frame: out carries its bytes and offset');
  eq(parseServerFrame('{"t":"opened","term":"term-3"}'), { t: 'opened', term: 'term-3' }, 'ide terminal frame: opened');
  eq(parseServerFrame('{"t":"lost","bytes":9}'), { t: 'lost', bytes: 9 }, 'ide terminal frame: lost');
  eq(parseServerFrame('{"t":"exit","code":null}'), { t: 'exit', code: null }, 'ide terminal frame: exit without a code');
  eq(parseServerFrame('{"t":"exit","code":2}'), { t: 'exit', code: 2 }, 'ide terminal frame: exit with a code');
  eq(parseServerFrame('{"t":"error","message":"no such terminal"}'), { t: 'error', message: 'no such terminal' }, 'ide terminal frame: error');
  eq(parseServerFrame('{"t":"pong"}'), { t: 'pong' }, 'ide terminal frame: pong');
  eq(
    [parseServerFrame('not json'), parseServerFrame('{"t":"nope"}'), parseServerFrame('{"t":"out","data":"!!","offset":0}'), parseServerFrame('{"t":"out","data":"","offset":-1}'), parseServerFrame('{"t":"opened"}')],
    [null, null, null, null, null],
    'ide terminal frame: a frame that is not of the protocol is ignored',
  );

  // Offset tracking: a re-attach resumes one past the last byte the view holds (a count of bytes, not characters).
  const wire = [
    { t: 'out', data: bytesToBase64(encoder.encode('héllo')), offset: 0 },
    { t: 'out', data: bytesToBase64(encoder.encode(' wörld')), offset: 6 },
  ].map((frame) => parseServerFrame(JSON.stringify(frame)));
  const last = wire[1];
  eq(last && last.t === 'out' ? nextOffset(last) : -1, 13, 'ide terminal offset: one past the last byte held');

  // The reconnect schedule: 1 s, 2 s, 5 s, then 10 s for good.
  eq([0, 1, 2, 3, 4, 50].map(reconnectDelay), [1_000, 2_000, 5_000, 10_000, 10_000, 10_000], 'ide terminal reconnect: 1 s, 2 s, 5 s, then 10 s');
  eq([reconnectDelay(-1), reconnectDelay(0.5)], [1_000, 1_000], 'ide terminal reconnect: a negative or fractional attempt is the first wait');
  eq(MAX_TERMINALS, 4, 'ide terminal: four terminals per workspace, the hub cap');

  // Session storage: one key per workspace and slot, and a storage that refuses is no session.
  const memory = new Map<string, string>();
  const store: KeyValueStore = {
    getItem: (key) => memory.get(key) ?? null,
    setItem: (key, value) => {
      memory.set(key, value);
    },
    removeItem: (key) => {
      memory.delete(key);
    },
  };
  const key = sessionKey('agent-1', '0');
  eq(new Set([sessionKey('agent-1', '0'), sessionKey('agent-2', '0'), sessionKey('agent-1', '1')]).size, 3, 'ide terminal session keys: one per workspace and slot');
  eq(sessionKey('a/b', '0').includes('/'), false, 'ide terminal session keys: the workspace name is encoded');
  eq(readSession(store, key), null, 'ide terminal session: nothing stored reads as null');
  writeSession(store, key, { term: 'term-1', offset: 42 });
  eq(readSession(store, key), { term: 'term-1', offset: 42 }, 'ide terminal session: a stored session reads back');
  eq(readSession(store, sessionKey('agent-1', '1')), null, 'ide terminal session: slots do not share a session');
  store.setItem(key, '{"term":"","offset":1}');
  eq(readSession(store, key), null, 'ide terminal session: an empty term is no session');
  store.setItem(key, 'not json');
  eq(readSession(store, key), null, 'ide terminal session: a malformed value is no session');
  clearSession(store, key);
  eq(readSession(store, key), null, 'ide terminal session: a cleared session reads as null');
  eq(readSession(undefined, key), null, 'ide terminal session: no storage at all is no session');
  const blocked: KeyValueStore = {
    getItem: () => {
      throw new Error('storage is blocked');
    },
    setItem: () => {
      throw new Error('storage is blocked');
    },
    removeItem: () => {
      throw new Error('storage is blocked');
    },
  };
  let threw = false;
  try {
    writeSession(blocked, key, { term: 'term-1', offset: 0 });
    clearSession(blocked, key);
  } catch {
    threw = true;
  }
  eq([readSession(blocked, key), threw], [null, false], 'ide terminal session: blocked storage is no session and no error');

  // The socket's address: wss on https, ws on http, and the workspace encoded.
  eq(terminalSocketUrl('agent 1/x', { protocol: 'https:', host: 'console.example' }), 'wss://console.example/v1/ide/terminal/agent%201%2Fx', 'ide terminal socket: wss on https, the workspace encoded');
  eq(terminalSocketUrl('agent-7', { protocol: 'http:', host: 'localhost:8080' }), 'ws://localhost:8080/v1/ide/terminal/agent-7', 'ide terminal socket: ws on http');

  // Notices: no control characters from the hub reach the terminal.
  eq(plainText('\u001b[31mred\u0007\n'), '[31mred', 'ide terminal notices: control characters are dropped');
}
sectionEnd('ide terminal tests: ok');

// ---- IDE agent view model (src/ide/view.ts) ----
{
  const ag = (cid: number, name: string, over: Record<string, unknown> = {}): Agent => ({ cid, name, status: 'idle', work: null, ...over }) as unknown as Agent;
  const en = (seq: number, at_ms: number, kind: Entry['kind'], data: Record<string, unknown> = {}, cid = 1, work = 7): Entry => ({ seq, at_ms, cid, work, kind, data });

  eq(rowStatus({ status: 'working' }), 'working', 'rowStatus: working');
  eq(rowStatus({ status: 'offline' }, { state: 'done' }), 'sleeping', 'rowStatus: offline is sleeping whatever the last task');
  eq(rowStatus({ status: 'idle' }, { state: 'done' }), 'done', 'rowStatus: idle after a done task');
  eq(rowStatus({ status: 'idle' }, { state: 'failed' }), 'failed', 'rowStatus: idle after a failed task');
  eq(rowStatus({ status: 'idle' }, null), 'idle', 'rowStatus: idle with no task');

  eq(currentTask(ag(1, 'a', { work: { prompt: '\n  Fix the login bug  \nmore' } })), 'Fix the login bug', 'currentTask: first non-empty line, trimmed');
  eq(currentTask(ag(1, 'a')), null, 'currentTask: null with no work');
  eq(currentTask(ag(1, 'a', { work: { prompt: 'x'.repeat(200) } }))?.length, 120, 'currentTask: cut to 120 characters');

  const chat = buildChat([
    en(1, 0, 'work_start', { kind: 'chat', prompt: 'hello' }),
    en(2, 10, 'model_response', { content: [{ type: 'text', text: 'On it.' }, { type: 'tool_use', id: 'c1', name: 'read_file', input: { path: 'a.rs' } }] }),
    en(3, 20, 'tool_request', { id: 'c1', name: 'read_file', input: { path: 'src/main.rs' } }),
    en(4, 30, 'tool_result', { id: 'c1', is_error: false, content: 'fn main' }),
    en(5, 40, 'tool_request', { id: 'c2', name: 'bash', input: { command: 'cargo test' } }),
    en(6, 50, 'tool_result', { id: 'c2', is_error: true, content: 'boom' }),
    en(7, 60, 'model_response', { content: [{ type: 'text', text: 'Done.' }] }),
  ], true);
  eq(chat.map((c) => c.kind).join(','), 'owner,agent,steps,agent,working', 'buildChat: owner, prose, one steps item, prose, working');
  const stepsItem = chat[2];
  eq(stepsItem.kind === 'steps' ? stepsItem.steps.length : -1, 2, 'buildChat: both tool calls collapse into one steps item');
  eq(stepsItem.kind === 'steps' ? stepsItem.last : '', 'Ran command', 'buildChat: last is the label of the last call');
  eq(stepsItem.kind === 'steps' ? stepsItem.outcome : '', 'mixed', 'buildChat: one ok and one failed is mixed');
  eq(stepsItem.kind === 'steps' ? stepLabel(stepsItem.steps[0]) : '', 'Read src/main.rs', 'stepLabel: read names the path');

  const act = activitySplit([
    en(1, 0, 'model_request'),
    en(2, 100, 'model_response'),
    en(3, 110, 'tool_request', { id: 'c1', name: 'bash', input: { command: 'ls' } }),
    en(4, 150, 'tool_result', { id: 'c1', is_error: false }),
    en(5, 160, 'model_request'),
    en(6, 300, 'model_response'),
  ]);
  eq(act.thinkingMs, 240, 'activitySplit: thinking is request to response, summed');
  eq(act.toolMs, 40, 'activitySplit: tool time is request to result');
  eq(act.totalMs, 300, 'activitySplit: total is last minus first');
  eq(act.calls.length === 1 && act.calls[0].ms === 40 && act.calls[0].outcome === 'ok' && act.calls[0].name === 'bash', true, 'activitySplit: one call with name, ms and outcome');
  eq(activitySplit([en(1, 5, 'work_start')]).totalMs, 0, 'activitySplit: no span with one entry');

  const pal: PaletteItem[] = [
    { id: 'a', group: 'command', label: 'Open agent Zed' },
    { id: 'b', group: 'file', label: 'Read main.rs' },
    { id: 'c', group: 'command', label: 'Toggle terminal' },
  ];
  eq(filterPalette(pal, '').map((i) => i.id).join(','), 'a,b,c', 'filterPalette: empty query keeps input order');
  eq(filterPalette(pal, '', 2).length, 2, 'filterPalette: empty query is capped at the limit');
  eq(filterPalette(pal, 'term').map((i) => i.id).join(','), 'c', 'filterPalette: only matching items remain');
  const rank: PaletteItem[] = [
    { id: 'x', group: 'file', label: 'xa' },
    { id: 'y', group: 'file', label: 'ax' },
  ];
  eq(filterPalette(rank, 'a').map((i) => i.id).join(','), 'y,x', 'filterPalette: earlier match ranks first');
  eq(matchScore('Open agent', ''), 0, 'matchScore: empty query scores 0');
  eq(matchScore('abc', 'z'), null, 'matchScore: no match is null');
  const sub = matchScore('Open agent', 'oa');
  eq(sub !== null && sub > 0 && sub < 100, true, 'matchScore: a subsequence scores below any substring');
  eq(matchScore('Open agent', 'AGENT') !== null, true, 'matchScore: case-insensitive');

  eq(formatMs(340), '340 ms', 'formatMs: milliseconds');
  eq(formatMs(1200), '1.2 s', 'formatMs: seconds');
  eq(formatMs(125000), '2 min 5 s', 'formatMs: minutes');

  eq(mentionsOf('ping @all and @ana, then @ana again and @dev-2_x.').join(','), 'all,ana,dev-2_x', 'mentionsOf: names in order, no duplicates, trailing dot dropped');
  eq(mentionsOf('mail a@b.com now').length, 0, 'mentionsOf: an address is not a mention');

  const hues = ['zed', 'amy', '', 'Long Name Here'].map(authorHue);
  eq(hues.every((h) => Number.isInteger(h) && h >= 0 && h <= 5), true, 'authorHue: integers 0..5');
  eq(authorHue('amy') === authorHue('amy'), true, 'authorHue: stable for one name');
}
sectionEnd('ide agent view model tests: ok');

// ---- Rich content: the classifier against the shared vectors ----------------------------------------------------
// crates/apz-rich/tests/vectors.json is the file the Rust crate tests too (tests/vectors.rs), so the two classifiers
// cannot drift apart. The file's keys are sorted and the port's are not, so both sides are compared with sorted keys.
{
  const canon = (value: unknown): string =>
    JSON.stringify(value, (_key, v: unknown) =>
      v && typeof v === 'object' && !Array.isArray(v)
        ? Object.fromEntries(Object.entries(v as Record<string, unknown>).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
        : v,
    );
  const rawOf = (span: Span): string => (span.kind === 'ref' ? span.value.raw : span.value);
  const cases = richVectors as unknown as { name: string; text: string; ctx: RichCtx; spans: Span[] }[];
  eq(cases.length >= 60, true, `rich: the shared vectors have at least 60 cases (${cases.length})`);
  for (const c of cases) {
    const got = classify(c.text, c.ctx);
    if (canon(got) !== canon(c.spans)) {
      failures++;
      console.error(`FAIL rich vector ${c.name}\n  got      ${canon(got)}\n  expected ${canon(c.spans)}`);
    }
    eq(got.map(rawOf).join(''), c.text, `rich vector ${c.name}: the spans join back to the text`);
  }

  // The chip labels (the Rust crate's label tests).
  const repo = { owner: 'o', repo: 'r' };
  eq(label({ kind: 'github_pull', number: 12, ...repo }), 'o/r #12', 'rich label: pull');
  eq(label({ kind: 'github_commit', sha: 'abc1234def5678', ...repo }), 'o/r @abc1234', 'rich label: commit is seven characters');
  eq(label({ kind: 'github_file', ref: 'main', path: 'src/a.rs', line: 10, dir: false, ...repo }), 'r: src/a.rs:10', 'rich label: file with a line');
  eq(label({ kind: 'github_file', ref: 'main', path: 'crates', line: null, dir: true, ...repo }), 'r: crates/', 'rich label: folder gets its slash');
  eq(label({ kind: 'task', number: 528 }), '#528', 'rich label: task');
  eq(label({ kind: 'agent', name: 'Name', cid: null }), '@Name', 'rich label: agent by name');
  eq(label({ kind: 'agent', name: null, cid: 7 }), 'agent:7', 'rich label: agent by cid');
  eq(label({ kind: 'path', path: 'src/main.rs', line: 42, dir: false }), 'src/main.rs:42', 'rich label: path with a line');
  eq(label({ kind: 'image', url: 'https://x.io/img/a.png?v=2' }), 'a.png', 'rich label: image is its file name');
  eq(label({ kind: 'link', url: 'https://example.com/docs/x/?q=1#f' }), 'example.com/docs/x', 'rich label: link is domain and path');
  eq(label({ kind: 'library_tool', name: 'search', version: 3 }), 'search v3', 'rich label: library tool');
}
sectionEnd('rich content tests: ok');

// ---- IDE grouping by project, duplicate names and the start choice (src/ide/groups.ts) ----
{
  const ag = (cid: number, name: string, over: Record<string, unknown> = {}): Agent => ({ cid, name, status: 'idle', work: null, ...over }) as unknown as Agent;
  const pj = (id: string, title: string, lead_cid: number, tasks: string[] = [], updated_ms = 0): Project => ({ id, title, lead_cid, tasks, updated_ms, repo: null }) as unknown as Project;
  const tk = (id: string, agent_cid: number | null): Task => ({ id, agent_cid, state: 'open' }) as unknown as Task;
  const names = (g: { agents: Agent[] }): string => g.agents.map((a) => a.name).join(',');
  const work = (task: string) => ({ prompt: 'x', origin: { task, motion: null, from_cid: null } });

  // Alpha: led by zed (1), holds t1 which amy (2) claimed. Beta: led by amy (2), holds t2 which cy (4) is working on.
  // Gamma: no agents. t3 is held by dee (5), but no project holds t3.
  const projects = [pj('p1', 'Alpha', 1, ['t1'], 100), pj('p2', 'Beta', 2, ['t2'], 300), pj('p3', 'Gamma', 9, [], 900)];
  const tasks = [tk('t1', 2), tk('t2', null), tk('t3', 5)];
  const agents = [
    ag(1, 'zed'),
    ag(2, 'amy'),
    ag(3, 'bob'),
    ag(4, 'cy', { status: 'working', work: work('t2') }),
    ag(5, 'dee', { work: work('nope') }),
  ];
  const groups = groupByProject(agents, projects, tasks);
  eq(groups.map((g) => g.title).join(','), 'Beta,Alpha,No project', 'groupByProject: most recent project first, empty projects skipped, No project last');
  eq(names(groups[0]), 'amy,cy', 'groupByProject: the lead and a work-origin assignee are in the project');
  eq(names(groups[1]), 'amy,zed', 'groupByProject: an agent in two projects is listed in both (amy: lead of Beta, claimed t1 in Alpha)');
  eq(groups[2].id, null, 'groupByProject: No project has a null id');
  eq(names(groups[2]), 'bob,dee', 'groupByProject: agents in no project are sorted by name');
  eq(groupByProject([ag(1, 'zed')], [pj('p1', 'Alpha', 1)], []).map((g) => g.title).join(','), 'Alpha', 'groupByProject: no No project group when every agent has a project');
  eq(groupByProject([ag(2, 'amy')], [], []).map((g) => g.title).join(','), 'No project', 'groupByProject: with no projects every agent is in No project');
  eq(groupByProject([ag(2, 'amy')], [pj('p1', 'Alpha', 1, [], 5)], [tk('t9', 2)])[0]?.title, 'No project', 'groupByProject: a claimed task outside every project does not place the agent');
  eq(projectOf(agents[1], projects, tasks)?.id, 'p2', 'projectOf: the most recently active project of the agent');
  eq(projectOf(agents[2], projects, tasks), null, 'projectOf: null for an agent in no project');

  eq([...sharedNames([ag(1, 'Ada'), ag(2, 'ada'), ag(3, 'Bo')])].sort().join(','), '1,2', 'sharedNames: names that collide, case-insensitively');
  eq(sharedNames([ag(1, 'Ada'), ag(2, 'Bo')]).size, 0, 'sharedNames: no collision, no cid shown');

  // The sidebar order is Beta (amy, cy), Alpha (amy, zed), No project (bob, dee); the unique order is 2, 4, 1, 3, 5.
  eq(startAgent(groups, 1), 1, 'startAgent: the agent selected last time, while it is listed');
  eq(startAgent(groups, 99), 4, 'startAgent: a gone agent falls back to the first working one');
  eq(startAgent(groups, null), 4, 'startAgent: with nothing stored, the first working agent');
  const idle = groupByProject([ag(2, 'amy'), ag(1, 'zed')], [], []);
  eq(startAgent(idle, null), 2, 'startAgent: no working agent, the first agent in the sidebar');
  eq(startAgent([], null), null, 'startAgent: null when there are no agents');
}
sectionEnd('ide grouping tests: ok');

// The stored terminals of a workspace: one per slot with a session, counted from the terminal module's storage.
{
  const store = new Map<string, string>();
  const kv = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v), removeItem: (k: string) => void store.delete(k) };
  eq(storedTerminalCount(kv, 'agent:77'), 0, 'storedTerminalCount: none for a workspace with no sessions');
  writeSession(kv, sessionKey('agent:77', '0'), { term: 't-a', offset: 0 });
  writeSession(kv, sessionKey('agent:77', '2'), { term: 't-b', offset: 5 });
  writeSession(kv, sessionKey('agent:78', '1'), { term: 't-c', offset: 0 });
  eq(storedTerminalCount(kv, 'agent:77'), 2, 'storedTerminalCount: one per stored slot of that workspace');
  clearSession(kv, sessionKey('agent:77', '0'));
  eq(storedTerminalCount(kv, 'agent:77'), 1, 'storedTerminalCount: a cleared session is no longer counted');
  eq(storedTerminalCount(undefined, 'agent:77'), 0, 'storedTerminalCount: no storage, no terminals');
}
sectionEnd('ide stored terminals tests: ok');

if (failedSections.length) {
  console.error(`${failedSections.length} section(s) failed: ${failedSections.join(', ')}`);
  process.exit(1);
}
