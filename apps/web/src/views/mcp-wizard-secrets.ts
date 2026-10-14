// Step 2 of the MCP wizard: the credentials each preset spec needs. A secret is either an existing
// vault entry (picked, then explicitly bound to this server) or a new one made in addSecretDialog.
// A plain setting is typed in and never goes to the vault. No value is ever shown or logged here.
import '../styles/mcp-wizard-secrets.css';
import { vaultApi, type McpSetting, type McpSpec, type VaultEntry } from '../api';
import { h, replace } from '../ui/dom';
import { attempt, badge, button, field, input } from '../ui/components';
import { mcpBinding, ownRowProblem } from './mcp-common';
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
  const warn = h('div.wsec-warn', { role: 'status' });
  const refreshWarn = (): void => {
    const missing = rows.filter(({ kind, spec }) => spec.required && !isSet(ctx, kind, spec.name));
    replace(
      warn,
      missing.length
        ? h('div.wsec-warn-box', null, h('strong', null, 'Required and not set'), h('ul', null, missing.map(({ spec }) => h('li', { class: 'mono' }, spec.name))))
        : null,
    );
  };
  // A custom or remote server's rows are the owner's own, so each can be removed.
  const own = ctx.state.ownRows;
  const cards = rows.map(({ kind, spec }) => {
    const card = spec.secret ? secretCard(ctx, kind, spec, refreshWarn) : plainCard(ctx, kind, spec, refreshWarn);
    return own ? h('div.wsec-own', null, card, button('Remove', () => removeOwn(ctx, kind, spec.name), { kind: 'ghost', size: 'sm' })) : card;
  });
  refreshWarn();
  return h(
    'div.wsec.stack',
    null,
    cards.length ? cards : h('p.muted', null, 'This server needs no credentials.'),
    own ? ownRowForm(ctx) : null,
    warn,
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
  const nameIn = input({ type: 'text', placeholder: remote ? 'Authorization' : 'GITHUB_TOKEN', autocomplete: 'off', spellcheck: 'false' });
  const formatIn = input({ type: 'text', placeholder: 'Bearer {}', autocomplete: 'off', spellcheck: 'false' });
  const problem = h('p.mcpw-warn', { role: 'alert' });
  const add = (): void => {
    const name = nameIn.value.trim();
    const format = remote && formatIn.value.trim() ? formatIn.value.trim() : null;
    const error =
      ownRowProblem(name, remote) ||
      (ctx.state.specs[kind].some((spec) => spec.name === name) ? `${name} is already listed.` : '') ||
      (format && !format.includes('{}') ? 'A format needs {} where the secret goes, such as Bearer {}.' : '');
    if (error) {
      replace(problem, error);
      return;
    }
    ctx.state.specs[kind] = [...ctx.state.specs[kind], { name, secret: true, required: false, help: '', format }];
    store(ctx, kind, name, { secret: '' });
    ctx.redraw();
  };
  return h(
    'div.wsec-spec',
    null,
    h('strong', null, remote ? 'Add a header' : 'Add an environment variable'),
    h('p.muted', null, remote ? 'Its value comes from the vault.' : 'Its value comes from the vault, and the hub sets it in the container.'),
    field('Name', nameIn, remote ? 'Letters, digits and hyphens.' : 'Capitals, digits and underscores.'),
    remote ? field('Format', formatIn, 'Optional. Use {} where the secret goes.') : null,
    problem,
    button('Add', add, { kind: 'primary', size: 'sm' }),
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

function specHead(spec: McpSpec): HTMLElement {
  return h(
    'div.wsec-head',
    null,
    h('span.mono.wsec-name', null, spec.name),
    spec.required ? badge('Required', 'warn') : null,
    spec.help ? h('p.wsec-help', null, spec.help) : null,
    spec.format ? h('p.wsec-hint', null, `Format: ${spec.format}`) : null,
  );
}

function plainCard(ctx: WizardCtx, kind: Kind, spec: McpSpec, refreshWarn: () => void): HTMLElement {
  const cur = current(ctx, kind, spec.name);
  const value = cur && 'value' in cur ? cur.value : '';
  const control = input({ type: 'text', value, autocomplete: 'off' });
  control.addEventListener('input', () => {
    store(ctx, kind, spec.name, { value: control.value });
    refreshWarn();
  });
  return h('div.wsec-spec', null, specHead(spec), field('Value', control));
}

function secretCard(ctx: WizardCtx, kind: Kind, spec: McpSpec, refreshWarn: () => void): HTMLElement {
  const group = `wsec-mode-${kind}-${spec.name}`;
  const stateLine = h('div.wsec-state');
  const body = h('div.wsec-body');
  let mode: 'vault' | 'create' = 'vault';

  const paintState = (): void => {
    const s = current(ctx, kind, spec.name);
    const secret = s && 'secret' in s ? s.secret : '';
    replace(stateLine, badge(secret ? `Set to ${secret}` : 'Not set', secret ? 'ok' : 'muted', true));
  };
  const paintBody = (): void => {
    replace(body, mode === 'vault' ? vaultPick(ctx, kind, spec, paintState, refreshWarn) : createPick(ctx, kind, spec));
  };
  const onMode = (next: 'vault' | 'create') => () => {
    mode = next;
    paintBody();
  };

  paintState();
  paintBody();
  return h(
    'div.wsec-spec',
    null,
    specHead(spec),
    h(
      'div.wsec-modes',
      { role: 'radiogroup' },
      radio(group, 'vault', 'Use a vault secret', true, onMode('vault')),
      radio(group, 'create', 'Create a new secret', false, onMode('create')),
    ),
    stateLine,
    body,
  );
}

function radio(name: string, value: string, label: string, checked: boolean, onChange: () => void): HTMLLabelElement {
  const r = h('input', { type: 'radio', name, value, checked });
  r.addEventListener('change', onChange);
  return h('label.wsec-opt', null, r, h('span', null, label));
}

function createPick(ctx: WizardCtx, kind: Kind, spec: McpSpec): HTMLElement {
  const name = spec.name.toUpperCase().replace(/[-.]/g, '_');
  return h(
    'div.wsec-create',
    null,
    h('p.muted', null, `Creates a vault secret named ${name} and binds it to this server.`),
    button(
      'Create secret',
      () =>
        addSecretDialog({ name, bindings: [mcpBinding(ctx.state.draft.id)] }, (entry) => {
          store(ctx, kind, spec.name, secretSetting(spec, entry.name));
          ctx.redraw();
        }),
      { kind: 'primary', size: 'sm' },
    ),
  );
}

function vaultPick(ctx: WizardCtx, kind: Kind, spec: McpSpec, paintState: () => void, refreshWarn: () => void): HTMLElement {
  const box = h('div.wsec-pick', null, h('p.muted', null, 'Loading vault secrets'));
  void fillPick(box, ctx, kind, spec, paintState, refreshWarn);
  return box;
}

async function fillPick(
  box: HTMLElement,
  ctx: WizardCtx,
  kind: Kind,
  spec: McpSpec,
  paintState: () => void,
  refreshWarn: () => void,
): Promise<void> {
  const binding = mcpBinding(ctx.state.draft.id);
  const wants = new Set([...spec.name.split('_'), ...SECRET_WORDS]);
  const group = `wsec-pick-${kind}-${spec.name}`;
  let entries: VaultEntry[] = [];
  let picked: VaultEntry | undefined;
  let showAll = false;

  const draftSecret = (): string => {
    const s = current(ctx, kind, spec.name);
    return s && 'secret' in s ? s.secret : '';
  };
  const plausible = (e: VaultEntry): boolean => e.name === spec.name || e.name.split('_').some((t) => wants.has(t));

  const pick = (e: VaultEntry): void => {
    picked = e;
    if (draftSecret() !== e.name) {
      store(ctx, kind, spec.name, null);
      paintState();
      refreshWarn();
    }
    paint();
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
        paint();
        return;
      }
      entries = entries.map((e) => (e.id === bound.id ? bound : e));
      picked = bound;
      store(ctx, kind, spec.name, secretSetting(spec, bound.name));
    }
    paintState();
    refreshWarn();
    paint();
  };

  const paint = (): void => {
    const shown = entries.filter((e) => showAll || plausible(e) || e.id === picked?.id);
    const consented = !!picked && draftSecret() === picked.name && picked.bindings.includes(binding);
    const all = h('input', { type: 'checkbox', checked: showAll });
    all.addEventListener('change', () => {
      showAll = all.checked;
      paint();
    });
    const consent = picked
      ? h(
          'label.wsec-consent',
          null,
          (() => {
            const c = h('input', { type: 'checkbox', checked: consented });
            c.addEventListener('change', () => void toggle(c.checked));
            return c;
          })(),
          h('span', null, `Allow the ${ctx.state.draft.id} server to use ${picked.name}`),
        )
      : null;
    replace(
      box,
      entries.length === 0
        ? h('p.muted', null, 'The vault has no secrets yet. Create one instead.')
        : [
            h('label.wsec-opt', null, all, h('span', null, showAll ? 'Show fewer' : 'Show all')),
            shown.length === 0
              ? h('p.muted', null, 'No secret has a plausible name. Tick Show all.')
              : h(
                  'ul.wsec-list',
                  null,
                  shown.map((e) => h('li', null, radio(group, e.id, `${e.name} · ${e.kind}`, picked?.id === e.id, () => pick(e)))),
                ),
            consent,
          ],
    );
  };

  try {
    const status = await vaultApi.status();
    if (status.state !== 'unsealed') {
      replace(box, h('p.muted', null, 'Unseal the vault to use secrets.'));
      return;
    }
    entries = await vaultApi.entries();
  } catch (err) {
    replace(box, badge(errorText(err), 'danger'));
    return;
  }
  picked = entries.find((e) => e.name === draftSecret());
  paint();
}
