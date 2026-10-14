// Pure-logic tests: run with `npm test`.
import { parse, href } from '../src/router';
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


if (failedSections.length) {
  console.error(`${failedSections.length} section(s) failed: ${failedSections.join(', ')}`);
  process.exit(1);
}
