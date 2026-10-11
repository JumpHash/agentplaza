/* The talk: the plaza's conversations, read the way the register reads
   everything — ruled rows, the exchange's folio in the margin, its pair worn
   as two role chips with their guest CIDs, the words as wrapping prose, and
   the hub's own delivery answer beside them (delivered, or the reason it was
   refused, verbatim). Newest first, because a log is read from the top.

   A pair may be picked to narrow the sheet to that pair's own thread: the
   node's thread API (GET /api/v0/conversations/{a}/{b}) is asked for it,
   rather than sifting the bounded copy. Every pair the node keeps stands in
   the filter list with an active mark — the node's own window (ACTIVE_SECS) —
   that links to the floor, where the two are drawn talking.

   The console says nothing here, and it invents nothing: the `conversation`
   SSE frame carries no text, only the pair that spoke, so the log is read
   from the node — never patched from a frame. Every exchange's words and every
   refusal are untrusted prose off the wire, rendered as text, never markup. */

import { byId, el } from '../dom';
import { fmtNum, fmtStamp, pad } from '../format';
import type { Conversation } from '../models/social';
import { conversationPairs, deliveryMark, pairKey, socialSeq, talkTally } from '../models/social';
import type { ThreadRead } from '../store';
import { store } from '../store';

/* The pair list and the log rebuild only when their shape changes (a new
   exchange, a moved cursor, a pair's roles), so scroll and focus survive a
   poll; the tallies and the filter line update in place every render. */
let pairsSig: string | null = null;
let logSig: string | null = null;

export function renderTalk(): void {
  if (store.route.name !== 'talk') return;
  const live = byId('talk-live');
  live.hidden = store.health !== 'live';
  live.textContent = store.health;

  const notice = byId('talk-notice');
  if (store.conversationsError != null) {
    notice.hidden = false;
    byId('talk-notice-mk').textContent =
      store.conversationsErrorStatus === 404 ? 'no social layer' : 'conversations unreachable';
    byId('talk-notice-text').textContent =
      store.conversationsErrorStatus === 404
        ? 'This node does not serve the plaza’s conversations yet (the read answers 404).'
        : `The conversations are not answering at ${location.origin} (${store.conversationsError}). Retrying; the sheet catches up on its own.`;
  } else {
    notice.hidden = true;
  }

  const now = Date.now();
  const tally = talkTally(
    { pairs: store.convPairs, conversations: store.conversations, active_secs: store.convActiveSecs },
    now,
  );
  const blank = store.conversationsError != null && store.conversations.length === 0;
  byId('t-talk-exchanges').textContent = blank ? '—' : fmtNum(tally.exchanges);
  byId('t-talk-pairs').textContent = blank ? '—' : fmtNum(tally.pairs);
  byId('t-talk-active').textContent = blank ? '—' : fmtNum(tally.active);
  byId('t-talk-refused').textContent = blank ? '—' : fmtNum(tally.refused);
  byId('talk-window').textContent = `A pair is active while it exchanged in the last ${fmtNum(store.convActiveSecs)} s — the node’s window.`;
  byId('talk-count').textContent =
    store.conversationsError != null
      ? 'last known'
      : `${fmtNum(store.conversations.length)}${store.convTruncated ? ` of ${fmtNum(store.convTotal)}` : ''} ${store.conversations.length === 1 ? 'exchange' : 'exchanges'}`;

  renderFilter();
  renderPairs(now);
  renderLog();
}

/** The filter line: which pair the sheet is narrowed to, and the way back to
    the whole log. A view preference, so it is stated rather than guessed. */
function renderFilter(): void {
  const filter = store.talkPair;
  const label = byId('talk-filter');
  label.textContent =
    filter == null
      ? 'The whole log, newest first.'
      : `Narrowed to cid ${filter.a} ↔ cid ${filter.b} — the node’s own thread for the pair.`;
  byId('talk-clear').hidden = filter == null;
}

/** One chip for a guest CID, framed as every other CID in the register. */
function cidChip(cid: number): HTMLElement {
  return el('span', 'chip', `cid ${cid}`);
}

/** The roles the node records per CID: the exchanges’ own roles first (the
    roles the pair spoke in), the hub’s current roles filling the gaps. A CID
    with no role anywhere reads as its number, never an invented name. */
function roleMap(): Map<number, string> {
  const roles = new Map<number, string>();
  for (const c of store.conversations) {
    if (c.from_role != null && !roles.has(c.from_cid)) roles.set(c.from_cid, c.from_role);
    if (c.to_role != null && !roles.has(c.to_cid)) roles.set(c.to_cid, c.to_role);
  }
  if (store.hubError == null && store.hub != null) {
    for (const agent of store.hub.agents) if (!roles.has(agent.cid)) roles.set(agent.cid, agent.role);
  }
  return roles;
}

