// Pure-logic tests: run with `npm test`.
import { parse, href } from '../src/router';
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
import type { Agent, FleetApple, FleetKind, FleetMachine, FleetResponse, FleetStatus, IdeWorkspaceInfo } from '../src/api';
import { agentTally, boardOf, gibText, machineFacts, machineSubtitle } from '../src/views/floor';
import { hubOrigin, setupSnippets } from '../src/views/hub-mcp-snippets';
import { ENV_NAME_RE, limitsProblem, matchingPreset, ownRowProblem, specsOfBag } from '../src/views/mcp-common';
import { wrapIde, type IdeGlue } from '../src/ide/wasm';
import { applyBuffer, badgeOf, diffMark, dirsOf, fuzzyScore, groupWorkspaces, isViewed, languageFor, lineCounts, onRemote, openBuffer, osLabel, parentDir, parseViewed, pickerLabel, rankPaths, viewedKey, withViewed } from '../src/ide/model';
import type { FileChange } from '../src/ide/types';
import { CENTRE_MIN, clampSize, defaultLayout, fitSize, hasDiff, isNarrow, LEFT_SIZE, leftViewFor, leftViewsFor, originFallback, parseLayout, RIGHT_SIZE, rightTabFor, rightTabsFor, runsFor, serializeLayout } from '../src/ide/layout';
import type { StationFacts, StationLease, StationSettings, StationView } from '../src/api';
import { auditLabel, codeChip, countdown, displaysText, formLabel, gpuLine, gpuSummary, joinCommand, leaseCount, leaseStateChip, nameProblem, osText, ramText, readSettings, accountBadge, accountText, settingsForm, sortStations, stationSummary, statusChip, takeoverText, type SettingsForm } from '../src/views/workstations-model';
import { actionFrame, activeAction, backoffMs, bytesText, buttonAction, clockText, controlSocketUrl, crumbsOf, dollarsPerHour, defaultsForm, frameAt, frameTime, framePoint, inputButton, joinPath, keyAction, moveAction, parentDir, parseControlText, parseFrame, parseScreenText, pendingPaths, rateText, readDefaults, readRate, recordingSpan, scrollNotches, screenSocketUrl, sessionChoices, takeoverBanner, treeOf, uploadProblem, wheelAction, actionText, type KeyEventLike } from '../src/views/workstations-model';
import type { StationLease, RecordingDefaults } from '../src/api';
import type { MachineNode } from '../src/api';
import { isStationNode, leaseCounts, stationBadge, stationLine, stationMachine, stationThumbnailPath } from '../src/views/station-machine';
import { MAX_TERMINALS, attachFrame, base64ToBytes, bytesToBase64, clearSession, inputFrame, killFrame, nextOffset, openFrame, parseServerFrame, plainText, readSession, reconnectDelay, resizeFrame, sessionKey, splitInput, terminalSocketUrl, writeSession, type KeyValueStore } from '../src/views/ide-terminal-model';

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
  eq(ide.formatWorkspaceId(ide.parseWorkspaceId('station:w3')), 'station:w3', 'ide: a station id round trips');
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

