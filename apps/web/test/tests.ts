// Pure-logic tests: run with `npm test`.
import { parse, href } from '../src/router';
import { ago, clip, compact } from '../src/ui/components';
import { floorName, familyLabel, roleFamily } from '../src/roles';
import { checkRetention, clampQuery, mergeNewest, mergeOlder, spanOf } from '../src/views/conversations';
import { level, membersOf, parentOf, trailOf } from '../src/views/society';
import type { Bond, Conversation, SocietyAgent } from '../src/api';

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

if (failures) {
  console.error(`${failures} failure(s)`);
  process.exit(1);
}
console.log('web tests: ok');
