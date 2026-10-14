// The shared state of the IDE screen. The controller (views/ide.ts) makes one IdeCtx per mount and hands it to the modules
// that split it: layout, selection, data, editing, panels, keys, git and rendering. A module reads and writes the fields
// directly. Where a call would otherwise import back into a module that imports it (the data loader opens a file; the
// editor paints the shell), it goes through `act`, which the controller wires. So no two of these modules import each other.

import type { EditorState } from '@codemirror/state';
import type { Agent, IdeCheckpoint, IdeWorkspaceInfo, Project, StationView, Task } from '../api';
import type { Buffer, Viewed } from '../ide/model';
import { SHUT, type Layout, type Sheets } from '../ide/layout';
import type { Entry, FileChange, IdeState, Stats } from '../ide/types';
import type { Ide } from '../ide/wasm';
import type { CentreView, DrawerTab, Frame, RightView } from './ide-frame';
import type { Editor } from './ide-editor';
import type { Sidebar } from './ide-sidebar';
import type { keyboardHelp, statusBar } from './ide-chrome';
import type { Palette } from './ide-palette';
import type { ChatPane } from './ide-chat';
import type { ActivityPanel } from './ide-activity';
import type { Panel as TabPanel } from './ide-tabs';
import type { BoardPanel } from './ide-board';
import type { TasksPanel } from './ide-tasks';
import type { TabsHandle } from './ide-terminal';

/** The layout and drawer keys, kept per browser. */
export const LAYOUT_KEY = 'apz.ide.layout';
export const DRAWER_KEY = 'apz.ide.drawer';
export const VIEWS: CentreView[] = ['chat', 'terminal', 'editor', 'diff'];

/** One open file: its buffer, and its editor state while another tab is shown (the shown tab's state is the view's). */
export interface Tab {
  buf: Buffer;
  state: EditorState | null;
  /** Counts the user's edits, so a save that raced an edit knows the buffer is still dirty. */
  version: number;
}

/** The DOM the controller builds once, and the modules paint. */
export interface IdeDom {
  frame: Frame;
  editor: Editor;
  sidebar: Sidebar;
  help: ReturnType<typeof keyboardHelp>;
  status: ReturnType<typeof statusBar>;
  pal: Palette;
  narrowMq: MediaQueryList;
  midMq: MediaQueryList;
  changedBox: HTMLInputElement;
  /** The "Changed only" label around the box: hidden for a workstation, which has no diff. */
  changedToggle: HTMLElement;
  treeList: HTMLElement;
  searchHost: HTMLElement;
  cpHost: HTMLElement;
  filesEl: HTMLElement;
  saveBtn: HTMLButtonElement;
  tabBar: HTMLElement;
  banners: HTMLElement;
  cmHost: HTMLElement;
  emptyHost: HTMLElement;
  reviewHost: HTMLElement;
  editorBar: HTMLElement;
  /** The terminal's element: it moves between the centre's Terminal view and the drawer, and keeps running when it moves. */
  termHost: HTMLElement;
}

/** The renders and openers that modules call through `act`. The controller wires them to the modules that own them. */
export interface Actions {
  renderAll(): void;
  renderTop(): void;
  renderStatus(): void;
  renderRightTabs(): void;
  openTab(path: string, line?: number): Promise<void>;
  reloadTab(path: string): Promise<void>;
  /** Selects an agent in the sidebar and shows its chat (a rich reference to an agent, in the IDE's own text). */
  selectAgent(cid: number): void;
}

export interface IdeCtx {
  readonly root: HTMLElement;
  readonly requested: string | null;
  readonly startFile: string | null;
  alive: boolean;
  /** Set once the start has chosen the first selection (the agents' store may fill in later, and then a first pick is made). */
  started: boolean;
  ide: Ide | null;
  /** The IDE workspaces (agents with a VM, and repositories). Refreshed with the projects. */
  wsList: IdeWorkspaceInfo[];
  /** The workspace the editor and the diff show (an agent's, or a repository's). */
  ws: IdeWorkspaceInfo | null;
  /** The selected agent (null for a repository). */
  cid: number | null;
  state: IdeState | null;
  changesError: string;
  files: FileChange[];
  stats: Stats;
  checkpoints: IdeCheckpoint[];
  marks: Viewed;
  agentList: Agent[];
  projects: Project[];
  tasks: Task[];
  centreView: CentreView;
  rightView: RightView;
  drawerOpen: boolean;
  drawerTab: DrawerTab;
  dockHeight: number;
  narrow: boolean;
  mid: boolean;
  /** The saved layout: the owner's open flags for a docked panel. A narrow window never changes them. */
  layout: Layout;
  /** The drawer and the sheet's open state at a narrow width: view only, never saved (see ide/layout). */
  sheets: Sheets;
  /** The workstations (`/v1/stations`): a workstation's badge reads its status here. Refreshed with the projects. */
  stations: StationView[];
  /** The file whose diff the review scrolls to, once it is drawn. */
  focusPath: string | null;
  /** Per-selection controllers: made on first use, disposed when the selection moves on. */
  chat: ChatPane | null;
  activity: ActivityPanel | null;
  approvals: TabPanel | null;
  run: TabPanel | null;
  board: BoardPanel | null;
  boardKey: string | null;
  boardCount: number | null;
  taskPanel: TasksPanel | null;
  taskKey: string | null;
  taskCounts: { done: number; total: number } | null;
  approvalCount: number;
  term: TabsHandle | null;
  saved: boolean;
  agentWorking: boolean;
  wasWorking: boolean;
  held: boolean;
  connected: boolean;
  /** The open overlay (quick open), with its close. */
  overlay: (() => void) | null;
  /** The file the editor shows (the reducer's `active` is the tab; this is what the view holds). */
  shown: string | null;
  /** The search view of the Files tab, per workspace: its results stay while the tab shows another view. */
  search: { wsId: string; ui: { el: HTMLElement; focus: () => void } } | null;
  refreshTimer: ReturnType<typeof setTimeout> | undefined;
  pendingAll: boolean;
  readonly pendingPaths: Set<string>;
  readonly entries: Map<string, Entry>;
  readonly loadedDirs: Set<string>;
  readonly expanded: Set<string>;
  readonly tabsByPath: Map<string, Tab>;
  /** Set by the controller right after the frame is built, before any handler can run. */
  dom: IdeDom;
  /** Set by the controller with the rest of the wiring, before the first render. */
  act: Actions;
}

