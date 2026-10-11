// The machines: the control plane and the machines it runs on, drawn as a graph;
// the selected machine's specs, GPUs and runs (kill, serial).

import '../styles/ops.css';
import { api, type MachineNode, type Overview, type Run, type Vm } from '../api';
import { on } from '../store';
import { ago, attempt, badge, button, card, empty, type Tone } from '../ui/components';
import { deviceSvg } from '../ui/devices';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

type VmDetail = Vm & { serial?: string };

const SVG_NS = 'http://www.w3.org/2000/svg';

const KIND_LABEL: Record<MachineNode['kind'], string> = {
  server: 'Server',
  workstation: 'Workstation',
  edge: 'Edge',
  apple: 'Apple',
};

// Column label, and whether the column is numeric.
const RUN_COLUMNS: [string, boolean][] = [
  ['ID', false],
  ['Role', false],
  ['State', false],
  ['VMs', true],
  ['vCPU / memory', true],
  ['Created', true],
  ['', false],
];

function stateTone(state: string): Tone {
  if (/run|ready|up|active/i.test(state)) return 'ok';
  if (/start|boot|pend|creat|launch/i.test(state)) return 'info';
  if (/fail|err|crash|kill|dead/i.test(state)) return 'danger';
  return 'muted';
}

const isLive = (runs: Run[]): boolean => runs.some((r) => r.vms.some((vm) => stateTone(vm.state) === 'ok'));

function svgEl(tag: string, attrs: Record<string, string>): SVGElement {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  return el;
}

function controlPlane(o: Overview | null): HTMLElement {
  return h(
    'div.mach-plane',
    null,
    h('div.mach-title', null, 'Control plane'),
    h('div.faint', null, o ? `${o.agents.total} agents · ${o.agents.working} working` : 'Unreachable'),
    h('div.mach-chips', null, badge('apid', o ? 'ok' : 'danger', true), badge('hub', o?.hub ? 'ok' : 'muted', true)),
  );
}

function machineNode(m: MachineNode, live: boolean): HTMLElement {
  const dev = deviceSvg(m.kind, { gpu: m.gpus.length > 0, active: live });
  dev.classList.add('mach-dev');
  return h(
    'div.mach-node',
    { role: 'button', tabindex: '0' },
    h(
      'div.mach-head',
      null,
      dev,
      h('div', null, h('div.mach-name', null, m.name), h('div.faint', null, `${KIND_LABEL[m.kind]} · ${m.arch} · ${m.backend}`)),
    ),
    h('div.faint', null, `${m.cpus} CPUs · ${(m.memory_mib / 1024).toFixed(1)} GiB`),
    m.gpus.length ? h('div.mach-chips', null, m.gpus.map((g) => h('span.mach-chip', null, g.name))) : null,
    m.reachable === false ? badge('unreachable', 'danger', true) : null,
  );
}

