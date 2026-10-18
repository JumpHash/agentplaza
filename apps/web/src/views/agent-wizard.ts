// The agent wizard: define a role properly and spawn agents of it — or review and approve an
// orchestrator's proposal. Steps: Purpose → Role → Tools & limits → Prompt → Review & spawn.

import '../styles/agent-wizard.css';
import { api, type AgentProposal, type ModelInfo, type RoleDef, type RoleView, type Settings } from '../api';
import { attempt, badge, button, clip, input, modal, select, textarea } from '../ui/components';
import { h, replace } from '../ui/dom';
import { modelPicker, type ModelChoice, type ModelPicker } from '../ui/model-picker';
import { renderMarkdown } from '../ui/markdown';
import { tierChoices } from './agent-detail';

export interface WizardOptions {
  /** Start from this role (an existing role, or a proposal's definition). */
  prefill?: Partial<RoleDef>;
  /** Reviewing an orchestrator's proposal: the last step approves/rejects it instead of spawning. */
  proposal?: AgentProposal;
  /** Called after a spawn, an approval or a rejection. */
  onDone?: () => void;
  /** The project the spawned agents join at their first hello (else they wait on the bench). */
  project?: string;
}

const STEPS = ['Purpose', 'Role', 'Tools & limits', 'Prompt', 'Review'];
const EFFORTS = ['none', 'low', 'medium', 'high', 'xhigh', 'max'];
const EFFORT_OPTS: { value: string; label: string }[] = [{ value: '', label: 'Model default' }, ...EFFORTS.map((e) => ({ value: e, label: e }))];
const TOOL_GROUPS: { group: string; tools: string[] }[] = [
  { group: 'Code & files', tools: ['code_engine', 'tool_package'] },
  { group: 'Plaza', tools: ['plaza_hire'] },
  { group: 'Git', tools: ['plaza_git'] },
];
const MIN_BUDGET = 200;
const MAX_BUDGET = 50000;
const MAX_AGENTS = 8;
const ID_OK = /^[a-z0-9-]+$/;

type Mode = 'existing' | 'new';

/** The wizard's working copy: an existing role's id, or a new role's fields, plus the spawn settings. */
interface Draft {
  mode: Mode;
  roleId: string;
  id: string;
  title: string;
  purpose: string;
  responsibilities: string[];
  deliverables: string[];
  collaboration: string;
  boundaries: string[];
  tools: string[];
  budget: number;
  /** Bare `provider/model`. */
  model: string;
  effort: string;
  count: number;
  /** The prompt as the user edited it; null means "generated from the fields". */
  promptEdited: string | null;
}

type PromptFields = Pick<RoleDef, 'title' | 'purpose' | 'responsibilities' | 'deliverables' | 'collaboration' | 'boundaries' | 'budget'>;

const nonEmpty = (xs: string[]): string[] => xs.map((x) => x.trim()).filter(Boolean);
const slug = (s: string): string => s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
const plural = (n: number, word: string): string => `${n} ${word}${n === 1 ? '' : 's'}`;

/** Splits `provider/model[@effort]`. */
function splitModel(value: string): { base: string; effort: string } {
  const at = value.indexOf('@');
  return at < 0 ? { base: value, effort: '' } : { base: value.slice(0, at), effort: value.slice(at + 1) };
}

/** The role prompt, in the sections the server reads (# Title, ## Purpose, … ## Budget). */
function promptFor(f: PromptFields): string {
  const bullets = (xs: string[]): string => nonEmpty(xs).map((x) => `- ${x}`).join('\n');
  const sections: [string, string][] = [
    ['Purpose', f.purpose.trim()],
    ['Responsibilities', bullets(f.responsibilities)],
    ['Deliverables', bullets(f.deliverables)],
    ['Working with others', f.collaboration.trim()],
    ['Boundaries', bullets(f.boundaries)],
    ['Budget', `Default task budget: ${f.budget}.`],
  ];
  const body = sections.filter(([, text]) => text).map(([name, text]) => `## ${name}\n${text}`);
  return [`# ${f.title.trim()}`, ...body].join('\n\n') + '\n';
}

