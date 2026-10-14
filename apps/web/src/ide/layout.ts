// The IDE's layout state and its pure helpers: the side panels' sizes and open flags (kept in localStorage), the narrow
// breakpoint, the screen the Console button returns to, and the panel an openIde link asks for. No DOM, so the tests
// check it directly.

import { parse } from '../router';
import type { WorkspaceKind } from './types';

export type LeftView = 'files' | 'search' | 'changes' | 'checkpoints';
export type RightTab = 'agent' | 'approvals' | 'run' | 'terminal';

/** The activity rail's width in pixels. */
export const RAIL_WIDTH = 44;
/** Under this width the side panel is a drawer over the editor and the right panel a sheet under it. */
export const NARROW_QUERY = '(max-width: 899px)';
export const NARROW_BELOW = 900;
/** The narrowest the editor column may get when both panels are open. */
export const CENTRE_MIN = 320;
export const LEFT_SIZE = { min: 180, max: 520, def: 260 };
export const RIGHT_SIZE = { min: 260, max: 720, def: 380 };

export interface Layout {
  left: number;
  right: number;
  leftOpen: boolean;
  rightOpen: boolean;
}

export const isNarrow = (width: number): boolean => width < NARROW_BELOW;

/** `n` as a whole number kept within [min, max]; anything that is not a finite number is the fallback. */
export function clampSize(n: unknown, min: number, max: number, fallback: number): number {
  const v = typeof n === 'number' && Number.isFinite(n) ? Math.round(n) : fallback;
  return Math.min(max, Math.max(min, v));
}

/**
 * The layout a first visit gets: both panels open. The saved open flags are the owner's choice for a panel that is docked;
 * a narrow window shows the panels as a drawer and a sheet instead (see Sheets), and that never changes the saved flags.
 */
export function defaultLayout(): Layout {
  return { left: LEFT_SIZE.def, right: RIGHT_SIZE.def, leftOpen: true, rightOpen: true };
}

/** The stored layout, with anything missing or out of range set to its default. Corrupt JSON is the default layout. */
export function parseLayout(raw: string | null): Layout {
  const base = defaultLayout();
  let v: Partial<Record<keyof Layout, unknown>> | null = null;
  try {
    v = raw === null ? null : (JSON.parse(raw) as Partial<Record<keyof Layout, unknown>>);
  } catch {
    v = null;
  }
  if (v === null || typeof v !== 'object') return base;
  return {
    left: clampSize(v.left, LEFT_SIZE.min, LEFT_SIZE.max, base.left),
    right: clampSize(v.right, RIGHT_SIZE.min, RIGHT_SIZE.max, base.right),
    leftOpen: typeof v.leftOpen === 'boolean' ? v.leftOpen : base.leftOpen,
    rightOpen: typeof v.rightOpen === 'boolean' ? v.rightOpen : base.rightOpen,
  };
}

export const serializeLayout = (l: Layout): string => JSON.stringify(l);

// ---- What the panels show -----------------------------------------------------------------------------------------
// A panel is docked (its saved open flag decides), or it is a drawer (the side panel under NARROW_BELOW), or a sheet (the
// right panel under 1200px, the view's MID_QUERY). The drawer and the sheet are open or shut as view state only: they
// start shut on every mount and after every breakpoint change, a pick in them shuts them, and they are never saved.
// Only an explicit toggle of a docked panel changes the saved flags.

/** The drawer's and the sheet's open state. View only: never saved. */
export interface Sheets {
  drawer: boolean;
  sheet: boolean;
}
export const SHUT: Sheets = { drawer: false, sheet: false };

/** Whether the side panel shows: its drawer at a narrow width, else the saved choice. */
export const leftShows = (layout: Layout, sheets: Sheets, narrow: boolean): boolean => (narrow ? sheets.drawer : layout.leftOpen);

/** Whether the right panel shows: its sheet under the mid breakpoint (which includes the narrow width), else the saved choice. */
export const rightShows = (layout: Layout, sheets: Sheets, mid: boolean): boolean => (mid ? sheets.sheet : layout.rightOpen);

/** The answer of a rule: the layout and the sheets to keep, and whether the layout is saved (only a docked panel's choice is). */
export interface Settled {
  layout: Layout;
  sheets: Sheets;
  save: boolean;
}

/** An explicit toggle of the side panel. Docked: the saved flag flips and is saved. As a drawer: only the drawer flips. */
export function toggleLeft(layout: Layout, sheets: Sheets, narrow: boolean): Settled {
  if (narrow) return { layout, sheets: { ...sheets, drawer: !sheets.drawer }, save: false };
  return { layout: { ...layout, leftOpen: !layout.leftOpen }, sheets, save: true };
}

