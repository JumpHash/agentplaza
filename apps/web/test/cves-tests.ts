// The CVE report's pure rules: the order of the findings and the filter by priority, the words and tones of a priority,
// a fix's state and a reachability, the release and environment join, the feeds' warnings, and the rows a fixture
// response renders as. The DOM is not touched here. Bundled with the other suites by test/tests.ts; it prints
// "cves tests: ok".

import type { CveFeed, CveFinding, CveReport } from '../src/api-cves';
import { cvesApi } from '../src/api-cves';
import {
  advisoryIds,
  countsLine,
  emptyTitle,
  feedLine,
  feedWarnings,
  feedWarningText,
  fixedText,
  fixStateLabel,
  fixStateTone,
  mainText,
  polledText,
  priorityBreakdown,
  priorityLabel,
  priorityTone,
  reachLabel,
  reachTone,
  releaseLine,
  sortFindings,
  visibleRows,
} from '../src/views/cves-model';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

/** A finding as the hub lists it; only the fields the report reads matter here. */
function finding(over: Partial<CveFinding>): CveFinding {
  return {
    id: 'cargo:x:RUSTSEC-0000', ids: ['RUSTSEC-0000'], ecosystem: 'cargo', package: 'x', title: 'a flaw', severity: 'high', priority: 'high',
    cvss: '', published: '', source: 'osv', url: '', functions: [], main_version: '1.0.0', main_environments: ['production'], fixed: '',
    reachability: 'unknown', reach_note: '', state: 'open', fix_state: '', fix_reason: '', fix_task: '', releases: [], first_ms: 1, updated_ms: 1,
    ...over,
  };
}

// The hub's answer for one project, as it arrives: two findings (the first reachable and high on main, with its fix task;
// the second unreachable and medium, on a release only) and one feed whose poll failed.
const fixture: CveReport = {
  project: 'pr1',
  polled_ms: 1_700_000_000_000,
  feeds: [
    { feed: 'osv', at_ms: 1_700_000_000_000, count: 2, error: '' },
    { feed: 'github', at_ms: 1_700_000_300_000, count: 0, error: 'rate limited until 18:00' },
  ],
  counts: { total: 2, open: 2, fixed: 0, by_priority: { high: 1, medium: 1 } },
  findings: [
    {
      id: 'cargo:h2:RUSTSEC-2024-0003', ids: ['RUSTSEC-2024-0003', 'CVE-2024-27316'], ecosystem: 'cargo', package: 'h2', title: 'Unbounded CONTINUATION frames',
      severity: 'high', priority: 'high', cvss: 'CVSS:3.1/AV:N/AC:L', published: '2024-03-11', source: 'osv, rustsec',
      url: 'https://rustsec.org/advisories/RUSTSEC-2024-0003.html', functions: ['h2::codec::read_frame'], main_version: '0.3.9', main_environments: ['production'],
      fixed: '0.3.26', reachability: 'reachable', reach_note: 'read_frame runs on every request', state: 'open', fix_state: 'task', fix_reason: '',
      fix_task: 'task-42', releases: [{ release: 'rel-3', version: '1.0.0', environments: ['production'] }], first_ms: 1, updated_ms: 2,
    },
    {
      id: 'npm:left:GHSA-aaaa-bbbb-cccc', ids: ['GHSA-aaaa-bbbb-cccc'], ecosystem: 'npm', package: 'left', title: 'Prototype pollution in a helper',
      severity: 'medium', priority: 'medium', cvss: '', published: '', source: 'osv', url: 'javascript:alert(1)', functions: [], main_version: '',
      main_environments: [], fixed: '', reachability: 'unreachable', reach_note: 'the helper is never called', state: 'open', fix_state: 'no_fix',
      fix_reason: 'the source names no release after 0.9.0 that fixes it', fix_task: '', releases: [{ release: 'rel-2', version: '0.9.0', environments: [] }],
      first_ms: 1, updated_ms: 2,
    },
  ],
};

// ---- Order and filter ----------------------------------------------------------------------------------------------