function vmList(vms: VmDetail[]): HTMLElement {
  if (!vms.length) return h('p.faint', null, 'No VMs.');
  return h(
    'div.vms',
    null,
    vms.map((vm) =>
      h(
        'div.vm',
        null,
        h('div.row', null, h('strong', null, `VM ${vm.index}`), badge(vm.state, stateTone(vm.state), true), h('span.faint', null, `guest CID ${vm.guest_cid ?? '—'}`)),
        vm.serial ? h('details', null, h('summary', null, 'Serial'), h('pre.pre', null, vm.serial)) : null,
      ),
    ),
  );
}

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  const svg = svgEl('svg', { class: 'mach-edges' });
  const nodesBox = h('div.mach-nodes');
  const labelsBox = h('div.mach-labels');
  const graphBox = h('div.mach-graph', null, svg, nodesBox, labelsBox);
  const detailBox = h('div');
  root.append(h('div.view-head', null, h('h1', null, 'Machines')), graphBox, detailBox);

  const expanded = new Set<string>();
  let alive = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let machines: MachineNode[] = [];
  let selectedId: string | null = null;
  let laid: { m: MachineNode; el: HTMLElement; live: boolean }[] = [];

  const detailRow = (id: string): HTMLTableRowElement => {
    const cell = h('td', { colspan: RUN_COLUMNS.length }, h('p.faint', null, 'Loading…'));
    const tr = h('tr.detail', null, cell);
    void api.run(id).then(
      (run) => {
        if (tr.isConnected) replace(cell, vmList(run.vms));
      },
      (err: unknown) => {
        if (tr.isConnected) replace(cell, h('p.faint', null, err instanceof Error ? err.message : String(err)));
      },
    );
    return tr;
  };

  const toggle = (id: string, tr: HTMLTableRowElement): void => {
    const next = tr.nextElementSibling;
    if (next && next.classList.contains('detail')) {
      next.remove();
      expanded.delete(id);
    } else {
      expanded.add(id);
      tr.after(detailRow(id));
    }
  };

  const killRun = async (id: string): Promise<void> => {
    await attempt(() => api.killRun(id), `Killed run ${id}`);
    if (alive) void refresh();
  };

  const runsTable = (runs: Run[]): HTMLTableElement => {
    const tbody = h('tbody');
    for (const run of runs) {
      const kill = button(
        'Kill',
        (ev) => {
          ev.stopPropagation();
          void killRun(run.id);
        },
        { kind: 'danger', size: 'sm', icon: 'stop' },
      );
      const tr = h(
        'tr.clickable',
        null,
        h('td', null, run.id),
        h('td', null, run.role ?? '—'),
        h('td', null, badge(run.state, stateTone(run.state), true)),
        h('td', { class: 'num' }, String(run.vms.length)),
        h('td', { class: 'num' }, `${run.vcpus} · ${run.memory_mib} MiB`),
        h('td', { class: 'num' }, ago(run.created_at)),
        h('td', { class: 'num' }, kill),
      );
      tr.addEventListener('click', () => toggle(run.id, tr));
      tbody.append(tr);
      if (expanded.has(run.id)) tbody.append(detailRow(run.id));
    }
    return h(
      'table.table',
      null,
      h('thead', null, h('tr', null, RUN_COLUMNS.map(([label, num]) => h('th', { class: num ? 'num' : '' }, label)))),
      tbody,
    );
  };

  // Edges run from the control plane's right edge to each machine's left edge,
  // measured from the laid-out cards; labels sit at each curve's midpoint.
  const layoutEdges = (): void => {
    const box = graphBox.getBoundingClientRect();
    svg.setAttribute('viewBox', `0 0 ${box.width} ${box.height}`);
    svg.replaceChildren();
    labelsBox.replaceChildren();
    const plane = nodesBox.querySelector<HTMLElement>('.mach-plane');
    if (!plane) return;
    const pr = plane.getBoundingClientRect();
    const x1 = pr.right - box.left;
    const y1 = pr.top + pr.height / 2 - box.top;
    for (const { m, el, live } of laid) {
      const nr = el.getBoundingClientRect();
      const x2 = nr.left - box.left;
      const y2 = nr.top + nr.height / 2 - box.top;
      const dx = (x2 - x1) / 2;
      svg.append(svgEl('path', { d: `M${x1},${y1} C${x1 + dx},${y1} ${x2 - dx},${y2} ${x2},${y2}`, class: live ? 'mach-edge live' : 'mach-edge' }));
      const vms = (m.runs ?? []).reduce((n, r) => n + r.vms.length, 0);
      const label = h('div.mach-label', null, `${m.agents ?? 0} agents · ${vms} VMs`);
      label.style.left = `${(x1 + x2) / 2}px`;
      label.style.top = `${(y1 + y2) / 2}px`;
      labelsBox.append(label);
    }
  };

  const select = (id: string): void => {
    selectedId = id;
    for (const { m, el } of laid) el.classList.toggle('selected', m.id === id);
    renderDetail();
  };

  const renderGraph = (o: Overview | null): void => {
    laid = machines.map((m) => {
      const live = isLive(m.runs ?? []);
      const el = machineNode(m, live);
      el.classList.toggle('selected', m.id === selectedId);
      el.addEventListener('click', () => select(m.id));
      return { m, el, live };
    });
    const col = h('div.mach-col');
    if (laid.length) col.append(...laid.map((l) => l.el));
    else col.append(empty('machine', 'No machines.'));
    nodesBox.replaceChildren(controlPlane(o), col);
    requestAnimationFrame(layoutEdges);
  };

  const renderDetail = (): void => {
    const node = machines.find((m) => m.id === selectedId);
    if (!node) {
      detailBox.replaceChildren();
      return;
    }
    const dl = h('dl.kv');
    const rows: [string, string][] = [
      ['Kind', KIND_LABEL[node.kind]],
      ['Class', node.class],
      ['Arch', node.arch],
      ['Backend', node.backend],
      ['Irqchip', node.irqchip],
      ['CPUs', String(node.cpus)],
      ['Memory', `${(node.memory_mib / 1024).toFixed(1)} GiB`],
      ['vsock', node.vsock_mode],
      ['Agents', String(node.agents ?? 0)],
      ['noded port', node.noded_port === undefined ? '—' : String(node.noded_port)],
    ];
    for (const [k, v] of rows) dl.append(h('dt', null, k), h('dd', null, v));

    const gpus = h('div.mach-gpus');
    if (node.gpus.length) {
      for (const g of node.gpus) {
        const detail = [g.vendor, g.memory_mib ? `${(g.memory_mib / 1024).toFixed(1)} GiB` : null].filter(Boolean).join(' · ');
        gpus.append(h('div.row', null, h('strong', null, g.name), h('span.faint', null, detail)));
      }
    } else {
      gpus.append(h('p.faint', null, 'No GPUs.'));
    }

    const runs = node.runs ?? [];
    replace(
      detailBox,
      h(
        'div.mach-detail',
        null,
        card(node.name, dl),
        card('GPUs', gpus),
        card('Runs', runs.length ? runsTable(runs) : empty('machine', 'No runs.'), { flush: true }),
      ),
    );
  };

  const refresh = async (): Promise<void> => {
    try {
      const [o, nodes] = await Promise.all([api.overview().catch(() => null), api.machines()]);
      if (!alive) return;
      machines = nodes;
      if (!nodes.some((m) => m.id === selectedId)) selectedId = nodes[0]?.id ?? null;
      const runIds = new Set(nodes.flatMap((m) => (m.runs ?? []).map((r) => r.id)));
      for (const id of Array.from(expanded)) if (!runIds.has(id)) expanded.delete(id);
      renderGraph(o);
      renderDetail();
    } catch (err) {
      if (!alive) return;
      laid = [];
      svg.replaceChildren();
      labelsBox.replaceChildren();
      nodesBox.replaceChildren(empty('alert', 'Machines unreachable', err instanceof Error ? err.message : String(err)));
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
  const poll = setInterval(() => void refresh(), 10_000);
  const ro = new ResizeObserver(() => layoutEdges());
  ro.observe(graphBox);
  void refresh();
  return () => {
    alive = false;
    offs.forEach((off) => off());
    clearInterval(poll);
    ro.disconnect();
    if (timer !== undefined) clearTimeout(timer);
  };
}
