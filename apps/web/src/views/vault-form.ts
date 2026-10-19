// The Add secret dialog, shared by the Vault screen and the MCP wizard's inline creation.
import { api, vaultApi } from '../api';
import type { NodeView, VaultConsumer, VaultEntry, VaultKind, VaultScope } from '../api';
import { badge, button, field, input, select, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import { THIS_HUB, VAULT_NAME_RE, bindingLabel, errorText, nodeName } from './vault-common';
import { check, dialog, dim, errLine, fieldset, nodeHeading, radio } from './vault-ui';

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
  const slot = h('div.vlt-form', null, dim('Loading nodes and consumers...'));
  let close: () => void = () => {};
  const cancel = () => button('Cancel', () => close(), { kind: 'ghost' });
  close = dialog('Add secret', [slot], [cancel()]);
  void Promise.all([api.nodes(), vaultApi.consumers()]).then(
    ([nodes, consumers]) => {
      const form = buildForm(prefill, nodes, consumers, () => close(), onSaved);
      replace(slot, form.body);
      slot.closest('.sheet')?.querySelector('footer.sheet-f')?.replaceChildren(...form.foot);
    },
    (err: unknown) => replace(slot, h('div.vlt-err', { role: 'alert' }, errorText(err))),
  );
}

function buildForm(
  prefill: SecretPrefill,
  nodes: NodeView[],
  consumers: VaultConsumer[],
  done: () => void,
  onSaved: (entry: VaultEntry) => void,
): { body: HTMLElement; foot: HTMLElement[] } {
  const nodeChoices = [
    { id: THIS_HUB, name: 'This hub' },
    ...nodes
      .filter((n) => n.id !== THIS_HUB && (!n.revoked || !!prefill.nodes?.includes(n.id)))
      .map((n) => ({ id: n.id, name: nodeName(n.id, nodes) })),
  ];

  const nameErr = errLine();
  const scopeErr = errLine();
  const errorBox = errLine();

  const nameInput = input({
    type: 'text',
    value: (prefill.name ?? '').toUpperCase(),
    autocomplete: 'off',
    spellcheck: 'false',
    maxlength: 64,
    placeholder: 'API_KEY',
    readonly: !!prefill.lockName,
    class: 'mono',
  });
  const kindSelect = select(KINDS, prefill.kind ?? 'api_key');
  const labelInput = input({ type: 'text', value: prefill.label ?? '', autocomplete: 'off', maxlength: 120 });
  const valueInput = input({ type: 'password', autocomplete: 'off', spellcheck: 'false', autocapitalize: 'off' });

  // Scope: plaza, or a set of nodes.
  const group = `vf-scope-${Math.random().toString(36).slice(2, 10)}`;
  const specific = (prefill.nodes?.length ?? 0) > 0;
  const plaza = radio(group, 'plaza', !specific, 'Plaza (every node)');
  const nodesPick = radio(group, 'nodes', specific, 'Specific nodes');
  const nodeBoxes = new Map<string, HTMLInputElement>();
  const nodeRows = nodeChoices.map((n) => {
    const box = check(!!prefill.nodes?.includes(n.id), nodeHeading(n.id, n.name), n.id);
    nodeBoxes.set(n.id, box.input);
    return box.row;
  });
  const nodeList = h('div', null, ...nodeRows);
  nodeList.hidden = !specific;
  const syncScope = () => {
    nodeList.hidden = !nodesPick.input.checked;
    scopeErr.textContent = '';
  };
  plaza.input.addEventListener('change', syncScope);
  nodesPick.input.addEventListener('change', syncScope);

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
  const bindingList = h('div', null);
  const renderBindings = () => {
    const name = nameInput.value.trim();
    const suggested = (i: BindingItem) => name !== '' && i.wants === name;
    const ordered = [...items].sort((a, b) => Number(suggested(b)) - Number(suggested(a)));
    replace(
      bindingList,
      ordered.length === 0
        ? dim('Nothing asks for a secret yet. You can bind it later.')
        : ordered.map((item) => {
            const box = check(
              checked.get(item.binding) ?? false,
              h(
                'span',
                null,
                item.label,
                suggested(item) ? ' ' : null,
                suggested(item) ? badge('suggested', 'accent') : null,
                item.wants ? h('span.vlt-sub.mono', null, ` wants ${item.wants}`) : null,
              ),
            );
            box.input.addEventListener('change', () => checked.set(item.binding, box.input.checked));
            return box.row;
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
    if (nodesPick.input.checked) {
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

  const body = h(
    'div.vlt-form',
    null,
    field('Name', nameInput, NAME_HINT),
    nameErr,
    field('Kind', kindSelect),
    field('Label', labelInput, 'Optional. Shown in lists.'),
    field('Value', valueInput, 'Never shown again. It is cleared on save, so type it again after an error.'),
    fieldset('Scope', [plaza.row, nodesPick.row, nodeList, scopeErr]),
    fieldset('Bindings', [h('p.vlt-sub', null, 'Consumers that may resolve this secret.'), bindingList]),
    errorBox,
  );
  return { body, foot: [cancelBtn, saveBtn] };
}
