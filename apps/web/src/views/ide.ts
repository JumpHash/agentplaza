// The IDE screen (route #/ide and #/ide/<workspace>): a full-window view of its own, with no console shell beside it. Its
// header (the workspace, the state, the actions), an activity rail that picks the side panel (files, search, changes,
// checkpoints), the editor tabs or the review in the centre, a right panel (agent, approvals, run, terminal) and a status
// bar. The terminal can dock under the editor instead (the centre's bottom dock). This module owns the data; the chrome is ide-chrome, the editor ide-editor, the panels ide-side, ide-find and
// ide-tabs. It loads on the first visit (views/index.ts), with CodeMirror. openIde (ide/open.ts) is how other screens get
// here, and the Console button (and Escape) goes back to the screen it came from.

import type { EditorState } from '@codemirror/state';
import ideCss from '../styles/ide.css';
import { ApiError, ideApi, type IdeChanges, type IdeCheckpoint, type IdeWorkspaceInfo } from '../api';
import { go } from '../router';
import { consoleOrigin } from '../ide/open';
import {
  applyBuffer,
  badgeOf,
  type Buffer,
  dirsOf,
  languageFor,
  lineCounts,
  LAST_WORKSPACE_KEY,
  onRemote,
  openBuffer,
  parentDir,
  parseViewed,
  viewedKey,
  withViewed,
  type Viewed,
} from '../ide/model';
import { fitSize, LEFT_SIZE, leftViewFor, type LeftView, NARROW_QUERY, parseLayout, rightTabFor, RIGHT_SIZE, RAIL_WIDTH, type RightTab, serializeLayout, type Layout } from '../ide/layout';
import { loadIde, type Ide } from '../ide/wasm';
import type { Base, Entry, FileChange, IdeState, Panel, Stats } from '../ide/types';
import { agents, on, onConnection } from '../store';
import { brandMark } from '../ui/brand';
import { badge, button, empty, modal, toast, type Tone } from '../ui/components';
import { h, icon, replace } from '../ui/dom';
import { heldBanner, onPlazaHeld } from '../ui/plaza-control';
import { themeSwitch } from '../ui/theme';
import type { Cleanup } from './index';
import { Editor, stateFor } from './ide-editor';
import { quickOpen, searchView } from './ide-find';
import { activityRail, dockEdge, keyboardHelp, LANG_LABEL, panelEdge, type RightTabItem, rightTabStrip, statusBar, type StatusInfo } from './ide-chrome';
import { changedList, changesPanel, checkpointsPanel } from './ide-side';
import { agentPanel, approvalsPanel, runPanel, type Panel as TabPanel } from './ide-tabs';
import { terminalTabs, type TabsHandle } from './ide-terminal';

const STYLE_ID = 'apz-ide-style';
const REFRESH_MS = 400;
/** The panel sizes and open flags, kept per browser. */
const LAYOUT_KEY = 'apz.ide.layout';
/** Where the terminal sits (the right panel or under the editor) and the dock's height, kept per browser. */
const TERM_KEY = 'apz.ide.terminal';
/** The dock's height limits: the editor keeps `editorMin` px above it. */
const DOCK = { min: 120, def: 240, max: 1200, editorMin: 160, step: 24 };

type TermDock = 'side' | 'bottom';

function readTermPrefs(): { dock: TermDock; height: number } {
  try {
    const v = JSON.parse(readStore(TERM_KEY) ?? 'null') as { dock?: unknown; height?: unknown } | null;
    const height = typeof v?.height === 'number' && Number.isFinite(v.height) ? Math.min(Math.max(v.height, DOCK.min), DOCK.max) : DOCK.def;
    return { dock: v?.dock === 'bottom' ? 'bottom' : 'side', height: Math.round(height) };
  } catch {
    return { dock: 'side', height: DOCK.def };
  }
}

/** One open file: its buffer, and its editor state while another tab is shown (the shown tab's state is the view's). */
interface Tab {
  buf: Buffer;
  state: EditorState | null;
  /** Counts the user's edits, so a save that raced an edit knows the buffer is still dirty. */
  version: number;
}

const TONE: Record<string, Tone> = { conflict: 'danger', 'agent working': 'info', saved: 'ok', idle: 'muted', held: 'danger' };

