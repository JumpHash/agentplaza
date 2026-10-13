// The Vault status card: the state, the key slots, the facts, and the actions for the current state.
// Value inputs are password fields, read once and cleared at once.
import '../styles/vault-status.css';
import { vaultApi } from '../api';
import type { VaultSlot, VaultSlotKind, VaultStatus } from '../api';
import { ago, attempt, badge, button, card, field, input, modal, toast } from '../ui/components';
import { h } from '../ui/dom';
import { STATE_LABEL, STATE_TONE, errorText } from './vault-common';
import type { VaultCtx } from './vault-common';

const KIND_LABEL: Record<VaultSlotKind, string> = { passphrase: 'Passphrase', keyfile: 'Key file', recovery: 'Recovery code' };

/** The status card at the top of the Vault page, with the actions the current state allows. */
export function vaultStatusSection(ctx: VaultCtx): HTMLElement {
  const { status } = ctx.data;
  const state = status.state;
  const unsealed = state === 'unsealed';
  const unsealable = state === 'sealed' || state === 'tampered';
  const envKeys = (status as { env_file_keys?: number }).env_file_keys ?? 0;
  const hasKeyfile = status.slots.some((s) => s.kind === 'keyfile');

  const actions: Node[] = [];
  if (unsealed) {
    actions.push(
      button('Seal', () => void seal(ctx)),
      button('Add passphrase', () => addPassphraseDialog(ctx)),
      button('Create recovery code', () => recoveryDialog(ctx)),
      button('Rekey', () => rekeyDialog(ctx)),
    );
  }
  if (unsealable) actions.push(button('Unseal', () => unsealDialog(ctx, hasKeyfile), { kind: 'primary' }));

  const slotList = status.slots.length
    ? h('ul.vault-slots', null, status.slots.map((slot) => slotRow(ctx, slot, unsealed, status.slots.length)))
    : h('span.muted', null, 'None');

  const facts = h(
    'dl.vault-facts',
    null,
    fact('Key slots', slotList),
    fact('Secrets', String(status.entries)),
    fact('Rekeyed', status.rekeyed_ms == null ? 'Never' : ago(status.rekeyed_ms)),
    fact(
      'Audit',
      status.audit_ok
        ? badge('Audit chain intact', 'ok', true)
        : badge('Audit chain broken, see the Audit tab', 'danger', true),
    ),
  );

  const top = h(
    'div.vault-status-top',
    null,
    badge(STATE_LABEL[state], STATE_TONE[state], true),
    h('div.vault-status-copy', null, stateLines(status)),
  );

  return h(
    'section.vault-status',
    null,
    envKeys > 0 ? envBanner() : null,
    card('Status', [top, facts], { actions: actions.length ? [h('div.vault-status-actions', null, ...actions)] : [] }),
  );
}

function stateLines(status: VaultStatus): HTMLElement[] {
  switch (status.state) {
    case 'unsealed':
      return [h('p', null, 'Secrets resolve for agents, connectors and MCP servers.')];
    case 'sealed':
      return [h('p', null, 'Secrets stay encrypted. Unseal to use them.')];
    case 'tampered':
      return [
        h('p', null, status.tampered ?? 'The vault failed an integrity check.'),
        h('p', null, 'Secrets are not available. Check the audit log.'),
      ];
    default:
      return [h('p', null, 'The vault is not set up. It is created when apid starts with a vault key.')];
  }
}

function envBanner(): HTMLElement {
  return h(
    'div.banner.warn.vault-status-banner',
    { role: 'status' },
    h(
      'div',
      null,
      h('p', null, 'Keys were found in an env file. They now live in the vault. Delete the file.'),
      h('p', null, 'To move what is left and remove the file in one step, run ', h('code', null, 'apz vault import FILE.env --shred'), '.'),
    ),
  );
}

function fact(label: string, value: Node | string): HTMLElement {
  return h('div.vault-fact', null, h('dt', null, label), h('dd', null, value));
}

