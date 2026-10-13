import { agentName, roleLabel } from '../roles';
// Tasks: the board of every task (four lanes of live cards) and one task in
// full (goal, result, facts, and a timeline from the assignee's transcript).
// A task is open, in progress, done, or failed or withdrawn; the page shows only that.

import '../styles/tasks.css';
import { api, type Agent, type Entry, type Task } from '../api';
import { go, href } from '../router';
import { agents, on } from '../store';
import { ago, attempt, badge, button, clip, field, input, modal, select, textarea, type Tone } from '../ui/components';
import { h, icon, replace } from '../ui/dom';
import type { Cleanup } from './index';

// The task budget's default and minimum: the hub's apz_plaza::tasks constants (DEFAULT_TASK_BUDGET,
// MIN_TASK_BUDGET). The web console cannot import Rust, so these mirror them; keep them in step.
const DEFAULT_TASK_BUDGET = 2000;
const MIN_TASK_BUDGET = 200;

const LANES = [
  { label: 'Open', idle: 'Nothing open.' },
  { label: 'In progress', idle: 'Nothing in progress.' },
  { label: 'Done', idle: 'Nothing finished yet.' },
  { label: 'Failed or withdrawn', idle: 'Nothing failed or withdrawn.' },
];
const FAILED = new Set(['failed', 'withdrawn', 'cancelled']);
const TIMELINE_KINDS = new Set(['work_start', 'tool_request', 'work_end', 'error']);

/** The lane for a state; unknown non-final states count as in progress. */
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

const str = (v: unknown): string => (typeof v === 'string' || typeof v === 'number' ? String(v) : '');
const titleOf = (goal: string): string => goal.split('\n').find((l) => l.trim())?.trim() ?? 'Untitled task';
const labelOf = (state: string): string => state.replace(/_/g, ' ').replace(/^./, (c) => c.toUpperCase());
const cidName = (cid: number): string => {
  const a = agents.get().find((x) => x.cid === cid);
  return a ? agentName(a) : `agent ${cid}`;
};
const idleWorkers = (): Agent[] => agents.get().filter((a) => a.status === 'idle' && a.tier === 'worker');
const statusOf = (cid: number | null): string => agents.get().find((a) => a.cid === cid)?.status ?? 'offline';
const pad = (n: number): string => String(n).padStart(2, '0');

/** mm:ss, the elapsed time of work that is running. */
const fmtElapsed = (ms: number): string => {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${pad(Math.floor(s / 60))}:${pad(s % 60)}`;
};

/** A span such as "4m 12s", the length of a finished task. */
const fmtSpan = (ms: number): string => {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${pad(s % 60)}s`;
  return `${Math.floor(s / 3600)}h ${pad(Math.floor((s % 3600) / 60))}m`;
};

const clock = (ms: number): string => new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });

const toolOf = (e: Entry): string => str(e.data.tool) || str(e.data.name) || 'tool';

/** The shell command of a bash tool request, or null for any other tool. */
function commandOf(e: Entry): string | null {
  const inp = (e.data.input ?? {}) as Record<string, unknown>;
  return toolOf(e).toLowerCase() === 'bash' && typeof inp.command === 'string' ? inp.command : null;
}

/** What one transcript entry says the agent is doing now, or null when it says nothing new. */
function activityOf(e: Entry): string | null {
  switch (e.kind) {
    case 'tool_request': {
      const cmd = commandOf(e);
      return cmd !== null ? `$ ${cmd}` : `→ ${toolOf(e)}`;
    }
    case 'model_request':
      return 'thinking…';
    case 'work_start':
      return 'starting…';
    case 'work_end':
      return 'wrapping up…';
    default:
      return null;
  }
}

/** One timeline line for a transcript entry. */
function describe(e: Entry): string {
  switch (e.kind) {
    case 'work_start':
      return `Started · ${str(e.data.kind) || 'work'}`;
    case 'tool_request': {
      const cmd = commandOf(e);
      return cmd !== null ? `$ ${cmd}` : `→ ${toolOf(e)}`;
    }
    case 'work_end': {
      const status = str(e.data.status);
      return status ? `Finished · ${status}` : 'Finished';
    }
    default:
      return str(e.data.message) || str(e.data.error) || 'Error';
  }
}

/** The entries of one task's work: the work_start entries that name the task, and the steps of their work. */
function timelineOf(taskId: string, entries: Entry[]): Entry[] {
  const works = new Set(
    entries
      .filter((e) => e.kind === 'work_start' && (((e.data.origin ?? {}) as { task?: unknown }).task === taskId || e.data.task === taskId))
      .map((e) => e.work),
  );
  return entries.filter((e) => works.has(e.work) && TIMELINE_KINDS.has(e.kind));
}

function assigneeLink(cid: number | null): HTMLElement {
  if (cid === null) return h('span.faint', null, 'Unassigned');
  const link = h('a', { href: href('agents', cid) }, cidName(cid));
  // Inside a clickable card, the name should open the agent, not the task.
  link.addEventListener('click', (e) => e.stopPropagation());
  return link;
}

