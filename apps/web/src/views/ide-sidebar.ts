// The IDE's left sidebar: search, the projects with their agents and repo
// workspaces, a hover card, keyboard movement and the pinned New agent button.
// It loads its own data: the agents Cell from the store, and projects and IDE
// workspaces polled every 20 s. Nothing here is shared with the shell.

import css from '../styles/ide-sidebar.css';
import { api, ideApi, type Agent, type IdeWorkspaceInfo, type Project } from '../api';
import { agents } from '../store';
import { ago } from '../ui/components';
import { h, icon } from '../ui/dom';
import { currentTask, groupByProject, rowStatus, type RowStatus } from '../ide/view';

export interface SidebarOpts {
  onOpen(key: string): void;
  onNewAgent(): void;
}

export interface Sidebar {
  el: HTMLElement;
  setSelected(key: string | null): void;
  focusSearch(): void;
  dispose(): void;
}

const STYLE_ID = 'apz-ide-sidebar-style';
const GROUPS_KEY = 'apz.ide.groups';
const POLL_MS = 20000;
const HOVER_MS = 300;
const LEAVE_MS = 100;
const UNASSIGNED = 'unassigned';
const REPOSITORIES = 'repositories';

const STATUS_WORD: Record<RowStatus, string> = {
  working: 'Working',
  idle: 'Idle',
  sleeping: 'Sleeping',
  done: 'Done',
  failed: 'Failed',
};

/** A group as the sidebar shows it: its agents, and the repo workspaces filed under its project. */
interface GroupView {
  gid: string;
  title: string;
  agents: Agent[];
  repos: IdeWorkspaceInfo[];
  count: number;
}

function injectStyle(): void {
  if (document.getElementById(STYLE_ID)) return;
  const style = document.createElement('style');
  style.id = STYLE_ID;
  style.textContent = css;
  document.head.append(style);
}

function loadCollapsed(): Set<string> {
  try {
    const list: unknown = JSON.parse(localStorage.getItem(GROUPS_KEY) ?? '[]');
    return new Set(Array.isArray(list) ? list.filter((x): x is string => typeof x === 'string') : []);
  } catch {
    return new Set();
  }
}

function saveCollapsed(collapsed: Set<string>): void {
  try {
    localStorage.setItem(GROUPS_KEY, JSON.stringify([...collapsed]));
  } catch {
    // Storage is blocked (a private window): the state lasts for this page only.
  }
}

/** The names a repo workspace answers to: its name, and the name inside its id `repo:<name>@<ref>`. */
function repoNames(ws: IdeWorkspaceInfo): string[] {
  const fromId = /^repo:([^@]+)/.exec(ws.id)?.[1] ?? '';
  return [ws.name, fromId].filter(Boolean).map((s) => s.toLowerCase());
}

/** Groups the agents by project, then files each repo workspace under the project whose repo it is. */
function groupsFor(agentList: Agent[], projects: Project[], workspaces: IdeWorkspaceInfo[]): GroupView[] {
  const groups: GroupView[] = groupByProject(agentList, projects).map((g) => ({
    gid: g.id ? `p:${g.id}` : UNASSIGNED,
    title: g.title,
    agents: g.agents,
    repos: [],
    count: g.agents.length,
  }));
  const orphans: IdeWorkspaceInfo[] = [];
  for (const ws of workspaces) {
    if (ws.kind !== 'repo') continue;
    const names = repoNames(ws);
    const proj = projects.find((p) => p.repo && names.includes(p.repo.toLowerCase()));
    if (!proj) {
      orphans.push(ws);
      continue;
    }
    const gid = `p:${proj.id}`;
    let group = groups.find((g) => g.gid === gid);
    if (!group) {
      group = { gid, title: proj.title, agents: [], repos: [], count: 0 };
      groups.push(group);
    }
    group.repos.push(ws);
  }
  if (orphans.length) {
    groups.push({ gid: REPOSITORIES, title: 'Repositories', agents: [], repos: orphans, count: orphans.length });
  }
  return groups;
}

