// The IDE's selection: which agent or repository the centre shows, and which view the centre and the right panel show.
// Choosing an agent shows its workspace (ide-data); the sidebar's keys and the palette come here.
import type { IdeWorkspaceInfo } from '../api';
import { groupByProject, startAgent, projectOf } from '../ide/groups';
import { closeDrawerIfNarrow, setRightOpen } from './ide-layout';
import { setWorkspace, showAgentWorkspace } from './ide-data';
import { type IdeCtx } from './ide-ctx';
import { chatPane } from './ide-chat';
import { openAgentWizard } from './agent-wizard';
import type { CentreView, RightView } from './ide-frame';

/** Whether the right panel's tab can show for the current selection: Approvals and Activity are for agents only. */
export function rightTabAllowed(c: IdeCtx, v: RightView): boolean {
  return c.cid !== null || (v !== 'approvals' && v !== 'activity');
}

/** Whether the centre's view can show: Chat and Terminal need an agent; Editor and Diff need a workspace. */
export function centreAllowed(c: IdeCtx, v: CentreView): boolean {
  if (v === 'chat' || v === 'terminal') return c.cid !== null;
  return true;
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
  if (!c.layout.rightOpen) setRightOpen(c, true);
  else c.act.renderAll();
}

/** Opens a sidebar key: `agent:<cid>`, `term:<cid>` (the agent's terminal) or `ws:<workspace id>` (a repository). */
export function openKey(c: IdeCtx, key: string): void {
  if (key.startsWith('agent:')) selectAgent(c, Number(key.slice('agent:'.length)), 'chat');
  else if (key.startsWith('term:')) selectAgent(c, Number(key.slice('term:'.length)), 'terminal');
  else if (key.startsWith('ws:')) {
    const w = c.wsList.find((x) => x.id === key.slice('ws:'.length));
    if (w) selectRepo(c, w);
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
  if (!c.chat) {
    const project = agent ? projectOf(agent, c.projects, c.tasks) : null;
    c.chat = chatPane({ cid: next, agentName: agent?.name ?? `agent ${next}`, project: project?.title ?? null, working: () => c.agentWorking });
  }
  c.dom.sidebar.setSelected(`agent:${next}`);
  void showAgentWorkspace(c, next);
  if (!rightTabAllowed(c, c.rightView)) c.rightView = 'files';
  c.act.renderAll();
}

export function selectRepo(c: IdeCtx, info: IdeWorkspaceInfo): void {
  disposeSelection(c);
  c.cid = null;
  if (c.centreView === 'chat' || c.centreView === 'terminal') c.centreView = 'editor';
  if (!rightTabAllowed(c, c.rightView)) c.rightView = 'files';
  c.dom.sidebar.setSelected(`ws:${info.id}`);
  void setWorkspace(c, info);
  c.act.renderAll();
}

/** The sidebar's "+ New agent": the agent wizard, in a modal over the IDE. */
export function newAgent(): void {
  openAgentWizard();
}

/**
 * Chooses the agent the IDE shows when nothing else is chosen (see startAgent): the one selected last time, else the first
 * working one. Does nothing when there are no agents.
 */
export function autoSelect(c: IdeCtx, last: number | null): void {
  const cid = startAgent(groupByProject(c.agentList, c.projects, c.tasks), last);
  if (cid !== null) selectAgent(c, cid, c.centreView === 'editor' ? 'chat' : c.centreView);
}