function fieldsOf(d: Draft): PromptFields {
  return {
    title: d.title,
    purpose: d.purpose,
    responsibilities: d.responsibilities,
    deliverables: d.deliverables,
    collaboration: d.collaboration,
    boundaries: d.boundaries,
    budget: d.budget,
  };
}

const currentPrompt = (d: Draft): string => d.promptEdited ?? promptFor(fieldsOf(d));
const modelValue = (d: Draft): string => (d.model && d.effort ? `${d.model}@${d.effort}` : d.model);

function roleDefOf(d: Draft, model: string): RoleDef {
  const def: RoleDef = {
    id: d.id,
    title: d.title.trim(),
    purpose: d.purpose.trim(),
    responsibilities: nonEmpty(d.responsibilities),
    deliverables: nonEmpty(d.deliverables),
    collaboration: d.collaboration.trim(),
    boundaries: nonEmpty(d.boundaries),
    tools: [...d.tools],
    budget: d.budget,
    model,
    prompt: '',
    created_by: '',
    created_ms: 0,
  };
  def.prompt = currentPrompt(d);
  return def;
}

/** Whether the current step's fields allow moving on. */
function isValid(d: Draft, step: number): boolean {
  switch (step) {
    case 0:
      return d.mode === 'new' || d.roleId !== '';
    case 1:
      return (
        d.mode === 'existing' ||
        (ID_OK.test(d.id) && d.title.trim() !== '' && d.purpose.trim() !== '' && nonEmpty(d.responsibilities).length > 0 && nonEmpty(d.deliverables).length > 0)
      );
    case 2:
      return d.model !== '' && (d.mode === 'existing' || (Number.isInteger(d.budget) && d.budget >= MIN_BUDGET && d.budget <= MAX_BUDGET));
    case 3:
      return d.mode === 'existing' || currentPrompt(d).trim() !== '';
    default:
      return true;
  }
}

/** Worker choices for the model picker: bare `provider/model`, one per model, the current one always offered. */
function workerChoices(settings: Settings, models: ModelInfo[], current: string): ModelChoice[] {
  const seen = new Set<string>();
  const out: ModelChoice[] = [];
  for (const c of tierChoices('worker', settings, models, undefined, current || undefined)) {
    const value = splitModel(c.value).base;
    if (!value || seen.has(value)) continue;
    seen.add(value);
    out.push({ value, label: c.label, provider: c.provider });
  }
  return out;
}

/** A block of label, control and hint, as the console's forms lay them out. */
function block(label: string, control: Node, hint?: string): HTMLElement {
  return h('div.field', null, h('span', null, label), control, hint ? h('span.hint', null, hint) : null);
}

function readOnly(label: string, body: string | Node): HTMLElement {
  return h('div.wiz-ro', null, h('span.wiz-kicker', null, label), typeof body === 'string' ? h('p', null, body || '—') : body);
}

function listNode(items: string[]): HTMLElement {
  return items.length ? h('ul', null, items.map((x) => h('li', null, x))) : h('p', null, '—');
}

