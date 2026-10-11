/* A tool call the operator drives on a connected agent, through the daemon
   (`POST /api/v0/agents/{cid}/tell`, crates/noded `tell_agent`). The answer is
   the agent's own — `ok` plus its `output` — read through a defensive parse:
   the relay between the console and the node could be hostile, so the shape is
   never trusted. The output is the AGENT's: guest-derived text, and it is
   rendered as text, never as markup. */

/** The agent's own answer to a driven call. */
export interface AgentTell {
  /** The guest CID of the agent the call was driven on. */
  cid: number;
  /** The agent's own `ok`: `false` is the tool refusing, which is still an
      answer — its reason is `output`, not a transport failure. */
  ok: boolean;
  /** The agent's own output: the tool's result, or its refusal's reason. */
  output: string;
}

/** A missing or mistyped required field; the message names its path. */
export class TellParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'TellParseError';
  }
}

function obj(v: unknown, path: string): Record<string, unknown> {
  if (v == null || typeof v !== 'object' || Array.isArray(v)) {
    throw new TellParseError(`${path} is missing or not an object`);
  }
  return v as Record<string, unknown>;
}

/** Parse one `/tell` answer defensively; unknown extras are tolerated. */
export function parseTell(raw: unknown): AgentTell {
  const o = obj(raw, 'tell');
  const ok = o['ok'];
  if (typeof ok !== 'boolean') throw new TellParseError('tell.ok is missing or not a boolean');
  const output = o['output'];
  if (typeof output !== 'string') {
    throw new TellParseError('tell.output is missing or not a string');
  }
  const cid = o['cid'];
  if (typeof cid !== 'number' || !Number.isInteger(cid) || cid < 0) {
    throw new TellParseError('tell.cid is missing or not a whole, non-negative number');
  }
  return { cid, ok, output };
}

/* ------------------------------------------------------------- how it ended
   The distinct, honest states a driven call can end in. [`tellStateOf`] is the
   one place that reads the endpoint's status table (crates/noded
   `tell_agent`): each refusal the hub can answer with is its own state — a role
   policy that forbids the tool, an agent that is not connected, a call that did
   not answer in time — and a node never told where the hub's admin surface is,
   or a hub out of reach, are their owns too. `sent` is the agent's own answer
   (`ok` true or false); none of the rest is ever dressed up as one. */
export type TellState =
  | 'driving'
  | 'sent'
  | 'policy'
  | 'not-connected'
  | 'timeout'
  | 'off'
  | 'unreachable'
  | 'refused';

/** The state a failed drive wears, from the status the daemon answered with —
    or from no answer at all (`null`: the node was never reached). */
export function tellStateOf(status: number | null): TellState {
  if (status === 403) return 'policy';
  if (status === 409) return 'not-connected';
  if (status === 504) return 'timeout';
  if (status === 503) return 'off';
  if (status == null || status === 502) return 'unreachable';
  return 'refused';
}

/** Parse the console's arguments box: the daemon refuses anything that is not a
    JSON object, so the console says so at the field rather than round-tripping
    a 400. An empty box is the empty object (a tool that takes no arguments). */
export function parseTellArgs(text: string): { args: Record<string, unknown> } | { error: string } {
  const trimmed = text.trim();
  if (trimmed === '') return { args: {} };
  let value: unknown;
  try {
    value = JSON.parse(trimmed);
  } catch (err) {
    return { error: `the arguments are not JSON: ${err instanceof Error ? err.message : String(err)}` };
  }
  if (value == null || typeof value !== 'object' || Array.isArray(value)) {
    return { error: 'the arguments must be a JSON object' };
  }
  return { args: value as Record<string, unknown> };
}
