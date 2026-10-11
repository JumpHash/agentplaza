/* Assertions for the live data layer's pure merge logic and the store's
   application of it — the piece where a plausible bug lives: serial offsets
   are capture BYTES, not string indices, and a stale GET must never rewind
   what the stream already carried.

   The repo has no JS test runner: `npm test` bundles this entry with esbuild
   (already the build tool) and executes it with node. No DOM is needed —
   the store's paint path is inert with no renderers registered. */

import {
  clipLeadingBytes,
  linesOf,
  serialDelta,
  utf8ByteLength,
  type SerialCache,
} from '../src/serial';
import { runReportText, runTranscriptText } from '../src/export';
import { fmtDateTime, fmtDur, fmtMoney, fmtStamp } from '../src/format';
import { hubLine, joinHub, parseHubStatus } from '../src/models/hub';
import type { Run, Vm, VmState } from '../src/models/run';
import { cidRange, roleOf } from '../src/models/run';
import { isFinishedTask, isOpenTask, parseTask, parseTasksView, seqOf as taskSeq, artifactPaths } from '../src/models/task';
import { parseArtifact, readStateOf } from '../src/models/artifact';
import { parseStop, parseStopReason, stopStateOf } from '../src/models/stop';
import { parseTell, parseTellArgs, tellStateOf } from '../src/models/tell';
import { balances, fmtSigned, parseJournal, parseTreasury, postingLabel } from '../src/models/treasury';
import type { Award, Bid, Call, Calls } from '../src/models/agora';
import { parseAgora, winningBid } from '../src/models/agora';
import type { Motion, Motions, OwnerDecision, Vote } from '../src/models/council';
import {
  councilTally,
  decisionStateOf,
  gateLine,
  KIND_LABEL,
  MAX_DECISION_NOTE,
  motionSeq,
  ownerReading,
  parseDecisionNote,
  parseMotion,
  parseMotions,
  SEAT_COUNT,
  STATE_LABEL,
  tallyHolds,
} from '../src/models/council';
import { sessionOf, sessionSummary } from '../src/models/session';
import {
  activePairs,
  conversationPairs,
  deliveryMark,
  deliveryText,
  parseBoard,
  parseContacts,
  parseConversations,
  parseThread,
  readLine,
  talkTally,
  upToDateLine,
  type Conversation,
} from '../src/models/social';
import {
  ROBOT_STATES,
  WORKING_LPS,
  clampPan,
  clampZoom,
  floorCountLine,
  floorLabel,
  gatherPlacements,
  gatherLinks,
  hireLinks,
  lastPostBy,
  readFloor,
  roleMonogram,
  rtsFraming,
  rtsPanOffset,
  rtsViewH,
  type FloorSource,
} from '../src/models/floor';
import { Store } from '../src/store';
import { readRoster } from '../src/views/roster';

let failures = 0;
function eq(actual: unknown, expected: unknown, name: string): void {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a === e) console.log('ok — ' + name);
  else {
    failures++;
    console.error(`FAIL — ${name}\n  expected ${e}\n  actual   ${a}`);
  }
}

/** Assert that `fn` throws and the message names `fragment`. */
function throws(fn: () => unknown, fragment: string, name: string): void {
  try {
    fn();
    failures++;
    console.error(`FAIL — ${name}\n  expected a throw containing ${fragment}, none thrown`);
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    if (msg.includes(fragment)) console.log('ok — ' + name);
    else {
      failures++;
      console.error(`FAIL — ${name}\n  expected message containing ${fragment}\n  actual   ${msg}`);
    }
  }
}

/* ---------------------------------------------------------- byte lengths */
eq(utf8ByteLength('hello'), 5, 'ascii bytes equal the length');
eq(utf8ByteLength('héllo'), 6, 'é is two bytes');
eq(utf8ByteLength('…'), 3, 'an ellipsis is three bytes');
eq(utf8ByteLength('a🙂'), 5, 'an astral character is four bytes');

/* ------------------------------------------------------------- clipping */
eq(clipLeadingBytes('abcdef', 0), 'abcdef', 'clipping nothing keeps all');
eq(clipLeadingBytes('abcdef', 3), 'def', 'three ascii bytes clipped');
eq(clipLeadingBytes('abcdef', 6), '', 'clipping everything leaves nothing');
eq(clipLeadingBytes('abcdef', 99), '', 'clipping past the end leaves nothing');
eq(clipLeadingBytes('abécd', 4), 'cd', 'a multibyte character clipped whole');
eq(clipLeadingBytes('…x', 3), 'x', 'a three-byte character clipped whole');

/* ------------------------------------------------- the merge decision */
eq(serialDelta(10, 10, 'abc'), { kind: 'append', text: 'abc' }, 'offset at the tail appends');
eq(serialDelta(10, 7, 'abc'), { kind: 'dup' }, 'a pure re-delivery is dropped');
eq(serialDelta(10, 5, '0123456789'), { kind: 'append', text: '56789' }, 'a partial re-delivery is clipped');
eq(serialDelta(10, 0, '0123456789'), { kind: 'dup' }, 'a full re-delivery is dropped');
eq(serialDelta(10, 11, 'x'), { kind: 'gap' }, 'past the tail is a gap');
eq(serialDelta(0, 0, 'boot'), { kind: 'append', text: 'boot' }, 'the first bytes append');
/* the case a .length-based merge gets wrong: the capture holds "abé" (4
   bytes) and the daemon re-sends from byte 2 ("é" is 2 bytes, then X) */
eq(serialDelta(4, 2, 'éX'), { kind: 'append', text: 'X' }, 'the clip counts bytes, not string indices');
eq(serialDelta(4, 4, '…'), { kind: 'append', text: '…' }, 'a multibyte append at the tail');
eq(serialDelta(7, 4, '…'), { kind: 'dup' }, 'a multibyte re-delivery is dropped');

/* ------------------------------------------- the line cache's byte book */
const cache = new Map<string, SerialCache>();
linesOf(cache, 'r1/0', 'abc');
eq(cache.get('r1/0')?.bytes, 3, 'the first capture is measured');
linesOf(cache, 'r1/0', 'abcdef');
eq(cache.get('r1/0')?.bytes, 6, 'append growth is measured');
linesOf(cache, 'r1/0', 'abcdef');
eq(cache.get('r1/0')?.bytes, 6, 'a repeat changes nothing');
linesOf(cache, 'r1/0', 'xyz');
eq(cache.get('r1/0')?.bytes, 3, 'a replaced capture is re-measured');
linesOf(cache, 'r1/1', 'aé\nb…');
eq(cache.get('r1/1')?.bytes, 8, 'a multibyte capture is measured');
eq(linesOf(cache, 'r1/1', 'aé\nb…'), ['aé', 'b…'], 'lines still split on multibyte text');

/* ----------------------------- the store applies it, end to end (no DOM) */
function runWith(serial: string) {
  return {
    id: 'r1',
    state: 'running' as const,
    vcpus: 2,
    memory_mib: 512,
    beats: null,
    created_at: 1,
    ended_at: null,
    vms: [{ id: 0, state: 'running' as const, serial }],
  };
}

const store = new Store();
const serialOf = () => store.details.get('r1')?.vms[0]?.serial;

store.mergeDetail(runWith('aaa\nbbb\n')); // the baseline GET: 8 bytes
eq(serialOf(), 'aaa\nbbb\n', 'the baseline lands');

store.applySerialEvent('r1', 0, 8, 'ccc\n');
eq(serialOf(), 'aaa\nbbb\nccc\n', 'an event at the tail appends');

store.applySerialEvent('r1', 0, 4, 'bbb\nccc\n');
eq(serialOf(), 'aaa\nbbb\nccc\n', 'a re-delivery does not duplicate');

store.applySerialEvent('r1', 0, 8, 'ccc\ndd');
eq(serialOf(), 'aaa\nbbb\nccc\ndd', 'a clip appends only the unseen stretch');
eq(store.linesFor('r1', 0, serialOf()), ['aaa', 'bbb', 'ccc', 'dd'], 'the line cache grew with the events');

store.applySerialEvent('r1', 0, 99, 'zzz');
eq(serialOf(), 'aaa\nbbb\nccc\ndd', 'a gap leaves the capture to the reconciling GET');

store.mergeDetail(runWith('aaa\nbbb\n')); // a GET snapshot older than the stream
eq(serialOf(), 'aaa\nbbb\nccc\ndd', 'a stale GET never rewinds the capture');

store.mergeDetail(runWith('aaa\nbbb\nccc\nddee\n')); // a GET ahead of the stream
eq(serialOf(), 'aaa\nbbb\nccc\nddee\n', 'a fresher GET replaces and re-measures');
eq(store.linesFor('r1', 0, serialOf()), ['aaa', 'bbb', 'ccc', 'ddee'], 'the line cache follows the GET');

store.applySerialEvent('r1', 0, 17, 'héllo'); // 17 bytes held; 6 bytes of multibyte text
eq(serialOf(), 'aaa\nbbb\nccc\nddee\nhéllo', 'multibyte appends at the tail');
store.applySerialEvent('r1', 0, 17, 'héllo');
eq(serialOf(), 'aaa\nbbb\nccc\nddee\nhéllo', 'a multibyte re-delivery does not duplicate');

store.applyRunEvent('r1', 'done');
eq(store.details.get('r1')?.state, 'done', 'a run event patches the cached run');
store.applyVmEvent('r1', 0, 'exited');
eq(store.details.get('r1')?.vms[0]?.state, 'exited', 'a vm event patches the cached vm');
eq(store.wire, { run: 1, vm: 1, serial: 6, task: 0, treasury: 0, agora: 0, board: 0, conversation: 0, motion: 0 }, 'the wire tallies every frame');

/* ------- run ids restart with the daemon: identity is (id, created_at) ---- */
function runShaped(id: string, createdAt: number, state: 'running' | 'done', serial: string) {
  return {
    id,
    state,
    vcpus: 2,
    memory_mib: 512,
    beats: null,
    created_at: createdAt,
    ended_at: state === 'done' ? createdAt + 5000 : null,
    vms: [{ id: 0, state: state === 'done' ? ('exited' as const) : ('running' as const), serial }],
  };
}

/* a_run_id_the_daemon_began_again_is_a_different_run */
{
  const s = new Store();
  s.mergeDetail(runShaped('r1', 1000, 'done', 'old capture\n'));
  s.applySerialEvent('r1', 0, 12, ' tail');
  eq(s.details.get('r1')?.vms[0]?.serial, 'old capture\n tail', 'the old run closed with its capture');

  /* the restarted daemon's new r1 arrives, same id, new created_at */
  s.mergeDetail(runShaped('r1', 9000, 'running', 'fresh\n'));
  eq(s.details.get('r1')?.vms[0]?.serial, 'fresh\n', 'the new run never inherits the old capture');
  eq(s.details.get('r1')?.created_at, 9000, 'the new run stands on its own identity');

  /* and the wire merges against the NEW capture's byte length, not the old */
  s.applySerialEvent('r1', 0, 6, 'boot\n');
  eq(s.details.get('r1')?.vms[0]?.serial, 'fresh\nboot\n', 'serial merges against the new capture, not the old');
  eq(s.linesFor('r1', 0, s.details.get('r1')?.vms[0]?.serial), ['fresh', 'boot'], 'the line cache forgot the old run too');
}

/* an_event_that_wakes_an_ended_run_asks_the_register_instead_of_believing_it */
{
  const s = new Store();
  s.mergeDetail(runShaped('r1', 1000, 'done', 'old capture\n'));
  s.applyRunEvent('r1', 'starting');
  eq(s.details.get('r1')?.state, 'done', 'an ended run is never woken by an event');
  s.applyVmEvent('r1', 0, 'running');
  eq(s.details.get('r1')?.vms[0]?.state, 'exited', 'a vm event does not wake the run either');
  /* the register was asked: pollList fired at once and, with no daemon here,
     its failure is recorded — the ask happened, the belief did not */
  const { promise: settled, resolve: settledDone } = Promise.withResolvers<void>();
  setTimeout(settledDone, 30);
  await settled;
  eq(typeof s.runsError, 'string', 'the register was asked for the truth');
}

/* ================= the vsock session, read out of the transcript =========
   The frozen session's own lines are the derivation: a session is in flight
   from the guest's hello until its bye; untrusted junk is never believed. */

const SESSION_LINES = [
  '[    3.110] plaza guest: agent harness starting',
  '[vsock->] {"type":"hello","vm":0,"guest_cid":10}',
  '[vsock<-] {"type":"welcome","echo":"plaza-orchestrator"}',
  '[vsock->] {"type":"tool_call","call":1,"name":"echo","args":{"text":"plaza@cid-10"}}',
  '[vsock<-] {"type":"tool_result","call":1,"ok":true,"output":"plaza@cid-10"}',
  '[vsock->] {"type":"tool_call","call":2,"name":"uptime","args":{}}',
  '[vsock<-] {"type":"tool_result","call":2,"ok":true,"output":"244073.16 5326141.84"}',
  '[vsock->] {"type":"bye","calls":2}',
  'agent-harness: session complete after 2 tool calls',
];

