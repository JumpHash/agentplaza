/* The council v0's frozen shapes (crates/noded council::MotionsView /
   MotionView / VoteView / TallyView), read through a defensive parse: the
   relay between the console and the node could be hostile, so this is never
   trusted. Every field is checked and a missing or mistyped one is an error
   that NAMES its path; unknown extra fields are tolerated, since the schema
   may grow columns the console does not yet read.

   A motion is the record of something the council did (docs/agents/council.md):
   a seat proposes it, the four seats vote, and the count (0034) decides. The
   daemon DERIVES a motion's `state` (from its votes) and its `needs_owner`
   flag (from its kind, and — for a budget move — the amount against the
   minted pool), so the console reads both and never recomputes a decision: a
   state it disagreed with would be a second count, not the register's.

   The motion's `text` and every vote's `reason` are prose the agents wrote
   off the wire: rendered as text, never markup.

   The owner's decision sits on the same motion (`owner`): the operator — no
   agent CID, holding no seat — ratifies or vetoes a motion the count gated to
   them (the 2–2 tie, or a carry above the council's authority), and the daemon
   records `{approve, note|null, at}` and moves the state to `ratified` or
   `vetoed`. The `owner` key is always present (null until decided); the two
   new states are the owner's own and no tally can derive them. */

import { utf8ByteLength } from '../serial';

/** The kinds a motion may be (the schema's enum, council::Kind). */
export type MotionKind =
  | 'priority'
  | 'budget_move'
  | 'town_proposal'
  | 'mayor_replacement'
  | 'law_proposal'
  | 'other';

/** A motion's state, derived by the daemon: the count's own five
    (0034) — fewer than four seats voted (`open`), a majority of yes below the
    gate (`carried`), a majority of no (`rejected`), an even 2–2 split that
    escalates to the owner (`tie_to_owner`), carried but owner-gated
    (`awaiting_owner`) — and the **owner's decision**, which no tally can
    derive: `ratified` the owner approved a motion the count gated to them,
    `vetoed` the owner refused it. The last two appear only after the owner's
    hand ([`Motion.owner`]); a motion without a decision is never either. */
export type MotionState =
  | 'open'
  | 'carried'
  | 'rejected'
  | 'tie_to_owner'
  | 'awaiting_owner'
  | 'ratified'
  | 'vetoed';

/** One vote word (`yes | no | abstain`). */
export type VoteWord = 'yes' | 'no' | 'abstain';

/** One seat's vote as the API reports it: which seat cast it, the agent that
    acted for that seat, the word, the reason, and when. */
export interface Vote {
  /** The seat that voted (its role in the fleet's roles). */
  seat: string;
  /** The guest CID of the agent that cast the seat's vote. */
  agent_cid: number;
  vote: VoteWord;
  /** Why, in the voter's own words. */
  reason: string;
  /** When the vote was cast, in milliseconds since the Unix epoch. */
  at: number;
}

/** A motion's tally: the votes cast, against the seats. */
export interface Tally {
  /** How many seats voted yes. */
  yes: number;
  /** How many voted no. */
  no: number;
  /** How many abstained. */
  abstain: number;
  /** How many seats have voted, however they voted. */
  voted: number;
  /** How many seats there are. */
  seats: number;
}

/** The owner's decision on a motion the count gated to them, as the API reports
    it. */
export interface OwnerDecision {
  /** Whether the owner approved the motion: `true` → `ratified`, `false` →
      `vetoed`. */
  approve: boolean;
  /** The note the owner left, or `null` when they left none. */
  note: string | null;
  /** When the owner decided, in milliseconds since the Unix epoch. */
  at: number;
}

