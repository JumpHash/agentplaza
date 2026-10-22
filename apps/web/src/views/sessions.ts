// The sessions screen: `#/sessions` is the sessions of the project in scope, live ones first, each with its state, seat,
// role, task, project, model, usage and start; a row opens its session. `#/sessions/<ref>` is one session (session.ts).
// The expert view adds the node, the VM, the run, the engine, the attempt, the compactions and the id.

import '../styles/proto/sessions.css';
import { workApi, type Session } from '../api-work';
import { roleLabel } from '../roles';
import { go, href } from '../router';
import { projects, projectTitle, scope } from '../scope';
import { on, RESYNC } from '../store';
import { ago, badge, button, card, empty, table, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { EXPERT_KEY, mountSession } from './session';
import { costText, PLAZA_PROJECT, pausedText, seatText, sessionRows, stateLabel, stateTone, usageText } from './session-model';
import { readExpert, switchBox, writeExpert } from './work-expert';

/** The rows drawn at first; "Show more" draws the rest. */
const LIST_ROWS = 200;
const REFRESH_MS = 300;

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const ref = params[0];
  return ref ? mountSession(root, ref) : mountList(root);
}

const link = (url: string, text: string): HTMLElement => h('a', { href: url, onclick: (ev: Event) => ev.stopPropagation() }, text);

const stateCell = (s: Session): HTMLElement => {
  const reason = s.state === 'paused' ? pausedText(s.paused_by) : null;
  return h('div.se-state', null, badge(stateLabel(s.state), stateTone(s.state), true), reason ? h('span.se-reason', null, reason) : null);
};

const usageCell = (s: Session): HTMLElement => h('div', null, usageText(s.usage), h('div.se-reason', null, costText(s.usage.cost_micros)));

function mountList(root: HTMLElement): Cleanup {
  const project = scope.get();
  const filter = { project: project ?? undefined };
  // The last answer paints at once; the fresh one follows.
  const kept = workApi.sessions.cached(filter);
  let all: Session[] = kept ?? [];
  // Set once a list has come in (kept or fresh): a failed reload after that keeps the rows and says so.
  let loaded = kept !== undefined;
  let failure: string | null = null;
  let liveOnly = false;
  let expert = readExpert(EXPERT_KEY);
  let limit = LIST_ROWS;
  let seq = 0;
  let alive = true;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const load = async (): Promise<void> => {
    clearTimeout(timer);
    timer = undefined;
    const mine = ++seq;
    try {
      const list = await workApi.sessions(filter);
      if (!alive || mine !== seq) return;
      all = list;
      loaded = true;
      failure = null;
    } catch (err) {
      if (!alive || mine !== seq) return;
      failure = err instanceof Error ? err.message : String(err);
    }
    draw();
  };

  /** A throttle: a frame starts a load in REFRESH_MS unless one is already due, so a busy stream never starves it. */
  const schedule = (): void => {
    timer ??= setTimeout(() => void load(), REFRESH_MS);
  };

  const offs = [on('session', schedule), on(RESYNC, schedule)];

  const columns = (): Column<Session>[] => {
    const base: Column<Session>[] = [
      { label: 'State', cell: stateCell },
      { label: 'Seat', cell: (s) => seatText(s) },
      { label: 'Role', cell: (s) => roleLabel(s.role) },
      { label: 'Task', cell: (s) => (s.task ? link(href('tasks', s.task), s.task) : h('span.muted', null, 'no task')) },
      { label: 'Project', cell: (s) => (s.project === PLAZA_PROJECT ? 'The plaza' : link(href('projects', s.project), projectTitle(s.project))) },
      { label: 'Model', cell: (s) => (s.model ? h('span.mono', null, s.model) : h('span.muted', null, '—')) },
      { label: 'Usage', num: true, cell: usageCell },
      { label: 'Started', cell: (s) => ago(s.started_ms ?? s.created_ms) },
    ];
    if (!expert) return base;
    return [
      ...base,
      { label: 'Node', cell: (s) => s.node ?? '—' },
      { label: 'VM', num: true, cell: (s) => s.cid ?? '—' },
      { label: 'Run', cell: (s) => s.run ?? '—' },
      { label: 'Engine', cell: (s) => s.engine },
      { label: 'Attempt', num: true, cell: (s) => s.attempt },
      { label: 'Compactions', num: true, cell: (s) => s.compactions },
      { label: 'Id', cell: (s) => h('span.se-id', null, s.id) },
    ];
  };

  const head = (): HTMLElement =>
    h(
      'header.view-head',
      null,
      h('div', null, h('h1', null, 'Sessions', project ? h('span.view-sub', null, ` · ${projectTitle(project)}`) : null)),
      h(
        'div.actions',
        null,
        switchBox('Live only', liveOnly, (checked) => {
          liveOnly = checked;
          limit = LIST_ROWS;
          draw();
        }),
        switchBox('Expert', expert, (checked) => {
          expert = checked;
          writeExpert(EXPERT_KEY, checked);
          draw();
        }),
      ),
    );

  const body = (): Node[] => {
    if (!loaded) return [failure ? empty('alert', 'Could not load the sessions', failure) : h('p.muted', null, 'Loading…')];
    // A failed reload keeps the rows that are shown; its failure is one line above them.
    const alert: Node[] = failure ? [h('p.hint.err', null, `Could not refresh the sessions: ${failure}`)] : [];
    const rows = sessionRows(all, { project, liveOnly });
    if (!rows.length) {
      if (all.length) return [...alert, empty('agents', 'Nothing is live', 'Turn off Live only to see the sessions that have ended.')];
      return [
        ...alert,
        empty(
          'agents',
          'No sessions yet',
          project
            ? 'This project has no sessions yet. Its tasks run as sessions, one step each, and its planner runs as one.'
            : 'The plaza has no sessions yet. Each step of a task, each project planner and the orchestrator run as a session.',
        ),
      ];
    }
    const shown = rows.slice(0, limit);
    const more: Node[] =
      rows.length > shown.length
        ? [h('div.se-foot', null, button(`Show ${rows.length - shown.length} more`, () => { limit = rows.length; draw(); }))]
        : [];
    return [...alert, card(null, [table(columns(), shown, (s) => go('sessions', s.id), (s) => s.id), ...more], { flush: true })];
  };

  const draw = (): void => {
    replace(root, head(), body());
  };

  // The Project cell shows the projects' titles. A projects read sets them without a frame, so each set redraws the list.
  const offProjects = projects.watch(draw);
  void load();

  return () => {
    alive = false;
    clearTimeout(timer);
    offProjects();
    offs.forEach((off) => off());
  };
}
