// Wakes an asleep agent as soon as the person shows intent to talk to it (opening its chat or IDE, typing to it), so the
// wake runs before the message is sent. The hub only queues the wake and answers at once; a wake of an awake agent is a
// no-op there. Errors are swallowed: the message still wakes the agent if this does not.

import { api } from './api';

/** The least time between two wake calls for one agent. */
const MIN_GAP_MS = 15_000;

const lastCall = new Map<number, number>();

/** Queues a wake of the agent when its status is asleep or not known. At most one call per agent per 15 s. */
export function prewake(cid: number, status?: string): void {
  if (status && status !== 'asleep') return;
  const now = Date.now();
  const last = lastCall.get(cid);
  if (last !== undefined && now - last < MIN_GAP_MS) return;
  lastCall.set(cid, now);
  api.wakeAgent(cid).catch(() => undefined);
}
