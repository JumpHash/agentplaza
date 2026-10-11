/* The council: the four seats' motions, read the way the register reads
   everything — ruled rows, the motion's folio in the margin, the proposer's
   seat and guest CID as chips, the kind as a chip, the motion itself as prose,
   the count as a mark, and the tally as a small ruled bar of the seats beside
   its figures. Newest first, because a minute is read from the top.

   A motion is the node's own derivation — its state (0034) and its
   `needs_owner` gate are computed by the daemon from the votes and the kind,
   so the sheet READS both and never recomputes a decision. The votes hang
   beneath their motion (each seat, its agent's CID, the word as a mark, the
   reason as prose), and a gated motion carries the plain-language line of what
   its gate is for.

   The console says nothing here and writes nothing here: a `motion` frame
   carries the motion's id, its kind and its state and NO text, so the sheet is
   read from the node — never patched from a frame. Every motion's text and
   every vote's reason are prose off the wire, rendered as text, never markup. */

import { announce, byId, el } from '../dom';
import { fmtNum, fmtStamp, pad } from '../format';
import type { Motion, MotionState, Tally, Vote } from '../models/council';
import { councilTally, gateLine, KIND_LABEL, motionSeq, STATE_LABEL, tallyHolds } from '../models/council';
import { store } from '../store';

/* The table rebuilds only when the council's shape changes (a motion proposed,
   a vote landed, a seat's reason — the motions' own words), so scroll and focus
   survive a poll; the tallies, the filter line and the count update in place
   every render. */
let councilSig: string | null = null;

/** The motions' columns: No. · Proposed by · Kind · Motion · State · Tally ·
    Proposed. A motion's votes and its gate are sub-rows spanning all of them. */
const COLUMNS = 7;

/** The state as the council marks it. `open` is the register's amber hollow
    square (the seats may yet vote); `carried` and `rejected` are decision
    stamps; the two states the owner's hand is what moves are toned orange —
    `awaiting_owner` a filled square (the gate), `tie_to_owner` a divided
    square (an even split that escalated) — so the state reads by shape first
    and never by colour alone. */
function stateMark(state: MotionState): HTMLElement {
  switch (state) {
    case 'open':
      return el('span', 'mk warn', STATE_LABEL.open);
    case 'carried':
      return el('span', 'stamp sm', STATE_LABEL.carried);
    case 'rejected':
      return el('span', 'stamp sm deny', STATE_LABEL.rejected);
    case 'tie_to_owner':
      return el('span', 'mk owner tie', STATE_LABEL.tie_to_owner);
    case 'awaiting_owner':
      return el('span', 'mk owner await', STATE_LABEL.awaiting_owner);
  }
}

/** The tally as a small ruled bar: one cell per seat, in tally order — the yes
    votes, then the no, then the abstentions, then the seats that have not
    voted. Cells, not widths, so nothing scales and nothing is an inline style.
    The bar is decorative; the figures beside it carry the reading. */
function voteBar(tally: Tally): HTMLElement {
  const bar = el('span', 'votebar');
  bar.setAttribute('aria-hidden', 'true');
  const cells: ReadonlyArray<readonly [number, string, string, string]> = [
    [tally.yes, 'vy', 'y', 'yes'],
    [tally.no, 'vn', 'n', 'no'],
    [tally.abstain, 'va', 'a', 'abstain'],
  ];
  let drawn = 0;
  for (const [count, cls, letter, word] of cells) {
    for (let i = 0; i < count; i++) {
      bar.appendChild(el('i', cls, letter)).title = word;
      drawn++;
    }
  }
  /* the seats that have not voted: counted, never assumed — a tally that
     reported more votes than seats would draw none rather than underflow */
  for (let i = drawn; i < tally.seats; i++) {
    bar.appendChild(el('i', 'vc', null)).title = 'no vote yet';
  }
  return bar;
}

/** One vote as a ruled sub-row: the seat, the agent that cast it, the word as
    a mark, the reason as prose, and when. The reason is the voter's own words
    — the playbook's "never a bare vote" — so it wraps rather than forcing the
    row wide. */
