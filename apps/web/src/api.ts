/* The JSON client for the frozen /api/v0 contract. Errors are values the
   store keeps and shows — never thrown away, never swallowed. */

import type { Calls } from './models/agora';
import { parseAgora } from './models/agora';
import type { TaskArtifact } from './models/artifact';
import { parseArtifact } from './models/artifact';
import type { HubStatus } from './models/hub';
import { parseHubStatus } from './models/hub';
import type { Machine } from './models/machine';
import type { CreateRun, Run, RunsView } from './models/run';
import type { Task } from './models/task';
import { parseTask, parseTasksView } from './models/task';
import type { AgentTell } from './models/tell';
import { parseTell } from './models/tell';
import type { Journal, Treasury } from './models/treasury';
import { parseJournal, parseTreasury } from './models/treasury';

export class ApiError extends Error {
  readonly status: number | null;
  constructor(message: string, status: number | null) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

interface ApiOptions {
  method?: string;
  body?: unknown;
}

export async function api<T>(path: string, opts: ApiOptions = {}): Promise<T> {
  const init: RequestInit = { cache: 'no-store', method: opts.method ?? 'GET' };
  if (opts.body != null) {
    init.method = opts.method ?? 'POST';
    init.headers = { 'content-type': 'application/json' };
    init.body = JSON.stringify(opts.body);
  }
  let res: Response;
  try {
    res = await fetch(path, init);
  } catch (err) {
    throw new ApiError(err instanceof Error ? err.message : String(err), null);
  }
  const body: unknown = await res.json().catch(() => null);
  if (!res.ok) throw new ApiError(errorMessage(body, res.status), res.status);
  return body as T; // the /api/v0 contract is frozen; the daemon owns the shape
}

function errorMessage(body: unknown, status: number): string {
  if (body && typeof body === 'object') {
    if ('error' in body && body.error != null) return String(body.error);
    if ('message' in body && body.message != null) return String(body.message);
  }
  return `HTTP ${status}`;
}

export function getMachine(): Promise<Machine> {
  return api<Machine>('/api/v0/machine');
}

export function listRuns(): Promise<RunsView> {
  return api<RunsView>('/api/v0/runs');
}

export function getRun(id: string): Promise<Run> {
  return api<Run>(`/api/v0/runs/${encodeURIComponent(id)}`);
}

export function createRun(body: CreateRun): Promise<Run> {
  return api<Run>('/api/v0/runs', { method: 'POST', body });
}

export function sendInput(id: string, vm: number, data: string): Promise<unknown> {
  return api(`/api/v0/runs/${encodeURIComponent(id)}/input`, { method: 'POST', body: { data, vm } });
}

export function deleteRun(id: string): Promise<Run> {
  return api<Run>(`/api/v0/runs/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

/** The hub's frozen status, through the daemon's same-origin proxy
    (GET /api/v0/hub). The proxy answers the hub's own /status bytes on
    success and 502/503 {"error":"hub unreachable: …"} otherwise; either way
    the body goes through the hub model's defensive parse — the relay is
    untrusted, and a missing field is an error naming it, never a silent zero.
    Read-only: the console only ever GETs the hub. */
export async function getHub(): Promise<HubStatus> {
  return parseHubStatus(await api<unknown>('/api/v0/hub'));
}

/* ------------------------------------------------------------- the ledger
   The task ledger's own frozen endpoints. Reads and writes both come back as
   the daemon's TaskView and go through the model's defensive parse; a refusal
   is an ApiError carrying the daemon's own message (409 for a claim from a CID
   with no live agent, or a transition the state machine forbids). */

/** The `POST /api/v0/tasks` body. */
export interface CreateTask {
  goal: string;
  acceptance?: string;
  /** Whole credits. */
  budget?: number;
  role?: string;
}

/** Every task, oldest first as the daemon lists them. */
export async function listTasks(): Promise<Task[]> {
  return parseTasksView(await api<unknown>('/api/v0/tasks'));
}

export async function getTask(id: string): Promise<Task> {
  return parseTask(await api<unknown>(`/api/v0/tasks/${encodeURIComponent(id)}`));
}

export async function createTask(body: CreateTask): Promise<Task> {
  return parseTask(await api<unknown>('/api/v0/tasks', { method: 'POST', body }));
}

/** Claim an open task for the live agent connected as `agentCid`. The daemon
    refuses (409) when no running VM of this node holds that guest CID. */
export async function claimTask(id: string, agentCid: number): Promise<Task> {
  return parseTask(
    await api<unknown>(`/api/v0/tasks/${encodeURIComponent(id)}/claim`, {
      method: 'POST',
      body: { agent_cid: agentCid },
    }),
  );
}

export async function finishTask(id: string, outcome: 'done' | 'failed', note?: string): Promise<Task> {
  const body: { outcome: 'done' | 'failed'; note?: string } = { outcome };
  if (note != null && note.trim() !== '') body.note = note;
  return parseTask(
    await api<unknown>(`/api/v0/tasks/${encodeURIComponent(id)}/finish`, { method: 'POST', body }),
  );
}

/** Withdraw a task that has not ended (`open` or `claimed`). */
export async function withdrawTask(id: string): Promise<Task> {
  return parseTask(await api<unknown>(`/api/v0/tasks/${encodeURIComponent(id)}`, { method: 'DELETE' }));
}

/* ----------------------------------------------------------- the treasury
   The plaza's account book: its frozen endpoints, both reads. Both bodies go
   through the model's defensive parse — the relay is untrusted, and a missing
   field is an error naming it, never a silent zero. Every amount is whole
   credits (1 credit = $0.001; docs/treasury-draft/schema.sql). */

/** The accounts with their derived balances, the totals line, and the honest
    zero-spend note. */
export async function getTreasury(): Promise<Treasury> {
  return parseTreasury(await api<unknown>('/api/v0/treasury'));
}

/** The double-entry journal, oldest first as the daemon lists it, bounded to
    its newest entries. */
export async function getTreasuryJournal(): Promise<Journal> {
  return parseJournal(await api<unknown>('/api/v0/treasury/journal'));
}

/* ------------------------------------------------------------- the agora
   The plaza's market: the calls its roled tasks offer, the bids on them and
   the awards that bind a task to its contractor. A read; the parse is
   defensive — the relay is untrusted, and a missing field is an error naming
   it, never a silent zero. */

/** The board: every call, with its bids and its award, oldest first as the
    daemon lists them, bounded to its newest calls. */
export async function getAgora(): Promise<Calls> {
  return parseAgora(await api<unknown>('/api/v0/agora'));
}

/** Read one file a task's agent wrote, through the daemon's read-through:
    it resolves the task's assignee to its guest CID and asks the hub to drive
    that agent's own `read_file`. A POST, and deliberately so — it drives the
    agent's session (a tool call with a cost and a refusal mode), so a
    prefetcher must not replay it; only the owner's hand calls this.

    Every status is the daemon's (see crates/noded `read_artifact`): 400 a body
    or path outside the contract, 403 the hub's own policy refusal, 404 no such
    task or unserved CID, 409 no assignee or an unconnected agent, 422 the
    agent's own refusal, 502 an unreachable hub, 503 a daemon never told where
    the hub's admin surface is. The daemon, hub and agent messages all ride the
    ApiError verbatim. */
export async function readTaskArtifact(id: string, path: string): Promise<TaskArtifact> {
  return parseArtifact(
    await api<unknown>(`/api/v0/tasks/${encodeURIComponent(id)}/artifact`, {
      method: 'POST',
      body: { path },
    }),
  );
}

/** Drive one tool call on the agent connected as `cid`, through the daemon's
    `/tell` proxy (crates/noded `tell_agent`) — the operator's own hand on the
    fleet. A POST, and deliberately so: it runs a real tool in the agent's
    session (a call with a cost, a refusal mode and an effect on its turn), so a
    prefetcher must not replay it; only the owner's hand calls this.

    Every status is the daemon's (see crates/noded `tell_agent`): 400 a body or
    CID outside the contract, 403 the hub's own role policy, 404 a CID the hub
    does not serve, 409 the agent is not connected, 504 it did not answer in
    time, 502 an unreachable hub, 503 a daemon never told where the hub's admin
    surface is. The daemon's, hub's and agent's messages all ride the ApiError
    verbatim. The tool name and its arguments go to the daemon as written — the
    hub's policy is the authority, and this client keeps no allow-list. */
export async function tellAgent(
  cid: number,
  name: string,
  args: Record<string, unknown>,
  timeoutMs: number,
): Promise<AgentTell> {
  return parseTell(
    await api<unknown>(`/api/v0/agents/${cid}/tell`, {
      method: 'POST',
      body: { name, args, timeout_ms: timeoutMs },
    }),
  );
}
