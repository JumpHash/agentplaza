// Setup > MCP > This hub as a server: the owner turns the hub's own MCP server on, mints tokens for outside clients,
// and reads the tokens and the call log (docs/design/hub-mcp.md, "Configuration flow"). A new token is shown once, in
// the reveal box. It is never written to localStorage, and leaving the tab clears the page, reveal box included.

import '../styles/mcp-hub.css';
import {
  hubMcpApi,
  type HubAuthMode,
  type HubBind,
  type HubCall,
  type HubMcpSettings,
  type HubMcpStatus,
  type HubToken,
} from '../api';
import { ago, attempt, badge, button, card, clip, empty, field, input, modal, select } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { HUB_SCOPES, HUB_TOOLS, scopePermits, setupSnippets, type HubScope } from './hub-mcp-snippets';
import { errorText } from './vault-common';

const BIND_OPTIONS: { value: HubBind; label: string }[] = [
  { value: 'local', label: 'Local only' },
  { value: 'remote', label: 'Remote over TLS' },
];

/** Secure needs a token; open lets anonymous callers in with the open scope. */
const AUTH_OPTIONS: { value: HubAuthMode; label: string; plain: string }[] = [
  { value: 'secure', label: 'Secure', plain: 'token required' },
  { value: 'open', label: 'Open', plain: 'no token' },
];

/** Each tool's own scope, for the checklist and for the scope change. */
const TOOL_SCOPE = new Map<string, HubScope>(
  (Object.keys(HUB_TOOLS) as HubScope[]).flatMap((scope) => HUB_TOOLS[scope].map((name) => [name, scope] as const)),
);
const ALL_TOOLS = Array.from(TOOL_SCOPE.keys());

/** A small confirm dialog, as the servers tab has: a message, Cancel, and the action in the danger tone. */
function confirmDialog(title: string, text: string, label: string, onConfirm: () => void): void {
  const close = modal(
    title,
    h(
      'div.mcp-confirm',
      null,
      h('p', null, text),
      h(
        'div.mcp-actions',
        null,
        button('Cancel', () => close()),
        button(
          label,
          () => {
            close();
            onConfirm();
          },
          { kind: 'danger' },
        ),
      ),
    ),
  );
}

/** A copy button for one piece of text. The text is never stored; the click copies it and says so. */
function copyButton(text: string, label = 'Copy'): HTMLButtonElement {
  return button(label, () => void attempt(() => navigator.clipboard.writeText(text), 'Copied'), {
    kind: 'ghost',
    size: 'sm',
  });
}

/** One block of setup text, with its copy button and a note on what it does. */
function snippet(title: string, text: string, note: string): HTMLElement {
  return h(
    'div.hub-snippet',
    null,
    h('div.hub-snippet-head', null, h('strong', null, title), copyButton(text)),
    h('pre.mcp-mono', null, text),
    h('p.mcp-muted', null, note),
  );
}

