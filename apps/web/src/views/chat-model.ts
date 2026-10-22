// The inbox's and the chats' logic as plain functions and one small class: the labels of cards, chats and authors, the
// amount an answer takes, the amounts kept across redraws, the text a settled card shows, the order and the tail of a
// chat's messages, and the lines of the expert view. No DOM, no timers and no runtime import of the API (type-only
// imports are fine), so node can test it (test/inbox-chats-tests.ts).

import type { BudgetUnit, Card, ChatKind, ChatMessage } from '../api-work';
import { parseDollars, parseTokens, type Parsed } from './budget-model';

/** Sentence case: the words of `text` with underscores as spaces, the first letter capital. */
function sentenceCase(text: string): string {
  const plain = text.replace(/_/g, ' ').trim().toLowerCase();
  return plain.charAt(0).toUpperCase() + plain.slice(1);
}

/** A card kind's label: `Budget`, `Tool call`, `Start a project`; a kind not listed reads in sentence case. */
export function cardKindLabel(kind: string): string {
  return kind === 'start' ? 'Start a project' : sentenceCase(kind);
}

/** The choices that stop, refuse or fail something: their buttons are danger. */
const DANGER_CHOICES = new Set(['stop', 'deny', 'reject', 'fail']);

export function isDangerChoice(id: string): boolean {
  return DANGER_CHOICES.has(id);
}

/**
 * The integer an answer's amount is sent as, in the unit of its card's owner's budget: micro-dollars for a `usd` budget
 * or none (the dollars typed, as the budget form reads them), whole tokens for a `tokens` budget. A card whose budget did
 * not read has no unit (`undefined`), so its amount is refused. The amount must be more than zero; a refusal says why.
 */
export function amountWire(unit: BudgetUnit | null | undefined, text: string): Parsed<number> {
  if (unit === undefined) return { ok: false, problem: 'The budget could not be read, so the unit of the amount is not known.' };
  const amount = unit === 'tokens' ? parseTokens(text) : parseDollars(text);
  if (!amount.ok) return amount;
  return amount.value > 0 ? amount : { ok: false, problem: 'The amount is more than zero.' };
}

/** The unit an amount is typed in, as a card names it: `tokens`, `dollars` (no budget, or a usd one), or unknown. */
export function amountUnitName(unit: BudgetUnit | null | undefined): string {
  if (unit === undefined) return 'unit not known';
  return unit === 'tokens' ? 'tokens' : 'dollars';
}

/** The amounts typed into cards, by card id. A view keeps them across its redraws until the card is answered. */
export class Drafts {
  private texts = new Map<string, string>();

  get(id: string): string {
    return this.texts.get(id) ?? '';
  }

  /** Keeps `text` for the card; an empty text keeps nothing. */
  set(id: string, text: string): void {
    if (text === '') this.texts.delete(id);
    else this.texts.set(id, text);
  }
}

/**
 * What a card that is no longer open reads as: `Answered: <the chosen label>`, or `Withdrawn`. An open card has no such
 * text; its buttons are its answer.
 */
export function settledText(c: Pick<Card, 'state' | 'answer' | 'choices'>): string {
  if (c.state === 'withdrawn') return 'Withdrawn';
  if (c.state === 'open') return '';
  const chosen = c.choices.find((x) => x.id === c.answer);
  return chosen ? `Answered: ${chosen.label}` : 'Answered';
}

/** Who a message is from: `You` for the owner, the seat's name for a session (`Session` without one), `Plaza` for the hub. */
export function authorLabel(m: Pick<ChatMessage, 'author' | 'seat'>): string {
  if (m.author === 'owner') return 'You';
  if (m.author === 'system') return 'Plaza';
  return m.seat ? sentenceCase(m.seat) : 'Session';
}

/** A chat's kind, as the list names it: the orchestrator's, a project's planner, or a task's. */
export function chatKindLabel(kind: ChatKind): string {
  if (kind === 'main') return 'Orchestrator';
  return kind === 'task' ? 'Task' : 'Project';
}

/** The last `limit` messages, oldest first. The list is copied, so the caller's order is kept. */
export function recentMessages(messages: readonly ChatMessage[], limit: number): ChatMessage[] {
  const ordered = [...messages].sort((a, b) => a.seq - b.seq);
  return ordered.slice(Math.max(0, ordered.length - limit));
}

/** A message's time by the reader's clock, `HH:MM`. */
export function clockText(ms: number): string {
  const at = new Date(ms);
  return `${String(at.getHours()).padStart(2, '0')}:${String(at.getMinutes()).padStart(2, '0')}`;
}

/** A card's dense line for the expert view: its id, raw kind, subject and the raw ids of its choices. */
export function cardExpertLine(c: Pick<Card, 'id' | 'kind' | 'subject' | 'choices'>): string {
  return `${c.id} · ${c.kind} · ${c.subject} · choices ${c.choices.map((x) => x.id).join(', ')}`;
}

/** A message's dense line for the expert view: its number, raw kind and author, its session and its card. */
export function messageExpertLine(m: Pick<ChatMessage, 'seq' | 'kind' | 'author' | 'session' | 'card'>): string {
  return [`#${m.seq}`, m.kind, m.author, m.session, m.card ? `card ${m.card}` : null].filter(Boolean).join(' · ');
}

/** The trimmed text, or null when nothing is left of it. */
export function nonBlank(text: string): string | null {
  const trimmed = text.trim();
  return trimmed === '' ? null : trimmed;
}

/** The toast an answer shows: the `done` text the hub wrote, or `Answered` when nothing else follows. */
export function doneText(done: string): string {
  return nonBlank(done) ?? 'Answered';
}

/** Whether an error is the hub's refusal of an answer to a card that is no longer open (409). */
export function isStaleAnswer(err: unknown): boolean {
  return typeof err === 'object' && err !== null && 'status' in err && err.status === 409;
}

/** The project a new chat opens with: the scope's, when it is one of the live projects; otherwise the first of them. */
export function preselectProject(liveIds: readonly string[], scopeId: string | null): string | null {
  return scopeId !== null && liveIds.includes(scopeId) ? scopeId : (liveIds[0] ?? null);
}

/** The chat a `chat` frame is about (`{chat, opened}` or `{chat, message}`), or null for any other data. */
export function frameChat(data: unknown): string | null {
  if (typeof data !== 'object' || data === null || !('chat' in data)) return null;
  return typeof data.chat === 'string' ? data.chat : null;
}
