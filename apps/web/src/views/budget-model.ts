// The rules and words of the work engine's budgets and seats: a budget's money as dollars (micro-dollars on the wire) or
// a count of tokens, the set-budget form read into the body the hub takes, the overview's figure, the ledger's and the
// seats' labels, and the test that a live frame is for this owner or this task. No DOM and no runtime imports from the
// console's modules, so node can test it (test/budget-tests.ts).

import type { Budget, BudgetBody, BudgetUnit, LedgerEntry, LedgerKind, ModelPolicy, OwnerKind, Seat, SeatState } from '../api-work';

/** Micro-dollars in a dollar: a `usd` amount on the wire is dollars times this (api-work.ts MICROS_PER_DOLLAR). */
const MICROS = 1_000_000;

/** The share of a cap at which the owner is told, when the budget names none. */
export const DEFAULT_WARN_PCT = 80;

/** A form value read into what it means, or the sentence that says why it is refused. */
export type Parsed<T> = { ok: true; value: T } | { ok: false; problem: string };

/** The set-budget form's fields, as typed. */
export interface BudgetForm {
  unit: BudgetUnit;
  cap: string;
  warnPct: string;
  monthlyCap: string;
}

/** Whose budget a card shows: a project's or a task's. */
export interface BudgetOwner {
  kind: OwnerKind;
  id: string;
}

export const UNIT_OPTIONS: { value: BudgetUnit; label: string }[] = [
  { value: 'usd', label: 'Dollars' },
  { value: 'tokens', label: 'Tokens' },
];

// ---- Money and counts ----------------------------------------------------------------------------------------------

/** Digits in groups of three: `1234567` is `1,234,567`; a negative keeps its sign. */
export function groupedText(n: number): string {
  const digits = String(Math.round(Math.abs(n))).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
  return n < 0 && Math.round(n) !== 0 ? `-${digits}` : digits;
}

/** Dollars from micro-dollars: `1500000` is `$1.50`. Cents round half up; a negative amount reads `-$0.50`. */
export function dollarsText(micros: number): string {
  const cents = Math.round(Math.abs(micros) / (MICROS / 100));
  const sign = micros < 0 && cents > 0 ? '-' : '';
  return `${sign}$${groupedText(Math.floor(cents / 100))}.${String(cents % 100).padStart(2, '0')}`;
}

/** An amount in its budget's unit: dollars for `usd`, a grouped count of tokens otherwise. */
export function amountText(unit: BudgetUnit, amount: number): string {
  const n = Number.isFinite(amount) ? amount : 0;
  return unit === 'usd' ? dollarsText(n) : `${groupedText(n)} tokens`;
}

/** A limit's text: its amount, or "No cap" when it has none (a null or missing cap). */
export function capText(unit: BudgetUnit, cap: number | null | undefined): string {
  return typeof cap === 'number' ? amountText(unit, cap) : 'No cap';
}

/** The unit's name as a card shows it. */
export function unitName(unit: BudgetUnit): string {
  return unit === 'usd' ? 'Dollars' : 'Tokens';
}

const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];

/** The month a `yyyymm` number names: `202610` is "October 2026"; a number that is no month reads as itself. */
export function monthText(yyyymm: number): string {
  const name = MONTHS[(yyyymm % 100) - 1];
  return name ? `${name} ${Math.floor(yyyymm / 100)}` : String(yyyymm);
}

/**
 * The share of the cap spent, with the share the owner is told at: "21% of the cap · warns at 80%". A cap of zero reads
 * "The cap is zero"; an uncapped budget reads "No cap".
 */
export function shareText(cap: number | null | undefined, pct: number | null | undefined, warnAt: number | undefined): string {
  if (cap === 0) return 'The cap is zero';
  return typeof pct === 'number' ? `${Math.round(pct)}% of the cap · warns at ${warnAt ?? DEFAULT_WARN_PCT}%` : 'No cap';
}

/** The overview's budget figure: what is left in the budget's unit; a dash, with why, when there is no budget or no cap. */
export function leftStat(budget: Budget | null): { value: string; sub: string } {
  if (!budget?.unit) return { value: '—', sub: 'from the plaza' };
  if (typeof budget.left !== 'number') return { value: '—', sub: 'no cap' };
  return budget.unit === 'usd' ? { value: dollarsText(budget.left), sub: 'dollars left' } : { value: groupedText(budget.left), sub: 'tokens left' };
}

// ---- The set-budget form -------------------------------------------------------------------------------------------