{
  // The workspace kinds: the picker's groups in order (an empty group is left out), the labels, and the views a kind has.
  const listed: IdeWorkspaceInfo[] = [
    { id: 'repo:api@main', kind: 'repo', owner: 'api', name: 'api', writable: true, status: 'ok' },
    { id: 'station:w3', kind: 'station', owner: 'w3', name: 'studio', writable: true, status: 'online', os: 'macos' },
    { id: 'agent:7', kind: 'agent', owner: '7', name: 'Ada', writable: true, status: 'working' },
    { id: 'station:w4', kind: 'station', owner: 'w4', name: '', writable: true, status: 'online', os: 'linux' },
  ];
  eq(
    groupWorkspaces(listed).map((g) => [g.kind, g.label, g.items.map((w) => w.id)]),
    [
      ['agent', 'Agents', ['agent:7']],
      ['station', 'Workstations', ['station:w3', 'station:w4']],
      ['repo', 'Repos', ['repo:api@main']],
    ],
    'ide picker: the workspaces group as agents, workstations, repos, each keeping the list order',
  );
  eq(groupWorkspaces(listed.filter((w) => w.kind === 'repo')).map((g) => g.label), ['Repos'], 'ide picker: a kind with no workspaces has no group');
  eq([osLabel('macos'), osLabel('windows'), osLabel('linux'), osLabel(undefined), osLabel('plan9')], ['macOS', 'Windows', 'Linux', 'Unknown OS', 'plan9'], 'ide picker: a station OS reads as a short label');
  eq(
    [pickerLabel(listed[1]!), pickerLabel(listed[2]!), pickerLabel(listed[3]!), pickerLabel({ ...listed[0]!, writable: false })],
    ['Workstation · studio · macOS · ● online', 'Agent · Ada', 'Workstation · w4 · Linux · ● online', 'Repo · api (read-only)'],
    'ide picker: each kind labels its option, and a station shows its OS and status',
  );
  eq([leftViewsFor('agent'), leftViewsFor(null)], [['files', 'search', 'changes', 'checkpoints'], ['files', 'search', 'changes', 'checkpoints']], 'ide views: an agent and no workspace have every side view');
  eq(leftViewsFor('station'), ['files', 'search'], 'ide views: a station has no Changes and no Checkpoints');
  eq(rightTabsFor('agent'), ['agent', 'approvals', 'run', 'terminal'], 'ide views: an agent has the Agent and Approvals tabs');
  eq([rightTabsFor('repo'), rightTabsFor('station')], [['run', 'terminal'], ['run', 'terminal']], 'ide views: a repo and a station have no Agent or Approvals tab');
  eq([hasDiff('agent'), hasDiff('repo'), hasDiff('station'), hasDiff(null)], [true, true, false, true], 'ide views: only a station has no diff');
  eq([runsFor('agent'), runsFor('station'), runsFor('repo')], [true, true, false], 'ide views: runs are for agents and stations, not repositories yet');
}
sectionEnd('ide workspace kind tests: ok');
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

// Workstations: the labels, chips and the settings form's serialization the screen draws from.
const linuxFacts: StationFacts = {
  os: 'linux', os_version: '6.8', arch: 'x86_64', hostname: 'studio', form: 'desktop', cpus: 16, ram_mb: 65536,
  gpus: [{ vendor: 'nvidia', name: 'RTX 4090', vram_mb: 24576 }], displays: [{ id: 1, w: 2560, h: 1440, scale: 1 }],
  account: { mode: 'agent_user', name: 'agentplaza' }, apps: 120, virtual_displays: false, ui_tree: true,
};
const macFacts: StationFacts = {
  ...linuxFacts, os: 'macos', os_version: '15.2', form: 'mini', ram_mb: 16384, gpus: [], displays: [], account: { mode: 'owner', name: 'Zarathos' },
};
eq(osText(linuxFacts), 'Linux 6.8', 'workstation os: the label and the version');
eq(osText(macFacts), 'macOS 15.2', 'workstation os: macOS');
eq(formLabel('vm'), 'VM', 'workstation form: a VM');
eq([ramText(16384), ramText(8192), ramText(0)], ['16 GB RAM', '8.0 GB RAM', '—'], 'workstation memory: whole GiB from 10, one decimal below, none');
eq(gpuLine({ vendor: 'nvidia', name: 'RTX 4090', vram_mb: 24576 }), 'RTX 4090 · 24 GB', 'workstation gpu: its memory in GB');
eq(gpuLine({ vendor: 'apple', name: 'Apple M4', vram_mb: 0 }), 'Apple M4 · shared memory', 'workstation gpu: shared memory');
eq(gpuLine({ vendor: 'x', name: 'Small', vram_mb: 512 }), 'Small · 512 MB', 'workstation gpu: under a GB in MB');
eq([gpuSummary([]), gpuSummary(linuxFacts.gpus)], ['no GPU', 'RTX 4090'], 'workstation gpu summary');
eq([displaysText([]), displaysText(linuxFacts.displays)], ['none', '2560×1440'], 'workstation displays');
eq(stationSummary(linuxFacts), 'Linux 6.8 · Desktop · 64 GB RAM · RTX 4090', 'workstation row summary: OS, form, memory, GPUs');
eq(stationSummary(macFacts), 'macOS 15.2 · Mini · 16 GB RAM · no GPU', 'workstation row summary: no GPU in words');
eq(statusChip('draining'), { label: 'draining', tone: 'warn' }, 'workstation status chip: draining is a warning');
eq(statusChip('revoked'), { label: 'revoked', tone: 'danger' }, 'workstation status chip: revoked is a danger');
eq(accountBadge(macFacts.account), { text: 'agents act as Zarathos', tone: 'warn' }, 'workstation account badge: the owner\'s own account is flagged');
eq(accountBadge(linuxFacts.account), null, 'workstation account badge: a separate agent account is not flagged');
eq(accountText(linuxFacts.account), 'a separate agent account (agentplaza)', 'workstation account in words: the agent account');