eq(sessionOf(['[    0.0] boot', 'plaza guest: hello world']), null, 'no session lines, no session');
eq(sessionOf(SESSION_LINES.slice(0, 2)), { phase: 'active', steps: ['hello'], calls: 0 }, 'hello opens the session');
eq(
  sessionOf(SESSION_LINES.slice(0, 3)),
  { phase: 'active', steps: ['hello', 'welcome'], calls: 0 },
  'the welcome joins the steps',
);
eq(
  sessionOf(SESSION_LINES.slice(0, 5)),
  { phase: 'active', steps: ['hello', 'welcome', 'echo✓'], calls: 1 },
  'an answered call carries its mark mid-session',
);
eq(
  sessionOf(SESSION_LINES),
  { phase: 'complete', steps: ['hello', 'welcome', 'echo✓', 'uptime✓', 'bye'], calls: 2 },
  'the frozen session reads whole',
);
eq(
  sessionSummary(sessionOf(SESSION_LINES) ?? { phase: 'active', steps: [], calls: 0 }),
  'hello→welcome→echo✓→uptime✓→bye, 2 tool calls',
  'the summary is the session in one line',
);
eq(
  sessionOf([
    '[vsock->] {"type":"hello","vm":1,"guest_cid":11}',
    '[vsock->] {"type":"tool_call","call":1,"name":"echo","args":{}}',
    '[vsock<-] {"type":"tool_result","call":1,"ok":false,"output":"refused"}',
  ]),
  { phase: 'active', steps: ['hello', 'echo✗'], calls: 1 },
  'a failed tool is data, marked ✗',
);
eq(
  sessionOf(['[vsock->] not json at all', '[vsock->] 42', '[vsock->] {"type":"mystery"}', '[vsock<-] ["array"]']),
  null,
  'untrusted junk is skipped, never believed',
);
eq(
  sessionOf([
    '[vsock->] {"type":"hello","vm":0,"guest_cid":10}',
    '[vsock->] {"type":"bye","calls":2}',
    '[vsock->] {"type":"hello","vm":0,"guest_cid":10}',
  ]),
  { phase: 'active', steps: ['hello'], calls: 0 },
  'a reboot begins a new session reading',
);
eq(
  sessionOf([
    '[vsock->] {"type":"hello","vm":0,"guest_cid":10}',
    '[vsock->] {"type":"tool_call","call":1,"name":"echo","args":{}}',
    '[vsock->] {"type":"tool_call","call":2,"name":"uptime","args":{}}',
    '[vsock->] {"type":"bye"}',
  ]),
  { phase: 'complete', steps: ['hello', 'echo', 'uptime', 'bye'], calls: 2 },
  'a bye without its count falls back to the observed calls',
);
eq(
  sessionOf([
    '[vsock->] {"type":"hello","vm":0,"guest_cid":10}',
    'agent-harness: session complete after 2 tool calls',
  ]),
  { phase: 'complete', steps: ['hello'], calls: 2 },
  'the closing note completes a session whose bye was lost',
);

/* ------------------------------------------- the run's CID range -------- */
function vmShaped(id: number, cid?: number): Vm {
  return cid == null ? { id, state: 'running' } : { id, state: 'running', guest_cid: cid };
}
const cidRun = (vms: Vm[]) => ({
  id: 'r1', state: 'running' as const, vcpus: 2, memory_mib: 512, beats: null,
  created_at: 1, ended_at: null, vms,
});
eq(cidRange(cidRun([vmShaped(0), vmShaped(1)])), null, 'no vsock, no range');
eq(cidRange(cidRun([vmShaped(0, 10)])), '10', 'a lone CID is itself');
eq(cidRange(cidRun([vmShaped(0, 11), vmShaped(1, 10), vmShaped(2, 12)])), '10–12', 'the range spans min to max');

/* ----------------------------------- the social layer’s parse ---------- */
/* The board, as `GET /api/v0/board` writes it (docs/agents/social.md, frozen
   envelopes): a post is prose written by an agent through its driver, so every
   field is checked and a mistyped one is an error naming its path. */
const boardBody = {
  posts: [
    {
      id: 'p2', agent_cid: 101, role: 'auditor', subject: 'available for t3', body: 'I can take it.',
      task: 't3', created_at: 2000, posted_by: 'dispatchd',
    },
    {
      id: 'p1', agent_cid: 100, role: 'architect', subject: 'hiring for t1', body: 'Two bids so far.',
      task: null, created_at: 1000, posted_by: 'dispatchd',
    },
  ],
  total: 2, limit: 256, truncated: false,
  unread: [{ agent_cid: 100, count: 1 }, { agent_cid: 101, count: 0 }],
};
{
  const board = parseBoard(boardBody);
  eq(board.posts.length, 2, 'the board parses its posts');
  eq(board.posts[0].id, 'p2', 'newest first, as the node lists them');
  eq(board.posts[0].task, 't3', 'a post’s task link is kept');
  eq(board.posts[1].task, null, 'a post with no task reads null, never a made-up id');
  eq(board.posts[1].posted_by, 'dispatchd', 'the driver that posted is kept');
  eq(board.unread, [{ agent_cid: 100, count: 1 }, { agent_cid: 101, count: 0 }], 'the unread column parses');
  eq(board.truncated, false, 'the bounding line parses');
  throws(() => parseBoard({ posts: [], total: 0, limit: 1 }), 'board.unread', 'a board without the unread column is named');
  throws(() => parseBoard({ posts: [{ ...boardBody.posts[0], body: 7 }], unread: [], total: 0, limit: 1, truncated: false }), 'board.posts[0].body', 'a non-string body is named at its path');
  throws(() => parseBoard({ posts: [{ ...boardBody.posts[0], id: 1 }], unread: [], total: 0, limit: 1, truncated: false }), 'board.posts[0].id', 'a non-string id is named');
  eq(parseBoard({ posts: [], total: 0, limit: 256, truncated: false, unread: [], extra: 'tolerated' }).posts.length, 0, 'an empty board parses, extras tolerated');
}

/* the contacts a card reads: wrapped by the node, oldest first */
{
  const contacts = parseContacts({
    agent_cid: 100,
    contacts: [
      { peer_cid: 101, role: 'auditor', since: 1500, kind: 'met' },
      { peer_cid: 102, role: 'builder', since: 1600, kind: 'hired' },
    ],
    total: 2, limit: 256, truncated: false,
  });
  eq(contacts.agent_cid, 100, 'the list says whose it is');
  eq(contacts.contacts[1].kind, 'hired', 'a contact’s kind parses');
  throws(() => parseContacts({ agent_cid: 100, contacts: [{ peer_cid: 1, role: 'r', since: 1, kind: 'befriended' }], total: 1, limit: 1, truncated: false }), 'contacts.contacts[0].kind', 'an unknown contact kind is refused whole');
  throws(() => parseContacts({ agent_cid: 100, contacts: [{ peer_cid: 1, role: 'r', kind: 'met' }], total: 1, limit: 1, truncated: false }), 'contacts.contacts[0].since', 'a contact without a since is named');
}

/* the conversations, and the pair rows the node itself keeps */
const convRow = (id: string, from: number, to: number, text: string, at: number): Record<string, unknown> => ({
  id, from_cid: from, to_cid: to, from_role: 'architect', to_role: 'auditor', text, created_at: at, delivered: true, delivery: null,
});
const conversationsBody = {
  conversations: [convRow('c2', 101, 100, 'on it', 3000), convRow('c1', 100, 101, 'take t3?', 2000)],
  pairs: [{ a: 100, b: 101, active: true, count: 2, last_at: 3000 }],
  active_secs: 120, total: 2, limit: 256, truncated: false,
};
{
  const view = parseConversations(conversationsBody);
  eq(view.conversations.length, 2, 'the exchanges parse');
  eq(view.conversations[0].from_role, 'architect', 'an exchange carries the roles the node records');
  eq(view.active_secs, 120, 'the activity window comes from the node');
  /* the node’s own pair rows are the authority; the exchange fallback is read
     only when they are absent (an older node, a fixture) */
  const pairs = activePairs(view, 3000);
  eq(pairs, [{ a: 100, b: 101, last_at: 3000, exchanges: 2 }], 'the node’s own pair row decides who is talking');
  const fresh = activePairs({ conversations: view.conversations, active_secs: 120 }, 3000);
  eq(fresh.length, 1, 'without pair rows the exchanges are read against the node’s window');
  const stale = activePairs({ conversations: view.conversations, active_secs: 120 }, 3000 + 121_000);
  eq(stale.length, 0, 'a pair falls out of the window after active_secs');
  /* a hostile frame: an exchange without its text is a mistyped body */
  throws(() => parseConversations({ ...conversationsBody, conversations: [{ ...convRow('c1', 1, 2, 'x', 1), text: 5 }] }), 'conversations.conversations[0].text', 'a non-string exchange text is named');
  /* the pair thread: the node sends `messages`, oldest first */
  const thread = parseThread({
    a: 100, b: 101, active: true, active_secs: 120,
    messages: [convRow('c1', 100, 101, 'take t3?', 2000), convRow('c2', 101, 100, 'on it', 3000)],
    total: 2, limit: 256, truncated: false,
  });
  eq(thread.messages.map((m) => m.id), ['c1', 'c2'], 'the thread is the node’s own order');
  eq(thread.a, 100, 'the thread echoes the pair as asked');
  throws(() => parseThread({ a: 1, b: 2, active: true, active_secs: 120, messages: [], total: 0, limit: 1 }), 'thread.truncated', 'a thread without its bounding line is named');
}

/* the board’s own lines: the read side of the node’s unread count */
eq(readLine(14, 2), 'read 12 of 14', 'the read line is the total less the unread');
eq(readLine(14, 0), 'read 14 of 14', 'an agent up to date has read all of it');
eq(readLine(3, 9), 'read 0 of 3', 'a stale cursor never reads as a negative');
eq(upToDateLine([]), 'no agent reads reported', 'no cursors reads honestly');
eq(upToDateLine([{ agent_cid: 1, count: 0 }, { agent_cid: 2, count: 3 }]), '1 of 2 agents up to date', 'the board’s summary counts the up-to-date agents');

/* ------------------------------------ the talk's own derivations -------- */
/* The pair list the filter walks, the delivery mark the hub's answer yields,
   and the tally over the console's copy. */
{
  const view = parseConversations(conversationsBody);
  eq(
    conversationPairs(view, 3000),
    [{ a: 100, b: 101, active: true, count: 2, last_at: 3000 }],
    'the pair list keeps the node’s own row',
  );
  eq(
    conversationPairs({ conversations: view.conversations, active_secs: 120 }, 3000),
    [{ a: 100, b: 101, active: true, count: 2, last_at: 3000 }],
    'without pair rows the exchanges fold into a pair, active within the window',
  );
  eq(
    conversationPairs({ conversations: view.conversations, active_secs: 120 }, 3000 + 121_000)[0]?.active,
    false,
    'the folded pair falls out of the window after active_secs',
  );
  eq(
    conversationPairs(
      { pairs: [{ a: 109, b: 103, active: false, count: 2, last_at: 9 }, { a: 100, b: 103, active: false, count: 1, last_at: 5 }] },
      0,
    ).map((p) => `${p.a}-${p.b}`),
    ['100-103', '103-109'],
    'pair rows are normalized (min, max) and sorted ascending',
  );
}
{
  const base: Conversation = {
    id: 'c1', from_cid: 1, to_cid: 2, from_role: null, to_role: null,
    text: 'x', created_at: 1, delivered: true, delivery: null,
  };
  eq(deliveryMark(base), { kind: 'delivered', text: 'delivered' }, 'a delivered exchange reads delivered');
  eq(
    deliveryMark({ ...base, delivered: false }),
    { kind: 'refused', text: 'the peer’s session did not take it' },
    'a failed delivery with no reason reads its own line',
  );
  eq(
    deliveryMark({ ...base, delivered: false, delivery: 'the hub refused: no grant' }),
    { kind: 'refused', text: 'the hub refused: no grant' },
    'the hub’s reason is carried verbatim',
  );
  eq(deliveryMark({ ...base, delivered: null }), { kind: 'unknown', text: '' }, 'a node that said neither yields no mark');
  eq(deliveryText({ ...base, delivered: false, delivery: 'the hub refused: no grant' }), 'the hub refused: no grant', 'the card’s line is the refusal’s reason');
  eq(deliveryText(base), null, 'a delivered exchange has no refusal line');
}
{
  const view = parseConversations(conversationsBody);
  eq(talkTally(view, 3000), { exchanges: 2, pairs: 1, active: 1, refused: 0 }, 'the tally counts exchanges, pairs and the active window');
  const withRefusal = talkTally(
    {
      conversations: [
        convRow('c1', 1, 2, 'x', 2900),
        { ...convRow('c2', 2, 1, 'y', 3000), delivered: false, delivery: 'nope' },
      ],
      active_secs: 120,
    },
    3000,
  );
  eq(withRefusal.refused, 1, 'a refused exchange is counted');
  eq(withRefusal.pairs, 1, 'both directions of a pair fold to one');
  eq(withRefusal.active, 1, 'the pair is active within the window');
}

