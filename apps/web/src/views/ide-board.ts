// The Board tab of the IDE's right panel: the agents' shared posts, newest first. The markup is the prototype's board
// (loom: m-ide-more's Board): a head with the count and the project, the owner's Pause and Export, the posts with their
// comments and a reply box under each, and the message box at the foot.
// For a project it shows the posts whose task is one of the project's tasks; with no project it shows them all. Each
// post carries its comments and a reply box (a reply is a comment on the post). The panel loads on its first show and
// refreshes every 15 seconds while it is shown; a board event refreshes it sooner.

import { api, type BoardComment, type BoardPost, type Project } from '../api';
import { roleLabel } from '../roles';
import { on } from '../store';
import { ago, button, empty, toast } from '../ui/components';
import type { EmbedContext } from '../ui/embeds';
import { h, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import { authorHue, mentionsOf } from '../ide/view';
import { boardOwnerBar, composer as ownerComposer } from '../board/owner';

const REFRESH_MS = 15_000;
const DEBOUNCE_MS = 300;
const REPLY_MAX = 1000;

export interface BoardPanel {
  el: HTMLElement;
  show(): void;
  dispose(): void;
}

export function boardPanel(opts: { project: Project | null; onCount(n: number): void; embeds: () => EmbedContext }): BoardPanel {
  const count = h('span.muted', null);
  const feed = h('div.idem-feed', null);
  // The owner's controls: Pause/Resume and Export for this project's board, the paused banner, and the
  // composer at the foot. A project's board is the hub's view of its posts (its own, and its tasks').
  const owner = boardOwnerBar({ project: () => opts.project?.id, onChanged: () => void load() });
  const post = ownerComposer({
    placeholder: 'Message the team… (@ to mention)',
    label: 'Message the team',
    project: () => opts.project?.id,
    onPosted: () => void load(),
  });
  const el = h(
    'section.idem-panel',
    { 'aria-label': 'Board' },
    h('div.idem-panel-h', null, h('span.idem-h', null, 'Board'), count),
    h('div.view-sub', null, opts.project?.title ?? 'All projects'),
    h('div.view-sub', null, 'Replies are comments on a post.'),
    h('div.idem-owner', null, owner.actions),
    owner.banner,
    feed,
    h('div.board-owner-dock', null, post.el),
  );

  let posts: BoardPost[] = [];
  let loaded = false;
  let failed = false;
  let started = false;
  let alive = true;
  let timer: number | undefined;
  let debounce: number | undefined;
  /** Unsent reply text, by post. It survives a redraw. */
  const drafts = new Map<string, string>();
  /** Posts with a reply on its way. */
  const sending = new Set<string>();
  /** Attachment names, by file id, once looked up. */
  const names = new Map<string, string>();
  const pending = new Set<string>();

  // The posts this panel shows, newest first: the hub already filtered them to the project.
  const visible = (): BoardPost[] => [...posts].sort((x, y) => y.created_at - x.created_at);

  const render = (): void => {
    if (!alive || !loaded) return;
    // Keep the caret in a reply being typed in, across the redraw.
    const active = document.activeElement;
    const typing = active instanceof HTMLTextAreaElement && feed.contains(active) ? active : null;
    const typingPost = typing?.dataset.post;
    const caret: [number | null, number | null] | null = typing ? [typing.selectionStart, typing.selectionEnd] : null;

    const list = visible();
    count.textContent = failed ? '' : `${list.length} ${list.length === 1 ? 'post' : 'posts'}`;
    if (failed) return replace(feed, empty('alert', 'Could not load the board.'));
    if (!list.length) {
      return replace(feed, empty('board', 'No posts for this project yet', 'Agents post here when they share work.'));
    }
    replace(feed, list.map(postCard));
    for (const ta of feed.querySelectorAll<HTMLTextAreaElement>('textarea[data-post]')) {
      grow(ta);
      if (caret && ta.dataset.post === typingPost) {
        ta.focus();
        ta.setSelectionRange(caret[0], caret[1]);
      }
    }
  };

  const load = async (): Promise<void> => {
    try {
      const view = await api.boardView(opts.project?.id);
      posts = view.posts;
      owner.setPaused(view.paused);
      failed = false;
    } catch {
      failed = true;
    }
    loaded = true;
    if (!alive) return;
    opts.onCount(failed ? 0 : visible().length);
    render();
  };

  // A board event (a new post or comment) refreshes the panel soon, once it has been shown.
  const refetch = (): void => {
    if (!started) return;
    window.clearTimeout(debounce);
    debounce = window.setTimeout(() => void load(), DEBOUNCE_MS);
  };
  const offs = [on('board', refetch), on('board_paused', refetch)];

  // An attachment's name: its title or file name, else its id. Each id is looked up once,
  // and the chips already on screen are filled in when the name arrives.
  const fileLabel = (id: string): string => {
    const known = names.get(id);
    if (known !== undefined) return known;
    if (!pending.has(id)) {
      pending.add(id);
      api
        .file(id)
        .then((d) => {
          names.set(id, (d && (d.title || d.name)) || id);
        })
        .catch(() => {
          names.set(id, id);
        })
        .finally(() => {
          pending.delete(id);
          if (!alive) return;
          for (const chip of feed.querySelectorAll<HTMLElement>('[data-file]')) {
            if (chip.dataset.file === id) chip.textContent = names.get(id) ?? id;
          }
        });
    }
    return id;
  };

  const attachments = (ids: string[] | undefined): HTMLElement | null =>
    ids && ids.length
      ? h(
          'div.idem-attach',
          null,
          ids.map((id) => h('span.chip', { 'data-file': id, title: id }, fileLabel(id))),
        )
      : null;

  // Markdown text, with its @mentions marked.
  const richText = (cls: string, text: string): HTMLElement => {
    const el = h('div', { class: cls }, renderMarkdown(text, { embeds: opts.embeds() }));
    markMentions(el, text);
    return el;
  };

  const byTime = <T extends { created_at: number }>(x: T, y: T): number => x.created_at - y.created_at;

  const commentRow = (c: BoardComment): HTMLElement =>
    h(
      'li',
      { 'data-key': c.id },
      h('strong', null, c.agent_cid === null ? 'You (owner)' : c.author || `#${c.agent_cid}`),
      ' ',
      h('span.muted', null, ago(c.created_at)),
      richText('idem-body', c.body),
      attachments(c.attachments),
    );

  // The reply box under a post. A reply is posted as a comment on the post.
  const composer = (p: BoardPost): HTMLElement => {
    const ta = h('textarea', {
      rows: 1,
      maxlength: REPLY_MAX,
      placeholder: 'Reply to this post (@all for everyone)',
      'aria-label': 'Reply to this post',
      'data-post': p.id,
    });
    ta.value = drafts.get(p.id) ?? '';
    const send = button('Comment', () => void submit(), { kind: 'primary', size: 'sm' });
    const sync = (): void => {
      send.disabled = !ta.value.trim() || sending.has(p.id);
    };

    const submit = async (): Promise<void> => {
      const sent = ta.value;
      const text = sent.trim();
      if (!text || text.length > REPLY_MAX || sending.has(p.id)) return;
      sending.add(p.id);
      sync();
      try {
        await api.commentOnPost(p.id, text);
        sending.delete(p.id);
        // Clear the box only if nothing was typed while the reply went out.
        if (ta.value === sent) {
          ta.value = '';
          drafts.delete(p.id);
        }
        void load();
      } catch {
        sending.delete(p.id);
        sync();
        toast('Could not post the reply.', 'error');
      }
    };

    ta.addEventListener('input', () => {
      drafts.set(p.id, ta.value);
      grow(ta);
      sync();
    });
    ta.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        void submit();
      }
    });
    send.disabled = !ta.value.trim() || sending.has(p.id);
    return h('div.idem-reply', null, ta, send);
  };

  const postCard = (p: BoardPost): HTMLElement => {
    const author = p.owner ? 'Owner' : `${roleLabel(p.role)} #${p.agent_cid}`;
    const subject = p.subject.trim();
    const body = p.body.trim();
    const bar = h('span.idem-bar-c', { 'aria-hidden': 'true' });
    bar.classList.add(`idem-hue-${authorHue(author)}`);
    const comments = [...(p.comments ?? [])].sort(byTime);
    return h(
      'article.idem-post',
      { 'data-key': p.id },
      bar,
      h(
        'div.idem-post-b',
        null,
        h(
          'div.idem-by',
          null,
          h('span', null, author),
          p.mentions?.length ? p.mentions.map((m) => h('span.idem-mention', null, `@${m}`)) : null,
          h('time', null, ago(p.created_at)),
        ),
        subject ? h('h3', null, p.subject) : null,
        body && body !== subject ? richText('idem-body', body) : null,
        attachments(p.attachments),
        comments.length ? h('ol.idem-comments', null, comments.map(commentRow)) : null,
        composer(p),
      ),
    );
  };

  const show = (): void => {
    if (!alive) return;
    if (!started) {
      started = true;
      void load();
    }
    window.clearInterval(timer);
    timer = window.setInterval(() => void load(), REFRESH_MS);
  };

  const dispose = (): void => {
    alive = false;
    window.clearInterval(timer);
    window.clearTimeout(debounce);
    offs.forEach((off) => off());
  };

  return { el, show, dispose };
}

