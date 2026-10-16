// The model policy: which models each tier may run, the defaults, the limits
// and the prices. One document (apid's /v1/settings), saved whole.

import '../styles/models-policy.css';
import { api, isContributor, type ModelsView, type Settings } from '../api';
import { agents } from '../store';
import { attempt, badge, button, card, dot, empty, field, input, select, table, TRAINS_NOTE, textarea, toast, trainsBadge } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { modelPicker } from '../ui/model-picker';
import { tierChoices } from './agent-detail';
import { DEFAULT_ENGINES, ENGINES, toggleEngine } from './engines-model';

type Tier = 'orchestrator' | 'worker';
type Price = { input: number; output: number };

const EFFORTS = ['default', 'none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'];
const LIST_ID = 'models-policy-known';

export function mountModelPolicy(root: HTMLElement): Cleanup {
  let alive = true;
  replace(root, h('p.faint', null, 'Loading…'));
  void attempt(() => Promise.all([api.settings(), api.models()])).then((data) => {
    if (!alive) return;
    if (!data) {
      replace(root, empty('alert', 'Models unavailable', 'apid did not answer. Reload to try again.'));
      return;
    }
    build(root, data[0], data[1]);
  });
  return () => {
    alive = false;
  };
}

function build(root: HTMLElement, settings: Settings, modelsView: ModelsView): void {
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
  const body = h('div.stack.models-policy');
  replace(root, h('div.policy-bar', null, saveBtn), datalist, body);

  function sync(): void {
    saveBtn.disabled = errors.size > 0 || JSON.stringify(copy) === JSON.stringify(saved);
  }

  function render(): void {
    errors.clear();
    replace(body, policyCard(), defaultsCard(), limitsCard(), enginesCard(), sleepCard(), pricesCard());
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
    return card(
      'Model policy',
      h(
        'div.stack',
        null,
        h('p.policy-lead', null, "Agents may only run the models on their tier's list."),
        h('div.policy-cols', null, tierColumn('orchestrator', 'Orchestrator'), tierColumn('worker', 'Workers')),
      ),
      { actions: devModels ? [badge('Development models enabled — policy relaxed', 'warn')] : [] },
    );
  }

  function tierColumn(tier: Tier, title: string): HTMLElement {
    const list = copy.model_policy[tier];
    const rows = list.map((entry, index) => policyRow(tier, entry, index));
    const addInput = input({ placeholder: 'provider/model', list: LIST_ID, autocomplete: 'off', 'aria-label': `${title}: model to add` });
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
      'section.policy-col',
      null,
      h('h3', null, title),
      h('ul.policy-list', null, ...(rows.length ? rows : [h('li.faint', null, 'No models')])),
      h('div.policy-add', null, addInput, button('Add model', add, { size: 'sm' })),
    );
  }

  function policyRow(tier: Tier, entry: string, index: number): HTMLElement {
    const { base, effort } = parseEntry(entry);
    const listed = known.has(base);
    const availability = dot(listed ? 'ok' : 'muted');
    availability.title = listed ? 'Listed by its provider' : 'Not listed by its provider';
    const pinned = effort === null ? null : badge('effort pinned', 'accent');
    if (pinned) pinned.title = `@${effort}`;
    const trains = trainers.has(base) ? trainsBadge() : null;
    const remove = button(
      '',
      () => {
        copy.model_policy[tier] = copy.model_policy[tier].filter((_, i) => i !== index);
        render();
      },
      { kind: 'ghost', size: 'sm', icon: 'x', title: `Remove ${entry}` },
    );
    return h('li.policy-row', { 'data-key': `policy-${tier}-${entry}` }, availability, h('span.mono.truncate', { title: entry }, base), pinned, trains, remove);
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
    const roleRows = roles.length
      ? table<string>(
          [
            { label: 'Role', cell: (role) => h('span.mono', null, role) },
            { label: 'Model', cell: (role) => roleSelect(role) },
          ],
          roles,
        )
      : empty('agents', 'No roles connected');
    return card(
      'Defaults',
      h('div.stack', null, trainingField(), field('Orchestrator model', orch.el), field('Worker default', worker.el), h('h3', null, 'Per role'), roleRows),
    );
  }

  /** The owner's confirmation for contributor models; off until the owner turns it on. */
  function trainingField(): HTMLElement {
    const box = input({ type: 'checkbox', style: 'width:auto' });
    box.checked = copy.allow_training_models === true;
    box.addEventListener('change', () => {
      copy.allow_training_models = box.checked;
      sync();
    });
    return h('div.stack', null, h('h3', null, 'Contributor models'), h('label', { style: 'display:flex;align-items:center;justify-content:flex-start;gap:8px;width:fit-content' }, box, h('span', null, 'Allow as defaults, role models and policy entries')), h('p.faint', null, TRAINS_NOTE));
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
    return card(
      'Limits',
      h(
        'div.stack',
        null,
        field('Max turns', turns),
        field('Max tokens (0 = provider default)', tokens),
        field('Effort', effort),
        field('Fetch blocklist (every other host is allowed)', block),
      ),
    );
  }

  /** The coding engines an agent may run, and how they run. At least one engine stays on (the last box is locked). */
  function enginesCard(): HTMLElement {
    // A hub from before the engine settings sends neither field: it runs the three engines over ACP.
    copy.engines_enabled ??= [...DEFAULT_ENGINES];
    const enabled = copy.engines_enabled;
    const boxes = ENGINES.map((engine) => {
      const on = enabled.includes(engine.value);
      const box = input({ type: 'checkbox', style: 'width:auto' });
      box.checked = on;
      box.disabled = on && enabled.length === 1;
      box.title = box.disabled ? 'At least one engine stays on' : '';
      box.addEventListener('change', () => {
        copy.engines_enabled = toggleEngine(copy.engines_enabled, engine.value, box.checked);
        render();
      });
      return h(
        'label',
        { style: 'display:flex;align-items:center;justify-content:flex-start;gap:8px;width:fit-content' },
        box,
        h('span', null, engine.label),
        h('span.mono.faint', null, engine.value),
      );
    });
    return card(
      'Engines',
      h(
        'div.stack',
        null,
        h('p.faint', null, 'The coding engines an agent may run. At least one stays on. Every engine runs over ACP: a live stream, with every action checked by the laws.'),
        h('div.stack', null, ...boxes),
      ),
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
    const auto = input({ type: 'checkbox', style: 'width:auto' });
    auto.checked = copy.vm_auto_checkpoints === true;
    auto.addEventListener('change', () => {
      copy.vm_auto_checkpoints = auto.checked;
      sync();
    });
    return card(
      'Sleep and checkpoints',
      h(
        'div.stack',
        null,
        field('Sleep after idle (minutes, 0 = never)', idle),
        field('Doze after (seconds idle, 0 = never)', doze),
        field('Roles that never sleep (comma-separated)', roles),
        h('div.stack', null, h('label', { style: 'display:flex;align-items:center;justify-content:flex-start;gap:8px;width:fit-content' }, auto, h('span', null, 'Checkpoint before irreversible actions')), h('p.faint', null, "A checkpoint of the agent's VM is taken before each irreversible tool call.")),
      ),
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
    const priceTable = keys.length
      ? table<string>(
          [
            { label: 'Model', cell: (k) => h('span', null, h('span.mono.truncate', { title: k }, k), trainers.has(k) ? trainsBadge() : null) },
            { label: 'Input (credits / 1M)', num: true, cell: (k) => priceInput(k, 'input') },
            { label: 'Output (credits / 1M)', num: true, cell: (k) => priceInput(k, 'output') },
          ],
          keys,
        )
      : empty('models', 'No models listed');
    const details = h('details.prices', null, h('summary', null, 'Models'), priceTable) as HTMLDetailsElement;
    details.open = pricesOpen;
    details.addEventListener('toggle', () => {
      pricesOpen = details.open;
    });
    return card('Prices', details);
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
