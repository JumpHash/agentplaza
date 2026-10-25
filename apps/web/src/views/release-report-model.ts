// The qualification report's pure rules: the words and tones of a verdict and of a stage's state, the rows of the stage
// table (a stage not in depth is muted), the depth and the deltas, the quarantine and the acceptances, and which hub
// links can be made links. The report JSON becomes one view model here (reportView); the DOM is not touched.
// The view (release-report.ts) draws it, and test/release-report-tests.ts checks it against a fixture.

import type { Tone } from '../ui/components';
import type { QualificationAcceptance, QualificationLink, QualificationQuarantine, QualificationReport, QualificationStage } from '../api-releases';

/** A link from the hub that can be made: a same-origin path (`/v1/...`), never a protocol-relative or scheme URL. */
export function isHubPath(href: string): boolean {
  return /^\/(?![/\\])[^\s\\]*$/.test(href);
}

/** A link in the view: its label, and the hub path it goes to (null: the hub's text is shown without a link). */
export interface LinkRow {
  label: string;
  href: string | null;
}

function linkRow(label: string, href: string): LinkRow {
  return { label, href: isHubPath(href) ? href : null };
}

const VERDICT_TEXT: Record<string, { text: string; tone: Tone }> = {
  passed: { text: 'Passed', tone: 'ok' },
  failed: { text: 'Failed', tone: 'danger' },
  incomplete: { text: 'Incomplete', tone: 'warn' },
};

/** A verdict in words and tone. An empty verdict is a qualification that has not concluded. */
export function verdictView(verdict: string): { text: string; tone: Tone } {
  return VERDICT_TEXT[verdict] ?? { text: 'No verdict yet', tone: 'muted' };
}

const STAGE_TEXT: Record<string, string> = {
  passed: 'Passed',
  accepted: 'Accepted',
  failed: 'Failed',
  not_run: 'Not run',
  pending: 'Pending',
  not_in_depth: 'Not in depth',
};

/** A stage's state in words. An unknown state (a newer hub) reads as it is, never as a blank. */
export function stageStateText(state: string): string {
  return STAGE_TEXT[state] ?? state;
}

/** The tone of a stage's state: passed is ok, accepted and pending are the ones to look at, a failure is danger, and
 * the stages that did not run or are not in depth are muted. */
export function stageTone(state: string): Tone {
  switch (state) {
    case 'passed':
      return 'ok';
    case 'accepted':
      return 'info';
    case 'pending':
      return 'warn';
    case 'failed':
      return 'danger';
    default:
      return 'muted';
  }
}

/** A metric's value as text: whole numbers as they are, others to two places; a text value as it is. */
export function metricText(value: number | string): string {
  if (typeof value === 'string') return value;
  if (!Number.isFinite(value)) return '—';
  return Number.isInteger(value) ? String(value) : String(Number(value.toFixed(2)));
}

/** A change in percent with its sign, one decimal place; a dash when the hub gave no number. */
export function changeText(pct: number): string {
  if (!Number.isFinite(pct)) return '—';
  const shown = `${pct.toFixed(1)}%`;
  return pct > 0 ? `+${shown}` : shown;
}

