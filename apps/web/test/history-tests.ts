// The History screen's pure rules: the labels, the query presets, a change's sign-off chain and its status line, blame
// runs, the file tree and the routes. The DOM is not touched here. Bundled and run with node (see the lead's wiring);
// it prints "history tests: ok" when every assert holds.

import type { WeftBlameLine, WeftChangeSummary, WeftHistory, WeftRecord, WeftRepo } from '../src/api-weft';
import {
  QUERY_PRESETS,
  blameRuns,
  changeOfRevision,
  changeStateLabel,
  changeStateTone,
  changeStatus,
  connectionLabel,
  connectionTone,
  diffClass,
  fileTree,
  headOf,
  latestRevision,
  parseHistoryRoute,
  presetKey,
  releaseStatus,
  repoLine,
  scopedRepos,
  layoutHistory,
  signOffChain,
  sizeText,
  splitLanes,
  waitsForOwner,
  type ChainRound,
} from '../src/views/history-model';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

function ok(cond: boolean, what: string): void {
  checks++;
  if (!cond) throw new Error(`failed: ${what}`);
}

/** A signed record, with the fields the chain reads. */
function rec(seq: number, type: string, revision: string, predicate: Record<string, unknown> = {}, extra: Partial<WeftRecord> = {}): WeftRecord {
  return { seq, type, revision, actor: 'agent:301', key: 'a1b2c3d4e5f6', via_console: false, at_ms: seq * 1000, predicate, ...extra };
}

function stepOutcomes(round: ChainRound): string[] {
  return round.steps.map((s) => `${s.step}:${s.outcome}`);
}

// ---- Labels and presets --------------------------------------------------------

eq(changeStateLabel('sealed'), 'Sealed', 'sealed is named');
eq(changeStateLabel('abandoned'), 'Dropped', 'abandoned reads as dropped');
eq(changeStateLabel('weird'), 'weird', 'an unknown state passes through');
eq(changeStateTone('landed'), 'ok', 'landed is ok');
eq(changeStateTone('conflict'), 'danger', 'a conflict is danger');
eq(connectionLabel('moved'), 'Moved on its own', 'moved is named in words');
eq(connectionTone('moved'), 'warn', 'moved needs a look');
eq(connectionTone('in-step'), 'ok', 'in step is ok');

ok(releaseStatus({ signed: null, imported: null }) === 'proposed', 'an unsigned release is proposed');
ok(releaseStatus({ signed: { seq: 71, key: 'k', digest: 'd' }, imported: null }) === 'signed', 'a signed release is signed');
ok(releaseStatus({ signed: null, imported: 'tag-1' }) === 'imported', 'an imported release is imported');

eq(
  QUERY_PRESETS.map((p) => p.key),
  ['all', 'open', 'landed', 'needs', 'conflicts'],
  'the presets, in order',
);
eq(QUERY_PRESETS.find((p) => p.key === 'needs')?.query, 'needs(owner)', 'Needs me asks for needs(owner)');
eq(presetKey('needs(owner)'), 'needs', 'a preset query is its preset');
eq(presetKey('  all() '), 'all', 'spaces around a preset do not matter');
eq(presetKey(''), 'all', 'an empty query is All');
eq(presetKey('lane(w-2)'), null, 'a query of the reader’s own is no preset');
eq(presetKey('open() & needs(owner)'), null, 'a combined query is no preset');

// ---- Sign-off chain ------------------------------------------------------------

