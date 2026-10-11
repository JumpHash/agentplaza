import { agentName } from '../roles';
// Tasks: the board of every task, and one task in full. A task is open, in
// progress, done, or failed or withdrawn; the page shows only that.

import '../styles/tasks.css';
import { api, type Agent, type Task } from '../api';
import { go, href } from '../router';
import { agents, on } from '../store';
import { ago, attempt, badge, button, card, empty, field, input, modal, select, textarea, type Tone } from '../ui/components';
import { h, icon, replace } from '../ui/dom';
import type { Cleanup } from './index';

const COLUMN_LABELS = ['Open', 'In progress', 'Done', 'Failed or withdrawn'];
const FAILED = new Set(['failed', 'withdrawn', 'cancelled']);

/** The board column for a state; unknown non-final states count as in progress. */
function columnOf(state: string): number {
  if (state === 'open') return 0;
  if (state === 'done') return 2;
  if (FAILED.has(state)) return 3;
  return 1;
}

function stateTone(state: string): Tone {
  switch (columnOf(state)) {
    case 0:
      return 'info';
    case 1:
      return 'accent';
    case 2:
      return 'ok';
    default:
      return state === 'withdrawn' ? 'muted' : 'danger';
  }
}

const titleOf = (goal: string): string => goal.split('\n').find((l) => l.trim())?.trim() ?? 'Untitled task';
const labelOf = (state: string): string => state.replace(/_/g, ' ').replace(/^./, (c) => c.toUpperCase());
const cidName = (cid: number): string => {
  const a = agents.get().find((x) => x.cid === cid);
  return a ? agentName(a) : `agent ${cid}`;
};
const idleWorkers = (): Agent[] => agents.get().filter((a) => a.status === 'idle' && a.tier === 'worker');

function assigneeLink(cid: number | null): HTMLElement {
  if (cid === null) return h('span.faint', null, 'Unassigned');
  const link = h('a', { href: href('agents', cid) }, cidName(cid));
  // Inside a clickable card, the name should open the agent, not the task.
  link.addEventListener('click', (e) => e.stopPropagation());
  return link;
}

function taskCard(t: Task): HTMLElement {
  const el = h(
    'div.task-card',
    { role: 'link', tabindex: 0 },
    h('div.task-top', null, h('span.task-id', null, t.id), badge(t.role ?? 'Any', 'muted')),
    h('strong.task-title', null, titleOf(t.goal)),
    h('div.task-meta', null, assigneeLink(t.agent_cid), h('span.faint', null, `${t.budget}`), h('span.faint', null, ago(t.updated_at))),
  );
  const open = (): void => go('tasks', t.id);
  el.addEventListener('click', open);
  el.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') open();
  });
  return el;
}