eq(sortFindings([finding({ id: 'b', priority: 'low' }), finding({ id: 'a', priority: 'critical' }), finding({ id: 'c', priority: 'mystery' as CveFinding['priority'] }), finding({ id: 'd', priority: 'medium' })]).map((f) => f.id), ['a', 'd', 'b', 'c'], 'most severe first; an unknown priority goes last');
eq(sortFindings([finding({ id: 'u', priority: 'high', reachability: 'unreachable' }), finding({ id: 'k', priority: 'high', reachability: 'unknown' }), finding({ id: 'r', priority: 'high', reachability: 'reachable' })]).map((f) => f.id), ['r', 'k', 'u'], 'at one priority, reachable first, then unknown, then unreachable');
eq(sortFindings([finding({ id: 'z', package: 'zlib' }), finding({ id: 'y', package: 'axum' })]).map((f) => f.id), ['y', 'z'], 'at one priority and reach, by package');
const given = [finding({ id: 'b', priority: 'low' }), finding({ id: 'a', priority: 'critical' })];
eq(sortFindings(given).map((f) => f.id), ['a', 'b'], 'the sorted list puts the critical first');
eq(given.map((f) => f.id), ['b', 'a'], 'the list it is given is not reordered');

eq(visibleRows(fixture, '').map((r) => r.id), ['cargo:h2:RUSTSEC-2024-0003', 'npm:left:GHSA-aaaa-bbbb-cccc'], 'all rows: the high first, then the medium');
eq(visibleRows(fixture, 'medium').map((r) => r.pkg), ['left'], 'the filter keeps one priority');
eq(visibleRows(fixture, 'critical'), [], 'a priority with no finding has no rows');

// ---- Priorities ----------------------------------------------------------------------------------------------------

eq(['critical', 'high', 'medium', 'low', 'unknown'].map(priorityLabel), ['Critical', 'High', 'Medium', 'Low', 'Unknown'], 'the priorities in words');
eq(['critical', 'high', 'medium', 'low', 'unknown'].map(priorityTone), ['danger', 'danger', 'warn', 'info', 'muted'], 'the tone of each priority');
eq(priorityLabel('info'), 'info', 'a priority a newer hub names reads as it is');
eq(priorityBreakdown({ high: 1, medium: 0, critical: 1 }), [
  { priority: 'critical', label: 'Critical', tone: 'danger', count: 1 },
  { priority: 'high', label: 'High', tone: 'danger', count: 1 },
], 'the breakdown is most severe first, and a priority with no finding is left out');
eq(priorityBreakdown({ weird: 3, unknown: 2, critical: 1 }).map((b) => b.priority), ['critical', 'unknown', 'weird'], 'an unknown key goes after the known ones');

// ---- Fix states and reachability ------------------------------------------------------------------------------------

eq(
  ['', 'started', 'task', 'current', 'tests_failed', 'apply_failed', 'review_failed', 'task_failed', 'refused', 'not_built', 'no_fix', 'plan_failed', 'fixed'].map(fixStateLabel),
  ['No fix asked for', 'Fix started', 'Task made', 'Nothing to update', 'Tests failed', 'Could not apply', 'Review failed', 'Task failed', 'Refused', 'Not built for this ecosystem', 'No fix named by the source', 'Plan failed', 'Fixed'],
  'the label of each fix state',
);
eq(fixStateLabel('mystery'), 'mystery', 'a fix state a newer hub names reads as it is');
eq(
  ['', 'started', 'task', 'current', 'tests_failed', 'refused', 'no_fix', 'not_built', 'fixed'].map(fixStateTone),
  ['muted', 'info', 'info', 'ok', 'danger', 'warn', 'warn', 'muted', 'ok'],
  'the tone of each fix state',
);
eq(fixStateTone('task_failed'), 'danger', 'a failed task is danger');
eq(reachLabel('reachable'), 'Reachable', 'reachable');
eq(reachLabel('unreachable'), 'Not reachable', 'unreachable');
eq(reachLabel('unknown'), 'Unknown', 'unknown');
eq([reachTone('reachable'), reachTone('unreachable'), reachTone('unknown')], ['danger', 'muted', 'warn'], 'reachability tones');

// ---- Releases, main, fixed and advisory ids -----------------------------------------------------------------------

eq(releaseLine({ release: 'rel-3', version: '1.0.0', environments: ['production', 'staging'] }), 'rel-3@1.0.0 — production, staging', 'a release joined with its environments');
eq(releaseLine({ release: 'rel-2', version: '0.9.0', environments: [] }), 'rel-2@0.9.0 — no environment', 'a release in no environment says so');
eq(mainText({ main_version: '0.3.9' }), '0.3.9 on main', 'the version on main');
eq(mainText({ main_version: '' }), 'not on main', 'not on main');
eq(fixedText('0.3.26'), 'Fixed in 0.3.26', 'the fixed release');
eq(fixedText(''), 'No fixed release named', 'no fixed release named');
eq(advisoryIds({ id: 'x', ids: ['RUSTSEC-2024-0003', 'CVE-2024-27316', 'GHSA-1'] }), { primary: 'RUSTSEC-2024-0003', aliases: ['CVE-2024-27316', 'GHSA-1'] }, 'the first id is the main one, the others are aliases');
eq(advisoryIds({ id: 'cargo:x:none', ids: [] }), { primary: 'cargo:x:none', aliases: [] }, 'an advisory with no ids shows its finding id');

