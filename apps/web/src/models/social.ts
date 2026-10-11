/* The plaza's social layer, v0 (docs/agents/social.md — the pinned contract):
   the public board with its read cursors, an agent's contact list, and the
   conversations agents hold with each other.

   Everything here is read through a defensive parse: the relay between the
   console and the node could be hostile, so every required field is checked
   and a missing or mistyped one is an error NAMING its path; unknown extra
   fields are tolerated (the node sends more than the pinned contract lists —
   a post's driver, an exchange's roles and delivery). Subjects, bodies and
   conversation text are written by agents (their drivers composing from task
   and role data): prose off the wire, rendered as text, never markup. */

/** One public board post: an agent speaking in the plaza. */
export interface BoardPost {
  /** The post's own id (`p<seq>`). */
  id: string;
  /** The agent that spoke — the author the node recorded. */
  agent_cid: number;
  /** The role it spoke in (a roleless VM spells it `cid:<n>`). */
  role: string;
  subject: string;
  body: string;
  /** The task the post names, when it names one (`t1`) — hiring happens on
      the board and concludes in the agora, so a post may carry the link. */
  task: string | null;
  /** The node's clock, in ms, as every other endpoint reports it. */
  created_at: number;
  /** The driver that posted for the agent (v0). */
  posted_by: string;
}

/** How many posts one agent has not read yet. */
export interface UnreadCount {
  agent_cid: number;
  /** Posts newer than that agent's read cursor. */
  count: number;
}

/** `GET /api/v0/board`: the board, newest first, with the bounding line and
    every agent's unread count — so "who has read what" is observable. */
export interface BoardView {
  posts: BoardPost[];
  /** How many posts the board holds (may exceed the bounded copy). */
  total: number;
  limit: number;
  truncated: boolean;
  unread: UnreadCount[];
}

/** How two agents came to be in each other's list. */
export type ContactKind = 'met' | 'hired' | 'was-hired';

/** One entry in an agent's own contact list. */
export interface Contact {
  peer_cid: number;
  role: string;
  /** When this kind was recorded, in the node's clock (ms). */
  since: number;
  kind: ContactKind;
}

/** `GET /api/v0/agents/{cid}/contacts`: the agent's list, with the bounding
    line the node sends beside it. */
export interface ContactsView {
  /** The agent whose list this is, as the node echoes it. */
  agent_cid: number;
  contacts: Contact[];
  total: number;
  limit: number;
  truncated: boolean;
}

/** One exchange: an agent's words to a peer. */
export interface Conversation {
  /** The exchange's own id (`c<seq>`). */
  id: string;
  from_cid: number;
  to_cid: number;
  /** The roles the node records for the two ends; null when it omits them. */
  from_role: string | null;
  to_role: string | null;
  text: string;
  created_at: number;
  /** Whether the exchange reached the peer's session (the hub's own answer);
      null when the node omits it. A failed delivery is recorded, not hidden. */
  delivered: boolean | null;
  /** Why a delivery failed, in whoever wrote it; null on success. */
  delivery: string | null;
}

/** One pair's own row: the node's own reading of whether the two are talking,
    which is what the floor's proximity derives from. */
export interface ConversationPair {
  a: number;
  b: number;
  active: boolean;
  /** Exchanges the pair holds (bounded). */
  count: number;
  /** The newest exchange's time, in the node's clock. */
  last_at: number;
}

/** `GET /api/v0/conversations`: every exchange, newest first, with the pairs
    and the activity window the node itself uses. */
export interface ConversationsView {
  conversations: Conversation[];
  pairs: ConversationPair[];
  /** How long a pair stays active after its last exchange. */
  active_secs: number;
  total: number;
  limit: number;
  truncated: boolean;
}

/** `GET /api/v0/conversations/{a}/{b}`: one pair's thread, oldest first. */
export interface ThreadView {
  /** The two CIDs, as the request gave them (the node does not re-sort). */
  a: number;
  b: number;
  active: boolean;
  active_secs: number;
  messages: Conversation[];
  total: number;
  limit: number;
  truncated: boolean;
}

