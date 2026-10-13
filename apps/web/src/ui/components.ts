// Shared components. Views compose these; they own no state.

import { BRAND_ICONS } from './brands.gen';
import { h, icon, markSvg, pathSvg, replace } from './dom';
import type { IconName } from './icons.gen';

export type Tone = 'ok' | 'warn' | 'danger' | 'info' | 'accent' | 'muted';

export function badge(text: string, tone: Tone = 'muted', withDot = false): HTMLElement {
  return h('span.badge', { class: tone === 'muted' ? '' : tone }, withDot ? h('span.dot', { class: tone }) : null, text);
}

export const TRAINS_NOTE = 'Meta trains future models on everything sent to and returned by these models, so use them only for data you are happy to share.';

/** The warning on a contributor model; its title carries the explanation. */
export function trainsBadge(): HTMLElement {
  const el = badge('WARNING: trains on your data', 'warn');
  el.title = TRAINS_NOTE;
  return el;
}

export function dot(tone: Tone): HTMLElement {
  return h('span.dot', { class: tone });
}

type ButtonOpts = { kind?: 'primary' | 'danger' | 'ghost'; size?: 'sm'; icon?: IconName; title?: string; disabled?: boolean };

export function button(label: string, onClick: (ev: MouseEvent) => void, opts: ButtonOpts = {}): HTMLButtonElement {
  const classes = [opts.kind, opts.size, label ? '' : 'icon-only'].filter(Boolean).join(' ');
  const b = h('button.btn', { class: classes, type: 'button', title: opts.title ?? null, disabled: !!opts.disabled }, opts.icon ? icon(opts.icon) : null, label || null);
  b.addEventListener('click', onClick);
  return b;
}

export function card(title: string | Node | null, body: Node | Node[], opts: { actions?: Node[]; flush?: boolean } = {}): HTMLElement {
  return h(
    'section.card',
    null,
    title != null ? h('header.card-head', null, typeof title === 'string' ? h('h2', null, title) : title, ...(opts.actions ?? [])) : null,
    h('div.card-body', { class: opts.flush ? 'flush' : '' }, body),
  );
}

export function stat(label: string, value: string | number, sub?: string): HTMLElement {
  return h('div.card.stat', null, h('span.label', null, label), h('span.value', null, String(value)), sub ? h('span.sub', null, sub) : null);
}

export function empty(iconName: IconName, title: string, text?: string, action?: Node): HTMLElement {
  return h('div.empty', null, icon(iconName), h('h3', null, title), text ? h('p', null, text) : null, action ?? null);
}

export interface Column<T> {
  label: string;
  cell: (row: T) => Node | string | number | null;
  num?: boolean;
  width?: string;
}

/** `key` gives each row a stable `data-key`, so a redraw keeps its state when rows move. */
export function table<T>(columns: Column<T>[], rows: T[], onRow?: (row: T) => void, key?: (row: T) => string): HTMLTableElement {
  return h(
    'table.table',
    null,
    h('thead', null, h('tr', null, columns.map((c) => h('th', { class: c.num ? 'num' : '', style: c.width ? `width:${c.width}` : null }, c.label)))),
    h(
      'tbody',
      null,
      rows.map((row) => {
        const tr = h('tr', { class: onRow ? 'clickable' : '', 'data-key': key?.(row) }, columns.map((c) => h('td', { class: c.num ? 'num' : '' }, c.cell(row))));
        if (onRow) tr.addEventListener('click', () => onRow(row));
        return tr;
      }),
    ),
  );
}

export function field(label: string, control: HTMLElement, hint?: string): HTMLLabelElement {
  return h('label.field', null, h('span', null, label), control, hint ? h('span.hint', null, hint) : null);
}

export function input(attrs: Record<string, string | number | boolean | null | undefined> = {}): HTMLInputElement {
  return h('input.input', attrs);
}

export function textarea(attrs: Record<string, string | number | boolean | null | undefined> = {}): HTMLTextAreaElement {
  return h('textarea.textarea', attrs);
}

