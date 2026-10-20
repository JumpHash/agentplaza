// Settings > Connections > MCP: the MCP servers the hub runs for agents. A table of servers with their status and row
// actions (start or stop, logs, test, edit, delete), the catalogue of presets, and the tab "This hub as a server". The
// Add and Edit flows are the wizard (mcp-wizard.ts); the logs are a drawer (mcp-logs.ts). The table turns into stacked
// rows on narrow screens (mcp.css is the prototype's settings-b.css).

import '../styles/proto/settings-b.css';
import { mcpApi, type McpPreset, type McpServerView, type McpTestResult, type McpToolRow } from '../api';
import { ago, attempt, badge, card, clip, tabs, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { on } from '../store';
import type { Cleanup } from './index';
import { openLogsDrawer } from './mcp-logs';
import { openWizard } from './mcp-wizard';
import { mountHubTab } from './mcp-hub';
import { STATUS_LABEL, STATUS_TONE, btn, dialog, emptyBox, iconBtn, kv, pageHead, statusSummary, tbl, transportText } from './mcp-common';
import { errorText } from './vault-common';

const RISK_TONE: Record<string, Tone> = { read: 'ok', write: 'info', irreversible: 'danger' };

/** A tool's MCP annotations as chips: every hint set to true (readOnlyHint, destructiveHint, ...). */
function annotationChips(a: unknown): HTMLElement[] {
  if (!a || typeof a !== 'object') return [];
  return Object.entries(a as Record<string, unknown>)
    .filter(([, v]) => v === true)
    .map(([k]) => h('span.mcp-chip', { title: k }, k));
}

/** One tool as the prototype draws it: its name and risk, the chips, and the description. `extra` follows the risk. */
function toolRow(t: { name: string; risk: string; description?: string; annotations?: unknown }, extra: Node | null = null): HTMLElement {
  return h(
    'div.mcp-tool',
    null,
    h('div.mcp-row', null, h('code.mcp-tn', null, t.name), badge(t.risk, RISK_TONE[t.risk] ?? 'muted'), extra),
    h('div.mcp-row', null, ...annotationChips(t.annotations)),
    h('p.muted', null, clip(t.description ?? '', 160)),
  );
}

/** The test result for one server: the error and stderr, or the server's facts and its tools. */
function testBody(r: McpTestResult | undefined): Node[] {
  if (!r) return [h('p.mcp-danger', null, 'The test could not run.')];
  if (!r.ok) {
    const err: (HTMLElement | null)[] = [h('p.mcp-danger', null, r.error ?? 'The server did not start.'), r.stderr?.length ? h('pre.mcp-pre.mono', null, r.stderr.map((l) => l.line).join('\n')) : null];
    return err.filter((x): x is HTMLElement => x !== null);
  }
  const tools = r.tools ?? [];
  const out: (Node | null)[] = [
    r.server_info ? kv([['Server', r.server_info.name], ['Version', r.server_info.version], ['Protocol', r.server_info.protocol]]) : null,
    h('h4', null, `Tools (${tools.length})`),
    ...(tools.length ? tools.map((t) => toolRow(t)) : [h('p.muted', null, 'This server offers no tools.')]),
  ];
  return out.filter((x): x is Node => x !== null);
}

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  let timer: number | undefined;
  let loadSeq = 0;
  let servers: McpServerView[] | null = null;
  let catalog: McpPreset[] = [];
  let loadError: unknown = null;

  const reload = async (): Promise<void> => {
    const seq = ++loadSeq;
    try {
      const [list, presets] = await Promise.all([mcpApi.servers(), mcpApi.catalog()]);
      if (!alive || seq !== loadSeq) return;
      servers = list;
      catalog = presets;
      loadError = null;
    } catch (err) {
      if (!alive || seq !== loadSeq) return;
      servers = null;
      loadError = err;
    }
    draw();
  };

  /** Mutations reload once the call settles, whether or not it succeeded. */
  const act = (run: () => Promise<unknown>, success: string): void => {
    void attempt(run, success).then(() => reload());
  };

  const later = (): void => {
    if (timer !== undefined) clearTimeout(timer);
    timer = window.setTimeout(() => {
      timer = undefined;
      void reload();
    }, 250);
  };

  const openAdd = (presetId: string | null = null): void => openWizard(null, () => void reload(), presetId);

  /** The dialog that shows a server's tools: the hub's own list, the same one the wizard's step 4 reads. */
  const openTools = (s: McpServerView): void => {
    const box = h('div', null, h('p.muted', null, 'Loading the tools'));
    const close = dialog(`Tools of ${s.name}`, box, [btn('Close', () => close(), { kind: 'ghost' })]);
    void attempt(() => mcpApi.serverTools(s.id)).then((rows: McpToolRow[] | undefined) => {
      if (!alive) return;
      replace(box, toolsBody(rows));
    });
  };

  function toolsBody(rows: McpToolRow[] | undefined): Node[] | HTMLElement {
    if (!rows) return h('p.mcp-danger', null, 'The tools could not load.');
    if (!rows.length) return h('p.muted', null, 'This server offers no tools.');
    return rows.map((t) => toolRow(t, t.allowed ? null : badge('Not offered', 'muted')));
  }

  function deleteDialog(s: McpServerView, onConfirm: (purge: boolean) => void): void {
    const purgeBox = h('input', { type: 'checkbox' }) as HTMLInputElement;
    const bodyNodes: Node[] = [h('p', null, `This stops ${s.name} and removes it. Its vault secrets stay, but lose the mcp:${s.id} binding.`)];
    if (s.storage?.volume) {
      bodyNodes.push(h('label.check', null, purgeBox, h('span', null, 'Also delete its storage volume')));
      bodyNodes.push(h('p.muted', null, 'The volume keeps what the server stored between runs. It is deleted only when this is ticked.'));
    }
    const close = dialog(
      `Delete ${s.name}`,
      bodyNodes,
      [
        btn('Cancel', () => close(), { kind: 'ghost' }),
        btn(
          'Delete',
          () => {
            const purge = purgeBox.checked;
            close();
            onConfirm(purge);
          },
          { kind: 'danger' },
        ),
      ],
    );
  }

  const addButton = (): HTMLElement => btn('Add server', () => openAdd(), { kind: 'primary', icon: 'plus' });

  /** The hub's presets: each opens the wizard with that preset chosen; the custom button opens it empty. */
  const catalogueCard = (): HTMLElement =>
    h(
      'div.mcp-cat',
      null,
      card(
        'Catalogue',
        catalog.length
          ? catalog.map((p) =>
              h(
                'div.mcp-cat-row',
                null,
                h('div', null, h('strong.mono', null, p.name), h('p.muted', null, p.description), h('p.muted', null, [p.transport, p.note].filter(Boolean).join(' · '))),
                btn('Add', () => openAdd(p.id), { kind: 'ghost', sm: true }),
              ),
            )
          : emptyBox('terminal', 'No presets', 'The catalogue is empty on this hub.'),
        { actions: [btn('Custom server', () => openAdd(), { kind: 'ghost' })] },
      ),
    );

  // The two tabs: the servers, and this hub as a server. Leaving the hub tab empties its page, token included.
  let tab: 'servers' | 'hub' = 'servers';
  let hubCleanup: Cleanup | null = null;
  /** The body under the tabs: the servers, or the hub's own cards (mountHubTab draws into it). */
  const bodyEl = h('div.mcp-body', null);
  const tabBar = (): HTMLElement =>
    tabs(
      [
        { key: 'servers', label: 'MCP servers' },
        { key: 'hub', label: 'This hub as a server' },
      ],
      tab,
      (key) => setTab(key === 'hub' ? 'hub' : 'servers'),
    );
  const setTab = (next: 'servers' | 'hub'): void => {
    if (next === tab) return;
    if (hubCleanup) {
      hubCleanup();
      hubCleanup = null;
    }
    tab = next;
    draw();
    if (next === 'hub') hubCleanup = mountHubTab(bodyEl);
  };

  function actionsFor(s: McpServerView): HTMLElement {
    return h(
      'div.mcp-row',
      null,
      s.status === 'running'
        ? btn('Stop', () => act(() => mcpApi.stopServer(s.id), `${s.name} stopped`), { kind: 'ghost', sm: true, icon: 'square', title: `Stop ${s.name}` })
        : btn('Start', () => act(() => mcpApi.startServer(s.id), `${s.name} started`), { kind: 'ghost', sm: true, icon: 'play', title: `Start ${s.name}` }),
      iconBtn('file-text', () => openLogsDrawer(s), `Logs for ${s.name}`),
      iconBtn('check', () => runTest(s), `Test ${s.name}`),
      iconBtn('pencil', () => openWizard(s, () => void reload()), `Edit ${s.name}`),
      iconBtn('trash-2', () => deleteDialog(s, (purge) => act(() => mcpApi.deleteServer(s.id, purge), `${s.name} deleted`)), `Delete ${s.name}`),
    );
  }

  function runTest(s: McpServerView): void {
    const box = h('div', null, h('p.muted', null, 'Testing'));
    const close = dialog(`Test ${s.name}`, box, [btn('Close', () => close(), { kind: 'ghost' })]);
    void attempt(() => mcpApi.testServer(s.id)).then((r) => {
      if (!alive) return;
      replace(box, testBody(r));
    });
  }

  function serversTable(): HTMLElement {
    return h(
      'div.mcp-tbl',
      null,
      tbl<McpServerView>(
        [
          { label: 'Server', cell: (s) => h('div', null, h('strong', null, s.name), h('div.mono.muted', null, s.id)) },
          { label: 'Transport', cls: 'mcp-tr', cell: (s) => h('span.mono.mcp-trunc', { title: transportText(s.transport) }, transportText(s.transport)) },
          {
            label: 'Status',
            cell: (s) => {
              const summary = statusSummary(s);
              return h(
                'div',
                null,
                h('div.mcp-row', null, badge(STATUS_LABEL[s.status], STATUS_TONE[s.status]), s.enabled ? null : badge('Disabled', 'muted')),
                summary ? h('div.mcp-err', { title: s.last_error ?? '' }, summary) : null,
                summary ? btn('View logs', () => openLogsDrawer(s), { kind: 'ghost', sm: true, icon: 'file-text', title: `Logs for ${s.name}` }) : null,
              );
            },
          },
          { label: 'Tools', cell: (s) => btn(String(s.tools_count), () => openTools(s), { kind: 'ghost', sm: true, title: `Tools of ${s.name}` }) },
          { label: 'Last used', cell: (s) => (s.last_used_ms ? ago(s.last_used_ms) : 'never') },
          { label: 'Actions', cell: (s) => actionsFor(s) },
        ],
        servers ?? [],
        (s) => s.id,
      ),
    );
  }

  /** The servers tab: the add button, the table (or the empty state), and the catalogue. */
  function serversTab(): (Node | null)[] {
    if (loadError) {
      return [emptyBox('triangle-alert', 'Could not load MCP servers', errorText(loadError), btn('Try again', () => void reload(), { kind: 'ghost' }))];
    }
    if (!servers) return [h('p.mcp-muted', null, 'Loading MCP servers...')];
    if (!servers.length) {
      return [
        emptyBox('server', 'No MCP servers', 'Add one to give agents outside tools. The hub runs each server, and agents never see its credentials.', addButton()),
        catalogueCard(),
      ];
    }
    return [h('div.mcp-row', null, addButton()), serversTable(), catalogueCard()];
  }

  function draw(): void {
    if (tab === 'servers') replace(bodyEl, serversTab());
    replace(root, pageHead('MCP servers'), tabBar(), bodyEl);
  }

  const offs = [on('mcp.changed', later), on('mcp.status', later)];

  draw();
  void reload();

  return () => {
    alive = false;
    if (hubCleanup) hubCleanup();
    if (timer !== undefined) clearTimeout(timer);
    offs.forEach((off) => off());
  };
}