/* -------------------------------- the floor’s pure derivations ---------- */
/* The layout rule: roles ascending by code unit, station r at column r mod
   ceil(sqrt(n)), row floor(r / ceil(sqrt(n))), at a fixed pitch; a role’s
   agents in ascending CID order inside its own block. */
{
  const sources: FloorSource[] = [
    { cid: 101, role: 'auditor', runId: 'r1', vmId: 1, lines: 4, lps: 0, vmState: 'running' },
    { cid: 100, role: 'architect', runId: 'r1', vmId: 0, lines: 2, lps: 0.5, vmState: 'running' },
    { cid: 102, role: 'builder', runId: 'r1', vmId: 2, lines: 0, lps: 0, vmState: 'running' },
  ];
  const read = readFloor(sources, new Set([100, 101]), null, [], 1000);
  eq(read.stations.map((s) => s.role), ['architect', 'auditor', 'builder'], 'the stations are the roles, ascending');
  eq(read.stations.length, 3, 'one station per role');
  eq(read.stations[0].x, 0, 'the first station stands at the origin');
  eq(read.stations[2].y > read.stations[0].y, true, 'a fourth station wraps to a second row');
  eq(read.width > 0 && read.height > 0, true, 'the plan has an extent');
  eq(read.counts, { talking: 0, working: 1, idle: 1, unreachable: 1 }, 'states: working past the threshold, idle, and the hub’s own absences');
  eq(read.robots.find((r) => r.source.cid === 100)?.state, 'working', `${WORKING_LPS} lines/s is working`);
  eq(read.robots.find((r) => r.source.cid === 102)?.state, 'unreachable', 'a register row the hub does not list is unreachable');
  eq(read.hubReadable, true, 'a readable hub is what lets a robot read unreachable');
  /* the join key is stable across renders: a robot the register gives no CID
     keys by its role, not by its index */
  const keyed = readFloor([{ cid: null, role: 'planner', runId: 'r2', vmId: 0, lines: 0, lps: 0, vmState: 'running' }], null, null, [], 0);
  eq(keyed.robots[0].key, 'role:planner#0', 'a CID-less row keys by role and rank');
  eq(keyed.robots[0].state, 'idle', 'with the hub unreadable no robot reads unreachable');
  eq(keyed.hubReadable, false, 'an unreadable hub is said so');
  /* no source at all: a blank register draws no robots and no stations */
  const blank = readFloor([], null, null, [], 0);
  eq(blank.robots.length, 0, 'a blank register draws no robots');
  eq(floorCountLine(blank), 'no agents on the floor', 'a blank floor says so');
  /* the monogram and the count line */
  eq(roleMonogram('legal-agents-counsel'), 'LA', 'a multi-word role monograms its first two words');
  eq(roleMonogram('architect'), 'AR', 'a single-word role monograms its first two letters');
  eq(roleMonogram(''), '??', 'a role with no letters monograms nothing');
  eq(floorCountLine(read), '3 agents · 1 working · 1 idle · 1 unreachable', 'the count line names each state it sees');
  eq(ROBOT_STATES.join(','), 'talking,working,idle,unreachable', 'the state order is fixed');
}

/* a conversation walks the pair adjacent: both robots stand at the midpoint
   of their own homes, PAIR_GAP apart, ordered by CID */
{
  const homes = new Map([[100, { x: 0, y: 0 }], [101, { x: 100, y: 0 }], [102, { x: 0, y: 200 }]]);
  const one = gatherPlacements([{ a: 100, b: 101, kind: 'conversation', at: 5 }], homes);
  eq(one.gatherings.length, 1, 'a pair is one gathering');
  eq(one.gatherings[0].x, 50, 'the pair meets at the midpoint of their stations');
  eq(one.targets.get(100)?.kind, 'conversation', 'the walk is a conversation');
  eq(Math.abs((one.targets.get(101)?.x ?? 0) - (one.targets.get(100)?.x ?? 0)) > 0, true, 'the pair stands apart, adjacent');
  eq(one.targets.get(102), undefined, 'an agent in no gathering stays at its station');
  /* three agents linked pairwise stand in one row at their own centroid */
  const three = gatherPlacements(
    [
      { a: 100, b: 101, kind: 'conversation', at: 1 },
      { a: 101, b: 102, kind: 'conversation', at: 2 },
    ],
    homes,
  );
  eq(three.gatherings.length, 1, 'a chain of links is one gathering');
  eq(three.gatherings[0].cids, [100, 101, 102], 'its members are the linked CIDs, ascending');
  /* a hiring outranks nothing: the same pair with a conversation is talking */
  const mixed = gatherLinks([{ a: 100, b: 101, last_at: 9 }], [
    { task: 't1', author_cid: 101, contractor_cid: 100, at: 1 },
  ]);
  eq(mixed, [{ a: 100, b: 101, kind: 'conversation', at: 9 }], 'a conversation outranks a hiring on the same pair');
  const hireOnly = gatherLinks([], [{ task: 't1', author_cid: 100, contractor_cid: 101, at: 3 }]);
  eq(hireOnly, [{ a: 100, b: 101, kind: 'hire', at: 3 }], 'a hiring alone draws its own joint');
}

/* a hiring is derived from the board and the ledger: the award’s note names
   the task, the ledger’s copy names the contractor, the note’s author is the
   awarder (the agora’s own template — docs/agents/social.md) */
{
  const posts = [
    { id: 'p3', agent_cid: 100, role: 'mayor', subject: 'hiring for t1', body: 'awarded', task: 't1', created_at: 3000, posted_by: 'noded' },
    { id: 'p2', agent_cid: 101, role: 'builder', subject: 'available for t1', body: 'me', task: 't1', created_at: 2000, posted_by: 'dispatchd' },
    { id: 'p1', agent_cid: 102, role: 'auditor', subject: 'hiring for t9', body: 'nothing on the ledger', task: null, created_at: 1000, posted_by: 'noded' },
  ];
  const tasks = [
    { id: 't1', goal: 'g', acceptance: null, budget: 0, state: 'claimed' as const, role: 'builder', assignee_id: 'builder', agent_cid: 101, verifier_id: null, note: null, created_at: 1, updated_at: 2 },
  ];
  const hires = hireLinks(posts, tasks);
  eq(hires.length, 1, 'only a note whose task the ledger names is a hiring');
  eq(hires[0], { task: 't1', author_cid: 100, contractor_cid: 101, at: 3000 }, 'the pair is the note’s author and the task’s agent');
  eq(hires[0].author_cid !== hires[0].contractor_cid, true, 'a robot never walks to itself');
  eq(lastPostBy(posts, 101)?.id, 'p2', 'the last post by an agent is the newest it wrote');
  eq(lastPostBy(posts, 999), null, 'an agent that never posted has no last post');
}

/* the floor’s own live label: the fleet by state, then the conversations */
{
  const sources: FloorSource[] = [
    { cid: 100, role: 'architect', runId: 'r1', vmId: 0, lines: 0, lps: 0, vmState: 'running' },
    { cid: 101, role: 'auditor', runId: 'r1', vmId: 1, lines: 0, lps: 0, vmState: 'running' },
  ];
  const conversations: Conversation[] = [
    { id: 'c1', from_cid: 100, to_cid: 101, from_role: 'architect', to_role: 'auditor', text: 'hi', created_at: 1000, delivered: true, delivery: null },
  ];
  const read = readFloor(sources, new Set([100, 101]), { conversations, pairs: [], active_secs: 120 }, [], 1000);
  eq(read.counts.talking, 2, 'both ends of an active pair are talking');
  eq(floorLabel(read, { 100: 'architect', 101: 'auditor' }), 'Plaza floor. 2 agents · 2 talking. architect with auditor.', 'the live label names the fleet and the pair');
  eq(read.gatherings.length, 1, 'the pair is drawn gathered');
  const quiet = readFloor(sources, new Set([100, 101]), { conversations, pairs: [], active_secs: 120 }, [], 1000 + 121_000);
  eq(quiet.counts.talking, 0, 'the window closes and nobody is talking');
  eq(floorLabel(quiet, {}), 'Plaza floor. 2 agents · 2 idle. No conversation is active.', 'an idle plaza says no conversation is active');
}

/* ------------------------------------- the RTS camera ------------------- */
{
  const tilt = (57 * Math.PI) / 180;
  const view = rtsViewH(1168, 630, 2);
  eq(view >= 630 * Math.sin(tilt), true, 'the frustum holds the plan’s foreshortened depth');
  eq(rtsViewH(100, 100, 0.2) >= rtsViewH(100, 100, 2), true, 'a narrow canvas needs a taller frustum');
  const frame = rtsFraming(1000, 500, 1.6, 1);
  eq(frame.target, [0, 0, 0], 'the camera aims at the plan’s centre');
  eq(frame.pos[1] > 0, true, 'the camera stands above the ground');
  eq(frame.pos[1] / Math.hypot(frame.pos[0], frame.pos[2]) > 1, true, 'and raked steeper than 45°');
  const zoomed = rtsFraming(1000, 500, 1.6, 2);
  eq(zoomed.view_h < frame.view_h, true, 'zooming narrows the frustum, never the plan');
  /* a drag pans against the pointer, and the pan is held inside the plan */
  const [ox, oz] = rtsPanOffset(10, 0, 1);
  eq(Math.hypot(ox, oz) > 0, true, 'a drag moves the aim');
  eq(rtsPanOffset(0, 0, 5), [0, 0], 'a drag of nothing moves nothing');
  const held = clampPan({ x: 1e6, z: -1e6 }, 1000, 500);
  eq(Math.abs(held.x) < 1000 && Math.abs(held.z) < 500, true, 'the aim is clamped inside the plan');
  eq(clampPan({ x: 1, z: 2 }, 1000, 500), { x: 1, z: 2 }, 'a small pan is left alone');
  eq(clampZoom(100), 3.2, 'the zoom has a ceiling');
  eq(clampZoom(0), 0.6, 'and a floor');
}

/* ------------------------------------- the run report, byte for byte ---- */
eq(fmtDateTime(null), '—', 'no time reads as a dash');
eq(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/.test(fmtDateTime(1791664501432)), true, 'the report’s timestamps are self-contained');

{
  const reportRun = {
    id: 'r1', state: 'done' as const, vcpus: 2, memory_mib: 512, beats: null,
    created_at: 1000, ended_at: 6000,
    vms: [
      { id: 0, state: 'exited' as const, guest_cid: 10 },
      { id: 1, state: 'exited' as const, guest_cid: 11 },
    ],
  };
  const linesOfVm = (vm: { id: number }): string[] =>
    vm.id === 0
      ? ['[    0.0] boot', '[vsock->] {"type":"hello","vm":0,"guest_cid":10}', '[vsock->] {"type":"bye","calls":0}', 'reboot: Power down']
      : ['[    0.0] boot', 'reboot: Restarting system'];
  const expected =
    `══ run r1 · report ══\n` +
    `state: done\n` +
    `opened: ${fmtDateTime(1000)}\n` +
    `closed: ${fmtDateTime(6000)}\n` +
    `elapsed: 5 s\n` +
    `shape: 2 VMs × 2 vCPUs · 512 MiB each · steady · runs until killed\n` +
    `vsock: guest cids 10–11\n` +
    `guests:\n` +
    `  vm 0 · cid 10 · exited · Slept · S5\n` +
    `  vm 1 · cid 11 · exited · Reset\n` +
    `\n` +
    `══ run r1 · vm 0 · cid 10 · exited · 4 lines ══\n` +
    `session: hello→bye, 0 tool calls\n` +
    `[    0.0] boot\n[vsock->] {"type":"hello","vm":0,"guest_cid":10}\n[vsock->] {"type":"bye","calls":0}\nreboot: Power down\n` +
    `\n` +
    `══ run r1 · vm 1 · cid 11 · exited · 2 lines ══\n` +
    `[    0.0] boot\nreboot: Restarting system\n` +
    `\n`;
  eq(runReportText(reportRun, linesOfVm), expected, 'the report assembles byte for byte');
  /* the shared section rule keeps the plain run export byte-stable */
  eq(
    runTranscriptText(reportRun, linesOfVm),
    `══ run r1 · vm 0 · cid 10 · exited · 4 lines ══\n` +
    `[    0.0] boot\n[vsock->] {"type":"hello","vm":0,"guest_cid":10}\n[vsock->] {"type":"bye","calls":0}\nreboot: Power down\n` +
    `\n` +
    `══ run r1 · vm 1 · cid 11 · exited · 2 lines ══\n` +
    `[    0.0] boot\nreboot: Restarting system\n` +
    `\n`,
    'the plain run export is byte-stable across the shared rule',
  );
  /* a VM whose session never completed gets no session line */
  const activeRun = { ...reportRun, vms: [{ id: 0, state: 'running' as const, guest_cid: 10 }] };
  const activeLines = (): string[] => ['[vsock->] {"type":"hello","vm":0,"guest_cid":10}'];
  eq(
    runReportText(activeRun, activeLines).includes('session:'),
    false,
    'an in-flight session is not summarised as done',
  );
}

