import { agentName } from '../roles';
// The conversations screen: agent-to-agent threads, grouped by the pair of agents.
// Exchanges come a page at a time, newest first, and a thread pages its own history.
// The search box narrows both; the retention limits are set from the header.

import '../styles/social.css';
import { api, type Conversation, type ConversationPair, type Retention } from '../api';
import { go, href } from '../router';
import { agents, on } from '../store';
import { ago, attempt, badge, button, clip, empty, input, toast } from '../ui/components';
import { h, icon, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import type { Cleanup } from './index';

const DEBOUNCE_MS = 300;
const SEARCH_MS = 250;
/** Exchanges per page, for the pair list and for a thread. */
export const PAGE = 50;
/** The hub caps a search at this many bytes. */
const QUERY_BYTES = 200;
export const RETENTION_DEFAULTS: Retention = { conversations_max: 4096, conversations_days: 30, posts_max: 1024, posts_days: 0 };
const KEEP_MIN = 100;
const KEEP_MAX = 100_000;
const DAYS_MAX = 3650;

interface Pair {
  key: string;
  a: number;
  b: number;
  messages: Conversation[];
  last: Conversation;
}

/** Exchanges kept oldest first. `next` is the `before` cursor of the page older than them, or null when none remain. */
export interface Span {
  items: Conversation[];
  next: number | null;
}

/** The open thread: the loaded span and the pair's whole-record count. */
interface ThreadState {
  key: string;
  span: Span;
  total: number;
  loaded: boolean;
  failed: boolean;
  loadingOlder: boolean;
}

export function spanOf(items: Conversation[], next: number | null): Span {
  return { items: [...items].sort((x, y) => x.created_at - y.created_at), next };
}

/** A fresh newest page folded into what is loaded: the older exchanges already paged in are kept. */
export function mergeNewest(prev: Span, fresh: Span): Span {
  const [first] = fresh.items;
  if (!prev.items.length || !first) return fresh;
  const ids = new Set(fresh.items.map((c) => c.id));
  const kept = prev.items.filter((c) => !ids.has(c.id) && c.created_at <= first.created_at);
  return { items: [...kept, ...fresh.items], next: kept.length ? prev.next : fresh.next };
}

/** An older page folded in before what is loaded. */
export function mergeOlder(prev: Span, older: Span): Span {
  const ids = new Set(prev.items.map((c) => c.id));
  return { items: [...older.items.filter((c) => !ids.has(c.id)), ...prev.items], next: older.next };
}

/** The longest prefix of a search that fits the hub's byte cap. */
export function clampQuery(text: string): string {
  const encoder = new TextEncoder();
  let bytes = 0;
  let out = '';
  for (const ch of text.trim()) {
    bytes += encoder.encode(ch).length;
    if (bytes > QUERY_BYTES) break;
    out += ch;
  }
  return out;
}

export interface RetentionCheck {
  max: number;
  days: number;
  errors: { max?: string; days?: string };
}

/** The retention fields as typed: whole numbers within the hub's limits. */
export function checkRetention(maxText: string, daysText: string): RetentionCheck {
  const maxTrim = maxText.trim();
  const daysTrim = daysText.trim();
  const max = Number(maxTrim);
  const days = Number(daysTrim);
  const errors: RetentionCheck['errors'] = {};
  if (!/^\d+$/.test(maxTrim) || max < KEEP_MIN || max > KEEP_MAX) errors.max = `Enter a whole number from ${KEEP_MIN} to ${KEEP_MAX}.`;
  if (!/^\d+$/.test(daysTrim) || days > DAYS_MAX) errors.days = `Enter a whole number from 0 to ${DAYS_MAX}. 0 keeps them forever.`;
  return { max, days, errors };
}

function retentionSummary(r: Retention): string {
  return `Retention · ${r.conversations_max} · ${r.conversations_days > 0 ? `${r.conversations_days} days` : 'kept forever'}`;
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const list = h('div.convo-list');
  const thread = h('div.thread');
  const search = input({ type: 'search', placeholder: 'Search exchanges, agents, roles', 'aria-label': 'Search conversations', autocomplete: 'off' });
  const count = h('span.convo-count', { 'aria-live': 'polite' });
  const clear = button('Clear', () => {
    window.clearTimeout(searchTimer);
    search.value = '';
    setQuery('');
    search.focus();
  }, { kind: 'ghost', size: 'sm', icon: 'x' });

  let alive = true;
  let timer: number | undefined;
  let searchTimer: number | undefined;
  let query = '';
  let selected: string | undefined = params[0] ? normalizeKey(params[0]) : undefined;

  // The pair list: the exchanges loaded so far, newest first, and the whole-record counts per pair.
  let feed: Span = { items: [], next: null };
  let pairCounts = new Map<string, number>();
  let feedLoaded = false;
  let feedFailed = false;
  let feedBusy = false;
  let feedSeq = 0;

  let threadState: ThreadState | undefined;
  let threadSeq = 0;
  let shownKey: string | undefined;

  // The retention block of the header: a row that opens under it.
  let retention: Retention = { ...RETENTION_DEFAULTS };
  let retentionBusy = false;
  const retentionLabel = h('span', null, retentionSummary(RETENTION_DEFAULTS));
  const retentionBtn = h('button.btn.ghost.sm', { type: 'button', 'aria-expanded': 'false', 'aria-controls': 'convo-retention' }, icon('settings'), retentionLabel);
  const maxInput = input({ type: 'number', inputmode: 'numeric', step: 1, min: KEEP_MIN, max: KEEP_MAX, 'aria-label': 'Exchanges to keep' });
  const daysInput = input({ type: 'number', inputmode: 'numeric', step: 1, min: 0, max: DAYS_MAX, 'aria-label': 'Days before an exchange is deleted' });
  maxInput.classList.add('retention-input');
  daysInput.classList.add('retention-input');
  const maxError = h('span.retention-error', null);
  const daysError = h('span.retention-error', null);
  const saveBtn = button('Save', () => void saveRetention(), { kind: 'primary', size: 'sm' });
  const retentionPanel = h(
    'section.retention',
    { id: 'convo-retention', 'aria-label': 'Retention', hidden: true },
    h(
      'div.retention-line',
      null,
      h('span.retention-label', null, 'Keep the newest'),
      maxInput,
      h('span.retention-label', null, 'exchanges · delete after'),
      daysInput,
      h('span.retention-label', null, 'days (0 = never)'),
    ),
    h('div.retention-errors', null, maxError, daysError),
    h('p.retention-note', null, 'Older exchanges are removed automatically: the oldest go first once the count is passed, and any older than the day limit go.'),
    h('div.retention-actions', null, saveBtn),
  );

  root.append(
    h('div.view-head', null, h('h1', null, 'Conversations'), h('div.actions', null, retentionBtn)),
    retentionPanel,
    h('div.convo-bar', null, search, clear, count),
    h('div.convo', null, list, thread),
  );

  const label = (cid: number): string => {
    const a = agents.get().find((x) => x.cid === cid);
    return a ? agentName(a) : `agent ${cid}`;
  };

  function updateCount(): void {
    if (!query || !feedLoaded || feedFailed) {
      count.textContent = '';
      return;
    }
    const n = feed.items.length;
    count.textContent = `${n}${feed.next != null ? '+' : ''} ${n === 1 && feed.next == null ? 'result' : 'results'}`;
  }

  // ---- Retention -------------------------------------------------------

  function syncRetention(): void {
    const check = checkRetention(maxInput.value, daysInput.value);
    maxError.textContent = check.errors.max ?? '';
    daysError.textContent = check.errors.days ?? '';
    maxInput.setAttribute('aria-invalid', String(check.errors.max !== undefined));
    daysInput.setAttribute('aria-invalid', String(check.errors.days !== undefined));
    const changed = check.max !== retention.conversations_max || check.days !== retention.conversations_days;
    saveBtn.disabled = retentionBusy || !changed || check.errors.max !== undefined || check.errors.days !== undefined;
  }

  function setRetentionOpen(open: boolean): void {
    retentionPanel.hidden = !open;
    retentionBtn.setAttribute('aria-expanded', String(open));
    if (!open) return;
    maxInput.value = String(retention.conversations_max);
    daysInput.value = String(retention.conversations_days);
    syncRetention();
    maxInput.focus();
  }

  async function saveRetention(): Promise<void> {
    const check = checkRetention(maxInput.value, daysInput.value);
    if (check.errors.max !== undefined || check.errors.days !== undefined) {
      syncRetention();
      return;
    }
    retentionBusy = true;
    syncRetention();
    const saved = await attempt(async () => {
      // The whole settings document is saved, so it is read fresh and only the retention block changes.
      const fresh = await api.settings();
      const block: Retention = { ...RETENTION_DEFAULTS, ...fresh.retention, conversations_max: check.max, conversations_days: check.days };
      await api.saveSettings({ ...fresh, retention: block });
      return block;
    }, 'Retention saved');
    retentionBusy = false;
    if (!alive) return;
    if (saved) {
      retention = saved;
      retentionLabel.textContent = retentionSummary(saved);
      setRetentionOpen(false);
      refetch();
    } else {
      syncRetention();
    }
  }

  retentionBtn.addEventListener('click', () => setRetentionOpen(retentionPanel.hidden));
  maxInput.addEventListener('input', syncRetention);
  daysInput.addEventListener('input', syncRetention);
  void api.settings().then(
    (s) => {
      if (!alive) return;
      retention = { ...RETENTION_DEFAULTS, ...s.retention };
      retentionLabel.textContent = retentionSummary(retention);
    },
    () => undefined,
  );

  // ---- The pair list ---------------------------------------------------

  const select = (key: string): void => {
    selected = key;
    threadState = undefined;
    renderList();
    renderThread();
    void loadThread(true);
    go('conversations', key);
  };

  function renderList(): void {
    if (!feedLoaded) return;
    updateCount();
    if (feedFailed) {
      replace(list, empty('alert', 'Could not load conversations.'));
      return;
    }
    const pairs = groupPairs(feed.items);
    const nodes: Node[] = [];
    if (!pairs.length) {
      nodes.push(query ? empty('search', `No exchanges match “${query}”.`) : empty('conversations', 'No conversations yet.'));
    } else {
      for (const p of pairs) {
        const exchanges = (!query && pairCounts.get(p.key)) || p.messages.length;
        nodes.push(pairItem(p, exchanges, p.key === selected, label, () => select(p.key)));
      }
    }
    if (feed.next != null) nodes.push(moreButton(feedBusy ? 'Loading…' : 'Load older exchanges', feedBusy, () => void loadOlder()));
    replace(list, nodes);
  }

  async function loadFeed(reset: boolean): Promise<void> {
    const seq = ++feedSeq;
    try {
      const page = await api.conversationsPage({ limit: PAGE, q: query || undefined });
      if (!alive || seq !== feedSeq) return;
      const span = spanOf(page.conversations, page.next_before);
      // A refresh keeps the older pages the reader has already paged in.
      feed = reset ? span : mergeNewest(feed, span);
      pairCounts = new Map(page.pairs.map((p: ConversationPair) => [`${p.a}-${p.b}`, p.count]));
      feedFailed = false;
    } catch {
      if (!alive || seq !== feedSeq) return;
      if (reset || !feedLoaded) feedFailed = true;
    }
    feedLoaded = true;
    if (!selected) {
      const first = groupPairs(feed.items)[0];
      if (first) {
        selected = first.key;
        renderThread();
        void loadThread(true);
      }
    }
    renderList();
  }

  async function loadOlder(): Promise<void> {
    const cursor = feed.next;
    if (cursor == null || feedBusy) return;
    const seq = feedSeq;
    feedBusy = true;
    renderList();
    try {
      const page = await api.conversationsPage({ limit: PAGE, before: cursor, q: query || undefined });
      if (!alive || seq !== feedSeq) return;
      feed = mergeOlder(feed, spanOf(page.conversations, page.next_before));
    } catch {
      if (alive && seq === feedSeq) toast('Could not load older exchanges.', 'error');
    } finally {
      feedBusy = false;
      if (alive) renderList();
    }
  }

  // ---- The open thread -------------------------------------------------

  function renderThread(prepended?: { height: number; top: number }): void {
    const key = selected;
    if (!key) {
      replace(thread);
      shownKey = undefined;
      return;
    }
    const st = threadState?.key === key ? threadState : undefined;
    const ready = !!st && (st.loaded || st.failed);
    // A reader at the foot of the thread follows new messages; one reading earlier keeps their place.
    const pinned = thread.scrollTop + thread.clientHeight >= thread.scrollHeight - 24;
    replace(thread, threadNodes(key, st));
    if (!ready) {
      shownKey = undefined;
      return;
    }
    if (shownKey !== key) thread.scrollTop = thread.scrollHeight;
    else if (prepended) thread.scrollTop = prepended.top + (thread.scrollHeight - prepended.height);
    else if (pinned) thread.scrollTop = thread.scrollHeight;
    shownKey = key;
  }

  function threadNodes(key: string, st: ThreadState | undefined): Node[] {
    const [a, b] = pairIds(key);
    const head = h(
      'div.thread-head',
      null,
      nameLink(a, label),
      ' · ',
      nameLink(b, label),
      st?.loaded ? h('span.faint', null, ` · ${query ? `${st.span.items.length}${st.span.next != null ? '+' : ''} matching` : `${st.total} exchanges`}`) : null,
    );
    if (!st || (!st.loaded && !st.failed)) return [head, h('p.faint', null, 'Loading…')];
    if (!st.loaded) return [head, h('p.faint', null, 'Could not load this thread.')];
    const nodes: Node[] = [head];
    if (st.span.next != null) nodes.push(moreButton(st.loadingOlder ? 'Loading…' : 'Load earlier messages', st.loadingOlder, () => void loadOlderThread()));
    if (!st.span.items.length) nodes.push(h('p.faint', null, query ? `No exchanges match “${query}”.` : 'No exchanges yet.'));
    const first = st.span.items[0]?.from_cid ?? a;
    for (const m of st.span.items) nodes.push(bubble(m, m.from_cid === first ? 'left' : 'right', label));
    return nodes;
  }

  /** Loads the selected pair's newest page; `reset` drops what was loaded, otherwise a refresh keeps older pages. */
  async function loadThread(reset: boolean): Promise<void> {
    const key = selected;
    if (!key) return;
    const seq = ++threadSeq;
    const [a, b] = pairIds(key);
    const prev = threadState?.key === key ? threadState : undefined;
    try {
      const page = await api.conversationThread(a, b, { limit: PAGE, q: query || undefined });
      if (!alive || seq !== threadSeq || selected !== key) return;
      const span = spanOf(page.messages, page.next_before);
      threadState = {
        key,
        span: reset || !prev ? span : mergeNewest(prev.span, span),
        total: page.total,
        loaded: true,
        failed: false,
        loadingOlder: false,
      };
    } catch {
      if (!alive || seq !== threadSeq || selected !== key) return;
      if (reset || !prev) threadState = { key, span: { items: [], next: null }, total: 0, loaded: false, failed: true, loadingOlder: false };
    }
    renderThread();
  }

  async function loadOlderThread(): Promise<void> {
    const st = threadState;
    if (!st || !st.loaded || st.loadingOlder || st.span.next == null) return;
    const cursor = st.span.next;
    const [a, b] = pairIds(st.key);
    const busy: ThreadState = { ...st, loadingOlder: true };
    threadState = busy;
    renderThread();
    try {
      const page = await api.conversationThread(a, b, { limit: PAGE, before: cursor, q: query || undefined });
      if (!alive || threadState !== busy) return;
      const height = thread.scrollHeight;
      const top = thread.scrollTop;
      threadState = { ...busy, span: mergeOlder(busy.span, spanOf(page.messages, page.next_before)), total: page.total, loadingOlder: false };
      renderThread({ height, top });
    } catch {
      if (!alive || threadState !== busy) return;
      threadState = { ...busy, loadingOlder: false };
      renderThread();
      toast('Could not load earlier messages.', 'error');
    }
  }

  // ---- Search and live updates -----------------------------------------

  function setQuery(next: string): void {
    if (next === query) return;
    query = next;
    feedBusy = false;
    updateCount();
    void loadFeed(true);
    if (selected) void loadThread(true);
  }

  search.addEventListener('input', () => {
    window.clearTimeout(searchTimer);
    searchTimer = window.setTimeout(() => setQuery(clampQuery(search.value)), SEARCH_MS);
  });
  search.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape' && search.value) {
      ev.preventDefault();
      clear.click();
    }
  });

  const refetch = (): void => {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => {
      void loadFeed(false);
      if (selected) void loadThread(false);
    }, DEBOUNCE_MS);
  };

  const offs: (() => void)[] = [
    on('conversation', refetch),
    agents.watch(() => {
      renderList();
      renderThread();
    }),
  ];
  void loadFeed(true);
  if (selected) {
    renderThread();
    void loadThread(true);
  }

  return () => {
    alive = false;
    window.clearTimeout(timer);
    window.clearTimeout(searchTimer);
    offs.forEach((off) => off());
  };
}

