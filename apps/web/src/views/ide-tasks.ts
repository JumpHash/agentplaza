// The Tasks tab of the IDE right column: the project's tasks with done / total.
import { api, type Project, type Task } from '../api';
import { empty } from '../ui/components';
import { h, replace } from '../ui/dom';
import css from '../styles/ide-panels.css';

const STYLE_ID = 'apz-ide-panels-style';
const MAX_TASKS = 50;
const REFRESH_MS = 20_000;

export interface TasksPanel {
  el: HTMLElement;
  show(): void;
  dispose(): void;
}

type Tone = 'done' | 'failed' | 'active' | 'open';

const GLYPH: Record<Tone, string> = {
  done: '✓',
  failed: '×',
  active: '•',
  open: '○',
};

// Active work first, then open, failed, and done last.
const RANK: Record<Tone, number> = { active: 0, open: 1, failed: 2, done: 3 };

function toneOf(state: string): Tone {
  const s = state.toLowerCase();
  if (/done|complete|finished|merged|accepted/.test(s)) return 'done';
  if (/fail|error|cancel|reject|abort/.test(s)) return 'failed';
  if (/progress|running|active|claimed|assigned|working/.test(s)) return 'active';
  return 'open';
}

function shortId(id: string): string {
  return `#${id.slice(-6)}`;
}

function ensureStyle(): void {
  if (document.getElementById(STYLE_ID)) return;
  const style = document.createElement('style');
  style.id = STYLE_ID;
  style.textContent = css;
  document.head.appendChild(style);
}

function row(t: Task): HTMLElement {
  const tone = toneOf(t.state);
  const who = t.agent_cid != null ? `agent ${t.agent_cid}` : t.assignee_id;
  return h('li.ipt-row', { 'data-tone': tone },
    h('span.ipt-glyph', { 'aria-hidden': 'true' }, GLYPH[tone]),
    h('div.ipt-main', null,
      h('div.ipt-line', null, h('span.ipt-id', null, shortId(t.id)), h('span.ipt-goal', null, t.goal)),
      h('div.ipt-meta', null, who ? h('span', null, who) : null, h('span.ipt-state', null, t.state.replace(/_/g, ' ')))));
}

export function tasksPanel(opts: { project: Project | null; onCount(done: number, total: number): void }): TasksPanel {
  ensureStyle();
  const { project } = opts;
  const count = h('span.ipt-total');
  const body = h('div.ipt-body', { tabindex: 0, 'aria-label': 'Project tasks' });
  const el = h('section.ipt', { 'aria-label': 'Tasks' },
    h('header.ipt-head', null, h('span.ipt-title', null, 'Tasks'), count),
    body);

  if (!project) {
    replace(body, empty('tasks', 'Not in a project', 'This workspace is not part of a project.'));
    return { el, show() {}, dispose() {} };
  }

  const ids = project.tasks.slice(0, MAX_TASKS);
  replace(body, h('p.ipa-note', null, 'Loading tasks'));

  let loaded = false;
  let inflight = false;
  let disposed = false;
  let timer: ReturnType<typeof setInterval> | null = null;

  function render(tasks: Task[]): void {
    count.textContent = String(tasks.length);
    if (tasks.length === 0) {
      replace(body, ids.length === 0
        ? empty('tasks', 'No tasks yet', 'Tasks added to this project appear here.')
        : empty('tasks', 'Could not load the tasks', 'Try again in a moment.'));
      return;
    }
    const sorted = [...tasks].sort((a, b) =>
      RANK[toneOf(a.state)] - RANK[toneOf(b.state)] || b.updated_at - a.updated_at);
    const top = body.scrollTop;
    replace(body, h('ul.ipt-list', null, sorted.map(row)));
    body.scrollTop = top;
  }

  async function load(): Promise<void> {
    if (inflight || disposed) return;
    inflight = true;
    try {
      // A failed load of one task is skipped, not fatal.
      const results = await Promise.all(ids.map((id) => api.task(id).catch(() => null)));
      if (disposed) return;
      const tasks = results.filter((t): t is Task => t !== null);
      render(tasks);
      const done = tasks.filter((t) => toneOf(t.state) === 'done').length;
      opts.onCount(done, tasks.length);
    } finally {
      inflight = false;
    }
  }

  function show(): void {
    if (disposed) return;
    if (!loaded) {
      loaded = true;
      void load();
    }
    if (timer === null) {
      timer = setInterval(() => {
        if (!document.hidden) void load();
      }, REFRESH_MS);
    }
  }

  function dispose(): void {
    disposed = true;
    if (timer !== null) clearInterval(timer);
    timer = null;
  }

  return { el, show, dispose };
}
