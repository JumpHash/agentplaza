// The machine view: one machine's own screen at `#/machines/<id>`. A header names it; a grid holds its live 3D stage
// with its hardware and load beside it, the four flat views, its VM slots, its agents and its events. Every panel
// reads its own source and shows a dash until that source answers; nothing waits on the metrics.
import '../styles/machine-view.css';
import { api, type Agent, type FleetMachine, type MachineNode } from '../api';
import { localOffset } from '../metrics/format';
import type { EventRow, View, ViewQuery } from '../metrics/model';
import { href } from '../router';
import { ago, badge, empty, type Tone } from '../ui/components';
import { sparkline, type ChartHandle } from '../ui/chart';
import { h, replace } from '../ui/dom';
import { fallbackPicture, fillViews, isLive, KIND_LABEL, normalizeMachines, specOf, stateTone, viewsStrip } from './machine-list';
import { engineWanted, followLook, startEngineStage, type EngineStage } from './engine-mount';
import { styleSwitch } from './machine-style';
import { agentLines, finiteValues, gpuCardLine, latestValue, metricValues, vmRows, type GpuCardInfo } from './machine-view-model';
import type { Cleanup } from './index';

const POLL_MS = 5000;
const STATUS_TONE: Record<string, Tone> = { online: 'ok', draining: 'warn', offline: 'muted', revoked: 'danger' };

const gib = (mib: number | null | undefined): string => (Number.isFinite(mib) ? ((mib as number) / 1024).toFixed(1) : '–');
const clamp = (n: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, n));

/** A card of the grid: its area, its title and its body. */
function card(area: string, title: string, ...kids: Node[]): HTMLElement {
  return h(
    `section.card.mv-panel.mv-${area}`,
    { 'aria-label': title },
    h('div.card-head', null, h('h2', null, title)),
    h('div.card-body', null, ...kids),
  );
}

/** A labelled bar, drawn with the list's load-bar classes. */
function meter(label: string, pct: number, text: string): HTMLElement {
  const fill = h('div.mach-bar-fill', null);
  fill.style.width = `${clamp(pct, 0, 100)}%`;
  return h('div.mlist-bar', null, h('span', null, label), h('div.mach-bar', null, fill), h('span', null, text));
}

/** The hardware: the CPU, memory, storage and platform, then each GPU with the reference card the engine matched. */
function hardware(f: FleetMachine, cards: GpuCardInfo[] | null): Node[] {
  const dl = h('dl.kv');
  const rows: [string, string][] = [
    ['CPU', f.cpu.model || '—'],
    ['Cores', `${f.cpu.cores} cores · ${f.cpu.threads} threads`],
    ['Memory', `${gib(f.memory_mib)} GiB`],
    ['Storage', `${Math.round(f.storage_gib)} GiB`],
    ['Platform', `${f.os} · ${f.arch} · ${f.backend}`],
  ];
  for (const [k, v] of rows) dl.append(h('dt', null, k), h('dd', null, v));
  const gpus = f.gpus.map((g, i) => {
    const card = cards?.[i]?.card ?? null;
    const line = cards === null ? `${g.vendor} · ${g.memory_mib ? `${gib(g.memory_mib)} GiB` : 'VRAM not reported'}` : gpuCardLine({ gpu: g.name, vendor: g.vendor, memory_mib: g.memory_mib, card });
    return h('div.mv-gpu', null, h('strong', null, g.name), h('span.faint', null, line));
  });
  return [dl, h('div.mv-caps', null, 'GPUs'), gpus.length ? h('div.mv-gpus', null, ...gpus) : h('p.faint', null, 'No GPU.')];
}

/** The load: CPU and memory from the fleet's live stats, the GPU's utilisation from the metrics ("—" until they arrive). */
function load(f: FleetMachine, gpu: number | null): Node[] {
  const s = f.stats;
  const memPct = s && f.memory_mib > 0 ? (s.mem_used_mib / f.memory_mib) * 100 : 0;
  const gpuText = f.gpus.length === 0 ? 'No GPU' : gpu === null ? '—' : `${gpu.toFixed(1)}%`;
  return [
    meter('CPU', s ? s.cpu_pct : 0, s ? `${s.cpu_pct.toFixed(1)}%` : '—'),
    meter('Memory', memPct, s ? `${gib(s.mem_used_mib)} / ${gib(f.memory_mib)} GiB` : '—'),
    h(
      'div.mv-metric',
      null,
      h('span', null, 'GPU'),
      h('span', gpu === null && f.gpus.length > 0 ? { title: 'no GPU metrics' } : null, gpuText),
    ),
  ];
}

