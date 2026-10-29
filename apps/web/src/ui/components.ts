// Shared components. Views compose these; they own no state. The markup is the Loom and Graphite consoles' own
// (styles/proto/base.css): chips, cards with a card-h head and a card-b body, the sheet dialog, the kit toasts.

import { BRAND_ICONS } from './brands.gen';
import { h, icon, lu, markSvg, pathSvg, replace } from './dom';
import { ICONS, type IconName, type LucideName } from './icons.gen';

export type Tone = 'ok' | 'warn' | 'danger' | 'info' | 'accent' | 'muted';

export function badge(text: string, tone: Tone = 'muted', withDot = false): HTMLElement {
  return h('span.chip', { class: `t-${tone}` }, withDot ? h('i.dot') : null, text);
}

export const TRAINS_NOTE = 'Meta trains future models on everything sent to and returned by these models, so use them only for data you are happy to share.';

/** The warning on a contributor model; its title carries the explanation. */
export function trainsBadge(): HTMLElement {
  const el = badge('WARNING: trains on your data', 'warn');
  el.title = TRAINS_NOTE;
  return el;
}

export function dot(tone: Tone): HTMLElement {
  return h('i.dot', { class: `t-${tone}` });
}

/** An icon by the console's name or by its lucide name (`shield-check`), as the prototype names them. */
export type AnyIcon = IconName | LucideName;
function anyIcon(name: AnyIcon, size: number): SVGSVGElement {
  return Object.prototype.hasOwnProperty.call(ICONS, name) ? icon(name as IconName, '', size) : lu(name as LucideName, size);
}

type ButtonOpts = { kind?: 'primary' | 'danger' | 'ghost'; size?: 'sm'; icon?: AnyIcon; title?: string; disabled?: boolean };

export function button(label: string, onClick: (ev: MouseEvent) => void, opts: ButtonOpts = {}): HTMLButtonElement {
  const classes = [opts.kind, opts.size, label ? '' : 'icon-only'].filter(Boolean).join(' ');
  const b = h(
    'button.btn',
    { class: classes, type: 'button', title: opts.title ?? null, 'aria-label': label ? null : (opts.title ?? null), disabled: !!opts.disabled },
    opts.icon ? anyIcon(opts.icon, opts.size === 'sm' ? 14 : 15) : null,
    label ? h('span', null, label) : null,
  );
  b.addEventListener('click', onClick);
  return b;
}

export function card(title: string | Node | null, body: Node | Node[], opts: { actions?: Node[]; flush?: boolean } = {}): HTMLElement {
  return h(
    'section.card',
    { class: opts.flush ? 'flush' : '' },
    title != null ? h('header.card-h', null, h('h3', null, title), h('span.sp'), ...(opts.actions ?? [])) : null,
    h('div.card-b', null, body),
  );
}

/** A stat value is a figure unless it has letters: a model id or a name is text, and takes the smaller text size. */
export function isTextValue(value: string | number): boolean {
  return typeof value === 'string' && /\p{L}/u.test(value);
}

export function stat(label: string, value: string | number, sub?: string): HTMLElement {
  const text = isTextValue(value);
  const shown = String(value);
  return h(
    'div.stat',
    null,
    h('span.stat-l', null, label),
    h(text ? 'b.stat-v.value-text' : 'b.stat-v.num', text ? { title: shown } : null, shown),
    sub ? h('small', null, sub) : null,
  );
}

export function empty(iconName: AnyIcon, title: string, text?: string, action?: Node): HTMLElement {
  return h('div.empty', null, anyIcon(iconName, 22), h('b', null, title), text ? h('span', null, text) : null, action ?? null);
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
    'table.tbl',
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
  return h('label.field', null, h('span.field-l', null, label), control, hint ? h('small.hint', null, hint) : null);
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
      const b = h('button.tab', { type: 'button', role: 'tab', 'aria-selected': t.key === current ? 'true' : 'false' }, t.label);
      b.addEventListener('click', () => onSelect(t.key));
      return b;
    }),
  );
}

/** A modal dialog (the sheet over a scrim), with its buttons in the sheet's foot when `foot` is given; returns a
 * close function. */
