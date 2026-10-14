// The IDE's editing: the open files as tabs, the editor's state for each, saving with the conflict rules (a save over a
// newer write from the agent shows the conflict, and Reload or Keep mine settles it), and the tab strip and banners.
import { ApiError, ideApi } from '../api';
import { applyBuffer, openBuffer } from '../ide/model';
import { button, modal, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import { stateFor } from './ide-editor';
import { checkOpen, refreshChanges } from './ide-data';
import { message, type IdeCtx, type Tab } from './ide-ctx';

/** Opens a file in a tab (or shows its tab) and, when asked, moves the cursor to a line. */
export async function openTab(c: IdeCtx, path: string, line?: number): Promise<void> {
  if (!c.ws || !c.ide || !c.state) return;
  if (!c.tabsByPath.has(path)) {
    try {
      c.ide.checkRead(path);
    } catch (err) {
      toast(message(err), 'error');
      return;
    }
    const id = c.ws.id;
    try {
      const f = await ideApi.file(id, path);
      if (!c.alive || c.ws?.id !== id) return;
      const t: Tab = { buf: openBuffer(path, f.hash), state: null, version: 0 };
      t.state = stateFor(path, f.text, editorOpts(c, path, !c.ws.writable));
      c.tabsByPath.set(path, t);
    } catch (err) {
      toast(message(err), 'error');
      return;
    }
  }
  c.state = c.ide.reduce(c.state, { open: path });
  c.state = c.ide.reduce(c.state, { activate: path });
  c.centreView = 'editor';
  showTab(c);
  if (line !== undefined) c.dom.editor.reveal(line);
  c.act.renderAll();
}

/** The editor options of one file: read-only when the workspace is, and the edit and cursor callbacks. */
function editorOpts(c: IdeCtx, path: string, readOnly: boolean) {
  return { readOnly, onEdit: () => onEdit(c, path), onSelect: () => c.act.renderStatus() };
}

/** Makes the editor show the reducer's active tab, keeping the state of the tab it leaves. */
export function showTab(c: IdeCtx): void {
  const want = c.state?.active ?? null;
  if (want === c.shown) return;
  if (c.shown !== null) {
    const leaving = c.tabsByPath.get(c.shown);
    if (leaving) leaving.state = c.dom.editor.current();
  }
  c.shown = want;
  if (want === null) return;
  const t = c.tabsByPath.get(want);
  if (t?.state) {
    c.dom.editor.show(t.state);
    t.state = null;
  }
}

export async function closeTab(c: IdeCtx, path: string): Promise<void> {
  if (!c.ide || !c.state) return;
  const t = c.tabsByPath.get(path);
  if (t?.buf.dirty && !(await confirmDiscard(path))) return;
  if (c.shown === path) c.shown = null;
  c.tabsByPath.delete(path);
  c.state = c.ide.reduce(c.state, { close: path });
  showTab(c);
  c.act.renderAll();
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

export function activate(c: IdeCtx, path: string): void {
  if (!c.ide || !c.state) return;
  c.state = c.ide.reduce(c.state, { activate: path });
  showTab(c);
  c.act.renderAll();
}

function onEdit(c: IdeCtx, path: string): void {
  const t = c.tabsByPath.get(path);
  if (!t) return;
  t.buf = applyBuffer(t.buf, { kind: 'edit' });
  t.version++;
  c.saved = false;
  renderTabs(c);
  c.act.renderTop();
  c.act.renderStatus();
}

export async function save(c: IdeCtx): Promise<void> {
  const path = c.state?.active ?? null;
  const t = path !== null ? c.tabsByPath.get(path) : undefined;
  if (!c.ws || !c.ide || path === null || !t || !c.ws.writable || !t.buf.dirty || t.buf.saving) return;
  try {
    c.ide.checkWrite(path);
  } catch (err) {
    toast(message(err), 'error');
    return;
  }
  const id = c.ws.id;
  const text = c.dom.editor.current().doc.toString();
  const version = t.version;
  t.buf = applyBuffer(t.buf, { kind: 'save' });
  c.act.renderAll();
  try {
    const r = await ideApi.save(id, { path, text, expected_hash: t.buf.base });
    const cur = c.tabsByPath.get(path);
    if (!c.alive || c.ws?.id !== id || !cur) return;
    cur.buf = applyBuffer(cur.buf, { kind: 'saved', hash: r.hash, edited: cur.version !== version });
    c.saved = true;
    await refreshChanges(c);
  } catch (err) {
    const cur = c.tabsByPath.get(path);
    if (!cur || c.ws?.id !== id) return;
    if (err instanceof ApiError && err.status === 409) {
      const current = (err.body as { current_hash?: unknown } | null)?.current_hash;
      if (typeof current === 'string') cur.buf = applyBuffer(cur.buf, { kind: 'conflict', current });
      else cur.buf = applyBuffer(cur.buf, { kind: 'save_failed' });
      if (typeof current !== 'string') void checkOpen(c, path);
    } else {
      cur.buf = applyBuffer(cur.buf, { kind: 'save_failed' });
      toast(message(err), 'error');
    }
  }
  c.act.renderAll();
}

/** "Reload": the buffer takes the hub's copy, and its edits go. */
export async function reloadTab(c: IdeCtx, path: string): Promise<void> {
  const t = c.tabsByPath.get(path);
  if (!c.ws || !t) return;
  const id = c.ws.id;
  try {
    const f = await ideApi.file(id, path);
    const cur = c.tabsByPath.get(path);
    if (!c.alive || c.ws?.id !== id || !cur) return;
    cur.buf = applyBuffer(cur.buf, { kind: 'reload', hash: f.hash });
    if (c.shown === path) c.dom.editor.setText(f.text);
    else cur.state = stateFor(path, f.text, editorOpts(c, path, !(c.ws?.writable ?? false)));
  } catch (err) {
    toast(message(err), 'error');
  }
  c.act.renderAll();
}

/** "Keep mine": the buffer re-bases on the hub's copy and saves its edits over it. */
async function keepMine(c: IdeCtx, path: string): Promise<void> {
  const t = c.tabsByPath.get(path);
  if (!t || t.buf.conflict === null) return;
  t.buf = applyBuffer(t.buf, { kind: 'keep_mine', current: t.buf.conflict });
  await save(c);
}

/** From the review: the editor shows the file. */
export function openFromChanges(c: IdeCtx, path: string): void {
  void openTab(c, path);
}

/** The tab strip: one tab per open file, with its dirty and conflict marks. */
export function renderTabs(c: IdeCtx): void {
  const items = (c.state?.open ?? []).map((path) => {
    const t = c.tabsByPath.get(path);
    const name = path.slice(path.lastIndexOf('/') + 1);
    const on = path === c.state?.active;
    return h(
      'div.ide-tab',
      { class: on ? 'on' : '', role: 'tab', 'aria-selected': on ? 'true' : 'false', title: path },
      h(
        'button.ide-tab-name',
        { type: 'button', onclick: () => activate(c, path) },
        t?.buf.conflict != null ? h('span.ide-conflict-mark', { title: 'Conflict' }, '!') : null,
        t?.buf.dirty ? h('span.ide-dirty', { title: 'Unsaved changes' }, '●') : null,
        name,
      ),
      h('button.ide-tab-x', { type: 'button', title: `Close ${name}`, 'aria-label': `Close ${name}`, onclick: () => void closeTab(c, path) }, '×'),
    );
  });
  replace(c.dom.tabBar, items);
}

/** The banners over the editor: a conflict on the active file, a read-only workspace, and the agent working in it. */
export function renderBanners(c: IdeCtx): void {
  const active = c.state?.active ?? null;
  const t = active !== null ? c.tabsByPath.get(active) : undefined;
  const rows: Node[] = [];
  if (active !== null && t?.buf.conflict != null) {
    rows.push(
      h(
        'div.ide-banner.conflict',
        { role: 'alert' },
        h('span', null, `${active} changed on the agent's side while you had unsaved edits. Reload replaces your edits with that copy.`),
        h('span.spacer'),
        button('Reload', () => void reloadTab(c, active), { size: 'sm' }),
        button('Keep mine', () => void keepMine(c, active), { kind: 'primary', size: 'sm' }),
      ),
    );
  }
  if (c.ws && !c.ws.writable) rows.push(h('div.ide-banner', null, 'This workspace is read-only.'));
  if (c.agentWorking && c.cid !== null) rows.push(h('div.ide-banner', null, 'The agent is working in this workspace. A save over its newer write shows a conflict.'));
  replace(c.dom.banners, rows);
}