/** A cap as the form shows it: dollars with the decimals it has, or a plain count of tokens; blank for none. */
export function capInput(unit: BudgetUnit, value: number | null | undefined): string {
  if (typeof value !== 'number') return '';
  if (unit === 'tokens') return String(value);
  const whole = Math.floor(value / MICROS);
  const rest = value % MICROS;
  return rest === 0 ? String(whole) : `${whole}.${String(rest).padStart(6, '0').replace(/0+$/, '')}`;
}

/** The form's start: the budget's own values, or the defaults (dollars, no cap, the default warn share) when it has none. */
export function formOf(budget: Budget | null): BudgetForm {
  const unit: BudgetUnit = budget?.unit ?? 'usd';
  return {
    unit,
    cap: capInput(unit, budget?.cap),
    warnPct: String(budget?.warn_at_pct ?? DEFAULT_WARN_PCT),
    monthlyCap: capInput(unit, budget?.monthly_cap),
  };
}

/** Whether a budget's unit is fixed: spend, a call in flight or a reservation is counted in the unit it was set in. */
export function unitLocked(budget: Pick<Budget, 'spent' | 'held' | 'reserved'>): boolean {
  return budget.spent > 0 || (budget.held ?? 0) > 0 || (budget.reserved ?? 0) > 0;
}

/**
 * Whether saving a form drops a cap the budget has: a unit change that leaves the cap blank. The cap was in the old unit
 * and is not carried over, so it is typed again in the new one. A blank cap in the same unit is a deliberate "no cap".
 */
export function dropsCap(stored: Pick<Budget, 'unit' | 'cap'> | null, unit: BudgetUnit, capText: string): boolean {
  return stored !== null && typeof stored.cap === 'number' && stored.unit !== unit && capText.trim() === '';
}

/**
 * A typed amount with its commas taken out. Commas may only group thousands (`1,000`, `1,000.50`); a comma anywhere else
 * (`2,5`, `5,00`, `1,2,3`) gives null.
 */
function ungrouped(text: string): string | null {
  if (!text.includes(',')) return text;
  return /^\d{1,3}(,\d{3})+(\.\d*)?$/.test(text) ? text.replace(/,/g, '') : null;
}

/**
 * Dollars typed in a form (`2.5`, `$1,200`) as micro-dollars: at most six decimals, and never negative. The whole part and
 * the fraction are integers taken from the text, so no float rounding moves an amount (`8999999999.999999` is exact).
 */
export function parseDollars(text: string): Parsed<number> {
  const typed = text.trim().replace(/^\$/, '');
  if (typed.startsWith('-')) return { ok: false, problem: 'The amount is 0 or more dollars.' };
  const s = ungrouped(typed);
  if (s === null) return { ok: false, problem: 'Commas only group thousands, like 1,000.' };
  const m = /^(\d*)\.?(\d*)$/.exec(s);
  if (!m || (m[1] === '' && m[2] === '')) return { ok: false, problem: 'Enter the amount in dollars, like 25 or 2.50.' };
  const fraction = m[2] ?? '';
  if (fraction.length > 6) return { ok: false, problem: 'Dollars take at most six decimals.' };
  // Exact while the result is below 2^53; a larger amount fails the safe-integer test below.
  const micros = Number(m[1] || '0') * MICROS + Number(fraction.padEnd(6, '0'));
  return Number.isSafeInteger(micros) ? { ok: true, value: micros } : { ok: false, problem: 'That amount is too large.' };
}

/** A whole count of tokens typed in a form; commas may only group thousands. */
export function parseTokens(text: string): Parsed<number> {
  const s = ungrouped(text.trim());
  if (s === null || !/^\d+$/.test(s)) return { ok: false, problem: 'Enter a whole number of tokens, like 500000.' };
  const n = Number(s);
  return Number.isSafeInteger(n) ? { ok: true, value: n } : { ok: false, problem: 'That many tokens is too large.' };
}

/** A cap typed for `unit`: blank is no cap (null); otherwise dollars for `usd` and whole tokens for `tokens`. */
export function parseCap(unit: BudgetUnit, text: string): Parsed<number | null> {
  if (text.trim() === '') return { ok: true, value: null };
  return unit === 'usd' ? parseDollars(text) : parseTokens(text);
}

