// Step 2 of the MCP wizard: the credentials each preset spec needs. A secret is either an existing
// vault entry (picked, then explicitly bound to this server) or a new one made in addSecretDialog.
// A plain setting is typed in and never goes to the vault. No value is ever shown or logged here.
import '../styles/proto/settings-b.css';
import { vaultApi, type McpSetting, type McpSpec, type VaultEntry } from '../api';
import { h, replace } from '../ui/dom';
import { attempt, badge, field, input } from '../ui/components';
import { btn, check, mcpBinding, ownRowProblem } from './mcp-common';
import { addSecretDialog } from './vault-form';
import { errorText } from './vault-common';
import type { WizardCtx } from './mcp-wizard';

/** Words that make a vault name plausible for any secret spec, besides the spec's own name tokens. */
const SECRET_WORDS = ['TOKEN', 'KEY', 'SECRET', 'PASSWORD'];

type Kind = 'env' | 'headers';
type Row = { kind: Kind; spec: McpSpec };

export function step2Credentials(ctx: WizardCtx): HTMLElement {
  const { env, headers } = ctx.state.specs;
  const rows: Row[] = [
    ...env.map((spec) => ({ kind: 'env' as const, spec })),
    ...headers.map((spec) => ({ kind: 'headers' as const, spec })),
  ];
  // The required-and-unset box. Hidden while nothing is missing (a hidden wrapper takes no space in the column).
  const warn = h('div', { role: 'status', hidden: true });
  const refreshWarn = (): void => {
    const missing = rows.filter(({ kind, spec }) => spec.required && !isSet(ctx, kind, spec.name)).map(({ spec }) => spec.name);
    warn.hidden = missing.length === 0;
    replace(
      warn,
      missing.length ? h('div.mcpwz-card.mcpwz-warnbox', null, h('strong', null, 'Required and not set'), h('p.mono', null, missing.join(', '))) : null,
    );
  };
  // A custom or remote server's rows are the owner's own, so each can be removed.
  const own = ctx.state.ownRows;
  const cards = rows.map(({ kind, spec }) =>
    spec.secret ? secretCard(ctx, kind, spec, refreshWarn, own) : plainCard(ctx, kind, spec, refreshWarn, own),
  );
  refreshWarn();
  return h(
    'div.mcpwz-body',
    null,
    !rows.length && !own ? h('p.muted', null, 'This server needs no credentials.') : cards,
    warn,
    own ? ownRowForm(ctx) : null,
  );
}

// ---- The owner's own rows (custom and remote servers) ------------------------

/** Drops a row the owner added: its spec and its draft setting. */
function removeOwn(ctx: WizardCtx, kind: Kind, name: string): void {
  ctx.state.specs[kind] = ctx.state.specs[kind].filter((spec) => spec.name !== name);
  store(ctx, kind, name, null);
  ctx.redraw();
}

/**
 * The form that adds a row: an env var for a container (its value a vault secret), a header for a remote URL
 * (its value a vault secret, optionally wrapped in a format such as "Bearer {}").
 */
function ownRowForm(ctx: WizardCtx): HTMLElement {
  const remote = ctx.state.draft.transport.kind === 'http';
  const kind: Kind = remote ? 'headers' : 'env';
  const nameIn = input({ type: 'text', placeholder: remote ? 'Authorization' : 'GITHUB_TOKEN', autocomplete: 'off', spellcheck: 'false', 'aria-label': 'Name' });
  const formatIn = input({ type: 'text', placeholder: 'Bearer {}', autocomplete: 'off', spellcheck: 'false', 'aria-label': 'Format' });
  const problem = h('p.mcpwz-err', { role: 'alert', hidden: true });
  const add = (): void => {
    const name = nameIn.value.trim();
    const format = remote && formatIn.value.trim() ? formatIn.value.trim() : null;
    const error =
      ownRowProblem(name, remote) ||
      (ctx.state.specs[kind].some((spec) => spec.name === name) ? `${name} is already listed.` : '') ||
      (format && !format.includes('{}') ? 'A format needs {} where the secret goes, such as Bearer {}.' : '');
    if (error) {
      problem.textContent = error;
      problem.hidden = false;
      return;
    }
    ctx.state.specs[kind] = [...ctx.state.specs[kind], { name, secret: true, required: false, help: '', format }];
    store(ctx, kind, name, { secret: '' });
    ctx.redraw();
  };
  return h(
    'div.mcpwz-card',
    null,
    h('h4', null, remote ? 'Add a header' : 'Add an environment variable'),
    h('p.muted', null, remote ? 'Its value comes from the vault.' : 'Its value comes from the vault, and the hub sets it in the container.'),
    field('Name', nameIn, remote ? 'Letters, digits and hyphens.' : 'Capitals, digits and underscores.'),
    remote ? field('Format', formatIn, 'Optional. Use {} where the secret goes.') : null,
    problem,
    btn('Add', add, { kind: 'primary', sm: true }),
  );
}

