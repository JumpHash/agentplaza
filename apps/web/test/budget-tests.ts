// The budgets and seats: the money text (dollars from micro-dollars, grouped tokens), the set-budget form's parse and the
// body the write takes, the overview's figure, the ledger's and the seats' words, and the tests that a live frame is for
// this owner or this task. The DOM is not touched here. Bundled with esbuild on its own; it prints "budget tests: ok".

import {
  amountText,
  budgetBody,
  capInput,
  capText,
  DEFAULT_WARN_PCT,
  dollarsText,
  dropsCap,
  formOf,
  groupedText,
  isBudgetOf,
  isTaskFrame,
  isTaskSession,
  ledgerAmount,
  ledgerLabel,
  leftStat,
  monthText,
  namesOwner,
  parseCap,
  parseDollars,
  parseTokens,
  parseWarnPct,
  policyText,
  seatLabel,
  seatNotes,
  shareText,
  unitLocked,
} from '../src/views/budget-model';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

// ---- Money and counts ------------------------------------------------------------------------------------------------

eq(dollarsText(1_500_000), '$1.50', '1500000 micro-dollars is $1.50');
eq(dollarsText(0), '$0.00', 'zero reads $0.00');
eq(dollarsText(2_500_000), '$2.50', 'two dollars and a half');
eq(dollarsText(1_234_567_890), '$1,234.57', 'thousands are grouped and the cents round');
eq(dollarsText(999_999), '$1.00', 'a cent that rounds up carries into the dollars');
eq(dollarsText(4_000), '$0.00', 'under half a cent reads $0.00');
eq(dollarsText(-500_000), '-$0.50', 'a negative amount keeps its sign first');
eq(groupedText(0), '0', 'zero is zero');
eq(groupedText(999), '999', 'under a thousand has no comma');
eq(groupedText(1000), '1,000', 'a thousand is grouped');
eq(groupedText(1_234_567), '1,234,567', 'millions are grouped twice');
eq(groupedText(-1234), '-1,234', 'a negative keeps its sign');
eq(groupedText(-0.4), '0', 'a negative that rounds to zero has no sign');
eq(amountText('usd', 1_500_000), '$1.50', 'a usd amount is dollars');
eq(amountText('tokens', 1_234_567), '1,234,567 tokens', 'a token amount is grouped, with its unit');
eq(amountText('tokens', Number.NaN), '0 tokens', 'a missing number reads 0');
eq(capText('usd', null), 'No cap', 'no cap reads No cap');
eq(capText('usd', undefined), 'No cap', 'a missing cap reads No cap');
eq(capText('tokens', 500), '500 tokens', 'a token cap');

// ---- Parsing the form ------------------------------------------------------------------------------------------------

eq(parseCap('usd', '2.5'), { ok: true, value: 2_500_000 }, '2.5 dollars is 2500000 micro-dollars');
eq(parseCap('usd', '25'), { ok: true, value: 25_000_000 }, 'a whole amount of dollars');
eq(parseCap('usd', '5.'), { ok: true, value: 5_000_000 }, 'a trailing point is still the amount');
eq(parseCap('usd', '$1,200'), { ok: true, value: 1_200_000_000 }, 'a dollar sign and commas are read');
eq(parseCap('usd', '0.000001'), { ok: true, value: 1 }, 'six decimals are the smallest step');
eq(parseCap('usd', '0'), { ok: true, value: 0 }, 'zero dollars is a cap of nothing');
eq(parseCap('usd', ''), { ok: true, value: null }, 'blank dollars is no cap');
eq(parseCap('usd', '   '), { ok: true, value: null }, 'spaces are blank');
eq(parseCap('tokens', ''), { ok: true, value: null }, 'blank tokens is no cap');
eq(parseCap('tokens', '500000'), { ok: true, value: 500_000 }, 'whole tokens');
eq(parseCap('tokens', '1,000'), { ok: true, value: 1_000 }, 'grouped tokens');
eq(parseCap('usd', '-3').ok, false, 'a negative amount is refused');
eq(parseCap('usd', 'abc').ok, false, 'words are refused');
eq(parseCap('usd', '.').ok, false, 'a lone point is refused');
eq(parseCap('usd', '2..5').ok, false, 'two points are refused');
eq(parseCap('usd', '1.0000001').ok, false, 'seven decimals are refused');
eq(parseTokens('2.5').ok, false, 'tokens are whole: a decimal is refused');
eq(parseTokens('-3').ok, false, 'negative tokens are refused');
eq(parseTokens('abc').ok, false, 'words are refused as tokens');

