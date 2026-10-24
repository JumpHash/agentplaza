// The Workflows tab's pure rules: the lines a workflow, a job and a step read as, the words and tones of a run's state,
// the counts of a GitHub Actions conversion and the lines of what it could not convert. The DOM is not touched here;
// the tab (project-workflows.ts) draws these, and test/workflows-tests.ts checks them.

import { clip, type Tone } from '../ui/components';
import type { ImportItem, ImportReport, WorkflowError, WorkflowJob, WorkflowRun, WorkflowStep, WorkflowSummary } from '../api-workflows';

/** `1 job`, `2 jobs`. */
export function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? '' : 's'}`;
}

/** The triggers of a workflow, joined; a workflow with none runs only when asked. */
export function triggersText(triggers: string[]): string {
  return triggers.length ? triggers.join(', ') : 'no triggers';
}

/** A workflow in the list: its name, its triggers, and its job and step counts. */
export function workflowSummaryLine(w: Pick<WorkflowSummary, 'name' | 'triggers' | 'jobs' | 'steps'>): string {
  return `${w.name}: ${triggersText(w.triggers)} · ${plural(w.jobs, 'job')} · ${plural(w.steps, 'step')}`;
}

/** A file that did not parse, as one line. */
export function errorLine(e: WorkflowError): string {
  return `${e.file}: ${e.error}`;
}

const RUN_LABELS: Record<WorkflowRun['state'], string> = {
  running: 'Running',
  owner: 'Waiting for you',
  passed: 'Passed',
  failed: 'Failed',
  cancelled: 'Cancelled',
};

/** A run's state in words. An unknown state (a newer hub) reads as it is, never as a blank. */
export function runStateLabel(state: string): string {
  return RUN_LABELS[state as WorkflowRun['state']] ?? state;
}

/** The tone of a run's state: running is info, waiting for the owner is warn, passed is ok, failed is danger. */
export function runStateTone(state: string): Tone {
  switch (state) {
    case 'running':
      return 'info';
    case 'owner':
      return 'warn';
    case 'passed':
      return 'ok';
    case 'failed':
      return 'danger';
    default:
      return 'muted';
  }
}

/** The words of a step, joined; empty words are left out so a missing environment does not leave a gap. */
function words(...parts: (string | false | undefined)[]): string {
  return parts.filter((p): p is string => Boolean(p)).join(' ');
}

/**
 * A step as one line: `run: npm test`, `test (changed)`, `deploy staging (ephemeral, ttl 30 min)`, `approval: Ship it?`.
 * A kind this screen does not know reads as its kind.
 */
export function stepLine(step: WorkflowStep): string {
  const environment = typeof step.environment === 'string' ? step.environment : '';
  switch (step.kind) {
    case 'run': {
      const command = typeof step.run === 'string' ? clip(step.run.replace(/\s+/g, ' ').trim(), 120) : '';
      return command ? `run: ${command}` : 'run';
    }
    case 'test':
      return step.select ? `test (${String(step.select)})` : 'test';
    case 'build':
      return 'build';
    case 'deploy': {
      const ttl = typeof step.ttl_minutes === 'number' ? `ttl ${step.ttl_minutes} min` : '';
      const extras = [step.ephemeral ? 'ephemeral' : '', ttl].filter(Boolean).join(', ');
      return words('deploy', environment, extras ? `(${extras})` : '');
    }
    case 'against':
      return words('against', environment);
    case 'promote':
      return words('promote to', environment);
    case 'teardown':
      return words('teardown', environment);
    case 'approval':
      return typeof step.message === 'string' && step.message ? `approval: ${step.message}` : 'approval';
    default:
      return String(step.kind);
  }
}

/** The `needs` of a job: the jobs it waits for, or none. */
export function needsLine(needs: string[]): string {
  return needs.length ? `Needs ${needs.join(', ')}` : 'Needs nothing';
}

/** A job's matrix as one line: `os: linux, macos · node: 20, 22`; empty when the job has no matrix. */
export function matrixLine(matrix: WorkflowJob['matrix']): string {
  return Object.entries(matrix)
    .map(([key, values]) => `${key}: ${values.join(', ')}`)
    .join(' · ');
}

/** The counts of a conversion: what mapped, what was noted, and what could not be converted. */
export interface ImportCounts {
  mapped: number;
  notes: number;
  unsupported: number;
}

export function importCounts(report: ImportReport): ImportCounts {
  return { mapped: report.mapped.length, notes: report.notes.length, unsupported: report.unsupported.length };
}

/** The counts as one line, as the conversion's header reads them. */
export function importCountsLine(counts: ImportCounts): string {
  return `${counts.mapped} mapped · ${plural(counts.notes, 'note')} · ${counts.unsupported} unsupported`;
}

/** One thing the conversion could not carry over: where it was, what it was, and why it was left out. */
export function unsupportedLine(u: ImportReport['unsupported'][number]): string {
  return `${u.at}: ${u.what}. ${u.reason}`;
}

/** A conversion's lines for the preview: the unsupported items first (they are what the owner must act on), then the notes. */
export function importLines(item: ImportItem): { unsupported: string[]; notes: string[]; mapped: string[] } {
  return {
    unsupported: item.report.unsupported.map(unsupportedLine),
    notes: [...item.report.notes],
    mapped: [...item.report.mapped],
  };
}

/** The line a saved conversion leaves: the change to land. */
export function savedText(change: string): string {
  return `Saved in change ${change}. Land it to add the workflow.`;
}

/** The line a started run leaves: its id and its first state. */
export function startedText(run: WorkflowRun): string {
  return `Run ${run.run} is ${runStateLabel(run.state).toLowerCase()}.`;
}
