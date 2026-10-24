// The IDE's panels: the right panel's controllers (board, tasks, approvals, run, activity), and the Files tab's actions
// (expand a folder, search, quick open, restore a checkpoint). Each controller is made on first use for the selection, and
// disposed when the selection moves on.
import { agentLabelIn } from '../roles';
import { ideApi, type IdeCheckpoint, type Project } from '../api';
import { projectOf } from '../ide/groups';
import { button, modal, toast } from '../ui/components';
import { h } from '../ui/dom';
import { activityPanel } from './ide-activity';
import { boardPanel } from './ide-board';
import { quickOpen, searchView } from './ide-find';
import { closeOverlay, message, repoNameOf, type IdeCtx } from './ide-ctx';
import { loadDir, restore, scheduleRefresh } from './ide-data';
import { openTab } from './ide-editing';
import { ideEmbeds } from './ide-rich';
import { closeDrawerIfNarrow, openRightPanel } from './ide-layout';
import { tasksPanel } from './ide-tasks';
import { approvalsPanel, runPanel, type Panel as TabPanel } from './ide-tabs';

/** The project of the current selection: the agent's, or the repository's. */
export function currentProject(c: IdeCtx): Project | null {
  if (c.cid !== null) {
    const a = c.agentList.find((x) => x.cid === c.cid);
    return a ? projectOf(a, c.projects, c.tasks) : null;
  }
  const ws = c.ws;
  if (ws?.kind === 'repo') return c.projects.find((p) => p.repo === repoNameOf(ws)) ?? null;
  return null;
}

/** The board of the project (made again when the project changes). */
export function boardFor(c: IdeCtx, project: Project | null) {
  const key = project?.id ?? '';
  if (!c.board || c.boardKey !== key) {
    c.board?.dispose();
    c.board = boardPanel({
      project,
      onCount: (n) => {
        c.boardCount = n;
        c.act.renderRightTabs();
      },
      embeds: () => ideEmbeds(c, project),
    });
    c.boardKey = key;
  }
  return c.board;
}

/** The tasks of the project (made again when the project changes). */
export function tasksFor(c: IdeCtx, project: Project | null) {
  const key = project?.id ?? '';
  if (!c.taskPanel || c.taskKey !== key) {
    c.taskPanel?.dispose();
    c.taskPanel = tasksPanel({
      project,
      onCount: (done, total) => {
        c.taskCounts = { done, total };
        c.act.renderRightTabs();
      },
    });
    c.taskKey = key;
  }
  return c.taskPanel;
}

/** The activity of the selected agent (made on first use). */
export function activityFor(c: IdeCtx, cid: number) {
  if (!c.activity) c.activity = activityPanel({ cid, agentName: agentLabelIn(c.agentList, cid) });
  return c.activity;
}

/** The approvals of the selected agent, made on first use. Null for a repository. */
export function approvalsFor(c: IdeCtx): TabPanel | null {
  if (c.cid === null) return null;
  if (!c.approvals) {
    c.approvals = approvalsPanel({
      wsId: `agent:${c.cid}`,
      cid: c.cid,
      onCount: (n) => {
        c.approvalCount = n;
        c.act.renderRightTabs();
      },
      onDecided: () => scheduleRefresh(c),
    });
  }
  return c.approvals;
}

/** The run panel of the workspace, made on first use. */
export function runFor(c: IdeCtx): TabPanel | null {
  if (!c.ws) return null;
  if (!c.run) c.run = runPanel(c.ws);
  return c.run;
}

/** A folder opens or closes in the tree; a folder is loaded the first time it opens. */
export async function toggleDir(c: IdeCtx, path: string): Promise<void> {
  if (c.expanded.has(path)) c.expanded.delete(path);
  else {
    c.expanded.add(path);
    if (!c.loadedDirs.has(path)) await loadDir(c, path).catch((err: unknown) => toast(message(err), 'error'));
  }
  c.act.renderAll();
}

/** The search view for the open workspace (made once per workspace, so its results stay). */
export function searchUi(c: IdeCtx): { el: HTMLElement; focus: () => void } | null {
  if (!c.ws) return null;
  if (!c.search || c.search.wsId !== c.ws.id) {
    const id = c.ws.id;
    c.search = {
      wsId: id,
      ui: searchView(
        (q, glob) => ideApi.search(id, q, glob || undefined),
        (path, line) => {
          closeDrawerIfNarrow(c);
          void openTab(c, path, line);
        },
      ),
    };
  }
  return c.search.ui;
}

/** Ctrl/Cmd+Shift+F and the Files tab's search button: the search view, shown above the tree. */
export function openSearch(c: IdeCtx): void {
  if (!c.ws) return;
  c.rightView = 'files';
  openRightPanel(c);
  c.dom.searchHost.hidden = false;
  c.act.renderAll();
  searchUi(c)?.focus();
}

export function toggleSearch(c: IdeCtx): void {
  if (!c.dom.searchHost.hidden) {
    c.dom.searchHost.hidden = true;
    c.act.renderAll();
    return;
  }
  openSearch(c);
}

/** Ctrl/Cmd+P: the quick open over the files loaded in the tree. */
export function openQuick(c: IdeCtx): void {
  if (!c.ws) return;
  closeOverlay(c);
  c.overlay = quickOpen(
    c.root,
    () => [...c.entries.values()].filter((e) => e.kind === 'file').map((e) => e.path),
    (path) => {
      closeDrawerIfNarrow(c);
      void openTab(c, path);
    },
    () => (c.overlay = null),
  );
}

/** The checkpoint's restore, after a confirm. */
export function confirmRestore(c: IdeCtx, cp: IdeCheckpoint): void {
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
            void restore(c, cp);
          },
          { kind: 'danger' },
        ),
        button('Cancel', () => close(), { kind: 'ghost' }),
      ),
    ),
  );
}