/* ==================== the role fleet: roles, roster, long runs =========
   The register's VM rows gained an optional `role`; a node boots one
   long-running agent per role, so the console must read the field
   defensively, group it into a roster, and keep a fleet's durations honest
   past a day. */

/* roleOf is defensive: the field is optional and its value comes off the wire */
eq(roleOf({ id: 0, state: 'running' }), null, 'a VM without a role reads as none');
eq(roleOf({ id: 0, state: 'running', role: 'architect' }), 'architect', 'a named role reads through');
eq(roleOf({ id: 0, state: 'running', role: '' }), null, 'a blank role is no role');
eq(roleOf({ id: 0, state: 'running', role: 42 as unknown as string }), null, 'a hostile non-string role is no role');
eq(roleOf(null), null, 'no VM, no role');

/* durations past a day roll into days — a role fleet runs for hours, and
   "72 h 00 m" is not a duration this register would ever write */
eq(fmtDur(59_000), '59 s', 'under a minute reads in seconds');
eq(fmtDur(90_000), '1 m 30 s', 'minutes and seconds');
eq(fmtDur(3 * 3_600_000 + 5 * 60_000), '3 h 05 m', 'hours and minutes');
eq(fmtDur(23 * 3_600_000 + 59 * 60_000), '23 h 59 m', 'the last hour still reads in hours');
eq(fmtDur(24 * 3_600_000), '1 d 00 h 00 m', 'the day boundary rolls into days');
eq(fmtDur(50 * 3_600_000 + 90_000), '2 d 02 h 01 m', 'days, hours and minutes');

/* a row's stamp carries the date once the day differs, so a fleet opened
   yesterday never reads as though it opened this morning */
const TODAY = new Date(2026, 9, 10, 12, 0, 0).getTime(); // 2026-10-10, local noon
eq(fmtStamp(new Date(2026, 9, 10, 21, 50, 5).getTime(), TODAY), '21:50:05', 'a moment today is its clock time');
eq(fmtStamp(new Date(2026, 9, 9, 23, 28, 1).getTime(), TODAY), '10-09 23:28', 'another day carries its date');
eq(fmtStamp(new Date(2025, 11, 31, 23, 59, 0).getTime(), TODAY), '2025-12-31 23:59', 'another year carries the year');
eq(fmtStamp(null, TODAY), '—', 'no time reads as a dash');

/* the roster: role VMs grouped by role, ordered and tallied; a register with
   no roles reads as an empty roster, never a placeholder one */
function fleetVm(id: number, role: string | undefined, state: VmState, cid?: number): Vm {
  const vm: Vm = { id, state };
  if (role != null) vm.role = role;
  if (cid != null) vm.guest_cid = cid;
  return vm;
}
function fleetRun(id: string, vms: Vm[], state: 'running' | 'done' = 'running'): Run {
  return { id, state, vcpus: 1, memory_mib: 128, beats: null, created_at: 1_000, ended_at: state === 'done' ? 2_000 : null, vms };
}
const fleetRuns = [
  fleetRun('r7', [
    fleetVm(0, 'builder', 'running', 101),
    fleetVm(1, undefined, 'running', 99), // an ordinary guest: not a roster row
    fleetVm(2, 'architect', 'running', 100),
  ]),
  fleetRun('r8', [fleetVm(0, 'architect', 'exited', 120)], 'done'),
];
const roster = readRoster(
  fleetRuns,
  (run, vm) => run.id === 'r7' && vm.id === 2 ? 40 : 7,
  (run, vm) => (vm.state === 'running' ? 1.5 : 0),
);
eq(roster.roles, 2, 'the roster counts the roles the register names');
eq(roster.agents, 3, 'the roster holds only the VMs with roles');
eq(roster.running, 2, 'the roster counts the agents running now');
eq(roster.groups.map((g) => g.role), ['architect', 'builder'], 'roles are grouped in alphabetical order');
eq(roster.groups[0]?.agents.map((a) => a.cid), [100, 120], 'a role\'s agents come by guest CID');
eq(roster.groups[1]?.agents.map((a) => a.vmId), [0], 'an ordinary guest is not a roster row');
eq(
  roster.groups[0]?.agents.map((a) => [a.runId, a.state, a.open, a.lines, a.lps]),
  [['r7', 'running', true, 40, 1.5], ['r8', 'exited', false, 7, 0]],
  'each agent carries its place, state, lines and rate',
);
eq(readRoster([fleetRun('r1', [fleetVm(0, undefined, 'running', 10)])], () => 0, () => 0), { groups: [], agents: 0, running: 0, roles: 0 }, 'a register with no roles reads as an empty roster');

/* ==================== the hub: parse, line, and the connected join ======
   The hub's status comes through the daemon's proxy and is read through a
   defensive parse (the relay is untrusted): a missing field is an error
   naming it, unknown extras are tolerated. The line must read the same
   whether the hub is empty or merely unreadable — and the join must never
   merge the two sides silently. */

const HUB_FIXTURE = {
  node: { cids: [10, 11, 100, 101], port: 5000, uptime_s: 3840, max_sessions: 64, keepalive_idle_s: 300 },
  agents: [
    { cid: 100, role: 'architect', connected_at_ms: 1000, last_heartbeat_ms: 4200, calls: 3 },
    { cid: 101, role: 'auditor', connected_at_ms: 1100, last_heartbeat_ms: 4000, calls: 0 },
  ],
  sessions_active: 2,
};

eq(parseHubStatus(HUB_FIXTURE), HUB_FIXTURE, 'a well-formed hub status parses unchanged');
eq(
  parseHubStatus({ ...HUB_FIXTURE, extra: 1, node: { ...HUB_FIXTURE.node, extra: 2 } }),
  HUB_FIXTURE,
  'unknown extra fields are tolerated and dropped',
);
throws(() => parseHubStatus(null), 'hub is missing or not an object', 'a non-object status is rejected by name');
throws(() => parseHubStatus({ agents: [], sessions_active: 0 }), 'hub.node', 'a missing node is an error naming it');
throws(() => parseHubStatus({ node: HUB_FIXTURE.node, sessions_active: 0 }), 'hub.agents', 'a missing agents array is an error naming it');
throws(
  () => parseHubStatus({ node: { cids: [10], uptime_s: 1, max_sessions: 1, keepalive_idle_s: 1 }, agents: [], sessions_active: 0 }),
  'hub.node.port',
  'a missing node.port names the field',
);
throws(() => parseHubStatus({ ...HUB_FIXTURE, agents: [{ role: 'x', connected_at_ms: 1, last_heartbeat_ms: 2, calls: 0 }] }), 'hub.agents[0].cid', 'a missing agent field names its path');
throws(() => parseHubStatus({ ...HUB_FIXTURE, node: { ...HUB_FIXTURE.node, cids: ['x'] } }), 'hub.node.cids[0]', 'a non-integer cid names its index');
throws(() => parseHubStatus({ ...HUB_FIXTURE, sessions_active: 'two' }), 'hub.sessions_active', 'a mistyped count is an error naming it');
eq(parseHubStatus(HUB_FIXTURE) instanceof Error, false, 'a good parse is a value, not a throw');

/* the hub line's voice: the register's own numerals and durations */
const parsedFixture = parseHubStatus(HUB_FIXTURE);
eq(
  hubLine(parsedFixture, null),
  'hub · 4 cid(s) served · 2 agent(s) connected · 2 session(s) active · up 1 h 04 m',
  'the hub line reads the fleet in the register voice',
);
const emptyHub = parseHubStatus({ node: { cids: [1, 2], port: 5000, uptime_s: 5, max_sessions: 8, keepalive_idle_s: 300 }, agents: [], sessions_active: 0 });
const emptyLine = hubLine(emptyHub, null);
eq(emptyLine, 'hub · 2 cid(s) served · 0 agent(s) connected · 0 session(s) active · up 5 s', 'an empty hub still names its CIDs and its zeroes');
const deadLine = hubLine(null, 'hub unreachable: cannot connect to 127.0.0.1:19080: Connection refused');
eq(deadLine, 'hub · unreachable — hub unreachable: cannot connect to 127.0.0.1:19080: Connection refused', 'an unreadable hub says so and why');
eq(hubLine(null, null), 'hub · reading…', 'before the first poll the line reads');
eq(emptyLine !== deadLine, true, 'an empty hub and an unreadable hub are distinct readings');

/* the join: keyed by guest CID, honest about who knows whom */
{
  const registered = [
    { cid: 100, role: 'architect' },
    { cid: 102, role: 'builder' },
    { cid: null, role: 'no-vsock' },
  ];
  const connected = parseHubStatus(HUB_FIXTURE).agents.concat([
    { cid: 120, role: 'ghost', connected_at_ms: 5000, last_heartbeat_ms: 5100, calls: 0 },
  ]);
  const join = joinHub(registered, connected);
  eq(join.map((r) => r.cid), [100, 101, 102, 120, null], 'the join is ordered by CID, the CID-less row last');
  eq(
    join.map((r) => r.presence),
    ['both', 'connected-only', 'registered-only', 'connected-only', 'registered-only'],
    'each row is honest about which side knows it',
  );
  eq(join[0]?.role, 'architect', 'a matched row carries the register role');
  eq(join[0]?.connected?.calls, 3, 'a matched row carries the hub facts');
  eq(join[1]?.role, 'auditor', 'a hub-only row is named by the hub');
  eq(join[1]?.registered, null, 'a hub-only row has no register entry');
  eq(join[2]?.connected, null, 'a register-only row has no hub facts');
  eq(join[3]?.role, 'ghost', 'a second hub-only row is named by the hub too');
  eq(join[4]?.cid, null, 'a role row without a vsock CID stays its own register-only row');
  eq(joinHub([], []).length, 0, 'two empty sides join to nothing');
  eq(
    joinHub([], parseHubStatus(HUB_FIXTURE).agents).map((r) => r.presence),
    ['connected-only', 'connected-only'],
    'a register that knows none of the hub still shows every connected agent',
  );
}
{
  const j = joinHub([{ cid: 5, role: 'register-role' }], [{ cid: 5, role: 'hub-role', connected_at_ms: 1, last_heartbeat_ms: 2, calls: 0 }]);
  eq(j[0]?.role, 'register-role', 'when both know the CID the register names the role');
  eq(j[0]?.connected?.role, 'hub-role', 'the hub’s role stays available for the view to flag');
}

/* ==================== the ledger: parse, money, and the store's copy ======
   The task ledger comes off the daemon's API and is read through a defensive
   parse (the relay is untrusted): every field is checked, a missing or
   mistyped one is an error naming its path, unknown extras are tolerated. The
   store holds the ledger as ONE copy, and the wire's task events patch it in
   place — with the register's identity guard (ids restart with the daemon). */

const TASK_FIXTURE = {
  id: 't1',
  goal: 'write the plaza handbooks',
  acceptance: 'all 21 role handbooks present',
  budget: 2000,
  role: 'librarian',
  state: 'done',
  outcome: 'accepted',
  assignee_id: 'librarian',
  verifier_id: null,
  agent_cid: 112,
  note: '21 handbooks committed',
  created_at: 1791671813178,
  updated_at: 1791671813568,
};
const UNCLAIMED = {
  ...TASK_FIXTURE,
  state: 'open',
  outcome: null,
  assignee_id: null,
  agent_cid: null,
  note: null,
};

eq(parseTask(TASK_FIXTURE), TASK_FIXTURE, 'a well-formed task parses unchanged');
eq(parseTask({ ...TASK_FIXTURE, extra: 1, party: 'x' }), TASK_FIXTURE, 'unknown extra fields are tolerated and dropped');
eq(parseTask(UNCLAIMED), UNCLAIMED, 'an unclaimed task reads every nullable column as null');
throws(() => parseTask(null), 'task is missing or not an object', 'a non-object task is rejected by name');
throws(() => parseTask({ ...TASK_FIXTURE, id: 7 }), 'task.id', 'a mistyped id names the field');
throws(() => parseTask({ ...TASK_FIXTURE, budget: 1.5 }), 'task.budget', 'a non-integer budget names the field');
throws(() => parseTask({ ...TASK_FIXTURE, state: 'pending' }), 'task.state', 'an unknown state is refused by name');
throws(() => parseTask({ ...TASK_FIXTURE, outcome: 'maybe' }), 'task.outcome', 'an unknown outcome is refused by name');
throws(() => parseTask({ ...TASK_FIXTURE, note: 5 }), 'task.note is not a string or null', 'a mistyped note names it');
throws(() => parseTask({ ...TASK_FIXTURE, agent_cid: 'x' }), 'task.agent_cid', 'a mistyped agent CID names it');
{
  const missing: Record<string, unknown> = { ...TASK_FIXTURE };
  delete missing['updated_at'];
  throws(() => parseTask(missing), 'task.updated_at', 'a missing required field names the path');
}