// Commas only group thousands: `1,000` and `1,000.50` read; any other comma is refused.
eq(parseCap('usd', '1,000'), { ok: true, value: 1_000_000_000 }, 'a comma that groups thousands reads: 1,000');
eq(parseCap('usd', '1,000.50'), { ok: true, value: 1_000_500_000 }, 'a comma that groups thousands reads before the decimals: 1,000.50');
eq(parseCap('usd', '0,50').ok, false, 'a comma with no thousands after it is refused: 0,50');
eq(parseCap('usd', '2,5').ok, false, 'a comma with one digit after it is refused: 2,5');
eq(parseCap('usd', '5,00').ok, false, 'a comma with two digits after it is refused: 5,00');
eq(parseCap('usd', '1,2,3').ok, false, 'two misplaced commas are refused: 1,2,3');
eq(parseCap('usd', '1,0000').ok, false, 'a group of four digits is refused: 1,0000');
eq(parseCap('usd', '1000,000').ok, false, 'a group after four digits is refused: 1000,000');
eq(parseCap('usd', '-1,000').ok, false, 'a negative grouped amount is refused');
eq(parseTokens('1,000,000'), { ok: true, value: 1_000_000 }, 'tokens group thousands');
eq(parseTokens('5,00').ok, false, 'tokens: 5,00 is refused');
eq(parseTokens('1,2').ok, false, 'tokens: 1,2 is refused');

// Dollars are converted as text, not as floats: every digit of a long amount is kept, and an amount past the safe integers
// is refused rather than rounded.
eq(parseDollars('8999999999.999999'), { ok: true, value: 8_999_999_999_999_999 }, 'the largest decimal amount keeps every digit');
eq(parseDollars('0.1'), { ok: true, value: 100_000 }, 'a tenth of a dollar is 100000 micro-dollars');
eq(parseDollars('2.5'), { ok: true, value: 2_500_000 }, '2.5 is 2500000 micro-dollars');
eq(parseDollars('1,000.5'), { ok: true, value: 1_000_500_000 }, 'a grouped amount with decimals is exact: 1,000.5');
eq(parseDollars('1.005'), { ok: true, value: 1_005_000 }, 'a decimal that a float would round is exact: 1.005');
eq(parseDollars('.5'), { ok: true, value: 500_000 }, 'a leading point is half a dollar');
eq(parseDollars('9007199254.740993').ok, false, 'an amount past the safe integers is refused, not rounded');

eq(parseWarnPct(''), { ok: true, value: 80 }, 'blank warns at the default 80');
eq(parseWarnPct('1'), { ok: true, value: 1 }, '1 is the lowest warn share');
eq(parseWarnPct('100'), { ok: true, value: 100 }, '100 is the highest warn share');
eq(parseWarnPct('0').ok, false, 'a warn share of 0 is refused');
eq(parseWarnPct('101').ok, false, 'a warn share over 100 is refused');
eq(parseWarnPct('80.5').ok, false, 'a fraction of a percent is refused');
eq(parseWarnPct('x').ok, false, 'a word is refused as a warn share');
eq(DEFAULT_WARN_PCT, 80, 'the default warn share is 80');

// ---- The body the write takes ----------------------------------------------------------------------------------------

