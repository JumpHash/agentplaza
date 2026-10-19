// Models > Providers: the providers manager. One card lists every provider instance (the API instances, then the coding
// engines) beside the selected one's detail; the cards below hold the usage providers (CLIProxyAPI hubs), the health-check
// interval and the decision models. The data is the hub's (api-providers.ts) and the event stream keeps it live. The
// markup is the Loom and Graphite kit's (styles/proto/base.css, providers.css).

import '../styles/proto/providers.css';
import { api, contributorIds, type ModelInfo, type ProviderKind, type Settings } from '../api';
import { type ApiInstance, type ApiInstanceInput, type EngineEdit, type EngineInstance, type Health, type HealthStatus, type UsageProvider, lastChecked, providersApi } from '../api-providers';
import { ago, attempt, badge, brandLogo, button, card, compact, dot, empty, field, input, modal, pmono, select, textarea, toast, type Tone } from '../ui/components';
import { BRAND_ICONS } from '../ui/brands.gen';
import { h, lu, replace } from '../ui/dom';
import { choicesFrom, modelPicker } from '../ui/model-picker';
import { on } from '../store';
import type { Cleanup } from './index';
import { decisionPanel } from './models-decisions';
import { openProviderWizard } from './provider-wizard';

/** The note on a contributor model: its vendor trains on what is sent to it. Shared by the policy view. */
export const CONTRIBUTOR_NOTE = 'Contributor models: their vendor trains future models on what is sent to and returned by them.';

/** The accents an instance can take (the Display name's swatch popover), and no accent. */
const ACCENTS = ['#5cc8ff', '#7ee0b5', '#b69cff', '#ff9ac1', '#ffd36e', '#6fe3e1', '#a3b8ff', '#c5f27a'];
const TONE: Record<HealthStatus, Tone> = { ok: 'ok', warn: 'warn', crit: 'danger', off: 'muted', unknown: 'muted' };
const CREDENTIAL: Record<ApiInstance['credential'], string> = {
  env: 'Read from the environment',
  secret: 'Key set in the vault',
  none: 'No key needed',
  missing: 'No key',
  command: 'Read from a command',
  sealed: 'Vault sealed',
  out_of_scope: 'Held by another node',
};
const SOURCE: Record<string, string> = { npm: 'npm package', release: 'Release', pip: 'PyPI', command: 'Command', catalogue: 'Catalogue' };
/** How many of a model list's rows get a price lookup. */
const PRICED_MODELS = 20;
const HEALTH_STEP = 60;
const HEALTH_MAX = 86_400;

/** The mark a built-in engine draws: its maker's (Claude Code is Claude's, Codex is OpenAI's). */
const ENGINE_BRAND: Record<string, string> = { claude: 'claude', codex: 'openai', gemini: 'google', opencode: 'opencode' };

/** One row of the list: an API instance or a coding engine. */
type Item = { family: 'api'; api: ApiInstance } | { family: 'engine'; eng: EngineInstance };

const view = (it: Item): ApiInstance | EngineInstance => (it.family === 'api' ? it.api : it.eng);
const keyOf = (it: Item): string => (it.family === 'api' ? `api:${it.api.id}` : `engine:${it.eng.id}`);

/** The auth-free part of an API instance's config, as `PUT /v1/providers/{id}` takes it (redacted values come back restored). */
const apiConfig = (a: ApiInstance): ApiInstanceInput => ({
  id: a.id,
  kind: a.kind,
  label: a.label,
  accent: a.accent,
  enabled: a.enabled,
  base_url: a.base_url,
  auth: a.auth,
  models: a.models,
  options: a.options,
  extra_headers: a.extra_headers,
  extra_body: a.extra_body,
});

/** The initials of a name: its first two words, else its first two letters. */
function initials(text: string): string {
  const words = text.split(/[\s_-]+/).filter(Boolean);
  const pick = words.length > 1 ? `${words[0]?.[0] ?? ''}${words[1]?.[0] ?? ''}` : text.slice(0, 2);
  return (pick || '?').toUpperCase();
}

/** A monogram tile in the instance's accent (the kit's `pmono` look, with the accent as its hue). */
function monogram(label: string, accent: string | null, large = false): HTMLElement {
  return h('span.prov-tile', { class: large ? 'lg' : '', style: accent ? `--tile:${accent}` : null, 'aria-hidden': 'true' }, initials(label));
}

/** A menu entry in a popover: its mark, its label. */
function menuItem(mark: Node, text: string, onPick: () => void, current = false): HTMLButtonElement {
  const b = h('button', { type: 'button', role: 'menuitem', 'aria-current': current ? 'true' : null }, mark, h('span', null, text));
  b.addEventListener('click', onPick);
  return b;
}

/** The modal with a footer row of buttons: `modal()` takes the footer itself, but callers here keep their `close`. */
export function sheet(title: string, body: Node, foot: Node[] = []): () => void {
  return modal(title, body, undefined, foot);
}

function accountTone(status: string): Tone {
  if (status === 'ok' || status === 'active' || status === 'ready') return 'ok';
  if (status === 'warn' || status === 'cooldown' || status === 'limited') return 'warn';
  if (status === 'crit' || status === 'error' || status === 'disabled') return 'danger';
  return 'muted';
}

