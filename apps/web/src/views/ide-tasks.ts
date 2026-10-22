// The Tasks tab of the IDE's right column: the project's tasks, active first, with done of total. The markup is the
// prototype's dock Tasks (loom: m-ide-tasks): a head with the project and its count, then a row per task with its state
// glyph, goal, id, agent and state chip.
import { agentLabel } from '../roles';
import { api, type Project, type Task } from '../api';
import { badge, empty } from '../ui/components';
import { h, lu, replace } from '../ui/dom';

const MAX_TASKS = 50;
const REFRESH_MS = 20_000;

export interface TasksPanel {
  el: HTMLElement;
  show(): void;
  dispose(): void;
}

type Tone = 'done' | 'failed' | 'active' | 'open';

/** The glyph's class in the prototype: a ring, a pulsing dot, a check, a cross. */
const GLYPH_CLASS: Record<Tone, string> = { done: 's-done', failed: 's-failed', active: 's-working', open: 's-open' };
/** The state chip's tone. */
const CHIP_TONE: Record<Tone, 'ok' | 'danger' | 'info' | 'muted'> = { done: 'ok', failed: 'danger', active: 'info', open: 'muted' };

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

function row(t: Task): HTMLElement {
  const tone = toneOf(t.state);
  const who = t.agent_cid != null ? agentLabel(t.agent_cid) : t.assignee_id;
  const glyph = tone === 'done' ? lu('check', 13, '', 2.4) : tone === 'failed' ? lu('x', 13, '', 2.4) : null;
  return h(
    'div.idt-r',
    { 'data-tone': tone },
    h(`span.itk-g.${GLYPH_CLASS[tone]}`, { 'aria-hidden': 'true' }, glyph),
    h(
      'span.itk-txt',
      null,
      h('span.itk-goal', null, t.goal),
      h(
        'span.itk-meta',
        null,
        h('span.mono', null, shortId(t.id)),
        who ? h('span', null, who) : null,
        badge(t.state.replace(/_/g, ' '), CHIP_TONE[tone]),
      ),
    ),
  );
}

export function tasksPanel(opts: { project: Project | null; onCount(done: number, total: number): void }): TasksPanel {
  const { project } = opts;
  const summary = h('span');
  const body = h('div', { tabindex: 0, 'aria-label': 'Project tasks' });
  const el = h(
    'section.idt',
    { 'aria-label': 'Tasks' },
    h('div.idt-h', null, h('b', null, project ? project.title : 'Tasks'), h('span.sp'), summary),
    body,
  );

  if (!project) {
    replace(body, empty('list-checks', 'Not in a project', 'Open an agent, a conversation or a repository of a project to see its tasks.'));
    return { el, show() {}, dispose() {} };
  }

  const ids = project.tasks.slice(0, MAX_TASKS);
  replace(body, h('p.ic-muted', null, 'Loading tasks'));

  let loaded = false;
  let inflight = false;
  let disposed = false;
  let timer: ReturnType<typeof setInterval> | null = null;

  function render(tasks: Task[]): void {
    const done = tasks.filter((t) => toneOf(t.state) === 'done').length;
    summary.textContent = tasks.length === 0 ? '' : `${done} of ${tasks.length} done`;
    if (tasks.length === 0) {
      replace(
        body,
        ids.length === 0
          ? empty('list-checks', 'No tasks yet', 'The orchestrator posts the plan\'s tasks once it is approved.')
          : empty('list-checks', 'Could not load the tasks', 'Try again in a moment.'),
      );
      return;
    }
    const sorted = [...tasks].sort((a, b) => RANK[toneOf(a.state)] - RANK[toneOf(b.state)] || b.updated_at - a.updated_at);
    const top = body.scrollTop;
    replace(body, sorted.map(row));
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
