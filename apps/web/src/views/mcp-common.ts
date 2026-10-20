// Shared by the MCP screens (mcp.ts, mcp-hub.ts, mcp-logs.ts, mcp-wizard*.ts) and the Git platforms screen: the pure
// rules, and the prototype's markup helpers (btn, iconBtn, pageHead, kv, seg, toggle, check, emptyBox, tbl, dialog).
import type { McpPick, McpPreset, McpRisk, McpServerConfig, McpServerView, McpSetting, McpSpec, McpStatus, McpTransport } from '../api';
import { h, lu } from '../ui/dom';
import type { LucideName } from '../ui/icons.gen';

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

// ---- Markup the prototype's screens use (core.js h.btn, h.iconBtn, h.head, h.kv, h.seg, h.toggle, h.check, h.empty,
// h.table, the sheet dialog). The console's components do not emit these exact shapes, so the Settings screens use them.

export type BtnKind = 'primary' | 'ghost' | 'danger';
export interface BtnOpts {
  kind?: BtnKind;
  sm?: boolean;
  icon?: LucideName;
  title?: string;
  disabled?: boolean;
}

/** A button as the prototype draws it: `button.btn(.primary|.ghost|.danger)(.sm)` with a lucide icon and a span. */
export function btn(label: string, onClick: (ev: MouseEvent) => void, o: BtnOpts = {}): HTMLButtonElement {
  const classes = [o.kind, o.sm ? 'sm' : '', label ? '' : 'icon-only'].filter(Boolean).join(' ');
  const b = h(
    'button.btn',
    { class: classes, type: 'button', title: o.title ?? null, 'aria-label': label ? null : (o.title ?? null), disabled: !!o.disabled },
    o.icon ? lu(o.icon, o.sm ? 14 : 15) : null,
    label ? h('span', null, label) : null,
  );
  b.addEventListener('click', onClick);
  return b;
}

/** An icon-only button (`button.iconbtn`), titled for the reader. */
export function iconBtn(name: LucideName, onClick: (ev: MouseEvent) => void, title: string, o: { pressed?: boolean; cls?: string } = {}): HTMLButtonElement {
  const b = h('button.iconbtn', { class: o.cls ?? '', type: 'button', title, 'aria-label': title, 'aria-pressed': o.pressed == null ? null : String(o.pressed) }, lu(name, 16));
  b.addEventListener('click', onClick);
  return b;
}

/** The page head: the title on the left, the actions on the right. */
export function pageHead(title: string, actions: Node[] = []): HTMLElement {
  return h('header.page-head', null, h('div.page-head-row', null, h('h1', null, title), h('span.sp'), h('div.page-actions', null, ...actions)));
}

/** A definition list of facts: `dl.kv`. */
export function kv(pairs: [string, Node | string][]): HTMLElement {
  return h('dl.kv', null, pairs.map(([k, v]) => h('div', null, h('dt', null, k), h('dd', null, v))));
}

/** A segmented choice: `div.seg`, the chosen option pressed. */
export function seg(options: [string, string][], current: string, onPick: (value: string) => void): HTMLElement {
  return h(
    'div.seg',
    { role: 'group' },
    options.map(([value, label]) => {
      const b = h('button', { type: 'button', 'aria-pressed': String(value === current) }, label);
      b.addEventListener('click', () => onPick(value));
      return b;
    }),
  );
}

/** A switch: `button.tgl` with role switch. */
export function toggle(on: boolean, onChange: (on: boolean) => void, label: string): HTMLButtonElement {
  const b = h('button.tgl', { type: 'button', role: 'switch', 'aria-checked': String(on), 'aria-label': label });
  b.addEventListener('click', () => onChange(b.getAttribute('aria-checked') !== 'true'));
  return b;
}

/** A labelled checkbox: `label.check` around the input and its text. */
export function check(label: string, o: { checked?: boolean; disabled?: boolean; id?: string; onChange?: (on: boolean) => void } = {}): HTMLLabelElement {
  const box = h('input', { type: 'checkbox', id: o.id ?? null, disabled: !!o.disabled }) as HTMLInputElement;
  box.checked = !!o.checked;
  box.addEventListener('change', () => o.onChange?.(box.checked));
  return h('label.check', null, box, h('span', null, label));
}

/** An empty state: `div.empty` with an icon, a bold title and a line of text. */
export function emptyBox(name: LucideName, title: string, text?: string, action?: Node): HTMLElement {
  return h('div.empty', null, lu(name, 22), h('b', null, title), text ? h('span', null, text) : null, action ?? null);
}

export interface TblCol<T> {
  label: string;
  cls?: string;
  cell: (row: T) => Node | string | null;
}

/** A plain table in the prototype's wrapper: `div.tbl-wrap > table.tbl`. */
export function tbl<T>(cols: TblCol<T>[], rows: T[], key?: (row: T) => string): HTMLElement {
  return h(
    'div.tbl-wrap',
    null,
    h(
      'table.tbl',
      null,
      h('thead', null, h('tr', null, cols.map((c) => h('th', { class: c.cls ?? null }, c.label)))),
      h(
        'tbody',
        null,
        rows.map((r) => h('tr', { 'data-key': key?.(r) ?? null }, cols.map((c) => h('td', { class: c.cls ?? null }, c.cell(r))))),
      ),
    ),
  );
}

export interface DialogOpts {
  cls?: string;
  onClose?: () => void;
}

/** A sheet over a scrim with a header, a body and a footer of buttons (the prototype's modal). Returns its close. */
export function dialog(title: string, body: Node | Node[], foot: Node[] = [], o: DialogOpts = {}): () => void {
  let closed = false;
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') close();
  };
  const close = (): void => {
    if (closed) return;
    closed = true;
    scrim.remove();
    document.removeEventListener('keydown', onKey);
    o.onClose?.();
  };
  const scrim = h(
    'div.scrim',
    null,
    h(
      'div.sheet',
      { class: o.cls ?? null, role: 'dialog', 'aria-modal': 'true', 'aria-label': title },
      h('header.sheet-h', null, h('h2', null, title), iconBtn('x', close, 'Close')),
      h('div.sheet-b', null, body),
      foot.length ? h('footer.sheet-f', null, foot) : null,
    ),
  );
  scrim.addEventListener('mousedown', (e) => {
    if (e.target === scrim) close();
  });
  document.addEventListener('keydown', onKey);
  document.body.append(scrim);
  setTimeout(() => scrim.querySelector<HTMLElement>('input:not([type=checkbox]), textarea, select')?.focus(), 0);
  return close;
}
