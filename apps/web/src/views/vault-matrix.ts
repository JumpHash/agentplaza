// Per-node matrix: which secret each consumer resolves to on each node.
import type { VaultConsumer, VaultEntry, VaultNodeRef } from '../api';
import { badge, card, empty } from '../ui/components';
import { h } from '../ui/dom';
import { THIS_HUB, bindingLabel, fingerprint, type VaultCtx } from './vault-common';
import { addSecretDialog } from './vault-form';
import { dim, nodeHeading, tableWrap } from './vault-ui';

/** The matrix: one row per consumer binding, one column per node (this hub first). */
export function vaultMatrixSection(ctx: VaultCtx): HTMLElement {
  if (ctx.locked) {
    return card('', dim('Unseal the vault to see what resolves on each node.'));
  }

  const { consumers, entries, status } = ctx.data;
  if (consumers.length === 0) {
    return card('', empty('link', 'Nothing asks for a secret yet', ''));
  }

  const hub = status.nodes.find((n) => n.id === THIS_HUB);
  const others = status.nodes.filter((n) => n.id !== THIS_HUB);
  const columns: VaultNodeRef[] = hub ? [hub, ...others] : others;

  const legend = h(
    'ul.vlt-legend',
    { 'aria-label': 'Legend' },
    h('li', null, badge('set', 'ok'), ' The secret resolves on this node.'),
    h('li', null, badge('missing', 'warn'), ' Nothing resolves. Select to add the secret.'),
    h('li', null, badge('out of scope', 'muted'), ' The secret exists for other nodes only.'),
  );

  const head = h('tr', null, h('th', null, 'Binding'), columns.map((n) => h('th', null, nodeHeading(n.id, n.name))));
  const rows = consumers.map((c) =>
    h(
      'tr',
      null,
      h('td', null, h('strong', null, bindingLabel(c.binding, consumers)), h('div.vlt-sub.mono', null, c.wants)),
      columns.map((n) => h('td', null, cell(ctx, c, n.id, entries))),
    ),
  );
  const table = h('table.tbl', null, h('thead', null, head), h('tbody', null, rows));
  return card('', [legend, tableWrap(table)], { flush: true });
}

function cell(ctx: VaultCtx, c: VaultConsumer, nodeId: string, entries: VaultEntry[]): Node {
  const value: string | undefined = c.per_node[nodeId];

  if (value === 'missing') {
    return h(
      'button.vlt-miss',
      {
        type: 'button',
        title: `Add ${c.wants} for this node`,
        'aria-label': `Add ${c.wants} for this node`,
        onclick: () => addSecretDialog({ name: c.wants, bindings: [c.binding], nodes: [nodeId] }, () => ctx.reload()),
      },
      badge('missing', 'warn'),
    );
  }

  if (value === 'out_of_scope') {
    return h('span', { title: 'This entry exists for other nodes only. Nothing resolves here.' }, badge('out of scope', 'muted'));
  }

  if (value) {
    const entry = entries.find((e) => e.id === value);
    return h(
      'span',
      null,
      badge(entry ? entry.name : 'Unknown entry', entry ? 'ok' : 'muted'),
      entry ? h('div.vlt-sub.mono', null, fingerprint(entry).slice(0, 14)) : null,
    );
  }

  return document.createTextNode('-');
}
