// The IDE's panels that reach past the files: the agent's held tool calls with their proposed changes (Approvals), and one
// bounded command (Run, in the drawer). The terminal is ide-terminal.ts. Each panel is a controller with its own element;
// the screen (views/ide.ts) keeps them per selection, so a panel switch keeps a running command, and disposes them when
// the selection changes. Pure rendering over the hub's routes.

import { api, ApiError, ideApi, type Approval, type IdeWorkspaceInfo } from '../api';
import { runsFor } from '../ide/layout';
import { on } from '../store';
import { ago, attempt, badge, button, empty, input, textarea, toast } from '../ui/components';
import { h, replace } from '../ui/dom';

/** One tab's controller: its element, a refresh for when the tab is shown, and the teardown. */
export interface Panel {
  el: HTMLElement;
  show?(): void;
  dispose(): void;
}

/** The poll interval for a run's output, in milliseconds. */
const RUN_POLL_MS = 500;
/** The tools whose input carries a file change: the Approvals tab shows it as a diff. */
const WRITES = new Set(['edit']);
/** Unchanged lines kept on each side of a change; a longer run of them collapses. */
const CONTEXT = 3;
/** Above this many cells (lines of before times lines of after) the diff is not computed. */
const DIFF_CELLS = 2_000_000;

const message = (err: unknown): string => (err instanceof Error ? err.message : String(err));

/** The entries of an edit's `create` or `edits` list that name `path`, in call order. */
function entriesFor(args: Record<string, unknown>, key: 'create' | 'edits', path: string): Record<string, unknown>[] {
  const list = Array.isArray(args[key]) ? (args[key] as Record<string, unknown>[]) : [];
  return list.filter((entry) => entry && entry['path'] === path);
}

/** The files an edit names, each once, in the order its call names them. */
function editPaths(args: Record<string, unknown>): string[] {
  const names: string[] = [];
  for (const key of ['create', 'edits'] as const) {
    const list = Array.isArray(args[key]) ? (args[key] as Record<string, unknown>[]) : [];
    for (const entry of list) {
      const path = entry && typeof entry['path'] === 'string' ? entry['path'] : null;
      if (path !== null && !names.includes(path)) names.push(path);
    }
  }
  return names;
}

// ---- Agent ------------------------------------------------------------------

/** A content block of a model response, as the transcript carries it. */

// ---- Approvals --------------------------------------------------------------

/** One line of a diff: kept, added or removed. */
export interface DiffRow {
  kind: 'context' | 'add' | 'remove';
  text: string;
}

const SIGN: Record<DiffRow['kind'], string> = { context: ' ', add: '+', remove: '-' };

/** A line diff by longest common subsequence; null when the two texts are too big to compare line by line. */
export function lineDiff(before: string, after: string): DiffRow[] | null {
  const a = before === '' ? [] : before.split('\n');
  const b = after === '' ? [] : after.split('\n');
  const n = a.length;
  const m = b.length;
  if ((n + 1) * (m + 1) > DIFF_CELLS) return null;
  const w = m + 1;
  const lcs = new Uint32Array((n + 1) * w);
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i * w + j] = a[i] === b[j] ? lcs[(i + 1) * w + j + 1]! + 1 : Math.max(lcs[(i + 1) * w + j]!, lcs[i * w + j + 1]!);
    }
  }
  const rows: DiffRow[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      rows.push({ kind: 'context', text: a[i]! });
      i++;
      j++;
    } else if (lcs[(i + 1) * w + j]! >= lcs[i * w + j + 1]!) {
      rows.push({ kind: 'remove', text: a[i]! });
      i++;
    } else {
      rows.push({ kind: 'add', text: b[j]! });
      j++;
    }
  }
  while (i < n) rows.push({ kind: 'remove', text: a[i++]! });
  while (j < m) rows.push({ kind: 'add', text: b[j++]! });
  return rows;
}

/** The rows of a diff, with long unchanged runs collapsed to a count. */
function diffNode(rows: DiffRow[]): HTMLElement {
  const keep = rows.map(() => false);
  rows.forEach((r, i) => {
    if (r.kind === 'context') return;
    for (let k = Math.max(0, i - CONTEXT); k <= Math.min(rows.length - 1, i + CONTEXT); k++) keep[k] = true;
  });
  const out: Node[] = [];
  let gap = 0;
  const flush = (): void => {
    if (gap) out.push(h('p.ide-note', null, `… ${gap} unchanged ${gap === 1 ? 'line' : 'lines'}`));
    gap = 0;
  };
  rows.forEach((r, i) => {
    if (!keep[i]) {
      gap++;
      return;
    }
    flush();
    // The grid has two line-number columns, left empty: a proposed change has no numbers in the file yet.
    out.push(h('div.ided-ln', { class: r.kind === 'add' ? 'add' : r.kind === 'remove' ? 'del' : '' }, h('span'), h('span'), h('span', null, SIGN[r.kind]), h('span', null, r.text)));
  });
  flush();
  return h('div.ided-hunk', null, out);
}