/** Opens the wizard in a modal. */
export function openAgentWizard(opts: WizardOptions = {}): void {
  // contract: W1
  const proposal = opts.proposal;
  const seed: Partial<RoleDef> = (proposal ? proposal.role : opts.prefill) ?? {};
  const fromProposal = proposal !== undefined && !proposal.new_role;
  const d: Draft = {
    mode: fromProposal ? 'existing' : 'new',
    roleId: fromProposal ? (proposal?.role.id ?? '') : '',
    id: seed.id ?? '',
    title: seed.title ?? '',
    purpose: seed.purpose ?? '',
    responsibilities: [...(seed.responsibilities ?? [])],
    deliverables: [...(seed.deliverables ?? [])],
    collaboration: seed.collaboration ?? '',
    boundaries: [...(seed.boundaries ?? [])],
    tools: [...(seed.tools ?? [])],
    budget: seed.budget ?? 2000,
    model: '',
    effort: '',
    count: Math.min(MAX_AGENTS, Math.max(1, proposal?.count ?? 1)),
    promptEdited: seed.prompt || null,
  };
  const start = splitModel(proposal?.model || seed.model || '');
  d.model = start.base;
  d.effort = EFFORTS.includes(start.effort) ? start.effort : '';
  let idTouched = Boolean(seed.id);

  let close: () => void = () => undefined;
  let loaded = false;
  let busy = false;
  let step = 0;
  let editing = false;
  let rejecting = false;
  let roles: RoleView[] = [];
  let settings: Settings | null = null;
  let models: ModelInfo[] = [];
  let modelChoices: ModelChoice[] = [];
  let picker: ModelPicker | null = null;
  let loadError: string | null = null;

  const stepsHost = h('div');
  const bodyHost = h('div.wiz-body');
  const navHost = h('div.wiz-nav');
  const root = h('div.wizard', null, stepsHost, bodyHost, navHost);

  const choicesFor = (current: string): ModelChoice[] => (settings ? workerChoices(settings, models, current) : []);
  const existingDef = (): RoleDef | undefined =>
    roles.find((r) => r.id === d.roleId)?.def ?? (proposal && proposal.role.id === d.roleId ? proposal.role : undefined);

  function renderNav(): void {
    const last = step === STEPS.length - 1;
    const nodes: Node[] = [button('Back', () => go(step - 1), { kind: 'ghost', disabled: step === 0 || busy })];
    if (!last) {
      nodes.push(button('Next', () => go(step + 1), { kind: 'primary', disabled: !loaded || busy || !isValid(d, step) }));
    } else if (proposal) {
      nodes.push(button('Reject…', () => { rejecting = !rejecting; render(); }, { kind: 'danger', disabled: busy }));
      nodes.push(button('Approve and spawn', approve, { kind: 'primary', disabled: busy }));
    } else {
      const label = d.mode === 'new' ? `Create role and spawn ${plural(d.count, 'agent')}` : `Spawn ${plural(d.count, 'agent')}`;
      nodes.push(button(label, spawn, { kind: 'primary', disabled: busy }));
    }
    replace(navHost, nodes);
  }

  function render(focus?: string): void {
    replace(stepsHost, stepper());
    if (!loaded || !picker) {
      replace(
        bodyHost,
        loadError
          ? h('div.wiz-ro', null, h('p', null, `Could not load roles and models: ${loadError}`), button('Retry', () => { loadError = null; render(); void load(); }, { kind: 'ghost' }))
          : h('p.wiz-muted', null, 'Loading roles…'),
      );
      replace(navHost);
      return;
    }
    replace(bodyHost, bodyFor(step));
    if (focus) bodyHost.querySelector<HTMLElement>(focus)?.focus();
    renderNav();
  }

  const go = (to: number): void => {
    if (to < 0 || to >= STEPS.length || (to > step && !isValid(d, step))) return;
    step = to;
    editing = false;
    rejecting = false;
    render();
  };

  const finish = async (run: () => Promise<unknown>, message: string): Promise<void> => {
    busy = true;
    renderNav();
    const ok = await attempt(async () => {
      await run();
      return true;
    }, message);
    if (ok) {
      close();
      opts.onDone?.();
      return;
    }
    busy = false;
    renderNav();
  };

  /** Deletes a custom role after a confirm, then reloads the list. The hub's refusal (a role in use) shows as the toast. */
  const removeRole = async (r: RoleView): Promise<void> => {
    if (busy || !window.confirm(`Delete the role ${r.title} (${r.id})? Its definition is removed for good.`)) return;
    busy = true;
    renderNav();
    const gone = await attempt(async () => {
      await api.deleteRole(r.id);
      return true;
    }, `Deleted ${r.title}`);
    if (gone) {
      if (d.roleId === r.id) d.roleId = '';
      roles = (await attempt(() => api.roles())) ?? roles.filter((x) => x.id !== r.id);
    }
    busy = false;
    render();
  };

  const spawn = (): void => {
    const model = modelValue(d);
    const n = d.count;
    void finish(async () => {
      if (d.mode === 'new') {
        const def = roleDefOf(d, model);
        await api.saveRole(def);
        await api.spawnAgents(def.id, n, model, opts.project);
      } else {
        await api.spawnAgents(d.roleId, n, model, opts.project);
      }
    }, `Spawned ${plural(n, 'agent')}`);
  };

  const approve = (): void => {
    if (!proposal) return;
    const id = proposal.id;
    const model = modelValue(d);
    const n = d.count;
    const edits = d.mode === 'new' ? { role: roleDefOf(d, model), count: n, model } : { count: n, model };
    void finish(() => api.approveAgentProposal(id, edits), 'Proposal approved; agents spawned');
  };

  function stepper(): HTMLElement {
    return h(
      'ol.wiz-steps',
      { 'aria-label': 'Wizard steps' },
      STEPS.map((name, i) =>
        h(
          'li',
          {
            class: i === step ? 'on' : i < step ? 'done' : '',
            'aria-current': i === step ? 'step' : null,
            onclick: i < step ? () => go(i) : null,
          },
          h('span.wiz-num', null, String(i + 1)),
          h('span', null, name),
        ),
      ),
    );
  }

  /** A list of text rows that edit `items` in place; `Add` and the × remove rows. */
  function listEditor(key: string, items: string[], placeholder: string): HTMLElement {
    const list = h('div.wiz-list', { 'data-list': key });
    items.forEach((value, i) => {
      const el = input({ placeholder, 'aria-label': `${key} ${i + 1}` });
      el.value = value;
      el.addEventListener('input', () => {
        items[i] = el.value;
        renderNav();
      });
      list.appendChild(
        h('div.wiz-row', null, el, button('', () => {
          items.splice(i, 1);
          render();
        }, { kind: 'ghost', icon: 'x', title: `Remove ${key} ${i + 1}` })),
      );
    });
    list.appendChild(
      button('Add', () => {
        items.push('');
        render(`[data-list="${key}"] .wiz-row:last-of-type input`);
      }, { kind: 'ghost', size: 'sm' }),
    );
    return list;
  }

  function roleList(): HTMLElement {
    if (!roles.length) return h('p.wiz-muted', null, 'The plaza has no roles yet. Define a new one.');
    return h(
      'div.wiz-roles',
      { role: 'radiogroup', 'aria-label': 'Roles' },
      // A custom role gets a Delete button right after its card; the grid keeps both full width.
      roles.flatMap((r) => [
        h(
          'button.wiz-role',
          {
            type: 'button',
            role: 'radio',
            'aria-checked': d.roleId === r.id ? 'true' : 'false',
            class: d.roleId === r.id ? 'on' : '',
            onclick: () => {
              d.roleId = r.id;
              // The role's own model becomes the default; the user can still change it in step 3.
              const m = splitModel(r.def.model || '');
              if (m.base && picker) {
                d.model = m.base;
                d.effort = EFFORTS.includes(m.effort) ? m.effort : '';
                picker.setChoices(choicesFor(m.base));
                picker.value = m.base;
              }
              render();
            },
          },
          h('span.wiz-role-head', null, h('strong', null, r.title), badge(r.source === 'builtin' ? 'built-in' : 'custom')),
          h('span.wiz-role-id', null, r.id),
          h('span', null, clip(r.def.purpose, 180)),
          h('span.wiz-muted', null, `${plural(r.agents, 'agent')} run this role`),
        ),
        ...(r.source === 'builtin' ? [] : [button('Delete role', () => void removeRole(r), { kind: 'ghost', size: 'sm', disabled: busy, title: `Delete ${r.title}` })]),
      ]),
    );
  }

  function purposeNodes(): Node[] {
    const nodes: Node[] = [];
    if (proposal) {
      nodes.push(
        h(
          'div.wiz-proposal',
          null,
          h('span.wiz-kicker', null, 'The orchestrator proposes'),
          h('p', null, proposal.reason),
          h('span.wiz-kicker', null, "The owner's instruction"),
          h('blockquote', null, proposal.instruction),
        ),
      );
    }
    const choice = (mode: Mode, name: string, text: string): HTMLElement =>
      h(
        'button.wiz-choice',
        {
          type: 'button',
          class: d.mode === mode ? 'on' : '',
          'aria-pressed': d.mode === mode ? 'true' : 'false',
          onclick: () => {
            d.mode = mode;
            render();
          },
        },
        h('strong', null, name),
        h('span', null, text),
      );
    nodes.push(
      h('h3', null, 'What should this agent do?'),
      h(
        'div.wiz-choices',
        null,
        choice('existing', 'Use an existing role', 'Start agents of a role the plaza already has.'),
        choice('new', 'Define a new role', 'Write the role properly first, then start agents of it.'),
      ),
    );
    if (d.mode === 'existing') nodes.push(roleList());
    return nodes;
  }

  function roleForm(): Node[] {
    const idEl = input({ 'aria-label': 'Role id', placeholder: 'api-builder', spellcheck: false, autocomplete: 'off' });
    idEl.value = d.id;
    idEl.addEventListener('input', () => {
      idTouched = true;
      idEl.value = idEl.value.toLowerCase().replace(/[^a-z0-9-]/g, '');
      d.id = idEl.value;
      renderNav();
    });
    const titleEl = input({ 'aria-label': 'Role title', placeholder: 'API builder' });
    titleEl.value = d.title;
    titleEl.addEventListener('input', () => {
      d.title = titleEl.value;
      if (!idTouched) {
        d.id = slug(d.title);
        idEl.value = d.id;
      }
      renderNav();
    });
    const purposeEl = textarea({ 'aria-label': 'Purpose', rows: 3 });
    purposeEl.value = d.purpose;
    purposeEl.addEventListener('input', () => {
      d.purpose = purposeEl.value;
      renderNav();
    });
    const collabEl = textarea({ 'aria-label': 'Working with others', rows: 3 });
    collabEl.value = d.collaboration;
    collabEl.addEventListener('input', () => {
      d.collaboration = collabEl.value;
      renderNav();
    });
    return [
      h('h3', null, 'Describe the role'),
      block('Id', idEl, 'Lowercase letters, digits and hyphens. It follows the title until you edit it by hand.'),
      block('Title', titleEl, 'What this role is called, e.g. "API builder".'),
      block('Purpose', purposeEl, 'Why the role exists, in a sentence or two. Good: "Builds and tests the plaza HTTP API." Vague: "helps with the project."'),
      block(
        'Responsibilities',
        listEditor('responsibilities', d.responsibilities, 'Write and test the API server'),
        'Concrete, checkable duties: "Write and test the API server", not "help with code". At least one.',
      ),
      block('Deliverables', listEditor('deliverables', d.deliverables, 'A merged change with tests'), 'What the agent hands over when it is done. At least one.'),
      block('Working with others', collabEl, 'Who this role takes work from, and who reviews what it produces.'),
      block('Boundaries', listEditor('boundaries', d.boundaries, 'Does not merge to main'), 'What it must not do. Optional, but a clear edge keeps roles from overlapping.'),
    ];
  }

  function toolsBlock(): HTMLElement {
    const groups = TOOL_GROUPS.map((g) =>
      h(
        'div.wiz-group',
        null,
        h('span.wiz-kicker', null, g.group),
        g.tools.map((tool) => {
          const cb = h('input', { type: 'checkbox' });
          cb.checked = d.tools.includes(tool);
          cb.addEventListener('change', () => {
            d.tools = cb.checked ? [...d.tools, tool] : d.tools.filter((t) => t !== tool);
          });
          return h('label.wiz-check', null, cb, h('code', null, tool));
        }),
      ),
    );
    return h(
      'fieldset.wiz-tools',
      null,
      h('legend', null, 'Extra tools'),
      h(
        'p.wiz-muted',
        null,
        "Every agent already gets its workspace tools, the board, messages, memory and the toolbox; the plaza's laws decide the rest.",
      ),
      groups,
    );
  }

  function budgetBlock(): HTMLElement {
    const el = input({ type: 'number', min: MIN_BUDGET, max: MAX_BUDGET, step: 100, 'aria-label': 'Task budget' });
    el.value = String(d.budget);
    el.addEventListener('input', () => {
      d.budget = Number(el.value);
      renderNav();
    });
    return block('Task budget', el, 'The most one task may spend: 200 to 50,000 (default 2,000).');
  }

  function countBlock(): HTMLElement {
    const sel = select(
      Array.from({ length: MAX_AGENTS }, (_, i) => ({ value: String(i + 1), label: String(i + 1) })),
      String(d.count),
    );
    sel.setAttribute('aria-label', 'Agents to spawn');
    sel.addEventListener('change', () => {
      d.count = Number(sel.value);
      renderNav();
    });
    return block('Agents', sel, 'How many agents of this role to start (1 to 8).');
  }

  function modelBlock(): HTMLElement {
    const effort = select(EFFORT_OPTS, d.effort);
    effort.setAttribute('aria-label', 'Reasoning effort');
    effort.addEventListener('change', () => {
      d.effort = effort.value;
    });
    return block(
      'Model and effort',
      h('div.wiz-model', null, picker?.el ?? h('span.wiz-muted', null, 'No models available'), effort),
      'Worker models only. Effort is saved with the model as model@effort; "Model default" leaves it to the model.',
    );
  }

  function promptForm(): Node[] {
    const edited = d.promptEdited !== null;
    const reset = edited
      ? button('Reset to generated', () => {
          d.promptEdited = null;
          editing = false;
          render();
        }, { kind: 'ghost', size: 'sm' })
      : null;
    if (editing) {
      const ta = textarea({ 'aria-label': 'Role prompt' });
      ta.value = currentPrompt(d);
      ta.rows = 18;
      ta.classList.add('wiz-prompt-edit');
      ta.addEventListener('input', () => {
        d.promptEdited = ta.value;
        renderNav();
      });
      return [
        h(
          'div.wiz-prompt-bar',
          null,
          h('span.wiz-muted', null, 'Markdown. This text is saved as the role prompt.'),
          reset,
          button('Done', () => {
            editing = false;
            render();
          }, { kind: 'ghost', size: 'sm' }),
        ),
        ta,
      ];
    }
    return [
      h(
        'div.wiz-prompt-bar',
        null,
        h('span.wiz-muted', null, edited ? 'Edited by hand.' : 'Generated from the fields in steps 2 and 3.'),
        reset,
        button('Edit', () => {
          editing = true;
          render('.wiz-prompt-edit');
        }, { kind: 'ghost', size: 'sm' }),
      ),
      h('div.wiz-prompt', null, renderMarkdown(currentPrompt(d))),
    ];
  }

  function previewRole(def: RoleDef | undefined): Node[] {
    if (!def) return [h('p.wiz-muted', null, 'No role selected. Go back and choose one.')];
    return [
      h('h3', null, def.title),
      h('p.wiz-muted', null, `${def.id}, an existing role`),
      readOnly('Purpose', def.purpose),
      readOnly('Responsibilities', listNode(def.responsibilities)),
      readOnly('Deliverables', listNode(def.deliverables)),
      readOnly('Working with others', def.collaboration),
      readOnly('Boundaries', listNode(def.boundaries)),
    ];
  }

  function reviewNodes(): Node[] {
    const def = d.mode === 'existing' ? existingDef() : undefined;
    const modelLabel = modelChoices.find((c) => c.value === d.model)?.label ?? (d.model || 'Model default');
    const effort = d.effort ? ` · ${d.effort} effort` : '';
    const rows: [string, string | Node][] = [];
    if (d.mode === 'existing') {
      rows.push(['Role', `${def?.title ?? d.roleId} (${d.roleId}), existing`]);
    } else {
      rows.push(
        ['New role', `${d.title.trim()} (${d.id})`],
        ['Purpose', d.purpose.trim()],
        ['Responsibilities', listNode(nonEmpty(d.responsibilities))],
        ['Deliverables', listNode(nonEmpty(d.deliverables))],
        ['Working with others', d.collaboration.trim()],
        ['Boundaries', listNode(nonEmpty(d.boundaries))],
        ['Tools', d.tools.join(', ') || 'Workspace tools only'],
        ['Budget', String(d.budget)],
      );
    }
    rows.push(['Agents', plural(d.count, 'agent')], ['Model', `${modelLabel}${effort}`]);
    const summary = h(
      'dl.wiz-summary',
      null,
      rows.map(([k, v]) => h('div', null, h('dt', null, k), h('dd', null, v))),
    );
    const nodes: Node[] = [h('h3', null, proposal ? 'Approve it, or reject it with a reason' : 'Review and spawn'), summary];
    if (proposal && rejecting) nodes.push(rejectPanel(proposal.id));
    return nodes;
  }

  function rejectPanel(id: string): HTMLElement {
    const reason = textarea({ 'aria-label': 'Reason for rejecting', rows: 3, placeholder: 'Why this agent is not needed' });
    const send = button('Reject proposal', () => {
      void finish(() => api.rejectAgentProposal(id, reason.value.trim()), 'Proposal rejected');
    }, { kind: 'danger', disabled: true });
    reason.addEventListener('input', () => {
      send.disabled = reason.value.trim() === '';
    });
    return h('div.wiz-reject', null, h('h3', null, 'Reject this proposal'), block('Reason', reason, 'The orchestrator reads this note.'), send);
  }

  function bodyFor(i: number): Node[] {
    const existing = d.mode === 'existing';
    const def = existing ? existingDef() : undefined;
    switch (i) {
      case 0:
        return purposeNodes();
      case 1:
        return existing ? [h('h3', null, 'The role'), ...previewRole(def)] : roleForm();
      case 2:
        if (existing) {
          return [
            h('h3', null, 'Tools & limits'),
            h('p.wiz-muted', null, "This role's tools and budget are fixed by its definition; choose how many agents to start and on which model."),
            readOnly('Tools', def ? def.tools.join(', ') || 'Workspace tools only' : '—'),
            readOnly('Budget', def ? String(def.budget) : '—'),
            countBlock(),
            modelBlock(),
          ];
        }
        return [h('h3', null, 'Tools & limits'), toolsBlock(), budgetBlock(), countBlock(), modelBlock()];
      case 3:
        if (existing) return [h('h3', null, 'Prompt'), h('div.wiz-prompt', null, renderMarkdown(def ? def.prompt || promptFor(def) : ''))];
        return [h('h3', null, 'Prompt'), ...promptForm()];
      default:
        return reviewNodes();
    }
  }

  async function load(): Promise<void> {
    try {
      const [rv, sv, mv] = await Promise.all([api.roles(), api.settings(), api.models()]);
      roles = rv;
      settings = sv;
      models = mv.models;
      if (!d.model) {
        const fallback = splitModel(sv.default_model || '');
        d.model = fallback.base;
      }
      modelChoices = workerChoices(sv, models, d.model);
      if (!d.model && modelChoices[0]) d.model = modelChoices[0].value;
      picker = modelPicker({
        choices: modelChoices,
        value: d.model,
        placeholder: 'Choose a model',
        label: 'Worker model',
        onChange: (value) => {
          d.model = value;
        },
      });
      loaded = true;
      render();
    } catch (err) {
      loadError = err instanceof Error ? err.message : String(err);
      render();
    }
  }

  close = modal(proposal ? "Review the orchestrator's proposal" : 'Spawn agents', root);
  root.closest('.modal')?.classList.add('wizard');
  void load();
}
