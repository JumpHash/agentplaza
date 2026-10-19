// The Vault status card: the state, the key slots, the facts, and the actions for the current state.
// Value inputs are password fields, read once and cleared at once.
import { vaultApi } from '../api';
import type { VaultSlot, VaultSlotKind, VaultStatus } from '../api';
import { attempt, badge, button, card, field, input, toast } from '../ui/components';
import { h } from '../ui/dom';
import { STATE_LABEL, STATE_TONE, errorText } from './vault-common';
import type { VaultCtx } from './vault-common';
import { agoText, check, dialog, dim, errLine, kv, radio } from './vault-ui';

const KIND_LABEL: Record<VaultSlotKind, string> = { passphrase: 'Passphrase', keyfile: 'Key file', recovery: 'Recovery code' };
const KIND_LOWER: Record<VaultSlotKind, string> = { passphrase: 'passphrase', keyfile: 'key file', recovery: 'recovery code' };

/** The status card at the top of the Vault page (with its env-file banner when keys were found there). */
export function vaultStatusSection(ctx: VaultCtx): Node[] {
  const { status } = ctx.data;
  const state = status.state;
  const unsealed = state === 'unsealed';
  const hasKeyfile = status.slots.some((s) => s.kind === 'keyfile');
  const envKeys = (status as { env_file_keys?: number }).env_file_keys ?? 0;

  const actions: Node[] = [];
  if (unsealed) {
    actions.push(
      button('Seal', () => void seal(ctx)),
      button('Add passphrase', () => addPassphraseDialog(ctx)),
      button('Add key file slot', () => addKeyfileDialog(ctx), {
        disabled: hasKeyfile,
        title: hasKeyfile ? 'This vault already has a key file slot.' : 'The hub creates a key file slot on its own disk.',
      }),
      button('Create recovery code', () => recoveryDialog(ctx)),
      button('Rekey', () => rekeyDialog(ctx)),
    );
  } else if (state === 'sealed' || state === 'tampered') {
    actions.push(button('Unseal', () => unsealDialog(ctx, hasKeyfile), { kind: 'primary' }));
  }

  const slots: Node[] = status.slots.map((slot) => slotRow(ctx, slot, unsealed, status.slots.length));
  const slotValue: Node | string = slots.length ? h('div', null, ...slots) : 'None';

  const top = h('div.vlt-top', null, badge(STATE_LABEL[state], STATE_TONE[state], true), h('span.vlt-copy', null, stateCopy(status)));
  const body = [top, kv([
    ['Key slots', slotValue],
    ['Secrets', String(status.entries)],
    ['Rekeyed', status.rekeyed_ms == null ? 'Never' : agoText(status.rekeyed_ms)],
    ['Audit', auditBadge(status.audit_ok)],
  ])];

  const out: Node[] = [];
  if (envKeys > 0) out.push(envBanner());
  out.push(card('Status', body, { actions }));
  return out;
}

function stateCopy(status: VaultStatus): string {
  switch (status.state) {
    case 'unsealed':
      return 'Secrets resolve for agents, connectors and MCP servers.';
    case 'sealed':
      return 'Secrets stay encrypted. Unseal to use them.';
    case 'tampered':
      return `${status.tampered ?? 'The vault failed an integrity check.'} Secrets are not available. Check the audit log.`;
    default:
      return 'The vault is not set up. It is created when apid starts with a vault key.';
  }
}

function envBanner(): HTMLElement {
  return h(
    'div.vlt-banner',
    { role: 'status' },
    h('strong', null, 'Keys were found in an env file. They now live in the vault. Delete the file.'),
    h('span', null, 'To move what is left and remove the file in one step, run ', h('code', null, 'apz vault import FILE.env --shred'), '.'),
  );
}

