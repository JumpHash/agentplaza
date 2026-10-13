// The Machines popup of the Floor and its pure helpers, shared by the three.js Floor (floor.ts) and the wasm engine's
// mount (engine-mount.ts). No three.js here, so the wasm mount does not load the plaza's code.
import type { FleetMachine } from '../api';
import { href } from '../router';
import { ago, button } from '../ui/components';
import { h } from '../ui/dom';

/** How many agent names the popup lists before "+N more". */
export const FLEET_CARD_AGENTS = 6;

/** The name label's style: the three.js plaza's labels and the wasm engine's share it. */
export const LABEL_CSS =
  'position:absolute;left:0;top:0;display:none;align-items:center;gap:6px;max-width:200px;padding:3px 7px;' +
  'background:var(--sheet);border:1px solid var(--ink);box-shadow:var(--print);color:var(--ink);' +
  'font:600 10.5px/1.35 var(--mono);white-space:nowrap;' +
  'pointer-events:none;will-change:transform';

/** A memory size in GiB: whole, or one decimal; a size the fleet does not know (null) reads as a dash. */
export function gibText(mib: number | null | undefined): string {
  if (mib == null) return '—';
  return `${Math.round(mib / 102.4) / 10} GiB`;
}

/** The popup's subtitle: the kind and the model, "workstation · ASUS ROG …". */
export function machineSubtitle(m: Pick<FleetMachine, 'kind' | 'model'>): string {
  return m.model ? `${m.kind} · ${m.model}` : m.kind;
}

/**
 * The GPUs as one line per kind: identical cards grouped, "8× NVIDIA H100 · 80 GiB". A GPU's memory is null when the
 * fleet does not know it: its name stands alone. The kinds keep the order their first card has in the fleet.
 */
export function gpuLines(gpus: FleetMachine['gpus']): string {
  if (gpus.length === 0) return 'none';
  const kinds = new Map<string, number>();
  for (const g of gpus) {
    const label = g.memory_mib == null ? g.name : `${g.name} · ${gibText(g.memory_mib)}`;
    kinds.set(label, (kinds.get(label) ?? 0) + 1);
  }
  return [...kinds].map(([label, n]) => (n > 1 ? `${n}× ${label}` : label)).join('\n');
}

/** The facts the popup lists, in order. The last-seen time is live, so the card adds it itself. */
export function machineFacts(m: FleetMachine): [string, string][] {
  return [
    ['CPU', m.cpu.model || '—'],
    ['Cores', `${m.cpu.cores} cores · ${m.cpu.threads} threads`],
    ['Memory', gibText(m.memory_mib)],
    ['GPUs', gpuLines(m.gpus)],
    ['Arch', m.arch || '—'],
    ['Backend', m.backend || '—'],
    ['OS', m.os || '—'],
    ['Role', m.role],
    ['Status', m.status],
  ];
}

/** The agents line: the working and idle split while the machine runs, otherwise its state. */
export function agentTally(total: number, working: number, status: FleetMachine['status']): string {
  if (total === 0) return 'No agents';
  const noun = total === 1 ? 'agent' : 'agents';
  if (status === 'online' || status === 'draining') return `${total} ${noun}: ${working} working, ${total - working} idle`;
  return `${total} ${noun} on a machine that is ${status}`;
}

/**
 * The popup's children: the head, the facts (with the live "Last seen" value, returned as `seen`), the agents and the
 * ENTER footer. The caller puts them in its card and keeps `seen` to update it.
 */
export function machineCard(
  m: FleetMachine,
  nameOf: (cid: number) => string,
  onEnter: () => void,
  now: number,
): { children: HTMLElement[]; seen: HTMLElement } {
  const facts = machineFacts(m);
  const names = m.agents.map((cid) => ({ cid, name: nameOf(cid) }));
  const seen = h('dd');
  seen.textContent = ago(m.last_seen_ms, now);
  const children = [
    h('header.floor-card-head', null, h('strong', null, m.name), h('span.floor-tag', null, m.status)),
    h('div.floor-card-activity', null, machineSubtitle(m)),
    h(
      'dl.machine-facts',
      null,
      ...facts.flatMap(([k, v]) => [h('dt', null, k), h('dd', null, v)]),
      h('dt', null, 'Last seen'),
      seen,
    ),
    h(
      'div.machine-agents',
      null,
      h('div.floor-card-activity', null, agentTally(m.agents.length, m.working, m.status)),
      h(
        'div.machine-agent-names',
        null,
        ...names.slice(0, FLEET_CARD_AGENTS).map(({ cid, name }) => h('a', { href: href('agents', cid) }, name)),
        names.length > FLEET_CARD_AGENTS ? h('span', null, `+${names.length - FLEET_CARD_AGENTS} more`) : null,
      ),
    ),
    h('footer.machine-card-foot', null, button('ENTER', onEnter, { kind: 'primary' })),
  ];
  return { children, seen };
}
