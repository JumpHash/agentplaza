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
import { fmtDateTime, fmtDur, fmtStamp } from '../src/format';
import type { Run, Vm, VmState } from '../src/models/run';
import { cidRange, roleOf } from '../src/models/run';
import { sessionOf, sessionSummary } from '../src/models/session';
import { Store } from '../src/store';
import { ARC_DWELL_MS, arcAlive, arcLift, focusFraming, overviewFraming, runPlateLines } from '../src/views/district';
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
eq(store.wire, { run: 1, vm: 1, serial: 6 }, 'the wire tallies every frame');

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

/* --------------------------------- the district's pure derivations ------ */
eq(
  runPlateLines({
    id: 'r3', state: 'running', vcpus: 2, memory_mib: 512, beats: null,
    created_at: 1, ended_at: null, vms: [vmShaped(0, 10), vmShaped(1, 11)],
  }),
  { title: 'r3 · running', sub: 'cid 10–11 · 2 VMs × 2 vCPU · 512 MiB' },
  'the nameplate carries id, state, CID range and shape',
);
eq(
  runPlateLines({
    id: 'r4', state: 'done', vcpus: 1, memory_mib: 128, beats: 12,
    created_at: 1, ended_at: 2, vms: [vmShaped(0)],
  }),
  { title: 'r4 · done', sub: 'no vsock · 1 VM × 1 vCPU · 128 MiB' },
  'a run without vsock says so on its plate',
);
{
  const one = overviewFraming(24, 128320, 1);
  eq(one.target, [0, 0, 0], 'the overview looks at the plot’s centre');
  eq(one.pos[0], one.pos[2], 'the overview keeps the boot framing’s diagonal');
  const many = overviewFraming(24, 128320, 20);
  eq(many.pos[0] > one.pos[0], true, 'many runs widen the overview past the machine span');
  eq(overviewFraming(2, 1024, 0).pos[0], 16 * 0.9, 'a small machine floors at the minimum span');
  const focus = focusFraming(5.2, 4.4, -4.5);
  eq(focus.target, [0, 0.8, -4.5], 'the focus frames the platform’s own centre');
  eq(focusFraming(20.8, 20, 0).pos[0] > focus.pos[0], true, 'a bigger platform frames from further');
  eq(arcLift(0), 2.5, 'the arc lift has a floor');
  eq(arcLift(10), 3.2, 'the arc lift follows the span');
  eq(arcLift(100), 14, 'the arc lift is bounded');
}

/* the session arc's bounds: the protocol window, one beat of dwell after a
   bye seen on an open run, and never for a killed VM or a historical run */
eq(arcAlive('active', 'running', null, 1000), true, 'a session in flight draws its arc');
eq(arcAlive('active', 'exited', null, 1000), false, 'a VM gone mid-session draws no arc');
eq(arcAlive('active', 'killed', null, 1000), false, 'a killed VM draws no arc');
eq(arcAlive('complete', 'running', 1000, 1000 + ARC_DWELL_MS - 1), true, 'the dwell keeps a fresh bye on the plot');
eq(arcAlive('complete', 'exited', 1000, 1000 + ARC_DWELL_MS - 1), true, 'the natural exit path keeps the dwell');
eq(arcAlive('complete', 'exited', 1000, 1000 + ARC_DWELL_MS), false, 'the dwell ends after one beat');
eq(arcAlive('complete', 'running', null, 1000), false, 'a session read from history draws no arc');
eq(arcAlive('complete', 'killed', 1000, 1001), false, 'the strike ends even a fresh dwell');

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

if (failures > 0) {
  console.error(`\n${failures} assertion(s) failed`);
  process.exitCode = 1;
} else {
  console.log('\nall assertions passed');
}