// Round 1 is sent back; round 2 goes to the owner, who rules from the console; the landing belongs to round 2.
const records: WeftRecord[] = [
  rec(38, 'gate', 'w-1.r2', { outcome: 'owner', reasons: ['security is -3 (rule: sec <= -3)'] }),
  rec(13, 'written', 'w-1.r1', { summary: 'next() now takes the oldest item' }),
  rec(25, 'gate', 'w-1.r1', { outcome: 'rework', reasons: ['tests is -3'] }),
  rec(17, 'review', 'w-1.r1', { node: 'function', pass: true, notes: 'fine' }),
  rec(19, 'review', 'w-1.r1', { node: 'quality', pass: false, notes: 'untested' }),
  rec(14, 'checked', 'w-1.r1', { tool: 'tree' }),
  rec(29, 'written', 'w-1.r2', { summary: 'tested' }),
  rec(32, 'review', 'w-1.r2', { node: 'risk', pass: true }),
  rec(39, 'ruling', 'w-1.r2', { decision: 'pass', notes: 'fine for now' }, { via_console: true, actor: 'owner' }),
  rec(42, 'landed', 'w-1.l', { ref: 'main' }, { actor: 'hub' }),
  rec(40, 'queue.add', 'w-1.r2', {}),
];

const chain = signOffChain(records);
eq(
  chain.map((r) => r.round),
  [1, 2],
  'two rounds, in order',
);
eq(
  stepOutcomes(chain[0]!),
  ['written:ok', 'checked:ok', 'review:warn', 'gate:warn', 'ruling:pending', 'landed:pending'],
  'round 1: sent back at the gate, nothing after it',
);
eq(
  chain[0]!.steps[2]!.dots.map((d) => d.seq),
  [17, 19],
  'reviews of a round keep their order',
);
eq(
  stepOutcomes(chain[1]!),
  ['written:ok', 'checked:pending', 'review:ok', 'gate:owner', 'ruling:ok', 'landed:ok'],
  'round 2: the owner ruled, and it landed',
);
ok(chain[1]!.steps[4]!.dots[0]!.console === true, 'a ruling from the console is marked');
eq(chain[1]!.steps[3]!.dots[0]!.text, 'owner: security is -3 (rule: sec <= -3)', 'a gate dot says its outcome and reasons');
eq(chain[1]!.steps[4]!.dots[0]!.signer, 'owner', 'a dot keeps the actor label, for the view to name');

const waiting = signOffChain([
  rec(20, 'written', 'w-2.r1'),
  rec(21, 'gate', 'w-2.r1', { outcome: 'owner', reasons: ['risk is -3'] }),
]);
eq(stepOutcomes(waiting[0]!)[4], 'ruling:owner', 'a ruling the gate sent to the owner waits for the owner');

eq(signOffChain([]), [], 'no records, no rounds');

// ---- Status and waiting --------------------------------------------------------

ok(waitsForOwner([rec(21, 'gate', 'w-2.r1', { outcome: 'owner' })]), 'a change sent to the owner waits');
ok(!waitsForOwner([rec(21, 'gate', 'w-2.r1', { outcome: 'owner' }), rec(22, 'ruling', 'w-2.r1', { decision: 'pass' })]), 'a ruling clears the wait');
ok(!waitsForOwner([rec(21, 'gate', 'w-2.r1', { outcome: 'owner' }), rec(22, 'gate', 'w-2.r2', { outcome: 'rework' })]), 'a later gate clears the wait');

const landedChange = { state: 'landed' as const, ref: 'main', landed: 'w-1.l', records: [] };
eq(changeStatus(landedChange).text, 'Landed on main as w-1.l.', 'a landed change says where it landed');
const ownerChange = { state: 'sealed' as const, ref: 'main', landed: null, records: [rec(21, 'gate', 'w-2.r1', { outcome: 'owner', reasons: ['security is -3'] })] };
eq(changeStatus(ownerChange), { text: 'Waiting for your ruling: security is -3', needsYou: true }, 'a waiting change asks for the owner');
eq(changeStatus({ state: 'conflict', ref: 'main', landed: null, records: [] }).needsYou, false, 'a conflict is not the owner’s to decide');

// ---- Lanes and repos -----------------------------------------------------------

function summary(change: string, state: WeftChangeSummary['state'], at: number): WeftChangeSummary {
  return {
    change,
    repo: 'pr1',
    ref: 'main',
    task: 't1',
    title: change,
    author: 'agent:300',
    state,
    kind: 'normal',
    rounds: 1,
    drafts: 1,
    files: 1,
    latest: null,
    landed: state === 'landed' ? `${change}.l` : null,
    at_ms: at,
    intents: [],
    imported: null,
  };
}

