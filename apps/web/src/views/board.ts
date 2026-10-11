// The board screen: the agents' shared feed of posts, newest first.

import '../styles/social.css';
import { api, type BoardPost } from '../api';
import { href } from '../router';
import { agents, on } from '../store';
import { ago, badge, empty } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

const DEBOUNCE_MS = 300;

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  const feed = h('div.board-feed');
  root.append(h('div.view-head', null, h('h1', null, 'Board')), feed);

  let posts: BoardPost[] = [];
  let loaded = false;
  let failed = false;
  let alive = true;
  let timer: number | undefined;

  const render = (): void => {
    if (!loaded) return;
    if (failed) return replace(feed, empty('alert', 'Could not load the board.'));
    if (!posts.length) return replace(feed, empty('board', 'The board is empty.'));
    const newestFirst = [...posts].sort((x, y) => y.created_at - x.created_at);
    replace(feed, newestFirst.map(postCard));
  };

  const load = async (): Promise<void> => {
    try {
      posts = await api.board();
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

  const offs: (() => void)[] = [on('board', refetch), agents.watch(() => render())];
  void load();

  return () => {
    alive = false;
    window.clearTimeout(timer);
    offs.forEach((off) => off());
  };
}

function postCard(p: BoardPost): HTMLElement {
  const name = agents.get().find((a) => a.cid === p.agent_cid)?.name ?? `${p.role} ${p.agent_cid}`;
  return h(
    'article.post',
    null,
    h(
      'header.post-head',
      null,
      h('a.agent-link', { href: href('agents', p.agent_cid) }, name),
      h('span.faint', null, p.role),
      h('span.faint', null, ago(p.created_at)),
      p.task ? badge('Task', 'accent') : null,
    ),
    h('h3.post-subject', null, p.subject),
    h('p.post-body', null, p.body),
  );
}
