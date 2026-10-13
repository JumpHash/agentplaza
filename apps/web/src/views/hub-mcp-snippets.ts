// The hub-as-a-server tab's pure helpers (mcp-hub.ts): the scopes with their plain-words lines, the tool checklist
// grouped by scope, and the setup text an owner copies after a token is made. No DOM here, so the builders are
// unit-tested (test/tests.ts). The lists follow docs/design/hub-mcp.md.

export type HubScope = 'observe' | 'operate' | 'govern';

/** The scopes in order of power. A token may hold tools of its own scope and of the scopes below it. */
const SCOPE_RANK: Record<HubScope, number> = { observe: 0, operate: 1, govern: 2 };

export const HUB_SCOPES: { value: HubScope; label: string; plain: string }[] = [
  { value: 'observe', label: 'Observe', plain: 'read only' },
  {
    value: 'operate',
    label: 'Operate',
    plain: "acts within the plaza's rules: tasks, messages, stopping or restarting an agent, proposing agents",
  },
  { value: 'govern', label: 'Govern', plain: 'files approvals and law proposals for you to decide' },
];

/** The tools by their own scope, as docs/design/hub-mcp.md lists them. */
export const HUB_TOOLS: Record<HubScope, string[]> = {
  observe: [
    'plaza_status',
    'list_agents',
    'get_agent',
    'get_transcript',
    'list_tasks',
    'get_task',
    'list_board',
    'list_projects',
    'get_project',
    'list_approvals',
    'list_machines',
    'treasury_summary',
    'usage_summary',
    'search_library',
    'list_laws',
  ],
  operate: [
    'create_task',
    'message_orchestrator',
    'tell_agent',
    'post_board',
    'stop_agent',
    'restart_agent',
    'propose_agent',
  ],
  govern: ['request_decision', 'propose_law_change'],
};

/** Whether a token of `scope` may hold a tool of scope `tool` (apid's `Scope::permits`). */
export function scopePermits(scope: HubScope, tool: HubScope): boolean {
  return SCOPE_RANK[tool] <= SCOPE_RANK[scope];
}

/** The hub's origin: the scheme and host of its MCP URL, with no path. */
export function hubOrigin(url: string): string {
  try {
    return new URL(url).origin;
  } catch {
    return url.replace(/\/mcp\/?$/, '');
  }
}

export interface SetupSnippets {
  /** The Claude Code command. The bearer header is dropped in open mode. */
  claudeCode: string;
  /** The Claude Desktop JSON block: the stdio bridge against the hub's origin. It never carries a token. */
  desktop: string;
  /** The line that saves the token (it reads it from stdin). Null in open mode, where there is no token. */
  desktopLogin: string | null;
  /** The URL and the header, for any other client. In open mode, the URL alone. */
  generic: string;
}

/**
 * The setup text for each client, built from the MCP URL and the token. `token` null means open mode: no token,
 * so no Authorization header and no login line.
 */
export function setupSnippets(url: string, token: string | null): SetupSnippets {
  const origin = hubOrigin(url);
  const header = token === null ? '' : ` --header "Authorization: Bearer ${token}"`;
  return {
    claudeCode: `claude mcp add --transport http agentplaza ${url}${header}`,
    desktop: JSON.stringify(
      { mcpServers: { agentplaza: { command: 'agentplaza', args: ['mcp', 'serve', '--hub', origin] } } },
      null,
      2,
    ),
    desktopLogin: token === null ? null : `agentplaza mcp login --hub ${origin}`,
    generic: token === null ? `URL: ${url}` : `URL: ${url}\nAuthorization: Bearer ${token}`,
  };
}
