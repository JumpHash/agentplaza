// The prototype's markup helpers for the Settings screens (Workstations, E2B API, Volumes, Setup): the page head,
// the buttons, the key-value lists, the tables that scroll, the empty states and the checkboxes, emitted with the
// prototype's own classes. They take the data from their callers; they hold none.

import { h, lu } from '../ui/dom';
import type { LucideName } from '../ui/icons.gen';

/** The prototype's icon names, as the lucide icons they draw. */
const ICON = {
  alert: 'triangle-alert',
  clock: 'clock',
  cpu: 'cpu',
  file: 'file',
  key: 'key',
  link: 'link',
  machine: 'server',
  play: 'play',
  plus: 'plus',
  square: 'square',
  stop: 'square',
  trash: 'trash',
  wrench: 'wrench',
  x: 'x',
  harness: 'cpu',
} satisfies Record<string, LucideName>;

export type IconKey = keyof typeof ICON;

export function ic(key: IconKey, size = 16): SVGSVGElement {
  return lu(ICON[key], size);
}

/** The relative time of the prototype: "just now", "4 min ago", "3 h ago", "2 d ago". A missing time is a dash. */
export function ago(ms: number | null | undefined, now = Date.now()): string {
  if (!ms) return '—';
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return 'just now';
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}

export interface BtnOpts {
  kind?: 'primary' | 'ghost' | 'danger';
  sm?: boolean;
  icon?: IconKey;
  disabled?: boolean;
  title?: string;
}

/** A button: `btn('Label', onClick, {kind, sm, icon})`, with the prototype's classes. */
export function btn(label: string, onClick: (ev: MouseEvent) => void, o: BtnOpts = {}): HTMLButtonElement {
  const classes = ['btn', o.kind, o.sm ? 'sm' : '', label ? '' : 'icon-only'].filter(Boolean).join(' ');
  const b = h(
    'button',
    { class: classes, type: 'button', title: o.title ?? null, 'aria-label': label ? null : (o.title ?? null), disabled: o.disabled ?? null },
    o.icon ? ic(o.icon, o.sm ? 14 : 15) : null,
    label ? h('span', null, label) : null,
  );
  b.addEventListener('click', onClick);
  return b;
}

/** The page head: the title left, the actions right, and the lede under them when there is one. */
export function head(title: string, actions: Node[] = [], sub?: string): HTMLElement {
  return h(
    'header.page-head',
    null,
    h('div.page-head-row', null, h('h1', null, title), h('span.sp'), h('div.page-actions', null, ...actions)),
    sub ? h('p.lede', null, sub) : null,
  );
}

/** A definition list of label and value pairs. */
export function kv(pairs: [string, Node | string | Node[]][]): HTMLElement {
  return h('dl.kv', null, pairs.map(([k, v]) => h('div', null, h('dt', null, k), h('dd', null, v))));
}

export interface PCol<T> {
  label: string;
  cls?: string;
  cell: (row: T) => Node | (Node | string)[] | string;
}

/** A table in its wrapper: the columns, and a row per item. `cls: 'num'` right-aligns a column. */
export function ptable<T>(columns: PCol<T>[], rows: T[]): HTMLElement {
  return h(
    'div.tbl-wrap',
    null,
    h(
      'table.tbl',
      null,
      h('thead', null, h('tr', null, columns.map((c) => h('th', { class: c.cls ?? null }, c.label)))),
      h('tbody', null, rows.map((r) => h('tr', null, columns.map((c) => h('td', { class: c.cls ?? null }, c.cell(r)))))),
    ),
  );
}

/** An empty state: the icon, the title, and the text under it when there is one. */
export function empty(icon: IconKey, title: string, text = ''): HTMLElement {
  return h('div.empty', null, ic(icon, 22), h('b', null, title), text ? h('span', null, text) : null);
}

/** A checkbox with its label, the words after the box. */
export function check(box: HTMLInputElement, label: string): HTMLLabelElement {
  return h('label.check', null, box, ` ${label}`);
}

/** A line of muted text. */
export function muted(text: string): HTMLElement {
  return h('p.muted', null, text);
}

/**
 * A dialog as the prototype opens it: the title with a close button, the body, and a footer with the buttons. One
 * dialog at a time; Escape and a click on the dimmed backdrop close it. Returns its close function.
 */
export function dialog(title: string, body: Node | Node[], foot: Node[] = [], onClose?: () => void): () => void {
  document.querySelectorAll('.scrim').forEach((x) => x.remove());
  let closed = false;
  const close = (): void => {
    if (closed) return;
    closed = true;
    scrim.classList.add('out');
    setTimeout(() => scrim.remove(), 170);
    document.removeEventListener('keydown', onKey);
    onClose?.();
  };
  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape') close();
  };
  const shut = h('button.iconbtn', { type: 'button', title: 'Close', 'aria-label': 'Close' }, lu('x', 16));
  shut.addEventListener('click', close);
  const scrim: HTMLElement = h(
    'div.scrim',
    null,
    h(
      'div.sheet',
      { role: 'dialog', 'aria-modal': 'true', 'aria-label': title },
      h('header.sheet-h', null, h('h2', null, title), shut),
      h('div.sheet-b', null, body),
      foot.length ? h('footer.sheet-f', null, ...foot) : null,
    ),
  );
  scrim.addEventListener('mousedown', (e) => {
    if (e.target === scrim) close();
  });
  document.addEventListener('keydown', onKey);
  document.body.append(scrim);
  scrim.querySelector<HTMLElement>('input:not([type=checkbox]), textarea, select')?.focus();
  return close;
}