eq(parseTasksView({ tasks: [TASK_FIXTURE] }), [TASK_FIXTURE], 'the list endpoint parses its tasks');
eq(parseTasksView({ tasks: [] }), [], 'an empty ledger parses to nothing');
throws(() => parseTasksView({}), 'ledger.tasks is missing or not an array', 'a body without tasks names the field');
throws(() => parseTasksView({ tasks: [{ ...TASK_FIXTURE, goal: 1 }] }), 'ledger.tasks[0].goal', 'a bad row names its index and field');

eq(taskSeq('t12'), 12, 'the task folio is its number');
eq(taskSeq('r3'), 0, 'a run id is not a task folio');
eq(isOpenTask({ ...TASK_FIXTURE, state: 'open' }), true, 'an open task is in flight');
eq(isOpenTask({ ...TASK_FIXTURE, state: 'claimed' }), true, 'a claimed task is in flight');
eq(isFinishedTask({ ...TASK_FIXTURE, state: 'done' }), true, 'a done task has ended');
eq(isFinishedTask({ ...TASK_FIXTURE, state: 'withdrawn' }), true, 'a withdrawn task has ended');

/* money reads both ways off the codex's own rate (1 credit = $0.001) */
eq(fmtMoney(2000), '2,000 cr · $2.00', 'a budget reads in credits and dollars');
eq(fmtMoney(0), '0 cr · $0.00', 'a zero budget reads plainly');
eq(fmtMoney(1500), '1,500 cr · $1.50', 'the dollar column follows the codex rate');

/* the store's ledger: one copy, newest first, patched in place by the wire */
{
  const s = new Store();
  s.mergeTask(parseTask({ ...UNCLAIMED, id: 't1' }));
  s.mergeTask(parseTask({ ...UNCLAIMED, id: 't2' }));
  eq(s.tasks.map((t) => t.id), ['t2', 't1'], 'the ledger reads newest first');
  eq(s.taskOf(null), null, 'no id, no task');
  s.applyTaskEvent('t1', 'claimed', 'librarian');
  eq(s.taskOf('t1')?.state, 'claimed', 'a task event patches the ledger in place');
  eq(s.taskOf('t1')?.assignee_id, 'librarian', 'the event carries the assignee');
  eq(s.wire.task, 1, 'the wire tallies the task frame');
  s.mergeTask(parseTask({ ...TASK_FIXTURE, id: 't1' })); // the reconciling list row
  eq(s.tasks.map((t) => t.id), ['t2', 't1'], 'a merged task keeps its place');
  eq(s.taskOf('t1')?.note, '21 handbooks committed', 'a merged task replaces the patched row');
  const { promise: settled, resolve: settledDone } = Promise.withResolvers<void>();
  setTimeout(settledDone, 250);
  await settled;
  eq(typeof s.tasksError, 'string', 'the event asked the ledger to reconcile');
}

/* an event for a task the console has never seen asks the ledger, coalesced */
{
  const s = new Store();
  s.applyTaskEvent('t9', 'open', null);
  eq(s.tasks.length, 0, 'no row is invented for an unknown task');
  const { promise: settled, resolve: settledDone } = Promise.withResolvers<void>();
  setTimeout(settledDone, 250);
  await settled;
  eq(typeof s.tasksError, 'string', 'the ledger was asked for the truth');
}

/* a finished task is never woken by an event: ids restart with the daemon */
{
  const s = new Store();
  s.mergeTask(parseTask(TASK_FIXTURE)); // t1, done/accepted
  s.applyTaskEvent('t1', 'claimed', 'librarian');
  eq(s.taskOf('t1')?.state, 'done', 'an ended task is not woken by an event');
  eq(s.taskOf('t1')?.assignee_id, 'librarian', 'the event did not touch the ended row');
  const { promise: settled, resolve: settledDone } = Promise.withResolvers<void>();
  setTimeout(settledDone, 30);
  await settled;
  eq(typeof s.tasksError, 'string', 'the ledger was asked at once for identity');
}

/* ==================== the work product: the note's paths, the read's shape
   The dispatcher finishes a task whose workflow wrote files with
   `artifacts: <p1>, <p2> (N tool call(s))`. The note is parsed into the paths
   the ledger offers to read — a read-only run's summary line and a note
   written by hand name none — and the read's own answer is parsed defensively,
   since the content is a file the AGENT wrote. */

eq(artifactPaths('artifacts: build-notes.txt (2 tool call(s))'), ['build-notes.txt'], 'a note names its one artifact');
eq(artifactPaths('artifacts: a.txt, b/index.txt (7 tool call(s))'), ['a.txt', 'b/index.txt'], 'a note names several, in its own order');
eq(artifactPaths('artifacts:  (0 tool call(s))'), [], 'a note naming no artifacts reads as none');
eq(artifactPaths('read-only: 3 match(es) (1 tool call(s))'), [], 'a read-only run’s summary names no artifacts');
eq(artifactPaths("the agent's note — 21 handbooks committed"), [], 'a hand-written note names none');
eq(artifactPaths('note: artifacts: x.txt (1 tool call(s))'), [], 'the line must begin the note, not merely contain the word');
eq(artifactPaths('artifacts: x.txt'), ['x.txt'], 'a note whose call count is absent still names its path');
eq(artifactPaths('  artifacts: x.txt (1 tool call(s))  '), ['x.txt'], 'a note’s surrounding space is trimmed');
eq(artifactPaths('artifacts: a.txt, (3 tool call(s))'), ['a.txt'], 'an empty segment between commas names nothing');
eq(artifactPaths(null), [], 'no note, no artifacts');
eq(artifactPaths(''), [], 'an empty note names none');

const ARTIFACT_FIXTURE = {
  task: 't3',
  agent_cid: 103,
  path: 'build-notes.txt',
  content: 'task t3: verify the fixed loop',
  size: 30,
  truncated: false,
};
eq(parseArtifact(ARTIFACT_FIXTURE), ARTIFACT_FIXTURE, 'a well-formed artifact parses unchanged');
eq(parseArtifact({ ...ARTIFACT_FIXTURE, extra: 1 }), ARTIFACT_FIXTURE, 'unknown extra fields are tolerated and dropped');
throws(() => parseArtifact(null), 'artifact is missing or not an object', 'a non-object artifact is rejected by name');
throws(() => parseArtifact({ ...ARTIFACT_FIXTURE, content: 5 }), 'artifact.content', 'mistyped content names the field');
throws(() => parseArtifact({ ...ARTIFACT_FIXTURE, truncated: 'yes' }), 'artifact.truncated', 'a non-boolean truncation names the field');
throws(() => parseArtifact({ ...ARTIFACT_FIXTURE, size: -1 }), 'artifact.size', 'a negative size names the field');
throws(() => parseArtifact({ ...ARTIFACT_FIXTURE, agent_cid: 1.5 }), 'artifact.agent_cid', 'a non-integer agent CID names the field');
{
  const missing: Record<string, unknown> = { ...ARTIFACT_FIXTURE };
  delete missing['path'];
  throws(() => parseArtifact(missing), 'artifact.path', 'a missing required field names the path');
}

/* the read's status, read as the console's own state: a node never told where
   the hub's admin surface is is not a hub out of reach, and neither is a
   refusal (the daemon's, the hub's or the agent's own words) */
eq(readStateOf(503), 'unconfigured', 'a daemon with no admin surface reads not configured');
eq(readStateOf(502), 'unreachable', 'a hub out of reach reads unreachable');
eq(readStateOf(null), 'unreachable', 'a node that never answered reads unreachable');
eq(readStateOf(403), 'refused', 'the hub’s policy refusal reads refused');
eq(readStateOf(422), 'refused', 'the agent’s own refusal reads refused');
eq(readStateOf(400), 'refused', 'a path outside the contract reads refused');
eq(readStateOf(404), 'refused', 'an unknown task reads refused');
eq(readStateOf(409), 'refused', 'an unassigned task reads refused');

/* the read is the owner's hand alone — and a console that cannot reach the
   node records that honestly, inventing no content and no success */
{
  const s = new Store();
  await s.readArtifact('t3', 'build-notes.txt');
  eq(s.artifact?.task, 't3', 'the ask records which task it was for');
  eq(s.artifact?.path, 'build-notes.txt', 'the ask records which path it was for');
  eq(s.artifact?.state, 'unreachable', 'a read that never reached the node reads as unreachable');
  eq((s.artifact?.error.length ?? 0) > 0, true, 'the failure carries a reason, not silence');
  eq(s.artifact?.content, '', 'no content is invented for a read that failed');
  eq(s.artifact?.status, null, 'and no status the node never sent');
}

/* ==================== the tool console: the model and the store's drives ===
   The operator drives a tool call on an agent through the daemon's /tell
   proxy. The answer is the agent's own (its `ok` and `output`) and is parsed
   defensively; each failure status is its own honest state; the arguments box
   must be a JSON object before it is ever sent; and the store keeps only the
   last few calls per agent, dropping an agent's history when it leaves the
   fleet — and inventing nothing for a call that never reached the node. */

const TELL_FIXTURE = { cid: 300, ok: true, output: '{"files_scanned":1}' };
eq(parseTell(TELL_FIXTURE), TELL_FIXTURE, 'a well-formed tell answer parses unchanged');
eq(parseTell({ ...TELL_FIXTURE, extra: 1 }), TELL_FIXTURE, 'unknown extra fields are tolerated and dropped');
throws(() => parseTell(null), 'tell is missing or not an object', 'a non-object tell is rejected by name');
throws(() => parseTell({ ...TELL_FIXTURE, ok: 'yes' }), 'tell.ok', 'a non-boolean ok names the field');
throws(() => parseTell({ ...TELL_FIXTURE, output: 5 }), 'tell.output', 'a mistyped output names the field');
throws(() => parseTell({ ...TELL_FIXTURE, cid: 1.5 }), 'tell.cid', 'a non-integer cid names the field');

/* the status, read as the console's own state: each refusal the hub can answer
   with is its own state, and neither a node that never answered nor a hub out
   of reach is dressed up as an answer */
eq(tellStateOf(403), 'policy', 'the hub’s role policy reads as its own state');
eq(tellStateOf(409), 'not-connected', 'an unconnected agent reads not connected');
eq(tellStateOf(504), 'timeout', 'a call that did not answer in time reads timeout');
eq(tellStateOf(503), 'off', 'a daemon with no admin surface reads hub admin off');
eq(tellStateOf(502), 'unreachable', 'a hub out of reach reads unreachable');
eq(tellStateOf(null), 'unreachable', 'a node that never answered reads unreachable');
eq(tellStateOf(400), 'refused', 'a body outside the contract reads refused');
eq(tellStateOf(404), 'refused', 'an unserved cid reads refused');

/* the arguments box: a JSON object, or nothing is sent */
eq(parseTellArgs(''), { args: {} }, 'an empty arguments box is the empty object');
eq(parseTellArgs('{"path":"notes.txt"}'), { args: { path: 'notes.txt' } }, 'an object parses to itself');
eq(parseTellArgs('  {"a":1}  '), { args: { a: 1 } }, 'surrounding space is trimmed');
eq('error' in parseTellArgs('[1,2]'), true, 'an array is not an object');
eq('error' in parseTellArgs('"x"'), true, 'a string is not an object');
eq('error' in parseTellArgs('5'), true, 'a number is not an object');
eq('error' in parseTellArgs('null'), true, 'null is not an object');
eq('error' in parseTellArgs('{oops}'), true, 'a box that is not JSON is refused before it is sent');

/* the drive is the owner's hand alone — and a console that cannot reach the
   node records that honestly, inventing no output and no success */
{
  const s = new Store();
  await s.driveTool(300, 'list', { path: '.' }, 2000);
  const read = s.tells.get(300)?.[0];
  eq(read?.cid, 300, 'the drive records which agent it was for');
  eq(read?.name, 'list', 'the drive records which tool it asked for');
  eq(read?.state, 'unreachable', 'a drive that never reached the node reads as unreachable');
  eq(read?.status, null, 'and no status the node never sent');
  eq(read?.output, '', 'no output is invented for a drive that failed');
  eq((read?.error.length ?? 0) > 0, true, 'the failure carries a reason, not silence');
  eq(s.lastTell?.state, 'unreachable', 'the last drive is the one just made');
}

/* a newer drive for the same agent supersedes the one still in flight */
{
  const s = new Store();
  const first = s.driveTool(300, 'first', {}, 1000);
  const second = s.driveTool(300, 'second', {}, 1000);
  await Promise.all([first, second]);
  eq(s.tells.get(300)?.length, 1, 'only the newest drive is recorded');
  eq(s.tells.get(300)?.[0]?.name, 'second', 'and it is the newer one');
}

/* the history is bounded, and an agent that leaves the fleet loses it */
{
  const s = new Store();
  for (let i = 0; i < 8; i++) await s.driveTool(300, `tool${i}`, {}, 1000);
  await s.driveTool(301, 'uptime', {}, 1000);
  eq(s.tells.get(300)?.length, 5, 'the per-agent history is bounded to the last few calls');
  eq(s.tells.get(300)?.[0]?.name, 'tool7', 'the newest call is first');

  /* the register now names only the agent at cid 301 */
  s.runs = [
    {
      id: 'r1',
      state: 'running',
      vcpus: 2,
      memory_mib: 512,
      beats: null,
      created_at: 1,
      ended_at: null,
      vms: [{ id: 0, state: 'running', serial: '', guest_cid: 301 }],
    },
  ];
  s.runsError = null;
  s.pruneTells();
  eq(s.tells.has(300), false, 'an agent the fleet no longer names loses its history');
  eq(s.tells.has(301), true, 'an agent the fleet still names keeps it');

  /* a source we cannot read is not read as an absence: the history stands */
  s.runsError = 'node unreachable';
  s.runs = [];
  s.pruneTells();
  eq(s.tells.has(301), true, 'an unreadable fleet does not erase the history it cannot confirm');
}

