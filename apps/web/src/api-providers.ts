// The providers & models routes on the hub: API provider instances (`/v1/providers`), coding-engine
// instances (`/v1/engines`), the ACP registry, usage providers (CLIProxyAPI hubs) and the health checks.
// Every call goes through the shared request helper in api.ts. This module only fetches; it draws nothing.
//
// Guest coding engines reach models through the plaza gateway. Local Codex and OpenCode providers run on the hub
// as model transports, returning tool calls to the same AgentPlaza harness; their credentials stay on the hub.

import { call, readable } from './api';
import type { Auth, ModelGroup } from './api';

/** A check's result: `off` for a disabled instance, `unknown` before its first check. */
export type HealthStatus = 'ok' | 'warn' | 'crit' | 'off' | 'unknown';

export interface Health {
  status: HealthStatus;
  /** One sentence for the list and the detail: "Ready · 14 models", "Not authenticated · add a key". */
  message: string;
  checked_ms?: number;
  latency_ms?: number;
  /** API instances: how many models the listing offered. */
  models?: number;
  /** Engines: the `<provider>/<model>` the engine runs through. */
  route?: string;
}

/** An API provider instance (`GET /v1/providers`). Secret values never appear. */
export interface ApiInstance {
  id: string;
  family: 'api';
  kind: string;
  group: ModelGroup;
  label: string;
  accent: string | null;
  enabled: boolean;
  health: Health;
  base_url: string | null;
  auth: Auth;
  models: string[];
  options: Record<string, unknown> | null;
  /** Header names; their values are redacted. */
  extra_headers: Record<string, string>;
  extra_body: unknown;
  credential: 'env' | 'secret' | 'none' | 'missing' | 'command' | 'sealed' | 'out_of_scope';
}

/** The fields `PUT /v1/providers/{id}` takes (the whole config; redacted values are restored by the hub). */
export interface ApiInstanceInput {
  id: string;
  kind: string;
  label?: string | null;
  accent?: string | null;
  enabled?: boolean;
  base_url?: string | null;
  auth?: Auth;
  models?: string[];
  options?: Record<string, unknown> | null;
  extra_headers?: Record<string, string>;
  extra_body?: unknown;
}

export type EngineInstall =
  | { kind: 'npm'; package: string; version: string; integrity: string }
  | { kind: 'pip'; package: string; version: string; file: string; sha256: string }
  | { kind: 'archive'; version: string; assets: Record<string, { url: string; sha256: string; cmd: string; args?: string[] }> }
  | { kind: 'command'; command: string };

/** A coding-engine instance (`GET /v1/engines`): a built-in engine, or one the owner added. */
export interface EngineInstance {
  id: string;
  family: 'engine';
  /** A built-in engine id, `acp:<registry id>` or `acp-local`. */
  engine: string;
  builtin: boolean;
  label: string;
  accent: string | null;
  enabled: boolean;
  /** `npm`, `release`, `pip`, `command` or `catalogue`. */
  source: string;
  package: string | null;
  /** The harness catalogue's pin (built-in engines). */
  pinned: string | null;
  /** The version the VM installs. */
  version: string | null;
  /** The newest version, from the last check (npm engines). */
  latest: string | null;
  install: EngineInstall | null;
  args: string[];
  /** Plain variables only: the agent can read them, so the hub refuses secrets here. */
  env: Record<string, string>;
  home: string;
  route: string | null;
  about: { name?: string; description?: string; icon?: string; website?: string; repository?: string; version?: string } | null;
  health: Health;
}

/** The fields `POST /v1/engines` and `PUT /v1/engines/{id}` take; absent fields keep their value. */
export interface EngineEdit {
  id?: string;
  engine?: string;
  label?: string | null;
  accent?: string | null;
  enabled?: boolean;
  /** An npm engine's version, `latest`, or `pinned` (back to the catalogue's). */
  version?: string;
  /** `acp-local` only: one program; its arguments go in `args`. */
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  home?: string | null;
  route?: string | null;
}

/** One agent of the ACP registry, as the Add dialog lists it. */
export interface RegistryAgent {
  id: string;
  name: string;
  version: string;
  description: string;
  website: string | null;
  repository: string | null;
  icon: string | null;
  license: string | null;
  distribution: 'npx' | 'uvx' | 'binary' | null;
  /** False when it ships no Linux build (agents run in Linux VMs). */
  runs_in_vm: boolean;
}

