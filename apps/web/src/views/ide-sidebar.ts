// The IDE's navigator (the threads column): the head (the mark, the brand, New chat and Hide), the actions (search,
// Needs you, Console), the project's links in project mode, the list (projects with their agents and repo workspaces, the
// workstations, the chats, a hover card, keyboard movement) and the foot (the hub's status). The agents
// are grouped by project (an agent is listed under every project it leads or holds a task of); the agents in no project sit
// last in "No project", collapsed while any project exists. The workstations form their own group, after the projects and
// before No project. A terminal row shows under its agent only when a terminal exists for that workspace. It loads its own
// data: the agents from the store, projects, tasks, IDE workspaces, the workstations, the chats and the pending
// approvals, polled every 20 s. Nothing here is shared with the shell.

import { agentName } from '../roles';
import { api, ideApi, type Agent, type IdeWorkspaceInfo, type Project, type StationView, type Task } from '../api';
import { workApi, type Chat } from '../api-work';
import { openTalk } from '../ide/open';
import { hrefIn } from '../router';
import { draftPending } from './chat-model';
import { projectTitle, scope } from '../scope';
import { agents, on } from '../store';
import { openNewProject } from '../ui/new-project';
import { brandMark } from '../ui/brand';
import { ago } from '../ui/components';
import { h, lu } from '../ui/dom';
import { matchingRows, NONE_GID, sidebarGroups, type SidebarGroup } from '../ide/groups';
import { osLabel } from '../ide/model';
import { currentTask, rowStatus, type RowStatus } from '../ide/view';
import { stationBadge } from './station-machine';
import { browserSession, storedTerminalCount } from './ide-terminal-model';
import { iconBtn } from './ide-frame';

export interface SidebarOpts {
  onOpen(key: string): void;
  /** The head's Hide button and Ctrl/Cmd+Alt+B: the navigator shows or hides. */
  onHide(): void;
  /** The Console action and Escape: back to the screen the IDE was opened from. */
  onConsole(): void;
}

export interface Sidebar {
  el: HTMLElement;
  setSelected(key: string | null): void;
  /** Repaints the list now (the terminals that exist, or the selection, changed). */
  refresh(): void;
  focusSearch(): void;
  dispose(): void;
}

const POLL_MS = 20000;
const HOVER_MS = 300;
const LEAVE_MS = 100;

const STATUS_WORD: Record<RowStatus, string> = {
  working: 'Working',
  idle: 'Idle',
  sleeping: 'Sleeping',
  done: 'Done',
  failed: 'Failed',
};

/** The groups the owner opened or closed: `gid` → open. Only the choices made are kept. */
const GROUPS_KEY = 'apz.ide.groups';

/** The groups the owner has opened or closed. An old or malformed value is ignored. */
function loadOpen(): Record<string, boolean> {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(GROUPS_KEY) ?? '{}');
    if (!v || typeof v !== 'object' || Array.isArray(v)) return {};
    const out: Record<string, boolean> = {};
    for (const [gid, open] of Object.entries(v)) if (typeof open === 'boolean') out[gid] = open;
    return out;
  } catch {
    return {};
  }
}

function saveOpen(open: Record<string, boolean>): void {
  try {
    localStorage.setItem(GROUPS_KEY, JSON.stringify(open));
  } catch {
    // Storage is blocked (a private window): the state lasts for this page only.
  }
}

function isEditable(t: EventTarget | null): boolean {
  return t instanceof HTMLElement && (t.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName));
}

function cardLine(label: string, value: string): HTMLElement {
  return h('div', null, h('dt', null, label), h('dd', null, value));
}

function fillCard(el: HTMLElement, a: Agent): void {
  el.replaceChildren(
    h('div.ide-card-n', null, agentName(a)),
    h('dl.ide-card-l', null,
      cardLine('Role', a.role_label || a.role || '—'),
      cardLine('Model', a.model || '—'),
      cardLine('Task', currentTask(a) ?? '—'),
      cardLine('Active', ago(a.last_seen_ms)),
      cardLine('Status', STATUS_WORD[rowStatus(a, null)])),
  );
}

