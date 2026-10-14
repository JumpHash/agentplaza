// The IDE sidebar's grouping and the start choice. Pure: no DOM, no fetch, no store. An agent belongs to every project it
// leads and every project with a task it is an assignee of; the agents in none sit in "No project".
import type { Agent, Project, Task } from '../api';

export interface ProjectGroup {
  id: string | null;
  title: string;
  agents: Agent[];
}

/** The title of the group for the agents in no project. It sorts last. */
export const NO_PROJECT = 'No project';

/** Projects, most recently active first. */
export function byRecent(projects: Project[]): Project[] {
  return [...projects].sort((a, b) => (b.updated_ms || 0) - (a.updated_ms || 0));
}

/**
 * The projects an agent belongs to, most recently active first: the ones it leads, and the ones holding a task it is an
 * assignee of (it claimed the task, or it is working on it now).
 */
export function projectsOf(agent: Pick<Agent, 'cid' | 'work'>, projects: Project[], tasks: Task[]): Project[] {
  const byId = new Map(tasks.map((t) => [t.id, t]));
  const hits = projects.filter(
    (p) =>
      p.lead_cid === agent.cid ||
      p.tasks.some((id) => agent.work?.origin.task === id || byId.get(id)?.agent_cid === agent.cid),
  );
  return byRecent(hits);
}

/** The project an agent is shown under first: its most recently active one, or null. */
export function projectOf(agent: Pick<Agent, 'cid' | 'work'>, projects: Project[], tasks: Task[]): Project | null {
  return projectsOf(agent, projects, tasks)[0] ?? null;
}

/**
 * One group per project that holds an agent, most recently active first; the agents in no project go to NO_PROJECT,
 * last. An agent in several projects is listed in each. Agents are sorted by name within a group.
 */
export function groupByProject(agents: Agent[], projects: Project[], tasks: Task[]): ProjectGroup[] {
  const sorted = byRecent(projects);
  const lists = new Map<string, Agent[]>();
  const none: Agent[] = [];
  for (const agent of agents) {
    const mine = projectsOf(agent, sorted, tasks);
    if (mine.length === 0) none.push(agent);
    for (const p of mine) lists.set(p.id, [...(lists.get(p.id) ?? []), agent]);
  }
  const byName = (xs: Agent[]): Agent[] => [...xs].sort((a, b) => a.name.localeCompare(b.name));
  const groups: ProjectGroup[] = [];
  for (const p of sorted) {
    const list = lists.get(p.id);
    if (list) groups.push({ id: p.id, title: p.title, agents: byName(list) });
  }
  if (none.length > 0) groups.push({ id: null, title: NO_PROJECT, agents: byName(none) });
  return groups;
}

/** The cids of the agents whose name another agent in the list shares (case-insensitive). Their rows show the cid. */
export function sharedNames(agents: Agent[]): Set<number> {
  const key = (a: Agent): string => a.name.trim().toLowerCase();
  const count = new Map<string, number>();
  for (const a of agents) count.set(key(a), (count.get(key(a)) ?? 0) + 1);
  return new Set(agents.filter((a) => (count.get(key(a)) ?? 0) > 1).map((a) => a.cid));
}

/**
 * The agent the IDE shows when nothing else is chosen: the one selected last time if it is still listed, else the first
 * working agent in the sidebar's order (group by group, top to bottom), else the first agent there. Null with no agents.
 */
export function startAgent(groups: ProjectGroup[], last: number | null): number | null {
  const order: Agent[] = [];
  const seen = new Set<number>();
  for (const g of groups) {
    for (const a of g.agents) {
      if (seen.has(a.cid)) continue;
      seen.add(a.cid);
      order.push(a);
    }
  }
  if (last !== null && seen.has(last)) return last;
  return (order.find((a) => a.status === 'working') ?? order[0])?.cid ?? null;
}
