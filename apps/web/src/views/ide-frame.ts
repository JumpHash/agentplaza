// The IDE's static chrome, in the prototype's agent-workspace layout: the navigator (the threads column) on the left; the
// centre with its top bar (the crumb, the state, the held banner, the Console button), its head (the status dot, the title,
// the view toggle, the pop-out), the view, the bottom drawer (terminal and run) and the status bar; and the dock on the right
// with its tabs. It only builds the elements and returns them; the controller (views/ide.ts) fills them and wires their
// events. Styles: styles/proto/ide.css (every rule under .ide-frame).

import { h, lu } from '../ui/dom';
import type { LucideName } from '../ui/icons.gen';

export type CentreView = 'chat' | 'terminal' | 'editor' | 'diff';
export type RightView = 'board' | 'files' | 'tasks' | 'approvals' | 'activity';
export type DrawerTab = 'terminal' | 'run';
/** The dock's own tabs that are not a right panel: the diff (the centre's Diff view) and the drawer's terminal and run. */
export type DockExtra = 'diff' | 'terminal' | 'run';

export const CENTRE_LABEL: Record<CentreView, string> = { chat: 'Chat', terminal: 'Terminal', editor: 'Editor', diff: 'Diff' };
export const RIGHT_LABEL: Record<RightView, string> = { board: 'Board', files: 'Files', tasks: 'Tasks', approvals: 'Approvals', activity: 'Activity' };
export const DRAWER_LABEL: Record<DrawerTab, string> = { terminal: 'Terminal', run: 'Run' };
export const EXTRA_LABEL: Record<DockExtra, string> = { diff: 'Diff', terminal: 'Terminal', run: 'Run' };

/** The icon of each view, the centre's toggle and the dock's tabs (the prototype's names). */
export const VIEW_ICON: Record<CentreView, LucideName> = { chat: 'bot', terminal: 'terminal', editor: 'file', diff: 'git-branch' };
export const DOCK_ICON: Record<RightView | DockExtra, LucideName> = {
  diff: 'git-compare',
  files: 'folder-tree',
  terminal: 'square-terminal',
  run: 'play',
  tasks: 'list-checks',
  board: 'pin',
  approvals: 'shield-check',
  activity: 'activity',
};

/** The dock's tabs in the prototype's order. */
const DOCK_ORDER: (RightView | DockExtra)[] = ['diff', 'files', 'terminal', 'run', 'tasks', 'board', 'approvals', 'activity'];

/** The parts the controller places into the frame (built elsewhere, so their own logic stays where it is). */
export interface FrameInputs {
  /** The navigator's body: its head, actions, list and foot (views/ide-sidebar). */
  side: HTMLElement;
  sideEdge: HTMLElement;
  rightEdge: HTMLElement;
  drawerEdge: HTMLElement;
  held: HTMLElement;
  theme: HTMLElement;
  status: HTMLElement;
}

export interface Frame {
  root: HTMLElement;
  sideToggle: HTMLButtonElement;
  rightToggle: HTMLButtonElement;
  consoleBtn: HTMLButtonElement;
  crumb: HTMLElement;
  state: HTMLElement;
  scrim: HTMLElement;
  side: HTMLElement;
  centre: {
    el: HTMLElement;
    dot: HTMLElement;
    title: HTMLElement;
    seg: Record<CentreView, HTMLButtonElement>;
    popout: HTMLButtonElement;
    views: Record<CentreView, HTMLElement>;
  };
  right: { el: HTMLElement; tabs: Record<RightView, HTMLButtonElement>; extra: Record<DockExtra, HTMLButtonElement>; body: HTMLElement };
  drawer: { el: HTMLElement; tabs: Record<DrawerTab, HTMLButtonElement>; body: HTMLElement; close: HTMLButtonElement };
}

const VIEWS: CentreView[] = ['chat', 'terminal', 'editor', 'diff'];
const RIGHT: RightView[] = ['board', 'files', 'tasks', 'approvals', 'activity'];
const EXTRA: DockExtra[] = ['diff', 'terminal', 'run'];
const DRAWER: DrawerTab[] = ['terminal', 'run'];

