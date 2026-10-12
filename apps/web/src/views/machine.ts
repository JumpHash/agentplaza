// The machines, as a register: the control plane wired to each machine, then one
// section per machine with its device, its live usage as tallies, its runs (kill,
// serial) and its technical details, folded away.

import '../styles/ops.css';
import { api, type MachineNode, type Overview, type Run, type Vm } from '../api';
import { on } from '../store';
import { ago, attempt, badge, button, empty, type Tone } from '../ui/components';
import { deviceVisual } from '../ui/devices';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

type Stats = NonNullable<MachineNode['stats']>;
type VmDetail = Vm & { serial?: string };

const SVG_NS = 'http://www.w3.org/2000/svg';
const POLL_MS = 2000;

const KIND_LABEL: Record<MachineNode['kind'], string> = {
  server: 'Server',
  workstation: 'Workstation',
  edge: 'Edge',
  apple: 'Apple',
};

// Column label, and whether the column is numeric.
const RUN_COLUMNS: [string, boolean][] = [
  ['Run', false],
  ['Role', false],
  ['State', false],
  ['VMs', true],
  ['Started', true],
  ['', false],
];

function stateTone(state: string): Tone {
  if (/run|ready|up|active/i.test(state)) return 'ok';
  if (/start|boot|pend|creat|launch/i.test(state)) return 'info';
  if (/fail|err|crash|kill|dead/i.test(state)) return 'danger';
  return 'muted';
}

const isLive = (runs: Run[]): boolean => runs.some((r) => r.vms.some((vm) => stateTone(vm.state) === 'ok'));
const machineVms = (m: MachineNode): number => (m.runs ?? []).reduce((n, r) => n + r.vms.length, 0);
const gib = (mib: number): string => (mib / 1024).toFixed(1);
const clamp = (n: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, n));

function svgEl(tag: string, attrs: Record<string, string>): SVGElement {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  return el;
}

function uptime(s: number): string {
  if (s < 60) return `${Math.floor(s)}s`;
  const mins = Math.floor(s / 60);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 48) return `${hours}h ${mins % 60}m`;
  return `${Math.floor(hours / 24)}d ${hours % 24}h`;
}

function sentence(m: MachineNode): string {
  const head = `${KIND_LABEL[m.kind]} · ${m.cpus}-core ${m.arch} on ${m.backend.toUpperCase()} · ${Math.round(m.memory_mib / 1024)} GiB memory`;
  return m.gpus.length ? `${head} · ${m.gpus.map((g) => g.name).join(', ')}` : head;
}

// The last 60 CPU samples on a small canvas: a ruled baseline and one ink line.
function sparkline(history: Stats['history']): HTMLCanvasElement {
  const W = 240;
  const H = 72;
  const canvas = document.createElement('canvas');
  canvas.className = 'mach-spark';
  canvas.width = W;
  canvas.height = H;
  canvas.setAttribute('aria-hidden', 'true');
  const ctx = canvas.getContext('2d');
  if (!ctx) return canvas;
  const css = getComputedStyle(document.documentElement);
  ctx.strokeStyle = css.getPropertyValue('--rule').trim() || '#999';
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(0, H - 0.5);
  ctx.lineTo(W, H - 0.5);
  ctx.stroke();
  const pts = history.slice(-60);
  if (pts.length < 2) return canvas;
  ctx.strokeStyle = css.getPropertyValue('--ink').trim() || '#111';
  ctx.lineWidth = 2;
  ctx.beginPath();
  pts.forEach((p, i) => {
    const x = (i / (pts.length - 1)) * W;
    const y = H - 4 - (clamp(p.cpu_percent, 0, 100) / 100) * (H - 8);
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  });
  ctx.stroke();
  return canvas;
}

function memBar(s: Stats): HTMLElement {
  const fill = h('div.mach-bar-fill', null);
  fill.style.width = `${clamp((s.mem_used_mib / Math.max(s.mem_total_mib, 1)) * 100, 0, 100)}%`;
  return h('div.mach-bar', null, fill);
}

