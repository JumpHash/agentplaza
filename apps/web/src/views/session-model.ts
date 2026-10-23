// The sessions screens' pure logic: a session's state (its label and tone, whether it is over), the list's rows (the
// scope, the live filter, the order), its seat and cost text, a route's reference (a VM's CID or a session id), the
// transcript's lines (a label, a text and a note for each kind) and a context's fill. No DOM, and no runtime imports
// from views/, store.ts or api.ts (type-only imports are fine), so node can test it (test/sessions-tests.ts).

import type { ContextTool, PausedBy, Session, SessionContext, SessionState, SessionUsage, TranscriptEntry } from '../api-work';
import { roleLabel } from '../roles';
import { tokensText } from '../ui/chat-tools';
import { clip, type Tone } from '../ui/components';

/** The project the main orchestrator's session works for: the whole plaza, not a project of its own. */
export const PLAZA_PROJECT = 'plaza';

/** Micro-dollars in a dollar, as the wire counts them (api-work.ts's MICROS_PER_DOLLAR). */
const MICROS_PER_DOLLAR = 1_000_000;

/** The longest tool output a log row shows; the rest is clipped. */
const OUTPUT_CHARS = 400;

const STATE_LABEL: Record<SessionState, string> = {
  queued: 'Queued',
  placing: 'Starting',
  forking: 'Starting',
  running: 'Running',
  paused: 'Paused',
  compacting: 'Compacting',
  handing_over: 'Handing over',
  ending: 'Ending',
  ended: 'Ended',
  failed: 'Failed',
};

const STATE_TONE: Record<SessionState, Tone> = {
  queued: 'muted',
  placing: 'info',
  forking: 'info',
  running: 'info',
  paused: 'warn',
  compacting: 'info',
  handing_over: 'info',
  ending: 'info',
  ended: 'muted',
  failed: 'danger',
};

const PAUSED_TEXT: Record<PausedBy, string> = {
  budget: 'Budget reached',
  allowance: 'Allowance reached',
  disk: 'Disk full',
  card: 'Waiting for you',
  node: 'Node unavailable',
};

export function stateLabel(state: SessionState): string {
  return STATE_LABEL[state];
}

export function stateTone(state: SessionState): Tone {
  return STATE_TONE[state];
}

/** Whether a session is over: ended or failed, and nothing more happens to it. */
export function isFinal(state: SessionState): boolean {
  return state === 'ended' || state === 'failed';
}

/** Whether a session is live: not over yet. */
export function isLive(session: Pick<Session, 'state'>): boolean {
  return !isFinal(session.state);
}

/** Whether a session works for `project` (null: every project). */
export function inScope(session: Pick<Session, 'project'>, project: string | null): boolean {
  return project === null || session.project === project;
}

/**
 * The sessions a list shows: those in scope, the live ones only when asked, live first and the newest first within each
 * group. A new array; `sessions` is left as it is.
 */
export function sessionRows(sessions: readonly Session[], opts: { project: string | null; liveOnly: boolean }): Session[] {
  return sessions
    .filter((s) => inScope(s, opts.project) && (!opts.liveOnly || isLive(s)))
    .sort((a, b) => Number(isFinal(a.state)) - Number(isFinal(b.state)) || b.created_ms - a.created_ms);
}

/** The seat a session fills, as a list shows it: `long-lived` for a long session, `helper` for a helper, else the seat. */
export function seatText(session: Pick<Session, 'kind' | 'seat'>): string {
  if (session.kind === 'long') return 'long-lived';
  if (session.kind === 'helper') return 'helper';
  return session.seat ?? '—';
}

/** A session's heading: its role and its seat, as `Builder · builder` or `Orchestrator · long-lived`. */
export function headingText(session: Pick<Session, 'role' | 'kind' | 'seat'>): string {
  return `${roleLabel(session.role)} · ${seatText(session)}`;
}

/** Why a session waits, in a few words; null when no reason is set. */
export function pausedText(by: PausedBy | null): string | null {
  return by ? PAUSED_TEXT[by] : null;
}

/** Micro-dollars as dollars with two decimals: 1500000 is "$1.50". A count that is not a number reads $0.00. */
export function costText(micros: number): string {
  const dollars = Number.isFinite(micros) && micros > 0 ? micros / MICROS_PER_DOLLAR : 0;
  return `$${dollars.toFixed(2)}`;
}

/** A session's tokens in and out, as the list shows them: "41.2k in · 5.3k out". */
export function usageText(usage: SessionUsage): string {
  return `${tokensText(usage.tokens_in)} in · ${tokensText(usage.tokens_out)} out`;
}

