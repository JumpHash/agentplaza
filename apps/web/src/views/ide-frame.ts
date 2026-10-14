// The IDE's static chrome: the top bar, the centre's header with the view toggle, the right panel's tabs, the bottom
// terminal drawer, and the grid that places them. It only builds the elements and returns them; the controller
// (views/ide.ts) fills them and wires their events. Styles: styles/ide-shell.css (the iw-* classes).

import { brandMark } from '../ui/brand';
import { button } from '../ui/components';
import { h, icon } from '../ui/dom';

export type CentreView = 'chat' | 'terminal' | 'editor' | 'diff';
export type RightView = 'board' | 'files' | 'tasks' | 'approvals' | 'activity';
export type DrawerTab = 'terminal' | 'run';

export const CENTRE_LABEL: Record<CentreView, string> = { chat: 'Chat', terminal: 'Terminal', editor: 'Editor', diff: 'Diff' };
export const RIGHT_LABEL: Record<RightView, string> = { board: 'Board', files: 'Files', tasks: 'Tasks', approvals: 'Approvals', activity: 'Activity' };
export const DRAWER_LABEL: Record<DrawerTab, string> = { terminal: 'Terminal', run: 'Run' };

/** The parts the controller places into the frame (built elsewhere, so their own logic stays where it is). */
export interface FrameInputs {
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
  git: HTMLElement;
  scrim: HTMLElement;
  side: HTMLElement;
  centre: { el: HTMLElement; dot: HTMLElement; title: HTMLElement; seg: Record<CentreView, HTMLButtonElement>; popout: HTMLButtonElement; views: Record<CentreView, HTMLElement> };
  right: { el: HTMLElement; tabs: Record<RightView, HTMLButtonElement>; body: HTMLElement };
  drawer: { el: HTMLElement; tabs: Record<DrawerTab, HTMLButtonElement>; body: HTMLElement; close: HTMLButtonElement };
}

const VIEWS: CentreView[] = ['chat', 'terminal', 'editor', 'diff'];
const RIGHT: RightView[] = ['board', 'files', 'tasks', 'approvals', 'activity'];
const DRAWER: DrawerTab[] = ['terminal', 'run'];

/** A tab-role button for a text tab; the count (when shown) is a span the controller fills. */
function tab(label: string, key: string, role: 'tab' | 'drawer'): HTMLButtonElement {
  const el = h(`button.iw-rtab`, { type: 'button', role: 'tab', 'aria-selected': 'false', 'data-key': key, class: role === 'drawer' ? 'iw-drawer-tab' : '' }, label, h('span.iw-count', { hidden: true }));
  return el as HTMLButtonElement;
}

function segButton(view: CentreView): HTMLButtonElement {
  return h('button.iw-seg-btn', { type: 'button', role: 'tab', 'aria-selected': 'false', 'data-view': view }, icon(view === 'chat' ? 'agents' : view === 'terminal' ? 'terminal' : view === 'editor' ? 'file' : 'git'), h('span.iw-seg-label', null, CENTRE_LABEL[view])) as HTMLButtonElement;
}