/** The Approvals tab: the agent's held tool calls in this workspace, each with its input or its proposed change. */
export function approvalsPanel(opts: { wsId: string; cid: number; onCount(n: number): void; onDecided(): void }): Panel {
  const { wsId, cid } = opts;
  let alive = true;
  let generation = 0;
  const list = h('div.ided-appr-list');
  const el = h('div.ided-panel', null, list);

  async function load(): Promise<void> {
    const mine = ++generation;
    try {
      const held = (await api.approvals()).filter((a) => a.cid === cid && a.kind === 'tool' && a.state === 'pending');
      if (!alive || mine !== generation) return;
      opts.onCount(held.length);
      replace(list, held.length ? held.map(card) : empty('approval', 'Nothing is held', 'The agent asks here before a write or a command when its mode is ask.'));
    } catch (err) {
      if (alive) replace(list, empty('alert', 'Approvals did not load', message(err)));
    }
  }

  function card(a: Approval): HTMLElement {
    const args = (a.input && typeof a.input === 'object' ? a.input : {}) as Record<string, unknown>;
    const paths = WRITES.has(a.tool) ? editPaths(args) : [];
    const body = h('div.ided-appr-body');
    const reason = textarea({ rows: 2, placeholder: 'Reason (optional)', 'aria-label': 'Reason' });
    const approve = button('Approve', () => void decide(true), { kind: 'primary', size: 'sm', icon: 'check' });
    const deny = button('Deny', () => void decide(false), { kind: 'danger', size: 'sm', icon: 'x' });
    const node = h(
      'article.ided-appr',
      null,
      h('header.ided-appr-h', null, badge(a.tool, 'info'), paths.length ? h('code', null, paths.join(', ')) : null, h('span.sp'), h('span.muted', null, ago(a.created_ms))),
      body,
      reason,
      h('div.ided-appr-acts', null, deny, approve),
    );
    if (paths.length) {
      // One diff per file the edit names.
      for (const path of paths) {
        const block = h('div.ided-appr-file');
        body.append(block);
        void showChange(block, path, args);
      }
    } else replace(body, h('pre.ided-pre', null, JSON.stringify(a.input, null, 2)));

    async function decide(ok: boolean): Promise<void> {
      approve.disabled = true;
      deny.disabled = true;
      const note = reason.value.trim();
      const done = await attempt(() => api.decideApproval(a.id, ok, note || undefined).then(() => true), ok ? 'Approved' : 'Denied');
      if (!alive) return;
      if (done) {
        await load();
        opts.onDecided();
      } else {
        approve.disabled = false;
        deny.disabled = false;
      }
    }
    return node;
  }

  /** The proposed change to one file against its text as it is now: a create's content, or the edits' old and new text in turn. */
  async function showChange(host: HTMLElement, path: string, args: Record<string, unknown>): Promise<void> {
    replace(host, h('p.muted', null, 'Reading the current file…'));
    const creates = entriesFor(args, 'create', path);
    const edits = entriesFor(args, 'edits', path);
    let before: string;
    try {
      before = (await ideApi.file(wsId, path)).text;
    } catch (err) {
      // A create may make the file: a missing file is an empty one for a create only.
      if (creates.length === 0 || !(err instanceof ApiError && err.status === 404)) {
        if (alive) replace(host, h('p.ide-note.error', null, `The current file did not load: ${message(err)}`), h('pre.ided-pre', null, JSON.stringify(args, null, 2)));
        return;
      }
      before = '';
    }
    if (!alive) return;
    let pair: [string, string] | null = null;
    let note = '';
    const create = creates[creates.length - 1];
    if (create) {
      if (typeof create['content'] === 'string') pair = [before, create['content']];
      else note = 'The write carries no content.';
    } else {
      let text = before;
      let fails = false;
      for (const edit of edits) {
        const oldText = typeof edit['old'] === 'string' ? edit['old'] : '';
        const newText = typeof edit['new'] === 'string' ? edit['new'] : '';
        const all = edit['replace_all'] === true;
        const hits = oldText === '' ? 0 : text.split(oldText).length - 1;
        if (hits === 0) {
          // The edit cannot apply to this file now: show the text it would change, not the file.
          pair = [oldText, newText];
          note = 'The old text is not in the current file, so this edit would fail.';
          fails = true;
          break;
        }
        if (hits > 1 && !all) note = `The old text appears ${hits} times; the diff shows the first.`;
        text = all ? text.split(oldText).join(newText) : text.replace(oldText, newText);
      }
      if (!fails) pair = [before, text];
    }
    if (!pair) {
      replace(host, h('p.ide-note', null, note));
      return;
    }
    if (pair[0] === pair[1]) note = note || 'No change to the file.';
    const rows = lineDiff(pair[0], pair[1]);
    replace(
      host,
      note ? h('p.ide-note', null, note) : null,
      rows ? diffNode(rows) : h('p.ide-note', null, 'This change is too large to show as a diff.'),
    );
  }

  const off = on('approval', () => {
    if (alive) void load();
  });
  void load();
  return {
    el,
    show: () => void load(),
    dispose: () => {
      alive = false;
      off();
    },
  };
}