export function modal(title: string, body: Node, onClose?: () => void, foot?: Node[]): () => void {
  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    scrim.classList.add('out');
    setTimeout(() => scrim.remove(), 170);
    document.removeEventListener('keydown', onKey);
    onClose?.();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape') close();
  };
  const shut = h('button.iconbtn', { type: 'button', title: 'Close', 'aria-label': 'Close' }, icon('x'));
  shut.addEventListener('click', close);
  const scrim = h(
    'div.scrim',
    null,
    h(
      'div.sheet',
      { role: 'dialog', 'aria-modal': 'true', 'aria-label': title },
      h('header.sheet-h', null, h('h2', null, title), shut),
      h('div.sheet-b', null, body),
      foot?.length ? h('footer.sheet-f', null, foot) : null,
    ),
  );
  scrim.addEventListener('mousedown', (e) => {
    if (e.target === scrim) close();
  });
  document.addEventListener('keydown', onKey);
  document.body.appendChild(scrim);
  scrim.querySelector<HTMLElement>('input:not([type=checkbox]), textarea, select, button.primary')?.focus();
  return close;
}

/** A toast at the foot of the window: a tick for news, a cross for an error. At most four show at once. */
export function toast(text: string, kind: 'info' | 'error' = 'info'): void {
  let host = document.querySelector('.kit-toasts');
  if (!host) {
    host = h('div.kit-toasts', { 'aria-live': 'polite' });
    document.body.appendChild(host);
  }
  const err = kind === 'error';
  const dur = Math.min(6000, 2400 + text.length * 28);
  const t = h('div.kit-toast', { class: err ? 'err' : '', role: err ? 'alert' : 'status', style: `--kt-dur:${dur}ms` }, h('span.kt-i', null, lu(err ? 'x' : 'check', 13, '', 2.6)), h('span.kt-t', null, text));
  host.appendChild(t);
  while (host.children.length > 4) host.firstElementChild?.remove();
  setTimeout(() => {
    t.classList.add('out');
    setTimeout(() => t.remove(), 230);
  }, dur);
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

/** A project's monogram tile (the Loom and Graphite consoles' `pmono`): two letters on one of the six hues, picked by
 * the id's digits. */
export function pmono(p: { id: string; title: string }, size = 18): HTMLElement {
  const words = (p.title || p.id || '?').replace(/[^A-Za-z0-9 ]/g, '').split(/\s+/).filter(Boolean);
  const m = (((words[0] ?? '?')[0] ?? '?') + ((words[1] ?? words[0] ?? '?')[0] ?? '')).toUpperCase();
  const hue = Number((p.id || '').replace(/\D/g, '') || 0) % 6;
  return h('span.pmono', { class: `h${hue}`, style: `--s:${size}px`, 'aria-hidden': 'true' }, m);
}

// ---- Brand logos -----------------------------------------------------------

/** The model's vendor, even when its connection has a custom local instance name. */
export function modelBrand(model: string): string {
  const base = model.split('@')[0] ?? model;
  const parts = base.split('/');
  const name = (parts.at(-1) ?? '').toLowerCase();
  if (/^(gpt-|o[134])/.test(name)) return 'openai';
  if (name.startsWith('deepseek')) return 'deepseek';
  if (name.startsWith('glm-')) return 'zai';
  if (name.startsWith('claude')) return 'claude';
  if (name.startsWith('gemini')) return 'google';
  return parts.reverse().find((p) => p in BRAND_ICONS) ?? parts.at(-1) ?? '';
}

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
    // Very dark brand colours vanish on a dark tile; keep those in ink. Dim ones (a deep purple) keep their colour on
    // the light palette and take ink on the dark one (`.logo.dim` in proto/shell.css).
    const hex = icon.hex.toLowerCase();
    const ch = (i: number): number => parseInt(hex.slice(i, i + 2), 16) / 255;
    const [r, g, b] = [ch(0), ch(2), ch(4)];
    const lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    tile.style.color = lum < 0.12 && r + g + b < 0.6 ? 'var(--ink)' : `#${hex}`;
    if (lum < 0.22) tile.classList.add('dim');
  } else {
    replace(tile, h('span.mono-mark', null, MONOGRAMS[brand] ?? brand.slice(0, 2).toUpperCase()));
  }
  return tile;
}

// ---- Formatting ------------------------------------------------------------

/** Relative time as the Loom and Graphite consoles say it: "just now", "4 min ago", "3 h ago", "2 d ago". */
export function ago(ms: number | null | undefined, now = Date.now()): string {
  if (!ms) return '—';
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return 'just now';
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}

export function compact(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  return `${(n / 1_000_000).toFixed(1)}M`;
}

export function clip(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}
