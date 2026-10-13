// The Add secret dialog, shared by the Vault screen and the MCP wizard's inline creation.
import '../styles/vault-form.css';
import { api, vaultApi } from '../api';
import type { NodeView, VaultConsumer, VaultEntry, VaultKind, VaultScope } from '../api';
import { badge, button, field, input, modal, select, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import { THIS_HUB, VAULT_NAME_RE, bindingLabel, errorText, nodeName } from './vault-common';

export interface SecretPrefill {
  /** Pre-fills the name (upper-case env style). `lockName` keeps it read-only. */
  name?: string;
  lockName?: boolean;
  kind?: VaultKind;
  label?: string;
  /** Bindings to tick, e.g. `mcp:github`. */
  bindings?: string[];
  /** Node ids for a `nodes` scope (`n0` is this hub). Omitted or empty means plaza. */
  nodes?: string[];
}

const KINDS: { value: VaultKind; label: string }[] = [
  { value: 'api_key', label: 'API key' },
  { value: 'token', label: 'Token' },
  { value: 'password', label: 'Password' },
  { value: 'generic', label: 'Generic' },
];

const NAME_HINT = 'Use 2 to 64 characters: a capital letter, then capitals, digits or underscores.';

interface BindingItem {
  binding: string;
  label: string;
  wants: string;
}

/** Opens the dialog. `onSaved` gets the created entry; the value is never passed on. */
export function addSecretDialog(prefill: SecretPrefill, onSaved: (entry: VaultEntry) => void): void {
  const host = h('div.vf', null, h('p.vf-muted', null, 'Loading nodes and consumers...'));
  const close = modal('Add secret', host);
  void Promise.all([api.nodes(), vaultApi.consumers()]).then(
    ([nodes, consumers]) => replace(host, buildForm(prefill, nodes, consumers, close, onSaved)),
    (err: unknown) => replace(host, h('p.error-text', { role: 'alert' }, errorText(err))),
  );
}

function buildForm(
  prefill: SecretPrefill,
  nodes: NodeView[],
  consumers: VaultConsumer[],
  done: () => void,
  onSaved: (entry: VaultEntry) => void,
): HTMLElement {
  const nodeChoices = [
    { id: THIS_HUB, name: 'This hub' },
    ...nodes
      .filter((n) => n.id !== THIS_HUB && (!n.revoked || !!prefill.nodes?.includes(n.id)))
      .map((n) => ({ id: n.id, name: nodeName(n.id, nodes) })),
  ];

  const nameErr = h('p.error-text', { role: 'alert' });
  const scopeErr = h('p.error-text', { role: 'alert' });
  const errorBox = h('p.error-text', { role: 'alert' });

  const nameInput = input({
    type: 'text',
    value: (prefill.name ?? '').toUpperCase(),
    autocomplete: 'off',
    spellcheck: 'false',
    maxlength: 64,
    placeholder: 'API_KEY',
    readonly: !!prefill.lockName,
  });
  const kindSelect = select(KINDS, prefill.kind ?? 'api_key');
  const labelInput = input({ type: 'text', value: prefill.label ?? '', autocomplete: 'off', maxlength: 120 });
  const valueInput = input({ type: 'password', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off' });

  // Scope: plaza, or a set of nodes.
  const group = `vf-scope-${Math.random().toString(36).slice(2, 10)}`;
  const specific = (prefill.nodes?.length ?? 0) > 0;
  const plazaRadio = h('input', { type: 'radio', name: group, value: 'plaza' });
  const nodesRadio = h('input', { type: 'radio', name: group, value: 'nodes' });
  plazaRadio.checked = !specific;
  nodesRadio.checked = specific;
  const nodeBoxes = new Map<string, HTMLInputElement>();
  const nodeList = h(
    'div.vf-list',
    null,
    nodeChoices.map((n) => {
      const box = h('input', { type: 'checkbox' });
      box.checked = !!prefill.nodes?.includes(n.id);
      nodeBoxes.set(n.id, box);
      return h('label.vf-check', null, box, h('span', null, nodeName(n.id, nodes)));
    }),
  );
  nodeList.hidden = !specific;
  const syncScope = () => {
    nodeList.hidden = !nodesRadio.checked;
    scopeErr.textContent = '';
  };
  plazaRadio.addEventListener('change', syncScope);
  nodesRadio.addEventListener('change', syncScope);

  // Bindings: the consumers, suggested ones first. Ticks live in `checked`, so a re-sort keeps them.
  const checked = new Map<string, boolean>();
  for (const b of prefill.bindings ?? []) checked.set(b, true);
  const items: BindingItem[] = consumers.map((c) => ({
    binding: c.binding,
    label: bindingLabel(c.binding, consumers),
    wants: c.wants,
  }));
  for (const b of prefill.bindings ?? []) {
    if (!items.some((i) => i.binding === b)) items.push({ binding: b, label: bindingLabel(b, consumers), wants: '' });
  }
  const bindingList = h('div.vf-list', null);
  const renderBindings = () => {
    const name = nameInput.value.trim();
    const suggested = (i: BindingItem) => name !== '' && i.wants === name;
    const ordered = [...items].sort((a, b) => Number(suggested(b)) - Number(suggested(a)));
    replace(
      bindingList,
      ordered.length === 0
        ? h('p.vf-muted', null, 'Nothing asks for a secret yet. You can bind it later.')
        : ordered.map((item) => {
            const box = h('input', { type: 'checkbox' });
            box.checked = checked.get(item.binding) ?? false;
            box.addEventListener('change', () => checked.set(item.binding, box.checked));
            return h(
              'label.vf-check',
              null,
              box,
              h('span.vf-grow', null, h('span', null, item.label), item.wants ? h('span.vf-mono', null, item.wants) : null),
              suggested(item) ? badge('suggested', 'accent') : null,
            );
          }),
    );
  };
  renderBindings();

  nameInput.addEventListener('input', () => {
    const pos = nameInput.selectionStart;
    nameInput.value = nameInput.value.toUpperCase();
    if (pos !== null) nameInput.setSelectionRange(pos, pos);
    nameErr.textContent = '';
    renderBindings();
  });

  let saving = false;
  const save = async (): Promise<void> => {
    if (saving) return;
    nameErr.textContent = '';
    scopeErr.textContent = '';
    errorBox.textContent = '';

    const name = nameInput.value.trim();
    if (!VAULT_NAME_RE.test(name)) {
      nameErr.textContent = NAME_HINT;
      nameInput.focus();
      return;
    }
    let scope: VaultScope = { kind: 'plaza' };
    if (nodesRadio.checked) {
      const nodeIds = Array.from(nodeBoxes)
        .filter(([, box]) => box.checked)
        .map(([id]) => id);
      if (nodeIds.length === 0) {
        scopeErr.textContent = 'Pick at least one node, or choose Plaza.';
        return;
      }
      scope = { kind: 'nodes', nodes: nodeIds };
    }
    if (valueInput.value === '') {
      errorBox.textContent = 'Enter the value.';
      valueInput.focus();
      return;
    }
    const bindings = Array.from(checked)
      .filter(([, on]) => on)
      .map(([binding]) => binding);

    // The value goes into the request, then the input is cleared at once.
    const pending = vaultApi.createEntry({
      name,
      kind: kindSelect.value as VaultKind,
      label: labelInput.value.trim(),
      value: valueInput.value,
      scope,
      bindings,
    });
    valueInput.value = '';
    saving = true;
    saveBtn.disabled = true;
    try {
      const entry = await pending;
      done();
      toast('Secret saved');
      onSaved(entry);
    } catch (err) {
      errorBox.textContent = errorText(err);
      saving = false;
      saveBtn.disabled = false;
    }
  };

  const saveBtn = button('Save secret', () => void save(), { kind: 'primary' });
  const cancelBtn = button('Cancel', () => done(), { kind: 'ghost' });

  return h(
    'div.vf',
    null,
    field('Name', nameInput, NAME_HINT),
    nameErr,
    field('Kind', kindSelect),
    field('Label', labelInput, 'Optional. Shown in lists.'),
    field('Value', valueInput, 'Never shown again. It is cleared on save, so type it again after an error.'),
    h(
      'fieldset.vf-group',
      null,
      h('legend', null, 'Scope'),
      h('label.vf-check', null, plazaRadio, h('span', null, 'Plaza (every node)')),
      h('label.vf-check', null, nodesRadio, h('span', null, 'Specific nodes')),
      nodeList,
      scopeErr,
    ),
    h(
      'fieldset.vf-group',
      null,
      h('legend', null, 'Bindings'),
      h('p.vf-muted', null, 'Consumers that may resolve this secret.'),
      bindingList,
    ),
    errorBox,
    h('div.vf-actions', null, cancelBtn, saveBtn),
  );
}
