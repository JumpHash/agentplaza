// The IDE's own chrome: the edges that resize a panel or the drawer, the status bar and the keyboard help. Pure rendering:
// the screen (views/ide.ts) owns the state and passes the callbacks, and these paint what it gives them.

import type { Lang } from '../ide/model';
import { h } from '../ui/dom';

/**
 * The edge of a side panel: drag it to resize (`onDrag` gets the pointer's x while it is held, `onDone` when it is let
 * go), double-click to reset the width. The edge sits inside its panel, at the side that faces the editor.
 */
export function panelEdge(side: 'left' | 'right', onDrag: (clientX: number) => void, onDone: () => void, onReset: () => void): HTMLElement {
  const el = h(`div.ide-edge.ide-edge-${side}`, {
    role: 'separator',
    'aria-orientation': 'vertical',
    'aria-label': side === 'left' ? 'Resize the side panel' : 'Resize the right panel',
    title: 'Drag to resize · double-click to reset',
  });
  const move = (e: PointerEvent) => onDrag(e.clientX);
  const end = () => {
    el.removeEventListener('pointermove', move);
    el.classList.remove('on');
    el.closest('.ide-frame')?.classList.remove('dragging');
    onDone();
  };
  el.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    el.setPointerCapture(e.pointerId);
    el.classList.add('on');
    el.closest('.ide-frame')?.classList.add('dragging');
    el.addEventListener('pointermove', move);
  });
  el.addEventListener('pointerup', end);
  el.addEventListener('pointercancel', end);
  el.addEventListener('dblclick', onReset);
  return el;
}

/**
 * The top edge of the terminal drawer: drag it to resize (`onDrag` gets the pointer's y while it is held, `onDone` when it is
 * let go); an arrow key steps it by `onStep`. It sits on the drawer's top border, inside the editor column.
 */
export function dockEdge(onDrag: (clientY: number) => void, onDone: () => void, onStep: (px: number) => void): HTMLElement {
  const el = h('div.ide-bottom-edge', {
    role: 'separator',
    'aria-orientation': 'horizontal',
    'aria-label': 'Resize the terminal',
    title: 'Drag to resize',
    tabindex: '0',
  });
  const move = (e: PointerEvent) => onDrag(e.clientY);
  const end = () => {
    el.removeEventListener('pointermove', move);
    el.classList.remove('on');
    el.closest('.ide-frame')?.classList.remove('dragging-rows');
    onDone();
  };
  el.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    el.setPointerCapture(e.pointerId);
    el.classList.add('on');
    el.closest('.ide-frame')?.classList.add('dragging-rows');
    el.addEventListener('pointermove', move);
  });
  el.addEventListener('pointerup', end);
  el.addEventListener('pointercancel', end);
  el.addEventListener('keydown', (e) => {
    if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return;
    e.preventDefault();
    onStep(e.key === 'ArrowUp' ? 24 : -24);
  });
  return el;
}

export interface StatusInfo {
  workspace: string;
  base: string;
  live: boolean;
  /** The cursor of the shown editor (1-based), or null when no file is shown. */
  cursor: { line: number; col: number } | null;
  /** The shown file's language, or '' when no file is shown. */
  lang: string;
  dirty: number;
}

export const LANG_LABEL: Record<Lang, string> = {
  typescript: 'TypeScript',
  tsx: 'TSX',
  javascript: 'JavaScript',
  jsx: 'JSX',
  rust: 'Rust',
  python: 'Python',
  markdown: 'Markdown',
  json: 'JSON',
  css: 'CSS',
  html: 'HTML',
  plain: 'Plain text',
};

/**
 * The status bar: the live link, the workspace and its base on the left; the cursor, the language, the unsaved count and the
 * help on the right (the prototype's footer, in mono).
 */
export function statusBar(onHelp: () => void): { el: HTMLElement; paint(s: StatusInfo): void } {
  const live = h('span.ide-st-live');
  const workspace = h('span.ide-st-ws');
  const base = h('span.ide-st-base');
  const cursor = h('span.ide-st-cursor');
  const lang = h('span.ide-st-lang');
  const dirty = h('span.ide-st-dirty');
  const help = h('button.ide-helpb', { type: 'button', title: 'Keyboard shortcuts (?)', 'aria-label': 'Keyboard shortcuts', onclick: onHelp }, '?');
  const el = h(
    'footer.ide-status',
    { 'aria-label': 'Status' },
    live,
    workspace,
    base,
    h('span.spacer'),
    h('span.ide-st-group', null, cursor, lang, dirty),
    help,
  );
  return {
    el,
    paint(s) {
      live.replaceChildren(h('span.ide-dot', { 'data-status': s.live ? 'online' : 'failed' }), s.live ? 'Live' : 'Reconnecting…');
      workspace.textContent = s.workspace;
      base.textContent = s.base;
      cursor.textContent = s.cursor ? `Ln ${s.cursor.line}, Col ${s.cursor.col}` : '';
      lang.textContent = s.lang;
      dirty.textContent = s.dirty > 0 ? `${s.dirty} unsaved` : '';
    },
  };
}

/** The keys, as the help popover lists them. */
export const SHORTCUTS: [string, string][] = [
  ['Ctrl/Cmd+S', 'Save the open file'],
  ['Ctrl/Cmd+P', 'Quick open: a file by name'],
  ['Ctrl/Cmd+Shift+F', 'Search the workspace (the Search view)'],
  ['Ctrl/Cmd+Alt+B', 'Show or hide the left panel'],
  ['Ctrl/Cmd+Alt+J', 'Show or hide the right panel'],
  ['?', 'This list'],
  ['Esc', 'Close the help, a panel or a search; with none open, back to the console'],
];

/** The help popover: the shortcuts, over the screen's corner. Hidden until `set(true)`. */
export function keyboardHelp(): { el: HTMLElement; isOpen(): boolean; set(open: boolean): void } {
  const el = h(
    'div.ide-help',
    { role: 'dialog', 'aria-label': 'Keyboard shortcuts', hidden: true },
    h('h3', null, 'Keyboard'),
    h('dl', null, ...SHORTCUTS.map(([keys, what]) => h('div.ide-help-r', null, h('dt', null, h('kbd.ide-kbd', null, keys)), h('dd', null, what)))),
  );
  let open = false;
  return {
    el,
    isOpen: () => open,
    set(v) {
      open = v;
      el.hidden = !v;
    },
  };
}
