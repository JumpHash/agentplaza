// A project's CVE report: the advisories that reach the project's packages, what the hub's feeds said when they were last
// polled, and the fix each one has (or has not). The hub's module is crates/apid/src/cves/ (routes.rs names the routes);
// the types follow its answers. The report is also downloadable as CSV or JSON.

import { call } from './api';

/** A finding's priority, and its severity as the source gave it. */
export type CvePriority = 'critical' | 'high' | 'medium' | 'low' | 'unknown';
export type CveEcosystem = 'cargo' | 'npm' | 'pip' | 'go';
export type CveFeedName = 'osv' | 'github' | 'rustsec' | 'main';
export type CveReachability = 'reachable' | 'unreachable' | 'unknown';
export type CveState = 'open' | 'fixed';
/** A fix's state: empty when none was asked for; otherwise the upgrade flow's own words (see upgrades/mod.rs). */
export type CveFixState =
  | ''
  | 'started'
  | 'task'
  | 'current'
  | 'tests_failed'
  | 'apply_failed'
  | 'review_failed'
  | 'task_failed'
  | 'refused'
  | 'not_built'
  | 'no_fix'
  | 'plan_failed'
  | 'fixed';

/** One feed's last poll: how many advisories it named, when, and the error it gave when it failed (empty when it did not). */
export interface CveFeed { feed: CveFeedName; at_ms: number; count: number; error: string }
export interface CveCounts { total: number; open: number; fixed: number; by_priority: Record<string, number> }
/** A release that ships the affected package, and the environments it reaches. */
export interface CveRelease { release: string; version: string; environments: string[] }

export interface CveFinding {
  id: string;
  /** The advisory ids: the first is the main one, the others are aliases of it. */
  ids: string[];
  ecosystem: CveEcosystem;
  package: string;
  title: string;
  severity: CvePriority;
  priority: CvePriority;
  cvss: string;
  published: string;
  source: string;
  url: string;
  functions: string[];
  /** `main`'s version of the package when `main` is affected, else empty. */
  main_version: string;
  main_environments: string[];
  /** The release that fixes it, empty when the source names none. */
  fixed: string;
  reachability: CveReachability;
  reach_note: string;
  state: CveState;
  fix_state: CveFixState;
  fix_reason: string;
  /** The plaza task the fix became, empty when it did not. */
  fix_task: string;
  releases: CveRelease[];
  first_ms: number;
  updated_ms: number;
}

/** `GET /v1/projects/{id}/cves`: the findings are already sorted most severe first. `polled_ms` is 0 when never polled. */
export interface CveReport {
  project: string;
  polled_ms: number;
  feeds: CveFeed[];
  counts: CveCounts;
  findings: CveFinding[];
}

const enc = encodeURIComponent;

export const cvesApi = {
  /** The project's report. */
  report: (project: string) => call<CveReport>('GET', `/v1/projects/${enc(project)}/cves`),
  /** One finding, by its id (`cargo:h2:RUSTSEC-2024-0003`). */
  finding: (project: string, cid: string) => call<CveFinding>('GET', `/v1/projects/${enc(project)}/cves/${enc(cid)}`),
  /** Polls the feeds now. The hub answers 202 and polls in the background, so the report may still be the old one. */
  poll: (project: string) => call<{ project: string }>('POST', `/v1/projects/${enc(project)}/cves/poll`),
  /** The address of the report as a download (the browser's own session cookie signs it). */
  exportUrl: (project: string, format: 'csv' | 'json'): string => `/v1/projects/${enc(project)}/cves/export?format=${format}`,
};
