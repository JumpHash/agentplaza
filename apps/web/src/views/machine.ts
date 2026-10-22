// The machines, as a list: the control plane wired to each machine on top, then one row per
// machine with its picture, facts and load. Opening a row morphs its picture into the live 3D
// view and shows its usage, where its resources go, its runs (kill, serial) and its technical
// details. The topology's machine blocks open their row.

import { api, type Agent, type MachineNode, type Overview, type StationView } from '../api';
import { on, RESYNC } from '../store';
import { attempt, badge, empty } from '../ui/components';
import { h, replace } from '../ui/dom';
import { agentCount, isLive, KIND_LABEL, machineList, machineRow, machineVms, newRunState, normalizeMachines } from './machine-list';
import { stationMachine } from './station-machine';
import type { Cleanup } from './index';

const POLL_MS = 2000;

// The control plane, and the machines it runs on, joined by rules: each machine's block sits on its own row, and
// its rule runs from the plane to the block (the prototype's `.mt-topo`).
function topology(o: Overview | null, machines: MachineNode[], selectedId: string | null, onPick: (id: string) => void): HTMLElement {
  const blocks = machines.map((m) => {
    const live = isLive(m.runs ?? []);
    const state = m.reachable === false ? 'unreachable' : live ? 'running' : 'idle';
    const mark = h('span.mt-mark', null);
    if (live) mark.classList.add('live');
    if (m.reachable === false) mark.classList.add('down');
    const block = h(
      'div.mt-block',
      { role: 'button', tabindex: '0', 'data-key': `block-${m.id}` },
      mark,
      h('strong', null, m.name),
      h('span.mt-faint', null, `${KIND_LABEL[m.kind] ?? 'Machine'} · ${state}`),
    );
    block.classList.toggle('selected', m.id === selectedId);
    block.addEventListener('click', () => onPick(m.id));
    block.addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter' || ev.key === ' ') {
        ev.preventDefault();
        onPick(m.id);
      }
    });
    const wire = h('div.mt-wire', null, h('span', { 'data-key': `wire-${m.id}` }, `${agentCount(m.agents)} agents · ${machineVms(m)} VMs`));
    if (live) wire.classList.add('live');
    return h('div.mt-trow', null, wire, block);
  });

  const plane = h(
    'div.mt-plane',
    null,
    h('p.mt-cap', null, 'Control plane'),
    h(
      'div.mt-chips',
      null,
      badge('apid', o ? 'ok' : 'danger'),
      badge('hub', o?.hub ? 'ok' : 'muted'),
    ),
    h('p.mt-faint', null, o ? `${o.agents.total} agents · ${o.agents.working} working` : 'Unreachable'),
  );
  return h(
    'div.mt-topo',
    null,
    plane,
    machines.length ? h('div.mt-trows', null, blocks) : h('div.mt-trows', null, empty('machine', 'No machines.')),
  );
}

/** The Machines screen's List view (machines.ts): the control plane, then one row per machine. */
export function mountList(root: HTMLElement): Cleanup {
  const topoBox = h('div');
  const list = machineList();
  root.append(topoBox, list.el);

  let alive = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let machines: MachineNode[] = [];
  let overview: Overview | null = null;
  let selectedId: string | null = null;
  let agentsByCid = new Map<number, Agent>(); // names for the agents' VMs, matched by CID

  async function killRun(id: string): Promise<void> {
    await attempt(() => api.killRun(id), `Killed run ${id}`);
    if (alive) void refresh();
  }
  const rs = newRunState((id) => void killRun(id));

  // A topology block opens its machine's row.
  const pick = (id: string): void => {
    selectedId = id;
    replace(topoBox, topology(overview, machines, selectedId, pick));
    list.open(id);
  };

  /** Draws the control plane and the rows from what the hub answered (the copy first, then each fresh read). */
  const paint = (o: Overview | null, fetched: MachineNode[], agentList: Agent[], stations: StationView[]): void => {
    // A partial reply (a machine without its gpus, runs or vms) renders as empty, never as an error.
    // The workstations follow the nodes, in the order the hub returned them.
    const nodes = [...normalizeMachines(fetched), ...(stations ?? []).map(stationMachine)];
    overview = o;
    machines = nodes;
    agentsByCid = new Map<number, Agent>((agentList ?? []).map((a): [number, Agent] => [a.cid, a]));
    if (!nodes.some((m) => m.id === selectedId)) selectedId = nodes[0]?.id ?? null;
    const runIds = new Set(nodes.flatMap((m) => (m.runs ?? []).map((r) => r.id)));
    for (const id of Array.from(rs.expanded)) if (!runIds.has(id)) rs.expanded.delete(id);
    replace(topoBox, topology(o, nodes, selectedId, pick));
    list.setRows(
      nodes.map((m) => machineRow(m, agentsByCid, rs)),
      nodes.length ? null : empty('machine', 'No machines.'),
    );
  };

  const refresh = async (): Promise<void> => {
    try {
      const [o, fetched, agentList, stations] = await Promise.all([
        api.overview().catch(() => null),
        api.machines(),
        api.agents().catch((): Agent[] => []),
        api.stations().catch((): StationView[] => []),
      ]);
      if (!alive) return;
      paint(o, fetched, agentList, stations);
    } catch (err) {
      if (!alive) return;
      replace(topoBox);
      const offline = empty('machine', 'The node is offline', 'Start the node daemon to see its machines and VMs.');
      offline.title = err instanceof Error ? err.message : String(err);
      list.setRows([], offline);
    }
  };

  const schedule = (): void => {
    if (timer !== undefined) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = undefined;
      void refresh();
    }, 500);
  };
  // The last read paints at once; the poll's reads (each a 304 when nothing changed) follow.
  const kept = api.machines.cached();
  if (kept) paint(api.overview.cached() ?? null, kept, api.agents.cached() ?? [], api.stations.cached() ?? []);
  const offs = [on('run', schedule), on('vm', schedule), on(RESYNC, schedule)];
  const poll = setInterval(() => void refresh(), POLL_MS);
  void refresh();
  return () => {
    alive = false;
    offs.forEach((off) => off());
    clearInterval(poll);
    if (timer !== undefined) clearTimeout(timer);
    list.destroy();
  };
}