/** The warn share typed in a form: blank is the default; otherwise a whole percent from 1 to 100. */
export function parseWarnPct(text: string): Parsed<number> {
  const s = text.trim();
  if (s === '') return { ok: true, value: DEFAULT_WARN_PCT };
  const n = Number(s);
  return /^\d+$/.test(s) && n >= 1 && n <= 100 ? { ok: true, value: n } : { ok: false, problem: 'Warn at a whole percent from 1 to 100.' };
}

/** A monthly cap typed for `unit`: blank is none; otherwise more than zero. */
function parseMonthlyCap(unit: BudgetUnit, text: string): Parsed<number | null> {
  const cap = parseCap(unit, text);
  if (!cap.ok || cap.value !== 0) return cap;
  return { ok: false, problem: 'The monthly cap is more than zero, or blank for none.' };
}

/**
 * The write's body from the form. A task's takes only the unit and the cap; a project's also takes the warn share and the
 * monthly cap (blank: none), both in the project's unit.
 */
export function budgetBody(kind: OwnerKind, form: BudgetForm): Parsed<BudgetBody> {
  const cap = parseCap(form.unit, form.cap);
  if (!cap.ok) return cap;
  if (kind === 'task') return { ok: true, value: { unit: form.unit, cap: cap.value } };
  const warn = parseWarnPct(form.warnPct);
  if (!warn.ok) return warn;
  const monthly = parseMonthlyCap(form.unit, form.monthlyCap);
  if (!monthly.ok) return monthly;
  return { ok: true, value: { unit: form.unit, cap: cap.value, warn_at_pct: warn.value, monthly_cap: monthly.value } };
}

// ---- Words -----------------------------------------------------------------------------------------------------------

const LEDGER_LABELS: Record<LedgerKind, string> = {
  set: 'Set',
  reserve: 'Reserved',
  hold: 'Held for a call',
  charge: 'Charged',
  release: 'Released',
  raise: 'Raised',
};

/** What a ledger entry records, in words; a kind the console does not know shows as it came. */
export function ledgerLabel(kind: LedgerKind): string {
  return LEDGER_LABELS[kind] ?? kind;
}

/** A ledger entry's amount in its budget's unit; a `set` of zero took the cap away, so it reads No cap. */
export function ledgerAmount(unit: BudgetUnit, entry: Pick<LedgerEntry, 'kind' | 'amount'>): string {
  return entry.kind === 'set' && entry.amount === 0 ? 'No cap' : amountText(unit, entry.amount);
}

const SEAT_LABELS: Record<SeatState, string> = { waiting: 'Waiting', ready: 'Ready', filled: 'Working', done: 'Done', failed: 'Failed' };

/** A seat's state in words: a seat with a session in it is working. */
export function seatLabel(state: SeatState): string {
  return SEAT_LABELS[state] ?? state;
}

/** A seat's tries and rounds, when they are past the first: "Attempt 2", "Round 3". */
export function seatNotes(seat: Pick<Seat, 'attempts' | 'round'>): string[] {
  const notes: string[] = [];
  if (seat.attempts > 1) notes.push(`Attempt ${seat.attempts}`);
  if (seat.round > 1) notes.push(`Round ${seat.round}`);
  return notes;
}

const POLICY_LABELS: Record<ModelPolicy, string> = { any: 'Any model the project allows', differ: 'A model no other seat used' };

/** Which models may fill a seat, in words. */
export function policyText(policy: ModelPolicy): string {
  return POLICY_LABELS[policy] ?? policy;
}

// ---- Live frames -----------------------------------------------------------------------------------------------------

/** A field of a frame's data, when the data is an object that has it. */
function fieldOf(data: unknown, key: string): unknown {
  return typeof data === 'object' && data !== null ? (data as Record<string, unknown>)[key] : undefined;
}

/** Whether a `budget` frame is the budget of `owner`: the same id and the same kind. */
export function isBudgetOf(data: unknown, owner: BudgetOwner): boolean {
  return fieldOf(data, 'owner') === owner.id && fieldOf(data, 'owner_kind') === owner.kind;
}

/** Whether a `task` frame (`{id}`) is about the task `id`. */
export function isTaskFrame(data: unknown, id: string): boolean {
  return fieldOf(data, 'id') === id;
}

/** Whether a `session` frame (the Session) works on the task `id`; a long-lived session has no task. */
export function isTaskSession(data: unknown, id: string): boolean {
  return fieldOf(data, 'task') === id;
}

/** Whether a `session` or `card` frame names the owner: a project's by its project, a task's by its task. */
export function namesOwner(data: unknown, owner: BudgetOwner): boolean {
  return fieldOf(data, owner.kind) === owner.id;
}
