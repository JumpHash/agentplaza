// The IDE's data: the workspace's tree, changes and checkpoints; switching the workspace; the live refresh that follows the
// plaza's writes; and the restore of a checkpoint. The controller's state is on the context (ide-ctx.ts).
import { api, ideApi, type IdeCheckpoint, type IdeWorkspaceInfo } from '../api';
import { dirsOf, lineCounts, LAST_WORKSPACE_KEY, onRemote, parentDir, parseViewed, viewedKey, withViewed } from '../ide/model';
import type { Base, Entry, FileChange, Stats } from '../ide/types';
import { toast } from '../ui/components';
import { disposeTerminal } from './ide-term-host';
import { cidOf, message, readStore, writeStore, type IdeCtx, type Tab } from './ide-ctx';

export const REFRESH_MS = 400;

export function totalsOf(files: FileChange[]): Stats {
  let additions = 0;
  let deletions = 0;
  for (const f of files) {
    const c = lineCounts(f);
    additions += c.add;
    deletions += c.del;
  }
  return { files: files.length, additions, deletions };
}

export function toBase(v: string): Base {
  return v.startsWith('checkpoint:') ? { checkpoint: Number(v.slice('checkpoint:'.length)) } : 'work_start';
}

export function baseValue(c: IdeCtx): string {
  const b = c.state?.base;
  return b !== undefined && typeof b === 'object' && 'checkpoint' in b ? `checkpoint:${b.checkpoint}` : 'work_start';
}

export function basesOf(c: IdeCtx): { value: string; label: string }[] {
  return [{ value: 'work_start', label: 'Since work started' }, ...c.checkpoints.map((cp) => ({ value: `checkpoint:${cp.n}`, label: `#${cp.n} ${cp.label || 'checkpoint'}` }))];
}

/** Fetches every page of one directory and replaces what was loaded for it. */
export async function loadDir(c: IdeCtx, dir: string): Promise<void> {
  if (!c.ws) return;
  const id = c.ws.id;
  const found: Entry[] = [];
  let cursor: string | null = null;
  do {
    const page = await ideApi.tree(id, dir, cursor);
    if (!c.alive || c.ws?.id !== id) return;
    found.push(...page.entries);
    cursor = page.next;
  } while (cursor);
  for (const path of [...c.entries.keys()]) if (parentDir(path) === dir) c.entries.delete(path);
  for (const e of found) c.entries.set(e.path, e);
  c.loadedDirs.add(dir);
}

/** Loads the directories above each changed file, so the changed-only tree can show them. */
export async function loadChangedDirs(c: IdeCtx): Promise<void> {
  const dirs = new Set<string>();
  for (const f of c.files) for (const d of dirsOf(f.path)) if (!c.loadedDirs.has(d)) dirs.add(d);
  for (const d of dirs) await loadDir(c, d).catch((err: unknown) => toast(message(err), 'error'));
  if (c.alive) c.act.renderAll();
}

export async function refreshChanges(c: IdeCtx): Promise<void> {
  if (!c.ws || !c.ide) return;
  const id = c.ws.id;
  const base = baseValue(c);
  try {
    const changes = await ideApi.changes(id, base);
    if (!c.alive || c.ws?.id !== id) return;
    c.files = changes.files ?? c.ide.parseUnified(changes.diff ?? '').files;
    c.stats = changes.stats ?? totalsOf(c.files);
    c.changesError = '';
  } catch (err) {
    if (c.ws?.id === id) c.changesError = message(err);
    return;
  }
  // The changed-only tree needs the directories above the changed files.
  if (c.state?.filter_changed) await loadChangedDirs(c);
}

export async function refreshCheckpoints(c: IdeCtx): Promise<void> {
  if (!c.ws) return;
  const id = c.ws.id;
  try {
    const list = await ideApi.checkpoints(id);
    if (c.alive && c.ws?.id === id) c.checkpoints = list;
  } catch {
    if (c.alive && c.ws?.id === id) c.checkpoints = [];
  }
}

/** Compares an open tab with the hub: a clean buffer reloads, an edited one shows the conflict. */
export async function checkOpen(c: IdeCtx, path: string): Promise<void> {
  const t = c.tabsByPath.get(path);
  if (!c.ws || !t || t.buf.saving) return;
  const id = c.ws.id;
  try {
    const f = await ideApi.file(id, path);
    const cur: Tab | undefined = c.tabsByPath.get(path);
    if (!c.alive || c.ws?.id !== id || !cur || cur.buf.saving) return;
    const res = onRemote(cur.buf, f.hash);
    cur.buf = res.buffer;
    if (res.action === 'reload') await c.act.reloadTab(path);
  } catch {
    // The file is gone or refused: the tab keeps what it has.
  }
}

export function setBase(c: IdeCtx, value: string): void {
  if (!c.ide || !c.state) return;
  c.state = c.ide.reduce(c.state, { set_base: toBase(value) });
  void refreshChanges(c).then(() => {
    if (c.alive) c.act.renderAll();
  });
}

export function markViewed(c: IdeCtx, path: string, mark: string, on: boolean): void {
  if (!c.ws) return;
  c.marks = withViewed(c.marks, path, mark, on);
  writeStore(viewedKey(c.ws.id), JSON.stringify(c.marks));
  c.act.renderAll();
}

