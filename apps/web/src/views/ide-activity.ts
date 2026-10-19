// The Activity tab of the IDE right column: time split per agent and the tool-call list. The markup is the prototype's
// Activity panel (m-ide-more.js; styles/proto/ide-more.css).
import moreCss from '../styles/proto/ide-more.css';
import { api, type Entry } from '../api';
import { ago } from '../ui/components';
import { h, replace } from '../ui/dom';
import { activitySplit, formatMs } from '../ide/view';
import { on } from '../store';

/** The prototype's IDE rules (styles/proto/ide-more.css), inserted once per page. */
const MORE_STYLE_ID = 'apz-ide-more-style';
function ensureMoreStyle(): void {
  if (document.getElementById(MORE_STYLE_ID)) return;
  const style = document.createElement('style');
  style.id = MORE_STYLE_ID;
  style.textContent = moreCss;
  document.head.append(style);
}

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

function pct(part: number, total: number): string {
  return `${Math.round((part / total) * 100)}%`;
}

function seg(cls: string, share: number): HTMLElement {
  const el = h(`span.${cls}`);
  el.style.width = `${(Math.max(0, Math.min(1, share)) * 100).toFixed(2)}%`;
  return el;
}

function callRow(c: Call): HTMLElement {
  return h('div.idem-call', { 'data-outcome': c.outcome },
    h('span.idem-call-name', { title: c.name }, c.name),
    h('span.num', null, c.ms == null ? 'running' : formatMs(c.ms)),
    h('span.muted', null, OUTCOME[c.outcome]),
    h('span.muted', null, ago(c.at)));
}

export function activityPanel(opts: { cid: number; agentName: string }): ActivityPanel {
  ensureMoreStyle();
  const { cid, agentName } = opts;
  const summaryEl = h('div.idem-summary');
  const capEl = h('div.muted');
  capEl.hidden = true;
  const scroller = h('div.idem-list', { tabindex: 0, 'aria-label': 'Tool calls' });
  const el = h('div.idem-panel', { 'aria-label': 'Activity' },
    h('div.idem-panel-h', null, h('span.idem-h', null, 'Activity'), h('span.muted', null, agentName)),
    summaryEl,
    capEl,
    scroller);
  replace(summaryEl, h('div.muted', null, 'Loading activity'));

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
      replace(summaryEl, h('div.muted', null, error));
    } else if (total === 0) {
      replace(summaryEl, h('div.muted', null, 'No activity yet'));
    } else {
      const other = Math.max(0, total - split.thinkingMs - split.toolMs);
      const bar = h('div.idem-activity-bar', { 'aria-hidden': 'true' },
        seg('idem-seg-t', split.thinkingMs / total),
        seg('idem-seg-h', split.toolMs / total),
        seg('idem-seg-o', other / total));
      const keys = h('div.idem-keys', null,
        h('span', null, `Thinking ${formatMs(split.thinkingMs)} · ${pct(split.thinkingMs, total)}`),
        h('span', null, `Tools ${formatMs(split.toolMs)} · ${pct(split.toolMs, total)}`),
        h('span', null, `Other ${formatMs(other)} · ${pct(other, total)}`));
      replace(summaryEl, bar, keys);
    }

    // Keep the reader's place in the list; only the newest rows are shown.
    const calls = error === null ? split.calls : [];
    const capped = calls.length > MAX_ROWS;
    const shown = capped ? calls.slice(-MAX_ROWS) : calls;
    capEl.hidden = !capped;
    if (capped) capEl.textContent = `Showing the newest ${MAX_ROWS} of ${calls.length} calls.`;
    const top = scroller.scrollTop;
    replace(scroller, shown.length > 0 ? shown.map(callRow) : h('div.muted', null, 'No tool calls yet'));
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
