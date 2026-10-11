// The live layer: one EventSource on /v1/events, typed listeners, and the
// few pieces of state the shell and several views share (agents, overview).

import { api, type Agent, type Overview } from './api';

type Listener = (data: unknown) => void;
const listeners = new Map<string, Set<Listener>>();
let source: EventSource | null = null;
let connected = false;
const connectionListeners = new Set<(up: boolean) => void>();

/** Subscribes to SSE frames of `kind`; returns the unsubscribe function. */
export function on(kind: string, fn: Listener): () => void {
  let set = listeners.get(kind);
  if (!set) listeners.set(kind, (set = new Set()));
  set.add(fn);
  return () => set.delete(fn);
}

export function onConnection(fn: (up: boolean) => void): () => void {
  connectionListeners.add(fn);
  fn(connected);
  return () => connectionListeners.delete(fn);
}

const KINDS = ['agent', 'work', 'transcript', 'delta', 'task', 'treasury', 'agora', 'board', 'conversation', 'motion', 'provider', 'run', 'vm', 'serial', 'memory'];

export function connect(): void {
  if (source) return;
  source = new EventSource('/v1/events');
  source.onopen = () => setConnected(true);
  source.onerror = () => setConnected(false);
  for (const kind of KINDS) {
    source.addEventListener(kind, (ev) => {
      let data: unknown;
      try {
        data = JSON.parse((ev as MessageEvent<string>).data);
      } catch {
        return;
      }
      listeners.get(kind)?.forEach((fn) => fn(data));
    });
  }
}

function setConnected(up: boolean): void {
  if (up === connected) return;
  connected = up;
  connectionListeners.forEach((fn) => fn(up));
  if (up) void refreshAgents();
}

// ---- Shared state ----------------------------------------------------------

/** A value plus its change listeners. */
export class Cell<T> {
  private fns = new Set<(v: T) => void>();
  constructor(private v: T) {}
  get(): T {
    return this.v;
  }
  set(v: T): void {
    this.v = v;
    this.fns.forEach((fn) => fn(v));
  }
  watch(fn: (v: T) => void): () => void {
    this.fns.add(fn);
    fn(this.v);
    return () => this.fns.delete(fn);
  }
}

export const agents = new Cell<Agent[]>([]);
export const overview = new Cell<Overview | null>(null);

let refreshing: Promise<void> | null = null;
export function refreshAgents(): Promise<void> {
  refreshing ??= api
    .agents()
    .then((list) => agents.set(list.sort((a, b) => a.cid - b.cid)))
    .catch(() => undefined)
    .finally(() => (refreshing = null));
  return refreshing;
}

export function refreshOverview(): Promise<void> {
  return api
    .overview()
    .then((o) => overview.set(o))
    .catch(() => overview.set(null));
}

/** Applies an `agent` frame (the full agent) to the roster. */
export function applyAgent(frame: Partial<Agent> & { cid: number }): Agent[] {
  const list = agents.get().slice();
  const i = list.findIndex((a) => a.cid === frame.cid);
  if (i >= 0) list[i] = { ...list[i], ...frame } as Agent;
  else list.push(frame as Agent);
  return list.sort((a, b) => a.cid - b.cid);
}

export function startShared(): void {
  connect();
  void refreshAgents();
  void refreshOverview();
  on('agent', (frame) => {
    agents.set(applyAgent(frame as Agent));
    // The roster frame lacks derived fields (model, work); refetch quietly.
    void refreshAgents();
  });
  on('work', () => void refreshAgents());
  setInterval(() => void refreshOverview(), 10_000);
  setInterval(() => void refreshAgents(), 15_000);
}
