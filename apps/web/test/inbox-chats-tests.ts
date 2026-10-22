// The inbox's and the chats' logic: the card kind labels, the amount an answer takes (in the unit of its owner's budget),
// the drafts kept across redraws, the text of a settled card, the author and chat kind labels, the message order and
// limit, and the expert lines. The views draw them. Bundled with the other suites by test/tests.ts; it prints
// "inbox chats tests: ok".

import type { Card, ChatMessage } from '../src/api-work';
import {
  amountUnitName,
  amountWire,
  authorLabel,
  cardExpertLine,
  cardKindLabel,
  chatKindLabel,
  clockText,
  Drafts,
  doneText,
  frameChat,
  isDangerChoice,
  isStaleAnswer,
  messageExpertLine,
  nonBlank,
  preselectProject,
  recentMessages,
  settledText,
} from '../src/views/chat-model';
import { throttle } from '../src/views/inbox';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

function message(seq: number, author: ChatMessage['author'] = 'owner', seat: string | null = null): ChatMessage {
  return { seq, author, session: null, seat, kind: 'message', text: `m${seq}`, card: null, at_ms: seq * 1000 };
}

const RAISE_TO = { id: 'raise_to', label: 'Raise to…', detail: null, takes_amount: true };
const STOP = { id: 'stop', label: 'Stop the task', detail: null, takes_amount: false };

// ---- Labels --------------------------------------------------------------------------------------------------------

eq(cardKindLabel('budget'), 'Budget', 'a budget card reads Budget');
eq(cardKindLabel('tool_call'), 'Tool call', 'an underscore is a space, the first letter capital');
eq(cardKindLabel('start'), 'Start a project', 'a start card reads Start a project');
eq(cardKindLabel('new_kind'), 'New kind', 'a kind not listed reads in sentence case');
eq(chatKindLabel('main'), 'Orchestrator', 'the main chat is the orchestrator');
eq(chatKindLabel('project'), 'Project', 'a project chat is a project');
eq(chatKindLabel('task'), 'Task', 'a task chat is a task');
eq(authorLabel({ author: 'owner', seat: null }), 'You', 'the owner is You');
eq(authorLabel({ author: 'system', seat: null }), 'Plaza', 'the hub speaks as the plaza');
eq(authorLabel({ author: 'session', seat: 'builder' }), 'Builder', 'a session is labelled with its seat');
eq(authorLabel({ author: 'session', seat: null }), 'Session', 'a session without a seat is Session');
eq(isDangerChoice('stop'), true, 'stop is a danger choice');
eq(isDangerChoice('deny'), true, 'deny is a danger choice');
eq(isDangerChoice('raise_to'), false, 'raise_to is not');

// ---- Amounts: the wire integer of a typed amount, in the unit of its owner's budget ------------------------------------

