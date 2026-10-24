// A project's workflows: the CI/CD pipelines its repo declares as `.plaza/workflows/<name>.toml`, one workflow in full,
// its runs (each a recipe run, read in the run view), a manual run, and the GitHub Actions workflows under
// `.github/workflows/` that the hub converts to plaza's format for a preview or a save. Same origin and the same `call`
// as the rest of the console; the types follow the hub's answers. The pure rules are in views/workflows-model.ts.

import { call } from './api';

/** A step's kind; the fields a kind carries (`run`, `environment`, `ephemeral`, `ttl_minutes`, `select`, `message`) are optional. */
export type WorkflowStepKind = 'run' | 'test' | 'build' | 'deploy' | 'against' | 'promote' | 'teardown' | 'approval';

export interface WorkflowStep {
  kind: WorkflowStepKind;
  run?: string;
  environment?: string;
  ephemeral?: boolean;
  ttl_minutes?: number;
  select?: string;
  message?: string;
  [key: string]: unknown;
}

/** A workflow in the project's list, and a file that did not parse (`error` is null for a workflow). */
export interface WorkflowSummary { name: string; file: string; triggers: string[]; cron: string; jobs: number; steps: number; error: null }
export interface WorkflowError { file: string; error: string }
export interface WorkflowList { workflows: WorkflowSummary[]; errors: WorkflowError[] }

export interface WorkflowJob { id: string; needs: string[]; runs_on: string; matrix: { [key: string]: string[] }; steps: WorkflowStep[] }
export interface WorkflowDetail { name: string; file: string; description: string; triggers: string[]; cron: string; jobs: WorkflowJob[] }

/** A run's state: `running`, `owner` (a card waits for the owner), or how it ended. */
export type WorkflowRunState = 'running' | 'owner' | 'passed' | 'failed' | 'cancelled';

/** A run of a workflow, as the runs list and the start answer give it. */
export interface WorkflowRun {
  run: string; recipe: string; version: number; source: string; trigger: string; task: string | null; project: string | null;
  subject: string | null; state: WorkflowRunState; outcome: string | null; created_ms: number; updated_ms: number;
}
export interface WorkflowRunList { runs: WorkflowRun[] }

/** What a GitHub Actions workflow converts to: what carried over, what was noted, and what could not be converted. */
export interface ImportReport {
  mapped: string[];
  notes: string[];
  unsupported: { at: string; what: string; reason: string }[];
}

/** A GitHub Actions workflow converted for a preview: its plaza name, its TOML and its report. */
export interface ImportItem { file: string; name: string; toml: string; report: ImportReport }
export interface ImportList { imports: ImportItem[]; errors: WorkflowError[] }

/** A save: the converted workflow, and the change that writes it (the owner lands the change). */
export interface SavedImport extends ImportItem { change: string }

/** One path segment, encoded. */
const seg = (text: string): string => encodeURIComponent(text);

export const workflowsApi = {
  /** The project's workflows, and the files that failed to parse. */
  list: (project: string): Promise<WorkflowList> => call<WorkflowList>('GET', `/v1/projects/${seg(project)}/workflows`),
  /** One workflow in full: its triggers, its jobs and their steps. */
  get: (project: string, name: string): Promise<WorkflowDetail> => call<WorkflowDetail>('GET', `/v1/projects/${seg(project)}/workflows/${seg(name)}`),
  /** The workflow's recent runs, newest first. */
  runs: (project: string, name: string): Promise<WorkflowRun[]> =>
    call<WorkflowRunList>('GET', `/v1/projects/${seg(project)}/workflows/${seg(name)}/runs`).then((r) => r.runs),
  /** Starts a manual run of the workflow; the answer is the new run, in its first state. */
  startRun: (project: string, name: string): Promise<WorkflowRun> => call<WorkflowRun>('POST', `/v1/projects/${seg(project)}/workflows/${seg(name)}/runs`, {}),
  /** The GitHub Actions workflows in the repo, converted for a preview. */
  imports: (project: string): Promise<ImportList> => call<ImportList>('GET', `/v1/projects/${seg(project)}/workflows/import`),
  /** One conversion, as a preview: nothing is written. */
  previewImport: (project: string, file: string): Promise<ImportItem> =>
    call<ImportItem>('POST', `/v1/projects/${seg(project)}/workflows/import`, { file, save: false }),
  /** Saves one conversion to `.plaza/workflows/<name>.toml` through a change; the answer names the change. */
  saveImport: (project: string, file: string): Promise<SavedImport> =>
    call<SavedImport>('POST', `/v1/projects/${seg(project)}/workflows/import`, { file, save: true }),
};
