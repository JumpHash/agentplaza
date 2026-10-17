// The machines as a list, shared by the Machines and Nodes screens. A row shows the machine's
// picture (a 160 x 120 thumbnail, drawn by the Rust engine) with its facts and load bars. Opening
// a row grows its panel open, and the engine's live 3D view fills the row's stage slot; closing
// hides it. One row is open at a time, and the engine's one stage canvas sits over the open row.
// A build without the engine shows each row's line-art picture instead.

import '../styles/ops.css';
import '../styles/machine-list.css';
import { api, type Agent, type FleetApple, type MachineNode, type ProcessUsage, type Run, type StationView, type Vm, type VmUsage } from '../api';
import { agentName, roleLabel } from '../roles';
import { ago, badge, button, type Tone } from '../ui/components';
import { deviceVisual } from '../ui/devices';
import { h, icon, replace } from '../ui/dom';
import { isStationNode, leaseCounts, stationBadge, stationLine, stationLook } from './station-machine';
import { lookOfSpec, sizeLine, specKey, THUMB_H, THUMB_W, VIEWS, type PictureSpec } from './machine-picture';
import { engineWanted, isEngineStopped, startEngineStage, startThumbEngine, type EngineStage } from './engine-mount';

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
export const gib = (mib: number | undefined): string => (Number.isFinite(mib) ? ((mib as number) / 1024).toFixed(1) : '–');
export const human = (mib: number): string => (mib >= 1024 ? `${gib(mib)} GiB` : `${Math.round(mib)} MiB`);
// A value a node did not report (an older node, or a VM still starting) shows as a dash, never a crash.
const fx1 = (n: number | undefined): string => (Number.isFinite(n) ? (n as number).toFixed(1) : '–');
const pct = (n: number | undefined): string => (Number.isFinite(n) ? `${(n as number).toFixed(1)}%` : '–');
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
  // A machine that did not answer (the unreachable row) reports few facts: each part shows only what it has.
  const cores = m.cpus ? `${m.cpus}-core${m.arch ? ` ${m.arch}` : ''}` : (m.arch ?? '');
  const where = m.backend ? ` on ${m.backend.toUpperCase()}` : '';
  const memory = m.memory_mib ? ` · ${Math.round(m.memory_mib / 1024)} GiB memory` : '';
  const chip = m.apple ? chipLine(m.apple) : `${cores}${where}${memory}`;
  const head = [KIND_LABEL[m.kind], m.model, chip].filter(Boolean).join(' · ');
  const gpus = m.gpus ?? [];
  return gpus.length ? `${head} · ${gpus.map((g) => g.name).join(', ')}` : head;
}

/** Fills the partial replies (a machine without its gpus, runs or vms) as empty lists, never as errors. */
export function normalizeMachines(fetched: MachineNode[] | null): MachineNode[] {
  return (fetched ?? []).map((m) => ({
    ...m,
    gpus: m.gpus ?? [],
    runs: (m.runs ?? []).map((r) => ({ ...r, vms: r.vms ?? [] })),
  }));
}

