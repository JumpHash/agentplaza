// Overlays of the Metrics view: the panel and sibling menus, dialogs, the find palette, the keyboard help and the
// "what changed" drawer. One overlay at a time; Esc closes it and focus returns where it was.

import { button } from '../ui/components';
import { h, icon } from '../ui/dom';
import { spark } from './figures';
import type { Around, EventRow, MoverRow, Status } from './model';

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
    'div.mt-menu',
    { role: 'menu' },
    items.map((item) => {
      const choice = h('button.mt-mi', { type: 'button', role: 'menuitem' }, item.label, item.key ? h('span.mt-hint', null, item.key) : null);
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

/**
 * The prototype's sheet: a dialog with its title and a close button, a body, and a footer of actions when there are
 * any. `shut` runs when the close button is clicked.
 */
function sheet(title: string, body: Node, foot: Node[], shut: () => void, cls?: string): HTMLElement {
  const close = h('button.iconbtn', { type: 'button', title: 'Close', 'aria-label': 'Close' }, icon('x'));
  close.addEventListener('click', shut);
  return h(
    'div.sheet',
    { role: 'dialog', 'aria-modal': 'true', 'aria-label': title, class: cls ?? null },
    h('header.sheet-h', null, h('h2', null, title), close),
    h('div.sheet-b', null, body),
    foot.length ? h('footer.sheet-f', null, foot) : null,
  );
}

/** A modal dialog; returns a function that closes it. */
export function dialog(title: string, body: HTMLElement, actions: DialogAction[]): () => void {
  const modal = sheet(
    title,
    body,
    actions.map((action) =>
      button(
        action.label,
        () => {
          if (action.run?.(modal) === false) return;
          close();
        },
        { kind: action.primary ? 'primary' : 'ghost' },
      ),
    ),
    () => close(),
  );
  const scrim = h('div.scrim', null, modal);
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
  (modal.querySelector<HTMLElement>('input, textarea, select') ?? modal.querySelector<HTMLElement>('.sheet-f .primary') ?? modal.querySelector<HTMLElement>('button'))?.focus();
  return close;
}

export interface PaletteItem {
  group: string;
  label: string;
  run(): void;
}

/**
 * The find palette: a filter box over `items` under group headings, ↑ ↓ to move, Enter to run, Esc to close.
 * `title` names the dialog ("Find" by default).
 */
export function palette(items: PaletteItem[], placeholder: string, title = 'Find'): void {
  const input = h('input.input.mt-find-q', { type: 'search', placeholder, 'aria-label': placeholder, autocomplete: 'off', 'data-input': 'mt-find-q' });
  const list = h('div.mt-list', { id: 'mt-find-list', role: 'listbox' });
  const body = h('div.mt-form', null, input, list);
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
    if (!shown.length) {
      list.replaceChildren(h('div.mt-empty', null, 'Nothing matches.'));
      return;
    }
    const rows: Node[] = [];
    let last: string | null = null;
    shown.forEach((item, i) => {
      if (item.group !== last) {
        rows.push(h('div.mt-gh', null, item.group));
        last = item.group;
      }
      const option = h('button.mt-li', { type: 'button', role: 'option', 'aria-selected': i === selected ? 'true' : 'false', class: i === selected ? 'mt-on' : null }, item.label);
      option.addEventListener('click', () => choose(i));
      rows.push(option);
    });
    list.replaceChildren(...rows);
    list.querySelector('.mt-on')?.scrollIntoView({ block: 'nearest' });
  };

  input.addEventListener('input', () => {
    selected = 0;
    draw();
  });
  const sheetEl = sheet(
    title,
    body,
    [h('span.mt-hint', null, '↑ ↓ move · Enter open · Esc close')],
    () => close(),
  );
  const scrim = h('div.scrim', null, sheetEl);
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
      trapTab(e, sheetEl);
    }
  });
  scrim.addEventListener('mousedown', (e) => {
    if (e.target === scrim) close();
  });
  draw();
  input.focus();
}

