/* The treasury v0's frozen shapes (crates/noded TreasuryView / JournalView),
   read through a defensive parse: the relay between the console and the node
   could be hostile, so this is never trusted. Every field is checked and a
   missing or mistyped one is an error that NAMES its path; unknown extra
   fields are tolerated, since the schema may grow columns the console does
   not yet read.

   The vocabulary is the treasury draft's own (docs/treasury-draft/schema.sql):
   six account kinds and nine entry kinds, one codebase's words. Every amount
   is whole credits (1 credit = $0.001 of real spend, statute 0109), a
   negative posting a debit and a positive one a credit. The `memo`, the
   `owner_ref` and the poster are text the service wrote, rendered as text,
   never markup. */

import { fmtNum } from '../format';

/** The kinds of account (the schema's `treasury.account_kind`). */
export type AccountKind =
  | 'mint'
  | 'treasury'
  | 'town_budget'
  | 'task_escrow'
  | 'agent_earnings'
  | 'external_spend';

/** The kinds of journal entry (the schema's `treasury.entry_kind`). */
export type EntryKind =
  | 'mint'
  | 'spend'
  | 'subcontract'
  | 'payment'
  | 'refund'
  | 'clawback'
  | 'fine'
  | 'fee'
  | 'budget_move';

/** One account, its balance derived from its postings. */
export interface Account {
  /** The account's id. */
  id: number;
  kind: AccountKind;
  /** The account's owner: a task id, an agent id, or a fixed name. */
  owner_ref: string;
  /** The limit below which `-credit_limit` the account may not fall (0 at v0). */
  credit_limit: number;
  /** The balance: the sum of the account's postings, in whole credits. */
  balance: number;
}

/** The treasury view's totals line. */
export interface Totals {
  /** How many accounts the treasury holds. */
  accounts: number;
  /** How many entries the journal holds. */
  entries: number;
  /** The total credited across the journal. */
  credits: number;
  /** The total debited across the journal. */
  debits: number;
  /** The sum of every account's balance: zero while the journal balances. */
  balance: number;
}

/** `GET /api/v0/treasury`'s body. */
export interface Treasury {
  /** The accounts, in creation order, each with its derived balance. */
  accounts: Account[];
  totals: Totals;
  /** Real money spent (0 at v0: nothing meters spend yet). */
  spent: number;
  /** The note on why `spent` is zero — the service's own words. */
  note: string;
}

/** One posting: one leg of a double-entry pair, which sums to zero. */
export interface Posting {
  /** The account's id. */
  account: number;
  /** The account's kind. */
  kind: AccountKind;
  /** The account's owner. */
  owner_ref: string;
  /** The amount: negative for a debit, positive for a credit. */
  amount: number;
}

/** One journal entry: the schema's `treasury.entries` row with its postings. */
export interface JournalEntry {
  /** The entry's id. */
  id: number;
  kind: EntryKind;
  /** The task the entry belongs to, or null. */
  task_id: string | null;
  /** The entry's chain root, or null (only `fine`/`fee` entries have none). */
  root_task: string | null;
  /** The entry's memo. */
  memo: string;
  /** Who posted it (always `treasury-service`). */
  posted_by: string;
  /** When it was posted, in milliseconds since the Unix epoch. */
  created_at: number;
  /** The entry's size: its credited total (equal to its debited total). */
  amount: number;
  /** The entry's postings, which sum to zero. */
  postings: Posting[];
}

/** `GET /api/v0/treasury/journal`'s body. */
export interface Journal {
  /** Up to the endpoint's limit of entries, oldest first as the API returns them. */
  entries: JournalEntry[];
  /** How many entries the journal holds. */
  total: number;
  /** The limit the response was cut to. */
  limit: number;
  /** Whether the journal holds more entries than the response carries. */
  truncated: boolean;
}

/** A missing or mistyped required field; the message names its path. */
export class TreasuryParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'TreasuryParseError';
  }
}

/* Static membership tables: the kinds noded v0 accepts, from the schema. */
const ACCOUNT_KINDS: Readonly<Record<string, true>> = {
  mint: true,
  treasury: true,
  town_budget: true,
  task_escrow: true,
  agent_earnings: true,
  external_spend: true,
};
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

function obj(v: unknown, path: string): Record<string, unknown> {
  if (v == null || typeof v !== 'object' || Array.isArray(v)) {
    throw new TreasuryParseError(`${path} is missing or not an object`);
  }
  return v as Record<string, unknown>;
}

function reqStr(o: Record<string, unknown>, key: string, path: string): string {
  const v = o[key];
  if (typeof v !== 'string') throw new TreasuryParseError(`${path}.${key} is missing or not a string`);
  return v;
}

/** A nullable string: present as `null`, or a string. An absent key is a
    missing field; the daemon always sends the column (serde writes `null`). */
function optStr(o: Record<string, unknown>, key: string, path: string): string | null {
  const v = o[key];
  if (v == null) {
    if (!(key in o)) throw new TreasuryParseError(`${path}.${key} is missing`);
    return null;
  }
  if (typeof v !== 'string') throw new TreasuryParseError(`${path}.${key} is not a string or null`);
  return v;
}

