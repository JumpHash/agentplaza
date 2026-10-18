// Tests of the review graph's pure helpers: the readings' and intents' colours, an intent's lowest score, the layout of
// the radial graph, the recipe's layers, and the version bump. Run as its own bundle:
//   esbuild apps/web/test/review-tests.ts --bundle --platform=node --format=esm --loader:.css=empty --outfile=<scratch>/rt.mjs && node <scratch>/rt.mjs

import { strict as assert } from 'node:assert';
import type { ChangeTree, ReviewScore, TreeFile, TreeSymbol } from '../src/api-review';
import {
  arcSpan,
  bumpVersion,
  indexTree,
  intentRadius,
  intentTone,
  intentViews,
  layerNodes,
  levelWord,
  lowestFor,
  lowestOf,
  polar,
  READING_AXES,
  readingTone,
  runStateLabel,
  scoresOf,
  spread,
  versionOf,
} from '../src/views/review-model';

const score = (symbol: string, axis: string, level: number, basis = 'measured'): ReviewScore => ({
  symbol,
  lineage: null,
  axis,
  level,
  confidence: 0.9,
  basis,
  evidence: 'test',
  node: 'quality',
});

const symbol = (id: string, scores: ReviewScore[] = []): TreeSymbol => ({
  id,
  kind: 'function',
  qualified: id.slice(id.indexOf('::') + 2),
  lineage: null,
  op: '+',
  start_line: 1,
  end_line: 2,
  add: 1,
  del: 0,
  from: null,
  provenance: [],
  touches: [],
  worded_touches: [],
  scores,
  findings: [],
});

const file = (path: string, symbols: TreeSymbol[], scores: ReviewScore[] = []): TreeFile => ({
  path,
  op: '+',
  pack: 'rust',
  binary: false,
  add: 1,
  del: 0,
  before: null,
  after: null,
  symbols,
  scores,
});

// A small change: two definitions in one file, a file-level measurement, and one definition no intent covers.
const next = symbol('src/queue.rs::impl Queue › fn next', [score('src/queue.rs::impl Queue › fn next', 'tst', -3), score('src/queue.rs::impl Queue › fn next', 'sec', -4)]);
const newType = symbol('src/queue.rs::struct Queue', [score('src/queue.rs::struct Queue', 'fea', 2)]);
const loose = symbol('src/queue.rs::fn helper', []);
const tree: ChangeTree = {
  change: 'w-1',
  revision: 'w-1.r1',
  base: 'root',
  add: 3,
  del: 0,
  files: [file('src/queue.rs', [newType, next, loose], [score('src/queue.rs', 'tst', 0)])],
  intents: [{ title: 'The queue hands out its oldest item first', kind: 'fix', summary: 'next() takes from the front.', items: ['src/queue.rs::struct Queue', 'src/queue.rs::impl Queue › fn next'] }],
  unsorted: [],
  divergence: [],
  languages: { rust: 1 },
};
const idx = indexTree(tree);

// Readings: above zero is better, below worse, zero neutral, none for no measure.
assert.equal(readingTone(-3), 'worse');
assert.equal(readingTone(0), 'neutral');
assert.equal(readingTone(2), 'better');
assert.equal(readingTone(null), 'none');
assert.equal(readingTone(undefined), 'none');
assert.equal(readingTone(Number.NaN), 'none');
assert.equal(levelWord(-3), 'much worse');
assert.equal(levelWord(-1), 'worse');
assert.equal(levelWord(3), 'much better');
assert.equal(levelWord(null), 'not measured');

// Intents: watch from -1, needs you from -3, better otherwise; nothing scored is none.
assert.equal(intentTone(null), 'none');
assert.equal(intentTone(-3), 'needs');
assert.equal(intentTone(-4), 'needs');
assert.equal(intentTone(-2), 'watch');
assert.equal(intentTone(-1), 'watch');
assert.equal(intentTone(0), 'better');
assert.equal(intentTone(4), 'better');

