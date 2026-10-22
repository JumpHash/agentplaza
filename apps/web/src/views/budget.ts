// A project's or a task's budget as a card: what it caps, what is spent, held and left, the set-budget form, the ledger
// and, for a project, its tasks' budgets. The card reloads when the hub says the budget changed, when a session or a card
// of its owner moves, and every POLL_MS while the page shows it (a charge publishes no frame). Its Expert switch shows the
// ids, the micro-dollars and every ledger column. The numbers and words are in budget-model.ts.

import { workApi, type Budget, type BudgetUnit, type LedgerEntry, type ProjectBudget } from '../api-work';
import { href } from '../router';
import { on, RESYNC } from '../store';
import { ago, attempt, badge, button, card, empty, field, input, modal, select, table, toast, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import {
  amountText,
  budgetBody,
  capText,
  dropsCap,
  formOf,
  groupedText,
  isBudgetOf,
  ledgerAmount,
  ledgerLabel,
  monthText,
  namesOwner,
  shareText,
  UNIT_OPTIONS,
  unitLocked,
  unitName,
  type BudgetForm,
  type BudgetOwner,
} from './budget-model';
import { readExpert, switchBox, writeExpert } from './work-expert';

/** The newest ledger entries a card shows. */
const LEDGER_LIMIT = 50;
/** Where the budget card's Expert switch is kept. */
const EXPERT_KEY = 'apz.budget.expert';
/** How long a card waits after a frame of its own before it reloads (see refreshOn). */
const REFRESH_MS = 300;
/** How often a card on the page reloads while the document is visible: a charge publishes no frame. */
const POLL_MS = 15_000;
/** Why the unit cannot change once the budget has spend. */
const UNIT_FIXED = 'Spend is counted in the unit it was set in, so the unit is fixed once there is spend.';

/** A task's budgets in a project's card: the task (a link), what it spent, and its cap in its own unit. */
function taskColumns(): Column<Budget>[] {
  return [
    { label: 'Task', cell: (t) => h('a', { href: href('tasks', t.owner) }, t.owner) },
    { label: 'Spent', num: true, cell: (t) => (t.unit ? amountText(t.unit, t.spent) : '—') },
    { label: 'Cap', num: true, cell: (t) => (t.unit ? capText(t.unit, t.cap) : 'Not set') },
  ];
}

/** A card's element, and the function that stops its refresh (its listeners and timers). */
export interface LiveCard {
  el: HTMLElement;
  dispose: () => void;
}

/**
 * Keeps a card fresh. A frame that `frames` counts as the card's own (by kind, with the test that tells it apart), or a
 * resync, starts a reload REFRESH_MS later unless one is pending already: a burst is one reload, a busy stream cannot
 * hold it off, and a frame that comes during a load queues one more. With `pollMs`, the card also reloads that often
 * while the document is visible. Each handler checks first that the card is still on the page; one that has left it
 * lets go of its listeners and timers and does nothing else. The returned function does the same at once.
 */
export function refreshOn(el: HTMLElement, frames: Record<string, (data: unknown) => boolean>, reload: () => void, pollMs?: number): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let poll: ReturnType<typeof setInterval> | undefined;
  let offs: (() => void)[] = [];
  const dispose = (): void => {
    offs.forEach((off) => off());
    offs = [];
    clearTimeout(timer);
    timer = undefined;
    clearInterval(poll);
    poll = undefined;
  };
  // True while the card is on the page; when it is not, the card is disposed.
  const onPage = (): boolean => {
    if (!el.isConnected) dispose();
    return el.isConnected;
  };
  const run = (): void => {
    timer = undefined;
    if (onPage()) reload();
  };
  const schedule = (): void => {
    timer ??= setTimeout(run, REFRESH_MS);
  };
  if (pollMs !== undefined) {
    poll = setInterval(() => {
      if (onPage() && document.visibilityState === 'visible') schedule();
    }, pollMs);
  }
  offs = [
    ...Object.entries(frames).map(([kind, own]) =>
      on(kind, (data) => {
        if (onPage() && own(data)) schedule();
      }),
    ),
    on(RESYNC, () => {
      if (onPage()) schedule();
    }),
  ];
  return dispose;
}