export function select(options: { value: string; label: string }[], value?: string): HTMLSelectElement {
  const s = h('select.select', null, options.map((o) => h('option', { value: o.value }, o.label)));
  if (value !== undefined) s.value = value;
  return s;
}

/** A tab strip; `onSelect` gets the chosen key. */
export function tabs(items: { key: string; label: string }[], current: string, onSelect: (key: string) => void): HTMLElement {
  return h(
    'div.tabs',
    { role: 'tablist' },
    items.map((t) => {
      const b = h('button', { role: 'tab', 'aria-selected': t.key === current ? 'true' : 'false' }, t.label);
      b.addEventListener('click', () => onSelect(t.key));
      return b;
    }),
  );
}

/** A modal dialog; returns a close function. */
export function modal(title: string, body: Node, onClose?: () => void): () => void {
  const close = () => {
    backdrop.remove();
    document.removeEventListener('keydown', onKey);
    onClose?.();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') close();
  };
  const backdrop = h(
    'div.modal-backdrop',
    null,
    h('div.modal', { role: 'dialog', 'aria-modal': 'true', 'aria-label': title }, h('header.card-head', null, h('h2', null, title), button('', close, { kind: 'ghost', icon: 'x', title: 'Close' })), h('div.card-body', null, body)),
  );
  backdrop.addEventListener('mousedown', (e) => {
    if (e.target === backdrop) close();
  });
  document.addEventListener('keydown', onKey);
  document.body.appendChild(backdrop);
  backdrop.querySelector<HTMLElement>('input, textarea, select, button.primary')?.focus();
  return close;
}

export function toast(text: string, kind: 'info' | 'error' = 'info'): void {
  let host = document.querySelector('.toast-host');
  if (!host) {
    host = h('div.toast-host', { 'aria-live': 'polite' });
    document.body.appendChild(host);
  }
  const t = h('div.toast', { class: kind === 'error' ? 'error' : '' }, text);
  host.appendChild(t);
  setTimeout(() => t.remove(), kind === 'error' ? 6000 : 3000);
}

/** Runs `action`, toasting its error; resolves to its value or undefined. */
export async function attempt<T>(action: () => Promise<T>, success?: string): Promise<T | undefined> {
  try {
    const value = await action();
    if (success) toast(success);
    return value;
  } catch (err) {
    toast(err instanceof Error ? err.message : String(err), 'error');
    return undefined;
  }
}

// ---- Brand logos -----------------------------------------------------------

const MONOGRAMS: Record<string, string> = {
  groq: 'Gq', together: 'Tg', fireworks: 'Fw', cerebras: 'Cb', llamacpp: 'll', mock: 'M',
};

/** A provider's logo tile: the brand mark in its colour, or a monogram. */
export function brandLogo(brand: string, large = false): HTMLElement {
  const tile = h('span.logo', { class: large ? 'lg' : '', title: brand });
  const icon = BRAND_ICONS[brand];
  if (icon?.paths) {
    // A multi-colour mark keeps each path's own fill: never tinted or recoloured.
    tile.appendChild(markSvg(icon.paths));
  } else if (icon?.path) {
    tile.appendChild(pathSvg(icon.path));
    // Very dark brand colours vanish on a dark tile; keep those white.
    const hex = icon.hex.toLowerCase();
    const dark = parseInt(hex.slice(0, 2), 16) + parseInt(hex.slice(2, 4), 16) + parseInt(hex.slice(4, 6), 16) < 150;
    tile.style.color = dark ? 'var(--text)' : `#${hex}`;
  } else {
    replace(tile, h('span.mono-mark', null, MONOGRAMS[brand] ?? brand.slice(0, 2).toUpperCase()));
  }
  return tile;
}

// ---- Formatting ------------------------------------------------------------

export function ago(ms: number | null | undefined, now = Date.now()): string {
  if (!ms) return '—';
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 5) return 'just now';
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

export function compact(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

export function clip(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}
