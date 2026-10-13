// Per-node matrix: which secret each consumer resolves to on each node.
import '../styles/vault-matrix.css';
import type { VaultConsumer, VaultEntry, VaultNodeRef } from '../api';
import { badge, card, empty } from '../ui/components';
import { h } from '../ui/dom';
import { THIS_HUB, bindingLabel, fingerprint, type VaultCtx } from './vault-common';
import { addSecretDialog } from './vault-form';

/** The matrix: one row per consumer binding, one column per node (this hub first). */
export function vaultMatrixSection(ctx: VaultCtx): HTMLElement {
  if (ctx.locked) {
    return h('section.vm', null, h('p.vm-note', null, 'Unseal the vault to see what resolves on each node.'));
  }

  const { consumers, entries, status } = ctx.data;
  if (consumers.length === 0) {
    return h('section.vm', null, empty('link', 'Nothing asks for a secret yet'));
  }

  const hub = status.nodes.find((n) => n.id === THIS_HUB);
  const others = status.nodes.filter((n) => n.id !== THIS_HUB);
  const columns: VaultNodeRef[] = hub ? [hub, ...others] : others;

  const legend = h('ul.vm-legend', { 'aria-label': 'Legend' },
    h('li', null, badge('set', 'ok', true), 'The secret resolves on this node.'),
    h('li', null, badge('missing', 'warn', true), 'Nothing resolves. Select to add the secret.'),
    h('li', null, badge('out of scope', 'muted'), 'The secret exists for other nodes only.'),
  );

  const thead = h('thead', null,
    h('tr', null,
      h('th.vm-corner', { scope: 'col' }, 'Binding'),
      columns.map((n) => h('th', { scope: 'col', title: n.id }, n.id === THIS_HUB ? 'This hub' : n.name)),
    ),
  );

  const rows = consumers.map((c) => h('tr', null,
    h('th.vm-row', { scope: 'row' },
      h('div.vm-binding', null, bindingLabel(c.binding, consumers)),
      h('div.vm-wants', null, c.wants),
    ),
    columns.map((n) => h('td', null, cell(ctx, c, n.id, entries))),
  ));

  const table = h('table.vm-table', null, thead, h('tbody', null, rows));
  return card(null, [legend, h('div.vm-scroll', null, table)]);
}

function cell(ctx: VaultCtx, c: VaultConsumer, nodeId: string, entries: VaultEntry[]): HTMLElement {
  const value: string | undefined = c.per_node[nodeId];

  if (value === 'missing') {
    return h('button.vm-missing', {
      type: 'button',
      title: `Add ${c.wants} for this node`,
      'aria-label': `Add ${c.wants} for this node`,
      onclick: () => addSecretDialog({ name: c.wants, bindings: [c.binding], nodes: [nodeId] }, () => ctx.reload()),
    }, badge('missing', 'warn', true));
  }

  if (value === 'out_of_scope') {
    return h('span.vm-out', {
      title: 'This entry exists for other nodes only. Nothing resolves here.',
    }, badge('out of scope', 'muted'));
  }

  if (value) {
    const entry = entries.find((e) => e.id === value);
    return h('div.vm-set', null,
      badge(entry ? entry.name : 'Unknown entry', 'ok', true),
      entry ? h('div.vm-fp', { title: fingerprint(entry) }, fingerprint(entry)) : null,
    );
  }

  return h('span.vm-none', null, '-');
}