export function sidebar(opts: SidebarOpts): Sidebar {
  let projects: Project[] = [];
  let tasks: Task[] = [];
  let workspaces: IdeWorkspaceInfo[] = [];
  /** The workstations' own records: the account each one's agents act as, which the workspace list does not carry. */
  let stationViews: StationView[] = [];
  /** The owner's chats (the orchestrator's, and each project's planner's), newest first: where a project is set up. */
  let chats: Chat[] = [];
  /** The agents that wait for the owner: a pending approval. Failed agents are added when the list is drawn. */
  let waiting = new Set<number>();
  /** In project mode (the IDE opened under a project), the sidebar is that project's. */
  const inProject = scope.get();
  let selected: string | null = null;
  let query = '';
  let needsOnly = false;
  let disposed = false;
  let queued = false;
  const openGroups = loadOpen();
  /** The focusable elements of the last render, by their data-key. */
  const els = new Map<string, HTMLElement>();

  // The hover card: one at a time, for one agent row.
  let cardKey: string | null = null;
  let showTimer = 0;
  let hideTimer = 0;

  const card = h('div.ide-card', { role: 'tooltip' });
  card.hidden = true;
  document.body.append(card);

  const search = h('input.ide-search', {
    type: 'search',
    placeholder: 'Search',
    'aria-label': 'Search tasks, chats, agents, repos and workstations',
    autocomplete: 'off',
    spellcheck: false,
    oninput: () => {
      query = search.value.trim();
      schedule();
    },
    onkeydown: (e) => onSearchKey(e as KeyboardEvent),
  });

  const list = h('div.ide-list', {
    role: 'group',
    'aria-label': 'Projects and agents',
    'data-ide-list': '',
    onkeydown: (e) => onListKey(e as KeyboardEvent),
  });

  // The head: the mark (back to the console), the brand (the project in project mode), New chat and Hide.
  const brand = h('span.inav-brand', null, inProject ? projectTitle(inProject) : 'AgentPlaza');
  const head = h('div.inav-top', null,
    h('a.mark.inav-mark', { href: '#/overview', title: 'AgentPlaza: back to the console', 'aria-label': 'AgentPlaza, back to the console' }, brandMark(22)),
    brand,
    iconBtn('square-pen', 'New chat with the orchestrator', () => openNewProject()),
    iconBtn('panel-left', 'Hide the task list (Ctrl/Cmd+Alt+B)', () => opts.onHide()));

  const needsRow = h('button.inav-r', {
    type: 'button',
    'aria-pressed': 'false',
    onclick: () => {
      needsOnly = !needsOnly;
      schedule();
    },
  }, lu('circle-dot', 16), h('span.inav-l', null, 'Needs you'));
  const consoleRow = h('button.inav-r', {
    type: 'button',
    title: 'Back to the screen you opened the IDE from',
    onclick: () => opts.onConsole(),
  }, lu('layout-dashboard', 16), h('span.inav-l', null, 'Console'), h('kbd', null, 'Esc'));
  const acts = h('div.inav-acts', null, h('label.inav-search', null, lu('search', 16), search, h('kbd', null, '/')), needsRow, consoleRow);

  // In project mode: the project, and its tasks, board, work and settings in the console.
  const projectName = h('span.ide-proj-t', null, projectTitle(inProject));
  const projectHead = inProject
    ? h('div.ide-proj', null,
        projectName,
        h('span.ide-proj-l', null,
          ...(['tasks', 'board', 'work', 'previews', 'settings'] as const).map((k) =>
            h('a', { href: k === 'settings' || k === 'previews' ? hrefIn(inProject, 'projects', inProject, k) : k === 'work' ? hrefIn(inProject, 'work', inProject) : hrefIn(inProject, k) }, k.charAt(0).toUpperCase() + k.slice(1)))))
    : null;
  const listHeadText = h('span', null, '');
  const listHead = h('div.inav-lh', null, listHeadText);

  // The foot: the hub's status (the agents working).
  const hubText = h('span', null, '');
  const hub = h('a.inav-hub', { href: '#/machines', title: 'The hub and its agents' }, h('span.dotlive'), hubText);
  const foot = h('div.inav-foot', null, hub);

  const el = h('div.ide-side', null, head, acts, ...(projectHead ? [projectHead] : []), listHead, list, foot);

  /** Coalesces store and poll updates into one render per task. */
  function schedule(): void {
    if (queued) return;
    queued = true;
    queueMicrotask(() => {
      queued = false;
      if (!disposed) render();
    });
  }

  function navItems(): HTMLElement[] {
    return [...list.querySelectorAll<HTMLElement>('button[data-key]')];
  }

  function onListKey(e: KeyboardEvent): void {
    const items = navItems();
    if (!items.length) return;
    const at = items.indexOf(document.activeElement as HTMLElement);
    let to: number;
    switch (e.key) {
      case 'ArrowDown':
        to = at < 0 ? 0 : (at + 1) % items.length;
        break;
      case 'ArrowUp':
        to = at < 0 ? items.length - 1 : (at - 1 + items.length) % items.length;
        break;
      case 'Home':
        to = 0;
        break;
      case 'End':
        to = items.length - 1;
        break;
      default:
        return;
    }
    e.preventDefault();
    items[to]?.focus();
  }

  function onSearchKey(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      e.preventDefault();
      search.value = '';
      query = '';
      hideCard();
      schedule();
    } else if (e.key === 'ArrowDown') {
      const first = navItems()[0];
      if (first) {
        e.preventDefault();
        first.focus();
      }
    }
  }

  /** `/` outside a field focuses the search; Escape hides the hover card. */
  function onDocKey(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      hideCard();
      return;
    }
    if (e.key !== '/' || e.ctrlKey || e.metaKey || e.altKey || isEditable(e.target)) return;
    e.preventDefault();
    search.focus();
  }

  function agentFor(key: string): Agent | undefined {
    return agents.get().find((a) => `agent:${a.cid}` === key);
  }

  function placeCard(row: HTMLElement): void {
    const r = row.getBoundingClientRect();
    const w = card.offsetWidth;
    const height = card.offsetHeight;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let left = r.right + 8;
    if (left + w > vw - 8) left = r.left - w - 8;
    left = Math.max(8, Math.min(left, vw - w - 8));
    const top = Math.max(8, Math.min(r.top, vh - height - 8));
    card.style.left = `${left}px`;
    card.style.top = `${top}px`;
  }

  function hideCard(): void {
    clearTimeout(showTimer);
    clearTimeout(hideTimer);
    showTimer = 0;
    hideTimer = 0;
    cardKey = null;
    card.hidden = true;
  }

  function showCard(key: string): void {
    const a = agentFor(key);
    const row = els.get(key);
    if (!a || !row) return;
    cardKey = key;
    fillCard(card, a);
    card.hidden = false;
    placeCard(row);
  }

  function scheduleShow(key: string): void {
    clearTimeout(hideTimer);
    hideTimer = 0;
    if (cardKey === key && !card.hidden) return;
    if (cardKey !== key) hideCard();
    clearTimeout(showTimer);
    showTimer = window.setTimeout(() => {
      showTimer = 0;
      showCard(key);
    }, HOVER_MS);
  }

  function leaveRow(): void {
    clearTimeout(showTimer);
    showTimer = 0;
    clearTimeout(hideTimer);
    hideTimer = window.setTimeout(() => {
      hideTimer = 0;
      hideCard();
    }, LEAVE_MS);
  }

  /** After a rebuild: the card follows its row's new element, or goes when the row is gone. */
  function refreshCard(): void {
    if (!cardKey || card.hidden) return;
    const a = agentFor(cardKey);
    const row = els.get(cardKey);
    if (!a || !row) {
      hideCard();
      return;
    }
    fillCard(card, a);
    placeCard(row);
  }

  function choose(key: string): void {
    hideCard();
    opts.onOpen(key);
  }

  /** A group is open unless the owner closed it. No project is closed by default while any project exists. */
  function isOpen(g: SidebarGroup, q: string, hasProjects: boolean): boolean {
    if (q !== '') return true;
    const saved = openGroups[g.gid];
    if (saved !== undefined) return saved;
    return g.gid !== NONE_GID || !hasProjects;
  }

  function toggleGroup(g: SidebarGroup): void {
    openGroups[g.gid] = !isOpen(g, query.toLowerCase(), projects.length > 0);
    saveOpen(openGroups);
    render();
  }

  function groupHead(g: SidebarGroup, open: boolean): HTMLElement {
    return h('button.ide-grp', {
      type: 'button',
      'data-key': `g:${g.gid}`,
      'aria-expanded': String(open),
      onclick: () => toggleGroup(g),
    },
      h('span.ide-caret-g', { 'aria-hidden': 'true' }, open ? '▾' : '▸'),
      h('span.ide-grp-title', null, g.title),
      h('span.ide-grp-n', null, String(g.count)));
  }

  /** An agent's row: its status dot, its label (role and CID), its task. */
  function agentRow(a: Agent): HTMLElement {
    const key = `agent:${a.cid}`;
    const st = rowStatus(a, null);
    const task = currentTask(a);
    const name = agentName(a);
    return h('button.ide-row.ide-agent', {
      type: 'button',
      'data-key': key,
      'data-status': st,
      'aria-current': selected === key ? 'true' : null,
      onclick: () => choose(key),
      onpointerenter: () => scheduleShow(key),
      onpointerleave: () => leaveRow(),
      onfocus: (e) => {
        if ((e.currentTarget as HTMLElement).matches(':focus-visible')) scheduleShow(key);
      },
      onblur: () => leaveRow(),
    },
      h('span.ide-dot', { 'data-status': st, 'aria-label': STATUS_WORD[st], title: STATUS_WORD[st] }),
      h('span.ide-row-txt', null,
        h('span.ide-row-name', null, name),
        task ? h('span.ide-row-task', { title: task }, task) : null));
  }

  /** The agent's terminal row, under the agent: shown only while a terminal exists for its workspace. */
  function termRow(a: Agent, count: number): HTMLElement {
    const key = `term:${a.cid}`;
    const name = agentName(a);
    return h('button.ide-row.ide-term', {
      type: 'button',
      'data-key': key,
      'aria-current': selected === key ? 'true' : null,
      title: `Terminal of ${name}`,
      onclick: () => choose(key),
    },
      lu('terminal', 14),
      h('span.ide-row-name', null, count > 1 ? `${count} terminals` : 'Terminal'));
  }

  function repoRow(ws: IdeWorkspaceInfo): HTMLElement {
    const key = `ws:${ws.id}`;
    return h('button.ide-row.ide-repo', {
      type: 'button',
      'data-key': key,
      'data-status': 'repo',
      'aria-current': selected === key ? 'true' : null,
      onclick: () => choose(key),
    },
      h('span.ide-dot', { 'data-status': 'repo' }),
      h('span.ide-row-txt', null,
        h('span.ide-row-name', null, ws.name || ws.id),
        h('span.ide-row-meta', null, ws.writable ? 'Writable repo' : 'Read only repo')));
  }

  /**
   * A workstation's row: a dot (green while it is online), its name, its OS, and the warning when its agents act as the
   * owner. The account comes from the station's record; a station the record does not show has no warning.
   */
  function stationRow(ws: IdeWorkspaceInfo): HTMLElement {
    const key = `ws:${ws.id}`;
    const name = ws.name || ws.owner;
    const badge = stationViews.find((s) => s.id === ws.owner);
    const warn = badge ? stationBadge(badge) : null;
    const online = ws.status === 'online';
    return h('button.ide-row.ide-station', {
      type: 'button',
      'data-key': key,
      'data-status': online ? 'online' : 'offline',
      'aria-current': selected === key ? 'true' : null,
      title: `Workstation ${name}`,
      onclick: () => choose(key),
    },
      h('span.ide-dot', { 'data-status': online ? 'online' : 'offline', 'aria-label': online ? 'Online' : 'Offline', title: online ? 'Online' : 'Offline' }),
      h('span.ide-row-txt', null,
        h('span.ide-row-name', null, name),
        h('span.ide-row-meta', null, osLabel(ws.os)),
        warn ? h('span.ide-warn', { title: 'The agents work in the owner’s own account on this machine' }, warn) : null));
  }

  /** The chats: the newest few, and the project's own chat in project mode. */
  function chatRows(q: string): HTMLElement[] {
    const shown = chats
      .filter((c) => !inProject || c.project === inProject)
      .filter((c) => !q || c.title.toLowerCase().includes(q))
      .slice(0, 6);
    if (!shown.length) return [];
    return [
      h('div.ide-subhead', null, 'Chats'),
      ...shown.map((c) =>
        h('button.ide-row.ide-conv', {
          type: 'button',
          'data-key': `chat:${c.id}`,
          'data-status': 'idle',
          title: c.title,
          onclick: () => openTalk(c.id),
        },
          h('span.ide-dot', { 'data-status': 'idle' }),
          h('span.ide-row-txt', null,
            h('span.ide-row-name', null, c.title),
            h('span.ide-row-meta', null,
              draftPending(c) && c.draft ? `draft · revision ${c.draft.revision}` : c.kind === 'project' && c.project ? projectTitle(c.project) : ago(c.updated_ms))))),
    ];
  }

  /** The agents that need the owner now: a pending approval. */
  function needsCids(_list: Agent[]): Set<number> {
    return new Set<number>(waiting);
  }

  /** Rebuilds the list, keeping the focused row focused by its key. */
  function render(): void {
    const active = document.activeElement;
    const focusKey = active instanceof HTMLElement && list.contains(active) ? active.dataset.key : undefined;
    const q = query.toLowerCase();
    const hasProjects = projects.length > 0;
    const roster = agents.get();
    const needs = needsCids(roster);
    // The title, once the project list has it (the header is built before the list loads).
    if (inProject) {
      const title = projects.find((p) => p.id === inProject)?.title ?? projectTitle(inProject);
      projectName.textContent = title;
      brand.textContent = title;
    }
    const working = roster.filter((a) => a.status === 'working').length;
    hubText.textContent = working > 0 ? `${working} working` : 'Idle';
    needsRow.setAttribute('aria-pressed', String(needsOnly));
    needsRow.title = needsOnly ? 'Show everything again' : 'Show only what waits for you';
    needsRow.querySelector('.count')?.remove();
    if (needs.size > 0) needsRow.append(h('span.count', { class: needsOnly ? '' : 'hot' }, String(needs.size)));
    listHeadText.textContent = needsOnly ? 'Needs you' : inProject ? 'Project' : 'By project';
    const nodes: HTMLElement[] = needsOnly ? [] : [...chatRows(q)];
    for (const g of sidebarGroups(roster, projects, tasks, workspaces)) {
      // In project mode, only the project's own group (its agents and its repositories).
      if (inProject && g.gid !== `p:${inProject}`) continue;
      const v = matchingRows(g, q);
      if (!v) continue;
      const agentsShown = needsOnly ? v.agents.filter((a) => needs.has(a.cid)) : v.agents;
      const reposShown = needsOnly ? [] : v.repos;
      const stationsShown = needsOnly ? [] : v.stations;
      if (needsOnly && !agentsShown.length) continue;
      const open = needsOnly || isOpen(g, q, hasProjects);
      nodes.push(groupHead(g, open));
      if (!open) continue;
      for (const a of agentsShown) {
        nodes.push(agentRow(a));
        const terminals = needsOnly ? 0 : storedTerminalCount(browserSession(), `agent:${a.cid}`);
        if (terminals > 0) nodes.push(termRow(a, terminals));
      }
      for (const ws of reposShown) nodes.push(repoRow(ws));
      for (const ws of stationsShown) nodes.push(stationRow(ws));
    }
    if (!nodes.length) {
      nodes.push(h('p.ide-empty-list', null,
        needsOnly ? 'Nothing waits for you.' : q ? 'No matches.' : inProject ? 'Nobody works on this project yet.' : 'Nothing yet. Start with a chat: New chat, above.'));
    }
    els.clear();
    for (const n of nodes) if (n.dataset.key) els.set(n.dataset.key, n);
    list.replaceChildren(...nodes);
    if (focusKey) els.get(focusKey)?.focus({ preventScroll: true });
    refreshCard();
  }

  async function load(): Promise<void> {
    const [p, w, t, s, c, ap] = await Promise.all([
      api.projects().catch(() => null),
      ideApi.workspaces().catch(() => null),
      api.tasks().catch(() => null),
      api.stations().catch(() => null),
      workApi.chats().catch(() => null),
      api.approvals().catch(() => null),
    ]);
    if (disposed) return;
    if (p) projects = p;
    if (w) workspaces = w;
    if (t) tasks = t;
    if (s) stationViews = s;
    if (c) chats = c;
    if (ap) waiting = new Set(ap.filter((a) => a.kind === 'tool' && a.state === 'pending').map((a) => a.cid));
    schedule();
  }

  const unwatch = agents.watch(() => schedule());
  const offChat = on('chat', () => void workApi.chats().then((c) => {
    if (disposed) return;
    chats = c;
    schedule();
  }).catch(() => undefined));
  document.addEventListener('keydown', onDocKey);
  void load();
  const poll = window.setInterval(() => void load(), POLL_MS);

  return {
    el,
    setSelected(key) {
      selected = key;
      schedule();
    },
    refresh() {
      schedule();
    },
    focusSearch() {
      search.focus();
    },
    dispose() {
      disposed = true;
      unwatch();
      offChat();
      window.clearInterval(poll);
      hideCard();
      document.removeEventListener('keydown', onDocKey);
      card.remove();
      el.remove();
    },
  };
}