/** An explicit toggle of the right panel. Docked: the saved flag flips and is saved. As a sheet: only the sheet flips. */
export function toggleRight(layout: Layout, sheets: Sheets, mid: boolean): Settled {
  if (mid) return { layout, sheets: { ...sheets, sheet: !sheets.sheet }, save: false };
  return { layout: { ...layout, rightOpen: !layout.rightOpen }, sheets, save: true };
}

/** Opens the right panel for a tab or a search: its sheet at a mid width (view only), else its saved flag, saved when it was shut. */
export function openRight(layout: Layout, sheets: Sheets, mid: boolean): Settled {
  if (mid) return { layout, sheets: { ...sheets, sheet: true }, save: false };
  if (layout.rightOpen) return { layout, sheets, save: false };
  return { layout: { ...layout, rightOpen: true }, sheets, save: true };
}

/** A pick in the drawer or the sheet, the scrim, or Escape: each one that is shown at its width shuts. View only. */
export function closeSheets(sheets: Sheets, narrow: boolean, mid: boolean): Sheets {
  return { drawer: narrow ? false : sheets.drawer, sheet: mid ? false : sheets.sheet };
}

/**
 * A side panel's width for a dragged or stored size: kept within the panel's range, and within what is left of the
 * window once the rail, the other open panel (`otherOpen`, its width, or 0) and the editor's minimum take their share.
 */
export function fitSize(px: number, side: 'left' | 'right', width: number, otherOpen: number): number {
  const range = side === 'left' ? LEFT_SIZE : RIGHT_SIZE;
  const room = width - RAIL_WIDTH - otherOpen - CENTRE_MIN;
  return clampSize(px, range.min, Math.max(range.min, Math.min(range.max, room)), range.def);
}

export const isIdeHash = (hash: string): boolean => parse(hash).screen === 'ide';

/** Where the Console button and Escape go: the console route the IDE was opened from, else the overview. */
export function originFallback(hash: string | null): string {
  if (!hash || isIdeHash(hash)) return '#/overview';
  return hash;
}

/** The left view an openIde link names with `panel` (changes or checkpoints), or null. */
export function leftViewFor(panel: string | null): LeftView | null {
  return panel === 'changes' || panel === 'checkpoints' ? panel : null;
}

/** The right tab an openIde link names with `panel`, or null. */
export function rightTabFor(panel: string | null): RightTab | null {
  return panel === 'agent' || panel === 'approvals' || panel === 'run' || panel === 'terminal' ? panel : null;
}

// ---- What each selection has ------------------------------------------------
// A workstation (`station:<id>`) has its work folder, its editor, its terminal and its runs. It has no diff (so no Diff
// view, no changed-only filter and no checkpoints), no chat, and no project (so no Board, Tasks, Approvals or Activity).
// A selection with no workspace yet (no agent, or an agent with no VM) has what a repository has.

export type CentreKey = 'chat' | 'terminal' | 'editor' | 'diff';
export type RightKey = 'board' | 'files' | 'tasks' | 'approvals' | 'activity';

/** Whether a kind has a diff: the Diff view, the changed-only filter and the checkpoints. Only a workstation has none. */
export const hasDiff = (kind: WorkspaceKind | null): boolean => kind !== 'station';

/** Whether a selection has a terminal: an agent's, or a workstation's own shell. A repository has none. */
export const hasTerminal = (kind: WorkspaceKind | null, agent: boolean): boolean => agent || kind === 'station';

/** The centre's views a selection has. Chat is an agent's; the editor is every workspace's; the diff is any kind with one. */
export function centreViewsFor(kind: WorkspaceKind | null, agent: boolean): CentreKey[] {
  const views: CentreKey[] = [];
  if (agent) views.push('chat');
  if (hasTerminal(kind, agent)) views.push('terminal');
  views.push('editor');
  if (hasDiff(kind)) views.push('diff');
  return views;
}

/** The right panel's tabs a selection has: an agent's are all five; a workstation has its files; else the project's three. */
export function rightViewsFor(kind: WorkspaceKind | null, agent: boolean): RightKey[] {
  if (agent) return ['board', 'files', 'tasks', 'approvals', 'activity'];
  if (kind === 'station') return ['files'];
  return ['board', 'files', 'tasks'];
}

/** Whether a kind runs commands: an agent's and a station's do; a repository's runs come in a later phase. */
export const runsFor = (kind: WorkspaceKind | null): boolean => kind === 'agent' || kind === 'station';