/** One motion as the API reports it. */
export interface Motion {
  /** `m<seq>`, monotonic and never reused. */
  id: string;
  /** The guest CID of the agent that proposed it. */
  proposer_cid: number;
  /** The proposing seat (its role). */
  proposer_role: string;
  kind: MotionKind;
  /** The motion itself, as prose. */
  text: string;
  /** The amount a `budget_move` names, or null. The field is ABSENT for a
      motion that names none (the daemon skips it), so absence is read as no
      amount — never a missing field. */
  amount: number | null;
  /** Whether the decision is owner-gated — derived by the daemon, never the
      caller's word for it. */
  needs_owner: boolean;
  state: MotionState;
  /** The seats' votes, in seat order (a re-vote replaces its seat's vote, so a
      seat can never be counted twice). */
  votes: Vote[];
  tally: Tally;
  /** The owner's decision, or `null` until the owner makes one. **The key is
      always present on the wire** (a Column-style optional, like the desktop
      client's `Column<T>`): `null` reads "the owner has not decided", never
      "the field was absent" — so the console refuses a row whose `owner` key is
      missing, by name, rather than reading a decided motion as undecided. */
  owner: OwnerDecision | null;
  /** When the motion was proposed, in milliseconds since the Unix epoch. */
  created_at: number;
  /** When it last changed (its proposal, or its latest vote), in ms. */
  updated_at: number;
}

/** `GET /api/v0/motions`'s body: the motions, with the bounding line the
    journal endpoint carries. */
export interface Motions {
  /** Up to the endpoint's limit of motions, newest first. */
  motions: Motion[];
  /** How many motions the council holds. */
  total: number;
  /** The limit the response was cut to. */
  limit: number;
  /** Whether the council holds more motions than the response carries. */
  truncated: boolean;
}

/** The doctrine's seat count. The tally's arithmetic rests on it, and
    [`tallyHolds`] is the one place the console asserts it. */
export const SEAT_COUNT = 4;

/** A missing or mistyped required field; the message names its path. */
export class CouncilParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'CouncilParseError';
  }
}

/* Static membership tables: the kinds, the states and the vote words the
   daemon accepts, so a hostile value is refused by name. */
const KINDS: Readonly<Record<string, true>> = {
  priority: true,
  budget_move: true,
  town_proposal: true,
  mayor_replacement: true,
  law_proposal: true,
  other: true,
};
const STATES: Readonly<Record<string, true>> = {
  open: true,
  carried: true,
  rejected: true,
  tie_to_owner: true,
  awaiting_owner: true,
  ratified: true,
  vetoed: true,
};
const VOTES: Readonly<Record<string, true>> = { yes: true, no: true, abstain: true };

function obj(v: unknown, path: string): Record<string, unknown> {
  if (v == null || typeof v !== 'object' || Array.isArray(v)) {
    throw new CouncilParseError(`${path} is missing or not an object`);
  }
  return v as Record<string, unknown>;
}

function reqStr(o: Record<string, unknown>, key: string, path: string): string {
  const v = o[key];
  if (typeof v !== 'string') throw new CouncilParseError(`${path}.${key} is missing or not a string`);
  return v;
}

function int(o: Record<string, unknown>, key: string, path: string): number {
  const v = o[key];
  if (typeof v !== 'number' || !Number.isFinite(v)) {
    throw new CouncilParseError(`${path}.${key} is missing or not a number`);
  }
  if (!Number.isInteger(v)) throw new CouncilParseError(`${path}.${key} is not an integer`);
  return v;
}

function bool(o: Record<string, unknown>, key: string, path: string): boolean {
  /* a serde column is always written (a `bool` as true/false, an `Option` as
     `null`); absence means the field the console needs never arrived */
  if (!(key in o)) throw new CouncilParseError(`${path}.${key} is missing`);
  const v = o[key];
  if (typeof v !== 'boolean') throw new CouncilParseError(`${path}.${key} is missing or not a boolean`);
  return v;
}

/** The amount a motion names: ABSENT (the daemon skips it), `null`, or an
    integer. Absence is legitimate here and reads as no amount; a present but
    mistyped value is refused by name. */