const split = splitLanes([summary('w-1', 'landed', 100), summary('w-2', 'open', 300), summary('w-3', 'landed', 200), summary('w-4', 'queued', 50)]);
eq(
  split.main.map((c) => c.change),
  ['w-3', 'w-1'],
  'landed changes are the line of main, newest first',
);
eq(
  split.lanes.map((c) => c.change),
  ['w-2', 'w-4'],
  'the other changes are the open lanes, newest first',
);

const repo = {
  name: 'pr1-queues',
  project: 'pr1',
  description: '',
  refs: { main: 'w-1.l' },
  queues: {},
  rules: [],
  connections: [],
  releases: [],
  open_changes: 0,
  landed: 1,
} as WeftRepo;
eq(headOf(repo), 'w-1.l', 'the head of main is its ref');
eq(headOf({ refs: {} }), 'root', 'before anything landed the head is root');
eq(repoLine(repo, 0), { text: 'main is at w-1.l. Nothing waits for you.', needsYou: false }, 'a quiet repo says so');
eq(repoLine(repo, 2), { text: '2 changes waiting for your ruling.', needsYou: true }, 'waiting rulings are counted');
const releasing = {
  ...repo,
  releases: [
    { version: 'v0.1.0', signed: false, imported: true, from: 'w-1.l' },
    { version: 'v0.2.0', signed: false, from: 'w-1.l' },
  ],
};
eq(repoLine(releasing, 0), { text: 'v0.2.0 waits for your signature.', needsYou: true }, 'a proposed release waits for the owner; an imported one does not');
eq(repoLine(releasing, 1).text, '1 change waiting for your ruling.', 'a ruling comes before a signature');
const moved = { ...repo, connections: [{ id: 'origin', remote: 'x', platform: 'git', branch: 'main', state: 'moved', detail: '', head: null, exported: 1, author: 'a' }] };
eq(repoLine(moved, 3), { text: 'origin moved on its own. Adopt it to bring its commits in.', needsYou: true }, 'a moved remote comes first');
eq(
  scopedRepos([repo, { ...repo, name: 'other', project: 'pr2' }], 'pr1').map((r) => r.name),
  ['pr1-queues'],
  'a project scope shows its repos',
);
eq(scopedRepos([repo], null).length, 1, 'All projects shows every repo');

// ---- Blame ---------------------------------------------------------------------

function blameLine(line: number, change: string, source: string, by: { kind: string; cid?: number }): WeftBlameLine {
  return {
    line,
    text: `line ${line}`,
    by: { change, source, start_line: line, end_line: line, model: by.kind === 'agent' ? 'claude' : null, turn: 1, tool_calls: [], by },
  };
}

const runs = blameRuns([
  blameLine(1, 'w-2', 'w-2.d1', { kind: 'agent', cid: 305 }),
  blameLine(2, 'w-2', 'w-2.d1', { kind: 'agent', cid: 305 }),
  blameLine(3, 'w-2', 'w-2.d2', { kind: 'agent', cid: 305 }),
  blameLine(4, 'w-2', 'w-2.d2', { kind: 'owner' }),
  blameLine(5, 'w-2', 'w-2.d2', { kind: 'owner' }),
]);
eq(
  runs.map((r) => [r.from, r.to, r.source, r.actorKind]),
  [
    [1, 2, 'w-2.d1', 'agent'],
    [3, 3, 'w-2.d2', 'agent'],
    [4, 5, 'w-2.d2', 'owner'],
  ],
  'blame runs break where the draft or the actor changes',
);
eq(runs[0]!.cid, 305, 'a run keeps its agent');
eq(changeOfRevision('w-12.r2'), 'w-12', 'a revision names its change');

// ---- Files ---------------------------------------------------------------------

