/* The agora v0's frozen shapes (crates/noded agora::CallsView / CallView /
   BidView / AwardView), read through a defensive parse: the relay between the
   console and the node could be hostile, so this is never trusted. Every field
   is checked and a missing or mistyped one is an error that NAMES its path;
   unknown extra fields are tolerated, since the schema may grow columns the
   console does not yet read.

   A v0 call IS a roled task's offer (crates/noded agora.rs): its id is the
   task's, its `capability` the task's role, its `max_price` the task's budget
   — the same credits the treasury's journal minted into the task's escrow —
   and its `state` is `awarded` once an award closed it, the task's own state
   otherwise. The board is that derivation, so it can never disagree with the
   ledger. The bidder and the contractor are the ledger's own words (an agent's
   role, or `cid:<n>`), and the memo-free rows are rendered as text, never
   markup. */

/** The states a call reports: `awarded` once an award closed it, the task's
    own state (crates/noded tasks::TaskState) otherwise. */
export type CallState = 'open' | 'claimed' | 'done' | 'failed' | 'withdrawn' | 'awarded';

/** One bid as the API reports it: the schema's `agora.bids` row, minus what v0
    does not record (`eta`, `confidence`). */
export interface Bid {
  /** The call this bid is on (its task's id). */
  task: string;
  /** The bidding agent's guest CID — the identity a re-bid replaces. */
  agent_cid: number;
  /** The schema's `bidder`: the agent's identity in the ledger's own terms. */
  bidder: string;
  /** The price the agent asks, in whole credits. */
  price: number;
  /** When the bid was made, in milliseconds since the Unix epoch. */
  created_at: number;
}

/** One award as the API reports it: the schema's `agora.awards` row, minus
    what v0 does not record (`warranty_until`, `outcome`). */
export interface Award {
  /** The call this award closed (its task's id). */
  task: string;
  /** The schema's `contractor`: the winning agent's identity (from the bid). */
  contractor: string;
  /** The winning agent's guest CID (from the bid). */
  agent_cid: number;
  /** The price the award was made at — always the price the agent bid. */
  price: number;
  /** The schema's `escrow_entry`: the treasury entry that funded the call's
      escrow (the task's own budget mint), or null for a task with no budget. */
  escrow_entry: number | null;
  /** When the award was made, in milliseconds since the Unix epoch. */
  created_at: number;
}

/** One call as the API reports it: a roled task's offer, with the bids it
    holds and the award that closed it. */
export interface Call {
  /** The call's id: the task's id (the offer and the task are one thing). */
  task: string;
  /** The schema's `capability`: what is needed (the task's role). */
  capability: string;
  /** The schema's `spec_ref`: how the work is judged complete, or null. */
  spec_ref: string | null;
  /** The schema's `max_price`: the ceiling a bid may not exceed (the budget). */
  max_price: number;
  state: CallState;
  /** When the call opened (the task's creation), in ms since the epoch. */
  created_at: number;
  /** The bids the call holds, in bid order (a re-bid keeps its place). */
  bids: Bid[];
  /** The award that closed the call, or null while it is open. */
  award: Award | null;
}

/** `GET /api/v0/agora`'s body: the calls, with the bounding line the journal
    endpoint carries. */
export interface Calls {
  /** Up to the endpoint's limit of calls, oldest first as the API returns them. */
  calls: Call[];
  /** How many calls the board holds. */
  total: number;
  /** The limit the response was cut to. */
  limit: number;
  /** Whether the board holds more calls than the response carries. */
  truncated: boolean;
}

/** A missing or mistyped required field; the message names its path. */
export class AgoraParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'AgoraParseError';
  }
}

/* Static membership tables: a call's states (the task's own, plus `awarded`),
   so a hostile value is refused by name. */
const STATES: Readonly<Record<string, true>> = {
  open: true,
  claimed: true,
  done: true,
  failed: true,
  withdrawn: true,
  awarded: true,
};

function obj(v: unknown, path: string): Record<string, unknown> {
  if (v == null || typeof v !== 'object' || Array.isArray(v)) {
    throw new AgoraParseError(`${path} is missing or not an object`);
  }
  return v as Record<string, unknown>;
}

function reqStr(o: Record<string, unknown>, key: string, path: string): string {
  const v = o[key];
  if (typeof v !== 'string') throw new AgoraParseError(`${path}.${key} is missing or not a string`);
  return v;
}

/** A nullable string: present as `null`, or a string. An absent key is a
    missing field; the daemon always sends the column (serde writes `null`). */
