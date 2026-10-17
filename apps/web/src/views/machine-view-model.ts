// The machine view's pure helpers: the VM rows of a machine, the reference-card line of a GPU, and the
// series a metrics view carries. No DOM here, so the tests read them directly.
import type { Agent, MachineNode } from '../api';
import type { View } from '../metrics/model';

/** One VM of a machine: its run and index (the `r12/3` the events name), its state and its agent's CID, if any. */
export interface VmRow {
  id: string;
  run: string;
  index: number;
  state: string;
  cid: number | null;
}

/** The VM rows of a machine's runs, in run order; a run without its vms lists none. */
export function vmRows(m: Pick<MachineNode, 'runs'>): VmRow[] {
  return (m.runs ?? []).flatMap((r) =>
    (r.vms ?? []).map((vm) => ({ id: `${r.id}/${vm.index}`, run: r.id, index: vm.index, state: vm.state, cid: vm.guest_cid })),
  );
}

/** A reference card the engine matched to one GPU (the engine's `gpu_cards` JSON). */
export interface GpuCard {
  id: string;
  name: string;
  segment: string;
  form: string;
  slots: number;
  size_mm: [number, number, number] | null;
  cooling: string;
  fans: number;
  generic: boolean;
}

/** One GPU of a machine with the card it matched, or null when no card matched. */
export interface GpuCardInfo {
  gpu: string;
  vendor: string;
  memory_mib: number | null;
  card: GpuCard | null;
}

/** The engine's gpu_cards JSON as the list of GPUs; null for `null`, or for JSON of another shape. */
export function parseGpuCards(json: string): GpuCardInfo[] | null {
  try {
    const v: unknown = JSON.parse(json);
    return Array.isArray(v) ? (v as GpuCardInfo[]) : null;
  } catch {
    return null;
  }
}

/** "RTX 4090 · Founders · 3-slot · open air (generic)"; a GPU with no card says so. */
export function gpuCardLine(g: GpuCardInfo): string {
  if (g.card === null) return `${g.gpu} · no reference card`;
  const c = g.card;
  const text = [c.name, c.form, c.slots > 0 ? `${c.slots}-slot` : '', c.cooling].filter(Boolean).join(' · ');
  return c.generic ? `${text} (generic)` : text;
}

/** The first chart series of the view's panel for `metric`, or [] when the view has none (a hub without metrics
 *  answers a partial or empty view). */
export function metricValues(view: View | null, metric: string): (number | null)[] {
  const panel = (view?.panels ?? []).find((p) => p.metric === metric);
  if (!panel || panel.body?.type !== 'chart') return [];
  return panel.body.series?.[0]?.values ?? [];
}

/** The values that are numbers, in order: what a sparkline draws. */
export function finiteValues(values: (number | null)[]): number[] {
  return values.filter((v): v is number => typeof v === 'number' && Number.isFinite(v));
}

/** The newest value that is a number, or null when there is none. */
export function latestValue(values: (number | null)[]): number | null {
  for (let i = values.length - 1; i >= 0; i--) {
    const v = values[i];
    if (typeof v === 'number' && Number.isFinite(v)) return v;
  }
  return null;
}

/** One agent of a machine: its CID, its name (a CID with no agent in the list), and its status. */
export interface AgentLine {
  cid: number;
  name: string;
  status: string;
}

/** The machine's agents, by CID, in the order the fleet lists them; an unknown CID keeps its number as the name. */
export function agentLines(cids: number[], byCid: Map<number, Agent>): AgentLine[] {
  return cids.map((cid) => {
    const a = byCid.get(cid);
    return { cid, name: a?.name ?? `cid ${cid}`, status: a?.status ?? 'unknown' };
  });
}
