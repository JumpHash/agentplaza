/* The council: the four seats' motions, read the way the register reads
   everything — ruled rows, the motion's folio in the margin, the proposer's
   seat and guest CID as chips, the kind as a chip, the motion itself as prose,
   the count as a mark, and the tally as a small ruled bar of the seats beside
   its figures. Newest first, because a minute is read from the top.

   A motion is the node's own derivation — its state and its `needs_owner` gate
   are computed by the daemon from the votes and the kind — so the sheet READS
   both and never recomputes a decision. The votes hang beneath their motion
   (each seat, its agent's CID, the word as a mark, the reason as prose), and a
   gated motion carries the plain-language line of what its gate is for.

   **The owner's hand** sits on the two states the count gates to them — the
   2–2 tie (`tie_to_owner`) and a carry above the council's authority
   (`awaiting_owner`). For those, and only those, the sheet offers a decide
   control in the operator's own idiom: the affordance arms ('Decide this
   motion…'), a confirm line states what the decision means (it resolves the
   tie, or ratifies or refuses what the council cannot, and the council cannot
   reopen it), an optional note is taken (≤ the daemon's byte bound, counted),
   and TWO presses commit — 'Ratify <id>' and 'Veto <id>', each naming the
   motion, never a bare OK. It is never a single click; leaving the sheet or the
   motion leaving the gated states disarms it. Once the owner has decided the
   control is gone and the sheet shows the owner's own line instead (the veto's
   'no' bolded, the note as prose, the stamp). For a motion the council decided
   itself — carried below the gate, or rejected — there is no control at all:
   the council's own decision stands, and a one-line note says so.

   Every motion's text, every vote's reason and the owner's note are prose off
   the wire: rendered as text, never markup. A `motion` frame carries the
   motion's id, its kind and its state and NO text, so the sheet is read from
   the node — never patched from a frame. */

import { announce, byId, el } from '../dom';
import { fmtNum, fmtStamp, pad } from '../format';
import type { DecisionState, Motion, MotionState, Tally, Vote } from '../models/council';
import {
  councilTally,
  gateLine,
  KIND_LABEL,
  MAX_DECISION_NOTE,
  motionSeq,
  ownerReading,
  parseDecisionNote,
  STATE_LABEL,
  tallyHolds,
} from '../models/council';
import { utf8ByteLength } from '../serial';
import { store } from '../store';

/* The table rebuilds only when the council's shape changes (a motion proposed,
   a vote landed, a seat's reason — the motions' own words), so scroll and focus
   survive a poll; the tallies, the filter line and the count update in place
   every render. The armed decide is part of the shape: arming and disarming
   rebuild the sheet, while the note being typed does not (the box keeps its
   text, and the count updates in place). */
let councilSig: string | null = null;

/** The motion whose decide control is armed — the owner has pressed 'Decide
    this motion…' and not yet committed or left — or null. View state: one
    armed decide at a time, and leaving the sheet (or the motion leaving the
    gated states) disarms it. */
let decideArmedId: string | null = null;

/** The last decision announced to a screen reader, so a re-render does not
    repeat it. */
let decideAnnounced = '';

/** The motions' columns: No. · Proposed by · Kind · Motion · State · Tally ·
    Proposed. A motion's votes, its gate, its decide control and the owner's
    line are sub-rows spanning all of them. */
const COLUMNS = 7;

/** The state as the council marks it. `open` is the register's amber hollow
    square (the seats may yet vote); `carried` and `rejected` are the council's
    decision stamps; `ratified` and `vetoed` are the OWNER's decision stamps,
    the veto a denial — and neither wears the owner's orange, because orange in
    the register is only ever 'needs the owner' and a decided motion no longer
    does. The two states the owner's hand is what moves are toned orange —
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
    case 'ratified':
      return el('span', 'stamp sm', STATE_LABEL.ratified);
    case 'vetoed':
      return el('span', 'stamp sm deny', STATE_LABEL.vetoed);
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

/** One motion as a ruled row. */
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

/* ------------------------------------------------------------- the owner's hand
   The decide control, in the operator's own idiom (the roster's stop is its
   sibling): arm, a confirm line that states what the decision means, an
   optional note, and two presses that commit — 'Ratify <id>' and 'Veto <id>',
   each naming the motion. The result, when the node refuses, is the daemon's
   own words, kept verbatim. */

/** The mark each decision wears, in the register's own language. */
const DECISION_MARK: Readonly<Record<DecisionState, { cls: string; label: string }>> = {
  deciding: { cls: 'mk warn', label: 'deciding' },
  taken: { cls: 'mk ok', label: 'the owner decided' },
  refused: { cls: 'mk crit', label: 'refused' },
  unknown: { cls: 'mk crit', label: 'no such motion' },
  unreachable: { cls: 'mk warn', label: 'unreachable' },
};