/** The rows of a group that match the lower-cased query (name, project title, current task); an empty query keeps all. */
function matching(g: GroupView, q: string): { agents: Agent[]; repos: IdeWorkspaceInfo[] } | null {
  if (!q) return { agents: g.agents, repos: g.repos };
  const hit = (s: string | null | undefined) => (s ?? '').toLowerCase().includes(q);
  const whole = hit(g.title);
  const agentsHit = g.agents.filter((a) => whole || hit(a.name) || hit(currentTask(a)));
  const reposHit = g.repos.filter((ws) => whole || hit(ws.name));
  return agentsHit.length || reposHit.length ? { agents: agentsHit, repos: reposHit } : null;
}

function isEditable(t: EventTarget | null): boolean {
  return t instanceof HTMLElement && (t.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName));
}

function mark(st: RowStatus): HTMLElement {
  const inner = st === 'sleeping' ? icon('moon') : st === 'done' ? icon('check') : h('span.ide-side-dot');
  return h('span.ide-side-mark', null, inner);
}

function cardLine(label: string, value: string): HTMLElement {
  return h('div.ide-side-card-line', null,
    h('span.ide-side-card-k', null, label),
    h('span.ide-side-card-v', null, value));
}

function fillCard(el: HTMLElement, a: Agent): void {
  el.replaceChildren(
    h('div.ide-side-card-name', null, a.name || `Agent ${a.cid}`),
    cardLine('Role', a.role_label || a.role || '—'),
    cardLine('Model', a.model || '—'),
    cardLine('Task', currentTask(a) ?? '—'),
    cardLine('Active', ago(a.last_seen_ms)),
    cardLine('Status', STATUS_WORD[rowStatus(a, null)]),
  );
}