/** An icon-only button in the prototype's markup: a 32 px square, with its name as the title and the label. */
export function iconBtn(icon: LucideName, title: string, onClick?: () => void): HTMLButtonElement {
  return h('button.iconbtn', { type: 'button', title, 'aria-label': title, onclick: onClick ?? null }, lu(icon, 16)) as HTMLButtonElement;
}

/** A dock tab: its icon and its label, and an optional count (shown when the controller sets one). */
function dockTab(key: RightView | DockExtra, label: string, role: 'tab' | 'drawer'): HTMLButtonElement {
  const count = h('span.iw-count', { hidden: true });
  return h(
    'button.ide-dtab',
    { type: 'button', role: 'tab', 'aria-selected': 'false', 'data-key': key, title: label, class: role === 'drawer' ? 'ide-drawer-tab' : '' },
    lu(DOCK_ICON[key], 15),
    h('span.ide-dtab-l', null, label),
    count,
  ) as HTMLButtonElement;
}

function drawerTab(key: DrawerTab): HTMLButtonElement {
  return h('button.ide-dtab', { type: 'button', role: 'tab', 'aria-selected': 'false', 'data-key': key, title: DRAWER_LABEL[key] }, lu(DOCK_ICON[key], 15), h('span.ide-dtab-l', null, DRAWER_LABEL[key])) as HTMLButtonElement;
}

function segButton(view: CentreView): HTMLButtonElement {
  return h('button.ide-segb', { type: 'button', role: 'tab', 'aria-selected': 'false', 'data-view': view }, lu(VIEW_ICON[view], 15), h('span.ide-seg-label', null, CENTRE_LABEL[view])) as HTMLButtonElement;
}