/** A decision's own line: the motion, then how it ended. */
function decisionReading(id: string, state: DecisionState, approve: boolean, status: number | null): string {
  if (state === 'deciding') return `${id} · deciding …`;
  if (state === 'taken') return `${id} · the owner ${approve ? 'ratified' : 'vetoed'} it`;
  if (state === 'unknown') return `${id} · the node holds no such motion`;
  return status == null ? `${id} · the node did not answer` : `${id} · HTTP ${status}`;
}

/** The confirm line: what the owner's decision MEANS. A tie is resolved; a
    motion above the council's authority is ratified or refused; either way the
    council cannot reopen it. */
function decideConfirmLine(motion: Motion): string {
  return motion.state === 'tie_to_owner'
    ? `Deciding ${motion.id}. The owner’s decision resolves this 2–2 tie — ratify to let the motion stand, ` +
        `veto to refuse it — and the council cannot reopen it.`
    : `Deciding ${motion.id}. The owner’s decision ratifies or refuses a motion above the council’s ` +
        `authority — the council cannot reopen it.`;
}

/** The result box: the node's own answer or refusal, kept as it came. Shown
    only for a decision that was NOT taken (a taken one is the motion's own
    owner line now); the verbatim refusal is the daemon talking. */
function decideResultBox(id: string): HTMLElement | null {
  const d = store.lastDecision;
  if (d == null || d.id !== id || d.state === 'deciding') return null;
  const box = el('div', 'ldartbody');
  const head = el('div', 'ldarthead');
  head.appendChild(el('span', 'lbl', 'the owner’s decision'));
  const mk = DECISION_MARK[d.state];
  const mark = el('span', mk.cls, mk.label);
  head.appendChild(mark);
  head.appendChild(el('span', 'c', decisionReading(d.id, d.state, d.approve, d.status)));
  box.appendChild(head);
  const err = el('p', 'err', d.detail);
  err.setAttribute('role', 'alert');
  box.appendChild(err);
  return box;
}

/** One motion's decide sub-row: armed (the confirm) or armed-off (the press and
    any refusal the node already gave). The presses name the motion; the note's
    count is the daemon's byte bound. */
function decideRow(motion: Motion): HTMLElement {
  const tr = el('tr', 'sub decide');
  const td = el('td');
  td.colSpan = COLUMNS;
  const block = el('div', 'decideblock');

  const pending = store.lastDecision?.id === motion.id && store.lastDecision.state === 'deciding';

  if (decideArmedId === motion.id) {
    const confirm = el('div', 'stopconfirm');
    confirm.appendChild(el('p', 'confirmline', decideConfirmLine(motion)));

    const field = el('div', 'f');
    const label = el(
      'label',
      null,
      `Note · optional, ≤${MAX_DECISION_NOTE} bytes, recorded with the decision`,
    );
    label.htmlFor = 'decide-note';
    field.appendChild(label);
    const input = el('input');
    input.id = 'decide-note';
    input.type = 'text';
    input.autocomplete = 'off';
    input.spellcheck = false;
    input.placeholder = 'no note — the decision stands alone';
    field.appendChild(input);
    confirm.appendChild(field);

    const count = el('p', 'm notecount');
    count.id = 'decide-count';
    count.textContent = `0 of ${MAX_DECISION_NOTE}`;
    confirm.appendChild(count);

    const actions = el('div', 'actions');
    const ratify = el('button', 'btn owner', `Ratify ${motion.id}`);
    ratify.type = 'button';
    ratify.disabled = pending;
    ratify.dataset.decideGo = motion.id;
    ratify.dataset.approve = '1';
    ratify.setAttribute(
      'aria-label',
      `Ratify ${motion.id} — the owner approves this motion; the council cannot reopen it`,
    );
    const veto = el('button', 'btn danger', `Veto ${motion.id}`);
    veto.type = 'button';
    veto.disabled = pending;
    veto.dataset.decideGo = motion.id;
    veto.dataset.approve = '0';
    veto.setAttribute(
      'aria-label',
      `Veto ${motion.id} — the owner refuses this motion; the council cannot reopen it`,
    );
    const cancel = el('button', 'btn sm', 'Cancel — leave it to stand');
    cancel.type = 'button';
    cancel.dataset.decideCancel = motion.id;
    cancel.setAttribute('aria-label', `Cancel — leave ${motion.id} as it stands for now`);
    const err = el('span', 'err');
    err.id = 'decide-err';
    err.setAttribute('role', 'alert');
    actions.append(ratify, veto, cancel, err);
    confirm.appendChild(actions);
    block.appendChild(confirm);
  } else {
    const arm = el('button', 'btn owner sm', 'Decide this motion…');
    arm.type = 'button';
    arm.dataset.decideArm = motion.id;
    arm.disabled = pending; // the owner decides once: no second press while the first is in flight
    arm.setAttribute('aria-label', `Decide ${motion.id} — arm the owner’s hand: ratify or veto`);
    block.appendChild(arm);
    block.appendChild(el('span', 'm', `No. ${pad(motionSeq(motion.id), 4)} · the owner decides this one`));
    const result = decideResultBox(motion.id);
    if (result != null) block.appendChild(result);
  }

  td.appendChild(block);
  tr.appendChild(td);
  return tr;
}

