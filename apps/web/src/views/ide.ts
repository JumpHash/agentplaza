// The IDE screen (route #/ide and #/ide/<workspace>): one workspace's files, an editor tab per open file, the changes
// since a base, the checkpoints, and the quick-open and search overlays. This module owns the data; the editor, panels
// and overlays are rendered by ide-editor, ide-side and ide-find. It loads on the first visit (views/index.ts), with
// CodeMirror. openIde (ide/open.ts) is how other screens get here.

import type { EditorState } from '@codemirror/state';
import ideCss from '../styles/ide.css';
import { ApiError, ideApi, type IdeChanges, type IdeCheckpoint, type IdeWorkspaceInfo } from '../api';
import { go } from '../router';
import { takeOpenedFromApp } from '../ide/open';
import {
  applyBuffer,
  badgeOf,
  type Buffer,
  dirsOf,
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
import { loadIde, type Ide } from '../ide/wasm';
import type { Base, Entry, FileChange, IdeState, Panel, Stats } from '../ide/types';
import { agents, on } from '../store';
import { badge, button, empty, modal, tabs, toast, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { Editor, stateFor } from './ide-editor';
import { quickOpen, searchPanel } from './ide-find';
import { changesPanel, checkpointsPanel } from './ide-side';
import { agentPanel, approvalsPanel, runPanel, terminalPanel, type Panel as TabPanel } from './ide-tabs';

const STYLE_ID = 'apz-ide-style';
const REFRESH_MS = 400;

/** The right-hand tabs. Checkpoints is the screen's own; the others are the reducer's panels (`Panel`). */
type SideTab = 'changes' | 'checkpoints' | 'agent' | 'approvals' | 'run' | 'terminal';
const SIDE_TABS: readonly SideTab[] = ['changes', 'checkpoints', 'agent', 'approvals', 'run', 'terminal'];
/** The tabs the reducer keeps as its active panel (`set_panel`). */
const REDUCER_PANELS = new Set<string>(['changes', 'agent', 'approvals', 'run', 'terminal']);

/** One open file: its buffer, and its editor state while another tab is shown (the shown tab's state is the view's). */
interface Tab {
  buf: Buffer;
  state: EditorState | null;
  /** Counts the user's edits, so a save that raced an edit knows the buffer is still dirty. */
  version: number;
}

const TONE: Record<string, Tone> = { conflict: 'danger', 'agent working': 'info', saved: 'ok', idle: 'muted' };

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

export function mount(root: HTMLElement, params: string[]): Cleanup {
  ensureStyle();
  root.classList.add('ide-host');
  // Read once, at mount: Escape goes back only to where openIde was called from.
  const fromApp = takeOpenedFromApp();
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
  let side: SideTab = SIDE_TABS.find((t) => t === startPanel) ?? 'changes';
  /** Review mode from openIde: applied once, to the first workspace this screen opens. */
  let reviewPending = query.get('review') === '1';
  /** The Agent, Approvals and Run tabs' controllers, per workspace: a tab switch keeps a draft or a running command. */
  let panels: { agent: TabPanel | null; approvals: TabPanel | null; run: TabPanel | null } = { agent: null, approvals: null, run: null };
  let approvalCount = 0;
  const terminal = terminalPanel();
  let saved = false;
  let agentWorking = false;
  let wasWorking = false;
  let overlay: (() => void) | null = null;
  /** The file the editor shows (the reducer's `active` is the tab; this is what the view holds). */
  let shown: string | null = null;
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
  const countEl = h('span.ide-count');
  const badgeHost = h('span.ide-badge-host');
  const saveBtn = button('Save', () => void save(), { kind: 'primary', size: 'sm', title: 'Save (Ctrl+S)' });
  const reviewBtn = button('Review', () => toggleReview(), { size: 'sm', title: 'Show the diff in the centre instead of the editor' });
  const top = h('div.ide-top', null, picker, countEl, badgeHost, h('span.spacer'), reviewBtn, saveBtn);

  const changedBox = h('input', { type: 'checkbox' });
  const changedToggle = h('label.ide-toggle', null, changedBox, h('span', null, 'Changed only'));
  changedBox.addEventListener('change', () => {
    if (!ide || !state) return;
    state = ide.reduce(state, 'toggle_changed_filter');
    if (state.filter_changed) void loadChangedDirs();
    renderAll();
  });
  const treeList = h('div.ide-tree-list', { role: 'tree' });
  const tree = h(
    'aside.ide-tree',
    { 'aria-label': 'Files' },
    h('div.ide-tree-tools', null, changedToggle, button('', () => openQuick(), { icon: 'search', title: 'Quick open (Ctrl+P)', size: 'sm' })),
    treeList,
  );

  const tabBar = h('div.ide-tabbar', { role: 'tablist' });
  const banners = h('div.ide-banners');
  const cmHost = h('div.ide-cm');
  const emptyHost = h('div.ide-empty-centre');
  /** Review mode: the changes since the base, in place of the editor. */
  const reviewHost = h('div.ide-review');
  const centre = h('section.ide-centre', { 'aria-label': 'Editor' }, tabBar, banners, cmHost, emptyHost, reviewHost);
  const editor = new Editor(cmHost);

  const sideTabs = h('div.ide-side-tabs');
  const sideBody = h('div.ide-side-body');
  const side$ = h('aside.ide-side', { 'aria-label': 'Changes and checkpoints' }, sideTabs, sideBody);

  const body = h('div.ide-body', null, tree, centre, side$);
  const screen = h('div.ide', null, top, body);
  root.replaceChildren(screen);

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

  // ---- Workspaces -----------------------------------------------------------
  async function selectWorkspace(info: IdeWorkspaceInfo): Promise<void> {
    if (!ide) return;
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
    // The agent tabs exist for agent workspaces only; a repo keeps the Changes tab.
    if ((side === 'agent' || side === 'approvals') && cidOf(info.id) === null) side = 'changes';
    if (reviewPending) {
      reviewPending = false;
      state = ide.reduce(state, 'toggle_review');
    }
    setSide(side);
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
        t.state = stateFor(path, f.text, { readOnly: !ws.writable, onEdit: () => onEdit(path) });
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
      else cur.state = stateFor(path, f.text, { readOnly: !ws.writable, onEdit: () => onEdit(path) });
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

  // ---- Keys -----------------------------------------------------------------
  function openQuick(): void {
    if (!ws) return;
    closeOverlay();
    overlay = quickOpen(
      screen,
      () => [...entries.values()].filter((e) => e.kind === 'file').map((e) => e.path),
      (path) => void openTab(path),
      () => (overlay = null),
    );
  }

  function openSearch(): void {
    if (!ws) return;
    const id = ws.id;
    closeOverlay();
    overlay = searchPanel(
      screen,
      (q, glob) => ideApi.search(id, q, glob || undefined),
      (path, line) => void openTab(path, line),
      () => (overlay = null),
    );
  }

  function closeOverlay(): void {
    const close = overlay;
    overlay = null;
    close?.();
  }

  const onKey = (e: KeyboardEvent) => {
    if (!alive) return;
    const mod = e.metaKey || e.ctrlKey;
    if (mod && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 's') {
      e.preventDefault();
      void save();
    } else if (mod && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'p') {
      e.preventDefault();
      openQuick();
    } else if (mod && !e.altKey && e.shiftKey && e.key.toLowerCase() === 'f') {
      e.preventDefault();
      openSearch();
    } else if (e.key === 'Escape' && !e.defaultPrevented && fromApp && overlay === null && !document.querySelector('.modal-backdrop')) {
      history.back();
    }
  };
  document.addEventListener('keydown', onKey);

  // ---- Side tabs and review -------------------------------------------------
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
          renderSideTabs();
        },
        onDecided: () => scheduleRefresh(),
      });
    }
    return panels.approvals;
  }

  /** Shows a right-hand tab, and tells the reducer which panel it is (for the reducer's tabs). */
  function setSide(next: SideTab): void {
    side = next;
    if (ide && state && REDUCER_PANELS.has(next)) state = ide.reduce(state, { set_panel: next as Panel });
    if (next === 'agent' || next === 'approvals' || next === 'run') panelFor(next)?.show?.();
    renderSide();
  }

  function toggleReview(): void {
    if (!ide || !state) return;
    state = ide.reduce(state, 'toggle_review');
    renderAll();
  }

  function renderSideTabs(): void {
    const agentWs = ws !== null && cidOf(ws.id) !== null;
    const items: { key: string; label: string }[] = [
      { key: 'changes', label: `Changes${files.length ? ` (${files.length})` : ''}` },
      { key: 'checkpoints', label: `Checkpoints${checkpoints.length ? ` (${checkpoints.length})` : ''}` },
    ];
    if (agentWs) {
      items.push({ key: 'agent', label: 'Agent' }, { key: 'approvals', label: `Approvals${approvalCount ? ` (${approvalCount})` : ''}` });
    }
    items.push({ key: 'run', label: 'Run' }, { key: 'terminal', label: 'Terminal' });
    replace(sideTabs, tabs(items, side, (k) => setSide(k as SideTab)));
  }

  /** The changes since the base, with the base picker and the viewed marks. The side tab and the review centre share it. */
  function changesView(onOpen: (path: string) => void): HTMLElement {
    const bases = [{ value: 'work_start', label: 'Since work started' }, ...checkpoints.map((cp) => ({ value: `checkpoint:${cp.n}`, label: `#${cp.n} ${cp.label || 'checkpoint'}` }))];
    return h(
      'div.ide-review-body',
      null,
      changesError ? h('p.ide-note.error', null, changesError) : null,
      changesPanel({
        files,
        stats,
        base: baseValue(),
        bases,
        marks,
        onBase: (value) => {
          if (!ide || !state) return;
          state = ide.reduce(state, { set_base: toBase(value) });
          void refreshChanges().then(() => alive && renderAll());
        },
        onOpen,
        onMark: (path, mark, on) => {
          if (!ws) return;
          marks = withViewed(marks, path, mark, on);
          writeStore(viewedKey(ws.id), JSON.stringify(marks));
          renderAll();
        },
      }),
    );
  }

  // ---- Rendering ------------------------------------------------------------
  function renderAll(): void {
    if (!alive) return;
    renderPicker();
    renderTop();
    renderTree();
    renderTabs();
    renderBanners();
    renderCentre();
    renderSide();
  }

  function renderPicker(): void {
    const opts = wsList.map((w) => h('option', { value: w.id, selected: w.id === ws?.id }, `${w.kind === 'agent' ? 'Agent' : 'Repo'} · ${w.name || w.owner}${w.writable ? '' : ' (read-only)'}`));
    replace(picker, ws ? [] : h('option', { value: '', selected: true }, 'Choose a workspace'), opts);
  }

  function renderTop(): void {
    const conflict = [...tabsByPath.values()].some((t) => t.buf.conflict !== null);
    const label = badgeOf({ conflict, agentWorking, saved });
    replace(badgeHost, badge(label, TONE[label] ?? 'muted'));
    countEl.textContent = changes ? `${files.length} changed` : '';
    const active = state?.active ?? null;
    const t = active !== null ? tabsByPath.get(active) : undefined;
    saveBtn.disabled = !(ws?.writable && t?.buf.dirty && !t.buf.saving);
    const reviewing = !!state?.review;
    reviewBtn.textContent = reviewing ? 'Editor' : 'Review';
    reviewBtn.setAttribute('aria-pressed', reviewing ? 'true' : 'false');
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
      else void openTab(e.path);
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
      // Opening a file from the review leaves review: the editor is what the owner asked for.
      replace(reviewHost, changesView((path) => {
        if (ide && state?.review) state = ide.reduce(state, 'toggle_review');
        void openTab(path);
      }));
      return;
    }
    if (!has) {
      replace(
        emptyHost,
        ws
          ? empty('file', 'No file open', 'Pick a file in the tree, or press Ctrl+P to find one by name.')
          : empty('file', 'Choose a workspace', 'Agent workspaces and project repositories are listed above.'),
      );
    }
  }

  function renderSide(): void {
    renderSideTabs();
    if (side === 'checkpoints') {
      replace(sideBody, checkpointsPanel({ checkpoints, canRestore: !!ws, onRestore: confirmRestore }));
      return;
    }
    if (side === 'changes') {
      replace(sideBody, changesView((path) => void openTab(path)));
      return;
    }
    if (side === 'terminal') {
      replace(sideBody, terminal);
      return;
    }
    const p = panelFor(side);
    replace(sideBody, p ? p.el : empty('file', 'Not for this workspace', 'The Agent and Approvals tabs are for agent workspaces.'));
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
    replace(screen, empty('alert', 'The IDE is not available', text));
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
    for (const u of unsub) u();
    unwatch();
    resetPanels();
    closeOverlay();
    editor.destroy();
  };
}