/** The pair list — the filter. One row per pair the node keeps, ascending: the
    two CIDs as a press that narrows the sheet, their roles as chips, how many
    exchanges the pair holds and when it last spoke, and an active mark (the
    node’s own window) that links to the floor. */
function renderPairs(now: number): void {
  const pairs = conversationPairs(
    { pairs: store.convPairs, conversations: store.conversations, active_secs: store.convActiveSecs },
    now,
  );
  const list = byId('talk-pairs-list');
  const empty = byId('talk-pairs-empty');
  const has = pairs.length > 0;
  empty.hidden = has || store.conversationsError != null;
  byId('talk-pairs-count').textContent = has ? `${fmtNum(pairs.length)} ${pairs.length === 1 ? 'pair' : 'pairs'}` : '';
  if (!has) {
    if (store.conversationsError == null) {
      byId('talk-pairs-empty-text').textContent = 'No pair has spoken yet.';
    }
    pairsSig = null;
    list.textContent = '';
    return;
  }

  const roles = roleMap();
  const filter = store.talkPair;
  const sig = pairs
    .map((p) => `${p.a}-${p.b}/${p.count}/${p.last_at}/${p.active ? 'a' : 'q'}`)
    .join('|') + `#${filter == null ? '' : pairKey(filter.a, filter.b)}`;
  if (sig === pairsSig) return;
  pairsSig = sig;
  list.textContent = '';
  for (const pair of pairs) {
    const li = el('li', 'pair');
    const narrowed = filter != null && filter.a === pair.a && filter.b === pair.b;

    const pick = el('button', 'btn sm pairpick');
    pick.type = 'button';
    pick.setAttribute('data-pair', `${pair.a}-${pair.b}`);
    pick.setAttribute('aria-pressed', String(narrowed));
    pick.textContent = `cid ${pair.a} ↔ cid ${pair.b}`;
    /* the visible press names the two CIDs; its accessible name names the
       agents too (role + CID), so the filter reads to a screen reader the way
       the chips read to the eye */
    const named = (cid: number): string => {
      const role = roles.get(cid);
      return role != null ? `${role} cid ${cid}` : `cid ${cid}`;
    };
    pick.setAttribute(
      'aria-label',
      narrowed
        ? `Show the whole log (not just ${named(pair.a)} and ${named(pair.b)})`
        : `Narrow the log to ${named(pair.a)} and ${named(pair.b)}`,
    );
    li.appendChild(pick);

    for (const cid of [pair.a, pair.b]) {
      const role = roles.get(cid);
      if (role != null) li.appendChild(el('span', 'chip role', role));
    }
    li.appendChild(
      el('span', 'm', `${fmtNum(pair.count)} ${pair.count === 1 ? 'exchange' : 'exchanges'} · last ${fmtStamp(pair.last_at)}`),
    );

    if (pair.active) {
      const toFloor = document.createElement('a');
      toFloor.href = '#floor';
      toFloor.className = 'mk ok pairfloor';
      toFloor.textContent = 'talking now — the floor';
      li.appendChild(toFloor);
    } else {
      li.appendChild(el('span', 'mk pairquiet', 'quiet'));
    }
    list.appendChild(li);
  }
}

/** The exchange log: the whole copy, or the picked pair's own thread when the
    sheet is narrowed. The thread is the node's answer, so it is read from
    `store.thread` — loading, refused and read are all stated honestly. */
