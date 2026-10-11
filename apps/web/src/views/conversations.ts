// The conversations screen: agent-to-agent threads, grouped by the pair of agents.

import '../styles/social.css';
import { api, type Conversation } from '../api';
import { go, href } from '../router';
import { agents, on } from '../store';
import { ago, badge, clip, empty } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

const DEBOUNCE_MS = 300;

interface Pair {
  key: string;
  a: number;
  b: number;
  messages: Conversation[];
  last: Conversation;
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const list = h('div.convo-list');
  const thread = h('div.thread');
  root.append(h('div.view-head', null, h('h1', null, 'Conversations')), h('div.convo', null, list, thread));

  let conversations: Conversation[] = [];
  let loaded = false;
  let failed = false;
  let alive = true;
  let timer: number | undefined;
  let selected: string | undefined = params[0] ? normalizeKey(params[0]) : undefined;

  const roleOf = (cid: number): string => {
    for (const c of conversations) {
      if (c.from_cid === cid) return c.from_role;
      if (c.to_cid === cid) return c.to_role;
    }
    return 'agent';
  };

  const label = (cid: number): string => agents.get().find((a) => a.cid === cid)?.name ?? `${roleOf(cid)} ${cid}`;

  const select = (key: string): void => {
    selected = key;
    render();
    go('conversations', key);
  };

  const render = (): void => {
    if (!loaded) return;
    if (failed) {
      replace(list, empty('alert', 'Could not load conversations.'));
      replace(thread);
      return;
    }
    const pairs = groupPairs(conversations);
    if (!pairs.length) {
      replace(list, empty('conversations', 'No conversations yet.'));
      replace(thread);
      return;
    }
    const current = pairs.find((p) => p.key === selected) ?? pairs[0];
    if (!current) return;
    selected = current.key;
    replace(
      list,
      pairs.map((p) => pairItem(p, p.key === current.key, label, () => select(p.key))),
    );
    replace(thread, ...threadNodes(current, label));
    thread.scrollTop = thread.scrollHeight;
  };

  const load = async (): Promise<void> => {
    try {
      conversations = await api.conversations();
      failed = false;
    } catch {
      failed = true;
    }
    loaded = true;
    if (alive) render();
  };

  const refetch = (): void => {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => void load(), DEBOUNCE_MS);
  };

  const offs: (() => void)[] = [on('conversation', refetch), agents.watch(() => render())];
  void load();

  return () => {
    alive = false;
    window.clearTimeout(timer);
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

function nameLink(cid: number, label: (cid: number) => string): HTMLElement {
  return h('a.agent-link', { href: href('agents', cid) }, label(cid));
}

function pairItem(p: Pair, active: boolean, label: (cid: number) => string, onSelect: () => void): HTMLElement {
  const item = h(
    'button.convo-item',
    { type: 'button', 'aria-current': active ? 'true' : null },
    h('span.convo-names', null, `${label(p.a)} · ${label(p.b)}`),
    h('span.convo-last', null, clip(p.last.text, 90)),
    h('span.convo-meta', null, `${ago(p.last.created_at)} · ${p.messages.length} messages`),
  );
  item.addEventListener('click', onSelect);
  return item;
}

function threadNodes(p: Pair, label: (cid: number) => string): Node[] {
  const first = p.messages[0]?.from_cid ?? p.a;
  return [
    h('div.thread-head', null, nameLink(p.a, label), ' · ', nameLink(p.b, label)),
    ...p.messages.map((m) => bubble(m, m.from_cid === first ? 'left' : 'right', label)),
  ];
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
      h('p.bubble-text', null, m.text),
    ),
  );
}