/** The stylesheet, inserted once on the first visit (build.mjs makes it text). */
function ensureStyle(): void {
  if (document.getElementById(STYLE_ID)) return;
  const style = document.createElement('style');
  style.id = STYLE_ID;
  style.textContent = ideCss;
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

/** The agent's cid for an `agent:<cid>` workspace, else null. */
const cidOf = (id: string): number | null => {
  const m = /^agent:(\d+)$/.exec(id);
  return m ? Number(m[1]) : null;
};

const message = (err: unknown): string => (err instanceof Error ? err.message : String(err));

function toBase(v: string): Base {
  return v.startsWith('checkpoint:') ? { checkpoint: Number(v.slice('checkpoint:'.length)) } : 'work_start';
}

/** A repository workspace's branch (the ref after the @), else ''. */
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
  let ws: IdeWorkspaceInfo | null = null;
  let state: IdeState | null = null;
  let changes: IdeChanges | null = null;
  let changesError = '';
  let files: FileChange[] = [];
  let stats: Stats = { files: 0, additions: 0, deletions: 0 };
  let checkpoints: IdeCheckpoint[] = [];
  let marks: Viewed = {};
  /** The side panel's view, and the right panel's tab. A link's `panel` picks one of them and opens its panel. */
  let leftView: LeftView = leftViewFor(startPanel) ?? 'files';
  let rightTab: RightTab = rightTabFor(startPanel) ?? 'agent';
  const narrowMq = matchMedia(NARROW_QUERY);
  let narrow = narrowMq.matches;
  let layout: Layout = parseLayout(readStore(LAYOUT_KEY), window.innerWidth);
  if (leftViewFor(startPanel)) layout.leftOpen = true;
  if (rightTabFor(startPanel)) layout.rightOpen = true;
  /** Review mode from openIde: applied once, to the first workspace this screen opens. */
  let reviewPending = query.get('review') === '1';
  /** The file whose diff the review scrolls to, once it is drawn. */
  let focusPath: string | null = null;
  /** The Agent, Approvals and Run tabs' controllers, per workspace: a tab switch keeps a draft or a running command. */
  let panels: { agent: TabPanel | null; approvals: TabPanel | null; run: TabPanel | null } = { agent: null, approvals: null, run: null };
  let approvalCount = 0;
  /** The terminal's place and size. The terminals themselves are made on first need, per agent workspace (see renderTerminal). */
  const termPrefs = readTermPrefs();
  let dock: TermDock = termPrefs.dock;
  let dockHeight = termPrefs.height;
  let term: TabsHandle | null = null;
  let saved = false;
  let agentWorking = false;
  let wasWorking = false;
  let held = false;
  let connected = true;
  let overlay: (() => void) | null = null;
  /** The file the editor shows (the reducer's `active` is the tab; this is what the view holds). */
  let shown: string | null = null;
  /** The search view of the side panel, per workspace: its results stay while the panel shows another view. */
  let search: { wsId: string; ui: { el: HTMLElement; focus: () => void } } | null = null;
  const entries = new Map<string, Entry>();
  const loadedDirs = new Set<string>();
  const expanded = new Set<string>();
  const tabsByPath = new Map<string, Tab>();
  const pendingPaths = new Set<string>();
  let pendingAll = false;
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;

  // ---- The shell ------------------------------------------------------------
  const picker = h('select.select.ide-picker', { 'aria-label': 'Workspace' });
  picker.addEventListener('change', () => go('ide', picker.value));
  const kindHost = h('span.ide-kind');
  const metaHost = h('span.ide-meta');
  const countEl = h('span.ide-count');
  const badgeHost = h('span.ide-badge-host');
  const saveBtn = button('Save', () => void save(), { kind: 'primary', size: 'sm', title: 'Save (Ctrl+S)' });
  const reviewBtn = button('Review', () => toggleReview(), { size: 'sm', title: 'Show the diff in the centre instead of the editor' });
  const consoleBtn = button('Console', () => leaveIde(), { kind: 'ghost', size: 'sm', icon: 'overview', title: 'Back to the screen you opened the IDE from (Esc)' });
  const leftToggle = button('', () => toggleLeft(), { kind: 'ghost', size: 'sm', icon: 'panel-left', title: 'Show or hide the side panel (Ctrl/Cmd+Alt+B)' });
  const rightToggle = button('', () => toggleRight(), { kind: 'ghost', size: 'sm', icon: 'panel-left', title: 'Show or hide the right panel (Ctrl/Cmd+Alt+J)' });
  rightToggle.classList.add('ide-flip');
  const help = keyboardHelp();
  const head = h(
    'header.ide-head',
    { 'aria-label': 'IDE header' },
    h('div.ide-brand', null, brandMark(20), h('span.ide-brand-name', null, 'IDE')),
    leftToggle,
    h('div.ide-ws', null, kindHost, picker, metaHost),
    countEl,
    badgeHost,
    h('span.spacer'),
    h('div.ide-actions', null, reviewBtn, saveBtn),
    // Repository actions (Commit, Open PR) go in this slot when repository workspaces get them.
    h('span.ide-repo-actions'),
    rightToggle,
    themeSwitch(),
    consoleBtn,
    // The plaza's held state: the strip shows only while STOP ALL holds the plaza.
    heldBanner(),
  );

  const rail = activityRail((key) => pickLeft(key));
  const leftTitle = h('span.ide-left-title');
  const leftBody = h('div.ide-left-body');
  const leftHead = h(
    'div.ide-left-head',
    null,
    leftTitle,
    h('span.spacer'),
    button('', () => setLeft(false), { icon: 'x', size: 'sm', title: 'Hide the side panel (Ctrl/Cmd+Alt+B)' }),
  );

  const changedBox = h('input', { type: 'checkbox' });
  const changedToggle = h('label.ide-toggle', null, changedBox, h('span', null, 'Changed only'));
  changedBox.addEventListener('change', () => {
    if (!ide || !state) return;
    state = ide.reduce(state, 'toggle_changed_filter');
    if (state.filter_changed) void loadChangedDirs();
    renderAll();
  });
  const treeList = h('div.ide-tree-list', { role: 'tree' });
  const filesView = h('div.ide-tree', null, h('div.ide-tree-tools', null, changedToggle, button('', () => openQuick(), { icon: 'search', title: 'Quick open (Ctrl/Cmd+P)', size: 'sm' })), treeList);
  const leftPanel = h(
    'aside.ide-left',
    { 'aria-label': 'Side panel' },
    leftHead,
    leftBody,
    panelEdge(
      'left',
      (x) => {
        layout.left = fitSize(x - body.getBoundingClientRect().left - RAIL_WIDTH, 'left', window.innerWidth, layout.rightOpen ? layout.right : 0);
        applyLayout();
      },
      saveLayout,
      () => {
        layout.left = LEFT_SIZE.def;
        applyLayout();
        saveLayout();
      },
    ),
  );

  const tabBar = h('div.ide-tabbar', { role: 'tablist' });
  const banners = h('div.ide-banners');
  const cmHost = h('div.ide-cm');
  const emptyHost = h('div.ide-empty-centre');
  /** Review mode: the changes since the base, in place of the editor. */
  const reviewHost = h('div.ide-review');

  // The terminal. Its tabs (terminalTabs) mount into termHost, which moves between the two places a terminal can sit: the
  // bottom dock under the editor, and the Terminal tab's slot in the right panel. A move keeps the terminal alive.
  const termHost = h('div.ide-term-host');
  const bottomBody = h('div.ide-bottom-body');
  const bottomHost = h(
    'section.ide-bottom',
    { 'aria-label': 'Terminal, docked under the editor', hidden: true },
    dockEdge(
      (y) => {
        dockHeight = clampDock(bottomHost.getBoundingClientRect().bottom - y);
        bottomHost.style.height = `${dockHeight}px`;
      },
      () => saveTermPrefs(),
      (px) => {
        dockHeight = clampDock(dockHeight + px);
        bottomHost.style.height = `${dockHeight}px`;
        saveTermPrefs();
      },
    ),
    h('div.ide-bottom-head', null, h('span.ide-bottom-title', null, 'Terminal'), h('span.spacer'), button('Move to side panel', () => setDock('side'), { kind: 'ghost', size: 'sm', title: 'Show the terminal in the right panel' })),
    bottomBody,
  );
  const centre = h('section.ide-centre', { 'aria-label': 'Editor' }, tabBar, banners, cmHost, emptyHost, reviewHost, bottomHost);
  const editor = new Editor(cmHost);

  const rightTabs = h('div.ide-side-tabs');
  const rightBody = h('div.ide-side-body');
  const dockToggle = button('Move terminal to bottom', () => setDock(dock === 'bottom' ? 'side' : 'bottom'), { size: 'sm', icon: 'terminal' });
  const termSlot = h('div.ide-term-slot');
  const termNote = h('div.ide-term-note');
  /** The Terminal tab: the dock toggle, then the terminal in its slot (or a note in place of it). */
  const termSide = h('div.ide-term-side', null, h('div.ide-term-side-bar', null, dockToggle), termSlot, termNote);
  const rightPanel = h(
    'aside.ide-right',
    { 'aria-label': 'Agent, approvals, run and terminal' },
    rightTabs,
    rightBody,
    panelEdge(
      'right',
      (x) => {
        layout.right = fitSize(body.getBoundingClientRect().right - x, 'right', window.innerWidth, layout.leftOpen ? layout.left : 0);
        applyLayout();
      },
      saveLayout,
      () => {
        layout.right = RIGHT_SIZE.def;
        applyLayout();
        saveLayout();
      },
    ),
  );

  const status = statusBar(() => help.set(!help.isOpen()));
  const body = h('div.ide-body', null, rail.el, leftPanel, centre, rightPanel);
  const screen = h('div.ide', null, head, body, status.el, help.el);
  root.replaceChildren(screen);
  applyLayout();

  // ---- Layout ---------------------------------------------------------------
  /** Puts the sizes and open flags on the screen: the columns are CSS variables, and a closed panel is hidden. */
  function applyLayout(): void {
    const width = window.innerWidth;
    const leftPx = fitSize(layout.left, 'left', width, layout.rightOpen && !narrow ? layout.right : 0);
    const rightPx = fitSize(layout.right, 'right', width, layout.leftOpen && !narrow ? leftPx : 0);
    screen.style.setProperty('--ide-left', layout.leftOpen && !narrow ? `${leftPx}px` : '0px');
    screen.style.setProperty('--ide-right', layout.rightOpen && !narrow ? `${rightPx}px` : '0px');
    screen.classList.toggle('narrow', narrow);
    leftPanel.hidden = !layout.leftOpen;
    rightPanel.hidden = !layout.rightOpen;
    leftToggle.setAttribute('aria-pressed', String(layout.leftOpen));
    rightToggle.setAttribute('aria-pressed', String(layout.rightOpen));
    rail.paint({ active: layout.leftOpen ? leftView : null, counts: { changes: files.length, checkpoints: checkpoints.length } });
  }

  function saveLayout(): void {
    writeStore(LAYOUT_KEY, serializeLayout(layout));
  }

  function setLeft(open: boolean): void {
    layout.leftOpen = open;
    saveLayout();
    renderAll();
  }

  function toggleLeft(): void {
    setLeft(!layout.leftOpen);
  }

  function toggleRight(): void {
    layout.rightOpen = !layout.rightOpen;
    saveLayout();
    renderAll();
  }

  /** Narrow screens: the drawer closes once the owner has picked something in it. */
  function closeDrawerIfNarrow(): void {
    if (narrow) layout.leftOpen = false;
  }

  /** The rail's click: the open view's item closes the panel; any other shows its view. */
  function pickLeft(key: LeftView): void {
    if (layout.leftOpen && leftView === key) {
      setLeft(false);
      return;
    }
    showLeft(key);
  }

  function showLeft(key: LeftView): void {
    leftView = key;
    layout.leftOpen = true;
    if (key === 'changes' && ide && state) state = ide.reduce(state, { set_panel: 'changes' });
    saveLayout();
    renderAll();
    if (key === 'search') searchUi()?.focus();
  }

  narrowMq.addEventListener('change', onNarrow);
  window.addEventListener('resize', applyLayout);
  function onNarrow(): void {
    narrow = narrowMq.matches;
    applyLayout();
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
      changes = c;
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
  async function selectWorkspace(info: IdeWorkspaceInfo): Promise<void> {
    if (!ide) return;
    disposeTerminal();
    ws = info;
    writeStore(LAST_WORKSPACE_KEY, info.id);
    marks = parseViewed(readStore(viewedKey(info.id)));
    entries.clear();
    loadedDirs.clear();
    expanded.clear();
    tabsByPath.clear();
    shown = null;
    changes = null;
    files = [];
    stats = { files: 0, additions: 0, deletions: 0 };
    checkpoints = [];
    saved = false;
    resetPanels();
    try {
      const canonical = ide.parseWorkspaceId(info.id);
      state = ide.reduce(ide.initialState(), { set_workspace: canonical });
    } catch (err) {
      toast(message(err), 'error');
      state = ide.initialState();
    }
    // The agent tabs exist for agent workspaces only; a repo keeps the Run tab.
    if ((rightTab === 'agent' || rightTab === 'approvals') && cidOf(info.id) === null) rightTab = 'run';
    if (reviewPending) {
      reviewPending = false;
      state = ide.reduce(state, 'toggle_review');
    }
    setRight(rightTab);
    if (cidOf(info.id) !== null) void panelFor('approvals');
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

  const unsub = [
    on('ide', (data) => {
      const e = data as { workspace?: unknown; path?: unknown };
      if (ws && e.workspace === ws.id) scheduleRefresh(typeof e.path === 'string' ? e.path : undefined);
    }),
    on('transcript', (data) => agentFrame(data)),
    on('work', (data) => agentFrame(data)),
  ];
  function agentFrame(data: unknown): void {
    const cid = (data as { cid?: unknown } | null)?.cid;
    if (ws && cidOf(ws.id) !== null && (typeof cid !== 'number' || cid === cidOf(ws.id))) scheduleRefresh();
  }
  // The agent's working state comes from the roster. When it stops, its writes are in: refresh everything.
  const unwatch = agents.watch((list) => {
    const cid = ws ? cidOf(ws.id) : null;
    const working = cid !== null && list.some((a) => a.cid === cid && a.status === 'working');
    if (wasWorking && !working) scheduleRefresh();
    wasWorking = working;
    agentWorking = working;
    if (alive && ws) renderTop();
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

  // ---- Keys and overlays ----------------------------------------------------
  function openQuick(): void {
    if (!ws) return;
    closeOverlay();
    overlay = quickOpen(
      screen,
      () => [...entries.values()].filter((e) => e.kind === 'file').map((e) => e.path),
      (path) => {
        closeDrawerIfNarrow();
        void openTab(path);
      },
      () => (overlay = null),
    );
  }

  /** Ctrl/Cmd+Shift+F: the Search view, with the cursor in its field. */
  function openSearch(): void {
    if (!ws) return;
    closeOverlay();
    showLeft('search');
  }

  /** The Search view for the open workspace (made once per workspace, so its results stay). */
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

  const onKey = (e: KeyboardEvent) => {
    if (!alive) return;
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.altKey && !e.shiftKey && e.code === 'KeyB') {
      e.preventDefault();
      toggleLeft();
    } else if (mod && e.altKey && !e.shiftKey && e.code === 'KeyJ') {
      e.preventDefault();
      toggleRight();
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
    } else if (e.key === 'Escape' && !e.defaultPrevented && overlay === null && !document.querySelector('.modal-backdrop') && !inField(e.target)) {
      // A field's Escape (the editor's search, a note) is the field's. Otherwise: the help, then a narrow drawer, then the console.
      if (help.isOpen()) help.set(false);
      else if (narrow && layout.leftOpen) setLeft(false);
      else leaveIde();
    }
  };
  document.addEventListener('keydown', onKey);

  // ---- Right panel and review -----------------------------------------------
  function resetPanels(): void {
    panels.agent?.dispose();
    panels.approvals?.dispose();
    panels.run?.dispose();
    panels = { agent: null, approvals: null, run: null };
    approvalCount = 0;
  }

  /** The controller of an agent-only or run tab, made on first use for this workspace. Null when the tab does not apply. */
  function panelFor(k: 'agent' | 'approvals' | 'run'): TabPanel | null {
    if (!ws) return null;
    if (k === 'run') {
      if (!panels.run) panels.run = runPanel(ws);
      return panels.run;
    }
    const cid = cidOf(ws.id);
    if (cid === null) return null;
    if (k === 'agent') {
      if (!panels.agent) panels.agent = agentPanel(cid);
      return panels.agent;
    }
    if (!panels.approvals) {
      panels.approvals = approvalsPanel({
        wsId: ws.id,
        cid,
        onCount: (n) => {
          approvalCount = n;
          renderRightTabs();
        },
        onDecided: () => scheduleRefresh(),
      });
    }
    return panels.approvals;
  }

  /** Shows a right-hand tab, and tells the reducer which panel it is. A tab the owner chose takes the focus (`focus`). */
  function setRight(next: RightTab, focus = false): void {
    rightTab = next;
    if (ide && state) state = ide.reduce(state, { set_panel: next as Panel });
    if (next !== 'terminal') panelFor(next)?.show?.();
    renderRight();
    if (focus && next === 'terminal') term?.focus();
  }

  // ---- Terminal -------------------------------------------------------------
  /** Keeps the dock's height within the editor column: at least DOCK.min, and the editor keeps DOCK.editorMin above it. */
  function clampDock(px: number): number {
    const column = bottomHost.parentElement;
    const room = column && column.clientHeight > 0 ? column.clientHeight - DOCK.editorMin : Infinity;
    return Math.round(Math.max(DOCK.min, Math.min(px, Math.max(DOCK.min, room))));
  }

  function saveTermPrefs(): void {
    writeStore(TERM_KEY, JSON.stringify({ dock, height: dockHeight }));
  }

  /** Moves the terminal between the bottom dock and the right panel's Terminal tab. The terminal keeps running; it is focused. */
  function setDock(next: TermDock): void {
    dock = next;
    saveTermPrefs();
    renderRight();
    term?.focus();
  }

  /** Ends the terminals of the workspace being left (the guest shells keep running, and re-attach when mounted again). */
  function disposeTerminal(): void {
    term?.dispose();
    term = null;
    termHost.remove();
  }

  /**
   * Puts the terminal where it belongs: in the bottom dock when docked, else in the Terminal tab's slot while that tab
   * shows, else nowhere (detached, still alive). The terminals are made the first time one is wanted, in an agent
   * workspace; a move that is not needed is not made, so a redraw does not move the focus or the xterm DOM.
   */
  function renderTerminal(): void {
    const agentWs = ws !== null && cidOf(ws.id) !== null;
    const bottom = agentWs && dock === 'bottom';
    const side = agentWs && !bottom && rightTab === 'terminal' && layout.rightOpen;
    bottomHost.hidden = !bottom;
    if (bottom) bottomHost.style.height = `${clampDock(dockHeight)}px`;
    termSlot.hidden = !side;
    // The host is in its place before a terminal is made in it: a terminal opened in a detached or hidden host measures nothing.
    const home = bottom ? bottomBody : side ? termSlot : null;
    if (home) {
      if (termHost.parentElement !== home) home.append(termHost);
    } else {
      termHost.remove();
    }
    if (ws && (bottom || side) && !term) term = terminalTabs(termHost, ws.id);
  }

  /** The Terminal tab's own bar and note: the toggle's label, and why there is no terminal here (a repo, or docked). */
  function paintTermSide(): void {
    const agentWs = ws !== null && cidOf(ws.id) !== null;
    const docked = agentWs && dock === 'bottom';
    dockToggle.textContent = docked ? 'Move terminal to side panel' : 'Move terminal to bottom';
    dockToggle.setAttribute('aria-pressed', String(docked));
    dockToggle.disabled = !agentWs;
    if (!ws) replace(termNote, empty('terminal', 'Choose a workspace', 'Terminals run in agent workspaces.'));
    else if (!agentWs) replace(termNote, empty('terminal', 'Terminals run in agent workspaces', 'A repository has no shell of its own. Open an agent workspace to get one.'));
    else if (docked) replace(termNote, h('p.ide-note', null, 'The terminal is docked under the editor.'));
    else replace(termNote);
  }

  function toggleReview(): void {
    if (!ide || !state) return;
    state = ide.reduce(state, 'toggle_review');
    renderAll();
  }

  /** From the changes list or the review: the editor shows the file, leaving review. */
  function openFromChanges(path: string): void {
    closeDrawerIfNarrow();
    if (ide && state?.review) state = ide.reduce(state, 'toggle_review');
    void openTab(path);
  }

  /** From the changes list: the review, scrolled to the file's diff. */
  function focusChange(path: string): void {
    if (!ide || !state) return;
    if (!state.review) state = ide.reduce(state, 'toggle_review');
    focusPath = path;
    closeDrawerIfNarrow();
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

  /** The side panel's Changes view: the changed files, a click on one opens its diff in the review. */
  function changedListView(): HTMLElement {
    return changedList({
      files,
      stats,
      base: baseValue(),
      bases: basesOf(),
      marks,
      active: state?.active ?? null,
      error: changesError,
      onBase: setBase,
      onFocus: focusChange,
      onOpen: openFromChanges,
      onMark: markViewed,
    });
  }

  // ---- Rendering ------------------------------------------------------------
  function renderAll(): void {
    if (!alive) return;
    renderPicker();
    renderTop();
    renderLeft();
    renderTabs();
    renderBanners();
    renderCentre();
    renderRight();
    renderStatus();
    applyLayout();
  }

  function renderPicker(): void {
    const opts = wsList.map((w) => h('option', { value: w.id, selected: w.id === ws?.id }, `${w.kind === 'agent' ? 'Agent' : 'Repo'} · ${w.name || w.owner}${w.writable ? '' : ' (read-only)'}`));
    replace(picker, ws ? [] : h('option', { value: '', selected: true }, 'Choose a workspace'), opts);
    replace(kindHost, ws ? icon(ws.kind === 'agent' ? 'agents' : 'git') : null);
    const ref = ws ? refOf(ws) : '';
    const vs = state?.base !== undefined && typeof state.base === 'object' && 'checkpoint' in state.base ? `vs #${state.base.checkpoint}` : 'vs work start';
    replace(metaHost, ws ? [ref ? `${ref} · ` : '', vs] : null);
  }

  function renderTop(): void {
    const conflict = [...tabsByPath.values()].some((t) => t.buf.conflict !== null);
    const label: string = held ? 'held' : badgeOf({ conflict, agentWorking, saved });
    replace(badgeHost, badge(label, TONE[label] ?? 'muted'));
    countEl.textContent = changes ? `${files.length} changed` : '';
    const active = state?.active ?? null;
    const t = active !== null ? tabsByPath.get(active) : undefined;
    saveBtn.disabled = !(ws?.writable && t?.buf.dirty && !t.buf.saving);
    const reviewing = !!state?.review;
    reviewBtn.textContent = reviewing ? 'Editor' : 'Review';
    reviewBtn.setAttribute('aria-pressed', reviewing ? 'true' : 'false');
  }

  /** The side panel: the view's title, and its body. The files tree is kept; the other views are made from the data. */
  function renderLeft(): void {
    const titles: Record<LeftView, string> = { files: 'Files', search: 'Search', changes: 'Changes', checkpoints: 'Checkpoints' };
    leftTitle.textContent = titles[leftView];
    if (leftView === 'files') {
      renderTree();
      replace(leftBody, filesView);
    } else if (leftView === 'search') {
      const s = searchUi();
      replace(leftBody, s ? s.el : empty('search', 'Choose a workspace', 'Search runs in a workspace.'));
    } else if (leftView === 'changes') {
      replace(leftBody, ws ? changedListView() : empty('git', 'Choose a workspace', 'The changes are per workspace.'));
    } else {
      replace(leftBody, checkpointsPanel({ checkpoints, canRestore: !!ws, onRestore: confirmRestore }));
    }
  }

  function renderTree(): void {
    if (!ide) return;
    const filter = !!state?.filter_changed;
    changedBox.checked = filter;
    const all = [...entries.values()];
    const changedPaths = new Set(files.map((f) => f.path));
    const keep = filter ? new Set(ide.changedFilter(all, { files }).map((e) => e.path)) : null;
    const active = state?.active ?? null;
    const rows: Node[] = [];
    const visit = (dir: string, depth: number): void => {
      let children = ide!.children(all, dir);
      if (keep) children = children.filter((c) => keep.has(c.path));
      for (const c of children) {
        const isDir = c.kind === 'dir';
        const open = isDir && (keep !== null || expanded.has(c.path));
        rows.push(treeRow(c, depth, open, changedPaths.has(c.path), c.path === active));
        if (!isDir || !open) continue;
        if (!loadedDirs.has(c.path)) rows.push(h('div.ide-row.loading', { style: `padding-left:${22 + depth * 14}px` }, 'Loading…'));
        else visit(c.path, depth + 1);
      }
    };
    if (ws && loadedDirs.has('')) visit('', 0);
    else if (ws) rows.push(h('div.ide-row.loading', { style: 'padding-left:8px' }, 'Loading…'));
    if (ws && rows.length === 0) rows.push(h('p.ide-note', null, filter ? 'No changed files in the loaded tree.' : 'The workspace is empty.'));
    replace(treeList, rows);
  }

  function treeRow(e: Entry, depth: number, open: boolean, changed: boolean, active: boolean): HTMLElement {
    const isDir = e.kind === 'dir';
    const name = e.path.slice(e.path.lastIndexOf('/') + 1);
    const row = h(
      'div.ide-row',
      {
        class: [isDir ? 'dir' : 'file', changed ? 'changed' : '', active ? 'on' : ''].filter(Boolean).join(' '),
        role: 'treeitem',
        'aria-expanded': isDir ? (open ? 'true' : 'false') : null,
        'aria-selected': active ? 'true' : null,
        style: `padding-left:${8 + depth * 14}px`,
        title: e.path,
      },
      h('span.ide-caret', null, isDir ? (open ? '▾' : '▸') : ''),
      h('span.ide-name', null, name),
      e.binary ? h('span.ide-tag', null, 'bin') : null,
      changed && !isDir ? h('span.ide-dot', { title: 'Changed since this base' }) : null,
    );
    row.addEventListener('click', () => {
      if (isDir) void toggleDir(e.path);
      else {
        closeDrawerIfNarrow();
        void openTab(e.path);
      }
    });
    return row;
  }

  async function toggleDir(path: string): Promise<void> {
    if (expanded.has(path)) expanded.delete(path);
    else {
      expanded.add(path);
      if (!loadedDirs.has(path)) await loadDir(path).catch((err: unknown) => toast(message(err), 'error'));
    }
    renderAll();
  }

  function renderTabs(): void {
    const items = (state?.open ?? []).map((path) => {
      const t = tabsByPath.get(path);
      const name = path.slice(path.lastIndexOf('/') + 1);
      const on = path === state?.active;
      const tab = h(
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
      return tab;
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
    if (agentWorking) rows.push(h('div.ide-banner', null, 'The agent is working in this workspace. A save over its newer write shows a conflict.'));
    replace(banners, rows);
  }

  function renderCentre(): void {
    const review = !!state?.review && !!ws;
    const has = !!state?.active;
    cmHost.hidden = review || !has;
    emptyHost.hidden = review || has;
    reviewHost.hidden = !review;
    if (review) {
      replace(reviewHost, reviewView());
      // A file picked in the changes list: its diff is opened and brought into view.
      if (focusPath !== null) {
        const path = focusPath;
        focusPath = null;
        const block = [...reviewHost.querySelectorAll<HTMLElement>('[data-path]')].find((el) => el.dataset.path === path);
        if (block instanceof HTMLDetailsElement) block.open = true;
        block?.scrollIntoView({ block: 'start' });
      }
      return;
    }
    if (!has) {
      replace(
        emptyHost,
        ws
          ? empty('file', 'No file open', 'Pick a file in the tree, or press Ctrl/Cmd+P to find one by name.')
          : empty('file', 'Choose a workspace', 'Agent workspaces and project repositories are listed above.'),
      );
    }
  }

  function renderRightTabs(): void {
    const agentWs = ws !== null && cidOf(ws.id) !== null;
    const items: RightTabItem[] = [];
    if (agentWs) items.push({ key: 'agent', label: 'Agent', icon: 'agents' }, { key: 'approvals', label: 'Approvals', icon: 'approval', count: approvalCount });
    items.push({ key: 'run', label: 'Run', icon: 'play' }, { key: 'terminal', label: 'Terminal', icon: 'terminal' });
    replace(rightTabs, rightTabStrip(items, rightTab, (k) => setRight(k as RightTab, true)));
  }

  function renderRight(): void {
    renderRightTabs();
    if (rightTab === 'terminal') {
      // The slot keeps its place: a redraw does not detach the terminal that sits in it.
      if (rightBody.firstElementChild !== termSide || rightBody.childElementCount !== 1) replace(rightBody, termSide);
      paintTermSide();
    } else if (!ws) {
      replace(rightBody, empty('agents', 'Choose a workspace', 'The agent, approvals and runs are per workspace.'));
    } else {
      const p = panelFor(rightTab);
      replace(rightBody, p ? p.el : empty('file', 'Not for this workspace', 'The Agent and Approvals tabs are for agent workspaces.'));
    }
    renderTerminal();
  }

  /** The status bar: the workspace and its base, the live connection, the cursor, the language and the unsaved count. */
  function renderStatus(): void {
    const active = state?.active ?? null;
    let cursor: StatusInfo['cursor'] = null;
    if (active !== null && !state?.review) {
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
        h('div.actions', null, button('Restore', () => {
          close();
          void restore(cp);
        }, { kind: 'danger' }), button('Cancel', () => close(), { kind: 'ghost' })),
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
    replace(body, empty('alert', 'The IDE is not available', text));
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
    if (!state) state = ide.initialState();
    const want = requested ?? readStore(LAST_WORKSPACE_KEY);
    const chosen = want !== null ? wsList.find((w) => w.id === want) : undefined;
    if (requested !== null && !chosen) toast(`No workspace ${requested}`, 'error');
    renderAll();
    if (chosen) await selectWorkspace(chosen);
  })();

  return () => {
    alive = false;
    clearTimeout(refreshTimer);
    document.removeEventListener('keydown', onKey);
    narrowMq.removeEventListener('change', onNarrow);
    window.removeEventListener('resize', applyLayout);
    for (const u of unsub) u();
    unwatch();
    unheld();
    unconnect();
    resetPanels();
    disposeTerminal();
    closeOverlay();
    editor.destroy();
  };
}
