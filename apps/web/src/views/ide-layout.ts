// The IDE's layout: the panel sizes and open flags, the drawer's open state and height, the drag edges and the breakpoints.
// The sizes are kept per browser. A closed panel is hidden, and the columns are CSS variables on the frame.
import { fitSize, parseLayout, serializeLayout, type Layout } from '../ide/layout';
import { dockEdge, panelEdge } from './ide-chrome';
import { DRAWER_KEY, LAYOUT_KEY, readStore, writeStore, type IdeCtx } from './ide-ctx';
import type { DrawerTab } from './ide-frame';

/** The drawer's height limits: the centre keeps `editorMin` px above it. */
export const DRAWER = { min: 120, def: 240, max: 1200, editorMin: 160 };
export const MID_QUERY = '(max-width: 1199px)';

export interface DrawerPrefs {
  open: boolean;
  tab: DrawerTab;
  height: number;
}

export function readDrawerPrefs(): DrawerPrefs {
  try {
    const v = JSON.parse(readStore(DRAWER_KEY) ?? 'null') as { open?: unknown; tab?: unknown; height?: unknown } | null;
    const height = typeof v?.height === 'number' && Number.isFinite(v.height) ? Math.min(Math.max(v.height, DRAWER.min), DRAWER.max) : DRAWER.def;
    return { open: v?.open === true, tab: v?.tab === 'run' ? 'run' : 'terminal', height: Math.round(height) };
  } catch {
    return { open: false, tab: 'terminal', height: DRAWER.def };
  }
}

export function readLayout(width: number): Layout {
  return parseLayout(readStore(LAYOUT_KEY), width);
}

/** Puts the sizes and open flags on the screen: the columns are CSS variables, and a closed panel is hidden. */
export function applyLayout(c: IdeCtx): void {
  const { frame } = c.dom;
  const L = c.layout;
  const width = window.innerWidth;
  const leftPx = fitSize(L.left, 'left', width, !c.narrow && L.rightOpen && !c.mid ? L.right : 0);
  const rightPx = fitSize(L.right, 'right', width, !c.narrow && L.leftOpen ? leftPx : 0);
  frame.root.style.setProperty('--iw-side', L.leftOpen && !c.narrow ? `${leftPx}px` : '0px');
  frame.root.style.setProperty('--iw-right', L.rightOpen && !c.mid ? `${rightPx}px` : '0px');
  frame.root.style.setProperty('--iw-drawer', c.drawerOpen ? `${c.dockHeight}px` : '0px');
  frame.root.classList.toggle('narrow', c.narrow);
  frame.root.classList.toggle('mid', c.mid);
  frame.side.hidden = !L.leftOpen && !c.narrow;
  frame.side.classList.toggle('open', L.leftOpen && c.narrow);
  frame.right.el.hidden = !L.rightOpen && !c.mid;
  frame.right.el.classList.toggle('open', L.rightOpen && c.mid);
  frame.scrim.classList.toggle('show', (c.narrow && L.leftOpen) || (c.mid && L.rightOpen));
  frame.sideToggle.setAttribute('aria-pressed', String(L.leftOpen));
  frame.rightToggle.setAttribute('aria-pressed', String(L.rightOpen));
}

export function saveLayout(c: IdeCtx): void {
  writeStore(LAYOUT_KEY, serializeLayout(c.layout));
}

export function saveDrawerPrefs(c: IdeCtx): void {
  writeStore(DRAWER_KEY, JSON.stringify({ open: c.drawerOpen, tab: c.drawerTab, height: c.dockHeight }));
}

/** Keeps the drawer within the centre column: at least DRAWER.min, and the centre keeps DRAWER.editorMin above it. */
export function clampDock(c: IdeCtx, px: number): number {
  const column = c.dom.frame.centre.el;
  const room = column.clientHeight > 0 ? column.clientHeight - DRAWER.editorMin : Infinity;
  return Math.round(Math.max(DRAWER.min, Math.min(px, Math.max(DRAWER.min, room))));
}

export function setLeft(c: IdeCtx, open: boolean): void {
  c.layout.leftOpen = open;
  saveLayout(c);
  applyLayout(c);
}

export function setRightOpen(c: IdeCtx, open: boolean): void {
  c.layout.rightOpen = open;
  saveLayout(c);
  applyLayout(c);
}

/** Narrow screens: the sidebar drawer closes once the owner has picked something in it. */
export function closeDrawerIfNarrow(c: IdeCtx): void {
  if (c.narrow) c.layout.leftOpen = false;
  if (c.mid) c.layout.rightOpen = false;
  applyLayout(c);
}

export function setDrawer(c: IdeCtx, open: boolean, tab: DrawerTab = c.drawerTab): void {
  c.drawerOpen = open;
  c.drawerTab = tab;
  saveDrawerPrefs(c);
  c.act.renderAll();
  if (open && tab === 'terminal') c.term?.focus();
}

/** A breakpoint changed: the flags follow the media queries, and the layout is applied again. */
export function onBreakpoint(c: IdeCtx): void {
  c.narrow = c.dom.narrowMq.matches;
  c.mid = c.dom.midMq.matches;
  applyLayout(c);
}

/**
 * The three drag handles: the sidebar's and the right panel's inner edges, and the drawer's top edge. They are built before
 * the frame exists (the frame takes them as inputs), so their handlers read the frame through the context when they run.
 */
export function layoutEdges(c: IdeCtx): { sideEdge: HTMLElement; rightEdge: HTMLElement; drawerEdge: HTMLElement } {
  const sideEdge = panelEdge(
    'left',
    (x) => {
      c.layout.left = fitSize(x - c.dom.frame.root.getBoundingClientRect().left, 'left', window.innerWidth, c.layout.rightOpen && !c.mid ? c.layout.right : 0);
      applyLayout(c);
    },
    () => saveLayout(c),
    () => {
      c.layout.left = 260;
      applyLayout(c);
      saveLayout(c);
    },
  );
  const rightEdge = panelEdge(
    'right',
    (x) => {
      c.layout.right = fitSize(c.dom.frame.root.getBoundingClientRect().right - x, 'right', window.innerWidth, c.layout.leftOpen && !c.narrow ? c.layout.left : 0);
      applyLayout(c);
    },
    () => saveLayout(c),
    () => {
      c.layout.right = 380;
      applyLayout(c);
      saveLayout(c);
    },
  );
  const drawerEdge = dockEdge(
    (y) => {
      c.dockHeight = clampDock(c, c.dom.frame.drawer.el.getBoundingClientRect().bottom - y);
      applyLayout(c);
    },
    () => saveDrawerPrefs(c),
    (px) => {
      c.dockHeight = clampDock(c, c.dockHeight + px);
      applyLayout(c);
      saveDrawerPrefs(c);
    },
  );
  return { sideEdge, rightEdge, drawerEdge };
}
