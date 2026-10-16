// Overlays of the Metrics view: the panel and sibling menus, dialogs, the find palette, the keyboard help, the
// "what changed" drawer and toasts. One overlay at a time; Esc closes it and focus returns where it was.

import { button } from '../ui/components';
import { h } from '../ui/dom';
import { spark } from './figures';
import type { Around, EventRow, MoverRow, Status, Tone } from './model';

export interface MenuItem {
  label: string;
  key?: string;
  run(): void;
}

/** Closes an open overlay; `restoreFocus` false leaves focus where the user put it. */
type Close = (restoreFocus?: boolean) => void;

/** The overlay that is open now, if any. Opening another one closes it first. */
let current: Close | null = null;

/**
 * Mounts `root` as the only open overlay. It remembers the focused element and gives focus back to it on close,
 * sends every keydown to `onKey` until then, and runs `onClose` once its element is removed.
 */
function mount(root: HTMLElement, onKey: (e: KeyboardEvent) => void, onClose?: () => void): Close {
  closeOverlay();
  const returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  let closed = false;
  const close: Close = (restoreFocus = true) => {
    if (closed) return;
    closed = true;
    document.removeEventListener('keydown', onKey);
    root.remove();
    onClose?.();
    if (current === close) current = null;
    if (restoreFocus && returnTo?.isConnected) returnTo.focus();
  };
  current = close;
  document.addEventListener('keydown', onKey);
  document.body.appendChild(root);
  return close;
}

/** Keeps Tab inside `root`: focus wraps from the last field to the first, and back again. */
function trapTab(e: KeyboardEvent, root: HTMLElement): void {
  if (e.key !== 'Tab') return;
  const fields = [...root.querySelectorAll<HTMLElement>('input, textarea, select, button')].filter((el) => !(el as HTMLButtonElement).disabled);
  const first = fields[0];
  const last = fields[fields.length - 1];
  if (!first || !last) {
    e.preventDefault();
    return;
  }
  const active = document.activeElement;
  if (e.shiftKey && (active === first || !root.contains(active))) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && (active === last || !root.contains(active))) {
    e.preventDefault();
    first.focus();
  }
}

/** Places `panel` under `anchor`, right-aligned to it, and pulls it back inside the viewport. */
function placeMenu(panel: HTMLElement, anchor: HTMLElement): void {
  const r = anchor.getBoundingClientRect();
  const left = Math.min(r.right - panel.offsetWidth, window.innerWidth - panel.offsetWidth - 8);
  const top = Math.min(r.bottom + 4, window.innerHeight - panel.offsetHeight - 8);
  panel.style.position = 'fixed';
  panel.style.left = `${Math.max(8, left)}px`;
  panel.style.top = `${Math.max(8, top)}px`;
}

/** A menu under `anchor`; closes on a choice, Esc or a click outside. */
export function menu(anchor: HTMLElement, items: MenuItem[]): void {
  const panel = h(
    'div.mx-menu',
    { role: 'menu' },
    items.map((item) => {
      const choice = h('button', { type: 'button', role: 'menuitem' }, h('span', null, item.label), item.key ? h('kbd', null, item.key) : null);
      choice.addEventListener('click', () => {
        close();
        item.run();
      });
      return choice;
    }),
  );
  const buttons = [...panel.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
  const outside = (e: PointerEvent) => {
    const target = e.target as Node;
    if (!panel.contains(target) && !anchor.contains(target)) close();
  };
  const close = mount(
    panel,
    (e) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        close();
      } else if (e.key === 'Tab') {
        close(false);
      } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const step = e.key === 'ArrowDown' ? 1 : -1;
        const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
        const next = at < 0 ? (step > 0 ? 0 : buttons.length - 1) : (at + step + buttons.length) % buttons.length;
        buttons[next]?.focus();
      }
    },
    () => {
      anchor.setAttribute('aria-expanded', 'false');
      document.removeEventListener('pointerdown', outside, true);
    },
  );
  anchor.setAttribute('aria-expanded', 'true');
  document.addEventListener('pointerdown', outside, true);
  placeMenu(panel, anchor);
  buttons[0]?.focus();
}

export interface DialogAction {
  label: string;
  primary?: boolean;
  /** Return false to keep the dialog open (a validation failed). */
  run?(dialog: HTMLElement): boolean | void;
}

/** A modal dialog; returns a function that closes it. */
export function dialog(title: string, body: HTMLElement, actions: DialogAction[]): () => void {
  const modal = h(
    'div.mx-modal',
    { role: 'dialog', 'aria-modal': 'true', 'aria-label': title },
    h('h2', null, title),
    body,
    h(
      'div.mx-acts',
      null,
      actions.map((action) =>
        button(
          action.label,
          () => {
            if (action.run?.(modal) === false) return;
            close();
          },
          { kind: action.primary ? 'primary' : undefined },
        ),
      ),
    ),
  );
  const scrim = h('div.mx-scrim', null, modal);
  scrim.addEventListener('mousedown', (e) => {
    if (e.target === scrim) close();
  });
  const close = mount(scrim, (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    } else {
      trapTab(e, modal);
    }
  });
  (modal.querySelector<HTMLElement>('input, textarea, select') ?? modal.querySelector<HTMLElement>('.mx-acts .primary') ?? modal.querySelector<HTMLElement>('button'))?.focus();
  return close;
}

export interface PaletteItem {
  group: string;
  label: string;
  run(): void;
}

