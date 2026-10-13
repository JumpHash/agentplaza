// Pure-logic tests: run with `npm test`.
import { parse, href } from '../src/router';
import { ago, clip, compact } from '../src/ui/components';
import { floorName, familyLabel, roleFamily } from '../src/roles';
import { checkRetention, clampQuery, mergeNewest, mergeOlder, spanOf } from '../src/views/conversations';
import { level, membersOf, parentOf, trailOf } from '../src/views/society';
import type { Bond, Conversation, PriceRates, SocietyAgent, UsageCall } from '../src/api';
import { easeOut, niceScale, resample, tweenArray } from '../src/ui/chart';
import { cacheSaving, fixtureCalls, ledgerSeries, ledgerSummary, mergeCalls, pageCalls, rankGroups, areaData, totalsOf, windowOf } from '../src/views/usage';
import * as THREE from 'three';
import { readdirSync } from 'node:fs';
import { join } from 'node:path';
import { buildModel, currentPulses, evalWhen, modelsFor, parseWhen, placeAtom, traceMeshes, validate, validatePalette } from '../src/views/floor3d/hardware';
import { MODELS, PALETTE, PALETTE_FILE } from '../src/views/floor3d/hardware-index';

let failures = 0;
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

// `npm test` runs from apps/web; the bundle sits in a temp dir, so the path is from the working directory.
const hwRoot = join(process.cwd(), '../../assets/hardware');
const hwFiles = ['exterior', 'board', 'parts']
  .flatMap((d) => {
    try {
      return readdirSync(join(hwRoot, d)).filter((f) => f.endsWith('.json')).map((f) => `${d}/${f.slice(0, -5)}`);
    } catch {
      return [];
    }
  })
  .sort();
eq(Object.keys(MODELS).sort(), hwFiles, 'every hardware file is in the index');
eq(Object.entries(MODELS).flatMap(([name, m]) => validate(m, name)), [], 'every hardware model validates');
eq(validatePalette(PALETTE_FILE), [], 'the palette validates');
eq(validatePalette({ schema: 1, colors: {} }).length > 0, true, 'the palette needs the status LEDs');

const hwPart = (extra: object): unknown => ({ schema: 1, kind: 'x', view: 'part', size: [1, 1, 1], parts: [{ geo: { type: 'box', w: 1, h: 1, d: 1 }, mat: 'case', ...extra }] });
const mentions = (model: unknown, word: string, name: string): void => eq(validate(model, 'bad').some((e) => e.includes(word)), true, name);
mentions(hwPart({ geo: { type: 'blob' } }), 'unknown geo type', 'an unknown geo type is an error');
mentions({ ...(hwPart({}) as object), parts: [{ geo: { type: 'box', w: 1, h: 1 }, mat: 'case' }] }, 'missing dimension "d"', 'a missing dimension is an error');
mentions(hwPart({ mat: { color: 'red' } }), 'color', 'a bad colour is an error');
mentions(hwPart({ mat: 'nope' }), 'unknown palette name', 'an unknown palette name is an error');
mentions({ ...(hwPart({}) as object), parts: [{ id: 'a', geo: { type: 'box', w: 1, h: 1, d: 1 } }, { id: 'a', geo: { type: 'box', w: 1, h: 1, d: 1 } }] }, 'duplicate id', 'a duplicate id is an error');
mentions(hwPart({ use: 'parts/missing' }), 'unknown part', 'a use of a missing part is an error');
mentions(hwPart({ when: 'gpus >>> 1' }), 'bad when', 'a bad when expression is an error');
mentions(
  { schema: 1, kind: 'x', view: 'board', size: [1, 1, 1], parts: [{ id: 'pcb', geo: { type: 'box', w: 1, h: 1, d: 1 } }], anchors: { camera: { at: [0, 1, 1], look: [0, 0, 0] }, atom: [0, 1, 0] }, slots: [{ id: 's', on: 'cpu9', type: 'cpu', top: 1, rect: [0, 0, 1, 1], capacity: 1 }] },
  'no part with id "cpu9"',
  'a slot on a missing part is an error',
);
eq(validate(hwPart({ repeat: { count: 'p_cores', step: [1, 0, 0] } }), 'ok'), [], 'a repeat count may name a param');
eq(validate(hwPart({ when: 'gpu_cores >= 30' }), 'ok'), [], 'a when may name any numeric param');

