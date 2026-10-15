import { agentName, roleLabel } from '../roles';
// The roster: every agent the plaza holds, filterable by role and status, with
// each row's own actions. Live through the agents cell; it fetches nothing itself.

import { api, type Agent, type AgentStatus, type WorkKind } from '../api';
import { go } from '../router';
import { agents, refreshAgents } from '../store';
import {
  attempt,
  badge,
  button,
  card,
  clip,
  compact,
  dot,
  empty,
  select,
  table,
  type Column,
  type Tone,
} from '../ui/components';
import { h, replace } from '../ui/dom';
import { avatar } from './agent-profile';
import { openAgentWizard } from './agent-wizard';
import type { Cleanup } from './index';
import '../styles/agents.css';

const STATUS_TONE: Record<AgentStatus, Tone> = { working: 'info', idle: 'ok', offline: 'muted', asleep: 'muted', dozing: 'muted' };
const WORK_TONE: Record<WorkKind, Tone> = { task: 'accent', chat: 'info', message: 'muted', motion: 'warn' };

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

export function mountAgentList(root: HTMLElement): Cleanup {
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
    {
      label: 'Agent',
      cell: (a) =>
        h(
          'div.row.agent-cell',
          null,
          avatar(a, 40),
          h('div', null, h('div.row', null, dot(statusPill(a.status).tone), h('strong.truncate', null, agentName(a)), pillWord(a)), h('div.faint.truncate', null, roleLabel(a.role))),
        ),
    },
    {
      label: 'Model',
      cell: (a) =>
        h(
          'div.row.agent-model',
          null,
          h('span.mono.truncate', null, a.model),
          a.tier === 'orchestrator' ? badge('orchestrator', 'accent') : null,
          a.model_source !== 'default' ? badge(a.model_source === 'agent' ? 'own' : 'role') : null,
        ),
    },
    {
      label: 'Work',
      cell: (a) =>
        a.work
          ? h('div.row.agent-work', null, badge(a.work.kind, WORK_TONE[a.work.kind]), h('span.truncate', null, clip(a.work.prompt, 56)))
          : h('span.faint', null, '—'),
    },
    { label: 'Tokens', num: true, cell: (a) => compact(a.usage.input_tokens + a.usage.output_tokens) },
    { label: '', width: '1%', cell: (a) => actions(a) },
  ];

  root.append(
    h('div.view-head', null, h('h1', null, 'Agents'), h('div.row.agent-filters', null, roleSel, statusSel, spawnHead)),
    card(null, body, { flush: true }),
  );

  const off = agents.watch(render);
  return () => off();

  function actions(a: Agent): HTMLElement {
    return h(
      'div.row.agent-actions',
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

  function render(list: Agent[]): void {
    syncRoles(list);
    if (!list.length) {
      replace(body, empty('agents', 'No agents connected', 'Spawn agents to fill the roster.', button('Spawn agents', () => openWizard(), { kind: 'primary', icon: 'plus' })));
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
    replace(body, h('div.table-scroll', null, table(columns, rows, (a) => go('agents', a.cid), (a) => `ag-${a.cid}`)));
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
