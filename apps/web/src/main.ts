// The console: the shell (sidebar, top bar, banners) and the screen mount.

import { api, ApiError } from './api';
import { heldBanner, plazaControl } from './ui/plaza-control';
import './ui/theme';
import './styles/tokens.css';
import './styles/base.css';
import './styles/views.css';
import { applySidebar, sidebar, sidebarToggle } from './ui/sidebar';

import { onRoute, type Route } from './router';
import { agents, overview, startShared } from './store';
import { button, dot, modal } from './ui/components';
import { h, icon, replace } from './ui/dom';
import { SCREENS, type Cleanup } from './views';
import { signInScreen } from './views/sign-in';

const navLinks = new Map<string, HTMLElement>();
const counts = new Map<string, HTMLElement>();

// This machine's own addresses: the sign-out control is for a hub reached over the network.
const LOOPBACK = new Set(['localhost', '127.0.0.1', '[::1]']);

function showSignIn(): void {
  document.body.replaceChildren(signInScreen(() => location.reload()));
}

function signOutControl(): HTMLElement | null {
  if (LOOPBACK.has(location.hostname)) return null;
  // Asks first, as the desktop does: signing out ends this browser's session with the hub.
  const signOut = async () => {
    try {
      await api.signOut();
    } catch {
      // the session may already be gone; reload either way to show where things stand
    }
    location.reload();
  };
  return button(
    'Sign out',
    () => {
      const close = modal(
        'Sign out?',
        h('div', null, [
          h('p', null, 'You will need the owner token to sign in to this hub again.'),
          h('div.actions', null, [button('Sign out', () => void signOut(), { kind: 'primary' }), button('Cancel', () => close(), { kind: 'ghost' })]),
        ]),
      );
    },
    { kind: 'ghost', size: 'sm' },
  );
}

function topbar(title: HTMLElement): HTMLElement {
  const health = h('div.health');
  overview.watch((o) => {
    if (!o) return replace(health, h('span', null, dot('danger'), 'apid unreachable'));
    replace(
      health,
      h('span', null, dot(o.hub ? 'ok' : 'danger'), 'Hub'),
      h('span', null, dot(o.noded ? 'ok' : 'danger'), 'Node'),
      h('span', null, dot(o.agents.working ? 'info' : 'muted'), `${o.agents.working} working · ${o.agents.idle} idle`),
    );
  });
  return h('header.topbar', null, sidebarToggle(), title, h('span.spacer'), health, plazaControl(), signOutControl());
}

function banner(): HTMLElement {
  const el = h('div', { hidden: true });
  overview.watch((o) => {
    el.hidden = !o?.dev_models;
    replace(el, icon('alert'), 'Development models are enabled: the model policy is relaxed (mock and local models allowed).');
    el.className = 'banner danger';
  });
  return el;
}

function main(): void {
  startShared();
  const title = h('span.title');
  const content = h('main.view-host');
  const app = h('div.app', null, sidebar({ counts, navLinks }), h('div.main', null, topbar(title), heldBanner(), banner(), content));
  document.body.replaceChildren(app);
  applySidebar();
  // A 401 from a later call (the session expired or was revoked) swaps the console for the sign-in screen, once.
  window.addEventListener('apz-auth-required', showSignIn, { once: true });

  agents.watch((list) => {
    const working = list.filter((a) => a.status === 'working').length;
    counts.get('agents')!.textContent = list.length ? (working ? `${working}/${list.length}` : String(list.length)) : '';
  });

  let cleanup: Cleanup | undefined;
  let mounted = '';
  onRoute((route: Route) => {
    const screen = SCREENS.find((s) => s.key === route.screen) ?? SCREENS[0]!;
    for (const [key, link] of navLinks) {
      if (key === screen.key) link.setAttribute('aria-current', 'page');
      else link.removeAttribute('aria-current');
    }
    const key = `${screen.key}/${route.params.join('/')}`;
    if (key === mounted) return;
    mounted = key;
    cleanup?.();
    title.textContent = screen.label;
    document.title = `${screen.label} · AgentPlaza`;
    const root = h('div.view');
    content.replaceChildren(root);
    cleanup = screen.mount(root, route.params);
  });
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
  main();
}

void start();