const timelineRow = (e: Entry): HTMLElement => h('li', { class: `k-${e.kind}` }, h('time', null, clock(e.at_ms)), h('span', null, describe(e)));

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const id = params[0] ?? '';
  let tasks: Task[] = [];
  let current: Task | null = null;
  let timeline: Entry[] | null = null; // null while the assignee's transcript loads
  let missing = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let seq = 0;
  let dismiss: (() => void) | undefined;

  const activity = new Map<number, string>(); // assignee cid -> its latest line
  const lastState = new Map<string, string>(); // task id -> the state it was last drawn in
  const seen = new Set<string>(); // task ids already drawn, so they fade in only once
  let ticking: { el: HTMLElement; since: number }[] = []; // elapsed timers of the board

  const render = (): void => {
    replace(root, id ? detailPage() : listPage());
  };

  const loadTimeline = async (t: Task): Promise<void> => {
    timeline = null;
    if (t.agent_cid === null) {
      timeline = [];
      return;
    }
    try {
      timeline = timelineOf(t.id, await api.transcript(t.agent_cid));
    } catch {
      timeline = [];
    }
  };

  const load = async (): Promise<void> => {
    const n = ++seq;
    try {
      if (id) {
        const t = await api.task(id);
        if (n !== seq) return;
        current = t;
        missing = false;
        await loadTimeline(t);
        if (n !== seq) return;
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
    if (n === seq) render();
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
    // Empty leaves the budget to the hub's default (the hint shows it).
    const budget = input({ type: 'number', min: MIN_TASK_BUDGET, step: 1, placeholder: `${DEFAULT_TASK_BUDGET} (hub default)` });
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
      const budgetText = budget.value.trim();
      const budgetCredits = budgetText === '' ? undefined : Number(budgetText);
      if (budgetCredits !== undefined && (!Number.isInteger(budgetCredits) || budgetCredits < MIN_TASK_BUDGET)) {
        budget.setCustomValidity(`Budget must be a whole number of at least ${MIN_TASK_BUDGET} credits.`);
        budget.reportValidity();
        return;
      }
      budget.setCustomValidity('');
      submit.disabled = true;
      const created = await attempt(
        () =>
          api.createTask({
            goal: detail ? `${goalTitle}\n\n${detail}` : goalTitle,
            acceptance: acceptance.value.trim() || undefined,
            role: role.value || undefined,
            budget: budgetCredits,
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

  /** The head strip: figures for each lane, and how many agents are working now. */
  function summary(list: Task[]): HTMLElement {
    const count = (col: number): number => list.filter((t) => columnOf(t.state) === col).length;
    const working = agents.get().filter((a) => a.status === 'working').length;
    const fig = (label: string, value: number): HTMLElement => h('div.tk-fig', null, h('b', null, String(value)), h('span', null, label));
    return h(
      'div.tk-summary',
      null,
      fig('Open', count(0)),
      fig('In progress', count(1)),
      fig('Done', count(2)),
      fig('Failed', count(3)),
      fig('Agents working', working),
    );
  }

  /** One card. A card new to the board fades in once; a card whose state changed flashes once. */
  function taskCard(t: Task): HTMLElement {
    const col = columnOf(t.state);
    const prev = lastState.get(t.id);
    const classes = ['task-card', `lane-${col}`];
    if (!seen.has(t.id)) classes.push('enter');
    if (prev !== undefined && prev !== t.state) classes.push('changed');
    seen.add(t.id);
    lastState.set(t.id, t.state);

    const poster = (t as Task & { poster?: string | null }).poster;
    const who = h(
      'div.task-who',
      null,
      h('span.task-dot', { class: `s-${statusOf(t.agent_cid)}` }),
      assigneeLink(t.agent_cid),
      poster ? h('span.faint', null, `by ${poster}`) : null,
    );

    let detail: HTMLElement;
    if (col === 1) {
      const elapsed = h('span.task-elapsed', null, fmtElapsed(Date.now() - t.updated_at));
      ticking.push({ el: elapsed, since: t.updated_at });
      const line = (t.agent_cid !== null && activity.get(t.agent_cid)) || 'waiting for the first step…';
      detail = h(
        'div.task-live',
        null,
        h('div.task-stripe'),
        h('div.task-activity', { 'data-activity': String(t.agent_cid ?? '') }, line),
        h('div.task-meta', null, h('span.faint', null, 'elapsed'), elapsed),
      );
    } else if (col === 2) {
      detail = h(
        'div',
        null,
        t.note ? h('p.task-note', null, clip(t.note, 120)) : null,
        h('div.task-meta', null, h('span.faint', null, `took ${fmtSpan(t.updated_at - t.created_at)}`), h('span.faint', null, ago(t.updated_at))),
      );
    } else if (col === 3) {
      detail = h(
        'div',
        null,
        h('p.task-reason', null, t.outcome || t.note || labelOf(t.state)),
        h('div.task-meta', null, h('span.faint', null, labelOf(t.state)), h('span.faint', null, ago(t.updated_at))),
      );
    } else {
      detail = h('div.task-meta', null, h('span.faint', null, `budget ${t.budget}`), h('span.faint', null, `created ${ago(t.created_at)}`));
    }

    const el = h(
      'div',
      { class: classes.join(' '), role: 'link', tabindex: 0, 'data-key': `task-${t.id}` },
      h('div.task-top', null, h('span.task-id', null, t.id), h('span.task-role', null, t.role ? roleLabel(t.role) : 'Any role')),
      h('strong.task-title', null, titleOf(t.goal)),
      who,
      detail,
    );
    const open = (): void => go('tasks', t.id);
    el.addEventListener('click', open);
    el.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') open();
    });
    return el;
  }

  /** The four lanes, each full height, with a placeholder line when empty. */
  function board(list: Task[]): HTMLElement {
    ticking = [];
    const sorted = list.slice().sort((a, b) => b.updated_at - a.updated_at);
    return h(
      'div.tk-board-wrap',
      null,
      h(
        'div.tk-board',
        null,
        LANES.map((lane, i) => {
          const items = sorted.filter((t) => columnOf(t.state) === i);
          return h(
            'section.tk-lane',
            { class: `lane-${i}`, 'data-key': `lane-${i}` },
            h(
              'header.tk-lane-head',
              null,
              h('span.tk-mark', { class: `mark-${i}` }),
              h('h2', null, lane.label),
              h('span.tk-count', null, String(items.length)),
            ),
            h('div.tk-lane-body', null, items.length ? items.map(taskCard) : h('p.tk-empty', null, lane.idle)),
          );
        }),
      ),
    );
  }

  function listPage(): HTMLElement {
    return h(
      'div.tasks-view',
      null,
      h('div.view-head', null, h('h1', null, 'Tasks'), h('div.actions', null, button('New task', openCreate, { kind: 'primary', icon: 'plus' }))),
      summary(tasks),
      board(tasks),
    );
  }

  function detailPage(): HTMLElement {
    const back = h('a.back', { href: href('tasks') }, icon('chevronRight'), 'Tasks');
    if (missing) return h('div.tasks-view', null, back, h('p.tk-empty', null, 'Task not found.'));
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

    const facts = h(
      'dl.kv',
      null,
      h('dt', null, 'State'),
      h('dd', null, badge(labelOf(t.state), stateTone(t.state), true)),
      h('dt', null, 'Role'),
      h('dd', null, t.role ? roleLabel(t.role) : 'Any role'),
      h('dt', null, 'Assignee'),
      h('dd', null, assigneeLink(t.agent_cid)),
      h('dt', null, 'Budget'),
      h('dd', null, String(t.budget)),
      h('dt', null, 'Created'),
      h('dd', null, ago(t.created_at)),
      h('dt', null, 'Updated'),
      h('dd', null, ago(t.updated_at)),
    );

    const tl = timeline;
    const steps =
      tl === null
        ? h('p.tk-empty', null, 'Loading…')
        : tl.length === 0
          ? h('p.tk-empty', null, t.agent_cid === null ? 'Not yet assigned.' : 'No steps recorded yet.')
          : h('ol.tk-tl', null, tl.map(timelineRow));

    const main = h(
      'div.tk-main',
      null,
      h('section.tk-block', null, h('h3.tk-label', null, 'Goal'), h('div.tk-goal', null, t.goal)),
      t.acceptance ? h('section.tk-block', null, h('h3.tk-label', null, 'Acceptance'), h('div.tk-prose', null, t.acceptance)) : null,
      t.note ? h('section.tk-block', null, h('h3.tk-label', null, 'Result'), h('div.tk-result', null, t.note)) : null,
    );
    const side = h(
      'aside.tk-side',
      null,
      h(
        'section.tk-block',
        null,
        h('h3.tk-label', null, 'Facts'),
        facts,
        isOpen ? assignSel : null,
        isOpen ? h('div.row', null, h('span', { style: 'flex:1' }), withdrawBtn) : null,
      ),
      h('section.tk-block', null, h('h3.tk-label', null, 'Timeline'), steps),
    );

    return h(
      'div.tasks-view',
      null,
      back,
      h('div.view-head', null, h('div.tk-head', null, h('span.task-id', null, t.id), h('h1', null, titleOf(t.goal)))),
      h('div.tk-cols', null, main, side),
    );
  }

  render();
  const clockTimer = setInterval(() => {
    const now = Date.now();
    for (const x of ticking) x.el.textContent = fmtElapsed(now - x.since);
  }, 1000);
  const offs = [
    on('task', schedule),
    on('work', schedule),
    on('transcript', (data) => {
      const e = data as Entry;
      if (id) {
        // On a task page the timeline is rebuilt from the assignee's transcript.
        if (current && e.cid === current.agent_cid) schedule();
        return;
      }
      const line = activityOf(e);
      if (line === null) return;
      activity.set(e.cid, line);
      root.querySelectorAll<HTMLElement>(`[data-activity="${e.cid}"]`).forEach((el) => {
        el.textContent = line;
      });
    }),
    agents.watch(() => render()),
  ];
  void load();

  return () => {
    clearTimeout(timer);
    clearInterval(clockTimer);
    offs.forEach((off) => off());
    ticking = [];
    dismiss?.();
  };
}
