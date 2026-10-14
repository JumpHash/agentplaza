// The IDE's layout state and its pure helpers: the side panels' sizes and open flags (kept in localStorage), the narrow
// breakpoint, the screen the Console button returns to, and the panel an openIde link asks for. No DOM, so the tests
// check it directly.

import { parse } from '../router';

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

/** The layout a first visit gets: both panels open on a wide window, both closed on a narrow one. */
export function defaultLayout(width: number): Layout {
  const narrow = isNarrow(width);
  return { left: LEFT_SIZE.def, right: RIGHT_SIZE.def, leftOpen: !narrow, rightOpen: !narrow };
}

/** The stored layout, with anything missing or out of range set to its default. Corrupt JSON is the default layout. */
export function parseLayout(raw: string | null, width: number): Layout {
  const base = defaultLayout(width);
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