export interface Registry {
  fetched_ms: number;
  agents: RegistryAgent[];
}

/** What the Add provider dialog offers. */
export interface ProviderCatalog {
  api: { id: string; kind: string; label: string; brand: string; base_url: string | null; auth: Auth; notes: string; models: string[] }[];
  engines: { id: string; label: string; source: string; package: string; pinned: string }[];
  registry: string;
}

/** A usage provider (a CLIProxyAPI hub). Its management key stays in the hub's vault. */
export interface UsageProvider {
  id: string;
  kind: 'cliproxyapi';
  url: string;
  label: string;
  /** Whether the vault holds its key: `secret`, `missing`, `sealed` … */
  key: string;
  last: UsageReading | null;
}

export interface UsageReading {
  status: 'ok' | 'crit';
  message: string;
  checked_ms: number;
  totals?: { requests: number; success: number; failure: number; tokens: number };
  models?: { model: string; requests: number; tokens: number }[];
  accounts?: { name: string; provider: string; status: string; message: string }[];
}

export interface EngineUpdate {
  dry_run: boolean;
  changes: { id: string; from: string; to: string }[];
}

const enc = encodeURIComponent;

export const providersApi = {
  // API instances
  list: readable<ApiInstance[]>('/v1/providers'),
  get: (id: string) => call<ApiInstance>('GET', `/v1/providers/${enc(id)}`),
  create: (cfg: ApiInstanceInput) => call<ApiInstance>('POST', '/v1/providers', cfg),
  replace: (cfg: ApiInstanceInput) => call<ApiInstance>('PUT', `/v1/providers/${enc(cfg.id)}`, cfg),
  remove: (id: string) => call<{ deleted: string }>('DELETE', `/v1/providers/${enc(id)}`),
  setKey: (id: string, value: string) => call<ApiInstance>('PUT', `/v1/providers/${enc(id)}/secret`, { value }),
  check: (id: string) => call<{ id: string; health: Health }>('POST', `/v1/providers/${enc(id)}/check`),
  checkAll: () => call<Record<string, Health>>('POST', '/v1/providers/check'),
  catalog: () => call<ProviderCatalog>('GET', '/v1/provider-catalog'),

  // Engines
  engines: readable<EngineInstance[]>('/v1/engines'),
  engine: (id: string) => call<EngineInstance>('GET', `/v1/engines/${enc(id)}`),
  addEngine: (edit: EngineEdit) => call<EngineInstance>('POST', '/v1/engines', edit),
  editEngine: (id: string, edit: EngineEdit) => call<EngineInstance>('PUT', `/v1/engines/${enc(id)}`, edit),
  removeEngine: (id: string) => call<{ deleted: string }>('DELETE', `/v1/engines/${enc(id)}`),
  checkEngines: () => call<Record<string, Health>>('POST', '/v1/engines/check'),
  updateEngines: (dryRun: boolean) => call<EngineUpdate>('POST', '/v1/engines/update', { dry_run: dryRun }),
  registry: (refresh = false) => call<Registry>('GET', `/v1/acp/registry${refresh ? '?refresh=1' : ''}`),

  // Usage providers
  usage: readable<UsageProvider[]>('/v1/usage-providers'),
  addUsage: (body: { url: string; key: string; label?: string }) =>
    call<UsageProvider>('POST', '/v1/usage-providers', { kind: 'cliproxyapi', ...body }),
  readUsage: (id: string) => call<UsageReading>('GET', `/v1/usage-providers/${enc(id)}/usage`),
  removeUsage: (id: string) => call<{ deleted: string }>('DELETE', `/v1/usage-providers/${enc(id)}`),
};

/** The newest `checked_ms` across instances, or null before any check: the "Checked just now" line. */
export function lastChecked(items: { health: Health }[]): number | null {
  const times = items.map((i) => i.health.checked_ms ?? 0).filter((t) => t > 0);
  return times.length ? Math.max(...times) : null;
}

/** The tone a health status draws with (the kit's chip and dot tones). */
export function healthTone(status: HealthStatus): 'ok' | 'warn' | 'crit' | 'mute' {
  return status === 'ok' ? 'ok' : status === 'warn' ? 'warn' : status === 'crit' ? 'crit' : 'mute';
}