/** The picture a machine is drawn as: the same spec the engine's thumbnails take. */
export function specOf(m: MachineNode): PictureSpec {
  const spec: PictureSpec = { kind: m.kind, vendor: m.vendor ?? null, memory_mib: m.memory_mib, gpus: m.gpus.map((g) => ({ vendor: g.vendor })) };
  // A workstation's kind is always "workstation": its picture follows its form (a laptop, a mini, a tower, a rack).
  return isStationNode(m) ? { ...spec, look: stationLook(m.station.facts) } : spec;
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

// ---- Workstations: the detail's live picture, and the timers that keep it ----

const STATION_PIC_MS = 10_000;
const STATION_OS: Record<StationView['facts']['os'], string> = { macos: 'macOS', windows: 'Windows', linux: 'Linux' };
const STATION_FORM: Record<StationView['facts']['form'], string> = { laptop: 'Laptop', desktop: 'Desktop', mini: 'Mini', server: 'Server', vm: 'VM' };
// A workstation's last picture by station id (null: the request failed). A detail rebuilt by a refresh shows it at once.
const stationPics = new Map<string, { url: string | null; at: number }>();
// A detail's stop function, by its root element: run when the panel is rebuilt, closed or the list ends.
const detailStops = new WeakMap<Element, () => void>();

/** Stops the timers of the detail panels inside `scope`. */
function stopDetailTimers(scope: Element): void {
  for (const el of Array.from(scope.querySelectorAll<HTMLElement>('.mlist-machine'))) {
    detailStops.get(el)?.();
    detailStops.delete(el);
  }
}

/** A workstation's live picture: fetched now and then every 10 seconds, until the detail stops. */
function stationPicture(id: string): { el: HTMLElement; stop: () => void } {
  const img = h('img', { alt: '', draggable: 'false' }) as HTMLImageElement;
  img.style.maxWidth = '100%';
  img.hidden = true;
  const note = h('p.faint', null, 'Loading the picture...');
  const el = h('div.mlist-station-pic', null, img, note);
  let alive = true;
  let timer: number | undefined;

  const show = (url: string | null): void => {
    img.hidden = url === null;
    note.hidden = url !== null;
    note.textContent = 'No picture from the workstation.';
    if (url) img.src = url;
    else img.removeAttribute('src');
  };
  const load = async (): Promise<void> => {
    const url = await api.stationThumbnailUrl(id);
    if (!alive) {
      if (url) URL.revokeObjectURL(url);
      return;
    }
    const old = stationPics.get(id);
    if (old?.url) URL.revokeObjectURL(old.url);
    stationPics.set(id, { url, at: Date.now() });
    show(url);
  };
  // Waits until the cached picture is 10 seconds old, then fetches and waits again.
  const arm = (): void => {
    const c = stationPics.get(id);
    const wait = c ? Math.max(0, c.at + STATION_PIC_MS - Date.now()) : 0;
    timer = window.setTimeout(async () => {
      if (!alive) return;
      if (!el.isConnected) {
        alive = false; // the panel left the page without a stop: end here
        return;
      }
      await load();
      if (alive) arm();
    }, wait);
  };

  const cached = stationPics.get(id);
  if (cached) show(cached.url);
  arm();
  return {
    el,
    stop: () => {
      alive = false;
      window.clearTimeout(timer);
    },
  };
}

/** A workstation's detail: its sentence, facts, GPUs, the account the agents work in, its leases and its live picture. */
function stationDetail(s: StationView): HTMLElement {
  const f = s.facts;
  const pic = stationPicture(s.id);
  const pairs: [string, string][] = [
    ['Hostname', f.hostname || '—'],
    ['OS', `${STATION_OS[f.os]}${f.os_version ? ` ${f.os_version}` : ''}`],
    ['Form', STATION_FORM[f.form]],
    ['Arch', f.arch || '—'],
    ['CPUs', f.cpus > 0 ? `${f.cpus} cores` : '—'],
    ['RAM', f.ram_mb > 0 ? `${gib(f.ram_mb)} GiB` : '—'],
    ['Account', `${f.account.name} (${f.account.mode === 'owner' ? 'the owner’s account' : 'agent account'})`],
    ['Status', s.draining ? `${s.status}, draining` : s.status],
    ['Version', s.version || '—'],
    ['Address', s.address || '—'],
  ];
  const gpus = f.gpus.length
    ? h(
        'div.mlist-facts',
        null,
        f.gpus.map((g) => h('div.mlist-fact', null, h('span', null, `${g.vendor} ${g.name}`), h('span', null, g.vram_mb > 0 ? `${gib(g.vram_mb)} GiB VRAM` : 'VRAM not reported'))),
      )
    : h('p.faint', null, 'No GPUs.');
  const leases = s.leases.length
    ? h(
        'ul.mlist-leases',
        null,
        s.leases.map((l) =>
          h(
            'li',
            null,
            [`Agent ${l.agent}`, l.state, l.task, l.state === 'queued' && l.position !== null ? `queue position ${l.position}` : null].filter(Boolean).join(' · '),
          ),
        ),
      )
    : h('p.faint', null, 'No leases.');
  const root = h(
    'div.mlist-machine',
    null,
    h('p.mach-sentence', null, stationLine(s)),
    h('dl.kv', null, pairs.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)])),
    h('div.mach-caps', null, 'GPUs'),
    gpus,
    h('div.mach-caps', null, 'Leases'),
    leases,
    h('div.mach-caps', null, 'Live picture'),
    pic.el,
    h('p', null, h('a', { href: '#/workstations' }, 'Open in Workstations')),
  );
  detailStops.set(root, pic.stop);
  return root;
}

