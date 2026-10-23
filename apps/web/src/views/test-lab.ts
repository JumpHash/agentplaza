// The Test lab view (plan 22.5): the machines that take qualification runs, the share of each the lab takes and what it
// has free for the lab, the queue in the order runs will be admitted, the runs that are running, and the last that
// ended. Read-only: the roles are set on the Nodes screen. Refreshes every few seconds.

import { api, type LabNode, type LabRun, type TestLabOverview } from '../api';
import { badge, card, empty, type Tone } from '../ui/components';
import { h } from '../ui/dom';
import { pageHead, tableWrap } from './vault-ui';
import type { Cleanup } from './index';

const REFRESH_MS = 5000;

const CHANNEL: Record<string, Tone> = { yank: 'danger', hotfix: 'danger', stable: 'ok', beta: 'info', nightly: 'muted' };

/** A node's roles as the owner reads them: work and test percentages. */
export function rolesText(n: LabNode): string {
  return `work ${n.work_share}% · test ${n.test_share}%`;
}

/** The lab's free capacity on a node, as CPUs and memory of what the lab gives it. */
export function freeText(n: LabNode): string {
  if (n.test_share === 0) return '—';
  return `${n.free_cpus} of ${n.lab_cpus} CPUs, ${n.free_memory_mib} of ${n.lab_memory_mib} MiB`;
}

function runRow(run: LabRun, extra: Node[] = []): HTMLElement {
  return h(
    'tr',
    null,
    h('td', null, run.id),
    h('td', null, run.project),
    h('td', null, badge(run.channel, CHANNEL[run.channel] ?? 'muted')),
    h('td', null, `${run.cpus} CPUs, ${run.memory_mib} MiB`),
    h('td', null, run.reservations.map((r) => r.node).join(', ') || '—'),
    ...extra,
  );
}

const RUN_HEAD = ['Run', 'Project', 'Channel', 'Asks for', 'Nodes'];

function table(head: string[], rows: HTMLElement[]): HTMLElement {
  return tableWrap(h('table', null, h('thead', null, h('tr', null, ...head.map((t) => h('th', null, t)))), h('tbody', null, ...rows)));
}

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  const body = h('div');
  root.replaceChildren(pageHead('Test lab'), body);

  const paint = (o: TestLabOverview | null): void => {
    if (!o) {
      body.replaceChildren(card('The lab', empty('link', 'The lab could not be read', 'It will be read again in a few seconds.')));
      return;
    }
    const nodes = o.nodes.length
      ? table(
          ['Machine', 'Label', 'Roles', 'Lab capacity free', 'Capabilities'],
          o.nodes.map((n) =>
            h(
              'tr',
              null,
              h('td', null, n.name),
              h('td', null, n.label || '—'),
              h('td', null, rolesText(n)),
              h('td', null, freeText(n)),
              h('td', null, n.capabilities.join(', ') || '—'),
            ),
          ),
        )
      : empty('link', 'No machine takes test runs', 'Set a test share on a machine on the Nodes screen.');
    const queue = o.queue.length
      ? table(RUN_HEAD, o.queue.map((run) => runRow(run)))
      : empty('link', 'Nothing is queued', 'Qualification runs wait here by channel: yank and hotfix first, then stable, beta and nightly.');
    const running = o.running.length ? table(RUN_HEAD, o.running.map((run) => runRow(run))) : empty('link', 'Nothing is running in the lab');
    const done = o.done.length
      ? table(
          [...RUN_HEAD, 'Outcome'],
          o.done.map((run) => runRow(run, [h('td', null, run.outcome ?? '—')])),
        )
      : empty('link', 'No run has ended yet');
    body.replaceChildren(card('Machines', nodes, { flush: true }), card('Queue', queue, { flush: true }), card('Running', running, { flush: true }), card('Ended', done, { flush: true }));
  };

  const load = async (): Promise<void> => {
    const o = await api.testLab().catch(() => null);
    if (!alive) return;
    paint(o);
  };
  void load();
  const timer = window.setInterval(() => void load(), REFRESH_MS);
  return () => {
    alive = false;
    window.clearInterval(timer);
  };
}