/** The wire integer `amountWire` gives, or null when it refuses the text. */
const wire = (unit: 'usd' | 'tokens' | null | undefined, text: string): number | null => {
  const r = amountWire(unit, text);
  return r.ok ? r.value : null;
};
eq(wire('usd', '2.5'), 2_500_000, '2.5 dollars is 2,500,000 micro-dollars');
eq(wire(null, '2.5'), 2_500_000, 'an owner with no budget is counted in dollars');
eq(wire('usd', '$1.50'), 1_500_000, 'a leading dollar sign is allowed');
eq(wire('usd', '  3 '), 3_000_000, 'the text is trimmed');
eq(wire('usd', '.5'), 500_000, 'a bare fraction is allowed');
eq(wire('usd', '2.'), 2_000_000, 'a trailing point is allowed');
eq(wire('usd', '0.000001'), 1, 'one micro-dollar is the smallest amount');
eq(wire('usd', '1,000'), 1_000_000_000, 'commas group thousands');
eq(wire('usd', '1,000.50'), 1_000_500_000, 'commas group thousands before the decimals');
eq(wire('usd', '0,50'), null, 'a comma that groups no thousands is refused: 0,50');
eq(wire('usd', '2,5'), null, '2,5 is refused');
eq(wire('usd', '5,00'), null, '5,00 is refused');
eq(wire('usd', '1,2,3'), null, '1,2,3 is refused');
eq(wire('usd', ''), null, 'a blank amount is refused');
eq(wire('usd', '   '), null, 'a spaces-only amount is refused');
eq(wire('usd', '$'), null, 'a lone dollar sign is refused');
eq(wire('usd', '0'), null, 'zero is not a positive amount');
eq(wire('usd', '0.00'), null, 'zero with decimals is refused too');
eq(wire('usd', '-2'), null, 'a negative amount is refused');
eq(wire('usd', 'abc'), null, 'a word is refused');
eq(wire('usd', '1e3'), null, 'an exponent is refused');
eq(wire('usd', '1.1234567'), null, 'more than six decimals is refused');
eq(wire('usd', '9999999999999'), null, 'an amount past the safe integers is refused');
eq(wire('tokens', '5'), 5, '5 tokens is 5: a token amount is sent as it is');
eq(wire('tokens', '1,000'), 1000, 'grouped tokens are read');
eq(wire('tokens', ''), null, 'blank tokens are refused');
eq(wire('tokens', '-5'), null, 'negative tokens are refused');
eq(wire('tokens', '2.5'), null, 'decimal tokens are refused');
eq(wire('tokens', '0'), null, 'zero tokens are not positive');
eq(wire('tokens', '5,00'), null, 'a misplaced comma in tokens is refused');
eq(amountWire('tokens', '2.5').ok, false, 'a refusal carries its reason');
eq(wire(undefined, '2.5'), null, 'a card whose budget did not read is refused, whatever its text');
eq(
  amountWire(undefined, '2.5'),
  { ok: false, problem: 'The budget could not be read, so the unit of the amount is not known.' },
  'the refusal says the unit is not known',
);
eq(amountUnitName(undefined), 'unit not known', 'a budget that did not read names no unit');
eq(amountUnitName(null), 'dollars', 'a card with no budget unit is in dollars');
eq(amountUnitName('usd'), 'dollars', 'a usd budget is in dollars');
eq(amountUnitName('tokens'), 'tokens', 'a tokens budget is in tokens');

// ---- Drafts --------------------------------------------------------------------------------------------------------

const drafts = new Drafts();
eq(drafts.get('c1'), '', 'a card with no draft reads empty');
drafts.set('c1', '2.5');
eq(drafts.get('c1'), '2.5', 'a typed draft is kept for its card');
eq(drafts.get('c2'), '', 'a draft belongs to its own card');
drafts.set('c1', '');
eq(drafts.get('c1'), '', 'an emptied draft is dropped');

// ---- Settled cards and answers -------------------------------------------------------------------------------------

