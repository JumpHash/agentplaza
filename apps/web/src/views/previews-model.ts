// The Previews tab's pure rules: how a link's days become its seconds, the words and tones of a preview's status, a
// link's scope and its state. The DOM is not touched here (the view is project-previews.ts). Bundled with the other
// suites by test/tests.ts.

import type { PreviewLinkState, PreviewScope, PreviewStatus } from '../api-projects';
import type { Tone } from '../ui/components';

/** A share lasts from one day to ninety; the form's default is a week. */
export const DAYS_MIN = 1;
export const DAYS_MAX = 90;
export const DAYS_DEFAULT = 7;
/** The hub keeps a link's label to 80 characters. */
export const LABEL_MAX = 80;

/** A link's length in seconds from its whole days (1 to 90), or undefined for any other number. */
export function linkSeconds(days: number): number | undefined {
  return Number.isInteger(days) && days >= DAYS_MIN && days <= DAYS_MAX ? days * 86400 : undefined;
}

export const STATUS: Record<PreviewStatus, { label: string; tone: Tone }> = {
  awake: { label: 'Awake', tone: 'ok' },
  asleep: { label: 'Asleep', tone: 'muted' },
  closed: { label: 'Closed', tone: 'muted' },
};

export const SCOPE_LABEL: Record<PreviewScope, string> = { view: 'Watch', use: 'Use' };
export const SCOPE_CHOICES: { value: PreviewScope; label: string }[] = [
  { value: 'view', label: 'Watch: view the app only (GET and HEAD)' },
  { value: 'use', label: 'Use: use the app as the owner does' },
];

export const LINK_STATE: Record<PreviewLinkState, { label: string; tone: Tone }> = {
  active: { label: 'Active', tone: 'ok' },
  revoked: { label: 'Revoked', tone: 'danger' },
  expired: { label: 'Expired', tone: 'muted' },
};
