/* The treasury: the plaza's account-book. The accounts the node keeps as a
   ruled table — each account's kind, its owner, its derived balance in credits
   and the dollars they stand for, and the limit below which it may not fall
   (`mint` is the source of all credits and may go arbitrarily negative — that
   is its nature) — then the double-entry journal, newest first, the way the
   register reads: every posted entry as a ruled row, its folio in the margin,
   its kind, the task it belongs to (a link into the ledger when it names one),
   its memo, its size in credits and dollars, its postings as a ruled pair
   (`mint −750 / task_escrow t1 +750`), and when it was posted.

   Above them, the totals line the double-entry book rests on: credits and
   debits are equal and every account's balance sums to zero, so the line says
   so and stamps it `balanced`. The service's own note on real spend is shown
   verbatim — v0 meters none. Nothing here is written from a frame alone: the
   journal's rows and the accounts' balances come from the node's GETs, so a
   hostile frame can only ask, never invent a post. */

import { byId, el, announce } from '../dom';
import { fmtMoney, fmtNum, fmtStamp, pad } from '../format';
import type { JournalEntry } from '../models/treasury';
import { balances, fmtSigned, postingLabel } from '../models/treasury';
import { store } from '../store';

/* The two tables rebuild only when their data changes, so scroll and focus
   survive a render that carries no new post. */
let accountsSig: string | null = null;
let journalSig: string | null = null;

/** The totals line, the balance it rests on, and the service's spend note. */
function renderBook(): void {
  const treasury = store.treasury;
  byId('t-tre-accounts').textContent = treasury == null ? '—' : fmtNum(treasury.totals.accounts);
  byId('t-tre-entries').textContent =
    store.journalError != null && store.journal.length === 0 ? '—' : fmtNum(store.journalTotal);
  byId('t-tre-credits').textContent = treasury == null ? '—' : fmtNum(treasury.totals.credits);
  byId('t-tre-spent').textContent = treasury == null ? '—' : fmtNum(treasury.spent);

  const line = byId('tre-totals');
  const mark = byId('tre-balanced');
  mark.textContent = '';
  if (treasury == null) {
    line.textContent = 'credits — · debits — · balance —';
  } else {
    line.textContent =
      `credits ${fmtNum(treasury.totals.credits)} · debits ${fmtNum(treasury.totals.debits)} · ` +
      `balance ${fmtNum(treasury.totals.balance)}`;
    /* the invariant as a decision: credits equal debits and the balances sum
       to zero — the double-entry book balances, or it does not */
    mark.appendChild(
      balances(treasury.totals)
        ? el('span', 'stamp sm', 'balanced')
        : el('span', 'mk warn', 'unbalanced'),
    );
  }
  /* the service's note on why `spent` is zero, verbatim — never paraphrased */
  byId('tre-spent-line').textContent =
    treasury == null ? '' : `spent ${fmtNum(treasury.spent)} — ${treasury.note}`;
}

/** The accounts table, in the treasury's own creation order. */
function renderAccounts(): void {
  const accounts = store.treasury?.accounts ?? [];
  const has = accounts.length > 0;
  byId('treasury-empty').hidden = has;
  byId('treasury-table').hidden = !has;
  byId('tre-accounts-count').textContent =
    store.treasury == null
      ? '—'
      : fmtNum(accounts.length) + (accounts.length === 1 ? ' account' : ' accounts');
  if (!has) {
    byId('treasury-empty-text').textContent =
      store.treasuryError != null
        ? `The accounts are not answering (${store.treasuryError}). Retrying; the book catches up on its own.`
        : 'The treasury holds no accounts.';
    accountsSig = null;
    byId('treasury-body').textContent = '';
    return;
  }

  const sig = accounts
    .map((a) => `${a.id}:${a.kind}:${a.owner_ref}:${a.balance}:${a.credit_limit}`)
    .join('|');
  if (sig === accountsSig) return;
  accountsSig = sig;
  const body = byId('treasury-body');
  body.textContent = '';
  for (const account of accounts) {
    const tr = el('tr');
    tr.appendChild(el('td', null, pad(account.id, 4)));
    tr.appendChild(el('td', null, account.kind));
    tr.appendChild(el('td', 'm owner', account.owner_ref));
    tr.appendChild(el('td', 'num', fmtMoney(account.balance)));
    tr.appendChild(el('td', 'num', fmtNum(account.credit_limit)));
    body.appendChild(tr);
  }
}

/** The journal, newest first. */
function renderJournal(): void {
  const entries = store.journal;
  const has = entries.length > 0;
  byId('journal-empty').hidden = has;
  byId('journal-table').hidden = !has;
  byId('tre-journal-count').textContent = has
    ? store.journalTruncated
      ? `newest ${fmtNum(entries.length)} of ${fmtNum(store.journalTotal)}`
      : fmtNum(entries.length) + (entries.length === 1 ? ' entry' : ' entries')
    : store.journalError != null
      ? '—'
      : '0 entries';
  if (!has) {
    byId('journal-empty-text').textContent =
      store.journalError != null
        ? `The journal is not answering (${store.journalError}). Retrying; the book catches up on its own.`
        : 'The journal holds no entries — no budget has been minted and nothing has been refunded yet.';
    journalSig = null;
    byId('journal-body').textContent = '';
    return;
  }

  const sig = entries.map((e) => e.id).join('|');
  if (sig === journalSig) return;
  journalSig = sig;
  const body = byId('journal-body');
  body.textContent = '';
  for (const entry of entries) {
    const tr = el('tr');
    tr.appendChild(el('td', null, pad(entry.id, 4)));
    tr.appendChild(el('td')).appendChild(el('span', 'chip lkind', entry.kind));
    /* the task the entry belongs to, as a link into the ledger when it has one */
    const taskTd = el('td', 'm');
    if (entry.task_id != null) {
      const link = el('a', null, entry.task_id);
      link.href = `#ledger/${encodeURIComponent(entry.task_id)}`;
      taskTd.appendChild(link);
    } else {
      taskTd.appendChild(document.createTextNode('—'));
    }
    tr.appendChild(taskTd);
    tr.appendChild(el('td', 'memo', entry.memo));
    tr.appendChild(el('td', 'num', fmtMoney(entry.amount)));
    tr.appendChild(el('td', 'postings', postingsLine(entry)));
    tr.appendChild(el('td', 'm', fmtStamp(entry.created_at)));
    body.appendChild(tr);
  }
  announce(`${fmtNum(entries.length)} entries in the journal.`);
}

/** An entry's postings as one ruled pair: each leg named in the account-book's
    own words and signed, joined the way the ledger writes them. The text is
    the service's own, so it is rendered as text. */
function postingsLine(entry: JournalEntry): string {
  return entry.postings.map((p) => `${postingLabel(p)} ${fmtSigned(p.amount)}`).join(' / ');
}

export function renderTreasury(): void {
  const notice = byId('treasury-notice');
  const unreadable = store.treasuryError != null && store.treasury == null;
  notice.hidden = !unreadable;
  if (unreadable) {
    byId('treasury-notice-text').textContent =
      `The treasury is not answering at ${location.origin} (${store.treasuryError}). ` +
      'Retrying; the book catches up on its own.';
  }
  const live = byId('treasury-live');
  live.hidden = store.health !== 'live';
  live.textContent = store.health;

  renderBook();
  renderAccounts();
  renderJournal();
}
