// The tool library screen's pure rules, over a fixture of the hub's answers: the crumbs and parents of a path, the
// rows a group shows and the filter over them, a schema's inputs, the seats line, and the reasons an entry is out of
// reach. The DOM is not touched here. Bundled with the other suites by test/tests.ts; it prints "tool library tests: ok".

import fixture from './fixtures/tool-library.json';
import type { ToolEntryDetail, ToolHit, ToolListing } from '../src/api-tool-library';
import {
  accessLabel,
  costTone,
  crumbs,
  nodeLabel,
  parentOf,
  queryOf,
  runsLabel,
  schemaRows,
  seatsLine,
  unavailableText,
  visibleNodes,
} from '../src/views/tool-library-model';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks += 1;
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`tool library: ${what}: got ${JSON.stringify(actual)}, want ${JSON.stringify(expected)}`);
  }
}

const listing = fixture.listing as ToolListing;
const root = fixture.root as ToolListing;
const entry = fixture.entry as ToolEntryDetail;
const search = fixture.search as { query: string; hits: ToolHit[] };

// The path's crumbs and its parent.
eq(crumbs(''), [], 'the root has no crumbs');
eq(crumbs('deploy.rollback'), [{ name: 'deploy', path: 'deploy' }, { name: 'rollback', path: 'deploy.rollback' }], 'the crumbs of an entry');
eq(parentOf('deploy.rollback'), 'deploy', 'an entry is in its group');
eq(parentOf('deploy'), '', 'a top-level group is in the root');

// The root's rows: groups with their counts, and the route groups left out with their reasons.
eq(root.children.map(nodeLabel), ['code (7)', 'deploy (3)', 'connectors (120)'], 'the root shows each group with its count');
eq(root.internal.map((item) => item.group), ['vault'], 'the root names the groups the library leaves out');

// A group's rows: the entries, labelled by their last segment; the filter needs every word.
eq(listing.children.map(nodeLabel), ['deploy.promote', 'deploy.rollback', 'deploy.run'].map((p) => p.slice(p.lastIndexOf('.') + 1)), 'an entry is labelled by its last segment');
eq(visibleNodes(listing.children, '').length, 3, 'no filter keeps every row');
eq(visibleNodes(listing.children, 'roll back').map((n) => n.path), ['deploy.rollback'], 'every word of the filter must appear');
eq(visibleNodes(listing.children, 'PROMOTE').map((n) => n.path), ['deploy.promote'], 'the filter ignores case');
eq(visibleNodes(listing.children, 'nothing-like-this'), [], 'a filter that matches nothing shows nothing');

// An entry's detail: its input schema as rows, the runs, the cost and the access it reads as.
eq(
  schemaRows(entry.input_schema).map((row) => [row.name, row.type, row.required]),
  [['id', 'string', true], ['env', 'string', true]],
  'the inputs of the rollback, both required',
);
eq(runsLabel(entry), 'on the hub', 'a hub entry runs on the hub');
eq(accessLabel(entry.access), 'orchestrator only', 'an operator entry is the orchestrator\'s');
eq(costTone(entry.cost), 'accent', 'a slow entry is marked slow');

// Who may use it, and why a seat may not.
eq(seatsLine(entry.seats), '1 of 2 seats may use it; not: builder.', 'the seats line names the seats left out');
eq(seatsLine([]), 'No seat is listed.', 'an entry no seat is listed for says so');
eq(unavailableText(entry), 'deploy.rollback is the orchestrator\'s: the builder seat may not use it', 'the reason an entry is out of reach');
eq(unavailableText({ available: true, reason: null }), '', 'an available entry has no reason');

// The search: its query is trimmed, and its hits carry where each one runs.
eq(queryOf('  roll   back  '), 'roll back', 'the query is trimmed and its spaces collapse');
eq(queryOf('   '), '', 'a blank query is no search');
eq(search.hits.map((hit) => hit.path), ['deploy.rollback', 'deploy.kubernetes.undo'], 'the rollback and the Kubernetes undo are both found');
eq(search.hits.map((hit) => runsLabel({ runs: hit.runs })), ['on the hub', 'on the hub'], 'each hit says where it runs');

console.log(`tool library tests: ok (${checks} checks)`);
