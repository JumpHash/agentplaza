// A project's trends tab (`#/projects/<id>/trends`): the releases oldest first, with four small tables. The performance
// metrics each release measured (one column per metric name), the artifact sizes per target, when each qualification
// started, and the flake rate. The rules are in trends-model.ts; the API is api-trends.ts. The markup is the project page's
// (projects.css) and the console's tables and cards. Every text from the hub is set as text, never as markup.

import { trendsApi, type TrendRelease } from '../api-trends';
import type { Project } from '../api';
import { href } from '../router';
import { ago, badge, card, empty, table, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import { verdictView } from './release-report-model';
import { flakeText, metricCell, metricNames, targetNames, sizeCell, startText } from './trends-model';

/** The trends of project `p`: it loads them once, and a failed reload keeps the trends it had. */
export function trendsTab(p: Project): HTMLElement {
  const root = h('div.pj-tab.trends');
  /** Undefined until the first answer. */
  let rows: TrendRelease[] | undefined;
  let failed = false;

  const versionCell = (r: TrendRelease): Node => h('a', { href: href('releases', r.release) }, r.version || r.release);
  const verdictCell = (r: TrendRelease): Node => {
    const v = verdictView(r.verdict);
    return badge(v.text, v.tone);
  };

  const performance = (list: TrendRelease[]): HTMLElement => {
    const names = metricNames(list);
    if (!names.length) return card('Performance', h('p.muted', null, 'No metric measured yet.'));
    const columns: Column<TrendRelease>[] = [
      { label: 'Release', cell: versionCell },
      { label: 'Verdict', cell: verdictCell },
      ...names.map((name): Column<TrendRelease> => ({ label: name, cell: (r) => metricCell(r, name), num: true })),
    ];
    return card('Performance', table(columns, list), { flush: true });
  };

  const sizes = (list: TrendRelease[]): HTMLElement => {
    const targets = targetNames(list);
    if (!targets.length) return card('Artifact sizes', h('p.muted', null, 'No artifact size recorded yet.'));
    const columns: Column<TrendRelease>[] = [
      { label: 'Release', cell: versionCell },
      ...targets.map((target): Column<TrendRelease> => ({ label: target, cell: (r) => sizeCell(r, target), num: true })),
    ];
    return card('Artifact sizes', table(columns, list), { flush: true });
  };

  const starts = (list: TrendRelease[]): HTMLElement =>
    card(
      'Start times',
      table<TrendRelease>(
        [
          { label: 'Release', cell: versionCell },
          { label: 'Channel', cell: (r) => r.channel || '—' },
          { label: 'Verdict', cell: verdictCell },
          { label: 'Started', cell: (r) => (r.start_ms === null ? '—' : h('span', { title: ago(r.start_ms) }, startText(r.start_ms))) },
        ],
        list,
      ),
      { flush: true },
    );

  const flakes = (list: TrendRelease[]): HTMLElement =>
    card(
      'Flake rates',
      table<TrendRelease>(
        [
          { label: 'Release', cell: versionCell },
          { label: 'Flake rate', cell: (r) => flakeText(r.flake_rate), num: true },
        ],
        list,
      ),
      { flush: true },
    );

  const draw = (): void => {
    if (rows === undefined) {
      replace(root, failed ? h('div.rel-error', null, h('p', null, 'The trends could not be loaded.')) : h('p.muted', null, 'Loading the trends…'));
      return;
    }
    if (!rows.length) {
      replace(root, empty('activity', 'No releases yet', 'The trends fill in as the project’s releases are cut and qualified.'));
      return;
    }
    const list = rows;
    replace(
      root,
      h('p.muted', null, `${list.length} ${list.length === 1 ? 'release' : 'releases'}, oldest first.`),
      performance(list),
      sizes(list),
      starts(list),
      flakes(list),
    );
  };

  const load = async (): Promise<void> => {
    try {
      rows = await trendsApi.list(p.id);
      failed = false;
    } catch {
      failed = true;
    }
    draw();
  };

  draw();
  void load();
  return root;
}
