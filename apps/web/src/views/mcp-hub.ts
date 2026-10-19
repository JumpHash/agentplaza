// Setup > MCP > This hub as a server, in the prototype's markup: the owner turns the hub's own MCP server on, mints
// tokens for outside clients, and reads the tokens and the call log (docs/design/hub-mcp.md, "Configuration flow").
// A new token is shown once, in the reveal box. It is never written to localStorage, and leaving the tab clears the
// page, reveal box included.

import '../styles/proto/settings-b.css';
import {
  acpApi,
  api,
  hubMcpApi,
  type AcpSession,
  type HubAuthMode,
  type HubBind,
  type HubCall,
  type HubMcpSettings,
  type HubMcpStatus,
  type HubToken,
} from '../api';
import { ago, attempt, badge, card, clip, field, input, select, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import { agents } from '../store';
import type { Cleanup } from './index';
import { btn, check, dialog, emptyBox, iconBtn, tbl, type TblCol } from './mcp-common';
import { HUB_SCOPES, HUB_TOOLS, acpSnippets, scopePermits, setupSnippets, type HubScope } from './hub-mcp-snippets';
import { errorText } from './vault-common';

const BIND_OPTIONS: { value: HubBind; label: string }[] = [
  { value: 'local', label: 'Local only' },
  { value: 'remote', label: 'Remote over TLS' },
];

/** Secure needs a token; open lets anonymous callers in with the open scope. */
const AUTH_OPTIONS: { value: HubAuthMode; label: string }[] = [
  { value: 'secure', label: 'Secure: token required' },
  { value: 'open', label: 'Open: no token' },
];

/** Each tool's own scope, for the checklist and for the scope change. */
const TOOL_SCOPE = new Map<string, HubScope>(
  (Object.keys(HUB_TOOLS) as HubScope[]).flatMap((scope) => HUB_TOOLS[scope].map((name) => [name, scope] as const)),
);
const ALL_TOOLS = Array.from(TOOL_SCOPE.keys());

/** The editor setup text (Zed, JetBrains, Neovim). It holds no token, so it is built once. */
const ACP_SNIPPETS = acpSnippets();

/** A radio in the prototype's label.check. check() draws only checkboxes, so the radios are drawn here. */
function radioRow(
  name: string,
  value: string,
  text: string,
  checked: boolean,
  onPick: () => void,
): { row: HTMLLabelElement; el: HTMLInputElement } {
  const el = h('input', { type: 'radio', name, value }) as HTMLInputElement;
  el.checked = checked;
  el.addEventListener('change', () => {
    if (el.checked) onPick();
  });
  return { el, row: h('label.check', null, el, h('span', null, text)) };
}

/** A checkbox in the prototype's label.check, with its input kept so the tab can read and set it. */
function checkRow(
  text: string,
  onChange?: (on: boolean) => void,
  disabled = false,
): { row: HTMLLabelElement; box: HTMLInputElement } {
  const row = check(text, onChange ? { onChange, disabled } : { disabled });
  return { row, box: row.querySelector('input') as HTMLInputElement };
}

/** A ghost copy button for one piece of text. The text is never stored; the click copies it and says so. */
function copyButton(text: string, label = 'Copy'): HTMLButtonElement {
  return btn(label, () => void attempt(() => navigator.clipboard.writeText(text), 'Copied'), { kind: 'ghost', sm: true });
}

/** One block of setup text: its title and copy button, the note on what it does, then the text. */
function snippet(title: string, note: string, text: string): HTMLElement {
  return h(
    'div.mcp-snip',
    null,
    h('div.mcp-row', null, h('strong', null, title), copyButton(text)),
    h('p.muted', null, note),
    h('pre.mono', null, text),
  );
}

/** The revoke confirm: a sheet with the warning, Cancel, and Revoke in the danger tone. */
function confirmRevoke(name: string, onConfirm: () => void): void {
  const close: () => void = dialog(
    `Revoke ${name}`,
    h('p', null, 'Clients that use this token lose access at once. This cannot be undone.'),
    [
      btn('Cancel', () => close(), { kind: 'ghost' }),
      btn(
        'Revoke',
        () => {
          close();
          onConfirm();
        },
        { kind: 'danger' },
      ),
    ],
  );
}

/** The tab. It fills `root` with the prototype's stack of cards and returns a cleanup that empties it, so no token outlives the tab. */
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

  const statusSlot = h('div.mcp-stack', null, h('p.muted', null, 'Loading...'));
  const originsSlot = h('div.mcp-stack');
  const effectiveLine = h('p.muted');
  const toolsSlot = h('fieldset.mcp-tools');
  const revealSlot = h('div');
  revealSlot.hidden = true;
  const tokensSlot = h('div', null, h('p.muted', null, 'Loading...'));
  const tokenFilterSlot = h('div');
  const callsSlot = h('div', null, h('p.muted', null, 'Loading...'));

  // ---- Enable -----------------------------------------------------------------------------------------------------

  const enabledCheck = checkRow('Serve this hub over MCP', (on) => {
    enabled = on;
  });

  const bindRows = BIND_OPTIONS.map((o) =>
    radioRow('hub-bind', o.value, o.label, o.value === bind, () => {
      bind = o.value;
      drawAuth();
    }),
  );

  const syncControls = (): void => {
    enabledCheck.box.checked = enabled;
    BIND_OPTIONS.forEach((o, i) => {
      const row = bindRows[i];
      if (row) row.el.checked = o.value === bind;
    });
  };

  const drawOrigins = (): void => {
    replace(
      originsSlot,
      origins.length
        ? origins.map((o) =>
            h(
              'div.mcp-row',
              null,
              h('span.mono', null, o),
              iconBtn(
                'x',
                () => {
                  origins = origins.filter((x) => x !== o);
                  drawOrigins();
                },
                `Remove ${o}`,
              ),
            ),
          )
        : h('p.muted', null, 'No origins yet.'),
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

  // The origins the endpoint accepts in practice, read-only: the hub's own, then the owner's.
  const drawEffective = (): void => {
    const list = status?.effective_origins ?? [];
    replace(effectiveLine, `Effective origins: ${list.length ? list.join(', ') : 'None.'}`);
  };

  /** The status block: the badge and the TLS line, the live tokens and the URL, and the database note when it is off. */
  const drawStatus = (): void => {
    if (statusError) {
      replace(
        statusSlot,
        emptyBox(
          'triangle-alert',
          'Could not load the hub server',
          errorText(statusError),
          btn('Try again', () => void loadStatus(), { kind: 'ghost' }),
        ),
      );
      return;
    }
    if (!status) {
      replace(statusSlot, h('p.muted', null, 'Loading...'));
      return;
    }
    replace(
      statusSlot,
      h(
        'div.mcp-row',
        null,
        badge(
          status.enabled ? `On, ${status.bind === 'remote' ? 'remote over TLS' : 'local only'}` : 'Off',
          status.enabled ? 'ok' : 'muted',
        ),
        badge(status.tls ? 'TLS on' : 'TLS off', status.tls ? 'ok' : 'warn'),
        h(
          'p.muted',
          null,
          status.tls
            ? 'This endpoint serves TLS, so remote bind is available.'
            : 'Remote bind needs TLS. This endpoint has none, so the hub refuses remote bind until it serves TLS.',
        ),
      ),
      h(
        'p.muted',
        null,
        `${status.tokens === 1 ? '1 live token' : `${status.tokens} live tokens`} · `,
        h('span.mono', null, status.url),
      ),
      status.stored ? null : h('p.mcp-warn', null, 'The database is off, so these settings last only until the hub restarts.'),
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
    drawEffective();
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

  const authRows = AUTH_OPTIONS.map((o) =>
    radioRow('hub-auth', o.value, o.label, o.value === authMode, () => {
      authMode = o.value;
      drawAuth();
    }),
  );

  const openScopeSelect = select(
    HUB_SCOPES.map((s) => ({ value: s.value, label: `${s.label}: ${s.plain}` })),
    openScope,
  );
  openScopeSelect.addEventListener('change', () => {
    openScope = openScopeSelect.value as HubScope;
  });
  const openScopeField = field('Open scope (anonymous callers)', openScopeSelect);
  const openWarn = h('p.mcp-warn', null, 'Open mode lets callers without a token use this hub at the open scope.');
  const serverWarn = h('p.mcp-warn');
  const confirmCheck = checkRow('I understand anyone on the network can use it');

  const drawAuth = (): void => {
    const open = authMode === 'open';
    for (const a of authRows) a.el.checked = a.el.value === authMode;
    openScopeSelect.value = openScope;
    openScopeField.hidden = !open;
    openWarn.hidden = !open;
    replace(serverWarn, warning ?? '');
    serverWarn.hidden = !(open && warning);
    confirmCheck.row.hidden = !(open && bind === 'remote');
  };

  const save = (): void => {
    const body: HubMcpSettings = { enabled, bind, allowed_origins: origins, auth_mode: authMode, open_scope: openScope };
    if (authMode === 'open' && bind === 'remote') body.confirm_open_remote = confirmCheck.box.checked;
    void attempt(() => hubMcpApi.save(body), 'Saved').then((next) => {
      if (alive && next) applyStatus(next);
    });
  };

  const enableCard = card(
    'Enable',
    h(
      'div.mcp-stack',
      null,
      statusSlot,
      enabledCheck.row,
      field('Bind', h('div.mcp-row', null, bindRows.map((b) => b.row))),
      field('Auth', h('div.mcp-row', null, authRows.map((a) => a.row))),
      openScopeField,
      openWarn,
      serverWarn,
      confirmCheck.row,
      field(
        'Allowed origins',
        h('div.mcp-row', null, originInput, btn('Add', addOrigin, { kind: 'ghost', sm: true, icon: 'plus' })),
      ),
      originsSlot,
      effectiveLine,
      h(
        'div.mcp-row',
        null,
        btn(
          'Copy URL',
          () => {
            if (status) void attempt(() => navigator.clipboard.writeText(status?.url ?? ''), 'Copied');
          },
          { kind: 'ghost', sm: true },
        ),
      ),
    ),
    { actions: [btn('Save', save, { kind: 'primary', sm: true })] },
  );

  // ---- New token --------------------------------------------------------------------------------------------------

  const nameInput = input({ maxlength: 80, placeholder: 'laptop', 'aria-label': 'Name' });
  const expiresInput = input({
    type: 'number',
    min: 1,
    max: 3650,
    placeholder: 'Never',
    'aria-label': 'Expires after (days)',
  });

  /** The tool checklist, grouped by scope. A group above the chosen scope is greyed out. */
  const drawTools = (): void => {
    replace(
      toolsSlot,
      h('legend', null, 'Tools (optional)'),
      h('p.muted', null, 'Leave them all unticked for every tool the scope allows.'),
      HUB_SCOPES.map((group) => {
        const allowed = scopePermits(scope, group.value);
        return h(
          'fieldset.mcp-grp',
          null,
          h('legend', null, allowed ? group.value : `${group.value} (above this scope)`),
          HUB_TOOLS[group.value].map((name) => {
            const box = h('input', { type: 'checkbox', disabled: !allowed }) as HTMLInputElement;
            box.checked = picked.has(name);
            box.addEventListener('change', () => {
              if (box.checked) picked.add(name);
              else picked.delete(name);
            });
            return h('label.check', null, box, h('span.mono', null, name));
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

  const scopeRows = HUB_SCOPES.map(
    (s) => radioRow('hub-token-scope', s.value, `${s.label}: ${s.plain}`, s.value === scope, () => setScope(s.value)).row,
  );

  const hideReveal = (): void => {
    replace(revealSlot);
    revealSlot.hidden = true;
  };

  /** Shows a new token once, with the setup text for each client. The token lives only in this reveal box. */
  const showReveal = (token: string, name: string): void => {
    const s = setupSnippets(status ? status.url : '', authMode === 'open' ? null : token);
    replace(
      revealSlot,
      h(
        'div.mcp-reveal',
        null,
        h('h4', null, `Token for ${name}`),
        h('p.mcp-warn', null, 'Shown once. Copy it now, because it cannot be shown again.'),
        h('div.mcp-row', null, h('pre.mono.mcp-pre', null, token), copyButton(token, 'Copy token')),
        snippet('Claude Code', 'Run this in a terminal.', s.claudeCode),
        snippet(
          'Claude Desktop',
          'Add this to the MCP servers of Claude Desktop or another stdio client. It runs the stdio bridge against this hub and never carries a token.',
          s.desktop,
        ),
        s.desktopLogin
          ? snippet(
              'Save the token',
              'Reads the token from stdin and saves it in cli.json. Paste the token, then end the input.',
              s.desktopLogin,
            )
          : null,
        snippet('Generic', 'The URL and the header, for any other client.', s.generic),
        btn('Hide', hideReveal, { kind: 'ghost' }),
      ),
    );
    revealSlot.hidden = false;
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
    h(
      'div.mcp-stack',
      null,
      field('Name', nameInput, 'What this client is, such as laptop or ci.'),
      field('Scope', h('div.mcp-stack', null, scopeRows)),
      toolsSlot,
      field('Expires after (days)', expiresInput, 'Blank means the token never expires.'),
      h('div.mcp-row', null, btn('Create token', () => void createToken(), { kind: 'primary', icon: 'plus' })),
      revealSlot,
    ),
  );

  // ---- Tokens -----------------------------------------------------------------------------------------------------

  const tokenName = (id: string): string => tokens.find((t) => t.id === id)?.name ?? id;

  const tokenCols: TblCol<HubToken>[] = [
    {
      label: 'Name',
      cell: (t) => h('div', null, h('strong', null, t.name), h('div.mono.muted', null, t.id)),
    },
    { label: 'Scope', cell: (t) => badge(t.scope, 'info') },
    {
      label: 'Tools',
      cell: (t) =>
        t.tools.length
          ? h('span.mono', null, clip(t.tools.join(', '), 80))
          : h('span.muted', null, "All the scope's tools"),
    },
    { label: 'Last used', cell: (t) => (t.last_used_ms ? ago(t.last_used_ms) : 'never') },
    { label: 'Calls', cell: (t) => String(t.calls) },
    { label: 'Expires', cell: (t) => (t.expires_ms ? new Date(t.expires_ms).toLocaleDateString() : 'never') },
    {
      label: 'Actions',
      cell: (t) =>
        btn(
          'Revoke',
          () =>
            confirmRevoke(t.name, () => void attempt(() => hubMcpApi.revokeToken(t.id), `${t.name} revoked`).then(() => loadTokens())),
          { kind: 'danger', sm: true, icon: 'trash-2', title: `Revoke ${t.name}` },
        ),
    },
  ];

  const drawTokens = (): void => {
    if (tokensError) {
      replace(tokensSlot, emptyBox('triangle-alert', 'Could not load the tokens', errorText(tokensError)));
      return;
    }
    const live = tokens.filter((t) => t.revoked_at_ms == null);
    replace(
      tokensSlot,
      live.length
        ? tbl(tokenCols, live, (t) => t.id)
        : emptyBox('key', 'No tokens', 'Make one above to give an outside client access to this hub.'),
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

  const callCols: TblCol<HubCall>[] = [
    { label: 'Time', cell: (c) => new Date(c.at_ms).toLocaleString() },
    { label: 'Token', cell: (c) => tokenName(c.token_id) },
    { label: 'Tool', cell: (c) => h('code', null, c.tool) },
    {
      label: 'Result',
      cell: (c) =>
        c.ok
          ? badge('ok', 'ok')
          : h('span', null, badge('error', 'danger'), ' ', h('span.muted', null, clip(c.error ?? '', 120))),
    },
    { label: 'Duration', cell: (c) => `${c.duration_ms} ms` },
  ];

  const drawCalls = (): void => {
    if (callsError) {
      replace(callsSlot, emptyBox('triangle-alert', 'Could not load the calls', errorText(callsError)));
      return;
    }
    replace(
      callsSlot,
      calls.length
        ? tbl(callCols, calls, (c) => `${c.at_ms}-${c.tool}`)
        : emptyBox('message-square', 'No calls', 'Calls appear here once a client uses a token.'),
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

  const callsCard = card(
    'Calls',
    h('div.mcp-stack', null, h('div.mcp-row', null, tokenFilterSlot, field('Tool', toolSelect)), callsSlot),
  );

  // ---- Editors (ACP) ---------------------------------------------------------------------------------------------

  // The switches save at once, as the strict permits switch does: the whole settings document is read, the one
  // field changed, and saved back. A failed save puts the switch back where it was. Both wait while either save is
  // on its way, since each save reads the whole document and two at once could overwrite each other.
  let acpSaving = false;
  const acpBox = checkRow('Let editors prompt a plaza agent', (on) =>
    void saveAcpFlag('acp_enabled', on, on ? 'Editors may prompt agents' : 'Editors are off'),
  );
  const writeBack = checkRow("Offer to apply an agent's change to the editor's project", (on) =>
    void saveAcpFlag(
      'acp_write_back',
      on,
      on ? "The editor will offer to apply agents' changes" : "The editor will not offer to apply agents' changes",
    ),
  );
  const v2Box = checkRow('Speak the ACP v2 draft', (on) =>
    void saveAcpFlag(
      'acp_v2',
      on,
      on ? 'Editors and engines that ask for protocol 2 get the ACP v2 draft' : 'Editors and engines are back on the stable v1',
    ),
  );
  const acpFlagBoxes = { acp_enabled: acpBox.box, acp_write_back: writeBack.box, acp_v2: v2Box.box };
  // The write-back switch stays greyed out while Editors (ACP) is off: no editor is there to offer the change.
  // The v2 switch is not greyed out by Editors: the engines inside the VMs speak v2 too.
  const drawAcpSwitches = (): void => {
    acpBox.box.disabled = acpSaving;
    writeBack.box.disabled = acpSaving || !acpBox.box.checked;
    v2Box.box.disabled = acpSaving;
  };
  const saveAcpFlag = async (
    key: 'acp_enabled' | 'acp_write_back' | 'acp_v2',
    on: boolean,
    success: string,
  ): Promise<void> => {
    const box = acpFlagBoxes[key];
    acpSaving = true;
    drawAcpSwitches();
    const saved = await attempt(async () => {
      const current = await api.settings();
      return api.saveSettings({ ...current, [key]: on });
    }, success);
    if (!saved && alive) box.checked = !on;
    acpSaving = false;
    if (alive) drawAcpSwitches();
  };
  drawAcpSwitches();

  const loadAcpSwitch = async (): Promise<void> => {
    try {
      const current = await api.settings();
      if (!alive) return;
      acpBox.box.checked = current.acp_enabled === true;
      writeBack.box.checked = current.acp_write_back === true;
      v2Box.box.checked = current.acp_v2 === true;
      drawAcpSwitches();
    } catch (err) {
      if (alive) toast(errorText(err), 'error');
    }
  };

  const sessionsSlot = h('div', null, h('p.muted', null, 'Loading...'));
  let sessions: AcpSession[] = [];
  let sessionsError: unknown = null;

  const agentName = (cid: number): string => agents.get().find((a) => a.cid === cid)?.name ?? `agent ${cid}`;

  const sessionCols: TblCol<AcpSession>[] = [
    { label: 'Title', cell: (s) => s.title || 'untitled' },
    { label: 'Agent', cell: (s) => agentName(s.cid) },
    { label: 'Client', cell: (s) => s.client || 'unknown' },
    { label: 'Folder', cell: (s) => h('span.mono', null, clip(s.cwd, 60)) },
    { label: 'Updated', cell: (s) => ago(s.updated_ms) },
    { label: 'State', cell: (s) => (s.closed ? badge('closed', 'muted') : badge('open', 'ok')) },
  ];

  const drawSessions = (): void => {
    if (sessionsError) {
      replace(sessionsSlot, emptyBox('triangle-alert', 'Could not load the sessions', errorText(sessionsError)));
      return;
    }
    replace(
      sessionsSlot,
      sessions.length
        ? tbl(sessionCols, sessions, (s) => `${s.cid}-${s.updated_ms}`)
        : emptyBox('message-square', 'No editor sessions', 'Sessions appear here once an editor prompts a plaza agent.'),
    );
  };

  const loadSessions = async (): Promise<void> => {
    try {
      const r = await acpApi.sessions();
      if (!alive) return;
      sessions = [...r].sort((a, b) => b.updated_ms - a.updated_ms);
      sessionsError = null;
    } catch (err) {
      if (!alive) return;
      sessionsError = err;
    }
    drawSessions();
  };

  const editorsCard = card(
    'Editors (ACP)',
    h(
      'div.mcp-stack',
      null,
      acpBox.row,
      h(
        'p.muted',
        null,
        "Zed, JetBrains IDEs, Neovim and other ACP clients prompt a plaza agent from the editor; its approvals are asked in the editor. Agents never reach your machine: the editor's MCP servers and files stay local.",
      ),
      writeBack.row,
      h(
        'p.muted',
        null,
        'Write-back runs through agentplaza acp --repo, on a lane of its own, mirrored in Weft. The editor writes the files; the agent never does.',
      ),
      v2Box.row,
      h(
        'p.muted',
        null,
        "v2 is a draft that still changes, and it has no write-back: the editor's file methods are gone in v2.",
      ),
      snippet('Zed', 'Add this to the settings.json of Zed. It runs the default plaza agent.', ACP_SNIPPETS.zed),
      snippet(
        'Zed, one agent',
        "The same, pinned to one plaza agent. Replace <name> with the agent's name.",
        ACP_SNIPPETS.zedAgent,
      ),
      snippet('JetBrains IDEs', 'AI Assistant › ACP agents: enter the command and the arguments.', ACP_SNIPPETS.jetbrains),
      snippet('Neovim (CodeCompanion)', 'Under the adapters of the CodeCompanion setup.', ACP_SNIPPETS.neovim),
      sessionsSlot,
    ),
    { actions: [btn('Refresh', () => void loadSessions(), { kind: 'ghost', sm: true })] },
  );

  // ---- Mount ------------------------------------------------------------------------------------------------------

  replace(root, h('div.mcp-stack', null, enableCard, formCard, tokensCard, callsCard, editorsCard));
  syncControls();
  drawOrigins();
  drawEffective();
  drawTools();
  drawAuth();
  drawTokenFilter();
  void loadStatus();
  void loadTokens();
  void loadCalls();
  void loadAcpSwitch();
  void loadSessions();

  return () => {
    alive = false;
    replace(root);
  };
}