// The lowest score: of a list, and of an intent's definitions (with the file's own measurement counted).
assert.equal(lowestOf([{ level: 2 }, { level: -3 }, { level: 0 }]), -3);
assert.equal(lowestOf([]), null);
const [fixView, orphanView] = intentViews(idx);
assert.ok(fixView && orphanView, 'an intent and the orphan group');
assert.equal(lowestFor(idx, fixView.items), -4, 'the lowest over the intent: security -4 on next()');
assert.equal(lowestFor(idx, fixView.items, READING_AXES['code health']), -3, 'the code-health reading sees the tests score only');
assert.equal(lowestFor(idx, fixView.items, READING_AXES['cost to run']), null, 'no score on the cost axes');
assert.equal(lowestFor(idx, ['src/queue.rs::fn helper']), 0, 'the file-level tests measurement reaches its definitions');
assert.equal(scoresOf(idx, 'src/queue.rs::fn helper').length, 1, 'only the file-level score bears on a definition with none of its own');

// The views: the intent, then the group of what no intent covers, listed once each.
assert.equal(fixView.key, 'i0');
assert.equal(fixView.orphan, false);
assert.equal(orphanView.orphan, true);
assert.deepEqual(orphanView.items, ['src/queue.rs::fn helper']);

// The radial graph's layout: angles run clockwise from the top; a span spreads evenly and centred.
const top = polar(100, 100, 50, 0);
assert.ok(Math.abs(top[0] - 100) < 1e-9 && Math.abs(top[1] - 50) < 1e-9, 'angle 0 is straight up');
const right = polar(100, 100, 50, 90);
assert.ok(Math.abs(right[0] - 150) < 1e-9 && Math.abs(right[1] - 100) < 1e-9, 'angle 90 is to the right');
assert.deepEqual(spread(0, 3, 60), [-30, 0, 30]);
assert.deepEqual(spread(10, 1, 60), [10]);
assert.deepEqual(spread(0, 0, 60), []);
const quarter = arcSpan(0, 4, 6);
assert.equal(quarter.from, 3);
assert.equal(quarter.to, 87);
assert.equal(intentRadius(0), 8);
assert.equal(intentRadius(100), 22, 'the intent dot stops growing');
assert.ok(intentRadius(4) > intentRadius(1), 'more definitions, a larger dot');

// Run states, as words.
assert.equal(runStateLabel('rework'), 'Sent back');
assert.equal(runStateLabel('owner'), 'Your call');
assert.equal(runStateLabel('passed'), 'Passed');

// The recipe's layers: a step is one deeper than the deepest step it waits on; unknown links are ignored; a cycle ends.
const layers = layerNodes([
  { id: 'tree', after: [] },
  { id: 'checks', after: ['tree'] },
  { id: 'function', after: ['tree'] },
  { id: 'quality', after: ['checks', 'function'] },
  { id: 'gate', after: ['quality', 'elsewhere'] },
]);
assert.equal(layers.get('tree'), 0);
assert.equal(layers.get('checks'), 1);
assert.equal(layers.get('quality'), 2);
assert.equal(layers.get('gate'), 3);
const cyclic = layerNodes([
  { id: 'a', after: ['b'] },
  { id: 'b', after: ['a'] },
]);
assert.ok(cyclic.has('a') && cyclic.has('b'), 'a cycle does not hang and both get a layer');

// The version bump: the owner's unchanged version moves on; a version the owner typed stays; none means one is added.
const toml = 'id = "code"\nversion = 3\ntitle = "Code"\n\n[[node]]\nid = "tree"\nkind = "tool"\n';
assert.equal(versionOf(toml), 3);
assert.equal(bumpVersion(toml, 3), 'id = "code"\nversion = 4\ntitle = "Code"\n\n[[node]]\nid = "tree"\nkind = "tool"\n');
assert.equal(bumpVersion(toml.replace('version = 3', 'version = 7'), 3), toml.replace('version = 3', 'version = 7'), 'a typed version is kept');
assert.equal(bumpVersion('id = "x"\n', 1), 'version = 2\nid = "x"\n', 'a missing version is added at the top');
assert.equal(versionOf('id = "x"\n[[node]]\nversion = 9\n'), null, 'a version inside a table is not the recipe\'s');
assert.equal(bumpVersion('id = "x"\n[[node]]\nversion = 9\n', 1), 'version = 2\nid = "x"\n[[node]]\nversion = 9\n');

console.log('review tests: ok');
