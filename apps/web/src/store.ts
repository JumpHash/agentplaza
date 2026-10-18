// The live layer: one WebSocket on /v1/ws (same origin, subprotocol
// apz.events.v1) carrying typed frames to listeners, and the few pieces of
// state the shell and several views share (agents, overview).

import { api, type Agent, type Overview } from './api';

type Listener = (data: unknown) => void;
const listeners = new Map<string, Set<Listener>>();
let socket: WebSocket | null = null;
let connected = false;
const connectionListeners = new Set<(up: boolean) => void>();

// Reconnects: 1 s doubling to 30 s, each wait jittered into [half, whole] of
// the current step; the step resets once a socket has stayed open for a minute.
const BACKOFF_MIN_MS = 1_000;
const BACKOFF_MAX_MS = 30_000;
const STABLE_MS = 60_000;
const MAX_FRAME_BYTES = 1 << 20;
let wanted = false; // connect() was called and the page has not left
let backoff = BACKOFF_MIN_MS;
let retryTimer: ReturnType<typeof setTimeout> | undefined;
let stableTimer: ReturnType<typeof setTimeout> | undefined;

/** Subscribes to frames of `kind`; returns the unsubscribe function. */
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

const KINDS = new Set(['agent', 'work', 'transcript', 'delta', 'task', 'treasury', 'agora', 'board', 'conversation', 'motion', 'provider', 'run', 'vm', 'serial', 'memory', 'approval', 'library', 'connector', 'file', 'toolbox', 'project', 'loop', 'plaza', 'agent_proposal', 'role', 'usage', 'training', 'vault.changed', 'vault.audit_broken', 'mcp.changed', 'mcp.status', 'ide', 'engine', 'prompt', 'skill', 'station', 'lease_waiting', 'lease_granted', 'lease_released', 'lease_ended', 'lease_failed', 'lease_owner_input', 'station_takeover', 'station_give_back', 'station_sync_conflict', 'station_sync_paused', 'metrics', 'metrics.alert', 'board_paused', 'law', 'foundry', 'task_link', 'agent_step', 'owner_conversation', 'weft_change', 'weft_queue', 'weft_land', 'weft_release', 'weft_export', 'weft_op', 'weft_revert', 'review_run', 'review_node', 'review_replan', 'recipe', 'recipe_proposal', 'platform']);

/** Opens the live layer (idempotent). It reconnects on its own until the page leaves. */
export function connect(): void {
  wanted = true;
  if (socket || retryTimer !== undefined) return;
  open();
}

function open(): void {
  const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const ws = new WebSocket(`${scheme}//${location.host}/v1/ws`, 'apz.events.v1');
  socket = ws;
  ws.onopen = () => {
    if (socket !== ws) return;
    stableTimer = setTimeout(() => {
      stableTimer = undefined;
      backoff = BACKOFF_MIN_MS;
    }, STABLE_MS);
    setConnected(true);
  };
  ws.onmessage = (ev) => dispatch(ev.data);
  ws.onclose = () => {
    if (socket !== ws) return;
    socket = null;
    clearTimeout(stableTimer);
    stableTimer = undefined;
    setConnected(false);
    if (wanted) scheduleRetry();
  };
}

function scheduleRetry(): void {
  const wait = backoff * (0.5 + Math.random() * 0.5);
  backoff = Math.min(backoff * 2, BACKOFF_MAX_MS);
  retryTimer = setTimeout(() => {
    retryTimer = undefined;
    if (wanted && !socket) open();
  }, wait);
}

/** Routes one text frame `{"event", "data"}` to its listeners; anything else is ignored. */
function dispatch(raw: unknown): void {
  if (typeof raw !== 'string' || oversized(raw)) return;
  let frame: unknown;
  try {
    frame = JSON.parse(raw);
  } catch {
    return;
  }
  if (typeof frame !== 'object' || frame === null) return;
  const f = frame as { event?: unknown; data?: unknown };
  if (typeof f.event !== 'string' || !KINDS.has(f.event) || !('data' in f)) return;
  const data = f.data;
  listeners.get(f.event)?.forEach((fn) => fn(data));
}

/** True when the frame is over 1 MB of UTF-8. */
function oversized(text: string): boolean {
  // UTF-8 takes at least one byte per UTF-16 unit and at most three per unit,
  // so only strings in between need an exact count.
  if (text.length > MAX_FRAME_BYTES) return true;
  if (text.length * 3 <= MAX_FRAME_BYTES) return false;
  return new TextEncoder().encode(text).byteLength > MAX_FRAME_BYTES;
}

/** Page hide or unload: close cleanly and stop retrying until the page is shown again. */
function leave(): void {
  wanted = false;
  clearTimeout(retryTimer);
  retryTimer = undefined;
  clearTimeout(stableTimer);
  stableTimer = undefined;
  const ws = socket;
  socket = null;
  ws?.close(1000, 'page hidden');
  setConnected(false);
}

if (typeof window !== 'undefined') {
  window.addEventListener('pagehide', leave);
  // A page restored from the back-forward cache has lost its socket.
  window.addEventListener('pageshow', (ev) => {
    if (ev.persisted) connect();
  });
}

function setConnected(up: boolean): void {
  if (up === connected) return;
  connected = up;
  connectionListeners.forEach((fn) => fn(up));
  if (up) {
    void refreshAgents();
    void refreshOverview();
  }
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
