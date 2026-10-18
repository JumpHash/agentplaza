// The console's scope: one project, or All projects. The route carries it (`#/p/<id>/…`, router.ts), so every link the
// console draws keeps it; the choice is remembered for the next visit. Screens read it to show only that project's
// agents, tasks, board and approvals, and All projects is the whole plaza, with the bench.

import { api, type Agent, type Project } from './api';
import { hrefIn, parse, setLinkScope } from './router';
import { Cell, on } from './store';

const KEY = 'apz.scope';

/** The project the console is scoped to, or null for All projects. */
export const scope = new Cell<string | null>(null);
/** Every project, as the hub lists them; refreshed on `project` events. */
export const projects = new Cell<Project[]>([]);

/** The project remembered from the last visit, if any. */
export function remembered(): string | null {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
}

function remember(project: string | null): void {
  try {
    if (project) localStorage.setItem(KEY, project);
    else localStorage.removeItem(KEY);
  } catch {
    // no storage (a private window): the scope still follows the route
  }
}

/** Called by the shell on every route: the route's project is the scope. */
export function followRoute(project: string | null): void {
  setLinkScope(project);
  remember(project);
  if (scope.get() !== project) scope.set(project);
}

/** Switches the console to `project` (null: All projects), on the same screen. A screen's own item (one agent, one
 * task) belonged to the old scope, so the switch opens the screen itself. */
export function switchTo(project: string | null): void {
  location.hash = hrefIn(project, parse(location.hash).screen);
}

/** Whether a project still has work: not archived, not delivered. */
export function isLive(p: Pick<Project, 'archived' | 'stage'>): boolean {
  return !p.archived && p.stage !== 'delivered';
}

/** The projects the switcher offers: the live ones, newest first. */
export function liveProjects(list: Project[]): Project[] {
  return list.filter(isLive).sort((a, b) => b.created_ms - a.created_ms);
}

/** A project's title, or its id while the list has not loaded it. */
export function projectTitle(id: string | null | undefined): string {
  if (!id) return '';
  return projects.get().find((p) => p.id === id)?.title ?? id;
}

/** Whether `agent` shows in the scope: on `project`, or anywhere under All projects. The orchestrator works on every
 * project, so it shows in each. */
export function inScope(agent: Pick<Agent, 'project' | 'tier'>, project: string | null): boolean {
  return project === null || agent.project === project || agent.tier === 'orchestrator';
}

/** Whether `agent` is on the bench: no project, and not the orchestrator (which works without one). */
export function onBench(agent: Pick<Agent, 'project' | 'tier' | 'kind'>): boolean {
  return !agent.project && agent.tier !== 'orchestrator' && agent.kind !== 'model';
}

let loading: Promise<void> | null = null;

export function refreshProjects(): Promise<void> {
  loading ??= api
    .projects()
    .then((list) => {
      projects.set(list);
      // A remembered or linked project that no longer exists sends the console back to All projects.
      const current = scope.get();
      if (current && !list.some((p) => p.id === current)) switchTo(null);
    })
    .catch(() => undefined)
    .finally(() => (loading = null));
  return loading;
}

/** Loads the projects and keeps them current. */
export function startScope(): void {
  void refreshProjects();
  on('project', () => void refreshProjects());
}
