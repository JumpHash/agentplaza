/* The hub's frozen status surface, read through the daemon's same-origin
   proxy (GET /api/v0/hub). Two upstreams, two shapes: the register's runs
   come from noded, and this is the hub's — the vsock daemon that serves the
   fleet's agents. The proxy hands back the hub's own /status bytes on
   success and a 502/503 {"error":"hub unreachable: …"} otherwise.

   The relay between the console and the daemon could be hostile, so this is
   READ THROUGH A DEFENSIVE PARSE, never trusted: every required field is
   checked, a missing or mistyped one is an error that NAMES it, and unknown
   extra fields are tolerated (the hub may grow fields the console does not
   yet read). It is read-only — the console never writes the hub, and nothing
   here is ever sent back. */

import { fmtDur, fmtNum } from '../format';

export interface HubNode {
  /** Every vsock CID the hub serves: the machine's own (1-2) plus each VM's. */
  cids: number[];
  port: number;
  uptime_s: number;
  max_sessions: number;
  keepalive_idle_s: number;
}

export interface HubAgent {
  /** The guest's vsock CID — the join key against the register's VMs. */
  cid: number;
  role: string;
  connected_at_ms: number;
  last_heartbeat_ms: number;
  calls: number;
}

export interface HubStatus {
  node: HubNode;
  agents: HubAgent[];
  sessions_active: number;
}

/** A missing or mistyped required field; the message names the field. */
export class HubParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'HubParseError';
  }
}

function obj(v: unknown, path: string): Record<string, unknown> {
  if (v == null || typeof v !== 'object' || Array.isArray(v)) {
    throw new HubParseError(`${path} is missing or not an object`);
  }
  return v as Record<string, unknown>;
}

function num(o: Record<string, unknown>, key: string, path: string): number {
  const v = o[key];
  if (typeof v !== 'number' || !Number.isFinite(v)) {
    throw new HubParseError(`${path}.${key} is missing or not a number`);
  }
  return v;
}

function int(o: Record<string, unknown>, key: string, path: string): number {
  const n = num(o, key, path);
  if (!Number.isInteger(n)) throw new HubParseError(`${path}.${key} is not an integer`);
  return n;
}

function str(o: Record<string, unknown>, key: string, path: string): string {
  const v = o[key];
  if (typeof v !== 'string') throw new HubParseError(`${path}.${key} is missing or not a string`);
  return v;
}

/** Parse the hub's frozen status defensively. Unknown extra fields are
    tolerated; every required field — a missing one is an error naming it. */
export function parseHubStatus(raw: unknown): HubStatus {
  const root = obj(raw, 'hub');
  const nodeRaw = obj(root['node'], 'hub.node');
  const cidsRaw = nodeRaw['cids'];
  if (!Array.isArray(cidsRaw)) throw new HubParseError('hub.node.cids is missing or not an array');
  const cids: number[] = [];
  for (let i = 0; i < cidsRaw.length; i++) {
    const c = cidsRaw[i];
    if (typeof c !== 'number' || !Number.isInteger(c)) {
      throw new HubParseError(`hub.node.cids[${i}] is not an integer`);
    }
    cids.push(c);
  }
  const node: HubNode = {
    cids,
    port: num(nodeRaw, 'port', 'hub.node'),
    uptime_s: num(nodeRaw, 'uptime_s', 'hub.node'),
    max_sessions: num(nodeRaw, 'max_sessions', 'hub.node'),
    keepalive_idle_s: num(nodeRaw, 'keepalive_idle_s', 'hub.node'),
  };
  const agentsRaw = root['agents'];
  if (!Array.isArray(agentsRaw)) throw new HubParseError('hub.agents is missing or not an array');
  const agents: HubAgent[] = agentsRaw.map((a, i) => {
    const path = `hub.agents[${i}]`;
    const o = obj(a, path);
    return {
      cid: int(o, 'cid', path),
      role: str(o, 'role', path),
      connected_at_ms: num(o, 'connected_at_ms', path),
      last_heartbeat_ms: num(o, 'last_heartbeat_ms', path),
      calls: num(o, 'calls', path),
    };
  });
  return { node, agents, sessions_active: num(root, 'sessions_active', 'hub') };
}

/* ------------------------------------------------------------- the hub line
   The hub as one ruled line in the register's voice, or its own honest
   unreachable reading — distinct from an empty hub, which still names its
   served CIDs and its zeroes. */
export function hubLine(hub: HubStatus | null, error: string | null): string {
  if (error != null) return `hub · unreachable — ${error}`;
  if (hub == null) return 'hub · reading…';
  const n = hub.node;
  return (
    `hub · ${fmtNum(n.cids.length)} cid(s) served · ` +
    `${fmtNum(hub.agents.length)} agent(s) connected · ` +
    `${fmtNum(hub.sessions_active)} session(s) active · ` +
    `up ${fmtDur(n.uptime_s * 1000)}`
  );
}

/* --------------------------------------------------------------- the join
   The hub's connected agents against the register's role rows. The join key
   is the guest CID — the one identifier both sides carry. A row is honest
   about who knows whom: both, only the hub (`connected, not registered`), or
   only the register (`registered, not connected`); the two are never
   silently merged. */

/** The least a register row must carry to join: its role and its guest CID. */
export interface RegisteredRef {
  cid: number | null;
  role: string;
}

export type HubPresence = 'both' | 'connected-only' | 'registered-only';

export interface HubJoinRow<R extends RegisteredRef = RegisteredRef> {
  cid: number | null;
  /** The register's role when it names one, else the hub's. */
  role: string | null;
  presence: HubPresence;
  registered: R | null;
  connected: HubAgent | null;
}

export function joinHub<R extends RegisteredRef>(
  registered: readonly R[],
  connected: readonly HubAgent[],
): Array<HubJoinRow<R>> {
  const rows: Array<HubJoinRow<R>> = [];
  const byCid = new Map<number, R>();
  for (const r of registered) {
    /* a role row with no vsock CID cannot be joined — it stands as its own
       register-only row rather than being dropped or matched by guesswork */
    if (r.cid == null) {
      rows.push({ cid: null, role: r.role, presence: 'registered-only', registered: r, connected: null });
      continue;
    }
    if (!byCid.has(r.cid)) byCid.set(r.cid, r);
  }
  const seen = new Set<number>();
  for (const a of connected) {
    const reg = byCid.get(a.cid) ?? null;
    seen.add(a.cid);
    rows.push({
      cid: a.cid,
      role: reg?.role ?? a.role,
      presence: reg ? 'both' : 'connected-only',
      registered: reg,
      connected: a,
    });
  }
  for (const [cid, reg] of byCid) {
    if (seen.has(cid)) continue;
    rows.push({ cid, role: reg.role, presence: 'registered-only', registered: reg, connected: null });
  }
  rows.sort((x, y) => (x.cid ?? Number.MAX_SAFE_INTEGER) - (y.cid ?? Number.MAX_SAFE_INTEGER));
  return rows;
}
