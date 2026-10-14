// The Activity tab of the IDE right column: time split per agent and the tool-call list.
import { api, type Entry } from '../api';
import { ago } from '../ui/components';
import { h, replace } from '../ui/dom';
import { activitySplit, formatMs } from '../ide/view';
import { on } from '../store';
import css from '../styles/ide-panels.css';

const STYLE_ID = 'apz-ide-panels-style';
const LOAD_LAST = 1000;
const MAX_ROWS = 200;
const DEBOUNCE_MS = 400;
const REFRESH_MS = 10_000;

export interface ActivityPanel {
  el: HTMLElement;
  show(): void;
  dispose(): void;
}

type Split = ReturnType<typeof activitySplit>;
type Call = Split['calls'][number];

const OUTCOME: Record<Call['outcome'], string> = {
  ok: 'ok',
  failed: 'failed',
  denied: 'denied',
  running: 'running',
};

function ensureStyle(): void {
  if (document.getElementById(STYLE_ID)) return;
  const style = document.createElement('style');
  style.id = STYLE_ID;
  style.textContent = css;
  document.head.appendChild(style);
}

function pct(part: number, total: number): string {
  return `${Math.round((part / total) * 100)}%`;
}

function seg(cls: string, share: number): HTMLElement {
  const el = h('span.ipa-seg');
  el.classList.add(cls);
  el.style.width = `${(Math.max(0, Math.min(1, share)) * 100).toFixed(2)}%`;
  return el;
}

function key(cls: string, text: string): HTMLElement {
  const swatch = h('i.ipa-swatch', { 'aria-hidden': 'true' });
  swatch.classList.add(cls);
  return h('li.ipa-key', null, swatch, text);
}

function callRow(c: Call): HTMLElement {
  return h('li.ipa-row', { 'data-outcome': c.outcome },
    h('span.ipa-name', null, c.name),
    h('span.ipa-ms', null, c.ms == null ? 'running' : formatMs(c.ms)),
    h('span.ipa-outcome', null, OUTCOME[c.outcome]),
    h('span.ipa-ago', null, ago(c.at)));
}

export function activityPanel(opts: { cid: number; agentName: string }): ActivityPanel {
  ensureStyle();
  const { cid, agentName } = opts;
  const summaryEl = h('div.ipa-summary');
  const scroller = h('div.ipa-scroll', { tabindex: 0, 'aria-label': 'Tool calls' });
  const el = h('section.ipa', { 'aria-label': 'Activity' },
    h('header.ipa-head', null, h('span.ipa-title', null, 'Activity'), h('span.ipa-agent', null, agentName)),
    summaryEl,
    scroller);
  replace(summaryEl, h('p.ipa-note', null, 'Loading activity'));

  let entries: Entry[] = [];
  let error: string | null = null;
  let loaded = false;
  let inflight = false;
  let queued = false;
  let disposed = false;
  let debounce: ReturnType<typeof setTimeout> | null = null;
  let timer: ReturnType<typeof setInterval> | null = null;

  function render(): void {
    const split = activitySplit(entries);
    const total = split.totalMs;

    if (error !== null) {
      replace(summaryEl, h('p.ipa-note', null, error));
    } else if (total === 0) {
      replace(summaryEl, h('p.ipa-note', null, 'No activity yet'));
    } else {
      const other = Math.max(0, total - split.thinkingMs - split.toolMs);
      const bar = h('div.ipa-bar', { 'aria-hidden': 'true' },
        seg('ipa-think', split.thinkingMs / total),
        seg('ipa-tool', split.toolMs / total),
        seg('ipa-other', other / total));
      const keys = h('ul.ipa-keys', null,
        key('ipa-think', `Thinking ${formatMs(split.thinkingMs)} · ${pct(split.thinkingMs, total)}`),
        key('ipa-tool', `Tools ${formatMs(split.toolMs)} · ${pct(split.toolMs, total)}`),
        key('ipa-other', `Other ${formatMs(other)} · ${pct(other, total)}`));
      replace(summaryEl, bar, keys);
    }

    // Keep the reader's place in the list; only the newest rows are shown.
    const calls = error === null ? split.calls : [];
    const capped = calls.length > MAX_ROWS;
    const shown = capped ? calls.slice(-MAX_ROWS) : calls;
    const top = scroller.scrollTop;
    replace(scroller,
      capped ? h('p.ipa-note', null, `Showing the newest ${MAX_ROWS} of ${calls.length} calls.`) : null,
      shown.length > 0
        ? h('ol.ipa-list', null, shown.map(callRow))
        : h('p.ipa-note', null, 'No tool calls yet'));
    scroller.scrollTop = top;
  }

  async function load(): Promise<void> {
    if (disposed) return;
    if (inflight) {
      queued = true;
      return;
    }
    inflight = true;
    try {
      entries = await api.transcriptLast(cid, LOAD_LAST);
      error = null;
    } catch {
      error = 'Could not load activity.';
    } finally {
      inflight = false;
    }
    if (disposed) return;
    render();
    if (queued) {
      queued = false;
      void load();
    }
  }

  // Store frames carry the agent's cid; only this agent's frames refresh the panel.
  function onFrame(data: unknown): void {
    if (disposed || !loaded) return;
    if ((data as { cid?: unknown } | null)?.cid !== cid) return;
    if (debounce !== null) clearTimeout(debounce);
    debounce = setTimeout(() => {
      debounce = null;
      void load();
    }, DEBOUNCE_MS);
  }

  const offs = [on('transcript', onFrame), on('work', onFrame)];

  function show(): void {
    if (disposed) return;
    if (!loaded) {
      loaded = true;
      void load();
    }
    if (timer === null) {
      timer = setInterval(() => {
        if (!document.hidden) void load();
      }, REFRESH_MS);
    }
  }

  function dispose(): void {
    disposed = true;
    for (const off of offs) off();
    if (debounce !== null) clearTimeout(debounce);
    if (timer !== null) clearInterval(timer);
    debounce = null;
    timer = null;
  }

  return { el, show, dispose };
}
