// Opens the IDE screen. Entry points (the top bar, the agent and project screens) import this module, never the view:
// the view and CodeMirror load on the first visit (views/index.ts), so this file must stay small.

import { href } from '../router';

export interface OpenOpts {
  /** A file to open in a tab, a path in the workspace. */
  path?: string;
  /** The right-hand panel to show first. */
  panel?: 'changes' | 'checkpoints' | 'agent' | 'approvals' | 'run';
  /** Review mode: the centre shows the diff instead of the editor. */
  review?: boolean;
}

/** Set by openIde, read once by the screen that mounts for it: only then does Escape go back. */
let fromApp = false;

/**
 * Opens the IDE on a workspace id (`agent:7`), optionally on a file, a panel and review mode. An empty id opens the
 * picker, which selects the last workspace the IDE used. Escape in the IDE goes back here.
 */
export function openIde(workspaceId: string, opts: OpenOpts = {}): void {
  const q = new URLSearchParams();
  if (opts.path) q.set('file', opts.path);
  if (opts.panel) q.set('panel', opts.panel);
  if (opts.review) q.set('review', '1');
  const query = q.toString();
  const target = (workspaceId ? href('ide', workspaceId) : href('ide')) + (query ? `?${query}` : '');
  // Already on this route: no hashchange, so no screen mounts to read the flag.
  fromApp = location.hash !== target;
  location.hash = target;
}

/** True once for the IDE screen that mounted after openIde; false for a screen reached any other way. */
export function takeOpenedFromApp(): boolean {
  const v = fromApp;
  fromApp = false;
  return v;
}