// ---- Draft access -----------------------------------------------------------

function current(ctx: WizardCtx, kind: Kind, name: string): McpSetting | undefined {
  const bag: Record<string, McpSetting> = kind === 'env' ? ctx.state.draft.env : ctx.state.draft.headers;
  return bag[name];
}

function store(ctx: WizardCtx, kind: Kind, name: string, setting: McpSetting | null): void {
  const bag: Record<string, McpSetting> = kind === 'env' ? ctx.state.draft.env : ctx.state.draft.headers;
  if (setting) bag[name] = setting;
  else delete bag[name];
}

function isSet(ctx: WizardCtx, kind: Kind, name: string): boolean {
  const s = current(ctx, kind, name);
  if (!s) return false;
  return 'secret' in s ? s.secret !== '' : s.value !== '';
}

function secretSetting(spec: McpSpec, secret: string): McpSetting {
  return spec.format ? { secret, format: spec.format } : { secret };
}

const union = (list: string[], item: string): string[] => (list.includes(item) ? list : [...list, item]);

// ---- Cards -------------------------------------------------------------------

/** A radio in a `label.check`, the prototype's choice row. */
function radioOpt(name: string, value: string, label: string, checked: boolean, onChange: () => void): HTMLLabelElement {
  const r = h('input', { type: 'radio', name, value, checked });
  r.addEventListener('change', onChange);
  return h('label.check', null, r, h('span', null, label));
}

/** The name row (code, Required, the state, Remove for the owner's rows), then the help and the format. */
function specHead(spec: McpSpec, state: Node | null, onRemove: (() => void) | null): (Node | null)[] {
  return [
    h(
      'div.mcpwz-row',
      null,
      h('code.mono', null, spec.name),
      spec.required ? badge('Required', 'warn') : null,
      state,
      onRemove ? btn('Remove', onRemove, { kind: 'ghost', sm: true }) : null,
    ),
    spec.help ? h('p.muted', null, spec.help) : null,
    spec.format ? h('p.muted', null, `Format: ${spec.format}`) : null,
  ];
}

function removeHandler(ctx: WizardCtx, kind: Kind, spec: McpSpec, own: boolean): (() => void) | null {
  return own ? () => removeOwn(ctx, kind, spec.name) : null;
}

function plainCard(ctx: WizardCtx, kind: Kind, spec: McpSpec, refreshWarn: () => void, own: boolean): HTMLElement {
  const cur = current(ctx, kind, spec.name);
  const value = cur && 'value' in cur ? cur.value : '';
  const control = input({ type: 'text', value, autocomplete: 'off', 'aria-label': 'Value' });
  control.addEventListener('input', () => {
    store(ctx, kind, spec.name, { value: control.value });
    refreshWarn();
  });
  return h('div.mcpwz-card', null, specHead(spec, null, removeHandler(ctx, kind, spec, own)), field('Value', control));
}

/**
 * A secret row: its state (Set to <vault name> or Not set), the mode (a vault secret or a new one), and the body of
 * that mode. The card is drawn again on each change, in place, so the parts stay in the prototype's layout.
 */
