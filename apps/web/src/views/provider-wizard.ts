// Add provider: the dialog that adds an API provider, or a coding engine on this plaza (a second built-in instance,
// an ACP registry agent, or a local ACP command). Three steps: Provider, Identity, Config. It creates through
// providersApi (an API kind by `create`, an engine by `addEngine`), saves the typed key with `setKey`, then calls
// `onDone`. An engine gets no key: it reaches models only through the plaza gateway.

import '../styles/proto/provider-wizard.css';
import { api } from '../api';
import { providersApi, type EngineEdit, type ProviderCatalog, type RegistryAgent } from '../api-providers';
import { badge, brandLogo, button, modal, toast } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import { modelPicker, type ModelChoice } from '../ui/model-picker';

type ApiPreset = ProviderCatalog['api'][number];
type BuiltinEngine = ProviderCatalog['engines'][number];

/** What the Provider step chose. */
type Choice =
  | { kind: 'api'; preset: ApiPreset }
  | { kind: 'engine'; engine: BuiltinEngine }
  | { kind: 'registry'; agent: RegistryAgent }
  | { kind: 'local' };

export interface ProviderWizardOptions {
  /** Runs once the instance exists (after its key is saved, when one was typed). */
  onDone: () => void;
  /** A kind to preselect: an API kind (`openai-compatible` for the CLIProxyAPI path) or a built-in engine id. */
  kind?: string;
}

const STEPS = ['Provider', 'Identity', 'Config'];
const ACCENTS = ['#5cc8ff', '#7ee0b5', '#b69cff', '#ff9ac1', '#ffd36e', '#6fe3e1', '#a3b8ff', '#c5f27a'];
/** The brand a built-in engine's logo draws; other engines draw a monogram. */
const ENGINE_BRAND: Record<string, string> = { claude: 'claude', codex: 'openai', gemini: 'google', opencode: 'opencode' };
/** Used when the catalog has no OpenAI-compatible kind (Configure manually). */
const OPENAI_COMPATIBLE: ApiPreset = {
  id: 'openai-compatible', kind: 'openai-compatible', label: 'OpenAI-compatible', brand: 'openai', base_url: null, auth: 'none', notes: '', models: [],
};
const ID_RE = /^[a-z0-9_-]+$/;
const HTTP_URL = /^https?:\/\//;
const HEX = /^#[0-9a-f]{3,8}$/i;

const msg = (e: unknown): string => (e instanceof Error ? e.message : String(e));
const choiceKey = (c: Choice): string =>
  c.kind === 'api' ? `api:${c.preset.kind}` : c.kind === 'engine' ? `engine:${c.engine.id}` : c.kind === 'registry' ? `registry:${c.agent.id}` : 'local';

/** A registry id as an instance id: lowercase, anything else dashed. */
const slug = (s: string): string => s.toLowerCase().replace(/[^a-z0-9_-]+/g, '-').replace(/^-+|-+$/g, '') || 'engine';

/** The Settings-row layout: the name and its hint on the left, the control on the right (stacked for a wide control). */
function row(name: string, control: Node, hint?: string | Node | null, stacked = false): HTMLElement {
  return h(
    'div.pw-row',
    { class: stacked ? 'stacked' : '' },
    h('div.pw-row-l', null, h('span.pw-row-n', null, name), typeof hint === 'string' ? h('small.hint', null, hint) : (hint ?? null)),
    h('div.pw-row-c', null, control),
  );
}

/** A provider's or engine's tile in the Provider grid. */
function tile(label: string, logo: Node, chip: Node | null, on: boolean, pick: () => void): HTMLElement {
  return h(
    'button.pw-tile',
    { type: 'button', 'aria-pressed': on ? 'true' : 'false', onclick: pick },
    logo,
    h('span.pw-tile-t', null, label),
    chip,
    on ? lu('check', 15, 'pw-tile-ck') : null,
  );
}

/** A registry agent's icon, or its initials when it has none (or it fails to load). */
function monogram(name: string): HTMLElement {
  const letters = name.replace(/[^A-Za-z0-9 ]/g, '').split(/\s+/).filter(Boolean).slice(0, 2).map((w) => w[0] ?? '').join('');
  return h('span.pw-mono', { 'aria-hidden': 'true' }, (letters || '?').toUpperCase());
}

