// A project's life beyond its plan: its mode (build or maintenance), what its watchers keep in maintenance (signals,
// the map, proposals, sweeps), where it runs (environments and deploys), adopting a project that exists already, and
// the files the owner brings in (into its repository, or attached to a message). The hub's modules are
// crates/apid/src/{projects,maintenance,environments,adopt,project_files,deliverables}.rs; the types follow their
// answers.

import { call, raw, type Project } from './api';

export type ProjectMode = 'build' | 'maintenance';
export interface ModeDef { id: ProjectMode; label: string; summary: string }

// ---- Maintenance -------------------------------------------------------------------------------------------------

export type SignalKind = 'feedback' | 'report' | 'analytics' | 'error' | 'usage' | 'note';
export const SIGNAL_KINDS: SignalKind[] = ['feedback', 'report', 'analytics', 'error', 'usage', 'note'];
export interface Signal {
  id: string; seq: number; kind: SignalKind; title: string; text: string; from: string;
  url?: string; environment?: string; at_ms: number;
}

export type Health = 'good' | 'watch' | 'poor' | 'unknown';
export interface Area {
  id: string; name: string; summary: string; paths: string[]; health: Health; notes: string; links: string[];
  updated_ms: number; by: string;
}
export interface ProductMap { summary: string; areas: Area[]; updated_ms: number; by: string }

export type ProposalKind = 'fix' | 'improve' | 'feature' | 'security' | 'performance' | 'chore' | 'instrument';
export type Risk = 'low' | 'medium' | 'high';
export type Size = 's' | 'm' | 'l' | 'xl';
export type ProposalState = 'proposed' | 'approved' | 'declined' | 'later' | 'withdrawn';
export interface Proposal {
  id: string; kind: ProposalKind; title: string; summary: string; area?: string;
  impact: { changes: string; users: string; areas: string[]; risk: Risk; risk_note: string };
  /** 1 (little) to 5 (a lot), and why. */
  value: { score: number; why: string };
  effort: { size: Size; note: string };
  evidence: { signal?: string; note: string }[];
  /** Data the watchers lack, and what would give it to them. `catalog`: a connector or MCP preset the owner sets up. */
  instrument?: { kind: 'connector' | 'mcp' | 'analytics' | 'tool' | 'other'; name: string; catalog?: string; data: string };
  role: string; acceptance: string; state: ProposalState;
  by_cid: number; by: string; created_ms: number; decided_ms: number; note: string;
  /** The task it became, and that task's state. */
  task?: string; task_state?: string;
}

export interface Sweep {
  runs: number; last_ms: number; next_ms: number;
  running?: { work: number; cid: number; started_ms: number; why: string };
  last_summary: string; last_cid: number; seen_seq: number;
}

export interface MaintenanceConfig { every_hours: number; watchers: number[]; focus: string }

export interface Maintenance {
  project: string;
  config: MaintenanceConfig;
  /** The agents who sweep, in turn (the owner's choice, else the project's agents). */
  watchers: number[];
  map: ProductMap;
  /** The newest 200, newest first. */
  signals: Signal[];
  signals_total: number;
  /** Signals no sweep has read yet. */
  unseen: number;
  /** Newest first. */
  proposals: Proposal[];
  sources: { inbox: number; ingest: boolean; connectors: { id: string; label: string; category: string }[]; mcp: { id: string; name: string }[] };
  sweep: Sweep;
}

export interface SignalInput { kind?: SignalKind; title?: string; text: string; from?: string; url?: string; environment?: string }

// ---- Environments ------------------------------------------------------------------------------------------------

export type EnvKind = 'production' | 'staging' | 'preview' | 'development' | 'other';
export const ENV_KINDS: EnvKind[] = ['production', 'staging', 'preview', 'development', 'other'];
/** What an environment runs: each landing on main, each signed release, or what is deployed by hand. */
export type EnvTrack = 'main' | 'releases' | 'manual';
export type DeployStatus = 'recorded' | 'sending' | 'sent' | 'failed' | 'waiting' | 'refused';
export interface Deploy { id: string; version: string; by: string; at_ms: number; status: DeployStatus; detail: string; approval?: string }
export interface Environment {
  name: string; kind: EnvKind; url?: string; track: EnvTrack; protected: boolean;
  variables: Record<string, string>; notes: string; version?: string; deployed_ms: number;
  /** Oldest first, the last 30. */
  deploys: Deploy[];
  created_ms: number; updated_ms: number;
  /** The names of its secrets in the vault (never their values), and whether it has a deploy hook. */
  secrets: string[]; hook: boolean;
}
export interface EnvironmentInput {
  kind: EnvKind; url?: string; track?: EnvTrack; protected?: boolean; variables?: Record<string, string>; notes?: string;
  /** A deploy hook URL (kept in the vault); null removes it; absent keeps it. */
  hook?: string | null;
}

// ---- Adopting ----------------------------------------------------------------------------------------------------

export type AdoptSource =
  | { platform: string; repo: string; sign?: boolean }
  | { git: string; branch?: string }
  | { path: string; branch?: string };
