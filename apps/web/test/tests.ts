// Pure-logic tests: run with `npm test`.
import { parse, href } from '../src/router';
import { ago, clip, compact } from '../src/ui/components';
import { floorName } from '../src/roles';
import { checkRetention, clampQuery, mergeNewest, mergeOlder, spanOf } from '../src/views/conversations';
import type { Conversation } from '../src/api';

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

if (failures) {
  console.error(`${failures} failure(s)`);
  process.exit(1);
}
console.log('web tests: ok');