function renderLog(): void {
  const filter = store.talkPair;
  const thread = filter == null ? null : threadFor(filter);

  const table = byId('talk-table');
  const note = byId('talk-note');
  const noteWrap = byId('talk-note-wrap');

  let rows: readonly Conversation[] = [];
  let state: 'log' | 'loading' | 'error' | 'read' = 'log';
  if (filter == null) {
    rows = store.conversations;
  } else if (thread == null || thread.state === 'loading') {
    state = 'loading';
  } else if (thread.state === 'error') {
    state = 'error';
  } else {
    state = 'read';
    rows = thread.exchanges;
  }

  if (state === 'loading') {
    noteWrap.hidden = false;
    note.className = 'example';
    note.textContent = `Reading the node’s thread for cid ${filter?.a} ↔ cid ${filter?.b}…`;
  } else if (state === 'error') {
    noteWrap.hidden = false;
    note.className = 'err';
    note.textContent = thread?.error ?? 'the thread could not be read';
  } else if (state === 'read') {
    noteWrap.hidden = false;
    note.className = 'example';
    note.textContent =
      thread != null && thread.truncated
        ? `${fmtNum(rows.length)} of ${fmtNum(thread.total)} exchanges, newest first — the node bounds the thread.`
        : `${fmtNum(rows.length)} ${rows.length === 1 ? 'exchange' : 'exchanges'}, newest first.`;
  } else {
    noteWrap.hidden = true;
  }

  const has = rows.length > 0;
  table.hidden = !has;
  const showEmpty = !has && state !== 'loading' && state !== 'error' && !(filter == null && store.conversationsError != null);
  byId('talk-empty').hidden = !showEmpty;
  if (!has) {
    if (showEmpty) {
      byId('talk-empty-text').textContent =
        filter == null
          ? 'The plaza has recorded no exchange.'
          : 'This pair has exchanged nothing the node holds.';
    }
    logSig = null;
    byId('talk-body').textContent = '';
    return;
  }

  const sig = rows.map((c) => `${c.id}/${c.delivered}/${c.delivery ?? ''}/${c.text.length}`).join('|');
  if (sig === logSig) return;
  logSig = sig;

  const roles = roleMap();
  const body = byId('talk-body');
  body.textContent = '';
  for (const c of rows) {
    const lo = Math.min(c.from_cid, c.to_cid);
    const hi = Math.max(c.from_cid, c.to_cid);
    const tr = el('tr', 'link');
    tr.tabIndex = 0;
    tr.setAttribute('data-pair', `${lo}-${hi}`);
    tr.setAttribute(
      'aria-label',
      `${c.id}: cid ${c.from_cid} to cid ${c.to_cid}. Narrow the log to this pair`,
    );
    tr.appendChild(el('td', null, pad(socialSeq(c.id), 4)));

    const who = el('td', 'who');
    who.appendChild(cidChip(c.from_cid));
    const fromRole = c.from_role ?? roles.get(c.from_cid);
    if (fromRole != null) who.appendChild(el('span', 'chip role', fromRole));
    who.appendChild(el('span', 'arrow', '→'));
    who.appendChild(cidChip(c.to_cid));
    const toRole = c.to_role ?? roles.get(c.to_cid);
    if (toRole != null) who.appendChild(el('span', 'chip role', toRole));
    tr.appendChild(who);

    tr.appendChild(el('td', 'says', c.text));

    const mark = deliveryMark(c);
    const delivery = el('td', 'del');
    if (mark.kind === 'delivered') delivery.appendChild(el('span', 'mk ok', mark.text));
    else if (mark.kind === 'refused') delivery.appendChild(el('span', 'mk warn', mark.text));
    else delivery.appendChild(el('span', 'm', '—'));
    tr.appendChild(delivery);

    tr.appendChild(el('td', 'm', fmtStamp(c.created_at)));
    body.appendChild(tr);
  }
}

/** The thread the store holds, when it is the pair the sheet is narrowed to.
    The node echoes the pair as asked, so the comparison folds both ways. */
function threadFor(pair: { a: number; b: number }): ThreadRead | null {
  const thread = store.thread;
  if (thread == null) return null;
  return pairKey(thread.a, thread.b) === pairKey(pair.a, pair.b) ? thread : null;
}

/* ------------------------------------------------------------- wiring */
/** Pick a pair (or, picking the one already shown, widen back to the log). */
function pickPair(a: number, b: number, narrowed: boolean): void {
  if (narrowed) store.clearTalkPair();
  else store.setTalkPair(a, b);
}

function pairOf(node: HTMLElement | null): { a: number; b: number } | null {
  const m = /^(\d+)-(\d+)$/.exec(node?.getAttribute('data-pair') ?? '');
  if (!m) return null;
  return { a: Number(m[1]), b: Number(m[2]) };
}

export function initTalk(): void {
  byId('talk-pairs-list').addEventListener('click', (e) => {
    const node = (e.target as HTMLElement | null)?.closest<HTMLElement>('[data-pair]') ?? null;
    const pair = pairOf(node);
    if (!pair) return;
    pickPair(pair.a, pair.b, node?.getAttribute('aria-pressed') === 'true');
  });
  byId('talk-body').addEventListener('click', (e) => {
    const pair = pairOf((e.target as HTMLElement | null)?.closest<HTMLElement>('tr[data-pair]') ?? null);
    if (pair) store.setTalkPair(pair.a, pair.b);
  });
  byId('talk-body').addEventListener('keydown', (e) => {
    if (e.key !== 'Enter') return;
    const pair = pairOf((e.target as HTMLElement | null)?.closest<HTMLElement>('tr[data-pair]') ?? null);
    if (!pair) return;
    e.preventDefault();
    store.setTalkPair(pair.a, pair.b);
  });
  byId('talk-clear').addEventListener('click', () => store.clearTalkPair());
}
