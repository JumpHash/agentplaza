// The MCP logs drawer: the last lines a server wrote to stdout and stderr, newest at the bottom.
// The hub redacts secrets before they reach this page. Opened from the MCP list (mcp.ts).
import '../styles/mcp-logs.css';
import { mcpApi, type McpLogLine, type McpServerView, type McpStatus } from '../api';
import { badge, button, empty } from '../ui/components';
import { h, replace } from '../ui/dom';
import { on } from '../store';
import { STATUS_LABEL, STATUS_TONE } from './mcp-common';
import { errorText } from './vault-common';

const REFRESH_MS = 5000;
const LINE_LIMIT = 200;

const pad = (n: number, width = 2): string => String(n).padStart(width, '0');

/** Local time as HH:MM:SS.mmm. */
function clock(ts: number): string {
  const d = new Date(ts);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`;
}

/** Opens the logs drawer for a server. Returns the close function (also run by X and Escape). */
export function openLogsDrawer(server: McpServerView): () => void {
  let status: McpStatus = server.status;
  let closed = false;
  let timer: ReturnType<typeof setInterval> | undefined;
  let seq = 0;

  const statusBadge = (): HTMLElement => badge(STATUS_LABEL[status], STATUS_TONE[status], true);
  const titleBadge = h('span.mcp-logs-status', null, statusBadge());
  const bodyEl = h('div.mcp-logs-body', { role: 'log', 'aria-live': 'off', tabindex: '0' }, h('p.mcp-logs-note', null, 'Loading'));
  const refreshBtn = button('', () => void refresh(), { kind: 'ghost', icon: 'refresh', title: 'Refresh' });
  const closeBtn = button('', () => close(), { kind: 'ghost', icon: 'x', title: 'Close' });

  const drawer = h(
    'aside.mcp-logs',
    { role: 'dialog', 'aria-modal': 'false', 'aria-label': `Logs for ${server.name}`, tabindex: '-1' },
    h(
      'header.mcp-logs-head',
      null,
      h('div.mcp-logs-title', null, h('h2', null, server.name), titleBadge),
      h('div.mcp-logs-actions', null, refreshBtn, closeBtn),
    ),
    bodyEl,
  );

  function renderLines(lines: McpLogLine[]): void {
    if (lines.length === 0) {
      replace(bodyEl, empty('terminal', 'No output yet'));
      return;
    }
    const rows = lines.map((l) =>
      h(
        l.stream === 'stderr' ? 'div.mcp-log-line.is-err' : 'div.mcp-log-line',
        null,
        h('span.mcp-log-time', null, clock(l.ts_ms)),
        badge(l.stream, l.stream === 'stderr' ? 'danger' : 'muted'),
        h('span.mcp-log-text', null, l.line),
      ),
    );
    replace(bodyEl, ...rows);
    // Newest at the bottom.
    bodyEl.scrollTop = bodyEl.scrollHeight;
  }

  function renderError(err: unknown): void {
    replace(bodyEl, h('p.mcp-logs-note.is-danger', null, errorText(err)));
  }

  async function refresh(): Promise<void> {
    const mine = ++seq;
    refreshBtn.disabled = true;
    try {
      const lines = await mcpApi.serverLogs(server.id, LINE_LIMIT);
      if (closed || mine !== seq) return;
      renderLines(lines);
    } catch (err) {
      if (closed || mine !== seq) return;
      renderError(err);
    } finally {
      if (!closed) refreshBtn.disabled = false;
    }
  }

  function setStatus(next: McpStatus): void {
    status = next;
    replace(titleBadge, statusBadge());
  }

  const onStatus = (data: unknown): void => {
    const ev = data as { id?: unknown; status?: unknown } | null;
    if (!ev || ev.id !== server.id) return;
    if (typeof ev.status === 'string' && ev.status in STATUS_LABEL) setStatus(ev.status as McpStatus);
    void refresh();
  };

  const unsubscribe = on('mcp.status', onStatus);

  const onKey = (e: KeyboardEvent): void => {
    // A modal opened over the drawer takes Escape first.
    if (e.key === 'Escape' && !document.querySelector('.modal-backdrop')) close();
  };

  function close(): void {
    if (closed) return;
    closed = true;
    if (timer !== undefined) clearInterval(timer);
    unsubscribe();
    document.removeEventListener('keydown', onKey);
    drawer.remove();
  }

  document.addEventListener('keydown', onKey);
  document.body.appendChild(drawer);
  drawer.focus();
  timer = setInterval(() => void refresh(), REFRESH_MS);
  void refresh();

  return close;
}
