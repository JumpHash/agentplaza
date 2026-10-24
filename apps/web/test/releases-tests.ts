// The Releases view's pure rules: the channel groups of the list, the words and tones of a state, the candidate line,
// a digest and a size in short form, a verdict and a promotion as lines, the required inputs of an action, and which
// links from the hub are web addresses. The DOM is not touched here. Bundled with the other suites by test/tests.ts;
// it prints "releases tests: ok".

import type { Release } from '../src/api-releases';
import {
  candidateText,
  groupByChannel,
  isWebUrl,
  promotionLine,
  requiredProblem,
  shortDigest,
  sizeText,
  stateText,
  stateTone,
  verdictLine,
} from '../src/views/releases-model';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

/** A release as the hub lists it; only the fields the list and the page read matter here. */
function release(over: Partial<Release>): Release {
  return {
    id: 'rel1', project: 'pr1', repo: 'agentplaza', version: '1.0.0', channel: 'stable', base: null, lane: 'main', from: 'abc123',
    state: 'draft', number: null, notes: '', run: null, sbom: '', provenance: '', changes: [], artifacts: [], verdicts: [],
    promotions: [], sync: [], created_ms: 1, updated_ms: 1, ...over,
  };
}

// ---- Channels -----------------------------------------------------------------------------------------------------------

const listed = [
  release({ id: 'a', version: '2.0.0', channel: 'beta' }),
  release({ id: 'b', version: '1.2.0', channel: 'stable' }),
  release({ id: 'c', version: '1.1.0', channel: 'beta' }),
  release({ id: 'd', version: '1.0.0', channel: 'stable' }),
];
const groups = groupByChannel(listed);
eq(groups.map((g) => g.channel), ['beta', 'stable'], 'channels: the newest channel leads');
eq(groups.map((g) => g.releases.map((r) => r.id)), [['a', 'c'], ['b', 'd']], 'channels: each keeps the list order, newest first');
eq(groupByChannel([]), [], 'channels: no releases, no groups');
eq(listed.map((r) => r.id), ['a', 'b', 'c', 'd'], 'channels: the list it is given is not reordered');

// ---- States -------------------------------------------------------------------------------------------------------------

eq(
  ['draft', 'candidate', 'qualifying', 'qualified', 'published', 'promoted', 'failed', 'abandoned', 'superseded', 'yanked'].map(stateText),
  ['Draft', 'Candidate', 'Qualifying', 'Qualified', 'Published', 'Promoted', 'Failed', 'Abandoned', 'Superseded', 'Yanked'],
  'states: each state in words',
);
eq(stateText('reviewing'), 'reviewing', 'states: an unknown state reads as it is');
eq(
  ['published', 'promoted', 'qualified', 'candidate', 'qualifying', 'failed', 'yanked', 'draft', 'abandoned'].map(stateTone),
  ['ok', 'ok', 'info', 'info', 'info', 'danger', 'danger', 'muted', 'muted'],
  'states: a shipped release is ok, one in progress is info, a failure or yank is danger',
);

// ---- Candidate, digest, size ----------------------------------------------------------------------------------------------

eq(candidateText(null), 'No candidate yet', 'candidate: none yet');
eq(candidateText(3), 'Candidate 3', 'candidate: its number');
eq(shortDigest('sha256:0123456789abcdef0123'), '0123456789ab…', 'digest: the prefix goes and the first twelve characters stay');
eq(shortDigest('abc'), 'abc', 'digest: a short digest is shown whole');
eq(shortDigest(''), '—', 'digest: none is a dash');
eq(sizeText(0), '0 B', 'size: zero bytes');
eq(sizeText(1023), '1023 B', 'size: under a KiB is bytes');
eq(sizeText(1024), '1 KiB', 'size: a whole KiB');
eq(sizeText(1536), '1.5 KiB', 'size: one decimal place when not whole');
eq(sizeText(5 * 1024 * 1024), '5 MiB', 'size: MiB');
eq(sizeText(3 * 1024 * 1024 * 1024), '3 GiB', 'size: GiB');
eq(sizeText(2 * 1024 ** 4), '2048 GiB', 'size: beyond GiB stays in GiB');
eq([sizeText(-1), sizeText(Number.NaN)], ['—', '—'], 'size: a negative or missing size is a dash');

// ---- Verdicts and promotions --------------------------------------------------------------------------------------------

eq(verdictLine({ source: 'review', verdict: 'pass', reason: '', seq: 1 }), 'review: pass', 'verdict: no reason, no dash');
eq(verdictLine({ source: 'check', verdict: 'fail', reason: 'tests red', seq: 2 }), 'check: fail — tests red', 'verdict: the reason follows a dash');
eq(promotionLine({ environment: 'staging', seq: 1, health: 'good' }), 'Promoted to staging (good)', 'promotion: the environment and its health');
eq(promotionLine({ environment: 'production', seq: 2, health: '' }), 'Promoted to production', 'promotion: no health, no brackets');

// ---- Required inputs ----------------------------------------------------------------------------------------------------

eq(requiredProblem('  ', 'a reason'), 'Give a reason first.', 'a blank reason is named');
eq(requiredProblem(' staging ', 'an environment'), null, 'a filled-in environment passes');
eq(requiredProblem('', 'an environment'), 'Give an environment first.', 'an empty environment is named');

// ---- Links from the hub -------------------------------------------------------------------------------------------------

eq(isWebUrl('https://ci.example.test/run/1'), true, 'an https link is a web address');
eq(isWebUrl('HTTP://ci.example.test'), true, 'the scheme is matched in any case');
eq(isWebUrl('javascript:alert(1)'), false, 'a script link is not made a link');
eq(isWebUrl('https://'), false, 'a bare scheme is not a web address');
eq(isWebUrl('ftp://x.test/a'), false, 'an ftp link is not made a link');

console.log(`releases tests: ok (${checks} checks)`);