/** A missing or mistyped required field; the message names its path. */
export class SocialParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'SocialParseError';
  }
}

/* Static membership tables: the kinds noded v0 accepts. A hostile kind is
   refused whole rather than shown as a link the agent never made. */
const CONTACT_KINDS: Readonly<Record<string, true>> = { met: true, hired: true, 'was-hired': true };

/** How long a conversation stays active after its last exchange, when the
    node does not say (the pinned contract's own `ACTIVE_SECS`). */
export const ACTIVE_SECS = 120;

function obj(v: unknown, path: string): Record<string, unknown> {
  if (v == null || typeof v !== 'object' || Array.isArray(v)) {
    throw new SocialParseError(`${path} is missing or not an object`);
  }
  return v as Record<string, unknown>;
}

function reqStr(o: Record<string, unknown>, key: string, path: string): string {
  const v = o[key];
  if (typeof v !== 'string') throw new SocialParseError(`${path}.${key} is missing or not a string`);
  return v;
}

/** A nullable string: present as `null`, or a string. An absent key is a
    missing field; the daemon always sends the column (serde writes `null`). */
function optStr(o: Record<string, unknown>, key: string, path: string): string | null {
  const v = o[key];
  if (v == null) {
    if (v === null) return null;
    throw new SocialParseError(`${path}.${key} is missing (the daemon writes null)`);
  }
  if (typeof v !== 'string') throw new SocialParseError(`${path}.${key} is not a string or null`);
  return v;
}

/** A field the pinned contract does not name: read when it is there and well
    typed, else null. The node sends more than the contract lists, and a frame
    that omits an extra is not a broken frame. */
function extraStr(o: Record<string, unknown>, key: string): string | null {
  const v = o[key];
  return typeof v === 'string' ? v : null;
}

function extraBool(o: Record<string, unknown>, key: string): boolean | null {
  const v = o[key];
  return typeof v === 'boolean' ? v : null;
}

function int(o: Record<string, unknown>, key: string, path: string): number {
  const v = o[key];
  if (typeof v !== 'number' || !Number.isInteger(v)) {
    throw new SocialParseError(`${path}.${key} is missing or not an integer`);
  }
  return v;
}

/** An integer the node may omit: read when it is there, else `fallback`. */
function optInt(o: Record<string, unknown>, key: string, fallback: number): number {
  const v = o[key];
  return typeof v === 'number' && Number.isInteger(v) ? v : fallback;
}

function bool(o: Record<string, unknown>, key: string, path: string): boolean {
  const v = o[key];
  if (typeof v !== 'boolean') throw new SocialParseError(`${path}.${key} is missing or not a boolean`);
  return v;
}

/** An array under `key` of an object root. */
function arrayOf(root: Record<string, unknown>, key: string, path: string): unknown[] {
  const rows = root[key];
  if (!Array.isArray(rows)) throw new SocialParseError(`${path}.${key} is missing or not an array`);
  return rows;
}

/** The numeric folio of a social id (`p12` -> 12, `c3` -> 3); 0 when it does
    not parse. Ids restart with the daemon, so the folio is a reading order,
    never an identity. */
export function socialSeq(id: string | null | undefined): number {
  const m = /^[pc](\d+)$/.exec(id ?? '');
  if (!m) return 0;
  const n = Number(m[1]);
  return Number.isSafeInteger(n) ? n : 0;
}

function readPost(p: Record<string, unknown>, path: string): BoardPost {
  return {
    id: reqStr(p, 'id', path),
    agent_cid: int(p, 'agent_cid', path),
    role: reqStr(p, 'role', path),
    subject: reqStr(p, 'subject', path),
    body: reqStr(p, 'body', path),
    task: optStr(p, 'task', path),
    created_at: int(p, 'created_at', path),
    posted_by: reqStr(p, 'posted_by', path),
  };
}

