/* The market: the agora's board. One ruled row per call — its folio in the
   margin, its task id a link into the ledger entry, its capability as a role
   chip, its ceiling in credits and the dollars they stand for, its state as a
   mark (`offered` while the offer stands, `awarded` once an award closed it),
   and when it opened — newest first, the way the register reads. Beneath each
   call stands its own block: every bid it holds (the bidder's role, the
   agent's guest CID, the price, when it was made, the winning bid marked once
   the call is awarded), and the award when there is one — its contractor, the
   price it was made at, and the treasury entry that funded the call's escrow.

   Nothing here is written from a frame: the board is the node's own derivation
   of the ledger, and the `agora` events only ever ask the board to be re-read,
   so a hostile frame can never invent a bid or an award. */

import { byId, el, announce } from '../dom';
import { fmtMoney, fmtNum, fmtStamp, pad } from '../format';
import type { Award, Bid, Call, CallState } from '../models/agora';
import { winningBid } from '../models/agora';
import { seqOf } from '../models/task';
import { store } from '../store';

/* The table rebuilds only when the board's shape changes, so scroll and focus
   survive a render that carries no new bid. */
let boardSig: string | null = null;

/** The board's columns: No. · Call · Capability · Ceiling · State · Entered.
    A call's bids and award are sub-rows spanning all of them. */
const COLUMNS = 6;

/** The state as the market marks it: `offered` while the offer stands, a stamp
    for a call an award closed, and the ledger's own marks for a call that
    carries the task's later state. */
function stateMark(state: CallState): HTMLElement {
  switch (state) {
    case 'open':
      return el('span', 'mk warn', 'offered');
    case 'awarded':
      return el('span', 'stamp sm', 'awarded');
    case 'claimed':
      return el('span', 'mk ok', 'claimed');
    case 'done':
      return el('span', 'stamp sm', 'accepted');
    case 'failed':
      return el('span', 'stamp sm deny', 'failed');
    case 'withdrawn':
      return el('span', 'stamp sm deny', 'cancelled');
  }
}

/** One bid as a ruled sub-row: the bidder's role, the agent's guest CID, the
    price, when it was made, and — when the call is awarded and this is the bid
    the award names — the mark that it won. */
function bidRow(bid: Bid, won: boolean): HTMLElement {
  const tr = el('tr', 'sub bid');
  const td = el('td');
  td.colSpan = COLUMNS;
  td.appendChild(el('span', 'lbl', 'bid'));
  td.appendChild(el('span', 'chip role', bid.bidder));
  td.appendChild(el('span', 'chip', `cid ${fmtNum(bid.agent_cid)}`));
  td.appendChild(el('span', 'num price', fmtMoney(bid.price)));
  td.appendChild(el('span', 'm when', fmtStamp(bid.created_at)));
  if (won) td.appendChild(el('span', 'stamp sm won', 'won'));
  tr.appendChild(td);
  return tr;
}

/** The award as a ruled sub-row: its contractor, the price it was made at, and
    the treasury entry that funded the call's escrow. The escrow entry links to
    the treasury — the entry the book lists by its own number — and its meaning
    is spelled out, since the treasury view takes no entry in its address and
    cannot be sent to a single row. */
function awardRow(award: Award): HTMLElement {
  const tr = el('tr', 'sub award');
  const td = el('td');
  td.colSpan = COLUMNS;
  td.appendChild(el('span', 'lbl', 'award'));
  td.appendChild(el('span', 'm', award.contractor));
  td.appendChild(el('span', 'm dot', '·'));
  td.appendChild(el('span', 'm', fmtMoney(award.price)));
  td.appendChild(el('span', 'm dot', '·'));
  if (award.escrow_entry == null) {
    /* a task with no budget funds no escrow, so the award names no entry */
    td.appendChild(el('span', 'm', 'no escrow entry — the call had no funded budget'));
  } else {
    td.appendChild(el('span', 'm', 'escrow entry'));
    const link = el('a', 'm', fmtNum(award.escrow_entry));
    link.href = '#treasury';
    link.title = 'the treasury entry that minted this call’s budget into its escrow';
    td.appendChild(link);
    td.appendChild(
      el('span', 'm note', '— the treasury entry that minted this call’s budget; read it by its number in the book'),
    );
  }
  tr.appendChild(td);
  return tr;
}

