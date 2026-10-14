// The IDE's right-panel tabs that reach past the files: the agent's transcript and steer box (Agent), the agent's held
// tool calls with their proposed changes (Approvals), one bounded command (Run), and the Terminal placeholder. Each
// tab is a controller with its own element; the screen (views/ide.ts) keeps them per workspace, so a tab switch keeps a
// draft or a running command, and disposes them when the workspace changes. Pure rendering over the hub's routes.

import { api, ApiError, ideApi, type Approval, type Entry, type IdeWorkspaceInfo } from '../api';
import { on } from '../store';
import { runsFor } from '../ide/layout';
import { ago, attempt, badge, button, clip, empty, input, textarea, toast } from '../ui/components';
import { h, replace } from '../ui/dom';

/** One tab's controller: its element, a refresh for when the tab is shown, and the teardown. */
export interface Panel {
  el: HTMLElement;
  show?(): void;
  dispose(): void;
}

/** How much of one reply, prompt or tool output the lists show at once. */
const CLIP = 4000;
/** The transcript entries the Agent tab loads before it follows the live ones. */
const HISTORY = 200;
/** The poll interval for a run's output, in milliseconds. */
const RUN_POLL_MS = 500;
/** The hub's limit for a steer note (harness_control.rs). */
const STEER_MAX = 2000;
/** The tools whose input carries a file change: the Approvals tab shows it as a diff. */
const WRITES = new Set(['write_file', 'edit_file']);
/** Unchanged lines kept on each side of a change; a longer run of them collapses. */
const CONTEXT = 3;
/** Above this many cells (lines of before times lines of after) the diff is not computed. */
const DIFF_CELLS = 2_000_000;

const message = (err: unknown): string => (err instanceof Error ? err.message : String(err));

// ---- Agent ------------------------------------------------------------------

/** A content block of a model response, as the transcript carries it. */
interface Block {
  type?: string;
  text?: string;
  name?: string;
}

/** One transcript entry as a line of the Agent tab, or null for the kinds it leaves out (requests and plain results). */
function entryNode(e: Entry): HTMLElement | null {
  const d = e.data;
  switch (e.kind) {
    case 'work_start':
      return h('div.ide-ag-item', null, h('span.ide-ag-kind', null, String(d['kind'] ?? 'work')), h('p.ide-ag-text', null, clip(String(d['prompt'] ?? ''), CLIP)));
    case 'model_response': {
      const blocks = Array.isArray(d['content']) ? (d['content'] as Block[]) : [];
      const parts = blocks.map((b) => (b.type === 'text' ? h('p.ide-ag-text', null, clip(b.text ?? '', CLIP)) : b.type === 'tool_use' ? h('span.ide-ag-chip', null, `→ ${b.name ?? 'tool'}`) : null));
      return parts.length ? h('div.ide-ag-item.reply', null, parts) : null;
    }
    case 'tool_result': {
      const failed = d['is_error'] === true;
      const body = String(d['content'] ?? d['output'] ?? '');
      return h('details.ide-ag-result', null, h('summary', { class: failed ? 'danger' : 'ok' }, failed ? 'Error' : 'Output'), h('pre.ide-pre', null, clip(body, CLIP)));
    }
    case 'tool_decision':
      return d['decision'] === 'deny' ? h('div.ide-ag-item', null, badge(`denied: ${String(d['reason'] ?? '')}`, 'danger')) : null;
    case 'work_end': {
      const s = String(d['status'] ?? '');
      const tone = /fail|error/.test(s) ? 'danger' : /cancel|stop/.test(s) ? 'warn' : 'ok';
      return h('div.ide-ag-item', null, badge(s || 'done', tone));
    }
    case 'error':
      return h('div.ide-ag-item', null, h('p.ide-ag-error', null, String(d['message'] ?? 'Error')));
    default:
      return null;
  }
}

