// The IDE screen (route #/ide and #/ide/<workspace>): a full-window view of its own, with no console shell beside it. The
// agent-first layout: projects and agents on the left; the selected agent's chat, terminal, editor or diff in the centre;
// the board, files, tasks, approvals and activity on the right; a terminal and run drawer under the centre (Ctrl+`); a
// status bar. This module owns the data and wires the parts: the frame (ide-frame), the sidebar, the chat, the panels
// and the palette, plus the editor and the terminal of the workspace. The model is the wasm IDE state (ide/wasm).
// Console (and Escape) goes back to the screen the IDE was opened from.

import type { EditorState } from '@codemirror/state';
import ideCss from '../styles/ide.css';
import shellCss from '../styles/ide-shell.css';
import { ApiError, api, ideApi, type Agent, type IdeCheckpoint, type IdeWorkspaceInfo, type Project, type Task } from '../api';
import { go } from '../router';
import { consoleOrigin } from '../ide/open';
import { applyBuffer, badgeOf, type Buffer, dirsOf, languageFor, lineCounts, LAST_WORKSPACE_KEY, onRemote, openBuffer, parentDir, parseViewed, viewedKey, withViewed, type Viewed } from '../ide/model';
import { fitSize, NARROW_QUERY, parseLayout, serializeLayout, type Layout } from '../ide/layout';
import { loadIde, type Ide } from '../ide/wasm';
import { projectFor, rowStatus, type PaletteItem } from '../ide/view';
import type { Base, Entry, FileChange, IdeState, Stats } from '../ide/types';
import { agents, on, onConnection } from '../store';
import { badge, button, empty, modal, toast, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { heldBanner, onPlazaHeld } from '../ui/plaza-control';
import { themeSwitch } from '../ui/theme';
import type { Cleanup } from './index';
import { Editor, stateFor } from './ide-editor';
import { quickOpen, searchView } from './ide-find';
import { dockEdge, keyboardHelp, LANG_LABEL, panelEdge, statusBar, type StatusInfo } from './ide-chrome';
import { changesPanel, checkpointsPanel } from './ide-side';
import { approvalsPanel, runPanel, type Panel as TabPanel } from './ide-tabs';
import { terminalTabs, type TabsHandle } from './ide-terminal';
import { buildFrame, CENTRE_LABEL, type CentreView, type DrawerTab, type Frame, paintTabs, type RightView, setCrumb } from './ide-frame';
import { treeRows } from './ide-files';
import { sidebar } from './ide-sidebar';
import { chatPane, type ChatPane } from './ide-chat';
import { palette, type Palette } from './ide-palette';
import { boardPanel, type BoardPanel } from './ide-board';
import { tasksPanel, type TasksPanel } from './ide-tasks';
import { activityPanel, type ActivityPanel } from './ide-activity';

const STYLE_ID = 'apz-ide-style';
const REFRESH_MS = 400;
/** The panel sizes and open flags, kept per browser. */
const LAYOUT_KEY = 'apz.ide.layout';
/** The drawer's open state, its tab and its height, kept per browser. */
const DRAWER_KEY = 'apz.ide.drawer';
/** The drawer's height limits: the centre keeps `editorMin` px above it. */
const DRAWER = { min: 120, def: 240, max: 1200, editorMin: 160 };
const MID_QUERY = '(max-width: 1199px)';
const VIEWS: CentreView[] = ['chat', 'terminal', 'editor', 'diff'];

const TONE: Record<string, Tone> = { conflict: 'danger', 'agent working': 'info', saved: 'ok', idle: 'muted', held: 'danger' };

/** The stylesheets, inserted once on the first visit (build.mjs makes them text). The hidden attribute always hides. */
function ensureStyle(): void {
  if (document.getElementById(STYLE_ID)) return;
  const style = document.createElement('style');
  style.id = STYLE_ID;
  style.textContent = `${ideCss}\n${shellCss}\n.ide-host [hidden]{display:none!important}`;
  document.head.appendChild(style);
}

function readStore(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStore(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // A private window or blocked storage: the choice just is not remembered.
  }
}

interface DrawerPrefs {
  open: boolean;
  tab: DrawerTab;
  height: number;
}

function readDrawerPrefs(): DrawerPrefs {
  try {
    const v = JSON.parse(readStore(DRAWER_KEY) ?? 'null') as { open?: unknown; tab?: unknown; height?: unknown } | null;
    const height = typeof v?.height === 'number' && Number.isFinite(v.height) ? Math.min(Math.max(v.height, DRAWER.min), DRAWER.max) : DRAWER.def;
    return { open: v?.open === true, tab: v?.tab === 'run' ? 'run' : 'terminal', height: Math.round(height) };
  } catch {
    return { open: false, tab: 'terminal', height: DRAWER.def };
  }
}

/** One open file: its buffer, and its editor state while another tab is shown (the shown tab's state is the view's). */
interface Tab {
  buf: Buffer;
  state: EditorState | null;
  /** Counts the user's edits, so a save that raced an edit knows the buffer is still dirty. */
  version: number;
}

/** The agent's cid for an `agent:<cid>` workspace, else null. */
const cidOf = (id: string): number | null => {
  const m = /^agent:(\d+)$/.exec(id);
  return m ? Number(m[1]) : null;
};

const message = (err: unknown): string => (err instanceof Error ? err.message : String(err));

function toBase(v: string): Base {
  return v.startsWith('checkpoint:') ? { checkpoint: Number(v.slice('checkpoint:'.length)) } : 'work_start';
}

/** A repository workspace's name and branch: `repo:<name>@<ref>`. */
const repoNameOf = (w: IdeWorkspaceInfo): string => w.id.slice('repo:'.length, w.id.lastIndexOf('@'));
const refOf = (w: IdeWorkspaceInfo): string => (w.kind === 'repo' ? w.id.slice(w.id.lastIndexOf('@') + 1) : '');

function totalsOf(files: FileChange[]): Stats {
  let additions = 0;
  let deletions = 0;
  for (const f of files) {
    const c = lineCounts(f);
    additions += c.add;
    deletions += c.del;
  }
  return { files: files.length, additions, deletions };
}

/** Whether a key press is typed into a field or the editor (so a shortcut that is a letter or a `?` does not fire). */
const inTyping = (target: EventTarget | null): boolean =>
  !!(target as Element | null)?.closest?.('input, textarea, select, [contenteditable]:not([contenteditable="false"]), .cm-editor, .xterm');

/** Whether the key press is in a terminal: its keys belong to the shell, so the screen's Ctrl shortcuts stay off. */
const inTerminal = (target: EventTarget | null): boolean => !!(target as Element | null)?.closest?.('.xterm');

/** Whether the key press is in a form field (Escape there is the field's, not the screen's). */
const inField = (target: EventTarget | null): boolean => !!(target as Element | null)?.closest?.('input, textarea, select');

export function mount(root: HTMLElement, params: string[]): Cleanup {
  ensureStyle();
  root.classList.add('ide-host');
  const query = new URLSearchParams(location.hash.split('?')[1] ?? '');
  const requested = params[0] ?? null;
  const startFile = query.get('file');
  const startPanel = query.get('panel');

  let alive = true;
  let ide: Ide | null = null;
  let wsList: IdeWorkspaceInfo[] = [];
  /** The workspace the editor and the diff show (an agent's, or a repository's). */
  let ws: IdeWorkspaceInfo | null = null;
  /** The selected agent (null for a repository). */
  let cid: number | null = null;
  let state: IdeState | null = null;
  let changesError = '';
  let files: FileChange[] = [];
  let stats: Stats = { files: 0, additions: 0, deletions: 0 };
  let checkpoints: IdeCheckpoint[] = [];
  let marks: Viewed = {};
  let agentList: Agent[] = [];
  let projects: Project[] = [];
  let tasks: Task[] = [];
  let centreView: CentreView = 'editor';
  let rightView: RightView = 'files';
  const drawerPrefs = readDrawerPrefs();
  let drawerOpen = drawerPrefs.open;
  let drawerTab: DrawerTab = drawerPrefs.tab;
  let dockHeight = drawerPrefs.height;
  const narrowMq = matchMedia(NARROW_QUERY);
  const midMq = matchMedia(MID_QUERY);
  let narrow = narrowMq.matches;
  let mid = midMq.matches;
  let layout: Layout = parseLayout(readStore(LAYOUT_KEY), window.innerWidth);
  /** The file whose diff the review scrolls to, once it is drawn. */
  let focusPath: string | null = null;
  /** Per-selection controllers: made on first use, disposed when the selection moves on. */
  let chat: ChatPane | null = null;
  let activity: ActivityPanel | null = null;
  let approvals: TabPanel | null = null;
  let run: TabPanel | null = null;
  let board: BoardPanel | null = null;
  let boardKey: string | null = null;
  let boardCount: number | null = null;
  let taskPanel: TasksPanel | null = null;
  let taskKey: string | null = null;
  let taskCounts: { done: number; total: number } | null = null;
  let approvalCount = 0;
  let term: TabsHandle | null = null;
  let saved = false;
  let agentWorking = false;
  let wasWorking = false;
  let held = false;
  let connected = true;
  let overlay: (() => void) | null = null;
  /** The file the editor shows (the reducer's `active` is the tab; this is what the view holds). */
  let shown: string | null = null;
  /** The search view of the Files tab, per workspace: its results stay while the tab shows another view. */
  let search: { wsId: string; ui: { el: HTMLElement; focus: () => void } } | null = null;
  const entries = new Map<string, Entry>();
  const loadedDirs = new Set<string>();
  const expanded = new Set<string>();
  const tabsByPath = new Map<string, Tab>();
  const pendingPaths = new Set<string>();
  let pendingAll = false;
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;

  // ---- The shell ------------------------------------------------------------
  const changedBox = h('input', { type: 'checkbox' });
  const changedToggle = h('label.ide-toggle', null, changedBox, h('span', null, 'Changed only'));
  changedBox.addEventListener('change', () => {
    if (!ide || !state) return;
    state = ide.reduce(state, 'toggle_changed_filter');
    if (state.filter_changed) void loadChangedDirs();
    renderAll();
  });
  const searchBtn = button('', () => toggleSearch(), { icon: 'search', size: 'sm', title: 'Search the workspace (Ctrl/Cmd+Shift+F)' });
  const quickBtn = button('', () => openQuick(), { icon: 'file', size: 'sm', title: 'Quick open (Ctrl/Cmd+P)' });
  const treeList = h('div.ide-tree-list', { role: 'tree' });
  const searchHost = h('div.iw-files-search', { hidden: true });
  const cpHost = h('div.iw-files-cp');
  const filesEl = h('div.iw-files', null, h('div.ide-tree-tools', null, changedToggle, searchBtn, quickBtn), searchHost, treeList, cpHost);

  const saveBtn = button('Save', () => void save(), { kind: 'primary', size: 'sm', title: 'Save (Ctrl+S)' });
  const tabBar = h('div.ide-tabbar', { role: 'tablist' });
  const banners = h('div.ide-banners');
  const cmHost = h('div.ide-cm');
  const emptyHost = h('div.ide-empty-centre');
  /** The diff view: the changes since the base, in place of the editor. */
  const reviewHost = h('div.ide-review');
  const editorBar = h('div.iw-editor-bar', null, h('span.spacer'), saveBtn);
  /** The terminal's element: it moves between the centre's Terminal view and the drawer, and keeps running when it moves. */
  const termHost = h('div.ide-term-host');
  const editor = new Editor(cmHost);

  const sideEl = sidebar({ onOpen: (key) => openKey(key), onNewAgent: () => newAgent() });
  const help = keyboardHelp();
  const status = statusBar(() => help.set(!help.isOpen()));
  const sideEdge = panelEdge(
    'left',
    (x) => {
      layout.left = fitSize(x - frame.root.getBoundingClientRect().left, 'left', window.innerWidth, layout.rightOpen && !mid ? layout.right : 0);
      applyLayout();
    },
    saveLayout,
    () => {
      layout.left = 260;
      applyLayout();
      saveLayout();
    },
  );
  const rightEdge = panelEdge(
    'right',
    (x) => {
      layout.right = fitSize(frame.root.getBoundingClientRect().right - x, 'right', window.innerWidth, layout.leftOpen && !narrow ? layout.left : 0);
      applyLayout();
    },
    saveLayout,
    () => {
      layout.right = 380;
      applyLayout();
      saveLayout();
    },
  );
  const drawerEdge = dockEdge(
    (y) => {
      dockHeight = clampDock(frame.drawer.el.getBoundingClientRect().bottom - y);
      applyLayout();
    },
    () => saveDrawerPrefs(),
    (px) => {
      dockHeight = clampDock(dockHeight + px);
      applyLayout();
      saveDrawerPrefs();
    },
  );

  const frame: Frame = buildFrame({
    side: sideEl.el,
    sideEdge,
    rightEdge,
    drawerEdge,
    held: heldBanner(),
    theme: themeSwitch(),
    status: status.el,
  });
  root.replaceChildren(frame.root, help.el);
  frame.centre.views.editor.append(editorBar, tabBar, banners, cmHost, emptyHost);
  frame.centre.views.diff.append(reviewHost);
  frame.centre.views.terminal.append(termHost);
  frame.centre.views.terminal.classList.add('iw-view-term');
  wireFrame();

  /** The palette (Ctrl/Cmd+K): agents, repositories, files, tasks and commands. */
  const pal: Palette = palette((item) => runPaletteItem(item));

  // ---- Layout ---------------------------------------------------------------
  /** Puts the sizes and open flags on the screen: the columns are CSS variables, and a closed panel is hidden. */
  function applyLayout(): void {
    const width = window.innerWidth;
    const leftPx = fitSize(layout.left, 'left', width, !narrow && layout.rightOpen && !mid ? layout.right : 0);
    const rightPx = fitSize(layout.right, 'right', width, !narrow && layout.leftOpen ? leftPx : 0);
    frame.root.style.setProperty('--iw-side', layout.leftOpen && !narrow ? `${leftPx}px` : '0px');
    frame.root.style.setProperty('--iw-right', layout.rightOpen && !mid ? `${rightPx}px` : '0px');
    frame.root.style.setProperty('--iw-drawer', drawerOpen ? `${dockHeight}px` : '0px');
    frame.root.classList.toggle('narrow', narrow);
    frame.root.classList.toggle('mid', mid);
    frame.side.hidden = !layout.leftOpen && !narrow;
    frame.side.classList.toggle('open', layout.leftOpen && narrow);
    frame.right.el.hidden = !layout.rightOpen && !mid;
    frame.right.el.classList.toggle('open', layout.rightOpen && mid);
    frame.scrim.classList.toggle('show', (narrow && layout.leftOpen) || (mid && layout.rightOpen));
    frame.sideToggle.setAttribute('aria-pressed', String(layout.leftOpen));
    frame.rightToggle.setAttribute('aria-pressed', String(layout.rightOpen));
  }

  function saveLayout(): void {
    writeStore(LAYOUT_KEY, serializeLayout(layout));
  }

  function saveDrawerPrefs(): void {
    writeStore(DRAWER_KEY, JSON.stringify({ open: drawerOpen, tab: drawerTab, height: dockHeight }));
  }

  /** Keeps the drawer within the centre column: at least DRAWER.min, and the centre keeps DRAWER.editorMin above it. */
  function clampDock(px: number): number {
    const column = frame.centre.el;
    const room = column.clientHeight > 0 ? column.clientHeight - DRAWER.editorMin : Infinity;
    return Math.round(Math.max(DRAWER.min, Math.min(px, Math.max(DRAWER.min, room))));
  }

  function setLeft(open: boolean): void {
    layout.leftOpen = open;
    saveLayout();
    applyLayout();
  }

  function setRightOpen(open: boolean): void {
    layout.rightOpen = open;
    saveLayout();
    applyLayout();
  }

  /** Narrow screens: the sidebar drawer closes once the owner has picked something in it. */
  function closeDrawerIfNarrow(): void {
    if (narrow) layout.leftOpen = false;
    if (mid) layout.rightOpen = false;
    applyLayout();
  }

  function setDrawer(open: boolean, tab: DrawerTab = drawerTab): void {
    drawerOpen = open;
    drawerTab = tab;
    saveDrawerPrefs();
    renderAll();
    if (open && tab === 'terminal') term?.focus();
  }

  narrowMq.addEventListener('change', onBreakpoint);
  midMq.addEventListener('change', onBreakpoint);
  window.addEventListener('resize', applyLayout);
  function onBreakpoint(): void {
    narrow = narrowMq.matches;
    mid = midMq.matches;
    applyLayout();
  }

  /** The header toggles, the scrim and the drawer's own buttons. */
  function wireFrame(): void {
    frame.sideToggle.addEventListener('click', () => setLeft(!layout.leftOpen));
    frame.rightToggle.addEventListener('click', () => setRightOpen(!layout.rightOpen));
    frame.consoleBtn.addEventListener('click', () => leaveIde());
    frame.scrim.addEventListener('click', () => {
      setLeft(false);
      setRightOpen(false);
    });
    frame.drawer.close.addEventListener('click', () => setDrawer(false));
    for (const v of VIEWS) frame.centre.seg[v].addEventListener('click', () => setCentre(v));
    frame.centre.popout.addEventListener('click', () => {
      if (cid !== null) go('agents', cid);
    });
    for (const k of Object.keys(frame.right.tabs) as RightView[]) frame.right.tabs[k].addEventListener('click', () => setRight(k));
    for (const k of Object.keys(frame.drawer.tabs) as DrawerTab[]) frame.drawer.tabs[k].addEventListener('click', () => setDrawer(true, k));
  }

  // ---- Views ----------------------------------------------------------------
  /** The centre's view. Chat and Terminal need an agent; Editor and Diff need a workspace. */
  function setCentre(v: CentreView): void {
    centreView = v;
    renderAll();
    if (v === 'terminal') term?.focus();
  }

  /** The right panel's tab. Approvals and Activity are for agents only. */
  function setRight(v: RightView): void {
    rightView = v;
    if (!rightTabAllowed(v)) rightView = 'files';
    if (!layout.rightOpen) setRightOpen(true);
    else renderAll();
  }

  function rightTabAllowed(v: RightView): boolean {
    return cid !== null || (v !== 'approvals' && v !== 'activity');
  }

  function centreAllowed(v: CentreView): boolean {
    if (v === 'chat' || v === 'terminal') return cid !== null;
    return true;
  }

  // ---- Sidebar selection ----------------------------------------------------
  /** Opens a sidebar key: `agent:<cid>`, `term:<cid>` (the agent's terminal) or `ws:<workspace id>` (a repository). */
  function openKey(key: string): void {
    if (key.startsWith('agent:')) selectAgent(Number(key.slice('agent:'.length)), 'chat');
    else if (key.startsWith('term:')) selectAgent(Number(key.slice('term:'.length)), 'terminal');
    else if (key.startsWith('ws:')) {
      const w = wsList.find((x) => x.id === key.slice('ws:'.length));
      if (w) selectRepo(w);
    }
    closeDrawerIfNarrow();
  }

  function selectAgent(next: number, view: CentreView): void {
    if (cid !== next) {
      chat?.dispose();
      chat = null;
      activity?.dispose();
      activity = null;
      approvals?.dispose();
      approvals = null;
      cid = next;
    }
    centreView = view;
    const agent = agentList.find((a) => a.cid === next);
    const wsNext = wsList.find((w) => w.id === `agent:${next}`) ?? null;
    if (!chat) {
      const project = agent ? projectFor(agent, projects) : null;
      chat = chatPane({ cid: next, agentName: agent?.name ?? `agent ${next}`, project: project?.title ?? null, working: () => agentWorking });
    }
    sideEl.setSelected(`agent:${next}`);
    void setWorkspace(wsNext);
    if (!rightTabAllowed(rightView)) rightView = 'files';
    renderAll();
  }

  function selectRepo(info: IdeWorkspaceInfo): void {
    chat?.dispose();
    chat = null;
    activity?.dispose();
    activity = null;
    approvals?.dispose();
    approvals = null;
    cid = null;
    if (centreView === 'chat' || centreView === 'terminal') centreView = 'editor';
    if (!rightTabAllowed(rightView)) rightView = 'files';
    sideEl.setSelected(`ws:${info.id}`);
    void setWorkspace(info);
    renderAll();
  }

  /** The sidebar's "+ New agent": the agent wizard. */
  function newAgent(): void {
    go('agents');
  }

  // ---- Data -----------------------------------------------------------------
  /** Fetches every page of one directory and replaces what was loaded for it. */
  async function loadDir(dir: string): Promise<void> {
    if (!ws) return;
    const id = ws.id;
    const found: Entry[] = [];
    let cursor: string | null = null;
    do {
      const page = await ideApi.tree(id, dir, cursor);
      if (!alive || ws?.id !== id) return;
      found.push(...page.entries);
      cursor = page.next;
    } while (cursor);
    for (const path of [...entries.keys()]) if (parentDir(path) === dir) entries.delete(path);
    for (const e of found) entries.set(e.path, e);
    loadedDirs.add(dir);
  }

  /** Loads the directories above each changed file, so the changed-only tree can show them. */
  async function loadChangedDirs(): Promise<void> {
    const dirs = new Set<string>();
    for (const f of files) for (const d of dirsOf(f.path)) if (!loadedDirs.has(d)) dirs.add(d);
    for (const d of dirs) await loadDir(d).catch((err: unknown) => toast(message(err), 'error'));
    if (alive) renderAll();
  }

  async function refreshChanges(): Promise<void> {
    if (!ws || !ide) return;
    const id = ws.id;
    const base = baseValue();
    try {
      const c = await ideApi.changes(id, base);
      if (!alive || ws?.id !== id) return;
      files = c.files ?? ide.parseUnified(c.diff ?? '').files;
      stats = c.stats ?? totalsOf(files);
      changesError = '';
    } catch (err) {
      if (ws?.id === id) changesError = message(err);
      return;
    }
    // The changed-only tree needs the directories above the changed files.
    if (state?.filter_changed) await loadChangedDirs();
  }

  async function refreshCheckpoints(): Promise<void> {
    if (!ws) return;
    const id = ws.id;
    try {
      const list = await ideApi.checkpoints(id);
      if (alive && ws?.id === id) checkpoints = list;
    } catch {
      if (alive && ws?.id === id) checkpoints = [];
    }
  }

  /** Compares an open tab with the hub: a clean buffer reloads, an edited one shows the conflict. */
  async function checkOpen(path: string): Promise<void> {
    const t = tabsByPath.get(path);
    if (!ws || !t || t.buf.saving) return;
    const id = ws.id;
    try {
      const f = await ideApi.file(id, path);
      const cur = tabsByPath.get(path);
      if (!alive || ws?.id !== id || !cur || cur.buf.saving) return;
      const res = onRemote(cur.buf, f.hash);
      cur.buf = res.buffer;
      if (res.action === 'reload') await reloadTab(path);
    } catch {
      // The file is gone or refused: the tab keeps what it has.
    }
  }

  function baseValue(): string {
    const b = state?.base;
    return b !== undefined && typeof b === 'object' && 'checkpoint' in b ? `checkpoint:${b.checkpoint}` : 'work_start';
  }

  function basesOf(): { value: string; label: string }[] {
    return [{ value: 'work_start', label: 'Since work started' }, ...checkpoints.map((cp) => ({ value: `checkpoint:${cp.n}`, label: `#${cp.n} ${cp.label || 'checkpoint'}` }))];
  }

  function setBase(value: string): void {
    if (!ide || !state) return;
    state = ide.reduce(state, { set_base: toBase(value) });
    void refreshChanges().then(() => alive && renderAll());
  }

  function markViewed(path: string, mark: string, on: boolean): void {
    if (!ws) return;
    marks = withViewed(marks, path, mark, on);
    writeStore(viewedKey(ws.id), JSON.stringify(marks));
    renderAll();
  }

  // ---- Workspaces -----------------------------------------------------------
  /** Shows a workspace in the editor and the diff: loads its tree, changes and checkpoints. Null clears the view. */
  async function setWorkspace(info: IdeWorkspaceInfo | null): Promise<void> {
    if (info?.id === ws?.id && info !== null) return;
    disposeTerminal();
    run?.dispose();
    run = null;
    ws = info;
    entries.clear();
    loadedDirs.clear();
    expanded.clear();
    tabsByPath.clear();
    shown = null;
    files = [];
    stats = { files: 0, additions: 0, deletions: 0 };
    checkpoints = [];
    saved = false;
    if (!info || !ide) {
      state = ide ? ide.initialState() : null;
      renderAll();
      return;
    }
    writeStore(LAST_WORKSPACE_KEY, info.id);
    marks = parseViewed(readStore(viewedKey(info.id)));
    try {
      const canonical = ide.parseWorkspaceId(info.id);
      state = ide.reduce(ide.initialState(), { set_workspace: canonical });
    } catch (err) {
      toast(message(err), 'error');
      state = ide.initialState();
    }
    renderAll();
    try {
      await loadDir('');
      await Promise.all([refreshChanges(), refreshCheckpoints()]);
    } catch (err) {
      if (alive) toast(message(err), 'error');
    }
    if (!alive || ws?.id !== info.id) return;
    if (startFile) await openTab(startFile);
    renderAll();
  }

  // ---- Tabs -----------------------------------------------------------------
  /** Opens a file in a tab (or shows its tab) and, when asked, moves the cursor to a line. */
  async function openTab(path: string, line?: number): Promise<void> {
    if (!ws || !ide || !state) return;
    if (!tabsByPath.has(path)) {
      try {
        ide.checkRead(path);
      } catch (err) {
        toast(message(err), 'error');
        return;
      }
      const id = ws.id;
      try {
        const f = await ideApi.file(id, path);
        if (!alive || ws?.id !== id) return;
        const t: Tab = { buf: openBuffer(path, f.hash), state: null, version: 0 };
        t.state = stateFor(path, f.text, editorOpts(path, !ws.writable));
        tabsByPath.set(path, t);
      } catch (err) {
        toast(message(err), 'error');
        return;
      }
    }
    state = ide.reduce(state, { open: path });
    state = ide.reduce(state, { activate: path });
    centreView = 'editor';
    showTab();
    if (line !== undefined) editor.reveal(line);
    renderAll();
  }

  /** The editor options of one file: read-only when the workspace is, and the edit and cursor callbacks. */
  function editorOpts(path: string, readOnly: boolean) {
    return { readOnly, onEdit: () => onEdit(path), onSelect: () => renderStatus() };
  }

  /** Makes the editor show the reducer's active tab, keeping the state of the tab it leaves. */
  function showTab(): void {
    const want = state?.active ?? null;
    if (want === shown) return;
    if (shown !== null) {
      const leaving = tabsByPath.get(shown);
      if (leaving) leaving.state = editor.current();
    }
    shown = want;
    if (want === null) return;
    const t = tabsByPath.get(want);
    if (t?.state) {
      editor.show(t.state);
      t.state = null;
    }
  }

  async function closeTab(path: string): Promise<void> {
    if (!ide || !state) return;
    const t = tabsByPath.get(path);
    if (t?.buf.dirty && !(await confirmDiscard(path))) return;
    if (shown === path) shown = null;
    tabsByPath.delete(path);
    state = ide.reduce(state, { close: path });
    showTab();
    renderAll();
  }

  function confirmDiscard(path: string): Promise<boolean> {
    return new Promise((resolve) => {
      let answered = false;
      const answer = (ok: boolean) => {
        if (answered) return;
        answered = true;
        close();
        resolve(ok);
      };
      const close = modal(
        'Discard your edits?',
        h(
          'div',
          null,
          h('p', null, `${path} has unsaved changes. Closing the tab drops them.`),
          h('div.actions', null, button('Discard', () => answer(true), { kind: 'danger' }), button('Keep editing', () => answer(false), { kind: 'ghost' })),
        ),
        () => answer(false),
      );
    });
  }

  function onEdit(path: string): void {
    const t = tabsByPath.get(path);
    if (!t) return;
    t.buf = applyBuffer(t.buf, { kind: 'edit' });
    t.version++;
    saved = false;
    renderTabs();
    renderTop();
    renderStatus();
  }

  // ---- Save and conflicts ---------------------------------------------------
  async function save(): Promise<void> {
    const path = state?.active ?? null;
    const t = path !== null ? tabsByPath.get(path) : undefined;
    if (!ws || !ide || path === null || !t || !ws.writable || !t.buf.dirty || t.buf.saving) return;
    try {
      ide.checkWrite(path);
    } catch (err) {
      toast(message(err), 'error');
      return;
    }
    const id = ws.id;
    const text = editor.current().doc.toString();
    const version = t.version;
    t.buf = applyBuffer(t.buf, { kind: 'save' });
    renderAll();
    try {
      const r = await ideApi.save(id, { path, text, expected_hash: t.buf.base });
      const cur = tabsByPath.get(path);
      if (!alive || ws?.id !== id || !cur) return;
      cur.buf = applyBuffer(cur.buf, { kind: 'saved', hash: r.hash, edited: cur.version !== version });
      saved = true;
      await refreshChanges();
    } catch (err) {
      const cur = tabsByPath.get(path);
      if (!cur || ws?.id !== id) return;
      if (err instanceof ApiError && err.status === 409) {
        const current = (err.body as { current_hash?: unknown } | null)?.current_hash;
        if (typeof current === 'string') cur.buf = applyBuffer(cur.buf, { kind: 'conflict', current });
        else cur.buf = applyBuffer(cur.buf, { kind: 'save_failed' });
        if (typeof current !== 'string') void checkOpen(path);
      } else {
        cur.buf = applyBuffer(cur.buf, { kind: 'save_failed' });
        toast(message(err), 'error');
      }
    }
    renderAll();
  }

  /** "Reload": the buffer takes the hub's copy, and its edits go. */
  async function reloadTab(path: string): Promise<void> {
    const t = tabsByPath.get(path);
    if (!ws || !t) return;
    const id = ws.id;
    try {
      const f = await ideApi.file(id, path);
      const cur = tabsByPath.get(path);
      if (!alive || ws?.id !== id || !cur) return;
      cur.buf = applyBuffer(cur.buf, { kind: 'reload', hash: f.hash });
      if (shown === path) editor.setText(f.text);
      else cur.state = stateFor(path, f.text, editorOpts(path, !ws.writable));
    } catch (err) {
      toast(message(err), 'error');
    }
    renderAll();
  }

  /** "Keep mine": the buffer re-bases on the hub's copy and saves its edits over it. */
  async function keepMine(path: string): Promise<void> {
    const t = tabsByPath.get(path);
    if (!t || t.buf.conflict === null) return;
    t.buf = applyBuffer(t.buf, { kind: 'keep_mine', current: t.buf.conflict });
    await save();
  }

  // ---- Live -----------------------------------------------------------------
  function scheduleRefresh(path?: string): void {
    if (path) pendingPaths.add(path);
    else pendingAll = true;
    clearTimeout(refreshTimer);
    refreshTimer = setTimeout(() => {
      const paths = new Set(pendingPaths);
      const all = pendingAll;
      pendingPaths.clear();
      pendingAll = false;
      void refreshFor(paths, all);
    }, REFRESH_MS);
  }

  async function refreshFor(paths: Set<string>, all: boolean): Promise<void> {
    if (!ws || !alive) return;
    await refreshChanges();
    const dirs = new Set<string>();
    for (const p of paths) if (loadedDirs.has(parentDir(p))) dirs.add(parentDir(p));
    for (const d of dirs) await loadDir(d).catch(() => undefined);
    for (const p of all ? [...tabsByPath.keys()] : [...paths]) if (tabsByPath.has(p)) await checkOpen(p);
    if (alive) renderAll();
  }

  /** The project and the task list follow the console's own polls: the sidebar, the board and the tasks tab. */
  async function refreshProjects(): Promise<void> {
    try {
      projects = await api.projects();
    } catch {
      // The sidebar shows what it has; the next poll tries again.
    }
    if (alive) renderAll();
  }

  const unsub = [
    on('ide', (data) => {
      const e = data as { workspace?: unknown; path?: unknown };
      if (ws && e.workspace === ws.id) scheduleRefresh(typeof e.path === 'string' ? e.path : undefined);
    }),
    on('transcript', (data) => agentFrame(data)),
    on('work', (data) => agentFrame(data)),
  ];
  function agentFrame(data: unknown): void {
    const frameCid = (data as { cid?: unknown } | null)?.cid;
    if (ws && cidOf(ws.id) !== null && (typeof frameCid !== 'number' || frameCid === cidOf(ws.id))) scheduleRefresh();
  }
  // The agents' roster: the sidebar's and the palette's list, and the selected agent's working state. When it stops, its writes are in.
  const unwatch = agents.watch((list) => {
    agentList = list;
    const working = cid !== null && list.some((a) => a.cid === cid && a.status === 'working');
    if (wasWorking && !working) scheduleRefresh();
    wasWorking = working;
    agentWorking = working;
    if (alive) renderAll();
  });
  // The plaza's held state and the live connection: the header's badge, the status bar.
  const unheld = onPlazaHeld((v) => {
    held = v;
    if (alive) renderTop();
  });
  const unconnect = onConnection((up) => {
    connected = up;
    if (alive) renderStatus();
  });
  const projectsTimer = setInterval(() => void refreshProjects(), 20_000);

  // ---- Keys and overlays ----------------------------------------------------
  function openQuick(): void {
    if (!ws) return;
    closeOverlay();
    overlay = quickOpen(
      root,
      () => [...entries.values()].filter((e) => e.kind === 'file').map((e) => e.path),
      (path) => {
        closeDrawerIfNarrow();
        void openTab(path);
      },
      () => (overlay = null),
    );
  }

  /** Ctrl/Cmd+Shift+F and the Files tab's search button: the search view, shown above the tree. */
  function openSearch(): void {
    if (!ws) return;
    rightView = 'files';
    if (!layout.rightOpen) setRightOpen(true);
    searchHost.hidden = false;
    renderAll();
    searchUi()?.focus();
  }

  function toggleSearch(): void {
    if (!searchHost.hidden) {
      searchHost.hidden = true;
      renderAll();
      return;
    }
    openSearch();
  }

  /** The search view for the open workspace (made once per workspace, so its results stay). */
  function searchUi(): { el: HTMLElement; focus: () => void } | null {
    if (!ws) return null;
    if (!search || search.wsId !== ws.id) {
      const id = ws.id;
      search = {
        wsId: id,
        ui: searchView(
          (q, glob) => ideApi.search(id, q, glob || undefined),
          (path, line) => {
            closeDrawerIfNarrow();
            void openTab(path, line);
          },
        ),
      };
    }
    return search.ui;
  }

  function closeOverlay(): void {
    const close = overlay;
    overlay = null;
    close?.();
  }

  /** Console: back to the screen the IDE was opened from. */
  function leaveIde(): void {
    closeOverlay();
    location.hash = consoleOrigin();
  }

  /** The palette's items: agents, repositories, the loaded files, the tasks and the commands. */
  function paletteItems(): PaletteItem[] {
    const items: PaletteItem[] = [];
    for (const a of agentList) {
      const project = projectFor(a, projects);
      items.push({ id: `agent:${a.cid}`, group: 'agent', label: a.name, hint: `${rowStatus(a, null)}${project ? ` · ${project.title}` : ''}` });
    }
    for (const w of wsList) if (w.kind === 'repo') items.push({ id: `ws:${w.id}`, group: 'agent', label: repoNameOf(w), hint: `repository ${refOf(w)}` });
    for (const e of entries.values()) if (e.kind === 'file') items.push({ id: `file:${e.path}`, group: 'file', label: e.path.slice(e.path.lastIndexOf('/') + 1), hint: e.path });
    for (const t of tasks) items.push({ id: `task:${t.id}`, group: 'task', label: t.goal, hint: t.state });
    const commands: [string, string][] = [
      ['chat', 'Show the chat'],
      ['terminal', 'Show the terminal'],
      ['editor', 'Show the editor'],
      ['diff', 'Show the diff'],
      ['board', 'Show the board'],
      ['files', 'Show the files'],
      ['tasks', 'Show the tasks'],
      ['approvals', 'Show the approvals'],
      ['activity', 'Show the activity'],
      ['drawer', 'Toggle the terminal drawer'],
      ['sidebar', 'Toggle the sidebar'],
      ['right', 'Toggle the right panel'],
      ['search', 'Search the workspace'],
      ['save', 'Save the file'],
      ['console', 'Back to the console'],
    ];
    for (const [id, label] of commands) items.push({ id: `cmd:${id}`, group: 'command', label });
    return items;
  }

  /** Opens the palette with fresh tasks (they are fetched when it opens). */
  async function openPalette(): Promise<void> {
    if (pal.isOpen()) {
      pal.close();
      return;
    }
    try {
      tasks = await api.tasks();
    } catch {
      // The palette shows the tasks it has.
    }
    pal.open(paletteItems());
  }

  function runPaletteItem(item: PaletteItem): void {
    const at = item.id.indexOf(':');
    const kind = item.id.slice(0, at);
    const value = item.id.slice(at + 1);
    if (kind === 'agent') selectAgent(Number(value), 'chat');
    else if (kind === 'ws') {
      const w = wsList.find((x) => x.id === value);
      if (w) selectRepo(w);
    } else if (kind === 'file') void openTab(value);
    else if (kind === 'task') setRight('tasks');
    else if (kind === 'cmd') runCommand(value);
  }

  function runCommand(id: string): void {
    const views: Record<string, CentreView> = { chat: 'chat', terminal: 'terminal', editor: 'editor', diff: 'diff' };
    const rights: Record<string, RightView> = { board: 'board', files: 'files', tasks: 'tasks', approvals: 'approvals', activity: 'activity' };
    if (views[id]) {
      if (centreAllowed(views[id])) setCentre(views[id]);
    } else if (rights[id]) setRight(rights[id]);
    else if (id === 'drawer') setDrawer(!drawerOpen);
    else if (id === 'sidebar') setLeft(!layout.leftOpen);
    else if (id === 'right') setRightOpen(!layout.rightOpen);
    else if (id === 'search') openSearch();
    else if (id === 'save') void save();
    else if (id === 'console') leaveIde();
  }

  const onKey = (e: KeyboardEvent) => {
    if (!alive) return;
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.altKey && !e.shiftKey && e.code === 'KeyB') {
      e.preventDefault();
      setLeft(!layout.leftOpen);
    } else if (mod && e.altKey && !e.shiftKey && e.code === 'KeyJ') {
      e.preventDefault();
      setRightOpen(!layout.rightOpen);
    } else if (mod && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      void openPalette();
    } else if (!mod && !e.altKey && !e.shiftKey && e.code === 'Backquote' && !inTyping(e.target) && cid !== null) {
      e.preventDefault();
      setDrawer(!drawerOpen);
    } else if (mod && !e.altKey && !e.shiftKey && e.code === 'Backquote' && !inTerminal(e.target) && cid !== null) {
      e.preventDefault();
      setDrawer(!drawerOpen);
    } else if (mod && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 's' && !inTerminal(e.target)) {
      e.preventDefault();
      void save();
    } else if (mod && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'p' && !inTerminal(e.target)) {
      e.preventDefault();
      openQuick();
    } else if (mod && !e.altKey && e.shiftKey && e.key.toLowerCase() === 'f' && !inTerminal(e.target)) {
      e.preventDefault();
      openSearch();
    } else if (e.key === '?' && !mod && !e.altKey && !inTyping(e.target)) {
      e.preventDefault();
      help.set(!help.isOpen());
    } else if (e.key === 'Escape' && !e.defaultPrevented && overlay === null && !pal.isOpen() && !document.querySelector('.modal-backdrop') && !inField(e.target)) {
      // A field's Escape (the editor's search, a note) is the field's. Otherwise: the help, then a narrow drawer, then the console.
      if (help.isOpen()) help.set(false);
      else if ((narrow && layout.leftOpen) || (mid && layout.rightOpen)) closeDrawerIfNarrow();
      else leaveIde();
    }
  };
  document.addEventListener('keydown', onKey);

  // ---- Panels ---------------------------------------------------------------
  /** The project of the current selection: the agent's, or the repository's. */
  function currentProject(): Project | null {
    if (cid !== null) {
      const a = agentList.find((x) => x.cid === cid);
      return a ? projectFor(a, projects) : null;
    }
    if (ws?.kind === 'repo') return projects.find((p) => p.repo === repoNameOf(ws as IdeWorkspaceInfo)) ?? null;
    return null;
  }

  /** The controller of an agent's run tab or approvals, made on first use. */
  function approvalsFor(): TabPanel | null {
    if (cid === null) return null;
    if (!approvals) {
      approvals = approvalsPanel({
        wsId: `agent:${cid}`,
        cid,
        onCount: (n) => {
          approvalCount = n;
          renderRightTabs();
        },
        onDecided: () => scheduleRefresh(),
      });
    }
    return approvals;
  }

  function runFor(): TabPanel | null {
    if (!ws) return null;
    if (!run) run = runPanel(ws);
    return run;
  }

  /** The right panel's body: the current tab's panel (made and shown on first use), or a note. */
  function renderRight(): void {
    const project = currentProject();
    if (!ws && cid === null) {
      replace(frame.right.body, empty('agents', 'Choose a project or an agent', 'The board, files, tasks and activity follow the selection.'));
    } else if (rightView === 'board') {
      const key = project?.id ?? '';
      if (!board || boardKey !== key) {
        board?.dispose();
        board = boardPanel({
          project,
          onCount: (n) => {
            boardCount = n;
            renderRightTabs();
          },
        });
        boardKey = key;
      }
      replace(frame.right.body, board.el);
      board.show();
    } else if (rightView === 'files') {
      replace(frame.right.body, filesEl);
      renderFiles();
    } else if (rightView === 'tasks') {
      const key = project?.id ?? '';
      if (!taskPanel || taskKey !== key) {
        taskPanel?.dispose();
        taskPanel = tasksPanel({
          project,
          onCount: (done, total) => {
            taskCounts = { done, total };
            renderRightTabs();
          },
        });
        taskKey = key;
      }
      replace(frame.right.body, taskPanel.el);
      taskPanel.show();
    } else if (rightView === 'approvals') {
      const p = approvalsFor();
      replace(frame.right.body, p ? p.el : empty('approval', 'No approvals here', 'Approvals are per agent.'));
      p?.show?.();
    } else if (rightView === 'activity') {
      if (cid === null) replace(frame.right.body, empty('loop', 'No activity here', 'Activity is per agent.'));
      else {
        if (!activity) activity = activityPanel({ cid, agentName: agentList.find((a) => a.cid === cid)?.name ?? `agent ${cid}` });
        replace(frame.right.body, activity.el);
        activity.show();
      }
    }
  }

  function renderRightTabs(): void {
    const counts: Partial<Record<RightView, string | null>> = {
      board: boardCount !== null && boardCount > 0 ? String(boardCount) : null,
      files: files.length > 0 ? String(files.length) : null,
      tasks: taskCounts ? `${taskCounts.done}/${taskCounts.total}` : null,
      approvals: approvalCount > 0 ? String(approvalCount) : null,
      activity: null,
    };
    paintTabs(frame.right.tabs, rightView, counts);
    frame.right.tabs.approvals.hidden = cid === null;
    frame.right.tabs.activity.hidden = cid === null;
  }

  /** The Files tab's tree, its changed-only filter and the checkpoints. */
  function renderFiles(): void {
    if (!ide) return;
    const filter = !!state?.filter_changed;
    changedBox.checked = filter;
    const all = [...entries.values()];
    const keep = filter ? new Set(ide.changedFilter(all, { files }).map((e) => e.path)) : null;
    const rows = treeRows({
      ide,
      all,
      loaded: loadedDirs,
      expanded,
      changed: new Set(files.map((f) => f.path)),
      keep,
      active: state?.active ?? null,
      hasWorkspace: ws !== null,
      onOpen: (path) => {
        closeDrawerIfNarrow();
        void openTab(path);
      },
      onToggle: (path) => void toggleDir(path),
    });
    if (ws && rows.length === 0) rows.push(h('p.ide-note', null, filter ? 'No changed files in the loaded tree.' : 'The workspace is empty.'));
    replace(treeList, rows);
    if (!ws) replace(treeList, empty('file', 'No workspace', 'Pick an agent or a repository to see its files.'));
    if (!searchHost.hidden) {
      const s = searchUi();
      replace(searchHost, s ? s.el : h('p.ide-note', null, 'Search runs in a workspace.'));
    }
    replace(cpHost, ws ? checkpointsPanel({ checkpoints, canRestore: true, onRestore: confirmRestore }) : null);
  }

  async function toggleDir(path: string): Promise<void> {
    if (expanded.has(path)) expanded.delete(path);
    else {
      expanded.add(path);
      if (!loadedDirs.has(path)) await loadDir(path).catch((err: unknown) => toast(message(err), 'error'));
    }
    renderAll();
  }

  /** The review in the centre: the changes since the base, with the base picker and the viewed marks. */
  function reviewView(): HTMLElement {
    return h(
      'div.ide-review-body',
      null,
      changesError ? h('p.ide-note.error', null, changesError) : null,
      changesPanel({
        files,
        stats,
        base: baseValue(),
        bases: basesOf(),
        marks,
        onBase: setBase,
        onOpen: openFromChanges,
        onMark: markViewed,
      }),
    );
  }

  /** From the review: the editor shows the file. */
  function openFromChanges(path: string): void {
    void openTab(path);
  }

  // ---- Terminal -------------------------------------------------------------
  function disposeTerminal(): void {
    term?.dispose();
    term = null;
    termHost.remove();
  }

  /**
   * Puts the terminal where it belongs: in the centre's Terminal view while that view shows, else in the drawer while the
   * drawer shows the Terminal tab, else nowhere (detached, still alive). It is made the first time it is wanted, in an
   * agent workspace; a move that is not needed is not made, so a redraw does not move focus or the xterm DOM.
   */
  function renderTerminal(): void {
    const agentWs = ws !== null && cidOf(ws.id) !== null;
    const inCentre = agentWs && centreView === 'terminal';
    const inDrawer = agentWs && !inCentre && drawerOpen && drawerTab === 'terminal';
    const home = inCentre ? frame.centre.views.terminal : inDrawer ? frame.drawer.body : null;
    if (home) {
      if (termHost.parentElement !== home) home.append(termHost);
    } else {
      termHost.remove();
    }
    if (ws && home && !term && agentWs) term = terminalTabs(termHost, ws.id);
  }

  // ---- Rendering ------------------------------------------------------------
  function renderAll(): void {
    if (!alive) return;
    renderSelection();
    renderTop();
    renderCentre();
    renderRightTabs();
    renderRight();
    renderDrawer();
    renderTabs();
    renderBanners();
    renderTerminal();
    renderStatus();
    applyLayout();
  }

  /** The centre's header: the dot, the title, the view toggle and the pop-out. */
  function renderSelection(): void {
    const agent = cid !== null ? agentList.find((a) => a.cid === cid) : undefined;
    const title = agent?.name ?? (ws?.kind === 'repo' ? repoNameOf(ws) : cid !== null ? `agent ${cid}` : 'No selection');
    frame.centre.title.textContent = title;
    frame.centre.dot.className = `iw-dot ${agent ? rowStatus(agent, null) : 'idle'}`;
    frame.centre.popout.hidden = cid === null;
    for (const v of VIEWS) {
      const btn = frame.centre.seg[v];
      btn.hidden = (v === 'chat' || v === 'terminal') && cid === null;
      btn.querySelector('.iw-seg-label')?.replaceChildren(CENTRE_LABEL[v]);
    }
    paintTabs(frame.centre.seg, centreView, {});
    if (!centreAllowed(centreView)) centreView = 'editor';
    for (const v of VIEWS) frame.centre.views[v].hidden = v !== centreView;
    if (chat && frame.centre.views.chat.firstElementChild !== chat.el) replace(frame.centre.views.chat, chat.el);
    if (cid === null && !ws) replace(frame.centre.views.editor, [empty('file', 'Choose a project or an agent', 'Pick an agent in the sidebar, or a repository. Ctrl/Cmd+K opens the palette.')]);
  }

  function renderTop(): void {
    const conflict = [...tabsByPath.values()].some((t) => t.buf.conflict !== null);
    const label: string = held ? 'held' : badgeOf({ conflict, agentWorking, saved });
    const project = currentProject();
    const parts = ws
      ? [project?.title ?? (cid !== null ? 'Unassigned' : 'Repositories'), ws.kind === 'agent' ? (agentList.find((a) => a.cid === cid)?.name ?? `agent ${cid}`) : repoNameOf(ws), refOf(ws) || 'work'].filter(Boolean)
      : ['IDE'];
    setCrumb(frame.crumb, parts);
    frame.state.replaceChildren(badge(label, TONE[label] ?? 'muted'));
    const active = state?.active ?? null;
    const t = active !== null ? tabsByPath.get(active) : undefined;
    saveBtn.disabled = !(ws?.writable && t?.buf.dirty && !t.buf.saving);
  }

  function renderTabs(): void {
    const items = (state?.open ?? []).map((path) => {
      const t = tabsByPath.get(path);
      const name = path.slice(path.lastIndexOf('/') + 1);
      const on = path === state?.active;
      return h(
        'div.ide-tab',
        { class: on ? 'on' : '', role: 'tab', 'aria-selected': on ? 'true' : 'false', title: path },
        h(
          'button.ide-tab-name',
          { type: 'button', onclick: () => activate(path) },
          t?.buf.conflict != null ? h('span.ide-conflict-mark', { title: 'Conflict' }, '!') : null,
          t?.buf.dirty ? h('span.ide-dirty', { title: 'Unsaved changes' }, '●') : null,
          name,
        ),
        h('button.ide-tab-x', { type: 'button', title: `Close ${name}`, 'aria-label': `Close ${name}`, onclick: () => void closeTab(path) }, '×'),
      );
    });
    replace(tabBar, items);
  }

  function activate(path: string): void {
    if (!ide || !state) return;
    state = ide.reduce(state, { activate: path });
    showTab();
    renderAll();
  }

  function renderBanners(): void {
    const active = state?.active ?? null;
    const t = active !== null ? tabsByPath.get(active) : undefined;
    const rows: Node[] = [];
    if (active !== null && t?.buf.conflict != null) {
      rows.push(
        h(
          'div.ide-banner.conflict',
          { role: 'alert' },
          h('span', null, `${active} changed on the agent's side while you had unsaved edits. Reload replaces your edits with that copy.`),
          h('span.spacer'),
          button('Reload', () => void reloadTab(active), { size: 'sm' }),
          button('Keep mine', () => void keepMine(active), { kind: 'primary', size: 'sm' }),
        ),
      );
    }
    if (ws && !ws.writable) rows.push(h('div.ide-banner', null, 'This workspace is read-only.'));
    if (agentWorking && cid !== null) rows.push(h('div.ide-banner', null, 'The agent is working in this workspace. A save over its newer write shows a conflict.'));
    replace(banners, rows);
  }

  /** The centre's editor and diff: the editor shows the active file or a note; the diff shows the changes. */
  function renderCentre(): void {
    const has = !!state?.active;
    const editing = centreView === 'editor' && !!ws;
    cmHost.hidden = !editing || !has;
    emptyHost.hidden = !editing || has;
    tabBar.hidden = !editing;
    banners.hidden = !editing;
    editorBar.hidden = !editing;
    if (editing && !has) {
      replace(emptyHost, empty('file', 'No file open', 'Pick a file in the Files tab, or press Ctrl/Cmd+P to find one by name.'));
    }
    if (!ws && centreView === 'editor' && cid !== null) replace(emptyHost, empty('file', 'No workspace for this agent', 'This agent has no workspace yet.'));
    if (centreView === 'diff') {
      replace(reviewHost, ws ? reviewView() : empty('git', 'Choose a workspace', 'The changes are per workspace.'));
      if (focusPath !== null) {
        const path = focusPath;
        focusPath = null;
        const block = [...reviewHost.querySelectorAll<HTMLElement>('[data-path]')].find((el) => el.dataset.path === path);
        if (block instanceof HTMLDetailsElement) block.open = true;
        block?.scrollIntoView({ block: 'start' });
      }
    }
  }

  /** The bottom drawer: its tab strip, and the run panel while the Run tab shows. */
  function renderDrawer(): void {
    frame.drawer.el.hidden = !drawerOpen;
    paintTabs(frame.drawer.tabs, drawerTab, {});
    if (!drawerOpen) return;
    const body = frame.drawer.body;
    if (drawerTab === 'run') {
      const p = runFor();
      if (p && body.firstElementChild !== p.el) replace(body, p.el);
      p?.show?.();
    } else if (cid === null) {
      replace(body, empty('terminal', 'Terminals run in agent workspaces', 'Pick an agent to open its terminal.'));
    } else if (body.firstElementChild !== termHost) {
      // The Run panel leaves the drawer; the terminal (if it belongs here) is placed by renderTerminal.
      body.replaceChildren();
    }
  }

  function renderStatus(): void {
    const active = state?.active ?? null;
    let cursor: StatusInfo['cursor'] = null;
    if (active !== null && centreView === 'editor') {
      const s = editor.current();
      const at = s.selection.main.head;
      const line = s.doc.lineAt(at);
      cursor = { line: line.number, col: at - line.from + 1 };
    }
    const base = state?.base;
    const vs = base !== undefined && typeof base === 'object' && 'checkpoint' in base ? `vs checkpoint #${base.checkpoint}` : 'vs work start';
    status.paint({
      workspace: ws?.id ?? '',
      base: ws ? vs : '',
      live: connected,
      cursor,
      lang: active !== null ? LANG_LABEL[languageFor(active)] : '',
      dirty: [...tabsByPath.values()].filter((t) => t.buf.dirty).length,
    });
  }

  function confirmRestore(cp: IdeCheckpoint): void {
    const close = modal(
      `Restore checkpoint #${cp.n}?`,
      h(
        'div',
        null,
        h('p', null, 'The workspace files go back to this checkpoint. Changes made after it are lost. Open files are reloaded.'),
        h(
          'div.actions',
          null,
          button(
            'Restore',
            () => {
              close();
              void restore(cp);
            },
            { kind: 'danger' },
          ),
          button('Cancel', () => close(), { kind: 'ghost' }),
        ),
      ),
    );
  }

  async function restore(cp: IdeCheckpoint): Promise<void> {
    if (!ws) return;
    const id = ws.id;
    try {
      await ideApi.restore(id, cp.n);
      toast(`Restored checkpoint #${cp.n}`);
    } catch (err) {
      toast(message(err), 'error');
      return;
    }
    await Promise.all([refreshChanges(), refreshCheckpoints()]);
    for (const d of [...loadedDirs]) await loadDir(d).catch(() => undefined);
    for (const p of [...tabsByPath.keys()]) await checkOpen(p);
    if (alive && ws?.id === id) renderAll();
  }

  // ---- Start ----------------------------------------------------------------
  function fatal(text: string): void {
    replace(frame.centre.el, empty('alert', 'The IDE is not available', text));
  }

  /** The panel a link asks for (openIde's `panel`), mapped onto the new places. */
  function applyStartPanel(): void {
    if (startPanel === 'changes' || query.get('review') === '1') centreView = 'diff';
    else if (startPanel === 'agent') centreView = 'chat';
    else if (startPanel === 'terminal') centreView = 'terminal';
    if (startPanel === 'checkpoints' || startPanel === 'files') rightView = 'files';
    if (startPanel === 'approvals') rightView = 'approvals';
    if (startPanel === 'run') {
      drawerOpen = true;
      drawerTab = 'run';
    }
  }

  void (async () => {
    try {
      ide = await loadIde();
    } catch (err) {
      if (alive) fatal(`The IDE model did not load: ${message(err)}`);
      return;
    }
    try {
      wsList = await ideApi.workspaces();
    } catch (err) {
      if (alive) fatal(message(err));
      return;
    }
    if (!alive) return;
    state = ide.initialState();
    await refreshProjects();
    try {
      tasks = await api.tasks();
    } catch {
      tasks = [];
    }
    const want = requested ?? readStore(LAST_WORKSPACE_KEY);
    const chosen = want !== null ? wsList.find((w) => w.id === want) : undefined;
    if (requested !== null && !chosen) toast(`No workspace ${requested}`, 'error');
    applyStartPanel();
    renderAll();
    if (chosen?.kind === 'agent') selectAgent(cidOf(chosen.id) as number, centreView);
    else if (chosen) selectRepo(chosen);
  })();

  return () => {
    alive = false;
    clearTimeout(refreshTimer);
    clearInterval(projectsTimer);
    document.removeEventListener('keydown', onKey);
    narrowMq.removeEventListener('change', onBreakpoint);
    midMq.removeEventListener('change', onBreakpoint);
    window.removeEventListener('resize', applyLayout);
    for (const u of unsub) u();
    unwatch();
    unheld();
    unconnect();
    chat?.dispose();
    activity?.dispose();
    approvals?.dispose();
    run?.dispose();
    board?.dispose();
    taskPanel?.dispose();
    sideEl.dispose();
    pal.dispose();
    disposeTerminal();
    closeOverlay();
    editor.destroy();
  };
}
