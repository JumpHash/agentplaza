// The Maintenance tab's pure rules: which proposals wait and in what order, a proposal's line, what became of a
// decided one, the headline and the sweep line, the map's layout and links, the proposals by area, a signal's line.
// The DOM is not touched here. Bundled with the other suites by test/tests.ts; it prints "maintenance tests: ok".

import type { Area, Maintenance, Proposal, Signal } from '../src/api-projects';
import {
  decided,
  everyText,
  headline,
  healthTone,
  mapLinks,
  outcomeLine,
  proposalsByArea,
  ringLayout,
  signalLine,
  sweepLine,
  waitingFirst,
  weighLine,
} from '../src/views/maintenance-model';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

function proposal(over: Partial<Proposal> & Pick<Proposal, 'id'>): Proposal {
  return {
    kind: 'fix',
    title: `Proposal ${over.id}`,
    summary: 'What and why',
    impact: { changes: 'The button works', users: '', areas: [], risk: 'low', risk_note: '' },
    value: { score: 3, why: 'People asked' },
    effort: { size: 's', note: '' },
    evidence: [],
    role: 'builder',
    acceptance: '',
    state: 'proposed',
    by_cid: 301,
    by: 'Ada',
    created_ms: 1000,
    decided_ms: 0,
    note: '',
    ...over,
  };
}

function area(id: string, links: string[] = []): Area {
  return { id, name: id, summary: '', paths: [], health: 'good', notes: '', links, updated_ms: 0, by: '' };
}

function maintenance(over: Partial<Maintenance>): Maintenance {
  return {
    project: 'pr3',
    config: { every_hours: 24, watchers: [], focus: '' },
    watchers: [301],
    map: { summary: '', areas: [], updated_ms: 0, by: '' },
    signals: [],
    signals_total: 0,
    unseen: 0,
    proposals: [],
    sources: { inbox: 0, ingest: false, connectors: [], mcp: [] },
    sweep: { runs: 0, last_ms: 0, next_ms: 0, last_summary: '', last_cid: 0, seen_seq: 0 },
    ...over,
  };
}

// Waiting first: the most valuable, then the least risky, then not put off, then the newest.
const list = [
  proposal({ id: 'mp1', value: { score: 3, why: '' } }),
  proposal({ id: 'mp2', value: { score: 5, why: '' }, impact: { changes: '', users: '', areas: [], risk: 'high', risk_note: '' } }),
  proposal({ id: 'mp3', value: { score: 5, why: '' } }),
  proposal({ id: 'mp4', value: { score: 3, why: '' }, state: 'later' }),
  proposal({ id: 'mp5', value: { score: 3, why: '' }, created_ms: 2000 }),
  proposal({ id: 'mp6', state: 'declined', decided_ms: 50 }),
  proposal({ id: 'mp7', state: 'approved', decided_ms: 90, task: 't12', task_state: 'awaiting_review' }),
];
eq(waitingFirst(list).map((p) => p.id), ['mp3', 'mp2', 'mp5', 'mp1', 'mp4'], 'the waiting proposals, best first');
eq(decided(list).map((p) => p.id), ['mp7', 'mp6'], 'the decided ones, newest decision first');

eq(weighLine(list[0]!), 'Value 3/5 · low risk · small', 'a proposal weighed in a line');
eq(outcomeLine(list[6]!), 'Picked: task t12 (awaiting review)', 'a picked proposal names its task and its state');
eq(outcomeLine(proposal({ id: 'x', state: 'declined', note: 'not now' })), 'Declined: not now', 'a declined one says why');
eq(
  outcomeLine(proposal({ id: 'y', kind: 'instrument', state: 'approved', instrument: { kind: 'mcp', name: 'Sentry', catalog: 'sentry', data: 'errors' } })),
  'Picked: set up Sentry in Connectors or MCP',
  'a known instrument is set up by the owner',
);

