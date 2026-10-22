// The machine view: one machine's own screen at `#/machines/<id>`. A header names it; a grid holds its live 3D stage
// with its hardware and load beside it, the four flat views, its VM slots, its agents and its events. Every panel
// reads its own source and shows a dash until that source answers; nothing waits on the metrics. The markup is the
// prototype's (styles/proto/machines.css): the back link, the head, and the `.mt-cards` grid of cards.
import { agentName } from '../roles';
import { api, type Agent, type FleetMachine, type MachineNode } from '../api';
import { localOffset } from '../metrics/format';
import type { EventRow, View, ViewQuery } from '../metrics/model';
import { href } from '../router';
import { ago, badge, card, empty, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { cap, faint, fallbackPicture, fillViews, isLive, KIND_LABEL, kv, meter, normalizeMachines, sparkSvg, specOf, stateTone, viewsStrip } from './machine-list';
import { engineWanted, followLook, startEngineStage, type EngineStage } from './engine-mount';
import { styleSwitch } from './machine-style';
import { agentLines, finiteValues, gpuCardLine, latestValue, metricValues, vmRows, type GpuCardInfo } from './machine-view-model';
import type { Cleanup } from './index';

const POLL_MS = 5000;
const STATUS_TONE: Record<string, Tone> = { online: 'ok', draining: 'warn', offline: 'muted', revoked: 'danger' };

const gib = (mib: number | null | undefined): string => (Number.isFinite(mib) ? ((mib as number) / 1024).toFixed(1) : '–');

/** The hardware: the CPU, memory, storage and platform, then each GPU with the reference card the engine matched. */
function hardware(f: FleetMachine, cards: GpuCardInfo[] | null): Node[] {
  const gpus = f.gpus.map((g, i) => {
    const ref = cards?.[i]?.card ?? null;
    const line = cards === null ? `${g.vendor} · ${g.memory_mib ? `${gib(g.memory_mib)} GiB` : 'VRAM not reported'}` : gpuCardLine({ gpu: g.name, vendor: g.vendor, memory_mib: g.memory_mib, card: ref });
    return h('div.mt-gpu', null, h('strong', null, g.name), h('div.mt-faint', null, line));
  });
  return [
    kv([
      ['CPU', f.cpu.model || '—'],
      ['Cores', `${f.cpu.cores} cores · ${f.cpu.threads} threads`],
      ['Memory', `${gib(f.memory_mib)} GiB`],
      ['Storage', `${Math.round(f.storage_gib)} GiB`],
      ['Platform', `${f.os} · ${f.arch} · ${f.backend}`],
    ]),
    cap('GPUs'),
    ...(gpus.length ? gpus : [faint('No GPU.')]),
  ];
}

/** The load: CPU and memory from the fleet's live stats, the GPU's utilisation from the metrics ("—" until they arrive). */
function load(f: FleetMachine, gpu: number | null): Node[] {
  const s = f.stats;
  const memPct = s && f.memory_mib > 0 ? (s.mem_used_mib / f.memory_mib) * 100 : 0;
  return [
    meter('CPU', s ? s.cpu_pct : 0, s ? `${s.cpu_pct.toFixed(1)}%` : '—'),
    meter('Memory', memPct, s ? `${gib(s.mem_used_mib)} / ${gib(f.memory_mib)} GiB` : '—'),
    f.gpus.length === 0
      ? meter('GPU', 0, 'No GPU')
      : gpu === null
        ? meter('GPU', 0, '—', 'no GPU metrics')
        : meter('GPU', gpu, `${gpu.toFixed(1)}%`),
  ];
}

/** The VM slots: each VM's id, its agent's name when it has one, and its state. */
function vmList(node: MachineNode | null, byCid: Map<number, Agent>): Node[] {
  const rows = node ? vmRows(node) : [];
  if (!rows.length) return [faint('No VMs on this machine.')];
  return [
    faint(`${rows.length} ${rows.length === 1 ? 'VM' : 'VMs'}`),
    ...rows.map((r) => {
      const agent = r.cid !== null ? byCid.get(r.cid) : undefined;
      return h('div.mt-vrow', null, h('span.mt-mono', null, r.id), h('span.mt-grow', null, agent ? `· ${agentName(agent)}` : ''), badge(r.state, stateTone(r.state)));
    }),
  ];
}

/** The agents on the machine, each linking to its page, with its status. */
function agentList(f: FleetMachine, byCid: Map<number, Agent>): Node[] {
  const lines = agentLines(f.agents, byCid);
  if (!lines.length) return [faint('No agents on this machine.')];
  return [
    faint(`${f.working} working`),
    ...lines.map((a) => h('div.mt-vrow', null, h('a.mt-grow', { href: href('agents', a.cid) }, a.name), badge(a.status, a.status === 'working' ? 'ok' : 'muted'))),
  ];
}

/** The events: the newest few in the last hour. No events (or a failed read) shows the machine's last-seen time. */
function eventList(events: EventRow[], lastSeen: number | null): Node[] {
  if (!events.length) {
    return [h('p', null, 'No recent events'), faint(`Last seen ${lastSeen === null ? '—' : ago(lastSeen)}`)];
  }
  return events
    .slice(0, 8)
    .map((e) => h('div.mt-vrow', null, h('span.mt-faint', null, ago(e.t_ms)), h('span.mt-grow', null, [e.subject, e.detail].filter(Boolean).join(' · ') || e.kind)));
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
  const subEl = h('p.view-sub', null);
  const statusBox = h('span');
  const lookSwitch = styleSwitch(); // the owner's machine style, in the header
  const head = h(
    'div.view-head.mch-head',
    null,
    h('div.mch-head-l', null, h('a.mt-back', { href: href('machines') }, '‹ Machines'), titleEl, subEl),
    h('div.mt-actions', null, statusBox, lookSwitch.el),
  );

  const stageBox = h('div.f3d.f3d-m', null, stage ? canvas : empty('machine', 'Loading the 3D view'));
  const stageCard = card('3D view', [
    stageBox,
    faint('Drag to turn it. The cubes around it are its VM slots, lit while their VM runs; the dots circling it are the agents it hosts.'),
  ]);
  stageCard.classList.add('mt-wide');
  const hwBox = h('div', null, faint('Loading…'));
  const loadBox = h('div');
  const sparkBox = h('div');
  sparkBox.hidden = true;
  const loadCard = card('Load', [loadBox, sparkBox]);
  const strip = viewsStrip();
  const viewsCard = card('Views', strip);
  viewsCard.classList.add('mt-wide');
  viewsCard.hidden = stage === null;
  const vmBox = h('div');
  const agentBox = h('div');
  const eventsBox = h('div', null, faint('—'));
  const missing = h('div');
  missing.hidden = true;
  const grid = h(
    'div.mt-cards',
    null,
    stageCard,
    card('Hardware', [hwBox]),
    loadCard,
    card('VM slots', [vmBox]),
    card('Agents', [agentBox]),
    card('Events', [eventsBox]),
    viewsCard,
  );
  root.append(h('div', null, head, grid, missing));
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
    replace(sparkBox, cpu.length > 0 ? [cap('CPU, last hour'), sparkSvg(cpu, 240, 56, 'CPU over the last hour', 'tall')] : null);
  };

  const paintEvents = (): void => {
    if (events !== null) replace(eventsBox, ...eventList(events, fleet?.last_seen_ms ?? null));
  };

  const paint = (): void => {
    grid.hidden = unknown;
    missing.hidden = !unknown;
    if (unknown) {
      replace(missing, empty('refresh', `No machine ${id}`, undefined, h('a.btn.sm', { href: href('machines') }, 'Back to Machines')));
      return;
    }
    const node = nodes.find((m) => m.id === id) ?? null;
    if (!engine && !pictured && node !== null) {
      pictured = true;
      replace(stageBox, h('div.f3d-fallback', null, fallbackPicture({ spec: specOf(node), active: isLive(node.runs ?? []) })));
    }
    if (fleet === null) return;
    titleEl.textContent = fleet.name;
    replace(statusBox, badge(fleet.status, STATUS_TONE[fleet.status] ?? 'muted'));
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
    stopLook();
    lookSwitch.dispose();
    stage?.destroy();
    canvas.remove();
  };
}
