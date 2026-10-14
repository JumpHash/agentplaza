// The IDE's Chat view: one agent's transcript as a reading column, and the composer that steers it.
// The transcript is reloaded (debounced) when the plaza reports a transcript or work frame for this agent.

import css from '../styles/ide-chat.css';
import { api, type Entry } from '../api';
import { buildChat, formatMs, stepLabel, type ChatItem, type Step } from '../ide/view';
import { on } from '../store';
import { attempt, button, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';

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
  input.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter' && !ev.shiftKey && !ev.isComposing) {
      ev.preventDefault();
      void send();
    }
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

  function renderItem(item: ChatItem, index: number): HTMLElement {
    switch (item.kind) {
      case 'owner':
        return h('div.ide-chat-msg.owner', null, h('div.ide-chat-bubble', null, item.text));
      case 'agent':
        return h('div.ide-chat-agent', null, renderMarkdown(item.text));
      case 'steps':
        return stepsBlock(item, `${item.at}:${index}`);
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
    replace(
      column,
      items.length ? items.map((item, index) => renderItem(item, index)) : h('div.ide-chat-empty', null, `No messages with ${opts.agentName} yet.`),
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
  const offs = [on('transcript', onFrame), on('work', onFrame)];

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