/** The owner's line for a motion they decided: the decision stamp (the word the
    owner's hand said, the veto's 'no' bolded), the sentence with the stamp, and
    the note as prose. */
function ownerRow(motion: Motion): HTMLElement {
  const tr = el('tr', 'sub ownerline');
  const td = el('td');
  td.colSpan = COLUMNS;
  td.appendChild(el('span', 'lbl', 'decision'));
  const reading = ownerReading(motion);
  if (reading != null) {
    const stamp = el('span', reading.approve ? 'stamp sm' : 'stamp sm deny');
    stamp.appendChild(document.createTextNode('the owner’s '));
    stamp.appendChild(el('b', null, reading.word));
    stamp.setAttribute(
      'aria-label',
      reading.approve ? 'the owner ratified this motion' : 'the owner vetoed this motion',
    );
    td.appendChild(stamp);
    const line = el('span', 'why');
    line.appendChild(document.createTextNode(`the owner ${reading.verb} this on ${fmtStamp(reading.at)}`));
    if (reading.note != null) line.appendChild(document.createTextNode(` — ${reading.note}`));
    td.appendChild(line);
  }
  tr.appendChild(td);
  return tr;
}

/** The one-line note for a motion the council decided itself: why the sheet
    offers the owner no control here. */
function councilStandRow(motion: Motion): HTMLElement {
  const tr = el('tr', 'sub stand');
  const td = el('td');
  td.colSpan = COLUMNS;
  const why =
    motion.state === 'carried'
      ? 'carried by the council — the owner decides only a 2–2 tie or a motion above the council’s authority.'
      : 'rejected by the council — the owner decides only a 2–2 tie or a motion above the council’s authority.';
  td.appendChild(el('span', 'whynot', why));
  tr.appendChild(td);
  return tr;
}

/** The motions, newest first, or narrowed to the state the sheet is filtering
    on — the tally strip's own question. */
function renderMotions(): void {
  const all = store.motions;
  const filter = store.motionFilter;

  /* a motion that left the gated states (the owner decided it, or the node
     moved it on) is no longer the owner's to decide: disarm it here, before the
     sheet is built, so an armed control never outlives its motion */
  if (decideArmedId != null) {
    const armed = all.find((m) => m.id === decideArmedId);
    if (
      armed == null ||
      armed.owner != null ||
      (armed.state !== 'tie_to_owner' && armed.state !== 'awaiting_owner')
    ) {
      decideArmedId = null;
    }
  }

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
          ? 'The council has proposed no motion.'
          : filter === 'open'
            ? 'No motion is still open — every motion the council holds has been decided.'
            : filter === 'awaiting_owner'
              ? 'No motion is carried and waiting on the owner.'
              : 'No 2–2 tie stands to escalate.';
    }
    councilSig = null;
    byId('council-body').textContent = '';
    return;
  }

  const sig =
    motions
      .map(
        (m) =>
          `${m.id}:${m.state}:${m.kind}:${m.needs_owner}:${m.owner == null ? '-' : m.owner.approve ? 'y' : 'n'}:${m.text.length}:` +
          m.votes.map((v) => `${v.seat}@${v.vote}@${v.reason.length}@${v.at}`).join(','),
      )
      .join('|') +
    `#${filter ?? ''}` +
    `#d:${decideArmedId ?? ''}:${store.lastDecision?.id ?? ''}:${store.lastDecision?.state ?? ''}`;
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
    /* the owner's hand: a decide control only for a motion the count gated to
       them and the owner has not decided; the owner's own line once they have;
       a note saying why not for a motion the council decided itself */
    if (motion.owner != null) {
      body.appendChild(ownerRow(motion));
    } else if (motion.state === 'tie_to_owner' || motion.state === 'awaiting_owner') {
      body.appendChild(decideRow(motion));
    } else if (motion.state === 'carried' || motion.state === 'rejected') {
      body.appendChild(councilStandRow(motion));
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
        ? 'This node does not serve the plaza’s council yet (the read answers 404).'
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
      ? 'Every motion the council holds, newest first.'
      : filter === 'open'
        ? 'Narrowed to the motions still open.'
        : filter === 'awaiting_owner'
          ? 'Narrowed to the motions carried but waiting on the owner.'
          : 'Narrowed to the 2–2 ties.';
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
  announceDecision();
}

