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

/** `GET /v1/releases/{id}/report`: the release, its run (null when it has none) and its life, as the hub reports it. */
export interface ReleaseReport { release: Release; run: Record<string, unknown> | null; life: Record<string, unknown> }

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