/* ==================== the stop: the hub's own view and the console's hand ===
   The operator ends one agent's session through the daemon's /stop proxy. The
   answer is the HUB's own stop view — either the session ended (with its call
   count) or it still stands (with the hub's reason) — parsed defensively; a
   missing `stopped`, and a verdict missing the one fact on its own side, are
   refused by name; each refusal status is its own honest state; and the reason
   is bounded to the hub's own 256 bytes at the field. */

const STOP_ENDED = { cid: 300, stopped: true, calls: 3 };
eq(parseStop(STOP_ENDED), { cid: 300, stopped: true, calls: 3, reason: null }, 'a session the hub ended parses with its call count');
const STOP_STANDS = { cid: 300, stopped: false, reason: 'the agent did not say bye in time' };
eq(
  parseStop(STOP_STANDS),
  { cid: 300, stopped: false, calls: null, reason: 'the agent did not say bye in time' },
  'a session that still stands parses with the hub’s reason',
);
eq(parseStop({ ...STOP_ENDED, extra: 1 }), { cid: 300, stopped: true, calls: 3, reason: null }, 'unknown extra fields are tolerated and dropped');
eq(parseStop({ cid: 0, stopped: true, calls: 0 }), { cid: 0, stopped: true, calls: 0, reason: null }, 'a session that made no calls is honest');
throws(() => parseStop(null), 'stop is missing or not an object', 'a non-object stop is rejected by name');
throws(() => parseStop({ ...STOP_ENDED, cid: 1.5 }), 'stop.cid', 'a non-integer cid names the field');
throws(() => parseStop({ ...STOP_ENDED, cid: -1 }), 'stop.cid', 'a negative cid names the field');
throws(() => parseStop({ cid: 300, calls: 3 }), 'stop.stopped', 'a missing stopped names the field');
throws(() => parseStop({ ...STOP_ENDED, stopped: 'yes' }), 'stop.stopped', 'a non-boolean stopped names the field');
throws(() => parseStop({ cid: 300, stopped: true }), 'stop.calls', 'an ended session without its call count is refused by name');
throws(() => parseStop({ cid: 300, stopped: true, calls: 1.5 }), 'stop.calls', 'a non-integer call count names the field');
throws(() => parseStop({ cid: 300, stopped: false }), 'stop.reason', 'a session that stands without the hub’s reason is refused by name');
throws(() => parseStop({ cid: 300, stopped: false, reason: 7 }), 'stop.reason', 'a mistyped reason names the field');
/* the fact the verdict does not name is dropped, never carried */
eq(parseStop({ cid: 300, stopped: true, calls: 2, reason: 'stray' }), { cid: 300, stopped: true, calls: 2, reason: null }, 'a reason on an ended session is dropped');
eq(parseStop({ cid: 300, stopped: false, reason: 'why', calls: 9 }), { cid: 300, stopped: false, calls: null, reason: 'why' }, 'a call count on a standing session is dropped');

/* the status, read as the console's own state: each refusal is its own, and a
   node that never answered is not dressed up as the hub's word */
eq(stopStateOf(404), 'unserved', 'a CID the hub does not serve reads unserved');
eq(stopStateOf(409), 'not-connected', 'an unconnected agent reads not connected');
eq(stopStateOf(503), 'off', 'a daemon with no admin surface reads hub admin off');
eq(stopStateOf(502), 'unreachable', 'a hub out of reach reads unreachable');
eq(stopStateOf(null), 'unreachable', 'a node that never answered reads unreachable');
eq(stopStateOf(400), 'refused', 'a body outside the contract reads refused');
eq(stopStateOf(500), 'refused', 'any other status reads refused');

/* the reason box: the hub's own 256-byte bound, named at the field */
eq(parseStopReason(''), {}, 'an empty reason is the hub’s own default');
eq(parseStopReason('   '), {}, 'a blank reason is the hub’s own default');
eq(parseStopReason('  maintenance window  '), { reason: '  maintenance window  ' }, 'a reason is sent as written, never trimmed');
eq(parseStopReason('x'.repeat(256)), { reason: 'x'.repeat(256) }, '256 bytes is the bound');
eq('error' in parseStopReason('x'.repeat(257)), true, '257 bytes is refused before it is sent');
eq(parseStopReason('é'.repeat(128)), { reason: 'é'.repeat(128) }, 'the bound counts bytes: 128 two-byte characters pass');
eq('error' in parseStopReason('é'.repeat(129)), true, 'and 129 two-byte characters are refused');

/* the stop is the owner's hand alone — and a console that cannot reach the
   node records that honestly, inventing no verdict and no call count */
{
  const s = new Store();
  await s.stopSession(300);
  eq(s.lastStop?.cid, 300, 'the stop records which agent it was for');
  eq(s.lastStop?.state, 'unreachable', 'a stop that never reached the node reads as unreachable');
  eq(s.lastStop?.stopped, null, 'and no verdict the hub never gave');
  eq(s.lastStop?.calls, null, 'and no call count the hub never sent');
  eq(s.lastStop?.status, null, 'and no status the node never sent');
  eq((s.lastStop?.detail.length ?? 0) > 0, true, 'the failure carries a reason, not silence');
}

/* a newer stop for the same agent supersedes the one still in flight */
{
  const s = new Store();
  const first = s.stopSession(300, 'first');
  const second = s.stopSession(300, 'second');
  await Promise.all([first, second]);
  eq(s.lastStop?.reason, 'second', 'the newer stop is the one kept');
}

/* ==================== the treasury: the accounts and the double-entry book
   The node's treasury is read whole and parsed defensively — the relay is
   untrusted, so a missing or mistyped field is refused by name. The journal's
   postings are the account-book's own words: a mint leg reads by its kind (its
   owner IS the mint), an escrow leg by its kind and its task, and each amount
   is signed. A `treasury` frame carries no row, only the news that the book
   grew: an entry the console already holds is complete, and an unseen id asks
   the journal once rather than inventing a post. */

const MINT_ACCOUNT = { id: 1, kind: 'mint', owner_ref: 'mint', credit_limit: 0, balance: -1500 };
const ESCROW_ACCOUNT = { id: 2, kind: 'task_escrow', owner_ref: 't1', credit_limit: 0, balance: 1500 };
const TOTALS_FIXTURE = { accounts: 2, entries: 2, credits: 1500, debits: 1500, balance: 0 };
const TREASURY_FIXTURE = {
  accounts: [MINT_ACCOUNT, ESCROW_ACCOUNT],
  totals: TOTALS_FIXTURE,
  spent: 0,
  note: 'v0 meters no real spend; a done task refunds its whole escrow',
};
const POSTING_MINT = { account: 1, kind: 'mint', owner_ref: 'mint', amount: -750 };
const POSTING_ESCROW = { account: 2, kind: 'task_escrow', owner_ref: 't1', amount: 750 };
const ENTRY_FIXTURE = {
  id: 1,
  kind: 'mint',
  task_id: 't1',
  root_task: 't1',
  memo: 'task budget',
  posted_by: 'treasury-service',
  created_at: 1791680766728,
  amount: 750,
  postings: [POSTING_MINT, POSTING_ESCROW],
};
const JOURNAL_FIXTURE = { entries: [ENTRY_FIXTURE], total: 1, limit: 256, truncated: false };

eq(parseTreasury(TREASURY_FIXTURE), TREASURY_FIXTURE, 'a well-formed treasury parses unchanged');
eq(parseTreasury({ ...TREASURY_FIXTURE, git: 'sha' }), TREASURY_FIXTURE, 'unknown extra fields are tolerated and dropped');
throws(() => parseTreasury(null), 'treasury is missing or not an object', 'a non-object treasury is rejected by name');
throws(() => parseTreasury({ ...TREASURY_FIXTURE, accounts: {} }), 'treasury.accounts is missing or not an array', 'an accounts field that is not an array names the field');
throws(() => parseTreasury({ ...TREASURY_FIXTURE, accounts: [{ ...MINT_ACCOUNT, kind: 'vault' }] }), 'treasury.accounts[0].kind', 'an unknown account kind is refused by name');
throws(() => parseTreasury({ ...TREASURY_FIXTURE, accounts: [{ ...MINT_ACCOUNT, balance: 1.5 }] }), 'treasury.accounts[0].balance', 'a non-integer balance names the field');
throws(() => parseTreasury({ ...TREASURY_FIXTURE, totals: { ...TOTALS_FIXTURE, credits: '1500' } }), 'treasury.totals.credits', 'a mistyped total names its path');
throws(() => parseTreasury({ ...TREASURY_FIXTURE, spent: 1.5 }), 'treasury.spent', 'a non-integer spend names the field');
{
  const missing: Record<string, unknown> = { ...TREASURY_FIXTURE };
  delete missing['note'];
  throws(() => parseTreasury(missing), 'treasury.note', 'a missing required field names the path');
}

eq(parseJournal(JOURNAL_FIXTURE), JOURNAL_FIXTURE, 'a well-formed journal parses unchanged');
eq(parseJournal({ ...JOURNAL_FIXTURE, extra: 1 }), JOURNAL_FIXTURE, 'unknown extra fields are tolerated and dropped');
throws(() => parseJournal(null), 'journal is missing or not an object', 'a non-object journal is rejected by name');
throws(() => parseJournal({ ...JOURNAL_FIXTURE, entries: [{ ...ENTRY_FIXTURE, kind: 'notary' }] }), 'journal.entries[0].kind', 'an unknown entry kind is refused by name');
throws(() => parseJournal({ ...JOURNAL_FIXTURE, entries: [{ ...ENTRY_FIXTURE, postings: 5 }] }), 'journal.entries[0].postings', 'a postings field that is not an array names the field');
throws(() => parseJournal({ ...JOURNAL_FIXTURE, entries: [{ ...ENTRY_FIXTURE, postings: [{ ...POSTING_MINT, amount: 'x' }] }] }), 'journal.entries[0].postings[0].amount', 'a bad posting names its index and field');
throws(() => parseJournal({ ...JOURNAL_FIXTURE, truncated: 'yes' }), 'journal.truncated', 'a non-boolean truncation names the field');
{
  const missing: Record<string, unknown> = { ...ENTRY_FIXTURE };
  delete missing['memo'];
  throws(() => parseJournal({ ...JOURNAL_FIXTURE, entries: [missing] }), 'journal.entries[0].memo', 'a missing required entry field names the path');
}
eq(
  parseJournal({ ...JOURNAL_FIXTURE, entries: [{ ...ENTRY_FIXTURE, task_id: null, root_task: null }] }).entries[0]?.task_id,
  null,
  'an entry that belongs to no task reads its null task',
);

eq(postingLabel(POSTING_MINT), 'mint', 'the mint leg names the account by its kind alone');
eq(postingLabel(POSTING_ESCROW), 'task_escrow t1', 'an escrow leg names its kind and its task');
eq(fmtSigned(-750), '\u2212750', 'a debit is written with a typed minus');
eq(fmtSigned(750), '+750', 'a credit is written with a plus');
eq(fmtSigned(-1500), '\u22121,500', 'a thousands-separated debit keeps its sign');
eq(balances(TOTALS_FIXTURE), true, 'credits equal debits and the balances sum to zero');
eq(balances({ ...TOTALS_FIXTURE, balance: 5 }), false, 'a non-zero balance is not balanced');
eq(balances({ ...TOTALS_FIXTURE, credits: 1400 }), false, 'credits that do not equal debits are not balanced');

/* a frame for an entry the journal already holds asks nothing — the book is
   append-only and the row it has is complete */
{
  const s = new Store();
  s.journal = [parseJournal(JOURNAL_FIXTURE).entries[0]!];
  s.applyTreasuryEvent(1);
  eq(s.wire.treasury, 1, 'the wire tallies the treasury frame');
  const { promise: settled, resolve: settledDone } = Promise.withResolvers<void>();
  setTimeout(settledDone, 250);
  await settled;
  eq(s.treasuryError, null, 'a frame for a known entry asks the accounts for nothing');
  eq(s.journalError, null, 'and asks the journal for nothing');
}

/* a frame for an entry the console has never seen asks the book once, and
   invents no post from the frame alone */
{
  const s = new Store();
  s.applyTreasuryEvent(9);
  eq(s.wire.treasury, 1, 'the wire tallies the treasury frame');
  eq(s.journal.length, 0, 'no journal row is invented from a frame');
  const { promise: settled, resolve: settledDone } = Promise.withResolvers<void>();
  setTimeout(settledDone, 250);
  await settled;
  eq(typeof s.journalError, 'string', 'the journal was asked for the truth');
  eq(typeof s.treasuryError, 'string', 'and the accounts were asked too');
}

