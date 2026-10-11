// The machine: this host's shape, and its runs with their VMs and serial output.

import '../styles/ops.css';
import { api, type Machine, type Run, type Vm } from '../api';
import { on } from '../store';
import { ago, attempt, badge, button, card, empty, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

type VmDetail = Vm & { serial?: string };

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

function machineCard(m: Machine): HTMLElement {
  const dl = h('dl.kv');
  const rows: [string, string][] = [
    ['Class', m.class],
    ['Arch', m.arch],
    ['Backend', m.backend],
    ['Irqchip', m.irqchip],
    ['CPUs', String(m.cpus)],
    ['Memory', `${(m.memory_mib / 1024).toFixed(1)} GiB`],
    ['vsock', m.vsock_mode],
  ];
  for (const [k, v] of rows) dl.append(h('dt', null, k), h('dd', null, v));
  return card(m.name, dl);
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
  const hostBox = h('div');
  const runsBox = h('div');
  root.append(h('div.view-head', null, h('h1', null, 'Machine')), hostBox, card('Runs', runsBox, { flush: true }));

  const expanded = new Set<string>();
  let alive = true;
  let timer: ReturnType<typeof setTimeout> | undefined;

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

  const refreshRuns = async (): Promise<void> => {
    try {
      const runs = await api.runs();
      if (!alive) return;
      for (const id of Array.from(expanded)) if (!runs.some((r) => r.id === id)) expanded.delete(id);
      replace(runsBox, runs.length ? runsTable(runs) : empty('machine', 'No runs.'));
    } catch (err) {
      if (alive) replace(runsBox, empty('alert', 'Runs unreachable', err instanceof Error ? err.message : String(err)));
    }
  };

  const killRun = async (id: string): Promise<void> => {
    await attempt(() => api.killRun(id), `Killed run ${id}`);
    if (alive) void refreshRuns();
  };

  const loadHost = async (): Promise<void> => {
    try {
      const m = await api.machine();
      if (alive) replace(hostBox, machineCard(m));
    } catch (err) {
      if (alive) replace(hostBox, empty('alert', 'Machine unreachable', err instanceof Error ? err.message : String(err)));
    }
  };

  const schedule = (): void => {
    if (timer !== undefined) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = undefined;
      void refreshRuns();
    }, 500);
  };
  const offs = [on('run', schedule), on('vm', schedule)];
  void loadHost();
  void refreshRuns();
  return () => {
    alive = false;
    offs.forEach((off) => off());
    if (timer !== undefined) clearTimeout(timer);
  };
}