function optAmount(o: Record<string, unknown>, key: string, path: string): number | null {
  const v = o[key];
  if (v == null) return null;
  if (typeof v !== 'number' || !Number.isInteger(v)) {
    throw new CouncilParseError(`${path}.${key} is not an integer`);
  }
  return v;
}

function kindOf(o: Record<string, unknown>, key: string, path: string): MotionKind {
  const v = o[key];
  if (typeof v !== 'string' || KINDS[v] !== true) {
    throw new CouncilParseError(`${path}.${key} is missing or not a motion kind`);
  }
  return v as MotionKind;
}

function stateOf(o: Record<string, unknown>, key: string, path: string): MotionState {
  const v = o[key];
  if (typeof v !== 'string' || STATES[v] !== true) {
    throw new CouncilParseError(`${path}.${key} is missing or not a motion state`);
  }
  return v as MotionState;
}

function voteOf(o: Record<string, unknown>, key: string, path: string): VoteWord {
  const v = o[key];
  if (typeof v !== 'string' || VOTES[v] !== true) {
    throw new CouncilParseError(`${path}.${key} is missing or not a vote`);
  }
  return v as VoteWord;
}

function readVote(v: Record<string, unknown>, path: string): Vote {
  return {
    seat: reqStr(v, 'seat', path),
    agent_cid: int(v, 'agent_cid', path),
    vote: voteOf(v, 'vote', path),
    reason: reqStr(v, 'reason', path),
    at: int(v, 'at', path),
  };
}

function readTally(t: Record<string, unknown>, path: string): Tally {
  return {
    yes: int(t, 'yes', path),
    no: int(t, 'no', path),
    abstain: int(t, 'abstain', path),
    voted: int(t, 'voted', path),
    seats: int(t, 'seats', path),
  };
}

/** The owner's decision, read through the "always keyed" rule: the `owner` key
    is required to be PRESENT — `null` reads "the owner has not decided", and a
    missing key is refused by name, so a decided motion is never read as
    undecided (and vice versa). When the key is present and not null it holds
    the three fields the daemon writes, each named on absence: `approve` (a
    boolean), `note` (a string or `null` — the key is written either way) and
    `at` (an integer). */
function readOwner(o: Record<string, unknown>, path: string): OwnerDecision | null {
  if (!('owner' in o)) {
    throw new CouncilParseError(
      `${path}.owner is missing: the owner column is always keyed (null until the owner decides)`,
    );
  }
  const v = o['owner'];
  if (v == null) return null;
  if (typeof v !== 'object' || Array.isArray(v)) {
    throw new CouncilParseError(`${path}.owner is missing or not an object`);
  }
  const d = v as Record<string, unknown>;
  if (!('note' in d)) {
    throw new CouncilParseError(
      `${path}.owner.note is missing: the note column is always keyed (null when the owner left none)`,
    );
  }
  const note = d['note'];
  if (note !== null && typeof note !== 'string') {
    throw new CouncilParseError(`${path}.owner.note is missing or not a string`);
  }
  return {
    approve: bool(d, 'approve', `${path}.owner`),
    note,
    at: int(d, 'at', `${path}.owner`),
  };
}

function readMotion(m: Record<string, unknown>, path: string): Motion {
  const votes = m['votes'];
  if (!Array.isArray(votes)) {
    throw new CouncilParseError(`${path}.votes is missing or not an array`);
  }
  const tally = m['tally'];
  if (tally == null || typeof tally !== 'object' || Array.isArray(tally)) {
    throw new CouncilParseError(`${path}.tally is missing or not an object`);
  }
  return {
    id: reqStr(m, 'id', path),
    proposer_cid: int(m, 'proposer_cid', path),
    proposer_role: reqStr(m, 'proposer_role', path),
    kind: kindOf(m, 'kind', path),
    text: reqStr(m, 'text', path),
    amount: optAmount(m, 'amount', path),
    needs_owner: bool(m, 'needs_owner', path),
    state: stateOf(m, 'state', path),
    votes: votes.map((v, i) => readVote(obj(v, `${path}.votes[${i}]`), `${path}.votes[${i}]`)),
    tally: readTally(tally as Record<string, unknown>, `${path}.tally`),
    owner: readOwner(m, path),
    created_at: int(m, 'created_at', path),
    updated_at: int(m, 'updated_at', path),
  };
}