const tree = fileTree([
  { path: 'src/queue.rs', size: 403 },
  { path: 'README.md', size: 40 },
  { path: 'src/lib/mod.rs', size: 12 },
]);
eq(
  tree.map((n) => [n.name, n.dir]),
  [
    ['src', true],
    ['README.md', false],
  ],
  'folders come first at each level',
);
eq(
  tree[0]!.children.map((n) => [n.name, n.dir]),
  [
    ['lib', true],
    ['queue.rs', false],
  ],
  'a folder lists its folders before its files',
);
eq(tree[0]!.children[1]!.path, 'src/queue.rs', 'a file keeps its full path');
eq(sizeText(512), '512 B', 'bytes');
eq(sizeText(1536), '1.5 KB', 'kilobytes');
eq(sizeText(3 * 1024 * 1024), '3.0 MB', 'megabytes');

// ---- Routes and small rules ----------------------------------------------------

eq(parseHistoryRoute([]), { view: 'repos' }, 'no repo is the repos');
eq(parseHistoryRoute(['pr1-queues']), { view: 'repo', repo: 'pr1-queues', tab: 'changes' }, 'a repo opens its changes');
eq(parseHistoryRoute(['pr1-queues', 'releases']), { view: 'repo', repo: 'pr1-queues', tab: 'releases' }, 'a tab is kept');
eq(parseHistoryRoute(['pr1-queues', 'nope']), { view: 'repo', repo: 'pr1-queues', tab: 'changes' }, 'an unknown tab falls back to changes');
eq(
  parseHistoryRoute(['pr1-queues', 'change', 'w-1']),
  { view: 'change', repo: 'pr1-queues', change: 'w-1' },
  'a change route names the change',
);

eq(latestRevision({ rounds: ['w-1.r1', 'w-1.r2'], drafts: ['w-1.d1'], landed: 'w-1.l' }), 'w-1.r2', 'the latest round is the revision');
eq(latestRevision({ rounds: [], drafts: ['w-1.d1', 'w-1.d2'], landed: null }), 'w-1.d2', 'with no round, the newest draft');
eq(latestRevision({ rounds: [], drafts: [], landed: null }), null, 'with nothing, no revision');
eq([diffClass('+'), diffClass('-'), diffClass(' ')], ['add', 'del', 'ctx'], 'diff lines by their operator');

// The history as a graph (21.6): landings on the first column, newest on top; each open lane in a column of its own,
// half a row above the landing it starts from.
const landing = (id: string, parent: string | null, title: string) => ({ id, parents: parent ? [parent] : [], title }) as unknown as WeftHistory['landings'][number];
const lane = (change: string, base: string | null) => ({ change, title: `lane ${change}`, state: 'open', round: 1, base, kind: 'feature' }) as unknown as WeftHistory['lanes'][number];
const graph = layoutHistory({
  repo: 'app',
  ref: 'main',
  head: 'l-c',
  landings: [landing('l-a', null, 'first'), landing('l-b', 'l-a', 'second'), landing('l-c', 'l-b', 'third')],
  lanes: [lane('w-5', 'l-b'), lane('w-6', 'l-c')],
} as WeftHistory);
const rowOfLanding = (id: string) => graph.nodes.find((n) => n.kind === 'landing' && n.id === id)?.row;
eq([rowOfLanding('l-c'), rowOfLanding('l-b'), rowOfLanding('l-a')], [0, 1, 2], 'the newest landing is on top');
eq(graph.nodes.filter((n) => n.kind === 'landing').map((n) => n.column), [0, 0, 0], 'landings share the first column');
const w5 = graph.nodes.find((n) => n.id === 'w-5')!;
const w6 = graph.nodes.find((n) => n.id === 'w-6')!;
eq([w5.column, w5.row, w5.parent], [1, 0.5, 'l-b'], 'a lane is in its own column, half a row above its base');
eq([w6.column, w6.row, w6.parent], [2, -0.5, 'l-c'], 'the second lane is beside the head');
eq(graph.columns, 3, 'the graph is as wide as its lanes');
eq(layoutHistory({ repo: 'app', ref: 'main', head: null, landings: [], lanes: [lane('w-7', 'l-zz')] } as WeftHistory).nodes[0].parent, null, 'a lane whose base is unknown starts from nothing');

console.log(`history tests: ok (${checks} checks)`);