/* ==================== the market: the agora's calls, bids and awards
   The board is the node's own derivation of the ledger, read whole and parsed
   defensively — the relay is untrusted, so a missing or mistyped field is
   refused by name. A call's state is `awarded` once an award closed it, the
   task's own state otherwise; an award names the bid it was made at. An
   `agora` frame carries no row — not the bids list, not the award's contractor
   or escrow entry — so the store asks the board once and invents no bid. */

const CALL_BID: Bid = { task: 't1', agent_cid: 103, bidder: 'builder', price: 640, created_at: 1791683465342 };
const CALL_AWARD: Award = {
  task: 't1',
  contractor: 'builder',
  agent_cid: 103,
  price: 640,
  escrow_entry: 1,
  created_at: 1791683465434,
};
const CALL_FIXTURE: Call = {
  task: 't1',
  capability: 'builder',
  spec_ref: 'an award binds the task',
  max_price: 900,
  state: 'awarded',
  created_at: 1791683465210,
  bids: [CALL_BID],
  award: CALL_AWARD,
};
const AGORA_FIXTURE: Calls = { calls: [CALL_FIXTURE], total: 1, limit: 256, truncated: false };

eq(parseAgora(AGORA_FIXTURE), AGORA_FIXTURE, 'a well-formed board parses unchanged');
eq(parseAgora({ ...AGORA_FIXTURE, git: 'sha' }), AGORA_FIXTURE, 'unknown extra fields are tolerated and dropped');
throws(() => parseAgora(null), 'agora is missing or not an object', 'a non-object board is rejected by name');
throws(() => parseAgora({ ...AGORA_FIXTURE, calls: {} }), 'agora.calls is missing or not an array', 'a calls field that is not an array names the field');
throws(() => parseAgora({ ...AGORA_FIXTURE, calls: [{ ...CALL_FIXTURE, state: 'pending' }] }), 'agora.calls[0].state', 'an unknown call state is refused by name');
throws(() => parseAgora({ ...AGORA_FIXTURE, calls: [{ ...CALL_FIXTURE, max_price: 900.5 }] }), 'agora.calls[0].max_price', 'a non-integer ceiling names the field');
throws(() => parseAgora({ ...AGORA_FIXTURE, calls: [{ ...CALL_FIXTURE, bids: 5 }] }), 'agora.calls[0].bids', 'a bids field that is not an array names the field');
throws(() => parseAgora({ ...AGORA_FIXTURE, calls: [{ ...CALL_FIXTURE, bids: [{ ...CALL_BID, price: 'x' }] }] }), 'agora.calls[0].bids[0].price', 'a bad bid names its index and field');
throws(() => parseAgora({ ...AGORA_FIXTURE, calls: [{ ...CALL_FIXTURE, award: 5 }] }), 'agora.calls[0].award is missing or not an object', 'an award that is not an object is refused by name');
throws(() => parseAgora({ ...AGORA_FIXTURE, calls: [{ ...CALL_FIXTURE, award: { ...CALL_AWARD, escrow_entry: 1.5 } }] }), 'agora.calls[0].award.escrow_entry', 'a non-integer escrow entry names the field');
throws(() => parseAgora({ ...AGORA_FIXTURE, truncated: 'yes' }), 'agora.truncated', 'a non-boolean truncation names the field');
{
  const missing: Record<string, unknown> = { ...CALL_FIXTURE };
  delete missing['capability'];
  throws(() => parseAgora({ ...AGORA_FIXTURE, calls: [missing] }), 'agora.calls[0].capability', 'a missing required call field names the path');
}
{
  const missing: Record<string, unknown> = { ...CALL_FIXTURE };
  delete missing['award'];
  throws(() => parseAgora({ ...AGORA_FIXTURE, calls: [missing] }), 'agora.calls[0].award', 'a call with no award key at all names the field');
}
eq(
  parseAgora({ ...AGORA_FIXTURE, calls: [{ ...CALL_FIXTURE, state: 'open', award: null, bids: [] }] }).calls[0]?.award,
  null,
  'an open call reads its null award',
);
eq(
  parseAgora({ ...AGORA_FIXTURE, calls: [{ ...CALL_FIXTURE, spec_ref: null }] }).calls[0]?.spec_ref,
  null,
  'a call with no spec reads its null spec',
);
eq(
  parseAgora({ ...AGORA_FIXTURE, calls: [{ ...CALL_FIXTURE, award: { ...CALL_AWARD, escrow_entry: null } }] }).calls[0]?.award?.escrow_entry,
  null,
  'an award on a task with no budget names no escrow entry',
);

eq(winningBid(CALL_FIXTURE), CALL_BID, 'the winning bid is the one the award names');
eq(winningBid({ ...CALL_FIXTURE, state: 'open', award: null }), null, 'an open call has no winning bid');
eq(
  winningBid({ ...CALL_FIXTURE, award: { ...CALL_AWARD, agent_cid: 999 } }),
  null,
  'an award naming a CID with no bid marks no winner',
);
eq(
  winningBid({ ...CALL_FIXTURE, award: { ...CALL_AWARD, price: 555 } }),
  null,
  'an award at a price no bid offered marks no winner',
);

/* an agora frame carries no row: the board is asked once, and nothing is
   invented from the frame alone */
{
  const s = new Store();
  s.applyAgoraEvent();
  eq(s.wire.agora, 1, 'the wire tallies the agora frame');
  eq(s.calls.length, 0, 'no call is invented from a frame');
  const { promise: settled, resolve: settledDone } = Promise.withResolvers<void>();
  setTimeout(settledDone, 250);
  await settled;
  eq(typeof s.callsError, 'string', 'the board was asked for the truth');
  eq(s.callsTotal, 0, 'and the frame set no total of its own');
}

/* a board frame carries one post (or one cursor) — never the totals, the
   bounding line or the other agents' unread counts: the board is asked, and
   no post and no read is invented from the frame alone */
{
  const s = new Store();
  s.applyBoardEvent();
  eq(s.wire.board, 1, 'the wire tallies the board frame');
  eq(s.board.length, 0, 'no post is invented from a frame');
  eq(s.boardUnread.length, 0, 'and no read cursor either');
  const { promise: settled, resolve: settledDone } = Promise.withResolvers<void>();
  setTimeout(settledDone, 250);
  await settled;
  eq(typeof s.boardError, 'string', 'the board was asked for the truth');
  eq(s.boardErrorStatus, null, 'a board that never answered reports no HTTP status');
  eq(s.boardTotal, 0, 'and the frame set no total of its own');
}

/* a conversation frame says one exchange happened; whether the pair is now
   active is the node's own answer, so the store asks for it rather than
   deciding it here — and a thread the card is showing is refreshed with it */
{
  const s = new Store();
  s.applyConversationEvent(100, 101);
  eq(s.wire.conversation, 1, 'the wire tallies the conversation frame');
  eq(s.conversations.length, 0, 'no exchange is invented from a frame');
  eq(s.convPairs.length, 0, 'and no pair: activity is the node’s own word');
  const { promise: settled, resolve: settledDone } = Promise.withResolvers<void>();
  setTimeout(settledDone, 250);
  await settled;
  eq(typeof s.conversationsError, 'string', 'the conversations were asked for the truth');
  eq(s.convTotal, 0, 'and the frame set no total of its own');
  /* a thread the card holds is refreshed by a frame for THAT pair, in either
     direction — and left alone by a frame for another pair */
  const t = new Store();
  t.thread = { a: 100, b: 101, state: 'read', exchanges: [], total: 0, truncated: false, error: '' };
  t.applyConversationEvent(200, 201);
  const { promise: other, resolve: otherDone } = Promise.withResolvers<void>();
  setTimeout(otherDone, 250);
  await other;
  eq(t.thread?.state, 'read', 'a frame for another pair leaves the card’s thread alone');

  const u = new Store();
  u.thread = { a: 100, b: 101, state: 'read', exchanges: [], total: 0, truncated: false, error: '' };
  u.applyConversationEvent(101, 100);
  const { promise: mine, resolve: mineDone } = Promise.withResolvers<void>();
  setTimeout(mineDone, 250);
  await mine;
  eq(u.thread?.state, 'error', 'a frame for the shown pair re-reads its thread (either direction)');
}

/* ==================== the council: the seats' motions and the owner's gate
   The motions endpoint is the only source for a motion's text and its votes'
   reasons, so the whole motion is read from it and parsed defensively — the
   relay is untrusted, and a missing or mistyped field is refused by name. A
   motion's state and its gate are the node's own derivation; the console reads
   them and asserts only the tally's arithmetic (voted == votes carried, of
   four seats) without recomputing a decision. A `motion` frame carries no text
   and no row, so the store asks the node and patches nothing in. */

const VOTE_YES: Vote = { seat: 'finance', agent_cid: 105, vote: 'yes', reason: 'the record supports it', at: 1791705629367 };
const VOTE_NO: Vote = { seat: 'incident', agent_cid: 106, vote: 'no', reason: 'the record is thin', at: 1791705629412 };
const MOTION_FIXTURE: Motion = {
  id: 'm1',
  proposer_cid: 105,
  proposer_role: 'finance',
  kind: 'priority',
  text: 'prioritise the east wing over new towns this week',
  amount: null,
  needs_owner: false,
  state: 'tie_to_owner',
  votes: [
    { ...VOTE_YES, seat: 'finance' },
    { ...VOTE_YES, seat: 'legal-agents-counsel', agent_cid: 107, at: 1791705629458 },
    { ...VOTE_NO, seat: 'incident' },
    { ...VOTE_NO, seat: 'mayor-office', agent_cid: 114, at: 1791705629506 },
  ],
  tally: { yes: 2, no: 2, abstain: 0, voted: 4, seats: 4 },
  owner: null,
  created_at: 1791705623593,
  updated_at: 1791705629506,
};
const COUNCIL_FIXTURE: Motions = { motions: [MOTION_FIXTURE], total: 1, limit: 256, truncated: false };
const GATED_FIXTURE: Motion = {
  ...MOTION_FIXTURE,
  id: 'm2',
  proposer_cid: 107,
  proposer_role: 'legal-agents-counsel',
  kind: 'law_proposal',
  text: 'amend S-001 so builders may read the review record',
  needs_owner: true,
  state: 'awaiting_owner',
  votes: MOTION_FIXTURE.votes.map((v) => ({ ...v, vote: 'yes' as const })),
  tally: { yes: 4, no: 0, abstain: 0, voted: 4, seats: 4 },
};

eq(parseMotions(COUNCIL_FIXTURE), COUNCIL_FIXTURE, 'a well-formed council parses unchanged');
eq(parseMotions({ ...COUNCIL_FIXTURE, git: 'sha' }), COUNCIL_FIXTURE, 'unknown extra fields are tolerated and dropped');
eq(parseMotion(MOTION_FIXTURE), MOTION_FIXTURE, 'one motion parses as the list reports it');
throws(() => parseMotions(null), 'council is missing or not an object', 'a non-object council is rejected by name');
throws(() => parseMotions({ ...COUNCIL_FIXTURE, motions: {} }), 'council.motions is missing or not an array', 'a motions field that is not an array names the field');
throws(() => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, kind: 'edict' }] }), 'council.motions[0].kind', 'an unknown kind is refused by name');
throws(() => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, state: 'pending' }] }), 'council.motions[0].state', 'an unknown state is refused by name');
throws(() => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, votes: [{ ...VOTE_YES, vote: 'aye' }] }] }), 'council.motions[0].votes[0].vote', 'an unknown vote word is refused by name');
throws(() => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, proposer_cid: 105.5 }] }), 'council.motions[0].proposer_cid', 'a non-integer proposer CID names the field');
throws(() => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, amount: 'lots' }] }), 'council.motions[0].amount', 'an amount that is not an integer names the field');
throws(() => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, needs_owner: 'yes' }] }), 'council.motions[0].needs_owner', 'a non-boolean gate names the field');
throws(() => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, votes: 5 }] }), 'council.motions[0].votes', 'a votes field that is not an array names the field');
throws(() => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, tally: null }] }), 'council.motions[0].tally', 'a tally that is not an object is refused by name');
throws(() => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, tally: { ...MOTION_FIXTURE.tally, yes: 1.5 } }] }), 'council.motions[0].tally.yes', 'a non-integer tally figure names its field');
throws(() => parseMotions({ ...COUNCIL_FIXTURE, truncated: 'yes' }), 'council.truncated', 'a non-boolean truncation names the field');
throws(() => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, votes: [{ ...VOTE_YES, seat: 5 }] }] }), 'council.motions[0].votes[0].seat', 'a vote seat that is not a string names the field');
throws(() => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, votes: [{ ...VOTE_YES, at: 1.2 }] }] }), 'council.motions[0].votes[0].at', 'a vote stamp that is not an integer names the field');
{
  const missing: Record<string, unknown> = { ...MOTION_FIXTURE };
  delete missing['text'];
  throws(() => parseMotions({ ...COUNCIL_FIXTURE, motions: [missing] }), 'council.motions[0].text', 'a missing required motion field names the path');
}
{
  const missing: Record<string, unknown> = { ...MOTION_FIXTURE };
  delete missing['votes'];
  throws(() => parseMotions({ ...COUNCIL_FIXTURE, motions: [missing] }), 'council.motions[0].votes', 'a motion with no votes key at all names the field');
}
{
  const missing: Record<string, unknown> = { ...MOTION_FIXTURE };
  delete missing['amount'];
  eq(parseMotions({ ...COUNCIL_FIXTURE, motions: [missing] }).motions[0]?.amount, null, 'an absent amount column reads as no amount, not a missing field');
}
eq(
  parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, amount: 500 }] }).motions[0]?.amount,
  500,
  'a budget move’s amount is read',
);
eq(
  parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, amount: null }] }).motions[0]?.amount,
  null,
  'a null amount reads as none',
);

