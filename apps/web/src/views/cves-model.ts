// The CVE report's pure rules: the priority order and its words and tones, the sort of the findings and the filter by
// priority, the count lines, the last poll of each feed and the failures to show, the words of a fix's state and of a
// reachability, and the row each finding is drawn as. The DOM is not touched here; project-cves.ts draws these, and
// test/cves-tests.ts checks them.

import type { CveCounts, CveFeed, CveFinding, CveRelease, CveReport, CvePriority } from '../api-cves';
import { ago, type Tone } from '../ui/components';
import { isWebUrl } from './releases-model';

/** The priorities, most severe first. */
export const PRIORITY_ORDER: CvePriority[] = ['critical', 'high', 'medium', 'low', 'unknown'];

const PRIORITY_LABELS: Record<CvePriority, string> = { critical: 'Critical', high: 'High', medium: 'Medium', low: 'Low', unknown: 'Unknown' };

function priorityRank(priority: string): number {
  const i = PRIORITY_ORDER.indexOf(priority as CvePriority);
  return i < 0 ? PRIORITY_ORDER.length : i;
}

/** A priority in words. A priority a newer hub names reads as it is. */
export function priorityLabel(priority: string): string {
  return PRIORITY_LABELS[priority as CvePriority] ?? (priority || 'Unknown');
}

/** The badge tone of a priority: critical and high are danger, medium is warn, low is info, the rest are muted. */
export function priorityTone(priority: string): Tone {
  switch (priority) {
    case 'critical':
    case 'high':
      return 'danger';
    case 'medium':
      return 'warn';
    case 'low':
      return 'info';
    default:
      return 'muted';
  }
}

/** The filter's choices: all priorities (the empty value), then each priority. */
export const PRIORITY_CHOICES: { value: string; label: string }[] = [
  { value: '', label: 'All priorities' },
  ...PRIORITY_ORDER.map((p) => ({ value: p, label: priorityLabel(p) })),
];

const REACH_RANK: Record<string, number> = { reachable: 0, unknown: 1, unreachable: 2 };

/** Most severe priority first; then the reachable ones before the unknown and the unreachable; then by package and id. A new list. */
export function sortFindings(list: CveFinding[]): CveFinding[] {
  return [...list].sort(
    (a, b) =>
      priorityRank(a.priority) - priorityRank(b.priority) ||
      (REACH_RANK[a.reachability] ?? 1) - (REACH_RANK[b.reachability] ?? 1) ||
      a.package.localeCompare(b.package) ||
      a.id.localeCompare(b.id),
  );
}

/** The counts by priority, most severe first. A priority with no finding is left out; an unknown one goes last. */
export function priorityBreakdown(by: Record<string, number>): { priority: string; label: string; tone: Tone; count: number }[] {
  return Object.keys(by)
    .filter((p) => (by[p] ?? 0) > 0)
    .sort((a, b) => priorityRank(a) - priorityRank(b) || a.localeCompare(b))
    .map((p) => ({ priority: p, label: priorityLabel(p), tone: priorityTone(p), count: by[p] ?? 0 }));
}