function optStr(o: Record<string, unknown>, key: string, path: string): string | null {
  const v = o[key];
  if (v == null) {
    if (!(key in o)) throw new AgoraParseError(`${path}.${key} is missing`);
    return null;
  }
  if (typeof v !== 'string') throw new AgoraParseError(`${path}.${key} is not a string or null`);
  return v;
}

function int(o: Record<string, unknown>, key: string, path: string): number {
  const v = o[key];
  if (typeof v !== 'number' || !Number.isFinite(v)) {
    throw new AgoraParseError(`${path}.${key} is missing or not a number`);
  }
  if (!Number.isInteger(v)) throw new AgoraParseError(`${path}.${key} is not an integer`);
  return v;
}

/** A nullable integer: present as a number, or `null`. An absent key is a
    missing field; a non-integer is refused by name. */
function optInt(o: Record<string, unknown>, key: string, path: string): number | null {
  const v = o[key];
  if (v == null) {
    if (!(key in o)) throw new AgoraParseError(`${path}.${key} is missing`);
    return null;
  }
  if (typeof v !== 'number' || !Number.isInteger(v)) {
    throw new AgoraParseError(`${path}.${key} is not an integer or null`);
  }
  return v;
}

function bool(o: Record<string, unknown>, key: string, path: string): boolean {
  const v = o[key];
  if (typeof v !== 'boolean') throw new AgoraParseError(`${path}.${key} is missing or not a boolean`);
  return v;
}

function stateOf(o: Record<string, unknown>, key: string, path: string): CallState {
  const v = o[key];
  if (typeof v !== 'string' || STATES[v] !== true) {
    throw new AgoraParseError(`${path}.${key} is missing or not a call state`);
  }
  return v as CallState;
}

function readBid(b: Record<string, unknown>, path: string): Bid {
  return {
    task: reqStr(b, 'task', path),
    agent_cid: int(b, 'agent_cid', path),
    bidder: reqStr(b, 'bidder', path),
    price: int(b, 'price', path),
    created_at: int(b, 'created_at', path),
  };
}

function readAward(a: Record<string, unknown>, path: string): Award {
  return {
    task: reqStr(a, 'task', path),
    contractor: reqStr(a, 'contractor', path),
    agent_cid: int(a, 'agent_cid', path),
    price: int(a, 'price', path),
    escrow_entry: optInt(a, 'escrow_entry', path),
    created_at: int(a, 'created_at', path),
  };
}

function readCall(c: Record<string, unknown>, path: string): Call {
  const bids = c['bids'];
  if (!Array.isArray(bids)) {
    throw new AgoraParseError(`${path}.bids is missing or not an array`);
  }
  /* an award is present as `null` (never absent — serde writes the column); a
     non-null value must be an object, refused by name when it is not */
  const rawAward = c['award'];
  let award: Award | null;
  if (rawAward == null) {
    if (!('award' in c)) throw new AgoraParseError(`${path}.award is missing`);
    award = null;
  } else {
    award = readAward(obj(rawAward, `${path}.award`), `${path}.award`);
  }
  return {
    task: reqStr(c, 'task', path),
    capability: reqStr(c, 'capability', path),
    spec_ref: optStr(c, 'spec_ref', path),
    max_price: int(c, 'max_price', path),
    state: stateOf(c, 'state', path),
    created_at: int(c, 'created_at', path),
    bids: bids.map((b, i) => readBid(obj(b, `${path}.bids[${i}]`), `${path}.bids[${i}]`)),
    award,
  };
}

/** Parse `GET /api/v0/agora`; unknown extras are tolerated. A bad row is an
    error naming its index. */
export function parseAgora(raw: unknown): Calls {
  const root = obj(raw, 'agora');
  const calls = root['calls'];
  if (!Array.isArray(calls)) {
    throw new AgoraParseError('agora.calls is missing or not an array');
  }
  return {
    calls: calls.map((c, i) => readCall(obj(c, `agora.calls[${i}]`), `agora.calls[${i}]`)),
    total: int(root, 'total', 'agora'),
    limit: int(root, 'limit', 'agora'),
    truncated: bool(root, 'truncated', 'agora'),
  };
}

/** The bid an awarded call was won at: the one bid the award names, matched by
    the winning agent's CID (and its price, since an award is always made at
    the price bid). Null while the call is open, and null for an award that
    names a bid the board no longer carries — the console marks only a bid it
    can actually see, and invents no winner. */
export function winningBid(call: Call): Bid | null {
  const award = call.award;
  if (award == null) return null;
  return call.bids.find((b) => b.agent_cid === award.agent_cid && b.price === award.price) ?? null;
}
