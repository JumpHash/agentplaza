// The machines as a list, shared by the Machines and Nodes screens. A row shows the machine's
// picture (a 160 x 120 thumbnail) with its facts and load bars. Opening a row grows its panel
// open and morphs the picture into the live 3D stage; closing reverses it. One row is open at a
// time. The stage is a singleton: only the open row holds its canvas.

import '../styles/ops.css';
import '../styles/machine-list.css';
import { api, type Agent, type FleetApple, type MachineNode, type ProcessUsage, type Run, type Vm, type VmUsage } from '../api';
import { agentName, roleLabel } from '../roles';
import { ago, badge, button, type Tone } from '../ui/components';
import { deviceVisual } from '../ui/devices';
import { h, icon, replace } from '../ui/dom';
import { lookOf } from './floor3d/machine-model';
import { getStage, specKey, THUMB_H, THUMB_W, thumbnail, type PictureSpec, type Stage } from './floor3d/machine-stage';

export type Stats = NonNullable<MachineNode['stats']>;
type VmDetail = Vm & { serial?: string };
// One segment of a stacked bar: services, agents, the rest of the system, or what is free.
type Part = { label: string; value: number; cls: 'services' | 'agents' | 'rest' | 'free' };
// The Agents table's sort: by CPU or host memory, and its direction (-1 descending).
type AgentSort = { key: 'cpu' | 'mem'; dir: 1 | -1 };

export const KIND_LABEL: Record<MachineNode['kind'], string> = {
  server: 'Server',
  workstation: 'Workstation',
  laptop: 'Laptop',
  edge: 'Edge',
  phone: 'Phone',
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

const DEFAULT_SORT: AgentSort = { key: 'cpu', dir: -1 };
// Each machine's Agents-table sort, kept across the refreshes.
const agentSort = new Map<string, AgentSort>();

export function stateTone(state: string): Tone {
  if (/run|ready|up|active/i.test(state)) return 'ok';
  if (/start|boot|pend|creat|launch/i.test(state)) return 'info';
  if (/fail|err|crash|kill|dead/i.test(state)) return 'danger';
  return 'muted';
}

export const isLive = (runs: Run[]): boolean => runs.some((r) => r.vms.some((vm) => stateTone(vm.state) === 'ok'));
export const machineVms = (m: MachineNode): number => (m.runs ?? []).reduce((n, r) => n + r.vms.length, 0);
/** A machine's agents: the count, or (on the stand-in hub) the array of their CIDs. */
export const agentCount = (a: unknown): number => (Array.isArray(a) ? a.length : typeof a === 'number' ? a : 0);
export const gib = (mib: number): string => (mib / 1024).toFixed(1);
export const human = (mib: number): string => (mib >= 1024 ? `${gib(mib)} GiB` : `${Math.round(mib)} MiB`);
const pct = (n: number): string => `${n.toFixed(1)}%`;
const clamp = (n: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, n));

export function uptime(s: number): string {
  if (s < 60) return `${Math.floor(s)}s`;
  const mins = Math.floor(s / 60);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 48) return `${hours}h ${mins % 60}m`;
  return `${Math.floor(hours / 24)}d ${hours % 24}h`;
}

// "Apple M3 Max · 16 cores (12P + 4E) · 40-core GPU · 64 GB unified".
function chipLine(a: FleetApple): string {
  return `${a.chip} · ${a.p_cores + a.e_cores} cores (${a.p_cores}P + ${a.e_cores}E) · ${a.gpu_cores}-core GPU · ${a.unified_memory_gib} GB unified`;
}

export function sentence(m: MachineNode): string {
  const chip = m.apple
    ? chipLine(m.apple)
    : `${m.cpus}-core ${m.arch} on ${m.backend.toUpperCase()} · ${Math.round(m.memory_mib / 1024)} GiB memory`;
  const head = [KIND_LABEL[m.kind], m.model, chip].filter(Boolean).join(' · ');
  return m.gpus.length ? `${head} · ${m.gpus.map((g) => g.name).join(', ')}` : head;
}

/** Fills the partial replies (a machine without its gpus, runs or vms) as empty lists, never as errors. */
export function normalizeMachines(fetched: MachineNode[] | null): MachineNode[] {
  return (fetched ?? []).map((m) => ({
    ...m,
    gpus: m.gpus ?? [],
    runs: (m.runs ?? []).map((r) => ({ ...r, vms: r.vms ?? [] })),
  }));
}