eq(headline(maintenance({})), 'No sweep yet', 'nothing has run yet');
eq(headline(maintenance({ proposals: [list[0]!] })), 'One proposal waits for you', 'one waiting');
eq(headline(maintenance({ proposals: list })), '5 proposals wait for you', 'several waiting');
eq(headline(maintenance({ proposals: [list[5]!], sweep: { runs: 2, last_ms: 1, next_ms: 0, last_summary: '', last_cid: 301, seen_seq: 0 } })), 'Nothing waits for you', 'all answered');

eq(everyText(0), 'only when you ask', 'no schedule');
eq(everyText(1), 'every hour', 'one hour');
eq(everyText(24), 'every 24 h', 'a day');

const name = (cid: number): string => (cid === 301 ? 'Ada' : `agent ${cid}`);
const H = 3600_000;
eq(sweepLine(maintenance({}), 10 * H, name), 'The first sweep maps the project', 'before the first sweep');
eq(sweepLine(maintenance({ watchers: [] }), 10 * H, name), 'Add an agent to the project to watch it', 'no watcher');
eq(sweepLine(maintenance({ sweep: { runs: 1, last_ms: 0, next_ms: 0, last_summary: '', last_cid: 301, seen_seq: 0, running: { work: 4, cid: 302, started_ms: 0, why: 'owner' } } }), 10 * H, name), 'agent 302 is sweeping now', 'a sweep under way');
eq(sweepLine(maintenance({ sweep: { runs: 3, last_ms: 7 * H, next_ms: 0, last_summary: '', last_cid: 301, seen_seq: 0 } }), 10 * H, name), 'Last sweep 3 h ago by Ada · the next in 21 h', 'the last and the next sweep');
eq(sweepLine(maintenance({ config: { every_hours: 0, watchers: [], focus: '' }, sweep: { runs: 3, last_ms: 10 * H - 5 * 60_000, next_ms: 0, last_summary: '', last_cid: 301, seen_seq: 0 } }), 10 * H, name), 'Last sweep 5 min ago by Ada', 'no schedule, no next');

eq([healthTone('good'), healthTone('watch'), healthTone('poor'), healthTone('unknown')], ['ok', 'warn', 'danger', 'muted'], 'health tones');

// The ring: the first area at the top, the others evenly around; one area sits in the middle.
const ring = ringLayout([area('a'), area('b'), area('c'), area('d')], 400);
eq(Math.round(ring.get('a')!.x), 200, 'the first area is at the top (x)');
eq(Math.round(ring.get('a')!.y) < 200, true, 'the first area is above the middle');
eq(Math.round(ring.get('c')!.y) > 200, true, 'the third of four is below');
eq(ringLayout([area('solo')], 400).get('solo'), { x: 200, y: 200 }, 'a lone area sits in the middle');

eq(mapLinks([area('a', ['b', 'zz']), area('b', ['a', 'c']), area('c', ['c'])]), [['a', 'b'], ['b', 'c']], 'each link once, unknown and self links dropped');

const byArea = proposalsByArea([
  proposal({ id: 'p1', area: 'checkout', value: { score: 2, why: '' } }),
  proposal({ id: 'p2', impact: { changes: '', users: '', areas: ['checkout'], risk: 'low', risk_note: '' }, value: { score: 4, why: '' } }),
  proposal({ id: 'p3', area: 'search', state: 'declined' }),
  proposal({ id: 'p4' }),
]);
eq([...byArea.entries()].map(([k, v]) => [k, v.map((p) => p.id)]), [['checkout', ['p2', 'p1']]], 'waiting proposals by area, best first; no area, or decided, are left out');

const signal = (over: Partial<Signal>): Signal => ({ id: 's1', seq: 1, kind: 'report', title: '', text: '', from: 'owner', at_ms: 0, ...over });
eq(signalLine(signal({ title: 'Checkout fails', text: 'long' })), 'Checkout fails', 'a signal reads by its title');
eq(signalLine(signal({ text: 'The  pay\nbutton' })), 'The pay button', 'else by its text, on one line');
eq(signalLine(signal({ text: 'x'.repeat(200) })).length, 120, 'a long one is cut');

console.log(`maintenance tests: ok (${checks} checks)`);