/** The keyboard shortcuts. */
export function help(): void {
  const keys: [string, string][] = [
    ['/ or Ctrl K', 'Find a scope, tab or action'],
    ['?', 'Keyboard shortcuts'],
    ['l', 'Live on or off'],
    ['z', 'Reset zoom'],
    ['f', 'Pin or unpin this scope'],
    ['a', 'Annotations on or off'],
    ['c', 'Compare with another scope'],
    ['d', 'Compact density'],
    ['n', 'New alert rule'],
    ['[ and ]', 'Earlier and later'],
    ['1 to 9', 'Select the tab at that position'],
    ['Esc', 'Close the open overlay, or leave full screen'],
  ];
  dialog(
    'Keyboard shortcuts',
    h('div.mt-kb', null, keys.map(([key, text]) => h('div.mt-kb-row', null, h('kbd', null, key), h('span', null, text)))),
    [{ label: 'Close', primary: true }],
  );
}

export interface DrawerCtx {
  go(target: { scope?: string; tab?: string }): void;
  zoom(z: [number, number]): void;
  traces(): void;
}

/** The chart tone of a mover's status; a muted status draws in the ink tone. */
function sparkTone(status: Status): 'ok' | 'warn' | 'crit' | 'info' | 'ink-3' {
  return status === 'mute' ? 'ink-3' : status;
}

/** "Around" a clicked point: the events within 30 min either side, the series that moved most, the kept traces, a zoom. */
export function drawer(a: Around, ctx: DrawerCtx): void {
  const event = (row: EventRow) =>
    h('div.mt-ev', null,
      h('span', { class: `mt-st mt-${row.status}` }),
      h('span.mt-mono', null, row.time),
      h('span', null, row.subject),
      h('span.muted', null, row.detail));

  const mover = (row: MoverRow) => {
    const moverButton = h('button.mt-mv', { type: 'button' },
      h('span', null, row.name),
      spark(row.spark, sparkTone(row.status), 96, 22, 'mt-spark mt-spk'),
      h('span.num', null, row.change));
    moverButton.addEventListener('click', () => {
      close();
      ctx.go({ tab: row.tab, scope: row.scope });
    });
    return moverButton;
  };

  const body = h('div.mt-form', null,
    h('div.mt-sub', null, a.sub),
    h('h3', null, 'Events'),
    h('div.mt-ev-list', null, a.events.length ? a.events.map(event) : h('div.mt-empty', null, 'Nothing in the 30 minutes either side.')),
    h('h3', null, 'Moved the most'),
    h('div.mt-mv-list', null, a.movers.length ? a.movers.map(mover) : h('div.mt-empty', null, 'Nothing moved in this window.')),
    h('h3', null, 'Traces kept in this window'),
    h('div.mt-kv2', null,
      h('span', null, h('b.num', null, a.traces.total), ' kept'),
      h('span', null, h('b.num', null, a.traces.errors), ' errors'),
      h('span', null, h('b.num', null, a.traces.slow), ' slow')),
    h('div.mt-row2', null, button('Open traces', () => {
      close();
      ctx.traces();
    }, { kind: 'ghost', size: 'sm' })));

  const foot = [
    button('Close', () => close(), { kind: 'ghost' }),
    button('Zoom to ± 30 min', () => {
      close();
      ctx.zoom(a.zoom);
    }, { kind: 'primary' }),
  ];
  const scrim = h('div.scrim', null, sheet(`Around ${a.label}`, body, foot, () => close(), 'mt-aside'));
  scrim.addEventListener('mousedown', (e) => {
    if (e.target === scrim) close();
  });
  const close = mount(scrim, (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    }
  });
  scrim.querySelector<HTMLElement>('.sheet-h .iconbtn')?.focus();
}

/** Closes whatever overlay is open; true when one was. */
export function closeOverlay(): boolean {
  if (!current) return false;
  current();
  return true;
}

/** Whether an overlay is open. */
export function overlayOpen(): boolean {
  return document.querySelector('.scrim, .mt-menu') !== null;
}
