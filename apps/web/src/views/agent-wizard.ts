// The agent wizard: define a role properly and spawn agents of it, or review and decide on an orchestrator's proposal.
// Steps: Purpose → Role → Tools & limits → Prompt → Review. A proposal walks The proposal → The role → Decide.

import { api, type AgentProposal, type ModelInfo, type RoleDef, type RoleView, type Settings } from '../api';
import { attempt, badge, button, clip, field, input, modal, select, textarea } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
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
}

const STEPS = ['Purpose', 'Role', 'Tools & limits', 'Prompt', 'Review'];
const PROPOSAL_STEPS = ['The proposal', 'The role', 'Decide'];
const EFFORTS = ['none', 'low', 'medium', 'high', 'xhigh', 'max'];
const EFFORT_OPTS: { value: string; label: string }[] = [{ value: '', label: 'Model default' }, ...EFFORTS.map((e) => ({ value: e, label: e }))];
const TOOL_GROUPS: { group: string; tools: string[] }[] = [
  { group: 'Code & files', tools: ['code_engine', 'tool_package'] },
  { group: 'Plaza', tools: ['plaza_hire'] },
  { group: 'Weft', tools: ['weft_status', 'weft_diff', 'weft_describe', 'weft_log'] },
];
const MIN_BUDGET = 200;
const MAX_BUDGET = 50000;
const MAX_AGENTS = 8;
const ID_OK = /^[a-z0-9-]+$/;

type Mode = 'existing' | 'new';
type ListKey = 'responsibilities' | 'deliverables' | 'boundaries';

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

/** A read-only row: a muted label over its value, as the prototype's previews lay them out. */
function ro(label: string, value: string | Node): HTMLElement {
  return h('div', null, h('div.muted', null, label), h('div', null, value));
}