/** One call as a ruled row, with its bids and its award beneath it. */
function callRow(call: Call): HTMLElement {
  const tr = el('tr', 'call');
  tr.appendChild(el('td', null, pad(seqOf(call.task), 4)));
  const idTd = el('td', 'callid');
  const link = el('a', 'm', call.task);
  link.href = `#ledger/${encodeURIComponent(call.task)}`;
  idTd.appendChild(link);
  tr.appendChild(idTd);
  const capTd = el('td');
  capTd.appendChild(el('span', 'chip role', call.capability));
  tr.appendChild(capTd);
  tr.appendChild(el('td', 'num', fmtMoney(call.max_price)));
  const stateTd = el('td');
  stateTd.appendChild(stateMark(call.state));
  tr.appendChild(stateTd);
  tr.appendChild(el('td', 'm', fmtStamp(call.created_at)));
  return tr;
}

/** The board, newest first. */
function renderBoard(): void {
  const calls = store.calls;
  const has = calls.length > 0;
  byId('market-empty').hidden = has || store.callsError != null;
  byId('market-table').hidden = !has;
  byId('market-count').textContent = has
    ? store.callsTruncated
      ? `newest ${fmtNum(calls.length)} of ${fmtNum(store.callsTotal)}`
      : fmtNum(calls.length) + (calls.length === 1 ? ' call' : ' calls')
    : store.callsError != null
      ? '—'
      : 'no call stands';
  if (!has) {
    byId('market-empty-text').textContent =
      store.callsError != null
        ? `The board is not answering (${store.callsError}). Retrying; the market catches up on its own.`
        : 'no call stands right now — a roled task whose offer is still open, or one an agent has bid on, ' +
          'appears here; enter a task for a role on the ledger and its call is offered.';
    boardSig = null;
    byId('market-body').textContent = '';
    return;
  }

  const sig = calls
    .map(
      (c) =>
        `${c.task}:${c.state}:${c.max_price}:` +
        c.bids.map((b) => `${b.agent_cid}@${b.price}@${b.created_at}`).join(',') +
        `:` +
        (c.award == null
          ? '—'
          : `${c.award.agent_cid}@${c.award.price}@${c.award.escrow_entry ?? '—'}@${c.award.created_at}`),
    )
    .join('|');
  if (sig === boardSig) return;
  boardSig = sig;
  const body = byId('market-body');
  body.textContent = '';
  for (const call of calls) {
    body.appendChild(callRow(call));
    if (call.bids.length === 0) {
      const tr = el('tr', 'sub bid');
      const td = el('td', 'm note');
      td.colSpan = COLUMNS;
      td.textContent = 'no bids on this call yet';
      tr.appendChild(td);
      body.appendChild(tr);
    } else {
      const won = winningBid(call);
      for (const bid of call.bids) {
        body.appendChild(bidRow(bid, won != null && bid === won));
      }
    }
    if (call.award != null) body.appendChild(awardRow(call.award));
  }
  announce(`${fmtNum(calls.length)} calls on the board.`);
}

export function renderMarket(): void {
  const unreadable = store.callsError != null && store.calls.length === 0;
  const notice = byId('market-notice');
  notice.hidden = !unreadable;
  if (unreadable) {
    byId('market-notice-text').textContent =
      `The market is not answering at ${location.origin} (${store.callsError}). ` +
      'Retrying; the board catches up on its own.';
  }
  const live = byId('market-live');
  live.hidden = store.health !== 'live';
  live.textContent = store.health;

  const calls = store.calls;
  const offered = calls.filter((c) => c.state === 'open').length;
  const bids = calls.reduce((n, c) => n + c.bids.length, 0);
  const awarded = calls.filter((c) => c.award != null).length;
  byId('t-market-calls').textContent = store.callsError != null && calls.length === 0 ? '—' : fmtNum(calls.length);
  byId('t-market-offers').textContent = fmtNum(offered);
  byId('t-market-bids').textContent = fmtNum(bids);
  byId('t-market-awarded').textContent = fmtNum(awarded);

  renderBoard();
}