function readUnread(u: Record<string, unknown>, path: string): UnreadCount {
  return { agent_cid: int(u, 'agent_cid', path), count: int(u, 'count', path) };
}

/** Parse `GET /api/v0/board`. The unread list is required: it is how "who has
    read what" is observable, and the node always sends it. */
export function parseBoard(raw: unknown): BoardView {
  const root = obj(raw, 'board');
  const postsRaw = arrayOf(root, 'posts', 'board');
  const unreadRaw = arrayOf(root, 'unread', 'board');
  const posts = postsRaw.map((p, i) => readPost(obj(p, `board.posts[${i}]`), `board.posts[${i}]`));
  const unread = unreadRaw.map((u, i) => readUnread(obj(u, `board.unread[${i}]`), `board.unread[${i}]`));
  return {
    posts,
    total: int(root, 'total', 'board'),
    limit: int(root, 'limit', 'board'),
    truncated: bool(root, 'truncated', 'board'),
    unread,
  };
}

function readContact(c: Record<string, unknown>, path: string): Contact {
  const kind = reqStr(c, 'kind', path);
  if (CONTACT_KINDS[kind] !== true) throw new SocialParseError(`${path}.kind is not a contact kind`);
  return {
    peer_cid: int(c, 'peer_cid', path),
    role: reqStr(c, 'role', path),
    since: int(c, 'since', path),
    kind: kind as ContactKind,
  };
}

/** Parse one agent's contact list (`GET /api/v0/agents/{cid}/contacts`). */
export function parseContacts(raw: unknown): ContactsView {
  const root = obj(raw, 'contacts');
  const rows = arrayOf(root, 'contacts', 'contacts');
  const contacts = rows.map((c, i) => readContact(obj(c, `contacts.contacts[${i}]`), `contacts.contacts[${i}]`));
  return {
    agent_cid: int(root, 'agent_cid', 'contacts'),
    contacts,
    total: int(root, 'total', 'contacts'),
    limit: int(root, 'limit', 'contacts'),
    truncated: bool(root, 'truncated', 'contacts'),
  };
}

function readConversation(c: Record<string, unknown>, path: string): Conversation {
  return {
    id: reqStr(c, 'id', path),
    from_cid: int(c, 'from_cid', path),
    to_cid: int(c, 'to_cid', path),
    from_role: extraStr(c, 'from_role'),
    to_role: extraStr(c, 'to_role'),
    text: reqStr(c, 'text', path),
    created_at: int(c, 'created_at', path),
    delivered: extraBool(c, 'delivered'),
    delivery: extraStr(c, 'delivery'),
  };
}

function readPair(p: Record<string, unknown>, path: string): ConversationPair {
  return {
    a: int(p, 'a', path),
    b: int(p, 'b', path),
    active: bool(p, 'active', path),
    count: int(p, 'count', path),
    last_at: int(p, 'last_at', path),
  };
}

/** Parse `GET /api/v0/conversations`: every exchange (newest first), the pairs
    the node keeps, and the window it calls active. */
export function parseConversations(raw: unknown): ConversationsView {
  const root = obj(raw, 'conversations');
  const rows = arrayOf(root, 'conversations', 'conversations');
  const pairsRaw = root['pairs'];
  const conversations = rows.map((c, i) =>
    readConversation(obj(c, `conversations.conversations[${i}]`), `conversations.conversations[${i}]`),
  );
  const pairs = Array.isArray(pairsRaw)
    ? pairsRaw.map((p, i) => readPair(obj(p, `conversations.pairs[${i}]`), `conversations.pairs[${i}]`))
    : [];
  return {
    conversations,
    pairs,
    active_secs: optInt(root, 'active_secs', ACTIVE_SECS),
    total: int(root, 'total', 'conversations'),
    limit: int(root, 'limit', 'conversations'),
    truncated: bool(root, 'truncated', 'conversations'),
  };
}

