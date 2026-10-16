// The IDE's Chat view: one agent's transcript as a reading column, and the composer that steers it.
// The transcript is reloaded (debounced) when the plaza reports a transcript or work frame for this agent.

import css from '../styles/ide-chat.css';
import { api, type Entry } from '../api';
import {
  buildChat,
  formatMs,
  stepLabel,
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
import { attempt, button, toast } from '../ui/components';
import type { EmbedContext } from '../ui/embeds';
import { h, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
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

const STYLE_ID = 'apz-ide-chat-style';
const HISTORY_LIMIT = 400;
const RELOAD_MS = 400;
/** Within this many pixels of the bottom, the transcript keeps following new entries. */
const NEAR_BOTTOM_PX = 48;
/** The composer grows to about eight lines, then scrolls. */
const INPUT_MAX_PX = 200;

const OUTCOME_WORD: Record<'ok' | 'failed' | 'running' | 'mixed', string> = {
  ok: 'done',
  failed: 'failed',
  running: 'running',
  mixed: 'mixed',
};

type EngineBlock = Extract<ChatItem, { kind: 'engine' }>;

const ENGINE_WORD: Record<EngineStatus, string> = { running: 'running', ok: 'done', failed: 'failed', denied: 'denied' };
const TOOL_WORD: Record<ToolStatus, string> = { pending: 'pending', in_progress: 'running', completed: 'done', failed: 'failed' };
const PLAN_MARK: Record<PlanEntry['status'], string> = { pending: '○', in_progress: '◐', completed: '✓' };
/** One glyph per ACP tool kind; a kind not listed gets the bullet. */
const KIND_GLYPH: Record<string, string> = { read: '◫', edit: '✎', delete: '✕', move: '↦', search: '⌕', execute: '▸', think: '…', fetch: '↓', switch_mode: '⇄' };
/** A diff or a text result shows this many lines in its block; the rest is counted. */
const PREVIEW_LINES = 20;

interface PreviewLine { cls: 'add' | 'del' | ''; text: string }

function splitLines(text: string, cls: PreviewLine['cls']): PreviewLine[] {
  return text ? text.split('\n').map((t) => ({ cls, text: t })) : [];
}

/** A tool call's content: a diff as its removed then its added lines, text as plain lines, a terminal by its id. */
function contentBlock(c: EngineContent): HTMLElement {
  if (c.type === 'terminal') return h('div.ide-chat-engine-more', null, `Terminal ${c.terminalId}`);
  const lines = c.type === 'diff' ? [...splitLines(c.oldText, 'del'), ...splitLines(c.newText, 'add')] : splitLines(c.text, '');
  const shown = lines.slice(0, PREVIEW_LINES);
  const more = lines.length - shown.length;
  return h(
    'div.ide-chat-engine-body',
    null,
    c.type === 'diff' ? h('div.ide-chat-engine-diff-path', null, c.path) : null,
    h(
      'div.ide-chat-engine-pre',
      null,
      shown.map((l) => h('div', { class: l.cls ? `ide-chat-engine-line ${l.cls}` : 'ide-chat-engine-line' }, l.text || ' ')),
    ),
    more > 0 ? h('div.ide-chat-engine-more', null, `${more} more lines`) : null,
  );
}

/** One tool call: its kind glyph, title and status, its places as links, and its content. */
function toolRow(tool: EngineTool, embeds: EmbedContext): HTMLElement {
  const places = tool.locations.map((loc) => {
    const label = loc.line !== null ? `${loc.path}:${loc.line}` : loc.path;
    if (!embeds.openPath) return h('span.ide-chat-engine-loc', null, label);
    return button(label, () => embeds.openPath?.(loc.path, loc.line), { kind: 'ghost', size: 'sm' });
  });
  return h(
    'li.ide-chat-engine-tool',
    { 'data-status': tool.status },
    h(
      'div.ide-chat-engine-tool-head',
      null,
      h('span.ide-chat-engine-glyph', { 'aria-hidden': 'true' }, KIND_GLYPH[tool.kind] ?? '•'),
      h('span.ide-chat-engine-tool-title', null, tool.title),
      h('span.ide-chat-engine-tool-status', null, TOOL_WORD[tool.status]),
    ),
    places.length ? h('div.ide-chat-engine-locs', null, places) : null,
    tool.content.map((c) => contentBlock(c)),
  );
}

/** A check the engine asked for; while held, the owner's Approve and Deny decide its approval. */
function checkRow(check: EngineCheck): HTMLElement {
  const word = check.decision === 'allow' ? 'allowed' : check.decision === 'deny' ? 'denied' : 'waiting';
  const id = check.approval;
  return h(
    'li.ide-chat-engine-check',
    { 'data-decision': check.decision },
    h('span.ide-chat-engine-tool-title', null, check.title),
    h('span.ide-chat-engine-check-name', null, check.name),
    h('span.ide-chat-engine-tool-status', null, word),
    check.decision === 'pending' && id
      ? h(
          'div.ide-chat-engine-decide',
          null,
          button('Deny', () => void attempt(() => api.decideApproval(id, false), 'Denied'), { kind: 'danger' }),
          button('Approve', () => void attempt(() => api.decideApproval(id, true), 'Approved'), { kind: 'primary' }),
        )
      : null,
  );
}

function injectStyle(): void {
  if (document.getElementById(STYLE_ID)) return;
  const style = document.createElement('style');
  style.id = STYLE_ID;
  style.textContent = css;
  document.head.append(style);
}

/** A chat pane for one agent: its transcript, and a composer that sends steering text to it. */
export function chatPane(opts: ChatPaneOpts): ChatPane {
  injectStyle();

  let alive = true;
  let entries: Entry[] = [];
  let timer: ReturnType<typeof setTimeout> | undefined;
  let loadSeq = 0;
  let pinned = true;
  let sending = false;
  let stopping = false;
  /** Which steps disclosures the owner has opened, keyed by item position, so a reload keeps them open. */
  const openSteps = new Set<string>();
  /** The engine text that streamed in and is not yet in the transcript, per code_engine call. */
  const liveText = new Map<string, string>();
  /** The live text node of each running engine block in the current render, per call. */
  const liveEls = new Map<string, HTMLElement>();

  const column = h('div.ide-chat-column');
  const log = h('div.ide-chat-log', { role: 'log', 'aria-live': 'polite', 'aria-label': `Conversation with ${opts.agentName}` }, column);

  const onScroll = (): void => {
    pinned = log.scrollHeight - log.scrollTop - log.clientHeight < NEAR_BOTTOM_PX;
  };
  log.addEventListener('scroll', onScroll, { passive: true });

  // The composer.
  const input = h('textarea.ide-chat-input', { rows: 2, autocomplete: 'off', spellcheck: true });
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
  // Image paste is off: the hub has no owner upload route to store a pasted image in, so the image is not taken. Text pastes.
  input.addEventListener('paste', (ev) => {
    const images = [...(ev.clipboardData?.files ?? [])].some((f) => f.type.startsWith('image/'));
    if (!images) return;
    ev.preventDefault();
    toast('Pasting images is not available yet: the hub has no upload route for them.', 'error');
  });

  const sendBtn = button('Send', () => void send(), { kind: 'primary' });
  const stopBtn = button('Stop', () => void stop(), { kind: 'danger' });
  const actions = h('div.ide-chat-actions');

  const composer = h(
    'div.ide-chat-composer',
    null,
    h(
      'div.ide-chat-composer-inner',
      null,
      h('div.ide-chat-target', null, `Sending to: ${opts.agentName} on ${opts.project ?? 'no project'}`),
      h(
        'div.ide-chat-input-row',
        null,
        h('label.ide-chat-field', null, h('span.ide-chat-sr', null, `Message to ${opts.agentName}`), input),
        actions,
      ),
    ),
  );

  const el = h('div.ide-chat', null, log, composer);

  function syncComposer(): void {
    const working = opts.working();
    replace(actions, working ? stopBtn : sendBtn);
    sendBtn.disabled = sending;
    stopBtn.disabled = stopping;
  }

  async function send(): Promise<void> {
    const text = input.value.trim();
    if (!text || sending) return;
    sending = true;
    syncComposer();
    try {
      await api.agentControl(opts.cid, { action: 'steer', text });
      if (!alive) return;
      input.value = '';
      resize();
      pinned = true;
      schedule();
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

  function stepsBlock(item: Extract<ChatItem, { kind: 'steps' }>, key: string): HTMLElement {
    const count = item.steps.length;
    const d = h('details.ide-chat-steps');
    d.open = openSteps.has(key);
    d.addEventListener('toggle', () => {
      if (d.open) openSteps.add(key);
      else openSteps.delete(key);
    });
    d.append(
      h(
        'summary',
        null,
        h('span.ide-chat-steps-label', null, `${count} ${count === 1 ? 'step' : 'steps'} · ${item.last}`),
        h('span.ide-chat-outcome', { class: item.outcome }, OUTCOME_WORD[item.outcome]),
      ),
      h(
        'ol.ide-chat-step-list',
        null,
        item.steps.map((step: Step) =>
          h(
            'li.ide-chat-step',
            null,
            h('span', null, stepLabel(step)),
            h('span.ide-chat-step-meta', null, step.ms != null ? formatMs(step.ms) : null, ` ${step.outcome}`),
          ),
        ),
      ),
    );
    return d;
  }

  /** A coding engine's run under its code_engine call: header, message, tools, plan, checks and summary. */
  function engineBlock(item: EngineBlock, embeds: EmbedContext): HTMLElement {
    const live = h('div.ide-chat-engine-live');
    if (item.status === 'running') {
      liveEls.set(item.call, live);
      live.textContent = liveText.get(item.call) ?? '';
    }
    const head = h(
      'div.ide-chat-engine-head',
      null,
      h('span.ide-chat-engine-badge', null, item.engine),
      item.session ? h('span.ide-chat-engine-session', { title: item.session }, item.session.slice(0, 8)) : null,
      h('span.ide-chat-engine-status', { 'data-status': item.status }, ENGINE_WORD[item.status]),
    );
    const section = (label: string, body: HTMLElement): HTMLElement =>
      h('section.ide-chat-engine-section', null, h('div.ide-chat-engine-label', null, label), body);
    return h(
      'section.ide-chat-engine',
      { 'data-status': item.status, 'aria-label': `${item.engine} run` },
      head,
      item.prompt ? h('div.ide-chat-engine-prompt', { title: item.prompt }, item.prompt) : null,
      item.message ? h('div.ide-chat-engine-message', null, renderMarkdown(item.message, { embeds })) : null,
      live,
      item.tools.length ? section('Tools', h('ul.ide-chat-engine-tools', null, item.tools.map((t) => toolRow(t, embeds)))) : null,
      item.plan.length
        ? section(
            'Plan',
            h(
              'ul.ide-chat-engine-plan',
              null,
              item.plan.map((p) =>
                h('li', { 'data-status': p.status }, h('span.ide-chat-engine-mark', { 'aria-hidden': 'true' }, PLAN_MARK[p.status]), h('span', null, p.content)),
              ),
            ),
          )
        : null,
      item.checks.length ? section('Checks', h('ul.ide-chat-engine-checks', null, item.checks.map((c) => checkRow(c)))) : null,
      item.summary
        ? h(
            'details.ide-chat-engine-summary',
            null,
            h('summary', null, 'Summary'),
            h('div.ide-chat-engine-summary-body', null, renderMarkdown(item.summary, { embeds })),
          )
        : null,
    );
  }

  function renderItem(item: ChatItem, index: number, embeds: EmbedContext): HTMLElement {
    switch (item.kind) {
      case 'owner':
        return h('div.ide-chat-msg.owner', null, h('div.ide-chat-bubble', null, item.text));
      case 'agent':
        return h('div.ide-chat-agent', null, renderMarkdown(item.text, { embeds }));
      case 'steps':
        return stepsBlock(item, `${item.at}:${index}`);
      case 'engine':
        return engineBlock(item, embeds);
      case 'working':
        return h(
          'div.ide-chat-working',
          null,
          h('span.ide-chat-dots', { 'aria-hidden': 'true' }, h('span'), h('span'), h('span')),
          h('span', null, 'Working...'),
        );
      case 'system':
        return h('div.ide-chat-system', null, item.text);
    }
  }

  function render(): void {
    const items = buildChat(entries, opts.working());
    const embeds = opts.embeds();
    // A finished engine's live text is in the transcript now; the live nodes belong to this render.
    for (const item of items) if (item.kind === 'engine' && item.status !== 'running') liveText.delete(item.call);
    liveEls.clear();
    replace(
      column,
      items.length ? items.map((item, index) => renderItem(item, index, embeds)) : h('div.ide-chat-empty', null, `No messages with ${opts.agentName} yet.`),
    );
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
  }

  void load(true);
  syncComposer();

  return {
    el,
    focusComposer: () => input.focus(),
    refresh: () => void load(true),
    dispose,
  };
}