function count(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** The totals in one line: the advisories, how many are open, how many are fixed. */
export function countsLine(c: CveCounts): string {
  return `${count(c.total, 'advisory', 'advisories')} · ${c.open} open · ${c.fixed} fixed`;
}

/** When the report was last polled, or that it never was (`polled_ms` is 0 then). */
export function polledText(polledMs: number, now = Date.now()): string {
  return polledMs > 0 ? `Last polled ${ago(polledMs, now)}` : 'Never polled';
}

/** The line of one feed: how many advisories it named and when it was polled. */
export function feedLine(f: CveFeed, now = Date.now()): string {
  const found = count(f.count, 'advisory', 'advisories');
  return f.at_ms > 0 ? `${found} · polled ${ago(f.at_ms, now)}` : `${found} · not polled yet`;
}

/** The feeds whose last poll failed, with the message they gave. A feed that polled fine is not in the list. */
export function feedWarnings(feeds: CveFeed[]): { feed: string; message: string }[] {
  return feeds
    .filter((f) => (f.error || '').trim() !== '')
    .map((f) => ({ feed: f.feed, message: (f.error || '').trim() }));
}

/** The warning line of a failed feed. */
export function feedWarningText(w: { feed: string; message: string }): string {
  return `${w.feed} feed failed: ${w.message}`;
}

/** The empty state's title: the report says nothing reaches the project when it was polled, else that it has not been. */
export function emptyTitle(polled: boolean): string {
  return polled ? 'No advisory reaches this project' : 'Not polled yet: poll now';
}

const FIX_LABELS: Record<string, string> = {
  '': 'No fix asked for',
  started: 'Fix started',
  task: 'Task made',
  current: 'Nothing to update',
  tests_failed: 'Tests failed',
  apply_failed: 'Could not apply',
  review_failed: 'Review failed',
  task_failed: 'Task failed',
  refused: 'Refused',
  not_built: 'Not built for this ecosystem',
  no_fix: 'No fix named by the source',
  plan_failed: 'Plan failed',
  fixed: 'Fixed',
};

/** A fix's state in words. A state a newer hub names reads as it is. */
export function fixStateLabel(state: string): string {
  return FIX_LABELS[state] ?? state;
}

/** The tone of a fix's state: fixed or nothing to update is ok, in progress is info, a refusal or no fix named is warn,
 * a failure is danger, and the rest (none asked, not built here, a newer state) are muted. */
export function fixStateTone(state: string): Tone {
  switch (state) {
    case 'fixed':
    case 'current':
      return 'ok';
    case 'started':
    case 'task':
      return 'info';
    case 'refused':
    case 'no_fix':
      return 'warn';
    case 'tests_failed':
    case 'apply_failed':
    case 'review_failed':
    case 'task_failed':
    case 'plan_failed':
      return 'danger';
    default:
      return 'muted';
  }
}

const REACH_LABELS: Record<string, string> = { reachable: 'Reachable', unreachable: 'Not reachable', unknown: 'Unknown' };

/** A reachability in words. */
export function reachLabel(reachability: string): string {
  return REACH_LABELS[reachability] ?? 'Unknown';
}

/** The tone of a reachability: reachable is danger, not reachable is muted (it is not overclaimed as safe), unknown is warn. */
export function reachTone(reachability: string): Tone {
  switch (reachability) {
    case 'reachable':
      return 'danger';
    case 'unreachable':
      return 'muted';
    default:
      return 'warn';
  }
}

/** The fixed release, or a line that the source names none. */
export function fixedText(fixed: string): string {
  return fixed ? `Fixed in ${fixed}` : 'No fixed release named';
}

/** The package's version on `main`, or that it is not on main. */
export function mainText(f: Pick<CveFinding, 'main_version'>): string {
  return f.main_version ? `${f.main_version} on main` : 'not on main';
}

/** The advisory's main id (the first of its ids; its finding id when it names none), and the others as its aliases. */
export function advisoryIds(f: Pick<CveFinding, 'id' | 'ids'>): { primary: string; aliases: string[] } {
  const ids = f.ids ?? [];
  return { primary: ids[0] ?? f.id, aliases: ids.slice(1) };
}

/** A release joined with the environments it reaches, as one line: `rel-3@1.0.0 — production, staging`. */
export function releaseLine(r: CveRelease): string {
  const envs = r.environments ?? [];
  return `${r.release}@${r.version} — ${envs.length ? envs.join(', ') : 'no environment'}`;
}

/** The lines of a finding's releases, in the order the hub gave them. */
export function releaseLines(releases: CveRelease[]): string[] {
  return (releases ?? []).map(releaseLine);
}

/** One finding as the table draws it: every cell is text, and the tones and the link are decided here. */
export interface CveRow {
  id: string;
  priority: string;
  priorityLabel: string;
  priorityTone: Tone;
  pkg: string;
  ecosystem: string;
  main: string;
  onMain: boolean;
  primary: string;
  aliases: string[];
  /** The advisory's address when it is a web address, else empty (no link is made). */
  url: string;
  title: string;
  reach: string;
  reachTone: Tone;
  reachNote: string;
  fixed: string;
  fix: string;
  fixTone: Tone;
  fixTask: string;
  fixReason: string;
  releases: string[];
}

export function rowOf(f: CveFinding): CveRow {
  const ids = advisoryIds(f);
  return {
    id: f.id,
    priority: f.priority,
    priorityLabel: priorityLabel(f.priority),
    priorityTone: priorityTone(f.priority),
    pkg: f.package,
    ecosystem: f.ecosystem,
    main: mainText(f),
    onMain: f.main_version !== '',
    primary: ids.primary,
    aliases: ids.aliases,
    url: isWebUrl(f.url || '') ? f.url : '',
    title: f.title,
    reach: reachLabel(f.reachability),
    reachTone: reachTone(f.reachability),
    reachNote: f.reach_note,
    fixed: fixedText(f.fixed),
    fix: fixStateLabel(f.fix_state),
    fixTone: fixStateTone(f.fix_state),
    fixTask: f.fix_task,
    fixReason: f.fix_reason,
    releases: releaseLines(f.releases),
  };
}

/** The rows the table draws: the findings in order, kept to one priority when `priority` names one ('' keeps all). */
export function visibleRows(report: Pick<CveReport, 'findings'>, priority: string): CveRow[] {
  return sortFindings(report.findings)
    .filter((f) => !priority || f.priority === priority)
    .map(rowOf);
}