/** `null` means the chain has not been checked yet (no unlock or audit read this run), which is not a failure. */
function auditBadge(ok: boolean | null): HTMLElement {
  if (ok === null) return badge('Audit chain not checked until unsealed', 'muted');
  return ok ? badge('Audit chain intact', 'ok') : badge('Audit chain broken, see the Audit tab', 'danger');
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
  return h('div.vlt-slot', null, h('span', null, KIND_LABEL[slot.kind]), h('span.vlt-sub', null, `Added ${agoText(slot.created_ms)}`), remove);
}

async function seal(ctx: VaultCtx): Promise<void> {
  const status = await attempt(() => vaultApi.seal(), 'Vault sealed');
  if (status) await ctx.reload();
}

// ---- Dialogs ---------------------------------------------------------------

function unsealDialog(ctx: VaultCtx, hasKeyfile: boolean): void {
  const err = errLine();
  const usePass = radio('vlt-umode', 'pass', true, 'Use a passphrase');
  const useCode = radio('vlt-umode', 'rc', false, 'Use a recovery code');
  const pass = input({ type: 'password', autocomplete: 'off' });
  const code = input({ type: 'password', autocomplete: 'off' });
  code.disabled = true;
  const syncChoice = () => {
    pass.disabled = !usePass.input.checked;
    code.disabled = usePass.input.checked;
  };
  usePass.input.addEventListener('change', syncChoice);
  useCode.input.addEventListener('change', syncChoice);

  let close: () => void = () => {};
  const run = async (body: { passphrase?: string; recovery_code?: string }) => {
    try {
      await vaultApi.unseal(body);
      close();
      await ctx.reload();
    } catch (e) {
      err.textContent = errorText(e);
    }
  };
  const submit = () => {
    const passMode = usePass.input.checked;
    if (passMode ? !pass.value : !code.value) {
      err.textContent = 'Enter a passphrase or a recovery code.';
      return;
    }
    const body = passMode ? { passphrase: pass.value } : { recovery_code: code.value };
    pass.value = '';
    code.value = '';
    void run(body);
  };

  close = dialog(
    'Unseal the vault',
    [
      dim('Enter a passphrase or a recovery code. Only the one you pick is sent.'),
      usePass.row,
      useCode.row,
      field('Passphrase', pass),
      field('Recovery code', code),
      err,
    ],
    [
      hasKeyfile ? button('Use the key file', () => void run({}), { kind: 'ghost' }) : null,
      button('Cancel', () => close(), { kind: 'ghost' }),
      button('Unseal', submit, { kind: 'primary' }),
    ],
  );
}

function addPassphraseDialog(ctx: VaultCtx): void {
  const err = errLine();
  const pass = input({ type: 'password', autocomplete: 'off' });
  const again = input({ type: 'password', autocomplete: 'off' });
  let close: () => void = () => {};
  const save = async () => {
    const first = pass.value;
    const second = again.value;
    pass.value = '';
    again.value = '';
    if (!first) {
      err.textContent = 'Enter a passphrase.';
      return;
    }
    if (first !== second) {
      err.textContent = 'The passphrases do not match.';
      return;
    }
    try {
      await vaultApi.addPassphrase(first);
      close();
      await ctx.reload();
    } catch (e) {
      err.textContent = errorText(e);
    }
  };

  close = dialog(
    'Add passphrase',
    [field('Passphrase', pass, 'Another way to unseal the vault.'), field('Confirm passphrase', again), err],
    [button('Cancel', () => close(), { kind: 'ghost' }), button('Add passphrase', () => void save(), { kind: 'primary' })],
  );
}

// The hub creates the key file itself (POST {kind:"keyfile"}); no file is chosen or sent from the browser.
function addKeyfileDialog(ctx: VaultCtx): void {
  const err = errLine();
  let close: () => void = () => {};
  const add = async () => {
    try {
      await vaultApi.addKeyfile();
      close();
      toast('Key file slot added');
      await ctx.reload();
    } catch (e) {
      err.textContent = errorText(e);
    }
  };

  close = dialog(
    'Add key file slot',
    [
      h('p', null, 'The hub creates the key file on its own disk. The vault can then be unsealed with that file, without a passphrase.'),
      dim('Anyone with that file and a copy of the vault data can unseal the vault. Remove the slot to destroy the file.'),
      err,
    ],
    [button('Cancel', () => close(), { kind: 'ghost' }), button('Add key file slot', () => void add(), { kind: 'primary' })],
  );
}

