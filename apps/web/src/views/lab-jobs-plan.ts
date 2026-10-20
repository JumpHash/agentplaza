// The plan of a dry run (`POST /v1/foundry/plan`) as the prototype lays it out in the New job form:
// a key-value list (mode, devices, inner steps, staleness, rounds, frozen, islands, output dir). The
// job detail draws its own view of a stored plan.

import type { JobPlan } from '../api';
import { h } from '../ui/dom';
import { fmtNumber } from './lab-common';

type Island = JobPlan['islands'][number];

/** One island as the prototype's line: its backend, leader, rendezvous and each machine's devices. */
function islandLine(island: Island): string {
  const machines = island.machines.map((m) => `${m.id} (devices ${m.devices.join(', ')})`).join('; ');
  return `Island ${island.id}: ${island.backend}, leader ${island.leader}, rendezvous ${island.rdzv}, machines ${machines}`;
}

/** The plan of a dry run as the form's key-value list. */
export function planDiagram(plan: JobPlan): HTMLElement {
  const loose = plan.mode === 'loose';
  const backend = plan.islands[0]?.backend ?? '—';
  const pairs: [string, Node | string][] = [
    ['Mode', loose ? `loose, ${fmtNumber(plan.rounds ?? 0)} rounds, staleness ${fmtNumber(plan.staleness)}` : plan.mode],
    ['Devices', `${fmtNumber(plan.devices_total)} on ${backend}`],
    ['Inner steps', fmtNumber(plan.inner_steps)],
    ['Staleness', loose ? fmtNumber(plan.staleness) : '—'],
    ['Rounds', plan.rounds === undefined ? '—' : fmtNumber(plan.rounds)],
    ['Frozen', plan.frozen ?? 'none'],
    ['Islands', h('span', null, ...plan.islands.flatMap((i, n) => (n ? [h('br'), islandLine(i)] : [islandLine(i)])))],
    ['Output dir', h('span.mono', null, plan.output_dir || '—')],
  ];
  return h(
    'dl.kv',
    null,
    pairs.map(([k, v]) => h('div', null, h('dt', null, k), h('dd', null, v))),
  );
}