/** The Agent tab: the agent's recent transcript, followed live, and a steer box that posts a note to its next step. */
export function agentPanel(cid: number): Panel {
  let alive = true;
  let loaded = false;
  let lastSeq = 0;
  const early: Entry[] = [];
  const log = h('div.ide-ag-log', { 'aria-live': 'polite' }, empty('clock', 'Loading the transcript…'));
  const steerText = textarea({ rows: 3, placeholder: 'A note the agent reads before its next step', 'aria-label': 'Steer note' });
  const send = button('Send note', () => void steer(), { kind: 'primary', size: 'sm', icon: 'send', title: 'Send the note (Ctrl+Enter)' });
  steerText.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      void steer();
    }
  });
  const el = h('div.ide-agent', null, log, h('div.ide-steer', null, steerText, h('div.ide-steer-row', null, h('span.spacer'), send)));

  const add = (e: Entry): void => {
    const node = entryNode(e);
    if (!node) return;
    const stick = log.scrollHeight - log.scrollTop - log.clientHeight < 80;
    log.append(node);
    if (stick) log.scrollTop = log.scrollHeight;
  };

  async function load(): Promise<void> {
    try {
      const history = (await api.transcriptLast(cid, HISTORY)).slice().sort((a, b) => a.seq - b.seq);
      if (!alive) return;
      const items = history.map(entryNode).filter((n): n is HTMLElement => n !== null);
      lastSeq = history.length ? history[history.length - 1]!.seq : 0;
      replace(log, items.length ? items : empty('clock', 'No transcript yet'));
      loaded = true;
      for (const e of early.splice(0)) if (e.seq > lastSeq) {
        lastSeq = e.seq;
        add(e);
      }
      log.scrollTop = log.scrollHeight;
    } catch (err) {
      if (alive) replace(log, empty('alert', 'The transcript did not load', message(err)));
    }
  }

  async function steer(): Promise<void> {
    const text = steerText.value.trim();
    if (!text) {
      toast('Write a note first', 'error');
      return;
    }
    if (text.length > STEER_MAX) {
      toast(`A note is at most ${STEER_MAX} characters`, 'error');
      return;
    }
    send.disabled = true;
    const sent = await attempt(() => api.agentControl(cid, { action: 'steer', text }).then(() => true), 'Note sent to the agent');
    if (!alive) return;
    send.disabled = false;
    if (sent) steerText.value = '';
  }

  const off = on('transcript', (raw) => {
    const e = raw as Entry;
    if (!alive || e.cid !== cid) return;
    if (!loaded) {
      early.push(e);
      return;
    }
    if (e.seq <= lastSeq) return;
    lastSeq = e.seq;
    add(e);
  });
  void load();
  return {
    el,
    dispose: () => {
      alive = false;
      off();
    },
  };
}

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
    out.push(h('div.ide-line', { class: r.kind }, h('span.ln'), h('span.ln'), h('span.sign', null, SIGN[r.kind]), h('span.code', null, r.text)));
  });
  flush();
  return h('div.ide-lines', null, out);
}

/** The Approvals tab: the agent's held tool calls in this workspace, each with its input or its proposed change. */
export function approvalsPanel(opts: { wsId: string; cid: number; onCount(n: number): void; onDecided(): void }): Panel {
  const { wsId, cid } = opts;
  let alive = true;
  let generation = 0;
  const list = h('div.ide-appr-list');
  const el = h('div.ide-appr-panel', null, list);

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
    const path = typeof args['path'] === 'string' ? args['path'] : null;
    const body = h('div.ide-appr-body');
    const reason = textarea({ rows: 2, placeholder: 'Reason (optional)', 'aria-label': 'Reason' });
    const approve = button('Approve', () => void decide(true), { kind: 'primary', size: 'sm', icon: 'check' });
    const deny = button('Deny', () => void decide(false), { kind: 'danger', size: 'sm', icon: 'x' });
    const node = h(
      'article.ide-appr',
      null,
      h('header.ide-appr-head', null, badge(a.tool, 'info'), path !== null ? h('code', null, path) : null, h('span.ide-totals', null, ago(a.created_ms))),
      body,
      reason,
      h('div.ide-appr-actions', null, deny, approve),
    );
    if (WRITES.has(a.tool) && path !== null) void showChange(body, path, a.tool, args);
    else replace(body, h('pre.ide-pre', null, JSON.stringify(a.input, null, 2)));

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

  /** The proposed change against the file as it is now: a write's content, or an edit's old and new text. */
  async function showChange(host: HTMLElement, path: string, tool: string, args: Record<string, unknown>): Promise<void> {
    replace(host, h('p.ide-note', null, 'Reading the current file…'));
    let before: string;
    try {
      before = (await ideApi.file(wsId, path)).text;
    } catch (err) {
      // A write may create the file: a missing file is an empty one for a write only.
      if (tool !== 'write_file' || !(err instanceof ApiError && err.status === 404)) {
        if (alive) replace(host, h('p.ide-note.error', null, `The current file did not load: ${message(err)}`), h('pre.ide-pre', null, JSON.stringify(args, null, 2)));
        return;
      }
      before = '';
    }
    if (!alive) return;
    let pair: [string, string] | null;
    let note = '';
    if (tool === 'write_file') {
      pair = typeof args['content'] === 'string' ? [before, args['content']] : null;
      if (!pair) note = 'The write carries no content.';
    } else {
      const oldText = typeof args['old'] === 'string' ? args['old'] : '';
      const newText = typeof args['new'] === 'string' ? args['new'] : '';
      const hits = oldText === '' ? 0 : before.split(oldText).length - 1;
      if (hits > 0) {
        pair = [before, before.replace(oldText, newText)];
        if (hits > 1) note = `The old text appears ${hits} times; the diff shows the first.`;
      } else {
        // The edit cannot apply to this file now: show the text it would change, not the file.
        pair = [oldText, newText];
        note = 'The old text is not in the current file, so this edit would fail.';
      }
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

/** The Run tab: one bounded command in an agent or station workspace, its output polled from the hub, and a stop. */
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
  const status = h('p.ide-run-status', { 'aria-live': 'polite' }, enabled ? 'Not run yet.' : 'Runs are for agent and workstation workspaces. Repository runs come in phase 3.');
  const out = h('pre.ide-run-out', { 'aria-label': 'Output' });
  const el = h(
    'div.ide-run',
    null,
    h('div.ide-run-form', null, command, seconds, go, stop),
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

/** The Terminal tab until phase 5: a placeholder, no controller. */
export function terminalPanel(): HTMLElement {
  return h('div.ide-term', null, empty('terminal', 'Terminal comes in the next phase', 'A shell in the agent VM, with resize and reconnect, lands with the terminal service.'));
}