const lease = (state: StationLease['state'], position: number | null = null): StationLease => ({
  id: 'l1', station: 'ws-1', agent: 7, task: null, kind: 'pixel', session: null, started_ms: 1, state, position,
});
eq(leaseCount([lease('queued', 1), lease('opening'), lease('active')]), { holding: 2, waiting: 1 }, 'workstation leases: holding and waiting');
eq([leaseStateChip(lease('queued', 2)), leaseStateChip(lease('queued'))], [{ label: 'queued #2', tone: 'warn' }, { label: 'queued', tone: 'warn' }], 'workstation lease chip: queued shows its place');
eq(leaseStateChip(lease('active')), { label: 'active', tone: 'ok' }, 'workstation lease chip: active');
eq(takeoverText('release_lease').includes('back to the queue'), true, 'workstation takeover: the release policy is explained');
eq(takeoverText('agent_waits').includes('resumes when you give the session back'), true, 'workstation takeover: the wait policy is explained');
eq(joinCommand('abc.def'), 'agentplaza workstation join abc.def', 'workstation join: the command for the machine');
eq(codeChip('used'), { label: 'Used', tone: 'info' }, 'workstation join code chip: used');
eq([countdown(754_000), countdown(-5)], ['12:34', '00:00'], 'workstation join code countdown: mm:ss, never negative');
eq([nameProblem('  '), nameProblem('a'.repeat(65)), nameProblem('ab\u0007'), nameProblem(' studio ')], ['Give the workstation a name.', 'The name is up to 64 characters.', 'The name has no control characters.', null], 'workstation name: the hub\'s rules');
eq(nameProblem('a'.repeat(64)), null, 'workstation name: 64 characters is the most');
eq([auditLabel('lease_granted'), auditLabel('something_new')], ['Lease granted', 'something new'], 'workstation audit: known kinds have words, unknown ones show as text');
const named = (name: string) => ({ name }) as StationView;
const input = [named('studio'), named('Alpha'), named('beta')];
eq([sortStations(input).map((s) => s.name), input.map((s) => s.name)], [['Alpha', 'beta', 'studio'], ['studio', 'Alpha', 'beta']], 'workstation list: name order, input untouched');

const defaults: StationSettings = { takeover: 'agent_waits', record: true, retention_days: 30, record_fps: 2, record_takeovers: true, work_folder: '~/AgentPlaza', snapshot_reset: false, rate_per_hour: 0 };
const form = settingsForm(defaults);
eq(form, { takeover: 'agent_waits', record: true, retention_days: '30', record_fps: '2', record_takeovers: true, work_folder: '~/AgentPlaza', snapshot_reset: false, rate_per_hour: '0' }, 'workstation settings form: the numbers as text');
eq(readSettings(form), { ok: true, settings: defaults }, 'workstation settings form: the defaults round-trip');
eq(readSettings({ ...form, work_folder: '  /srv/work  ', retention_days: ' 7 ', record_fps: '1' }), { ok: true, settings: { ...defaults, work_folder: '/srv/work', retention_days: 7, record_fps: 1 } }, 'workstation settings form: trimmed and parsed');
const edited: SettingsForm = { takeover: 'keep_working_pointer_paused', record: false, retention_days: '365', record_fps: '30', record_takeovers: false, work_folder: 'D:\\work', snapshot_reset: true, rate_per_hour: '1000000' };
eq(readSettings(edited), { ok: true, settings: { takeover: 'keep_working_pointer_paused', record: false, retention_days: 365, record_fps: 30, record_takeovers: false, work_folder: 'D:\\work', snapshot_reset: true, rate_per_hour: 1000000 } }, 'workstation settings form: every field, at the bounds');
const refused = (patch: Partial<SettingsForm>): boolean => readSettings({ ...form, ...patch }).ok;
eq(
  [
    refused({ retention_days: '0' }),
    refused({ retention_days: '1.5' }),
    refused({ retention_days: '3651' }),
    refused({ record_fps: '31' }),
    refused({ record_fps: '' }),
    refused({ work_folder: '   ' }),
    refused({ work_folder: 'a'.repeat(513) }),
    refused({ work_folder: 'a\tb' }),
    refused({ takeover: 'nope' as SettingsForm['takeover'] }),
    refused({ rate_per_hour: '' }),
    refused({ rate_per_hour: '1.5' }),
    refused({ rate_per_hour: '-1' }),
    refused({ rate_per_hour: '1000001' }),
  ],
  [false, false, false, false, false, false, false, false, false, false, false, false, false],
  'workstation settings form: every out-of-bounds value is refused',
);
eq(readSettings({ ...form, rate_per_hour: ' 2500 ' }), { ok: true, settings: { ...defaults, rate_per_hour: 2500 } }, 'workstation rate: whole credits, trimmed');
eq(readSettings({ ...form, retention_days: '0' }), { ok: false, error: 'Keep recordings for whole days, from 1 to 3650.' }, 'workstation settings form: the retention message');
sectionEnd('workstation tests: ok');

