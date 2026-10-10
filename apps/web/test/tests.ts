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
import { Store } from '../src/store';

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

if (failures > 0) {
  console.error(`\n${failures} assertion(s) failed`);
  process.exitCode = 1;
} else {
  console.log('\nall assertions passed');
}