/** Sizes a reply box to its text: one row when empty, growing as lines are added. */
function grow(ta: HTMLTextAreaElement): void {
  ta.style.height = 'auto';
  ta.style.height = `${ta.scrollHeight}px`;
}

const escapeRe = (s: string): string => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** Wraps each @mention of `text` that appears in the element's text (not in code or pre) in a span.mention. */
function markMentions(root: HTMLElement, text: string): void {
  const found = [...new Set(mentionsOf(text).map((m) => (m.startsWith('@') ? m : `@${m}`)))];
  if (!found.length) return;
  found.sort((a, b) => b.length - a.length);
  const pattern = new RegExp(`(${found.map(escapeRe).join('|')})(?![\\w-])`);
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    // A reference rendered as a chip already says what it is: the mention is not marked again inside it.
    acceptNode: (node) => (node.parentElement?.closest('code, pre, .embed-chip') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
  });
  const nodes: Text[] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) nodes.push(node as Text);
  for (const node of nodes) {
    // split with a capture group: odd entries are the mentions themselves.
    const parts = node.data.split(pattern);
    if (parts.length < 3) continue;
    const frag = document.createDocumentFragment();
    parts.forEach((part, i) => {
      if (!part) return;
      frag.append(i % 2 ? h('span.idem-mention', null, part) : part);
    });
    node.replaceWith(frag);
  }
}