export function openProviderWizard(opts: ProviderWizardOptions): void {
  let step: 1 | 2 | 3 = 1;
  let furthest: 1 | 2 | 3 = 1;
  let choice: Choice | null = null;
  let catalog: ProviderCatalog | null = null;
  let loadError = '';
  let taken = new Set<string>();
  let routes: ModelChoice[] = [{ value: '', provider: '', label: 'Plaza default' }];
  let registry: RegistryAgent[] | null = null;
  let registryError = '';
  let registryQuery = '';
  let regListEl: HTMLElement | null = null;
  let busy = false;
  let err = '';
  let accentOpen = false;
  // The form. Each field is kept here as it is typed, so a redraw loses nothing.
  let id = '';
  let idEdited = false;
  let label = '';
  let accent: string | null = null;
  let baseUrl = '';
  let key = '';
  let models = '';
  let localExecutable = '';
  let versionSel = '';
  let versionText = '';
  let home = '';
  let argsText = '';
  let command = '';
  let route = '';
  let env: [string, string][] = [];

  const railHost = h('div.pw-rail-host');
  const stageHost = h('div.pw-stage');
  const footHost = h('div.pw-foot');
  const sub = h('p.pw-sub', null, 'Add an account or configure a provider on this plaza.');
  const close = modal('Add provider', h('div.pw-wrap', null, sub, railHost, stageHost), undefined, [footHost]);
  document.body.lastElementChild?.querySelector('.sheet')?.classList.add('pw-sheet');

  const idProblem = (): string | null => {
    if (!id) return 'Enter an ID.';
    if (!ID_RE.test(id)) return 'Use lowercase letters, digits, - and _.';
    if (taken.has(id)) return `"${id}" is already taken.`;
    return null;
  };

  /** What blocks the step's next move: the ID on Identity and Config, the command on a local ACP command. */
  const configProblem = (): string | null => {
    if (choice?.kind === 'local' && !command.trim()) return 'Enter the command.';
    return null;
  };

  function uniqueId(base: string): string {
    if (!taken.has(base)) return base;
    for (let n = 2; ; n++) {
      const candidate = `${base}_${n}`;
      if (!taken.has(candidate)) return candidate;
    }
  }

  /** Starts a fresh form for a choice: its ID (the kind's own, or kind_2 when taken), label and version. */
  function startFrom(c: Choice): void {
    choice = c;
    const base = c.kind === 'api' ? c.preset.kind : c.kind === 'engine' ? c.engine.id : c.kind === 'registry' ? slug(c.agent.id) : 'local-acp';
    label = c.kind === 'api' ? c.preset.label : c.kind === 'engine' ? c.engine.label : c.kind === 'registry' ? c.agent.name : 'Local ACP command';
    id = uniqueId(base);
    idEdited = false;
    accent = null;
    baseUrl = c.kind === 'api' ? (c.preset.base_url ?? '') : '';
    key = '';
    models = '';
    localExecutable = '';
    home = '';
    argsText = '';
    command = '';
    route = '';
    env = [];
    versionSel = c.kind === 'registry' ? c.agent.version : c.kind === 'engine' ? 'pinned' : '';
    versionText = '';
    err = '';
  }

  /** Chooses a tile, a registry agent or the local command; a choice already made stays as it is. */
  function pickChoice(c: Choice): void {
    if (!choice || choiceKey(choice) !== choiceKey(c)) startFrom(c);
    renderStage();
    renderFoot();
  }

  function goTo(n: 1 | 2 | 3): void {
    step = n;
    furthest = Math.max(furthest, n) as 1 | 2 | 3;
    err = '';
    renderAll();
    const first = stageHost.querySelector<HTMLElement>('[data-f="base"]') ?? stageHost.querySelector<HTMLElement>('input, select, textarea');
    first?.focus();
  }

  function configureManually(): void {
    if (!catalog) return;
    pickChoice({ kind: 'api', preset: catalog.api.find((p) => p.kind === 'openai-compatible') ?? OPENAI_COMPATIBLE });
    goTo(3);
  }

  function setAccentOpen(v: boolean): void {
    accentOpen = v;
    if (v) document.addEventListener('click', onAccentOutside);
    else document.removeEventListener('click', onAccentOutside);
    renderStage();
  }

  const onAccentOutside = (e: MouseEvent): void => {
    if (e.composedPath().some((n) => n instanceof Element && n.classList.contains('pw-accent'))) return;
    setAccentOpen(false);
  };

  function renderRail(): void {
    replace(
      railHost,
      h(
        'ol.pw-steps',
        { 'aria-label': 'Steps' },
        STEPS.map((name, i) => {
          const n = (i + 1) as 1 | 2 | 3;
          return h(
            'li',
            null,
            h(
              'button.pw-step',
              {
                type: 'button',
                class: n === step ? 'on' : n < step ? 'done' : '',
                'aria-current': n === step ? 'step' : null,
                disabled: n > furthest,
                onclick: () => goTo(n),
              },
              h('span.pw-num', null, n < step ? lu('check', 12) : String(n)),
              name,
            ),
          );
        }),
      ),
    );
  }

  function renderStage(): void {
    if (loadError) {
      replace(stageHost, h('p.hint.err', null, `The providers could not be loaded. ${loadError}`));
      return;
    }
    if (!catalog) {
      replace(stageHost, h('p.pw-muted', null, 'Loading the providers…'));
      return;
    }
    replace(stageHost, step === 1 ? providerStep(catalog) : step === 2 ? identityStep() : configStep());
  }

  function renderFoot(): void {
    const left: Node[] = [];
    const right: Node[] = [button('Cancel', () => close(), { kind: 'ghost' })];
    if (step > 1) left.push(button('Back', () => goTo((step - 1) as 1 | 2), { kind: 'ghost', icon: 'arrow-left' }));
    if (step === 1) {
      left.push(button('Configure manually', configureManually, { kind: 'ghost' }));
      right.push(button('Next', () => goTo(2), { kind: 'primary', disabled: !choice }));
    } else if (step === 2) {
      right.push(button('Next', () => goTo(3), { kind: 'primary', disabled: !!idProblem() }));
    } else {
      const pinning = busy && (choice?.kind === 'registry' || choice?.kind === 'engine');
      const label3 = busy ? (pinning ? 'Pinning…' : 'Adding…') : 'Add instance';
      right.push(button(label3, () => void submit(), { kind: 'primary', disabled: busy || !!idProblem() || !!configProblem() }));
    }
    footHost.replaceChildren(
      ...(err ? [h('p.pw-err', { role: 'alert' }, err)] : []),
      h('div.pw-foot-row', null, h('div.pw-foot-l', null, left), h('div.pw-foot-r', null, right)),
    );
  }

  function renderAll(): void {
    renderRail();
    renderStage();
    renderFoot();
  }

  // ---- Step 1: Provider -----------------------------------------------------------

  function providerStep(cat: ProviderCatalog): HTMLElement {
    const on = (c: Choice): boolean => !!choice && choiceKey(choice) === choiceKey(c);
    const grid = h(
      'div.pw-grid',
      { role: 'group', 'aria-label': 'Providers and engines' },
      cat.api.map((p) => tile(p.label, brandLogo(p.brand), null, on({ kind: 'api', preset: p }), () => pickChoice({ kind: 'api', preset: p }))),
      cat.engines.map((e) =>
        tile(e.label, brandLogo(ENGINE_BRAND[e.id] ?? e.id), badge('Engine', 'accent'), on({ kind: 'engine', engine: e }), () =>
          pickChoice({ kind: 'engine', engine: e }),
        ),
      ),
    );
    const search = h(
      'label.pw-search',
      null,
      lu('search', 15),
      h('input.pw-search-in', {
        type: 'search',
        placeholder: 'Search the ACP Registry',
        'aria-label': 'Search the ACP Registry',
        autocomplete: 'off',
        value: registryQuery,
        oninput: (e: Event) => {
          registryQuery = (e.target as HTMLInputElement).value;
          renderRegistry();
        },
      }),
    );
    const localOn = choice?.kind === 'local';
    regListEl = h('div.pw-reg-list', { role: 'list', 'aria-label': 'ACP Registry agents' });
    const body = h(
      'div.pw-step-body',
      null,
      grid,
      h('div.pw-divider', null, h('span', null, 'Or choose from ACP Registry')),
      h(
        'div.pw-reg-tools',
        null,
        search,
        button('Enter manually', () => pickChoice({ kind: 'local' }), { kind: 'ghost', size: 'sm', title: 'Enter the command of a local ACP agent' }),
        button('Local ACP command', () => pickChoice({ kind: 'local' }), { kind: 'ghost', size: 'sm', title: 'Run a program on the VM as an ACP agent' }),
      ),
      localOn ? h('p.pw-muted', null, 'A local ACP command is selected. Next sets it up.') : null,
      regListEl,
    );
    renderRegistry();
    return body;
  }

  function renderRegistry(): void {
    const el = regListEl;
    if (!el) return;
    if (registryError) {
      replace(el, h('p.hint.err', null, `The ACP Registry could not be reached. ${registryError}`));
      return;
    }
    if (!registry) {
      replace(el, h('p.pw-muted', null, 'Loading the ACP Registry…'));
      return;
    }
    const q = registryQuery.trim().toLowerCase();
    const agents = registry.filter((a) => !q || `${a.name} ${a.id} ${a.description}`.toLowerCase().includes(q));
    replace(el, agents.length ? agents.map(registryRow) : h('p.pw-muted', null, 'No agents match.'));
  }

  function registryRow(a: RegistryAgent): HTMLElement {
    const chosen = choice?.kind === 'registry' && choice.agent.id === a.id;
    const logo = a.icon && HTTP_URL.test(a.icon)
      ? h('img', {
          src: a.icon,
          alt: '',
          loading: 'lazy',
          onerror: (e: Event) => (e.currentTarget as HTMLElement).replaceWith(monogram(a.name)),
        })
      : monogram(a.name);
    return h(
      'div.pw-reg-row',
      { role: 'listitem', class: chosen ? 'on' : '' },
      h('span.pw-reg-ic', null, logo),
      h(
        'span.pw-reg-main',
        null,
        h('span.pw-reg-name', null, a.name, h('span.pw-reg-ver', null, a.version)),
        h('span.pw-reg-desc', null, a.description),
      ),
      a.website && HTTP_URL.test(a.website)
        ? h('a.iconbtn', { href: a.website, target: '_blank', rel: 'noreferrer noopener', title: `${a.name} website`, 'aria-label': `${a.name} website` }, lu('external-link', 14))
        : null,
      a.runs_in_vm
        ? button(chosen ? 'Chosen' : 'Add', () => {
            pickChoice({ kind: 'registry', agent: a });
            goTo(2);
          }, { size: 'sm', icon: chosen ? 'check' : 'plus' })
        : badge('No Linux build', 'muted'),
    );
  }

  // ---- Step 2: Identity -----------------------------------------------------------

  function identityStep(): HTMLElement {
    const idHint = h('small.hint');
    const idInput = h('input.input.mono', {
      value: id,
      spellcheck: 'false',
      autocomplete: 'off',
      'aria-label': 'Instance ID',
      oninput: (e: Event) => {
        id = (e.target as HTMLInputElement).value.trim();
        idEdited = true;
        checkId(idInput, idHint);
        renderFoot();
      },
    });
    checkId(idInput, idHint);
    const labelInput = h('input.input', {
      value: label,
      autocomplete: 'off',
      'aria-label': 'Label',
      oninput: (e: Event) => {
        label = (e.target as HTMLInputElement).value;
      },
    });
    return h(
      'div.pw-step-body',
      null,
      row('Label', labelInput, 'The name the Providers list and the model picker show.'),
      row('Instance ID', idInput, idHint),
      row('Accent colour', accentControl(), 'Optional. Tints the instance in the list and its rail icon.'),
    );
  }

  /** Writes the ID's problem (or the rule) under the field, and marks the field when it is not valid. */
  function checkId(input: HTMLInputElement, hint: HTMLElement): void {
    const p = idProblem();
    hint.textContent = p ?? (idEdited ? 'Looks good.' : 'Lowercase letters, digits, - and _.');
    hint.classList.toggle('err', !!p);
    input.classList.toggle('invalid', !!p);
    input.setAttribute('aria-invalid', p ? 'true' : 'false');
  }

  function accentControl(): HTMLElement {
    const swatch = (c: string | null, cls = ''): HTMLElement =>
      h('span.pw-swatch', { class: cls, style: c && HEX.test(c) ? `--acc:${c}` : null, 'aria-hidden': 'true' }, c ? null : lu('ban', 12));
    const trigger = h(
      'button.pw-accent-btn',
      {
        type: 'button',
        'aria-haspopup': 'listbox',
        'aria-expanded': accentOpen ? 'true' : 'false',
        onclick: (e: Event) => {
          e.stopPropagation();
          setAccentOpen(!accentOpen);
        },
      },
      swatch(accent, accent ? '' : 'none'),
      h('span', null, accent ?? 'None'),
    );
    const menu = accentOpen
      ? h(
          'div.pw-swatches',
          { role: 'listbox', 'aria-label': 'Accent colour' },
          [null, ...ACCENTS].map((c) =>
            h(
              'button.pw-swatch-opt',
              {
                type: 'button',
                role: 'option',
                'aria-selected': accent === c ? 'true' : 'false',
                'aria-label': c ?? 'None',
                title: c ?? 'None',
                onclick: (e: Event) => {
                  e.stopPropagation();
                  accent = c;
                  setAccentOpen(false);
                },
              },
              swatch(c, c ? '' : 'none'),
            ),
          ),
        )
      : null;
    return h('div.pw-accent', null, trigger, menu);
  }

  // ---- Step 3: Config -------------------------------------------------------------

  function configStep(): HTMLElement {
    const c = choice;
    if (!c) return h('p.pw-muted', null, 'Choose a provider first.');
    if (c.kind === 'api') return apiConfig(c.preset);
    return engineConfig(c);
  }

  function apiConfig(p: ApiPreset): HTMLElement {
    const local = p.kind === 'codex-app-server' || p.kind === 'opencode-local';
    const executableIn = h('input.input', {
      value: localExecutable,
      placeholder: p.kind === 'codex-app-server' ? '/path/to/codex' : '/path/to/opencode',
      autocomplete: 'off',
      spellcheck: 'false',
      'aria-label': 'Local executable',
      oninput: (e: Event) => { localExecutable = (e.target as HTMLInputElement).value; },
    });
    const baseIn = h('input.input', {
      value: baseUrl,
      placeholder: p.base_url ?? 'https://',
      spellcheck: 'false',
      autocomplete: 'off',
      'data-f': 'base',
      'aria-label': 'Base URL',
      oninput: (e: Event) => {
        baseUrl = (e.target as HTMLInputElement).value;
      },
    });
    const keyIn = h('input.input', {
      type: 'password',
      value: key,
      placeholder: 'Optional',
      autocomplete: 'new-password',
      spellcheck: 'false',
      'aria-label': 'Key',
      oninput: (e: Event) => {
        key = (e.target as HTMLInputElement).value;
      },
    });
    const modelsIn = h('input.input', {
      value: models,
      placeholder: p.models.slice(0, 3).join(', ') || 'Comma separated',
      autocomplete: 'off',
      spellcheck: 'false',
      'aria-label': 'Models',
      oninput: (e: Event) => {
        models = (e.target as HTMLInputElement).value;
      },
    });
    return h(
      'div.pw-step-body',
      null,
      p.base_url !== null || p.kind === 'openai-compatible'
        ? row('Base URL', baseIn, 'The API root the requests go to.')
        : null,
      local ? row('Local executable', executableIn, 'Optional. The installed program on this hub; empty uses the command on PATH.') : null,
      p.kind === 'codex-app-server'
        ? h('p.pw-muted', null, 'Uses the existing Codex login on this hub. Sign in with Codex before checking the provider.')
        : row('Key', keyIn, 'Optional. Stored in the vault and never shown again; you can set it later.'),
      row('Models', modelsIn, "Optional, comma separated. Empty uses the kind's own list."),
    );
  }

  function engineConfig(c: Exclude<Choice, { kind: 'api' }>): HTMLElement {
    const parts: Node[] = [];
    if (c.kind !== 'local') {
      const pinned = c.kind === 'registry' ? c.agent.version : c.engine.pinned;
      const options: [string, string][] = [
        [c.kind === 'registry' ? c.agent.version : 'pinned', c.kind === 'registry' ? `${pinned} (registry)` : `${pinned} (catalogue)`],
        ['latest', 'Latest'],
        ['custom', 'Type a version…'],
      ];
      const sel = h(
        'select.select',
        {
          'aria-label': 'Version',
          onchange: (e: Event) => {
            versionSel = (e.target as HTMLSelectElement).value;
            renderStage();
          },
        },
        options.map(([v, t]) => h('option', { value: v }, t)),
      );
      sel.value = versionSel;
      parts.push(row('Version', sel, 'Saving a pinned version takes a few seconds: the hub installs it.'));
      if (versionSel === 'custom') {
        parts.push(
          row(
            'Version number',
            h('input.input.mono', {
              value: versionText,
              placeholder: 'For example 1.4.2',
              autocomplete: 'off',
              spellcheck: 'false',
              'aria-label': 'Version number',
              oninput: (e: Event) => {
                versionText = (e.target as HTMLInputElement).value;
              },
            }),
          ),
        );
      }
    } else {
      parts.push(
        row(
          'Command',
          h('input.input.mono', {
            value: command,
            placeholder: 'One program, for example claude-acp',
            autocomplete: 'off',
            spellcheck: 'false',
            'aria-label': 'Command',
            oninput: (e: Event) => {
              command = (e.target as HTMLInputElement).value;
              renderFoot();
            },
          }),
          command.trim() ? null : 'One program on the VM, by name or path. Arguments go below.',
        ),
      );
    }
    parts.push(
      row(
        'Home folder',
        h('input.input.mono', {
          value: home,
          placeholder: 'Default',
          autocomplete: 'off',
          spellcheck: 'false',
          'aria-label': 'Home folder',
          oninput: (e: Event) => {
            home = (e.target as HTMLInputElement).value;
          },
        }),
        'Where the engine keeps its files in the VM. Empty uses the default.',
      ),
    );
    parts.push(
      row(
        c.kind === 'local' ? 'Arguments' : 'Launch arguments',
        h(
          'textarea.textarea',
          {
            rows: 3,
            spellcheck: 'false',
            'aria-label': 'Launch arguments',
            oninput: (e: Event) => {
              argsText = (e.target as HTMLTextAreaElement).value;
            },
          },
          argsText,
        ),
        'One per line.',
        true,
      ),
    );
    parts.push(row('Variables', envEditor(), 'Plain settings for the engine. The agent can read them, so keys never go here.', true));
    const picker = modelPicker({
      choices: routes,
      value: route,
      placeholder: 'Plaza default',
      label: 'Route',
      onChange: (v) => {
        route = v;
      },
    });
    parts.push(row('Route', picker.el, 'The model the engine runs through. Empty uses the plaza default.', true));
    return h('div.pw-step-body', null, parts);
  }

  /** The plain variables of an engine: a name, a value and a remove button per row. */
  function envEditor(): HTMLElement {
    return h(
      'div.pw-env',
      null,
      env.map(([name, val], i) =>
        h(
          'div.pw-env-row',
          null,
          h('input.input.mono', {
            value: name,
            placeholder: 'NAME',
            autocomplete: 'off',
            spellcheck: 'false',
            'aria-label': 'Variable name',
            oninput: (e: Event) => {
              env[i]![0] = (e.target as HTMLInputElement).value;
            },
          }),
          h('input.input.mono', {
            value: val,
            placeholder: 'value',
            autocomplete: 'off',
            spellcheck: 'false',
            'aria-label': 'Variable value',
            oninput: (e: Event) => {
              env[i]![1] = (e.target as HTMLInputElement).value;
            },
          }),
          h(
            'button.iconbtn',
            {
              type: 'button',
              title: 'Remove variable',
              'aria-label': 'Remove variable',
              onclick: () => {
                env.splice(i, 1);
                renderStage();
              },
            },
            lu('x', 14),
          ),
        ),
      ),
      button(
        'Add variable',
        () => {
          env.push(['', '']);
          renderStage();
          const rows = stageHost.querySelectorAll<HTMLInputElement>('.pw-env-row input');
          rows[rows.length - 2]?.focus();
        },
        { kind: 'ghost', size: 'sm', icon: 'plus' },
      ),
    );
  }

  // ---- Create ---------------------------------------------------------------------

  /** The engine's body: the plain fields, and the version and command for the kinds that take them. */
  function engineEdit(c: Exclude<Choice, { kind: 'api' }>): EngineEdit {
    const edit: EngineEdit = {
      id,
      engine: c.kind === 'registry' ? `acp:${c.agent.id}` : c.kind === 'engine' ? c.engine.id : 'acp-local',
      label: label.trim() || id,
      accent,
      args: argsText.split('\n').map((s) => s.trim()).filter(Boolean),
      env: Object.fromEntries(env.filter(([n]) => n.trim()).map(([n, v]): [string, string] => [n.trim(), v])),
    };
    if (home.trim()) edit.home = home.trim();
    if (route) edit.route = route;
    if (c.kind === 'local') {
      edit.command = command.trim();
    } else {
      const version = versionSel === 'custom' ? versionText.trim() : versionSel;
      if (version) edit.version = version;
    }
    return edit;
  }

  async function submit(): Promise<void> {
    const c = choice;
    if (!c || busy || idProblem() || configProblem()) return;
    busy = true;
    err = '';
    renderFoot();
    try {
      if (c.kind === 'api') {
        const typed = key.trim();
        // A typed key gets the instance's own vault name; the kind's env var or none stays otherwise.
        const auth = typed && c.preset.auth !== 'none' ? { secret: id } : c.preset.auth;
        const list = models.split(',').map((m) => m.trim()).filter(Boolean);
        await providersApi.create({
          id,
          kind: c.preset.kind,
          label: label.trim() || c.preset.label,
          accent,
          base_url: baseUrl.trim() || null,
          auth,
          models: list.length ? list : c.preset.models,
          ...((c.preset.kind === 'codex-app-server' || c.preset.kind === 'opencode-local') && localExecutable.trim()
            ? { options: { executable: localExecutable.trim() } } : {}),
          enabled: true,
        });
        if (typed) {
          try {
            await providersApi.setKey(id, typed);
          } catch (e) {
            toast(`Added, but the key was not saved. ${msg(e)}`, 'error');
          }
        }
      } else {
        await providersApi.addEngine(engineEdit(c));
      }
      key = '';
      toast(`${label.trim() || id} added`);
      close();
      opts.onDone();
    } catch (e) {
      busy = false;
      err = msg(e);
      renderFoot();
    }
  }

  // ---- Open -----------------------------------------------------------------------

  /** Preselects the kind the caller asked for: an API kind opens Config with its base URL focused, an engine stays on Provider. */
  function preselect(kind: string, cat: ProviderCatalog): void {
    const preset = cat.api.find((p) => p.kind === kind || p.id === kind);
    if (preset) {
      startFrom({ kind: 'api', preset });
      step = 3;
      furthest = 3;
      return;
    }
    const engine = cat.engines.find((e) => e.id === kind);
    if (engine) startFrom({ kind: 'engine', engine });
  }

  renderAll();
  void Promise.all([providersApi.catalog(), providersApi.list(), providersApi.engines(), api.models().catch(() => null)])
    .then(([cat, instances, engines, view]) => {
      catalog = cat;
      taken = new Set([...instances.map((i) => i.id), ...engines.map((e) => e.id)]);
      routes = [
        { value: '', provider: '', label: 'Plaza default' },
        ...(view?.models ?? []).map((m) => ({
          value: `${m.provider}/${m.id}`,
          label: m.name ?? m.id,
          provider: m.provider,
          trains: m.trains_on_data === true,
        })),
      ];
      if (opts.kind) preselect(opts.kind, cat);
      renderAll();
      // The field the step opens on: the base URL of a preselected API kind, else the registry search.
      (step === 3 ? stageHost.querySelector<HTMLElement>('[data-f="base"]') : stageHost.querySelector<HTMLElement>('.pw-search-in'))?.focus();
    })
    .catch((e: unknown) => {
      loadError = msg(e);
      renderAll();
    });
  providersApi
    .registry()
    .then((r) => {
      registry = r.agents;
      renderRegistry();
    })
    .catch((e: unknown) => {
      registryError = msg(e);
      renderRegistry();
    });
}
