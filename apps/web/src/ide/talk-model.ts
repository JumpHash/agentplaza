// The words of the IDE's talk view (views/ide-talk.ts): a chat's workspace id, the draft's tasks, budget and settings as
// lines, whether Start can be pressed, the draft's budget read from its field, and the text that streams in for a reply.
// Pure: no DOM, no fetch.

import type { ProjectSettings } from '../api';
import type { BudgetUnit, Chat, DraftBudget, DraftTask, ProjectDraft } from '../api-work';
import { roleLabel } from '../roles';
import { amountText, parseCap, type Parsed } from '../views/budget-model';

/** The IDE workspace of a chat: `chat:<id>` (`chat:main` is the orchestrator's chat). */
export function talkWorkspace(chatId: string): string {
  return `chat:${chatId}`;
}

/** The chat a workspace id opens (`chat:<id>` gives `<id>`), or null when the id is no chat's. */
export function chatOfWorkspace(id: string | null | undefined): string | null {
  return (id ? /^chat:([^:/?#]+)$/.exec(id)?.[1] : undefined) ?? null;
}

/** Whether a workspace id opens a chat. */
export function isTalkWorkspace(id: string | null | undefined): id is string {
  return chatOfWorkspace(id) !== null;
}

/** A task's `after` in words: `after 1, 2`, or '' when it runs at once. */
export function afterText(task: Pick<DraftTask, 'after'>): string {
  return task.after?.length ? `after ${task.after.join(', ')}` : '';
}

/** Why Start cannot be pressed now, or null when it can: a started or archived chat, no draft, or the draft's problems. */
export function startBlock(chat: Pick<Chat, 'archived'>, draft: Pick<ProjectDraft, 'problems' | 'started_ms'> | null): string | null {
  if (draft?.started_ms !== undefined) return 'The project is started.';
  if (chat.archived) return 'The chat is archived.';
  if (!draft) return 'There is no draft yet: the orchestrator drafts the project as you talk.';
  if (draft.problems.length) return `Before it can start: ${draft.problems.join('; ')}.`;
  return null;
}

/** The draft's budget in words: its cap in its unit, and the planner's monthly cap when it has one. */
export function budgetText(budget: DraftBudget | undefined): string {
  if (!budget) return 'From the plaza’s budget';
  const cap = amountText(budget.unit, budget.cap);
  return budget.monthly_cap === undefined ? cap : `${cap} · ${amountText(budget.unit, budget.monthly_cap)} a month`;
}

/**
 * The draft budget an edit sends, from the budget field's text in the draft's unit: the cap, with the draft's monthly cap
 * kept (the edit replaces the whole budget). A blank field sends no budget, so the draft keeps its own; a cap of zero or
 * less is refused.
 */
export function draftBudgetOf(unit: BudgetUnit, text: string, monthlyCap?: number): Parsed<DraftBudget | undefined> {
  const cap = parseCap(unit, text);
  if (!cap.ok) return cap;
  if (cap.value === null) return { ok: true, value: undefined };
  if (cap.value <= 0) return { ok: false, problem: 'The budget is more than zero.' };
  return { ok: true, value: monthlyCap === undefined ? { unit, cap: cap.value } : { unit, cap: cap.value, monthly_cap: monthlyCap } };
}

/** A draft's settings in words: `Builder model: anthropic/claude-sonnet-5-5 · effort high · permission: ask`. */
export function settingsLine(settings: ProjectSettings | undefined): string {
  if (!settings) return '';
  const parts: string[] = [];
  for (const [role, model] of Object.entries(settings.role_models ?? {})) parts.push(`${roleLabel(role)} model: ${model}`);
  const named: Record<string, string> = { default_model: 'model', effort: 'effort', permission_mode: 'permission', task_budget: 'task budget', max_turns: 'turns per task', review_timeout_minutes: 'review timeout (min)' };
  for (const [key, value] of Object.entries(settings)) {
    if (key === 'role_models' || value === undefined || value === null) continue;
    parts.push(`${named[key] ?? key.replaceAll('_', ' ')}${key === 'default_model' || key === 'permission_mode' || key === 'review_timeout_minutes' ? ':' : ''} ${String(value)}`);
  }
  return parts.join(' · ');
}

/** The text a pending reply has streamed so far, from the `delta` events of its work items (text only). */
export function streamed(deltas: { work: number; delta: { type: string; text?: string } }[], pending: number[]): string {
  return deltas
    .filter((d) => pending.includes(d.work) && d.delta.type === 'text')
    .map((d) => d.delta.text ?? '')
    .join('');
}
