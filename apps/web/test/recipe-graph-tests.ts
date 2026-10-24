// The recipe graph editor's pure helpers: the TOML a save sends, the checks run before it, the edits to a draft, an
// edge's label and loop, and the layout's input. The DOM is not touched here. Bundled with the other suites by
// test/tests.ts; it prints "recipe graph tests: ok (N checks)".

import type { EngineRecipe, RecipeRunGraph } from '../src/api-review';
import {
  addEdge,
  addStep,
  edgeClassOf,
  edgeLabel,
  isLoop,
  layoutInput,
  listOf,
  nextStepId,
  nodeClassOf,
  nodeLabelOf,
  parseCount,
  parseFanOut,
  parseShare,
  patchStep,
  plainOf,
  problemOf,
  recipeToml,
  removeEdge,
  removeStep,
  renameStep,
  runIdOf,
  runLabelOf,
  runToneOf,
  runViewOf,
  stepLine,
  tomlValue,
} from '../src/views/recipe-graph-model';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

const sample = (): EngineRecipe => ({
  id: 'change',
  name: 'Change',
  version: 3,
  scope: 'built_in',
  inputs: [],
  outputs: [],
  triggers: [],
  steps: [
    { id: 'write', seat: 'worker', model: 'differ', tools: [], fan_out: 1, gate: [] },
    { id: 'review', role: 'reviewer', model: 'any', tools: ['git'], fan_out: 'per_file', gate: ['no findings'], budget_share: 0.25 },
  ],
  edges: [
    { from: 'write', to: 'review', when: 'passed' },
    { from: 'review', to: 'write', when: 'findings>0', max: 2 },
  ],
});

eq(tomlValue('say "hi"'), '"say \\"hi\\""', 'toml: a string is quoted, its quotes escaped');
eq(tomlValue(3), '3', 'toml: a number is bare');
eq(tomlValue(['a', 'b']), '["a", "b"]', 'toml: an array is inline');
eq(tomlValue({ a: 1, 'b c': 'x' }), '{ a = 1, "b c" = "x" }', 'toml: a table is inline, odd keys quoted');
eq(tomlValue(null), null, 'toml: null has no value');
eq(tomlValue(Number.NaN), null, 'toml: NaN has no value');

eq(
  recipeToml({ ...sample(), version: 4 }),
  [
    'id = "change"',
    'name = "Change"',
    'version = 4',
    'scope = "built_in"',
    '',
    '[[steps]]',
    'id = "write"',
    'seat = "worker"',
    'model = "differ"',
    'fan_out = 1',
    '',
    '[[steps]]',
    'id = "review"',
    'role = "reviewer"',
    'model = "any"',
    'tools = ["git"]',
    'fan_out = "per_file"',
    'gate = ["no findings"]',
    'budget_share = 0.25',
    '',
    '[[edges]]',
    'from = "write"',
    'to = "review"',
    'when = "passed"',
    '',
    '[[edges]]',
    'from = "review"',
    'to = "write"',
    'when = "findings>0"',
    'max = 2',
  ].join('\n') + '\n',
  'toml: the header, then a table per step and per edge; empty arrays are left out',
);

eq(problemOf(sample()), null, 'checks: the sample passes');
eq(problemOf(addStep(sample(), 'write')).includes('Two steps'), true, 'checks: a duplicate step id is refused');
eq(problemOf(addEdge(sample(), { from: 'write', to: 'ship', when: 'passed' })).includes('"ship"'), true, 'checks: an edge to no step is refused');
eq(problemOf(patchStep(sample(), 'write', { fan_out: 0 })).includes('fan_out'), true, 'checks: a fan_out of 0 is refused');
eq(problemOf({ ...sample(), edges: [{ from: 'write', to: 'review', when: 'passed', max: 0 }] }).includes('max'), true, 'checks: a max of 0 is refused');

eq('problem' in parseCount('0'), true, 'count: 0 is refused');
eq(JSON.stringify(parseCount('2')), '{"value":2}', 'count: 2 is read');
eq(JSON.stringify(parseFanOut('')), '{"value":1}', 'fan-out: empty is 1');
eq(JSON.stringify(parseFanOut('per_target')), '{"value":"per_target"}', 'fan-out: a word is kept');
eq('problem' in parseFanOut('soon'), true, 'fan-out: other words are refused');
eq(JSON.stringify(parseShare('0.5')), '{"value":0.5}', 'share: a number is read');
eq('problem' in parseShare('-1'), true, 'share: a negative share is refused');
eq(listOf(' passed, tests green ,, '), ['passed', 'tests green'], 'lists: trimmed, empty items dropped');

eq(edgeLabel(sample().edges[0]!), 'passed', 'label: a plain edge shows its condition');
eq(edgeLabel(sample().edges[1]!), 'findings>0 (max 2)', 'label: a loop shows its bound');
eq(isLoop(sample().edges[0]!), false, 'loop: an edge without a bound is not a loop');
eq(isLoop(sample().edges[1]!), true, 'loop: an edge with a bound is a loop');

eq(
  layoutInput(addEdge(sample(), { from: 'write', to: 'review', when: 'failed' })).edges,
  [
    ['write', 'review'],
    ['review', 'write'],
  ],
  'layout: a repeated pair is one link',
);
eq(layoutInput(addEdge(sample(), { from: 'write', to: 'ship', when: 'passed' })).edges.length, 2, 'layout: a link to no step is left out');