/** A machine's detail: its sentence, live tallies, where the resources go, runs and technical details. */
export function machineDetail(m: MachineNode, byCid: Map<number, Agent>, rs: RunState): HTMLElement {
  if (isStationNode(m)) return stationDetail(m.station);
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
          tally('Load', (s.load ?? []).map(fx1).join(' · ') || '–', '1 / 5 / 15 min'),
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
  const station = isStationNode(m) ? m.station : null;
  const status: ListRow['status'] = station
    ? station.online
      ? { label: 'online', tone: 'ok' }
      : { label: 'offline', tone: 'muted' }
    : m.reachable === false
      ? { label: 'unreachable', tone: 'danger' }
      : live
        ? { label: 'running', tone: 'ok' }
        : { label: 'idle', tone: 'muted' };
  const bars: ListBar[] = [];
  if (s) {
    bars.push({ label: 'CPU', pct: clamp(s.cpu_percent, 0, 100), text: pct(s.cpu_percent) });
    bars.push({
      label: 'Mem',
      pct: clamp((s.mem_used_mib / Math.max(s.mem_total_mib, 1)) * 100, 0, 100),
      text: `${gib(s.mem_used_mib)} / ${gib(s.mem_total_mib)} GiB`,
    });
    bars.push({ label: 'Load', pct: clamp(((s.load?.[0] ?? 0) / Math.max(m.cpus, 1)) * 100, 0, 100), text: (s.load ?? []).map(fx1).join(' · ') });
  }
  const counts = station ? leaseCounts(station) : null;
  const facts: [string, string][] = station && counts
    ? [
        ['Leases', `${counts.leases} leases, ${counts.active} active${counts.queued ? `, ${counts.queued} queued` : ''}`],
        ['Address', station.address || '—'],
        ['Last seen', ago(station.last_seen_ms)],
      ]
    : [
        ['Platform', m.apple ? `macOS · ${m.arch}` : m.arch],
        ['CPU', m.apple ? `${m.apple.chip} · ${m.cpus} cores` : `${m.cpus} cores`],
        ['Memory', `${gib(m.memory_mib)} GiB`],
        ['GPUs', gpuText(m.gpus)],
        ['Agents', `${agentCount(m.agents)} · ${machineVms(m)} VMs`],
      ];
  const badgeText = station ? stationBadge(station) : null;
  return {
    key: m.id,
    // A workstation is not in the engine's fleet: its row has no orbit and no 3D view.
    machineId: station ? null : m.id,
    name: m.name,
    subtitle: station ? stationLine(station) : [KIND_LABEL[m.kind], m.model].filter(Boolean).join(' · '),
    status,
    facts,
    bars,
    spec: specOf(m),
    active: live,
    actions: null,
    tags: badgeText ? [badgeText] : [],
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
  /** The machine the row shows (its id in the engine's fleet), or null: the engine shows no orbit for the row. */
  machineId: string | null;
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
  /** Small badges beside the name (a workstation's "agents act as ..."). */
  tags?: string[];
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
  /** The flat views under the open row's stage (set when the row opens with an engine). */
  views?: HTMLElement | null;
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
  h0: number;
  h1: number;
  t0: number;
}

const DURATION_MS = 600;
const thumbs = new Map<string, string | null>();

const pixelRatio = (): number => Math.min(2, Math.max(1, window.devicePixelRatio || 1));
const reducedMotion = (): boolean => window.matchMedia?.('(prefers-reduced-motion: reduce)').matches === true;
const lerp = (a: number, b: number, t: number): number => a + (b - a) * t;
const easeInOutCubic = (t: number): number => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);