/** The picture a machine is drawn as: the same spec the stage and the thumbnails take. */
export function specOf(m: MachineNode): PictureSpec {
  return { kind: m.kind, vendor: m.vendor ?? null, memory_mib: m.memory_mib, gpus: m.gpus.map((g) => ({ vendor: g.vendor })) };
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

// ---- Where the resources go -------------------------------------------------

// A bar that is 100% of the machine (or of its total when the parts overrun it), with a legend.
function stackedBar(title: string, parts: Part[], whole: number, fmt: (v: number) => string): HTMLElement {
  const sum = parts.reduce((n, p) => n + p.value, 0);
  const span = Math.max(whole, sum, 1);
  const bar = h(
    'div.res-bar',
    null,
    parts.map((p) => {
      const seg = h(`span.res-seg.res-${p.cls}`, { title: `${p.label}: ${fmt(p.value)}`, 'data-key': `seg-${p.cls}` });
      seg.style.width = `${clamp((p.value / span) * 100, 0, 100)}%`;
      return seg;
    }),
  );
  const legend = h(
    'ul.res-legend',
    null,
    parts.map((p) => h('li', { 'data-key': `part-${p.cls}` }, h(`span.res-swatch.res-${p.cls}`, null), h('span', null, p.label), h('strong', null, fmt(p.value)))),
  );
  return h('div.res-row', null, h('div.res-title', null, title), bar, legend);
}

function servicesTable(rows: ProcessUsage[]): HTMLElement {
  return h(
    'div.res-scroll',
    null,
    h(
      'table.table.res-table',
      null,
      h(
        'thead',
        null,
        h('tr', null, h('th', null, 'Service'), h('th', { class: 'num' }, 'PID'), h('th', { class: 'num' }, 'CPU'), h('th', { class: 'num' }, 'Memory')),
      ),
      h(
        'tbody',
        null,
        rows.map((p) =>
          h('tr', { 'data-key': `svc-${p.name}` }, h('td', null, p.name), h('td', { class: 'num res-pid' }, String(p.pid)), h('td', { class: 'num' }, pct(p.cpu_percent)), h('td', { class: 'num' }, human(p.rss_mib))),
        ),
      ),
    ),
  );
}

// A VM's guest memory as "used / usable", with a thin bar of the share used.
function guestCell(v: VmUsage): HTMLElement {
  const used = v.guest_used_mib;
  const usable = v.usable_mib;
  const text = `${used === null ? '—' : human(used)} / ${usable === null ? '—' : human(usable)}`;
  let mini: HTMLElement | null = null;
  if (used !== null && usable !== null && usable > 0) {
    const fill = h('div.mach-bar-fill', null);
    fill.style.width = `${clamp((used / usable) * 100, 0, 100)}%`;
    mini = h('div.mach-bar.res-mini', null, fill);
  }
  return h('div.res-guest', null, h('span', null, text), mini);
}

function agentsTable(m: MachineNode, vms: VmUsage[], byCid: Map<number, Agent>, onSort: (key: AgentSort['key']) => void): HTMLElement {
  const sort: AgentSort = agentSort.get(m.id) ?? DEFAULT_SORT;
  const key = (v: VmUsage): number => (sort.key === 'cpu' ? v.cpu_percent : v.rss_mib);
  const rows = [...vms].sort((a, b) => sort.dir * (key(a) - key(b)));

  const head = (label: string, k: AgentSort['key']): HTMLElement => {
    const active = sort.key === k;
    const btn = h('button.res-sort', { type: 'button' }, active ? `${label} ${sort.dir === -1 ? '↓' : '↑'}` : label);
    btn.addEventListener('click', () => onSort(k));
    return h('th', { class: 'num', 'aria-sort': active ? (sort.dir === -1 ? 'descending' : 'ascending') : 'none' }, btn);
  };

  const body = rows.map((v) => {
    const agent = v.cid === null ? undefined : byCid.get(v.cid);
    const role = agent?.role ?? v.role;
    const name = agent ? agentName(agent) : v.cid === null ? 'Unattached' : `CID ${v.cid}`;
    return h(
      'tr',
      { 'data-key': `vm-${v.run}-${v.vm}` },
      h('td', null, h('div', null, name), h('div.faint', null, role ? roleLabel(role) : '—')),
      h('td', { class: 'res-pid' }, `${v.run}/${v.vm}`),
      h('td', { class: 'num' }, pct(v.cpu_percent)),
      h('td', { class: 'num' }, human(v.rss_mib)),
      h('td', { class: 'num' }, guestCell(v)),
    );
  });

  const sumCpu = vms.reduce((n, v) => n + v.cpu_percent, 0);
  const sumRss = vms.reduce((n, v) => n + v.rss_mib, 0);
  const guestReported = vms.filter((v) => v.guest_used_mib !== null);
  const guestUsed = guestReported.reduce((n, v) => n + (v.guest_used_mib ?? 0), 0);
  const guestUsable = vms.filter((v) => v.usable_mib !== null).reduce((n, v) => n + (v.usable_mib ?? 0), 0);
  const guestTotal = guestReported.length ? `${human(guestUsed)} / ${human(guestUsable)}` : '—';

  return h(
    'div.res-scroll',
    null,
    h(
      'table.table.res-table',
      null,
      h(
        'thead',
        null,
        h('tr', null, h('th', null, 'Agent'), h('th', null, 'VM'), head('CPU', 'cpu'), head('Host memory', 'mem'), h('th', { class: 'num' }, 'Guest used / usable')),
      ),
      h('tbody', null, body),
      h(
        'tfoot',
        null,
        h(
          'tr',
          { class: 'res-total' },
          h('td', null, `${vms.length} VMs`),
          h('td', null),
          h('td', { class: 'num' }, pct(sumCpu)),
          h('td', { class: 'num' }, human(sumRss)),
          h('td', { class: 'num' }, guestTotal),
        ),
      ),
    ),
  );
}

// The panel between a machine's tallies and its runs: CPU and memory split four ways, the
// services that run the plaza, and the agents' VMs. Sorting the Agents table rebuilds it.
function resourcesPanel(m: MachineNode, byCid: Map<number, Agent>): HTMLElement {
  const s = m.stats;
  const plaza = s?.plaza;
  if (!s || !plaza) {
    return h('div.res', null, h('div.mach-caps', null, 'Where the resources go'), h('p.faint', null, "This node doesn't report a breakdown yet."));
  }
  const t = plaza.total;
  const cpuParts: Part[] = [
    { label: 'Plaza services', value: t.services_cpu_percent, cls: 'services' },
    { label: 'Agents', value: t.vms_cpu_percent, cls: 'agents' },
    { label: 'Rest of the system', value: Math.max(0, s.cpu_percent - (t.services_cpu_percent + t.vms_cpu_percent)), cls: 'rest' },
    { label: 'Free', value: Math.max(0, 100 - s.cpu_percent), cls: 'free' },
  ];
  const memParts: Part[] = [
    { label: 'Plaza services', value: t.services_rss_mib, cls: 'services' },
    { label: 'Agents', value: t.vms_rss_mib, cls: 'agents' },
    { label: 'Rest of the system', value: Math.max(0, s.mem_used_mib - (t.services_rss_mib + t.vms_rss_mib)), cls: 'rest' },
    { label: 'Free', value: Math.max(0, s.mem_total_mib - s.mem_used_mib), cls: 'free' },
  ];

  let root: HTMLElement | undefined;
  const onSort = (k: AgentSort['key']): void => {
    const cur: AgentSort = agentSort.get(m.id) ?? DEFAULT_SORT;
    agentSort.set(m.id, cur.key === k ? { key: k, dir: cur.dir === -1 ? 1 : -1 } : { key: k, dir: -1 });
    if (root) root.replaceWith(resourcesPanel(m, byCid));
  };

  root = h(
    'div.res',
    null,
    h('div.mach-caps', null, 'Where the resources go'),
    stackedBar('CPU', cpuParts, 100, pct),
    stackedBar('Memory', memParts, s.mem_total_mib, (v) => `${gib(v)} GiB`),
    h('div.mach-caps.res-sub', null, 'Services'),
    servicesTable(plaza.services),
    h('div.mach-caps.res-sub', null, 'Agents'),
    plaza.vms.length ? agentsTable(m, plaza.vms, byCid, onSort) : h('p.faint', null, 'No agent VMs on this node.'),
  );
  return root;
}

// ---- Runs and their state, held by the screen so a refresh keeps them -------------

export interface RunState {
  /** Runs with their VMs open. */
  expanded: Set<string>;
  /** `${run}/${vm index}` with serial open. */
  serialOpen: Set<string>;
  /** Machines with technical details open. */
  techOpen: Set<string>;
  /** Last VMs seen per run, so a refresh does not blank them. */
  runCache: Map<string, Run>;
  /** Kills a run; the screen refreshes after. */
  kill: (id: string) => void;
}

export function newRunState(kill: (id: string) => void): RunState {
  return { expanded: new Set(), serialOpen: new Set(), techOpen: new Set(), runCache: new Map(), kill };
}

function vmList(runId: string, vms: VmDetail[], rs: RunState): HTMLElement {
  if (!vms.length) return h('p.faint', null, 'No VMs.');
  return h(
    'div.vms',
    null,
    vms.map((vm) => {
      const key = `${runId}/${vm.index}`;
      let serial: HTMLElement | null = null;
      if (vm.serial) {
        const d = h('details', null, h('summary', null, 'Serial'), h('pre.pre', null, vm.serial)) as HTMLDetailsElement;
        d.open = rs.serialOpen.has(key);
        d.addEventListener('toggle', () => (d.open ? rs.serialOpen.add(key) : rs.serialOpen.delete(key)));
        serial = d;
      }
      return h(
        'div.vm',
        { 'data-key': `vm-${vm.index}` },
        h('div.row', null, h('strong', null, `VM ${vm.index}`), badge(vm.state, stateTone(vm.state), true), h('span.faint', null, `guest CID ${vm.guest_cid ?? '—'}`)),
        serial,
      );
    }),
  );
}

function detailRow(run: Run, rs: RunState): HTMLTableRowElement {
  const cached = rs.runCache.get(run.id);
  const cell = h('td', { colspan: RUN_COLUMNS.length }, cached ? vmList(run.id, cached.vms, rs) : h('p.faint', null, 'Loading…'));
  const tr = h('tr.detail', { 'data-key': `detail-${run.id}` }, cell);
  void api.run(run.id).then(
    (fresh) => {
      rs.runCache.set(run.id, fresh);
      if (tr.isConnected) replace(cell, vmList(run.id, fresh.vms, rs));
    },
    (err: unknown) => {
      if (tr.isConnected) replace(cell, h('p.faint', null, err instanceof Error ? err.message : String(err)));
    },
  );
  return tr;
}

function toggleRun(run: Run, tr: HTMLTableRowElement, rs: RunState): void {
  const next = tr.nextElementSibling;
  if (next && next.classList.contains('detail')) {
    next.remove();
    rs.expanded.delete(run.id);
  } else {
    rs.expanded.add(run.id);
    tr.after(detailRow(run, rs));
  }
}

function runsTable(runs: Run[], rs: RunState): HTMLElement {
  const tbody = h('tbody');
  for (const run of runs) {
    const kill = button(
      'Kill',
      (ev) => {
        ev.stopPropagation();
        rs.kill(run.id);
      },
      { kind: 'danger', size: 'sm', icon: 'stop' },
    );
    const tr = h(
      'tr.clickable',
      { 'data-key': `run-${run.id}` },
      h('td', null, run.id),
      h('td', null, run.role ?? 'fleet'),
      h('td', null, badge(run.state, stateTone(run.state), true)),
      h('td', { class: 'num' }, `${run.vms.length} VMs`),
      h('td', { class: 'num' }, `started ${ago(run.created_at)}`),
      h('td', { class: 'num' }, kill),
    );
    tr.addEventListener('click', () => toggleRun(run, tr, rs));
    tbody.append(tr);
    if (rs.expanded.has(run.id)) tbody.append(detailRow(run, rs));
  }
  return h(
    'table.table.mach-runs',
    null,
    h('thead', null, h('tr', null, RUN_COLUMNS.map(([label, num]) => h('th', { class: num ? 'num' : '' }, label)))),
    tbody,
  );
}

/** A machine's detail: its sentence, live tallies, where the resources go, runs and technical details. */
export function machineDetail(m: MachineNode, byCid: Map<number, Agent>, rs: RunState): HTMLElement {
  const runs = m.runs ?? [];
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
  tech.open = rs.techOpen.has(m.id);
  tech.addEventListener('toggle', () => (tech.open ? rs.techOpen.add(m.id) : rs.techOpen.delete(m.id)));

  return h(
    'div.mlist-machine',
    null,
    h('p.mach-sentence', null, sentence(m)),
    live,
    resourcesPanel(m, byCid),
    h('div.mach-caps', null, 'Runs'),
    runs.length ? runsTable(runs, rs) : h('p.faint', null, 'No runs.'),
    tech,
  );
}

// ---- The list ----------------------------------------------------------------

function gpuText(gpus: MachineNode['gpus']): string {
  if (!gpus.length) return 'None';
  const vendors = [...new Set(gpus.map((g) => g.vendor))];
  return `${gpus.length} · ${vendors.join(', ')}`;
}

/** A machine's row on the Machines screen (and a node's, once joined to its machine). */
export function machineRow(m: MachineNode, byCid: Map<number, Agent>, rs: RunState): ListRow {
  const runs = m.runs ?? [];
  const live = isLive(runs);
  const s = m.stats;
  const status: ListRow['status'] = m.reachable === false ? { label: 'unreachable', tone: 'danger' } : live ? { label: 'running', tone: 'ok' } : { label: 'idle', tone: 'muted' };
  const bars: ListBar[] = [];
  if (s) {
    bars.push({ label: 'CPU', pct: clamp(s.cpu_percent, 0, 100), text: pct(s.cpu_percent) });
    bars.push({
      label: 'Mem',
      pct: clamp((s.mem_used_mib / Math.max(s.mem_total_mib, 1)) * 100, 0, 100),
      text: `${gib(s.mem_used_mib)} / ${gib(s.mem_total_mib)} GiB`,
    });
    bars.push({ label: 'Load', pct: clamp((s.load[0] / Math.max(m.cpus, 1)) * 100, 0, 100), text: s.load.map((x) => x.toFixed(1)).join(' · ') });
  }
  const facts: [string, string][] = [
    ['Platform', m.apple ? `macOS · ${m.arch}` : m.arch],
    ['CPU', m.apple ? `${m.apple.chip} · ${m.cpus} cores` : `${m.cpus} cores`],
    ['Memory', `${gib(m.memory_mib)} GiB`],
    ['GPUs', gpuText(m.gpus)],
    ['Agents', `${agentCount(m.agents)} · ${machineVms(m)} VMs`],
  ];
  return {
    key: m.id,
    name: m.name,
    subtitle: [KIND_LABEL[m.kind], m.model].filter(Boolean).join(' · '),
    status,
    facts,
    bars,
    spec: specOf(m),
    active: live,
    actions: null,
    detail: () => machineDetail(m, byCid, rs),
  };
}

/** A load bar: `pct` runs 0 to 100 and `text` is what it reads. */
export interface ListBar {
  label: string;
  pct: number;
  text: string;
}

/** One row. `spec` is the picture (null when the kind is unknown: a generic icon is shown). */
export interface ListRow {
  key: string;
  name: string;
  subtitle: string | null;
  status: { label: string; tone: Tone };
  facts: [string, string][];
  bars: ListBar[];
  spec: PictureSpec | null;
  /** The machine is running VMs (its line-art fallback shows LEDs lit). */
  active: boolean;
  /** Inline actions under the row (nodes: Drain and Revoke). */
  actions: Node | null;
  /** The expanded panel's right column; rebuilt on every refresh while the row is open. */
  detail: () => Node;
}

export interface MachineList {
  readonly el: HTMLElement;
  /** Replaces the rows in place, keyed by `key`. `fallback` shows when there are none. Deferred while a morph runs. */
  setRows(rows: ListRow[], fallback?: Node | null): void;
  /** Opens the row with `key` (closing the open one first), or closes the open row when null. */
  open(key: string | null): void;
  /** Cancels frames and timers, releases the stage and removes its canvas. */
  destroy(): void;
}

interface Item {
  row: ListRow;
  key: string;
  el: HTMLElement;
  summary: HTMLElement;
  pic: HTMLElement;
  body: HTMLElement;
  tail: HTMLElement;
  panel: HTMLElement;
  inner: HTMLElement;
  /** The spec key the row wants: undefined before the first paint, null when it has no spec. */
  want: string | null | undefined;
  /** The spec key the picture shows, once painted. */
  picKey: string | null;
  queued: boolean;
  visible: boolean;
  /** This row's panel holds the stage canvas. */
  onStage: boolean;
  gone: boolean;
}

/** A box in the list's own coordinates. */
interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface Anim {
  kind: 'open' | 'close';
  it: Item;
  /** The canvas flies between the thumbnail and the stage slot (false: only the panel grows). */
  flight: boolean;
  pic: Box;
  slot: Box;
  h0: number;
  h1: number;
  t0: number;
}

const DURATION_MS = 600;
const thumbs = new Map<string, string | null>();
const prepared = new Set<string>();

const pixelRatio = (): number => Math.min(2, Math.max(1, window.devicePixelRatio || 1));
const reducedMotion = (): boolean => window.matchMedia?.('(prefers-reduced-motion: reduce)').matches === true;
const lerp = (a: number, b: number, t: number): number => a + (b - a) * t;
const easeInOutCubic = (t: number): number => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);