/** A count with its noun: "1 turn", "3 turns". */
export function count(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** Whether a route's reference is a VM's CID (digits, as an agent's old link `#/sessions/105` gave) rather than a session id. */
export function isCidRef(ref: string): boolean {
  return /^\d+$/.test(ref);
}

/** The newest session on VM `cid`, or null when none runs there. */
export function newestWithCid(sessions: readonly Session[], cid: number): Session | null {
  let newest: Session | null = null;
  for (const s of sessions) {
    if (s.cid === cid && (newest === null || s.created_ms > newest.created_ms)) newest = s;
  }
  return newest;
}

/** The share of the window the last call filled, from 0 to 1; null when the window is unknown. */
export function contextFill(ctx: Pick<SessionContext, 'used' | 'window' | 'fill'>): number | null {
  let share: number | null = null;
  if (typeof ctx.fill === 'number' && Number.isFinite(ctx.fill)) share = ctx.fill;
  else if (ctx.window && ctx.window > 0) share = ctx.used / ctx.window;
  return share === null ? null : Math.min(1, Math.max(0, share));
}

/**
 * Whether a session has made a model call yet. The hub puts the VM's CID in every context once the session has a VM, so
 * a CID alone does not say so; the call count does.
 */
export function hasCall(ctx: Pick<SessionContext, 'calls'>): boolean {
  return (ctx.calls ?? 0) > 0;
}

/** The tools of a context, the ones that sent the most result tokens first. A new array. */
export function toolsByResult(tools: readonly ContextTool[]): ContextTool[] {
  return [...tools].sort((a, b) => b.result_tokens - a.result_tokens || a.tool.localeCompare(b.tool));
}

/** A time of day on the reader's clock, `14:03:07`. */
export function clockText(ms: number): string {
  const d = new Date(ms);
  return [d.getHours(), d.getMinutes(), d.getSeconds()].map((n) => String(n).padStart(2, '0')).join(':');
}

// ---- The transcript ---------------------------------------------------------------------------------------------------

/**
 * One row of a session's log: what it is (`label`), what it says (`text`, in mono when it is code or a tool's output),
 * a fact beside the label (`note`), whether it is an alert, and whether this console knows its kind (`known`: a kind it
 * does not draw keeps its own name and no text).
 */
interface TranscriptLine {
  label: string;
  text: string;
  mono: boolean;
  note: string;
  alert: boolean;
  known: boolean;
}

const VERDICT: Record<string, string> = { allow: 'Allowed', deny: 'Denied', done: 'Done', pending: 'Pending' };
const OUTCOME: Record<string, string> = { done: 'Done', failed: 'Failed', stopped: 'Stopped' };

const textOf = (v: unknown): string => (typeof v === 'string' ? v : '');
const num = (v: unknown): number => (typeof v === 'number' && Number.isFinite(v) ? v : 0);

/** A value as compact JSON, or '' when there is none. */
function jsonText(v: unknown): string {
  return v === undefined ? '' : (JSON.stringify(v) ?? '');
}

/** The text of one content part, when it is a text part. */
function partText(part: unknown): string | null {
  if (typeof part !== 'object' || part === null) return null;
  const p = part as { type?: unknown; text?: unknown };
  return p.type === 'text' && typeof p.text === 'string' ? p.text : null;
}

/** A model response's words: its text parts joined by line breaks (a plain string is itself). Tool calls are no words. */
export function modelText(content: unknown): string {
  if (typeof content === 'string') return content;
  if (!Array.isArray(content)) return '';
  return content
    .map(partText)
    .filter((t): t is string => t !== null)
    .join('\n');
}

/** A tool's output as the log shows it: its text, or its JSON, clipped to 400 characters. */
export function resultText(output: unknown): string {
  return clip(typeof output === 'string' ? output : jsonText(output), OUTPUT_CHARS);
}

/** A model call's tokens as a note: "3 in · 1 out". Empty when the entry carries no usage. */
function tokensNote(usage: unknown): string {
  if (typeof usage !== 'object' || usage === null) return '';
  const u = usage as { input_tokens?: unknown; output_tokens?: unknown };
  return `${tokensText(num(u.input_tokens))} in · ${tokensText(num(u.output_tokens))} out`;
}

/** A decision with what it produced: "Allowed", or "Denied: the output". */
function decisionText(decision: unknown, output: unknown): string {
  const word = VERDICT[textOf(decision)] ?? textOf(decision);
  const out = resultText(output);
  return out ? `${word}: ${out}` : word;
}

/**
 * A transcript entry as the log draws it. The kinds this console draws get a label and a text (the prompt, the model's
 * words, a tool's call and its output, a decision, an error, the end of the work, a compression); any other kind keeps
 * its own name and no text, and the expert view shows its data.
 */
export function transcriptLine(entry: Pick<TranscriptEntry, 'kind' | 'data'>): TranscriptLine {
  const d = entry.data;
  const line = (label: string, part: Partial<TranscriptLine> = {}): TranscriptLine => ({
    label,
    text: '',
    mono: false,
    note: '',
    alert: false,
    known: true,
    ...part,
  });
  switch (entry.kind) {
    case 'work_start':
      return line('Started', { text: textOf(d.prompt) });
    case 'model_response':
      return line('Model', { text: modelText(d.content), note: tokensNote(d.usage) });
    case 'tool_request':
      return line('Tool call', { text: [textOf(d.name), jsonText(d.input)].filter(Boolean).join(' '), mono: true });
    case 'tool_result': {
      const ok = d.ok === true;
      return line(ok ? 'Tool result' : 'Tool failed', { text: resultText(d.output), mono: true, alert: !ok });
    }
    case 'tool_decision':
      return line('Decision', { text: decisionText(d.decision, d.output) });
    case 'error':
      return line('Error', { text: textOf(d.message), alert: true });
    case 'work_end': {
      const status = textOf(d.status);
      return line('Work ended', { text: textOf(d.summary), note: OUTCOME[status] ?? status, alert: status === 'failed' });
    }
    case 'compressed':
      return line('Compressed', {
        text: `${tokensText(num(d.before))} → ${tokensText(num(d.after))} tokens, ${count(num(d.folded), 'turn', 'turns')} folded`,
      });
    default:
      return { label: entry.kind, text: '', mono: false, note: '', alert: false, known: false };
  }
}
