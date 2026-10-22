import '../styles/proto/overview.css';
import { agentLabelIn, agentName, roleLabel } from '../roles';
import { api, type Agent, type AgentProposal, type AgentStep, type Approval, type Entry, type Project, type Task } from '../api';
import { go, href } from '../router';
import { inScope, isLive, onBench, projectTitle, scope } from '../scope';
import { agents, on, overview, RESYNC } from '../store';
import { ago, attempt, clip, dot, empty, stat, type Tone } from '../ui/components';
import { attachBar, maySend } from '../ui/attach-bar';
import { contextBar, loadModelChoices, modelChip } from '../ui/chat-tools';
import { contextApi } from '../api-context';
import { h, lu, replace } from '../ui/dom';
import type { LucideName } from '../ui/icons.gen';
import { openNewProject } from '../ui/new-project';
import type { Cleanup } from './index';
import { planWaitsForOwner, waitsForOwner, type StepOf } from './work-model';

// Overview: the plaza at a glance, in the prototype's bare page. The orchestrator's composer leads, then the three
// ways in, the figures, what waits on the owner beside the live activity, the projects in progress and the fleet.

const FEED_MAX = 40;
const DEBOUNCE_MS = 300;

const ORCH: Record<string, [string, Tone]> = {
  working: ['Working', 'info'],
  idle: ['Idle', 'ok'],
  asleep: ['Asleep', 'muted'],
  dozing: ['Asleep (light)', 'muted'],
};
const KIND_ICON: Record<string, LucideName> = { review: 'git-pull-request', tool: 'terminal', deploy: 'rocket', payment: 'credit-card' };

/** A card with the prototype's head and body: `.ov-cardh` over `.ov-body`, flush when the body is a list. */
function sectionCard(cls: string, head: Node[], body: Node, flush = false): HTMLElement {
  return h('section.card.ov-card', { class: cls }, h('div.ov-cardh', null, ...head), h('div.ov-body', { class: flush ? 'flush' : null }, body));
}

/** A way in: an icon, its line, the New project tag and an arrow; it opens the New project dialog. */
function way(icon: LucideName, text: string): HTMLButtonElement {
  const b = h('button', { type: 'button' }, lu(icon, 15), text, h('small', null, 'New project'), h('span.hw-go', null, lu('arrow-right', 15)));
  b.addEventListener('click', () => openNewProject());
  return b;
}

/** The worker model and its provider, from `provider/model`. */
function modelParts(m: string): [string, string] {
  const s = String(m || '');
  const i = s.indexOf('/');
  return [s.slice(s.lastIndexOf('/') + 1) || '—', i > 0 ? s.slice(0, i) : ''];
}

const modelStat = (label: string, [name, provider]: [string, string]): HTMLElement =>
  h('div.stat', null, h('span.stat-l', null, label), h('b.stat-v.ov-model', null, name), h('small', null, provider));

