// A project's CVE report tab: the advisories that reach the project's packages, most severe first. The header says how
// many there are and how many are open or fixed, the priorities, when the report was last polled and what each feed said
// (a feed that failed is a warning, not hidden). The table says each advisory's package and version on main, its id and
// aliases, its reachability, its fix and the releases it reaches. The owner polls the feeds now, refreshes the report,
// or downloads it as CSV or JSON. The rules are in cves-model.ts; the API is api-cves.ts. The markup is the project
// page's (projects.css) and cves.css. Every text from the hub is set as text, never as markup.

import '../styles/cves.css';
import { cvesApi, type CveReport } from '../api-cves';
import type { Project } from '../api';
import { attempt, badge, button, card, empty, field, select, stat, table, type Column } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import {
  PRIORITY_CHOICES,
  emptyTitle,
  feedLine,
  feedWarnings,
  feedWarningText,
  countsLine,
  polledText,
  priorityBreakdown,
  visibleRows,
  type CveRow,
} from './cves-model';

/** The CVE report of project `p`. It loads the report, and redraws after a poll or a refresh; a read-only page (archived) cannot poll. */
export function cvesTab(p: Project): HTMLElement {
  const root = h('div.pj-tab.cve');
  const writable = !p.archived;
  /** Undefined until the first answer. A failed reload keeps the report it had. */
  let report: CveReport | undefined;
  let failed = false;
  let polling = false;
  let priority = '';

  const load = async (): Promise<void> => {
    try {
      report = await cvesApi.report(p.id);
      failed = false;
    } catch {
      failed = true;
    }
    draw();
  };

  /** Asks the hub to poll the feeds now, then reads the report. The hub polls in the background, so the answer may still be the old one. */
  const poll = async (): Promise<void> => {
    if (polling) return;
    polling = true;
    draw();
    const asked = await attempt(() => cvesApi.poll(p.id), 'Polling the feeds now');
    polling = false;
    if (asked) await load();
    else draw();
  };

  const pollButton = (kind: 'primary' | 'ghost'): HTMLButtonElement =>
    button('Poll now', () => void poll(), { kind, icon: 'scan', disabled: polling });

  const intro = (): HTMLElement =>
    h(
      'div.cve-intro',
      null,
      h('p', null, 'Advisories that reach this project’s packages, most severe first. Each one says whether the project reaches it, and what the fix is.'),
      h(
        'div.cve-acts',
        null,
        writable ? pollButton('primary') : null,
        button('Refresh', () => void load(), { kind: 'ghost', icon: 'refresh-cw', disabled: polling }),
        h('a.btn.ghost', { href: cvesApi.exportUrl(p.id, 'csv'), download: `${p.id}-cves.csv` }, lu('download', 15), h('span', null, 'CSV')),
        h('a.btn.ghost', { href: cvesApi.exportUrl(p.id, 'json'), download: `${p.id}-cves.json` }, lu('download', 15), h('span', null, 'JSON')),
      ),
    );

  const totals = (r: CveReport): HTMLElement =>
    card(
      'Advisories',
      [
        h('div.cve-stats', null, stat('Total', r.counts.total), stat('Open', r.counts.open), stat('Fixed', r.counts.fixed)),
        h(
          'div.cve-chips',
          null,
          priorityBreakdown(r.counts.by_priority ?? {}).map((b) => badge(`${b.label} ${b.count}`, b.tone)),
        ),
        h('p.cve-polled', null, `${countsLine(r.counts)} · ${polledText(r.polled_ms)}`),
      ],
    );

  const feeds = (r: CveReport): HTMLElement =>
    card(
      'Feeds',
      r.feeds.length
        ? r.feeds.map((f) => {
            const warning = feedWarnings([f])[0];
            return h(
              'div.cve-feed',
              null,
              h('b', null, f.feed),
              h('span.pj-faint', null, feedLine(f)),
              warning ? h('p.cve-warn', { role: 'alert' }, lu('triangle-alert', 15), h('span', null, feedWarningText(warning))) : null,
            );
          })
        : h('p.pj-faint', null, 'No feed has been polled yet.'),
    );

  const columns: Column<CveRow>[] = [
    { label: 'Priority', cell: (row) => badge(row.priorityLabel, row.priorityTone) },
    {
      label: 'Package',
      cell: (row) => h('div.cve-pkg', null, h('b', null, row.pkg), h('span.pj-faint', null, `${row.ecosystem} · ${row.main}`)),
    },
    {
      label: 'Advisory',
      cell: (row) =>
        h(
          'div.cve-adv',
          null,
          h('b', null, row.primary),
          row.aliases.length ? h('span.pj-faint', null, `also ${row.aliases.join(', ')}`) : null,
          row.url ? h('a', { href: row.url, target: '_blank', rel: 'noopener' }, lu('external-link', 12), h('span', null, 'Advisory')) : null,
        ),
    },
    { label: 'Title', cell: (row) => h('span.cve-title', null, row.title) },
    {
      label: 'Reachability',
      cell: (row) =>
        h('div.cve-reach', null, badge(row.reach, row.reachTone), row.reachNote ? h('small.pj-faint', { title: row.reachNote }, row.reachNote) : null),
    },
    {
      label: 'Fix',
      cell: (row) =>
        h(
          'div.cve-fix',
          null,
          h('span', null, row.fixed),
          badge(row.fix, row.fixTone),
          row.fixTask ? h('code', null, row.fixTask) : null,
          row.fixReason ? h('small.pj-faint', null, row.fixReason) : null,
        ),
    },
    {
      label: 'Releases',
      cell: (row) => (row.releases.length ? h('div.cve-rels', null, row.releases.map((l) => h('span', null, l))) : h('span.pj-faint', null, 'None')),
    },
  ];

  const findings = (r: CveReport): HTMLElement => {
    const filter = select(PRIORITY_CHOICES, priority);
    filter.addEventListener('change', () => {
      priority = filter.value;
      draw();
    });
    if (!r.findings.length) {
      return empty('shield-alert', emptyTitle(r.polled_ms > 0), undefined, writable ? pollButton('primary') : undefined);
    }
    const rows = visibleRows(r, priority);
    return h(
      'div',
      null,
      h('div.cve-filter', null, field('Priority', filter)),
      rows.length
        ? card(null, h('div.tbl-wrap', null, table(columns, rows, undefined, (row) => row.id)), { flush: true })
        : h('p.pj-faint', null, 'No advisory has this priority.'),
    );
  };

  const body = (): Node[] => {
    if (report === undefined) {
      return [
        failed
          ? h('div.cve-acts', null, h('p.pj-faint', null, 'The CVE report could not be loaded.'), button('Try again', () => void load(), { kind: 'ghost' }))
          : h('p.pj-faint', null, 'Loading the CVE report…'),
      ];
    }
    const r = report;
    return [totals(r), feeds(r), findings(r)];
  };

  const draw = (): void => {
    replace(root, intro(), body());
  };

  draw();
  void load();
  return root;
}