eq(
  budgetBody('task', { unit: 'usd', cap: '2.5', warnPct: '', monthlyCap: '' }),
  { ok: true, value: { unit: 'usd', cap: 2_500_000 } },
  'a task takes only its unit and cap',
);
eq(
  budgetBody('task', { unit: 'usd', cap: '', warnPct: '500', monthlyCap: 'x' }),
  { ok: true, value: { unit: 'usd', cap: null } },
  'a task ignores the project-only fields',
);
eq(
  budgetBody('project', { unit: 'usd', cap: '', warnPct: '', monthlyCap: '' }),
  { ok: true, value: { unit: 'usd', cap: null, warn_at_pct: 80, monthly_cap: null } },
  'a project with no cap warns at 80 and has no monthly cap',
);
eq(
  budgetBody('project', { unit: 'tokens', cap: '1000', warnPct: '90', monthlyCap: '5,000' }),
  { ok: true, value: { unit: 'tokens', cap: 1000, warn_at_pct: 90, monthly_cap: 5000 } },
  'a project takes the warn share and a monthly cap in its unit',
);
eq(budgetBody('project', { unit: 'usd', cap: '10', warnPct: '80', monthlyCap: 'lots' }).ok, false, 'a bad monthly cap refuses the body');
eq(budgetBody('task', { unit: 'tokens', cap: '-1', warnPct: '', monthlyCap: '' }).ok, false, 'a bad cap refuses the body');
eq(budgetBody('project', { unit: 'usd', cap: '10', warnPct: '0', monthlyCap: '' }).ok, false, 'a warn share of 0 refuses the body');
eq(
  budgetBody('project', { unit: 'usd', cap: '0', warnPct: '80', monthlyCap: '1' }),
  { ok: true, value: { unit: 'usd', cap: 0, warn_at_pct: 80, monthly_cap: 1_000_000 } },
  'a cap of zero is allowed; a monthly cap of one dollar is 1,000,000 micro-dollars',
);

// A monthly cap of zero is refused (at least 1, or blank for none), in either unit and however it is written.
eq(budgetBody('project', { unit: 'usd', cap: '10', warnPct: '80', monthlyCap: '0' }).ok, false, 'a monthly cap of zero dollars is refused');
eq(budgetBody('project', { unit: 'usd', cap: '10', warnPct: '80', monthlyCap: '0.000000' }).ok, false, 'a monthly cap of zero with decimals is refused');
eq(budgetBody('project', { unit: 'tokens', cap: '', warnPct: '', monthlyCap: '0' }).ok, false, 'a monthly cap of zero tokens is refused');
eq(budgetBody('project', { unit: 'usd', cap: '10', warnPct: '80', monthlyCap: '' }).ok, true, 'a blank monthly cap is none, not refused');
eq(budgetBody('project', { unit: 'usd', cap: '10', warnPct: '80', monthlyCap: '2,5' }).ok, false, 'a monthly cap with a misplaced comma is refused');

// ---- The form's start and the caps as it shows them ------------------------------------------------------------------

eq(capInput('usd', 2_500_000), '2.5', 'two and a half dollars shows as 2.5');
eq(capInput('usd', 50_000_000), '50', 'whole dollars show without decimals');
eq(capInput('usd', 1), '0.000001', 'the smallest step keeps all six decimals');
eq(capInput('usd', 1_000_010), '1.00001', 'trailing zeros are dropped, inner ones kept');
eq(capInput('tokens', 500), '500', 'tokens show as a plain count');
eq(capInput('usd', null), '', 'no cap is a blank field');
eq(
  formOf(null),
  { unit: 'usd', cap: '', warnPct: '80', monthlyCap: '' },
  'no budget starts in dollars, with no cap and the default warn share',
);
eq(
  formOf({ owner: 'pr1', unit: 'usd', cap: 2_500_000, spent: 0, warn_at_pct: 70, monthly_cap: 10_000_000 }),
  { unit: 'usd', cap: '2.5', warnPct: '70', monthlyCap: '10' },
  'a set budget starts with its own values, in dollars',
);
eq(
  formOf({ owner: 't1', unit: 'tokens', cap: 1500, spent: 0 }),
  { unit: 'tokens', cap: '1500', warnPct: '80', monthlyCap: '' },
  'a token budget starts in tokens, and an unset warn share is the default',
);

