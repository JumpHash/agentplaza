// A project's Settings tab: every setting its agents get, inherited from the plaza or overridden here, with Reset to
// plaza; and the project's budget. The instructions and skills are the prompt and skills bound to `project:<id>`.

import { api, type EffectiveSettings, type EffectiveValue, type Project } from '../api';
import { href } from '../router';
import { isLive } from '../scope';
import { attempt, badge, button, card, field, input, modal, select, toast } from '../ui/components';
import { h, replace } from '../ui/dom';

/** One setting: its label, what it does, and how it is entered. */
interface SettingSpec {
  key: string;
  label: string;
  hint: string;
  kind: 'model' | 'number' | 'choice';
  choices?: string[];
}

const SPECS: SettingSpec[] = [
  { key: 'default_model', label: 'Model', hint: "The project's agents' model, for a role with none of its own here", kind: 'model' },
  { key: 'effort', label: 'Effort', hint: 'The reasoning effort of their model calls', kind: 'choice', choices: ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'] },
  { key: 'max_tokens', label: 'Longest answer', hint: 'Tokens per model call (0 at the plaza: the provider decides)', kind: 'number' },
  { key: 'max_turns', label: 'Turns per task', hint: 'How many model turns a work item may take (1 to 500)', kind: 'number' },
  { key: 'compress_at', label: 'Compress at', hint: 'The conversation size, in bytes, at which it is compressed (0: never)', kind: 'number' },
  { key: 'keep_recent', label: 'Turns kept', hint: 'How many recent turns a compression keeps (1 to 50)', kind: 'number' },
  { key: 'task_budget', label: 'Task budget', hint: 'Credits a task gets when it names none', kind: 'number' },
  { key: 'permission_mode', label: 'Permission mode', hint: 'For agents with no mode of their own', kind: 'choice', choices: ['autonomous', 'ask', 'read_only'] },
  { key: 'review_timeout_minutes', label: 'Review timeout', hint: 'Minutes a finished task waits for a reviewer before you are asked', kind: 'number' },
];

/** A value as the table shows it. */
function shown(v: EffectiveValue | undefined): string {
  const value = v?.value;
  if (value === null || value === undefined || value === '') return '—';
  return String(value);
}

function sourceBadge(v: EffectiveValue | undefined): HTMLElement {
  return v?.source === 'project' ? badge('Overridden', 'accent') : badge('Inherited from the plaza', 'muted');
}

/** Asks for a new value of `spec` and saves it as the project's. */
function openEdit(p: Project, spec: SettingSpec, current: unknown, done: () => void, roleKey?: string): void {
  const control =
    spec.kind === 'choice'
      ? select((spec.choices ?? []).map((c) => ({ value: c, label: c })), String(current ?? spec.choices?.[0] ?? ''))
      : input({ type: spec.kind === 'number' ? 'number' : 'text', value: current === null || current === undefined ? '' : String(current), placeholder: spec.kind === 'model' ? 'provider/model, e.g. anthropic/claude-sonnet-5-5' : '' });
  const save = button(
    'Save',
    () => {
      const raw = control.value.trim();
      if (!raw) {
        toast('Give a value, or reset it to the plaza’s', 'error');
        return;
      }
      const value: unknown = spec.kind === 'number' ? Number(raw) : raw;
      const change: Record<string, unknown> = roleKey ? { role_models: { [roleKey]: value } } : { [spec.key]: value };
      void (async () => {
        const ok = await attempt(() => api.patchProjectSettings(p.id, change), `${spec.label} saved`);
        if (ok) {
          close();
          done();
        }
      })();
    },
    { kind: 'primary' },
  );
  const close = modal(roleKey ? `${roleKey}'s model` : spec.label, h('div.pj-form', null, field(spec.label, control, spec.hint), h('div.pj-form-actions', null, save)));
}

/** Asks for credits to set aside for the project (or, `back`, to give back to the plaza). */
function openBudget(p: Project, back: boolean, done: () => void): void {
  const amount = input({ type: 'number', min: 1, step: 1, placeholder: '5000' });
  const go = button(
    back ? 'Give back' : 'Set aside',
    () => {
      const n = Math.trunc(Number(amount.value));
      if (!(n > 0)) {
        toast('A whole number of credits, more than 0', 'error');
        return;
      }
      void (async () => {
        const ok = await attempt(() => api.moveProjectBudget(p.id, back ? -n : n), back ? `${n} credits back to the plaza` : `${n} credits set aside`);
        if (ok) {
          close();
          done();
        }
      })();
    },
    { kind: 'primary' },
  );
  const close = modal(
    back ? 'Give credits back to the plaza' : 'Set credits aside',
    h(
      'div.pj-form',
      null,
      h('p.faint', null, back ? 'They go back to the plaza’s budget.' : 'They move from the plaza’s budget to this project’s: its tasks are funded from it.'),
      field('Credits', amount),
      h('div.pj-form-actions', null, go),
    ),
  );
}

/** The Settings tab of project `p`. */
export function settingsTab(p: Project): HTMLElement {
  const root = h('div.pj-sections');
  const live = isLive(p);
  const draw = async (): Promise<void> => {
    const view: EffectiveSettings | undefined = await attempt(() => api.effectiveSettings(p.id));
    if (!view) return;
    const reset = (change: Record<string, unknown>, what: string): void =>
      void attempt(() => api.patchProjectSettings(p.id, change), `${what} is the plaza’s again`).then(() => void draw());
    const rows = SPECS.map((spec) => {
      const v = view.settings[spec.key];
      return h(
        'tr',
        null,
        h('td', null, h('strong', null, spec.label), h('div.faint', null, spec.hint)),
        h('td.mono', null, shown(v)),
        h('td', null, sourceBadge(v)),
        h(
          'td.pj-cell-actions',
          null,
          live ? button('Change', () => openEdit(p, spec, v?.value, () => void draw()), { size: 'sm', kind: 'ghost' }) : null,
          live && v?.source === 'project' ? button('Reset to plaza', () => reset({ [spec.key]: null }, spec.label), { size: 'sm', kind: 'ghost' }) : null,
        ),
      );
    });
    const modelSpec = SPECS[0]!;
    const roleRows = Object.entries(view.role_models).map(([role, v]) =>
      h(
        'tr',
        null,
        h('td', null, h('strong', null, role)),
        h('td.mono', null, shown(v)),
        h('td', null, sourceBadge(v)),
        h(
          'td.pj-cell-actions',
          null,
          live ? button('Change', () => openEdit(p, modelSpec, v.value, () => void draw(), role), { size: 'sm', kind: 'ghost' }) : null,
          live && v.source === 'project' ? button('Reset to plaza', () => reset({ role_models: { [role]: null } }, `${role}'s model`), { size: 'sm', kind: 'ghost' }) : null,
        ),
      ),
    );
    const newRole = (): void => {
      const role = input({ placeholder: 'builder' });
      const pick = button(
        'Next',
        () => {
          const r = role.value.trim();
          if (!r) return;
          close();
          openEdit(p, modelSpec, '', () => void draw(), r);
        },
        { kind: 'primary' },
      );
      const close = modal('A role’s model', h('div.pj-form', null, field('Role', role, 'Its agents on this project run the model you pick next'), h('div.pj-form-actions', null, pick)));
    };
    const b = view.budget;
    replace(
      root,
      card(
        'Settings',
        h('div.table-scroll', null, h('table.table.pj-settings', null, h('thead', null, h('tr', null, h('th', null, 'Setting'), h('th', null, 'Value'), h('th', null, 'From'), h('th', null, ''))), h('tbody', null, rows))),
      ),
      card(
        'Models by role',
        roleRows.length
          ? h('div.table-scroll', null, h('table.table.pj-settings', null, h('tbody', null, roleRows)))
          : h('p.pj-none', null, 'No role has a model of its own here or at the plaza: every role runs the model above.'),
        { actions: live ? [button('Set a role’s model', newRole, { size: 'sm', icon: 'plus' })] : [] },
      ),
      card(
        'Budget',
        h(
          'div.stack',
          null,
          b.own
            ? h('p', null, h('strong', null, `${b.balance.toLocaleString()} credits`), ' left in this project’s budget. Its tasks are funded from it, and when it is over the rest goes back to the plaza.')
            : h('p', null, 'This project has no budget of its own yet: its tasks are funded from the plaza’s.'),
          h('p.faint', null, `The plaza’s budget: ${b.plaza.toLocaleString()} credits.`),
        ),
        {
          actions: live
            ? [
                button('Set credits aside…', () => openBudget(p, false, () => void draw()), { size: 'sm', icon: 'plus' }),
                ...(b.own && b.balance > 0 ? [button('Give back…', () => openBudget(p, true, () => void draw()), { size: 'sm', kind: 'ghost' })] : []),
              ]
            : [],
        },
      ),
      card(
        'Instructions and skills',
        h(
          'p',
          null,
          'An AGENTS.md prompt bound to ',
          h('code', null, `project:${p.id}`),
          ' is added to this project’s agents’ instructions, and skills bound to it are theirs too. Bind them in ',
          h('a', { href: href('prompts') }, 'Prompts'),
          ' and ',
          h('a', { href: href('skills') }, 'Skills'),
          '.',
        ),
      ),
    );
  };
  void draw();
  return root;
}