function voteRow(vote: Vote): HTMLElement {
  const tr = el('tr', 'sub vote');
  const td = el('td');
  td.colSpan = COLUMNS;
  td.appendChild(el('span', 'lbl', 'vote'));
  td.appendChild(el('span', 'chip role', vote.seat));
  td.appendChild(el('span', 'chip', `cid ${fmtNum(vote.agent_cid)}`));
  const word = el('span', vote.vote === 'yes' ? 'mk ok' : vote.vote === 'no' ? 'mk crit' : 'mk', vote.vote);
  td.appendChild(word);
  td.appendChild(el('span', 'why', vote.reason));
  td.appendChild(el('span', 'm when', fmtStamp(vote.at)));
  tr.appendChild(td);
  return tr;
}

/** One motion as a ruled row, with its votes and its gate beneath it. */
function motionRow(motion: Motion): HTMLElement {
  const tr = el('tr', 'motion');
  tr.appendChild(el('td', null, pad(motionSeq(motion.id), 4)));

  const who = el('td', 'who');
  who.appendChild(el('span', 'chip role', motion.proposer_role));
  who.appendChild(el('span', 'chip', `cid ${fmtNum(motion.proposer_cid)}`));
  tr.appendChild(who);

  const kind = el('td');
  kind.appendChild(el('span', 'chip lkind', KIND_LABEL[motion.kind]));
  tr.appendChild(kind);

  tr.appendChild(el('td', 'motiontext', motion.text));

  const state = el('td');
  state.appendChild(stateMark(motion.state));
  tr.appendChild(state);

  const tally = el('td', 'tallycell');
  tally.appendChild(voteBar(motion.tally));
  const t = motion.tally;
  tally.appendChild(
    el(
      'span',
      'm tallyfig',
      `${fmtNum(t.yes)} yes · ${fmtNum(t.no)} no · ${fmtNum(t.abstain)} abstain · ${fmtNum(t.voted)} of ${fmtNum(t.seats)} seats`,
    ),
  );
  if (!tallyHolds(motion)) {
    /* the daemon's figures do not square with its own votes: shown as sent,
       and said to disagree — never silently corrected into a count the node
       did not derive */
    tally.appendChild(el('span', 'mk warn', 'the tally does not square with the votes'));
  }
  tr.appendChild(tally);

  tr.appendChild(el('td', 'm', fmtStamp(motion.created_at)));
  return tr;
}

/** The motions, newest first, or narrowed to the state the sheet is filtering
    on — the tally strip's own question. */
function renderMotions(): void {
  const all = store.motions;
  const filter = store.motionFilter;
  const motions = filter == null ? all : all.filter((m) => m.state === filter);
  const has = motions.length > 0;
  const unreadable = store.motionsError != null;

  byId('council-empty').hidden = has || unreadable;
  byId('council-table').hidden = !has;
  byId('council-count').textContent = unreadable
    ? 'last known'
    : `${fmtNum(motions.length)}${store.motionsTruncated && filter == null ? ` of ${fmtNum(store.motionsTotal)}` : ''} ` +
      (motions.length === 1 ? 'motion' : 'motions');

  if (!has) {
    if (!unreadable) {
      byId('council-empty-text').textContent =
        all.length === 0
          ? 'The council has proposed no motion. A motion is a seat’s own act — finance, legal (agents), incident or the mayor office proposes, and the four seats vote; nothing has been put to them yet.'
          : filter === 'open'
            ? 'No motion is still open — every motion the council holds has been decided.'
            : filter === 'awaiting_owner'
              ? 'No motion is carried and waiting on the owner. A carrying that is owner-gated waits here until the owner ratifies it.'
              : 'No 2–2 tie stands to escalate. An even split — two seats for, two against — is carried here to the owner.';
    }
    councilSig = null;
    byId('council-body').textContent = '';
    return;
  }

  const sig = motions
    .map(
      (m) =>
        `${m.id}:${m.state}:${m.kind}:${m.needs_owner}:${m.text.length}:` +
        m.votes.map((v) => `${v.seat}@${v.vote}@${v.reason.length}@${v.at}`).join(','),
    )
    .join('|') + `#${filter ?? ''}`;
  if (sig === councilSig) return;
  councilSig = sig;

  const body = byId('council-body');
  body.textContent = '';
  for (const motion of motions) {
    body.appendChild(motionRow(motion));
    for (const vote of motion.votes) body.appendChild(voteRow(vote));
    const gate = gateLine(motion);
    if (gate != null) {
      const tr = el('tr', 'sub gate');
      const td = el('td');
      td.colSpan = COLUMNS;
      td.appendChild(el('span', 'gatel', gate));
      tr.appendChild(td);
      body.appendChild(tr);
    }
  }
  announce(`${fmtNum(motions.length)} ${motions.length === 1 ? 'motion' : 'motions'} on the council.`);
}

