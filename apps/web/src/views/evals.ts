// Evals: what the step sessions cost and how they did (PLAN 25.2, 13.1), as the owner reads it. The cost and success
// per step type and model as a grid; the cost report grouped by step, model, task or project; the reviewer models'
// precision and misses; and the benchmark's replays of archived tasks. Read by polling every ten seconds, as the
// history view does: the live bus does not carry the eval records.

import { evalsApi, type BenchRun, type CostBy, type CostRow, type EvalScore, type ReviewerScore } from '../api-evals';
import { card, empty, select, table, badge, type Column } from '../ui/components';
import { h, replace } from '../ui/dom';
import { dollarsText } from './budget-model';
import {
  COST_GROUPS,
  benchVerdict,
  costPerSuccessText,
  durationText,
  grid,
  gridKey,
  percentText,
  reviewerLine,
  successText,
  type GridCell,
} from './evals-model';

/** How often the view reads the records again. */
const POLL_MS = 10_000;

interface Loaded {
  by: CostBy;
  costs: CostRow[];
  scores: EvalScore[];
  reviewers: ReviewerScore[];
  bench: BenchRun[];
}

/** Renders the evals screen into `root`; returns the cleanup that stops its polling. */
export function mount(root: HTMLElement, _params: string[]): () => void {
  let by: CostBy = 'step';
  let loaded: Loaded | null = null;
  let failure = '';
  let stopped = false;

  const draw = () => {
    if (!stopped) replace(root, render());
  };
  const onGroup = (value: string) => {
    by = value as CostBy;
    void load();
  };
  const render = (): HTMLElement => {
    if (failure) return h('div.evals', null, empty('zap', 'The eval records did not load', failure));
    if (!loaded) return h('div.evals', null, empty('zap', 'Loading the eval records'));
    return h(
      'div.evals',
      null,
      h('div.page-h', null, h('h1', null, 'Evals'), h('p.sub', null, 'What the step sessions cost, and how they did')),
      card('Cost and success by step type and model', scoreGrid(loaded.scores)),
      card(
        'Cost report',
        [groupSelect(loaded.by, onGroup), costTable(loaded.costs, loaded.by)],
      ),
      card('Reviewers: precision and misses', reviewerTable(loaded.reviewers)),
      card('Benchmark: archived tasks replayed', benchTable(loaded.bench)),
    );
  };

  const load = async () => {
    try {
      const [costs, scores, reviewers, bench] = await Promise.all([
        evalsApi.costs(by),
        evalsApi.leaderboard(),
        evalsApi.reviewers(),
        evalsApi.bench(),
      ]);
      loaded = { by, costs, scores, reviewers, bench };
      failure = '';
    } catch (err) {
      failure = err instanceof Error ? err.message : String(err);
    }
    draw();
  };

  const timer = setInterval(() => void load(), POLL_MS);
  void load();
  return () => {
    stopped = true;
    clearInterval(timer);
  };
}

/** The grid of step types down and models across: each cell says how many succeeded, the cost and the quality. */
function scoreGrid(scores: EvalScore[]): HTMLElement {
  const g = grid(scores);
  if (g.steps.length === 0) return empty('zap', 'No step has run yet', 'Its runs appear here as they end.');
  const columns: Column<string>[] = [
    { label: 'Step type', cell: (step) => h('b', null, step), width: '18%' },
    ...g.models.map<Column<string>>((model) => ({
      label: model,
      cell: (step) => cellView(g.cells.get(gridKey(step, model))),
    })),
  ];
  return table(columns, g.steps);
}

/** One grid cell: "2 of 3 succeeded", the cost per success, the time per success and the quality. */
function cellView(cell: GridCell | undefined): HTMLElement | string {
  if (!cell) return '—';
  return h(
    'div.cell',
    null,
    h('div', null, h('b', null, successText(cell.successes, cell.runs)), ` · ${percentText(cell.success_rate)}`),
    h('div.muted', null, `${costPerSuccessText(cell.cost_per_success_micros)} · ${durationText(cell.time_per_success_ms)}`),
    h('div.muted', null, `${percentText(cell.quality)} clean`),
  );
}

function groupSelect(current: CostBy, onChange: (value: string) => void): HTMLElement {
  const control = select(COST_GROUPS, current);
  control.addEventListener('change', () => onChange(control.value));
  return h('div.row', null, h('span.muted', null, 'Group by'), control);
}

function costTable(rows: CostRow[], by: CostBy): HTMLElement {
  if (rows.length === 0) return empty('zap', 'No runs to report yet');
  const columns: Column<CostRow>[] = [
    { label: groupLabel(by), cell: (r) => h('b', null, r.key) },
    { label: 'Runs', cell: (r) => String(r.runs), num: true },
    { label: 'Succeeded', cell: (r) => successText(r.successes, r.runs), num: true },
    { label: 'Cost', cell: (r) => dollarsText(r.cost_micros), num: true },
    { label: 'Per success', cell: (r) => costPerSuccessText(r.cost_per_success_micros), num: true },
    { label: 'Time per success', cell: (r) => durationText(r.time_per_success_ms), num: true },
    { label: 'Clean', cell: (r) => percentText(r.quality), num: true },
  ];
  return table(columns, rows, undefined, (r) => r.key);
}

function groupLabel(by: CostBy): string {
  return COST_GROUPS.find((g) => g.value === by)?.label ?? by;
}

function reviewerTable(rows: ReviewerScore[]): HTMLElement {
  if (rows.length === 0) return empty('zap', 'No reviewer has ruled on a finding yet');
  const columns: Column<ReviewerScore>[] = [
    { label: 'Reviewer model', cell: (r) => h('b', null, r.model) },
    { label: 'Changes passed', cell: (r) => String(r.changes_passed), num: true },
    { label: 'Precision', cell: (r) => percentText(r.precision), num: true },
    { label: 'Misses', cell: (r) => String(r.misses), num: true },
    { label: 'Summary', cell: (r) => h('span.muted', null, reviewerLine(r)) },
  ];
  return table(columns, rows, undefined, (r) => r.model);
}

function benchTable(rows: BenchRun[]): HTMLElement {
  if (rows.length === 0) return empty('zap', 'No archived task has been replayed yet');
  const columns: Column<BenchRun>[] = [
    { label: 'Archived case', cell: (r) => `#${r.archive}` },
    { label: 'Known outcome', cell: (r) => r.known },
    { label: 'Replay', cell: (r) => badge(r.outcome, r.matched ? 'muted' : 'muted') },
    { label: 'Verdict', cell: (r) => benchVerdict(r.outcome, r.known, r.matched) },
    { label: 'Cost', cell: (r) => dollarsText(r.cost_micros), num: true },
    { label: 'Wall time', cell: (r) => durationText(r.wall_ms), num: true },
  ];
  return table(columns, rows, undefined, (r) => String(r.id));
}
