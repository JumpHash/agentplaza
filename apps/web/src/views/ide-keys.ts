// The IDE's keyboard and command palette: the shortcuts (Ctrl/Cmd keys, the drawer's Ctrl+`, Escape), the palette's items
// (agents, repositories, files, tasks and commands) and what running one of them does.
import { api } from '../api';
import { consoleOrigin } from '../ide/open';
import { projectOf } from '../ide/groups';
import { hasTerminal } from '../ide/layout';
import { osLabel } from '../ide/model';
import { rowStatus, type PaletteItem } from '../ide/view';
import { closeOverlay, inField, inTerminal, inTyping, refOf, repoNameOf, type IdeCtx } from './ide-ctx';
import { closeDrawerIfNarrow, setDrawer, toggleLeftPanel, toggleRightPanel } from './ide-layout';
import { centreAllowed, selectAgent, selectWorkspace, setCentre, setRight } from './ide-select';
import { openQuick, openSearch } from './ide-panels';
import { openTab, save } from './ide-editing';
import type { CentreView, RightView } from './ide-frame';

/** Console (and Escape) goes back to the screen the IDE was opened from. */
export function leaveIde(c: IdeCtx): void {
  closeOverlay(c);
  location.hash = consoleOrigin();
}

/** The palette's items: agents, repositories, the loaded files, the tasks and the commands. */
export function paletteItems(c: IdeCtx): PaletteItem[] {
  const items: PaletteItem[] = [];
  for (const a of c.agentList) {
    const project = projectOf(a, c.projects, c.tasks);
    items.push({ id: `agent:${a.cid}`, group: 'agent', label: a.name, hint: `${rowStatus(a, null)}${project ? ` · ${project.title}` : ''}` });
  }
  for (const w of c.wsList) {
    if (w.kind === 'repo') items.push({ id: `ws:${w.id}`, group: 'agent', label: repoNameOf(w), hint: `repository ${refOf(w)}` });
    else if (w.kind === 'station') items.push({ id: `ws:${w.id}`, group: 'agent', label: w.name || w.owner, hint: `workstation · ${osLabel(w.os)}` });
  }
  for (const e of c.entries.values()) if (e.kind === 'file') items.push({ id: `file:${e.path}`, group: 'file', label: e.path.slice(e.path.lastIndexOf('/') + 1), hint: e.path });
  for (const t of c.tasks) items.push({ id: `task:${t.id}`, group: 'task', label: t.goal, hint: t.state });
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
export async function openPalette(c: IdeCtx): Promise<void> {
  if (c.dom.pal.isOpen()) {
    c.dom.pal.close();
    return;
  }
  try {
    c.tasks = await api.tasks();
  } catch {
    // The palette shows the tasks it has.
  }
  c.dom.pal.open(paletteItems(c));
}

export function runPaletteItem(c: IdeCtx, item: PaletteItem): void {
  const at = item.id.indexOf(':');
  const kind = item.id.slice(0, at);
  const value = item.id.slice(at + 1);
  if (kind === 'agent') selectAgent(c, Number(value), 'chat');
  else if (kind === 'ws') {
    const w = c.wsList.find((x) => x.id === value);
    if (w) selectWorkspace(c, w);
  } else if (kind === 'file') void openTab(c, value);
  else if (kind === 'task') setRight(c, 'tasks');
  else if (kind === 'cmd') runCommand(c, value);
}

function runCommand(c: IdeCtx, id: string): void {
  const views: Record<string, CentreView> = { chat: 'chat', terminal: 'terminal', editor: 'editor', diff: 'diff' };
  const rights: Record<string, RightView> = { board: 'board', files: 'files', tasks: 'tasks', approvals: 'approvals', activity: 'activity' };
  if (views[id]) {
    if (centreAllowed(c, views[id])) setCentre(c, views[id]);
  } else if (rights[id]) setRight(c, rights[id]);
  else if (id === 'drawer') setDrawer(c, !c.drawerOpen);
  else if (id === 'sidebar') toggleLeftPanel(c);
  else if (id === 'right') toggleRightPanel(c);
  else if (id === 'search') openSearch(c);
  else if (id === 'save') void save(c);
  else if (id === 'console') leaveIde(c);
}

/** The document-level key handler. The shortcuts that are letters are off in fields and the terminal. */
export function onKey(c: IdeCtx, e: KeyboardEvent): void {
  if (!c.alive) return;
  const mod = e.metaKey || e.ctrlKey;
  /** The drawer's terminal: an agent's or a workstation's (a repository has none). */
  const termOk = hasTerminal(c.ws?.kind ?? null, c.cid !== null);
  if (mod && e.altKey && !e.shiftKey && e.code === 'KeyB') {
    e.preventDefault();
    toggleLeftPanel(c);
  } else if (mod && e.altKey && !e.shiftKey && e.code === 'KeyJ') {
    e.preventDefault();
    toggleRightPanel(c);
  } else if (mod && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'k') {
    e.preventDefault();
    void openPalette(c);
  } else if (!mod && !e.altKey && !e.shiftKey && e.code === 'Backquote' && !inTyping(e.target) && termOk) {
    e.preventDefault();
    setDrawer(c, !c.drawerOpen);
  } else if (mod && !e.altKey && !e.shiftKey && e.code === 'Backquote' && !inTerminal(e.target) && termOk) {
    e.preventDefault();
    setDrawer(c, !c.drawerOpen);
  } else if (mod && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 's' && !inTerminal(e.target)) {
    e.preventDefault();
    void save(c);
  } else if (mod && !e.altKey && !e.shiftKey && e.key.toLowerCase() === 'p' && !inTerminal(e.target)) {
    e.preventDefault();
    openQuick(c);
  } else if (mod && !e.altKey && e.shiftKey && e.key.toLowerCase() === 'f' && !inTerminal(e.target)) {
    e.preventDefault();
    openSearch(c);
  } else if (e.key === '?' && !mod && !e.altKey && !inTyping(e.target)) {
    e.preventDefault();
    c.dom.help.set(!c.dom.help.isOpen());
  } else if (e.key === 'Escape' && !e.defaultPrevented && c.overlay === null && !c.dom.pal.isOpen() && !document.querySelector('.scrim') && !inField(e.target)) {
    // A field's Escape (the editor's search, a note) is the field's. Otherwise: the help, then a narrow drawer, then the console.
    if (c.dom.help.isOpen()) c.dom.help.set(false);
    else if ((c.narrow && c.sheets.drawer) || (c.mid && c.sheets.sheet)) closeDrawerIfNarrow(c);
    else leaveIde(c);
  }
}
