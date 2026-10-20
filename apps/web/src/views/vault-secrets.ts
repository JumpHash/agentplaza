// The Secrets tab: one row per vault entry, with the row actions (rotate, roll back, edit, test, delete).
// No secret value is ever shown here. The only value input is the one in the rotate dialog, cleared on read.
import { h } from '../ui/dom';
import type { IconName } from '../ui/icons.gen';
import { attempt, badge, button, card, clip, empty, field, input, select, toast } from '../ui/components';
import { type VaultConsumer, type VaultEntry, type VaultNodeRef, type VaultScope, vaultApi } from '../api';
import { THIS_HUB, type VaultCtx, bindingLabel, errorText, fingerprint, lastFour, nodeName, scopeNodeIds } from './vault-common';
import { agoText, check, dialog, dim, errLine, fieldset, nodeHeading, radio, tableWrap } from './vault-ui';

/** Which secrets the list shows: all of them, the plaza-wide ones, or those scoped to one node. */
export type SecretScope = { kind: 'all' } | { kind: 'plaza' } | { kind: 'node'; id: string };

/** The desktop's rule: a node shows only the entries whose scope lists it; plaza entries are not bound to one node. */
function inScope(entry: VaultEntry, scope: SecretScope): boolean {
  switch (scope.kind) {
    case 'all':
      return true;
    case 'plaza':
      return entry.scope.kind === 'plaza';
    case 'node':
      return entry.scope.kind === 'nodes' && entry.scope.nodes.includes(scope.id);
  }
}

function scopeValue(scope: SecretScope): string {
  return scope.kind === 'node' ? `node:${scope.id}` : scope.kind;
}

function scopeFromValue(value: string): SecretScope {
  if (value.startsWith('node:')) return { kind: 'node', id: value.slice('node:'.length) };
  return value === 'plaza' ? { kind: 'plaza' } : { kind: 'all' };
}

/** The scope select above the list. Its node choices are the vault's own nodes, the same ones the scope picker offers. */
function scopeSelect(nodes: VaultNodeRef[], scope: SecretScope, onScope: (next: SecretScope) => void): HTMLElement {
  const options = [
    { value: 'all', label: 'All' },
    { value: 'plaza', label: 'Plaza' },
    ...nodes.map((n) => ({ value: `node:${n.id}`, label: nodeName(n.id, nodes) })),
  ];
  const control = select(options, scopeValue(scope));
  control.setAttribute('aria-label', 'Scope');
  control.addEventListener('change', () => onScope(scopeFromValue(control.value)));
  return field('Scope', control);
}

/** The Secrets tab: the scope select, then the list (or the empty state) in a flush card. */
export function vaultSecretsSection(ctx: VaultCtx, scope: SecretScope, onScope: (next: SecretScope) => void): Node[] {
  if (ctx.locked) {
    return [card('Secrets', empty('key', 'Vault is locked', 'Unseal the vault to see and manage secrets.'))];
  }
  const { entries, consumers, status } = ctx.data;
  if (entries.length === 0) {
    return [card('Secrets', empty('key', 'No secrets yet', 'Add one with Add secret, or import a .env file.'))];
  }
  const shown = entries.filter((e) => inScope(e, scope));
  const filter = scopeSelect(status.nodes, scope, onScope);
  if (shown.length === 0) {
    return [filter, card('Secrets', empty('key', 'No secrets in this scope', 'Pick another scope above.'), { flush: true })];
  }

  const columns: { label: string; cls?: string; cell: (e: VaultEntry) => Node | string }[] = [
    {
      label: 'Name',
      cell: (e) => h('div', null, h('div.vlt-name', null, e.name.replace(/_/g, '_​')), e.label ? h('div.vlt-sub', null, e.label) : null),
    },
    { label: 'Kind', cell: (e) => h('span.mono', null, e.kind) },
    { label: 'Scope', cell: (e) => scopeChips(e.scope, status.nodes) },
    { label: 'Bindings', cell: (e) => bindingChips(e.bindings, consumers) },
    { label: 'Fingerprint', cell: (e) => h('span.mono', { title: fingerprint(e) }, clip(fingerprint(e), 14)) },
    { label: 'Last 4', cell: (e) => h('span.mono', null, lastFour(e)) },
    { label: 'Version', cell: (e) => h('span.mono', null, `v${e.current}`) },
    { label: 'Last used', cell: (e) => (e.last_used_ms ? agoText(e.last_used_ms) : 'never') },
    { label: 'Uses', cls: 'num', cell: (e) => String(e.use_count) },
    { label: 'Actions', cell: (e) => rowActions(ctx, e) },
  ];

  const head = h('tr', null, columns.map((c) => h('th', { class: c.cls ?? null }, c.label)));
  const rows = shown.map((e) =>
    h('tr', null, columns.map((c) => h('td', { 'data-label': c.label, class: c.cls ?? null }, c.cell(e)))),
  );
  const table = h('table.tbl.vs-table', null, h('thead', null, head), h('tbody', null, rows));
  return [filter, card('Secrets', tableWrap(table), { flush: true })];
}