function slotRow(ctx: VaultCtx, slot: VaultSlot, unsealed: boolean, count: number): HTMLElement {
  const last = count <= 1;
  const remove = unsealed
    ? button('Remove', () => removeSlotDialog(ctx, slot), {
        kind: 'ghost',
        size: 'sm',
        disabled: last,
        title: last ? 'A vault needs at least one key slot.' : 'Remove this key slot',
      })
    : null;
  return h('li.vault-slot', null, h('span.vault-slot-name', null, KIND_LABEL[slot.kind], ' ', h('span.muted', null, `Added ${ago(slot.created_ms)}`)), remove);
}

async function seal(ctx: VaultCtx): Promise<void> {
  const status = await attempt(() => vaultApi.seal(), 'Vault sealed');
  if (status) await ctx.reload();
}

// ---- Dialogs ---------------------------------------------------------------

function dialog(title: string, body: Node[], onClose?: () => void): () => void {
  return modal(title, h('div.vault-status-dialog', null, ...body), onClose);
}

function row(...nodes: (Node | null)[]): HTMLElement {
  return h('div.vault-status-row', null, ...nodes);
}

function errorLine(): HTMLParagraphElement {
  return h('p.vault-status-error', { role: 'alert', hidden: true });
}

function setError(el: HTMLElement, text: string): void {
  el.textContent = text;
  el.hidden = !text;
}

function unsealDialog(ctx: VaultCtx, hasKeyfile: boolean): void {
  const err = errorLine();
  const usePass = h('input', { type: 'radio', name: 'vault-unseal-method', value: 'passphrase' });
  const useCode = h('input', { type: 'radio', name: 'vault-unseal-method', value: 'recovery' });
  usePass.checked = true;
  const pass = input({ type: 'password', autocomplete: 'off' });
  const code = input({ type: 'password', autocomplete: 'off' });
  const syncChoice = () => {
    pass.disabled = !usePass.checked;
    code.disabled = usePass.checked;
  };
  usePass.addEventListener('change', syncChoice);
  useCode.addEventListener('change', syncChoice);
  syncChoice();

  let close: () => void = () => {};
  const run = async (body: { passphrase?: string; recovery_code?: string }) => {
    try {
      await vaultApi.unseal(body);
      close();
      await ctx.reload();
    } catch (e) {
      setError(err, errorText(e));
    }
  };
  const submit = () => {
    if (usePass.checked ? !pass.value : !code.value) {
      setError(err, 'Enter a passphrase or a recovery code.');
      return;
    }
    const body = usePass.checked ? { passphrase: pass.value } : { recovery_code: code.value };
    pass.value = '';
    code.value = '';
    void run(body);
  };

  close = dialog('Unseal the vault', [
    h('p', null, 'Enter a passphrase or a recovery code. Only the one you pick is sent.'),
    h('div.vault-status-choice', null, h('label', null, usePass, ' Use a passphrase'), h('label', null, useCode, ' Use a recovery code')),
    field('Passphrase', pass),
    field('Recovery code', code),
    err,
    row(
      button('Cancel', () => close()),
      hasKeyfile ? button('Use the key file', () => void run({})) : null,
      button('Unseal', submit, { kind: 'primary' }),
    ),
  ]);
}

function addPassphraseDialog(ctx: VaultCtx): void {
  const err = errorLine();
  const pass = input({ type: 'password', autocomplete: 'off' });
  const again = input({ type: 'password', autocomplete: 'off' });
  let close: () => void = () => {};
  const save = async () => {
    const first = pass.value;
    const second = again.value;
    pass.value = '';
    again.value = '';
    if (!first) {
      setError(err, 'Enter a passphrase.');
      return;
    }
    if (first !== second) {
      setError(err, 'The passphrases do not match.');
      return;
    }
    try {
      await vaultApi.addPassphrase(first);
      close();
      await ctx.reload();
    } catch (e) {
      setError(err, errorText(e));
    }
  };

  close = dialog('Add passphrase', [
    field('Passphrase', pass, 'Another way to unseal the vault.'),
    field('Confirm passphrase', again),
    err,
    row(button('Cancel', () => close()), button('Add passphrase', () => void save(), { kind: 'primary' })),
  ]);
}

