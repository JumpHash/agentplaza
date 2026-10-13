// The plan diagram of a dry run (`POST /v1/foundry/plan`): the mode and its facts, then one box per
// island with its backend, its leader and rendezvous, and each machine with the device indexes the
// island takes from it. Used by the New job form. The job detail draws its own view of a stored plan.

import type { JobPlan } from '../api';
import { h } from '../ui/dom';
import { fmtNumber } from './lab-common';

const MODE_NOTE: Record<string, string> = {
  single: 'One island on one machine: the job trains in a single process group.',
  tight: 'The islands train as one group, synchronised every step over the leader’s rendezvous.',
  loose: 'Each island trains on its own and the islands average through the hub between inner rounds.',
};

type Island = JobPlan['islands'][number];

function fact(label: string, value: string): HTMLElement {
  return h('div.lab-jobs-fact', null, h('span.lab-stat-label', null, label), h('span.lab-mono', null, value));
}

function islandBox(island: Island): HTMLElement {
  return h(
    'section.lab-jobs-island',
    null,
    h('header.lab-jobs-island-head', null, h('span.lab-mono', null, island.id), h('span.lab-badge.accent', null, island.backend)),
    h(
      'dl.lab-jobs-kv',
      null,
      h('dt', null, 'Leader'),
      h('dd.lab-mono', null, island.leader || '—'),
      h('dt', null, 'Rendezvous'),
      h('dd.lab-mono', null, island.rdzv || '—'),
    ),
    h(
      'ul.lab-jobs-machines',
      null,
      island.machines.map((m) =>
        h(
          'li',
          null,
          h('span.lab-mono', null, m.id),
          h(
            'span.lab-jobs-devs',
            null,
            m.devices.map((d) => h('span.lab-jobs-dev', null, `device ${d}`)),
          ),
        ),
      ),
    ),
  );
}

/** The plan of a dry run as a diagram: its facts first, then the islands. */
export function planDiagram(plan: JobPlan): HTMLElement {
  const facts: HTMLElement[] = [
    fact('Mode', plan.mode),
    fact('Devices', fmtNumber(plan.devices_total)),
    fact('Inner steps', fmtNumber(plan.inner_steps)),
    fact('Staleness', fmtNumber(plan.staleness)),
  ];
  if (plan.rounds !== undefined) facts.push(fact('Rounds', fmtNumber(plan.rounds)));
  facts.push(fact('Frozen', plan.frozen ?? 'none'));
  return h(
    'div.lab-jobs-plan',
    null,
    h('div.lab-jobs-facts', null, facts),
    MODE_NOTE[plan.mode] ? h('p.lab-muted', null, MODE_NOTE[plan.mode]) : null,
    plan.islands.length ? h('div.lab-jobs-islands', null, plan.islands.map(islandBox)) : h('p.lab-muted', null, 'The plan has no islands.'),
    h('div.lab-jobs-out', null, h('span.lab-stat-label', null, 'Output'), h('span.lab-mono', null, plan.output_dir || '—')),
  );
}
