// Pure view model for the IDE's agent views. No DOM, no fetch, no store:
// every function takes plain data from api.ts and returns plain data.
import type { Agent, Block, Entry, Task } from '../api';

// ---- Agent rows -------------------------------------------------------------

export type RowStatus = 'working' | 'idle' | 'sleeping' | 'done' | 'failed';

/** The task states that make an idle agent's last row read as done or failed. */
const DONE_STATE = 'done';
const FAILED_STATE = 'failed';

export function rowStatus(agent: Pick<Agent, 'status'>, lastTask?: Pick<Task, 'state'> | null): RowStatus {
  if (agent.status === 'working') return 'working';
  if (agent.status === 'offline') return 'sleeping';
  if (lastTask?.state === DONE_STATE) return 'done';
  if (lastTask?.state === FAILED_STATE) return 'failed';
  return 'idle';
}

/** The first non-empty line of the agent's current prompt, trimmed and cut to 120 characters. */
export function currentTask(agent: Agent): string | null {
  const prompt = agent.work?.prompt ?? '';
  const line = prompt.split('\n').map((s) => s.trim()).find((s) => s.length > 0);
  return line ? line.slice(0, 120) : null;
}

// ---- Chat transcript --------------------------------------------------------

export interface Step {
  at: number;
  name: string;
  detail: string;
  ms: number | null;
  outcome: 'ok' | 'failed' | 'denied' | 'running';
}

export type ChatItem =
  | { kind: 'owner' | 'agent' | 'system'; at: number; text: string }
  | { kind: 'steps'; at: number; steps: Step[]; last: string; outcome: 'ok' | 'failed' | 'running' | 'mixed' }
  | { kind: 'working'; at: number };

function str(v: unknown): string | null {
  return typeof v === 'string' && v.length > 0 ? v : null;
}

function clip(s: string, n = 80): string {
  const line = (s.split('\n')[0] ?? '').trim();
  return line.length > n ? line.slice(0, n) : line;
}

/** The argument that best names what a tool call acted on: a path, a command, a query. */
function detailOf(input: unknown): string {
  if (typeof input === 'string') return clip(input);
  if (!input || typeof input !== 'object') return '';
  const rec = input as Record<string, unknown>;
  for (const key of ['path', 'file_path', 'file', 'command', 'query', 'pattern', 'url', 'title', 'name']) {
    const v = str(rec[key]);
    if (v) return clip(v);
  }
  return '';
}

export function stepLabel(step: Step): string {
  const n = step.name.toLowerCase();
  const d = step.detail;
  const verb = (word: string, bare: string): string => (d ? `${word} ${d}` : bare);
  if (n.includes('board') && n.includes('read')) return 'Read board message';
  if (n.includes('board') && (n.includes('post') || n.includes('write'))) return 'Posted to board';
  if (n === 'bash' || n === 'shell' || n.includes('command') || n.includes('exec') || n.includes('run')) return 'Ran command';
  if (/^(read|view|open)/.test(n)) return verb('Read', 'Read file');
  if (/(edit|patch|replace)/.test(n)) return verb('Edited', 'Edited file');
  if (/(write|create)/.test(n)) return verb('Wrote', 'Wrote file');
  if (/(grep|search|glob|find)/.test(n)) return verb('Searched', 'Searched files');
  return `Used ${step.name}`;
}

function toolName(d: Record<string, unknown>): string {
  return str(d.name) ?? str(d.tool) ?? 'tool';
}

function toolInput(d: Record<string, unknown>): unknown {
  return d.input ?? d.args ?? null;
}

function callId(d: Record<string, unknown>): string | null {
  const v = d.id ?? d.tool_use_id ?? d.call_id;
  return v === undefined || v === null ? null : String(v);
}

interface OpenCall { step: Step; id: string | null; key: string; reqAt: number }
interface Tracker { open: OpenCall[] }

/**
 * Feeds one entry through the tool-call tracker. A tool_request opens a step; a tool_result
 * or a denying tool_decision closes the matching open step (by id when both carry one, else
 * the oldest open step of the same agent and work). Returns the step and whether it is new
 * (fresh) or an existing step that was just closed; null for entries that are not tool calls.
 */
