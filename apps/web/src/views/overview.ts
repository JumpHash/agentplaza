import { agentName, roleLabel } from '../roles';
// Overview: the plaza at a glance — the orchestrator front and centre, the
// fleet's state, and a live activity feed.

import { api, type Agent, type Entry } from '../api';
import { go, href } from '../router';
import { agents, on, overview } from '../store';
import { ago, attempt, badge, button, card, clip, empty, stat, textarea } from '../ui/components';
import { h, icon, replace } from '../ui/dom';
import type { Cleanup } from './index';

const FEED_MAX = 40;

export function mount(root: HTMLElement): Cleanup {
  const stats = h('div.grid.cols-4');
  const orchestrator = h('div');
  const feed = h('ol.feed');
  const fleet = h('div');

  root.append(
    h('div.view-head', null, h('h1', null, 'Overview')),
    stats,
    h('div.grid.cols-2.overview-main', null, orchestrator, card('Activity', feed, { flush: true })),
    card('Fleet', fleet, { flush: true }),
  );

  const offs = [
    overview.watch((o) => {
      if (!o) return replace(stats, empty('alert', 'apid is unreachable', 'Start apid, then reload.'));
      const open = (o.tasks['open'] ?? 0) + (o.tasks['claimed'] ?? 0);
      replace(
        stats,
        stat('Agents', o.agents.total, `${o.agents.working} working · ${o.agents.offline} offline`),
        stat('Open tasks', open, `${o.tasks['done'] ?? 0} done`),
        stat('Worker model', o.default_model.split('/').pop() ?? '—', o.default_model.split('/')[0]),
        stat('Orchestrator model', o.orchestrator_model.split('/').pop() ?? '—', o.orchestrator_model.split('/')[0]),
      );
    }),
    agents.watch((list) => {
      renderOrchestrator(orchestrator, list.find((a) => a.tier === 'orchestrator'));
      renderFleet(fleet, list);
    }),
    on('transcript', (data) => pushFeed(feed, data as Entry)),
  ];
  replace(feed, h('li.feed-empty.faint', null, 'Waiting for activity…'));
  return () => offs.forEach((off) => off());
}

function renderOrchestrator(root: HTMLElement, orch: Agent | undefined): void {
  if (!orch) {
    replace(root, card('Orchestrator', empty('orchestrator', 'No orchestrator connected', 'Add the orchestrator role to the fleet to delegate work by goal.')));
    return;
  }
  const goal = textarea({ placeholder: 'Tell the orchestrator what you want done…', rows: 3 });
  const send = button('Send', () => submit(), { kind: 'primary', icon: 'send' });
  const submit = async () => {
    const text = goal.value.trim();
    if (!text) return;
    send.disabled = true;
    const ok = await attempt(() => api.chat(orch.cid, text), 'Sent to the orchestrator');
    send.disabled = false;
    if (ok) goal.value = '';
  };
  goal.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) void submit();
  });
  const status = orch.status === 'working' ? badge('Working', 'info', true) : orch.status === 'idle' ? badge('Idle', 'ok', true) : orch.status === 'asleep' ? badge('Asleep', 'muted', true) : badge('Offline', 'muted', true);
  replace(
    root,
    card(
      h('div.row', null, icon('orchestrator'), h('h2', null, agentName(orch)), h('span.faint', null, roleLabel(orch.role)), status),
      h(
        'div.stack',
        null,
        h('p.muted', null, `Plans the work and delegates it to the fleet · ${orch.model}`),
        goal,
        h('div.row', null, h('span.faint', null, 'Ctrl+Enter to send'), h('span', { style: 'flex:1' }), h('a', { href: href('agents', orch.cid) }, 'Open transcript'), send),
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
          h('span.faint.truncate', null, a.work ? clip(a.work.prompt, 60) : a.status === 'offline' ? 'offline' : a.status === 'asleep' ? 'asleep' : roleLabel(a.role)),
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