// Workstations, live view and takeover, recordings and the work folder: the wire shapes and the mappings the pages draw from.
const frameBytes = (header: Record<string, unknown>, image: number[]): ArrayBuffer => {
  const head = new TextEncoder().encode(JSON.stringify(header));
  const out = new Uint8Array(4 + head.length + image.length);
  new DataView(out.buffer).setUint32(0, head.length, false);
  out.set(head, 4);
  out.set(image, 4 + head.length);
  return out.buffer;
};
const parsed = parseFrame(frameBytes({ w: 4, h: 2, format: 'jpeg', ts: 9, seq: 3 }, [1, 2, 3]));
eq(parsed && { ...parsed, image: Array.from(parsed.image) }, { w: 4, h: 2, format: 'jpeg', ts: 9, seq: 3, image: [1, 2, 3] }, 'live frame: the header and the image after it');
eq(
  [
    parseFrame(new ArrayBuffer(2)),
    parseFrame(frameBytes({ w: 4, h: 2, format: 'gif', ts: 9, seq: 3 }, [])),
    parseFrame(frameBytes({ w: 0, h: 2, format: 'png', ts: 9, seq: 3 }, [])),
    parseFrame(frameBytes({ w: 4, h: 2, format: 'png', ts: 9 }, [])),
    parseFrame(frameBytes({ w: 4, h: 2, format: 'png', ts: 9, seq: 1.5 }, [])),
  ].map((f) => f === null),
  [true, true, true, true, true],
  'live frame: anything that is not a frame of the shape is refused',
);
{
  // A header longer than the message is refused, not read past its end.
  const buf = new Uint8Array([0, 0, 1, 0, 123]).buffer;
  eq(parseFrame(buf), null, 'live frame: a header longer than the message is refused');
}
eq(parseScreenText('{"t":"started","session":4}'), { t: 'started', session: 4 }, 'live text: started names the session');
eq([parseScreenText('{"t":"ended","reason":"done"}'), parseScreenText('{"t":"pong"}'), parseScreenText('{"t":"nope"}'), parseScreenText('not json')], [{ t: 'ended', reason: 'done' }, { t: 'pong' }, null, null], 'live text: ended and pong, and anything else is null');
eq(parseControlText('{"t":"taken","session":4,"policy":"release_lease"}'), { t: 'taken', session: 4, policy: 'release_lease' }, 'takeover text: taken names the session and the policy');
eq([parseControlText('{"t":"taken","session":4,"policy":"whatever"}'), parseControlText('{"t":"given_back"}'), parseControlText('{"t":"error","error":"no"}')], [null, { t: 'given_back' }, { t: 'error', error: 'no' }], 'takeover text: an unknown policy is refused');
eq(screenSocketUrl('w1', { session: 'shared', fps: 5, scale: 0.5 }, { protocol: 'https:', host: 'hub.local:7443' }), 'wss://hub.local:7443/v1/stations/w1/screen?session=shared&fps=5&scale=0.5', 'live socket: wss on https, the view in the query');
eq(controlSocketUrl('a b', 12, { protocol: 'http:', host: 'localhost:7443' }), 'ws://localhost:7443/v1/stations/a%20b/control?session=12', 'takeover socket: ws on http, the id encoded');
eq([0, 1, 2, 3, 9].map(backoffMs), [1000, 2000, 5000, 10000, 10000], 'sockets: the reconnect waits, the last one repeats');
eq(framePoint(150, 75, { left: 50, top: 25, width: 200, height: 100 }, { w: 400, h: 200 }), { x: 200, y: 100 }, 'pointer: a client point is a point of the frame');
eq([framePoint(1000, -50, { left: 50, top: 25, width: 200, height: 100 }, { w: 400, h: 200 }), framePoint(1, 1, { left: 0, top: 0, width: 0, height: 0 }, { w: 4, h: 4 })], [{ x: 399, y: 0 }, null], 'pointer: clamped to the frame, and none before it has a size');
eq([inputButton(0), inputButton(1), inputButton(2), inputButton(3)], ['left', 'middle', 'right', null], 'pointer: the DOM buttons as the daemon names them');
eq([buttonAction('right', true), buttonAction('right', false), moveAction({ x: 3, y: 4 })], [{ action: 'down', button: 'right' }, { action: 'up', button: 'right' }, { action: 'move', x: 3, y: 4 }], 'pointer: press, release and move');
eq(
  [scrollNotches(120, 0), scrollNotches(-120, 0), scrollNotches(-1, 0), scrollNotches(0, 0), scrollNotches(5000, 0), scrollNotches(3, 1), scrollNotches(1, 2)],
  [2, -2, -1, 0, 20, 3, 10],
  'wheel: notches keep the sign (positive scrolls down), clamped to 20',
);
eq(wheelAction({ x: 5, y: 6 }, 0, 120, 0), { action: 'scroll', x: 5, y: 6, dx: 0, dy: 2 }, 'wheel: a turn down is dy above zero');
eq(wheelAction({ x: 5, y: 6 }, -240, -120, 0), { action: 'scroll', x: 5, y: 6, dx: -5, dy: -2 }, 'wheel: a turn up and left is negative');
const keyOf = (key: string, mods = ''): KeyEventLike => ({ key, ctrlKey: mods.includes('c'), altKey: mods.includes('a'), shiftKey: mods.includes('s'), metaKey: mods.includes('m') });
eq(
  [
    keyAction(keyOf('a')),
    keyAction(keyOf('S', 'cs')),
    keyAction(keyOf('Tab', 's')),
    keyAction(keyOf('Enter')),
    keyAction(keyOf(' ')),
    keyAction(keyOf(' ', 'c')),
    keyAction(keyOf('F5')),
    keyAction(keyOf('+', 'c')),
    keyAction(keyOf('k', 'm')),
    keyAction(keyOf('ArrowLeft', 'a')),
    keyAction(keyOf('Shift', 's')),
    keyAction(keyOf('Dead')),
    keyAction(keyOf('é', 'a')),
    keyAction(keyOf('é')),
    keyAction(keyOf('Escape')),
  ],
  [
    { action: 'type', text: 'a' },
    { action: 'key', combo: 'ctrl+shift+s' },
    { action: 'key', combo: 'shift+tab' },
    { action: 'key', combo: 'enter' },
    { action: 'type', text: ' ' },
    { action: 'key', combo: 'ctrl+space' },
    { action: 'key', combo: 'f5' },
    { action: 'key', combo: 'ctrl+plus' },
    { action: 'key', combo: 'meta+k' },
    { action: 'key', combo: 'alt+left' },
    null,
    null,
    null,
    { action: 'type', text: 'é' },
    { action: 'key', combo: 'escape' },
  ],
  'keys: printable text is typed, chords carry their modifiers in order, and an unnamed key is nothing',
);
eq(actionFrame({ action: 'key', combo: 'enter' }), '{"t":"input","action":{"action":"key","combo":"enter"}}', 'takeover: an input frame carries the action');
eq(takeoverBanner('agent_waits').includes('wait until you give it back'), true, 'takeover banner: the wait policy');
eq(takeoverBanner('release_lease').includes('back to the queue'), true, 'takeover banner: the release policy');

