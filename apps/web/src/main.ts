// The console: the shell (sidebar, top bar, banners) and the screen mount.

import { agentName } from './roles';
import { api, ApiError } from './api';
// The plaza control's styles load with the console (the IDE's held strip uses them too), so the CSS is imported here, not
// in the module: a lazy screen that imports the module would otherwise split the styles into a stylesheet of its own.
import './styles/plaza-control.css';
import { heldBanner } from './ui/plaza-control';
import { startAppearance } from './ui/theme';
import { startBackdrop } from './view/backdrop';
import './styles/tokens.css';
import './styles/scope.css';
import './styles/proto/base.css';
import './styles/proto/skins.css';
import './styles/proto/shell.css';
import { appRoot, frameRoute, navLabel, mountAmbient, pageFor, setBanners, settingsCrumbs, settingsPage, SETTINGS, startShell, type Crumbs } from './ui/shell';

import { hrefIn, onRoute, type Route } from './router';
import { followRoute, projects, remembered, startScope } from './scope';
import { noteConsoleRoute, openIde } from './ide/open';
import { agents, overview, startShared } from './store';
import { h, icon, replace } from './ui/dom';
import { SCREENS, type Cleanup } from './views';
import { runFirstStart } from './ui/onboarding';
import { signInScreen } from './views/sign-in';

// This machine's own addresses: signing out is for a hub reached over the network (the shell's owner menu).
function showSignIn(): void {
  document.body.replaceChildren(signInScreen(() => location.reload()));
}

/** The banner while development models are on. */
function devBanner(): HTMLElement {
  const el = h('div.banner.danger', { hidden: true });
  overview.watch((o) => {
    el.hidden = !o?.dev_models;
    replace(el, icon('alert'), h('span', null, 'Development models are enabled: the model policy is relaxed (mock and local models allowed).'));
  });
  return el;
}

/** The setup screens hold the plaza's settings, which a project overrides in its own: under a project, they say so. */
const PLAZA_SETTINGS = new Set(['models', 'tools', 'mcp', 'prompts', 'skills', 'rules', 'vault', 'egress']);

function plazaSettingsNote(project: string): HTMLElement {
  return h(
    'div.banner.scope-note',
    null,
    icon('settings'),
    h('span', null, 'These are the plaza\'s settings, which every project starts from. '),
    h('a', { href: hrefIn(project, 'projects', project, 'settings') }, 'This project\'s own settings'),
    h('span', null, ' override some of them.'),
  );
}

/** Screens that draw their own page (Overview's hero) or take the whole column. */
const LAYOUT: Record<string, 'bare' | 'full'> = { overview: 'bare' };

function crumbsOf(route: Route, screenLabel: string): Crumbs {
  const label = navLabel(route.screen) ?? screenLabel;
  const [a] = route.params;
  if (route.screen === 'projects' && a) {
    const p = projects.get().find((x) => x.id === a) ?? null;
    return { mono: p, parts: [p ? p.title : a] };
  }
  if (route.screen === 'sessions' && a) {
    const ag = agents.get().find((x) => String(x.cid) === a);
    return { parts: ['Sessions', ag ? agentName(ag) : a] };
  }
  if (a && ['machines', 'history', 'tasks'].includes(route.screen)) return { parts: [label, a] };
  return { parts: [label] };
}

function main(): void {
  startShared();
  startScope();
  // A visit to the console's front door opens the project chosen last time.
  const last = remembered();
  if (last && (!location.hash || location.hash === '#' || location.hash === '#/')) history.replaceState(null, '', hrefIn(last, 'overview'));

  // The IDE is a view of its own: its host is the whole window, and the console shell (.app) is hidden while it shows.
  const ideHost = h('div.ide-root', { hidden: true });
  const appEl = appRoot();
  document.body.replaceChildren(appEl, ideHost);
  mountAmbient();
  startShell();
  setBanners([heldBanner(), devBanner()]);
  // Ctrl+Shift+E (Cmd+Shift+E on a Mac) opens the IDE. Not while the owner types in a field or in the editor.
  document.addEventListener('keydown', (e) => {
    if (!(e.ctrlKey || e.metaKey) || !e.shiftKey || e.altKey || e.key.toLowerCase() !== 'e') return;
    const target = e.target as Element | null;
    if (target?.closest?.('input, textarea, select, [contenteditable]:not([contenteditable="false"]), .cm-editor')) return;
    e.preventDefault();
    openIde('');
  });
  // A 401 from a later call (the session expired or was revoked) swaps the console for the sign-in screen, once.
  window.addEventListener('apz-auth-required', showSignIn, { once: true });

  let cleanup: Cleanup | undefined;
  let mounted = '';
  onRoute((route: Route) => {
    followRoute(route.project);
    // Settings: `#/settings/<group>/<screen>/…` mounts that screen beside the settings index.
    const inSettings = route.screen === 'settings';
    const group = inSettings ? (route.params[0] ?? 'models') : '';
    const settingsKey = inSettings ? (route.params[1] ?? SETTINGS.find((g) => g[0] === group)?.[2][0]?.[0] ?? 'models') : '';
    const screen = SCREENS.find((s) => s.key === (inSettings ? settingsKey : route.screen)) ?? SCREENS[0]!;
    const params = inSettings ? route.params.slice(2) : route.params;
    const ide = screen.key === 'ide';
    if (!ide) noteConsoleRoute(location.hash);
    appEl.hidden = ide;
    ideHost.hidden = !ide;
    const crumbs = inSettings ? settingsCrumbs(group, settingsKey) : crumbsOf(route, screen.label);
    frameRoute(route, crumbs);
    // A switch of project remounts the screen: it shows another project's things.
    const key = `${route.project ?? ''}|${inSettings ? `settings/${group}/` : ''}${screen.key}/${params.join('/')}`;
    if (key === mounted) return;
    mounted = key;
    cleanup?.();
    document.title = `${crumbs.parts[crumbs.parts.length - 1] ?? screen.label} · AgentPlaza`;
    if (ide) {
      // The IDE mounts into the window itself, with no console content beside it.
      ideHost.replaceChildren();
      cleanup = screen.mount(ideHost, params);
      return;
    }
    ideHost.replaceChildren();
    const page = inSettings ? settingsPage(group, settingsKey) : pageFor(route, LAYOUT[screen.key] ?? 'page');
    const note = route.project && PLAZA_SETTINGS.has(screen.key) ? plazaSettingsNote(route.project) : null;
    const root = h('div.view');
    page.replaceChildren(...(note ? [note, root] : [root]));
    cleanup = screen.mount(root, params);
  });
  // The wizard and the tour: once, on the first start.
  runFirstStart();
}

// Off loopback the hub wants the owner's sign-in first; the auth check decides the first screen.
async function start(): Promise<void> {
  try {
    await api.authCheck();
  } catch (err) {
    if (err instanceof ApiError && err.status === 401) {
      showSignIn();
      return;
    }
    // any other failure: the console shows its own offline state
  }
  // The hub's appearance (and this device's override): boot-theme.js painted the cached one already.
  void startAppearance();
  main();
  // The theme's background, drawn by the same shader as the desktop's (it paints the prototype's fields, pattern and
  // grain; the ambient layer's CSS draws them only when no GPU path is there). It loads after the console is up.
  const idle = (window as Window & { requestIdleCallback?: (f: () => void) => void }).requestIdleCallback ?? ((f: () => void) => setTimeout(f, 200));
  idle(() => startBackdrop(document.body));
}

void start();