/** The tab. It fills `root` and returns a cleanup that empties it, so no token outlives the tab. */
export function mountHubTab(root: HTMLElement): Cleanup {
  let alive = true;
  let status: HubMcpStatus | null = null;
  let statusError: unknown = null;
  let tokens: HubToken[] = [];
  let tokensError: unknown = null;
  let calls: HubCall[] = [];
  let callsError: unknown = null;
  let callsSeq = 0;
  // The settings draft: what the switch, the bind and the origins show until the owner saves.
  let enabled = false;
  let bind: HubBind = 'local';
  let origins: string[] = [];
  // The new-token form.
  let scope: HubScope = 'observe';
  const picked = new Set<string>();
  // The call filters.
  let filterToken = '';
  let filterTool = '';
  let authMode: HubAuthMode = 'secure';
  let openScope: HubScope = 'observe';
  let warning: string | null = null;

  const statusSlot = h('div.hub-status-slot', null, h('p.mcp-loading', null, 'Loading...'));
  const originsSlot = h('ul.hub-origins');
  const toolsSlot = h('div.hub-tools');
  const revealSlot = h('div.hub-reveal-slot');
  const tokensSlot = h('div', null, h('p.mcp-loading', null, 'Loading...'));
  const tokenFilterSlot = h('div.hub-filter');
  const callsSlot = h('div', null, h('p.mcp-loading', null, 'Loading...'));

  // ---- Enable -----------------------------------------------------------------------------------------------------

  const enabledBox = input({ type: 'checkbox' });
  enabledBox.addEventListener('change', () => {
    enabled = enabledBox.checked;
  });

  const bindRows = BIND_OPTIONS.map((o) => {
    const el = input({ type: 'radio', name: 'hub-bind', value: o.value });
    el.addEventListener('change', () => {
      if (el.checked) {
        bind = o.value;
        drawAuth();
      }
    });
    return { value: o.value, el, row: h('label.hub-radio', null, el, h('span', null, o.label)) };
  });

  const syncControls = (): void => {
    enabledBox.checked = enabled;
    for (const b of bindRows) b.el.checked = b.value === bind;
  };

  const drawOrigins = (): void => {
    replace(
      originsSlot,
      origins.length
        ? origins.map((o) =>
            h(
              'li',
              null,
              h('span.mcp-mono', null, o),
              button(
                '',
                () => {
                  origins = origins.filter((x) => x !== o);
                  drawOrigins();
                },
                { kind: 'ghost', size: 'sm', icon: 'trash', title: `Remove ${o}` },
              ),
            ),
          )
        : h('li.mcp-muted', null, 'No origins yet.'),
    );
  };

  const originInput = input({ placeholder: 'https://app.example.com', 'aria-label': 'Allowed origin' });
  const addOrigin = (): void => {
    const value = originInput.value.trim();
    if (!value) return;
    if (!origins.includes(value)) origins = [...origins, value];
    originInput.value = '';
    drawOrigins();
  };
  originInput.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter') {
      ev.preventDefault();
      addOrigin();
    }
  });

  const drawStatus = (): void => {
    if (statusError) {
      replace(
        statusSlot,
        empty('alert', 'Could not load the hub server', errorText(statusError), button('Try again', () => void loadStatus(), { kind: 'ghost' })),
      );
      return;
    }
    if (!status) {
      replace(statusSlot, h('p.mcp-loading', null, 'Loading...'));
      return;
    }
    const url = status.url;
    replace(
      statusSlot,
      h(
        'div.hub-status',
        null,
        badge(
          status.enabled ? `On, ${status.bind === 'remote' ? 'remote over TLS' : 'local only'}` : 'Off',
          status.enabled ? 'ok' : 'muted',
          true,
        ),
        h('span.mcp-mono.hub-url', null, url),
        copyButton(url, 'Copy URL'),
        h('span.mcp-muted', null, `${status.tokens} live token${status.tokens === 1 ? '' : 's'}`),
      ),
      status.stored ? null : h('p.mcp-muted', null, 'The database is off, so these settings last only until the hub restarts.'),
    );
  };

  const applyStatus = (next: HubMcpStatus): void => {
    status = next;
    statusError = null;
    enabled = next.enabled;
    bind = next.bind;
    origins = [...next.allowed_origins];
    authMode = next.auth_mode;
    openScope = next.open_scope;
    warning = next.warning ?? null;
    syncControls();
    drawOrigins();
    drawStatus();
    drawAuth();
  };

  const loadStatus = async (): Promise<void> => {
    try {
      const next = await hubMcpApi.status();
      if (alive) applyStatus(next);
    } catch (err) {
      if (!alive) return;
      statusError = err;
      drawStatus();
    }
  };

  // ---- Auth: secure (a token) or open (no token; anonymous callers get the open scope) ----------------------------

  const authRows = AUTH_OPTIONS.map((o) => {
    const el = input({ type: 'radio', name: 'hub-auth', value: o.value });
    el.addEventListener('change', () => {
      if (el.checked) {
        authMode = o.value;
        drawAuth();
      }
    });
    return {
      value: o.value,
      el,
      row: h(
        'label.hub-radio',
        null,
        el,
        h('span', null, h('strong', null, o.label), ' ', h('span.mcp-muted', null, o.plain)),
      ),
    };
  });

  const openScopeSelect = select(
    HUB_SCOPES.map((s) => ({ value: s.value, label: `${s.label}: ${s.plain}` })),
    openScope,
  );
  openScopeSelect.addEventListener('change', () => {
    openScope = openScopeSelect.value as HubScope;
  });
  const openScopeRow = h(
    'div.hub-group',
    null,
    h('span.hub-label', null, 'Open scope (anonymous callers)'),
    openScopeSelect,
  );

  const confirmBox = input({ type: 'checkbox' });
  const confirmRow = h(
    'label.hub-switch',
    null,
    confirmBox,
    h('span', null, 'I understand anyone on the network can use it'),
  );
  const warningBox = h('div.hub-warning');
  const authBox = h(
    'div.hub-group',
    null,
    h('span.hub-label', null, 'Auth'),
    h('div.hub-auth', null, authRows.map((a) => a.row)),
    openScopeRow,
    warningBox,
    confirmRow,
  );

  const drawAuth = (): void => {
    const open = authMode === 'open';
    for (const a of authRows) a.el.checked = a.value === authMode;
    openScopeSelect.value = openScope;
    openScopeRow.hidden = !open;
    warningBox.hidden = !open;
    confirmRow.hidden = !(open && bind === 'remote');
    replace(
      warningBox,
      h('strong', null, 'Anyone who can reach this address can use these tools without a token.'),
      warning ? h('p', null, warning) : null,
    );
  };

  const save = (): void => {
    const body: HubMcpSettings = { enabled, bind, allowed_origins: origins, auth_mode: authMode, open_scope: openScope };
    if (authMode === 'open' && bind === 'remote') body.confirm_open_remote = confirmBox.checked;
    void attempt(() => hubMcpApi.save(body), 'Saved').then((next) => {
      if (alive && next) applyStatus(next);
    });
  };

  const enableCard = card(
    'Enable',
    [
      h('label.hub-switch', null, enabledBox, h('span', null, 'Serve this hub over MCP')),
      h('fieldset.hub-group', null, h('legend', null, 'Bind'), bindRows.map((b) => b.row)),
      authBox,
      h(
        'div.hub-group',
        null,
        h('span.hub-label', null, 'Allowed origins'),
        originsSlot,
        h('div.hub-row', null, originInput, button('Add', addOrigin, { kind: 'ghost', size: 'sm', icon: 'plus' })),
      ),
      statusSlot,
    ],
    { actions: [button('Save', save, { kind: 'primary' })] },
  );

  // ---- New token --------------------------------------------------------------------------------------------------

  const nameInput = input({ maxlength: 80, placeholder: 'laptop' });
  const expiresInput = input({ type: 'number', min: 1, max: 3650, placeholder: 'Never' });

  const drawTools = (): void => {
    replace(
      toolsSlot,
      HUB_SCOPES.map((group) => {
        const allowed = scopePermits(scope, group.value);
        return h(
          'fieldset.hub-tool-group',
          null,
          h('legend', null, allowed ? group.label : `${group.label} (above this scope)`),
          HUB_TOOLS[group.value].map((name) => {
            const box = input({ type: 'checkbox', disabled: !allowed });
            box.checked = picked.has(name);
            box.addEventListener('change', () => {
              if (box.checked) picked.add(name);
              else picked.delete(name);
            });
            return h('label.hub-tool', null, box, h('code', null, name));
          }),
        );
      }),
    );
  };

  const setScope = (next: HubScope): void => {
    scope = next;
    // A tool above the new scope cannot stay ticked: the server would refuse the token.
    for (const id of Array.from(picked)) {
      const toolScope = TOOL_SCOPE.get(id);
      if (!toolScope || !scopePermits(next, toolScope)) picked.delete(id);
    }
    drawTools();
  };

  const scopeRows = HUB_SCOPES.map((s) => {
    const el = input({ type: 'radio', name: 'hub-token-scope', value: s.value });
    el.checked = s.value === scope;
    el.addEventListener('change', () => {
      if (el.checked) setScope(s.value);
    });
    return h('label.hub-scope', null, el, h('span', null, h('strong', null, s.label), ' ', h('span.mcp-muted', null, s.plain)));
  });

  /** Shows a new token once, with the setup text for each client. The token lives only in this reveal box. */
  const showReveal = (token: string, name: string): void => {
    const s = setupSnippets(status ? status.url : '', authMode === 'open' ? null : token);
    replace(
      revealSlot,
      h(
        'div.hub-reveal',
        null,
        h('h3', null, `Token for ${name}`),
        h('p.hub-warn', null, 'Shown once. Copy it now, because it cannot be shown again.'),
        h('div.hub-token-row', null, h('code.mcp-mono.hub-token', null, token), copyButton(token, 'Copy token')),
        snippet('Claude Code', s.claudeCode, 'Run this in a terminal.'),
        snippet(
          'Claude Desktop',
          s.desktop,
          'Add this to the MCP servers of Claude Desktop or another stdio client. It runs the CLI bridge, which uses the token saved by the login line below.',
        ),
        s.desktopLogin
          ? snippet('Save the token', s.desktopLogin, 'Reads the token from stdin and saves it in cli.json. Paste the token, then end the input.')
          : null,
        snippet('Generic', s.generic, 'The URL and the header, for any other client.'),
        button('Hide', () => replace(revealSlot), { kind: 'ghost' }),
      ),
    );
  };

  const createToken = async (): Promise<void> => {
    if (!status) await loadStatus();
    if (!status || !alive) return;
    const name = nameInput.value.trim();
    const days = expiresInput.value.trim();
    const made = await attempt(
      () =>
        hubMcpApi.createToken({
          name,
          scope,
          ...(picked.size ? { tools: Array.from(picked) } : {}),
          ...(days ? { expires_days: Number(days) } : {}),
        }),
      'Token created',
    );
    if (!made || !alive) return;
    nameInput.value = '';
    expiresInput.value = '';
    picked.clear();
    drawTools();
    showReveal(made.token, made.record.name);
    void loadTokens();
  };

  const formCard = card(
    'New token',
    [
      field('Name', nameInput, 'What this client is, such as laptop or ci.'),
      h('fieldset.hub-group', null, h('legend', null, 'Scope'), scopeRows),
      h(
        'fieldset.hub-group',
        null,
        h('legend', null, 'Tools (optional)'),
        h('p.mcp-muted', null, 'Leave them all unticked for every tool the scope allows.'),
        toolsSlot,
      ),
      field('Expires after (days)', expiresInput, 'Blank means the token never expires.'),
      revealSlot,
    ],
    { actions: [button('Create token', () => void createToken(), { kind: 'primary', icon: 'plus' })] },
  );

  // ---- Tokens -----------------------------------------------------------------------------------------------------

  const tokenName = (id: string): string => tokens.find((t) => t.id === id)?.name ?? id;

  const tokenRow = (t: HubToken): HTMLElement =>
    h(
      'tr',
      null,
      h('td', { 'data-label': 'Name' }, h('strong.mcp-name', null, t.name), h('span.mcp-id', null, t.id)),
      h('td', { 'data-label': 'Scope' }, badge(t.scope, 'info')),
      h('td', { 'data-label': 'Last used' }, t.last_used_ms ? ago(t.last_used_ms) : 'never'),
      h('td', { 'data-label': 'Calls' }, String(t.calls)),
      h('td', { 'data-label': 'Expires' }, t.expires_ms ? new Date(t.expires_ms).toLocaleDateString() : 'never'),
      h(
        'td',
        { 'data-label': 'Actions', class: 'mcp-actions-cell' },
        button(
          'Revoke',
          () =>
            confirmDialog(
              `Revoke ${t.name}`,
              'Clients that use this token lose access at once. This cannot be undone.',
              'Revoke',
              () => void attempt(() => hubMcpApi.revokeToken(t.id), `${t.name} revoked`).then(() => loadTokens()),
            ),
          { kind: 'danger', size: 'sm', icon: 'trash', title: `Revoke ${t.name}` },
        ),
      ),
    );

  const drawTokens = (): void => {
    if (tokensError) {
      replace(tokensSlot, empty('alert', 'Could not load the tokens', errorText(tokensError)));
      return;
    }
    const live = tokens.filter((t) => t.revoked_at_ms == null);
    replace(
      tokensSlot,
      live.length
        ? h(
            'div.mcp-table-wrap',
            null,
            h(
              'table.mcp-table',
              null,
              h(
                'thead',
                null,
                h(
                  'tr',
                  null,
                  h('th', null, 'Name'),
                  h('th', null, 'Scope'),
                  h('th', null, 'Last used'),
                  h('th', null, 'Calls'),
                  h('th', null, 'Expires'),
                  h('th', null, 'Actions'),
                ),
              ),
              h('tbody', null, live.map(tokenRow)),
            ),
          )
        : empty('terminal', 'No tokens', 'Make one above to give an outside client access to this hub.'),
    );
  };

  const drawTokenFilter = (): void => {
    const tokenSelect = select(
      [{ value: '', label: 'All tokens' }, ...tokens.map((t) => ({ value: t.id, label: t.name }))],
      filterToken,
    );
    tokenSelect.addEventListener('change', () => {
      filterToken = tokenSelect.value;
      void loadCalls();
    });
    replace(tokenFilterSlot, field('Token', tokenSelect));
  };

  const loadTokens = async (): Promise<void> => {
    try {
      const r = await hubMcpApi.tokens();
      if (!alive) return;
      tokens = r.tokens;
      tokensError = null;
    } catch (err) {
      if (!alive) return;
      tokensError = err;
    }
    drawTokens();
    drawTokenFilter();
  };

  const tokensCard = card('Tokens', tokensSlot);

  // ---- Calls ------------------------------------------------------------------------------------------------------

  const callRow = (c: HubCall): HTMLElement =>
    h(
      'tr',
      null,
      h('td', { 'data-label': 'Time' }, new Date(c.at_ms).toLocaleString()),
      h('td', { 'data-label': 'Token' }, tokenName(c.token_id)),
      h('td', { 'data-label': 'Tool' }, h('code', null, c.tool)),
      h(
        'td',
        { 'data-label': 'Result' },
        c.ok
          ? badge('ok', 'ok')
          : h('span', null, badge('error', 'danger'), ' ', h('span.mcp-danger', null, clip(c.error ?? '', 120))),
      ),
      h('td', { 'data-label': 'Duration' }, `${c.duration_ms} ms`),
    );

  const drawCalls = (): void => {
    if (callsError) {
      replace(callsSlot, empty('alert', 'Could not load the calls', errorText(callsError)));
      return;
    }
    replace(
      callsSlot,
      calls.length
        ? h(
            'div.mcp-table-wrap',
            null,
            h(
              'table.mcp-table',
              null,
              h(
                'thead',
                null,
                h(
                  'tr',
                  null,
                  h('th', null, 'Time'),
                  h('th', null, 'Token'),
                  h('th', null, 'Tool'),
                  h('th', null, 'Result'),
                  h('th', null, 'Duration'),
                ),
              ),
              h('tbody', null, calls.map(callRow)),
            ),
          )
        : empty('terminal', 'No calls', 'Calls appear here once a client uses a token.'),
    );
  };

  const loadCalls = async (): Promise<void> => {
    const seq = ++callsSeq;
    try {
      const r = await hubMcpApi.calls({
        token: filterToken || undefined,
        tool: filterTool || undefined,
        limit: 100,
      });
      if (!alive || seq !== callsSeq) return;
      calls = r.calls;
      callsError = null;
    } catch (err) {
      if (!alive || seq !== callsSeq) return;
      callsError = err;
    }
    drawCalls();
  };

  const toolSelect = select([
    { value: '', label: 'All tools' },
    ...ALL_TOOLS.map((t) => ({ value: t, label: t })),
  ]);
  toolSelect.addEventListener('change', () => {
    filterTool = toolSelect.value;
    void loadCalls();
  });

  const callsCard = card('Calls', [
    h('div.hub-filters', null, tokenFilterSlot, field('Tool', toolSelect)),
    callsSlot,
  ]);

  // ---- Mount ------------------------------------------------------------------------------------------------------

  replace(root, h('div.hub-page', null, enableCard, formCard, tokensCard, callsCard));
  drawOrigins();
  drawTools();
  drawAuth();
  void loadStatus();
  void loadTokens();
  void loadCalls();

  return () => {
    alive = false;
    replace(root);
  };
}
