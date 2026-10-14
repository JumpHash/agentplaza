import { filterPalette, type PaletteItem } from '../ide/view';
import css from '../styles/ide-palette.css';

export interface Palette {
  el: HTMLElement;
  open(items: PaletteItem[]): void;
  close(): void;
  isOpen(): boolean;
  dispose(): void;
}

const STYLE_ID = 'apz-ide-palette-style';
const INPUT_ID = 'ide-pal-input';
const LIST_ID = 'ide-pal-list';
const PLACEHOLDER = 'Go to an agent, file, task or command';

const GROUPS: { group: PaletteItem['group']; label: string }[] = [
  { group: 'agent', label: 'Agents' },
  { group: 'file', label: 'Files' },
  { group: 'task', label: 'Tasks' },
  { group: 'command', label: 'Commands' },
];

function injectStyle(): void {
  if (document.getElementById(STYLE_ID)) return;
  const style = document.createElement('style');
  style.id = STYLE_ID;
  style.textContent = css;
  document.head.append(style);
}

/** The hits in display order: the groups in GROUPS order, each keeping the order filterPalette gave it. */
function groupHits(hits: PaletteItem[]): { label: string; items: PaletteItem[] }[] {
  return GROUPS.map(({ group, label }) => ({ label, items: hits.filter((item) => item.group === group) })).filter(
    (g) => g.items.length > 0,
  );
}

function make(tag: string, cls: string, text?: string): HTMLElement {
  const node = document.createElement(tag);
  node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

/**
 * The command palette (Ctrl/Cmd+K). The shell creates it once; open(items) attaches the overlay to the body,
 * close() detaches it and hands focus back to whatever had it before.
 */
export function palette(onRun: (item: PaletteItem) => void): Palette {
  injectStyle();

  const root = make('div', 'ide-pal');
  const dialog = make('div', 'ide-pal-dialog');
  dialog.setAttribute('role', 'dialog');
  dialog.setAttribute('aria-modal', 'true');
  dialog.setAttribute('aria-label', 'Command palette');

  const input = document.createElement('input');
  input.type = 'text';
  input.id = INPUT_ID;
  input.className = 'ide-pal-input';
  input.placeholder = PLACEHOLDER;
  input.setAttribute('aria-label', PLACEHOLDER);
  input.setAttribute('role', 'combobox');
  input.setAttribute('aria-expanded', 'true');
  input.setAttribute('aria-controls', LIST_ID);
  input.setAttribute('aria-autocomplete', 'list');
  input.setAttribute('autocomplete', 'off');
  input.setAttribute('autocapitalize', 'off');
  input.setAttribute('spellcheck', 'false');

  const list = make('div', 'ide-pal-list');
  list.id = LIST_ID;
  list.setAttribute('role', 'listbox');
  list.setAttribute('aria-label', 'Results');

  dialog.append(input, list);
  root.append(dialog);

  let handler: ((item: PaletteItem) => void) | null = onRun;
  let all: PaletteItem[] = [];
  let shown: PaletteItem[] = []; // the visible options, in display order
  let optEls: HTMLElement[] = [];
  let active = 0;
  let opened = false;
  let running = false;
  let disposed = false;
  let prevFocus: HTMLElement | null = null;

  function setActive(i: number, scroll = true): void {
    const n = shown.length;
    if (n === 0) return;
    active = ((i % n) + n) % n; // wraps at both ends
    optEls.forEach((opt, j) => opt.setAttribute('aria-selected', j === active ? 'true' : 'false'));
    const cur = optEls[active];
    if (!cur) return;
    input.setAttribute('aria-activedescendant', cur.id);
    if (scroll && typeof cur.scrollIntoView === 'function') cur.scrollIntoView({ block: 'nearest' });
  }

  function render(): void {
    list.textContent = '';
    shown = [];
    optEls = [];
    list.scrollTop = 0;
    const groups = groupHits(filterPalette(all, input.value));
    if (groups.length === 0) {
      const none = make('div', 'ide-pal-empty', 'No match');
      none.setAttribute('role', 'presentation');
      list.append(none);
    }
    for (const g of groups) {
      const section = make('div', 'ide-pal-group');
      section.setAttribute('role', 'group');
      section.setAttribute('aria-label', g.label);
      const heading = make('div', 'ide-pal-heading', g.label);
      heading.setAttribute('aria-hidden', 'true');
      section.append(heading);
      for (const item of g.items) {
        const opt = make('div', 'ide-pal-opt');
        opt.id = `ide-pal-opt-${shown.length}`;
        opt.setAttribute('role', 'option');
        opt.setAttribute('aria-selected', 'false');
        opt.append(make('span', 'ide-pal-label', item.label));
        if (item.hint) opt.append(make('span', 'ide-pal-hint', item.hint));
        opt.addEventListener('click', () => run(item));
        shown.push(item);
        optEls.push(opt);
        section.append(opt);
      }
      list.append(section);
    }
    if (shown.length === 0) input.removeAttribute('aria-activedescendant');
    else setActive(0, false);
  }

  /** Keeps focus on the input while the palette is open (the only focusable control in the dialog). */
  function keepFocus(e: FocusEvent): void {
    if (opened && !running && !root.contains(e.target as Node)) input.focus();
  }

  function close(): void {
    if (!opened) return;
    opened = false;
    document.removeEventListener('focusin', keepFocus);
    const focus = document.activeElement;
    // Restore focus only when it still sits in the palette (or nowhere), so a target that took focus in onRun keeps it.
    const inPalette = !focus || focus === document.body || root.contains(focus);
    root.remove();
    const back = prevFocus;
    prevFocus = null;
    if (inPalette && back && back.isConnected) back.focus();
  }

  function run(item: PaletteItem): void {
    running = true;
    try {
      handler?.(item);
    } finally {
      running = false;
      close();
    }
  }

  function open(items: PaletteItem[]): void {
    if (disposed) return;
    all = items.slice();
    input.value = '';
    if (!opened) {
      opened = true;
      prevFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      document.body.append(root);
      document.addEventListener('focusin', keepFocus);
    }
    render();
    input.focus();
  }

  input.addEventListener('input', render);
  input.addEventListener('keydown', (e) => {
    if (e.isComposing) return;
    let handled = true;
    switch (e.key) {
      case 'ArrowDown':
        setActive(active + 1);
        break;
      case 'ArrowUp':
        setActive(active - 1);
        break;
      case 'Home':
        setActive(0);
        break;
      case 'End':
        setActive(shown.length - 1);
        break;
      case 'Enter': {
        const item = shown[active];
        if (item) run(item);
        break;
      }
      case 'Escape':
        close();
        break;
      case 'Tab':
        break; // focus stays on the input
      default:
        handled = false;
    }
    if (handled) {
      e.preventDefault();
      e.stopPropagation();
    }
  });

  // Mousedown never moves focus off the input (options run on click). A click on the backdrop closes.
  root.addEventListener('mousedown', (e) => {
    if (e.target !== input) e.preventDefault();
  });
  root.addEventListener('click', (e) => {
    if (e.target === root) close();
  });

  function dispose(): void {
    close();
    handler = null;
    disposed = true;
  }

  return { el: root, open, close, isOpen: () => opened, dispose };
}
