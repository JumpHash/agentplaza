/* The JSON client for the frozen /api/v0 contract. Errors are values the
   store keeps and shows — never thrown away, never swallowed. */

import type { HubStatus } from './models/hub';
import { parseHubStatus } from './models/hub';
import type { Machine } from './models/machine';
import type { CreateRun, Run, RunsView } from './models/run';
import type { Task } from './models/task';
import { parseTask, parseTasksView } from './models/task';

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
