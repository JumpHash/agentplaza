// The Secrets tab: one row per vault entry, with the row actions (rotate, roll back, edit, test, delete).
// No secret value is ever shown here. The only value input is the one in the rotate dialog, cleared on read.
import '../styles/vault-secrets.css';
import { h } from '../ui/dom';
import type { IconName } from '../ui/icons.gen';
import { attempt, ago, badge, button, card, clip, empty, field, input, modal, select, table, toast, type Column } from '../ui/components';
import { type VaultConsumer, type VaultEntry, type VaultNodeRef, type VaultScope, vaultApi } from '../api';
import { THIS_HUB, type VaultCtx, bindingLabel, errorText, fingerprint, lastFour, nodeName, scopeNodeIds } from './vault-common';

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
  control.addEventListener('change', () => onScope(scopeFromValue(control.value)));
  return h('div.vs-form', { style: 'padding: var(--s3) var(--s3) 0' }, field('Scope', control));
}

export function vaultSecretsSection(ctx: VaultCtx, scope: SecretScope, onScope: (next: SecretScope) => void): HTMLElement {
  if (ctx.locked) {
    return card(null, empty('key', 'Vault is locked', 'Unseal the vault to see and manage secrets.'), { flush: true });
  }
  const { entries, consumers, status } = ctx.data;
  if (entries.length === 0) {
    return card(null, empty('key', 'No secrets yet', 'Add one with Add secret, or import a .env file.'), { flush: true });
  }
  const shown = entries.filter((e) => inScope(e, scope));
  const filter = scopeSelect(status.nodes, scope, onScope);
  if (shown.length === 0) {
    return card(null, [filter, empty('key', 'No secrets in this scope', 'Pick another scope above.')], { flush: true });
  }

  const columns: Column<VaultEntry>[] = [
    {
      label: 'Name',
      cell: (e) => h('div', null, h('span.vs-name', null, e.name.replace(/_/g, '_\u200b')), e.label ? h('span.vs-sub', null, e.label) : null),
    },
    { label: 'Kind', cell: (e) => e.kind },
    { label: 'Scope', cell: (e) => scopeChips(e.scope, status.nodes) },
    { label: 'Bindings', cell: (e) => bindingChips(e.bindings, consumers) },
    { label: 'Fingerprint', cell: (e) => h('span.mono', { title: fingerprint(e) }, clip(fingerprint(e), 14)) },
    { label: 'Last 4', cell: (e) => h('span.mono', null, lastFour(e)) },
    { label: 'Version', cell: (e) => h('span.mono', null, `v${e.current}`) },
    { label: 'Last used', cell: (e) => (e.last_used_ms ? ago(e.last_used_ms) : 'never') },
    { label: 'Uses', cell: (e) => e.use_count, num: true },
    { label: 'Actions', cell: (e) => rowActions(ctx, e) },
  ];

  const tbl = table(columns, shown, undefined, (e) => e.id);
  tbl.classList.add('vs-table');
  // The stacked (narrow) layout labels each cell with its column name.
  tbl.querySelectorAll('tbody tr').forEach((tr) => {
    tr.querySelectorAll<HTMLTableCellElement>('td').forEach((td, i) => {
      td.dataset.label = columns[i]?.label ?? '';
    });
  });
  return card(null, [filter, h('div.vs-scroll', null, tbl)], { flush: true });
}

function chip(text: string): HTMLElement {
  return h('span.vs-chip', { title: text }, text);
}

function scopeChips(scope: VaultScope, nodes: VaultNodeRef[]): HTMLElement {
  if (scope.kind === 'plaza') return h('span.vs-chips', null, chip('plaza'));
  return h('span.vs-chips', null, ...scope.nodes.map((id) => chip(nodeName(id, nodes))));
}

function bindingChips(bindings: string[], consumers: VaultConsumer[]): HTMLElement {
  if (bindings.length === 0) return h('span.vs-sub', null, 'none');
  return h('span.vs-chips', null, ...bindings.map((b) => chip(bindingLabel(b, consumers))));
}

function rowActions(ctx: VaultCtx, e: VaultEntry): HTMLElement {
  return h(
    'div.vs-actions-row',
    null,
    rowButton('Rotate value', 'refresh', () => rotateDialog(ctx, e), ctx.locked),
    rowButton('Rollback', 'clock', () => rollbackDialog(ctx, e), ctx.locked),
    rowButton('Edit scope and bindings', 'edit', () => editDialog(ctx, e), ctx.locked),
    rowButton('Test', 'play', () => testDialog(ctx, e), ctx.locked),
    rowButton('Delete', 'trash', () => deleteDialog(ctx, e), ctx.locked, true),
  );
}

