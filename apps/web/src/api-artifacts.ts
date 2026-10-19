// The owner's artifact routes on the hub (`/v1/artifacts`). Every JSON call goes through the shared request helper in
// api.ts; the types are in view/model.ts. This module only fetches and links; it draws nothing.

import { call, raw } from './api';
import { href } from './router';
import type { Artifact, ArtifactSummary, RowsPage, VersionInfo } from './view/model';

/** How long a fetched artifact is reused. Embeds in one view share the fetch; a newer version appears within a minute. */
const TTL_MS = 60_000;

/** The paging, sorting and filter of a dataset panel's rows (the same shape the renderers pass). */
export interface RowsParams {
  offset: number;
  limit: number;
  sort?: string;
  desc?: boolean;
  filter?: string;
}

/** A query string from the defined parameters: `?q=x&limit=50`, or `''`. */
function query(params: Record<string, string | number | undefined>): string {
  const parts = Object.entries(params)
    .filter(([, v]) => v !== undefined && v !== '')
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(String(v))}`);
  return parts.length ? `?${parts.join('&')}` : '';
}

const path = (id: string): string => `/v1/artifacts/${encodeURIComponent(id)}`;

/** The hash route of an artifact's viewer, with `?v=` when a version is named. */
export function artifactHref(id: string, version: number | null = null): string {
  return href('artifact', id) + (version !== null ? `?v=${version}` : '');
}

/** The artifacts the owner can see, newest first as the hub sorts them. */
export function listArtifacts(filter: { project?: string; q?: string } = {}): Promise<ArtifactSummary[]> {
  return call<ArtifactSummary[]>('GET', `/v1/artifacts${query(filter)}`);
}

/** One artifact at a version (the newest when `version` is omitted). */
export function getArtifact(id: string, version?: number): Promise<Artifact> {
  return call<Artifact>('GET', `${path(id)}${query({ v: version })}`);
}

/** Every saved version of an artifact, for the version switcher. */
export function listVersions(id: string): Promise<VersionInfo[]> {
  return call<VersionInfo[]>('GET', `${path(id)}/versions`);
}

/** One page of a dataset panel's rows. */
export function artifactRows(id: string, panel: string, q: RowsParams): Promise<RowsPage> {
  const params = {
    panel,
    offset: q.offset,
    limit: q.limit,
    sort: q.sort,
    desc: q.desc ? 1 : undefined,
    filter: q.filter,
  };
  return call<RowsPage>('GET', `${path(id)}/rows${query(params)}`);
}

/** The URL of a live frame for an html panel at a version. The hub answers the URL. */
export function artifactFrameUrl(id: string, version: number, panel: string): Promise<{ url: string }> {
  return call<{ url: string }>('GET', `${path(id)}/frame${query({ v: version, panel })}`);
}

/** The URL of one blob (an image, a file) of an artifact. */
export function artifactBlobUrl(id: string, blob: string): string {
  return `${path(id)}/blobs/${encodeURIComponent(blob)}`;
}

/** Downloads an export (CSV or JSON of a dataset panel, or of every panel when `panel` is omitted). */
export async function downloadExport(id: string, format: 'csv' | 'json', panel: string | null, name: string): Promise<void> {
  const res = await raw('GET', `${path(id)}/export${query({ format, panel: panel ?? undefined })}`);
  const url = URL.createObjectURL(await res.blob());
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** Deletes an artifact and all its versions. The hub refuses it for anyone but its owner. */
export function deleteArtifact(id: string): Promise<unknown> {
  return call<unknown>('DELETE', path(id));
}

const cache = new Map<string, { at: number; value: Promise<Artifact> }>();

/**
 * An artifact fetched once and reused for a minute, keyed by id and version. A failed fetch is not kept, so the next
 * request asks again.
 */
export function cachedArtifact(id: string, version?: number): Promise<Artifact> {
  const key = `${id}@${version ?? 'latest'}`;
  const hit = cache.get(key);
  if (hit && Date.now() - hit.at < TTL_MS) return hit.value;
  const value = getArtifact(id, version);
  cache.set(key, { at: Date.now(), value });
  value.catch(() => {
    if (cache.get(key)?.value === value) cache.delete(key);
  });
  return value;
}
