// The share API: links to an artifact or a Library file that anyone can open, with optional
// expiry, password, pinned version and download. One share can be live per target at a time;
// revoking one ends both of its links.

import { call } from './api';

/** What a share points at: an artifact, or a file in the Library. */
export interface ShareTarget {
  kind: 'artifact' | 'file';
  id: string;
}

export interface ShareLinks {
  /** The link anyone on the internet can open, served through agentplaza.io. */
  public: string;
  /** The link for people on the owner's network. */
  network: string;
}

export interface ShareView {
  id: string;
  target: ShareTarget;
  /** The pinned version, or null when the share follows the latest one. */
  version: number | null;
  has_password: boolean;
  /** Absolute epoch milliseconds, or null for a share that never expires. */
  expires_ms: number | null;
  allow_download: boolean;
  created_by: string;
  created_ms: number;
  revoked_ms: number | null;
  title: string;
  views: number;
  downloads: number;
  state: 'live' | 'revoked' | 'expired';
  links: ShareLinks;
}

export interface ShareEvent {
  share: string;
  at_ms: number;
  kind: 'view' | 'download' | 'unlock_failed';
  via: 'network' | 'relay';
}

/** A new share. Omitted fields take the hub's defaults: latest version, no password, no expiry, no download. */
export interface ShareCreate {
  target: ShareTarget;
  version?: number | null;
  password?: string;
  expires_ms?: number;
  expires_in_days?: number;
  allow_download?: boolean;
}

/** Any subset of a share's settings; `null` clears a setting (no password, no expiry, latest version). */
export interface SharePatch {
  password?: string | null;
  expires_ms?: number | null;
  allow_download?: boolean;
  version?: number | null;
}

/** The `target` query value the hub reads, such as `artifact:<id>`. */
const targetParam = (target: ShareTarget): string => encodeURIComponent(`${target.kind}:${target.id}`);

export const shares = {
  /** Every share of one target, live and ended, newest first as the hub sends them. */
  list: (target: ShareTarget) => call<ShareView[]>('GET', `/v1/shares?target=${targetParam(target)}`),
  create: (body: ShareCreate) => call<ShareView>('POST', '/v1/shares', body),
  update: (id: string, patch: SharePatch) => call<ShareView>('PATCH', `/v1/shares/${encodeURIComponent(id)}`, patch),
  /** Revokes the share and both of its links; the answer is the revoked share. */
  revoke: (id: string) => call<ShareView>('DELETE', `/v1/shares/${encodeURIComponent(id)}`),
  events: (id: string, limit = 50) => call<ShareEvent[]>('GET', `/v1/shares/${encodeURIComponent(id)}/events?limit=${limit}`),
};