/** The line art of a row with no picture of its own: the device drawing, or a generic icon when the kind is unknown. */
function fallbackPicture(row: ListRow): Node {
  if (!row.spec) return h('span.mlist-unknown', null, icon('machine'));
  return deviceVisual(lookOfSpec(row.spec), { gpu: row.spec.gpus.length > 0, active: row.active });
}

/** Shows a thumbnail (a PNG data URL, or null for the line art) in the picture box. */
function showThumb(it: Item, k: string, url: string | null): void {
  if (it.gone) return;
  it.picKey = k;
  if (url) replace(it.pic, h('img', { src: url, width: THUMB_W, height: THUMB_H, alt: '', draggable: 'false' }));
  else replace(it.pic, fallbackPicture(it.row));
}

/** Paints the engine's RGBA8 buffer onto a canvas of w by h pixels (false when the buffer is not w by h pixels). */
function paintRgba(c: HTMLCanvasElement, rgba: Uint8Array, w: number, h: number): boolean {
  if (rgba.length !== w * h * 4) return false;
  c.width = w;
  c.height = h;
  const ctx = c.getContext('2d');
  if (!ctx) return false;
  ctx.putImageData(new ImageData(new Uint8ClampedArray(rgba), w, h), 0, 0);
  return true;
}

/** The engine's RGBA8 buffer as a PNG data URL (null when the buffer is not w by h pixels). */
function pngOf(rgba: Uint8Array, w: number, h: number): string | null {
  const c = document.createElement('canvas');
  return paintRgba(c, rgba, w, h) ? c.toDataURL('image/png') : null;
}

/** The flat views under an open row's stage: four labelled figures; fillViews paints them. */
function viewsStrip(): HTMLElement {
  return h(
    'div.machine-views',
    null,
    ...VIEWS.map(({ label }) => h('figure', null, h('canvas', { role: 'img', 'aria-label': label, width: THUMB_W, height: THUMB_H }), h('figcaption', null, label))),
  );
}