const open = { state: 'open' as const, answer: null, choices: [RAISE_TO, STOP] };
eq(settledText(open), '', 'an open card has no settled text; its buttons are the answer');
eq(settledText({ state: 'answered', answer: 'raise_to', choices: [RAISE_TO, STOP] }), 'Answered: Raise to…', 'an answered card names its choice');
eq(settledText({ state: 'answered', answer: 'gone', choices: [RAISE_TO, STOP] }), 'Answered', 'an answer the card does not offer reads Answered');
eq(settledText({ state: 'withdrawn', answer: null, choices: [RAISE_TO, STOP] }), 'Withdrawn', 'a withdrawn card reads Withdrawn');
eq(doneText('Budget raised to $9.00.'), 'Budget raised to $9.00.', 'the done text is shown');
eq(doneText('  '), 'Answered', 'no done text reads Answered');
eq(doneText(''), 'Answered', 'an empty done text reads Answered');
eq(nonBlank('  hello '), 'hello', 'a non-blank text is trimmed');
eq(nonBlank(' \n '), null, 'a blank text is nothing');
eq(isStaleAnswer({ status: 409 }), true, 'a 409 is an answer to a card already answered');
eq(isStaleAnswer({ status: 400 }), false, 'a 400 is not stale');
eq(isStaleAnswer(new Error('network down')), false, 'a network error is not stale');
eq(isStaleAnswer(null), false, 'nothing is not stale');
eq(preselectProject(['pr1', 'pr2'], 'pr2'), 'pr2', 'the scope project is preselected when it is live');
eq(preselectProject(['pr1', 'pr2'], 'pr9'), 'pr1', 'a scope that is not live falls back to the first live project');
eq(preselectProject(['pr1'], null), 'pr1', 'under All projects the first live project is preselected');
eq(preselectProject([], 'pr1'), null, 'no live project preselects nothing');
eq(frameChat({ chat: 'main', opened: true }), 'main', 'an opened chat frame names its chat');
eq(frameChat({ chat: 'ch2', message: { seq: 1 } }), 'ch2', 'a message frame names its chat');
eq(frameChat({ message: {} }), null, 'a frame with no chat names none');
eq(frameChat(null), null, 'no data names no chat');

// ---- Messages ------------------------------------------------------------------------------------------------------

const shuffled = [message(3), message(1), message(2)];
eq(recentMessages(shuffled, 10).map((m) => m.seq), [1, 2, 3], 'messages come oldest first, whatever order they arrive in');
eq(shuffled.map((m) => m.seq), [3, 1, 2], 'the caller’s list is not reordered');
const many = [1, 2, 3, 4, 5].map((seq) => message(seq));
eq(recentMessages(many, 2).map((m) => m.seq), [4, 5], 'the limit keeps the last messages');
eq(recentMessages(many, 9).map((m) => m.seq), [1, 2, 3, 4, 5], 'a limit past the count keeps them all');
eq(recentMessages(many, 0), [], 'a zero limit keeps none');
eq(/^\d{2}:\d{2}$/.test(clockText(Date.UTC(2026, 9, 22, 12, 5))), true, 'a clock time reads HH:MM');

// ---- Expert lines --------------------------------------------------------------------------------------------------

const budget: Pick<Card, 'id' | 'kind' | 'subject' | 'choices'> = {
  id: 'c12',
  kind: 'budget',
  subject: 'task:t3',
  choices: [RAISE_TO, STOP],
};
eq(cardExpertLine(budget), 'c12 · budget · task:t3 · choices raise_to, stop', 'a card’s expert line: id, kind, subject, raw choices');
eq(messageExpertLine({ seq: 4, kind: 'message', author: 'session', session: 's7', card: null }), '#4 · message · session · s7', 'a message’s expert line');
eq(messageExpertLine({ seq: 5, kind: 'question', author: 'system', session: null, card: 'c3' }), '#5 · question · system · card c3', 'a question names its card');

// ---- The throttle: a burst is one refresh, the wait starts with the first call, a call during a load queues one more ----

const wait = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

let burst = 0;
const once = throttle(() => {
  burst++;
});
for (let i = 0; i < 10; i++) once.run();
await wait(350);
eq(burst, 1, 'a burst of calls is one refresh');

let joined = 0;
const waited = throttle(() => {
  joined++;
});
waited.run();
await wait(150);
waited.run(); // during the wait: it joins the refresh already due, the wait is not started again
await wait(200);
eq(joined, 1, 'a call during the wait does not start a second wait');

let requeued = 0;
const again = throttle(() => {
  requeued++;
  if (requeued === 1) again.run(); // a call during a load queues one more refresh
});
again.run();
await wait(650);
eq(requeued, 2, 'a call made during a load queues one more refresh');

let halted = 0;
const dropped = throttle(() => {
  halted++;
});
dropped.run();
dropped.stop();
await wait(350);
eq(halted, 0, 'stop drops a call still waiting');

console.log(`inbox chats tests: ok (${checks} checks)`);