/** The VM slots: each VM's id, its agent's name when it has one, and its state. */
function vmList(node: MachineNode | null, byCid: Map<number, Agent>): Node[] {
  const rows = node ? vmRows(node) : [];
  if (!rows.length) return [h('p.faint', null, 'No VMs on this machine.')];
  return [
    h('div.mv-caps', null, `${rows.length} ${rows.length === 1 ? 'VM' : 'VMs'}`),
    h(
      'div.mv-list',
      null,
      rows.map((r) => {
        const agent = r.cid !== null ? byCid.get(r.cid) : undefined;
        return h('div.mv-row', null, h('span', null, agent ? `${r.id} · ${agent.name}` : r.id), badge(r.state, stateTone(r.state), true));
      }),
    ),
  ];
}

/** The agents on the machine, each linking to its page, with its status. */
function agentList(f: FleetMachine, byCid: Map<number, Agent>): Node[] {
  const lines = agentLines(f.agents, byCid);
  if (!lines.length) return [h('p.faint', null, 'No agents on this machine.')];
  return [
    h('div.mv-caps', null, `${f.working} working`),
    h(
      'div.mv-list',
      null,
      lines.map((a) => h('a.mv-row', { href: href('agents', a.cid) }, h('span', null, a.name), badge(a.status, a.status === 'working' ? 'ok' : 'muted', true))),
    ),
  ];
}

/** The events: the newest few in the last hour. No events (or a failed read) shows the machine's last-seen time. */
function eventList(events: EventRow[], lastSeen: number | null): Node[] {
  if (!events.length) {
    return [h('p.faint', null, 'No recent events'), h('p.faint', null, `Last seen ${lastSeen === null ? '—' : ago(lastSeen)}`)];
  }
  return [
    h(
      'ul.mv-event-list',
      null,
      events.slice(0, 8).map((e) => h('li', null, h('span.faint', null, ago(e.t_ms)), h('span', null, [e.subject, e.detail].filter(Boolean).join(' · ') || e.kind))),
    ),
  ];
}

/** The machine's CPU and GPU, from the metrics view of its compute tab. */
function computeQuery(id: string, tz: number): ViewQuery {
  return { tab: 'compute', scope: `node:${id}`, range: '1h', shift: 0, compare: 'prev', group: 'provider', width: 600, tz, fixture: false };
}

/** The events of the machine: its overview tab's scope, read at this moment. */
function eventsQuery(id: string, tz: number): ViewQuery {
  return { ...computeQuery(id, tz), tab: 'overview' };
}