/** Normalises a `a-b` route segment to the ordered key used for grouping. */
function normalizeKey(raw: string): string {
  return raw
    .split('-')
    .map(Number)
    .sort((x, y) => x - y)
    .join('-');
}

function pairIds(key: string): [number, number] {
  const [a = 0, b = 0] = key.split('-').map(Number);
  return [a, b];
}

function groupPairs(list: Conversation[]): Pair[] {
  const pairs = new Map<string, Pair>();
  const oldestFirst = [...list].sort((x, y) => x.created_at - y.created_at);
  for (const c of oldestFirst) {
    const a = Math.min(c.from_cid, c.to_cid);
    const b = Math.max(c.from_cid, c.to_cid);
    const key = `${a}-${b}`;
    const pair = pairs.get(key) ?? { key, a, b, messages: [], last: c };
    pair.messages.push(c);
    pair.last = c;
    pairs.set(key, pair);
  }
  return Array.from(pairs.values()).sort((x, y) => y.last.created_at - x.last.created_at);
}

function moreButton(label: string, busy: boolean, onClick: () => void): HTMLButtonElement {
  const b = button(label, onClick, { kind: 'ghost', size: 'sm', disabled: busy });
  b.classList.add('convo-more');
  return b;
}

function nameLink(cid: number, label: (cid: number) => string): HTMLElement {
  return h('a.agent-link', { href: href('agents', cid) }, label(cid));
}

function pairItem(p: Pair, count: number, active: boolean, label: (cid: number) => string, onSelect: () => void): HTMLElement {
  const item = h(
    'button.convo-item',
    { type: 'button', 'aria-current': active ? 'true' : null },
    h('span.convo-names', null, `${label(p.a)} · ${label(p.b)}`),
    h('span.convo-last', null, clip(p.last.text, 90)),
    h('span.convo-meta', null, `${ago(p.last.created_at)} · ${count} exchanges`),
  );
  item.addEventListener('click', onSelect);
  return item;
}

function bubble(m: Conversation, side: 'left' | 'right', label: (cid: number) => string): HTMLElement {
  return h(
    'div.bubble-row',
    { class: side },
    h(
      'div.bubble',
      null,
      h(
        'div.bubble-meta',
        null,
        nameLink(m.from_cid, label),
        h('span.faint', null, ago(m.created_at)),
        m.delivered ? null : badge('undelivered', 'warn'),
      ),
      h('div.bubble-text', null, renderMarkdown(m.text)),
    ),
  );
}
