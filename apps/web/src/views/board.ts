import { agentLabel, agentName, roleLabel } from '../roles';
import { projectTitle, scope } from '../scope';
// The board screen, as the Loom console draws it (the bd-* rules in styles/proto/social.css): the
// agents' shared feed of posts, newest first, each with its numbered head, its clamped body, its
// thread of comments (the last few shown, the rest on request) and a reply composer that opens under
// it. The owner's composer sits in a dock at the foot.

import '../styles/proto/social.css';
import { api, type BoardComment, type BoardPost, type Deliverable } from '../api';
import { boardOwnerBar, composer as ownerComposer } from '../board/owner';
import { href } from '../router';
import { agents, on } from '../store';
import { ago, attempt, button, empty } from '../ui/components';
import { embedContextFromStore } from '../ui/embeds';
import { h, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import type { Cleanup } from './index';

const DEBOUNCE_MS = 300;
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
  // Under a project, the board is its board: its agents' posts, and the owner's posts go to it.
  const inProject = scope.get() ?? undefined;
  const author = h('select.select', { 'aria-label': 'Author', 'data-input': 'bd-author' });
  const search = h('input.input', {
    type: 'search',
    placeholder: 'Search subject or body',
    'aria-label': 'Search the board',
  });
  const feed = h('div.bd-feed');
  // The owner's controls: Pause/Resume and Export, the paused banner, and the composer at the foot.
  const owner = boardOwnerBar({ project: () => inProject, onChanged: () => void load(), proto: true });
  const post = ownerComposer({
    placeholder: 'Message the team… (@ to mention)',
    label: 'Message the team',
    project: () => inProject,
    onPosted: () => void load(),
    proto: true,
  });
  const title = h('h1', null, 'Board', inProject ? h('span.scoped', null, ` · ${projectTitle(inProject)}`) : null);
  root.append(
    h(
      'header.page-head',
      null,
      h('div.page-head-row', null, title, h('span.sp'), h('div.page-actions', null, h('div.bd-bar', null, author, search, owner.actions))),
    ),
    owner.banner,
    feed,
    post.el,
  );

  let posts: BoardPost[] = [];
  let loaded = false;
  let failed = false;
  let alive = true;
  let timer: number | undefined;
  let authorCid = '';
  let query = '';
  let pendingFocus: string | null = null;

  const nameOf = (p: BoardPost): string => {
    if (p.owner) return 'Owner';
    return agents.get().filter((a) => a.cid === p.agent_cid).map(agentName)[0] ?? agentLabel(p.agent_cid);
  };

  // A comment's author: the agent's name, or "You (owner)" for the owner's own.
  const commenter = (c: BoardComment): string => {
    if (c.agent_cid === null) return 'You (owner)';
    const agent = agents.get().find((a) => a.cid === c.agent_cid);
    return agent ? agentName(agent) : c.author || agentLabel(c.agent_cid);
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

  // A body that overflows its clamp, or is open, marks its "Show more" toggle (the CSS shows it).
  const fit = (): void => {
    for (const body of feed.querySelectorAll<HTMLElement>('.bd-body')) {
      if (body.classList.contains('open') || body.scrollHeight > body.clientHeight + 1) body.dataset.over = '1';
      else delete body.dataset.over;
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
      const view = await api.boardView(inProject);
      posts = view.posts;
      owner.setPaused(view.paused);
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
  const offs: (() => void)[] = [on('board', refetch), on('board_paused', refetch), agents.watch(() => render())];
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
  const body = extra && extra !== p.subject.trim() ? h('div.bd-body.md.clamp', null, renderMarkdown(extra, { embeds: embedContextFromStore({ project: p.project ?? null }) })) : null;
  const show = (open: boolean): void => {
    if (!body) return;
    body.classList.toggle('open', open);
    body.classList.toggle('clamp', !open);
    more.textContent = open ? 'Show less' : 'Show more';
  };
  // The open state lives in `ui.opened`, so a redraw (any board event) keeps it.
  const more: HTMLButtonElement = h(
    'button.btn.ghost.sm.bd-more',
    {
      type: 'button',
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
  const mentions = (p.mentions ?? []).map((m) => h('span.chip', { class: m === 'all' ? 't-accent' : 't-muted' }, `@${m}`));
  return h(
    'article.bd-post',
    { 'data-id': p.id },
    h(
      'div.bd-top',
      null,
      h('span.num.muted', null, `p${no}`),
      h('span', null, p.owner ? name : h('a', { href: href('agents', p.agent_cid) }, name)),
      p.owner ? null : h('span.muted', null, roleLabel(p.role)),
      mentions.length ? h('span', null, ...mentions.flatMap((m, i) => (i ? [' ', m] : [m]))) : null,
      p.task ? h('span', null, h('a', { href: href('tasks', p.task) }, `Task ${p.task}`)) : null,
      h('span', null, countLabel(commentsOf(p).length)),
      h('span.bd-time', { title: new Date(p.created_at).toLocaleString('en-US') }, ago(p.created_at)),
    ),
    h('h3', null, p.subject),
    body,
    body ? more : null,
    attachmentsOf(p.id, p.attachments),
    thread(p, ui),
    h('div.bd-foot', null, replyLink(p, ui)),
    ui.composing.has(p.id) ? composer(p, ui) : null,
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
    'div.bd-atts',
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
  const slot = h('span.bd-slot', null, h('a.bd-att', { href: url, target: '_blank', rel: 'noopener' }, h('span.hint', null, 'file …')));
  void metaOf(id).then((d) => {
    if (!d) {
      slot.replaceChildren(h('span.chip.t-muted', null, 'File unavailable'));
      fileNodes.forEach((node, key) => node === slot && fileNodes.delete(key));
      return;
    }
    const name = d.title || d.name;
    if (d.content_type.startsWith('image/')) {
      slot.replaceChildren(
        h('a.bd-att', { href: url, target: '_blank', rel: 'noopener' }, h('img.bd-thumb', { src: url, loading: 'lazy', alt: name }), h('span.hint', null, `${name} · ${sizeOf(d.size)}`)),
      );
    } else if (d.content_type.startsWith('video/')) {
      const v = h('video.bd-thumb', { src: url, preload: 'metadata', 'aria-label': name }) as HTMLVideoElement;
      v.controls = true;
      slot.replaceChildren(h('figure.bd-att', null, v, h('figcaption.hint', null, `${name} · ${sizeOf(d.size)}`)));
    } else {
      slot.replaceChildren(h('a.chip.t-muted', { href: url, target: '_blank', rel: 'noopener' }, `${name} · ${sizeOf(d.size)}`));
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
    'button.btn.ghost.sm',
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

/** The thread under a post: its last comments (or all, on request). */
function thread(p: BoardPost, ui: Ui): HTMLElement | null {
  const all = commentsOf(p);
  if (!all.length) return null;
  const expanded = ui.expanded.has(p.id);
  const shown = all.length > THREAD_PREVIEW && !expanded ? all.slice(-THREAD_PREVIEW) : all;
  return h(
    'div.bd-thread',
    null,
    all.length > THREAD_PREVIEW
      ? h(
          'div',
          null,
          h(
            'button.btn.ghost.sm',
            {
              type: 'button',
              'aria-expanded': expanded ? 'true' : 'false',
              onclick: () => {
                if (!ui.expanded.delete(p.id)) ui.expanded.add(p.id);
                ui.rerender();
              },
            },
            expanded ? 'Show fewer' : `Show all ${all.length} comments`,
          ),
        )
      : null,
    shown.map((c) => commentRow(c, ui, p.project ?? null)),
  );
}

function commentRow(c: BoardComment, ui: Ui, project: string | null): HTMLElement {
  const name = ui.commenter(c);
  return h(
    'div.bd-c',
    { class: c.agent_cid === null ? 'owner' : '', 'data-key': c.id },
    h('span.bd-chip', { 'aria-hidden': 'true' }, name.charAt(0).toUpperCase() || '?'),
    h(
      'div',
      null,
      h('div.bd-cm', null, h('b', null, name), h('span', null, ago(c.created_at))),
      h('div.bd-md', null, renderMarkdown(c.body, { embeds: embedContextFromStore({ project }) })),
      attachmentsOf(c.id, c.attachments),
    ),
  );
}

/** The reply composer. Its draft lives in `ui.drafts`, so a redraw never loses it. */
function composer(p: BoardPost, ui: Ui): HTMLElement {
  const ta = h('textarea.textarea', {
    'data-post': p.id,
    rows: 3,
    maxlength: REPLY_MAX,
    placeholder: 'Reply as the owner. Ctrl+Enter sends.',
    'aria-label': 'Reply to this post',
  });
  ta.value = ui.drafts.get(p.id) ?? '';
  const meter = h('span.bd-meter', null);
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
  return h('div.bd-reply', null, ta, h('div.bd-foot', null, meter, cancelBtn, sendBtn));
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
  for (const ta of feed.querySelectorAll<HTMLTextAreaElement>('textarea[data-post]')) {
    if (ta.dataset.post !== focus.post) continue;
    const start = focus.start < 0 ? ta.value.length : focus.start;
    ta.focus();
    ta.setSelectionRange(start, focus.start < 0 ? start : focus.end);
    return;
  }
}
