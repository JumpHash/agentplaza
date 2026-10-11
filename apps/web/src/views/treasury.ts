// The treasury: the plaza budget, where it sits, where it went, every wallet
// and the journal. The owner mints and fines here; everything else is read.

import '../styles/ops.css';
import { api, type Agent, type JournalEntry, type Treasury, type Wallet } from '../api';
import { on } from '../store';
import { ago, badge, button, card, empty, field, input, modal, select, stat, table, toast, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

const money = (n: number): string => n.toLocaleString('en-US');
const msg = (err: unknown): string => (err instanceof Error ? err.message : String(err));

const KIND_TONE: Record<string, Tone> = {
  mint: 'ok',
  spend: 'warn',
  payment: 'accent',
  refund: 'info',
  transfer: 'info',
  fine: 'danger',
};

// A balanced entry carries its debit legs as positive amounts; the entry's size is their sum.
const amountOf = (e: JournalEntry): number => e.postings.reduce((s, p) => (p.amount > 0 ? s + p.amount : s), 0);
const sumKind = (entries: JournalEntry[], kind: string): number =>
  entries.filter((e) => e.kind === kind).reduce((s, e) => s + amountOf(e), 0);

// The flow: budget -> escrow -> spend (providers, compute) or payment (agents' wallets).
const flow = (budget: number, escrow: number, spent: number, paid: number, wallets: number): HTMLElement => {
  const box = (x: number, y: number, label: string, value: number): string =>
    `<rect class="tr-box" x="${x}" y="${y}" width="170" height="70"/>` +
    `<text class="tr-label" x="${x + 12}" y="${y + 22}">${label}</text>` +
    `<text class="tr-fig" x="${x + 12}" y="${y + 54}">${money(value)}</text>`;
  const line = (x1: number, y1: number, x2: number, y2: number): string =>
    `<line class="tr-line" x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}" marker-end="url(#tr-arrow)"/>`;
  const svg =
    `<svg class="tr-flow" viewBox="0 0 830 180" role="img" aria-label="Where the plaza budget goes">` +
    `<defs><marker id="tr-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto">` +
    `<path class="tr-head" d="M0 0L10 5L0 10z"/></marker></defs>` +
    box(0, 55, 'Budget', budget) +
    box(220, 55, 'Escrow', escrow) +
    box(440, 10, 'Spend', spent) +
    box(440, 100, 'Payment', paid) +
    box(660, 55, 'Wallets', wallets) +
    line(170, 90, 220, 90) +
    line(390, 90, 440, 45) +
    line(390, 90, 440, 135) +
    line(610, 135, 660, 90) +
    '</svg>';
  const wrap = h('div.tr-flow-wrap');
  wrap.innerHTML = svg;
  return wrap;
};

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  const body = h('div.stack');

  let alive = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let kindFilter = '';
  let data: { t: Treasury; journal: JournalEntry[]; wallets: Wallet[]; roster: Agent[] } | null = null;

  const openMint = (): void => {
    const amount = input({ type: 'number', min: 1, step: 1, required: true });
    let close = (): void => {};
    const go = button('Mint', () => {
      const n = Number(amount.value);
      if (!Number.isInteger(n) || n <= 0) {
        toast('Enter a whole amount above zero.', 'error');
        return;
      }
      go.disabled = true;
      api
        .mintBudget(n)
        .then(() => {
          toast(`Minted ${money(n)} into the plaza budget.`);
          close();
          void refresh();
        })
        .catch((err: unknown) => {
          go.disabled = false;
          toast(msg(err), 'error');
        });
    });
    go.classList.add('primary');
    close = modal(
      'Mint budget',
      h('div.stack', null, field('Amount', amount, 'Whole units added to the plaza budget.'), h('div.row', null, go)),
    );
  };

  const openFine = (): void => {
    const roster = data?.roster ?? [];
    if (roster.length === 0) {
      toast('There are no agents to fine.', 'error');
      return;
    }
    const who = select(roster.map((a) => ({ value: String(a.cid), label: a.name })));
    const amount = input({ type: 'number', min: 1, step: 1, required: true });
    const memo = input({ type: 'text', required: true, placeholder: 'What for' });
    let close = (): void => {};
    const go = button('Fine', () => {
      const n = Number(amount.value);
      const text = memo.value.trim();
      if (!Number.isInteger(n) || n <= 0) {
        toast('Enter a whole amount above zero.', 'error');
        return;
      }
      if (!text) {
        toast('A fine needs a memo.', 'error');
        return;
      }
      go.disabled = true;
      api
        .fineAgent(Number(who.value), n, text)
        .then(() => {
          toast(`Fined ${money(n)}.`);
          close();
          void refresh();
        })
        .catch((err: unknown) => {
          go.disabled = false;
          toast(msg(err), 'error');
        });
    });
    go.classList.add('danger');
    close = modal(
      'Fine an agent',
      h(
        'div.stack',
        null,
        field('Agent', who),
        field('Amount', amount, 'Taken from the agent’s wallet.'),
        field('Memo', memo, 'Shown in the journal.'),
        h('div.row', null, go),
      ),
    );
  };

  const fineBtn = button('Fine an agent', () => openFine());
  const mintBtn = button('Mint budget', () => openMint());
  mintBtn.classList.add('primary');
  root.append(h('div.view-head', null, h('h1', null, 'Treasury'), h('div.actions', null, fineBtn, mintBtn)), body);

  const render = (): void => {
    if (!data) return;
    const { t, journal, wallets, roster } = data;
    const roles = new Map(roster.map((a) => [a.cid, a.role_label] as const));
    const escrow = t.accounts.filter((a) => a.kind === 'escrow').reduce((s, a) => s + a.balance, 0);
    const paid = sumKind(journal, 'payment');
    const walletTotal = wallets.reduce((s, w) => s + w.balance, 0);
    const kinds = [...new Set(journal.map((e) => e.kind))].sort();
    const shown = journal.filter((e) => !kindFilter || e.kind === kindFilter).slice(0, 50);

    const columns: Column<Wallet>[] = [
      { label: 'Agent', cell: (w) => h('a', { href: `#/agents/${w.cid}` }, w.name) },
      { label: 'Role', cell: (w) => roles.get(w.cid) ?? '—' },
      { label: 'Balance', num: true, cell: (w) => money(w.balance) },
    ];
    const walletRows = wallets.slice().sort((a, b) => b.balance - a.balance);

    const filter = select(
      [{ value: '', label: 'All kinds' }, ...kinds.map((k) => ({ value: k, label: k }))],
      kindFilter,
    );
    filter.addEventListener('change', () => {
      kindFilter = filter.value;
      render();
    });

    const rows = shown.map((e) =>
      h(
        'div.tr-row',
        null,
        h('span.tr-n', null, `#${e.id}`),
        h('span.tr-at', null, ago(e.at)),
        badge(e.kind, KIND_TONE[e.kind] ?? 'muted'),
        h('span.tr-memo', null, e.memo || `${e.kind}${e.task ? ` for ${e.task}` : ''}`),
        h('span.tr-amt', null, money(amountOf(e))),
      ),
    );

    replace(
      body,
      h(
        'div.grid.tr-tallies',
        null,
        stat('Plaza budget', money(t.plaza_budget)),
        stat('In escrow', money(escrow), 'held for open tasks'),
        stat('Spent', money(t.spent), 'to providers and compute'),
        stat('Paid to agents', money(paid), 'margins for verified work'),
        stat('Agents’ wallets', money(walletTotal)),
      ),
      card('Flow', flow(t.plaza_budget, escrow, t.spent, paid, walletTotal)),
      card(
        'Wallets',
        wallets.length ? table(columns, walletRows) : empty('user', 'No wallets yet.'),
        { flush: true },
      ),
      card(
        'Journal',
        shown.length ? h('div.tr-ledger', null, ...rows) : empty('clock', 'No entries.'),
        { flush: true, actions: [filter] },
      ),
    );
  };

  const refresh = async (): Promise<void> => {
    try {
      const [t, journal, wallets, roster] = await Promise.all([
        api.treasury(),
        api.treasuryJournal(),
        api.treasuryWallets(),
        api.agents(),
      ]);
      if (!alive) return;
      data = { t, journal: journal.slice().sort((a, b) => b.at - a.at), wallets, roster };
      render();
    } catch (err) {
      if (alive) replace(body, empty('alert', 'Treasury unreachable', msg(err)));
    }
  };

  const schedule = (): void => {
    if (timer !== undefined) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = undefined;
      void refresh();
    }, 400);
  };
  const off = on('treasury', schedule);
  void refresh();
  return () => {
    alive = false;
    off();
    if (timer !== undefined) clearTimeout(timer);
  };
}
