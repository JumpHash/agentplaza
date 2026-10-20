import { agentName, roleLabel } from '../roles';
// Overview: the plaza at a glance — the orchestrator front and centre, the
// projects with their flow lines, the fleet's state, and a live activity feed.

import { api, type Agent, type AgentStep, type Entry, type Project, type Task } from '../api';
import { go, href } from '../router';
import { inScope, isLive, onBench, projectTitle, scope } from '../scope';
import { agents, on, overview } from '../store';
import { ago, attempt, badge, button, card, clip, empty, stat, textarea } from '../ui/components';
import { attachBar, maySend } from '../ui/attach-bar';
import { h, icon, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { flowLine } from './work';
import { FLOW_TASKS, flowSegments, planWaitsForOwner, waitsForOwner, type StepOf } from './work-model';

const FEED_MAX = 40;

export function mount(root: HTMLElement): Cleanup {
  // Under a project, the overview is that project's: its agents, its tasks, its budget, its activity.
  const inProject = scope.get();
  const stats = h('div.grid.cols-4');
  const orchestrator = h('div');
  const orchBox = orchestratorBox();
  const feed = h('ol.feed');
  const fleet = h('div');
  const projects = h('div');

  root.append(
    h('div.view-head', null, h('h1', null, 'Overview', inProject ? h('span.faint', null, ` · ${projectTitle(inProject)}`) : null)),
    stats,
    h('div.grid.cols-2.overview-main', null, orchestrator, card('Activity', feed, { flush: true })),
    card('Projects', projects, { flush: true }),
    card('Fleet', fleet, { flush: true }),
  );

  // The projects' flow lines, read again when a project or a task moves. The agents' steps come as they move: a task
  // whose agent waits on the owner is the owner's square.
  let projectsTimer = 0;
  let drawn: { ps: Project[]; ts: Task[] } | null = null;
  const steps = new Map<number, AgentStep | null>();
  const stepOf: StepOf = (cid) => (steps.has(cid) ? steps.get(cid) : agents.get().find((a) => a.cid === cid)?.step);
  const loadProjects = async (): Promise<void> => {
    try {
      const [all, ts] = await Promise.all([api.projects(), inProject ? api.tasksWhere({ project: inProject }) : api.tasks()]);
      const ps = inProject ? all.filter((p) => p.id === inProject) : all;
      drawn = { ps, ts };
      renderProjects(projects, ps, ts, stepOf);
      drawStats();
    } catch {
      replace(projects, empty('alert', 'Projects are unavailable', 'apid did not answer; they come back with it.'));
    }
  };
  const scheduleProjects = (): void => {
    window.clearTimeout(projectsTimer);
    projectsTimer = window.setTimeout(() => void loadProjects(), 300);
  };
  void loadProjects();

  // The figures: the plaza's (with its bench), or under a project the project's.
  let budget: number | null = null;
  const drawStats = (): void => {
    const o = overview.get();
    if (!o) return replace(stats, empty('alert', 'apid is unreachable', 'Start apid, then reload.'));
    const roster = agents.get();
    if (inProject) {
      const team = roster.filter((a) => a.project === inProject);
      const ts = drawn?.ts ?? [];
      const open = ts.filter((t) => t.state === 'open' || t.state === 'claimed').length;
      const p = drawn?.ps[0];
      replace(
        stats,
        stat('Agents', team.length, `${team.filter((a) => a.status === 'working').length} working`),
        stat('Open tasks', open, `${ts.filter((t) => t.state === 'done').length} done`),
        stat('Stage', p ? p.stage.charAt(0).toUpperCase() + p.stage.slice(1) : '—', p?.paused ? 'paused' : p?.archived ? 'archived' : 'in progress'),
        stat('Budget', budget === null ? '—' : budget.toLocaleString(), budget === null ? 'from the plaza' : 'credits left'),
      );
      return;
    }
    const open = (o.tasks['open'] ?? 0) + (o.tasks['claimed'] ?? 0);
    const bench = roster.filter(onBench).length;
    replace(
      stats,
      stat('Agents', o.agents.total, `${o.agents.working} working · ${bench} on the bench`),
      stat('Open tasks', open, `${o.tasks['done'] ?? 0} done`),
      stat('Worker model', o.default_model.split('/').pop() ?? '—', o.default_model.split('/')[0]),
      stat('Orchestrator model', o.orchestrator_model.split('/').pop() ?? '—', o.orchestrator_model.split('/')[0]),
    );
  };
  if (inProject)
    void api
      .projectBudget(inProject)
      .then((b) => {
        budget = b.own ? b.balance : null;
        drawStats();
      })
      .catch(() => undefined);

  const offs = [
    overview.watch(() => drawStats()),
    agents.watch((list) => {
      renderOrchestrator(orchestrator, list.find((a) => a.tier === 'orchestrator'), orchBox);
      renderFleet(fleet, inProject ? list.filter((a) => inScope(a, inProject)) : list);
      drawStats();
    }),
    on('transcript', (data) => {
      const entry = data as Entry;
      // Under a project, the activity of its agents only.
      if (inProject) {
        const agent = agents.get().find((a) => a.cid === entry.cid);
        if (agent && !inScope(agent, inProject)) return;
      }
      pushFeed(feed, entry);
    }),
    on('project', scheduleProjects),
    on('task', scheduleProjects),
    on('agent_step', (data) => {
      const { cid, step } = data as { cid: number; step: AgentStep | null };
      const decided = (stepOf(cid)?.stage === 'approval') !== (step?.stage === 'approval');
      steps.set(cid, step);
      if (decided && drawn) renderProjects(projects, drawn.ps, drawn.ts, stepOf);
    }),
  ];
  replace(feed, h('li.feed-empty.faint', null, 'Waiting for activity…'));
  return () => {
    window.clearTimeout(projectsTimer);
    offs.forEach((off) => off());
    orchBox.unbind();
  };
}

/** A line per project still in progress: its title, its stage and its flow line, opening its Work view. */
function renderProjects(root: HTMLElement, list: Project[], tasks: Task[], stepOf: StepOf): void {
  const active = list.filter(isLive);
  if (!active.length) {
    replace(root, empty('projects', 'No projects in progress', 'Each goal you give the orchestrator becomes a project; its flow shows here.'));
    return;
  }
  replace(
    root,
    h(
      'div.ov-projects',
      null,
      active.map((p) => {
        const own = tasks.filter((t) => t.project === p.id || p.tasks.includes(t.id));
        const done = own.filter((t) => t.state === 'done').length;
        const stage = p.stage.charAt(0).toUpperCase() + p.stage.slice(1);
        const yours = planWaitsForOwner(p) ? 'waits for your approval' : waitsForOwner(p, own, stepOf) ? 'waits for you' : '';
        return h(
          'a.ov-project',
          { href: href('work', p.id), 'data-key': `pj-${p.id}` },
          h('span.ov-project-title', null, p.title),
          h('span.faint', null, [stage, own.length ? `${done} of ${own.length} tasks done` : '', yours].filter(Boolean).join(' · ')),
          flowLine(flowSegments(p, own, stepOf), Math.max(0, own.length - FLOW_TASKS)),
        );
      }),
    ),
  );
}

/** The message box to the orchestrator. It is made once per mount, so a draft and its files survive the card's redraws. */
interface OrchestratorBox {
  /** The text and the files, stacked. */
  el: HTMLElement;
  /** The attach button and Send, for the card's row. */
  controls: HTMLElement;
  /** The agent the next message goes to. */
  aim: (cid: number) => void;
  unbind: () => void;
}

function orchestratorBox(): OrchestratorBox {
  let cid = 0;
  let sending = false;
  const goal = textarea({ placeholder: 'Tell the orchestrator what you want done…', rows: 3 });
  const bar = attachBar({ onChange: () => syncSend() });
  const send = button('Send', () => void submit(), { kind: 'primary', icon: 'send' });
  const syncSend = (): void => {
    send.disabled = sending || bar.busy() || bar.failed();
  };
  const submit = async (): Promise<void> => {
    const text = goal.value.trim();
    const ids = bar.ids();
    if (!maySend(text, ids, sending || bar.busy() || bar.failed())) return;
    sending = true;
    syncSend();
    const ok = await attempt(() => api.chat(cid, text, ids), 'Sent to the orchestrator');
    sending = false;
    syncSend();
    if (ok) {
      goal.value = '';
      bar.clear();
    }
  };
  goal.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) void submit();
  });
  const el = h('div.stack', null, bar.el, goal);
  return {
    el,
    controls: h('div.row', null, bar.button, send),
    aim: (next) => {
      cid = next;
    },
    unbind: bar.bind(el),
  };
}

