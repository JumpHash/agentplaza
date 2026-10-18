// The words of the IDE's conversation view (views/ide-conversation.ts): the draft's team and tasks as lines, whether
// Start can be pressed, and the text that streams in for a reply. Pure: no DOM, no fetch.

import type { Agent, DraftSeat, DraftTask, OwnerConversation, ProjectDraft, ProjectSettings } from '../api';
import { roleLabel } from '../roles';

/** A workspace id the IDE opens as a conversation: `conv-<n>`. */
export function isConversationId(id: string | null | undefined): id is string {
  return !!id && /^conv-\d+$/.test(id);
}

/** One role of the team as the draft shows it: `2 Builder (Ada from the bench, 1 new)`. */
export function seatLine(seat: DraftSeat, roster: Pick<Agent, 'cid' | 'name'>[]): string {
  const named = (seat.bench ?? []).map((cid) => roster.find((a) => a.cid === cid)?.name || `agent ${cid}`);
  const fresh = seat.count - named.length;
  const parts: string[] = [];
  if (named.length) parts.push(`${named.join(', ')} from the bench`);
  if (fresh > 0) parts.push(`${fresh} new`);
  return `${seat.count} ${roleLabel(seat.role)}${parts.length ? ` (${parts.join(', ')})` : ''}`;
}

/** The team in one line, roles joined by a dot. */
export function teamLine(team: DraftSeat[], roster: Pick<Agent, 'cid' | 'name'>[]): string {
  return team.map((seat) => seatLine(seat, roster)).join(' · ');
}

/** A task's `after` in words: `after 1, 2`, or '' when it runs at once. */
export function afterText(task: Pick<DraftTask, 'after'>): string {
  return task.after?.length ? `after ${task.after.join(', ')}` : '';
}

/** Why Start cannot be pressed now, or null when it can. */
export function startBlock(c: Pick<OwnerConversation, 'draft' | 'project' | 'archived'>): string | null {
  if (c.project) return 'The project is started.';
  if (c.archived) return 'The conversation is archived.';
  if (!c.draft) return 'There is no draft yet: the orchestrator drafts the project as you talk.';
  if (c.draft.problems.length) return `Before it can start: ${c.draft.problems.join('; ')}.`;
  return null;
}

/** The draft's budget in words. */
export function budgetText(draft: Pick<ProjectDraft, 'budget'>): string {
  return draft.budget === undefined ? 'From the plaza’s budget' : `${draft.budget.toLocaleString('en-US')} credits`;
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
