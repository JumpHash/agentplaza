// The IDE's own chrome: the activity rail, the edges that resize a side panel, the status bar and the keyboard help.
// Pure rendering: the screen (views/ide.ts) owns the state and passes the callbacks, and these paint what it gives them.

import type { Lang } from '../ide/model';
import type { LeftView } from '../ide/layout';
import { dot } from '../ui/components';
import { h, icon, replace } from '../ui/dom';
import type { IconName } from '../ui/icons.gen';

const RAIL: { key: LeftView; label: string; icon: IconName }[] = [
  { key: 'files', label: 'Files', icon: 'file' },
  { key: 'search', label: 'Search', icon: 'search' },
  { key: 'changes', label: 'Changes', icon: 'git' },
  { key: 'checkpoints', label: 'Checkpoints', icon: 'clock' },
];

export interface RailPaint {
  /** The view the side panel shows, or null when the panel is closed. */
  active: LeftView | null;
  /** The count a rail item shows (changed files, checkpoints). */
  counts: Partial<Record<LeftView, number>>;
}

/** The activity rail: the side panel's views. Clicking the open view's item closes the panel (the screen decides). */
export function activityRail(onPick: (key: LeftView) => void): { el: HTMLElement; paint(p: RailPaint): void } {
  const items = new Map<LeftView, { button: HTMLButtonElement; count: HTMLElement }>();
  const el = h('nav.ide-rail', { 'aria-label': 'IDE views' });
  for (const item of RAIL) {
    const count = h('span.ide-rail-count');
    const button = h('button.ide-rail-item', { type: 'button', title: item.label, 'aria-label': item.label }, icon(item.icon), count);
    button.addEventListener('click', () => onPick(item.key));
    items.set(item.key, { button, count });
    el.appendChild(button);
  }
  return {
    el,
    paint(p) {
      for (const [key, { button, count }] of items) {
        const on = key === p.active;
        button.classList.toggle('on', on);
        button.setAttribute('aria-pressed', on ? 'true' : 'false');
        const n = p.counts[key];
        count.textContent = n ? String(n) : '';
      }
    },
  };
}

/**
 * The edge of a side panel: drag it to resize (`onDrag` gets the pointer's x while it is held, `onDone` when it is let
 * go), double-click to reset the width. The edge sits inside its panel, at the side that faces the editor.
 */
export function panelEdge(side: 'left' | 'right', onDrag: (clientX: number) => void, onDone: () => void, onReset: () => void): HTMLElement {
  const el = h('div.ide-edge', {
    role: 'separator',
    'aria-orientation': 'vertical',
    'aria-label': side === 'left' ? 'Resize the side panel' : 'Resize the right panel',
    title: 'Drag to resize · double-click to reset',
  });
  const move = (e: PointerEvent) => onDrag(e.clientX);
  const end = () => {
    el.removeEventListener('pointermove', move);
    el.classList.remove('on');
    el.closest('.ide')?.classList.remove('dragging');
    onDone();
  };
  el.addEventListener('pointerdown', (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    el.setPointerCapture(e.pointerId);
    el.classList.add('on');
    el.closest('.ide')?.classList.add('dragging');
    el.addEventListener('pointermove', move);
  });
  el.addEventListener('pointerup', end);
  el.addEventListener('pointercancel', end);
  el.addEventListener('dblclick', onReset);
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

/** The status bar: the workspace and its base on the left; the cursor, the language, the unsaved count and the help on the right. */
export function statusBar(onHelp: () => void): { el: HTMLElement; paint(s: StatusInfo): void } {
  const live = h('span.ide-st-live');
  const workspace = h('span.ide-st-ws');
  const base = h('span.ide-st-base');
  const cursor = h('span.ide-st-cursor');
  const lang = h('span.ide-st-lang');
  const dirty = h('span.ide-st-dirty');
  const help = h('button.ide-st-help', { type: 'button', title: 'Keyboard shortcuts (?)', 'aria-label': 'Keyboard shortcuts', onclick: onHelp }, '?');
  const el = h(
    'footer.ide-status',
    { 'aria-label': 'Status' },
    h('span.ide-st-group', null, live, workspace, base),
    h('span.spacer'),
    h('span.ide-st-group', null, cursor, lang, dirty, help),
  );
  return {
    el,
    paint(s) {
      replace(live, dot(s.live ? 'ok' : 'danger'), s.live ? 'Live' : 'Reconnecting…');
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

/** The help popover: the shortcuts, under the header. Hidden until `set(true)`. */
export function keyboardHelp(): { el: HTMLElement; isOpen(): boolean; set(open: boolean): void } {
  const el = h(
    'div.ide-help',
    { role: 'dialog', 'aria-label': 'Keyboard shortcuts', hidden: true },
    h('h2', null, 'Keyboard'),
    h('dl', null, SHORTCUTS.flatMap(([keys, what]) => [h('dt', null, h('kbd', null, keys)), h('dd', null, what)])),
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