function int(o: Record<string, unknown>, key: string, path: string): number {
  const v = o[key];
  if (typeof v !== 'number' || !Number.isFinite(v)) {
    throw new TreasuryParseError(`${path}.${key} is missing or not a number`);
  }
  if (!Number.isInteger(v)) throw new TreasuryParseError(`${path}.${key} is not an integer`);
  return v;
}

function bool(o: Record<string, unknown>, key: string, path: string): boolean {
  const v = o[key];
  if (typeof v !== 'boolean') throw new TreasuryParseError(`${path}.${key} is missing or not a boolean`);
  return v;
}

function accountKind(o: Record<string, unknown>, key: string, path: string): AccountKind {
  const v = o[key];
  if (typeof v !== 'string' || ACCOUNT_KINDS[v] !== true) {
    throw new TreasuryParseError(`${path}.${key} is missing or not an account kind`);
  }
  return v as AccountKind;
}

function entryKind(o: Record<string, unknown>, key: string, path: string): EntryKind {
  const v = o[key];
  if (typeof v !== 'string' || ENTRY_KINDS[v] !== true) {
    throw new TreasuryParseError(`${path}.${key} is missing or not an entry kind`);
  }
  return v as EntryKind;
}

function readAccount(a: Record<string, unknown>, path: string): Account {
  return {
    id: int(a, 'id', path),
    kind: accountKind(a, 'kind', path),
    owner_ref: reqStr(a, 'owner_ref', path),
    credit_limit: int(a, 'credit_limit', path),
    balance: int(a, 'balance', path),
  };
}

function readTotals(t: Record<string, unknown>, path: string): Totals {
  return {
    accounts: int(t, 'accounts', path),
    entries: int(t, 'entries', path),
    credits: int(t, 'credits', path),
    debits: int(t, 'debits', path),
    balance: int(t, 'balance', path),
  };
}

function readPosting(p: Record<string, unknown>, path: string): Posting {
  return {
    account: int(p, 'account', path),
    kind: accountKind(p, 'kind', path),
    owner_ref: reqStr(p, 'owner_ref', path),
    amount: int(p, 'amount', path),
  };
}

function readEntry(e: Record<string, unknown>, path: string): JournalEntry {
  const postings = e['postings'];
  if (!Array.isArray(postings)) {
    throw new TreasuryParseError(`${path}.postings is missing or not an array`);
  }
  return {
    id: int(e, 'id', path),
    kind: entryKind(e, 'kind', path),
    task_id: optStr(e, 'task_id', path),
    root_task: optStr(e, 'root_task', path),
    memo: reqStr(e, 'memo', path),
    posted_by: reqStr(e, 'posted_by', path),
    created_at: int(e, 'created_at', path),
    amount: int(e, 'amount', path),
    postings: postings.map((p, i) => readPosting(obj(p, `${path}.postings[${i}]`), `${path}.postings[${i}]`)),
  };
}

/** Parse `GET /api/v0/treasury`; unknown extras are tolerated. */
export function parseTreasury(raw: unknown): Treasury {
  const root = obj(raw, 'treasury');
  const accounts = root['accounts'];
  if (!Array.isArray(accounts)) {
    throw new TreasuryParseError('treasury.accounts is missing or not an array');
  }
  return {
    accounts: accounts.map((a, i) => readAccount(obj(a, `treasury.accounts[${i}]`), `treasury.accounts[${i}]`)),
    totals: readTotals(obj(root['totals'], 'treasury.totals'), 'treasury.totals'),
    spent: int(root, 'spent', 'treasury'),
    note: reqStr(root, 'note', 'treasury'),
  };
}

/** Parse `GET /api/v0/treasury/journal`; unknown extras are tolerated. A bad
    row is an error naming its index. */
export function parseJournal(raw: unknown): Journal {
  const root = obj(raw, 'journal');
  const entries = root['entries'];
  if (!Array.isArray(entries)) {
    throw new TreasuryParseError('journal.entries is missing or not an array');
  }
  return {
    entries: entries.map((e, i) => readEntry(obj(e, `journal.entries[${i}]`), `journal.entries[${i}]`)),
    total: int(root, 'total', 'journal'),
    limit: int(root, 'limit', 'journal'),
    truncated: bool(root, 'truncated', 'journal'),
  };
}

/** A posting's name in the account-book: the kind, and the owner when it is
    not the kind's own fixed name — the mint account's owner is `mint`, so its
    leg reads `mint −750`, while an escrow's reads `task_escrow t1 +750`. */
export function postingLabel(p: Posting): string {
  return p.owner_ref === p.kind ? p.kind : `${p.kind} ${p.owner_ref}`;
}

/** A posting's amount as the ledger writes it: a signed figure, the debit
    side with a typed minus (`−`) and the credit side with a plus (`+`). */
export function fmtSigned(amount: number): string {
  return (amount < 0 ? '\u2212' : '+') + fmtNum(Math.abs(amount));
}

/** Whether the journal balances: credits equal debits and every account's
    balance sums to zero. The invariant the double-entry book rests on. */
export function balances(totals: Totals): boolean {
  return totals.credits === totals.debits && totals.balance === 0;
}
