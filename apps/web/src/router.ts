// Hash routes: `#/agents/105` → screen `agents`, params `['105']`.

export interface Route {
  screen: string;
  params: string[];
}

export function parse(hash: string): Route {
  // A query (`#/hardware-lab?model=…`) is read by its screen; the route is the part before it.
  const parts = (hash.replace(/^#\/?/, '').split('?')[0] ?? '').split('/').filter(Boolean).map(decodeURIComponent);
  return { screen: parts[0] ?? 'overview', params: parts.slice(1) };
}

export function href(screen: string, ...params: (string | number)[]): string {
  return `#/${[screen, ...params.map((p) => encodeURIComponent(String(p)))].join('/')}`;
}

export function go(screen: string, ...params: (string | number)[]): void {
  location.hash = href(screen, ...params);
}

/** Screens that moved: an old hash is rewritten to its new place before the screen mounts (movedTo). */
const MOVED: Record<string, (route: Route) => string> = {
  usage: () => '#/metrics?tab=tokens',
  // The Floor and the Setup list are one Machines screen: `#/machine/<id>` opens that machine's row.
  floor: () => href('machines'),
  machine: (route) => (route.params[0] ? href('machines', route.params[0]) : '#/machines?view=list'),
};

/** Where an old hash moved to, or null when its screen has not moved. */
export function movedTo(hash: string): string | null {
  const route = parse(hash);
  const move = Object.prototype.hasOwnProperty.call(MOVED, route.screen) ? MOVED[route.screen] : undefined;
  return move ? move(route) : null;
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
