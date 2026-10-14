// The pure part of the rich content (ui/embeds.ts): the tone a state or status takes, and the author of a card with its
// avatar. No DOM and no I/O, so the test suite runs it under node. The tone is the chip's and the badge's colour role:
// the stylesheet maps each tone to a token, and the label stays in the text colour.

/** A colour role: `ok` (open, done, success), `info` (in progress), `warn` (waiting on the owner), `danger` (closed,
 * failed), `merged`, and `neutral` (draft, open task, offline, anything unknown). */
export type Tone = 'ok' | 'info' | 'warn' | 'danger' | 'merged' | 'neutral';

/** What a state belongs to: a pull request or issue (`change`), a task, an agent, or a run or a library tool (`run`). */
export type Subject = 'change' | 'task' | 'agent' | 'run';

const RUN_TONES: Record<string, Tone> = {
  success: 'ok',
  done: 'ok',
  approved: 'ok',
  published: 'ok',
  passed: 'ok',
  failure: 'danger',
  failed: 'danger',
  error: 'danger',
  cancelled: 'danger',
  rejected: 'danger',
  closed: 'danger',
  running: 'info',
  in_progress: 'info',
  queued: 'info',
  claimed: 'info',
};

const TONES: Record<Subject, Record<string, Tone>> = {
  // A pull request or issue: open is success, a draft is secondary, a merged pull request is purple, closed is an error.
  change: { open: 'ok', draft: 'neutral', merged: 'merged', closed: 'danger' },
  task: {
    open: 'neutral',
    claimed: 'info',
    awaiting_review: 'warn',
    done: 'ok',
    failed: 'danger',
    rejected: 'danger',
    cancelled: 'danger',
  },
  agent: { idle: 'ok', working: 'info', offline: 'neutral' },
  run: RUN_TONES,
};

/** The tone of a state of `subject`. An unknown state is neutral. */
export function toneOf(subject: Subject, state: string): Tone {
  const table = TONES[subject];
  return Object.prototype.hasOwnProperty.call(table, state) ? (table[state] ?? 'neutral') : 'neutral';
}

/** An author: a user object (login, avatar_url, name) or a plain name. Avatars are https only; a card with no avatar
 * shows the author's monogram instead. */
export interface Person {
  name: string;
  avatar: string | null;
}

export function personOf(value: unknown): Person | null {
  if (typeof value === 'string' && value !== '') return { name: value, avatar: null };
  if (!value || typeof value !== 'object') return null;
  const o = value as Record<string, unknown>;
  const login = typeof o.login === 'string' && o.login !== '' ? o.login : null;
  const name = login ?? (typeof o.name === 'string' && o.name !== '' ? o.name : null);
  if (name === null) return null;
  const avatar = typeof o.avatar_url === 'string' && o.avatar_url.startsWith('https://') ? o.avatar_url : null;
  return { name, avatar };
}

/** The letter an avatar shows when there is no picture: the first letter or digit of the name, upper case. */
export function monogramOf(name: string): string {
  const first = Array.from(name.replace(/^[^\p{L}\p{N}]+/u, ''))[0];
  return first ? first.toUpperCase() : '?';
}