// The unit is fixed once the budget has spend, a call in flight or a reservation.
eq(unitLocked({ spent: 0 }), false, 'a budget with no spend leaves its unit free');
eq(unitLocked({ spent: 0, held: 0, reserved: 0 }), false, 'zeros leave the unit free');
eq(unitLocked({ spent: 0, held: undefined, reserved: undefined }), false, 'missing holds and reservations leave the unit free');
eq(unitLocked({ spent: 1_500_000 }), true, 'spend fixes the unit');
eq(unitLocked({ spent: 0, held: 250 }), true, 'a call in flight fixes the unit');
eq(unitLocked({ spent: 0, reserved: 9 }), true, 'a reservation fixes the unit');

// A unit change must not drop the cap the budget has: the cap was in the old unit, so it is typed again. A blank cap in the
// same unit is a deliberate no cap, and a budget with no cap has nothing to drop.
const usd25 = { unit: 'usd' as const, cap: 25_000_000 };
eq(dropsCap(usd25, 'tokens', ''), true, 'a unit change with the cap left blank drops the cap');
eq(dropsCap(usd25, 'tokens', '  '), true, 'spaces are blank in the new unit too');
eq(dropsCap(usd25, 'tokens', '500000'), false, 'a cap typed in the new unit keeps the budget capped');
eq(dropsCap(usd25, 'usd', ''), false, 'a blank cap in the same unit is a deliberate no cap');
eq(dropsCap(usd25, 'usd', '25'), false, 'the same unit with its cap keeps it');
eq(dropsCap({ unit: 'tokens', cap: 0 }, 'usd', ''), true, 'a cap of zero is a cap too');
eq(dropsCap({ unit: 'usd', cap: null }, 'tokens', ''), false, 'a budget with no cap has none to drop');
eq(dropsCap({ unit: null, cap: null }, 'usd', ''), false, 'a budget not set has none to drop');
eq(dropsCap(null, 'tokens', ''), false, 'no budget read yet has nothing to drop');

// ---- The overview's figure and the shares ----------------------------------------------------------------------------

eq(leftStat(null), { value: '—', sub: 'from the plaza' }, 'no budget reads a dash, as the overview always did');
eq(leftStat({ owner: 'pr1', unit: null, cap: null, spent: 0 }), { value: '—', sub: 'from the plaza' }, 'an unset budget reads the same dash');
eq(leftStat({ owner: 'pr1', unit: 'usd', cap: null, spent: 0, left: null }), { value: '—', sub: 'no cap' }, 'an uncapped budget has no figure');
eq(
  leftStat({ owner: 'pr1', unit: 'usd', cap: 20_000_000, spent: 7_660_000, left: 12_340_000 }),
  { value: '$12.34', sub: 'dollars left' },
  'dollars left in the budget are dollars',
);
eq(
  leftStat({ owner: 'pr1', unit: 'tokens', cap: 100_000, spent: 1_000, left: 99_000 }),
  { value: '99,000', sub: 'tokens left' },
  'tokens left are a grouped count',
);
eq(
  leftStat({ owner: 'pr1', unit: 'usd', cap: 1_000_000, spent: 1_500_000, left: -500_000 }),
  { value: '-$0.50', sub: 'dollars left' },
  'over the cap the figure is negative',
);
eq(shareText(null, null, 80), 'No cap', 'an uncapped budget has no share');
eq(shareText(0, null, 80), 'The cap is zero', 'a cap of zero says so, and not No cap');
eq(shareText(0, 0, 80), 'The cap is zero', 'a cap of zero says so even with a share');
eq(shareText(20_000_000, 21.4, 80), '21% of the cap · warns at 80%', 'the share and the warn share');
eq(shareText(20_000_000, 60, undefined), '60% of the cap · warns at 80%', 'a budget with no warn share warns at 80');
eq(monthText(202610), 'October 2026', 'a yyyymm month reads in words');
eq(monthText(202601), 'January 2026', 'January');
eq(monthText(202613), '202613', 'a number that is no month reads as itself');

// ---- Words -----------------------------------------------------------------------------------------------------------

