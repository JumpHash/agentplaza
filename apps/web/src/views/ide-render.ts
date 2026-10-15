// The IDE's rendering: every paint of the screen, in one order (renderAll). Each function reads the context and writes the
// DOM the controller built; none of them changes the state, except the review's scroll target, which is taken once.
import { badge, empty, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { badgeOf, languageFor, stationBadgeOf, stationCrumb } from '../ide/model';
import type { Entry } from '../ide/types';
import { centreViewsFor, hasDiff, hasTerminal, rightViewsFor } from '../ide/layout';
import { rowStatus } from '../ide/view';
import { VIEWS, refOf, repoNameOf, type IdeCtx } from './ide-ctx';
import { CENTRE_LABEL, paintTabs, setCrumb, type RightView } from './ide-frame';
import { LANG_LABEL, type StatusInfo } from './ide-chrome';
import { changesPanel, checkpointsPanel } from './ide-side';
import { treeRows } from './ide-files';
import { renderGit } from './ide-git';
import { openFromChanges, openTab, renderBanners, renderTabs } from './ide-editing';
import { baseValue, basesOf, markViewed, setBase } from './ide-data';
import { renderTerminal } from './ide-term-host';
import { activityFor, approvalsFor, boardFor, confirmRestore, currentProject, runFor, searchUi, tasksFor, toggleDir } from './ide-panels';
import { centreAllowed } from './ide-select';
import { applyLayout, closeDrawerIfNarrow } from './ide-layout';

const TONE: Record<string, Tone> = {
  conflict: 'danger',
  'agent working': 'info',
  saved: 'ok',
  idle: 'muted',
  held: 'danger',
  online: 'ok',
  draining: 'warn',
  offline: 'muted',
};

/** The whole screen, in the order the parts depend on. The sidebar follows the selection and the terminals that exist. */
export function renderAll(c: IdeCtx): void {
  if (!c.alive) return;
  renderSelection(c);
  renderTop(c);
  renderCentre(c);
  renderRightTabs(c);
  renderRight(c);
  renderDrawer(c);
  renderTabs(c);
  renderBanners(c);
  renderTerminal(c);
  renderStatus(c);
  applyLayout(c);
  c.dom.sidebar.refresh();
}

/** The centre's header: the dot, the title, the view toggle and the pop-out. */
export function renderSelection(c: IdeCtx): void {
  const { frame } = c.dom;
  const agent = c.cid !== null ? c.agentList.find((a) => a.cid === c.cid) : undefined;
  const title =
    agent?.name ??
    (c.ws?.kind === 'repo' ? repoNameOf(c.ws) : c.ws?.kind === 'station' ? c.ws.name || c.ws.owner : c.cid !== null ? `agent ${c.cid}` : 'No selection');
  frame.centre.title.textContent = title;
  frame.centre.dot.className = `iw-dot ${agent ? rowStatus(agent, null) : 'idle'}`;
  frame.centre.popout.hidden = c.cid === null;
  const views = centreViewsFor(c.ws?.kind ?? null, c.cid !== null);
  for (const v of VIEWS) {
    const btn = frame.centre.seg[v];
    btn.hidden = !views.includes(v);
    btn.querySelector('.iw-seg-label')?.replaceChildren(CENTRE_LABEL[v]);
  }
  paintTabs(frame.centre.seg, c.centreView, {});
  if (!centreAllowed(c, c.centreView)) c.centreView = 'editor';
  for (const v of VIEWS) frame.centre.views[v].hidden = v !== c.centreView;
  if (c.chat && frame.centre.views.chat.firstElementChild !== c.chat.el) replace(frame.centre.views.chat, c.chat.el);
}

export function renderTop(c: IdeCtx): void {
  const conflict = [...c.tabsByPath.values()].some((t) => t.buf.conflict !== null);
  const ws = c.ws;
  // A workstation's badge is its status: its record (`/v1/stations`), else the workspace list's, which lists only online ones.
  const station = ws?.kind === 'station' ? stationBadgeOf(c.stations.find((s) => s.id === ws.owner) ?? { online: ws.status === 'online', draining: false, revoked: false }) : null;
  const label: string = c.held ? 'held' : badgeOf({ conflict, agentWorking: c.agentWorking && c.cid !== null, saved: c.saved, station });
  const project = currentProject(c);
  const parts = !ws
    ? ['IDE']
    : ws.kind === 'station'
      ? stationCrumb(ws)
      : [project?.title ?? (c.cid !== null ? 'No project' : 'Repositories'), ws.kind === 'agent' ? (c.agentList.find((a) => a.cid === c.cid)?.name ?? `agent ${c.cid}`) : repoNameOf(ws), refOf(ws) || 'work'].filter(Boolean);
  setCrumb(c.dom.frame.crumb, parts);
  c.dom.frame.state.replaceChildren(badge(label, TONE[label] ?? 'muted'));
  const active = c.state?.active ?? null;
  const t = active !== null ? c.tabsByPath.get(active) : undefined;
  c.dom.saveBtn.disabled = !(c.ws?.writable && t?.buf.dirty && !t.buf.saving);
  renderGit(c);
}

/** The centre's editor and diff: the editor shows the active file or a note; the diff shows the changes. */
export function renderCentre(c: IdeCtx): void {
  const { dom } = c;
  const has = !!c.state?.active;
  const inEditor = c.centreView === 'editor';
  const editing = inEditor && !!c.ws;
  dom.cmHost.hidden = !editing || !has;
  dom.emptyHost.hidden = !inEditor || (editing && has);
  dom.tabBar.hidden = !editing;
  dom.banners.hidden = !editing;
  dom.editorBar.hidden = !editing;
  if (!c.ws) {
    if (inEditor) {
      replace(
        dom.emptyHost,
        c.cid === null
          ? empty('file', 'Choose a project or an agent', 'Pick an agent in the sidebar, or a repository. Ctrl/Cmd+K opens the palette.')
          : noWorkspaceNote(c),
      );
    }
  } else if (inEditor && !has) {
    replace(dom.emptyHost, empty('file', 'No file open', 'Pick a file in the Files tab, or press Ctrl/Cmd+P to find one by name.'));
  }
  if (c.centreView === 'diff') {
    replace(dom.reviewHost, c.ws ? reviewView(c) : empty('git', 'Choose a workspace', 'The changes are per workspace.'));
    if (c.focusPath !== null) {
      const path = c.focusPath;
      c.focusPath = null;
      const block = [...dom.reviewHost.querySelectorAll<HTMLElement>('[data-path]')].find((el) => el.dataset.path === path);
      if (block instanceof HTMLDetailsElement) block.open = true;
      block?.scrollIntoView({ block: 'start' });
    }
  }
}

/** The review in the centre: the changes since the base, with the base picker and the viewed marks. */
function reviewView(c: IdeCtx): HTMLElement {
  return h(
    'div.ide-review-body',
    null,
    c.changesError ? h('p.ide-note.error', null, c.changesError) : null,
    changesPanel({
      files: c.files,
      stats: c.stats,
      base: baseValue(c),
      bases: basesOf(c),
      marks: c.marks,
      onBase: (v) => setBase(c, v),
      onOpen: (path) => openFromChanges(c, path),
      onMark: (path, mark, on) => markViewed(c, path, mark, on),
    }),
  );
}

/** The Files tab's and the centre's note when no workspace shows: an agent with no VM has no files; else it is not ready. */
function noWorkspaceNote(c: IdeCtx): HTMLElement {
  if (c.cid === null) return empty('file', 'No workspace', 'Pick an agent, a workstation or a repository to see its files.');
  const agent = c.agentList.find((a) => a.cid === c.cid);
  if (agent?.vm === null) return empty('file', 'No files for this agent', 'This agent has no VM, so it has no files.');
  return empty('file', 'Files are not ready yet', 'This agent has a VM, and its workspace shows once the list has it.');
}

/** The right panel's count of each tab; the tabs the selection's kind does not have are hidden (see rightViewsFor). */
export function renderRightTabs(c: IdeCtx): void {
  const counts: Partial<Record<RightView, string | null>> = {
    board: c.boardCount !== null && c.boardCount > 0 ? String(c.boardCount) : null,
    files: c.files.length > 0 ? String(c.files.length) : null,
    tasks: c.taskCounts ? `${c.taskCounts.done}/${c.taskCounts.total}` : null,
    approvals: c.approvalCount > 0 ? String(c.approvalCount) : null,
    activity: null,
  };
  const tabs = c.dom.frame.right.tabs;
  paintTabs(tabs, c.rightView, counts);
  const shown = rightViewsFor(c.ws?.kind ?? null, c.cid !== null);
  for (const k of Object.keys(tabs) as RightView[]) tabs[k].hidden = !shown.includes(k);
}

/** The right panel's body: the current tab's panel (made and shown on first use), or a note. */
export function renderRight(c: IdeCtx): void {
  const body = c.dom.frame.right.body;
  const project = currentProject(c);
  if (!c.ws && c.cid === null) {
    replace(body, empty('agents', 'Choose a project or an agent', 'The board, files, tasks and activity follow the selection.'));
  } else if (c.rightView === 'board') {
    const b = boardFor(c, project);
    replace(body, b.el);
    b.show();
  } else if (c.rightView === 'files') {
    replace(body, c.dom.filesEl);
    renderFiles(c);
  } else if (c.rightView === 'tasks') {
    const t = tasksFor(c, project);
    replace(body, t.el);
    t.show();
  } else if (c.rightView === 'approvals') {
    const p = approvalsFor(c);
    replace(body, p ? p.el : empty('approval', 'No approvals here', 'Approvals are per agent.'));
    p?.show?.();
  } else if (c.rightView === 'activity') {
    if (c.cid === null) replace(body, empty('loop', 'No activity here', 'Activity is per agent.'));
    else {
      const a = activityFor(c, c.cid);
      replace(body, a.el);
      a.show();
    }
  }
}

/** The key of what each node (the tree, the checkpoints panel) last painted; see renderFiles. */
const painted = new WeakMap<Node, string>();

/** The Files tab's tree, its changed-only filter, its search and the checkpoints. */
export function renderFiles(c: IdeCtx): void {
  const { dom } = c;
  if (!c.ide) return;
  // A workstation has no diff: no changed-only filter and no checkpoints.
  const diff = hasDiff(c.ws?.kind ?? null);
  dom.changedToggle.hidden = !diff;
  const filter = diff && !!c.state?.filter_changed;
  dom.changedBox.checked = filter;
  const all = [...c.entries.values()];
  const keep = filter ? new Set(c.ide.changedFilter(all, { files: c.files }).map((e) => e.path)) : null;
  const changed = new Set(c.files.map((f) => f.path));
  const active = c.state?.active ?? null;
  // The rows are rebuilt only when their inputs changed: the screen repaints several times a second while events arrive,
  // and a rebuild between a press and its release on a row would drop the click. The workspace-less note is cheap and redone.
  if (!c.ws) {
    painted.delete(dom.treeList);
    replace(dom.treeList, noWorkspaceNote(c));
  } else {
    const key = treeKey({ workspace: c.ws.id, all, loaded: c.loadedDirs, expanded: c.expanded, changed, keep, active, hasWorkspace: true });
    if (painted.get(dom.treeList) !== key) {
      painted.set(dom.treeList, key);
      const rows = treeRows({
        ide: c.ide,
        all,
        loaded: c.loadedDirs,
        expanded: c.expanded,
        changed,
        keep,
        active,
        hasWorkspace: true,
        onOpen: (path) => {
          closeDrawerIfNarrow(c);
          void openTab(c, path);
        },
        onToggle: (path) => void toggleDir(c, path),
      });
      if (rows.length === 0) rows.push(h('p.ide-note', null, filter ? 'No changed files in the loaded tree.' : 'The workspace is empty.'));
      replace(dom.treeList, rows);
    }
  }
  if (!dom.searchHost.hidden) {
    const s = searchUi(c);
    replace(dom.searchHost, s ? s.el : h('p.ide-note', null, 'Search runs in a workspace.'));
  }
  const cpKey = checkpointsKey(c.ws?.id ?? null, diff, c.checkpoints);
  if (painted.get(dom.cpHost) !== cpKey) {
    painted.set(dom.cpHost, cpKey);
    replace(dom.cpHost, c.ws && diff ? checkpointsPanel({ checkpoints: c.checkpoints, canRestore: true, onRestore: (cp) => confirmRestore(c, cp) }) : null);
  }
}

/** What the tree paints, as a key: the inputs that decide its rows. Equal keys mean equal rows. The changed-only filter is on
    when `keep` is not null, and its kept paths are then the key's. */
export interface TreeKeyInput {
  workspace: string | null;
  all: Entry[];
  loaded: Set<string>;
  expanded: Set<string>;
  changed: Set<string>;
  keep: Set<string> | null;
  active: string | null;
  hasWorkspace: boolean;
}

export function treeKey(p: TreeKeyInput): string {
  return JSON.stringify([
    p.workspace,
    p.hasWorkspace,
    [...p.loaded].sort(),
    [...p.expanded].sort(),
    p.all.map((e) => [e.path, e.kind, e.binary]),
    [...p.changed].sort(),
    p.keep === null ? null : [...p.keep].sort(),
    p.active,
  ]);
}

/** What the checkpoints panel paints, as a key: the workspace, and each checkpoint's number, label and file count. A
    workspace without a diff shows no panel (the key is then null). */
export function checkpointsKey(workspace: string | null, diff: boolean, checkpoints: { n: number; label: string; files: number }[]): string {
  return JSON.stringify(workspace === null || !diff ? null : [workspace, checkpoints.map((cp) => [cp.n, cp.label, cp.files])]);
}

/** The bottom drawer: its tab strip, and the run panel while the Run tab shows. */
export function renderDrawer(c: IdeCtx): void {
  const { frame } = c.dom;
  frame.drawer.el.hidden = !c.drawerOpen;
  paintTabs(frame.drawer.tabs, c.drawerTab, {});
  if (!c.drawerOpen) return;
  const body = frame.drawer.body;
  if (c.drawerTab === 'run') {
    const p = runFor(c);
    if (p && body.firstElementChild !== p.el) replace(body, p.el);
    p?.show?.();
  } else if (!hasTerminal(c.ws?.kind ?? null, c.cid !== null)) {
    replace(body, empty('terminal', 'Terminals run in agent and workstation workspaces', 'Pick an agent or a workstation to open its terminal.'));
  } else if (body.firstElementChild !== c.dom.termHost) {
    // The Run panel leaves the drawer; the terminal (if it belongs here) is placed by renderTerminal.
    body.replaceChildren();
  }
}

export function renderStatus(c: IdeCtx): void {
  const active = c.state?.active ?? null;
  let cursor: StatusInfo['cursor'] = null;
  if (active !== null && c.centreView === 'editor') {
    const s = c.dom.editor.current();
    const at = s.selection.main.head;
    const line = s.doc.lineAt(at);
    cursor = { line: line.number, col: at - line.from + 1 };
  }
  const base = c.state?.base;
  const vs = base !== undefined && typeof base === 'object' && 'checkpoint' in base ? `vs checkpoint #${base.checkpoint}` : 'vs work start';
  c.dom.status.paint({
    workspace: c.ws?.id ?? '',
    base: c.ws && hasDiff(c.ws.kind) ? vs : '',
    live: c.connected,
    cursor,
    lang: active !== null ? LANG_LABEL[languageFor(active)] : '',
    dirty: [...c.tabsByPath.values()].filter((t) => t.buf.dirty).length,
  });
}