function track(t: Tracker, e: Entry): { step: Step; fresh: boolean } | null {
  const d = e.data;
  const key = `${e.cid}:${e.work}`;
  if (e.kind === 'tool_request') {
    const step: Step = { at: e.at_ms, name: toolName(d), detail: detailOf(toolInput(d)), ms: null, outcome: 'running' };
    t.open.push({ step, id: callId(d), key, reqAt: e.at_ms });
    return { step, fresh: true };
  }
  if (e.kind !== 'tool_result' && e.kind !== 'tool_decision') return null;
  const denied = e.kind === 'tool_decision';
  if (denied && d.decision !== 'deny') return null;
  const outcome: Step['outcome'] = denied ? 'denied' : d.is_error === true ? 'failed' : 'ok';
  const id = callId(d);
  let i = id === null ? -1 : t.open.findIndex((c) => c.key === key && c.id === id);
  if (i < 0) i = t.open.findIndex((c) => c.key === key);
  if (i < 0) {
    const step: Step = { at: e.at_ms, name: toolName(d), detail: detailOf(toolInput(d)), ms: null, outcome };
    return { step, fresh: true };
  }
  const call = t.open.splice(i, 1)[0];
  if (!call) return null;
  call.step.outcome = outcome;
  call.step.ms = Math.max(0, e.at_ms - call.reqAt);
  return { step: call.step, fresh: false };
}

function fromOwner(d: Record<string, unknown>): boolean {
  const origin = d.origin as { from_cid?: unknown } | null | undefined;
  return (d.from_cid ?? origin?.from_cid ?? null) === null;
}

function textOf(content: unknown): string {
  if (!Array.isArray(content)) return '';
  const parts: string[] = [];
  for (const b of content as Block[]) {
    if (b && b.type === 'text' && b.text.trim()) parts.push(b.text.trim());
  }
  return parts.join('\n\n');
}

function groupOutcome(steps: Step[]): 'ok' | 'failed' | 'running' | 'mixed' {
  if (steps.some((s) => s.outcome === 'running')) return 'running';
  const bad = steps.filter((s) => s.outcome === 'failed' || s.outcome === 'denied').length;
  if (bad === 0) return 'ok';
  return bad === steps.length ? 'failed' : 'mixed';
}

type Draft = Exclude<ChatItem, { kind: 'steps' }> | { kind: 'steps'; at: number; steps: Step[] };

/**
 * The transcript as chat items, oldest first. Owner prompts (chat or message work from the
 * owner) and model prose become single items; the tool calls between two pieces of prose
 * collapse into one 'steps' item. A 'working' item is appended while the agent is working.
 */
export function buildChat(entries: Entry[], working: boolean): ChatItem[] {
  const drafts: Draft[] = [];
  const tracker: Tracker = { open: [] };
  const pending = { steps: [] as Step[], at: 0 };
  const flush = (): void => {
    if (pending.steps.length > 0) drafts.push({ kind: 'steps', at: pending.at, steps: pending.steps });
    pending.steps = [];
  };
  for (const e of entries) {
    const d = e.data;
    if (e.kind === 'work_start') {
      const kind = d.kind;
      const text = str(d.prompt);
      if ((kind === 'chat' || kind === 'message') && fromOwner(d) && text) {
        flush();
        drafts.push({ kind: 'owner', at: e.at_ms, text });
      }
      continue;
    }
    if (e.kind === 'model_response') {
      const text = textOf(d.content);
      if (text) {
        flush();
        drafts.push({ kind: 'agent', at: e.at_ms, text });
      }
      continue;
    }
    const t = track(tracker, e);
    if (t && t.fresh) {
      if (pending.steps.length === 0) pending.at = e.at_ms;
      pending.steps.push(t.step);
    }
  }
  flush();
  const items: ChatItem[] = drafts.map((x): ChatItem => {
    if (x.kind !== 'steps') return x;
    const lastStep = x.steps[x.steps.length - 1];
    return {
      kind: 'steps',
      at: x.at,
      steps: x.steps,
      last: lastStep ? stepLabel(lastStep) : '',
      outcome: groupOutcome(x.steps),
    };
  });
  if (working) {
    const lastEntry = entries[entries.length - 1];
    items.push({ kind: 'working', at: lastEntry ? lastEntry.at_ms : 0 });
  }
  return items;
}