/** A whole number with a comma every three digits (1234567 reads as 1,234,567). */
export function countText(n: number): string {
  if (!Number.isFinite(n)) return '—';
  return Math.round(n).toString().replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

/** The stage rows of the table, in stage order. A stage that is not in depth (or whose state says so) is muted. */
export interface StageRow {
  number: number;
  key: string;
  title: string;
  state: string;
  stateText: string;
  tone: Tone;
  muted: boolean;
  reason: string;
  evidence: LinkRow[];
  findings: LinkRow[];
  metrics: string[];
  parts: string[];
}

function stageRow(s: QualificationStage): StageRow {
  return {
    number: s.number,
    key: s.key,
    title: s.title,
    state: s.state,
    stateText: stageStateText(s.state),
    tone: stageTone(s.state),
    muted: !s.in_depth || s.state === 'not_in_depth',
    reason: s.reason,
    evidence: (s.evidence ?? []).map((e: QualificationLink) => linkRow(e.label || e.kind || e.href, e.href)),
    findings: (s.findings ?? []).map((f: QualificationLink) => linkRow(f.text || f.id || f.href, f.href)),
    metrics: (s.metrics ?? []).map((m) => `${m.name} ${metricText(m.value)}`),
    parts: (s.parts ?? []).map((p) => (p.reason ? `${p.name}: ${stageStateText(p.state)} (${p.reason})` : `${p.name}: ${stageStateText(p.state)}`)),
  };
}

/** The depth in lines: the stages in depth, the short ones, why the depth was chosen, and each excluded stage with its why. */
export interface DepthView {
  inDepth: string[];
  short: string[];
  reason: string;
  excluded: { stage: string; why: string }[];
}

export interface DeltaRow {
  name: string;
  previous: string;
  current: string;
  change: string;
  /** The direction of the change, for a tone: up, down, or flat. */
  direction: 'up' | 'down' | 'flat';
}

export interface QuarantineRow {
  test: string;
  since_ms: number;
  flaked: string;
  fixTask: string;
}

export interface AcceptanceRow {
  stage: string;
  key: string;
  reason: string;
  planner: string;
  plannerSays: string;
  second: string;
  secondSays: string;
  tokens: string;
  at_ms: number;
}

/** The report as the view draws it: every list is in the order the hub gave it, and the empty ones are empty arrays. */
export interface ReportView {
  verdict: { text: string; tone: Tone };
  reasons: string[];
  depth: DepthView;
  stages: StageRow[];
  comparedWith: string | null;
  deltas: DeltaRow[];
  findings: { new: string[]; fixed: string[] };
  quarantine: QuarantineRow[];
  flaky: string[];
  acceptances: AcceptanceRow[];
  matrices: { name: string; state: string; reason: string }[];
  cost: { tokens: string; labMinutes: string; labRuns: string };
  fixTasks: { stage: string; goal: string; task: string }[];
  labRuns: string[];
  reviewRun: string;
}

/** The depth of a qualification. A stage that is excluded from it carries the hub's why. */
export function depthView(depth: QualificationReport['qualification']['depth']): DepthView {
  return {
    inDepth: [...(depth.stages ?? [])],
    short: [...(depth.short ?? [])],
    reason: depth.reason ?? '',
    excluded: (depth.excluded ?? []).map((e) => ({ stage: e.stage, why: e.why })),
  };
}

/** The deltas as rows: the previous and the current value of each metric, and the change in percent. */
export function deltaRows(deltas: QualificationReport['deltas']): DeltaRow[] {
  return (deltas ?? []).map((d) => ({
    name: d.name,
    previous: metricText(d.previous),
    current: metricText(d.current),
    change: changeText(d.change_pct),
    direction: d.change_pct > 0 ? 'up' : d.change_pct < 0 ? 'down' : 'flat',
  }));
}

/** A count of flaked runs: the number the hub gave, or the length of the list of runs. */
export function flakedText(flaked: number | string[]): string {
  const n = Array.isArray(flaked) ? flaked.length : flaked;
  return n === 1 ? '1 flaked run' : `${countText(n)} flaked runs`;
}

/** The quarantine, every quarantined test in the order the hub gave it, until it is fixed. */
export function quarantineRows(list: QualificationQuarantine[]): QuarantineRow[] {
  return (list ?? []).map((q) => ({
    test: q.test,
    since_ms: q.since_ms,
    flaked: flakedText(q.flaked_runs ?? 0),
    fixTask: q.fix_task || '—',
  }));
}

/** The acceptances: the stage, the reason, the planner and the second model with what each answered, and the tokens. */
export function acceptanceRows(list: QualificationAcceptance[]): AcceptanceRow[] {
  return (list ?? []).map((a) => ({
    stage: String(a.stage),
    key: a.key,
    reason: a.reason,
    planner: a.planner,
    plannerSays: a.planner_says,
    second: a.second,
    secondSays: a.second_says,
    tokens: countText(a.tokens),
    at_ms: a.at_ms,
  }));
}

/** The whole report as the view draws it. */
export function reportView(report: QualificationReport): ReportView {
  const q = report.qualification;
  const stages = [...(report.stages ?? [])].sort((a, b) => a.number - b.number).map(stageRow);
  return {
    verdict: verdictView(q.verdict),
    reasons: [...(q.reasons ?? [])],
    depth: depthView(q.depth),
    stages,
    comparedWith: report.compared_with ? report.compared_with.version : null,
    deltas: deltaRows(report.deltas),
    findings: { new: [...(report.findings?.new ?? [])], fixed: [...(report.findings?.fixed ?? [])] },
    quarantine: quarantineRows(report.quarantine),
    flaky: [...(report.flaky ?? [])],
    acceptances: acceptanceRows(report.acceptances),
    matrices: [...(report.matrices ?? [])],
    cost: {
      tokens: countText(report.cost?.model_tokens ?? 0),
      labMinutes: metricText(report.cost?.lab_minutes ?? 0),
      labRuns: countText(report.cost?.lab_runs ?? 0),
    },
    fixTasks: [...(report.fix_tasks ?? [])],
    labRuns: [...(q.lab_runs ?? [])],
    reviewRun: q.review_run ?? '',
  };
}