function board(tasks: Task[]): HTMLElement {
  const sorted = tasks.slice().sort((a, b) => b.updated_at - a.updated_at);
  return h(
    'div.task-board',
    null,
    COLUMN_LABELS.map((label, i) => {
      const items = sorted.filter((t) => columnOf(t.state) === i);
      return h(
        'section.task-col',
        null,
        h('header.task-col-head', null, h('h2', null, label), h('span.faint', null, String(items.length))),
        h('div.task-col-body', null, items.map(taskCard)),
      );
    }),
  );
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const id = params[0] ?? '';
  let tasks: Task[] = [];
  let current: Task | null = null;
  let missing = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let seq = 0;
  let dismiss: (() => void) | undefined;

  const render = (): void => {
    replace(root, id ? detailPage() : listPage());
  };

  const load = async (): Promise<void> => {
    const n = ++seq;
    try {
      if (id) {
        const t = await api.task(id);
        if (n === seq) {
          current = t;
          missing = false;
        }
      } else {
        const list = await api.tasks();
        if (n === seq) tasks = list;
      }
    } catch {
      if (n === seq && id) {
        current = null;
        missing = true;
      }
    }
    render();
  };

  const schedule = (): void => {
    clearTimeout(timer);
    timer = setTimeout(() => void load(), 300);
  };

  const openCreate = (): void => {
    const title = input({ required: true, placeholder: 'What should get done?' });
    const details = textarea({ rows: 4 });
    const acceptance = textarea({ rows: 3 });
    const roles = Array.from(new Set(agents.get().filter((a) => a.tier === 'worker').map((a) => a.role).filter(Boolean))).sort();
    const role = select([{ value: '', label: 'Any' }, ...roles.map((r) => ({ value: r, label: r }))]);
    const budget = input({ type: 'number', min: 0, step: 1, value: 100 });
    const assignee = select([
      { value: '', label: 'Let the scheduler choose' },
      ...idleWorkers().map((a) => ({ value: String(a.cid), label: agentName(a) })),
    ]);
    const submit = h('button.btn.primary', { type: 'submit' }, icon('plus'), 'Create task');
    const form = h(
      'form.stack',
      null,
      field('Title', title),
      field('Details', details),
      field('Acceptance', acceptance),
      h('div.grid.cols-2', null, field('Role', role), field('Budget', budget)),
      field('Assign to', assignee),
      h('div.row', null, h('span', { style: 'flex:1' }), submit),
    );
    form.addEventListener('submit', async (e) => {
      e.preventDefault();
      const goalTitle = title.value.trim();
      if (!goalTitle) {
        title.focus();
        return;
      }
      const detail = details.value.trim();
      submit.disabled = true;
      const created = await attempt(
        () =>
          api.createTask({
            goal: detail ? `${goalTitle}\n\n${detail}` : goalTitle,
            acceptance: acceptance.value.trim() || undefined,
            role: role.value || undefined,
            budget: budget.value === '' ? 100 : Number(budget.value),
            assignee: assignee.value ? Number(assignee.value) : undefined,
          }),
        'Task created',
      );
      submit.disabled = false;
      if (created) {
        close();
        go('tasks', created.id);
      }
    });
    const close = modal('New task', form, () => {
      dismiss = undefined;
    });
    dismiss = close;
  };

  function listPage(): HTMLElement {
    return h(
      'div.tasks-view',
      null,
      h('div.view-head', null, h('h1', null, 'Tasks'), button('New task', openCreate, { kind: 'primary', icon: 'plus' })),
      board(tasks),
    );
  }

  function detailPage(): HTMLElement {
    const back = h('a.back', { href: href('tasks') }, icon('chevronRight'), 'Tasks');
    if (missing) return h('div.tasks-view', null, back, empty('alert', 'Task not found'));
    const t = current;
    if (!t) return h('div.tasks-view', null, back);

    const isOpen = t.state === 'open';
    const assignSel = select(
      [{ value: '', label: 'Assign to…' }, ...idleWorkers().map((a) => ({ value: String(a.cid), label: agentName(a) }))],
      '',
    );
    assignSel.addEventListener('change', async () => {
      if (!assignSel.value) return;
      assignSel.disabled = true;
      await attempt(() => api.assign(Number(assignSel.value), t.id), 'Assigned');
      assignSel.disabled = false;
      void load();
    });
    const withdrawBtn: HTMLButtonElement = button(
      'Withdraw',
      async () => {
        withdrawBtn.disabled = true;
        const done = await attempt(() => api.withdrawTask(t.id), 'Task withdrawn');
        if (done) void load();
        else withdrawBtn.disabled = false;
      },
      { kind: 'danger', icon: 'x' },
    );

    const goalCard = card(
      'Goal',
      h(
        'div.stack',
        null,
        h('div.pre', null, t.goal),
        t.acceptance ? h('div.stack', null, h('h3', null, 'Acceptance'), h('div.pre', null, t.acceptance)) : null,
      ),
    );
    const detailsCard = card(
      'Details',
      h(
        'div.stack',
        null,
        h(
          'dl.kv',
          null,
          h('dt', null, 'State'),
          h('dd', null, badge(labelOf(t.state), stateTone(t.state), true)),
          h('dt', null, 'Role'),
          h('dd', null, t.role ?? 'Any'),
          h('dt', null, 'Budget'),
          h('dd', null, String(t.budget)),
          h('dt', null, 'Assignee'),
          h('dd', null, assigneeLink(t.agent_cid)),
          h('dt', null, 'Created'),
          h('dd', null, ago(t.created_at)),
          h('dt', null, 'Updated'),
          h('dd', null, ago(t.updated_at)),
        ),
        isOpen ? assignSel : null,
        isOpen ? h('div.row', null, h('span', { style: 'flex:1' }), withdrawBtn) : null,
      ),
    );
    const resultCard = t.note ? card('Result', h('div.pre', null, t.note)) : null;

    return h(
      'div.tasks-view',
      null,
      back,
      h('div.view-head', null, h('h1', null, titleOf(t.goal))),
      h('div.grid.cols-2.task-detail', null, h('div.stack', null, goalCard, resultCard), detailsCard),
    );
  }

  render();
  const offs = [on('task', schedule), on('work', schedule), agents.watch(() => render())];
  void load();

  return () => {
    clearTimeout(timer);
    offs.forEach((off) => off());
    dismiss?.();
  };
}
