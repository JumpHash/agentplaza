// The console's shell, as the Loom and Graphite consoles draw it (styles/proto/base.css): the sidebar (the brand, search,
// the screens, what is moving inside each project, the foot with the hub and STOP ALL), the main column (its head with
// the scope, the crumbs and the live line, then the screen), Settings with its own index, and the ambient layer behind
// the work. The screens mount into `.page` exactly as before; the shell only frames them.

import { api, type Approval, type OwnerConversationSummary, type Project, type Task } from '../api';
import { workApi, type Card } from '../api-work';
import { hrefIn, type Route } from '../router';
import { isLive, projects, scope, switchTo } from '../scope';
import { agents, on, overview } from '../store';
import { agentName } from '../roles';
import { openIde } from '../ide/open';
import { openNewProject } from './new-project';
import { stopAll } from './plaza-control';
import { pmono } from './components';
import { h, icon, lu, replace } from './dom';

export { pmono };
import type { LucideName } from './icons.gen';

// ---- What the shell lists ---------------------------------------------------------------------------------------

/** The screens of the sidebar, in its order; More holds the rest. */
export const NAV: [string, LucideName, string][] = [
  ['overview', 'house', 'Overview'],
  ['inbox', 'inbox', 'Inbox'],
  ['approvals', 'shield-check', 'Approvals'],
  ['chats', 'message-square', 'Chats'],
  ['work', 'workflow', 'Work'],
  ['history', 'git-branch', 'Weft'],
  ['sessions', 'bot', 'Sessions'],
  ['machines', 'server', 'Machines'],
];
export const MORE: [string, LucideName, string][] = [
  ['tasks', 'list-checks', 'Tasks'],
  ['metrics', 'chart-column', 'Metrics'],
  ['library', 'library', 'Library'],
  ['board', 'pin', 'Board'],
  ['treasury', 'coins', 'Treasury'],
  ['rules', 'scale', 'Rules'],
  ['lab', 'flask-conical', 'Lab'],
];

/** A screen's name as the sidebar says it (`history` is Weft), or null when the sidebar does not list it. */
export function navLabel(key: string): string | null {
  return [...NAV, ...MORE].find(([k]) => k === key)?.[2] ?? null;
}

/** Settings: its groups and their screens (`#/settings/<group>/<screen>/…`). */
export const SETTINGS: [string, string, [string, string][]][] = [
  ['models', 'Models', [['models', 'Models']]],
  ['appearance', 'Appearance', [['appearance', 'Appearance']]],
  ['agents', 'Agents', [['prompts', 'Prompts'], ['skills', 'Skills'], ['recipes', 'Recipes']]],
  ['connections', 'Connections', [['tools', 'Tools'], ['mcp', 'MCP'], ['platforms', 'Git platforms']]],
  ['secrets', 'Secrets', [['vault', 'Vault'], ['egress', 'Egress credentials']]],
  ['compute', 'Compute', [['nodes', 'Nodes'], ['workstations', 'Workstations'], ['volumes', 'Volumes'], ['e2b', 'E2B API']]],
  ['health', 'Health', [['setup', 'Setup']]],
];
const GROUP_ICON: Record<string, LucideName> = {
  models: 'cpu', appearance: 'palette', agents: 'bot', connections: 'plug', secrets: 'key-round', compute: 'server', health: 'heart-pulse',
};

/** The settings group a screen sits in, or null when it is not a settings screen. */
export function settingsGroupOf(screen: string): string | null {
  return SETTINGS.find(([, , items]) => items.some(([k]) => k === screen))?.[0] ?? null;
}

/** The title of a settings screen. */
function settingsLabel(key: string): string {
  return SETTINGS.flatMap((g) => g[2]).find(([k]) => k === key)?.[1] ?? 'Settings';
}

// ---- What the shell remembers per viewer -------------------------------------------------------------------------

interface UiState {
  side: boolean;
  more: boolean;
  landed: boolean;
  closed: Record<string, boolean>;
}
const UI_KEY = 'apz.shell';
const ui: UiState = (() => {
  const d: UiState = { side: true, more: false, landed: false, closed: {} };
  try {
    return { ...d, ...(JSON.parse(localStorage.getItem(UI_KEY) ?? '{}') as Partial<UiState>) };
  } catch {
    return d;
  }
})();
function saveUi(): void {
  try {
    localStorage.setItem(UI_KEY, JSON.stringify(ui));
  } catch {
    // Not kept: the state holds for this page.
  }
}
/** The narrow layout (the sidebar becomes a drawer); read lazily, so the module loads without a window. */
const narrow = (): MediaQueryList => matchMedia('(max-width: 860px)');

// ---- Small parts ---------------------------------------------------------------------------------------------------