/** Shows a workspace in the editor and the diff: loads its tree, changes and checkpoints. Null clears the view. */
export async function setWorkspace(c: IdeCtx, info: IdeWorkspaceInfo | null): Promise<void> {
  if (info?.id === c.ws?.id && info !== null) return;
  disposeTerminal(c);
  c.run?.dispose();
  c.run = null;
  c.ws = info;
  c.entries.clear();
  c.loadedDirs.clear();
  c.expanded.clear();
  c.tabsByPath.clear();
  c.shown = null;
  c.files = [];
  c.stats = { files: 0, additions: 0, deletions: 0 };
  c.checkpoints = [];
  c.saved = false;
  if (!info || !c.ide) {
    c.state = c.ide ? c.ide.initialState() : null;
    c.act.renderAll();
    return;
  }
  writeStore(LAST_WORKSPACE_KEY, info.id);
  c.marks = parseViewed(readStore(viewedKey(info.id)));
  try {
    const canonical = c.ide.parseWorkspaceId(info.id);
    c.state = c.ide.reduce(c.ide.initialState(), { set_workspace: canonical });
  } catch (err) {
    toast(message(err), 'error');
    c.state = c.ide.initialState();
  }
  c.act.renderAll();
  try {
    await loadDir(c, '');
    await Promise.all([refreshChanges(c), refreshCheckpoints(c)]);
  } catch (err) {
    if (c.alive) toast(message(err), 'error');
  }
  if (!c.alive || c.ws?.id !== info.id) return;
  if (c.startFile) await c.act.openTab(c.startFile);
  c.act.renderAll();
}

/**
 * The workspace of a selected agent: its `agent:<cid>` workspace from the list. An agent with a VM that the list does not
 * know yet gets the list again first. An agent with no VM has no workspace, so the view is cleared.
 */
export async function showAgentWorkspace(c: IdeCtx, cid: number): Promise<void> {
  const id = `agent:${cid}`;
  let info = c.wsList.find((w) => w.id === id) ?? null;
  const hasVm = (c.agentList.find((a) => a.cid === cid)?.vm ?? null) !== null;
  if (!info && hasVm) {
    try {
      c.wsList = await ideApi.workspaces();
    } catch {
      // The list the IDE has stays.
    }
    if (!c.alive || c.cid !== cid) return;
    info = c.wsList.find((w) => w.id === id) ?? null;
  }
  await setWorkspace(c, info);
}

/** Schedules a refresh of the changes, the loaded directories and the open tabs (one path, or all of them). */
export function scheduleRefresh(c: IdeCtx, path?: string): void {
  if (path) c.pendingPaths.add(path);
  else c.pendingAll = true;
  clearTimeout(c.refreshTimer);
  c.refreshTimer = setTimeout(() => {
    const paths = new Set(c.pendingPaths);
    const all = c.pendingAll;
    c.pendingPaths.clear();
    c.pendingAll = false;
    void refreshFor(c, paths, all);
  }, REFRESH_MS);
}

async function refreshFor(c: IdeCtx, paths: Set<string>, all: boolean): Promise<void> {
  if (!c.ws || !c.alive) return;
  await refreshChanges(c);
  const dirs = new Set<string>();
  for (const p of paths) if (c.loadedDirs.has(parentDir(p))) dirs.add(parentDir(p));
  for (const d of dirs) await loadDir(c, d).catch(() => undefined);
  for (const p of all ? [...c.tabsByPath.keys()] : [...paths]) if (c.tabsByPath.has(p)) await checkOpen(c, p);
  if (c.alive) c.act.renderAll();
}

/** The projects, the task list and the workspaces follow the polls: the sidebar, the board, the tasks and the palette. */
export async function refreshProjects(c: IdeCtx): Promise<void> {
  try {
    c.projects = await api.projects();
  } catch {
    // The sidebar shows what it has; the next poll tries again.
  }
  try {
    c.tasks = await api.tasks();
  } catch {
    // The grouping keeps the tasks it has.
  }
  try {
    c.wsList = await ideApi.workspaces();
  } catch {
    // The list the IDE has stays until the next poll.
  }
  if (c.alive) c.act.renderAll();
}

/** Puts a checkpoint's files back: the changes, the checkpoints, the loaded directories and the open tabs follow. */
export async function restore(c: IdeCtx, cp: IdeCheckpoint): Promise<void> {
  if (!c.ws) return;
  const id = c.ws.id;
  try {
    await ideApi.restore(id, cp.n);
    toast(`Restored checkpoint #${cp.n}`);
  } catch (err) {
    toast(message(err), 'error');
    return;
  }
  await Promise.all([refreshChanges(c), refreshCheckpoints(c)]);
  for (const d of [...c.loadedDirs]) await loadDir(c, d).catch(() => undefined);
  for (const p of [...c.tabsByPath.keys()]) await checkOpen(c, p);
  if (c.alive && c.ws?.id === id) c.act.renderAll();
}

/** The agent's cid of the workspace the IDE last showed, when it was an agent's (else null). */
export function lastAgentCid(): number | null {
  const last = readStore(LAST_WORKSPACE_KEY);
  return last === null ? null : cidOf(last);
}
