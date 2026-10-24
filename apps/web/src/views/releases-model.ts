// The Releases view's pure rules: the channels a list is grouped by, what a state says and how it is toned, the short
// forms of a digest and a size, the lines of a verdict and a promotion, and the checks an action's input must pass.
// The DOM is not touched here; the view (releases.ts) draws these, and test/releases-tests.ts checks them.

import type { Tone } from '../ui/components';
import type { Release, ReleasePromotion, ReleaseState, ReleaseVerdict } from '../api-releases';

export const STATE_LABELS: Record<ReleaseState, string> = {
  draft: 'Draft',
  candidate: 'Candidate',
  qualifying: 'Qualifying',
  qualified: 'Qualified',
  published: 'Published',
  promoted: 'Promoted',
  failed: 'Failed',
  abandoned: 'Abandoned',
  superseded: 'Superseded',
  yanked: 'Yanked',
};

/** A state in words. An unknown state (a newer hub) reads as it is, never as a blank. */
export function stateText(state: string): string {
  return STATE_LABELS[state as ReleaseState] ?? state;
}

/** The tone of a state: a release that shipped is ok, one in progress is info, a failure or a yank is danger, and the
 * ones that were set aside are muted. */
export function stateTone(state: string): Tone {
  switch (state) {
    case 'published':
    case 'promoted':
      return 'ok';
    case 'qualified':
    case 'candidate':
    case 'qualifying':
      return 'info';
    case 'failed':
    case 'yanked':
      return 'danger';
    default:
      return 'muted';
  }
}

/** The releases of a list, grouped by channel. Each group keeps the order it was given in (newest first), and the
 * groups come in the order their channel first appears, so the newest channel leads. */
export function groupByChannel(releases: Release[]): { channel: string; releases: Release[] }[] {
  const groups = new Map<string, Release[]>();
  for (const r of releases) {
    const list = groups.get(r.channel);
    if (list) list.push(r);
    else groups.set(r.channel, [r]);
  }
  return Array.from(groups, ([channel, list]) => ({ channel, releases: list }));
}

/** The candidate a release is on, or a line that it has none. */
export function candidateText(number: number | null): string {
  return number === null ? 'No candidate yet' : `Candidate ${number}`;
}

/** The first twelve characters of a digest, without its `sha256:` prefix; a dash when there is none. */
export function shortDigest(digest: string): string {
  const bare = digest.replace(/^sha256:/, '');
  if (!bare) return '—';
  return bare.length > 12 ? `${bare.slice(0, 12)}…` : bare;
}

/** A size in bytes, in the largest unit it fills (B, KiB, MiB, GiB), one decimal place when it is not a whole number. */
export function sizeText(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—';
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KiB', 'MiB', 'GiB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  const shown = Number.isInteger(value) ? String(value) : value.toFixed(1);
  return `${shown} ${units[unit]}`;
}

/** A verdict as one line: its source and verdict, and its reason after a dash when it has one. */
export function verdictLine(v: ReleaseVerdict): string {
  const head = `${v.source}: ${v.verdict}`;
  return v.reason ? `${head} — ${v.reason}` : head;
}

/** A promotion as one line: the environment it reached and the health it left it in. */
export function promotionLine(p: ReleasePromotion): string {
  return p.health ? `Promoted to ${p.environment} (${p.health})` : `Promoted to ${p.environment}`;
}

/** A required text input (a reason, an environment): a problem to show, or null when it is filled in. */
export function requiredProblem(value: string, what: string): string | null {
  return value.trim() ? null : `Give ${what} first.`;
}

/** Whether a link from the hub is a web address (http or https): only those are made links. */
export function isWebUrl(url: string): boolean {
  return /^https?:\/\/\S+$/i.test(url);
}