function scopeChips(scope: VaultScope, nodes: VaultNodeRef[]): HTMLElement {
  if (scope.kind === 'plaza') return badge('plaza', 'accent');
  return h('div.vlt-chips', null, ...scope.nodes.map((id) => badge(nodeName(id, nodes), 'info')));
}

function bindingChips(bindings: string[], consumers: VaultConsumer[]): HTMLElement {
  if (bindings.length === 0) return h('span.vlt-sub', null, 'none');
  return h('div.vlt-chips', null, ...bindings.map((b) => badge(bindingLabel(b, consumers), 'muted')));
}

function rowActions(ctx: VaultCtx, e: VaultEntry): HTMLElement {
  return h(
    'span',
    null,
    rowButton('Rotate value', 'refresh', () => rotateDialog(ctx, e), ctx.locked),
    rowButton('Rollback', 'clock', () => rollbackDialog(ctx, e), ctx.locked),
    rowButton('Edit scope and bindings', 'edit', () => editDialog(ctx, e), ctx.locked),
    rowButton('Test', 'play', () => testDialog(ctx, e), ctx.locked),
    rowButton('Delete', 'trash', () => deleteDialog(ctx, e), ctx.locked, true),
  );
}

function rowButton(title: string, iconName: IconName, onClick: () => void, disabled: boolean, danger = false): HTMLButtonElement {
  return button('', () => onClick(), { kind: danger ? 'danger' : 'ghost', size: 'sm', icon: iconName, title, disabled });
}

// Rotate: one password input, never pre-filled. The value is cleared as soon as it is read.
function rotateDialog(ctx: VaultCtx, e: VaultEntry): void {
  const value = input({ type: 'password', autocomplete: 'off', spellcheck: 'false', 'aria-label': 'New value' });
  const err = errLine();
  let close: () => void = () => {};

  const save = async () => {
    let secret: string | null = value.value;
    value.value = '';
    if (!secret) {
      err.textContent = 'Enter the new value.';
      return;
    }
    try {
      const updated = await vaultApi.rotateEntry(e.id, secret);
      close();
      toast(`${e.name} is now v${updated.current}`);
      await ctx.reload();
    } catch (error) {
      err.textContent = errorText(error);
    } finally {
      secret = null;
    }
  };

  close = dialog(
    `Rotate ${e.name}`,
    [field('New value', value, 'Never shown again. Each change is a new version.'), err],
    [button('Cancel', () => close(), { kind: 'ghost' }), button('Rotate value', () => void save(), { kind: 'primary' })],
    () => {
      value.value = '';
    },
  );
}

// Rollback: pick one of the non-current versions, then confirm.
function rollbackDialog(ctx: VaultCtx, e: VaultEntry): void {
  const older = e.versions.filter((ver) => ver.v !== e.current).sort((a, b) => b.v - a.v);
  if (older.length === 0) {
    dialog(`Roll back ${e.name}`, [empty('clock', 'No earlier version', 'Only the current version exists.')]);
    return;
  }

  let picked: number | null = null;
  const err = errLine();
  let close: () => void = () => {};
  const confirm = button('Roll back', () => void run(), { kind: 'primary', disabled: true });
  const group = `vlt-rb-${e.id}`;

  const rows = older.map((ver) => {
    const r = radio(group, String(ver.v), false, h('span.mono', null, `v${ver.v} · ${agoText(ver.created_ms)} · ${ver.fp.slice(0, 14)} ${ver.hint}`));
    r.input.addEventListener('change', () => {
      picked = ver.v;
      confirm.disabled = false;
    });
    return r.row;
  });

  const run = async () => {
    if (picked === null) return;
    const v = picked;
    const updated = await attempt(() => vaultApi.rollbackEntry(e.id, v), `${e.name} is back on v${v}`);
    if (updated) {
      close();
      await ctx.reload();
    }
  };

  close = dialog(
    `Roll back ${e.name}`,
    [dim('Pick an earlier version. It becomes the current value.'), ...rows, err],
    [button('Cancel', () => close(), { kind: 'ghost' }), confirm],
  );
}