// ---- Activity ---------------------------------------------------------------

export interface Activity {
  totalMs: number;
  thinkingMs: number;
  toolMs: number;
  calls: { name: string; ms: number | null; outcome: Step['outcome']; at: number }[];
}

/** Time split of a transcript: thinking (model request to response), tool time, and the whole span. */
export function activitySplit(entries: Entry[]): Activity {
  const tracker: Tracker = { open: [] };
  const thinkingStart = new Map<string, number>();
  let thinkingMs = 0;
  const steps: Step[] = [];
  for (const e of entries) {
    const key = `${e.cid}:${e.work}`;
    if (e.kind === 'model_request') {
      thinkingStart.set(key, e.at_ms);
    } else if (e.kind === 'model_response') {
      const start = thinkingStart.get(key);
      if (start !== undefined) {
        thinkingMs += Math.max(0, e.at_ms - start);
        thinkingStart.delete(key);
      }
    }
    const t = track(tracker, e);
    if (t && t.fresh) steps.push(t.step);
  }
  const toolMs = steps.reduce((sum, s) => sum + (s.ms ?? 0), 0);
  const first = entries[0];
  const last = entries[entries.length - 1];
  const totalMs = entries.length < 2 || !first || !last ? 0 : Math.max(0, last.at_ms - first.at_ms);
  return {
    totalMs,
    thinkingMs,
    toolMs,
    calls: steps.map((s) => ({ name: s.name, ms: s.ms, outcome: s.outcome, at: s.at })),
  };
}

// ---- Command palette --------------------------------------------------------

export interface PaletteItem { id: string; group: 'agent' | 'file' | 'task' | 'command'; label: string; hint?: string }

/**
 * Fuzzy score of query against label, case-insensitive; higher is better, null is no match.
 * Empty query scores 0. A contiguous substring scores 100 to 1000, earlier starts higher.
 * A subsequence match scores 1 to 99, tighter spans higher.
 */
export function matchScore(label: string, query: string): number | null {
  const q = query.trim().toLowerCase();
  if (q === '') return 0;
  const l = label.toLowerCase();
  const at = l.indexOf(q);
  if (at >= 0) return 1000 - Math.min(at, 900);
  let qi = 0;
  let first = -1;
  let last = -1;
  for (let i = 0; i < l.length && qi < q.length; i++) {
    if (l[i] === q[qi]) {
      if (first < 0) first = i;
      last = i;
      qi++;
    }
  }
  if (qi < q.length) return null;
  const span = last - first + 1;
  return Math.max(1, 99 - (span - q.length));
}

/** Empty query: the first `limit` items in order. Otherwise matches by score, ties in input order. */
export function filterPalette(items: PaletteItem[], query: string, limit = 60): PaletteItem[] {
  if (query.trim() === '') return items.slice(0, limit);
  const scored: { item: PaletteItem; score: number }[] = [];
  for (const item of items) {
    const score = matchScore(item.label, query);
    if (score !== null) scored.push({ item, score });
  }
  scored.sort((a, b) => b.score - a.score);
  return scored.slice(0, limit).map((s) => s.item);
}

// ---- Formatting and mentions ------------------------------------------------

/** "340 ms", "1.2 s", "2 min 5 s". */
export function formatMs(ms: number): string {
  const v = Math.max(0, ms);
  if (v < 1000) return `${Math.round(v)} ms`;
  if (v < 60000) return `${(v / 1000).toFixed(1)} s`;
  const total = Math.round(v / 1000);
  return `${Math.floor(total / 60)} min ${total % 60} s`;
}

/** Each @name in the text (letters, digits, dot, dash, underscore; @all included), in order, no duplicates. */
export function mentionsOf(text: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  const re = /(?<![A-Za-z0-9_])@([A-Za-z0-9._-]+)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    const name = (m[1] ?? '').replace(/\.+$/, '');
    if (name && !seen.has(name)) {
      seen.add(name);
      out.push(name);
    }
  }
  return out;
}

/** A stable hue slot 0..5 for an author name, from a string hash. */
export function authorHue(name: string): number {
  let h = 0;
  for (const ch of name) h = (h * 31 + (ch.codePointAt(0) ?? 0)) >>> 0;
  return h % 6;
}