// ---- Run --------------------------------------------------------------------

/** The Run tab: one bounded command in an agent's or a workstation's workspace, its output polled from the hub, and a stop. */
export function runPanel(ws: IdeWorkspaceInfo): Panel {
  const enabled = runsFor(ws.kind);
  let alive = true;
  let run: string | null = null;
  let offset = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const command = input({ placeholder: 'npm test', 'aria-label': 'Command', autocomplete: 'off', spellcheck: false, disabled: !enabled });
  const seconds = input({ type: 'number', min: 1, max: 600, value: 60, 'aria-label': 'Timeout in seconds', disabled: !enabled });
  const go = button('Run', () => void start(), { kind: 'primary', size: 'sm', icon: 'play', disabled: !enabled });
  const stop = button('Stop', () => void stopRun(), { kind: 'danger', size: 'sm', icon: 'stop', disabled: true });
  const status = h('p.ided-status', { 'aria-live': 'polite' }, enabled ? 'Not run yet.' : 'Runs are for agent and workstation workspaces. Repository runs come in phase 3.');
  const out = h('pre.ided-pre', { 'aria-label': 'Output' });
  const el = h(
    'div.ided-run',
    null,
    h('div.ided-run-form', null, command, seconds, go, stop),
    status,
    out,
  );

  const setStatus = (text: string, bad = false): void => {
    status.textContent = text;
    status.classList.toggle('error', bad);
  };
  const busy = (on: boolean): void => {
    go.disabled = !enabled || on;
    stop.disabled = !on;
    command.disabled = on;
  };

  async function start(): Promise<void> {
    const text = command.value.trim();
    if (!text) {
      toast('Write a command first', 'error');
      return;
    }
    const timeout = Math.min(600, Math.max(1, Math.round(Number(seconds.value) || 60)));
    out.textContent = '';
    offset = 0;
    busy(true);
    setStatus('Starting…');
    try {
      run = (await ideApi.runStart(ws.id, text, timeout)).run;
    } catch (err) {
      busy(false);
      setStatus(message(err), true);
      return;
    }
    if (!alive) return;
    setStatus('Running…');
    void poll();
  }

  /** Reads the output from the last offset; the next read starts where this one ended. */
  async function poll(): Promise<void> {
    if (!alive || run === null) return;
    const id = run;
    try {
      const p = await ideApi.runPoll(ws.id, id, offset);
      if (!alive) return;
      if (p.output) {
        out.append(p.output);
        out.scrollTop = out.scrollHeight;
      }
      offset = p.next;
      if (!p.done) {
        timer = setTimeout(() => void poll(), RUN_POLL_MS);
        return;
      }
      run = null;
      busy(false);
      const exit = p.exit_code ?? 'none';
      setStatus(p.timed_out ? `Timed out. Exit ${exit}.` : `Exit ${exit}.`, p.exit_code !== 0);
    } catch (err) {
      if (!alive) return;
      run = null;
      busy(false);
      setStatus(message(err), true);
    }
  }

  async function stopRun(): Promise<void> {
    if (run === null) return;
    stop.disabled = true;
    try {
      await ideApi.runStop(ws.id, run);
      setStatus('Stopping…');
    } catch (err) {
      stop.disabled = false;
      toast(message(err), 'error');
    }
  }

  return {
    el,
    dispose: () => {
      alive = false;
      clearTimeout(timer);
    },
  };
}