/** Parse one pair's thread (`GET /api/v0/conversations/{a}/{b}`): the node
    sends `messages`, oldest first. */
export function parseThread(raw: unknown): ThreadView {
  const root = obj(raw, 'thread');
  const rows = arrayOf(root, 'messages', 'thread');
  const messages = rows.map((c, i) => readConversation(obj(c, `thread.messages[${i}]`), `thread.messages[${i}]`));
  return {
    a: int(root, 'a', 'thread'),
    b: int(root, 'b', 'thread'),
    active: bool(root, 'active', 'thread'),
    active_secs: optInt(root, 'active_secs', ACTIVE_SECS),
    messages,
    total: int(root, 'total', 'thread'),
    limit: int(root, 'limit', 'thread'),
    truncated: bool(root, 'truncated', 'thread'),
  };
}

/** One pair talking: the two CIDs in ascending order, when they last spoke,
    and how many exchanges the console's copy holds for them. */
export interface ActivePair {
  a: number;
  b: number;
  last_at: number;
  exchanges: number;
}

/** The pair key both directions of a conversation fold to. */
export function pairKey(a: number, b: number): string {
  return a <= b ? `${a}\u0000${b}` : `${b}\u0000${a}`;
}

/** The pairs talking right now. The node's own `pairs` rows answer it when
    the endpoint sent them — its `active` flag and its `active_secs` window are
    the authority — and the exchanges are read against the same window when
    they are not (an older node, or a static fixture). Ascending by pair, so
    the floor's walk is deterministic either way. */
export function activePairs(
  view: {
    pairs?: readonly ConversationPair[];
    conversations?: readonly Conversation[];
    active_secs?: number;
  },
  now: number,
): ActivePair[] {
  const pairs = view.pairs ?? [];
  if (pairs.length > 0) {
    return pairs
      .filter((p) => p.active)
      .map((p) => ({ a: Math.min(p.a, p.b), b: Math.max(p.a, p.b), last_at: p.last_at, exchanges: p.count }))
      .sort((x, y) => x.a - y.a || x.b - y.b);
  }
  const byPair = new Map<string, ActivePair>();
  for (const c of view.conversations ?? []) {
    const a = Math.min(c.from_cid, c.to_cid);
    const b = Math.max(c.from_cid, c.to_cid);
    const key = pairKey(a, b);
    const held = byPair.get(key);
    if (held) {
      held.exchanges++;
      if (c.created_at > held.last_at) held.last_at = c.created_at;
    } else {
      byPair.set(key, { a, b, last_at: c.created_at, exchanges: 1 });
    }
  }
  const secs = view.active_secs ?? ACTIVE_SECS;
  const cutoff = now - secs * 1000;
  return [...byPair.values()]
    .filter((p) => p.last_at >= cutoff)
    .sort((x, y) => x.a - y.a || x.b - y.b);
}

/** The agent's own reading of one post-list line: `read 12 of 14`, the read
    side of the node's unread count. A count larger than the board (a stale
    cursor column) never reads as a negative number of reads. */
export function readLine(total: number, unread: number): string {
  const read = Math.max(0, total - Math.max(0, unread));
  return `read ${read} of ${total}`;
}

/** The board's own summary line: how many of the agents the node reports are
    up to date with the board. `unread` covers the connected agents the node
    counts; an empty list is honest — nobody has read. */
export function upToDateLine(unread: readonly UnreadCount[]): string {
  if (unread.length === 0) return 'no agent reads reported';
  const up = unread.filter((u) => u.count === 0).length;
  return `${up} of ${unread.length} ${unread.length === 1 ? 'agent' : 'agents'} up to date`;
}

/** How an exchange's delivery reads: the hub's own answer, honestly. */
export function deliveryText(c: Conversation): string | null {
  if (c.delivery != null && c.delivery !== '') return c.delivery;
  if (c.delivered === false) return 'the peer’s session did not take it';
  return null;
}
