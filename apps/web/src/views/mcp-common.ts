// Shared by the MCP screen (mcp.ts, mcp-logs.ts, mcp-wizard.ts, mcp-wizard-secrets.ts). Pure helpers.
import type { McpPick, McpRisk, McpServerConfig, McpServerView, McpStatus, McpTransport } from '../api';

/** A server id: lower-case, starts with a letter, 2 to 32 characters. */
export const MCP_ID_RE = /^[a-z][a-z0-9-]{1,31}$/;

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
