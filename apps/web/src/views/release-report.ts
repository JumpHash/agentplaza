// A release's qualification report: `#/releases/<id>/report`. Its verdict and reasons, its depth (the stages in depth, the
// short ones, and each excluded stage with its why), the eleven stages with their state, reason and evidence, the findings,
// the deltas against the release it was compared with, the quarantine and the flaky tests, the acceptances (the planner and
// the second model with what each said), the matrices, the cost and the fix tasks. The rules are in release-report-model.ts;
// the API is api-releases.ts. Every text from the hub is set as text (h() and the components), never as markup; a hub path
// becomes a link only when it is a same-origin path.

import { releasesApi, type QualificationRecord, type ReleaseReport } from '../api-releases';
import { href } from '../router';
import { ago, badge, card, empty, stat, table, type Column } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import {
  reportView,
  stageStateText,
  stageTone,
  type AcceptanceRow,
  type DeltaRow,
  type DepthView,
  type LinkRow,
  type QuarantineRow,
  type ReportView,
  type StageRow,
} from './release-report-model';
import { stateText } from './releases-model';

/** The report page of release `id`: it loads the report once, and redraws on a failed reload with what it had. */
export function reportPage(view: HTMLElement, id: string): () => void {
  let alive = true;
  /** Undefined until the first answer. */
  let answer: ReleaseReport | undefined;
  let failed = false;

  const back = (): HTMLElement => h('a.rel-back', { href: href('releases', id) }, lu('arrow-left', 14), h('span', null, 'Release'));

  /** A hub link as a link, or as its text when the hub gave no same-origin path. */
  const linkOf = (row: LinkRow): Node => (row.href ? h('a', { href: row.href }, row.label) : h('span', null, row.label));

  const verdictCard = (r: ReportView, q: QualificationRecord): HTMLElement =>
    card(
      'Verdict',
      h(
        'div',
        null,
        h(
          'dl.kv',
          null,
          h('dt', null, 'Qualification'),
          h('dd', null, q.id || '—'),
          h('dt', null, 'State'),
          h('dd', null, q.state || '—'),
          h('dt', null, 'Started'),
          h('dd', null, ago(q.started_ms)),
          h('dt', null, 'Ended'),
          h('dd', null, q.ended_ms === null ? 'Still running' : ago(q.ended_ms)),
          h('dt', null, 'Review run'),
          h('dd', null, r.reviewRun ? h('code', null, r.reviewRun) : '—'),
          h('dt', null, 'Lab runs'),
          h('dd', null, r.labRuns.length ? r.labRuns.map((run) => h('code', { class: 'rel-run' }, run)) : '—'),
        ),
        r.reasons.length ? h('ul.rel-list', null, r.reasons.map((reason) => h('li', null, reason))) : h('p.muted', null, 'No reasons given.'),
      ),
    );

  const depthCard = (r: ReportView): HTMLElement =>
    card(
      'Depth',
      h(
        'div',
        null,
        h(
          'dl.kv',
          null,
          h('dt', null, 'In depth'),
          h('dd', null, r.depth.inDepth.length ? r.depth.inDepth.map((k) => h('code', { class: 'rel-run' }, k)) : '—'),
          h('dt', null, 'Short'),
          h('dd', null, r.depth.short.length ? r.depth.short.map((k) => h('code', { class: 'rel-run' }, k)) : '—'),
          h('dt', null, 'Why'),
          h('dd', null, r.depth.reason || '—'),
        ),
        h('h3', null, 'Excluded'),
        r.depth.excluded.length
          ? table<DepthView['excluded'][number]>(
              [
                { label: 'Stage', cell: (e) => h('code', null, e.stage) },
                { label: 'Why', cell: (e) => e.why },
              ],
              r.depth.excluded,
            )
          : h('p.muted', null, 'No stage is excluded from the depth.'),
      ),
      { flush: true },
    );

  const stagesCard = (rows: StageRow[]): HTMLElement => {
    if (!rows.length) return card('Stages', h('p.muted', null, 'No stages recorded.'));
    const columns: Column<StageRow>[] = [
      { label: '#', cell: (s) => s.number, num: true, width: '3ch' },
      { label: 'Stage', cell: (s) => h('div', null, h('b', null, s.title), ' ', h('code', null, s.key)) },
      { label: 'State', cell: (s) => badge(s.stateText, s.tone) },
      {
        label: 'Reason',
        cell: (s) =>
          h(
            'div',
            null,
            s.reason || '—',
            s.metrics.length ? h('div.rel-faint', null, s.metrics.join(' · ')) : null,
            s.parts.length ? h('ul.rel-list.rel-faint', null, s.parts.map((p) => h('li', null, p))) : null,
          ),
      },
      { label: 'Evidence', cell: (s) => (s.evidence.length ? h('div.rel-links', null, s.evidence.map((e) => h('div', null, linkOf(e)))) : '—') },
    ];
    const t = table(columns, rows);
    // A stage that is not in depth is shown muted, row by row, in the order of the rows.
    const trs = t.tBodies[0]?.rows;
    rows.forEach((s, i) => {
      if (s.muted) trs?.[i]?.classList.add('rel-muted');
    });
    return card('Stages', t, { flush: true });
  };

  const deltasCard = (r: ReportView): HTMLElement => {
    const sub = r.comparedWith ? `Against ${r.comparedWith}` : 'Not compared with an earlier release.';
    const rows: DeltaRow[] = r.deltas;
    return card(
      'Deltas',
      h(
        'div',
        null,
        h('p.muted', null, sub),
        rows.length
          ? table<DeltaRow>(
              [
                { label: 'Metric', cell: (d) => h('code', null, d.name) },
                { label: 'Previous', cell: (d) => d.previous, num: true },
                { label: 'Current', cell: (d) => d.current, num: true },
                { label: 'Change', cell: (d) => h('span', { class: `rel-dir-${d.direction}` }, d.change), num: true },
              ],
              rows,
            )
          : h('p.muted', null, r.comparedWith ? 'No metric to compare.' : 'Nothing to compare yet.'),
      ),
      { flush: true },
    );
  };

  const findingsCard = (r: ReportView): HTMLElement => {
    const withFindings = r.stages.filter((s) => s.findings.length);
    const list = (items: string[], none: string): Node =>
      items.length ? h('ul.rel-list', null, items.map((text) => h('li', null, text))) : h('p.muted', null, none);
    return card(
      'Findings',
      h(
        'div',
        null,
        h('h3', null, 'New'),
        list(r.findings.new, 'No new findings.'),
        h('h3', null, 'Fixed'),
        list(r.findings.fixed, 'No fixed findings.'),
        withFindings.length
          ? [
              h('h3', null, 'By stage'),
              h(
                'ul.rel-list',
                null,
                withFindings.map((s) => h('li', null, h('b', null, s.title), ': ', s.findings.flatMap((f, i) => [i ? ', ' : '', linkOf(f)]))),
              ),
            ]
          : null,
      ),
    );
  };

  const quarantineCard = (rows: QuarantineRow[]): HTMLElement =>
    card(
      'Quarantine',
      rows.length
        ? table<QuarantineRow>(
            [
              { label: 'Test', cell: (q) => h('code', null, q.test) },
              { label: 'Since', cell: (q) => ago(q.since_ms) },
              { label: 'Flaked', cell: (q) => q.flaked },
              { label: 'Fix task', cell: (q) => (q.fixTask === '—' ? '—' : h('code', null, q.fixTask)) },
            ],
            rows,
          )
        : h('p.muted', null, 'No test is quarantined.'),
      { flush: true },
    );

  const flakyCard = (items: string[]): HTMLElement =>
    card('Flaky', items.length ? h('ul.rel-list', null, items.map((t) => h('li', null, h('code', null, t)))) : h('p.muted', null, 'No flaky test.'));

  const acceptancesCard = (rows: AcceptanceRow[]): HTMLElement =>
    card(
      'Acceptances',
      rows.length
        ? rows.map((a) =>
            h(
              'div.rel-acc',
              null,
              h('p', null, h('b', null, `Stage ${a.stage}`), ' ', h('code', null, a.key)),
              h('p.rel-notes', null, a.reason),
              h(
                'dl.kv',
                null,
                h('dt', null, 'Planner'),
                h('dd', null, a.planner || '—', a.plannerSays ? h('div.rel-faint', null, a.plannerSays) : null),
                h('dt', null, 'Second model'),
                h('dd', null, a.second || '—', a.secondSays ? h('div.rel-faint', null, a.secondSays) : null),
                h('dt', null, 'Tokens'),
                h('dd', null, a.tokens),
                h('dt', null, 'At'),
                h('dd', null, ago(a.at_ms)),
              ),
            ),
          )
        : h('p.muted', null, 'No acceptances.'),
    );

  const matricesCard = (rows: ReportView['matrices']): HTMLElement =>
    card(
      'Matrices',
      rows.length
        ? table<ReportView['matrices'][number]>(
            [
              { label: 'Matrix', cell: (m) => h('code', null, m.name) },
              { label: 'State', cell: (m) => badge(stageStateText(m.state), stageTone(m.state)) },
              { label: 'Reason', cell: (m) => m.reason || '—' },
            ],
            rows,
          )
        : h('p.muted', null, 'No matrices.'),
      { flush: true },
    );

  const costCard = (r: ReportView): HTMLElement =>
    card('Cost', h('div.rel-stats', null, stat('Model tokens', r.cost.tokens), stat('Lab minutes', r.cost.labMinutes), stat('Lab runs', r.cost.labRuns)));

  const fixTasksCard = (rows: ReportView['fixTasks']): HTMLElement =>
    card(
      'Fix tasks',
      rows.length
        ? table(
            [
              { label: 'Stage', cell: (t) => h('code', null, t.stage) },
              { label: 'Goal', cell: (t) => t.goal },
              { label: 'Task', cell: (t) => (t.task ? h('code', null, t.task) : '—') },
            ],
            rows,
          )
        : h('p.muted', null, 'No fix tasks.'),
      { flush: true },
    );

  const draw = (): void => {
    if (!alive) return;
    if (answer === undefined) {
      replace(
        view,
        back(),
        failed
          ? h('div.rel-error', null, h('p', null, 'The report could not be loaded.'))
          : h('p.muted', null, 'Loading the report…'),
      );
      return;
    }
    const r = answer.release;
    const q = answer.qualification;
    const head = (verdict: Node | null): HTMLElement =>
      h(
        'header.rel-h',
        null,
        h('h1', null, `Qualification of ${r.version}`),
        verdict,
        h('p.muted', null, `${r.channel} channel · release ${stateText(r.state)}`),
      );
    if (!q) {
      replace(view, back(), head(badge('No verdict yet', 'muted')), empty('package', 'Not qualified yet', 'The report shows here once the release has a qualification.'));
      return;
    }
    const v = reportView(q);
    replace(
      view,
      back(),
      head(badge(v.verdict.text, v.verdict.tone)),
      verdictCard(v, q.qualification),
      depthCard(v),
      stagesCard(v.stages),
      deltasCard(v),
      findingsCard(v),
      quarantineCard(v.quarantine),
      flakyCard(v.flaky),
      acceptancesCard(v.acceptances),
      matricesCard(v.matrices),
      costCard(v),
      fixTasksCard(v.fixTasks),
    );
  };

  const load = async (): Promise<void> => {
    try {
      answer = await releasesApi.report(id);
      failed = false;
    } catch {
      failed = true;
    }
    draw();
  };

  draw();
  void load();
  return () => {
    alive = false;
  };
}
