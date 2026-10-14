// Shared by the MCP screen (mcp.ts, mcp-logs.ts, mcp-wizard.ts, mcp-wizard-secrets.ts). Pure helpers.
import type { McpPick, McpPreset, McpRisk, McpServerConfig, McpServerView, McpSetting, McpSpec, McpStatus, McpTransport } from '../api';

/** A server id: lower-case, starts with a letter, 2 to 32 characters. */
export const MCP_ID_RE = /^[a-z][a-z0-9-]{1,31}$/;

/** An env var name the hub takes: upper-case, as a shell variable is written. */
export const ENV_NAME_RE = /^[A-Z_][A-Z0-9_]{0,63}$/;

/** A header name the hub takes: letters, digits and hyphens. */
export const HEADER_NAME_RE = /^[A-Za-z0-9-]{1,64}$/;

/** The bounds of a server's limits, as the hub checks them (crates/apid/src/mcp/config.rs). */
export const LIMIT_BOUNDS: Record<keyof McpServerConfig['limits'], { label: string; lo: number; hi: number }> = {
  timeout_s: { label: 'Timeout', lo: 1, hi: 600 },
  max_output_kb: { label: 'Output size', lo: 1, hi: 4096 },
  idle_stop_min: { label: 'Idle stop', lo: 1, hi: 1440 },
};

/** The first problem with a server's limits, or '' when each is a whole number inside its bounds. */
export function limitsProblem(limits: McpServerConfig['limits']): string {
  for (const key of Object.keys(LIMIT_BOUNDS) as (keyof McpServerConfig['limits'])[]) {
    const { label, lo, hi } = LIMIT_BOUNDS[key];
    const v = limits[key];
    if (!Number.isInteger(v) || v < lo || v > hi) return `${label}: enter a whole number from ${lo} to ${hi}.`;
  }
  return '';
}

/** The first problem with a name typed for a row of the server's own settings (a custom or remote server), or ''. */
export function ownRowProblem(name: string, header: boolean): string {
  const n = name.trim();
  if (header) return HEADER_NAME_RE.test(n) ? '' : 'A header name uses letters, digits and hyphens.';
  return ENV_NAME_RE.test(n) ? '' : 'An env var name uses capitals, digits and underscores, and starts with a letter or underscore.';
}

/** The preset a server's transport belongs to (its image or its URL), if any. */
export function matchingPreset(t: McpTransport, catalog: McpPreset[]): McpPreset | undefined {
  return catalog.find((p) => (t.kind === 'stdio' ? p.transport === 'stdio' && p.image === t.image : p.transport === 'http' && p.url === t.url));
}

/** The specs of a server's own settings, for a server with no preset: one per name, secret when it is a vault reference. */
export function specsOfBag(bag: Record<string, McpSetting>): McpSpec[] {
  return Object.entries(bag).map(([name, s]) => ({
    name,
    secret: 'secret' in s,
    required: false,
    help: '',
    format: 'secret' in s && s.format ? s.format : null,
  }));
}

/** The vault binding a server's secrets carry. */
export const mcpBinding = (id: string): string => `mcp:${id}`;

export const STATUS_TONE: Record<McpStatus, 'muted' | 'info' | 'ok' | 'danger'> = {
  stopped: 'muted',
  starting: 'info',
  running: 'ok',
  error: 'danger',
};

export const STATUS_LABEL: Record<McpStatus, string> = {
  stopped: 'Stopped',
  starting: 'Starting',
  running: 'Running',
  error: 'Error',
};

export const RISKS: McpRisk[] = ['read', 'write', 'irreversible'];

/** `container image` or `https://host/path` for the list, the wizard and the summary. */
export function transportText(t: McpTransport): string {
  return t.kind === 'stdio' ? `container · ${t.image}` : `remote · ${t.url}`;
}

/** A pick as the owner reads it: "all" or the names joined. An empty list reads as "nobody". */
export function pickText(p: McpPick, empty = 'nobody'): string {
  if (p === 'all') return 'all';
  return p.length ? p.join(', ') : empty;
}

/** A new, empty server config for the wizard: disabled, nobody can use it until the owner says so. */
export function blankConfig(id: string, name: string, transport: McpTransport): McpServerConfig {
  return {
    id,
    name,
    enabled: false,
    transport,
    env: {},
    headers: {},
    tools: { allow: 'all', risk: {} },
    access: { roles: [], nodes: [] },
    limits: { timeout_s: 60, max_output_kb: 256, idle_stop_min: 10 },
    allow_private: false,
    storage: { volume: false },
    created_ms: 0,
    updated_ms: 0,
  };
}

/** The status line for a server row: the state, plus the last error's first line when it is in error. */
/** The first line of the server's last error, whenever it is not running (a failed test or start leaves it). */
export function statusSummary(s: McpServerView): string {
  if (s.status !== 'running' && s.last_error) return s.last_error.split('\n')[0] ?? '';
  return '';
}