/** The find palette: a filter box over `items`, ↑ ↓ to move, Enter to run, Esc to close. */
export function palette(items: PaletteItem[], placeholder: string): void {
  const input = h('input', { type: 'text', placeholder, 'aria-label': 'Search', autocomplete: 'off' });
  const list = h('ul', { role: 'listbox' });
  const panel = h(
    'div.mx-pal',
    { role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Find' },
    input,
    list,
    h(
      'div.mx-foot',
      null,
      h('span', null, h('kbd', null, '↑'), h('kbd', null, '↓'), ' move'),
      h('span', null, h('kbd', null, 'Enter'), ' open'),
      h('span', null, h('kbd', null, 'Esc'), ' close'),
    ),
  );
  const scrim = h('div.mx-scrim', null, panel);
  let shown: PaletteItem[] = [];
  let selected = 0;

  const choose = (index: number) => {
    const item = shown[index];
    if (!item) return;
    close();
    item.run();
  };
  const draw = () => {
    const query = input.value.trim().toLowerCase();
    const matches = query ? items.filter((item) => `${item.label} ${item.group}`.toLowerCase().includes(query)) : items;
    shown = matches.slice(0, 40);
    selected = Math.max(0, Math.min(selected, shown.length - 1));
    list.replaceChildren(
      ...(shown.length
        ? shown.map((item, i) => {
            const option = h('li', { role: 'option', 'aria-selected': i === selected ? 'true' : 'false' }, h('span.mx-mute', null, item.group), h('span', null, item.label));
            option.addEventListener('click', () => choose(i));
            return option;
          })
        : [h('li.mx-mute', null, 'Nothing matches.')]),
    );
    list.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
  };

  input.addEventListener('input', () => {
    selected = 0;
    draw();
  });
  const close = mount(scrim, (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      selected = Math.min(shown.length - 1, selected + 1);
      draw();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      selected = Math.max(0, selected - 1);
      draw();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      choose(selected);
    } else {
      trapTab(e, panel);
    }
  });
  scrim.addEventListener('mousedown', (e) => {
    if (e.target === scrim) close();
  });
  draw();
  input.focus();
}

/** The keyboard and mouse help. */
export function help(): void {
  const keys: [string, string][] = [
    ['/', 'Find a scope, tab or action'],
    ['Ctrl K', 'Same as /'],
    ['1 – 9', 'Switch tab'],
    ['[ and ]', 'Earlier / later period'],
    ['l', 'Live on / off'],
    ['z', 'Reset zoom'],
    ['f', 'Pin this scope'],
    ['a', 'Annotations on / off'],
    ['c', 'Compare with another scope'],
    ['d', 'Compact density'],
    ['t', 'Theme'],
    ['n', 'New alert rule'],
    ['?', 'This help'],
    ['Esc', 'Close, exit full screen'],
    ['drag on a chart', 'Zoom every chart to that span'],
    ['click a point', 'What changed around then'],
  ];
  dialog(
    'Keyboard and mouse',
    h(
      'div.mx-help',
      null,
      keys.map(([key, text]) => h('div', null, h('span', null, text), h('kbd', null, key))),
    ),
    [{ label: 'Close', primary: true }],
  );
}

export interface DrawerCtx {
  go(target: { scope?: string; tab?: string }): void;
  zoom(z: [number, number]): void;
  traces(): void;
}

/** The chart tone of a mover's status; a muted status draws in the ink tone. */
function sparkTone(status: Status): Tone {
  return status === 'mute' ? 'ink-3' : status;
}

/** "What changed around then": events, the metrics that moved most, kept traces, a zoom button. */
export function drawer(a: Around, ctx: DrawerCtx): void {
  const event = (row: EventRow) =>
    h(
      'div.mx-ev',
      null,
      h('span.mx-when', null, row.time),
      h('span', { class: `mx-${row.status}` }, '●'),
      h('span', null, row.subject, row.detail ? h('span.mx-hint', null, row.detail) : null),
    );

  const mover = (row: MoverRow) => {
    const moverButton = h(
      'button.mx-mv',
      { type: 'button' },
      h('span', null, row.name),
      h('b', { class: `mx-${row.status} num` }, row.change),
      spark(row.spark, sparkTone(row.status), 90, 20),
    );
    moverButton.addEventListener('click', () => {
      close();
      ctx.go({ tab: row.tab, scope: row.scope });
    });
    return moverButton;
  };

  const panel = h(
    'aside.mx-drawer',
    { 'aria-label': 'What changed' },
    h('div.mx-drawer-head', null, h('h2', null, `Around ${a.label}`), button('Close', () => close())),
    h('p.mx-sub', null, a.sub),
    h('h4', null, 'Events'),
    a.events.map(event),
    h('h4', null, 'Moved the most'),
    a.movers.map(mover),
    h('h4', null, 'Traces kept in this window'),
    h(
      'div.mx-ev',
      null,
      h('span', null, `${a.traces.total} turns · ${a.traces.errors} errors · ${a.traces.slow} slow`),
      button('Open traces', () => {
        close();
        ctx.traces();
      }),
    ),
    h(
      'div.mx-acts',
      null,
      button(
        'Zoom to ± 30 min',
        () => {
          close();
          ctx.zoom(a.zoom);
        },
        { kind: 'primary' },
      ),
    ),
  );
  const close = mount(panel, (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    }
  });
  panel.querySelector<HTMLElement>('.mx-drawer-head .btn')?.focus();
}

/** Closes whatever overlay is open; true when one was. */
export function closeOverlay(): boolean {
  if (!current) return false;
  current();
  return true;
}

/** Whether an overlay is open. */
export function overlayOpen(): boolean {
  return document.querySelector('.mx-scrim, .mx-menu, .mx-drawer') !== null;
}