/** A list of the items, or a dash when there are none. */
function listNode(items: string[]): string | Node {
  const xs = nonEmpty(items);
  return xs.length ? h('ul', null, xs.map((x) => h('li', null, x))) : '—';
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
  let rejectReason = '';
  let delConfirm: string | null = null;
  /** Runs once after the next redraw (focus a field the reader just opened). */
  let afterRender: (() => void) | null = null;
  let roles: RoleView[] = [];
  let settings: Settings | null = null;
  let models: ModelInfo[] = [];
  let modelChoices: ModelChoice[] = [];
  let picker: ModelPicker | null = null;
  let loadError: string | null = null;

  const stepNames = (): string[] => (proposal ? PROPOSAL_STEPS : STEPS);
  const stepsHost = h('div');
  const bodyHost = h('div.ag-wiz-body');
  const navHost = h('div.cluster', { style: 'justify-content:flex-end;width:100%' });
  const root = h('div', null, stepsHost, bodyHost);

  const choicesFor = (current: string): ModelChoice[] => (settings ? workerChoices(settings, models, current) : []);
  const existingDef = (): RoleDef | undefined =>
    roles.find((r) => r.id === d.roleId)?.def ?? (proposal && proposal.role.id === d.roleId ? proposal.role : undefined);
  /** The step's fields allow Next. A proposal's steps are read-only, so they always do. */
  const canAdvance = (): boolean => proposal !== undefined || isValid(d, step);
  const modelLabelOf = (): string => modelChoices.find((c) => c.value === d.model)?.label ?? (d.model || 'Model default');

  function renderNav(): void {
    const last = step === stepNames().length - 1;
    const nodes: Node[] = [button('Back', () => go(step - 1), { kind: 'ghost', disabled: step === 0 || busy })];
    if (proposal) {
      if (!last) {
        nodes.push(button('Next', () => go(step + 1), { kind: 'primary', disabled: !loaded || busy }));
      } else if (rejecting) {
        nodes.push(button('Keep it', keepIt, { kind: 'ghost' }));
        nodes.push(button('Reject', rejectNow, { kind: 'danger', disabled: busy || rejectReason.trim() === '' }));
      } else {
        nodes.push(button('Reject...', openReject, { kind: 'ghost', disabled: busy }));
        nodes.push(button('Approve and spawn', approve, { kind: 'primary', disabled: busy }));
      }
    } else if (!last) {
      nodes.push(button('Next', () => go(step + 1), { kind: 'primary', disabled: !loaded || busy || !canAdvance() }));
    } else {
      const label = d.mode === 'new' ? `Create role and spawn ${plural(d.count, 'agent')}` : `Spawn ${plural(d.count, 'agent')}`;
      nodes.push(button(label, spawn, { kind: 'primary', disabled: busy }));
    }
    replace(navHost, nodes);
  }

  function render(): void {
    replace(stepsHost, stepper());
    if (!loaded || !picker) {
      replace(
        bodyHost,
        loadError
          ? h('div.ag-stack', null, h('p', null, `Could not load roles and models: ${loadError}`), button('Retry', () => { loadError = null; render(); void load(); }, { kind: 'ghost' }))
          : h('p.muted', null, 'Loading roles…'),
      );
      replace(navHost);
      return;
    }
    replace(bodyHost, bodyFor(step));
    const after = afterRender;
    afterRender = null;
    after?.();
    renderNav();
  }

  const go = (to: number): void => {
    if (to < 0 || to >= stepNames().length || (to > step && !canAdvance())) return;
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

  /** Deletes a custom role after the inline confirm, then reloads the list. The hub's refusal (a role in use) shows as the toast. */
  const removeRole = async (r: RoleView): Promise<void> => {
    if (busy) return;
    busy = true;
    delConfirm = null;
    render();
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
        await api.spawnAgents(def.id, n, model);
      } else {
        await api.spawnAgents(d.roleId, n, model);
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

  const rejectNow = (): void => {
    if (!proposal) return;
    const id = proposal.id;
    const reason = rejectReason.trim();
    if (!reason) return;
    void finish(() => api.rejectAgentProposal(id, reason), 'Proposal rejected');
  };

  const openReject = (): void => {
    rejecting = true;
    afterRender = () => bodyHost.querySelector<HTMLElement>('textarea[aria-label="Reason"]')?.focus();
    render();
  };

  const keepIt = (): void => {
    rejecting = false;
    rejectReason = '';
    render();
  };

  function stepper(): HTMLElement {
    return h(
      'ol.ag-wiz-steps',
      { 'aria-label': 'Wizard steps' },
      stepNames().map((name, i) => {
        const label = `${i + 1}. ${name}`;
        return h('li', { 'aria-current': i === step ? 'step' : null }, i < step ? button(label, () => go(i), { kind: 'ghost', size: 'sm' }) : label);
      }),
    );
  }

  /** A list of text rows that edit the draft's `key` in place; `Add` and the × remove rows (one row always stays). */
  function listEditor(key: ListKey, label: string, placeholder: string): HTMLElement {
    const items = d[key];
    const list = h('div.ag-stack', { 'data-list': key });
    items.forEach((value, i) => {
      const el = input({ placeholder, 'aria-label': `${label} ${i + 1}` });
      el.value = value;
      el.addEventListener('input', () => {
        items[i] = el.value;
        renderNav();
      });
      const remove = `Remove ${label.toLowerCase()} ${i + 1}`;
      const rm = h('button.btn.ghost.sm.icon-only', { type: 'button', title: remove, 'aria-label': remove }, lu('x', 14));
      rm.addEventListener('click', () => {
        items.splice(i, 1);
        if (!items.length) items.push('');
        render();
      });
      list.appendChild(h('div.cluster', null, el, rm));
    });
    list.appendChild(
      h(
        'div',
        null,
        button('Add', () => {
          items.push('');
          afterRender = () => {
            const boxes = bodyHost.querySelectorAll<HTMLInputElement>(`[data-list="${key}"] input`);
            boxes[boxes.length - 1]?.focus();
          };
          render();
        }, { kind: 'ghost', size: 'sm' }),
      ),
    );
    return list;
  }

  function pick(r: RoleView): void {
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
  }

  function roleCard(r: RoleView): HTMLElement {
    const on = d.roleId === r.id;
    return h(
      'div.ag-choice',
      {
        role: 'radio',
        tabindex: '0',
        'aria-checked': on ? 'true' : 'false',
        onclick: () => pick(r),
        onkeydown: (e: Event) => {
          const key = (e as KeyboardEvent).key;
          if (key === 'Enter' || key === ' ') {
            e.preventDefault();
            pick(r);
          }
        },
      },
      h('div.cluster', null, h('strong', null, r.title), badge(r.source === 'builtin' ? 'built-in' : 'custom'), h('span.mono.muted', null, r.id)),
      h('span.muted', null, clip(r.def.purpose, 180)),
      h('span.muted', null, `${plural(r.agents, 'agent')} run this role`),
      r.source === 'custom'
        ? h(
            'div',
            null,
            button('Delete role', (ev) => {
              ev.stopPropagation();
              delConfirm = r.id;
              render();
            }, { kind: 'ghost', size: 'sm', disabled: busy, title: `Delete ${r.title}` }),
          )
        : null,
    );
  }

  function roleList(): HTMLElement {
    if (!roles.length) return h('p.muted', null, 'The plaza has no roles yet. Define a new one.');
    return h('div.ag-stack', { role: 'radiogroup', 'aria-label': 'Roles' }, roles.map(roleCard));
  }

  /** The inline confirm for deleting a custom role (the prototype's, not a native dialog). */
  function confirmRow(): HTMLElement | null {
    const r = roles.find((x) => x.id === delConfirm);
    if (!r) return null;
    return h(
      'div.ag-row',
      null,
      h('p', null, `Delete the role ${r.title} (${r.id})? Its definition is removed for good.`),
      h(
        'div.cluster',
        null,
        button('Delete role', () => void removeRole(r), { kind: 'danger', size: 'sm', disabled: busy }),
        button('Keep', () => {
          delConfirm = null;
          render();
        }, { kind: 'ghost', size: 'sm' }),
      ),
    );
  }

  function purposeNodes(): Node[] {
    const choice = (mode: Mode, name: string, text: string): HTMLElement =>
      h(
        'button.ag-choice',
        {
          type: 'button',
          'aria-pressed': d.mode === mode ? 'true' : 'false',
          onclick: () => {
            d.mode = mode;
            delConfirm = null;
            render();
          },
        },
        h('strong', null, name),
        h('span.muted', null, text),
      );
    const nodes: Node[] = [
      h('h3', null, 'What should this agent do?'),
      h(
        'div.ag-stack',
        null,
        choice('existing', 'Use an existing role', 'Start agents of a role the plaza already has.'),
        choice('new', 'Define a new role', 'Write the role properly first, then start agents of it.'),
      ),
    ];
    if (d.mode === 'existing') {
      nodes.push(roleList());
      const confirm = confirmRow();
      if (confirm) nodes.push(confirm);
    }
    return nodes;
  }

  function roleForm(): Node[] {
    const idEl = input({ 'aria-label': 'Id', placeholder: 'api-builder', spellcheck: 'false', autocomplete: 'off' });
    idEl.value = d.id;
    idEl.addEventListener('input', () => {
      idTouched = true;
      idEl.value = idEl.value.toLowerCase().replace(/[^a-z0-9-]/g, '');
      d.id = idEl.value;
      renderNav();
    });
    const titleEl = input({ 'aria-label': 'Title', placeholder: 'API builder' });
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
      field('Id', idEl, 'Lowercase letters, digits and hyphens. It follows the title until you edit it by hand.'),
      field('Title', titleEl, 'What this role is called, e.g. "API builder".'),
      field('Purpose', purposeEl, 'Why the role exists, in a sentence or two. Good: "Builds and tests the plaza HTTP API." Vague: "helps with the project."'),
      field(
        'Responsibilities',
        listEditor('responsibilities', 'Responsibilities', 'Write and test the API server'),
        'Concrete, checkable duties: "Write and test the API server", not "help with code". At least one.',
      ),
      field('Deliverables', listEditor('deliverables', 'Deliverables', 'A merged change with tests'), 'What the agent hands over when it is done. At least one.'),
      field('Working with others', collabEl, 'Who this role takes work from, and who reviews what it produces.'),
      field('Boundaries', listEditor('boundaries', 'Boundaries', 'Does not merge to main'), 'What it must not do. Optional, but a clear edge keeps roles from overlapping.'),
    ];
  }

  function toolsBlock(): HTMLElement {
    return h(
      'fieldset',
      null,
      h('legend', null, 'Extra tools'),
      h('p.muted', null, "Every agent already gets its workspace tools, the board, messages, memory and the toolbox; the plaza's laws decide the rest."),
      TOOL_GROUPS.map((g) =>
        h(
          'div',
          null,
          h('div.muted', null, g.group),
          g.tools.map((tool) => {
            const cb = h('input', { type: 'checkbox' });
            cb.checked = d.tools.includes(tool);
            cb.addEventListener('change', () => {
              d.tools = cb.checked ? [...d.tools, tool] : d.tools.filter((t) => t !== tool);
            });
            return h('label.cluster', null, cb, tool);
          }),
        ),
      ),
    );
  }

  function budgetBlock(): HTMLElement {
    const el = input({ type: 'number', min: MIN_BUDGET, max: MAX_BUDGET, step: 100, 'aria-label': 'Task budget' });
    el.value = String(d.budget);
    el.addEventListener('input', () => {
      d.budget = Number(el.value);
      renderNav();
    });
    return field('Task budget', el, 'The most one task may spend: 200 to 50,000 (default 2,000).');
  }

  function countSelect(): HTMLSelectElement {
    const sel = select(
      Array.from({ length: MAX_AGENTS }, (_, i) => ({ value: String(i + 1), label: String(i + 1) })),
      String(d.count),
    );
    sel.setAttribute('aria-label', 'Agents');
    sel.addEventListener('change', () => {
      d.count = Number(sel.value);
      renderNav();
    });
    return sel;
  }

  function countBlock(): HTMLElement {
    return field('Agents', countSelect(), 'How many agents of this role to start (1 to 8).');
  }

  function modelBlock(): HTMLElement {
    const effort = select(EFFORT_OPTS, d.effort);
    effort.setAttribute('aria-label', 'Effort');
    effort.addEventListener('change', () => {
      d.effort = effort.value;
    });
    return field(
      'Model and effort',
      h('div.cluster', null, picker?.el ?? h('span.muted', null, 'No models available'), effort),
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
      const ta = textarea({ 'aria-label': 'Role prompt', rows: 18 });
      ta.value = currentPrompt(d);
      ta.addEventListener('input', () => {
        d.promptEdited = ta.value;
        renderNav();
      });
      return [
        h(
          'div.cluster',
          null,
          h('span.muted', null, 'Markdown. This text is saved as the role prompt.'),
          button('Done', () => {
            editing = false;
            render();
          }, { kind: 'ghost', size: 'sm' }),
          reset,
        ),
        ta,
      ];
    }
    return [
      h(
        'div.cluster',
        null,
        h('span.muted', null, edited ? 'Edited by hand.' : 'Generated from the fields in steps 2 and 3.'),
        button('Edit', () => {
          editing = true;
          afterRender = () => bodyHost.querySelector<HTMLElement>('textarea[aria-label="Role prompt"]')?.focus();
          render();
        }, { kind: 'ghost', size: 'sm' }),
        reset,
      ),
      h('div.prose', null, renderMarkdown(currentPrompt(d))),
    ];
  }

  function previewRole(def: RoleDef | undefined): Node[] {
    if (!def) return [h('p.muted', null, 'No role selected. Go back and choose one.')];
    return [
      h('h3', null, def.title),
      h('p.muted', null, `${def.id}, an existing role`),
      ro('Purpose', def.purpose || '—'),
      ro('Responsibilities', listNode(def.responsibilities)),
      ro('Deliverables', listNode(def.deliverables)),
      ro('Working with others', def.collaboration || '—'),
      ro('Boundaries', listNode(def.boundaries)),
    ];
  }

  function reviewNodes(): Node[] {
    const def = d.mode === 'existing' ? existingDef() : undefined;
    const effort = d.effort ? ` · ${d.effort} effort` : '';
    const rows: [string, string | Node][] = [];
    if (d.mode === 'existing') {
      rows.push(['Role', `${def?.title ?? d.roleId} (${d.roleId}), existing`]);
    } else {
      rows.push(
        ['New role', `${d.title.trim()} (${d.id})`],
        ['Purpose', d.purpose.trim()],
        ['Responsibilities', nonEmpty(d.responsibilities).join('; ')],
        ['Deliverables', nonEmpty(d.deliverables).join('; ')],
        ['Working with others', d.collaboration.trim()],
        ['Boundaries', nonEmpty(d.boundaries).join('; ') || '—'],
        ['Tools', d.tools.join(', ') || 'Workspace tools only'],
        ['Budget', String(d.budget)],
      );
    }
    rows.push(['Agents', plural(d.count, 'agent')], ['Model', `${modelLabelOf()}${effort}`]);
    return [
      h('h3', null, 'Review and spawn'),
      h(
        'dl.ag-kv',
        null,
        rows.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)]),
      ),
    ];
  }

  /** The proposal's steps: the proposal and its reason, the role it defines (with the count and model), then the decision. */
  function proposalBody(i: number): Node[] {
    if (!proposal) return [];
    const role = proposal.role;
    const rid = role.id ?? '';
    const title = role.title || rid || 'Role';
    const ex = roles.find((r) => r.id === rid);
    switch (i) {
      case 0:
        return [
          h('h3', null, 'The orchestrator proposes'),
          h('p', null, proposal.reason || '—'),
          h('h4', null, "The owner's instruction"),
          proposal.instruction ? h('blockquote.prose', null, proposal.instruction) : h('p.muted', null, 'No instruction was quoted with this proposal.'),
        ];
      case 1:
        return [
          h('h3', null, title),
          h('p.muted', null, `${rid}, ${ex ? 'an existing role' : 'a new role'}`),
          ex
            ? h('p.hint', null, `${plural(ex.agents, 'agent')} already run this role. The proposal adds agents of it.`)
            : h('p.hint', null, 'Approving it writes the role into the plaza with this definition.'),
          ro('Purpose', role.purpose || '—'),
          ro('Responsibilities', listNode(role.responsibilities)),
          ro('Deliverables', listNode(role.deliverables)),
          ro('Working with others', role.collaboration || '—'),
          ro('Boundaries', listNode(role.boundaries)),
          field('Agents', countSelect()),
          field('Model', picker?.el ?? h('span.muted', null, 'No models available')),
        ];
      default:
        return [
          h('h3', null, 'Approve it, or reject it with a reason'),
          h('p.muted', null, `Approving starts ${plural(d.count, 'agent')} of ${title} on ${modelLabelOf()}.`),
          ...(rejecting ? [rejectPanel()] : []),
        ];
    }
  }

  function rejectPanel(): HTMLElement {
    const ta = textarea({ 'aria-label': 'Reason', rows: 3, placeholder: 'Why the proposal is rejected' });
    ta.value = rejectReason;
    ta.addEventListener('input', () => {
      rejectReason = ta.value;
      renderNav();
    });
    return h('div.ag-stack', null, h('h4', null, 'Reject it with a reason'), ta);
  }

  function bodyFor(i: number): Node[] {
    if (proposal) return proposalBody(i);
    const existing = d.mode === 'existing';
    const def = existing ? existingDef() : undefined;
    switch (i) {
      case 0:
        return purposeNodes();
      case 1:
        return existing ? previewRole(def) : roleForm();
      case 2:
        if (existing) {
          return [
            h('h3', null, 'Tools & limits'),
            h('p.muted', null, "This role's tools and budget are fixed by its definition; choose how many agents to start and on which model."),
            ro('Tools', def && def.tools.length ? def.tools.join(', ') : 'Workspace tools only'),
            ro('Budget', def ? String(def.budget) : '—'),
            countBlock(),
            modelBlock(),
          ];
        }
        return [h('h3', null, 'Tools & limits'), toolsBlock(), budgetBlock(), countBlock(), modelBlock()];
      case 3:
        if (existing) return [h('h3', null, 'Prompt'), def ? h('div.prose', null, renderMarkdown(def.prompt || promptFor(def))) : h('p.muted', null, '—')];
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
  // The modal's sheet has no footer of its own: the navigation goes in one under the body, as the prototype's does.
  (root.closest('.sheet') ?? root).appendChild(h('footer.sheet-f', null, navHost));
  render();
  void load();
}
