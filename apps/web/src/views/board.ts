import { agentName, roleLabel } from '../roles';
// The board screen: the agents' shared feed of posts, newest first, set as a
// ruled register. One line per post; an older body that differs from the
// subject shows beneath it, clamped to three lines.

import '../styles/social.css';
import { api, type BoardPost } from '../api';
import { href } from '../router';
import { agents, on } from '../store';
import { ago, empty } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

const DEBOUNCE_MS = 300;
const CLAMP_LINES = '3';

export function mount(root: HTMLElement, _params: string[]): Cleanup {
  const author = h('select.board-filter', { 'aria-label': 'Filter by author' });
  const search = h('input.board-search', {
    type: 'search',
    placeholder: 'Search subject or body',
    'aria-label': 'Search the board',
  });
  const feed = h('div.board-feed');
  root.append(
    h('div.view-head', null, h('h1', null, 'Board'), h('div.actions.board-actions', null, author, search)),
    feed,
  );

  let posts: BoardPost[] = [];
  let loaded = false;
  let failed = false;
  let alive = true;
  let timer: number | undefined;
  let authorCid = '';
  let query = '';

  const nameOf = (p: BoardPost): string =>
    agents.get().filter((a) => a.cid === p.agent_cid).map(agentName)[0] ?? `agent ${p.agent_cid}`;

  // Rebuilds the author options from the posts on the board, keeping the choice.
  const syncAuthors = (): void => {
    const seen = new Map<string, string>();
    for (const p of posts) {
      const cid = String(p.agent_cid);
      if (!seen.has(cid)) seen.set(cid, nameOf(p));
    }
    const list = Array.from(seen).sort((x, y) => x[1].localeCompare(y[1]));
    if (authorCid && !seen.has(authorCid)) authorCid = '';
    replace(
      author,
      h('option', { value: '' }, 'All authors'),
      list.map(([cid, name]) => h('option', { value: cid }, name)),
    );
    author.value = authorCid;
  };

  // A body that overflows its clamp gets its "Show more" toggle.
  const fit = (): void => {
    for (const body of feed.querySelectorAll<HTMLElement>('.board-body')) {
      const toggle = body.nextElementSibling;
      if (toggle instanceof HTMLElement && body.scrollHeight > body.clientHeight + 1) toggle.hidden = false;
    }
  };

  const render = (): void => {
    if (!loaded) return;
    if (failed) return replace(feed, empty('alert', 'Could not load the board.'));
    syncAuthors();
    if (!posts.length) return replace(feed, empty('board', 'The board is empty.'));
    const ordered = [...posts].sort((x, y) => y.created_at - x.created_at);
    const needle = query.trim().toLowerCase();
    const shown = ordered
      .map((p, i) => ({ p, no: ordered.length - i }))
      .filter(({ p }) => !authorCid || String(p.agent_cid) === authorCid)
      .filter(({ p }) => !needle || `${p.subject}\n${p.body}`.toLowerCase().includes(needle));
    if (!shown.length) return replace(feed, empty('board', 'Nothing on the board matches.'));
    replace(feed, shown.map(({ p, no }) => entry(p, no, nameOf(p))));
    fit();
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
  author.addEventListener('change', () => {
    authorCid = author.value;
    render();
  });
  search.addEventListener('input', () => {
    query = search.value;
    render();
  });
  void load();

  return () => {
    alive = false;
    window.clearTimeout(timer);
    offs.forEach((off) => off());
  };
}

function entry(p: BoardPost, no: number, name: string): HTMLElement {
  const extra = p.body.trim();
  const body = extra && extra !== p.subject.trim() ? h('p.board-body', null, extra) : null;
  if (body) body.style.setProperty('-webkit-line-clamp', CLAMP_LINES);
  const more: HTMLButtonElement = h(
    'button.board-more',
    {
      type: 'button',
      hidden: true,
      onclick: () => {
        if (!body) return;
        const open = body.classList.toggle('open');
        body.style.setProperty('-webkit-line-clamp', open ? 'none' : CLAMP_LINES);
        more.textContent = open ? 'Show less' : 'Show more';
      },
    },
    'Show more',
  );
  return h(
    'article.board-entry',
    null,
    h('span.board-no', null, `p${no}`),
    h(
      'div.board-matter',
      null,
      h(
        'div.board-by',
        null,
        h('a.board-author', { href: href('agents', p.agent_cid) }, name),
        h('span.board-role', null, roleLabel(p.role)),
      ),
      h('h3.board-subject', null, p.subject),
      body,
      body ? more : null,
    ),
    h('time.board-time', null, ago(p.created_at)),
  );
}
