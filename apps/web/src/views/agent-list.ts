import { agentName, roleLabel } from '../roles';
// The roster: every agent the plaza holds, filterable by role and status, with
// each row's own actions. Live through the agents cell; it fetches nothing itself.

import { api, type Agent, type AgentStatus, type WorkKind } from '../api';
import { go, href } from '../router';
import { inScope, projectTitle, scope } from '../scope';
import { agents, refreshAgents } from '../store';
import { attempt, badge, button, card, clip, compact, dot, empty, select, table, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { openAgentWizard } from './agent-wizard';
import type { Cleanup } from './index';

const STATUS_TONE: Record<AgentStatus, Tone> = { working: 'info', idle: 'ok', offline: 'muted', asleep: 'muted', dozing: 'muted' };
const WORK_TONE: Record<WorkKind, Tone> = { task: 'accent', chat: 'info', message: 'muted' };
/** The badge a model's source earns on the roster: the agent's own, its project's, or its role's. */
const SOURCE: Partial<Record<Agent['model_source'], [string, Tone]>> = { agent: ['own', 'info'], project: ['project', 'accent'], role: ['role', 'muted'] };

/**
 * The status pill: the dot in the status's tone, and a word for the states the dot alone does not tell apart
 * (asleep and offline are both muted, so asleep also reads its word). A dozing agent is asleep too, the light kind.
 * The word is calm: a muted badge.
 */
export function statusPill(status: AgentStatus): { tone: Tone; word: string | null } {
  return { tone: STATUS_TONE[status], word: sleepWord(status) };
}

/** The word for a napping status (asleep, or dozing as "asleep (light)"); null for the others. */
export function sleepWord(status: AgentStatus): string | null {
  if (status === 'asleep') return 'Asleep';
  if (status === 'dozing') return 'Asleep (light)';
  return null;
}

function pillWord(a: Agent): HTMLElement | null {
  const { word, tone } = statusPill(a.status);
  return word ? badge(word, tone) : null;
}

/** Where an agent works, as the roster says it: its project (a link), every project (the orchestrator), or a dash. */
export function projectCell(a: Agent): HTMLElement {
  if (a.project) return h('a.ag-clip', { href: href('projects', a.project), title: a.held ?? undefined }, projectTitle(a.project));
  if (a.tier === 'orchestrator') return h('span.muted', null, 'Every project');
  return h('span.muted', null, '—');
}

/** The agent's label and role: its status dot and word on the label line. */
function agentCell(a: Agent): HTMLElement {
  return h(
    'div.ag-agent',
    null,
    h('div.ag-who', null, h('div.ag-name', null, dot(statusPill(a.status).tone), ' ', agentName(a), ' ', pillWord(a)), h('div.ag-role', null, roleLabel(a.role))),
  );
}

/** The model as the roster shows it: the id in mono, then the orchestrator and source badges. */
function modelCell(a: Agent): HTMLElement {
  const source = SOURCE[a.model_source];
  return h(
    'div.ag-mdl',
    null,
    h('span.ag-model', { title: a.model }, a.model),
    a.tier === 'orchestrator' ? badge('orchestrator', 'accent') : null,
    source ? badge(source[0], source[1]) : null,
  );
}

export function mountAgentList(root: HTMLElement): Cleanup {
  // Under a project the roster is its agents; under All projects, every agent.
  const inProject = scope.get();
  let roleFilter = '';
  let statusFilter: 'all' | AgentStatus = 'all';
  let roles: string[] = [];

  const roleSel = select([{ value: '', label: 'All roles' }]);
  const statusSel = select([
    { value: 'all', label: 'All statuses' },
    { value: 'working', label: 'Working' },
    { value: 'idle', label: 'Idle' },
    { value: 'asleep', label: 'Asleep' },
    { value: 'offline', label: 'Offline' },
  ]);
  const body = h('div');
  const spawnHead = button('Spawn agents', () => openWizard(), { kind: 'primary', icon: 'plus' });

  roleSel.addEventListener('change', () => {
    roleFilter = roleSel.value;
    render(agents.get());
  });
  statusSel.addEventListener('change', () => {
    statusFilter = statusSel.value as 'all' | AgentStatus;
    render(agents.get());
  });

  const columns: Column<Agent>[] = [
    { label: 'Agent', cell: (a) => agentCell(a) },
    // Under All projects, where each agent works; under a project, all of them work on it.
    ...(inProject ? [] : [{ label: 'Project', cell: (a: Agent) => projectCell(a) }]),
    { label: 'Model', cell: modelCell },
    {
      label: 'Work',
      cell: (a) =>
        a.work
          ? h('span.ag-work', { title: a.work.prompt }, badge(a.work.kind, WORK_TONE[a.work.kind]), h('span.ag-work-t', null, clip(a.work.prompt, 90)))
          : h('span.muted', null, '—'),
    },
    { label: 'Tokens', num: true, cell: (a) => compact(a.usage.input_tokens + a.usage.output_tokens) },
    { label: '', cell: (a) => actions(a) },
  ];

  root.append(
    h(
      'header.view-head',
      null,
      h('div', null, h('h1', null, 'Agents', inProject ? h('span.view-sub', null, ` · ${projectTitle(inProject)}`) : null)),
      h('div.ag-head-actions', null, roleSel, statusSel, spawnHead),
    ),
    card('Agents', body, { flush: true }),
  );

  const off = agents.watch(render);
  return () => off();

  function actions(a: Agent): HTMLElement {
    return h(
      'div.cluster.ag-acts',
      null,
      button(
        '',
        (ev) => {
          ev.stopPropagation();
          go('agents', a.cid);
        },
        { kind: 'ghost', size: 'sm', icon: 'conversations', title: 'Chat' },
      ),
      a.status === 'working'
        ? button(
            '',
            (ev) => {
              ev.stopPropagation();
              void attempt(() => api.cancel(a.cid), 'Work cancelled');
            },
            { kind: 'ghost', size: 'sm', icon: 'ban', title: 'Cancel work' },
          )
        : null,
      button(
        '',
        (ev) => {
          ev.stopPropagation();
          void attempt(() => api.stop(a.cid), 'Session stopped');
        },
        { kind: 'ghost', size: 'sm', icon: 'stop', title: 'Stop session' },
      ),
    );
  }

  function render(all: Agent[]): void {
    const list = all.filter((a) => inScope(a, inProject));
    syncRoles(list);
    if (!list.length) {
      replace(
        body,
        inProject
          ? empty('agents', 'Nobody works on this project yet', 'Its agents show here while they work on its tasks.')
          : empty('agents', 'No agents connected', 'Spawn agents to fill the roster.', button('Spawn agents', () => openWizard(), { kind: 'primary', icon: 'plus' })),
      );
      return;
    }
    const rows = list.filter(
      (a) =>
        (roleFilter === '' || a.role === roleFilter) &&
        (statusFilter === 'all' || a.status === statusFilter || (statusFilter === 'asleep' && a.status === 'dozing')),
    );
    if (!rows.length) {
      replace(body, empty('search', 'No agents match', 'Clear the filters to see the roster.', button('Clear filters', clearFilters, { kind: 'ghost' })));
      return;
    }
    replace(body, h('div.tbl-wrap', null, table(columns, rows, (a) => go('agents', a.cid), (a) => `ag-${a.cid}`)));
  }

  function syncRoles(list: Agent[]): void {
    const next = [...new Set(list.map((a) => a.role))].sort();
    if (next.join('\n') === roles.join('\n')) return;
    roles = next;
    if (roleFilter && !roles.includes(roleFilter)) roleFilter = '';
    roleSel.replaceChildren(new Option('All roles', ''), ...roles.map((r) => new Option(roleLabel(r), r)));
    roleSel.value = roleFilter;
  }

  function clearFilters(): void {
    roleFilter = '';
    statusFilter = 'all';
    roleSel.value = '';
    statusSel.value = 'all';
    render(agents.get());
  }

  // Spawning goes through the agent wizard: pick or define a role, then spawn of it.
  function openWizard(): void {
    openAgentWizard({ onDone: () => void refreshAgents() });
  }
}