/** A new context with the state of a fresh mount. `dom` and `act` are set by the controller. */
export function newCtx(init: { root: HTMLElement; requested: string | null; startFile: string | null; layout: Layout; drawerOpen: boolean; drawerTab: DrawerTab; dockHeight: number; narrow: boolean; mid: boolean; centreView: CentreView; rightView: RightView }): IdeCtx {
  return {
    root: init.root,
    requested: init.requested,
    startFile: init.startFile,
    alive: true,
    started: false,
    ide: null,
    wsList: [],
    ws: null,
    cid: null,
    state: null,
    changesError: '',
    files: [],
    stats: { files: 0, additions: 0, deletions: 0 },
    checkpoints: [],
    marks: {},
    agentList: [],
    projects: [],
    tasks: [],
    centreView: init.centreView,
    rightView: init.rightView,
    drawerOpen: init.drawerOpen,
    drawerTab: init.drawerTab,
    dockHeight: init.dockHeight,
    narrow: init.narrow,
    mid: init.mid,
    layout: init.layout,
    sheets: SHUT,
    stations: [],
    focusPath: null,
    chat: null,
    activity: null,
    approvals: null,
    run: null,
    board: null,
    boardKey: null,
    boardCount: null,
    taskPanel: null,
    taskKey: null,
    taskCounts: null,
    approvalCount: 0,
    term: null,
    saved: false,
    agentWorking: false,
    wasWorking: false,
    held: false,
    connected: true,
    overlay: null,
    shown: null,
    search: null,
    refreshTimer: undefined,
    pendingAll: false,
    pendingPaths: new Set(),
    entries: new Map(),
    loadedDirs: new Set(),
    expanded: new Set(),
    tabsByPath: new Map(),
    dom: undefined as unknown as IdeDom,
    act: undefined as unknown as Actions,
  };
}

// ---- Small shared helpers ----------------------------------------------------------------------------------------

export function readStore(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function writeStore(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // A private window or blocked storage: the choice just is not remembered.
  }
}

/** The agent's cid for an `agent:<cid>` workspace, else null. */
export function cidOf(id: string): number | null {
  const m = /^agent:(\d+)$/.exec(id);
  return m ? Number(m[1]) : null;
}

export const message = (err: unknown): string => (err instanceof Error ? err.message : String(err));

/** A repository workspace's name and branch: `repo:<name>@<ref>`. */
export const repoNameOf = (w: IdeWorkspaceInfo): string => w.id.slice('repo:'.length, w.id.lastIndexOf('@'));
export const refOf = (w: IdeWorkspaceInfo): string => (w.kind === 'repo' ? w.id.slice(w.id.lastIndexOf('@') + 1) : '');

/** Whether a key press is typed into a field or the editor (so a shortcut that is a letter or a `?` does not fire). */
export const inTyping = (target: EventTarget | null): boolean =>
  !!(target as Element | null)?.closest?.('input, textarea, select, [contenteditable]:not([contenteditable="false"]), .cm-editor, .xterm');

/** Whether the key press is in a terminal: its keys belong to the shell, so the screen's Ctrl shortcuts stay off. */
export const inTerminal = (target: EventTarget | null): boolean => !!(target as Element | null)?.closest?.('.xterm');

/** Whether the key press is in a form field (Escape there is the field's, not the screen's). */
export const inField = (target: EventTarget | null): boolean => !!(target as Element | null)?.closest?.('input, textarea, select');

/** Closes the overlay that is open, if any. */
export function closeOverlay(c: IdeCtx): void {
  const close = c.overlay;
  c.overlay = null;
  close?.();
}
