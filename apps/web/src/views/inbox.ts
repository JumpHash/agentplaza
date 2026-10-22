// Inbox (`#/inbox`): the cards waiting for the owner. Each is a decision the plaza needs before it goes on: its kind, its
// title and body as plain text, the project, task, session and chat it is about (links), and its choices as buttons; a
// choice that takes an amount has a field before its button, in the unit of the owner's budget. Reloads on `card` frames
// and on RESYNC, throttled.
// Expert adds each card's id, raw kind and subject, and the raw ids of its choices.
// The answering (Answers) and a card's buttons are shared with the chat view, which imports them from here.

import { workApi, type BudgetUnit, type Card, type CardChoice } from '../api-work';
import { href } from '../router';
import { projectTitle, projects } from '../scope';
import { on, RESYNC } from '../store';
import { ago, badge, button, card, empty, input, toast, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { amountUnitName, amountWire, cardExpertLine, cardKindLabel, Drafts, doneText, isDangerChoice, isStaleAnswer } from './chat-model';
import type { Cleanup } from './index';
import { readExpert, switchBox, writeExpert } from './work-expert';

/** How long a view waits after a frame before it reloads: a burst of frames is one reload, and a busy stream never holds it off. */
const THROTTLE_MS = 300;
const INBOX_EXPERT = 'apz.inbox.expert';

/** A card kind's tone: an incident is danger; the kinds that spend, grant or release something are warnings. */
export function kindTone(kind: string): Tone {
  if (kind === 'incident') return 'danger';
  return ['budget', 'allowance', 'disk', 'tool_call', 'deploy', 'release'].includes(kind) ? 'warn' : 'muted';
}

/** Agent-written text as plain text: its line breaks kept, nothing parsed. */
export function prose(text: string): HTMLElement {
  return h('p', { style: 'margin: 0; white-space: pre-line; overflow-wrap: anywhere' }, text);
}

/**
 * `fn` at most once per THROTTLE_MS: a call starts the wait only when none is pending, so a busy stream cannot hold the
 * refresh off, and a call during the load queues one more. `stop` drops a call still waiting.
 */
export function throttle(fn: () => void): { run: () => void; stop: () => void } {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return {
    run: () => {
      timer ??= setTimeout(() => {
        timer = undefined;
        fn();
      }, THROTTLE_MS);
    },
    stop: () => {
      clearTimeout(timer);
      timer = undefined;
    },
  };
}

/**
 * The owner's answers to cards, for one view: the dollar amounts typed (kept while the view redraws), the answers in
 * flight (their buttons are disabled meanwhile), and what the view does when an answer lands or the hub has moved on.
 */
export class Answers {
  readonly drafts = new Drafts();
  /**
   * The unit each card's amount is counted in (its owner's budget unit; null for dollars), as the view last loaded it.
   * A card whose budget did not read has no entry.
   */
  units = new Map<string, BudgetUnit | null>();
  private inFlight = new Set<string>();

  constructor(
    private readonly redraw: () => void,
    private readonly landed: (answered: Card) => void,
    private readonly stale: () => void,
  ) {}

  busy(id: string): boolean {
    return this.inFlight.has(id);
  }

  /** Sends the owner's choice. A choice that takes an amount sends the typed one, as the wire takes it in the card's unit. */
  async choose(c: Card, choice: CardChoice): Promise<void> {
    let amount: number | undefined;
    if (choice.takes_amount) {
      const wire = amountWire(this.units.get(c.id), this.drafts.get(c.id));
      if (!wire.ok) {
        toast(wire.problem, 'error');
        return;
      }
      amount = wire.value;
    }
    this.inFlight.add(c.id);
    this.redraw();
    try {
      const reply = await workApi.answerCard(c.id, choice.id, amount);
      this.drafts.set(c.id, '');
      this.landed(reply.card);
      toast(doneText(reply.done));
    } catch (err) {
      if (isStaleAnswer(err)) {
        toast('That card is already answered or withdrawn.', 'error');
        this.stale();
      } else {
        toast(err instanceof Error ? err.message : String(err), 'error');
      }
    } finally {
      this.inFlight.delete(c.id);
      this.redraw();
    }
  }
}

/** The card's choices as buttons: the first is primary, a stop-like choice is danger. An amount field comes before the
 * first choice that takes an amount. */
export function choiceButtons(c: Card, answers: Answers): HTMLElement {
  const amountAt = c.choices.findIndex((x) => x.takes_amount);
  const busy = answers.busy(c.id);
  return h(
    'div.row',
    { style: 'flex-wrap: wrap' },
    c.choices.flatMap((choice, i) => {
      const b = button(choice.label, () => void answers.choose(c, choice), {
        kind: isDangerChoice(choice.id) ? 'danger' : i === 0 ? 'primary' : undefined,
        title: choice.detail ?? undefined,
        disabled: busy,
      });
      return i === amountAt ? [amountField(c, answers, busy), b] : [b];
    }),
  );
}

/** The amount a choice takes: a field in the card's unit (dollars, or tokens), with that unit named beside it. */
function amountField(c: Card, answers: Answers, busy: boolean): HTMLElement {
  const unit = answers.units.get(c.id);
  const name = amountUnitName(unit);
  const field = input({
    inputmode: unit === 'tokens' ? 'numeric' : 'decimal',
    placeholder: unit === undefined ? 'Amount' : unit === 'tokens' ? '0' : '$0.00',
    'aria-label': `Amount, ${name}`,
    autocomplete: 'off',
    disabled: busy,
    style: 'width: 9rem; flex: none',
  });
  field.value = answers.drafts.get(c.id);
  field.addEventListener('input', () => answers.drafts.set(c.id, field.value));
  return h('span.row', { style: 'flex: none' }, field, h('small.muted', null, name));
}

/** The nodes of a line, with a middle dot between each pair. */
function dotted(nodes: Node[]): (Node | string)[] {
  return nodes.flatMap((node, i) => (i === 0 ? [node] : [' · ', node]));
}

function fact(label: string, text: string, to: string): HTMLElement {
  return h('span', null, `${label} `, h('a', { href: to }, text));
}

/** A link with its text, and the id it points at after it (the expert view only). */
function linkFact(text: string, to: string, id: string | null): HTMLElement {
  return h('span', null, h('a', { href: to }, text), id ? ` (${id})` : null);
}

/** The facts a card is about, as links: its project, task, session and chat (when set), and when it was asked. */
function factsLine(c: Card, expert: boolean): HTMLElement {
  const facts: Node[] = [];
  if (c.project) facts.push(fact('Project', projectTitle(c.project), href('projects', c.project)));
  if (c.task) facts.push(linkFact('Open task', href('tasks', c.task), expert ? c.task : null));
  if (c.session) facts.push(linkFact('Open session', href('sessions', c.session), expert ? c.session : null));
  if (c.chat) facts.push(h('a', { href: href('chats', c.chat) }, 'Open the chat'));
  facts.push(h('span', null, `Created ${ago(c.created_ms)}`));
  return h('p.muted', { style: 'margin: 0; font-size: 13px' }, dotted(facts));
}

/** One card: its kind, title, body, facts and choices. Expert adds its raw line. */
function cardView(c: Card, answers: Answers, expert: boolean): HTMLElement {
  return card(
    badge(cardKindLabel(c.kind), kindTone(c.kind)),
    h(
      'div.stack',
      null,
      h('b', null, c.title),
      c.body ? prose(c.body) : null,
      factsLine(c, expert),
      expert ? h('code', { style: 'overflow-wrap: anywhere' }, cardExpertLine(c)) : null,
      choiceButtons(c, answers),
    ),
  );
}

/**
 * The unit each card's amount is counted in: the budget of its task, else of its project; null (dollars) when that has no
 * unit or the card has neither. Only the cards with a choice that takes an amount are read, each on its own: a card whose
 * read fails gets no entry, so its amount is refused, and the other cards still load.
 */
export async function amountUnits(cards: readonly Card[]): Promise<Map<string, BudgetUnit | null>> {
  const units = new Map<string, BudgetUnit | null>();
  await Promise.all(
    cards
      .filter((c) => c.choices.some((x) => x.takes_amount))
      .map(async (c) => {
        try {
          const budget = c.task ? await workApi.taskBudget(c.task) : c.project ? await workApi.projectBudget(c.project) : null;
          units.set(c.id, budget?.unit ?? null);
        } catch {
          // No entry: the card's amount is refused until its budget reads.
        }
      }),
  );
  return units;
}

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  let cards: Card[] = [];
  let loaded = false;
  let failed = false;
  let expert = readExpert(INBOX_EXPERT);
  let alive = true;
  let seq = 0;
  const head = h('div.view-head');
  const list = h('div.stack');
  root.append(head, list);

  const draw = (): void => {
    replace(
      head,
      h('div', null, h('h1', null, 'Inbox'), h('p.muted', null, loaded && cards.length ? `${cards.length} waiting for you` : null)),
      h(
        'div.actions',
        null,
        switchBox('Expert', expert, (checked) => {
          expert = checked;
          writeExpert(INBOX_EXPERT, checked);
          draw();
        }),
      ),
    );
    if (!loaded) replace(list);
    else if (failed) replace(list, h('p.muted', { role: 'alert' }, 'Could not load the inbox.'));
    else if (!cards.length) replace(list, empty('inbox', 'Nothing waits for you', 'Decisions the plaza needs from you appear here.'));
    else replace(list, cards.map((c) => cardView(c, answers, expert)));
  };
  const redraw = (): void => {
    if (alive) draw();
  };

  const answers = new Answers(
    redraw,
    (answered) => {
      cards = cards.filter((c) => c.id !== answered.id);
    },
    () => void load(),
  );

  // Each load takes a number; a load that a newer one has overtaken writes nothing.
  const load = async (): Promise<void> => {
    const mine = ++seq;
    try {
      const list = await workApi.cards(true);
      if (mine !== seq) return;
      const units = await amountUnits(list);
      if (mine !== seq) return;
      answers.units = units;
      cards = list;
      failed = false;
    } catch {
      if (mine !== seq) return;
      failed = true;
    }
    loaded = true;
    redraw();
  };

  const later = throttle(() => void load());
  const offs: (() => unknown)[] = [on('card', later.run), on(RESYNC, later.run), projects.watch(redraw)];
  void load();

  return () => {
    alive = false;
    later.stop();
    offs.forEach((off) => off());
  };
}