export function mount(root: HTMLElement): Cleanup {
  // Under a project, the overview is that project's: its agents, its tasks, its budget, its activity.
  const inProject = scope.get();
  const statsSlot = h('div');
  const orchSlot = h('div');
  const subLine = h('p.home-sub');
  const waitingSlot = h('div');
  const feedWait = h('p.muted.ov-line', null, 'Waiting for activity…');
  const feed = h('ul.ov-feed');
  const projects = h('div');
  const fleet = h('div');
  const orch = composerBox();

  root.append(
    h(
      'div.home',
      null,
      h('h1.home-h', null, inProject ? projectTitle(inProject) : 'What should the plaza work on?'),
      subLine,
      orchSlot,
      h('div.home-ways', null, way('messages-square', 'Start a project by talking it through'), way('folder-plus', 'Open a project directly'), way('git-pull-request-arrow', 'Bring in a project that exists')),
      statsSlot,
      h('div.ov-cols', null, waitingSlot, sectionCard('ov-act', [h('h2.ov-h', null, 'Activity')], h('div', null, feedWait, feed))),
      sectionCard('ov-full', [h('h2.ov-h', null, 'Projects')], projects, true),
      sectionCard('ov-full', [h('h2.ov-h', null, 'Fleet')], fleet, true),
    ),
  );

  // The projects, read again when a project or a task moves. An agent's step comes as it moves: a task whose agent
  // waits on the owner is the owner's.
  let projectsTimer = 0;
  let drawn: { ps: Project[]; ts: Task[] } | null = null;
  const steps = new Map<number, AgentStep | null>();
  const stepOf: StepOf = (cid) => (steps.has(cid) ? steps.get(cid) : agents.get().find((a) => a.cid === cid)?.step);
  const paintProjects = (all: Project[], ts: Task[]): void => {
    const ps = inProject ? all.filter((p) => p.id === inProject) : all;
    drawn = { ps, ts };
    renderProjects(projects, ps, ts, stepOf);
    drawStats();
  };
  const loadProjects = async (): Promise<void> => {
    try {
      const [all, ts] = await Promise.all([api.projects(), inProject ? api.tasksWhere({ project: inProject }) : api.tasks()]);
      paintProjects(all, ts);
    } catch {
      replace(projects, empty('alert', 'Projects are unavailable', 'apid did not answer; they come back with it.'));
    }
  };
  const scheduleProjects = (): void => {
    window.clearTimeout(projectsTimer);
    projectsTimer = window.setTimeout(() => void loadProjects(), DEBOUNCE_MS);
  };
  void loadProjects();

  // What waits on the owner: the pending approvals and the proposed agents, read again when either moves.
  let approvals: Approval[] = [];
  let proposals: AgentProposal[] = [];
  let waitTimer = 0;
  let alive = true;
  const loadWaiting = async (): Promise<void> => {
    try {
      approvals = await api.approvals();
    } catch {
      // Keep the last list the owner saw; the next event reads again.
    }
    try {
      proposals = await api.agentProposals();
    } catch {
      // As above.
    }
    if (alive) drawWaiting();
  };
  const scheduleWaiting = (): void => {
    window.clearTimeout(waitTimer);
    waitTimer = window.setTimeout(() => void loadWaiting(), DEBOUNCE_MS);
  };
  void loadWaiting();

  // The figures: the plaza's (with its bench), or under a project the project's.
  let budget: number | null = null;
  const drawStats = (): void => {
    const o = overview.get();
    if (!o) return replace(statsSlot, h('div.card.ov-card', null, empty('alert', 'apid is unreachable', 'Start apid, then reload.')));
    const roster = agents.get();
    if (inProject) {
      const team = roster.filter((a) => a.project === inProject);
      const ts = drawn?.ts ?? [];
      const open = ts.filter((t) => t.state === 'open' || t.state === 'claimed').length;
      const p = drawn?.ps[0];
      replace(
        statsSlot,
        h(
          'div.ov-stats',
          null,
          stat('Agents', team.length, `${team.filter((a) => a.status === 'working').length} working`),
          stat('Open tasks', open, `${ts.filter((t) => t.state === 'done').length} done`),
          stat('Stage', p ? p.stage.charAt(0).toUpperCase() + p.stage.slice(1) : '—', p?.paused ? 'paused' : p?.archived ? 'archived' : 'in progress'),
          stat('Budget', budget === null ? '—' : budget.toLocaleString('en-US'), budget === null ? 'from the plaza' : 'credits left'),
        ),
      );
      return;
    }
    const open = (o.tasks['open'] ?? 0) + (o.tasks['claimed'] ?? 0);
    const bench = roster.filter(onBench).length;
    replace(
      statsSlot,
      h(
        'div.ov-stats',
        null,
        stat('Agents', o.agents.total.toLocaleString('en-US'), `${o.agents.working.toLocaleString('en-US')} working · ${bench} on the bench`),
        stat('Open tasks', open.toLocaleString('en-US'), `${(o.tasks['done'] ?? 0).toLocaleString('en-US')} done`),
        modelStat('Worker model', modelParts(o.default_model)),
        modelStat('Orchestrator model', modelParts(o.orchestrator_model)),
      ),
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

  // The waiting card: the approvals and proposals the owner has yet to decide, under a project only its own.
  const nameOf = (cid: number): string => agentLabelIn(agents.get(), cid);
  const shownAp = (a: Approval): boolean => {
    if (!inProject || a.cid === 0) return true;
    const agent = agents.get().find((x) => x.cid === a.cid);
    return !agent || agent.role === 'orchestrator' || inScope(agent, inProject);
  };
  const drawWaiting = (): void => {
    const list = approvals.filter((a) => a.state === 'pending' && shownAp(a)).sort((x, y) => x.created_ms - y.created_ms);
    const props = proposals.filter((p) => p.state === 'pending');
    const n = list.length + props.length;
    const rows = [
      ...list.map((a) => {
        const title = waitTitle(a);
        return h('a', { href: href('approvals', a.id) }, lu(KIND_ICON[a.kind] ?? 'shield-check', 15), h('span.lab', { title }, title), h('small', null, `${nameOf(a.cid)} · ${ago(a.created_ms)}`));
      }),
      ...props.map((p) => h('a', { href: href('approvals', p.id) }, lu('user-plus', 15), h('span.lab', null, p.role.title || p.role.id), h('small', null, `${p.by} · ${ago(p.created_ms)}`))),
    ];
    replace(
      waitingSlot,
      sectionCard(
        'ov-waiting',
        [h('h2.ov-h', null, `Waiting on you · ${n}`), h('span.ov-grow'), h('a.btn.ghost.sm', { href: href('approvals') }, 'Approvals')],
        n ? h('div.ov-wait', null, rows) : empty('approval', 'Nothing is waiting', 'Agents ask here before acts that need your approval, and the orchestrator proposes new agents here.'),
        true,
      ),
    );
  };

  // The live feed: the transcript's events of the plaza's agents, or of the project's.
  const offs = [
    overview.watch(() => drawStats()),
    agents.watch((list) => {
      renderOrchestrator(list.find((a) => a.tier === 'orchestrator'));
      renderFleet(fleet, inProject ? list.filter((a) => inScope(a, inProject)) : list);
      drawStats();
      drawWaiting();
    }),
    on('transcript', (data) => {
      const entry = data as Entry;
      // Under a project, the activity of its agents only.
      if (inProject) {
        const agent = agents.get().find((a) => a.cid === entry.cid);
        if (agent && !inScope(agent, inProject)) return;
      }
      pushFeed(feed, feedWait, entry);
    }),
    on('project', scheduleProjects),
    on('task', scheduleProjects),
    on('approval', scheduleWaiting),
    on('agent_proposal', scheduleWaiting),
    on('agent_step', (data) => {
      const { cid, step } = data as { cid: number; step: AgentStep | null };
      const decided = (stepOf(cid)?.stage === 'approval') !== (step?.stage === 'approval');
      steps.set(cid, step);
      if (decided && drawn) renderProjects(projects, drawn.ps, drawn.ts, stepOf);
    }),
  ];

  // The orchestrator: its composer when one is connected, else a card that says so.
  function renderOrchestrator(o: Agent | undefined): void {
    if (!o) {
      subLine.textContent = '';
      replace(orchSlot, h('div.card', null, empty('orchestrator', 'No orchestrator connected', 'Add the orchestrator role to the fleet to delegate work by goal.')));
      return;
    }
    const [label, tone] = ORCH[o.status] ?? ['Offline', 'muted'];
    subLine.textContent = `${agentName(o)} plans the work and delegates it to the fleet · ${o.model} · Ctrl+Enter to send`;
    orch.aim(o.cid, agentName(o), o.model, label, tone);
    orch.follow(o);
    if (orchSlot.firstChild !== orch.el) replace(orchSlot, orch.el);
  }
  renderOrchestrator(agents.get().find((a) => a.tier === 'orchestrator'));
  drawStats();
  drawWaiting();
  // The projects and tasks as they were last read paint at once; the fresh read (already asked) replaces them.
  const keptProjects = api.projects.cached();
  const keptTasks = inProject ? api.tasksWhere.cached({ project: inProject }) : api.tasks.cached();
  if (keptProjects && keptTasks) paintProjects(keptProjects, keptTasks);
  offs.push(
    on(RESYNC, () => {
      scheduleProjects();
      scheduleWaiting();
    }),
  );

  return () => {
    alive = false;
    window.clearTimeout(projectsTimer);
    window.clearTimeout(waitTimer);
    offs.forEach((off) => off());
    orch.unbind();
  };
}

/** A project's monogram tile, as the shell draws it: two letters on one of six hues. Kept here, since the shell's own
 *  module reads the window's matchMedia when it loads, which the node test run has not. */
function projectMark(p: Pick<Project, 'id' | 'title'>, size: number): HTMLElement {
  const words = (p.title || p.id || '?').replace(/[^A-Za-z0-9 ]/g, '').split(/\s+/).filter(Boolean);
  const m = (((words[0] ?? '?')[0] ?? '?') + ((words[1] ?? words[0] ?? '?')[0] ?? '')).toUpperCase();
  const hue = Number((p.id || '').replace(/\D/g, '') || 0) % 6;
  return h('span.pmono', { class: `h${hue}`, style: `--s:${size}px`, 'aria-hidden': 'true' }, m);
}

/** A line per project still in progress: its title, its stage and its tasks, opening its Work view. */
function renderProjects(root: HTMLElement, list: Project[], tasks: Task[], stepOf: StepOf): void {
  const active = list.filter(isLive);
  if (!active.length) {
    replace(root, empty('projects', 'No projects in progress', 'Each goal you give the orchestrator becomes a project; its flow shows here.'));
    return;
  }
  replace(
    root,
    active.map((p) => {
      const own = tasks.filter((t) => t.project === p.id || p.tasks.includes(t.id));
      const done = own.filter((t) => t.state === 'done').length;
      const stage = p.stage.charAt(0).toUpperCase() + p.stage.slice(1);
      const yours = planWaitsForOwner(p) ? 'waits for your approval' : waitsForOwner(p, own, stepOf) ? 'waits for you' : '';
      const sub = [stage, own.length ? `${done} of ${own.length} tasks done` : '', yours].filter(Boolean).join(' · ');
      return h(
        'a.ov-row',
        { href: href('work', p.id), 'data-key': `pj-${p.id}` },
        projectMark(p, 28),
        h('span.ov-grow.ov-min', null, h('span.ov-strong.ov-clip', null, p.title), h('span.muted.ov-clip', null, sub)),
      );
    }),
  );
}

/** The composer to the orchestrator: the files, the message, and its bar. Made once per mount, so a draft and its files survive redraws. */
interface Composer {
  el: HTMLElement;
  /** Points the next message at this agent, with its line on the bar. */
  aim: (cid: number, name: string, model: string, label: string, tone: Tone) => void;
  /** Shows the orchestrator's model and override on the chip. */
  follow: (agent: Agent) => void;
  unbind: () => void;
}

function composerBox(): Composer {
  let cid = 0;
  let sending = false;
  let pick: HTMLElement | null = null;
  let choicesLoaded = false;
  const ta = h('textarea', { rows: 3, placeholder: 'Tell the orchestrator what you want done…', 'aria-label': 'Message to the orchestrator' });
  const bar = attachBar({ onChange: () => syncSend() });
  const send = h('button.send', { type: 'button', 'aria-label': 'Send', title: 'Send (Ctrl+Enter)' }, lu('arrow-up', 16, '', 2.4));
  const transcript = h('button.pick', { type: 'button' }, lu('scroll-text', 15), h('span', null, 'Open transcript'));
  bar.button.className = 'pick';
  bar.button.title = 'Attach files';
  bar.button.setAttribute('aria-label', 'Attach files');
  // The model chip joins the tool row; the context line sits above the composer; a save error shows under it.
  const chip = modelChip({ label: 'Orchestrator model', save: (value) => contextApi.setAgentModel(cid, value || null) });
  const ctx = contextBar({ cid: () => cid, source: (next) => contextApi.agent(next) });
  const cbar = h('div.cbar', null, transcript, chip.el, bar.button, send);
  const el = h('div.composer.ov-composer', { 'data-ov': 'composer' }, bar.el, ta, cbar);
  const wrap = h('div.ov-compose-wrap', null, ctx.el, el, chip.note);
  const syncSend = (): void => {
    send.disabled = sending || bar.busy() || bar.failed();
  };
  const submit = async (): Promise<void> => {
    const text = ta.value.trim();
    const ids = bar.ids();
    if (!maySend(text, ids, sending || bar.busy() || bar.failed())) return;
    sending = true;
    syncSend();
    const ok = await attempt(() => api.chat(cid, text, ids), 'Sent to the orchestrator');
    sending = false;
    syncSend();
    if (ok) {
      ta.value = '';
      bar.clear();
      ctx.refresh();
    }
  };
  send.addEventListener('click', () => void submit());
  transcript.addEventListener('click', () => {
    if (cid) go('agents', cid);
  });
  ta.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) void submit();
  });
  const unbindDrop = bar.bind(el);
  return {
    el: wrap,
    aim: (next, name, model, label, tone) => {
      const moved = next !== cid;
      cid = next;
      const line = h('a.pick', { href: href('agents', next), title: `${name} · ${model}` }, dot(tone), h('span', null, name), h('span.muted', null, label));
      if (pick) pick.replaceWith(line);
      else cbar.prepend(line);
      pick = line;
      syncSend();
      if (moved) ctx.refresh();
    },
    follow: (agent) => {
      if (!choicesLoaded) {
        choicesLoaded = true;
        void loadModelChoices('orchestrator').then((c) => chip.setChoices(c), () => undefined);
      }
      chip.set({ value: agent.model_override ?? '', shown: agent.model });
    },
    unbind: () => {
      unbindDrop();
      ctx.dispose();
      chip.dispose();
    },
  };
}