const MARK = (): SVGSVGElement => {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('class', 'ic');
  svg.setAttribute('width', '15');
  svg.setAttribute('height', '15');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  svg.innerHTML =
    '<rect x="3" y="3" width="7.8" height="7.8" rx="2.2" fill="currentColor"/><rect x="13.2" y="3" width="7.8" height="7.8" rx="2.2" fill="currentColor" opacity=".55"/><rect x="3" y="13.2" width="7.8" height="7.8" rx="2.2" fill="currentColor" opacity=".55"/><rect x="13.2" y="13.2" width="7.8" height="7.8" rx="2.2" fill="currentColor" opacity=".85"/><circle class="mk-dot" cx="12" cy="12" r="2.6" fill="var(--mark-dot, var(--signal))" stroke="var(--mark-bg, var(--action))" stroke-width="1.6"/>';
  return svg;
};

function iconBtn(name: LucideName, title: string, onClick: (e: MouseEvent) => void, cls = ''): HTMLButtonElement {
  const b = h('button.iconbtn', { type: 'button', class: cls, title, 'aria-label': title }, lu(name, 16));
  b.addEventListener('click', onClick);
  return b;
}

/** Short relative time for the sidebar: `just now`, `4 min`, `3 h`, `2 d`. */
function shortAgo(ms: number | null | undefined, now = Date.now()): string {
  if (!ms) return '';
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return 'now';
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min`;
  if (s < 86400) return `${Math.round(s / 3600)} h`;
  return `${Math.round(s / 86400)} d`;
}

const STAGE: Record<string, string> = {
  draft: 'Draft', planning: 'Planning', critique: 'Critique', approval: 'Approval', building: 'Implementation',
  implementation: 'Implementation', review: 'Review', delivered: 'Delivered', research: 'Research',
};

// ---- Popovers and the palette --------------------------------------------------------------------------------------

let openPop: { el: HTMLElement; off: () => void } | null = null;

/** A menu beside `anchor` (below it, or above when `above`); a pick or a click outside closes it. */
export function popover(anchor: HTMLElement, menu: HTMLElement, above = false): void {
  closePop();
  const pop = h('div.pop', { role: 'presentation' }, menu);
  document.body.appendChild(pop);
  const r = anchor.getBoundingClientRect();
  const w = pop.offsetWidth;
  const left = Math.max(8, Math.min(innerWidth - w - 8, r.left));
  pop.style.left = `${left}px`;
  if (above) pop.style.bottom = `${innerHeight - r.top + 6}px`;
  else pop.style.top = `${r.bottom + 6}px`;
  const onDown = (e: MouseEvent): void => {
    if (!pop.contains(e.target as Node) && e.target !== anchor && !anchor.contains(e.target as Node)) closePop();
  };
  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape') closePop();
  };
  setTimeout(() => document.addEventListener('mousedown', onDown), 0);
  document.addEventListener('keydown', onKey);
  menu.addEventListener('click', (e) => {
    if ((e.target as Element).closest('[role="menuitem"]')) closePop();
  });
  openPop = {
    el: pop,
    off: () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    },
  };
  (menu.querySelector('[role="menuitem"]') as HTMLElement | null)?.focus();
}
function closePop(): void {
  if (!openPop) return;
  openPop.off();
  openPop.el.remove();
  openPop = null;
}

function menuItem(ic: LucideName | HTMLElement, text: string, small: string | null, onPick: () => void, current = false): HTMLButtonElement {
  const b = h(
    'button',
    { type: 'button', role: 'menuitem', 'aria-current': current ? 'true' : null },
    typeof ic === 'string' ? lu(ic, 15) : ic,
    h('span', null, text, small ? h('small', null, small) : null),
  );
  b.addEventListener('click', onPick);
  return b;
}

/** Ctrl K: search the screens, settings, projects and agents, and jump. */
export function openPalette(): void {
  if (document.querySelector('.pal-scrim')) return;
  type Item = { group: string; icon: LucideName; label: string; hint: string; to: string };
  const items: Item[] = [
    ...[...NAV, ...MORE].map(([k, ic, l]) => ({ group: 'Go to', icon: ic, label: l, hint: 'Screen', to: `#/${k}` })),
    ...projects.get().filter(isLive).map((p) => ({ group: 'Projects', icon: 'folder' as LucideName, label: p.title, hint: STAGE[p.stage] ?? p.stage, to: `#/projects/${encodeURIComponent(p.id)}/plan` })),
    ...agents.get().map((a) => ({ group: 'Sessions', icon: 'bot' as LucideName, label: agentName(a), hint: a.role ?? '', to: `#/sessions/${a.cid}` })),
    ...SETTINGS.flatMap(([g, gl, list]) =>
      list.map(([k, l]) => ({ group: 'Settings', icon: GROUP_ICON[g] ?? 'settings', label: l === gl ? l : `${gl}: ${l}`, hint: 'Settings', to: `#/settings/${g}/${k}` })),
    ),
  ];
  const sp = scope.get() ? projects.get().find((p) => p.id === scope.get()) : null;
  const input = h('input', { id: 'pal-q', placeholder: 'Search projects, agents, screens, or settings…', autocomplete: 'off', spellcheck: 'false', role: 'combobox', 'aria-expanded': 'true', 'aria-controls': 'pal-list' });
  const list = h('div.pal-list', { id: 'pal-list', role: 'listbox', 'aria-label': 'Results' });
  let sel = 0;
  let shown: Item[] = [];
  const close = (): void => {
    if (scrim.classList.contains('out')) return;
    scrim.classList.add('out');
    document.removeEventListener('keydown', onKey, true);
    setTimeout(() => scrim.remove(), 160);
  };
  const pick = (it: Item | undefined): void => {
    if (!it) return;
    close();
    location.hash = it.to;
  };
  const paint = (): void => {
    list.querySelectorAll<HTMLElement>('.pal-o').forEach((o) => o.setAttribute('aria-selected', String(Number(o.dataset.i) === sel)));
    input.setAttribute('aria-activedescendant', shown.length ? `pal-o-${sel}` : '');
    list.querySelector(`#pal-o-${sel}`)?.scrollIntoView({ block: 'nearest' });
  };
  const draw = (): void => {
    const q = input.value.trim().toLowerCase();
    shown = (q ? items.filter((it) => it.label.toLowerCase().includes(q) || it.hint.toLowerCase().includes(q)) : items.filter((it) => it.group === 'Go to' || it.group === 'Projects')).slice(0, 40);
    sel = Math.min(sel, Math.max(0, shown.length - 1));
    const rows: HTMLElement[] = [];
    let group = '';
    shown.forEach((it, i) => {
      if (it.group !== group) {
        group = it.group;
        rows.push(h('div.pal-g', { role: 'presentation' }, group));
      }
      const row = h('div.pal-o', { role: 'option', id: `pal-o-${i}`, 'data-i': String(i), 'aria-selected': String(i === sel) }, lu(it.icon, 16), h('span.pal-l', null, it.label), h('span.pal-h', null, it.hint), h('span.pal-go', null, lu('corner-down-left', 14)));
      row.addEventListener('click', () => pick(it));
      row.addEventListener('mousemove', () => {
        if (sel !== i) {
          sel = i;
          paint();
        }
      });
      rows.push(row);
    });
    replace(list, rows.length ? rows : [h('div.pal-empty', null, `Nothing matches “${input.value.trim()}”.`)]);
    paint();
  };
  const onKey = (e: KeyboardEvent): void => {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    } else if ((e.key === 'ArrowDown' || e.key === 'ArrowUp') && shown.length) {
      e.preventDefault();
      sel = (sel + (e.key === 'ArrowDown' ? 1 : shown.length - 1)) % shown.length;
      paint();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      pick(shown[sel]);
    }
  };
  input.addEventListener('input', () => {
    sel = 0;
    draw();
  });
  const foot = h('div.pal-f', null, h('span', null, h('kbd', null, '↑'), h('kbd', null, '↓'), ' move'), h('span', null, h('kbd', null, 'Enter'), ' open'), h('span', null, h('kbd', null, 'Esc'), ' close'), h('span.sp'), h('span', null, `${items.length} things in the plaza`));
  const box = h('div.pal', { role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Search and commands' }, h('div.pal-in', null, lu('search', 17), input, h('span.pal-scope', null, sp ? sp.title : 'Whole plaza')), list, foot);
  const scrim = h('div.scrim.pal-scrim', null, box);
  scrim.addEventListener('mousedown', (e) => {
    if (e.target === scrim) close();
  });
  document.addEventListener('keydown', onKey, true);
  document.body.appendChild(scrim);
  draw();
  input.focus();
}

// ---- The sidebar --------------------------------------------------------------------------------------------------

interface SideData {
  approvals: Approval[];
  tasks: Task[];
  conversations: OwnerConversationSummary[];
  /** The cards waiting for the owner (the inbox's count). */
  cards: Card[];
}
const side: SideData = { approvals: [], tasks: [], conversations: [], cards: [] };

async function refreshSide(): Promise<void> {
  const [approvals, tasks, conversations, cards] = await Promise.allSettled([api.approvals(), api.tasks(), api.ownerConversations(), workApi.cards(true)]);
  if (cards.status === 'fulfilled') side.cards = cards.value;
  if (approvals.status === 'fulfilled') side.approvals = approvals.value.filter((a) => (a as { status?: string }).status === undefined || (a as { status?: string }).status === 'pending');
  if (tasks.status === 'fulfilled') side.tasks = tasks.value;
  if (conversations.status === 'fulfilled') side.conversations = conversations.value;
}

function navRow([k, ic, l]: [string, LucideName, string], current: string, count?: number): HTMLElement {
  return h(
    'a.nrow',
    { href: hrefIn(scope.get(), k), 'aria-current': current === k ? 'page' : null, title: k === 'history' ? 'Version control: repositories, changes, releases' : null },
    lu(ic, 16),
    h('span.lab', null, l),
    count ? h('span.count.hot', null, String(count)) : null,
  );
}

function thread(to: string, ic: LucideName, text: string, end: Node | null, hash: string): HTMLElement {
  return h('a.thread.line', { href: to, 'aria-current': hash === to ? 'page' : null }, h('div.t1', null, lu(ic, 14), h('span.tt', null, text), end));
}

function taskChip(t: Task): HTMLElement {
  const s = (t as { status?: string }).status ?? 'open';
  const [label, tone] =
    s === 'claimed' || s === 'working' ? ['Working', 'info'] : s === 'review' ? ['Review', 'review'] : s === 'failed' ? ['Failed', 'danger'] : s === 'done' ? ['Done', 'ok'] : ['Waiting', 'muted'];
  return h('span.chip', { class: `t-${tone}` }, label);
}

function sidebarBody(route: Route): HTMLElement[] {
  const hash = location.hash.split('?')[0] ?? '';
  const cur = route.screen === 'settings' ? 'settings' : route.screen;
  const pending = side.approvals.length;
  const moreOn = ui.more || MORE.some(([k]) => k === cur);
  const top = h(
    'div.side-top',
    null,
    h('a.mark', { href: '#/overview', title: 'AgentPlaza', 'aria-label': 'AgentPlaza, Overview' }, MARK()),
    h('span.brand', null, 'AgentPlaza'),
    iconBtn('square-pen', 'New project', () => openNewProject()),
    iconBtn('panel-left', 'Hide the sidebar (Ctrl+\\)', () => toggleSide()),
  );
  const search = h('button.side-search', { type: 'button', title: 'Search the plaza and run a command' }, lu('search', 15), h('span', null, 'Search or jump to…'), h('kbd', null, 'Ctrl K'));
  search.addEventListener('click', () => openPalette());
  const more = h('button.nrow.more', { type: 'button', 'aria-expanded': String(moreOn) }, lu(moreOn ? 'chevron-up' : 'ellipsis', 16), h('span.lab', null, moreOn ? 'Less' : 'More'));
  more.addEventListener('click', () => {
    ui.more = !ui.more;
    saveUi();
    redraw();
  });
  const nav = h('nav.nav', { 'aria-label': 'Plaza' }, NAV.map((n) => navRow(n, cur, n[0] === 'approvals' ? pending : n[0] === 'inbox' ? side.cards.length : undefined)), more, moreOn ? MORE.map((n) => navRow(n, cur)) : null);

  const scroll = h('div.scroll');
  const drafting = side.conversations.filter((c) => !c.project && !c.archived);
  if (drafting.length) {
    scroll.append(
      h('div.sec', null, h('span', null, 'Talking it through')),
      h(
        'div.proj',
        null,
        drafting.map((c) => {
          const to = `#/ide/talk/${encodeURIComponent(c.id)}`;
          return h(
            'a.thread',
            { href: to, 'aria-current': hash === to ? 'page' : null },
            h('div.t1', null, lu('file-pen-line', 14), h('span.tt', null, c.title), h('span.ago', null, shortAgo(c.updated_ms))),
            h('div.t2', null, h('span', null, c.draft ? `Draft · revision ${c.draft.revision}` : 'Talking it through'), h('span.sp'), c.pending ? h('span.chip.t-info', null, 'Answering') : null),
          );
        }),
      ),
    );
  }
  const plus = iconBtn('plus', 'New project', () => openNewProject(), 'xs');
  scroll.append(h('div.sec', null, h('span', null, 'Projects'), plus));
  const live = projects
    .get()
    .filter(isLive)
    .sort((a, b) => b.updated_ms - a.updated_ms);
  for (const p of live) {
    const closed = !!ui.closed[p.id];
    const tw = h('button.proj-tw', { type: 'button', 'aria-expanded': String(!closed), title: closed ? 'Show what is inside' : 'Hide what is inside' }, lu(closed ? 'chevron-right' : 'chevron-down', 13));
    tw.addEventListener('click', () => {
      ui.closed[p.id] = !closed;
      saveUi();
      redraw();
    });
    const tag = p.paused ? h('span.modebadge.t-warn', null, 'Paused') : p.mode === 'maintenance' ? h('span.modebadge', null, 'Maintenance') : h('span.count', null, STAGE[p.stage] ?? p.stage);
    const head = h('a.proj-head', { href: `#/projects/${encodeURIComponent(p.id)}/plan`, 'aria-current': cur === 'projects' && route.params[0] === p.id ? 'page' : null }, pmono(p, 18), h('span.lab', null, p.title), tag);
    const items: HTMLElement[] = [];
    if (!closed) {
      const conv = side.conversations.find((c) => c.project === p.id && !c.archived);
      if (conv) items.push(thread(`#/ide/talk/${encodeURIComponent(conv.id)}`, 'messages-square', 'With the orchestrator', h('span.ago', null, shortAgo(conv.updated_ms)), hash));
      const waits = side.approvals.filter((a) => (a as { project?: string | null }).project === p.id);
      for (const a of waits.slice(0, 3)) {
        const title = (a as { title?: string; summary?: string; kind?: string }).title ?? (a as { summary?: string }).summary ?? (a as { kind?: string }).kind ?? 'Waiting on you';
        items.push(thread(`#/approvals`, 'shield-check', title, h('i.dot.t-accent', { title: 'Needs you' }), hash));
      }
      const moving = side.tasks.filter((t) => (t as { project?: string | null }).project === p.id && !['done', 'verified', 'failed', 'cancelled', 'withdrawn'].includes(String((t as { status?: string }).status)));
      for (const t of moving.slice(0, 6)) {
        const to = `#/tasks/${encodeURIComponent(t.id)}`;
        const who = agents.get().find((a) => a.cid === (t as { agent_cid?: number | null }).agent_cid);
        items.push(
          h(
            'a.thread',
            { href: to, 'aria-current': hash === to ? 'page' : null },
            h('div.t1', null, h('span.tt', null, (t as { goal?: string }).goal ?? t.id), h('span.ago', null, shortAgo((t as { updated_at?: number; updated_ms?: number }).updated_ms ?? (t as { updated_at?: number }).updated_at))),
            h('div.t2', null, h('span.mono', null, t.id), who ? h('span', null, agentName(who)) : null, h('span.sp'), taskChip(t)),
          ),
        );
      }
    }
    scroll.append(h('div.proj', null, h('div.proj-row', null, tw, head), items));
  }
  if (!live.length) scroll.append(h('div.side-empty', null, 'No live projects. Start one with the orchestrator.'));

  const hub = h('span.hub');
  const o = overview.get();
  hub.classList.toggle('held', !o);
  hub.title = !o ? 'apid unreachable' : `Hub ${o.hub ? 'up' : 'down'} · Node ${o.noded ? 'up' : 'down'} · ${o.agents.working} working · ${o.agents.idle} idle`;
  replace(hub, h('span.dotlive'), h('span.lab', null, !o ? 'apid unreachable' : `${o.agents.working} working`));
  const me = h('button.iconbtn.me-btn', { type: 'button', title: 'You: settings, help, sign out', 'aria-label': 'Your menu' }, 'O');
  me.addEventListener('click', () => ownerMenu(me));
  const gear = h('a.iconbtn', { href: '#/settings/models/models', title: 'Settings', 'aria-label': 'Settings', 'aria-current': cur === 'settings' ? 'page' : null }, lu('settings', 16));
  const foot = h('div.side-foot', null, hub, iconBtn('code-xml', 'Open the IDE (Ctrl+Shift+E)', () => openIde('')), me, gear, frame().stop);
  return [top, search, nav, scroll, foot];
}

function ownerMenu(anchor: HTMLElement): void {
  const local = ['localhost', '127.0.0.1', '[::1]'].includes(location.hostname);
  const menu = h(
    'div.menu',
    { role: 'menu' },
    h('div.menu-h', null, 'Owner of this plaza'),
    menuItem('settings', 'Settings', null, () => (location.hash = '#/settings/models/models')),
    menuItem('palette', 'Appearance', null, () => (location.hash = '#/settings/appearance/appearance')),
    menuItem('wand-sparkles', 'Set up AgentPlaza', null, () => (location.hash = '#/settings/health/setup')),
    local ? null : h('hr'),
    local
      ? null
      : menuItem('log-out', 'Sign out', null, () => {
          void api.signOut().finally(() => location.reload());
        }),
  );
  popover(anchor, menu, true);
}

// ---- The main column's head ---------------------------------------------------------------------------------------

function scopeMenu(anchor: HTMLElement, route: Route): void {
  const cur = route.project;
  const list = projects.get().filter((p) => isLive(p) || p.id === cur);
  const menu = h(
    'div.menu',
    { role: 'menu' },
    h('div.menu-h', null, 'Show'),
    menuItem('layers', 'All projects', 'The whole plaza', () => switchTo(null), !cur),
    h('hr'),
    list.map((p) => menuItem(pmono(p, 18), p.title, p.mode === 'maintenance' ? 'Maintenance' : (STAGE[p.stage] ?? p.stage), () => switchTo(p.id), p.id === cur)),
    h('hr'),
    menuItem('plus', 'New project…', null, () => openNewProject()),
  );
  popover(anchor, menu);
}

/** The crumbs of the main column's head: the screen, then what it shows. */
export type Crumbs = { mono?: Project | null; parts: string[] };

function topHead(route: Route, crumbs: Crumbs, extra: Node[]): HTMLElement {
  const sideBtn = iconBtn('panel-left', 'Show the sidebar (Ctrl+\\)', () => toggleSide(), ui.side ? 'show-sm' : '');
  const sp = route.project ? projects.get().find((p) => p.id === route.project) : null;
  const scopeBtn = h(
    'button.scope-pill',
    { type: 'button', 'aria-haspopup': 'menu', title: 'Show the whole plaza or one project' },
    sp ? pmono(sp, 16) : lu('layers', 14),
    h('span', null, sp ? sp.title : 'All projects'),
    lu('chevrons-up-down', 13),
  );
  scopeBtn.addEventListener('click', () => scopeMenu(scopeBtn, route));
  const parts: Node[] = [];
  crumbs.parts.forEach((x, i) => {
    if (i) parts.push(h('span.sl.hide-sm', null, '/'));
    parts.push(h('span', { class: i === crumbs.parts.length - 1 ? 'c2' : 'c1 hide-sm' }, x));
  });
  return h(
    'header.top',
    null,
    sideBtn,
    route.screen === 'settings' ? null : scopeBtn,
    h('div.crumb', null, crumbs.mono ? pmono(crumbs.mono, 18) : null, parts),
    extra.length ? extra : liveEl(),
  );
}

// ---- The live line: what the agents are doing, one at a time -------------------------------------------------------

let liveAt = 0;
let liveBtn: HTMLButtonElement | null = null;
/** The head's live line, made once on first use. */
function liveEl(): HTMLButtonElement {
  if (liveBtn) return liveBtn;
  liveBtn = h('button.top-live.hide-sm', { type: 'button', 'data-tone': 'live', title: 'What the agents are doing now. Click to open it', hidden: true });
  liveBtn.addEventListener('click', () => {
    const items = liveItems();
    location.hash = items[liveAt % Math.max(1, items.length)]?.to ?? '#/sessions';
  });
  return liveBtn;
}
function liveItems(): { text: string; tone: string; to: string }[] {
  return agents
    .get()
    .filter((a) => a.status === 'working')
    .map((a) => {
      const work = (a as { work?: { origin?: { task?: string } } | null }).work;
      const step = (a as { step?: { stage?: string; tool?: string } | null }).step;
      const verb = step?.stage === 'approval' ? 'waits on you' : step?.stage === 'tool' ? `runs ${step.tool ?? 'a tool'}` : step?.stage === 'streaming' ? 'is writing' : 'is thinking';
      return { text: `${agentName(a)} ${verb} · ${work?.origin?.task ?? 'chat'}`, tone: step?.stage === 'approval' ? 'needs' : 'live', to: step?.stage === 'approval' ? '#/approvals' : `#/sessions/${a.cid}` };
    });
}
function drawLive(): void {
  const items = liveItems();
  const live = liveEl();
  live.hidden = !items.length;
  if (!items.length) return;
  const it = items[liveAt % items.length]!;
  live.dataset.tone = it.tone;
  replace(live, h('span.tl-dot'), h('span.tl-win', null, h('span.tl-t', { 'data-tone': it.tone }, it.text)), h('span.tl-n', null, String(items.length)));
}

// ---- The ambient layer ---------------------------------------------------------------------------------------------

function ornament(skin: string): string {
  if (skin === 'graphite') {
    const n = Math.max(3, Math.min(7, agents.get().filter((a) => a.status === 'working').length));
    const rings: [number, number][] = [[96, 34], [158, 58], [222, 84]];
    const orbits = rings.map(([rx, ry], i) => `<ellipse class="o" cx="660" cy="34" rx="${rx}" ry="${ry}" style="--i:${i}"/>`).join('');
    const streaks = Array.from({ length: n }, (_, k) => {
      const [rx, ry] = rings[k % 3]!;
      return `<ellipse class="s${k === 0 ? ' hot' : ''}" cx="660" cy="34" rx="${rx}" ry="${ry}" pathLength="1000" style="--d:${30 + k * 9}s;--o:${-(k * 7.3).toFixed(1)}s"/>`;
    }).join('');
    return `<svg viewBox="0 0 820 240" preserveAspectRatio="xMaxYMin slice">${orbits}${streaks}</svg>`;
  }
  const rows: string[] = [];
  const warps: string[] = [];
  for (let i = 0; i < 9; i++) {
    const y = 26 + i * 22;
    const a = 7 + (i % 3) * 3;
    const ph = (i % 2) * 40;
    const d = `M-40 ${y} C ${80 + ph} ${y - a}, ${160 + ph} ${y + a}, ${260 + ph} ${y} S ${440 + ph} ${y - a}, ${560 + ph} ${y} S 760 ${y + a}, 860 ${y}`;
    rows.push(`<path class="w${i === 4 ? ' hot' : ''}" d="${d}" style="--i:${i}"/>`);
  }
  for (let j = 0; j < 8; j++) warps.push(`<line class="v" x1="${120 + j * 84}" y1="0" x2="${120 + j * 84}" y2="240"/>`);
  return `<svg viewBox="0 0 820 240" preserveAspectRatio="xMaxYMin slice">${warps.join('')}${rows.join('')}</svg>`;
}

let amb: HTMLElement | null = null;
/** The quiet layer behind the work: colour fields, the skin's pattern (lit under the pointer), the ornament, grain. */
export function mountAmbient(): void {
  if (amb) return;
  const root = document.documentElement;
  amb = h('div.amb', { 'aria-hidden': 'true' });
  const draw = (): void => {
    amb!.innerHTML = `<i class="amb-f amb-f1"></i><i class="amb-f amb-f2"></i><i class="amb-f amb-f3"></i><i class="amb-pat"></i><i class="amb-lit"></i><div class="amb-orn">${ornament(root.dataset.skin ?? 'loom')}</div><i class="amb-grain"></i>`;
  };
  draw();
  document.body.prepend(amb);
  // A change of skin redraws the ornament (threads for Loom, orbits for Graphite).
  new MutationObserver(draw).observe(root, { attributes: true, attributeFilter: ['data-skin'] });
  let ev: PointerEvent | null = null;
  let raf = 0;
  let idle = 0;
  const reduced = matchMedia('(prefers-reduced-motion: reduce)');
  const frame = (): void => {
    raf = 0;
    if (!ev || !amb) return;
    const lit = amb.querySelector<HTMLElement>('.amb-lit');
    const a = amb.getBoundingClientRect();
    lit?.style.setProperty('--mx', `${ev.clientX - a.left}px`);
    lit?.style.setProperty('--my', `${ev.clientY - a.top}px`);
    const target = ev.target as Element | null;
    const c = target?.closest?.('.card, .stat, .pjc, .ov-card') as HTMLElement | null;
    if (c) {
      const r = c.getBoundingClientRect();
      c.style.setProperty('--px', `${ev.clientX - r.left}px`);
      c.style.setProperty('--py', `${ev.clientY - r.top}px`);
    }
    const t = target?.closest?.('.top') as HTMLElement | null;
    if (t) t.style.setProperty('--hx', `${ev.clientX - t.getBoundingClientRect().left}px`);
  };
  document.addEventListener(
    'pointermove',
    (e) => {
      if (reduced.matches || e.pointerType === 'touch') return;
      ev = e;
      root.classList.add('fx-lit');
      clearTimeout(idle);
      idle = window.setTimeout(() => root.classList.remove('fx-lit'), 1600);
      if (!raf) raf = requestAnimationFrame(frame);
    },
    { passive: true },
  );
  document.addEventListener('pointerleave', () => root.classList.remove('fx-lit'));
  document.addEventListener(
    'scroll',
    (e) => {
      const t = e.target as HTMLElement | null;
      if (t?.id === 'cmain') {
        if (t.scrollTop > 4) root.setAttribute('data-scrolled', '');
        else root.removeAttribute('data-scrolled');
      }
    },
    true,
  );
}

// ---- The frame -----------------------------------------------------------------------------------------------------

interface Frame {
  app: HTMLElement;
  side: HTMLElement;
  main: HTMLElement;
  top: HTMLElement;
  banners: HTMLElement;
  body: HTMLElement;
  stop: HTMLElement;
}
let built: Frame | null = null;
/** The shell's frame (sidebar, main column, head, banners, body), made once on first use. */
function frame(): Frame {
  if (built) return built;
  const side = h('aside.side', { 'aria-label': 'Projects and screens' });
  const top = h('div.top-slot');
  const banners = h('div.banners');
  const body = h('div.body', { id: 'cmain' });
  const main = h('main.main', null, top, banners, body);
  built = { app: h('div.app', { id: 'app' }, side, main), side, main, top, banners, body, stop: stopAll() };
  return built;
}
/** The shell's root element, to put in the document. */
export function appRoot(): HTMLElement {
  return frame().app;
}

let current: Route = { screen: 'overview', params: [], project: null };
let crumbsNow: Crumbs = { parts: ['Overview'] };
let extraNow: Node[] = [];

function toggleSide(): void {
  if (narrow().matches) document.body.classList.toggle('side-mobile');
  else {
    ui.side = !ui.side;
    saveUi();
  }
  applyClasses();
  redraw();
}
function applyClasses(): void {
  frame().app.classList.toggle('side-closed', !ui.side);
}

/** Draws the sidebar and the head again from what the shell holds now. */
export function redraw(): void {
  replace(frame().side, sidebarBody(current));
  replace(frame().top, topHead(current, crumbsNow, extraNow));
  drawLive();
}

/** The shell for a route: the root's data-screen, the crumbs and anything the screen adds to the head. */
export function frameRoute(route: Route, crumbs: Crumbs, extra: Node[] = []): void {
  current = route;
  crumbsNow = crumbs;
  extraNow = extra;
  document.documentElement.dataset.screen = route.screen;
  document.body.classList.remove('side-mobile');
  redraw();
}

/** The banners under the head (held, development models). */
export function setBanners(nodes: Node[]): void {
  replace(frame().banners, nodes);
}

/** The page a screen mounts into. `layout` 'bare' leaves the page's padding to the screen (Overview's hero). */
export function pageFor(route: Route, layout: 'page' | 'bare' | 'full' = 'page'): HTMLElement {
  frame().main.classList.toggle('full', layout === 'full');
  frame().main.classList.toggle('bare', layout === 'bare');
  frame().body.scrollTop = 0;
  document.documentElement.removeAttribute('data-scrolled');
  if (layout !== 'page') {
    const root = h('div.view-root', { class: `page-${route.screen}` });
    replace(frame().body, root);
    return root;
  }
  const page = h('div.page', { class: `page-${route.screen}` });
  replace(frame().body, page);
  return page;
}

/** Settings: its index beside the screen's page. Returns the page the screen mounts into. */
export function settingsPage(group: string, key: string): HTMLElement {
  frame().main.classList.remove('full', 'bare');
  frame().body.scrollTop = 0;
  const snav = h(
    'nav.snav',
    { 'aria-label': 'Settings' },
    SETTINGS.map(([g, gl, items]) =>
      items.length === 1 && items[0]![1] === gl
        ? h('a.nrow', { href: `#/settings/${g}/${items[0]![0]}`, 'aria-current': items[0]![0] === key ? 'page' : null }, lu(GROUP_ICON[g] ?? 'settings', 16), h('span.lab', null, gl))
        : h(
            'div.snav-g',
            null,
            h('div.nrow.snav-gh', null, lu(GROUP_ICON[g] ?? 'settings', 16), h('span.lab', null, gl)),
            items.map(([k, l]) => h('a.nrow.snav-sub', { href: `#/settings/${g}/${k}`, 'aria-current': k === key ? 'page' : null }, h('span.lab', null, l))),
          ),
    ),
    h('p.snav-note', null, 'A project overrides models, permissions and limits in its own Settings tab.'),
  );
  const page = h('div.page', { class: `page-${key}` });
  replace(frame().body, h('div.settings', null, snav, h('div.spage', null, page)));
  void group;
  return page;
}

/** The crumbs of a settings screen. */
export function settingsCrumbs(group: string, key: string): Crumbs {
  const grp = SETTINGS.find((g) => g[0] === group) ?? SETTINGS[0]!;
  const label = settingsLabel(key);
  return { parts: label === grp[1] ? ['Settings', grp[1]] : ['Settings', grp[1], label] };
}

/** Starts the shell's own data: the sidebar's lists, kept current, and the keys. */
export function startShell(): void {
  applyClasses();
  projects.watch(() => redraw());
  agents.watch(() => {
    drawLive();
    replace(frame().side, sidebarBody(current));
  });
  overview.watch(() => replace(frame().side, sidebarBody(current)));
  const pull = (): void => {
    void refreshSide().then(() => replace(frame().side, sidebarBody(current)));
  };
  pull();
  on('card', pull);
  setInterval(pull, 15000);
  setInterval(() => {
    liveAt++;
    if (!document.hidden) drawLive();
  }, 4200);
  document.addEventListener('keydown', (e) => {
    const target = e.target as Element | null;
    const typing = !!target?.closest?.('input, textarea, select, [contenteditable]:not([contenteditable="false"]), .cm-editor');
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      openPalette();
    } else if ((e.ctrlKey || e.metaKey) && e.key === '\\' && !typing) {
      e.preventDefault();
      toggleSide();
    }
  });
  narrow().addEventListener('change', () => document.body.classList.remove('side-mobile'));
  void icon;
}