export function mountMachineView(root: HTMLElement, id: string): Cleanup {
  let alive = true;
  let timer: ReturnType<typeof setInterval> | undefined;
  let fleet: FleetMachine | null = null;
  let unknown = false;
  let nodes: MachineNode[] = [];
  let byCid = new Map<number, Agent>();
  let cards: GpuCardInfo[] | null = null;
  let gpu: number | null = null;
  let cpu: number[] = [];
  let events: EventRow[] | null = null;
  let pictured = false;
  let cpuSpark: ChartHandle<number[]> | null = null;

  // The live 3D view: one engine on the page, its canvas in the stage cell. Without an engine, the picture shows instead.
  const engine = engineWanted();
  const canvas = document.createElement('canvas');
  canvas.className = 'mv-engine';
  canvas.style.opacity = '0';
  canvas.style.transition = 'opacity 240ms ease';
  canvas.style.touchAction = 'none';
  const stage: EngineStage | null = engine
    ? startEngineStage(canvas, id, () => {
        canvas.style.opacity = '1';
      })
    : null;

  const titleEl = h('h1', null, id);
  const statusBox = h('span.mv-status');
  const subEl = h('div.mv-sub');
  const lookSwitch = styleSwitch(); // the owner's machine style, in the header
  const head = h(
    'header.mv-head',
    null,
    h('a.mv-back', { href: href('machines') }, '‹ Machines'),
    h('div.mv-title', null, titleEl, statusBox),
    subEl,
    lookSwitch.el,
  );

  const stageBox = h('div.mv-stage-box', null, stage ? canvas : empty('machine', 'Loading the 3D view'));
  const stageCell = card('stage', '3D view', stageBox, ...(stage ? [h('p.mv-caption', null, 'Drag to rotate, scroll to zoom')] : []));
  const hwBox = h('div.mv-body', null, h('p.faint', null, 'Loading…'));
  const loadBox = h('div.mv-body');
  const sparkBox = h('div.mv-spark');
  sparkBox.hidden = true;
  const loadCell = card('load', 'Load', loadBox, h('div.mv-caps', null, 'CPU, last hour'), sparkBox);
  const strip = viewsStrip();
  const viewsCell = card('views', 'Views', strip);
  viewsCell.hidden = stage === null;
  const vmBox = h('div.mv-body');
  const agentBox = h('div.mv-body');
  const eventsBox = h('div.mv-body', null, h('p.faint', null, '—'));
  const missing = h('div.mv-missing');
  missing.hidden = true;
  const grid = h(
    'div.mv-grid',
    null,
    stageCell,
    card('hardware', 'Hardware', hwBox),
    loadCell,
    viewsCell,
    card('vms', 'VM slots', vmBox),
    card('agents', 'Agents', agentBox),
    card('events', 'Events', eventsBox),
  );
  root.classList.add('machine-view-body');
  root.append(h('div.machine-view', null, head, grid, missing));
  if (stage) fillViews(strip, id, stage);
  // A new style or theme: the views are drawn again in it (the stage engine follows the look on its own).
  const stopLook = stage ? followLook(() => requestAnimationFrame(() => fillViews(strip, id, stage))) : (): void => undefined;

  // The machine's cards: the engine's GPU cards once, then each refresh's facts.
  stage?.gpuCards(id).then(
    (c) => {
      if (!alive) return;
      cards = c;
      paint();
    },
    () => undefined,
  );

  const paintLoad = (): void => {
    if (fleet !== null) replace(loadBox, ...load(fleet, gpu));
    sparkBox.hidden = cpu.length === 0;
    if (cpu.length > 0) {
      if (cpuSpark === null) cpuSpark = sparkline(sparkBox, cpu, { height: 56, ariaLabel: 'CPU over the last hour' });
      else cpuSpark.update(cpu);
    }
  };

  const paintEvents = (): void => {
    if (events !== null) replace(eventsBox, ...eventList(events, fleet?.last_seen_ms ?? null));
  };

  const paint = (): void => {
    grid.hidden = unknown;
    missing.hidden = !unknown;
    if (unknown) {
      replace(missing, empty('machine', `No machine ${id}`, undefined, h('a', { href: href('machines') }, 'Back to Machines')));
      return;
    }
    const node = nodes.find((m) => m.id === id) ?? null;
    if (!engine && !pictured && node !== null) {
      pictured = true;
      replace(stageBox, fallbackPicture({ spec: specOf(node), active: isLive(node.runs ?? []) }));
    }
    if (fleet === null) return;
    titleEl.textContent = fleet.name;
    replace(statusBox, badge(fleet.status, STATUS_TONE[fleet.status] ?? 'muted', true));
    subEl.textContent = [KIND_LABEL[fleet.kind] ?? fleet.kind, fleet.model ?? fleet.vendor].filter(Boolean).join(' · ');
    replace(hwBox, ...hardware(fleet, cards));
    paintLoad();
    replace(vmBox, ...vmList(node, byCid));
    replace(agentBox, ...agentList(fleet, byCid));
    paintEvents();
  };

  const loadMetrics = async (): Promise<void> => {
    const tz = localOffset();
    const [view, around] = await Promise.all([
      api.metricsView(computeQuery(id, tz)).catch((): View | null => null),
      api.metricsAround(eventsQuery(id, tz), Date.now()).then(
        (a) => a.events ?? [],
        (): EventRow[] => [],
      ),
    ]);
    if (!alive) return;
    gpu = view ? latestValue(metricValues(view, 'host.gpu_util_pct')) : null;
    cpu = view ? finiteValues(metricValues(view, 'host.cpu_pct')) : [];
    events = around;
    paintLoad();
    paintEvents();
  };

  const refresh = async (): Promise<void> => {
    const [f, ms, ags] = await Promise.all([
      api.fleet().catch(() => null),
      api.machines().catch(() => null),
      api.agents().catch((): Agent[] => []),
    ]);
    if (!alive) return;
    if (f !== null) {
      fleet = f.machines.find((m) => m.id === id) ?? null;
      unknown = fleet === null;
    }
    if (ms !== null) nodes = normalizeMachines(ms);
    byCid = new Map<number, Agent>((ags ?? []).map((a): [number, Agent] => [a.cid, a]));
    paint();
    void loadMetrics();
  };

  void refresh();
  timer = setInterval(() => void refresh(), POLL_MS);
  return () => {
    alive = false;
    if (timer !== undefined) clearInterval(timer);
    cpuSpark?.destroy();
    stopLook();
    lookSwitch.dispose();
    stage?.destroy();
    canvas.remove();
  };
}