eq('seat' in patchStep(sample(), 'write', { seat: undefined }).steps[0]!, false, 'patch: a field given undefined is taken out');
eq(renameStep(sample(), 'write', 'build').edges[0]!.from, 'build', 'rename: the edges that name the step follow');
eq(removeStep(sample(), 'review').edges.length, 0, 'remove: the edges that touch the step go with it');
eq(removeEdge(sample(), 0).edges.map((e) => e.when), ['findings>0'], 'remove: the edge at the index goes');
eq(nextStepId(sample()), 'step-3', 'new id: the first step-N no step has');
eq(stepLine(sample().steps[1]!), 'any · gate no findings', 'card line: the model and the gate');
eq(stepLine(addStep(sample(), 'lint').steps.at(-1)!), 'any · gate passed', 'card line: an empty gate reads as passed');

// A run's graph as the hub sends it (GET /v1/recipe-runs/{id}/graph): a step with no walk has a null state.
const RUN_GRAPH = `{
  "run": "run-7", "recipe": "change", "state": "failed",
  "nodes": [
    { "id": "write", "seat": "worker", "role": null, "engine": null, "model": "differ", "fan_out": 1, "gate": [], "state": "passed", "visits": 1, "items": 2, "reported": 2, "gate_read": "passed" },
    { "id": "review", "seat": null, "role": "reviewer", "engine": null, "model": "any", "fan_out": "per_file", "gate": ["no findings"], "state": "failed", "visits": 2, "items": 2, "reported": 1, "gate_read": ["findings>0"] },
    { "id": "ship", "seat": null, "role": null, "engine": null, "model": "any", "fan_out": 1, "gate": [], "state": "skipped", "visits": 0, "items": 0, "reported": 0, "gate_read": null },
    { "id": "lint", "seat": null, "role": null, "engine": null, "model": "any", "fan_out": 1, "gate": [], "state": null, "visits": 0, "items": 0, "reported": 0, "gate_read": null }
  ],
  "edges": [
    { "index": 0, "from": "write", "to": "review", "when": "passed", "state": "taken", "taken": true },
    { "index": 1, "from": "review", "to": "write", "when": "findings>0", "max": 2, "state": "dead", "taken": false },
    { "index": 2, "from": "review", "to": "ship", "when": "no findings", "state": "open", "taken": false }
  ]
}`;

eq(runIdOf(['run', 'run-7']), 'run-7', 'route: #/recipe-graph/run/<id> names the run');
eq(runIdOf(['change']), null, 'route: a recipe id is not a run');
eq(runIdOf(['run']), null, 'route: the run route needs its id');
eq(runIdOf(['run', '']), null, 'route: an empty run id is no run');
eq(nodeClassOf('running'), 'rr-running', 'run: a running step takes rr-running');
eq(nodeClassOf(null), 'rr-none', 'run: a step with no walk takes rr-none');
eq(nodeLabelOf('waiting'), 'Waiting', 'run: a waiting step is labelled Waiting');
eq(nodeLabelOf(null), 'No walk', 'run: a step with no walk is labelled No walk');
eq(edgeClassOf({ state: 'taken', taken: true }), 'rr-taken', 'run: a taken link is rr-taken');
eq(edgeClassOf({ state: 'dead', taken: false }), 'rr-dead', 'run: a dead link is rr-dead');
eq(edgeClassOf({ state: 'open', taken: false }), 'rr-open', 'run: an open link is rr-open');
eq(runLabelOf('owner'), 'Waiting for you', 'run: the owner state is worded for the owner');
eq(runToneOf('failed'), 'danger', 'run: a failed run has the danger tone');
eq(plainOf(['findings>0', 'tests green']), 'findings>0, tests green', 'run: a list shows its items joined');
eq(plainOf(null), '', 'run: null shows nothing');
eq(plainOf(3), '3', 'run: a number shows as it is');

const graph = JSON.parse(RUN_GRAPH) as RecipeRunGraph;
const view = runViewOf(graph);
eq(
  view.nodes.map((n) => [n.id, n.cls, n.label]),
  [
    ['write', 'rr-passed', 'Passed'],
    ['review', 'rr-failed', 'Failed'],
    ['ship', 'rr-skipped', 'Skipped'],
    ['lint', 'rr-none', 'No walk'],
  ],
  'run view: each step takes the class and the word of its state',
);
eq(view.nodes.map((n) => [n.visits, n.items, n.reported, n.gateRead]), [
  [1, 2, 2, 'passed'],
  [2, 2, 1, 'findings>0'],
  [0, 0, 0, ''],
  [0, 0, 0, ''],
], 'run view: visits, items, reported and gate_read as the list shows them');
eq(view.edges.map((e) => e.cls), ['rr-taken', 'rr-dead', 'rr-open'], 'run view: each link takes the class of its state');
eq(view.edges.map((e) => e.taken), [true, false, false], 'run view: only the taken link is taken');
eq(view.edges.map((e) => e.dashed), [false, true, true], 'run view: the taken link is solid, the others dashed');
eq(view.recipe.steps.map((s) => s.id), ['write', 'review', 'ship', 'lint'], 'run view: the steps are laid out in the hub order');
eq(edgeLabel(view.recipe.edges[1]!), 'findings>0 (max 2)', 'run view: a loop keeps its bound in its label');
eq(layoutInput(view.recipe).edges.length, 3, 'run view: each linked pair goes to the layout once');

console.log(`recipe graph tests: ok (${checks} checks)`);