eq(ledgerLabel('set'), 'Set', 'the set entry');
eq(ledgerLabel('reserve'), 'Reserved', 'the reserve entry');
eq(ledgerLabel('hold'), 'Held for a call', 'the hold entry');
eq(ledgerLabel('charge'), 'Charged', 'the charge entry');
eq(ledgerLabel('release'), 'Released', 'the release entry');
eq(ledgerLabel('raise'), 'Raised', 'the raise entry');
eq(ledgerAmount('usd', { kind: 'set', amount: 0 }), 'No cap', 'a set of zero took the cap away: No cap');
eq(ledgerAmount('tokens', { kind: 'set', amount: 0 }), 'No cap', 'a set of zero tokens is No cap too');
eq(ledgerAmount('usd', { kind: 'set', amount: 2_500_000 }), '$2.50', 'a set cap shows its amount');
eq(ledgerAmount('usd', { kind: 'charge', amount: 0 }), '$0.00', 'a charge of zero is an amount, not No cap');
eq(ledgerAmount('tokens', { kind: 'charge', amount: 1200 }), '1,200 tokens', 'a charge in tokens');
eq(seatLabel('waiting'), 'Waiting', 'a waiting seat');
eq(seatLabel('ready'), 'Ready', 'a ready seat');
eq(seatLabel('filled'), 'Working', 'a filled seat is working');
eq(seatLabel('done'), 'Done', 'a done seat');
eq(seatLabel('failed'), 'Failed', 'a failed seat');
eq(seatNotes({ attempts: 1, round: 1 }), [], 'the first try in the first round says nothing');
eq(seatNotes({ attempts: 2, round: 1 }), ['Attempt 2'], 'a second try');
eq(seatNotes({ attempts: 1, round: 3 }), ['Round 3'], 'a third round');
eq(seatNotes({ attempts: 2, round: 2 }), ['Attempt 2', 'Round 2'], 'a try and a round');
eq(policyText('any'), 'Any model the project allows', 'any model');
eq(policyText('differ'), 'A model no other seat used', 'a different model');

// ---- Live frames: whose they are -------------------------------------------------------------------------------------

eq(isBudgetOf({ owner: 'pr1', owner_kind: 'project' }, { kind: 'project', id: 'pr1' }), true, 'a budget frame of the project');
eq(isBudgetOf({ owner: 'pr1', owner_kind: 'task' }, { kind: 'project', id: 'pr1' }), false, 'the same id of another kind is not this budget');
eq(isBudgetOf({ owner: 'pr2', owner_kind: 'project' }, { kind: 'project', id: 'pr1' }), false, 'another project is not this budget');
eq(isBudgetOf(null, { kind: 'task', id: 't1' }), false, 'a frame with no data is nobody’s');
eq(isTaskFrame({ id: 't1' }, 't1'), true, 'a task frame of the task');
eq(isTaskFrame({ id: 't2' }, 't1'), false, 'a task frame of another task');
eq(isTaskFrame('t1', 't1'), false, 'a frame that is not an object is not a task');
eq(isTaskSession({ id: 's1', task: 't1' }, 't1'), true, 'a session on the task');
eq(isTaskSession({ id: 's1', task: null }, 't1'), false, 'a long-lived session has no task');
eq(namesOwner({ id: 's1', project: 'pr1', task: 't1' }, { kind: 'project', id: 'pr1' }), true, 'a session of the project names its budget');
eq(namesOwner({ id: 's1', project: 'pr2', task: 't1' }, { kind: 'project', id: 'pr1' }), false, 'a session of another project does not');
eq(namesOwner({ id: 's1', project: 'pr1', task: 't1' }, { kind: 'task', id: 't1' }), true, 'a session of the task names the task budget');
eq(namesOwner({ id: 's1', project: 'pr1', task: 't2' }, { kind: 'task', id: 't1' }), false, 'a session of another task does not');
eq(namesOwner({ id: 's1', project: 'pr1', task: null }, { kind: 'task', id: 't1' }), false, 'a long-lived session names no task');
eq(namesOwner({ project: 'pr1', task: 't1' }, { kind: 'task', id: 't1' }), true, 'a card of the task names its budget');
eq(namesOwner(null, { kind: 'project', id: 'pr1' }), false, 'a frame with no data names nobody');

console.log(`budget tests: ok (${checks} checks)`);
