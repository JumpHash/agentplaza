// A project's releases: the list by channel, one release and its report, the artifacts it names per target, and the
// owner's actions on it (cut a candidate, record a check, publish, qualify, promote, yank). Same origin and the same
// `call` as the rest of the console; the types follow the hub's answers.

import { call } from './api';

export type ReleaseState = 'draft' | 'candidate' | 'qualifying' | 'qualified' | 'published' | 'promoted' | 'failed' | 'abandoned' | 'superseded' | 'yanked';

/** A change a release carries, with the review run that judged it (null: not reviewed). */
export interface ReleaseChange { change: string; title: string; review_run: string | null }

/** One built artifact of a release, for one target. */
export interface ReleaseArtifact {
  target: string; artifact: string; digest: string; size: number; signatures: unknown; pool: string; base: string | null; run: string | null;
}

/** A verdict on the release: from a check or a gate, in the order it was given (`seq`). */
export interface ReleaseVerdict { source: string; verdict: string; reason: string; seq: number }

/** A promotion of the release into an environment, with the environment's health after it. */
export interface ReleasePromotion { environment: string; seq: number; health: string }

/** Where the release went on a git platform, if it went. */
export interface ReleaseSync { platform: string; state: string; url: string | null; at_ms: number }

export interface Release {
  id: string; project: string; repo: string; version: string; channel: string; base: string | null; lane: string; from: string;
  state: ReleaseState; number: number | null; notes: string; run: string | null; sbom: string; provenance: string;
  changes: ReleaseChange[]; artifacts: ReleaseArtifact[]; verdicts: ReleaseVerdict[]; promotions: ReleasePromotion[];
  sync: ReleaseSync[]; created_ms: number; updated_ms: number;
}

/** A stage's state in the qualification report. `not_in_depth` is a stage the qualification did not go deep on. */
export type QualificationStageState = 'passed' | 'accepted' | 'failed' | 'not_run' | 'pending' | 'not_in_depth';

/** A link the hub gives as evidence or as a finding: a label, and a hub path (same origin). */
export interface QualificationLink { kind?: string; label?: string; text?: string; id?: string; href: string }

/** One stage of the qualification (1 to 11), with what it found. */
export interface QualificationStage {
  number: number; key: string; title: string; in_depth: boolean; short: boolean; state: QualificationStageState; reason: string;
  evidence: QualificationLink[]; findings: QualificationLink[]; metrics: { name: string; value: number | string }[];
  parts: { name: string; state: string; reason: string }[];
}

/** The qualification record itself (the run that qualified the release). */
export interface QualificationRecord {
  id: string; state: string; verdict: '' | 'passed' | 'failed' | 'incomplete'; reasons: string[];
  depth: { stages: string[]; short: string[]; reason: string; excluded: { stage: string; why: string }[] };
  lab_runs: string[]; review_run: string; started_ms: number; ended_ms: number | null;
}

/** A change in a metric against the release it was compared with. */
export interface QualificationDelta { name: string; previous: number; current: number; change_pct: number }

/** A test the hub has quarantined until it is fixed. `flaked_runs` is a count (or the runs themselves). */
export interface QualificationQuarantine { test: string; since_ms: number; flaked_runs: number | string[]; fix_task: string }

/** A stage the planner asked to accept, and what the planner and a second model answered. */
export interface QualificationAcceptance {
  id: string; release: string; stage: number | string; key: string; reason: string; planner: string; planner_says: string;
  second: string; second_says: string; tokens: number; at_ms: number;
}

/** `GET /v1/releases/{id}/report` → `qualification`: the whole report of a release's qualification. */
export interface QualificationReport {
  release: { id: string; version: string; channel: string; state: string };
  qualification: QualificationRecord;
  stages: QualificationStage[];
  compared_with: { release: string; version: string; id: string } | null;
  deltas: QualificationDelta[];
  findings: { new: string[]; fixed: string[] };
  acceptances: QualificationAcceptance[];
  quarantine: QualificationQuarantine[];
  flaky: string[];
  matrices: { name: string; state: string; reason: string }[];
  cost: { model_tokens: number; lab_minutes: number; lab_runs: number };
  fix_tasks: { stage: string; goal: string; task: string }[];
}

/** `GET /v1/releases/{id}/report`: the release, its run (null when it has none), its life, and its qualification report (null when it has none). */
export interface ReleaseReport { release: Release; run: Record<string, unknown> | null; life: Record<string, unknown>; qualification: QualificationReport | null }

/** `GET /v1/releases/{id}/artifacts/{target}`: one artifact, with what the weft store says about it. */
export interface ReleaseArtifactAnswer { artifact: ReleaseArtifact; weft: Record<string, unknown> }

/** One path segment, encoded. */
const seg = (text: string): string => encodeURIComponent(text);

export const releasesApi = {
  /** A project's releases, newest first. */
  list: (project: string): Promise<Release[]> => call<{ releases: Release[] }>('GET', `/v1/projects/${seg(project)}/releases`).then((r) => r.releases),
  get: (id: string): Promise<Release> => call<Release>('GET', `/v1/releases/${seg(id)}`),
  report: (id: string): Promise<ReleaseReport> => call<ReleaseReport>('GET', `/v1/releases/${seg(id)}/report`),
  artifact: (id: string, target: string): Promise<ReleaseArtifactAnswer> => call<ReleaseArtifactAnswer>('GET', `/v1/releases/${seg(id)}/artifacts/${seg(target)}`),
  /** Cuts a candidate; `number` is the hub's pick when omitted. */
  candidate: (id: string, number?: number): Promise<Release> => call<Release>('POST', `/v1/releases/${seg(id)}/candidate`, number === undefined ? {} : { number }),
  /** Records a check on the candidate: passed or failed, and why. */
  check: (id: string, passed: boolean, reason: string): Promise<Release> => call<Release>('POST', `/v1/releases/${seg(id)}/checks`, { passed, reason }),
  publish: (id: string): Promise<Release> => call<Release>('POST', `/v1/releases/${seg(id)}/publish`, {}),
  qualify: (id: string): Promise<Release> => call<Release>('POST', `/v1/releases/${seg(id)}/qualify`, {}),
  promote: (id: string, environment: string): Promise<Release> => call<Release>('POST', `/v1/releases/${seg(id)}/promote`, { environment }),
  yank: (id: string, reason: string): Promise<Release> => call<Release>('POST', `/v1/releases/${seg(id)}/yank`, { reason }),
};