export function sidebar(opts: SidebarOpts): Sidebar {
  injectStyle();

  let projects: Project[] = [];
  let workspaces: IdeWorkspaceInfo[] = [];
  let selected: string | null = null;
  let query = '';
  let disposed = false;
  let queued = false;
  const collapsed = loadCollapsed();
  /** The focusable elements of the last render, by their data-key. */
  const els = new Map<string, HTMLElement>();

  // The hover card: one at a time, for one agent row.
  let cardKey: string | null = null;
  let showTimer = 0;
  let hideTimer = 0;

  const card = h('div.ide-side-card', { role: 'tooltip' });
  card.hidden = true;
  document.body.append(card);

  const search = h('input.ide-side-search', {
    type: 'search',
    placeholder: 'Search',
    'aria-label': 'Search agents, projects and repos',
    autocomplete: 'off',
    spellcheck: false,
    oninput: () => {
      query = search.value.trim();
      schedule();
    },
    onkeydown: (e) => onSearchKey(e as KeyboardEvent),
  });

  const list = h('div.ide-side-list', {
    role: 'group',
    'aria-label': 'Projects and agents',
    onkeydown: (e) => onListKey(e as KeyboardEvent),
  });

  const newBtn = h('button.ide-side-new', {
    type: 'button',
    onclick: () => opts.onNewAgent(),
  }, '+ New agent');

  const el = h('div.ide-side', null,
    h('div.ide-side-top', null, search),
    list,
    h('div.ide-side-foot', null, newBtn));

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

  function toggleGroup(gid: string): void {
    if (collapsed.has(gid)) collapsed.delete(gid);
    else collapsed.add(gid);
    saveCollapsed(collapsed);
    render();
  }

  function showsTerminal(a: Agent): boolean {
    return rowStatus(a, null) === 'working'
      || selected === `agent:${a.cid}`
      || selected === `term:${a.cid}`;
  }

  function groupHead(g: GroupView, open: boolean): HTMLElement {
    return h('button.ide-side-group', {
      type: 'button',
      'data-key': `g:${g.gid}`,
      'aria-expanded': String(open),
      onclick: () => toggleGroup(g.gid),
    },
      h('span.ide-side-caret', { 'aria-hidden': 'true' }, open ? '▾' : '▸'),
      h('span.ide-side-gtitle', null, g.title),
      h('span.ide-side-count', null, String(g.count)));
  }

  function agentRow(a: Agent): HTMLElement {
    const key = `agent:${a.cid}`;
    const st = rowStatus(a, null);
    const task = currentTask(a);
    const name = a.name || `Agent ${a.cid}`;
    return h('button.ide-side-row', {
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
      mark(st),
      h('span.ide-side-body', null,
        h('span.ide-side-name', null, name),
        task ? h('span.ide-side-task', { title: task }, task) : null),
      h('span.ide-side-sr', null, `, ${STATUS_WORD[st]}`));
  }

  function termRow(a: Agent): HTMLElement {
    const key = `term:${a.cid}`;
    return h('button.ide-side-term', {
      type: 'button',
      'data-key': key,
      'aria-current': selected === key ? 'true' : null,
      title: `Terminal of ${a.name || `Agent ${a.cid}`}`,
      onclick: () => choose(key),
    }, 'Terminal');
  }

  function repoRow(ws: IdeWorkspaceInfo): HTMLElement {
    const key = `ws:${ws.id}`;
    return h('button.ide-side-row', {
      type: 'button',
      'data-key': key,
      'data-status': 'repo',
      'aria-current': selected === key ? 'true' : null,
      onclick: () => choose(key),
    },
      h('span.ide-side-mark', null, h('span.ide-side-dot')),
      h('span.ide-side-body', null,
        h('span.ide-side-name', null, ws.name || ws.id),
        h('span.ide-side-meta', null, ws.writable ? 'Writable repo' : 'Read only repo')));
  }

  /** Rebuilds the list, keeping the focused row focused by its key and the open groups open. */
  function render(): void {
    const active = document.activeElement;
    const focusKey = active instanceof HTMLElement && list.contains(active) ? active.dataset.key : undefined;
    const q = query.toLowerCase();
    const nodes: HTMLElement[] = [];
    for (const g of groupsFor(agents.get(), projects, workspaces)) {
      const v = matching(g, q);
      if (!v) continue;
      const open = q !== '' || !collapsed.has(g.gid);
      nodes.push(groupHead(g, open));
      if (!open) continue;
      for (const a of v.agents) {
        nodes.push(agentRow(a));
        if (showsTerminal(a)) nodes.push(termRow(a));
      }
      for (const ws of v.repos) nodes.push(repoRow(ws));
    }
    if (!nodes.length) {
      nodes.push(h('div.ide-side-empty', null,
        q ? 'No matches.' : 'No agents yet. Start one with New agent below.'));
    }
    els.clear();
    for (const n of nodes) if (n.dataset.key) els.set(n.dataset.key, n);
    list.replaceChildren(...nodes);
    if (focusKey) els.get(focusKey)?.focus({ preventScroll: true });
    refreshCard();
  }

  async function load(): Promise<void> {
    const [p, w] = await Promise.all([
      api.projects().catch(() => null),
      ideApi.workspaces().catch(() => null),
    ]);
    if (disposed) return;
    if (p) projects = p;
    if (w) workspaces = w;
    schedule();
  }

  const unwatch = agents.watch(() => schedule());
  document.addEventListener('keydown', onDocKey);
  void load();
  const poll = window.setInterval(() => void load(), POLL_MS);

  return {
    el,
    setSelected(key) {
      selected = key;
      schedule();
    },
    focusSearch() {
      search.focus();
    },
    dispose() {
      disposed = true;
      unwatch();
      window.clearInterval(poll);
      hideCard();
      document.removeEventListener('keydown', onDocKey);
      card.remove();
      el.remove();
    },
  };
}
