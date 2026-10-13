// Setup > MCP: the MCP servers the hub runs for agents. A list of servers with their
// status, and row actions: start or stop, logs, test, edit, delete. The Add and Edit
// flows are the wizard (mcp-wizard.ts). Laid out like Vault's tables: a card with a
// table that turns into stacked cards on narrow screens.

import '../styles/mcp.css';
import { mcpApi, type McpServerView, type McpTestResult } from '../api';
import { ago, attempt, badge, button, clip, empty, modal, tabs, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { on } from '../store';
import type { Cleanup } from './index';
import { openLogsDrawer } from './mcp-logs';
import { openWizard } from './mcp-wizard';
import { mountHubTab } from './mcp-hub';
import { STATUS_LABEL, STATUS_TONE, statusSummary, transportText } from './mcp-common';
import { errorText } from './vault-common';

const RISK_TONE: Record<string, Tone> = { read: 'ok', write: 'info', irreversible: 'danger' };

/** A tool's MCP annotations as chips: every hint set to true (readOnlyHint, destructiveHint, ...). */
function annotationChips(a: unknown): HTMLElement[] {
  if (!a || typeof a !== 'object') return [];
  return Object.entries(a as Record<string, unknown>)
    .filter(([, v]) => v === true)
    .map(([k]) => badge(k, 'muted'));
}

/** A small confirm dialog: a message, Cancel, and the action in the danger tone. */
function confirmDialog(title: string, text: string, label: string, onConfirm: () => void): void {
  const close = modal(
    title,
    h(
      'div.mcp-confirm',
      null,
      h('p', null, text),
      h('div.mcp-actions', null, button('Cancel', () => close()), button(label, () => { close(); onConfirm(); }, { kind: 'danger' })),
    ),
  );
}

/** The test result for one server: the server's name, version and protocol, then the tools; or the error and stderr. */
function testBody(r: McpTestResult | undefined): HTMLElement {
  if (!r) return h('p.mcp-muted', null, 'The test could not run.');
  if (!r.ok) {
    return h(
      'div.mcp-test',
      null,
      h('p.mcp-danger', null, r.error ?? 'The server did not start.'),
      r.stderr?.length ? h('pre.mcp-mono', null, r.stderr.map((l) => l.line).join('\n')) : null,
    );
  }
  const info = r.server_info;
  return h(
    'div.mcp-test',
    null,
    info
      ? h('dl.mcp-facts', null, h('dt', null, 'Server'), h('dd', null, info.name), h('dt', null, 'Version'), h('dd', null, info.version), h('dt', null, 'Protocol'), h('dd', null, info.protocol))
      : null,
    h('h3', null, `Tools (${r.tools?.length ?? 0})`),
    r.tools?.length
      ? h(
          'ul.mcp-tools',
          null,
          r.tools.map((t) =>
            h(
              'li',
              null,
              h('strong', null, t.name.replace(/_/g, '_\u200b')),
              h('span.mcp-chips', null, badge(t.risk, RISK_TONE[t.risk] ?? 'muted'), ...annotationChips(t.annotations)),
              t.description ? h('span.mcp-muted', null, clip(t.description, 160)) : null,
            ),
          ),
        )
      : h('p.mcp-muted', null, 'This server offers no tools.'),
  );
}

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  let timer: number | undefined;
  let loadSeq = 0;
  let servers: McpServerView[] | null = null;
  let loadError: unknown = null;

  const reload = async (): Promise<void> => {
    const seq = ++loadSeq;
    try {
      const list = await mcpApi.servers();
      if (!alive || seq !== loadSeq) return;
      servers = list;
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

  const openAdd = (): void => openWizard(null, () => void reload());

  const addButton = (): HTMLElement => button('Add server', openAdd, { kind: 'primary', icon: 'plus' });

  // The two tabs: the servers, and this hub as a server. Leaving the hub tab empties its page, token included.
  let tab: 'servers' | 'hub' = 'servers';
  let hubCleanup: Cleanup | null = null;
  const hubBody = h('div.mcp-hub-host');
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
    if (next === 'hub') hubCleanup = mountHubTab(hubBody);
    draw();
  };

  function actionsFor(s: McpServerView): HTMLElement {
    const running = s.status === 'running';
    return h(
      'div.mcp-row-actions',
      null,
      running
        ? button('Stop', () => act(() => mcpApi.stopServer(s.id), `${s.name} stopped`), { kind: 'ghost', size: 'sm', icon: 'stop', title: `Stop ${s.name}` })
        : button('Start', () => act(() => mcpApi.startServer(s.id), `${s.name} started`), { kind: 'ghost', size: 'sm', icon: 'play', title: `Start ${s.name}` }),
      button('', () => { openLogsDrawer(s); }, { kind: 'ghost', size: 'sm', icon: 'file', title: `Logs for ${s.name}` }),
      button('', () => runTest(s), { kind: 'ghost', size: 'sm', icon: 'check', title: `Test ${s.name}` }),
      button('', () => openWizard(s, () => void reload()), { kind: 'ghost', size: 'sm', icon: 'edit', title: `Edit ${s.name}` }),
      button(
        '',
        () =>
          confirmDialog(
            `Delete ${s.name}`,
            `This stops ${s.name} and removes it. Its vault secrets stay, but lose the mcp:${s.id} binding.`,
            'Delete',
            () => act(() => mcpApi.deleteServer(s.id), `${s.name} deleted`),
          ),
        { kind: 'danger', size: 'sm', icon: 'trash', title: `Delete ${s.name}` },
      ),
    );
  }

  function runTest(s: McpServerView): void {
    const box = h('div.mcp-test', null, h('p.mcp-muted', null, 'Testing'));
    modal(`Test ${s.name}`, box);
    void attempt(() => mcpApi.testServer(s.id)).then((r) => {
      if (!alive) return;
      replace(box, testBody(r));
    });
  }

  function row(s: McpServerView): HTMLElement {
    const transport = transportText(s.transport);
    const summary = statusSummary(s);
    return h(
      'tr',
      null,
      h(
        'td',
        { 'data-label': 'Server' },
        h('strong.mcp-name', null, s.name),
        h('span.mcp-id', null, s.id),
      ),
      h('td', { 'data-label': 'Transport' }, h('span.mcp-mono.mcp-truncate', { title: transport }, transport)),
      h(
        'td',
        { 'data-label': 'Status' },
        h('span.mcp-status', null, badge(STATUS_LABEL[s.status], STATUS_TONE[s.status], true), s.enabled ? null : badge('Disabled', 'muted')),
        summary
          ? h(
              'span.mcp-err',
              null,
              h('span.mcp-danger.mcp-summary', { title: s.last_error ?? '' }, summary),
              button('View logs', () => openLogsDrawer(s), { kind: 'ghost', size: 'sm', icon: 'file', title: `Logs for ${s.name}` }),
            )
          : null,
      ),
      h('td', { 'data-label': 'Tools' }, String(s.tools_count)),
      h('td', { 'data-label': 'Last used' }, s.last_used_ms ? ago(s.last_used_ms) : 'never'),
      h('td', { 'data-label': 'Actions', class: 'mcp-actions-cell' }, actionsFor(s)),
    );
  }

  function draw(): void {
    let body: HTMLElement;
    if (loadError) {
      body = empty('alert', 'Could not load MCP servers', errorText(loadError), button('Try again', () => void reload(), { kind: 'ghost' }));
    } else if (!servers) {
      body = h('p.mcp-loading', null, 'Loading MCP servers...');
    } else if (!servers.length) {
      body = empty(
        'terminal',
        'No MCP servers',
        'Add one to give agents outside tools. The hub runs each server, and agents never see its credentials.',
        addButton(),
      );
    } else {
      body = h(
        'div.mcp-table-wrap',
        null,
        h(
          'table.mcp-table',
          null,
          h('thead', null, h('tr', null, h('th', null, 'Server'), h('th', null, 'Transport'), h('th', null, 'Status'), h('th', null, 'Tools'), h('th', null, 'Last used'), h('th', null, 'Actions'))),
          h('tbody', null, servers.map(row)),
        ),
      );
    }
    replace(
      root,
      h(
        'div.mcp-page',
        null,
        tabBar(),
        tab === 'hub' ? hubBody : [h('div.mcp-toolbar', null, servers?.length ? addButton() : null), body],
      ),
    );
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