/* the folio, and the tally's arithmetic the console asserts */
eq(motionSeq('m12'), 12, 'a motion id parses to its folio');
eq(motionSeq('m'), 0, 'an id that does not parse is folio 0');
eq(motionSeq(null), 0, 'a null id is folio 0');
eq(SEAT_COUNT, 4, 'the doctrine seats four');

eq(tallyHolds(MOTION_FIXTURE), true, 'a tally squares with its votes');
eq(
  tallyHolds({ ...MOTION_FIXTURE, tally: { ...MOTION_FIXTURE.tally, voted: 3 } }),
  false,
  'a tally whose voted disagrees with the votes carried is caught',
);
eq(
  tallyHolds({ ...MOTION_FIXTURE, tally: { ...MOTION_FIXTURE.tally, seats: 5 } }),
  false,
  'a tally that seats more than four is caught',
);
eq(
  tallyHolds({ ...MOTION_FIXTURE, tally: { ...MOTION_FIXTURE.tally, yes: 1 } }),
  false,
  'a tally whose words do not sum to voted is caught',
);

/* the sheet's figures: total, still open, awaiting the owner, the ties */
eq(
  councilTally([MOTION_FIXTURE, GATED_FIXTURE, { ...MOTION_FIXTURE, id: 'm3', state: 'open' }, { ...MOTION_FIXTURE, id: 'm4', state: 'carried' }]),
  { total: 4, open: 1, awaiting: 1, ties: 1, owner: 2 },
  'the tally counts the four the owner reads by, and nothing else as owner-waiting',
);
eq(councilTally([]), { total: 0, open: 0, awaiting: 0, ties: 0, owner: 0 }, 'an empty council counts zeroes');

/* the gate, in plain words, from the kind the node derived it with */
eq(
  gateLine(GATED_FIXTURE),
  'above the council’s authority: a law change waits for the owner’s ratification',
  'a law proposal names its gate',
);
eq(gateLine(MOTION_FIXTURE), null, 'a motion the council decides outright names no gate');
eq(
  gateLine({ ...GATED_FIXTURE, kind: 'town_proposal' }),
  'above the council’s authority: founding a town waits for the owner’s ratification',
  'a town proposal names its gate',
);
eq(
  gateLine({ ...GATED_FIXTURE, kind: 'mayor_replacement' }),
  'above the council’s authority: replacing a mayor waits for the owner’s ratification',
  'a mayor replacement names its gate',
);
eq(
  gateLine({ ...GATED_FIXTURE, kind: 'budget_move', amount: 5000 }),
  'above the council’s authority: a move over a tenth of the minted pool waits for the owner’s ratification',
  'a gated budget move names its gate',
);
eq(
  gateLine({ ...GATED_FIXTURE, kind: 'priority' }),
  'above the council’s authority: this decision waits for the owner’s ratification',
  'a gated kind v0 never derives reads as a plain gate, naming no invented rule',
);

/* the label tables cover every kind and every state */
eq(Object.keys(KIND_LABEL).length, 6, 'every kind has a label');
eq(Object.keys(STATE_LABEL).length, 7, 'every state has a label');

/* ==================== the owner's decision: the always-keyed column, the two
   new states, the reading the sheet draws, and the note's own byte cap. The
   `owner` column is present on every motion (null until decided) and the note
   column likewise (null when none), so a missing key is refused by name and a
   decided motion is never read as undecided. */
const OWNER_FIXTURE: OwnerDecision = { approve: false, note: 'the record is thin', at: 1791705630000 };
const VETOED_FIXTURE: Motion = { ...GATED_FIXTURE, id: 'm3', state: 'vetoed', owner: OWNER_FIXTURE };
const RATIFIED_FIXTURE: Motion = {
  ...GATED_FIXTURE,
  id: 'm4',
  state: 'ratified',
  owner: { approve: true, note: null, at: 1791705631000 },
};

eq(parseMotion(MOTION_FIXTURE), MOTION_FIXTURE, 'a null owner column reads as the owner has not decided');
eq(parseMotion(VETOED_FIXTURE), VETOED_FIXTURE, 'a vetoed motion parses with its owner column');
eq(parseMotion(RATIFIED_FIXTURE), RATIFIED_FIXTURE, 'a ratified motion parses with its owner column');
eq(
  parseMotions({ ...COUNCIL_FIXTURE, motions: [RATIFIED_FIXTURE, VETOED_FIXTURE] }).motions.map((m) => m.state),
  ['ratified', 'vetoed'],
  'the two states the owner’s hand produces are read from the wire',
);
{
  const missing: Record<string, unknown> = { ...MOTION_FIXTURE };
  delete missing['owner'];
  throws(
    () => parseMotions({ ...COUNCIL_FIXTURE, motions: [missing] }),
    'council.motions[0].owner is missing',
    'a missing owner column is refused by name — it is always keyed',
  );
}
{
  const missingNote: Record<string, unknown> = { approve: true, at: 5 };
  throws(
    () => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, owner: missingNote }] }),
    'council.motions[0].owner.note is missing',
    'a missing note key is refused by name — it is always keyed too',
  );
}
throws(
  () => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, owner: 'decided' }] }),
  'council.motions[0].owner',
  'an owner column that is not an object is refused by name',
);
throws(
  () => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, owner: { ...OWNER_FIXTURE, approve: 'yes' } }] }),
  'council.motions[0].owner.approve',
  'a non-boolean approve names its field',
);
throws(
  () => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, owner: { ...OWNER_FIXTURE, at: 1.5 } }] }),
  'council.motions[0].owner.at',
  'a non-integer decision stamp names its field',
);
throws(
  () => parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, owner: { approve: true, note: 5, at: 5 } }] }),
  'council.motions[0].owner.note',
  'a note that is neither a string nor null names the field',
);
eq(
  parseMotions({ ...COUNCIL_FIXTURE, motions: [{ ...MOTION_FIXTURE, owner: { approve: true, note: null, at: 5 } }] }).motions[0]?.owner,
  { approve: true, note: null, at: 5 },
  'a decided motion with no note reads its null note as none, not a missing field',
);

/* the owner's reading, the note's byte cap, and how a decision ended */
eq(ownerReading(MOTION_FIXTURE), null, 'no decision reads as nothing');
eq(
  ownerReading(VETOED_FIXTURE),
  { approve: false, word: 'no', verb: 'vetoed', at: 1791705630000, note: 'the record is thin' },
  'a veto reads as the owner’s no, with their note',
);
eq(
  ownerReading(RATIFIED_FIXTURE),
  { approve: true, word: 'yes', verb: 'ratified', at: 1791705631000, note: null },
  'a ratification reads as the owner’s yes, with no note',
);
eq(parseDecisionNote('   '), {}, 'a blank note is the owner’s own none');
eq(parseDecisionNote('the record decides it'), { note: 'the record decides it' }, 'a note rides as the owner wrote it');
eq(parseDecisionNote('x'.repeat(1000)), { note: 'x'.repeat(1000) }, 'a note at the cap is accepted');
eq(parseDecisionNote('x'.repeat(1001)), { error: 'the note is longer than 1000 bytes' }, 'a note past the cap is named at the field');
eq(
  parseDecisionNote('é'.repeat(501)),
  { error: 'the note is longer than 1000 bytes' },
  'the cap is the daemon’s bytes, not characters (é is two)',
);
eq(MAX_DECISION_NOTE, 1000, 'the note cap is the daemon’s own');
eq(decisionStateOf(200), 'taken', 'a 200 is the decision taken');
eq(decisionStateOf(409), 'refused', 'a 409 is a refusal');
eq(decisionStateOf(400), 'refused', 'a 400 is a refusal too');
eq(decisionStateOf(404), 'unknown', 'a 404 is no such motion');
eq(decisionStateOf(null), 'unreachable', 'no answer is unreachable');

/* the gate is spent once the owner decides, and a decided motion counts in the
   total alone — the strip's four figures never count a decided motion as
   waiting on the owner */
eq(gateLine(VETOED_FIXTURE), null, 'a decided motion carries no gate line');
eq(
  gateLine({ ...VETOED_FIXTURE, state: 'awaiting_owner', owner: null }),
  'above the council’s authority: a law change waits for the owner’s ratification',
  'the same motion undecided still names its gate',
);
eq(
  councilTally([VETOED_FIXTURE, RATIFIED_FIXTURE, MOTION_FIXTURE]),
  { total: 3, open: 0, awaiting: 0, ties: 1, owner: 1 },
  'a decided motion is in the total alone',
);

/* ==================== the decide: the node's own answer is merged, and a
   refusal rides verbatim. The store posts only from the owner's hand; the
   transport is stubbed here so the two paths (the motion with its `owner` set,
   and the doctrine's 409) are asserted without a node. */
{
  const real = globalThis.fetch;
  try {
    const s = new Store();
    s.motions = [{ ...GATED_FIXTURE }];
    const ratified: Motion = { ...GATED_FIXTURE, state: 'ratified', owner: { approve: true, note: 'carry it', at: 1791705632000 } };
    globalThis.fetch = async () =>
      new Response(JSON.stringify(ratified), { status: 200, headers: { 'content-type': 'application/json' } });
    await s.decideMotion('m2', true, 'carry it');
    eq(s.lastDecision?.state, 'taken', 'a 200 decision reads as taken');
    eq(s.lastDecision?.approve, true, 'and names the hand the owner used');
    eq(s.motions[0]?.state, 'ratified', 'the node’s own answer replaces the row the console holds');
    eq(s.motions[0]?.owner?.note, 'carry it', 'with the owner’s note, as the node recorded it');

    globalThis.fetch = async () =>
      new Response(JSON.stringify({ error: 'motion m2 is carried: the owner decides only a tie_to_owner or awaiting_owner motion — the council’s own decision stands' }), {
        status: 409,
        headers: { 'content-type': 'application/json' },
      });
    await s.decideMotion('m2', false);
    eq(s.lastDecision?.state, 'refused', 'a 409 reads as a refusal');
    eq(s.lastDecision?.status, 409, 'and keeps the status');
    eq(
      s.lastDecision?.detail,
      'motion m2 is carried: the owner decides only a tie_to_owner or awaiting_owner motion — the council’s own decision stands',
      'the refusal is the daemon’s own words, verbatim',
    );
    eq(s.motions[0]?.state, 'ratified', 'a refusal changes no row');
  } finally {
    globalThis.fetch = real;
  }
}

/* a motion frame carries no text: the council is asked once, and nothing is
   invented from the frame alone */
{
  const s = new Store();
  s.applyMotionEvent();
  eq(s.wire.motion, 1, 'the wire tallies the motion frame');
  eq(s.motions.length, 0, 'no motion is invented from a frame');
  const { promise: settled, resolve: settledDone } = Promise.withResolvers<void>();
  setTimeout(settledDone, 250);
  await settled;
  eq(typeof s.motionsError, 'string', 'the council was asked for the truth');
  eq(s.motionsTotal, 0, 'and the frame set no total of its own');
  eq(s.motionFilter, null, 'no filter stands until the owner presses for one');
  s.setMotionFilter('awaiting_owner');
  eq(s.motionFilter, 'awaiting_owner', 'pressing a figure narrows the sheet');
  s.clearMotionFilter();
  eq(s.motionFilter, null, 'showing every motion clears the filter');
}

/* picking a pair narrows the talk sheet and asks the node for that pair's
   thread at once; widening keeps the thread read — it is the node's answer
   for its pair, not the sheet's filter */
{
  const s = new Store();
  s.setTalkPair(101, 100);
  eq(s.talkPair, { a: 100, b: 101 }, 'the picked pair is held ascending');
  eq(s.thread?.state, 'loading', 'picking a pair asks for its thread at once');
  const { promise: settled, resolve: settledDone } = Promise.withResolvers<void>();
  setTimeout(settledDone, 250);
  await settled;
  eq(s.thread?.state, 'error', 'the thread read was asked for (no node here)');
  s.clearTalkPair();
  eq(s.talkPair, null, 'widening clears the filter');
  eq(s.thread?.state, 'error', 'widening keeps the thread last read');
}

if (failures > 0) {
  console.error(`\n${failures} assertion(s) failed`);
  process.exitCode = 1;
} else {
  console.log('\nall assertions passed');
}