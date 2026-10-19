// The Files tab's tree: the workspace's loaded entries, one level per open folder, with the changed-only filter. The
// controller loads the directories and owns the state; this only turns them into rows (the rows keep the tree styles
// of proto/ide.css: ided-row, dir, file, changed, on).

import type { Entry } from '../ide/types';
import type { Ide } from '../ide/wasm';
import { h } from '../ui/dom';

export interface TreeProps {
  ide: Ide;
  /** Every loaded entry of the workspace. */
  all: Entry[];
  /** The directories whose listing is loaded (the root is ''). */
  loaded: Set<string>;
  expanded: Set<string>;
  /** The paths changed since the base (their rows get a dot). */
  changed: Set<string>;
  /** The changed-only filter's kept paths, or null when the filter is off. */
  keep: Set<string> | null;
  active: string | null;
  hasWorkspace: boolean;
  onOpen(path: string): void;
  onToggle(path: string): void;
}

function treeRow(e: Entry, depth: number, open: boolean, changed: boolean, active: boolean, p: TreeProps): HTMLElement {
  const isDir = e.kind === 'dir';
  const name = e.path.slice(e.path.lastIndexOf('/') + 1);
  const row = h(
    'div.ided-row',
    {
      class: [isDir ? 'dir' : 'file', changed ? 'changed' : '', active ? 'on' : ''].filter(Boolean).join(' '),
      role: 'treeitem',
      'aria-expanded': isDir ? (open ? 'true' : 'false') : null,
      'aria-selected': active ? 'true' : null,
      style: `padding-left:${16 + depth * 14}px`,
      title: e.path,
      tabindex: 0,
    },
    h('span.ided-caret', null, isDir ? (open ? '▾' : '▸') : ''),
    h('span.ided-name', null, name),
    e.binary ? h('span.ided-tag', null, 'bin') : null,
    changed && !isDir ? h('span.ide-dot', { title: 'Changed since this base' }) : null,
  );
  const act = () => (isDir ? p.onToggle(e.path) : p.onOpen(e.path));
  row.addEventListener('click', act);
  row.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter' || ev.key === ' ') {
      ev.preventDefault();
      act();
    }
  });
  return row;
}

/** The rows of the tree, depth first, from the root down; a folder that is open but not loaded shows "Loading...". */
export function treeRows(p: TreeProps): HTMLElement[] {
  const rows: HTMLElement[] = [];
  const changedPaths = p.changed;
  const visit = (dir: string, depth: number): void => {
    let children = p.ide.children(p.all, dir);
    if (p.keep) children = children.filter((c) => p.keep?.has(c.path));
    for (const c of children) {
      const isDir = c.kind === 'dir';
      const open = isDir && (p.keep !== null || p.expanded.has(c.path));
      rows.push(treeRow(c, depth, open, changedPaths.has(c.path), c.path === p.active, p));
      if (!isDir || !open) continue;
      if (!p.loaded.has(c.path)) rows.push(h('div.ided-row.loading', { style: `padding-left:${30 + depth * 14}px` }, 'Loading...'));
      else visit(c.path, depth + 1);
    }
  };
  if (p.hasWorkspace && p.loaded.has('')) visit('', 0);
  else if (p.hasWorkspace) rows.push(h('div.ided-row.loading', { style: 'padding-left:16px' }, 'Loading...'));
  return rows;
}