// ---- Counts, polls and feeds --------------------------------------------------------------------------------------

const POLLED = 1_700_000_000_000;
eq(countsLine(fixture.counts), '2 advisories · 2 open · 0 fixed', 'the totals in one line');
eq(countsLine({ total: 1, open: 0, fixed: 1, by_priority: {} }), '1 advisory · 0 open · 1 fixed', 'one advisory is singular');
eq(polledText(POLLED, POLLED + 3 * 3600 * 1000), 'Last polled 3 h ago', 'the last poll, in relative time');
eq(polledText(0), 'Never polled', 'a report never polled');
eq(feedLine(fixture.feeds[0] as CveFeed, POLLED + 60_000), '2 advisories · polled 1 min ago', 'a feed that polled: its count and its time');
eq(feedLine({ feed: 'rustsec', at_ms: 0, count: 1, error: '' }), '1 advisory · not polled yet', 'a feed never polled says so');
eq(feedWarnings(fixture.feeds), [{ feed: 'github', message: 'rate limited until 18:00' }], 'only the feed that failed is a warning');
eq(feedWarningText(feedWarnings(fixture.feeds)[0] ?? { feed: '', message: '' }), 'github feed failed: rate limited until 18:00', 'the warning line names the feed and its error');
eq(feedWarnings([{ feed: 'osv', at_ms: 1, count: 0, error: '   ' }]), [], 'a blank error is no warning');
eq(emptyTitle(true), 'No advisory reaches this project', 'a polled report with no finding');
eq(emptyTitle(false), 'Not polled yet: poll now', 'a report not yet polled');

// ---- The rows a fixture response renders as ----------------------------------------------------------------------

const [high, medium] = visibleRows(fixture, '');
eq(
  high && {
    priority: high.priorityLabel, tone: high.priorityTone, pkg: high.pkg, main: high.main, onMain: high.onMain, primary: high.primary,
    aliases: high.aliases, url: high.url, reach: high.reach, reachTone: high.reachTone, reachNote: high.reachNote, fixed: high.fixed,
    fix: high.fix, fixTone: high.fixTone, fixTask: high.fixTask, fixReason: high.fixReason, releases: high.releases,
  },
  {
    priority: 'High', tone: 'danger', pkg: 'h2', main: '0.3.9 on main', onMain: true, primary: 'RUSTSEC-2024-0003', aliases: ['CVE-2024-27316'],
    url: 'https://rustsec.org/advisories/RUSTSEC-2024-0003.html', reach: 'Reachable', reachTone: 'danger', reachNote: 'read_frame runs on every request',
    fixed: 'Fixed in 0.3.26', fix: 'Task made', fixTone: 'info', fixTask: 'task-42', fixReason: '', releases: ['rel-3@1.0.0 — production'],
  },
  'the reachable high row: on main with its fix task and its release',
);
eq(
  medium && {
    priority: medium.priorityLabel, tone: medium.priorityTone, pkg: medium.pkg, main: medium.main, onMain: medium.onMain, primary: medium.primary,
    aliases: medium.aliases, url: medium.url, reach: medium.reach, reachTone: medium.reachTone, reachNote: medium.reachNote, fixed: medium.fixed,
    fix: medium.fix, fixTone: medium.fixTone, fixTask: medium.fixTask, fixReason: medium.fixReason, releases: medium.releases,
  },
  {
    priority: 'Medium', tone: 'warn', pkg: 'left', main: 'not on main', onMain: false, primary: 'GHSA-aaaa-bbbb-cccc', aliases: [],
    url: '', reach: 'Not reachable', reachTone: 'muted', reachNote: 'the helper is never called', fixed: 'No fixed release named',
    fix: 'No fix named by the source', fixTone: 'warn', fixTask: '', fixReason: 'the source names no release after 0.9.0 that fixes it',
    releases: ['rel-2@0.9.0 — no environment'],
  },
  'the unreachable medium row: not on main, a release only, and an address that is not a web link (so no link is made)',
);

// ---- The export links -----------------------------------------------------------------------------------------------

eq(cvesApi.exportUrl('pr 1', 'csv'), '/v1/projects/pr%201/cves/export?format=csv', 'the CSV download, with the project id encoded');
eq(cvesApi.exportUrl('pr1', 'json'), '/v1/projects/pr1/cves/export?format=json', 'the JSON download');

console.log(`cves tests: ok (${checks} checks)`);