/** Asks the engine for each view (at the device's pixel ratio, at most 512 a side) and for the machine's size caption. A rejected request leaves its canvas empty. */
function fillViews(strip: HTMLElement, machineId: string, stage: EngineStage): void {
  const pr = pixelRatio();
  const w = Math.min(512, Math.round(THUMB_W * pr));
  const h2 = Math.min(512, Math.round(THUMB_H * pr));
  strip.querySelectorAll<HTMLCanvasElement>('canvas').forEach((c, i) => {
    const view = VIEWS[i]?.view;
    if (view === undefined) return;
    stage.view(machineId, view, w, h2).then(
      (rgba) => paintRgba(c, rgba, w, h2),
      () => false,
    );
  });
  stage.machineInfo(machineId).then(
    (info) => {
      const line = sizeLine(info);
      if (line !== null) strip.append(h('p.machine-views-caption', null, line));
    },
    () => undefined,
  );
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
    h(
      'div.mlist-head',
      null,
      h('strong.mlist-name', null, row.name),
      badge(row.status.label, row.status.tone, true),
      (row.tags ?? []).map((t) => badge(t, 'info', true)),
    ),
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
  // The engine's stage for the open row: its canvas sits over the row's stage slot.
  let eng: { canvas: HTMLCanvasElement; stage: EngineStage } | null = null;
  let raf = 0;
  let destroyed = false;
  let uid = 0;
  // The thumbnail engine (no row open): one per page, started when a thumbnail waits and destroyed once the queue drains.
  let thumbStage: EngineStage | null = null;
  let inflight = 0;

  /** The engine that draws thumbnails: the open row's engine when there is one (one engine per page), else the thumbnail engine. */
  const thumbSource = (): EngineStage => {
    if (eng !== null) return eng.stage;
    thumbStage ??= startThumbEngine();
    return thumbStage;
  };

  const stopThumbStage = (): void => {
    if (thumbStage === null) return;
    thumbStage.destroy();
    thumbStage = null;
  };

  /** Asks the engine for a row's thumbnail. A stopped engine puts the row back in the queue; a failed one shows the line art. */
  function requestThumb(it: Item): void {
    const spec = it.row.spec;
    if (!spec) return;
    const k = specKey(spec);
    if (it.picKey === k) return;
    const pr = pixelRatio();
    const ck = `${k}@${pr}`;
    const cached = thumbs.get(ck);
    if (cached !== undefined) {
      showThumb(it, k, cached);
      return;
    }
    const w = Math.round(THUMB_W * pr);
    const h = Math.round(THUMB_H * pr);
    const machineId = it.row.machineId;
    if (machineId === null) {
      showThumb(it, k, null);
      return;
    }
    inflight++;
    thumbSource()
      .thumbnail(machineId, w, h)
      .then(
        (rgba) => {
          const url = pngOf(rgba, w, h);
          thumbs.set(ck, url);
          showThumb(it, k, url);
        },
        (e: unknown) => {
          if (isEngineStopped(e)) {
            if (!destroyed) wantThumb(it);
            return;
          }
          console.error('engine thumbnail did not draw', e);
          thumbs.set(ck, null);
          showThumb(it, k, null);
        },
      )
      .finally(() => {
        inflight--;
        if (inflight === 0 && queue.length === 0 && eng === null) stopThumbStage();
      });
  }

  /** Paints a row's thumbnail from the engine (a build without the engine keeps the line art). */
  const paintThumb = (it: Item): void => {
    if (engineWanted()) requestThumb(it);
  };

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
    stopDetailTimers(it.inner);
    const slot = h('div.mlist-stage-slot', null, it.onStage ? null : fallbackPicture(it.row));
    const caption = it.onStage ? h('p.mlist-caption', null, 'Drag to rotate, scroll to zoom') : null;
    // The desktop's labelled toggle: closes the open row's 3D view (the row's picture comes back).
    const toggle = it.onStage ? button('Hide 3D view', () => choose(it.key), { kind: 'ghost', size: 'sm' }) : null;
    replace(
      it.inner,
      h('div.mlist-detail', null, h('div.mlist-stage-col', null, slot, caption, toggle, it.views ?? null), h('div.mlist-detail-main', null, it.row.detail())),
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
      if (row.spec && engineWanted()) {
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
        placeEngine();
      }
    }
  };

  // ---- the stage: the open row's canvas ----

  // Keeps the engine's canvas on the stage slot when the list's width changes.
  let lastWidth = -1;
  const ro = new ResizeObserver((entries) => {
    const w = entries[0]?.contentRect.width ?? 0;
    if (w === lastWidth) return;
    lastWidth = w;
    if (!anim) placeEngine();
  });
  ro.observe(root);

  // Frees the stage at once (a removed row, or the list's end): no animation.
  function hardReset(): void {
    stopEngine();
    anim = null;
    openKey = null;
    desired = null;
  }

  /** The open row's engine canvas: over its stage slot, faded in once the engine has drawn its first frame. */
  function startEngineRow(machineId: string): EngineStage {
    const canvas = document.createElement('canvas');
    canvas.className = 'mlist-engine-canvas';
    canvas.style.opacity = '0';
    canvas.style.transition = 'opacity 240ms ease';
    canvas.style.touchAction = 'none';
    root.append(canvas);
    const stage = startEngineStage(canvas, machineId, () => {
      canvas.style.opacity = '1';
    });
    eng = { canvas, stage };
    placeEngine();
    return stage;
  }

  /** Keeps the engine canvas over the open row's stage slot (the panel's height moves the slot during a morph). */
  function placeEngine(): void {
    if (eng === null || openKey === null) return;
    const slotEl = items.get(openKey)?.inner.querySelector<HTMLElement>('.mlist-stage-slot');
    if (!slotEl) return;
    const box = rel(slotEl);
    placeCanvas(eng.canvas, box.x, box.y, box.w, box.h, 'none', true);
  }

  /** Destroys the engine and removes its canvas (the open row closed, or the list was destroyed). */
  function stopEngine(): void {
    if (eng === null) return;
    eng.stage.destroy();
    eng.canvas.remove();
    eng = null;
  }

  function beginOpen(it: Item): void {
    // The open row's 3D view is the engine's: a row with no machine id opens its panel with no view.
    const engineId = engineWanted() ? it.row.machineId : null;
    openKey = it.key;
    it.onStage = engineId !== null;
    it.views = engineId !== null ? viewsStrip() : null;
    it.panel.hidden = false;
    paintDetail(it);
    const h1 = it.panel.scrollHeight;
    it.summary.setAttribute('aria-expanded', 'true');
    if (engineId !== null) {
      stopThumbStage(); // one engine per page: the row's engine takes over the thumbnails
      const stage = startEngineRow(engineId);
      if (it.views) fillViews(it.views, engineId, stage);
    }
    anim = { kind: 'open', it, h0: 0, h1, t0: performance.now() };
    ensureLoop();
  }

  function finishOpen(a: Anim): void {
    a.it.panel.style.height = 'auto';
  }

  function beginClose(it: Item): void {
    if (eng !== null) {
      eng.canvas.style.opacity = '0';
      eng.canvas.style.pointerEvents = 'none';
    }
    const h0 = it.panel.getBoundingClientRect().height;
    paintThumb(it); // the row's picture is in place under the canvas when it lands
    it.panel.style.height = `${h0}px`;
    if (reducedMotion()) {
      finishClose(it);
      return;
    }
    anim = { kind: 'close', it, h0, h1: 0, t0: performance.now() };
    ensureLoop();
  }

  function finishClose(it: Item): void {
    stopEngine();
    it.el.classList.remove('is-open');
    it.summary.setAttribute('aria-expanded', 'false');
    it.panel.style.height = '0px';
    it.panel.hidden = true;
    stopDetailTimers(it.inner);
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

  // ---- the frame loop: the panel's morph steps, and the engine's canvas over the open row ----

  function needsLoop(): boolean {
    return anim !== null || (eng !== null && openKey !== null);
  }

  function ensureLoop(): void {
    if (!raf) raf = requestAnimationFrame(frame);
  }

  function frame(now: number): void {
    raf = 0;
    if (destroyed) return;
    const a = anim;
    if (a) {
      const t = Math.min(1, (now - a.t0) / DURATION_MS);
      a.it.panel.style.height = `${lerp(a.h0, a.h1, easeInOutCubic(t))}px`;
      if (t >= 1) {
        anim = null;
        if (a.kind === 'open') finishOpen(a);
        else finishClose(a.it);
        settle();
      }
    }
    placeEngine();
    if (!raf && needsLoop()) raf = requestAnimationFrame(frame);
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
      stopThumbStage();
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
      io.disconnect();
      ro.disconnect();
      queue.length = 0;
      for (const it of items.values()) stopDetailTimers(it.inner);
      items.clear();
      root.remove();
    },
  };
}
