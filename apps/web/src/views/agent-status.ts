// An agent's status as a pill: the dot in the status's tone, and a word for the states the dot alone does not tell apart
// (asleep and offline are both muted, so asleep also reads its word). The project's team rows show it.

import type { AgentStatus } from '../api';
import type { Tone } from '../ui/components';

const STATUS_TONE: Record<AgentStatus, Tone> = { working: 'info', idle: 'ok', offline: 'muted', asleep: 'muted', dozing: 'muted' };

/** The status pill: the dot in the status's tone, and the word for a napping status (null for the others). */
export function statusPill(status: AgentStatus): { tone: Tone; word: string | null } {
  return { tone: STATUS_TONE[status], word: sleepWord(status) };
}

/** The word for a napping status (asleep, or dozing as "asleep (light)"); null for the others. */
function sleepWord(status: AgentStatus): string | null {
  if (status === 'asleep') return 'Asleep';
  if (status === 'dozing') return 'Asleep (light)';
  return null;
}