/** Parse `GET /api/v0/motions`; unknown extras are tolerated. A bad row is an
    error naming its index. */
export function parseMotions(raw: unknown): Motions {
  const root = obj(raw, 'council');
  const list = root['motions'];
  if (!Array.isArray(list)) {
    throw new CouncilParseError('council.motions is missing or not an array');
  }
  return {
    motions: list.map((m, i) => readMotion(obj(m, `council.motions[${i}]`), `council.motions[${i}]`)),
    total: int(root, 'total', 'council'),
    limit: int(root, 'limit', 'council'),
    truncated: bool(root, 'truncated', 'council'),
  };
}

/** Parse one motion as `GET /api/v0/motions/{id}` reports it. */
export function parseMotion(raw: unknown): Motion {
  return readMotion(obj(raw, 'motion'), 'motion');
}

/** The numeric folio of a motion id (`m12` -> 12); 0 when it does not parse. */
export function motionSeq(id: string | null | undefined): number {
  const m = /^m(\d+)$/.exec(id ?? '');
  return m && m[1] ? Number(m[1]) : 0;
}

/** Whether a motion's tally squares with its own votes: the seats that voted
    are exactly the votes carried, the words sum to them, and the seat count is
    the doctrine's four.

    This is what the console ASSERTS and not what it recomputes: the parse
    checks shape and membership (a bad value is refused by name), the tests and
    the live read assert this arithmetic, and the sheet RENDERS the daemon's
    own figures as sent. A tally that disagrees is shown, and said to disagree
    — never silently corrected into a count the daemon did not derive. */
export function tallyHolds(motion: Motion): boolean {
  const t = motion.tally;
  return (
    t.seats === SEAT_COUNT &&
    t.voted === motion.votes.length &&
    t.yes + t.no + t.abstain === t.voted
  );
}

/** The plain-language line a gated motion carries: what the owner's gate is
    for, from the kind the daemon derived `needs_owner` with. `null` when the
    decision is the council's own — the console never invents a gate — and
    `null` once the owner has decided ([`Motion.owner`]): the gate is spent, and
    a line that still said the decision "waits for the owner" would be false. */
export function gateLine(motion: Motion): string | null {
  if (!motion.needs_owner || motion.owner != null) return null;
  switch (motion.kind) {
    case 'law_proposal':
      return 'above the council’s authority: a law change waits for the owner’s ratification';
    case 'town_proposal':
      return 'above the council’s authority: founding a town waits for the owner’s ratification';
    case 'mayor_replacement':
      return 'above the council’s authority: replacing a mayor waits for the owner’s ratification';
    case 'budget_move':
      return 'above the council’s authority: a move over a tenth of the minted pool waits for the owner’s ratification';
    default:
      /* a kind v0 never derives a gate for, but a later node might: say the
         gate rather than guess its rule */
      return 'above the council’s authority: this decision waits for the owner’s ratification';
  }
}

/** The owner's decision as the sheet reads it: the word their hand said and
    the verb the line spells, the stamp, and the note they left (or none).
    `null` when the owner has not decided — never a decision the console
    invented. */
export interface OwnerReading {
  /** Whether the owner approved: `true` ratified, `false` vetoed. */
  approve: boolean;
  /** The word the decision stamp carries: `yes` for a ratification, `no` for a
      veto (the register's own reading of a refusal). */
  word: 'yes' | 'no';
  /** The verb the owner's line reads: `ratified` or `vetoed`. */
  verb: 'ratified' | 'vetoed';
  /** When the owner decided, in milliseconds since the Unix epoch. */
  at: number;
  /** The note the owner left, or `null` when they left none. */
  note: string | null;
}