export function buildFrame(inp: FrameInputs): Frame {
  const sideToggle = button('', () => undefined, { kind: 'ghost', size: 'sm', icon: 'panel-left', title: 'Show or hide the sidebar (Ctrl/Cmd+Alt+B)' }) as HTMLButtonElement;
  sideToggle.classList.add('iw-toggle');
  const rightToggle = button('', () => undefined, { kind: 'ghost', size: 'sm', icon: 'panel-left', title: 'Show or hide the right panel (Ctrl/Cmd+Alt+J)' }) as HTMLButtonElement;
  rightToggle.classList.add('iw-toggle', 'iw-flip');
  const consoleBtn = button('Console', () => undefined, { kind: 'ghost', size: 'sm', icon: 'overview', title: 'Back to the screen you opened the IDE from (Esc)' }) as HTMLButtonElement;
  consoleBtn.classList.add('iw-console');
  const crumb = h('nav.iw-crumb', { 'aria-label': 'Breadcrumb' });
  const state = h('div.iw-state');
  const git = h('div.iw-git');

  const top = h(
    'header.iw-top',
    { 'aria-label': 'IDE header' },
    h('div.iw-brand', null, brandMark(20), h('span', null, 'IDE')),
    sideToggle,
    crumb,
    state,
    h('span.spacer'),
    git,
    inp.held,
    inp.theme,
    consoleBtn,
    rightToggle,
  );

  const side = h('aside.iw-side', { 'aria-label': 'Projects and agents' }, inp.side, inp.sideEdge);

  const seg = Object.fromEntries(VIEWS.map((v) => [v, segButton(v)])) as Record<CentreView, HTMLButtonElement>;
  const popout = button('', () => undefined, { kind: 'ghost', size: 'sm', icon: 'external', title: 'Open the agent page' }) as HTMLButtonElement;
  popout.classList.add('iw-popout');
  const dot = h('span.iw-dot', { 'aria-hidden': 'true' });
  const title = h('span.iw-title');
  const head = h('header.iw-centre-head', null, dot, title, h('div.iw-seg', { role: 'tablist', 'aria-label': 'View' }, ...VIEWS.map((v) => seg[v])), h('span.spacer'), popout);
  const views = Object.fromEntries(VIEWS.map((v) => [v, h(`div.iw-view.iw-view-${v}`, { 'data-view': v, hidden: true })])) as unknown as Record<CentreView, HTMLElement>;
  const pane = h('div.iw-pane', null, ...VIEWS.map((v) => views[v]));

  const drawerTabs = Object.fromEntries(DRAWER.map((k) => [k, tab(DRAWER_LABEL[k], k, 'drawer')])) as Record<DrawerTab, HTMLButtonElement>;
  const drawerClose = button('', () => undefined, { kind: 'ghost', size: 'sm', icon: 'x', title: 'Hide the drawer (Ctrl+`)' }) as HTMLButtonElement;
  const drawerBody = h('div.iw-drawer-body');
  const drawer = h(
    'section.iw-drawer',
    { 'aria-label': 'Terminal and run drawer', hidden: true },
    inp.drawerEdge,
    h('div.iw-drawer-head', { role: 'tablist' }, h('div.iw-drawer-tabs', null, ...DRAWER.map((k) => drawerTabs[k])), h('span.spacer'), drawerClose),
    drawerBody,
  );

  const centreEl = h('section.iw-centre', { 'aria-label': 'Workspace' }, head, pane, drawer);
  const rightTabs = Object.fromEntries(RIGHT.map((k) => [k, tab(RIGHT_LABEL[k], k, 'tab')])) as Record<RightView, HTMLButtonElement>;
  const rightBody = h('div.iw-right-body');
  const rightEl = h('aside.iw-right', { 'aria-label': 'Board, files, tasks, approvals and activity' }, h('div.iw-right-tabs', { role: 'tablist' }, ...RIGHT.map((k) => rightTabs[k])), rightBody, inp.rightEdge);
  const scrim = h('div.iw-scrim', { 'aria-hidden': 'true' });
  const body = h('div.iw-body', null, side, centreEl, rightEl);
  const root = h('div.iw', null, top, body, inp.status, scrim);

  return {
    root,
    sideToggle,
    rightToggle,
    consoleBtn,
    crumb,
    state,
    git,
    scrim,
    side,
    centre: { el: centreEl, dot, title, seg, popout, views },
    right: { el: rightEl, tabs: rightTabs, body: rightBody },
    drawer: { el: drawer, tabs: drawerTabs, body: drawerBody, close: drawerClose },
  };
}

/** Sets the breadcrumb: the parts joined by separators; the last part is the current one. */
export function setCrumb(el: HTMLElement, parts: string[]): void {
  el.replaceChildren(
    ...parts.flatMap((p, i) => {
      const part = h('span.iw-crumb-part', { title: p, 'aria-current': i === parts.length - 1 ? 'page' : null }, p);
      return i === 0 ? [part] : [h('span.iw-crumb-sep', { 'aria-hidden': 'true' }, '/'), part];
    }),
  );
}

/** Paints a tab strip's selection and the optional count of each tab. */
export function paintTabs<K extends string>(tabs: Record<K, HTMLButtonElement>, current: K, counts: Partial<Record<K, string | null>>): void {
  for (const k of Object.keys(tabs) as K[]) {
    const el = tabs[k];
    const on = k === current;
    el.setAttribute('aria-selected', on ? 'true' : 'false');
    el.tabIndex = on ? 0 : -1;
    const c = counts[k];
    const badge = el.querySelector<HTMLElement>('.iw-count');
    if (badge) {
      badge.textContent = c ?? '';
      badge.hidden = c === null || c === undefined || c === '';
    }
  }
}
