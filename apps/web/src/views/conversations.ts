// The conversations screen: agent-to-agent threads, grouped by the pair of agents.
// Exchanges come a page at a time, newest first, and a thread pages its own history.
// The search box narrows both; the retention limits are set from the header.
// The markup is the prototype's (styles/proto/society.css): the pair list, the thread card and its bubbles.

import '../styles/proto/society.css';
import { api, type Conversation, type ConversationPair, type Retention } from '../api';
import { agentName } from '../roles';
import { go, href } from '../router';
import { agents, on } from '../store';
import { attempt, badge, button, clip, empty, input, toast } from '../ui/components';
import { embedContextFromStore } from '../ui/embeds';
import { h, replace } from '../ui/dom';
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

/** The relative time as the prototype words it: "just now", "11 min ago", "1 h ago", "2 d ago". */
function agoText(ms: number | null | undefined, now = Date.now()): string {
  if (!ms) return '—';
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return 'just now';
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const list = h('div.cv-list');
  let threadEl: HTMLElement = h('div.cv-card');
  const wrap = h('div.cv-wrap', null, list, threadEl);
  const search = input({ type: 'search', placeholder: 'Search exchanges, agents, roles', 'aria-label': 'Search exchanges', class: 'cv-search', autocomplete: 'off' });
  const count = h('span.cv-count', { 'aria-live': 'polite' });
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
  const retentionBtn = h('button.btn.ghost.sm', { type: 'button', 'aria-expanded': 'false', 'aria-controls': 'convo-retention' }, retentionSummary(RETENTION_DEFAULTS));
  const maxInput = input({ inputmode: 'numeric', step: 1, min: KEEP_MIN, max: KEEP_MAX, 'aria-label': 'Exchanges to keep' });
  const daysInput = input({ inputmode: 'numeric', step: 1, min: 0, max: DAYS_MAX, 'aria-label': 'Days before an exchange is deleted' });
  maxInput.style.width = '120px';
  daysInput.style.width = '90px';
  const maxError = h('p.err', { hidden: true });
  const daysError = h('p.err', { hidden: true });
  const saveBtn = button('Save', () => void saveRetention(), { kind: 'primary', size: 'sm' });
  const retentionPanel = h(
    'section.cv-ret',
    { id: 'convo-retention', 'aria-label': 'Retention', hidden: true },
    h('div.cv-bar', null, 'Keep the newest ', maxInput, ' exchanges · delete after ', daysInput, ' days (0 = never)'),
    maxError,
    daysError,
    h('p.hint', null, 'Older exchanges are removed automatically: the oldest go first once the count is passed, and any older than the day limit go.'),
    h('div.cluster', null, saveBtn),
  );

  // The search bar: the box, Clear while a search is on, and the count.
  const bar = h('div.cv-bar', null, search, clear, count);

  root.append(
    h('header.view-head', null, h('div', null, h('h1', null, 'Conversations')), h('div.cluster', null, retentionBtn)),
    retentionPanel,
    bar,
    wrap,
  );

  const label = (cid: number): string => {
    const a = agents.get().find((x) => x.cid === cid);
    return a ? agentName(a) : `agent ${cid}`;
  };

  /** The scrolling ancestor of this view: the view host, or the page when there is none. */
  const scroller = (): HTMLElement => (root.closest('.view-host') as HTMLElement | null) ?? document.documentElement;

  function updateCount(): void {
    clear.hidden = !query;
    if (!query || !feedLoaded || feedFailed) {
      count.textContent = '';
      return;
    }
    const n = feed.items.length;
    count.textContent = `${n}${feed.next != null ? '+' : ''} ${n === 1 && feed.next == null ? 'result' : 'results'}`;
  }

  // ---- Retention -------------------------------------------------------

  function setErr(el: HTMLElement, msg: string | undefined): void {
    el.hidden = msg === undefined;
    el.textContent = msg ?? '';
  }

  function syncRetention(): void {
    const check = checkRetention(maxInput.value, daysInput.value);
    setErr(maxError, check.errors.max);
    setErr(daysError, check.errors.days);
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
      retentionBtn.textContent = retentionSummary(saved);
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
      retentionBtn.textContent = retentionSummary(retention);
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
      nodes.push(h('p.muted', null, query ? `No exchanges match “${query}”.` : 'No conversations yet.'));
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

  /** Brings the newest exchange into view when it sits below the fold. */
  const reveal = (el: Element | undefined): void => {
    if (el && el.getBoundingClientRect().bottom > window.innerHeight) el.scrollIntoView({ block: 'end' });
  };

  function renderThread(prepended?: { height: number; top: number }): void {
    const key = selected;
    const st = key !== undefined && threadState?.key === key ? threadState : undefined;
    const ready = !!st && (st.loaded || st.failed);
    // A reader at the newest exchange follows new messages; one reading earlier keeps their place.
    const old = threadEl.querySelectorAll('.cv-msg');
    const followed = old.length === 0 || (old[old.length - 1]?.getBoundingClientRect().bottom ?? 0) <= window.innerHeight + 24;
    const next = key !== undefined ? threadCard(key, st) : h('div.cv-card');
    wrap.replaceChild(next, threadEl);
    threadEl = next;
    if (key === undefined || !ready) {
      shownKey = undefined;
      return;
    }
    const msgs = threadEl.querySelectorAll('.cv-msg');
    const newest = msgs[msgs.length - 1];
    if (shownKey !== key) reveal(newest);
    else if (prepended) {
      const sc = scroller();
      sc.scrollTop = prepended.top + (sc.scrollHeight - prepended.height);
    } else if (followed) reveal(newest);
    shownKey = key;
  }

  function threadCard(key: string, st: ThreadState | undefined): HTMLElement {
    const [a, b] = pairIds(key);
    const count = st?.loaded ? (query ? `${st.span.items.length}${st.span.next != null ? '+' : ''} matching` : `${st.total} exchanges`) : '';
    const card = h(
      'div.cv-card.cv-thread',
      null,
      h('div.cv-bar', null, h('span', null, nameLink(a, label), ' · ', nameLink(b, label)), h('span.cv-count', null, count)),
    );
    if (!st || (!st.loaded && !st.failed)) {
      card.append(h('p.muted', null, 'Loading…'));
      return card;
    }
    if (!st.loaded) {
      card.append(h('p.muted', null, 'Could not load this thread.'));
      return card;
    }
    if (st.span.next != null) card.append(moreButton(st.loadingOlder ? 'Loading…' : 'Load earlier messages', st.loadingOlder, () => void loadOlderThread()));
    if (!st.span.items.length) card.append(h('p.muted', null, query ? `No exchanges match “${query}”.` : 'No exchanges yet.'));
    const first = st.span.items[0]?.from_cid ?? a;
    for (const m of st.span.items) card.append(bubble(m, m.from_cid !== first, label));
    return card;
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
      const sc = scroller();
      const height = sc.scrollHeight;
      const top = sc.scrollTop;
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
  updateCount();
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

/** A ghost small button under the list or the thread, busy while a page loads. */
function moreButton(label: string, busy: boolean, onClick: () => void): HTMLElement {
  return h('div.cluster', null, button(label, onClick, { kind: 'ghost', size: 'sm', disabled: busy }));
}

function nameLink(cid: number, label: (cid: number) => string): HTMLElement {
  return h('a', { href: href('agents', cid) }, label(cid));
}

function pairItem(p: Pair, count: number, active: boolean, label: (cid: number) => string, onSelect: () => void): HTMLElement {
  const item = h(
    'button.cv-item',
    { type: 'button', 'aria-current': String(active) },
    h('b', null, `${label(p.a)} · ${label(p.b)}`),
    h('span', null, clip(p.last.text, 90)),
    h('span', null, `${agoText(p.last.created_at)} · ${count} exchanges`),
  );
  item.addEventListener('click', onSelect);
  return item;
}

function bubble(m: Conversation, right: boolean, label: (cid: number) => string): HTMLElement {
  return h(
    'div.cv-msg',
    { class: right ? 'right' : null },
    h(
      'div.cv-meta',
      null,
      nameLink(m.from_cid, label),
      h('span', null, agoText(m.created_at)),
      m.delivered ? null : badge('undelivered', 'warn'),
    ),
    h('div.cv-body', null, renderMarkdown(m.text, { embeds: embedContextFromStore() })),
  );
}
