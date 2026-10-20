// The IDE's Chat view: one agent's transcript as a reading column, and the composer that talks to it: a chat turn when
// it is idle, a note that steers the work in flight when it is working. The markup is the prototype's workspace
// (loom's m-workspace): the steps disclosure, the tool rows, the composer with its target line and send button.
// The transcript is reloaded (debounced) when the plaza reports a transcript or work frame for this agent.

import { api, type Entry } from '../api';
import {
  buildChat,
  formatMs,
  type ChatItem,
  type EngineCheck,
  type EngineContent,
  type EngineStatus,
  type EngineTool,
  type PlanEntry,
  type Step,
  type ToolStatus,
} from '../ide/view';
import { on } from '../store';
import { attempt, badge, button, toast } from '../ui/components';
import type { EmbedContext } from '../ui/embeds';
import { h, lu, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import { attachBar, maySend } from '../ui/attach-bar';
import { contextBar, followAgentModel, modelChip } from '../ui/chat-tools';
import { contextApi } from '../api-context';
import { prewake } from '../prewake';

export interface ChatPane {
  el: HTMLElement;
  focusComposer(): void;
  refresh(): void;
  dispose(): void;
}

export interface ChatPaneOpts {
  cid: number;
  agentName: string;
  project: string | null;
  working: () => boolean;
  /** The rich-content context for the agent's text, read at each render (the IDE's files and handlers). */
  embeds: () => EmbedContext;
}

type LucideName = Parameters<typeof lu>[0];
type EngineBlock = Extract<ChatItem, { kind: 'engine' }>;
type StepsBlock = Extract<ChatItem, { kind: 'steps' }>;

const HISTORY_LIMIT = 400;
const RELOAD_MS = 400;
/** Within this many pixels of the bottom, the transcript keeps following new entries. */
const NEAR_BOTTOM_PX = 48;
/** The composer grows to about eight lines, then scrolls. */
const INPUT_MAX_PX = 200;

const OUTCOME_WORD: Record<StepsBlock['outcome'], string> = { ok: 'done', failed: 'failed', running: 'running', mixed: 'mixed' };
const STEP_WORD: Record<Step['outcome'], string> = { ok: 'done', failed: 'failed', denied: 'denied', running: 'running' };
const ENGINE_WORD: Record<EngineStatus, string> = { running: 'running', ok: 'done', failed: 'failed', denied: 'denied' };
const TOOL_WORD: Record<ToolStatus, string> = { pending: 'pending', in_progress: 'running', completed: 'done', failed: 'failed' };
const PLAN_MARK: Record<PlanEntry['status'], string> = { pending: '○', in_progress: '◐', completed: '✓' };
/** One glyph per ACP tool kind; a kind not listed gets the bullet. */
const KIND_GLYPH: Record<string, string> = { read: '◫', edit: '✎', delete: '✕', move: '↦', search: '⌕', execute: '▸', think: '…', fetch: '↓', switch_mode: '⇄' };
/** A step's icon and verb, by the tool's name (the first rule that matches). */
const STEP_VERB: [RegExp, LucideName, string][] = [
  [/^(read|view|open)/, 'file-text', 'Read'],
  [/(edit|patch|replace)/, 'pencil', 'Edited'],
  [/(write|create)/, 'file-plus', 'Wrote'],
  [/(bash|shell|command|exec|run)/, 'terminal', 'Ran'],
  [/(grep|search|glob|find)/, 'search', 'Searched'],
];
/** A diff or a text result shows this many lines in its block; the rest is counted. */
const PREVIEW_LINES = 20;

interface PreviewLine { cls: 'add' | 'del' | ''; text: string }

function splitLines(text: string, cls: PreviewLine['cls']): PreviewLine[] {
  return text ? text.split('\n').map((t) => ({ cls, text: t })) : [];
}

/** A step's icon and verb: the tool's kind when its name says one, else the name itself. */
function stepVerb(step: Step): [LucideName, string] {
  const n = step.name.toLowerCase();
  const hit = STEP_VERB.find(([re]) => re.test(n));
  return hit ? [hit[1], hit[2]] : ['circle', step.name];
}

/** A tool call's content: a diff as its removed then its added lines, text as plain lines, a terminal by its id. */
function contentBlock(c: EngineContent): HTMLElement {
  if (c.type === 'terminal') return h('div.ic-muted', null, `Terminal ${c.terminalId}`);
  if (c.type === 'diff') {
    const lines = [...splitLines(c.oldText, 'del'), ...splitLines(c.newText, 'add')];
    const shown = lines.slice(0, PREVIEW_LINES);
    const more = lines.length - shown.length;
    return h(
      'div.diff',
      null,
      h(
        'div.dfile',
        null,
        h('div.dfile-h', null, c.path),
        shown.map((l) => h('div', { class: l.cls ? `dl ${l.cls}` : 'dl' }, l.text || ' ')),
        more > 0 ? h('div.ic-muted', null, `${more} more lines`) : null,
      ),
    );
  }
  const lines = splitLines(c.text, '');
  const shown = lines.slice(0, PREVIEW_LINES);
  const more = lines.length - shown.length;
  return h(
    'div',
    null,
    shown.map((l) => h('div.cl', null, l.text || ' ')),
    more > 0 ? h('div.ic-muted', null, `${more} more lines`) : null,
  );
}

/** One tool call: its kind glyph, title and status, its places as links, and its content. */
function toolRow(tool: EngineTool, embeds: EmbedContext): HTMLElement {
  const places = tool.locations.map((loc) => {
    const label = loc.line !== null ? `${loc.path}:${loc.line}` : loc.path;
    if (!embeds.openPath) return h('span.ic-mono.ic-muted', null, label);
    return button(label, () => embeds.openPath?.(loc.path, loc.line), { kind: 'ghost', size: 'sm' });
  });
  return h(
    'li.ic-tool',
    { 'data-status': tool.status },
    h(
      'div.ic-tool-h',
      null,
      h('span.ic-glyph', { 'aria-hidden': 'true' }, KIND_GLYPH[tool.kind] ?? '•'),
      h('span.ic-tool-t', { title: tool.title }, tool.title),
      h('span.ic-muted', null, TOOL_WORD[tool.status]),
    ),
    places.length ? h('div.ic-chips', null, places) : null,
    tool.content.map((c) => contentBlock(c)),
  );
}

/** A check the engine asked for; while held, the owner's Approve and Deny decide its approval. */
function checkRow(check: EngineCheck): HTMLElement {
  const word = check.decision === 'allow' ? 'allowed' : check.decision === 'deny' ? 'denied' : 'waiting';
  const id = check.approval;
  return h(
    'li.ic-check',
    { 'data-decision': check.decision },
    h('span', null, check.title),
    h('span.ic-mono', null, check.name),
    h('span.ic-muted', null, word),
    check.decision === 'pending' && id
      ? h(
          'span.ic-ap',
          null,
          button('Deny', () => void attempt(() => api.decideApproval(id, false), 'Denied'), { kind: 'danger', size: 'sm' }),
          button('Approve', () => void attempt(() => api.decideApproval(id, true), 'Approved'), { kind: 'primary', size: 'sm' }),
        )
      : null,
  );
}

/** Each agent message and the engine runs are plain blocks in the column; the owner's words are a right-hand bubble. */
export function chatPane(opts: ChatPaneOpts): ChatPane {
  let alive = true;
  let entries: Entry[] = [];
  let timer: ReturnType<typeof setTimeout> | undefined;
  let loadSeq = 0;
  let pinned = true;
  let sending = false;
  let stopping = false;
  /** Which steps groups the owner has closed (they open by default), keyed by item position, so a reload keeps them. */
  const closedSteps = new Set<string>();
  /** The engine text that streamed in and is not yet in the transcript, per code_engine call. */
  const liveText = new Map<string, string>();
  /** The live text node of each running engine block in the current render, per call. */
  const liveEls = new Map<string, HTMLElement>();

  const column = h('div.col');
  const log = h('div.ws-log', { role: 'log', 'aria-live': 'polite', 'aria-label': `Conversation with ${opts.agentName}` }, column);

  const onScroll = (): void => {
    pinned = log.scrollHeight - log.scrollTop - log.clientHeight < NEAR_BOTTOM_PX;
  };
  log.addEventListener('scroll', onScroll, { passive: true });

  // The composer.
  const input = h('textarea', { rows: 2, autocomplete: 'off', spellcheck: true, 'aria-label': `Message to ${opts.agentName}` });
  const resize = (): void => {
    input.style.height = 'auto';
    input.style.height = `${Math.min(input.scrollHeight, INPUT_MAX_PX)}px`;
  };
  input.addEventListener('input', resize);
  input.addEventListener('input', () => prewake(opts.cid));
  input.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter' && !ev.shiftKey && !ev.isComposing) {
      ev.preventDefault();
      void send();
    }
  });
  // Files: picked, dropped on the pane or pasted into it. Each uploads as it is taken and goes with the next message.
  const bar = attachBar({ onChange: () => syncComposer() });
  // The attach button is the prototype's round pick button, not a text button.
  bar.button.className = 'pick';

  const sendBtn = h('button.send', { type: 'button', 'aria-label': 'Send', title: 'Send', onclick: () => void send() }, lu('arrow-up', 16, '', 2.4));
  const stopBtn = button('Stop', () => void stop(), { kind: 'danger', size: 'sm' });
  const actions = h('span.ws-actions');

  // The model chip sits in the tool row; the context line sits above the composer; a save error shows under it.
  const chip = modelChip({ label: 'Model', save: (value) => contextApi.setAgentModel(opts.cid, value || null) });
  const ctx = contextBar({ cid: () => opts.cid, source: (cid) => contextApi.agent(cid) });
  const unfollow = followAgentModel(chip, () => opts.cid);

  const composer = h(
    'div.composer-wrap',
    null,
    ctx.el,
    h(
      'div.composer',
      null,
      h('span.ws-sr', null, `Message to ${opts.agentName}`),
      input,
      bar.el,
      h(
        'div.cbar',
        null,
        h('span.ws-target', null, `Sending to: ${opts.agentName} on ${opts.project ?? 'no project'}`),
        h('span.sp'),
        chip.el,
        actions,
      ),
    ),
    chip.note,
  );

  const el = h('div.ws-main.ws-ide', null, log, composer);
  const unbind = bar.bind(el);

  function syncComposer(): void {
    const working = opts.working();
    replace(actions, bar.button, working ? stopBtn : sendBtn);
    sendBtn.disabled = sending || bar.busy() || bar.failed();
    stopBtn.disabled = stopping;
  }

  async function send(): Promise<void> {
    const text = input.value.trim();
    const ids = bar.ids();
    if (!maySend(text, ids, sending || bar.busy() || bar.failed())) return;
    sending = true;
    syncComposer();
    try {
      // Working, the text steers the work in flight; idle, it is a chat turn of its own (the hub says so when the agent
      // is on the bench, or its project is paused).
      if (opts.working()) await api.agentControl(opts.cid, { action: 'steer', text, attachments: ids });
      else await api.chat(opts.cid, text, ids);
      if (!alive) return;
      input.value = '';
      bar.clear();
      resize();
      pinned = true;
      schedule();
      ctx.refresh();
    } catch (err) {
      toast(err instanceof Error ? err.message : String(err), 'error');
    } finally {
      sending = false;
      if (alive) syncComposer();
    }
  }

  async function stop(): Promise<void> {
    if (stopping) return;
    stopping = true;
    syncComposer();
    try {
      await attempt(() => api.cancel(opts.cid), 'Stopped');
    } finally {
      stopping = false;
      if (alive) syncComposer();
    }
  }

  /** A run of tool calls: "N steps · the last one · outcome", open by default, with a row per step. */
  function stepsBlock(item: StepsBlock, key: string): HTMLElement {
    const count = item.steps.length;
    const open = !closedSteps.has(key);
    const label = `${count} ${count === 1 ? 'step' : 'steps'} · ${item.last} · ${OUTCOME_WORD[item.outcome]}`;
    const head = item.outcome === 'running' ? h('span.shimmer', null, label) : label;
    const toggle = h(
      'button.worked',
      {
        type: 'button',
        'aria-expanded': String(open),
        onclick: () => {
          if (closedSteps.has(key)) closedSteps.delete(key);
          else closedSteps.add(key);
          render();
        },
      },
      head,
      lu('chevron-right', 14),
    );
    if (!open) return toggle;
    const rows = h(
      'div.tools',
      null,
      item.steps.map((step: Step) => {
        const [icon, verb] = stepVerb(step);
        return h(
          'div.tool',
          null,
          lu(icon, 14),
          h('span.nm', null, verb),
          h('span.arg', null, step.detail),
          h('span.meta', null, step.ms != null ? formatMs(step.ms) : STEP_WORD[step.outcome]),
        );
      }),
    );
    return h('div', null, toggle, rows);
  }

  /** A coding engine's run under its code_engine call: header, message, tools, plan, checks and summary. */
  function engineBlock(item: EngineBlock, embeds: EmbedContext): HTMLElement {
    const live = h('div.ic-eng-live');
    if (item.status === 'running') {
      liveEls.set(item.call, live);
      live.textContent = liveText.get(item.call) ?? '';
    }
    const section = (label: string, body: HTMLElement): HTMLElement => h('div', null, h('div.ic-eng-l', null, label), body);
    return h(
      'section.ic-eng',
      { 'data-status': item.status, 'aria-label': `${item.engine} run` },
      h(
        'div.ic-eng-h',
        null,
        badge(item.engine, 'accent'),
        item.session ? h('span.ic-mono.ic-muted', { title: item.session }, item.session.slice(0, 8)) : null,
        h('span.ic-eng-st', { 'data-status': item.status }, ENGINE_WORD[item.status]),
      ),
      item.prompt ? h('div.ic-eng-prompt', { title: item.prompt }, item.prompt) : null,
      item.message ? h('div.ic-md', null, renderMarkdown(item.message, { embeds })) : null,
      live,
      item.tools.length ? section('Tools', h('ul.ic-tools', null, item.tools.map((t) => toolRow(t, embeds)))) : null,
      item.plan.length
        ? section(
            'Plan',
            h(
              'ul.ic-plan',
              null,
              item.plan.map((p) =>
                h('li', { 'data-status': p.status }, h('span.ic-mark', { 'aria-hidden': 'true' }, PLAN_MARK[p.status]), ' ', p.content),
              ),
            ),
          )
        : null,
      item.checks.length ? section('Checks', h('ul.ic-checks', null, item.checks.map((c) => checkRow(c)))) : null,
      item.summary
        ? h('details.ic-sum', null, h('summary', null, 'Summary'), h('div.ic-md', null, renderMarkdown(item.summary, { embeds })))
        : null,
    );
  }

  function renderItem(item: ChatItem, index: number, embeds: EmbedContext): HTMLElement {
    switch (item.kind) {
      case 'owner':
        return h('div.msg-owner', null, h('div.ws-owner-text', null, item.text));
      case 'agent':
        return h('div.ws-md', null, renderMarkdown(item.text, { embeds }));
      case 'steps':
        return stepsBlock(item, `${item.at}:${index}`);
      case 'engine':
        return engineBlock(item, embeds);
      case 'working':
        return h('p.ws-working', null, 'Working', h('span.ws-dots', { 'aria-hidden': 'true' }, h('i'), h('i'), h('i')));
      case 'system':
        return h('div.notice', null, lu('info', 14), h('span', null, item.text));
    }
  }

  function render(): void {
    const items = buildChat(entries, opts.working());
    const embeds = opts.embeds();
    // A finished engine's live text is in the transcript now; the live nodes belong to this render.
    for (const item of items) if (item.kind === 'engine' && item.status !== 'running') liveText.delete(item.call);
    liveEls.clear();
    replace(column, items.length ? items.map((item, index) => renderItem(item, index, embeds)) : h('p.muted', null, `No messages with ${opts.agentName} yet.`));
    syncComposer();
    if (pinned) log.scrollTop = log.scrollHeight;
  }

  async function load(report: boolean): Promise<void> {
    const seq = ++loadSeq;
    try {
      const rows = await api.transcriptLast(opts.cid, HISTORY_LIMIT);
      if (!alive || seq !== loadSeq) return;
      entries = rows.slice().sort((a, b) => a.seq - b.seq);
      render();
    } catch (err) {
      if (report && alive) toast(err instanceof Error ? err.message : String(err), 'error');
    }
  }

  // Frames arrive in bursts while an agent works; one reload per window covers them all.
  function schedule(): void {
    if (timer !== undefined) return;
    timer = setTimeout(() => {
      timer = undefined;
      if (alive) void load(false);
    }, RELOAD_MS);
  }

  const onFrame = (raw: unknown): void => {
    if ((raw as { cid?: unknown } | null)?.cid !== opts.cid) return;
    syncComposer();
    schedule();
  };
  // A coding engine's text streams as `engine` frames ahead of the transcript. It shows in its block until the
  // transcript holds it: a tool call or a mode change flushes the message so far, which the transcript then carries.
  const onEngine = (raw: unknown): void => {
    const f = raw as { cid?: unknown; call?: unknown; update?: unknown } | null;
    if (f?.cid !== opts.cid || typeof f.call !== 'string') return;
    const call = f.call;
    const update = (f.update && typeof f.update === 'object' ? f.update : {}) as Record<string, unknown>;
    if (update.sessionUpdate === 'agent_message_chunk') {
      const text = (update.content as { text?: unknown } | undefined)?.text;
      if (typeof text !== 'string') return;
      liveText.set(call, (liveText.get(call) ?? '') + text);
    } else if (
      update.sessionUpdate === 'tool_call' ||
      update.sessionUpdate === 'current_mode_update' ||
      (update.sessionUpdate === 'tool_call_update' && 'status' in update)
    ) {
      liveText.delete(call);
    } else {
      return;
    }
    const el = liveEls.get(call);
    if (!el) return;
    el.textContent = liveText.get(call) ?? '';
    if (pinned) log.scrollTop = log.scrollHeight;
  };
  const offs = [on('transcript', onFrame), on('work', onFrame), on('engine', onEngine)];

  function dispose(): void {
    alive = false;
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
    for (const off of offs) off();
    log.removeEventListener('scroll', onScroll);
    unbind();
    ctx.dispose();
    chip.dispose();
    unfollow();
  }

  void load(true);
  ctx.refresh();
  syncComposer();

  return {
    el,
    focusComposer: () => input.focus(),
    refresh: () => void load(true),
    dispose,
  };
}