function renderOrchestrator(root: HTMLElement, orch: Agent | undefined, box: OrchestratorBox): void {
  if (!orch) {
    replace(root, card('Orchestrator', empty('orchestrator', 'No orchestrator connected', 'Add the orchestrator role to the fleet to delegate work by goal.')));
    return;
  }
  box.aim(orch.cid);
  const status = orch.status === 'working' ? badge('Working', 'info', true) : orch.status === 'idle' ? badge('Idle', 'ok', true) : orch.status === 'asleep' ? badge('Asleep', 'muted', true) : orch.status === 'dozing' ? badge('Asleep (light)', 'muted', true) : badge('Offline', 'muted', true);
  replace(
    root,
    card(
      h('div.row', null, icon('orchestrator'), h('h2', null, agentName(orch)), h('span.faint', null, roleLabel(orch.role)), status),
      h(
        'div.stack',
        null,
        h('p.muted', null, `Plans the work and delegates it to the fleet · ${orch.model}`),
        box.el,
        h('div.row', null, h('span.faint', null, 'Ctrl+Enter to send'), h('span', { style: 'flex:1' }), h('a', { href: href('agents', orch.cid) }, 'Open transcript'), box.controls),
      ),
    ),
  );
}

function renderFleet(root: HTMLElement, list: Agent[]): void {
  if (!list.length) return replace(root, empty('agents', 'No agents yet', 'Agents appear here as their VMs connect.'));
  replace(
    root,
    h(
      'div.fleet',
      null,
      list.map((a) => {
        const tone = a.status === 'working' ? 'info' : a.status === 'idle' ? 'ok' : 'muted';
        const tile = h(
          'button.fleet-tile',
          { type: 'button', title: `${agentName(a)} · ${roleLabel(a.role)} · ${a.model}`, 'data-key': `ag-${a.cid}` },
          h('span.row', null, h('span.dot', { class: tone }), h('strong.truncate', null, agentName(a))),
          h('span.faint.truncate', null, a.work ? clip(a.work.prompt, 60) : a.status === 'offline' ? 'offline' : a.status === 'asleep' ? 'asleep' : a.status === 'dozing' ? 'asleep (light)' : roleLabel(a.role)),
        );
        tile.addEventListener('click', () => go('agents', a.cid));
        return tile;
      }),
    ),
  );
}

function pushFeed(feed: HTMLElement, e: Entry): void {
  const line = describe(e);
  if (!line) return;
  feed.querySelector('.feed-empty')?.remove();
  const found = agents.get().find((a) => a.cid === e.cid);
  const name = found ? agentName(found) : `agent ${e.cid}`;
  const item = h('li', null, h('span.feed-time.faint', null, ago(e.at_ms)), h('a', { href: href('agents', e.cid) }, name), h('span.muted', null, line));
  feed.prepend(item);
  while (feed.children.length > FEED_MAX) feed.lastElementChild?.remove();
}

function describe(e: Entry): string | null {
  const d = e.data;
  switch (e.kind) {
    case 'work_start':
      return `started ${String(d['kind'])}: ${clip(String(d['prompt'] ?? ''), 80)}`;
    case 'work_end':
      return `finished (${String(d['status'])})`;
    case 'tool_request':
      return `called ${String(d['name'])}`;
    case 'error':
      return `error: ${clip(String(d['message'] ?? ''), 80)}`;
    default:
      return null;
  }
}