function rowButton(title: string, iconName: IconName, onClick: () => void, disabled: boolean, danger = false): HTMLButtonElement {
  const b = button('', () => onClick(), { kind: 'ghost', size: 'sm', icon: iconName, title, disabled });
  b.setAttribute('aria-label', title);
  if (danger) b.classList.add('vs-danger');
  return b;
}

function errorLine(): HTMLElement {
  const el = h('p.vs-error', { role: 'alert' });
  el.hidden = true;
  return el;
}

function showError(el: HTMLElement, text: string): void {
  el.textContent = text;
  el.hidden = false;
}

function formButtons(...children: Node[]): HTMLElement {
  return h('div.vs-form-actions', null, ...children);
}

// `sub` is the env name a binding wants, so rows that share a provider name (Unsplash, Telegram) tell apart.
function checkRow(value: string, text: string, checked: boolean, sub?: string): { cb: HTMLInputElement; row: HTMLElement } {
  const cb = h('input', { type: 'checkbox', value });
  cb.checked = checked;
  const label = h('span.vs-pick-text', null, text, sub ? h('span.vs-pick-sub', null, sub) : null);
  return { cb, row: h('label.vs-pick', null, cb, label) };
}

// Rotate: one password input, never pre-filled. The value is cleared as soon as it is read.
function rotateDialog(ctx: VaultCtx, e: VaultEntry): void {
  const value = input({ type: 'password', autocomplete: 'off', 'aria-label': 'New value' });
  const err = errorLine();
  let close: () => void = () => {};

  const save = async () => {
    let secret: string | null = value.value;
    value.value = '';
    if (!secret) {
      showError(err, 'Enter the new value.');
      return;
    }
    try {
      const updated = await vaultApi.rotateEntry(e.id, secret);
      close();
      toast(`${e.name} is now v${updated.current}`);
      await ctx.reload();
    } catch (error) {
      showError(err, errorText(error));
    } finally {
      secret = null;
    }
  };

  const body = h(
    'div.vs-form',
    null,
    field('New value', value, 'Never shown again. Each change is a new version.'),
    err,
    formButtons(button('Cancel', () => close()), button('Rotate value', () => void save(), { kind: 'primary' })),
  );
  close = modal(`Rotate ${e.name}`, body, () => {
    value.value = '';
  });
}

