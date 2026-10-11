// Hash routes: `#/agents/105` → screen `agents`, params `['105']`.

export interface Route {
  screen: string;
  params: string[];
}

export function parse(hash: string): Route {
  const parts = hash.replace(/^#\/?/, '').split('/').filter(Boolean).map(decodeURIComponent);
  return { screen: parts[0] ?? 'overview', params: parts.slice(1) };
}

export function href(screen: string, ...params: (string | number)[]): string {
  return `#/${[screen, ...params.map((p) => encodeURIComponent(String(p)))].join('/')}`;
}

export function go(screen: string, ...params: (string | number)[]): void {
  location.hash = href(screen, ...params);
}

export function onRoute(fn: (route: Route) => void): void {
  window.addEventListener('hashchange', () => fn(parse(location.hash)));
  fn(parse(location.hash));
}