const leaseOf = (session: number | null, state: StationLease['state'], kind: StationLease['kind'] = 'pixel', task: string | null = null): StationLease => ({
  id: `l${session ?? 0}`, station: 'ws-1', agent: 7, task, kind, session, started_ms: 1, state, position: state === 'queued' ? 1 : null,
});
eq(
  sessionChoices([leaseOf(3, "active", "pixel", "render"), leaseOf(null, "queued"), leaseOf(null, "active", "shared"), leaseOf(5, "opening")], (cid) => `Ada${cid}`),
  [
    { value: 'shared', label: "Shared: the owner's screen" },
    { value: '3', label: 'Session 3: Ada7 · render' },
    { value: '5', label: 'Session 5: Ada7' },
  ],
  'sessions: the shared one, then each pixel lease that has a session and is not waiting',
);

eq(recordingSpan({ started_ms: 1000, ended_ms: 5000 }, null), 4000, 'recording span: start to end');
eq([recordingSpan({ started_ms: 1000, ended_ms: null }, 3000), recordingSpan({ started_ms: 1000, ended_ms: null }, null)], [2000, 0], 'recording span: an open one runs to its last action');
eq([frameAt(0, 4000, 5), frameAt(4000, 4000, 5), frameAt(2000, 4000, 5), frameAt(-5, 4000, 5), frameAt(9000, 4000, 5), frameAt(10, 4000, 0), frameAt(3000, 0, 1)], [1, 5, 3, 1, 5, 0, 1], 'timeline: the frame on screen at a time, clamped');
eq([frameTime(3, 4000, 5), frameTime(1, 4000, 5), frameTime(5, 4000, 5), frameTime(1, 4000, 1)], [2000, 0, 4000, 0], 'timeline: a frame\'s time, the inverse of the frame at a time');
eq([activeAction([{ at_ms: 1100 }, { at_ms: 1500 }, { at_ms: 3000 }], 1000, 0), activeAction([{ at_ms: 1100 }, { at_ms: 1500 }, { at_ms: 3000 }], 1000, 100), activeAction([{ at_ms: 1100 }, { at_ms: 1500 }, { at_ms: 3000 }], 1000, 600), activeAction([{ at_ms: 1100 }, { at_ms: 1500 }, { at_ms: 3000 }], 1000, 5000)], [-1, 0, 1, 2], 'timeline: the newest action at or before a time');
eq([clockText(0), clockText(65_000), clockText(3_725_000), clockText(-5)], ['0:00', '1:05', '1:02:05', '0:00'], 'timeline: m:ss, h:mm:ss, never negative');
eq(actionText({ kind: 'owner_input', summary: { kind: 'type', text: 'hi' } }), 'owner input: text hi', 'actions: the kind, then the fields it was asked with');
eq(actionText({ kind: 'click', summary: { kind: 'click', x: 3, y: 4, button: 'left' } }), 'click: x 3, y 4, button left', 'actions: the fields in words');

