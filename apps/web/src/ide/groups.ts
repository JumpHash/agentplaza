// The IDE sidebar's grouping and the start choice. Pure: no DOM, no fetch, no store. An agent sits under the project of its
// live session. With none, it sits under the projects it leads and those holding a task it is assigned or working on; the
// rest sit in "No project".
import { agentName } from '../roles';
import type { Agent, IdeWorkspaceInfo, Project, Task } from '../api';
import { currentTask } from './view';

export interface ProjectGroup {
  id: string | null;
  title: string;
  agents: Agent[];
}

/** The title of the group for the agents in no project. It sorts last. */
export const NO_PROJECT = 'No project';
/** The titles of the groups that are not a project: the repositories no project holds, and the workstations. */
export const REPOSITORIES = 'Repositories';
export const WORKSTATIONS = 'Workstations';
/** The ids of the groups that are not a project: the repositories, the workstations, and the agents in no project. */
export const REPOSITORIES_GID = 'repositories';
export const WORKSTATIONS_GID = 'workstations';
export const NONE_GID = 'none';

/** One group as the sidebar shows it: its agents, the repo workspaces filed under its project, and the workstations. */
export interface SidebarGroup {
  gid: string;
  title: string;
  agents: Agent[];
  repos: IdeWorkspaceInfo[];
  stations: IdeWorkspaceInfo[];
  count: number;
}

/** Projects, most recently active first. */
export function byRecent(projects: Project[]): Project[] {
  return [...projects].sort((a, b) => (b.updated_ms || 0) - (a.updated_ms || 0));
}

/**
 * The projects an agent belongs to, most recently active first: the ones it leads, and the ones holding a task it is an
 * assignee of (it claimed the task, or it is working on it now).
 */
export function projectsOf(agent: Pick<Agent, 'cid' | 'work' | 'project'>, projects: Project[], tasks: Task[]): Project[] {
  // An agent on a project sits under that project only.
  if (agent.project) return projects.filter((p) => p.id === agent.project);
  const byId = new Map(tasks.map((t) => [t.id, t]));
  const hits = projects.filter(
    (p) =>
      p.lead_cid === agent.cid ||
      p.tasks.some((id) => agent.work?.origin.task === id || byId.get(id)?.agent_cid === agent.cid),
  );
  return byRecent(hits);
}

/** The project an agent is shown under first: its most recently active one, or null. */
export function projectOf(agent: Pick<Agent, 'cid' | 'work' | 'project'>, projects: Project[], tasks: Task[]): Project | null {
  return projectsOf(agent, projects, tasks)[0] ?? null;
}

/**
 * One group per project that holds an agent, most recently active first; the agents in no project go to NO_PROJECT,
 * last. An agent in several projects is listed in each. Agents are sorted by their labels within a group.
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
  const byLabel = (xs: Agent[]): Agent[] => [...xs].sort((a, b) => agentName(a).localeCompare(agentName(b), undefined, { numeric: true }));
  const groups: ProjectGroup[] = [];
  for (const p of sorted) {
    const list = lists.get(p.id);
    if (list) groups.push({ id: p.id, title: p.title, agents: byLabel(list) });
  }
  if (none.length > 0) groups.push({ id: null, title: NO_PROJECT, agents: byLabel(none) });
  return groups;
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

/** The names a repo workspace answers to: its name, and the name inside its id `repo:<name>@<ref>`. */
function repoNames(ws: IdeWorkspaceInfo): string[] {
  const fromId = /^repo:([^@]+)/.exec(ws.id)?.[1] ?? '';
  return [ws.name, fromId].filter(Boolean).map((s) => s.toLowerCase());
}

/**
 * The sidebar's groups: the projects with agents (most recently active first), each with the repo workspaces of its repo;
 * then the repositories no project holds; then the workstations; then No project, last. A workstation is never filed under
 * a project: a station is not linked to one.
 */
export function sidebarGroups(agents: Agent[], projects: Project[], tasks: Task[], workspaces: IdeWorkspaceInfo[]): SidebarGroup[] {
  const grouped = groupByProject(agents, projects, tasks);
  const byProject = new Map<string, SidebarGroup>();
  for (const g of grouped) {
    if (g.id === null) continue;
    byProject.set(g.id, { gid: `p:${g.id}`, title: g.title, agents: g.agents, repos: [], stations: [], count: g.agents.length });
  }
  const orphans: IdeWorkspaceInfo[] = [];
  for (const ws of workspaces) {
    if (ws.kind !== 'repo') continue;
    const names = repoNames(ws);
    const proj = projects.find((p) => p.repo && names.includes(p.repo.toLowerCase()));
    if (!proj) {
      orphans.push(ws);
      continue;
    }
    let group = byProject.get(proj.id);
    if (!group) {
      group = { gid: `p:${proj.id}`, title: proj.title, agents: [], repos: [], stations: [], count: 0 };
      byProject.set(proj.id, group);
    }
    group.repos.push(ws);
  }
  const stations = workspaces.filter((ws) => ws.kind === 'station');
  const out: SidebarGroup[] = [];
  for (const p of byRecent(projects)) {
    const g = byProject.get(p.id);
    if (g) out.push(g);
  }
  if (orphans.length) out.push({ gid: REPOSITORIES_GID, title: REPOSITORIES, agents: [], repos: orphans, stations: [], count: orphans.length });
  if (stations.length) out.push({ gid: WORKSTATIONS_GID, title: WORKSTATIONS, agents: [], repos: [], stations, count: stations.length });
  const none = grouped.find((g) => g.id === null);
  if (none) out.push({ gid: NONE_GID, title: NO_PROJECT, agents: none.agents, repos: [], stations: [], count: none.agents.length });
  return out;
}

/**
 * The rows of a group that match the lower-cased query (name, group title, current task; a workstation by its name); an
 * empty query keeps all. Null when no row matches.
 */
export function matchingRows(g: SidebarGroup, q: string): { agents: Agent[]; repos: IdeWorkspaceInfo[]; stations: IdeWorkspaceInfo[] } | null {
  if (!q) return { agents: g.agents, repos: g.repos, stations: g.stations };
  const hit = (s: string | null | undefined) => (s ?? '').toLowerCase().includes(q);
  const whole = hit(g.title);
  const agents = g.agents.filter((a) => whole || hit(agentName(a)) || hit(currentTask(a)));
  const repos = g.repos.filter((ws) => whole || hit(ws.name));
  const stations = g.stations.filter((ws) => whole || hit(ws.name));
  return agents.length || repos.length || stations.length ? { agents, repos, stations } : null;
}
