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

export type EngineStatus = 'running' | 'ok' | 'failed' | 'denied';
export type ToolStatus = 'pending' | 'in_progress' | 'completed' | 'failed';
export type EngineContent =
  | { type: 'diff'; path: string; oldText: string; newText: string }
  | { type: 'text'; text: string }
  | { type: 'terminal'; terminalId: string };
/** One tool call a coding engine made, as ACP reports it: the latest title, status, places and content. */
export interface EngineTool {
  id: string;
  title: string;
  kind: string;
  status: ToolStatus;
  locations: { path: string; line: number | null }[];
  content: EngineContent[];
}
export interface PlanEntry { content: string; status: 'pending' | 'in_progress' | 'completed'; priority: string }
/** A check the engine asked the plaza to run for it; `approval` is set while the owner has to decide. */
export interface EngineCheck { title: string; name: string; decision: 'allow' | 'deny' | 'pending'; approval?: string }

export type ChatItem =
  | { kind: 'owner' | 'agent' | 'system'; at: number; text: string }
  | { kind: 'steps'; at: number; steps: Step[]; last: string; outcome: 'ok' | 'failed' | 'running' | 'mixed' }
  | {
      kind: 'engine';
      at: number;
      /** The code_engine call's id: the key its updates and live stream arrive under. */
      call: string;
      engine: string;
      session?: string;
      prompt: string;
      status: EngineStatus;
      /** The engine's final text, the messages of the turn joined in order. */
      message: string;
      tools: EngineTool[];
      plan: PlanEntry[];
      checks: EngineCheck[];
      summary?: string;
    }
  | { kind: 'working'; at: number };

type EngineItem = Extract<ChatItem, { kind: 'engine' }>;

/** A nested check's call id is its code_engine call's id with `.e<n>` added. */
const NESTED_CHECK = /\.e\d+$/;
/** The approval id in a held check's output: "waiting for the owner's approval (<id>)". */
const APPROVAL_ID = /approval \(([^()\s]+)\)/;
const TOOL_STATUSES: readonly ToolStatus[] = ['pending', 'in_progress', 'completed', 'failed'];
const CHECK_DECISIONS: readonly EngineCheck['decision'][] = ['allow', 'deny', 'pending'];

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
  // A nested check of an engine is not one of the plaza's own tool calls: it goes into its engine block.
  if (typeof d.call === 'string' && NESTED_CHECK.test(d.call)) return null;
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

function pick<T extends string>(v: unknown, allowed: readonly T[], fallback: T): T {
  return typeof v === 'string' && (allowed as readonly string[]).includes(v) ? (v as T) : fallback;
}

function locationsOf(v: unknown): EngineTool['locations'] {
  if (!Array.isArray(v)) return [];
  const out: EngineTool['locations'] = [];
  for (const l of v) {
    const rec = l as Record<string, unknown> | null;
    const path = str(rec?.path);
    if (path) out.push({ path, line: typeof rec?.line === 'number' ? rec.line : null });
  }
  return out;
}

function contentOf(v: unknown): EngineContent[] {
  if (!Array.isArray(v)) return [];
  const out: EngineContent[] = [];
  for (const c of v) {
    const rec = c as Record<string, unknown> | null;
    if (rec?.type === 'diff') {
      const path = str(rec.path);
      if (path) {
        out.push({
          type: 'diff',
          path,
          oldText: typeof rec.oldText === 'string' ? rec.oldText : '',
          newText: typeof rec.newText === 'string' ? rec.newText : '',
        });
      }
    } else if (rec?.type === 'content') {
      const inner = rec.content as Record<string, unknown> | null | undefined;
      if (inner?.type === 'text' && typeof inner.text === 'string') out.push({ type: 'text', text: inner.text });
    } else if (rec?.type === 'terminal') {
      const terminalId = str(rec.terminalId);
      if (terminalId) out.push({ type: 'terminal', terminalId });
    }
  }
  return out;
}

/** A tool call's ACP fields, as a row; a missing field takes its default. */
function toolOf(u: Record<string, unknown>): EngineTool {
  return {
    id: str(u.toolCallId) ?? '',
    title: str(u.title) ?? 'tool',
    kind: str(u.kind) ?? 'other',
    status: pick(u.status, TOOL_STATUSES, 'pending'),
    locations: locationsOf(u.locations),
    content: contentOf(u.content),
  };
}

/** Adds a tool call, or updates the row of the same id with the fields the update carries. */
function mergeTool(tools: EngineTool[], u: Record<string, unknown>): void {
  const id = str(u.toolCallId) ?? '';
  const cur = id ? tools.find((t) => t.id === id) : undefined;
  if (!cur) {
    tools.push(toolOf(u));
    return;
  }
  const fresh = toolOf(u);
  if (str(u.title)) cur.title = fresh.title;
  if (str(u.kind)) cur.kind = fresh.kind;
  if (u.status !== undefined) cur.status = fresh.status;
  if (Array.isArray(u.locations)) cur.locations = fresh.locations;
  if (Array.isArray(u.content)) cur.content = fresh.content;
}