/** The line art of a row with no picture of its own: the device drawing, or a generic icon when the kind is unknown. */
function fallbackPicture(row: ListRow): Node {
  if (!row.spec) return h('span.mlist-unknown', null, icon('machine'));
  return deviceVisual(lookOf(row.spec.kind, row.spec.vendor), { gpu: row.spec.gpus.length > 0, active: row.active });
}

/** Paints the thumbnail into the picture box: the PNG when WebGL renders it, else the line art. */
function paintThumb(it: Item): void {
  const spec = it.row.spec;
  if (!spec) return;
  const k = specKey(spec);
  if (it.picKey === k) return;
  const pr = pixelRatio();
  const ck = `${k}@${pr}`;
  let url = thumbs.get(ck);
  if (url === undefined) {
    url = thumbnail(spec, pr);
    thumbs.set(ck, url);
  }
  it.picKey = k;
  if (url) replace(it.pic, h('img', { src: url, width: THUMB_W, height: THUMB_H, alt: '', draggable: 'false' }));
  else replace(it.pic, fallbackPicture(it.row));
}

function prepare(spec: PictureSpec): void {
  const k = specKey(spec);
  if (prepared.has(k)) return;
  prepared.add(k);
  getStage()?.prepare(spec);
}

function placeCanvas(c: HTMLCanvasElement, x: number, y: number, w: number, h: number, transform: string, interactive: boolean): void {
  const s = c.style;
  s.position = 'absolute';
  s.left = `${x}px`;
  s.top = `${y}px`;
  s.width = `${w}px`;
  s.height = `${h}px`;
  s.transformOrigin = '0 0';
  s.transform = transform;
  s.pointerEvents = interactive ? 'auto' : 'none';
  s.zIndex = '2';
}