function recoveryDialog(ctx: VaultCtx): void {
  const err = errorLine();
  let close: () => void = () => {};
  const create = async () => {
    try {
      const res = await vaultApi.recovery();
      close();
      showRecoveryCode(ctx, res.recovery_code);
    } catch (e) {
      setError(err, errorText(e));
    }
  };

  close = dialog('Create recovery code', [
    h('p', null, 'This replaces the old recovery code.'),
    err,
    row(button('Cancel', () => close()), button('Create recovery code', () => void create(), { kind: 'primary' })),
  ]);
}

/** Shows the code once. It lives only in the read-only input, which is cleared on close. */
function showRecoveryCode(ctx: VaultCtx, code: string): void {
  const out = input({ readonly: true, autocomplete: 'off', 'aria-label': 'Recovery code' });
  out.value = code;
  const saved = h('input', { type: 'checkbox' });
  const closeBtn = button('Close', () => close(), { kind: 'primary' });
  closeBtn.disabled = true;
  saved.addEventListener('change', () => {
    closeBtn.disabled = !saved.checked;
  });
  const copy = button('Copy', async () => {
    try {
      await navigator.clipboard.writeText(out.value);
      toast('Copied');
    } catch {
      toast('Could not copy. Select the code and copy it.', 'error');
    }
  });

  let close: () => void = () => {};
  close = dialog(
    'Recovery code',
    [
      h('p', null, 'Save this code somewhere safe. It is shown once and cannot be shown again.'),
      row(out, copy),
      h('label.vault-status-check', null, saved, ' I saved it'),
      row(closeBtn),
    ],
    () => {
      out.value = '';
      void ctx.reload();
    },
  );
}

function removeSlotDialog(ctx: VaultCtx, slot: VaultSlot): void {
  const err = errorLine();
  let close: () => void = () => {};
  const remove = async () => {
    try {
      await vaultApi.removeSlot(slot.id);
      close();
      await ctx.reload();
    } catch (e) {
      setError(err, errorText(e));
    }
  };

  close = dialog('Remove key slot', [
    h('p', null, `Remove the ${KIND_LABEL[slot.kind].toLowerCase()} slot? The vault cannot be unsealed with it afterwards.`),
    err,
    row(button('Cancel', () => close()), button('Remove', () => void remove(), { kind: 'danger' })),
  ]);
}

function rekeyDialog(ctx: VaultCtx): void {
  const err = errorLine();
  const others = ctx.data.status.slots
    .filter((s) => s.kind !== 'keyfile')
    .map((slot) => ({
      slot,
      control: input({ type: 'password', autocomplete: 'off', 'aria-label': `${KIND_LABEL[slot.kind]} ${slot.id}` }),
    }));
  let close: () => void = () => {};
  const rekey = async () => {
    if (others.some(({ control }) => !control.value)) {
      setError(err, 'Give every passphrase and recovery code.');
      return;
    }
    const secrets: Record<string, string> = {};
    for (const { slot, control } of others) {
      secrets[slot.id] = control.value;
      control.value = '';
    }
    try {
      await vaultApi.rekey(secrets);
      close();
      await ctx.reload();
    } catch (e) {
      setError(err, errorText(e));
    }
  };

  close = dialog('Rekey the vault', [
    h(
      'p',
      null,
      'Rekey re-encrypts the vault under a new root key. Secret values do not change. Every passphrase and recovery slot is rewritten, so each one must be given now.',
    ),
    ...others.map(({ slot, control }) => field(`${KIND_LABEL[slot.kind]} (${slot.id})`, control)),
    err,
    row(button('Cancel', () => close()), button('Rekey', () => void rekey(), { kind: 'primary' })),
  ]);
}