function recoveryDialog(ctx: VaultCtx): void {
  const err = errLine();
  let close: () => void = () => {};
  const create = async () => {
    try {
      const res = await vaultApi.recovery();
      close();
      showRecoveryCode(ctx, res.recovery_code);
    } catch (e) {
      err.textContent = errorText(e);
    }
  };

  close = dialog(
    'Create recovery code',
    [h('p', null, 'This replaces the old recovery code.'), err],
    [button('Cancel', () => close(), { kind: 'ghost' }), button('Create recovery code', () => void create(), { kind: 'primary' })],
  );
}

/** Shows the code once. It lives only in the read-only input, which is cleared on close. */
function showRecoveryCode(ctx: VaultCtx, code: string): void {
  const out = input({ readonly: true, autocomplete: 'off', 'aria-label': 'Recovery code', class: 'mono' });
  out.value = code;
  const saved = check(false, 'I saved it');
  const closeBtn = button('Close', () => close(), { kind: 'primary' });
  closeBtn.disabled = true;
  saved.input.addEventListener('change', () => {
    closeBtn.disabled = !saved.input.checked;
  });
  const copy = button('Copy', async () => {
    try {
      await navigator.clipboard.writeText(out.value);
      toast('Copied');
    } catch {
      toast('Could not copy. Select the code and copy it.', 'error');
    }
  }, { kind: 'ghost' });

  let close: () => void = () => {};
  close = dialog(
    'Recovery code',
    [h('p', null, 'Save this code somewhere safe. It is shown once and cannot be shown again.'), h('div.vlt-res', null, out, copy), saved.row],
    [closeBtn],
    () => {
      out.value = '';
      void ctx.reload();
    },
  );
}

function removeSlotDialog(ctx: VaultCtx, slot: VaultSlot): void {
  const err = errLine();
  let close: () => void = () => {};
  const remove = async () => {
    try {
      await vaultApi.removeSlot(slot.id);
      close();
      await ctx.reload();
    } catch (e) {
      err.textContent = errorText(e);
    }
  };

  close = dialog(
    'Remove key slot',
    [h('p', null, `Remove the ${KIND_LOWER[slot.kind]} slot? The vault cannot be unsealed with it afterwards.`), err],
    [button('Cancel', () => close(), { kind: 'ghost' }), button('Remove', () => void remove(), { kind: 'danger' })],
  );
}

function rekeyDialog(ctx: VaultCtx): void {
  const err = errLine();
  // The slot that unlocked this session keeps its key (the hub reuses it), so its secret is not asked for.
  const others = ctx.data.status.slots
    .filter((s) => s.kind !== 'keyfile' && s.unsealed_with !== true)
    .map((slot) => ({
      slot,
      control: input({ type: 'password', autocomplete: 'off', 'aria-label': `${KIND_LABEL[slot.kind]} ${slot.id}` }),
    }));
  let close: () => void = () => {};
  const rekey = async () => {
    if (others.some(({ control }) => !control.value)) {
      err.textContent = 'Give every passphrase and recovery code.';
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
      err.textContent = errorText(e);
    }
  };

  close = dialog(
    'Rekey the vault',
    [
      dim('Rekey re-encrypts the vault under a new root key. Secret values do not change. Every passphrase and recovery slot is rewritten, so each one must be given now, except the one that unlocked this session.'),
      ...(others.length === 0 ? [dim('No other passphrase or recovery slot needs a secret.')] : []),
      ...others.map(({ slot, control }) => field(`${KIND_LABEL[slot.kind]} (${slot.id})`, control)),
      err,
    ],
    [button('Cancel', () => close(), { kind: 'ghost' }), button('Rekey', () => void rekey(), { kind: 'primary' })],
  );
}