// Rollback: pick one of the non-current versions, then confirm.
function rollbackDialog(ctx: VaultCtx, e: VaultEntry): void {
  const older = e.versions.filter((ver) => ver.v !== e.current).sort((a, b) => b.v - a.v);
  if (older.length === 0) {
    modal(`Roll back ${e.name}`, empty('clock', 'No earlier version', 'Only the current version exists.'));
    return;
  }

  let picked: number | null = null;
  const err = errorLine();
  let close: () => void = () => {};
  const confirm = button('Roll back', () => void run(), { kind: 'primary', disabled: true });
  const group = `vs-rollback-${e.id}`;

  const rows = older.map((ver) => {
    const radio = h('input', { type: 'radio', name: group, value: String(ver.v) });
    radio.addEventListener('change', () => {
      picked = ver.v;
      confirm.disabled = false;
    });
    return h(
      'label.vs-pick',
      null,
      radio,
      h(
        'span.vs-pick-text',
        null,
        h('span.mono', null, `v${ver.v}`),
        ` · ${ago(ver.created_ms)} · `,
        h('span.mono', { title: ver.fp }, clip(ver.fp, 14)),
        ver.hint ? h('span.vs-sub', null, ver.hint) : null,
      ),
    );
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

  const body = h(
    'div.vs-form',
    null,
    h('p.vs-muted', null, 'Pick an earlier version. It becomes the current value.'),
    h('div.vs-picks', null, rows),
    err,
    formButtons(button('Cancel', () => close()), confirm),
  );
  close = modal(`Roll back ${e.name}`, body);
}

// Edit: label, scope (plaza or specific nodes) and bindings.
function editDialog(ctx: VaultCtx, e: VaultEntry): void {
  const { nodes } = ctx.data.status;
  const consumers = ctx.data.consumers;
  const err = errorLine();
  let close: () => void = () => {};

  const label = input({ type: 'text', value: e.label, 'aria-label': 'Label' });

  const scopeGroup = `vs-scope-${e.id}`;
  const plaza = h('input', { type: 'radio', name: scopeGroup, value: 'plaza' });
  const specific = h('input', { type: 'radio', name: scopeGroup, value: 'nodes' });
  plaza.checked = e.scope.kind === 'plaza';
  specific.checked = e.scope.kind === 'nodes';
  const inScope = new Set(scopeNodeIds(e.scope));
  const nodeBoxes = nodes.map((n) => ({ id: n.id, ...checkRow(n.id, nodeName(n.id, nodes), inScope.has(n.id)) }));
  const syncNodes = () => {
    nodeBoxes.forEach((b) => {
      b.cb.disabled = !specific.checked;
    });
  };
  plaza.addEventListener('change', syncNodes);
  specific.addEventListener('change', syncNodes);
  syncNodes();

  // Keep the entry's current bindings ticked even when no consumer lists them.
  const known = new Set(consumers.map((c) => c.binding));
  const bindingBoxes = [
    ...consumers.map((c) => checkRow(c.binding, bindingLabel(c.binding, consumers), e.bindings.includes(c.binding), c.wants)),
    ...e.bindings.filter((b) => !known.has(b)).map((b) => checkRow(b, bindingLabel(b, consumers), true)),
  ];

  const save = async () => {
    const scope: VaultScope = specific.checked
      ? { kind: 'nodes', nodes: nodeBoxes.filter((b) => b.cb.checked).map((b) => b.id) }
      : { kind: 'plaza' };
    if (scope.kind === 'nodes' && scope.nodes.length === 0) {
      showError(err, 'Pick at least one node, or choose Plaza.');
      return;
    }
    const bindings = bindingBoxes.filter((b) => b.cb.checked).map((b) => b.cb.value);
    try {
      await vaultApi.patchEntry(e.id, { label: label.value.trim(), scope, bindings });
      close();
      toast(`Saved ${e.name}`);
      await ctx.reload();
    } catch (error) {
      showError(err, errorText(error));
    }
  };

  const body = h(
    'div.vs-form',
    null,
    field('Label', label),
    h('fieldset.vs-fieldset', null, h('legend', null, 'Scope'), h('div.vs-radios', null, labelled(plaza, 'Plaza (every node)'), labelled(specific, 'Specific nodes')), h('div.vs-picks', null, nodeBoxes.map((b) => b.row))),
    h('fieldset.vs-fieldset', null, h('legend', null, 'Bindings'), h('div.vs-picks', null, bindingBoxes.map((b) => b.row))),
    err,
    formButtons(button('Cancel', () => close()), button('Save', () => void save(), { kind: 'primary' })),
  );
  close = modal(`Edit ${e.name}`, body);
}

function labelled(control: HTMLInputElement, text: string): HTMLElement {
  return h('label.vs-pick', null, control, h('span.vs-pick-text', null, text));
}

// Test: run the hub's probe for this entry on one node. Only the hub's text is shown, never a value.
function testDialog(ctx: VaultCtx, e: VaultEntry): void {
  const nodeSelect = select(
    ctx.data.status.nodes.map((n) => ({ value: n.id, label: nodeName(n.id, ctx.data.status.nodes) })),
    THIS_HUB,
  );
  const result = h('div.vs-result', { 'aria-live': 'polite' });
  let close: () => void = () => {};
  const runButton = button('Run test', () => void run(), { kind: 'primary' });

  const run = async () => {
    runButton.disabled = true;
    runButton.textContent = 'Testing';
    result.replaceChildren(h('p.vs-muted', null, 'Testing'));
    try {
      const r = await vaultApi.testEntry(e.id, nodeSelect.value);
      result.replaceChildren(badge(r.ok ? 'Passed' : 'Failed', r.ok ? 'ok' : 'danger', true), h('p.vs-detail', null, r.detail));
    } catch (error) {
      result.replaceChildren(h('p.vs-error', { role: 'alert' }, errorText(error)));
    }
    runButton.disabled = false;
    runButton.textContent = 'Run test again';
  };

  const body = h(
    'div.vs-form',
    null,
    field('Node', nodeSelect),
    result,
    formButtons(button('Close', () => close()), runButton),
  );
  close = modal(`Test ${e.name}`, body);
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

  const body = h(
    'div.vs-form',
    null,
    h('p.vs-muted', null, `This removes ${e.name}. The removal is recorded in the audit log and blocks an env import from bringing it back.`),
    formButtons(button('Cancel', () => close()), button('Delete', () => void confirm(), { kind: 'danger' })),
  );
  close = modal(`Delete ${e.name}?`, body);
}
