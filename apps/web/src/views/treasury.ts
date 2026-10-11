// The treasury: the ledger's totals and every account's standing.

import '../styles/ops.css';
import { api, type Account } from '../api';
import { on } from '../store';
import { badge, card, empty, stat, table, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

const money = (n: number): string => n.toLocaleString('en-US');

const columns: Column<Account>[] = [
  { label: 'ID', cell: (a) => `#${a.id}` },
  { label: 'Kind', cell: (a) => badge(a.kind, 'info') },
  { label: 'Owner', cell: (a) => a.owner_ref },
  { label: 'Balance', num: true, cell: (a) => h('span', { class: a.balance < 0 ? 'danger-text' : '' }, money(a.balance)) },
  { label: 'Credit limit', num: true, cell: (a) => money(a.credit_limit) },
];

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  const body = h('div.stack');
  root.append(h('div.view-head', null, h('h1', null, 'Treasury')), body);

  let alive = true;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const refresh = async (): Promise<void> => {
    try {
      const t = await api.treasury();
      if (!alive) return;
      replace(
        body,
        h(
          'div.stack',
          null,
          h(
            'div.grid.cols-4',
            null,
            stat('Accounts', t.totals.accounts),
            stat('Entries', t.totals.entries),
            stat('Balance', money(t.totals.balance)),
            stat('Spent', money(t.spent)),
          ),
          t.note ? h('p.faint', null, t.note) : null,
        ),
        card('Accounts', t.accounts.length ? table(columns, t.accounts) : empty('treasury', 'No accounts.'), { flush: true }),
      );
    } catch (err) {
      if (alive) replace(body, empty('alert', 'Treasury unreachable', err instanceof Error ? err.message : String(err)));
    }
  };

  const schedule = (): void => {
    if (timer !== undefined) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = undefined;
      void refresh();
    }, 300);
  };
  const off = on('treasury', schedule);
  void refresh();
  return () => {
    alive = false;
    off();
    if (timer !== undefined) clearTimeout(timer);
  };
}
