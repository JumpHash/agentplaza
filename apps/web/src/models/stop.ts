/* Ending one connected agent's session, through the daemon's own proxy
   (`POST /api/v0/agents/{cid}/stop`, crates/noded `stop_agent`) — the
   operator's hand on a session, not on a guest.

   The answer is the HUB's own stop view, passed through verbatim: `{"cid",
   "stopped", …}`. It is read through a defensive parse — the relay between the
   console and the node could be hostile, so the shape is never trusted.
   Whether the session ended is the hub's to say; the console never reads a
   verdict of its own into a status, and a hub that could not end a session is
   a fact to report (`stopped: false`), not an error to dress up.

   The hub's view carries exactly one of two facts beside its verdict, and the
   verdict decides which: an ENDED session carries `calls` (how many calls it
   made before it said bye), and a session that still STANDS carries `reason`
   (why the hub could not end it). Both are required on their own side of the
   verdict — a parse that finds `stopped: true` without a `calls`, or
   `stopped: false` without a `reason`, refuses naming the field — so a hostile
   relay cannot strip the one fact that says what happened. */

import { utf8ByteLength } from '../serial';

/** The hub's own stop view, as the daemon passes it through verbatim. */
export interface AgentStop {
  /** The guest CID of the agent whose session was addressed. */
  cid: number;
  /** The hub's own verdict: `true` the session ended; `false` it still stands,
      and `reason` says why. */
  stopped: boolean;
  /** The ended session's call count — present (and required) when `stopped` is
      true; `null` otherwise (the hub writes it only for an ended session). */
  calls: number | null;
  /** Why a session the hub could not end still stands — present (and required)
      when `stopped` is false; `null` otherwise. */
  reason: string | null;
}

/** A missing or mistyped required field; the message names its path. */
export class StopParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'StopParseError';
  }
}

function obj(v: unknown, path: string): Record<string, unknown> {
  if (v == null || typeof v !== 'object' || Array.isArray(v)) {
    throw new StopParseError(`${path} is missing or not an object`);
  }
  return v as Record<string, unknown>;
}

/** Parse one `/stop` answer defensively; unknown extras are tolerated. */
export function parseStop(raw: unknown): AgentStop {
  const o = obj(raw, 'stop');
  const cid = o['cid'];
  if (typeof cid !== 'number' || !Number.isInteger(cid) || cid < 0) {
    throw new StopParseError('stop.cid is missing or not a whole, non-negative number');
  }
  const stopped = o['stopped'];
  if (typeof stopped !== 'boolean') {
    throw new StopParseError('stop.stopped is missing or not a boolean');
  }
  if (stopped) {
    const calls = o['calls'];
    if (typeof calls !== 'number' || !Number.isInteger(calls) || calls < 0) {
      throw new StopParseError(
        'stop.calls is missing or not a whole, non-negative number: an ended session carries its call count',
      );
    }
    return { cid, stopped: true, calls, reason: null };
  }
  const reason = o['reason'];
  if (typeof reason !== 'string') {
    throw new StopParseError(
      'stop.reason is missing or not a string: a session the hub could not end carries why it stands',
    );
  }
  return { cid, stopped: false, calls: null, reason };
}

/* ------------------------------------------------------------- how it ended
   The distinct, honest states a stop can end in. `stopStateOf` is the one place
   that reads the endpoint's status table (crates/noded `stop_agent`): a 200 is
   the hub's own verdict — the session ended, or it still stands — and each
   refusal the daemon or the hub can answer with is its own state. `null` (the
   node was never reached) and a 502 (a hub out of reach) are their own, never
   dressed up as the hub's word. */
export type StopState =
  | 'stopping'
  | 'ended'
  | 'standing'
  | 'unserved'
  | 'not-connected'
  | 'off'
  | 'unreachable'
  | 'refused';

/** The state a failed stop wears, from the status the daemon answered with —
    or from no answer at all (`null`: the node was never reached). */
export function stopStateOf(status: number | null): StopState {
  if (status === 404) return 'unserved';
  if (status === 409) return 'not-connected';
  if (status === 503) return 'off';
  if (status == null || status === 502) return 'unreachable';
  return 'refused';
}

/* --------------------------------------------------------------- the reason
   The most a stop's reason may occupy, in bytes — the hub's own bound
   (`vsockd`'s `MAX_STOP_REASON`, which `noded` keeps as `MAX_STOP_REASON`), so
   the console refuses a reason the daemon would refuse, at the field, rather
   than round-tripping its 400. */
export const MAX_STOP_REASON = 256;

/** Read the optional reason box: an empty (or all-whitespace) box is the hub's
    own default (`{ reason }` absent), and a reason past the hub's byte bound is
    named at the field. The reason is otherwise sent as the owner wrote it —
    never trimmed, since it is carried to the guest's transcript and the
    operator's log. */
export function parseStopReason(text: string): { reason?: string } | { error: string } {
  if (text.trim() === '') return {};
  if (utf8ByteLength(text) > MAX_STOP_REASON) {
    return { error: `the reason is longer than ${MAX_STOP_REASON} bytes` };
  }
  return { reason: text };
}