function secretCard(ctx: WizardCtx, kind: Kind, spec: McpSpec, refreshWarn: () => void, own: boolean): HTMLElement {
  const group = `m-${kind}|${spec.name}`;
  const pickGroup = `p-${kind}|${spec.name}`;
  const binding = mcpBinding(ctx.state.draft.id);
  const wants = new Set([...spec.name.split('_'), ...SECRET_WORDS]);
  let mode: 'vault' | 'create' = 'vault';
  let vault: 'loading' | 'sealed' | 'error' | 'ready' = 'loading';
  let vaultError = '';
  let entries: VaultEntry[] = [];
  let picked: VaultEntry | undefined;
  let showAll = false;
  let card: HTMLElement | undefined;

  const draftSecret = (): string => {
    const s = current(ctx, kind, spec.name);
    return s && 'secret' in s ? s.secret : '';
  };
  const plausible = (e: VaultEntry): boolean => e.name === spec.name || e.name.split('_').some((t) => wants.has(t));

  const repaint = (): void => {
    const next = build();
    card?.replaceWith(next);
    card = next;
  };

  const pick = (e: VaultEntry): void => {
    picked = e;
    if (draftSecret() !== e.name) {
      store(ctx, kind, spec.name, null);
      refreshWarn();
    }
    repaint();
  };

  const toggle = async (on: boolean): Promise<void> => {
    if (!picked) return;
    if (!on) {
      store(ctx, kind, spec.name, null);
    } else {
      const entry = picked;
      const bound = entry.bindings.includes(binding)
        ? entry
        : await attempt(() => vaultApi.patchEntry(entry.id, { bindings: union(entry.bindings, binding) }));
      if (!bound) {
        repaint();
        return;
      }
      entries = entries.map((e) => (e.id === bound.id ? bound : e));
      picked = bound;
      store(ctx, kind, spec.name, secretSetting(spec, bound.name));
    }
    refreshWarn();
    repaint();
  };

  const load = async (): Promise<void> => {
    let list: VaultEntry[];
    try {
      const status = await vaultApi.status();
      if (status.state !== 'unsealed') {
        vault = 'sealed';
        repaint();
        return;
      }
      list = await vaultApi.entries();
    } catch (err) {
      vault = 'error';
      vaultError = errorText(err);
      repaint();
      return;
    }
    entries = list;
    vault = 'ready';
    picked = list.find((e) => e.name === draftSecret());
    repaint();
  };

  const onMode = (next: 'vault' | 'create') => (): void => {
    mode = next;
    if (next === 'vault') {
      vault = 'loading';
      void load();
    }
    repaint();
  };

  const createBody = (): Node[] => {
    const name = spec.name.toUpperCase().replace(/[-.]/g, '_');
    return [
      h('p.muted', null, `Creates a vault secret named ${name} and binds it to this server.`),
      btn(
        'Create secret',
        () =>
          addSecretDialog({ name, bindings: [binding] }, (entry) => {
            store(ctx, kind, spec.name, secretSetting(spec, entry.name));
            ctx.redraw();
          }),
        { kind: 'primary', sm: true },
      ),
    ];
  };

  const vaultBody = (): (Node | null)[] => {
    if (vault === 'loading') return [h('p.muted', null, 'Loading vault secrets')];
    if (vault === 'sealed') return [h('p.muted', null, 'Unseal the vault to use secrets.')];
    if (vault === 'error') return [badge(vaultError, 'danger')];
    const shown = entries.filter((e) => showAll || plausible(e) || e.id === picked?.id);
    const consented = !!picked && draftSecret() === picked.name && picked.bindings.includes(binding);
    const listed: Node[] =
      entries.length === 0
        ? [h('p.muted', null, 'The vault has no secrets yet. Create one instead.')]
        : shown.length === 0
          ? [h('p.muted', null, 'No secret has a plausible name. Tick Show all.')]
          : shown.map((e) => radioOpt(pickGroup, e.id, `${e.name} · ${e.kind}`, picked?.id === e.id, () => pick(e)));
    return [
      check(showAll ? 'Show fewer' : 'Show all', {
        checked: showAll,
        onChange: (on) => {
          showAll = on;
          repaint();
        },
      }),
      ...listed,
      picked
        ? check(`Allow the ${ctx.state.draft.id} server to use ${picked.name}`, { checked: consented, onChange: (on) => void toggle(on) })
        : null,
    ];
  };

  const build = (): HTMLElement => {
    const s = current(ctx, kind, spec.name);
    const secret = s && 'secret' in s ? s.secret : '';
    const state = badge(secret ? `Set to ${secret}` : 'Not set', secret ? 'ok' : 'muted');
    return h(
      'div.mcpwz-card',
      null,
      specHead(spec, state, removeHandler(ctx, kind, spec, own)),
      h(
        'div.mcpwz-row',
        null,
        radioOpt(group, 'vault', 'Use a vault secret', mode === 'vault', onMode('vault')),
        radioOpt(group, 'create', 'Create a new secret', mode === 'create', onMode('create')),
      ),
      mode === 'create' ? createBody() : vaultBody(),
    );
  };

  card = build();
  void load();
  return card;
}

