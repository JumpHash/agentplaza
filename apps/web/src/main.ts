// The console: the shell (sidebar, top bar, banners) and the screen mount.

import './styles/tokens.css';
import './styles/base.css';
import './styles/views.css';

import { onRoute, href, type Route } from './router';
import { agents, onConnection, overview, startShared } from './store';
import { dot } from './ui/components';
import { h, icon, replace } from './ui/dom';
import { SCREENS, type Cleanup } from './views';

const navLinks = new Map<string, HTMLAnchorElement>();
const counts = new Map<string, HTMLElement>();

function sidebar(): HTMLElement {
  const nav = h('nav.nav', { 'aria-label': 'Screens' });
  let section = '';
  SCREENS.forEach((s, i) => {
    if (s.section !== section) {
      section = s.section;
      nav.appendChild(h('div.nav-section', null, section));
    }
    const count = h('span.count');
    counts.set(s.key, count);
    const link = h('a', { href: href(s.key) }, h('span.k', null, String(i + 1)), s.label, h('span.dots'), count);
    navLinks.set(s.key, link);
    nav.appendChild(link);
  });
  const foot = h('div.sidebar-foot');
  onConnection((up) => replace(foot, h('span.row', null, dot(up ? 'ok' : 'danger'), up ? 'Live' : 'Reconnecting…')));
  const brand = h('div.brand', null, h('span.brand-mark'), 'AgentPlaza', h('small', null, 'the plaza register'));
  return h('aside.sidebar', null, brand, nav, foot);
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
  return h('header.topbar', null, title, h('span.spacer'), health);
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
  const app = h('div.app', null, sidebar(), h('div.main', null, topbar(title), banner(), content));
  document.body.replaceChildren(app);

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

main();
