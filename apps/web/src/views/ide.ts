// The IDE screen (route #/ide and #/ide/<workspace>): a full-window view of its own, with no console shell beside it. The
// agent-first layout: projects and agents on the left; the selected agent's chat, terminal, editor or diff in the centre;
// the board, files, tasks, approvals and activity on the right; a terminal and run drawer under the centre (Ctrl+`); a
// status bar. This module builds the frame and wires it to the modules that split the controller: layout (ide-layout),
// selection (ide-select), data (ide-data), editing (ide-editing), panels (ide-panels), keys (ide-keys), git (ide-git),
// the terminal host (ide-term-host) and rendering (ide-render). The state lives on the context (ide-ctx). The model is the
// wasm IDE state (ide/wasm). Console (and Escape) goes back to the screen the IDE was opened from.

import ideCss from '../styles/ide.css';
import shellCss from '../styles/ide-shell.css';
import { ideApi, type IdeWorkspaceInfo } from '../api';
import { go } from '../router';
import { LAST_WORKSPACE_KEY } from '../ide/model';
import { loadIde } from '../ide/wasm';
import { agents, on, onConnection } from '../store';
import { button, empty, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import { heldBanner, onPlazaHeld } from '../ui/plaza-control';
import { themeSwitch } from '../ui/theme';
import type { Cleanup } from './index';
import { Editor } from './ide-editor';
import { keyboardHelp, statusBar } from './ide-chrome';
import { buildFrame, type DrawerTab, type Frame, type RightView } from './ide-frame';
import { sidebar } from './ide-sidebar';
import { palette, type Palette } from './ide-palette';
import { cidOf, closeOverlay, message, newCtx, readStore, VIEWS, type IdeCtx } from './ide-ctx';
import { applyLayout, closeDrawerIfNarrow, layoutEdges, MID_QUERY, onBreakpoint, readDrawerPrefs, readLayout, setDrawer, toggleLeftPanel, toggleRightPanel } from './ide-layout';
import { autoSelect, newAgent, openKey, selectAgent, selectWorkspace, setCentre, setRight } from './ide-select';
import { lastAgentCid, loadChangedDirs, refreshProjects, scheduleRefresh } from './ide-data';
import { openTab, reloadTab, save } from './ide-editing';
import { openQuick, toggleSearch } from './ide-panels';
import { disposeTerminal } from './ide-term-host';
import { leaveIde, onKey, runPaletteItem } from './ide-keys';
import { renderAll, renderRightTabs, renderStatus, renderTop } from './ide-render';
import { NARROW_QUERY } from '../ide/layout';
import { isConversationId } from '../ide/conversation-model';
import { mountConversation } from './ide-conversation';

const STYLE_ID = 'apz-ide-style';

/** The stylesheets, inserted once on the first visit (build.mjs makes them text). The hidden attribute always hides. */
function ensureStyle(): void {
  if (document.getElementById(STYLE_ID)) return;
  const style = document.createElement('style');
  style.id = STYLE_ID;
  style.textContent = `${ideCss}\n${shellCss}\n.ide-host [hidden]{display:none!important}`;
  document.head.appendChild(style);
}

/** The panel a link asks for (openIde's `panel`), mapped onto the new places. */
function applyStartPanel(c: IdeCtx, startPanel: string | null, query: URLSearchParams): void {
  if (startPanel === 'changes' || query.get('review') === '1') c.centreView = 'diff';
  else if (startPanel === 'agent') c.centreView = 'chat';
  else if (startPanel === 'terminal') c.centreView = 'terminal';
  if (startPanel === 'checkpoints' || startPanel === 'files') c.rightView = 'files';
  if (startPanel === 'approvals') c.rightView = 'approvals';
  if (startPanel === 'run') {
    c.drawerOpen = true;
    c.drawerTab = 'run';
  }
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  // A conversation with the orchestrator is a workspace of its own: the chat and the project it drafts.
  if (isConversationId(params[0])) return mountConversation(root, params[0]);
  ensureStyle();
  root.classList.add('ide-host');
  const query = new URLSearchParams(location.hash.split('?')[1] ?? '');
  const requested = params[0] ?? null;
  const startFile = query.get('file');
  const startPanel = query.get('panel');

  const narrowMq = matchMedia(NARROW_QUERY);
  const midMq = matchMedia(MID_QUERY);
  const drawerPrefs = readDrawerPrefs();
  const c: IdeCtx = newCtx({
    root,
    requested,
    startFile,
    layout: readLayout(),
    drawerOpen: drawerPrefs.open,
    drawerTab: drawerPrefs.tab,
    dockHeight: drawerPrefs.height,
    narrow: narrowMq.matches,
    mid: midMq.matches,
    centreView: 'editor',
    rightView: 'files',
  });

  // ---- The parts ---------------------------------------------------------------------------------------------------
  const changedBox = h('input', { type: 'checkbox' });
  const changedToggle = h('label.ide-toggle', null, changedBox, h('span', null, 'Changed only'));
  changedBox.addEventListener('change', () => {
    if (!c.ide || !c.state) return;
    c.state = c.ide.reduce(c.state, 'toggle_changed_filter');
    if (c.state.filter_changed) void loadChangedDirs(c);
    renderAll(c);
  });
  const searchBtn = button('', () => toggleSearch(c), { icon: 'search', size: 'sm', title: 'Search the workspace (Ctrl/Cmd+Shift+F)' });
  const quickBtn = button('', () => openQuick(c), { icon: 'file', size: 'sm', title: 'Quick open (Ctrl/Cmd+P)' });
  const treeList = h('div.ide-tree-list', { role: 'tree' });
  const searchHost = h('div.iw-files-search', { hidden: true });
  const cpHost = h('div.iw-files-cp');
  const filesEl = h('div.iw-files', null, h('div.ide-tree-tools', null, changedToggle, searchBtn, quickBtn), searchHost, treeList, cpHost);

  const saveBtn = button('Save', () => void save(c), { kind: 'primary', size: 'sm', title: 'Save (Ctrl+S)' });
  const tabBar = h('div.ide-tabbar', { role: 'tablist' });
  const banners = h('div.ide-banners');
  const cmHost = h('div.ide-cm');
  const emptyHost = h('div.ide-empty-centre');
  /** The diff view: the changes since the base, in place of the editor. */
  const reviewHost = h('div.ide-review');
  const editorBar = h('div.iw-editor-bar', null, h('span.spacer'), saveBtn);
  const termHost = h('div.ide-term-host');
  const editor = new Editor(cmHost);

  const sideEl = sidebar({ onOpen: (key) => openKey(c, key), onNewAgent: () => newAgent() });
  const help = keyboardHelp();
  const status = statusBar(() => help.set(!help.isOpen()));
  const edges = layoutEdges(c);
  const frame: Frame = buildFrame({
    side: sideEl.el,
    sideEdge: edges.sideEdge,
    rightEdge: edges.rightEdge,
    drawerEdge: edges.drawerEdge,
    held: heldBanner(),
    theme: themeSwitch(),
    status: status.el,
  });
  root.replaceChildren(frame.root, help.el);
  frame.centre.views.editor.append(editorBar, tabBar, banners, cmHost, emptyHost);
  frame.centre.views.diff.append(reviewHost);
  frame.centre.views.terminal.append(termHost);
  frame.centre.views.terminal.classList.add('iw-view-term');

  /** The palette (Ctrl/Cmd+K): agents, repositories, files, tasks and commands. */
  const pal: Palette = palette((item) => runPaletteItem(c, item));

  // The context gets the parts, then the actions the modules call across the split.
  c.dom = {
    frame,
    editor,
    sidebar: sideEl,
    help,
    status,
    pal,
    narrowMq,
    midMq,
    changedBox,
    changedToggle,
    treeList,
    searchHost,
    cpHost,
    filesEl,
    saveBtn,
    tabBar,
    banners,
    cmHost,
    emptyHost,
    reviewHost,
    editorBar,
    termHost,
  };
  c.act = {
    renderAll: () => renderAll(c),
    renderTop: () => renderTop(c),
    renderStatus: () => renderStatus(c),
    renderRightTabs: () => renderRightTabs(c),
    openTab: (path, line) => openTab(c, path, line),
    reloadTab: (path) => reloadTab(c, path),
    selectAgent: (cid) => selectAgent(c, cid, 'chat'),
  };
  wireFrame();

  // ---- Keys, breakpoints and the live feeds -------------------------------------------------------------------------
  const onBp = () => onBreakpoint(c);
  const onResize = () => applyLayout(c);
  narrowMq.addEventListener('change', onBp);
  midMq.addEventListener('change', onBp);
  window.addEventListener('resize', onResize);
  const onKeyDown = (e: KeyboardEvent) => onKey(c, e);
  document.addEventListener('keydown', onKeyDown);

  const unsub = [
    on('ide', (data) => {
      const e = data as { workspace?: unknown; path?: unknown };
      if (c.ws && e.workspace === c.ws.id) scheduleRefresh(c, typeof e.path === 'string' ? e.path : undefined);
    }),
    on('transcript', (data) => agentFrame(data)),
    on('work', (data) => agentFrame(data)),
  ];
  function agentFrame(data: unknown): void {
    const frameCid = (data as { cid?: unknown } | null)?.cid;
    const wsCid = c.ws ? cidOf(c.ws.id) : null;
    if (c.ws && wsCid !== null && (typeof frameCid !== 'number' || frameCid === wsCid)) scheduleRefresh(c);
  }
  // The agents' roster: the sidebar's and the palette's list, and the selected agent's working state. When it stops, its
  // writes are in. Until the first choice is made, an agent is picked as soon as one is listed.
  const unwatch = agents.watch((list) => {
    c.agentList = list;
    const working = c.cid !== null && list.some((a) => a.cid === c.cid && a.status === 'working');
    if (c.wasWorking && !working) scheduleRefresh(c);
    c.wasWorking = working;
    c.agentWorking = working;
    if (c.started && c.cid === null && c.ws === null) autoSelect(c, lastAgentCid());
    if (c.alive) renderAll(c);
  });
  // The plaza's held state and the live connection: the header's badge, the status bar.
  const unheld = onPlazaHeld((v) => {
    c.held = v;
    if (c.alive) renderTop(c);
  });
  const unconnect = onConnection((up) => {
    c.connected = up;
    if (c.alive) renderStatus(c);
  });
  // The project, the task list and the workspaces follow the console's own polls.
  const projectsTimer = setInterval(() => void refreshProjects(c), 20_000);

  /** The header toggles, the scrim and the drawer's own buttons. */
  function wireFrame(): void {
    frame.sideToggle.addEventListener('click', () => toggleLeftPanel(c));
    frame.rightToggle.addEventListener('click', () => toggleRightPanel(c));
    frame.consoleBtn.addEventListener('click', () => leaveIde(c));
    frame.scrim.addEventListener('click', () => closeDrawerIfNarrow(c));
    frame.drawer.close.addEventListener('click', () => setDrawer(c, false));
    for (const v of VIEWS) frame.centre.seg[v].addEventListener('click', () => setCentre(c, v));
    frame.centre.popout.addEventListener('click', () => {
      if (c.cid !== null) go('agents', c.cid);
    });
    for (const k of Object.keys(frame.right.tabs) as RightView[]) frame.right.tabs[k].addEventListener('click', () => setRight(c, k));
    for (const k of Object.keys(frame.drawer.tabs) as DrawerTab[]) frame.drawer.tabs[k].addEventListener('click', () => setDrawer(c, true, k));
  }

  /** Stops the screen: the frame says why, in the centre. */
  function fatal(text: string): void {
    replace(frame.centre.el, empty('alert', 'The IDE is not available', text));
  }

  // ---- Start ---------------------------------------------------------------------------------------------------------
  void (async () => {
    try {
      c.ide = await loadIde();
    } catch (err) {
      if (c.alive) fatal(`The IDE model did not load: ${message(err)}`);
      return;
    }
    try {
      c.wsList = await ideApi.workspaces();
    } catch (err) {
      if (c.alive) fatal(message(err));
      return;
    }
    if (!c.alive) return;
    c.state = c.ide.initialState();
    c.agentList = agents.get();
    await refreshProjects(c);
    if (!c.alive) return;
    // The workspace the link names, else the one shown last time; else the agent picked by startAgent.
    const want = requested ?? readStore(LAST_WORKSPACE_KEY);
    const chosen: IdeWorkspaceInfo | undefined = want !== null ? c.wsList.find((w) => w.id === want) : undefined;
    if (requested !== null && !chosen) toast(`No workspace ${requested}`, 'error');
    applyStartPanel(c, startPanel, query);
    c.started = true;
    renderAll(c);
    if (chosen?.kind === 'agent') selectAgent(c, cidOf(chosen.id) as number, c.centreView);
    else if (chosen) selectWorkspace(c, chosen);
    else autoSelect(c, lastAgentCid());
  })();

  return () => {
    c.alive = false;
    clearTimeout(c.refreshTimer);
    clearInterval(projectsTimer);
    document.removeEventListener('keydown', onKeyDown);
    narrowMq.removeEventListener('change', onBp);
    midMq.removeEventListener('change', onBp);
    window.removeEventListener('resize', onResize);
    for (const u of unsub) u();
    unwatch();
    unheld();
    unconnect();
    c.chat?.dispose();
    c.activity?.dispose();
    c.approvals?.dispose();
    c.run?.dispose();
    c.board?.dispose();
    c.taskPanel?.dispose();
    sideEl.dispose();
    pal.dispose();
    disposeTerminal(c);
    closeOverlay(c);
    editor.destroy();
  };
}