function bodyOf(row: ListRow): (Node | null)[] {
  return [
    h('div.mlist-head', null, h('strong.mlist-name', null, row.name), badge(row.status.label, row.status.tone, true)),
    row.subtitle ? h('div.mlist-sub', null, row.subtitle) : null,
    h(
      'div.mlist-facts',
      null,
      row.facts.map(([label, value]) => h('div.mlist-fact', null, h('span', null, label), h('span', null, value))),
    ),
    row.bars.length
      ? h(
          'div.mlist-bars',
          null,
          row.bars.map((b) => {
            const fill = h('div.mach-bar-fill', null);
            fill.style.width = `${clamp(b.pct, 0, 100)}%`;
            return h('div.mlist-bar', null, h('span', null, b.label), h('div.mach-bar', null, fill), h('span', null, b.text));
          }),
        )
      : null,
  ];
}

export function machineList(): MachineList {
  const root = h('div.mlist');
  const rowsBox = h('div.mlist-rows');
  const emptyBox = h('div.mlist-empty');
  root.append(rowsBox, emptyBox);

  const items = new Map<string, Item>();
  const byPic = new WeakMap<Element, Item>();
  const queue: Item[] = [];
  let pumpWanted = false;
  let latest: ListRow[] = [];
  let fallback: Node | null = null;
  let dirty = false;
  // The row the reader wants open (null: none), and the row that is open or closing.
  let desired: string | null = null;
  let openKey: string | null = null;
  let anim: Anim | null = null;
  // The stage, while a row holds its canvas, and the open row's stage slot.
  let held: Stage | null = null;
  let openBox: Box | null = null;
  let raf = 0;
  let lastNow = 0;
  let destroyed = false;
  let uid = 0;

  const rel = (el: Element): Box => {
    const r = el.getBoundingClientRect();
    const c = root.getBoundingClientRect();
    return { x: r.left - c.left - root.clientLeft, y: r.top - c.top - root.clientTop, w: r.width, h: r.height };
  };

  // ---- thumbnails: lazy, at most two per frame, paused during a morph ----

  const schedulePump = (): void => {
    if (pumpWanted || destroyed) return;
    pumpWanted = true;
    requestAnimationFrame(pump);
  };

  const wantThumb = (it: Item): void => {
    const spec = it.row.spec;
    if (!spec || it.queued || it.picKey === specKey(spec)) return;
    it.queued = true;
    queue.push(it);
    schedulePump();
  };

  function pump(): void {
    pumpWanted = false;
    if (destroyed || anim) return; // settle() schedules the queue again when the morph ends
    for (let n = 0; n < 2 && queue.length; n++) {
      const it = queue.shift();
      if (!it) break;
      it.queued = false;
      if (!it.gone) paintThumb(it);
    }
    if (queue.length) schedulePump();
  }

  const io = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        const it = byPic.get(e.target);
        if (!it) continue;
        it.visible = e.isIntersecting;
        if (it.visible) wantThumb(it);
      }
    },
    { rootMargin: '300px 0px' },
  );

  // ---- rows ----

  const paintDetail = (it: Item): void => {
    const slot = h('div.mlist-stage-slot', null, it.onStage ? null : fallbackPicture(it.row));
    const caption = it.onStage ? h('p.mlist-caption', null, 'Drag to rotate, scroll to zoom') : null;
    replace(
      it.inner,
      h('div.mlist-detail', null, h('div.mlist-stage-col', null, slot, caption), h('div.mlist-detail-main', null, it.row.detail())),
    );
  };

  const paintItem = (it: Item, row: ListRow): void => {
    it.row = row;
    replace(it.body, bodyOf(row));
    it.tail.hidden = row.actions === null;
    replace(it.tail, row.actions);
    const want = row.spec ? specKey(row.spec) : null;
    if (want !== it.want) {
      it.want = want;
      it.picKey = null;
      if (row.spec) {
        replace(it.pic); // the empty box until its thumbnail is drawn
        if (it.visible) wantThumb(it);
      } else {
        replace(it.pic, fallbackPicture(row));
      }
    }
  };

  const choose = (key: string): void => {
    desired = desired === key ? null : key;
    settle();
  };

  const createItem = (row: ListRow): Item => {
    const id = `mlist-panel-${++uid}`;
    const key = row.key;
    const pic = h('div.mlist-pic');
    const body = h('div.mlist-body');
    const summary = h('div.mlist-summary', { role: 'button', tabindex: '0', 'aria-expanded': 'false', 'aria-controls': id }, pic, body);
    const tail = h('div.mlist-tail');
    const inner = h('div.mlist-panel-inner');
    const panel = h('div.mlist-panel', { id, role: 'region' }, inner);
    panel.hidden = true;
    panel.style.height = '0px';
    const el = h('div.mlist-item', { 'data-key': key }, summary, tail, panel);
    const it: Item = {
      row,
      key,
      el,
      summary,
      pic,
      body,
      tail,
      panel,
      inner,
      want: undefined,
      picKey: null,
      queued: false,
      visible: false,
      onStage: false,
      gone: false,
    };
    summary.addEventListener('click', () => choose(key));
    summary.addEventListener('keydown', (ev) => {
      if (ev.key === 'Enter' || ev.key === ' ') {
        ev.preventDefault();
        choose(key);
      }
    });
    const prime = (): void => {
      const spec = items.get(key)?.row.spec;
      if (spec) prepare(spec);
    };
    summary.addEventListener('pointerenter', prime);
    summary.addEventListener('pointerdown', prime);
    byPic.set(pic, it);
    io.observe(pic);
    return it;
  };

  const apply = (): void => {
    dirty = false;
    const keep = new Set(latest.map((r) => r.key));
    for (const [key, it] of items) {
      if (keep.has(key)) continue;
      if (key === openKey) hardReset();
      it.gone = true;
      io.unobserve(it.pic);
      it.el.remove();
      items.delete(key);
    }
    let prev: Element | null = null;
    for (const row of latest) {
      let it = items.get(row.key);
      if (!it) {
        it = createItem(row);
        items.set(row.key, it);
      }
      paintItem(it, row);
      const at: Element | null = prev ? prev.nextElementSibling : rowsBox.firstElementChild;
      if (it.el !== at) rowsBox.insertBefore(it.el, at);
      prev = it.el;
    }
    replace(emptyBox, latest.length ? null : fallback);
    if (openKey !== null && !anim) {
      const it = items.get(openKey);
      if (it) {
        paintDetail(it);
        relayout();
      }
    }
  };

  // ---- the stage: the open row's canvas ----

  // Keeps the stage slot on the canvas when the list's width changes.
  function relayout(): void {
    if (!held || openKey === null || anim) return;
    const it = items.get(openKey);
    if (!it || !it.onStage) return;
    const slotEl = it.inner.querySelector<HTMLElement>('.mlist-stage-slot');
    if (!slotEl) return;
    const box = rel(slotEl);
    const prev = openBox;
    openBox = box;
    placeCanvas(held.canvas, box.x, box.y, box.w, box.h, 'none', true);
    if (!prev || prev.w !== box.w || prev.h !== box.h) held.resize({ w: box.w, h: box.h }, pixelRatio());
  }

  let lastWidth = -1;
  const ro = new ResizeObserver((entries) => {
    const w = entries[0]?.contentRect.width ?? 0;
    if (w === lastWidth) return;
    lastWidth = w;
    if (!anim) relayout();
  });
  ro.observe(root);

  // Frees the stage at once (a removed row, or the list's end): no animation.
  function hardReset(): void {
    if (held) {
      held.orbit(false);
      held.canvas.remove();
      held.release();
      held = null;
    }
    anim = null;
    openKey = null;
    desired = null;
    openBox = null;
  }

  function beginOpen(it: Item): void {
    const spec = it.row.spec;
    const st = spec ? getStage() : null;
    openKey = it.key;
    it.onStage = st !== null && spec !== null;
    it.panel.hidden = false;
    paintDetail(it);
    const h1 = it.panel.scrollHeight;
    it.summary.setAttribute('aria-expanded', 'true');
    const pr = pixelRatio();
    const pic = rel(it.pic);
    const slotEl = it.inner.querySelector<HTMLElement>('.mlist-stage-slot');
    const slot = slotEl ? rel(slotEl) : { x: 0, y: 0, w: THUMB_W, h: THUMB_H };
    openBox = slot;

    if (st && spec && reducedMotion()) {
      // No animation: the final state at once.
      held = st;
      st.place(spec, { w: slot.w, h: slot.h }, pr);
      st.pose(1);
      placeCanvas(st.canvas, slot.x, slot.y, slot.w, slot.h, 'none', true);
      root.append(st.canvas);
      st.orbit(true);
      it.el.classList.add('is-open');
      it.panel.style.height = 'auto';
      ensureLoop();
      return;
    }
    if (st && spec) {
      // The canvas starts exactly over the thumbnail, drawn in the same pose 0 pixels.
      held = st;
      st.place(spec, { w: THUMB_W, h: THUMB_H }, pr);
      // Origin at the list's corner: the frames move it by absolute translates from the thumbnail's box.
      placeCanvas(st.canvas, 0, 0, THUMB_W, THUMB_H, `translate(${pic.x}px, ${pic.y}px) scale(1)`, false);
      root.append(st.canvas);
      it.el.classList.add('is-open');
      anim = { kind: 'open', it, flight: true, pic, slot, h0: 0, h1, t0: performance.now() };
    } else {
      anim = { kind: 'open', it, flight: false, pic, slot, h0: 0, h1, t0: performance.now() };
    }
    ensureLoop();
  }

  function finishOpen(a: Anim): void {
    a.it.panel.style.height = 'auto';
    if (a.flight && held) {
      held.resize({ w: a.slot.w, h: a.slot.h }, pixelRatio());
      held.pose(1);
      placeCanvas(held.canvas, a.slot.x, a.slot.y, a.slot.w, a.slot.h, 'none', true);
      held.orbit(true);
    }
  }

  function beginClose(it: Item): void {
    const flight = held !== null && it.onStage;
    const h0 = it.panel.getBoundingClientRect().height;
    const pic = rel(it.pic);
    paintThumb(it); // the row's picture is in place under the canvas when it lands
    const slot = openBox ?? { x: 0, y: 0, w: THUMB_W, h: THUMB_H };
    if (flight && held) {
      held.orbit(false);
      const s = slot.w / THUMB_W;
      placeCanvas(held.canvas, 0, 0, THUMB_W, THUMB_H, `translate(${slot.x}px, ${slot.y}px) scale(${s})`, false);
    }
    it.panel.style.height = `${h0}px`;
    if (reducedMotion()) {
      finishClose(it);
      return;
    }
    anim = { kind: 'close', it, flight, pic, slot, h0, h1: 0, t0: performance.now() };
    ensureLoop();
  }

  function finishClose(it: Item): void {
    if (held) {
      held.canvas.remove();
      held.release();
      held = null;
    }
    openBox = null;
    it.el.classList.remove('is-open');
    it.summary.setAttribute('aria-expanded', 'false');
    it.panel.style.height = '0px';
    it.panel.hidden = true;
    it.inner.replaceChildren();
    it.onStage = false;
    openKey = null;
  }

  // Moves the open or closing row toward the state the reader wants, one step at a time.
  function settle(): void {
    for (let i = 0; i < 4 && !anim && !destroyed; i++) {
      if (dirty) apply();
      if (openKey !== null && openKey === desired) break;
      if (openKey !== null) {
        const cur = items.get(openKey);
        if (!cur) {
          hardReset();
          continue;
        }
        beginClose(cur);
        continue;
      }
      if (desired === null) break;
      const next = items.get(desired);
      if (!next) {
        desired = null;
        break;
      }
      beginOpen(next);
    }
    if (queue.length) schedulePump();
    if (needsLoop()) ensureLoop();
  }

  // ---- the frame loop: morph steps, and the live stage's ticks while a row is open ----

  function needsLoop(): boolean {
    return anim !== null || (held !== null && openKey !== null);
  }

  function ensureLoop(): void {
    if (!raf) raf = requestAnimationFrame(frame);
  }

  function frame(now: number): void {
    raf = 0;
    if (destroyed) return;
    const dt = lastNow ? Math.min(100, now - lastNow) : 16;
    lastNow = now;
    const a = anim;
    if (a) {
      const t = Math.min(1, (now - a.t0) / DURATION_MS);
      const e = easeInOutCubic(t);
      a.it.panel.style.height = `${lerp(a.h0, a.h1, e)}px`;
      if (a.flight && held) {
        // p runs 0 (the thumbnail) to 1 (the stage slot); a close runs it back.
        const p = a.kind === 'open' ? e : 1 - e;
        const s = a.slot.w / THUMB_W;
        held.canvas.style.transform = `translate(${lerp(a.pic.x, a.slot.x, p)}px, ${lerp(a.pic.y, a.slot.y, p)}px) scale(${lerp(1, s, p)})`;
        held.pose(p);
      }
      if (t >= 1) {
        anim = null;
        if (a.kind === 'open') finishOpen(a);
        else finishClose(a.it);
        settle();
      }
    } else if (held && openKey !== null) {
      held.tick(dt);
    }
    if (!raf && needsLoop()) raf = requestAnimationFrame(frame);
    if (!needsLoop()) lastNow = 0;
  }

  // Escape closes the open row.
  root.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape' && desired !== null) {
      desired = null;
      settle();
    }
  });

  return {
    el: root,
    setRows(rows, fb = null) {
      if (destroyed) return;
      latest = rows;
      fallback = fb;
      dirty = true;
      settle();
    },
    open(key) {
      if (destroyed) return;
      desired = key;
      if (key !== null) items.get(key)?.el.scrollIntoView({ block: 'nearest' });
      settle();
    },
    destroy() {
      destroyed = true;
      hardReset();
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
      io.disconnect();
      ro.disconnect();
      queue.length = 0;
      items.clear();
      root.remove();
    },
  };
}
