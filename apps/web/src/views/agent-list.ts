// The roster: every agent the plaza holds, filterable by role and status, with
// each row's own actions. Live through the agents cell; it fetches nothing itself.

import { api, type Agent, type AgentStatus, type WorkKind } from '../api';
import { go } from '../router';
import { agents, refreshAgents } from '../store';
import {
  ago,
  attempt,
  badge,
  button,
  card,
  clip,
  compact,
  dot,
  empty,
  field,
  input,
  modal,
  select,
  table,
  toast,
  type Column,
  type Tone,
} from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import '../styles/agents.css';

const STATUS_TONE: Record<AgentStatus, Tone> = { working: 'info', idle: 'ok', offline: 'muted' };
const WORK_TONE: Record<WorkKind, Tone> = { task: 'accent', chat: 'info', message: 'muted', motion: 'warn' };
const MAX_COUNT = 16;

export function mountAgentList(root: HTMLElement): Cleanup {
  let roleFilter = '';
  let statusFilter: 'all' | AgentStatus = 'all';
  let roles: string[] = [];

  const roleSel = select([{ value: '', label: 'All roles' }]);
  const statusSel = select([
    { value: 'all', label: 'All statuses' },
    { value: 'working', label: 'Working' },
    { value: 'idle', label: 'Idle' },
    { value: 'offline', label: 'Offline' },
  ]);
  const body = h('div');
  const spawnHead = button('Spawn agents', () => openSpawn(), { kind: 'primary', icon: 'plus' });

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
          dot(STATUS_TONE[a.status]),
          h('div', null, h('strong.truncate', null, a.name), h('div.faint.truncate', null, a.role)),
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
    { label: 'Queue', num: true, cell: (a) => a.queue },
    { label: 'Tokens', num: true, cell: (a) => compact(a.usage.input_tokens + a.usage.output_tokens) },
    { label: 'Seen', cell: (a) => ago(a.last_seen_ms) },
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
      replace(body, empty('agents', 'No agents connected', 'Spawn agents to fill the roster.', button('Spawn agents', () => openSpawn(), { kind: 'primary', icon: 'plus' })));
      return;
    }
    const rows = list.filter(
      (a) => (roleFilter === '' || a.role === roleFilter) && (statusFilter === 'all' || a.status === statusFilter),
    );
    if (!rows.length) {
      replace(body, empty('search', 'No agents match', 'Clear the filters to see the roster.', button('Clear filters', clearFilters, { kind: 'ghost' })));
      return;
    }
    replace(body, table(columns, rows, (a) => go('agents', a.cid)));
  }

  function syncRoles(list: Agent[]): void {
    const next = [...new Set(list.map((a) => a.role))].sort();
    if (next.join('\n') === roles.join('\n')) return;
    roles = next;
    if (roleFilter && !roles.includes(roleFilter)) roleFilter = '';
    roleSel.replaceChildren(new Option('All roles', ''), ...roles.map((r) => new Option(r, r)));
    roleSel.value = roleFilter;
  }

  function clearFilters(): void {
    roleFilter = '';
    statusFilter = 'all';
    roleSel.value = '';
    statusSel.value = 'all';
    render(agents.get());
  }

  function openSpawn(): void {
    const roleChoice = select(roles.map((r) => ({ value: r, label: r })));
    const count = input({ type: 'number', min: 1, max: MAX_COUNT, value: 1 });
    const model = select([{ value: '', label: 'Default for the role' }]);
    const send = button('Spawn', () => void submit(), { kind: 'primary', icon: 'plus' });
    let close: () => void = () => {};

    const submit = async (): Promise<void> => {
      const n = Math.trunc(Number(count.value));
      if (!roleChoice.value) {
        toast('Pick a role', 'error');
        return;
      }
      if (!(n >= 1 && n <= MAX_COUNT)) {
        toast(`Count must be 1 to ${MAX_COUNT}`, 'error');
        return;
      }
      send.disabled = true;
      const ok = await attempt(() => api.spawnAgents(roleChoice.value, n, model.value || undefined), 'Spawning…');
      send.disabled = false;
      if (ok === undefined) return;
      close();
      void refreshAgents();
    };

    void attempt(() => api.models()).then((view) => {
      if (!view) return;
      model.append(...view.models.map((m) => new Option(`${m.provider}/${m.id}`, `${m.provider}/${m.id}`)));
    });

    close = modal(
      'Spawn agents',
      h('div.stack', null, field('Role', roleChoice), field('Count', count), field('Model', model), h('div.row.modal-actions', null, send)),
    );
  }
}
