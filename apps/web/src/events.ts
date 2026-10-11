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
/* The two things an agora frame can say a call did: a bid landed, or the call
   was awarded. A frame that names neither is dropped whole. */
const AGORA_KINDS: Readonly<Record<string, true>> = { bid: true, award: true };
/* A static membership table for the board's own frames: a post written, or a
   read cursor advanced. A hostile kind is refused whole. */
const BOARD_KINDS: Readonly<Record<string, true>> = { post: true, read: true };
/* A static membership table for the council's own frames: the kinds and the
   states a motion can report. A hostile word is refused whole. */
const MOTION_KINDS: Readonly<Record<string, true>> = {
  priority: true,
  budget_move: true,
  town_proposal: true,
  mayor_replacement: true,
  law_proposal: true,
  other: true,
};
const MOTION_STATES: Readonly<Record<string, true>> = {
  open: true,
  carried: true,
  rejected: true,
  tie_to_owner: true,
  awaiting_owner: true,
  ratified: true,
  vetoed: true,
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
  source.addEventListener('agora', (e) => {
    const data = frame(e);
    const kind = data?.['kind'];
    if (
      !data ||
      typeof data['task'] !== 'string' ||
      typeof kind !== 'string' ||
      AGORA_KINDS[kind] !== true ||
      typeof data['agent_cid'] !== 'number' ||
      !Number.isInteger(data['agent_cid']) ||
      typeof data['price'] !== 'number' ||
      !Number.isInteger(data['price'])
    ) {
      return;
    }
    /* the frame names the call, the kind, the agent and the price — but no
       row: not the bids list, not the award's contractor or escrow entry. The
       store asks the board once (coalesced) and invents no bid or award. */
    store.applyAgoraEvent();
  });
  source.addEventListener('board', (e) => {
    const data = frame(e);
    const kind = data?.['kind'];
    const agent = data?.['agent_cid'];
    if (
      !data ||
      typeof kind !== 'string' ||
      BOARD_KINDS[kind] !== true ||
      typeof agent !== 'number' ||
      !Number.isInteger(agent)
    ) {
      return;
    }
    /* a post written, or an agent's read cursor advanced. The frame carries
       one post (or one cursor) but not the board's totals, its bounding line
       or the other agents' unread counts: the store asks the board once and
       invents neither a post nor a read. */
    store.applyBoardEvent();
  });
  source.addEventListener('conversation', (e) => {
    const data = frame(e);
    const from = data?.['from_cid'];
    const to = data?.['to_cid'];
    if (
      !data ||
      typeof from !== 'number' ||
      !Number.isInteger(from) ||
      typeof to !== 'number' ||
      !Number.isInteger(to)
    ) {
      return;
    }
    /* one exchange. Whether the pair is now active is the node's own answer on
       the conversations GET, so the store asks for that rather than deciding
       it here from a single frame. */
    store.applyConversationEvent(from, to);
  });
  source.addEventListener('motion', (e) => {
    const data = frame(e);
    const kind = data?.['kind'];
    const state = data?.['state'];
    if (
      !data ||
      typeof data['id'] !== 'string' ||
      typeof kind !== 'string' ||
      MOTION_KINDS[kind] !== true ||
      typeof state !== 'string' ||
      MOTION_STATES[state] !== true
    ) {
      return;
    }
    /* a motion proposed, or a seat's vote landed. The frame names the motion,
       its kind and its state — and carries NO text: the motion's text and its
       votes' reasons are read from the motions endpoint, the only source for
       them. The store asks the council once (coalesced) and patches nothing. */
    store.applyMotionEvent();
  });
}