function planOf(v: unknown): PlanEntry | null {
  const rec = v as Record<string, unknown> | null;
  const content = str(rec?.content);
  if (!content) return null;
  return {
    content,
    status: pick(rec?.status, ['pending', 'in_progress', 'completed'] as const, 'pending'),
    priority: str(rec?.priority) ?? 'medium',
  };
}

/** Applies one engine_update entry (an ACP update of the engine's own stream) to its engine block. */
function applyEngineUpdate(block: EngineItem, d: Record<string, unknown>): void {
  const update = d.update && typeof d.update === 'object' ? (d.update as Record<string, unknown>) : {};
  switch (d.item) {
    case 'message': {
      const text = str(d.text);
      if (text) block.message = block.message ? `${block.message}\n\n${text}` : text;
      return;
    }
    case 'tool_call':
    case 'tool_status':
      mergeTool(block.tools, update);
      return;
    case 'plan': {
      // The plan arrives as the ACP update (`{entries}`) or as its bare entries.
      const entries: unknown[] = Array.isArray(update) ? update : Array.isArray(update.entries) ? update.entries : [];
      block.plan = entries.map(planOf).filter((p): p is PlanEntry => p !== null);
      return;
    }
    default:
      return; // usage, mode and the truncation marker are not shown
  }
}

/** Fills a nested check from its request or its decision; a held decision carries the approval id. */
function applyCheck(check: EngineCheck, e: Entry): void {
  if (e.kind === 'tool_decision') {
    check.decision = pick(e.data.decision, CHECK_DECISIONS, check.decision);
    const found = APPROVAL_ID.exec(str(e.data.output) ?? '');
    if (found) check.approval = found[1];
    else delete check.approval;
  }
}

/**
 * Routes the entries of a code_engine call: the call itself opens an engine block, its updates
 * and its nested checks fill it. Returns 'none' for entries that are not engine entries (the tool
 * tracker takes those), 'handled' for ones that only fill a block, and the block a new call opened.
 */
function feedEngine(e: Entry, engines: Map<string, EngineItem>, checks: Map<string, EngineCheck>): 'none' | 'handled' | EngineItem {
  const d = e.data;
  const call = str(d.call);
  if (call === null) return 'none';
  const key = `${e.cid}:${e.work}:${call}`;
  const kind: string = e.kind;
  if (kind === 'engine_update') {
    const block = engines.get(key);
    if (block) applyEngineUpdate(block, d);
    return 'handled';
  }
  if (NESTED_CHECK.test(call)) {
    const block = engines.get(`${e.cid}:${e.work}:${call.replace(NESTED_CHECK, '')}`);
    if (e.kind === 'tool_request' && block) {
      const input = (d.input && typeof d.input === 'object' ? d.input : {}) as Record<string, unknown>;
      const check: EngineCheck = {
        title: str(input.title) ?? str(d.name) ?? 'check',
        name: str(d.name) ?? 'other',
        decision: 'pending',
      };
      checks.set(key, check);
      block.checks.push(check);
    } else {
      const check = checks.get(key);
      if (check) applyCheck(check, e);
    }
    return 'handled';
  }
  if (e.kind === 'tool_request' && str(d.name) === 'code_engine') {
    const input = (d.input && typeof d.input === 'object' ? d.input : {}) as Record<string, unknown>;
    const block: EngineItem = {
      kind: 'engine',
      at: e.at_ms,
      call,
      engine: str(input.engine) ?? 'engine',
      prompt: str(input.prompt) ?? '',
      status: 'running',
      message: '',
      tools: [],
      plan: [],
      checks: [],
    };
    const session = str(input.session);
    if (session) block.session = session;
    engines.set(key, block);
    return block;
  }
  const block = engines.get(key);
  if (!block) return 'none';
  if (e.kind === 'tool_decision' && d.decision === 'deny') {
    block.status = 'denied';
    block.summary = str(d.output) ?? block.summary;
  } else if (e.kind === 'tool_result') {
    block.status = d.ok === false || d.is_error === true ? 'failed' : 'ok';
    block.summary = str(d.output) ?? block.summary;
  }
  return 'handled';
}

/**
 * The transcript as chat items, oldest first. Owner prompts (chat or message work from the
 * owner) and model prose become single items; the tool calls between two pieces of prose
 * collapse into one 'steps' item. A code_engine call becomes one 'engine' item, with its
 * nested checks inside it. A 'working' item is appended while the agent is working.
 */
export function buildChat(entries: Entry[], working: boolean): ChatItem[] {
  const drafts: Draft[] = [];
  const tracker: Tracker = { open: [] };
  const pending = { steps: [] as Step[], at: 0 };
  const engines = new Map<string, EngineItem>();
  const checks = new Map<string, EngineCheck>();
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
    const engine = feedEngine(e, engines, checks);
    if (engine === 'handled') continue;
    if (engine !== 'none') {
      flush();
      drafts.push(engine);
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
