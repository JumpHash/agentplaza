// The model policy: which models each tier may run, the defaults, the limits, the engines, the sleep settings and the
// prices. One document (apid's /v1/settings), saved whole by the Save button in the screen's head.

import '../styles/proto/settings-a.css';
import { api, isContributor, type ModelsView, type Settings } from '../api';
import { agents } from '../store';
import { attempt, badge, button, dot, empty, field, input, select, textarea, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { modelPicker } from '../ui/model-picker';
import { tierChoices } from '../ui/tier-choices';
import { CONTRIBUTOR_NOTE } from './models-providers';

type Tier = 'orchestrator' | 'worker';
type Price = { input: number; output: number };

const EFFORTS = ['default', 'none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'];
const LIST_ID = 'mdl-known';

export function mountModelPolicy(root: HTMLElement, actions: HTMLElement): Cleanup {
  let alive = true;
  replace(root, h('p.mdl-faint', null, 'Loading…'));
  void attempt(() => Promise.all([api.settings(), api.models()])).then((data) => {
    if (!alive) return;
    if (!data) {
      replace(root, empty('alert', 'Models unavailable', 'apid did not answer. Reload to try again.'));
      return;
    }
    build(root, actions, data[0], data[1]);
  });
  return () => {
    alive = false;
  };
}

/** The checkbox row of the policy's switches (the prototype's `label.check`). */
function check(box: HTMLInputElement, text: string): HTMLElement {
  return h('label.check', null, box, h('span', null, text));
}

function build(root: HTMLElement, actions: HTMLElement, settings: Settings, modelsView: ModelsView): void {
  let saved: Settings = settings;
  let copy: Settings = clone(settings);
  const devModels = settings.dev_models === true;
  const known = new Set(modelsView.models.map((m) => `${m.provider}/${m.id}`));
  const trainers = new Set(modelsView.models.filter(isContributor).map((m) => `${m.provider}/${m.id}`));
  const errors = new Set<string>();
  let pricesOpen = false;

  const saveBtn = button('Save', () => void save(), { kind: 'primary', icon: 'check' });
  const datalist = h(
    'datalist',
    { id: LIST_ID },
    ...modelsView.models.map((m) => h('option', { value: `${m.provider}/${m.id}` }, m.name ?? `${m.provider}/${m.id}`)),
  );
  const body = h('div.mdl-stack', null, datalist);
  replace(actions, saveBtn);
  replace(root, body);

  function sync(): void {
    saveBtn.disabled = errors.size > 0 || JSON.stringify(copy) === JSON.stringify(saved);
  }

  function render(): void {
    errors.clear();
    replace(body, datalist, policyCard(), defaultsCard(), limitsCard(), enginesCard(), sleepCard(), pricesCard());
    sync();
  }

  async function save(): Promise<void> {
    saveBtn.disabled = true;
    const next = await attempt(() => api.saveSettings(copy), 'Policy saved');
    if (next) {
      saved = clone(next);
      copy = clone(next);
      render();
    } else {
      sync();
    }
  }

  function toggleError(el: HTMLElement, key: string, bad: boolean): void {
    if (bad) errors.add(key);
    else errors.delete(key);
    el.classList.toggle('invalid', bad);
    sync();
  }

  function policyCard(): HTMLElement {
    return h(
      'div.mdl-card.mdl-stack',
      null,
      h('h3.mdl-h3', { style: 'margin:0' }, 'Model policy'),
      devModels ? badge('Development models enabled — policy relaxed', 'warn') : null,
      h('p.mdl-faint', null, "Agents may only run the models on their tier's list."),
      h('div.mdl-two', null, tierColumn('orchestrator', 'Orchestrator'), tierColumn('worker', 'Workers')),
    );
  }

  function tierColumn(tier: Tier, title: string): HTMLElement {
    const rows = copy.model_policy[tier].map((entry, index) => policyRow(tier, entry, index));
    const addInput = input({ placeholder: 'provider/model', list: LIST_ID, autocomplete: 'off', 'aria-label': `${title}: model to add`, style: 'flex:1;min-width:160px' });
    const add = (): void => {
      const value = addInput.value.trim();
      if (value === '') return;
      if (!/^[^/\s]+\/\S+$/.test(parseEntry(value).base)) {
        toast('Use provider/model, with an optional @effort', 'error');
        return;
      }
      if (!copy.model_policy[tier].includes(value)) copy.model_policy[tier] = [...copy.model_policy[tier], value];
      render();
    };
    addInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        add();
      }
    });
    return h(
      'div',
      null,
      h('h3.mdl-h3', null, title),
      h('ul.mdl-policy', null, ...(rows.length ? rows : [h('li.mdl-faint', null, 'No models')])),
      h('div.mdl-row-a', null, addInput, button('Add model', add, { size: 'sm' })),
    );
  }

  function policyRow(tier: Tier, entry: string, index: number): HTMLElement {
    const { base, effort } = parseEntry(entry);
    const listed = known.has(base);
    const availability = dot(listed ? 'ok' : 'muted');
    availability.title = listed ? 'Listed by its provider' : 'Not listed by its provider';
    const pinned = effort === null ? null : h('span.badge-t.t-accent', { title: `@${effort}` }, 'effort pinned');
    const trains = trainers.has(base) ? badge('Trains on data', 'warn') : null;
    const remove = button(
      '',
      () => {
        copy.model_policy[tier] = copy.model_policy[tier].filter((_, i) => i !== index);
        render();
      },
      { kind: 'ghost', size: 'sm', icon: 'x', title: `Remove ${entry}` },
    );
    return h(
      'li',
      { 'data-key': `policy-${tier}-${entry}` },
      h('span', { title: availability.title, style: 'display:inline-flex;flex:none' }, availability),
      h('span.mdl-trunc', { title: entry }, base),
      pinned,
      trains,
      remove,
    );
  }

  function defaultsCard(): HTMLElement {
    const orch = modelPicker({
      choices: tierChoices('orchestrator', copy, modelsView.models, undefined, copy.orchestrator_model),
      value: copy.orchestrator_model,
      label: 'Orchestrator model',
      onChange: (v) => {
        copy.orchestrator_model = v;
        sync();
      },
    });
    const worker = modelPicker({
      choices: tierChoices('worker', copy, modelsView.models, undefined, copy.default_model),
      value: copy.default_model,
      label: 'Worker default',
      onChange: (v) => {
        copy.default_model = v;
        sync();
      },
    });
    const roles = Array.from(
      new Set([
        ...agents
          .get()
          .filter((a) => a.tier !== 'orchestrator' && a.role !== 'orchestrator')
          .map((a) => a.role),
        ...Object.keys(copy.role_models),
      ]),
    ).sort();
    const roleRows = roles.map((role) =>
      h('tr', { 'data-key': `role-${role}` }, h('td.mono', null, role), h('td', null, roleSelect(role))),
    );
    return h(
      'div.mdl-card',
      null,
      h('h3.mdl-h3', { style: 'margin-top:0' }, 'Defaults'),
      trainingField(),
      h(
        'div.mdl-two',
        null,
        field('Orchestrator model', orch.el),
        field('Worker default', worker.el),
      ),
      h('h3.mdl-h3', null, 'Per role'),
      roles.length
        ? h(
            'div.tbl-wrap',
            null,
            h('table.mdl-table', null, h('thead', null, h('tr', null, h('th', null, 'Role'), h('th', null, 'Model'))), h('tbody', null, roleRows)),
          )
        : empty('agents', 'No roles connected', ''),
    );
  }

  /** The owner's confirmation for contributor models; off until the owner turns it on. */
  function trainingField(): HTMLElement {
    const box = input({ type: 'checkbox' });
    box.checked = copy.allow_training_models === true;
    box.addEventListener('change', () => {
      copy.allow_training_models = box.checked;
      sync();
    });
    return h('div', null, check(box, 'Allow as defaults, role models and policy entries'), h('p.mdl-faint', { style: 'margin:8px 0 12px' }, CONTRIBUTOR_NOTE));
  }

  function roleSelect(role: string): HTMLElement {
    const current = copy.role_models[role] ?? '';
    // A role that runs the orchestrator tier is limited to its list; any other role may run any model.
    const orchestrator = role === 'orchestrator' || agents.get().some((a) => a.role === role && a.tier === 'orchestrator');
    return modelPicker({
      choices: tierChoices(orchestrator ? 'orchestrator' : 'worker', copy, modelsView.models, { label: orchestrator ? 'Orchestrator default' : 'Worker default' }, current),
      value: current,
      label: `${role} model`,
      onChange: (v) => {
        if (v === '') delete copy.role_models[role];
        else copy.role_models[role] = v;
        sync();
      },
    }).el;
  }

  function limitsCard(): HTMLElement {
    const turns = numeric('max_turns', 1, 200, copy.max_turns, (n) => {
      copy.max_turns = n;
    });
    const tokens = numeric('max_tokens', 0, 1_000_000_000, copy.max_tokens, (n) => {
      copy.max_tokens = n;
    });
    const effort = select(
      EFFORTS.map((v) => ({ value: v, label: v })),
      copy.effort ?? 'default',
    );
    effort.addEventListener('change', () => {
      copy.effort = effort.value === 'default' ? null : effort.value;
      sync();
    });
    const block = textarea({ rows: 5, spellcheck: 'false', placeholder: 'example.com\n*.example.net' });
    block.value = (copy.fetch_block ?? []).join('\n');
    block.addEventListener('input', () => {
      copy.fetch_block = block.value
        .split(/[\n,]/)
        .map((s) => s.trim())
        .filter((s) => s !== '');
      sync();
    });
    return h(
      'div.mdl-card',
      null,
      h('h3.mdl-h3', { style: 'margin-top:0' }, 'Limits'),
      h(
        'div.mdl-two',
        null,
        field('Max turns', turns),
        field('Max tokens (0 = provider default)', tokens),
        field('Effort', effort),
        field('Fetch blocklist (every other host is allowed)', block),
      ),
    );
  }

  /** Engines are providers now: their switches, versions and routes are in the Providers tab. */
  function enginesCard(): HTMLElement {
    return h(
      'div.mdl-card',
      null,
      h('h3.mdl-h3', { style: 'margin-top:0' }, 'Engines'),
      h(
        'p.mdl-faint',
        null,
        'The coding engines an agent may run are in the Providers tab now: turn each on or off, pick its version and the model it runs through.',
      ),
      h('a.btn.sm', { href: '#/settings/models/models' }, 'Open Providers'),
    );
  }

  /** The sleep settings: how long an idle agent's VM waits before it sleeps, who never sleeps, and the checkpoints. */
  function sleepCard(): HTMLElement {
    const idle = numeric('idle_sleep_min', 0, 1440, copy.idle_sleep_min, (n) => {
      copy.idle_sleep_min = n;
    });
    const doze = numeric('doze_idle_s', 0, 86400, copy.doze_idle_s, (n) => {
      copy.doze_idle_s = n;
    });
    const roles = input({ placeholder: 'coder, reviewer', spellcheck: 'false', value: copy.sleep_never_roles.join(', ') });
    roles.addEventListener('input', () => {
      copy.sleep_never_roles = sleepRoles(roles.value);
      sync();
    });
    const auto = input({ type: 'checkbox' });
    auto.checked = copy.vm_auto_checkpoints === true;
    auto.addEventListener('change', () => {
      copy.vm_auto_checkpoints = auto.checked;
      sync();
    });
    return h(
      'div.mdl-card.mdl-stack',
      null,
      h('h3.mdl-h3', { style: 'margin:0' }, 'Sleep and checkpoints'),
      h('div.mdl-two', null, field('Sleep after idle (minutes, 0 = never)', idle), field('Doze after (seconds idle, 0 = never)', doze), field('Roles that never sleep (comma-separated)', roles)),
      check(auto, 'Checkpoint before irreversible actions'),
      h('p.mdl-faint', null, "A checkpoint of the agent's VM is taken before each irreversible tool call."),
    );
  }

  function numeric(key: string, min: number, max: number, value: number, set: (n: number) => void): HTMLInputElement {
    const el = input({ type: 'number', min, max, step: 1, value });
    el.addEventListener('input', () => {
      const n = wholeInRange(el.value, min, max);
      if (n !== null) set(n);
      toggleError(el, key, n === null);
    });
    return el;
  }

  function pricesCard(): HTMLElement {
    const bases = copy.model_policy.orchestrator.concat(copy.model_policy.worker).map((e) => parseEntry(e).base);
    const keys = Array.from(new Set([...bases, ...Object.keys(copy.prices)])).sort();
    const priceInput = (key: string, side: keyof Price): HTMLInputElement => {
      const current = copy.prices[key] as Price | undefined;
      const el = input({ type: 'number', min: 0, step: 'any', value: current ? current[side] : 0 });
      el.addEventListener('input', () => {
        const n = Number(el.value);
        const ok = el.value.trim() !== '' && Number.isFinite(n) && n >= 0;
        if (ok) {
          const price: Price = copy.prices[key] ?? { input: 0, output: 0 };
          price[side] = n;
          copy.prices[key] = price;
        }
        toggleError(el, `price:${key}:${side}`, !ok);
      });
      return el;
    };
    const priceRows = keys.map((key) =>
      h(
        'tr',
        { 'data-key': `price-${key}` },
        h('td.mono', { title: key }, key, trainers.has(key) ? ' ' : null, trainers.has(key) ? badge('Trains on data', 'warn') : null),
        h('td', null, priceInput(key, 'input')),
        h('td', null, priceInput(key, 'output')),
      ),
    );
    const details = h(
      'details',
      null,
      h('summary', null, 'Models'),
      keys.length
        ? h(
            'div.tbl-wrap',
            null,
            h(
              'table.mdl-table',
              null,
              h('thead', null, h('tr', null, h('th', null, 'Model'), h('th', null, 'Input (credits / 1M)'), h('th', null, 'Output (credits / 1M)'))),
              h('tbody', null, priceRows),
            ),
          )
        : empty('models', 'No models listed', ''),
    ) as HTMLDetailsElement;
    details.open = pricesOpen;
    details.addEventListener('toggle', () => {
      pricesOpen = details.open;
    });
    return h('div.mdl-card', null, details);
  }

  render();
}

/** A whole number in [min, max] from a field's text, or null when the text is blank, not whole, or out of range. */
export function wholeInRange(text: string, min: number, max: number): number | null {
  const n = Number(text);
  return text.trim() !== '' && Number.isInteger(n) && n >= min && n <= max ? n : null;
}

/** The roles a comma-separated field names: trimmed, without blanks or repeats, in the order typed. */
export function sleepRoles(text: string): string[] {
  return [...new Set(text.split(',').map((s) => s.trim()).filter((s) => s !== ''))];
}

/** Splits `provider/model[@effort]` into its base id and its pinned effort. */
function parseEntry(entry: string): { base: string; effort: string | null } {
  const at = entry.lastIndexOf('@');
  return at > 0 ? { base: entry.slice(0, at), effort: entry.slice(at + 1) } : { base: entry, effort: null };
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}