/** The owner's decision on a motion, as the sheet's own line reads it, or
    `null` when the owner has not decided. */
export function ownerReading(motion: Motion): OwnerReading | null {
  const d = motion.owner;
  if (d == null) return null;
  return {
    approve: d.approve,
    word: d.approve ? 'yes' : 'no',
    verb: d.approve ? 'ratified' : 'vetoed',
    at: d.at,
    note: d.note,
  };
}

/** The most an owner's decision `note` may occupy, in bytes — the daemon's own
    bound (`crates/noded/src/council.rs` `MAX_DECISION_NOTE`), so the console
    refuses a note the daemon would refuse, at the field, rather than
    round-tripping its 400. */
export const MAX_DECISION_NOTE = 1000;

/** How a decision the console took ended: `deciding` while the POST is in
    flight, `taken` the node recorded it, `refused` the node refused it (the
    409s — not owner-gated, or already decided — and every other contract
    refusal), `unknown` no motion with that id, `unreachable` the node was never
    reached. The status alone decides, so a refusal is never dressed up as a
    decision. */
export type DecisionState = 'deciding' | 'taken' | 'refused' | 'unknown' | 'unreachable';

/** The state a decide ended in, from the status the daemon answered with — or
    from no answer at all (`null`: the node was never reached). */
export function decisionStateOf(status: number | null): DecisionState {
  if (status === 200) return 'taken';
  if (status === 404) return 'unknown';
  if (status == null) return 'unreachable';
  return 'refused';
}

/** Read the optional note box: an empty (or all-whitespace) box is the owner's
    own "no note" (`{ note }` absent, which the daemon records as `null`), and a
    note past the daemon's byte bound is named at the field. The note is
    otherwise sent as the owner wrote it — never trimmed, since it is the
    record of their decision. */
export function parseDecisionNote(text: string): { note?: string } | { error: string } {
  if (text.trim() === '') return {};
  if (utf8ByteLength(text) > MAX_DECISION_NOTE) {
    return { error: `the note is longer than ${MAX_DECISION_NOTE} bytes` };
  }
  return { note: text };
}

/** The council's own figures, as the sheet's tally strip reads them. */
export interface CouncilTally {
  /** Every motion the console carries. */
  total: number;
  /** Still open — the seats may yet vote. */
  open: number;
  /** Carried, but owner-gated: waiting on the owner's ratification. */
  awaiting: number;
  /** An even split that escalated to the owner. */
  ties: number;
  /** Everything waiting on the owner: [`awaiting`] + [`ties`]. */
  owner: number;
}

/** Count the motions the console carries. The strip counts the four the owner
    reads the council by; a motion the council decided outright (`carried`
    below the gate, or `rejected`) is in the total alone. */
export function councilTally(motions: readonly Motion[]): CouncilTally {
  let open = 0;
  let awaiting = 0;
  let ties = 0;
  for (const m of motions) {
    if (m.state === 'open') open++;
    else if (m.state === 'awaiting_owner') awaiting++;
    else if (m.state === 'tie_to_owner') ties++;
  }
  return { total: motions.length, open, awaiting, ties, owner: awaiting + ties };
}

/** The kind as the sheet's marginalia spells it. */
export const KIND_LABEL: Readonly<Record<MotionKind, string>> = {
  priority: 'priority',
  budget_move: 'budget move',
  town_proposal: 'town proposal',
  mayor_replacement: 'mayor replacement',
  law_proposal: 'law proposal',
  other: 'other',
};

/** The state as the sheet's mark spells it. */
export const STATE_LABEL: Readonly<Record<MotionState, string>> = {
  open: 'open',
  carried: 'carried',
  rejected: 'rejected',
  tie_to_owner: 'tie — to the owner',
  awaiting_owner: 'awaiting the owner',
  ratified: 'ratified',
  vetoed: 'vetoed',
};
