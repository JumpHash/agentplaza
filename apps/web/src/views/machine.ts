// The machines, as a list: the control plane wired to each machine on top, then one row per
// machine with its picture, facts and load. Opening a row morphs its picture into the live 3D
// view and shows its usage, where its resources go, its runs (kill, serial) and its technical
// details. The topology's machine blocks open their row.

import '../styles/ops.css';
import { api, type Agent, type MachineNode, type Overview, type StationView } from '../api';
import { on } from '../store';
import { attempt, badge, empty } from '../ui/components';
import { h, replace } from '../ui/dom';
import { agentCount, isLive, KIND_LABEL, machineList, machineRow, machineVms, newRunState, normalizeMachines } from './machine-list';
import { stationMachine } from './station-machine';
import type { Cleanup } from './index';

const SVG_NS = 'http://www.w3.org/2000/svg';
const POLL_MS = 2000;

function svgEl(tag: string, attrs: Record<string, string>): SVGElement {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  return el;
}

// The control plane, and the machines it runs on, joined by straight rules.
// Each machine's block sits on its own row; its rule ends at the row's centre.
function topology(o: Overview | null, machines: MachineNode[], selectedId: string | null, onPick: (id: string) => void): HTMLElement {
  const wires = svgEl('svg', { class: 'mach-wires', viewBox: '0 0 100 100', preserveAspectRatio: 'none', 'aria-hidden': 'true' });
  const wiresBox = h('div.mach-wires-box', null, wires);
  const fleet = h('div.mach-fleet');
  fleet.style.gridTemplateRows = `repeat(${Math.max(machines.length, 1)}, minmax(44px, 1fr))`;
  if (!machines.length) fleet.append(empty('machine', 'No machines.'));

  machines.forEach((m, i) => {
    const live = isLive(m.runs ?? []);
    const y = ((i + 0.5) / machines.length) * 100;
    wires.append(
      svgEl('line', {
        x1: '0',
        y1: '50',
        x2: '100',
        y2: String(y),
        class: live ? 'mach-wire live' : 'mach-wire',
        'vector-effect': 'non-scaling-stroke',
      }),
    );
    const label = h('div.mach-wire-label', { 'data-key': `wire-${m.id}` }, `${agentCount(m.agents)} agents · ${machineVms(m)} VMs`);
    label.style.top = `${(50 + y) / 2}%`;
    wiresBox.append(label);

    const mark = h('span.mach-mark', null);
    if (live) mark.classList.add('live');
    if (m.reachable === false) mark.classList.add('down');
    const state = m.reachable === false ? 'unreachable' : live ? 'running' : 'idle';
    const block = h(
      'div.mach-block',
      { role: 'button', tabindex: '0', 'data-key': `block-${m.id}` },
      h('div.mach-row', null, mark, h('span.mach-name', null, m.name)),
      h('div.faint', null, `${KIND_LABEL[m.kind] ?? 'Machine'} · ${state}`),
    );
    block.classList.toggle('selected', m.id === selectedId);
    block.addEventListener('click', () => onPick(m.id));
    block.addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter') onPick(m.id);
    });
    fleet.append(block);
  });

  const plane = h(
    'div.mach-plane',
    null,
    h('div.mach-caps', null, 'Control plane'),
    h('div.faint', null, o ? `${o.agents.total} agents · ${o.agents.working} working` : 'Unreachable'),
    h('div.mach-chips', null, badge('apid', o ? 'ok' : 'danger', true), badge('hub', o?.hub ? 'ok' : 'muted', true)),
  );
  return h('div.mach-topo', null, plane, wiresBox, fleet);
}

/** The Machines screen's List view (machines.ts): the control plane, then one row per machine. `openId` opens that row. */
export function mountList(root: HTMLElement, openId: string | null): Cleanup {
  const topoBox = h('div');
  const list = machineList();
  root.append(topoBox, list.el);

  let alive = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let machines: MachineNode[] = [];
  let overview: Overview | null = null;
  let selectedId: string | null = openId;
  // The row opens once it has loaded: the list keeps a wanted row until its rows arrive.
  if (openId !== null) list.open(openId);
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

  const refresh = async (): Promise<void> => {
    try {
      const [o, fetched, agentList, stations] = await Promise.all([
        api.overview().catch(() => null),
        api.machines(),
        api.agents().catch((): Agent[] => []),
        api.stations().catch((): StationView[] => []),
      ]);
      if (!alive) return;
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
  const offs = [on('run', schedule), on('vm', schedule)];
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