/** What the waiting card says of an approval: its goal, its payment, or the tool call. */
export function waitTitle(a: Approval): string {
  const d = (a.detail ?? {}) as { goal?: string; task?: string };
  const inp = (a.input ?? {}) as { command?: unknown };
  if (a.kind === 'review') return d.goal || `Review of ${d.task}`;
  if (a.kind === 'payment') return `Payment awaiting approval: ${a.subject}`;
  if (a.kind === 'deploy') return a.subject;
  if (a.kind === 'artifact.share') return `Share ${a.subject}`;
  return `${a.tool}${typeof inp.command === 'string' ? `: ${inp.command}` : typeof a.detail === 'string' ? `: ${a.detail}` : ''}`;
}

/** The fleet's tiles: each agent's name, its work or state, opening its transcript. */
function renderFleet(root: HTMLElement, list: Agent[]): void {
  if (!list.length) return replace(root, empty('agents', 'No agents yet', 'Agents appear here as their VMs connect.'));
  replace(
    root,
    h(
      'div.ov-fleet',
      null,
      list.map((a) => {
        const tone: Tone = a.status === 'working' ? 'info' : a.status === 'idle' ? 'ok' : 'muted';
        const sub = a.work ? clip(a.work.prompt, 60) : a.status === 'offline' ? 'offline' : a.status === 'asleep' ? 'asleep' : a.status === 'dozing' ? 'asleep (light)' : roleLabel(a.role);
        const tile = h(
          'button.ov-tile',
          { type: 'button', title: `${agentName(a)} · ${roleLabel(a.role)} · ${a.model}`, 'data-key': `ag-${a.cid}` },
          dot(tone),
          h('span.ov-min', null, h('span.ov-strong.ov-clip', null, agentName(a)), h('span.muted.ov-clip', null, sub)),
        );
        tile.addEventListener('click', () => go('agents', a.cid));
        return tile;
      }),
    ),
  );
}

/** One line of the Activity card: when, who, and what they did. */
function pushFeed(feed: HTMLElement, wait: HTMLElement, e: Entry): void {
  const line = describe(e);
  if (!line) return;
  wait.remove();
  const name = agentLabelIn(agents.get(), e.cid);
  const item = h(
    'li',
    null,
    h('span.muted.ov-time', null, ago(e.at_ms)),
    h('span.ov-feedmain', null, h('a.ov-agent', { href: href('agents', e.cid) }, name), ' ', h('span.muted', null, line)),
  );
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