export function mountProviders(root: HTMLElement, actions: HTMLElement): Cleanup {
  // The Providers tab draws its actions in its cards; the page head keeps none.
  replace(actions);

  let apis: ApiInstance[] = [];
  let engines: EngineInstance[] = [];
  let usage: UsageProvider[] = [];
  let kinds: ProviderKind[] = [];
  let chatModels: ModelInfo[] = [];
  let settings: Settings | null = null;
  // `?instance=<id>` (the model picker's "Open provider setup") opens on that instance: an API one first, else an engine.
  const asked = new URLSearchParams(location.hash.split('?')[1] ?? '').get('instance');
  let selected: string | null = null;
  let loaded = false;
  let closed = false;
  let healthSecs = 300;
  /** The detail's live status line, and a token that drops a model list which arrives after the selection moved. */
  let detailStatus: HTMLElement | null = null;
  let detailToken = 0;
  let liveTimer: ReturnType<typeof setTimeout> | undefined;
  let saveTimer: ReturnType<typeof setTimeout> | undefined;

  const items = (): Item[] => {
    const byEnabled = <T extends { enabled: boolean }>(list: T[]): T[] => [...list.filter((x) => x.enabled), ...list.filter((x) => !x.enabled)];
    return [...byEnabled(apis).map((a): Item => ({ family: 'api', api: a })), ...byEnabled(engines).map((e): Item => ({ family: 'engine', eng: e }))];
  };
  const current = (): Item | undefined => items().find((it) => keyOf(it) === selected);
  const brandOf = (a: ApiInstance): string => kinds.find((k) => k.id === a.id)?.brand ?? kinds.find((k) => k.kind === a.kind)?.brand ?? a.kind;
  const kindLabel = (a: ApiInstance): string => kinds.find((k) => k.kind === a.kind)?.label ?? a.kind;

  /** The brand mark where the kit has one; otherwise a monogram in the accent. */
  const tileOf = (it: Item, large = false): HTMLElement => {
    if (it.family === 'api') {
      const brand = brandOf(it.api);
      if (BRAND_ICONS[brand]) return brandLogo(brand, large);
    } else {
      const brand = ENGINE_BRAND[it.eng.engine];
      if (brand && BRAND_ICONS[brand]) return brandLogo(brand, large);
    }
    const base = view(it);
    return monogram(base.label, base.accent, large);
  };

  // ---- Data -----------------------------------------------------------------

  const loadAll = async (): Promise<void> => {
    const [a, e, u, k, m, s] = await Promise.all([
      attempt(() => providersApi.list()),
      attempt(() => providersApi.engines()),
      attempt(() => providersApi.usage()),
      attempt(() => api.providerKinds()),
      attempt(() => api.models()),
      attempt(() => api.settings()),
    ]);
    if (closed) return;
    if (a) apis = a;
    if (e) engines = e;
    if (u) usage = u;
    if (k) kinds = k;
    if (m) chatModels = m.models;
    if (s) {
      settings = s;
      if (!loaded) healthSecs = s.health_interval_s ?? 300;
    }
    loaded = true;
    const all = items();
    if (asked && selected === null) {
      const hit = all.find((it) => keyOf(it) === `api:${asked}`) ?? all.find((it) => keyOf(it) === `engine:${asked}`);
      if (hit) selected = keyOf(hit);
    }
    if (!all.some((it) => keyOf(it) === selected)) selected = all[0] ? keyOf(all[0]) : null;
  };

  /** Saves one API instance's change, then reloads. */
  const saveApi = async (a: ApiInstance, patch: Partial<ApiInstanceInput>, done?: string): Promise<boolean> => {
    const saved = await attempt(() => providersApi.replace({ ...apiConfig(a), ...patch }), done);
    if (saved === undefined) return false;
    await refresh();
    return true;
  };

  /** Saves one engine's change, then reloads. */
  const saveEngine = async (e: EngineInstance, edit: EngineEdit, done?: string): Promise<boolean> => {
    const saved = await attempt(() => providersApi.editEngine(e.id, edit), done);
    if (saved === undefined) return false;
    await refresh();
    return true;
  };

  const saveItem = (it: Item, patch: Partial<ApiInstanceInput> & EngineEdit, done?: string): Promise<boolean> =>
    it.family === 'api' ? saveApi(it.api, patch, done) : saveEngine(it.eng, patch, done);

  const setEnabled = (it: Item, on: boolean): void => {
    void saveItem(it, { enabled: on });
  };

  /** The favourites live in the settings, shared with every picker. */
  const toggleFavourite = async (key: string, pressed: (on: boolean) => void): Promise<void> => {
    const s = await attempt(() => api.settings());
    if (!s) return;
    const favourites = new Set(s.favourite_models ?? []);
    if (favourites.has(key)) favourites.delete(key);
    else favourites.add(key);
    s.favourite_models = [...favourites];
    const saved = await attempt(() => api.saveSettings(s));
    if (!saved) return;
    settings = saved;
    pressed(favourites.has(key));
  };

  // ---- Repaint --------------------------------------------------------------

  const checkedLabel = h('span', null, 'Checked just now');
  const checkedBtn = h('button.btn.ghost.sm', { type: 'button', title: 'Check every provider now' }, lu('refresh-cw', 14), checkedLabel);
  const updateBtn = button('Update all', () => void updateAll(), { kind: 'ghost', size: 'sm' });
  const list = h('div.prov-list');
  const detail = h('div.prov-detail');

  const paintHead = (): void => {
    const at = lastChecked([...apis, ...engines]);
    checkedLabel.textContent = at ? `Checked ${ago(at)}` : 'Not checked yet';
  };

  const listRow = (it: Item): HTMLElement => {
    const key = keyOf(it);
    const base = view(it);
    const version = it.family === 'engine' ? it.eng.version : null;
    const tgl = h('button.tgl', {
      type: 'button',
      role: 'switch',
      'aria-checked': base.enabled ? 'true' : 'false',
      'aria-label': `${base.label} enabled`,
      title: base.enabled ? 'Turn off' : 'Turn on',
    });
    tgl.addEventListener('click', (e) => {
      e.stopPropagation();
      setEnabled(it, !base.enabled);
    });
    const row = h(
      'div.row.clickable.prov-row',
      { role: 'button', tabindex: '0', 'data-key': `prov-${key}`, 'aria-current': key === selected ? 'true' : null, class: base.enabled ? '' : 'off' },
      tileOf(it),
      h(
        'span.row-main',
        null,
        h('span.prov-line', null, h('span.row-t', null, base.label), version ? h('span.mono.prov-ver', null, version) : null),
        h('span.row-s', null, dot(TONE[base.health.status]), h('span', null, base.health.message)),
      ),
      tgl,
    );
    row.addEventListener('click', () => choose(key));
    row.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        choose(key);
      }
    });
    return row;
  };

  const paintList = (): void => {
    const add = h('button.row.prov-add', { type: 'button' }, lu('plus', 15), h('span', null, 'Add provider'));
    add.addEventListener('click', () => openProviderWizard({ onDone: () => void refresh() }));
    replace(list, items().map(listRow), add);
  };

  const choose = (key: string): void => {
    selected = key;
    paintList();
    paintDetail();
  };

  // ---- Detail ---------------------------------------------------------------

  /** A detail section: a heading and its rows. */
  const section = (title: string, ...children: (Node | null)[]): HTMLElement => h('section.prov-sec', null, h('h4', null, title), ...children);

  const paintStatus = (): void => {
    const it = current();
    if (!it || !detailStatus) return;
    const health: Health = view(it).health;
    replace(detailStatus, dot(TONE[health.status]), h('span', null, health.message));
  };

  const paintDetail = (): void => {
    const token = ++detailToken;
    const it = current();
    if (!it) {
      detailStatus = null;
      replace(detail, empty('models', 'No providers yet', 'Add a provider to start running models.'));
      return;
    }
    const base = view(it);
    detailStatus = h('p.prov-status');
    const nameInput = input({ value: base.label, 'aria-label': 'Display name', maxlength: 60, autocomplete: 'off' });
    nameInput.addEventListener('change', () => {
      const label = nameInput.value.trim();
      if (!label) {
        nameInput.value = base.label;
        return;
      }
      if (label !== base.label) void saveItem(it, { label });
    });
    const swatch = h('button.prov-swatch', { type: 'button', title: 'Accent colour', 'aria-label': 'Accent colour', style: base.accent ? `--swatch:${base.accent}` : null });
    swatch.addEventListener('click', () => void openAccents(swatch, it));

    const head = h(
      'div.prov-dh',
      null,
      tileOf(it, true),
      h('div.prov-dh-t', null, h('h3', null, base.label), h('p.prov-sub', null, subtitle(it))),
      it.family === 'engine' ? versionHead(it.eng) : null,
    );

    const parts: (Node | null)[] = [
      head,
      section('Display name', h('div.prov-name', null, swatch, nameInput), detailStatus),
    ];
    if (it.family === 'api') {
      parts.push(setupApi(it.api));
      parts.push(headersSection(it.api));
    } else {
      parts.push(setupEngine(it.eng));
      parts.push(runtimeSection(it.eng));
      parts.push(variablesSection(it.eng));
    }
    const models = modelsSection(it);
    parts.push(models.el);
    if (it.family === 'api' || !it.eng.builtin) parts.push(dangerRow(it));
    replace(detail, parts);
    paintStatus();
    void loadModels(it, token, models);
  };

  const subtitle = (it: Item): string => (it.family === 'api' ? `${kindLabel(it.api)} · ${it.api.id}` : `${SOURCE[it.eng.source] ?? it.eng.source} · ${it.eng.id}`);

  /** The engine's version, mono, with a revert to the catalogue's pin when it moved off it. */
  const versionHead = (e: EngineInstance): HTMLElement => {
    const moved = !!e.pinned && e.version !== e.pinned;
    return h(
      'div.prov-dh-v',
      null,
      e.version ? h('span.mono.prov-ver', null, e.version) : null,
      moved ? button('', () => void saveEngine(e, { version: 'pinned' }, 'Reverted to the pinned version'), { kind: 'ghost', size: 'sm', icon: 'rotate-ccw', title: 'Revert to the pinned version' }) : null,
    );
  };

  /** An API instance's credential, its key and its base URL, with a test. */
  const setupApi = (a: ApiInstance): HTMLElement => {
    const credText = a.credential === 'env' && typeof a.auth === 'object' && 'env' in a.auth ? `Read from $${a.auth.env}` : CREDENTIAL[a.credential];
    const testOut = h('span.prov-test');
    const testBtn = button('Test', async () => {
      testBtn.disabled = true;
      testOut.textContent = 'Testing…';
      const report = await attempt(() => api.testProvider(a.id));
      testBtn.disabled = false;
      if (!report) {
        testOut.textContent = '';
        return;
      }
      testOut.textContent = report.ok ? `OK · ${report.latency_ms} ms · ${report.model}` : (report.error ?? 'Test failed');
      testOut.className = report.ok ? 'prov-test' : 'prov-test err-t';
    }, { kind: 'ghost', size: 'sm', icon: 'circle-check' });
    const keyBtn = a.credential === 'none' ? null : button(a.credential === 'secret' ? 'Replace key…' : 'Set key…', () => openKey(a), { kind: 'ghost', size: 'sm', icon: 'key-round' });
    const urlInput = input({ value: a.base_url ?? '', placeholder: 'https://', 'aria-label': 'Base URL', autocomplete: 'off' });
    urlInput.addEventListener('change', () => {
      const url = urlInput.value.trim() || null;
      if (url !== (a.base_url ?? null)) void saveApi(a, { base_url: url }, 'Base URL saved');
    });
    return section(
      'Setup',
      h('div.prov-line', null, h('span.prov-cred', null, credText), keyBtn, testBtn, testOut),
      a.kind === 'openai-compatible' || a.base_url !== null ? field('Base URL', urlInput) : null,
    );
  };

  /** Engines run only inside a VM and reach models through the plaza gateway: their route is one of the plaza's models. */
  const setupEngine = (e: EngineInstance): HTMLElement => {
    const values = chatModels.map((m) => `${m.provider}/${m.id}`);
    const names = new Map(chatModels.map((m) => [`${m.provider}/${m.id}`, m.name ?? m.id] as const));
    const picker = modelPicker({
      choices: choicesFrom(values, names, { label: 'Plaza default' }),
      value: e.route ?? '',
      placeholder: 'Plaza default',
      label: 'The model this engine runs through',
      onChange: (value) => void saveEngine(e, { route: value || null }, 'Route saved'),
    });
    const route = e.route ?? e.health.route ?? null;
    return section(
      'Setup',
      h('div.prov-line', null, h('span.prov-cred', null, route ? 'Runs through' : 'Runs through the plaza default'), route ? h('span.mono', null, route) : null, picker.el),
      h(
        'p.prov-note',
        null,
        "Engines use the plaza's model gateway. To use a ChatGPT or Claude subscription, run a CLIProxyAPI hub and add it as a provider.",
      ),
      h(
        'div.prov-line',
        null,
        button('Add a CLIProxyAPI hub', () => openProviderWizard({ onDone: () => void refresh(), kind: 'openai-compatible' }), { kind: 'ghost', size: 'sm', icon: 'plus' }),
      ),
    );
  };

  /** Engines only: source, version (pinned, latest or typed), home folder, arguments and, for a local ACP, the command. */
  const runtimeSection = (e: EngineInstance): HTMLElement => {
    const mode = e.version === e.pinned ? 'pinned' : e.version === e.latest ? 'latest' : 'custom';
    const busy = h('span.prov-busy', { role: 'status' });
    const typed = input({ value: e.version ?? '', placeholder: 'x.y.z', 'aria-label': 'Version', autocomplete: 'off', class: 'mono' });
    typed.hidden = mode !== 'custom';
    typed.addEventListener('change', () => {
      const v = typed.value.trim();
      if (v && v !== e.version) void saveEngine(e, { version: v }, 'Version saved');
    });
    const versionSelect = select(
      [
        { value: 'pinned', label: e.pinned ? `Pinned · ${e.pinned}` : 'Pinned' },
        { value: 'latest', label: e.latest ? `Latest · ${e.latest}` : 'Latest' },
        { value: 'custom', label: 'Type a version…' },
      ],
      mode,
    );
    versionSelect.addEventListener('change', () => {
      if (versionSelect.value === 'custom') {
        typed.hidden = false;
        typed.focus();
        return;
      }
      typed.hidden = true;
      busy.textContent = 'Pinning…';
      versionSelect.disabled = true;
      void saveEngine(e, { version: versionSelect.value }, 'Version saved').finally(() => {
        busy.textContent = '';
      });
    });
    const home = input({ value: e.home, placeholder: '~/', 'aria-label': 'Home folder', autocomplete: 'off' });
    home.addEventListener('change', () => {
      const v = home.value.trim();
      if (v !== e.home) void saveEngine(e, { home: v || null }, 'Home folder saved');
    });
    const args = textarea({ rows: 3, 'aria-label': 'Launch arguments', spellcheck: 'false' });
    args.value = e.args.join('\n');
    args.addEventListener('change', () => {
      const list = args.value.split('\n').map((s) => s.trim()).filter(Boolean);
      if (list.join('\n') !== e.args.join('\n')) void saveEngine(e, { args: list }, 'Arguments saved');
    });
    const command = e.engine === 'acp-local' ? input({ value: e.install?.kind === 'command' ? e.install.command : '', 'aria-label': 'Command', autocomplete: 'off', class: 'mono' }) : null;
    if (command) {
      command.addEventListener('change', () => {
        const v = command.value.trim();
        if (v) void saveEngine(e, { command: v }, 'Command saved');
      });
    }
    const body = h(
      'div.prov-rt',
      null,
      h('p.prov-faint', null, `${SOURCE[e.source] ?? e.source}${e.package ? ` · ${e.package}` : ''}`),
      field('Version', h('div.prov-line', null, versionSelect, typed, busy)),
      field('Home folder', home),
      field('Launch arguments', args, 'One per line.'),
      command ? field('Command', command, 'One program; its arguments go in Launch arguments.') : null,
    );
    return h('details.prov-sec.prov-rt-sec', null, h('summary', null, 'Runtime'), body);
  };

  /** Engines: plain variables. The agent can read them, so a key never goes here (the hub refuses secret-looking names). */
  const variablesSection = (e: EngineInstance): HTMLElement => {
    const rows = h('div.prov-env');
    const addRow = (name = '', value = ''): void => {
      const n = input({ value: name, placeholder: 'NAME', 'aria-label': 'Variable name', autocomplete: 'off', class: 'mono' });
      const v = input({ value, placeholder: 'value', 'aria-label': 'Variable value', autocomplete: 'off' });
      const rm = button('', () => {
        row.remove();
        commit();
      }, { kind: 'ghost', size: 'sm', icon: 'x', title: 'Remove variable' });
      const row = h('div.prov-env-row', null, n, v, rm);
      n.addEventListener('change', commit);
      v.addEventListener('change', commit);
      rows.appendChild(row);
    };
    /** Saves the rows that name a variable; blank rows wait for a name. */
    const commit = (): void => {
      const env: Record<string, string> = {};
      rows.querySelectorAll<HTMLElement>('.prov-env-row').forEach((row) => {
        const [n, v] = row.querySelectorAll<HTMLInputElement>('input');
        const name = n?.value.trim() ?? '';
        if (name) env[name] = v?.value ?? '';
      });
      if (JSON.stringify(env) !== JSON.stringify(e.env)) void saveEngine(e, { env }, 'Variables saved');
    };
    for (const [name, value] of Object.entries(e.env)) addRow(name, value);
    const add = button('Add variable', () => {
      addRow();
      rows.lastElementChild?.querySelector<HTMLInputElement>('input')?.focus();
    }, { kind: 'ghost', size: 'sm', icon: 'plus' });
    return section('Environment', h('p.prov-faint', null, 'Plain settings for the engine. The agent can read them, so keys never go here.'), rows, h('div.prov-line', null, add));
  };

  /** API instances: the header names (their values stay hidden) and a way to add or remove one. */
  const headersSection = (a: ApiInstance): HTMLElement => {
    const names = Object.keys(a.extra_headers);
    const rows = names.map((name) => {
      const rm = button('', () => void removeHeader(name), { kind: 'ghost', size: 'sm', icon: 'x', title: `Remove ${name}` });
      return h('div.prov-env-row', { 'data-key': `header-${name}` }, h('span.mono', null, name), h('span.mono.prov-faint', null, '••••••'), rm);
    });
    const removeHeader = (name: string): Promise<boolean> => {
      const rest = { ...a.extra_headers };
      delete rest[name];
      return saveApi(a, { extra_headers: rest }, `Removed ${name}`);
    };
    const nameInput = input({ placeholder: 'Header name', 'aria-label': 'Header name', autocomplete: 'off', class: 'mono' });
    const valueInput = input({ type: 'password', placeholder: 'Value', 'aria-label': 'Header value', autocomplete: 'new-password' });
    const saveHeader = button(
      'Add header',
      async () => {
        const name = nameInput.value.trim();
        if (!name || !valueInput.value) return;
        const done = await saveApi(a, { extra_headers: { ...a.extra_headers, [name]: valueInput.value } }, `Added ${name}`);
        if (done) valueInput.value = '';
      },
      { kind: 'primary', size: 'sm' },
    );
    const addBox = h('div.prov-env-row', null, nameInput, valueInput, saveHeader);
    addBox.hidden = true;
    const toggle = button('Add header', () => {
      addBox.hidden = !addBox.hidden;
      if (!addBox.hidden) nameInput.focus();
    }, { kind: 'ghost', size: 'sm', icon: 'plus' });
    return section(
      'Headers',
      h('p.prov-faint', null, 'Sent with every request. The values stay on the hub.'),
      rows.length ? h('div.prov-env', null, rows) : null,
      h('div.prov-line', null, toggle),
      addBox,
    );
  };

  /** Models: the count, the list (favourite, id, price, "Trains on data") and, for an API instance, a custom model. */
  const modelsSection = (it: Item): { el: HTMLElement; count: HTMLElement; list: HTMLElement } => {
    const count = h('span.prov-faint', null, 'Loading models…');
    const listEl = h('ul.prov-models');
    const extra: Node[] = [];
    if (it.family === 'api') {
      const customInput = input({ placeholder: 'Model id', 'aria-label': 'Custom model id', autocomplete: 'off', class: 'mono' });
      const add = button(
        'Add model',
        async () => {
          const id = customInput.value.trim();
          if (!id || it.family !== 'api') return;
          const done = await saveApi(it.api, { models: [...new Set([...it.api.models, id])] }, `Added ${id}`);
          if (done) customInput.value = '';
        },
        { kind: 'primary', size: 'sm' },
      );
      const customBox = h('div.prov-line', null, customInput, add);
      customBox.hidden = true;
      const toggle = button(
        'Add custom model',
        () => {
          customBox.hidden = !customBox.hidden;
          if (!customBox.hidden) customInput.focus();
        },
        { kind: 'ghost', size: 'sm', icon: 'plus' },
      );
      extra.push(h('div.prov-line', null, toggle), customBox);
    }
    const el = section('Models', h('p.prov-faint', null, 'Model preferences are saved on the hub and shared across your devices.'), count, listEl, ...extra);
    return { el, count, list: listEl };
  };

  /** One model as the detail lists it: a favourite star, the id, the price (filled in after), "Trains on data". */
  type ModelRow = { id: string; name: string | null; provider: string; trains: boolean };

  /** Lists the selected item's models once its detail is drawn: the instance's own, or the plaza's for an engine. */
  const loadModels = async (it: Item, token: number, parts: { count: HTMLElement; list: HTMLElement }): Promise<void> => {
    let rows: ModelRow[] | undefined;
    if (it.family === 'api') {
      const own = await attempt(() => api.providerModels(it.api.id));
      rows = own?.map((m) => ({ id: m.id, name: m.name, provider: it.api.id, trains: false }));
    } else {
      rows = chatModels.map((m) => ({ id: m.id, name: m.name, provider: m.provider, trains: m.trains_on_data === true }));
    }
    if (!rows || token !== detailToken || closed) return;
    const list: ModelRow[] = rows;
    const trained = new Set(it.family === 'api' ? contributorIds(it.api.options) : []);
    const favourites = new Set(settings?.favourite_models ?? []);
    parts.count.textContent = `${list.length} ${list.length === 1 ? 'model' : 'models'}`;
    const drawn = list.map((m) => {
      const key = `${m.provider}/${m.id}`;
      const starBtn = button('', () => void toggleFavourite(key, (on) => paintStar(starBtn, on)), {
        kind: 'ghost',
        size: 'sm',
        icon: 'star',
        title: favourites.has(key) ? 'Remove from favourites' : 'Add to favourites',
      });
      paintStar(starBtn, favourites.has(key));
      const price = h('span.prov-faint.prov-price');
      return h(
        'li.prov-model',
        { 'data-key': `model-${key}`, title: key },
        starBtn,
        h('span.mono.prov-mid', null, m.id),
        price,
        m.trains || trained.has(m.id) ? badge('Trains on data', 'warn') : null,
      );
    });
    replace(parts.list, drawn.length ? drawn : h('li.prov-faint', null, 'No models listed'));
    // Prices for the first few models only: each one is a request. A model with no price (the hub answers 404, or no
    // price at all) shows nothing.
    list.slice(0, PRICED_MODELS).forEach((m, i) => {
      const price = drawn[i]?.querySelector<HTMLElement>('.prov-price');
      if (!price) return;
      void api.price(`${m.provider}/${m.id}`).then(
        (p) => {
          const rates = p?.price;
          if (token === detailToken && rates) price.textContent = `$${rates.input} in · $${rates.output} out per 1M`;
        },
        () => undefined,
      );
    });
  };

  const paintStar = (b: HTMLElement, on: boolean): void => {
    b.setAttribute('aria-pressed', String(on));
    const svg = b.querySelector<SVGElement>('svg');
    if (svg) svg.style.fill = on ? 'currentColor' : 'none';
  };

  const dangerRow = (it: Item): HTMLElement => {
    const base = view(it);
    return h(
      'div.prov-danger',
      null,
      button('Remove provider', () => confirmRemove(it), { kind: 'danger', size: 'sm', icon: 'trash-2' }),
      h('span.prov-faint', null, `Agents stop using ${base.label}.`),
    );
  };

  // ---- Dialogs --------------------------------------------------------------

  /** Set or replace an API key: write-only, the hub keeps it in its vault. */
  const openKey = (a: ApiInstance): void => {
    let close = (): void => {};
    const value = input({ type: 'password', autocomplete: 'new-password', placeholder: 'API key', 'aria-label': 'API key' });
    const save = button(
      'Save key',
      async () => {
        const key = value.value.trim();
        if (!key) return;
        save.disabled = true;
        const saved = await attempt(() => providersApi.setKey(a.id, key), 'Key saved');
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
    close = sheet(
      a.credential === 'secret' ? `Replace key · ${a.label}` : `Set key · ${a.label}`,
      h('div.stack', null, field('API key', value, 'Write-only: the hub keeps it in its vault and never shows it again.')),
      [button('Cancel', () => close(), { kind: 'ghost' }), save],
    );
  };

  /** The accent swatch popover: the eight accents and none. */
  const openAccents = async (anchor: HTMLElement, it: Item): Promise<void> => {
    const { popover } = await import('../ui/shell');
    const swatch = (color: string | null): HTMLButtonElement => {
      const b = h('button.prov-sw', {
        type: 'button',
        role: 'menuitem',
        title: color ?? 'None',
        'aria-label': color ?? 'None',
        style: color ? `--swatch:${color}` : null,
        class: color ? '' : 'none',
      });
      b.addEventListener('click', () => void saveItem(it, { accent: color }));
      return b;
    };
    popover(anchor, h('div.menu.prov-pop', { role: 'menu' }, ACCENTS.map(swatch), swatch(null)));
  };

  const confirmRemove = (it: Item): void => {
    const base = view(it);
    let close = (): void => {};
    const yes = button(
      'Remove provider',
      async () => {
        const removed = await attempt(() => (it.family === 'api' ? providersApi.remove(it.api.id) : providersApi.removeEngine(it.eng.id)), `Removed ${base.label}`);
        if (removed === undefined) return;
        close();
        selected = null;
        await refresh();
      },
      { kind: 'danger' },
    );
    close = sheet(`Remove ${base.label}?`, h('div.stack', null, h('p.prov-faint', null, 'Agents stop using its models.')), [button('Cancel', () => close(), { kind: 'ghost' }), yes]);
  };

  /** Run `update all` as a dry run first, list the changes, then apply them. */
  const updateAll = async (): Promise<void> => {
    const plan = await attempt(() => providersApi.updateEngines(true));
    if (!plan) return;
    if (!plan.changes.length) {
      toast('Everything is up to date');
      return;
    }
    const n = plan.changes.length;
    const word = n === 1 ? 'engine' : 'engines';
    let close = (): void => {};
    const go = button(
      'Update',
      async () => {
        go.disabled = true;
        const done = await attempt(() => providersApi.updateEngines(false), `Updated ${n} ${word}`);
        go.disabled = false;
        if (done === undefined) return;
        close();
        await refresh();
      },
      { kind: 'primary' },
    );
    close = sheet(
      `Update ${n} ${word}?`,
      h(
        'div.stack',
        null,
        plan.changes.map((c) => h('div.prov-change', null, h('span.mono', null, c.id), h('span.mono.prov-faint', null, `${c.from} → ${c.to}`))),
      ),
      [button('Cancel', () => close(), { kind: 'ghost' }), go],
    );
  };

  /** Check every provider and engine now. */
  const checkAll = async (): Promise<void> => {
    checkedBtn.setAttribute('disabled', '');
    await attempt(() => Promise.all([providersApi.checkAll(), providersApi.checkEngines()]));
    checkedBtn.removeAttribute('disabled');
    await refresh();
  };
  checkedBtn.addEventListener('click', () => void checkAll());

  // ---- Usage providers ------------------------------------------------------

  const usageList = h('div.prov-usage');
  const addHub = button('Add hub', () => openAddHub(), { kind: 'ghost', size: 'sm', icon: 'plus' });

  const paintUsage = (): void => {
    if (!usage.length) {
      replace(usageList, empty('cpu', 'No hubs configured.'));
      return;
    }
    replace(usageList, usage.map(usageRow));
  };

  const usageRow = (u: UsageProvider): HTMLElement => {
    const reading = u.last;
    const tone: Tone = reading ? (reading.status === 'ok' ? 'ok' : 'danger') : 'muted';
    const totals = reading?.totals;
    const accounts = reading?.accounts ?? [];
    const refreshBtn = button('', async () => {
      refreshBtn.disabled = true;
      const fresh = await attempt(() => providersApi.readUsage(u.id));
      refreshBtn.disabled = false;
      if (!fresh) return;
      u.last = fresh;
      paintUsage();
    }, { kind: 'ghost', size: 'sm', icon: 'refresh-cw', title: `Refresh ${u.label}` });
    const removeBtn = button('', () => confirmRemoveHub(u), { kind: 'ghost', size: 'sm', icon: 'trash-2', title: `Remove ${u.label}` });
    return h(
      'div.prov-usage-row',
      { 'data-key': `usage-${u.id}` },
      h('div.prov-usage-h', null, h('span.row-t', null, u.label), h('span.mono.prov-url', null, u.url), h('span.sp'), refreshBtn, removeBtn),
      h('p.prov-status', null, dot(tone), h('span', null, reading?.message ?? 'Not checked yet')),
      totals ? h('p.prov-faint', null, `${totals.requests.toLocaleString('en-US')} requests · ${compact(totals.tokens)} tokens`) : null,
      accounts.length
        ? h(
            'div.cluster',
            null,
            accounts.map((a) => {
              const chip = badge(`${a.name} · ${a.provider} · ${a.status}`, accountTone(a.status));
              chip.title = a.message;
              return chip;
            }),
          )
        : null,
    );
  };

  const openAddHub = (): void => {
    let close = (): void => {};
    const url = input({ type: 'url', placeholder: 'https://hub.example.ts.net:8318', autocomplete: 'off', 'aria-label': 'Hub URL' });
    const key = input({ type: 'password', autocomplete: 'new-password', 'aria-label': 'Management key' });
    const label = input({ placeholder: "Defaults to the hub's host name", autocomplete: 'off', 'aria-label': 'Label' });
    const err = h('span.hint.err.prov-foot-err', { role: 'alert' });
    const add = button(
      'Add hub',
      async () => {
        if (!url.value.trim() || !key.value.trim()) {
          err.textContent = 'Give the hub URL and its management key.';
          return;
        }
        add.disabled = true;
        err.textContent = '';
        try {
          const created = await providersApi.addUsage({ url: url.value.trim(), key: key.value.trim(), label: label.value.trim() || undefined });
          close();
          toast(`Added ${created.label}`);
          await loadAll();
          paintUsage();
        } catch (e) {
          err.textContent = e instanceof Error ? e.message : String(e);
        } finally {
          add.disabled = false;
        }
      },
      { kind: 'primary' },
    );
    close = sheet(
      'Add a CLIProxyAPI hub',
      h(
        'div.stack',
        null,
        h('p.prov-lede', null, 'Show the quota of every account the hub pools, next to the providers. The key stays on the hub.'),
        field('Hub URL', url),
        field('Management key', key),
        field('Label', label, 'Optional.'),
      ),
      [err, button('Cancel', () => close(), { kind: 'ghost' }), add],
    );
  };

  const confirmRemoveHub = (u: UsageProvider): void => {
    let close = (): void => {};
    const yes = button(
      'Remove hub',
      async () => {
        const removed = await attempt(() => providersApi.removeUsage(u.id), `Removed ${u.label}`);
        if (removed === undefined) return;
        close();
        await loadAll();
        paintUsage();
      },
      { kind: 'danger' },
    );
    close = sheet(`Remove ${u.label}?`, h('div.stack', null, h('p.prov-faint', null, 'Its quota stops showing next to the providers.')), [button('Cancel', () => close(), { kind: 'ghost' }), yes]);
  };

  // ---- Advanced -------------------------------------------------------------

  const stepValue = h('span.prov-step-v', { 'aria-live': 'polite' });
  const stepDown = button('', () => setHealth(Math.max(0, healthSecs - HEALTH_STEP)), { kind: 'ghost', size: 'sm', icon: 'minus', title: 'Check less often' });
  const stepUp = button('', () => setHealth(Math.min(HEALTH_MAX, healthSecs + HEALTH_STEP)), { kind: 'ghost', size: 'sm', icon: 'plus', title: 'Check more often' });

  const setHealth = (secs: number): void => {
    healthSecs = secs;
    paintAdvanced();
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => void saveHealth(), 500);
  };

  const paintAdvanced = (): void => {
    stepValue.textContent = String(healthSecs);
    stepDown.toggleAttribute('disabled', healthSecs <= 0);
    stepUp.toggleAttribute('disabled', healthSecs >= HEALTH_MAX);
  };

  const saveHealth = async (): Promise<void> => {
    const s = await attempt(() => api.settings());
    if (!s) return;
    s.health_interval_s = healthSecs;
    const saved = await attempt(() => api.saveSettings(s));
    if (saved) settings = saved;
  };

  // ---- Decisions and scope --------------------------------------------------

  const panel = decisionPanel();

  const scopeBtn = h('button.scope-pill', { type: 'button', 'aria-haspopup': 'menu', title: 'Show the whole plaza or one project' }, lu('layers', 14), h('span', null, 'All projects'), lu('chevrons-up-down', 13));
  scopeBtn.addEventListener('click', () => void openScope(scopeBtn));

  /** The projects a settings change can be opened from: picking one opens its Settings tab, where its defaults live. */
  const openScope = async (anchor: HTMLElement): Promise<void> => {
    const { popover } = await import('../ui/shell');
    const { projects, isLive, refreshProjects } = await import('../scope');
    await attempt(() => refreshProjects());
    const menu = h(
      'div.menu',
      { role: 'menu' },
      h('div.menu-h', null, 'Settings for'),
      menuItem(lu('layers', 15), 'All projects', () => undefined, true),
      projects
        .get()
        .filter(isLive)
        .map((p) => menuItem(pmono(p, 18), p.title, () => (location.hash = `#/projects/${p.id}/settings`))),
    );
    popover(anchor, menu);
  };

  // ---- Assembly -------------------------------------------------------------

  const paintAll = (): void => {
    paintHead();
    paintList();
    paintDetail();
    paintUsage();
    paintAdvanced();
  };

  const refresh = async (): Promise<void> => {
    await loadAll();
    if (closed) return;
    paintAll();
    void panel.reload();
  };

  /** A live event: the lists refresh in place; the detail is redrawn only when nothing in it is being typed into. */
  const sync = async (): Promise<void> => {
    await loadAll();
    if (closed) return;
    if (detail.contains(document.activeElement)) {
      paintHead();
      paintList();
      paintStatus();
      return;
    }
    paintAll();
  };
  const live = (): void => {
    clearTimeout(liveTimer);
    liveTimer = setTimeout(() => void sync(), 300);
  };

  const providersCard = card('Providers', h('div.prov-split', null, list, detail), { flush: true, actions: [updateBtn, checkedBtn] });
  const usageCard = card('Usage providers', usageList, { actions: [addHub] });
  const advancedCard = card(
    'Advanced',
    h(
      'div.prov-setting',
      null,
      h('div.prov-setting-t', null, h('b', null, 'Health check interval'), h('p.hint', null, 'Refresh provider status, versions and models in the background. Set to 0 to turn it off.')),
      h('div.prov-step', null, stepDown, stepValue, stepUp, h('span.prov-faint', null, 'seconds')),
    ),
  );
  const decisionsCard = card('Decisions', [
    h('p.prov-lede', null, 'Models that answer typed questions with calibrated probabilities instead of writing text. The plaza uses them to decide; agents never chat with them.'),
    panel.el,
  ]);

  replace(root, h('div.prov-scope', null, h('span', null, 'Applying settings for'), scopeBtn), providersCard, usageCard, advancedCard, decisionsCard);
  replace(list, h('p.prov-faint', null, 'Loading providers…'));
  replace(detail, h('p.prov-faint', null, 'Loading providers…'));

  const offs = [on('providers.health', live), on('engines.health', live), on('engines.changed', live)];
  const tick = setInterval(paintHead, 30_000);
  void refresh();

  return () => {
    closed = true;
    offs.forEach((off) => off());
    clearInterval(tick);
    clearTimeout(liveTimer);
    clearTimeout(saveTimer);
  };
}