// Edit: label, scope (plaza or specific nodes) and bindings.
function editDialog(ctx: VaultCtx, e: VaultEntry): void {
  const { nodes } = ctx.data.status;
  const consumers = ctx.data.consumers;
  const err = errLine();
  let close: () => void = () => {};

  const label = input({ type: 'text', value: e.label, 'aria-label': 'Label' });

  const group = `vlt-e-scope-${e.id}`;
  const specific = e.scope.kind === 'nodes';
  const plaza = radio(group, 'plaza', !specific, 'Plaza (every node)');
  const nodesPick = radio(group, 'nodes', specific, 'Specific nodes');
  const inScopeIds = new Set(scopeNodeIds(e.scope));
  const nodeBoxes = nodes.map((n) => ({ id: n.id, ...check(specific && inScopeIds.has(n.id), nodeHeading(n.id, nodeName(n.id, nodes)), n.id) }));
  const syncNodes = () => {
    nodeBoxes.forEach((b) => {
      b.input.disabled = !nodesPick.input.checked;
    });
  };
  plaza.input.addEventListener('change', syncNodes);
  nodesPick.input.addEventListener('change', syncNodes);
  syncNodes();

  // Keep the entry's current bindings ticked even when no consumer lists them.
  const known = new Set(consumers.map((c) => c.binding));
  const bindingBoxes = [
    ...consumers.map((c) => ({ value: c.binding, ...check(e.bindings.includes(c.binding), bindingText(c.binding, consumers, c.wants), c.binding) })),
    ...e.bindings.filter((b) => !known.has(b)).map((b) => ({ value: b, ...check(true, bindingText(b, consumers, null), b) })),
  ];

  const save = async () => {
    const scope: VaultScope = nodesPick.input.checked
      ? { kind: 'nodes', nodes: nodeBoxes.filter((b) => b.input.checked).map((b) => b.id) }
      : { kind: 'plaza' };
    if (scope.kind === 'nodes' && scope.nodes.length === 0) {
      err.textContent = 'Pick at least one node, or choose Plaza.';
      return;
    }
    const bindings = bindingBoxes.filter((b) => b.input.checked).map((b) => b.value);
    try {
      await vaultApi.patchEntry(e.id, { label: label.value.trim(), scope, bindings });
      close();
      toast(`Saved ${e.name}`);
      await ctx.reload();
    } catch (error) {
      err.textContent = errorText(error);
    }
  };

  close = dialog(
    `Edit ${e.name}`,
    [
      field('Label', label),
      fieldset('Scope', [plaza.row, nodesPick.row, ...nodeBoxes.map((b) => b.row)]),
      fieldset('Bindings', bindingBoxes.length ? bindingBoxes.map((b) => b.row) : [dim('Nothing asks for a secret yet.')]),
      err,
    ],
    [button('Cancel', () => close(), { kind: 'ghost' }), button('Save', () => void save(), { kind: 'primary' })],
  );
}

/** A binding's label, then the env name it wants in a sub line (the rows that share a provider name tell apart). */
function bindingText(binding: string, consumers: VaultConsumer[], wants: string | null): Node {
  return h('span', null, bindingLabel(binding, consumers), wants ? h('span.vlt-sub.mono', null, ` wants ${wants}`) : null);
}

// Test: run the hub's probe for this entry on one node. Only the hub's text is shown, never a value.
function testDialog(ctx: VaultCtx, e: VaultEntry): void {
  const nodes = ctx.data.status.nodes;
  const nodeSelect = select(
    nodes.map((n) => ({ value: n.id, label: nodeHeading(n.id, nodeName(n.id, nodes)) })),
    THIS_HUB,
  );
  nodeSelect.setAttribute('aria-label', 'Node');
  const result = h('div.vlt-res', { 'aria-live': 'polite' });
  let close: () => void = () => {};
  const runButton = button('Run test', () => void run(), { kind: 'primary' });

  const run = async () => {
    runButton.disabled = true;
    runButton.textContent = 'Testing';
    result.replaceChildren(h('span.vlt-dim', null, 'Testing'));
    try {
      const r = await vaultApi.testEntry(e.id, nodeSelect.value);
      result.replaceChildren(badge(r.ok ? 'Passed' : 'Failed', r.ok ? 'ok' : 'danger'), h('span', null, r.detail));
    } catch (error) {
      result.replaceChildren(h('div.vlt-err', { role: 'alert' }, errorText(error)));
    }
    runButton.disabled = false;
    runButton.textContent = 'Run test again';
  };

  close = dialog(
    `Test ${e.name}`,
    [field('Node', nodeSelect), result],
    [button('Close', () => close(), { kind: 'ghost' }), runButton],
  );
}

// Delete: a confirm dialog. The hub records the removal and blocks an env import from bringing the name back.
function deleteDialog(ctx: VaultCtx, e: VaultEntry): void {
  let close: () => void = () => {};
  const confirm = async () => {
    const done = await attempt(
      async () => {
        await vaultApi.deleteEntry(e.id);
        return true;
      },
      `Deleted ${e.name}`,
    );
    if (done) {
      close();
      await ctx.reload();
    }
  };

  close = dialog(
    `Delete ${e.name}?`,
    [dim(`This removes ${e.name}. The removal is recorded in the audit log and blocks an env import from bringing it back.`), errLine()],
    [button('Cancel', () => close(), { kind: 'ghost' }), button('Delete', () => void confirm(), { kind: 'danger' })],
  );
}