export function renderCouncil(): void {
  if (store.route.name !== 'council') return;
  const live = byId('council-live');
  live.hidden = store.health !== 'live';
  live.textContent = store.health;

  const notice = byId('council-notice');
  if (store.motionsError != null) {
    notice.hidden = false;
    byId('council-notice-mk').textContent =
      store.motionsErrorStatus === 404 ? 'no council on this node' : 'council unreachable';
    byId('council-notice-text').textContent =
      store.motionsErrorStatus === 404
        ? 'This node does not serve the plaza’s council yet (the read answers 404). The register, the ledger and the treasury read from the node itself; the council arrives with the motions endpoint.'
        : `The council is not answering at ${location.origin} (${store.motionsError}). Retrying; the sheet catches up on its own.`;
  } else {
    notice.hidden = true;
  }

  const tally = councilTally(store.motions);
  const blank = store.motionsError != null && store.motions.length === 0;
  byId('t-council-total').textContent = blank ? '—' : fmtNum(tally.total);
  byId('t-council-open').textContent = blank ? '—' : fmtNum(tally.open);
  byId('t-council-awaiting').textContent = blank ? '—' : fmtNum(tally.awaiting);
  byId('t-council-ties').textContent = blank ? '—' : fmtNum(tally.ties);

  const filter = store.motionFilter;
  byId('council-filter').textContent =
    filter == null
      ? 'Every motion the council holds, newest first — the strip narrows them by state.'
      : filter === 'open'
        ? 'Narrowed to the motions still open — the seats may yet vote.'
        : filter === 'awaiting_owner'
          ? 'Narrowed to the motions carried but owner-gated — waiting on the owner’s ratification.'
          : 'Narrowed to the 2–2 ties — an even split the owner decides.';
  byId('council-clear').hidden = filter == null;
  /* the strip is the filter: the figure that is narrowing reads as pressed,
     and every figure's accessible name says what pressing it does — the
     caption below the figure is a sibling, so a screen reader would otherwise
     hear a bare number */
  const controls: ReadonlyArray<readonly [string, MotionState | null, string]> = [
    ['t-council-total', null, 'the whole council'],
    ['t-council-open', 'open', 'the motions still open'],
    ['t-council-awaiting', 'awaiting_owner', 'the motions carried but waiting on the owner'],
    ['t-council-ties', 'tie_to_owner', 'the 2–2 ties that escalated to the owner'],
  ];
  for (const [id, state, what] of controls) {
    const on = state === filter || (state == null && filter == null);
    const button = byId(id);
    button.setAttribute('aria-pressed', String(on));
    button.setAttribute(
      'aria-label',
      state == null
        ? on
          ? 'Showing every motion'
          : 'Show every motion'
        : on
          ? `Stop narrowing — show every motion (now showing ${what})`
          : `Narrow to ${what}`,
    );
  }

  renderMotions();
}

/* ------------------------------------------------------------- wiring */
/** The four figures are one control each: pressing a state's figure narrows
    the sheet to it; pressing the pressed one (or the total, or Show every
    motion) widens back to every motion. */
export function initCouncil(): void {
  const pick = (state: MotionState | null): void => {
    if (state == null || store.motionFilter === state) store.clearMotionFilter();
    else store.setMotionFilter(state);
  };
  byId('t-council-total').addEventListener('click', () => pick(null));
  byId('t-council-open').addEventListener('click', () => pick('open'));
  byId('t-council-awaiting').addEventListener('click', () => pick('awaiting_owner'));
  byId('t-council-ties').addEventListener('click', () => pick('tie_to_owner'));
  byId('council-clear').addEventListener('click', () => store.clearMotionFilter());
}
