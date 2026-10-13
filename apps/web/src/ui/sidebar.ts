// The sidebar: the brand, the screen index with its counts, the connection foot and the
// resize handle. Two states (open, rail) kept in localStorage; under 900px it is an overlay
// drawer instead. `sidebarToggle()` is the button at the left of the top bar, `[` the key. The
// state lives on the root (--sidebar-w) and on .app (sb-open/sb-rail).

import '../styles/sidebar.css';

import { href } from '../router';
import { onConnection } from '../store';
import { SCREENS } from '../views';
import { brandBlock } from './brand';
import { dot } from './components';
import { h, icon, replace } from './dom';
import { themeSwitch } from './theme';

type Mode = 'open' | 'rail';
interface State {
  mode: Mode;
  width: number;
}

const KEY = 'apz.sidebar';
const MODES: Mode[] = ['open', 'rail'];
const DEFAULT_WIDTH = 248;
const MIN_WIDTH = 200;
const MAX_WIDTH = 420;
const RAIL_WIDTH = 60;
const small = matchMedia('(max-width: 899px)');

function clampWidth(w: unknown): number {
  const n = typeof w === 'number' && Number.isFinite(w) ? w : DEFAULT_WIDTH;
  return Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, Math.round(n)));
}

function load(): State {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? 'null');
    // 'hidden' was a state the toggle no longer reaches; a stored one comes back as the rail.
    const mode = raw?.mode === 'hidden' ? 'rail' : raw?.mode;
    if (raw && MODES.includes(mode)) return { mode, width: clampWidth(raw.width) };
  } catch {
    // Storage refused or corrupt: the defaults.
  }
  return { mode: 'open', width: DEFAULT_WIDTH };
}

function save(): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(state));
  } catch {
    // Not persisted; the state still holds for this page.
  }
}

let state = load();
let drawerOpen = false;
let toggleEl: HTMLButtonElement | null = null;
let backdrop: HTMLElement | null = null;
const links: { label: string; el: HTMLElement }[] = [];

function widthPx(): number {
  return state.mode === 'open' ? state.width : RAIL_WIDTH;
}

function nextLabel(): string {
  if (small.matches) return drawerOpen ? 'Close the menu' : 'Open the menu';
  return state.mode === 'open' ? 'Collapse the sidebar' : 'Expand the sidebar';
}

/** Puts the state on the page: --sidebar-w on the root, the mode class and drawer on .app. */
export function applySidebar(): void {
  document.documentElement.style.setProperty('--sidebar-w', `${widthPx()}px`);
  const app = document.querySelector<HTMLElement>('.app');
  const isSmall = small.matches;
  if (app) {
    app.classList.remove('sb-open', 'sb-rail');
    app.classList.add(`sb-${state.mode}`);
    app.classList.toggle('sb-drawer', isSmall && drawerOpen);
    if (backdrop && backdrop.parentElement !== app) app.appendChild(backdrop);
  }
  const rail = state.mode === 'rail' && !isSmall;
  for (const link of links) link.el.title = rail ? link.label : '';
  if (toggleEl) {
    const label = nextLabel();
    toggleEl.setAttribute('aria-label', label);
    toggleEl.title = label;
  }
}

function closeDrawer(): void {
  if (!drawerOpen) return;
  drawerOpen = false;
  applySidebar();
}

/** Open and rail flip; on small screens, the drawer. */
function cycle(): void {
  if (small.matches) {
    drawerOpen = !drawerOpen;
  } else {
    state.mode = state.mode === 'open' ? 'rail' : 'open';
    save();
  }
  applySidebar();
}

small.addEventListener('change', () => {
  drawerOpen = false;
  applySidebar();
});

export function sidebar(opts: { counts: Map<string, HTMLElement>; navLinks: Map<string, HTMLElement> }): HTMLElement {
  const nav = h('nav.nav', { 'aria-label': 'Screens' });
  let section = '';
  for (const s of SCREENS) {
    if (s.section !== section) {
      section = s.section;
      nav.appendChild(h('div.nav-section', null, section));
    }
    const count = h('span.count');
    const link = h('a', { href: href(s.key) }, icon(s.icon), h('span.lbl', null, s.label), count);
    opts.counts.set(s.key, count);
    opts.navLinks.set(s.key, link);
    links.push({ label: s.label, el: link });
    nav.appendChild(link);
  }
  nav.addEventListener('click', (e) => {
    if (small.matches && e.target instanceof Element && e.target.closest('a')) closeDrawer();
  });

  const foot = h('div');
  onConnection((up) =>
    replace(foot, h('span.row', null, dot(up ? 'ok' : 'danger'), h('span.lbl', null, up ? 'Live' : 'Reconnecting…'))),
  );

  // The resize handle: pointer events with capture; double-click resets the width.
  const handle = h('div.sb-resize', {
    role: 'separator',
    'aria-orientation': 'vertical',
    'aria-label': 'Resize the sidebar',
    title: 'Drag to resize · double-click to reset',
  });
  let drag: { x: number; w: number } | null = null;
  handle.addEventListener('pointerdown', (e) => {
    if (state.mode !== 'open' || small.matches || e.button !== 0) return;
    e.preventDefault();
    handle.setPointerCapture(e.pointerId);
    drag = { x: e.clientX, w: state.width };
    handle.classList.add('on');
    document.querySelector('.app')?.classList.add('sb-dragging');
  });
  handle.addEventListener('pointermove', (e) => {
    if (!drag) return;
    state.width = clampWidth(drag.w + e.clientX - drag.x);
    document.documentElement.style.setProperty('--sidebar-w', `${state.width}px`);
  });
  const endDrag = (): void => {
    if (!drag) return;
    drag = null;
    handle.classList.remove('on');
    document.querySelector('.app')?.classList.remove('sb-dragging');
    save();
  };
  handle.addEventListener('pointerup', endDrag);
  handle.addEventListener('pointercancel', endDrag);
  handle.addEventListener('dblclick', () => {
    if (state.mode !== 'open') return;
    state.width = DEFAULT_WIDTH;
    save();
    applySidebar();
  });

  backdrop = h('div.sb-backdrop', { onclick: closeDrawer });
  return h('aside.sidebar', null, brandBlock(), nav, h('div.sidebar-foot', null, themeSwitch(), foot), handle);
}

/** The button at the left of the top bar, and the `[` key, both cycle the sidebar. */
export function sidebarToggle(): HTMLButtonElement {
  const btn = h('button.btn.ghost.icon-only.sb-toggle', { type: 'button', onclick: cycle }, icon('panel-left'));
  toggleEl = btn;
  document.addEventListener('keydown', (e) => {
    if (e.key !== '[' || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.target instanceof Element && e.target.closest('input, textarea, [contenteditable]:not([contenteditable="false"])')) return;
    e.preventDefault();
    cycle();
  });
  applySidebar();
  return btn;
}

// Before the first paint: the stored width, so the column does not animate on load.
applySidebar();
