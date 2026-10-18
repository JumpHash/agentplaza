// Opens the IDE screen. Entry points (the top bar, the agent and project screens) import this module, never the view:
// the view and CodeMirror load on the first visit (views/index.ts), so this file must stay small.

import { href } from '../router';
import { isIdeHash, originFallback } from './layout';

export interface OpenOpts {
  /** A file to open in a tab, a path in the workspace. */
  path?: string;
  /** The panel to show first: a left view (changes, checkpoints) or a right tab (agent, approvals, run). */
  panel?: 'changes' | 'checkpoints' | 'agent' | 'approvals' | 'run';
  /** Review mode: the centre shows the diff instead of the editor. */
  review?: boolean;
}

/** The console route the owner was on last: the IDE's Console button and Escape return to it. */
let consoleHash: string | null = null;

/** Called for each console route (main.ts), so the IDE knows the screen it was opened from. */
export function noteConsoleRoute(hash: string): void {
  if (!isIdeHash(hash)) consoleHash = hash;
}

/** The hash the IDE returns to: the console route it was opened from, else the overview. */
export function consoleOrigin(): string {
  return originFallback(consoleHash);
}

/** Opens the owner's conversation `id` (`conv-<n>`) in the IDE: the chat with the orchestrator and its project draft. */
export function openConversation(id: string): void {
  location.hash = href('ide', id);
}

/**
 * Opens the IDE on a workspace id (`agent:7`), optionally on a file, a panel and review mode. An empty id opens the
 * picker, which selects the last workspace the IDE used. The IDE's Console button goes back to the screen it came from.
 */
export function openIde(workspaceId: string, opts: OpenOpts = {}): void {
  const q = new URLSearchParams();
  if (opts.path) q.set('file', opts.path);
  if (opts.panel) q.set('panel', opts.panel);
  if (opts.review) q.set('review', '1');
  const query = q.toString();
  location.hash = (workspaceId ? href('ide', workspaceId) : href('ide')) + (query ? `?${query}` : '');
}
