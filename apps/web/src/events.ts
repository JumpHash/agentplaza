/* The live data layer: an EventSource on the daemon's event bus
   (GET /api/v0/events — run/vm state transitions, serial deltas, task state
   changes and treasury posts, LF-framed, heartbeats as comments the browser
   discards).

   Semantics respected here:
   - The bus has no backlog: the stream's open IS the subscription, so the
     initial state still comes from the GETs exactly as before — every open
     (first connect and every reconnect) re-runs the view's first fetch to
     reconcile whatever the subscription missed.
   - The stream only accelerates. A stream down is NOT a daemon down: the
     polls stay the source of truth for daemon health, and a stream that
     never opens (a static host answering 404) simply leaves the polls to
     carry the register, as they always have. EventSource reconnects on its
     own; there is nothing to manage.
   - Guest serial is untrusted text. It enters the store through the same
     serial paths the polls use; every view renders it as text, never markup. */

import type { RunState, VmState } from './models/run';
import type { TaskState } from './models/task';
import { store } from './store';

const RUN_STATES: ReadonlySet<string> = new Set(['starting', 'running', 'done', 'failed', 'killed']);
const VM_STATES: ReadonlySet<string> = new Set(['running', 'exited', 'killed']);
/* A static membership table, so a hostile frame's entry kind can be refused whole. */
const ENTRY_KINDS: Readonly<Record<string, true>> = {
  mint: true,
  spend: true,
  subcontract: true,
  payment: true,
  refund: true,
  clawback: true,
  fine: true,
  fee: true,
  budget_move: true,
};
/* A static membership table, so a hostile frame's state can be refused whole. */
const TASK_STATES: Readonly<Record<string, true>> = {
  open: true,
  claimed: true,
  done: true,
  failed: true,
  withdrawn: true,
};

/** The frame's JSON, or null when it is not an object we can use — a
    malformed frame (a hostile or broken relay) is dropped, never fatal. */
function frame(e: MessageEvent): Record<string, unknown> | null {
  try {
    const data: unknown = JSON.parse(typeof e.data === 'string' ? e.data : '');
    return data !== null && typeof data === 'object' ? (data as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

/** Subscribe to the daemon's event bus and drive the store's live merges. */
export function startEvents(): void {
  let source: EventSource;
  try {
    source = new EventSource('/api/v0/events');
  } catch {
    /* no EventSource at all — the polls carry the register */
    store.setStreamState('reconnecting');
    return;
  }

  source.onopen = () => {
    store.setStreamState('live');
    /* the subscription starts now — reconcile the window before it (and the
       window a reconnect dropped) with the view's own GETs */
    store.ensurePolled();
  };
  source.onerror = () => {
    /* EventSource is reconnecting on its own (or a static host will never
       answer); say so in the colophon and let the polls carry on */
    store.setStreamState('reconnecting');
  };

  source.addEventListener('run', (e) => {
    const data = frame(e);
    if (!data || typeof data['id'] !== 'string' || !RUN_STATES.has(String(data['state']))) return;
    store.applyRunEvent(data['id'], data['state'] as RunState);
  });
  source.addEventListener('vm', (e) => {
    const data = frame(e);
    if (!data || typeof data['run'] !== 'string' || typeof data['vm'] !== 'number' || !VM_STATES.has(String(data['state']))) return;
    store.applyVmEvent(data['run'], data['vm'], data['state'] as VmState);
  });
  source.addEventListener('serial', (e) => {
    const data = frame(e);
    if (
      !data ||
      typeof data['run'] !== 'string' ||
      typeof data['vm'] !== 'number' ||
      typeof data['offset'] !== 'number' ||
      typeof data['text'] !== 'string'
    ) {
      return;
    }
    store.applySerialEvent(data['run'], data['vm'], data['offset'], data['text']);
  });
  source.addEventListener('task', (e) => {
    const data = frame(e);
    const state = data?.['state'];
    if (!data || typeof data['id'] !== 'string' || typeof state !== 'string' || TASK_STATES[state] !== true) {
      return;
    }
    /* the assignee is the schema's assignee_id, null while nobody has one; a
       hostile non-string is read as "no assignee", never as a name */
    const assignee = data['assignee'];
    store.applyTaskEvent(data['id'], state as TaskState, typeof assignee === 'string' ? assignee : null);
  });
  source.addEventListener('treasury', (e) => {
    const data = frame(e);
    const entry = data?.['entry'];
    const kind = data?.['kind'];
    if (
      !data ||
      typeof entry !== 'number' ||
      !Number.isInteger(entry) ||
      typeof kind !== 'string' ||
      ENTRY_KINDS[kind] !== true
    ) {
      return;
    }
    /* the frame is a whole post or it is dropped; the store asks the journal
       for the row's detail the frame does not carry */
    store.applyTreasuryEvent(entry);
  });
}