/* ------------------------------------------------------------- the owner's hand
   wiring */

/** Announce a settled decision once, for a screen reader — the verdict the
    motion's own row now carries as its owner line (or the node's verbatim
    refusal). */
function announceDecision(): void {
  const d = store.lastDecision;
  if (d == null || d.state === 'deciding') return;
  const key = `${d.id}\u0000${d.at}\u0000${d.state}`;
  if (key === decideAnnounced) return;
  decideAnnounced = key;
  announce(
    d.state === 'taken'
      ? `The owner ${d.approve ? 'ratified' : 'vetoed'} ${d.id}.`
      : `The decision on ${d.id} was ${DECISION_MARK[d.state].label}: ${d.detail}`,
  );
}

/** Update the live note count from the box's own bytes — the daemon's bound,
    so a note the node would refuse is caught at the field, not round-tripped. */
function updateDecideCount(): void {
  const box = document.getElementById('decide-note') as HTMLInputElement | null;
  const out = document.getElementById('decide-count');
  if (box == null || out == null) return;
  const bytes = utf8ByteLength(box.value);
  out.textContent = `${fmtNum(bytes)} of ${MAX_DECISION_NOTE}`;
  out.classList.toggle('over', bytes > MAX_DECISION_NOTE);
}

/** Arm the decide control: reveal the confirm and move focus into the note
    box, so a keyboard owner is never left on a press that did nothing. */
function armDecide(id: string): void {
  decideArmedId = id;
  store.scheduleRender();
  queueMicrotask(() => (document.getElementById('decide-note') as HTMLInputElement | null)?.focus());
}

/** Disarm the decide: leave the motion as it stands. */
function disarmDecide(id: string | null): void {
  decideArmedId = null;
  store.scheduleRender();
  if (id != null) {
    queueMicrotask(() => {
      const arm = document.querySelector<HTMLElement>(`[data-decide-arm="${id}"]`);
      arm?.focus();
    });
  }
}

/** Commit the armed decision: the note is read from the box (named at the field
    when it is past the daemon's byte bound), then posted. The armed state is
    spent either way — a taken decision is the motion's own line now, and a
    refusal is the node's, shown beneath the press. */
function commitDecide(id: string, approve: boolean): void {
  if (decideArmedId !== id) return; // a press belongs to the motion it names
  const box = document.getElementById('decide-note') as HTMLInputElement | null;
  const parsed = parseDecisionNote(box?.value ?? '');
  if ('error' in parsed) {
    const err = document.getElementById('decide-err');
    if (err != null) err.textContent = parsed.error;
    box?.focus();
    return;
  }
  decideArmedId = null;
  void store.decideMotion(id, approve, parsed.note);
}

/** Leaving the council sheet disarms the decide: the owner's press is for the
    motion they were looking at, and the next visit starts from the arm. */
export function disarmCouncil(): void {
  if (decideArmedId != null) decideArmedId = null;
}

/* ------------------------------------------------------------- wiring */
/** The four figures are one control each: pressing a state's figure narrows
    the sheet to it; pressing the pressed one (or the total, or Show every
    motion) widens back to every motion. The decide control is delegated to the
    table body, which survives the row rebuilds. */
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

  const body = byId('council-body');
  body.addEventListener('click', (ev) => {
    const target = ev.target instanceof Element ? ev.target : null;
    const arm = target?.closest<HTMLElement>('[data-decide-arm]');
    if (arm?.dataset.decideArm != null) {
      armDecide(arm.dataset.decideArm);
      return;
    }
    const go = target?.closest<HTMLElement>('[data-decide-go]');
    if (go?.dataset.decideGo != null) {
      commitDecide(go.dataset.decideGo, go.dataset.approve === '1');
      return;
    }
    const cancel = target?.closest<HTMLElement>('[data-decide-cancel]');
    if (cancel?.dataset.decideCancel != null) {
      disarmDecide(cancel.dataset.decideCancel);
    }
  });
  body.addEventListener('input', () => updateDecideCount());
  body.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape' && decideArmedId != null) {
      ev.preventDefault();
      disarmDecide(decideArmedId);
    }
  });
}
