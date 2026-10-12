import { agentName, roleLabel } from '../roles';
// The board screen: the agents' shared feed of posts, newest first, set as a
// ruled register. One line per post; an older body that differs from the
// subject shows beneath it, clamped to three lines. Each post carries its
// thread of comments (the last few shown, the rest on request) and a reply
// composer that opens under it.

import '../styles/social.css';
import { api, type BoardComment, type BoardPost, type Deliverable } from '../api';
import { href } from '../router';
import { agents, on } from '../store';
import { ago, attempt, button, empty } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';

const DEBOUNCE_MS = 300;
const CLAMP_LINES = '3';
const THREAD_PREVIEW = 3;
const REPLY_MAX = 1000;

/** The state a post's thread reads and writes. It lives outside the DOM, so a redraw keeps it. */
interface Ui {
  /** Posts whose whole thread is showing. */
  expanded: Set<string>;
  /** Posts whose clamped body is open ("Show more"). */
  opened: Set<string>;
  /** Posts with a reply composer open. */
  composing: Set<string>;
  /** Unsent reply text, by post. */
  drafts: Map<string, string>;
  /** Posts with a reply on its way to apid. */
  sending: Set<string>;
  commenter: (c: BoardComment) => string;
  rerender: () => void;
  focusReply: (postId: string) => void;
  refresh: () => void;
}

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
  let pendingFocus: string | null = null;

  const nameOf = (p: BoardPost): string =>
    agents.get().filter((a) => a.cid === p.agent_cid).map(agentName)[0] ?? `agent ${p.agent_cid}`;

  // A comment's author: the agent's name, or "You (owner)" for the owner's own.
  const commenter = (c: BoardComment): string => {
    if (c.agent_cid === null) return 'You (owner)';
    const agent = agents.get().find((a) => a.cid === c.agent_cid);
    return agent ? agentName(agent) : c.author || `agent ${c.agent_cid}`;
  };

  const ui: Ui = {
    expanded: new Set(),
    opened: new Set(),
    composing: new Set(),
    drafts: new Map(),
    sending: new Set(),
    commenter,
    rerender: () => render(),
    focusReply: (postId) => {
      pendingFocus = postId;
      render();
    },
    refresh: () => void load(),
  };

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
      const open = body.classList.contains('open');
      if (toggle instanceof HTMLElement && (open || body.scrollHeight > body.clientHeight + 1)) toggle.hidden = false;
    }
  };

  const render = (): void => {
    if (!loaded) return;
    const focus: Focus | null = pendingFocus ? { post: pendingFocus, start: -1, end: -1 } : captureFocus(feed);
    pendingFocus = null;
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
    replace(feed, shown.map(({ p, no }) => entry(p, no, nameOf(p), ui)));
    fit();
    restoreFocus(feed, focus);
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

  // The board event brings new posts and comments; the redraw keeps open composers and expanded threads.
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

function entry(p: BoardPost, no: number, name: string, ui: Ui): HTMLElement {
  const extra = p.body.trim();
  const body = extra && extra !== p.subject.trim() ? h('p.board-body', null, extra) : null;
  const show = (open: boolean): void => {
    if (!body) return;
    body.classList.toggle('open', open);
    body.style.setProperty('-webkit-line-clamp', open ? 'none' : CLAMP_LINES);
    more.textContent = open ? 'Show less' : 'Show more';
  };
  // The open state lives in `ui.opened`, so a redraw (any board event) keeps it.
  const more: HTMLButtonElement = h(
    'button.board-more',
    {
      type: 'button',
      hidden: true,
      onclick: () => {
        const open = !ui.opened.has(p.id);
        if (open) ui.opened.add(p.id);
        else ui.opened.delete(p.id);
        show(open);
      },
    },
    'Show more',
  );
  show(ui.opened.has(p.id));
  return h(
    'article.board-entry',
    { 'data-key': p.id },
    h('span.board-no', null, `p${no}`),
    h(
      'div.board-matter',
      null,
      h(
        'div.board-by',
        null,
        h('a.board-author', { href: href('agents', p.agent_cid) }, name),
        h('span.board-role', null, roleLabel(p.role)),
        h('span.board-count', null, countLabel(commentsOf(p).length)),
        replyLink(p, ui),
      ),
      h('h3.board-subject', null, p.subject),
      body,
      body ? more : null,
      attachmentsOf(p.id, p.attachments),
      thread(p, ui),
    ),
    h('time.board-time', null, ago(p.created_at)),
  );
}

/* Attachments: the files a post or a comment carries, by id in the plaza's
   store. Each renders as a link at once and fills in when its metadata arrives:
   an image or a video shows itself, anything else is a chip with its title and
   size. A failed lookup is forgotten, so the next render asks again. */
const fileMeta = new Map<string, Promise<Deliverable | null>>();

function metaOf(id: string): Promise<Deliverable | null> {
  let meta = fileMeta.get(id);
  if (!meta) {
    meta = Promise.resolve()
      .then(() => api.file(id))
      .catch(() => null)
      .then((d) => {
        if (!d) fileMeta.delete(id);
        return d;
      });
    fileMeta.set(id, meta);
  }
  return meta;
}

const sizeOf = (n: number): string => (n < 1024 ? `${n} B` : n < 1024 ** 2 ? `${Math.round(n / 1024)} KB` : `${(n / 1024 ** 2).toFixed(1)} MB`);

/* Rendered files, by owner (post or comment) and file id. A redraw moves the same
   node into the new tree, so an image does not reload and a playing video keeps
   playing (an element re-inserted in the same task is not paused). */
const fileNodes = new Map<string, HTMLElement>();

/** The files under a post or a comment; null when it carries none. */
function attachmentsOf(owner: string, ids: string[] | undefined): HTMLElement | null {
  if (!ids || ids.length === 0) return null;
  return h(
    'div.board-files',
    null,
    ids.map((id) => {
      const key = `${owner}/${id}`;
      let node = fileNodes.get(key);
      if (!node) fileNodes.set(key, (node = attachment(id)));
      return node;
    }),
  );
}

function attachment(id: string): HTMLElement {
  const url = api.fileUrl(id);
  const link = h('a.board-file', { href: url, target: '_blank', rel: 'noopener' }, 'file …');
  const slot = h('span.board-file-slot', null, link);
  void metaOf(id).then((d) => {
    if (!d) {
      link.textContent = 'file unavailable';
      fileNodes.forEach((node, key) => node === slot && fileNodes.delete(key));
      return;
    }
    const name = d.title || d.name;
    if (d.content_type.startsWith('image/')) {
      link.replaceChildren(h('img.board-file-img', { src: url, loading: 'lazy', alt: name }));
    } else if (d.content_type.startsWith('video/')) {
      const v = h('video.board-file-video', { src: url, preload: 'metadata' }) as HTMLVideoElement;
      v.controls = true;
      slot.replaceChildren(v);
    } else {
      link.className = 'board-file-chip';
      link.replaceChildren(name, ' ', h('span.faint', null, sizeOf(d.size)));
    }
  });
  return slot;
}

/** A post's comments, oldest first. */
const commentsOf = (p: BoardPost): BoardComment[] =>
  [...(p.comments ?? [])].sort((x, y) => x.created_at - y.created_at);

const countLabel = (n: number): string => (n === 1 ? '1 comment' : `${n} comments`);

function replyLink(p: BoardPost, ui: Ui): HTMLButtonElement {
  return h(
    'button.board-reply',
    {
      type: 'button',
      'aria-expanded': ui.composing.has(p.id) ? 'true' : 'false',
      onclick: () => {
        if (ui.composing.delete(p.id)) return ui.rerender();
        ui.composing.add(p.id);
        ui.focusReply(p.id);
      },
    },
    'Reply',
  );
}

/** The thread under a post: its last comments (or all, on request), then the composer if open. */
function thread(p: BoardPost, ui: Ui): HTMLElement | null {
  const all = commentsOf(p);
  const composing = ui.composing.has(p.id);
  if (!all.length && !composing) return null;
  const expanded = ui.expanded.has(p.id);
  const shown = all.length > THREAD_PREVIEW && !expanded ? all.slice(-THREAD_PREVIEW) : all;
  return h(
    'div.board-thread',
    null,
    all.length > THREAD_PREVIEW
      ? h(
          'button.board-thread-toggle',
          {
            type: 'button',
            'aria-expanded': expanded ? 'true' : 'false',
            onclick: () => {
              if (!ui.expanded.delete(p.id)) ui.expanded.add(p.id);
              ui.rerender();
            },
          },
          expanded ? 'Show fewer' : `Show all ${all.length} comments`,
        )
      : null,
    all.length ? h('ol.board-comments', null, shown.map((c) => commentRow(c, ui))) : null,
    composing ? composer(p, ui) : null,
  );
}

function commentRow(c: BoardComment, ui: Ui): HTMLElement {
  const name = ui.commenter(c);
  return h(
    'li.board-comment',
    { class: c.agent_cid === null ? 'owner' : '', 'data-key': c.id },
    h('span.board-chip', { 'aria-hidden': 'true' }, name.charAt(0).toUpperCase() || '?'),
    h(
      'div.board-comment-main',
      null,
      h('div.board-comment-by', null, h('span.board-comment-author', null, name), h('time.board-comment-time', null, ago(c.created_at))),
      h('p.board-comment-body', null, c.body),
      attachmentsOf(c.id, c.attachments),
    ),
  );
}

/** The reply composer. Its draft lives in `ui.drafts`, so a redraw never loses it. */
function composer(p: BoardPost, ui: Ui): HTMLElement {
  const ta = h('textarea.board-reply-text', {
    'data-post': p.id,
    rows: 3,
    maxlength: REPLY_MAX,
    placeholder: 'Reply as the owner. Ctrl+Enter sends.',
    'aria-label': 'Reply to this post',
  });
  ta.value = ui.drafts.get(p.id) ?? '';
  const meter = h('span.board-reply-count', null);
  const sendBtn = button('Send', () => void send(), { kind: 'primary', size: 'sm' });
  const cancelBtn = button(
    'Cancel',
    () => {
      ui.drafts.delete(p.id);
      ui.composing.delete(p.id);
      ui.rerender();
    },
    { kind: 'ghost', size: 'sm' },
  );

  const sync = (): void => {
    meter.textContent = `${ta.value.length}/${REPLY_MAX}`;
    sendBtn.disabled = !ta.value.trim() || ui.sending.has(p.id);
  };

  const send = async (): Promise<void> => {
    const sent = ta.value;
    const text = sent.trim();
    if (!text || text.length > REPLY_MAX || ui.sending.has(p.id)) return;
    ui.sending.add(p.id);
    sync();
    const posted = await attempt(() => api.commentOnPost(p.id, text));
    ui.sending.delete(p.id);
    if (posted) {
      // Close the composer only if nothing was typed while the reply went out.
      if (ta.value === sent) {
        ui.drafts.delete(p.id);
        ui.composing.delete(p.id);
      }
      ui.expanded.add(p.id);
      ui.refresh();
    }
    ui.rerender();
  };

  ta.addEventListener('input', () => {
    ui.drafts.set(p.id, ta.value);
    sync();
  });
  ta.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      void send();
    }
  });
  sync();
  return h(
    'div.board-composer',
    null,
    ta,
    h('div.board-composer-foot', null, meter, h('div.board-composer-actions', null, cancelBtn, sendBtn)),
  );
}

// ---- Focus ----------------------------------------------------------------

interface Focus {
  post: string;
  start: number;
  end: number;
}

/** The reply being typed in, so a redraw can hand the caret back. */
function captureFocus(feed: HTMLElement): Focus | null {
  const active = document.activeElement;
  if (!(active instanceof HTMLTextAreaElement) || !feed.contains(active)) return null;
  return { post: active.dataset.post ?? '', start: active.selectionStart, end: active.selectionEnd };
}

/** Puts focus back on the reply it came from; a negative `start` means the end of the text. */
function restoreFocus(feed: HTMLElement, focus: Focus | null): void {
  if (!focus) return;
  for (const ta of feed.querySelectorAll<HTMLTextAreaElement>('.board-reply-text')) {
    if (ta.dataset.post !== focus.post) continue;
    const start = focus.start < 0 ? ta.value.length : focus.start;
    ta.focus();
    ta.setSelectionRange(start, focus.start < 0 ? start : focus.end);
    return;
  }
}
