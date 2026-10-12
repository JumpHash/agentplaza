// Models > Providers: the hub where the owner configures every provider.
// A grid of provider cards, grouped by how each one is credentialed.

import '../styles/models.css';
import { api, isDecisionKind, type Auth, type Provider, type ProviderKind, type TestReport } from '../api';
import { attempt, badge, brandLogo, button, empty, field, input, modal, toast, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { decisionPanel } from './models-decisions';

type Group = 'Configured with a key' | 'Needs a key' | 'Local';
const GROUPS: Group[] = ['Configured with a key', 'Needs a key', 'Local'];

const CREDENTIAL: Record<Provider['credential'], [string, Tone]> = {
  env: ['Key set', 'ok'],
  secret: ['Key set', 'ok'],
  none: ['No key needed', 'muted'],
  command: ['Command', 'info'],
  missing: ['Key missing', 'warn'],
};

/** The kind entry a configured provider was made from: same id, else the first of the same kind. */
function kindOf(p: Provider, kinds: ProviderKind[]): ProviderKind | undefined {
  return kinds.find((k) => k.id === p.id) ?? kinds.find((k) => k.kind === p.kind);
}

/** Decision providers sit under the Decisions heading; the rest are chat. */
function isDecision(p: Provider): boolean {
  return p.group ? p.group === 'decisions' : isDecisionKind(p.kind);
}

function groupOf(p: Provider): Group {
  if (p.credential === 'missing') return 'Needs a key';
  if (p.credential === 'none') return 'Local';
  return 'Configured with a key';
}

function splitList(text: string): string[] {
  return text.split(',').map((s) => s.trim()).filter(Boolean);
}

interface Draft {
  id: string;
  base_url: string;
  auth: Auth;
  /** null hides the models field. */
  models: string[] | null;
  idEditable: boolean;
  keyField: boolean;
  notes?: string;
}

interface Values {
  id: string;
  base_url: string | null;
  auth: Auth;
  models: string[] | null;
  key: string;
}

export function mountProviders(root: HTMLElement): Cleanup {
  let kinds: ProviderKind[] = [];
  let providers: Provider[] = [];

  const body = h('div.prov-body', null, h('p.faint', null, 'Loading providers…'));
  const add = button('Add provider', () => openAdd(), { kind: 'primary' });
  root.append(h('div.prov-bar', null, add), body);

  /** The decision model card and Try-it form, under the Decisions heading. */
  const panel = decisionPanel();

  const refresh = async (): Promise<void> => {
    const [k, p] = await Promise.all([attempt(() => api.providerKinds()), attempt(() => api.providers())]);
    if (k) kinds = k;
    if (p) providers = p;
    render();
    void panel.reload();
  };

  const render = (): void => {
    if (!providers.length) {
      replace(body, empty('models', 'No providers yet'));
      return;
    }
    const groups: Record<Group, Provider[]> = { 'Configured with a key': [], 'Needs a key': [], Local: [] };
    const chat = providers.filter((p) => !isDecision(p));
    for (const p of chat) groups[groupOf(p)].push(p);
    const chatSections = GROUPS.filter((g) => groups[g].length > 0).map((g) =>
      h(
        'section.prov-group',
        null,
        h('h2.prov-group-title', null, g),
        h('div.prov-grid', null, groups[g].map(providerCard)),
      ),
    );
    const decisionCards = providers.filter(isDecision);
    replace(
      body,
      h('section.prov-section', { 'data-key': 'chat-models' }, h('h2.prov-section-title', null, 'Chat models'), ...chatSections),
      h(
        'section.prov-section',
        { 'data-key': 'decision-models' },
        h('h2.prov-section-title', null, 'Decisions'),
        h(
          'p.prov-lede',
          null,
          'Models that answer typed questions with calibrated probabilities instead of writing text. The plaza uses them to decide; agents never chat with them.',
        ),
        decisionCards.length ? h('div.prov-grid', null, decisionCards.map(providerCard)) : null,
        panel.el,
      ),
    );
  };

  const providerCard = (p: Provider): HTMLElement => {
    const kind = kindOf(p, kinds);
    const [text, tone] = CREDENTIAL[p.credential];

    const models = h('div.prov-models');
    models.hidden = true;
    let loaded = false;
    const modelsBtn = button(
      '',
      async () => {
        models.hidden = !models.hidden;
        if (models.hidden || loaded) return;
        const list = await attempt(() => api.providerModels(p.id));
        if (!list) return;
        loaded = true;
        replace(
          models,
          list.length
            ? h('ul.prov-model-list', null, list.map((m) => h('li.mono', { 'data-key': `model-${m.id}` }, m.id)))
            : h('p.faint', null, 'No models listed'),
        );
      },
      { size: 'sm', kind: 'ghost', icon: 'models', title: 'Models' },
    );

    return h(
      'article.prov-card',
      { 'data-key': `prov-${p.id}` },
      h(
        'header.prov-head',
        null,
        brandLogo(kind?.brand ?? 'mock', true),
        h(
          'div.prov-title',
          null,
          h('h3.truncate', null, kind?.label ?? p.id),
          h('span.faint.mono.truncate', { title: p.base_url ?? '' }, `${p.id} · ${p.base_url ?? '—'}`),
        ),
      ),
      h(
        'div.prov-meta',
        null,
        badge(text, tone, true),
        h('span.faint', null, `${p.models.length} ${p.models.length === 1 ? 'model' : 'models'}`),
      ),
      h(
        'div.prov-actions',
        null,
        button('Test', () => void runTest(p), { size: 'sm' }),
        button(p.credential === 'missing' ? 'Add key' : 'Key', () => openKey(p), { size: 'sm' }),
        h('span.prov-spacer'),
        modelsBtn,
        button('', () => openEdit(p), { size: 'sm', kind: 'ghost', icon: 'edit', title: 'Edit' }),
        button('', () => confirmRemove(p), { size: 'sm', kind: 'ghost', icon: 'trash', title: 'Remove' }),
      ),
      models,
    );
  };

  const runTest = async (p: Provider): Promise<void> => {
    const report = await attempt<TestReport>(() => api.testProvider(p.id));
    if (!report) return;
    if (report.ok) toast(`OK · ${report.latency_ms} ms · ${report.model}`);
    else toast(report.error ?? 'Test failed', 'error');
  };

  const openKey = (p: Provider): void => {
    let close = () => {};
    const value = input({ type: 'password', autocomplete: 'new-password', placeholder: 'API key' });
    const save = button(
      'Save key',
      async () => {
        const key = value.value.trim();
        if (!key) return;
        save.disabled = true;
        const saved = await attempt(() => api.setSecret(p.id, key), 'Key saved');
        save.disabled = false;
        if (saved === undefined) return;
        value.value = '';
        close();
        await refresh();
      },
      { kind: 'primary' },
    );
    value.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') save.click();
    });
    close = modal(
      `Set key · ${p.id}`,
      h('div.stack', null, field('API key', value), h('div.prov-row', null, h('span.prov-spacer'), save)),
    );
  };

  const confirmRemove = (p: Provider): void => {
    let close = () => {};
    const yes = button(
      'Remove provider',
      async () => {
        const removed = await attempt(() => api.removeProvider(p.id), `Removed ${p.id}`);
        if (removed === undefined) return;
        close();
        await refresh();
      },
      { kind: 'danger' },
    );
    const no = button('Cancel', () => close(), {});
    close = modal(
      `Remove ${p.id}?`,
      h('div.stack', null, h('div.prov-row', null, h('span.prov-spacer'), no, yes)),
    );
  };

  /** Shared add/edit form. `save` returns true when the change landed, which closes the form. */
  const openForm = (title: string, draft: Draft, submitLabel: string, save: (v: Values) => Promise<boolean>): void => {
    let close = () => {};
    const idInput = input({ value: draft.id, placeholder: 'provider id', autocomplete: 'off', disabled: !draft.idEditable });
    const urlInput = input({ value: draft.base_url, placeholder: 'https://', autocomplete: 'off' });
    const envName = typeof draft.auth === 'object' && 'env' in draft.auth ? draft.auth.env : null;
    const envInput = envName === null ? null : input({ value: envName, placeholder: 'ENV_VAR_NAME', autocomplete: 'off' });
    const modelsInput = draft.models === null ? null : input({ value: draft.models.join(', '), autocomplete: 'off' });
    const keyInput = draft.keyField ? input({ type: 'password', autocomplete: 'new-password', placeholder: 'Optional' }) : null;

    const fields: Node[] = [field('Provider id', idInput), field('Base URL', urlInput)];
    if (envInput) fields.push(field('Auth env var', envInput));
    if (modelsInput) fields.push(field('Models', modelsInput, 'Comma-separated'));
    if (keyInput) fields.push(field('API key', keyInput, 'Optional'));

    const submit = button(
      submitLabel,
      async () => {
        const id = idInput.value.trim();
        if (!id) {
          toast('Give the provider an id', 'error');
          return;
        }
        submit.disabled = true;
        const done = await save({
          id,
          base_url: urlInput.value.trim() || null,
          auth: envInput ? { env: envInput.value.trim() } : draft.auth,
          models: modelsInput ? splitList(modelsInput.value) : null,
          key: keyInput?.value.trim() ?? '',
        });
        submit.disabled = false;
        if (done) close();
      },
      { kind: 'primary' },
    );

    close = modal(
      title,
      h(
        'div.stack',
        null,
        draft.notes ? h('p.faint', null, draft.notes) : null,
        ...fields,
        h('div.prov-row', null, h('span.prov-spacer'), submit),
      ),
    );
  };

  const openEdit = (p: Provider): void => {
    const kind = kindOf(p, kinds);
    openForm(
      `Edit ${p.id}`,
      {
        id: p.id,
        base_url: p.base_url ?? '',
        auth: p.auth,
        models: p.models,
        idEditable: false,
        keyField: false,
        notes: kind?.notes,
      },
      'Save',
      async (v) => {
        const saved = await attempt(
          () =>
            api.updateProvider({
              id: p.id,
              kind: p.kind,
              base_url: v.base_url,
              auth: v.auth,
              models: v.models ?? p.models,
              options: p.options ?? undefined,
              extra_headers: p.extra_headers,
            }),
          'Saved',
        );
        if (saved === undefined) return false;
        await refresh();
        return true;
      },
    );
  };

  const openKindForm = (k: ProviderKind): void => {
    openForm(
      k.label,
      {
        id: k.id,
        base_url: k.base_url ?? '',
        auth: k.auth,
        models: null,
        idEditable: true,
        keyField: true,
        notes: k.notes,
      },
      'Add provider',
      async (v) => {
        const created = await attempt(
          () =>
            api.addProvider({
              id: v.id,
              kind: k.kind,
              base_url: v.base_url,
              auth: v.auth,
              models: k.models,
              options: k.options,
            }),
          `Added ${v.id}`,
        );
        if (created === undefined) return false;
        if (v.key) await attempt(() => api.setSecret(v.id, v.key), 'Key saved');
        await refresh();
        return true;
      },
    );
  };

  const openAdd = (): void => {
    let close = () => {};
    const taken = new Set(providers.map((p) => p.id));
    const free = kinds.filter((k) => !taken.has(k.id));
    const search = input({ type: 'search', placeholder: 'Search providers', autocomplete: 'off' });
    const grid = h('div.stack');

    const pick = (k: ProviderKind): void => {
      close();
      openKindForm(k);
    };

    const draw = (): void => {
      const q = search.value.trim().toLowerCase();
      const shown = free.filter((k) => !q || `${k.label} ${k.id} ${k.kind}`.toLowerCase().includes(q));
      const tile = (k: ProviderKind): HTMLElement => {
        const el = h('button.prov-kind', { type: 'button', 'data-key': `kind-${k.id}` }, brandLogo(k.brand, true), h('span.truncate', null, k.label));
        el.addEventListener('click', () => pick(k));
        return el;
      };
      const section = (title: string, list: ProviderKind[]): HTMLElement[] =>
        list.length ? [h('h3.prov-kind-title', null, title), h('div.prov-kind-grid', null, list.map(tile))] : [];
      const chatKinds = shown.filter((k) => !isDecisionKind(k.kind));
      const decisionKinds = shown.filter((k) => isDecisionKind(k.kind));
      replace(
        grid,
        ...(shown.length
          ? [...section('Chat', chatKinds), ...section('Decisions', decisionKinds)]
          : [h('p.faint', null, free.length ? 'No matches' : 'Every provider is already configured')]),
      );
    };

    search.addEventListener('input', draw);
    close = modal('Add provider', h('div.stack', null, search, grid));
    draw();
  };

  void refresh();
  return () => undefined;
}