/** A project's or a task's budget. The header holds the Expert switch and, once a budget is set, the set-budget button. */
export function budgetCard(owner: BudgetOwner): LiveCard {
  const project = owner.kind === 'project';
  let expert = readExpert(EXPERT_KEY);
  let budget: ProjectBudget | undefined;
  let ledger: LedgerEntry[] = [];
  // Why the latest load failed, or null. A failed reload keeps the figures already on screen.
  let failure: string | null = null;
  let seq = 0;
  const acts = h('span');
  const body = h('div');
  const el = card('Budget', body, { actions: [acts] });

  async function load(): Promise<void> {
    // A reload that a newer one overtakes is dropped: only the latest answer is drawn.
    const mine = ++seq;
    try {
      const [b, entries] = await Promise.all([
        project ? workApi.projectBudget(owner.id) : workApi.taskBudget(owner.id),
        project ? workApi.projectLedger(owner.id, LEDGER_LIMIT) : workApi.taskLedger(owner.id, LEDGER_LIMIT),
      ]);
      if (mine !== seq) return;
      budget = b;
      ledger = entries;
      failure = null;
    } catch (err) {
      if (mine !== seq) return;
      failure = err instanceof Error ? err.message : String(err);
    }
    draw();
  }

  function setExpert(checked: boolean): void {
    expert = checked;
    writeExpert(EXPERT_KEY, checked);
    draw();
  }

  function draw(): void {
    replace(acts, switchBox('Expert', expert, setExpert), budget?.unit ? button('Set budget', openForm, { size: 'sm' }) : null);
    replace(body, content());
  }

  /** A project's tasks that have a budget of their own, as a list; none for a task, or when there are none. */
  function tasksList(b: ProjectBudget): Node[] {
    return project && b.tasks?.length ? [h('p.muted', null, 'Tasks'), table(taskColumns(), b.tasks)] : [];
  }

  function content(): Node[] {
    if (!budget) return failure ? [empty('alert', 'The budget could not be loaded', failure)] : [h('p.muted', null, 'Loading the budget…')];
    // A failed reload keeps the figures below and says so in one line above them.
    const alert: Node[] = failure ? [h('p.hint.err', { role: 'alert' }, 'Could not refresh the budget. Showing the last figures.')] : [];
    const unit = budget.unit;
    if (!unit) return [...alert, h('p', null, 'Not set.'), button('Set budget', openForm, { kind: 'primary' }), ...tasksList(budget)];
    return [...alert, ...setBudget(budget, unit)];
  }

  /** The facts of a set budget, then the project's tasks (when it has any) and the ledger. */
  function setBudget(b: ProjectBudget, unit: BudgetUnit): Node[] {
    // An amount; the dense view adds its micro-dollars (a token budget has none).
    const figure = (value: number | null | undefined): (Node | string | null)[] => {
      const text = capText(unit, value);
      if (!expert || unit !== 'usd' || typeof value !== 'number') return [text];
      return [text, ' ', h('span.mono', null, `${groupedText(value)} µ$`)];
    };
    const rows: [string, (Node | string | null)[]][] = [
      ['Unit', [unitName(unit)]],
      ['Cap', figure(b.cap)],
      ['Spent', figure(b.spent)],
      ['In flight', figure(b.held ?? 0)],
    ];
    if (project) rows.push(['Reserved', figure(b.reserved ?? 0)]);
    rows.push(['Left', figure(b.left)], ['Share spent', [shareText(b.cap, b.spent_pct, b.warn_at_pct)]]);
    if (project) {
      rows.push(['Monthly cap', typeof b.monthly_cap === 'number' ? figure(b.monthly_cap) : ['None']]);
      rows.push(['This month', [...figure(b.month_spent ?? 0), typeof b.month === 'number' ? ` in ${monthText(b.month)}` : null]]);
    }
    if (expert) rows.unshift(['Owner', [h('span.mono', null, owner.id)]]);
    const nodes: Node[] = [];
    if (b.paused) nodes.push(badge('Paused: at its cap', 'warn'));
    nodes.push(h('dl.kv', null, rows.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)])));
    nodes.push(...tasksList(b), h('p.muted', null, 'Ledger'), ledgerTable(unit));
    return nodes;
  }

  function ledgerTable(unit: BudgetUnit): Node {
    if (!ledger.length) return h('p.muted', null, 'Nothing recorded yet.');
    const columns: Column<LedgerEntry>[] = [
      { label: 'When', cell: (e) => ago(e.at_ms) },
      { label: 'What', cell: (e) => ledgerLabel(e.kind) },
      { label: 'Amount', num: true, cell: (e) => ledgerAmount(unit, e) },
      { label: 'Session', cell: (e) => (e.session ? h('a', { href: href('sessions', e.session) }, expert ? e.session : 'Open') : '—') },
      { label: 'Model', cell: (e) => e.model ?? '—' },
    ];
    if (expert) {
      columns.push({ label: 'Micro-dollars', num: true, cell: (e) => groupedText(e.micros) }, { label: 'Tokens', num: true, cell: (e) => groupedText(e.tokens) });
    }
    return table(columns, ledger);
  }

  /** The set-budget form, in a sheet: the unit, the cap (blank for none), and for a project the warn share and the monthly cap. */
  function openForm(): void {
    const start: BudgetForm = formOf(budget ?? null);
    // Once there is spend the unit is fixed: the form shows it, and the select cannot change.
    const locked = budget ? unitLocked(budget) : false;
    const unit = select(UNIT_OPTIONS, start.unit);
    unit.disabled = locked;
    const cap = input({ type: 'text', inputmode: 'decimal', value: start.cap, 'aria-label': 'Cap' });
    const warn = input({ type: 'text', inputmode: 'numeric', value: start.warnPct, 'aria-label': 'Warn at, percent' });
    const monthly = input({ type: 'text', inputmode: 'decimal', value: start.monthlyCap, 'aria-label': 'Monthly cap' });
    const hint = (u: BudgetUnit): void => {
      cap.placeholder = u === 'usd' ? 'For example 25 or 2.50' : 'For example 500000';
      monthly.placeholder = cap.placeholder;
    };
    hint(start.unit);
    unit.addEventListener('change', () => {
      // The caps were in the old unit, so they are cleared rather than converted.
      cap.value = '';
      monthly.value = '';
      hint(unit.value === 'tokens' ? 'tokens' : 'usd');
    });
    const fields: Node[] = [field('Unit', unit, locked ? UNIT_FIXED : undefined), field('Cap', cap, 'Blank for no cap.')];
    if (project) {
      fields.push(
        field('Warn at, percent', warn, 'You are told when the spend reaches this share of the cap.'),
        field('Monthly cap', monthly, 'For its long-lived sessions in a month. Blank for none.'),
      );
    }
    const submit = async (): Promise<void> => {
      const chosen: BudgetUnit = unit.value === 'tokens' ? 'tokens' : 'usd';
      // Once there is spend, the unit a save names must be the one the budget has.
      if (budget?.unit && budget.unit !== chosen && unitLocked(budget)) {
        toast(UNIT_FIXED, 'error');
        return;
      }
      // A unit change blanks the caps, so a cap the budget has (its monthly cap, for a project, too) is typed again.
      const monthlyStored = budget && project ? { unit: budget.unit, cap: budget.monthly_cap ?? null } : null;
      if (budget && (dropsCap(budget, chosen, cap.value) || dropsCap(monthlyStored, chosen, monthly.value))) {
        toast('Type the cap again in the new unit, or keep the unit.', 'error');
        return;
      }
      const parsed = budgetBody(owner.kind, { unit: chosen, cap: cap.value, warnPct: warn.value, monthlyCap: monthly.value });
      if (!parsed.ok) {
        toast(parsed.problem, 'error');
        return;
      }
      const next = parsed.value;
      save.disabled = true;
      const saved = await attempt(() => (project ? workApi.setProjectBudget(owner.id, next) : workApi.setTaskBudget(owner.id, next)), 'Budget saved');
      if (!saved) {
        save.disabled = false;
        return;
      }
      close();
      void load();
    };
    const save = button('Save', () => void submit(), { kind: 'primary' });
    const close = modal('Set budget', h('div', null, fields), undefined, [button('Cancel', () => close(), { kind: 'ghost' }), save]);
  }

  const dispose = refreshOn(
    el,
    {
      budget: (data) => isBudgetOf(data, owner),
      session: (data) => namesOwner(data, owner),
      card: (data) => namesOwner(data, owner),
    },
    () => void load(),
    POLL_MS,
  );
  draw();
  void load();
  return { el, dispose };
}