const files = [{ path: 'a.md' }, { path: 'docs/x.md' }, { path: 'docs/sub/y.md' }, { path: 'zz/q.txt' }];
eq(treeOf(files, ''), { folders: [{ name: 'docs', path: 'docs' }, { name: 'zz', path: 'zz' }], files: [{ path: 'a.md' }] }, 'work folder: the root has its folders and its own files');
eq(treeOf(files, 'docs'), { folders: [{ name: 'sub', path: 'docs/sub' }], files: [{ path: 'docs/x.md' }] }, 'work folder: a folder has its folders and files, and no others');
eq([parentDir('docs/sub'), parentDir('a'), joinPath('', 'a'), joinPath('docs', 'x')], ['docs', '', 'a', 'docs/x'], 'work folder: parents and joined paths');
eq(crumbsOf('docs/sub'), [{ name: 'Work folder', path: '' }, { name: 'docs', path: 'docs' }, { name: 'sub', path: 'docs/sub' }], 'work folder: the trail to a folder');
eq([pendingPaths(['a', 'b']), pendingPaths([{ path: 'c' }, 5, { x: 1 }]), pendingPaths(null)], [['a', 'b'], ['c'], []], 'work folder: a paused sync\'s deletions, as strings or as objects');
eq([uploadProblem('a.txt', 10), uploadProblem('../x', 1), uploadProblem('a/b', 1), uploadProblem('', 1), uploadProblem('big', 512 * 1024 * 1024 + 1)].map((p) => p !== null), [false, true, true, true, true], 'work folder: upload names and sizes that cannot be kept');
eq([bytesText(0), bytesText(1536), bytesText(10 * 1024 * 1024), bytesText(512 * 1024 * 1024)], ['0 B', '1.5 KB', '10 MB', '512 MB'], 'work folder: sizes in words');

