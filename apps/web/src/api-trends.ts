// A project's trends across its releases, oldest first: the performance metrics each release measured, the artifact sizes
// per target, when its qualification started, and its flake rate. Same origin and the same `call` as the rest of the
// console; the types follow the hub's answers.

import { call } from './api';

/** One release's place in the trends. `flake_rate` is a fraction of the runs that flaked (null: none measured). */
export interface TrendRelease {
  release: string;
  version: string;
  channel: string;
  verdict: string;
  metrics: { name: string; value: number }[];
  sizes: { target: string; size: number }[];
  start_ms: number | null;
  flake_rate: number | null;
}

/** One path segment, encoded. */
const seg = (text: string): string => encodeURIComponent(text);

export const trendsApi = {
  /** `GET /v1/projects/{id}/trends`: the project's releases, oldest first. */
  list: (project: string): Promise<TrendRelease[]> =>
    call<{ releases: TrendRelease[] }>('GET', `/v1/projects/${seg(project)}/trends`).then((r) => r.releases),
};
