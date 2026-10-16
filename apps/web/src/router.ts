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

/** Screens that moved: an old hash is rewritten to its new place before the screen mounts. */
const MOVED: Record<string, string> = { usage: '#/metrics?tab=tokens' };

export function onRoute(fn: (route: Route) => void): void {
  const route = (): void => {
    const to = MOVED[parse(location.hash).screen];
    if (to) history.replaceState(null, '', to);
    fn(parse(location.hash));
  };
  window.addEventListener('hashchange', route);
  route();
}