eq([readRate(' 120 '), readRate('1.5'), readRate('1000001'), readRate(''), readRate('0')], [120, null, null, null, 0], 'rate: whole credits from 0 to the bound');
eq([rateText(0), rateText(1500), dollarsPerHour(12)], ['Free', '$1.500 an hour', '0.012'], 'rate: free, or its dollars an hour');
const recDefaults: RecordingDefaults = { record: true, retention_days: 30, record_fps: 0, record_takeovers: false };
eq(readDefaults(defaultsForm(recDefaults)), { ok: true, defaults: recDefaults }, 'recording defaults: the form round-trips');
eq([readDefaults({ ...defaultsForm(recDefaults), record_fps: '6' }).ok, readDefaults({ ...defaultsForm(recDefaults), retention_days: '0' }).ok], [false, false], 'recording defaults: out-of-bounds values are refused');
sectionEnd('workstation live, recordings and work folder tests: ok');


// ---- Workstations: the Machines list's row for a station (views/station-machine.ts) ----
const stationFacts = (over: Partial<StationFacts> = {}): StationFacts => ({ os: 'macos', os_version: '15.1', arch: 'arm64', hostname: 'mini', form: 'mini', cpus: 10, ram_mb: 16384, gpus: [], displays: [], account: { mode: 'agent_user', name: 'apz-agent' }, apps: 0, virtual_displays: false, ui_tree: false, ...over });
const station = (over: Partial<StationView> = {}): StationView => ({ id: 'st1', name: 'Studio', status: 'online', online: true, draining: false, revoked: false, version: '1', code: '', joined_ms: 0, last_seen_ms: 0, address: '', facts: stationFacts(), settings: defaults, leases: [], ...over });
const lease2 = (state: StationLease['state']): StationLease => ({ id: state, station: 'st1', agent: 1, task: null, kind: 'pixel', session: null, started_ms: 0, state, position: null });
const miniView = station({ facts: stationFacts({ gpus: [{ vendor: 'apple', name: 'M4', vram_mb: 0 }] }), leases: [lease2('active'), lease2('opening'), lease2('queued')] });
const mini = stationMachine(miniView);
eq([mini.kind, mini.vendor, mini.agents, mini.id, mini.gpus], ['workstation', 'apple', 2, 'st1', [{ vendor: 'apple', name: 'M4', memory_mib: null }]], 'station machine: kind, apple vendor on macOS, agents holding the station, gpu memory null at 0 VRAM');
const pc = stationMachine(station({ facts: stationFacts({ os: 'linux', form: 'desktop', gpus: [{ vendor: 'nvidia', name: 'RTX 4090', vram_mb: 24576 }] }) }));
eq([pc.vendor, pc.gpus[0]?.memory_mib, pc.agents], [null, 24576, 0], 'station machine: no vendor off macOS, known VRAM kept, no agents without leases');
eq(stationLine(miniView),'Workstation · Mac mini · macOS 15.1 · arm64 · 10 cores · 16 GiB · apple M4', 'station line: a macOS mini, with its GPU');
eq(stationLine(station({ facts: stationFacts({ os: 'windows', os_version: '11', arch: 'x64', form: 'laptop', cpus: 1, ram_mb: 8192, gpus: [{ vendor: 'NVIDIA', name: 'RTX 4090', vram_mb: 24576 }] }) })), 'Workstation · Laptop · Windows 11 · x64 · 1 core · 8 GiB · NVIDIA RTX 4090', 'station line: a laptop, singular core, the GPU vendor kept once');
eq([stationBadge(station()), stationBadge(station({ facts: stationFacts({ account: { mode: 'owner', name: 'studio-owner' } }) }))], [null, 'agents act as studio-owner'], 'station badge: only when the agents act as the owner');
eq(leaseCounts(station({ leases: [lease2('active'), lease2('opening'), lease2('queued'), lease2('queued')] })), { leases: 4, active: 2, queued: 2 }, 'lease counts: all, holding, and queued');
const plain: MachineNode = { id: 'p1', name: 'Rig', kind: 'laptop', arch: 'x86_64', backend: 'kvm', irqchip: '', class: '', cpus: 4, memory_mib: 8192, vsock_mode: '', started_at_ms: 0, gpus: [], stats: null };
eq([isStationNode(mini), isStationNode(plain)], [true, false], 'isStationNode: true for a station row, false for a plain machine');
eq(stationThumbnailPath('a/b c'), '/v1/stations/a%2Fb%20c/thumbnail', 'station thumbnail path: the id is encoded');
sectionEnd('station machine tests: ok');

if (failedSections.length) {
  console.error(`${failedSections.length} section(s) failed: ${failedSections.join(', ')}`);
  process.exit(1);
}
