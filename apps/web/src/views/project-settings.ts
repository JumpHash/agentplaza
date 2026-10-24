// A project's Settings tab: every setting its agents get, inherited from the plaza or overridden here, with Reset to
// plaza. The instructions and skills are the prompt and skills bound to `project:<id>`. The
// markup is the prototype's Settings tab (styles/proto/projects.css).

import { api, type EffectiveSettings, type EffectiveValue, type Project } from '../api';
import { href } from '../router';
import { isLive } from '../scope';
import { attempt, badge, button, card, field, input, modal, select, toast } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import type { LucideName } from '../ui/icons.gen';
import { budgetCard, type LiveCard } from './budget';

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
  { key: 'task_budget', label: 'Task budget', hint: "A task's budget when it names none, in the project's unit", kind: 'number' },
  { key: 'permission_mode', label: 'Permission mode', hint: 'For agents with no mode of their own', kind: 'choice', choices: ['autonomous', 'ask', 'read_only'] },
  { key: 'review_timeout_minutes', label: 'Review timeout', hint: 'Minutes a finished task waits for a reviewer before you are asked', kind: 'number' },
];

/** A dialog as the prototype draws it: the body, and the actions in the sheet's own footer. */
function dialog(title: string, body: (Node | null)[], foot: (close: () => void) => Node[]): () => void {
  const close = modal(title, h('div.pj-dlg', null, body));
  const scrims = document.querySelectorAll<HTMLElement>('.scrim');
  scrims[scrims.length - 1]?.querySelector('.sheet')?.append(h('footer.sheet-f', null, foot(close)));
  return close;
}

/** A button with a lucide icon (button() takes the console's own icon names only). */
function lbtn(label: string, onClick: (ev: MouseEvent) => void, o: { kind?: 'primary' | 'ghost'; size?: 'sm'; icon?: LucideName } = {}): HTMLButtonElement {
  const b = button(label, onClick, { kind: o.kind, size: o.size });
  if (o.icon) b.insertBefore(lu(o.icon, o.size === 'sm' ? 14 : 15), b.firstChild);
  return b;
}

/** A table as the prototype has it: an optional head, then the rows. */
function table(head: string[] | null, rows: HTMLElement[]): HTMLElement {
  return h(
    'div.tbl-wrap',
    null,
    h(
      'table.tbl',
      null,
      head ? h('thead', null, h('tr', null, head.map((x) => h('th', null, x)))) : null,
      h('tbody', null, rows),
    ),
  );
}

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
  const hint = spec.hint;
  dialog(roleKey ? `${roleKey}’s model` : spec.label, [field(roleKey ? 'Model' : spec.label, control, hint)], (close) => [
    lbtn(
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
    ),
  ]);
}

/** The budget card kept for one project: the Settings tab redraws reuse it while the project stays the same. */
let budget: { id: string; card: LiveCard } | null = null;

/**
 * The budget card of project `id`. A redraw of the tab, or of the page, reuses the card kept for that id while it is still
 * on the page. A card whose tab has gone stops itself (see refreshOn), so it is rebuilt on the next visit; an old card for
 * another id is disposed here.
 */
function budgetCardOf(id: string): HTMLElement {
  if (budget?.id === id && budget.card.el.isConnected) return budget.card.el;
  budget?.card.dispose();
  budget = { id, card: budgetCard({ kind: 'project', id }) };
  return budget.card.el;
}

/** The Settings tab of project `p`. */
export function settingsTab(p: Project): HTMLElement {
  const root = h('div.pj-tab');
  // The budget card moves into this tab at once: the page then attaches it, and no event finds it detached while the tab loads.
  const budgetEl = budgetCardOf(p.id);
  root.append(budgetEl);
  const live = isLive(p);
  const draw = async (): Promise<void> => {
    const view: EffectiveSettings | undefined = await attempt(() => api.effectiveSettings(p.id));
    // A tab that a newer redraw has replaced draws nothing: the budget card now belongs to that tab.
    if (!view || !root.isConnected) return;
    const reset = (change: Record<string, unknown>, what: string): void =>
      void attempt(() => api.patchProjectSettings(p.id, change), `${what} is the plaza’s again`).then(() => void draw());
    const actions = (edit: () => void, undo: (() => void) | null): HTMLElement | null =>
      live ? h('div.pj-acts', null, lbtn('Change', edit, { size: 'sm', kind: 'ghost' }), undo ? lbtn('Reset to plaza', undo, { size: 'sm', kind: 'ghost' }) : null) : null;
    const rows = SPECS.map((spec) => {
      const v = view.settings[spec.key];
      return h(
        'tr',
        null,
        h('td', null, h('strong', null, spec.label), h('div.pj-hint', null, spec.hint)),
        h('td', null, h('span.mono', null, shown(v))),
        h('td', null, sourceBadge(v)),
        h('td.pj-cell-actions', null, actions(() => openEdit(p, spec, v?.value, () => void draw()), v?.source === 'project' ? () => reset({ [spec.key]: null }, spec.label) : null)),
      );
    });
    const modelSpec = SPECS[0]!;
    const roleRows = Object.entries(view.role_models).map(([role, v]) =>
      h(
        'tr',
        null,
        h('td', null, h('strong', null, role)),
        h('td', null, h('span.mono', null, shown(v))),
        h('td', null, sourceBadge(v)),
        h('td.pj-cell-actions', null, actions(() => openEdit(p, modelSpec, v.value, () => void draw(), role), v.source === 'project' ? () => reset({ role_models: { [role]: null } }, `${role}'s model`) : null)),
      ),
    );
    const newRole = (): void => {
      const role = input({ placeholder: 'builder' });
      dialog('A role’s model', [field('Role', role, 'Its agents on this project run the model you pick next')], (close) => [
        lbtn(
          'Next',
          () => {
            const r = role.value.trim();
            if (!r) return;
            close();
            openEdit(p, modelSpec, '', () => void draw(), r);
          },
          { kind: 'primary' },
        ),
      ]);
    };
    replace(
      root,
      card('Settings', table(['Setting', 'Value', 'From', ''], rows)),
      card(
        'Models by role',
        roleRows.length ? table(null, roleRows) : h('p.muted', null, 'No role has a model of its own here or at the plaza: every role runs the model above.'),
        { actions: live ? [lbtn('Set a role’s model', newRole, { size: 'sm', icon: 'plus' })] : [] },
      ),
      budgetEl,
      card(
        'Instructions and skills',
        h(
          'p',
          null,
          'An AGENTS.md prompt bound to ',
          h('span.mono', null, `project:${p.id}`),
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
