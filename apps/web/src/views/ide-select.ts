// The IDE's selection: which agent, repository or workstation the centre shows, and which view the centre and the right panel
// show. Choosing an agent shows its workspace (ide-data); the sidebar's keys and the palette come here. What a selection has
// (its views, its tabs) is the kind's rule in ide/layout.
import { agentLabel, agentName } from '../roles';
import type { IdeWorkspaceInfo, Project } from '../api';
import { groupByProject, startAgent, projectOf } from '../ide/groups';
import { centreViewsFor, rightViewsFor } from '../ide/layout';
import type { WorkspaceKind } from '../ide/types';
import { closeDrawerIfNarrow, openRightPanel } from './ide-layout';
import { setWorkspace, showAgentWorkspace } from './ide-data';
import { type IdeCtx } from './ide-ctx';
import { chatPane } from './ide-chat';
import { prewake } from '../prewake';
import { ideEmbeds } from './ide-rich';
import type { CentreView, RightView } from './ide-frame';

/**
 * Whether the right panel's tab can show for the selection: the kind's tabs (see rightViewsFor). The kind defaults to the
 * shown workspace's; a selection that is changing passes the kind it is changing to.
 */
export function rightTabAllowed(c: IdeCtx, v: RightView, kind: WorkspaceKind | null = c.ws?.kind ?? null): boolean {
  return rightViewsFor(kind, c.cid !== null).includes(v);
}

/** Whether the centre's view can show for the selection: the kind's views (see centreViewsFor). */
export function centreAllowed(c: IdeCtx, v: CentreView, kind: WorkspaceKind | null = c.ws?.kind ?? null): boolean {
  return centreViewsFor(kind, c.cid !== null).includes(v);
}

/** The centre's view. */
export function setCentre(c: IdeCtx, v: CentreView): void {
  c.centreView = v;
  c.act.renderAll();
  if (v === 'terminal') c.term?.focus();
}

/** The right panel's tab. Opens the panel when it is shut. */
export function setRight(c: IdeCtx, v: RightView): void {
  c.rightView = v;
  if (!rightTabAllowed(c, v)) c.rightView = 'files';
  openRightPanel(c);
  c.act.renderAll();
}

/** Opens a sidebar key: `agent:<cid>`, `term:<cid>` (the agent's terminal) or `ws:<workspace id>` (a repository or a workstation). */
export function openKey(c: IdeCtx, key: string): void {
  if (key.startsWith('agent:')) selectAgent(c, Number(key.slice('agent:'.length)), 'chat');
  else if (key.startsWith('term:')) selectAgent(c, Number(key.slice('term:'.length)), 'terminal');
  else if (key.startsWith('ws:')) {
    const w = c.wsList.find((x) => x.id === key.slice('ws:'.length));
    if (w) selectWorkspace(c, w);
  }
  closeDrawerIfNarrow(c);
}

/** Drops the controllers of the selection that moves on (its chat, activity and approvals). */
function disposeSelection(c: IdeCtx): void {
  c.chat?.dispose();
  c.chat = null;
  c.activity?.dispose();
  c.activity = null;
  c.approvals?.dispose();
  c.approvals = null;
}

export function selectAgent(c: IdeCtx, next: number, view: CentreView): void {
  if (c.cid !== next) {
    disposeSelection(c);
    c.cid = next;
  }
  c.centreView = view;
  const agent = c.agentList.find((a) => a.cid === next);
  prewake(next, agent?.status);
  if (!c.chat) {
    const project = agent ? projectOf(agent, c.projects, c.tasks) : null;
    // The project is read at each render: the agent's project can arrive after the chat is made.
    const projectNow = (): Project | null => {
      const a = c.agentList.find((x) => x.cid === next);
      return a ? projectOf(a, c.projects, c.tasks) : null;
    };
    c.chat = chatPane({
      cid: next,
      agentName: agent ? agentName(agent) : agentLabel(next),
      project: project?.title ?? null,
      working: () => c.agentWorking,
      embeds: () => ideEmbeds(c, projectNow()),
    });
  }
  c.dom.sidebar.setSelected(`agent:${next}`);
  void showAgentWorkspace(c, next);
  if (!rightTabAllowed(c, c.rightView)) c.rightView = 'files';
  c.act.renderAll();
}

/**
 * Shows a repository's or a workstation's workspace, with no agent. A view or tab the kind does not have is moved to the
 * editor or the files: a workstation keeps its Terminal, a repository has none.
 */
export function selectWorkspace(c: IdeCtx, info: IdeWorkspaceInfo): void {
  disposeSelection(c);
  c.cid = null;
  if (!centreAllowed(c, c.centreView, info.kind)) c.centreView = 'editor';
  if (!rightTabAllowed(c, c.rightView, info.kind)) c.rightView = 'files';
  c.dom.sidebar.setSelected(`ws:${info.id}`);
  void setWorkspace(c, info);
  c.act.renderAll();
}

/**
 * Chooses the agent the IDE shows when nothing else is chosen (see startAgent): the one selected last time, else the first
 * working one. Does nothing when there are no agents.
 */
export function autoSelect(c: IdeCtx, last: number | null): void {
  const cid = startAgent(groupByProject(c.agentList, c.projects, c.tasks), last);
  if (cid !== null) selectAgent(c, cid, c.centreView === 'editor' ? 'chat' : c.centreView);
}
