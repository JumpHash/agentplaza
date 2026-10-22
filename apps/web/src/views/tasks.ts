// Tasks: the board of every task (four lanes of live cards) and one task in full (goal, result, facts, and a timeline
// from the assignee's transcript). A task is open, in progress, done, or failed or withdrawn. An open task moves by drag
// or from its ⋯ menu: into In progress it is assigned to an idle agent of its project, into Failed or withdrawn it is
// withdrawn; a task outside projects dropped on a project moves into it. The markup is the prototype's (m-work.js,
// styles/proto/work.css).

import '../styles/proto/work.css';
import { api, type Agent, type Entry, type Task } from '../api';
import { openIde } from '../ide/open';
import { go, href } from '../router';
import { liveProjects, projects, projectTitle, scope } from '../scope';
import { agents, on, RESYNC } from '../store';
import { agentLabel, agentName, roleLabel } from '../roles';
import { attempt, badge, button, field, input, select, textarea, toast, type Tone } from '../ui/components';
import { embedContextFromStore } from '../ui/embeds';
import { h, lu, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import type { Cleanup } from './index';
import { agoLabel, shortSpan, waitsByRole } from './work-model';
import { checkBox, emptyBox } from './work';
import { reviewPanel } from './review-graph';

const LANES = ['Open', 'In progress', 'Done', 'Failed or withdrawn'];
const MARK: Record<string, string> = { Open: '○', 'In progress': '●', Done: '✓', 'Failed or withdrawn': '×' };
const IDLE: Record<string, string> = {
  Open: 'Nothing open.',
  'In progress': 'Nothing in progress.',
  Done: 'Nothing finished yet.',
  'Failed or withdrawn': 'Nothing failed or withdrawn.',
};
const FAILED = new Set(['failed', 'withdrawn', 'cancelled']);
// The states a task has been through the review in: its panel shows in the drawer.
const REVIEW_STATES = new Set(['awaiting_review', 'rework', 'done', 'failed']);
const TIMELINE_KINDS = new Set(['work_start', 'tool_request', 'work_end', 'error']);
/** The words of a state: a few have their own (the review sends a task back to its worker as "Sent back"). */
const STATE_LABELS: Record<string, string> = { rework: 'Sent back' };

/** The lane for a state, by its index in LANES; unknown non-final states count as in progress. */
function columnOf(state: string): number {
  if (state === 'open') return 0;
  if (state === 'done') return 2;
  if (FAILED.has(state)) return 3;
  return 1;
}

const laneOf = (state: string): string => LANES[columnOf(state)] ?? 'In progress';

function stateTone(state: string): Tone {
  if (state === 'rework') return 'warn';
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
const labelOf = (state: string): string => STATE_LABELS[state] ?? state.replace(/_/g, ' ').replace(/^./, (c) => c.toUpperCase());
const cidName = (cid: number): string => {
  const a = agents.get().find((x) => x.cid === cid);
  return a ? agentName(a) : agentLabel(cid);
};
const idleWorkers = (): Agent[] => agents.get().filter((a) => a.status === 'idle' && a.tier === 'worker');
/** The idle workers on `project`: a project's task goes only to its agents. */
const idleAgents = (project: string | null): Agent[] => idleWorkers().filter((a) => !!project && a.project === project);
/** The projects that take tasks now: live, their plan approved. */
const takingTasks = () => liveProjects(projects.get()).filter((p) => p.stage === 'implementation' || p.stage === 'review');
const statusOf = (cid: number | null): string => agents.get().find((a) => a.cid === cid)?.status ?? 'offline';
const pad = (n: number): string => String(n).padStart(2, '0');

/** mm:ss, the elapsed time of work that is running. */
const hms = (ms: number): string => {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${pad(Math.floor(s / 60))}:${pad(s % 60)}`;
};

/** A span such as "4 min" or "3 h 20 min", the length of a finished task. */
const dur = (ms: number): string => {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  if (s < 172_800) {
    const hr = Math.floor(s / 3600);
    const mi = Math.floor((s % 3600) / 60);
    return mi ? `${hr} h ${mi} min` : `${hr} h`;
  }
  const d = Math.floor(s / 86_400);
  const hr = Math.floor((s % 86_400) / 3600);
  return hr ? `${d} d ${hr} h` : `${d} d`;
};

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

/** A task's id from its route: `t528`, or just its number (`#/tasks/528`, the link a `task #528` reference opens). */
const taskId = (raw: string): string => (/^\d+$/.test(raw) ? `t${raw}` : raw);

/** A modal sheet over a scrim, as the prototype draws it: a head with the title and close, the body, and a footer. */
function sheet(title: string, body: Node, foot: Node | null, onClose?: () => void): () => void {
  let closed = false;
  const close = (): void => {
    if (closed) return;
    closed = true;
    document.removeEventListener('keydown', onKey);
    scrim.remove();
    onClose?.();
  };
  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape') close();
  };
  const scrim = h(
    'div.scrim',
    null,
    h(
      'div.sheet',
      { role: 'dialog', 'aria-modal': 'true', 'aria-label': title },
      h('header.sheet-h', null, h('h2', null, title), h('button.iconbtn', { type: 'button', title: 'Close', 'aria-label': 'Close', onclick: close }, lu('x', 16))),
      h('div.sheet-b', null, body),
      foot ? h('footer.sheet-f', null, foot) : null,
    ),
  );
  scrim.addEventListener('mousedown', (e) => {
    if (e.target === scrim) close();
  });
  document.querySelectorAll('.scrim').forEach((x) => x.remove());
  document.addEventListener('keydown', onKey);
  document.body.append(scrim);
  scrim.querySelector<HTMLElement>('input:not([type=checkbox]), textarea, select')?.focus();
  return close;
}

/** A drag of one card: where it started, where it is, and the ghost that follows the pointer once it has moved. */
interface Drag {
  id: string;
  t: Task;
  card: HTMLElement;
  x0: number;
  y0: number;
  x: number;
  y: number;
  on: boolean;
  ghost: HTMLElement | null;
  ox: number;
  oy: number;
  r: DOMRect | null;
}

/** What dropping a task into a lane may do: `same` stays put, `assign` and `withdraw` act, `refuse` says why. */
type Rule = { kind: 'same' } | { kind: 'assign' } | { kind: 'withdraw' } | { kind: 'refuse'; why: string };

/** A popover's items: a label and what picking it does. */
interface Item {
  label: string;
  run: () => void;
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const id = taskId(params[0] ?? '');
  // Under a project the board is that project's tasks; under All projects, every task.
  const inProject = scope.get();
  let tasks: Task[] = [];
  let current: Task | null = null;
  let timeline: Entry[] | null = null; // null while the assignee's transcript loads
  let missing = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let seq = 0;
  let dismiss: (() => void) | undefined;
  let readyOnly = false; // the board shows only the tasks that can be claimed now

  const activity = new Map<number, string>(); // assignee cid -> its latest line
  const seen = new Map<string, { state: string; at: number; cls: string }>(); // a card's state and its one-time animation
  let baselined = false;
  let animate = false;
  // The clock's texts: the elapsed times and the ages of the board, and the waits of the ready tasks.
  let ticking: { el: HTMLElement; fn: () => string }[] = [];

  // The drag, the open popover, and a render held back while a card is in the air.
  let drag: Drag | null = null;
  let pop: { el: HTMLElement; onClose?: () => void } | null = null;
  let suppressClick = false;
  let pending = false;

  const render = (): void => {
    if (drag?.on) {
      pending = true;
      return;
    }
    // A redraw keeps the card that had the focus (the arrow keys pick cards), so a live refresh does not drop it.
    const focused = document.activeElement;
    const kept = focused instanceof HTMLElement && root.contains(focused) ? focused.dataset.key : undefined;
    replace(root, ...(id ? detailPage() : listPage()));
    if (kept) root.querySelector<HTMLElement>(`[data-key="${CSS.escape(kept)}"]`)?.focus({ preventScroll: true });
  };

  /** Arrow keys move the focus card by card in the order drawn (lane by lane), as the desktop's board_order does. */
  const onArrow = (e: KeyboardEvent): void => {
    if (id || (e.key !== 'ArrowDown' && e.key !== 'ArrowUp')) return;
    if (document.querySelector('.scrim') || (e.target instanceof Element && e.target.closest('input, textarea, select, [contenteditable="true"]'))) return;
    const cards = Array.from(root.querySelectorAll<HTMLElement>('.task-card'));
    if (!cards.length) return;
    const focusedCard = (document.activeElement as Element | null)?.closest<HTMLElement>('.task-card');
    const at = focusedCard ? cards.indexOf(focusedCard) : -1;
    // Nothing picked yet: the first card, whichever arrow. The ends hold.
    const next = at < 0 ? 0 : e.key === 'ArrowDown' ? at + 1 : at - 1;
    e.preventDefault();
    cards[Math.min(Math.max(next, 0), cards.length - 1)]?.focus();
  };

  const loadTimeline = async (t: Task): Promise<void> => {
    timeline = null;
    if (t.agent_cid === null) {
      timeline = [];
      return;
    }
    try {
      timeline = timelineOf(t.id, await api.transcriptLast(t.agent_cid));
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
        const list = inProject ? await api.tasksWhere({ project: inProject }) : await api.tasks();
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

  // ---- Moves: assign, withdraw, move into a project (the API calls the console has) ----

  const assignTo = async (t: Task, a: Agent): Promise<void> => {
    const done = await attempt(() => api.assign(a.cid, t.id), `Assigned ${t.id} to ${agentName(a)}`);
    if (done) void load();
  };

  const withdraw = async (t: Task): Promise<void> => {
    const done = await attempt(() => api.withdrawTask(t.id), 'Task withdrawn');
    if (done) void load();
  };

  const moveTo = async (t: Task, pid: string): Promise<void> => {
    const moved = await attempt(() => api.moveTask(t.id, pid));
    if (!moved) return;
    const linked = moved.moved.filter((x) => x.id !== t.id).length;
    const title = projectTitle(moved.project);
    toast(linked ? `Moved ${t.id} and ${linked} linked into ${title}` : `Moved ${t.id} into ${title}`);
    void load();
  };

  // ---- New task ----

  const openCreate = (): void => {
    // Every task belongs to a project whose plan is approved: the one the console shows, or one picked here.
    const open = takingTasks();
    if (!open.length) {
      const closeNone = sheet(
        'New task',
        h('p.tk-empty', null, 'A task belongs to a project, and no project takes tasks yet. Open one first: New project, in the project switcher.'),
        button('Close', () => closeNone(), { kind: 'ghost' }),
      );
      return;
    }
    const projectSel = select(
      open.map((p) => ({ value: p.id, label: p.title })),
      inProject && open.some((p) => p.id === inProject) ? inProject : open[0]!.id,
    );
    const titleIn = input({ required: true, placeholder: 'What should get done?' });
    const details = textarea({ rows: 4 });
    const acceptance = textarea({ rows: 3 });
    const roles = Array.from(new Set(agents.get().filter((a) => a.tier === 'worker').map((a) => a.role).filter(Boolean))).sort();
    const roleSel = select([{ value: '', label: 'Any' }, ...roles.map((r) => ({ value: r, label: r }))]);
    const assignee = select([]);
    // The assignee is one of the chosen project's idle agents.
    const fillAssignees = (): void => {
      replace(
        assignee,
        [{ value: '', label: 'Let the scheduler choose' }, ...idleAgents(projectSel.value).map((a) => ({ value: String(a.cid), label: agentName(a) }))].map((o) =>
          h('option', { value: o.value }, o.label),
        ),
      );
    };
    fillAssignees();
    projectSel.addEventListener('change', fillAssignees);
    const submit = h('button.btn.primary', { type: 'submit', form: 'tk-new-form' }, lu('plus', 15), ' Create task');
    const form = h(
      'form.tk-new',
      { id: 'tk-new-form', novalidate: true },
      field('Project', projectSel),
      field('Title', titleIn),
      h('div.wide', null, field('Details', details, 'Optional.')),
      h('div.wide', null, field('Acceptance', acceptance, 'Optional.')),
      field('Role', roleSel),
      h('div.wide', null, field('Assign to', assignee)),
    );
    form.addEventListener('submit', (e) => {
      e.preventDefault();
      const goalTitle = titleIn.value.trim();
      if (!goalTitle) {
        titleIn.focus();
        return;
      }
      if (submit.disabled) return;
      const detail = details.value.trim();
      const acc = acceptance.value.trim();
      submit.disabled = true;
      submit.setAttribute('aria-busy', 'true');
      void (async () => {
        const created = await attempt(
          () =>
            api.createTask({
              goal: detail ? `${goalTitle}\n\n${detail}` : goalTitle,
              project: projectSel.value,
              acceptance: acc || undefined,
              role: roleSel.value || undefined,
              assignee: assignee.value ? Number(assignee.value) : undefined,
            }),
          'Task created',
        );
        submit.disabled = false;
        submit.removeAttribute('aria-busy');
        if (created) {
          close();
          go('tasks', created.id);
        }
      })();
    });
    const close = sheet(
      'New task',
      form,
      submit,
      () => {
        dismiss = undefined;
      },
    );
    dismiss = close;
  };

  /** Moves a task from before projects into one that takes tasks, with the tasks linked to it. */
  function moveSelect(t: Task): HTMLSelectElement | null {
    const open = takingTasks();
    if (!open.length) return null;
    const into = select([{ value: '', label: 'Move into a project…' }, ...open.map((p) => ({ value: p.id, label: p.title }))], '');
    into.setAttribute('aria-label', 'Move into a project…');
    into.addEventListener('change', () => {
      if (!into.value) return;
      into.disabled = true;
      void moveTo(t, into.value).finally(() => {
        into.disabled = false;
      });
    });
    return into;
  }

  // ---- The board ----

  /** The head strip: figures for each lane, and how many agents are working now. */
  function summary(list: Task[]): HTMLElement {
    const count = (col: number): number => list.filter((t) => columnOf(t.state) === col).length;
    const working = agents.get().filter((a) => a.status === 'working').length;
    const stat = (label: string, value: number): HTMLElement => h('div.stat', null, h('div.stat-l', null, label), h('div.stat-v', null, String(value)));
    return h(
      'div.tk-summary',
      null,
      stat('Open', count(0)),
      stat('In progress', count(1)),
      stat('Done', count(2)),
      stat('Failed', count(3)),
      stat('Agents working', working),
    );
  }

  /** A link to an agent; inside a clickable card, the name opens the agent, not the task. */
  function assigneeLink(cid: number): HTMLElement {
    const link = h('a', { href: href('agents', cid) }, cidName(cid));
    link.addEventListener('click', (e) => e.stopPropagation());
    return link;
  }

  /** A chip to a task: its id and its title; `fail` marks one that ended without being done. Inside a card a chip opens
   *  its own task, not the card's. */
  function chip(tid: string, fail: boolean): HTMLElement {
    const t = tasks.find((x) => x.id === tid);
    const el = h('a.task-chip', { href: href('tasks', tid), class: fail ? 'fail' : null }, `${tid}${t ? ` ${titleOf(t.goal).slice(0, 28)}` : ''}`);
    el.addEventListener('click', (e) => e.stopPropagation());
    return el;
  }

  /** A card's activity: what its agent does now, from the transcript it has been writing. */
  const activityText = (t: Task): string => (t.agent_cid !== null && activity.get(t.agent_cid)) || 'waiting for the first step…';

  /** A card's age, kept current by the clock. */
  function ageOf(since: number): HTMLElement {
    const el = h('span', null, agoLabel(since));
    ticking.push({ el, fn: () => agoLabel(since) });
    return el;
  }

  /** Whether a card is new to the board (it fades in once) or its state changed (it flashes once). */
  function animOf(t: Task): string {
    const at = Date.now();
    const prev = seen.get(t.id);
    if (!prev) seen.set(t.id, { state: t.state, at, cls: animate ? 'fx-in' : '' });
    else if (prev.state !== t.state) seen.set(t.id, { state: t.state, at, cls: 'fx-flash' });
    const rec = seen.get(t.id) as { state: string; at: number; cls: string };
    return rec.cls && at - rec.at < 1300 ? rec.cls : '';
  }

  /** One card. */
  function taskCard(t: Task): HTMLElement {
    const lane = columnOf(t.state);
    const a = agents.get().find((x) => x.cid === t.agent_cid);
    const fx = animOf(t);
    const who = h('div.task-who', null, h('span.dot', { class: `s-${statusOf(t.agent_cid)}` }), a ? assigneeLink(a.cid) : h('span.tk-empty', null, 'Unassigned'));
    let detail: HTMLElement;
    if (lane === 1) {
      const elapsed = h('span', null, hms(Date.now() - t.updated_at));
      ticking.push({ el: elapsed, fn: () => hms(Date.now() - t.updated_at) });
      detail = h('div', null, h('div.task-act', { 'data-activity': String(t.agent_cid ?? '') }, activityText(t)), h('div.task-foot', null, 'elapsed ', elapsed));
    } else if (lane === 2) {
      detail = h(
        'div',
        null,
        t.note ? h('div.task-act', null, t.note.slice(0, 120)) : null,
        h('div.task-foot', null, `took ${dur(t.updated_at - t.created_at)} · `, ageOf(t.updated_at)),
      );
    } else if (lane === 3) {
      detail = h('div', null, h('div.task-act', null, t.outcome || t.note || labelOf(t.state)), h('div.task-foot', null, `${labelOf(t.state)} · `, ageOf(t.updated_at)));
    } else {
      detail = h(
        'div',
        null,
        t.ready ? h('div.task-act', null, 'Ready to claim') : null,
        t.blocked_by.length ? h('div.task-foot', null, 'Blocked by ', t.blocked_by.map((x) => chip(x, true))) : null,
        !t.ready && t.waiting_on.length ? h('div.task-foot', null, 'Waits on ', t.waiting_on.map((x) => chip(x, false))) : null,
        h('div.task-foot', null, 'created ', ageOf(t.created_at)),
      );
    }
    const menu = h('button.iconbtn.task-menu-btn', { type: 'button', 'data-id': t.id, 'aria-label': `Moves for ${t.id}`, title: 'Moves' }, h('span', { 'aria-hidden': 'true' }, '⋯'));
    const el = h(
      'div',
      { class: ['task-card', fx].filter(Boolean).join(' '), role: 'link', tabindex: 0, 'data-key': `task-${t.id}`, 'data-id': t.id },
      h('div.task-top', null, h('span.task-id', null, t.id), h('span', null, t.role ? roleLabel(t.role) : 'Any role'), menu),
      h('div.task-title', null, titleOf(t.goal)),
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
    // Cards fade in only from the second draw on.
    animate = baselined;
    baselined = true;
    const sorted = list.slice().sort((a, b) => b.updated_at - a.updated_at);
    return h(
      'div.tk-board',
      null,
      LANES.map((ln, i) => {
        const items = sorted.filter((t) => columnOf(t.state) === i);
        return h(
          'section.tk-lane',
          { 'data-lane': ln },
          h('div.tk-lane-h', null, h('span', null, MARK[ln]), h('span', null, ln), h('span.n', null, String(items.length))),
          items.length ? items.map((t) => taskCard(t)) : h('p.tk-empty', null, IDLE[ln]),
        );
      }),
    );
  }

  /** The ready tasks nobody has claimed, by the role they wait for, longest wait first: where the plaza is short of an
   *  agent. Nothing when every ready task is taken. */
  function waits(list: Task[]): HTMLElement | null {
    const now = Date.now();
    const groups = waitsByRole(list, now);
    if (!groups.length) return null;
    const parts: (Node | string)[] = [];
    groups.forEach((g, i) => {
      const many = g.count > 1;
      const since = now - g.waited;
      const time = h('span', null, shortSpan(g.waited));
      ticking.push({ el: time, fn: () => shortSpan(Date.now() - since) });
      parts.push(i ? ' · ' : '', `${g.role ? roleLabel(g.role) : 'Any agent'} · ${g.count} ${many ? 'tasks' : 'task'} · ${many ? 'longest waited' : 'waited'} `, time);
    });
    return h('p.tk-waits', null, h('b', null, 'Ready, not claimed'), ' ', ...parts);
  }

  function listPage(): HTMLElement[] {
    const actions = [
      h('span', { title: 'Only the tasks that can be claimed now: open, and every task they run after done' }, checkBox(readyOnly, 'Ready only', (on) => {
        readyOnly = on;
        render();
      })),
      button('New task', openCreate, { kind: 'primary', icon: 'plus' }),
    ];
    const head = h(
      'header.page-head',
      null,
      h('div.page-head-row', null, h('h1', null, 'Tasks', inProject ? [' ', h('span.muted', null, `· ${projectTitle(inProject)}`)] : null), h('span.sp'), h('div.page-actions', null, ...actions)),
    );
    const shown = readyOnly ? tasks.filter((t) => t.ready) : tasks;
    // The board resets the clock's texts, so it is built before the waits line adds its own.
    const lanes = board(shown);
    const summaryEl = summary(tasks);
    const waitsEl = waits(tasks);
    // A task outside projects dropped on a project in this strip moves into it. Only on All projects.
    const strip = inProject
      ? null
      : h(
          'div.tk-drop-strip',
          { 'aria-label': 'Drop here to move the task into a project' },
          h('span', null, 'Move into'),
          takingTasks().map((p) => h('span.tk-drop-p', { 'data-drop-project': p.id }, p.title)),
        );
    return [head, h('div.tk-page', null, summaryEl, waitsEl, strip, lanes)];
  }

  // ---- The task in full ----

  function timelineRow(e: Entry): HTMLElement {
    return h('li', { class: `k-${e.kind}` }, h('time', null, new Date(e.at_ms).toTimeString().slice(0, 8)), h('span', null, describe(e)));
  }

  function detailPage(): Node[] {
    ticking = [];
    const back = h('a.back', { href: href('tasks') }, '‹ Tasks');
    if (missing) return [back, emptyBox('x', 'Task not found.')];
    const t = current;
    if (!t) return [back];

    const a = agents.get().find((x) => x.cid === t.agent_cid);
    const isOpen = t.state === 'open';
    const assignSel = select([{ value: '', label: 'Assign to…' }, ...idleAgents(t.project).map((x) => ({ value: String(x.cid), label: agentName(x) }))], '');
    assignSel.setAttribute('aria-label', 'Assign to…');
    assignSel.addEventListener('change', async () => {
      const cid = Number(assignSel.value);
      if (!cid) return;
      assignSel.disabled = true;
      await attempt(() => api.assign(cid, t.id), 'Assigned');
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

    const ids = (xs: string[], bad: boolean): Node[] => xs.map((x) => chip(x, bad));
    const projectCell = (): (Node | string | null)[] => {
      if (t.project) return [h('a', { href: href('projects', t.project) }, projectTitle(t.project)), ' · ', h('a', { href: href('work', t.project) }, 'its work')];
      const into = moveSelect(t);
      return ['Outside projects', into ? h('br') : null, into];
    };
    const facts: [string, Node | string | (Node | string | null)[]][] = [
      ['State', badge(labelOf(t.state), stateTone(t.state), true)],
      ['Role', t.role ? roleLabel(t.role) : 'Any role'],
      ['Assignee', a ? assigneeLink(a.cid) : 'Unassigned'],
      ['Project', projectCell()],
    ];
    if (t.after.length) facts.push(['Runs after', ids(t.after, false)]);
    if (t.blocked_by.length) facts.push(['Blocked by', ids(t.blocked_by, true)]);
    if (t.waiting_on.length) facts.push(['Waits on', ids(t.waiting_on, false)]);
    facts.push(['Created', agoLabel(t.created_at)], ['Updated', agoLabel(t.updated_at)]);
    const controls = isOpen ? h('div.tk-actions', null, assignSel, withdrawBtn) : null;

    const tl = timeline;
    const steps =
      tl === null
        ? h('p.tk-empty', null, 'Loading…')
        : tl.length === 0
          ? h('p.tk-empty', null, t.agent_cid === null ? 'Not yet assigned.' : 'No steps recorded yet.')
          : h('ol.tk-tl', null, tl.map(timelineRow));

    // The goal, acceptance, result and outcome are agent-written (or the owner's words to an agent): rich content applies.
    const embeds = embedContextFromStore();
    const block = (label: string, content: Node): HTMLElement => h('section.tk-block', null, h('div.tk-label', null, label), h('div', null, content));
    const main = [
      block('Goal', renderMarkdown(t.goal, { embeds })),
      t.acceptance ? block('Acceptance', renderMarkdown(t.acceptance, { embeds })) : null,
      t.note ? block('Result', renderMarkdown(t.note, { embeds })) : null,
      t.outcome && t.outcome !== t.note ? block('Outcome', renderMarkdown(t.outcome, { embeds })) : null,
      // The review of the change, once it is sealed: its verdict, the readings, the graph.
      REVIEW_STATES.has(t.state) ? block('Review', reviewPanel(t.id)) : null,
    ];
    const reviewBtn = a
      ? button('Review changes', () => openIde(`agent:${a.cid}`, { review: true }), { icon: 'square-code', title: "Review the assignee's changes since the work began" })
      : null;
    return [
      back,
      h('div.view-head', null, h('div', null, h('span.task-id', null, t.id), h('h1', null, titleOf(t.goal))), reviewBtn),
      h(
        'div.tk-cols',
        null,
        h('div.tk-stack', null, main),
        h(
          'div.tk-stack',
          null,
          h(
            'section.tk-block.tk-facts',
            null,
            facts.map(([k, v]) => h('div.tk-fact', null, h('span.tk-label', null, k), h('div.tk-fv', null, v))),
            controls,
          ),
          h('section.tk-block', null, h('div.tk-label', null, 'Timeline'), steps),
        ),
      ),
    ];
  }

  // ---- Drag and drop: pointer events, so mouse and touch alike ----
  // Open to In progress assigns (a popover picks an idle agent of the project). Open to Failed or withdrawn withdraws.
  // Any other move is refused, with its reason in a toast. A task outside projects dropped on a project in the strip
  // moves into it. Each allowed move is also in the card's ⋯ menu.

  const lanesEl = (): HTMLElement[] => [...root.querySelectorAll<HTMLElement>('.tk-lane[data-lane]')];
  const stripEl = (): HTMLElement | null => root.querySelector<HTMLElement>('.tk-drop-strip');

  const laneRule = (t: Task, ln: string): Rule => {
    const from = laneOf(t.state);
    if (ln === from) return { kind: 'same' };
    if (from === 'In progress') return { kind: 'refuse', why: 'Work in progress moves on its own: the hub decides its state.' };
    if (from === 'Done') return { kind: 'refuse', why: 'Done tasks stay done: the hub decides their state' };
    if (from === 'Failed or withdrawn') return { kind: 'refuse', why: 'Ended tasks stay ended: the hub decides their state' };
    if (ln === 'In progress') {
      if (!t.project) return { kind: 'refuse', why: 'Move it into a project first, then assign it.' };
      return idleAgents(t.project).length ? { kind: 'assign' } : { kind: 'refuse', why: `No idle agent in ${projectTitle(t.project)} to assign.` };
    }
    if (ln === 'Failed or withdrawn') return { kind: 'withdraw' };
    return { kind: 'refuse', why: 'Only the hub marks a task done: it decides when the work is finished.' };
  };

  const paintLanes = (t: Task): void =>
    lanesEl().forEach((el) => {
      const r = laneRule(t, el.dataset.lane ?? '');
      el.classList.toggle('drop-ok', r.kind === 'assign' || r.kind === 'withdraw');
      el.classList.toggle('drop-no', r.kind === 'refuse');
    });

  const dropDone = (): void => {
    document.body.style.cursor = '';
    document.body.style.userSelect = '';
    lanesEl().forEach((el) => el.classList.remove('drop-ok', 'drop-no'));
    stripEl()?.classList.remove('on');
    root.querySelectorAll('.tk-drop-p.over').forEach((el) => el.classList.remove('over'));
  };

  /** Shows the held-back render, once the card is back in place. */
  const flushPending = (): void => {
    if (pending && !drag?.on) {
      pending = false;
      render();
    }
  };

  const liftCard = (d: Drag): void => {
    const r = d.card.getBoundingClientRect();
    d.r = r;
    d.ox = d.x0 - r.left;
    d.oy = d.y0 - r.top;
    const g = d.card.cloneNode(true) as HTMLElement;
    g.className = 'task-card tk-ghost';
    Object.assign(g.style, { left: `${r.left}px`, top: `${r.top}px`, width: `${r.width}px`, height: `${r.height}px` });
    document.body.appendChild(g);
    d.ghost = g;
    d.on = true;
    d.card.classList.add('is-src');
    document.body.style.userSelect = 'none';
    paintLanes(d.t);
    if (!d.t.project) stripEl()?.classList.add('on');
  };

  const hoverAt = (d: Drag): void => {
    const under = document.elementFromPoint(d.x, d.y);
    const lane = under?.closest<HTMLElement>('.tk-lane[data-lane]') ?? null;
    const chipEl = under?.closest<HTMLElement>('.tk-drop-p') ?? null;
    root.querySelectorAll<HTMLElement>('.tk-drop-p').forEach((el) => el.classList.toggle('over', el === chipEl));
    const refuse = !!lane && laneRule(d.t, lane.dataset.lane ?? '').kind === 'refuse';
    document.body.style.cursor = refuse ? 'not-allowed' : 'grabbing';
  };

  /** A refused drop: the ghost glides back to the card's place, and the card is whole again. */
  const backGhost = (d: Drag): void => {
    const g = d.ghost;
    d.ghost = null;
    if (!g || !d.r) {
      g?.remove();
      d.card.classList.remove('is-src');
      return;
    }
    g.classList.add('back');
    void g.offsetWidth;
    g.style.left = `${d.r.left}px`;
    g.style.top = `${d.r.top}px`;
    setTimeout(() => {
      g.remove();
      d.card.classList.remove('is-src');
    }, 280);
  };

  const endDrag = (e: PointerEvent | null, cancelled: boolean): void => {
    const d = drag;
    drag = null;
    if (!d || !d.on) return;
    suppressClick = true;
    setTimeout(() => {
      suppressClick = false;
    }, 0);
    dropDone();
    const under = cancelled || !e ? null : document.elementFromPoint(e.clientX, e.clientY);
    const lane = under?.closest<HTMLElement>('.tk-lane[data-lane]') ?? null;
    const chipEl = under?.closest<HTMLElement>('.tk-drop-p') ?? null;
    const t = d.t;
    if (chipEl && !t.project) {
      d.ghost?.remove();
      d.card.classList.remove('is-src');
      void moveTo(t, chipEl.dataset.dropProject ?? '');
      flushPending();
      return;
    }
    const r: Rule = lane ? laneRule(t, lane.dataset.lane ?? '') : { kind: 'same' };
    if (r.kind === 'same') {
      backGhost(d);
      flushPending();
      return;
    }
    if (r.kind === 'refuse') {
      toast(r.why, 'error');
      backGhost(d);
      flushPending();
      return;
    }
    if (r.kind === 'withdraw') {
      d.ghost?.remove();
      d.card.classList.remove('is-src');
      void withdraw(t);
      flushPending();
      return;
    }
    // Assign: the ghost goes, the placeholder stays while the popover picks the agent; cancelling brings the card back.
    d.ghost?.remove();
    d.ghost = null;
    popOpen(
      e?.clientX ?? 0,
      e?.clientY ?? 0,
      'Assign to…',
      idleAgents(t.project).map((a) => ({ label: agentName(a), run: () => void assignTo(t, a) })),
      'Nobody idle in this project.',
      () => {
        d.card.classList.remove('is-src');
        render();
      },
    );
    flushPending();
  };

  // ---- Popovers: the ⋯ menu and the assign pick ----

  const popClose = (keep: boolean): void => {
    if (!pop) return;
    const p = pop;
    pop = null;
    p.el.remove();
    if (!keep) p.onClose?.();
  };

  /** A popover at a point: a heading and one button per item. Escape or a click outside closes it, and onClose runs. */
  const popOpen = (x: number, y: number, heading: string, items: Item[], empty: string, onClose?: () => void): void => {
    popClose(false);
    const el = h(
      'div.tk-pop',
      { role: 'menu', 'aria-label': heading },
      h('b', null, heading),
      items.length ? null : h('span.tk-pop-note', null, empty),
      items.map((it) => {
        const b = h('button', { type: 'button', role: 'menuitem' }, it.label);
        b.addEventListener('click', () => {
          popClose(true);
          it.run();
        });
        return b;
      }),
    );
    document.body.appendChild(el);
    const r = el.getBoundingClientRect();
    el.style.left = `${Math.max(8, Math.min(x, innerWidth - r.width - 8))}px`;
    el.style.top = `${Math.max(8, Math.min(y, innerHeight - r.height - 8))}px`;
    pop = { el, onClose };
    el.querySelector<HTMLElement>('button')?.focus();
  };

  /** The card's ⋯ menu: the same moves the drag offers. */
  const menuFor = (t: Task, x: number, y: number): void => {
    const items: Item[] = [];
    if (laneOf(t.state) === 'Open') {
      idleAgents(t.project).forEach((a) => items.push({ label: `Assign to ${agentName(a)}`, run: () => void assignTo(t, a) }));
      items.push({ label: 'Withdraw', run: () => void withdraw(t) });
    }
    if (!t.project) takingTasks().forEach((p) => items.push({ label: `Move into ${p.title}`, run: () => void moveTo(t, p.id) }));
    popOpen(x, y, `Moves for ${t.id}`, items, "No moves: the hub decides this task's state.");
  };

  const onPointerDown = (e: PointerEvent): void => {
    if (pop) {
      if (!pop.el.contains(e.target as Node)) popClose(false);
      return;
    }
    const target = e.target as Element | null;
    if (e.button > 0 || !target?.closest || target.closest('.task-menu-btn')) return;
    const card = target.closest<HTMLElement>('.task-card');
    if (!card || !root.contains(card)) return;
    const t = tasks.find((x) => x.id === card.dataset.id);
    if (!t) return;
    drag = { id: t.id, t, card, x0: e.clientX, y0: e.clientY, x: e.clientX, y: e.clientY, on: false, ghost: null, ox: 0, oy: 0, r: null };
  };

  const onPointerMove = (e: PointerEvent): void => {
    if (!drag) return;
    drag.x = e.clientX;
    drag.y = e.clientY;
    if (!drag.on) {
      if (Math.hypot(drag.x - drag.x0, drag.y - drag.y0) < 4) return;
      liftCard(drag);
    }
    if (drag.ghost) {
      drag.ghost.style.left = `${drag.x - drag.ox}px`;
      drag.ghost.style.top = `${drag.y - drag.oy}px`;
    }
    hoverAt(drag);
  };

  const onPointerUp = (e: PointerEvent): void => endDrag(e, false);
  const onPointerCancel = (): void => endDrag(null, true);

  const onEscape = (e: KeyboardEvent): void => {
    if (e.key !== 'Escape') return;
    if (pop) {
      e.stopPropagation();
      popClose(false);
      return;
    }
    if (drag?.on) {
      e.stopPropagation();
      const d = drag;
      drag = null;
      dropDone();
      backGhost(d);
      flushPending();
    }
  };

  /** A drag does not also open the card; the ⋯ button opens the card's move menu. */
  const onClickCapture = (e: MouseEvent): void => {
    if (suppressClick) {
      suppressClick = false;
      e.stopPropagation();
      e.preventDefault();
      return;
    }
    const b = (e.target as Element | null)?.closest?.('.task-menu-btn') as HTMLElement | null;
    if (!b) return;
    e.stopPropagation();
    e.preventDefault();
    const t = tasks.find((x) => x.id === b.dataset.id);
    if (t) {
      const r = b.getBoundingClientRect();
      menuFor(t, r.left, r.bottom + 4);
    }
  };

  // ---- Events ----

  const offs = [
    on('task', schedule),
    on('work', schedule),
    on(RESYNC, schedule),
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

  document.addEventListener('keydown', onArrow);
  document.addEventListener('keydown', onEscape, true);
  document.addEventListener('pointerdown', onPointerDown);
  document.addEventListener('pointermove', onPointerMove);
  document.addEventListener('pointerup', onPointerUp);
  document.addEventListener('pointercancel', onPointerCancel);
  window.addEventListener('click', onClickCapture, true);

  // The last read paints at once; the fresh read (asked below) replaces it.
  if (id) {
    current = api.task.cached(id) ?? null;
  } else {
    tasks = (inProject ? api.tasksWhere.cached({ project: inProject }) : api.tasks.cached()) ?? tasks;
  }
  render();
  const clockTimer = setInterval(() => {
    for (const x of ticking) x.el.textContent = x.fn();
  }, 1000);
  void load();

  return () => {
    clearTimeout(timer);
    clearInterval(clockTimer);
    offs.forEach((off) => off());
    document.removeEventListener('keydown', onArrow);
    document.removeEventListener('keydown', onEscape, true);
    document.removeEventListener('pointerdown', onPointerDown);
    document.removeEventListener('pointermove', onPointerMove);
    document.removeEventListener('pointerup', onPointerUp);
    document.removeEventListener('pointercancel', onPointerCancel);
    window.removeEventListener('click', onClickCapture, true);
    pop?.el.remove();
    pop = null;
    drag?.ghost?.remove();
    drag = null;
    ticking = [];
    dismiss?.();
  };
}