const when = (expr: string, params: Record<string, number>): boolean | string => {
  const w = parseWhen(expr);
  return w ? evalWhen(w, params) : 'bad';
};
eq(when('gpus >= 1', { gpus: 2 }), true, 'when: gpus >= 1');
eq(when('!gpus', { gpus: 0 }), true, 'when: !gpus');
eq(when('gpus', {}), false, 'when: a missing param is zero');
eq(when('gpu_cores >= 30', { gpu_cores: 16 }), false, 'when: any numeric param');
eq(when('gpus >>> 1', {}), 'bad', 'when: not an expression');

eq(modelsFor({ kind: 'laptop', vendor: 'apple' }), { exterior: 'exterior/apple-laptop', board: 'board/apple-soc' }, 'an Apple laptop');
eq(modelsFor({ kind: 'workstation', vendor: 'apple' }).exterior, 'exterior/apple-desktop', 'an Apple desktop');
eq(modelsFor({ kind: 'edge', vendor: null }), { exterior: 'exterior/edge', board: 'board/edge' }, 'a generic machine');
for (const m of [modelsFor({ kind: 'laptop', vendor: 'apple' }), modelsFor({ kind: 'server' })]) {
  eq([m.exterior in MODELS, m.board in MODELS], [true, true], `models exist: ${m.exterior}`);
}

const repeatModel = {
  schema: 1, kind: 'x', view: 'part', size: [1, 1, 1], params: { gpus: 0, n: 3 },
  parts: [
    { id: 'gpu', when: 'gpus >= 1', geo: { type: 'box', w: 1, h: 1, d: 1 }, mat: 'case' },
    { id: 'fin', repeat: { count: 'n', step: [2, 0, 0] }, geo: { type: 'box', w: 1, h: 1, d: 1 }, mat: 'case' },
  ],
};
const r0 = buildModel(repeatModel);
eq([...r0.parts.keys()].sort(), ['fin', 'fin#1', 'fin#2'], 'when hides a part; a repeat count names a param');
eq(buildModel(repeatModel, { gpus: 1 }).parts.has('gpu'), true, 'when shows a part with its param');

const statusModel = { schema: 1, kind: 'x', view: 'part', size: [1, 1, 1], parts: [{ id: 'led', geo: { type: 'sphere', r: 0.2 }, mat: 'led-ok', tag: 'status' }] };
const sb = buildModel(statusModel);
sb.update(0, { status: 'offline' });
eq((sb.parts.get('led') as THREE.Mesh).material && ((sb.parts.get('led') as THREE.Mesh).material as THREE.MeshToonMaterial).color.getHexString(), new THREE.Color(PALETTE['led-off']!.color).getHexString(), 'a status LED takes the offline colour');

// Every model builds and updates in every state, with and without GPUs.
for (const [name, m] of Object.entries(MODELS)) {
  for (const gpus of [0, 2]) {
    const b = buildModel(m, { gpus });
    b.update(0.016, { status: 'draining', working: 2, load: 0.5, open: 0.5 });
    eq(b.group.children.length > 0, true, `builds ${name} (gpus ${gpus})`);
  }
}
const board = MODELS['board/workstation']!;
placeAtom(board, new THREE.Group());
const traceModel = { schema: 1, kind: 'x', view: 'board', size: [10, 2, 10], parts: [], anchors: { camera: { at: [0, 10, 10], look: [0, 0, 0] }, atom: [0, 1, 0] }, traces: [{ path: [[0, 0], [4, 0], [4, 3]], y: 0.06, width: 0.25 }] } as unknown as Parameters<typeof traceMeshes>[0];
eq(traceMeshes(traceModel).children.length, 2, 'a trace is one copper strip per segment');
eq(currentPulses(traceModel).group.children.length, 1, 'a short trace carries one pulse');

if (failures) {
  console.error(`${failures} failure(s)`);
  process.exit(1);
}
console.log('web tests: ok');