export interface AdoptInput {
  title: string; goal?: string; kind?: 'code' | 'other'; mode?: ProjectMode;
  /** True (the default): it opens delivered; false: still in implementation. */
  finished?: boolean;
  source?: AdoptSource;
  environments?: ({ name: string } & EnvironmentInput)[];
}
export interface Adopted {
  project: Project;
  /** Weft's import answer (commits, files, releases), or the platform connection's; null when nothing was imported. */
  import: Record<string, unknown> | null;
  environments: Environment[];
  sweep: { running: Sweep['running'] | null; watchers: number[] } | null;
  /** What did not work; the project is open anyway. */
  problems: string[];
}

// ---- Files -------------------------------------------------------------------------------------------------------

export interface Uploaded { hash: string; size: number }
export interface FilesAdded { repo: string; files: number; title: string; landed: { change: string; landed: string } }
/** A file the owner attached to a message (`PUT /v1/files`), as messages show it. */
export interface Attachment { id: string; name: string; size: number; type: string }

const enc = encodeURIComponent;

export const projectsApi = {
  modes: () => call<ModeDef[]>('GET', '/v1/project-modes'),
  setMode: (id: string, mode: ProjectMode) => call<Project>('POST', `/v1/projects/${enc(id)}/mode`, { mode }),
  adopt: (input: AdoptInput) => call<Adopted>('POST', '/v1/projects/adopt', input),

  maintenance: (id: string) => call<Maintenance>('GET', `/v1/projects/${enc(id)}/maintenance`),
  setMaintenance: (id: string, change: Partial<MaintenanceConfig>) =>
    call<Maintenance>('PATCH', `/v1/projects/${enc(id)}/maintenance`, change),
  sweep: (id: string) => call<{ work: number }>('POST', `/v1/projects/${enc(id)}/maintenance/sweep`, {}),
  addSignal: (id: string, signal: SignalInput) => call<Signal>('POST', `/v1/projects/${enc(id)}/maintenance/signals`, signal),
  deleteSignal: (id: string, sid: string) => call<{ deleted: boolean }>('DELETE', `/v1/projects/${enc(id)}/maintenance/signals/${enc(sid)}`),
  /** A new ingest address: the token is shown once (the old address stops). */
  newIngest: (id: string) => call<{ token: string; path: string }>('POST', `/v1/projects/${enc(id)}/maintenance/ingest`, {}),
  stopIngest: (id: string) => call<{ ingest: false }>('DELETE', `/v1/projects/${enc(id)}/maintenance/ingest`),
  approveProposal: (id: string, pid: string, body: { note?: string; role?: string } = {}) =>
    call<Proposal>('POST', `/v1/projects/${enc(id)}/maintenance/proposals/${enc(pid)}/approve`, body),
  declineProposal: (id: string, pid: string, reason = '') =>
    call<Proposal>('POST', `/v1/projects/${enc(id)}/maintenance/proposals/${enc(pid)}/decline`, { reason }),
  laterProposal: (id: string, pid: string, note = '') =>
    call<Proposal>('POST', `/v1/projects/${enc(id)}/maintenance/proposals/${enc(pid)}/later`, { note }),

  environments: (id: string) => call<Environment[]>('GET', `/v1/projects/${enc(id)}/environments`),
  putEnvironment: (id: string, env: string, input: EnvironmentInput) =>
    call<Environment>('PUT', `/v1/projects/${enc(id)}/environments/${enc(env)}`, input),
  deleteEnvironment: (id: string, env: string) => call<{ deleted: boolean }>('DELETE', `/v1/projects/${enc(id)}/environments/${enc(env)}`),
  deploy: (id: string, env: string, version?: string) =>
    call<Deploy>('POST', `/v1/projects/${enc(id)}/environments/${enc(env)}/deploy`, version ? { version } : {}),
  putEnvSecret: (id: string, env: string, name: string, value: string) =>
    call<{ name: string; binding: string }>('PUT', `/v1/projects/${enc(id)}/environments/${enc(env)}/secrets/${enc(name)}`, { value }),
  deleteEnvSecret: (id: string, env: string, name: string) =>
    call<{ deleted: boolean }>('DELETE', `/v1/projects/${enc(id)}/environments/${enc(env)}/secrets/${enc(name)}`),

  /** One file's bytes into the project's store (no change yet): its hash for `addFiles`. */
  upload: async (id: string, bytes: Blob): Promise<Uploaded> =>
    (await raw('PUT', `/v1/projects/${enc(id)}/uploads`, bytes, 'application/octet-stream')).json() as Promise<Uploaded>,
  /** Uploaded files land on main as one change of the owner's. `folder` prefixes every path. */
  addFiles: (id: string, files: { path: string; hash: string }[], opts: { title?: string; folder?: string } = {}) =>
    call<FilesAdded>('POST', `/v1/projects/${enc(id)}/files`, { files, ...opts }),
  /** A file to attach to a chat or conversation message: private until a message carries it. */
  attach: async (file: Blob, name: string): Promise<Attachment> => {
    const res = await raw('PUT', `/v1/files?name=${enc(name)}`, file, file.type || 'application/octet-stream');
    const d = (await res.json()) as { id: string; title: string; size: number; content_type: string };
    return { id: d.id, name: d.title, size: d.size, type: d.content_type };
  },
};