function tally(label: string, figure: string, caption: string | null, extra?: Node): HTMLElement {
  const fig = h('div.mach-figure', null, figure);
  if (figure.length > 8) fig.classList.add('long');
  return h(
    'div.mach-tally',
    null,
    h('div.mach-tally-label', null, label),
    fig,
    extra ?? null,
    caption === null ? null : h('div.mach-caption', null, caption),
  );
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
    const label = h('div.mach-wire-label', null, `${m.agents ?? 0} agents · ${machineVms(m)} VMs`);
    label.style.top = `${(50 + y) / 2}%`;
    wiresBox.append(label);

    const mark = h('span.mach-mark', null);
    if (live) mark.classList.add('live');
    if (m.reachable === false) mark.classList.add('down');
    const state = m.reachable === false ? 'unreachable' : live ? 'running' : 'idle';
    const block = h(
      'div.mach-block',
      { role: 'button', tabindex: '0' },
      h('div.mach-row', null, mark, h('span.mach-name', null, m.name)),
      h('div.faint', null, `${KIND_LABEL[m.kind]} · ${state}`),
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

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  const topoBox = h('div');
  const sectionsBox = h('div');
  root.append(h('div.view-head', null, h('h1', null, 'Machines')), topoBox, sectionsBox);

  const expanded = new Set<string>(); // runs with their VMs open
  const serialOpen = new Set<string>(); // `${run}/${vm index}` with serial open
  const techOpen = new Set<string>(); // machines with technical details open
  const runCache = new Map<string, Run>(); // last VMs seen per run, so a refresh does not blank them
  let alive = true;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let machines: MachineNode[] = [];
  let overview: Overview | null = null;
  let selectedId: string | null = null;

  const vmList = (runId: string, vms: VmDetail[]): HTMLElement => {
    if (!vms.length) return h('p.faint', null, 'No VMs.');
    return h(
      'div.vms',
      null,
      vms.map((vm) => {
        const key = `${runId}/${vm.index}`;
        let serial: HTMLElement | null = null;
        if (vm.serial) {
          const d = h('details', null, h('summary', null, 'Serial'), h('pre.pre', null, vm.serial)) as HTMLDetailsElement;
          d.open = serialOpen.has(key);
          d.addEventListener('toggle', () => (d.open ? serialOpen.add(key) : serialOpen.delete(key)));
          serial = d;
        }
        return h(
          'div.vm',
          null,
          h(
            'div.row',
            null,
            h('strong', null, `VM ${vm.index}`),
            badge(vm.state, stateTone(vm.state), true),
            h('span.faint', null, `guest CID ${vm.guest_cid ?? '—'}`),
          ),
          serial,
        );
      }),
    );
  };

  const detailRow = (run: Run): HTMLTableRowElement => {
    const cached = runCache.get(run.id);
    const cell = h('td', { colspan: RUN_COLUMNS.length }, cached ? vmList(run.id, cached.vms) : h('p.faint', null, 'Loading…'));
    const tr = h('tr.detail', null, cell);
    void api.run(run.id).then(
      (fresh) => {
        runCache.set(run.id, fresh);
        if (tr.isConnected) replace(cell, vmList(run.id, fresh.vms));
      },
      (err: unknown) => {
        if (tr.isConnected) replace(cell, h('p.faint', null, err instanceof Error ? err.message : String(err)));
      },
    );
    return tr;
  };

  const toggle = (run: Run, tr: HTMLTableRowElement): void => {
    const next = tr.nextElementSibling;
    if (next && next.classList.contains('detail')) {
      next.remove();
      expanded.delete(run.id);
    } else {
      expanded.add(run.id);
      tr.after(detailRow(run));
    }
  };

  const killRun = async (id: string): Promise<void> => {
    await attempt(() => api.killRun(id), `Killed run ${id}`);
    if (alive) void refresh();
  };

  const runsTable = (runs: Run[]): HTMLElement => {
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
        h('td', null, run.role ?? 'fleet'),
        h('td', null, badge(run.state, stateTone(run.state), true)),
        h('td', { class: 'num' }, `${run.vms.length} VMs`),
        h('td', { class: 'num' }, `started ${ago(run.created_at)}`),
        h('td', { class: 'num' }, kill),
      );
      tr.addEventListener('click', () => toggle(run, tr));
      tbody.append(tr);
      if (expanded.has(run.id)) tbody.append(detailRow(run));
    }
    return h(
      'table.table.mach-runs',
      null,
      h('thead', null, h('tr', null, RUN_COLUMNS.map(([label, num]) => h('th', { class: num ? 'num' : '' }, label)))),
      tbody,
    );
  };

  const section = (m: MachineNode): HTMLElement => {
    const runs = m.runs ?? [];
    const dev = deviceVisual(m.kind, { gpu: m.gpus.length > 0, active: isLive(runs) });
    dev.classList.add('mach-dev');
    const s = m.stats;
    const live = s
      ? h(
          'div.mach-live',
          null,
          h(
            'div.mach-tallies',
            null,
            tally('CPU', `${Math.round(s.cpu_percent)}%`, null, sparkline(s.history)),
            tally('Memory', `${gib(s.mem_used_mib)} / ${gib(s.mem_total_mib)}`, 'GiB used of GiB', memBar(s)),
            tally('Load', `${s.load[0].toFixed(1)} · ${s.load[1].toFixed(1)} · ${s.load[2].toFixed(1)}`, '1 / 5 / 15 min'),
            tally("Agents' VMs", String(s.vms_running), `${gib(s.guest_mem_mib)} GiB reserved`),
          ),
          h('p.faint', null, `Up ${uptime(s.uptime_s)}`),
        )
      : h('p.mach-none', null, 'No live stats.');

    const dl = h('dl.kv');
    const rows: [string, string][] = [
      ['Class', m.class],
      ['Arch', m.arch],
      ['Backend', m.backend],
      ['Irqchip', m.irqchip],
      ['vsock', m.vsock_mode],
      ['noded port', m.noded_port === undefined ? '—' : String(m.noded_port)],
    ];
    for (const [k, v] of rows) dl.append(h('dt', null, k), h('dd', null, v));
    const tech = h('details.mach-tech', null, h('summary', null, 'Technical details'), dl) as HTMLDetailsElement;
    tech.open = techOpen.has(m.id);
    tech.addEventListener('toggle', () => (tech.open ? techOpen.add(m.id) : techOpen.delete(m.id)));

    return h(
      'section.mach-section',
      { id: `mach-${m.id}` },
      h('div.mach-figure-box', null, dev),
      h(
        'div.mach-body',
        null,
        h('h2.mach-heading', null, m.name),
        h('p.mach-sentence', null, sentence(m)),
        live,
        h('div.mach-caps', null, 'Runs'),
        runs.length ? runsTable(runs) : h('p.faint', null, 'No runs.'),
        tech,
      ),
    );
  };

  const pick = (id: string): void => {
    selectedId = id;
    topoBox.replaceChildren(topology(overview, machines, selectedId, pick));
    document.getElementById(`mach-${id}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  };

  const refresh = async (): Promise<void> => {
    try {
      const [o, nodes] = await Promise.all([api.overview().catch(() => null), api.machines()]);
      if (!alive) return;
      overview = o;
      machines = nodes;
      if (!nodes.some((m) => m.id === selectedId)) selectedId = nodes[0]?.id ?? null;
      const runIds = new Set(nodes.flatMap((m) => (m.runs ?? []).map((r) => r.id)));
      for (const id of Array.from(expanded)) if (!runIds.has(id)) expanded.delete(id);
      topoBox.replaceChildren(topology(o, nodes, selectedId, pick));
      sectionsBox.replaceChildren(...(nodes.length ? nodes.map(section) : [empty('machine', 'No machines.')]));
    } catch (err) {
      if (!alive) return;
      topoBox.replaceChildren();
      sectionsBox.replaceChildren(empty('alert', 'Machines unreachable', err instanceof Error ? err.message : String(err)));
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
  };
}
