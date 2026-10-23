// Hash routes: `#/agents/105` → screen `agents`, params `['105']`. Under a project, the route carries it first:
// `#/p/pr4/agents/105` is the same screen with the console scoped to project pr4 (scope.ts).

export interface Route {
  screen: string;
  params: string[];
  /** The project the console is scoped to (`#/p/<id>/…`), or null for All projects. */
  project: string | null;
}

export function parse(hash: string): Route {
  // A query (`#/hardware-lab?model=…`) is read by its screen; the route is the part before it.
  const parts = (hash.replace(/^#\/?/, '').split('?')[0] ?? '').split('/').filter(Boolean).map(decodeURIComponent);
  if (parts[0] === 'p' && parts[1]) {
    const rest = parts.slice(2);
    return { screen: rest[0] ?? 'overview', params: rest.slice(1), project: parts[1] };
  }
  return { screen: parts[0] ?? 'overview', params: parts.slice(1), project: null };
}

/** The scope links carry: the project of the route on screen (the shell sets it on every route). */
let linkScope: string | null = null;

/** Sets the project every `href` carries from now on (null: All projects). */
export function setLinkScope(project: string | null): void {
  linkScope = project;
}

/** A link to `screen` in the console's current scope. */
export function href(screen: string, ...params: (string | number)[]): string {
  return hrefIn(linkScope, screen, ...params);
}

/** A link to `screen` scoped to `project` (null: All projects). */
export function hrefIn(project: string | null, screen: string, ...params: (string | number)[]): string {
  const path = [screen, ...params.map((p) => encodeURIComponent(String(p)))].join('/');
  return project ? `#/p/${encodeURIComponent(project)}/${path}` : `#/${path}`;
}

export function go(screen: string, ...params: (string | number)[]): void {
  location.hash = href(screen, ...params);
}

/** Screens that moved: an old hash is rewritten to its new place before the screen mounts (movedTo). */
const MOVED: Record<string, (route: Route, hash: string) => string> = {
  usage: () => '#/metrics?tab=tokens',
  // The Floor and the Setup list are one Machines screen: `#/machine/<id>` opens that machine's row.
  floor: () => href('machines'),
  machine: (route) => (route.params[0] ? href('machines', route.params[0]) : '#/machines?view=list'),
  // The agent screens are the session screens (Engine 0.6): an agent's CID opens the session on its VM, which the
  // sessions screen finds from the CID. The project of the route is kept.
  agents: (route, hash) => hrefIn(route.project, 'sessions', ...route.params) + queryOf(hash),
};

// The setup screens live in Settings (`#/settings/<group>/<screen>/…`); their old hashes, and the links the screens
// still write with href('models', …), land there with their params.
const SETTINGS_OF: Record<string, string> = {
  models: 'models', appearance: 'appearance', prompts: 'agents', skills: 'agents', recipes: 'agents', tools: 'connections',
  mcp: 'connections', platforms: 'connections', vault: 'secrets', egress: 'secrets', nodes: 'compute', 'test-lab': 'compute', workstations: 'compute',
  volumes: 'compute', e2b: 'compute', setup: 'health',
};
for (const [screen, group] of Object.entries(SETTINGS_OF)) {
  MOVED[screen] = (route, hash) => `#/settings/${group}/${screen}${route.params.length ? `/${route.params.map(encodeURIComponent).join('/')}` : ''}${queryOf(hash)}`;
}
function queryOf(hash: string): string {
  const i = hash.indexOf('?');
  return i < 0 ? '' : hash.slice(i);
}

/** Where an old hash moved to, or null when its screen has not moved. */
export function movedTo(hash: string): string | null {
  const route = parse(hash);
  const move = Object.prototype.hasOwnProperty.call(MOVED, route.screen) ? MOVED[route.screen] : undefined;
  return move ? move(route, hash) : null;
}

export function onRoute(fn: (route: Route) => void): void {
  const route = (): void => {
    const to = movedTo(location.hash);
    if (to) history.replaceState(null, '', to);
    fn(parse(location.hash));
  };
  window.addEventListener('hashchange', route);
  route();
}