export function buildFrame(inp: FrameInputs): Frame {
  // The top bar: the navigator's toggle (shown when the navigator is hidden), the crumb, the state, the held banner, the
  // theme, the Console button and the dock's toggle.
  const sideToggle = iconBtn('panel-left', 'Show the task list (Ctrl/Cmd+Alt+B)');
  sideToggle.classList.add('ide-tbtn');
  const rightToggle = iconBtn('panel-left', 'Show or hide the right panel (Ctrl/Cmd+Alt+J)');
  rightToggle.classList.add('ide-tbtn', 'ide-flip');
  const consoleBtn = h('button.ide-btn', { type: 'button', title: 'Back to the screen you opened the IDE from (Esc)' }, 'Console') as HTMLButtonElement;
  consoleBtn.classList.add('ide-console');
  const crumb = h('nav.ide-crumb', { 'aria-label': 'Breadcrumb' });
  const state = h('div.ide-state');
  const top = h(
    'header.ide-top',
    { 'aria-label': 'IDE header' },
    sideToggle,
    crumb,
    state,
    h('span.spacer'),
    inp.held,
    inp.theme,
    consoleBtn,
    rightToggle,
  );

  // The navigator: the body from the sidebar, with its drag edge.
  const side = h('aside.ide-threads', { 'aria-label': 'Tasks, conversations and agents' }, inp.side, inp.sideEdge);

  // The centre's head: the dot, the title, the view toggle and the pop-out to the agent page.
  const seg = Object.fromEntries(VIEWS.map((v) => [v, segButton(v)])) as Record<CentreView, HTMLButtonElement>;
  const popout = iconBtn('external-link', 'Open the agent page');
  popout.classList.add('ide-iconlink', 'ide-popout');
  const dot = h('span.ide-dot', { 'aria-hidden': 'true', 'data-status': 'idle' });
  const title = h('span.ide-ctitle');
  const head = h('header.ide-chead', null, dot, title, h('div.ide-seg', { role: 'tablist', 'aria-label': 'View' }, ...VIEWS.map((v) => seg[v])), h('span.spacer'), popout);
  const views = Object.fromEntries(VIEWS.map((v) => [v, h(`div.ide-view.ide-view-${v}`, { 'data-view': v, hidden: true })])) as unknown as Record<CentreView, HTMLElement>;
  const pane = h('div.ide-pane', null, ...VIEWS.map((v) => views[v]));

  // The drawer under the centre: the terminal and the run.
  const drawerTabs = Object.fromEntries(DRAWER.map((k) => [k, drawerTab(k)])) as Record<DrawerTab, HTMLButtonElement>;
  const drawerClose = iconBtn('x', 'Hide the drawer (Ctrl+`)');
  const drawerBody = h('div.ide-drawer-body');
  const drawer = h(
    'section.ide-drawer',
    { 'aria-label': 'Terminal and run drawer', hidden: true },
    inp.drawerEdge,
    h('div.ide-drawer-head', { role: 'tablist' }, h('div.ide-drawer-tabs', null, ...DRAWER.map((k) => drawerTabs[k])), h('span.spacer'), drawerClose),
    drawerBody,
  );

  const centreEl = h('section.ide-centre', { 'aria-label': 'Workspace' }, top, head, pane, drawer, inp.status);

  // The dock: its tabs (the diff, the files, the drawer's terminal and run, then the project's panels), and its body.
  const rightTabs = Object.fromEntries(RIGHT.map((k) => [k, dockTab(k, RIGHT_LABEL[k], 'tab')])) as Record<RightView, HTMLButtonElement>;
  const extra = Object.fromEntries(EXTRA.map((k) => [k, dockTab(k, EXTRA_LABEL[k], 'tab')])) as Record<DockExtra, HTMLButtonElement>;
  const tabStrip = h('div.ide-dtabs', { role: 'tablist' }, ...DOCK_ORDER.map((k) => (k === 'diff' || k === 'terminal' || k === 'run' ? extra[k] : rightTabs[k])));
  const rightBody = h('div.ide-dbody');
  const rightEl = h('aside.ide-dock', { 'aria-label': 'Board, files, tasks, approvals and activity' }, tabStrip, rightBody, inp.rightEdge);

  const scrim = h('div.ide-scrim', { 'aria-hidden': 'true' });
  const root = h('div.ide-frame', null, side, centreEl, rightEl, scrim);

  return {
    root,
    sideToggle,
    rightToggle,
    consoleBtn,
    crumb,
    state,
    scrim,
    side,
    centre: { el: centreEl, dot, title, seg, popout, views },
    right: { el: rightEl, tabs: rightTabs, extra, body: rightBody },
    drawer: { el: drawer, tabs: drawerTabs, body: drawerBody, close: drawerClose },
  };
}

/** Sets the breadcrumb: the parts joined by separators; the last part is the current one. */
export function setCrumb(el: HTMLElement, parts: string[]): void {
  el.replaceChildren(
    ...parts.flatMap((p, i) => {
      const last = i === parts.length - 1;
      const part = h('span.ide-crumb-part', { class: last ? 'ide-crumb-last' : '', title: p, 'aria-current': last ? 'page' : null }, p);
      return i === 0 ? [part] : [h('span.ide-sep', { 'aria-hidden': 'true' }, '/'), part];
    }),
  );
}

/** Paints a tab strip's selection (aria-selected and the `on` class) and the optional count of each tab. */
export function paintTabs<K extends string>(tabs: Partial<Record<K, HTMLButtonElement>>, current: K | null, counts: Partial<Record<K, string | null>>): void {
  for (const k of Object.keys(tabs) as K[]) {
    const el = tabs[k];
    if (!el) continue;
    const on = k === current;
    el.setAttribute('aria-selected', on ? 'true' : 'false');
    el.classList.toggle('on', on);
    el.tabIndex = on ? 0 : -1;
    const c = counts[k];
    const badge = el.querySelector<HTMLElement>('.iw-count');
    if (badge) {
      badge.textContent = c ?? '';
      badge.hidden = c === null || c === undefined || c === '';
    }
  }
}
