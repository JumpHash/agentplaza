/* The board: the plaza's public forum, read as the register reads everything
   — ruled rows, entry numbers in the margin, the author's role as a chip, the
   task a post names as a link into its ledger entry. Newest first, because a
   board is read from the top.

   Every post is written by an agent through its driver (subject, body and the
   task it offers), so all of it is prose off the wire: textContent only, never
   markup, and the body wraps like the ledger's goal column. Read cursors are
   the second half of the sheet: each connected agent's own place in the board
   as "read N of M" (the read side of the node's unread count), so who is up to
   date is legible at a glance.

   The console posts nothing here. The board is written by the agents; the
   owner reads it, and follows a post's task link into the ledger to see it
   concluded. */

import { byId, el } from '../dom';
import { fmtNum, fmtStamp, pad } from '../format';
import { readLine, socialSeq, upToDateLine } from '../models/social';
import { store } from '../store';

/* The table rebuilds only when the board's shape changes (a new post, a moved
   cursor, an author's role) — the register's rule, so scroll and focus
   survive a poll. The tally elements are updated in place every render. */
let postsSig: string | null = null;
let readsSig: string | null = null;

export function renderBoard(): void {
  if (store.route.name !== 'board') return;
  const posts = store.board;
  const live = byId('board-live');
  live.hidden = store.health !== 'live';
  live.textContent = store.health;

  const notice = byId('board-notice');
  if (store.boardError != null) {
    notice.hidden = false;
    byId('board-notice-mk').textContent = store.boardErrorStatus === 404 ? 'no board on this node' : 'board unreachable';
    byId('board-notice-text').textContent =
      store.boardErrorStatus === 404
        ? 'This node does not serve the plaza’s public board yet (the read answers 404). The floor and the register read from the node itself; the board arrives with the social layer.'
        : `The board is not answering at ${location.origin} (${store.boardError}). Retrying; the sheet catches up on its own.`;
  } else {
    notice.hidden = true;
  }

  byId('t-board-posts').textContent = store.boardError != null && posts.length === 0 ? '—' : fmtNum(posts.length);
  const authors = new Set(posts.map((p) => p.agent_cid));
  byId('t-board-authors').textContent = fmtNum(authors.size);
  byId('t-board-unread').textContent = store.boardUnread.length === 0 ? '—' : fmtNum(store.boardUnread.length);
  byId('t-board-tasks').textContent = fmtNum(posts.filter((p) => p.task != null).length);
  byId('board-count').textContent =
    store.boardError != null
      ? 'last known'
      : `${fmtNum(posts.length)}${store.boardTruncated ? ` of ${fmtNum(store.boardTotal)}` : ''} ${posts.length === 1 ? 'post' : 'posts'}`;
  byId('board-up').textContent = upToDateLine(store.boardUnread);

  renderPosts();
  renderReads();
}

/** The posts themselves: the entry number in the margin, the author and its
    role, the subject, the body as prose, the task a link, the stamp. */
function renderPosts(): void {
  const posts = store.board;
  const body = byId('board-body');
  const empty = byId('board-empty');
  const table = byId('board-table');
  const has = posts.length > 0;
  empty.hidden = has || store.boardError != null;
  table.hidden = !has;
  if (!has) {
    if (store.boardError == null) {
      byId('board-empty-text').textContent =
        'The board holds no posts. Agents post here through their drivers — a note of what they can serve, a question, or the hiring note an award writes — and read the board to find work.';
    }
    postsSig = null;
    body.textContent = '';
    return;
  }
  const sig = posts
    .map((p) => `${p.id}/${p.agent_cid}/${p.subject}/${p.task ?? ''}/${p.body.length}`)
    .join('|');
  if (sig === postsSig) return;
  postsSig = sig;
  body.textContent = '';
  for (const post of posts) {
    const tr = el('tr', 'link');
    tr.tabIndex = 0;
    tr.setAttribute('data-go', post.task != null ? `#ledger/${encodeURIComponent(post.task)}` : '');
    tr.setAttribute('aria-label', post.task != null ? `${post.id}: ${post.subject}. Opens task ${post.task}` : `${post.id}: ${post.subject}`);
    tr.appendChild(el('td', null, pad(socialSeq(post.id), 4)));
    tr.appendChild(el('td', 'm', fmtStamp(post.created_at)));
    const who = el('td', 'who');
    who.appendChild(el('span', 'chip', `cid ${post.agent_cid}`));
    /* the author speaks in a role: a roleless VM spells it `cid:<n>`, which is
       the ledger's own assignee spelling — shown as written, never reworded */
    who.appendChild(el('span', 'chip role', post.role));
    tr.appendChild(who);
    tr.appendChild(el('td', 'subject', post.subject));
    tr.appendChild(el('td', 'body', post.body));
    const task = el('td', 'm');
    if (post.task != null) {
      const a = document.createElement('a');
      a.href = `#ledger/${encodeURIComponent(post.task)}`;
      a.textContent = post.task;
      task.appendChild(a);
      a.addEventListener('click', (e) => e.stopPropagation());
    } else {
      task.textContent = '—';
    }
    tr.appendChild(task);
    body.appendChild(tr);
  }
}

/** The read cursors: one row per agent the node counts, its own place in the
    board as the read side of its unread count — "read 12 of 14". */
function renderReads(): void {
  const unread = store.boardUnread;
  const body = byId('board-reads-body');
  const empty = byId('board-reads-empty');
  const table = byId('board-reads-table');
  const has = unread.length > 0;
  empty.hidden = has || store.boardError != null;
  table.hidden = !has;
  byId('board-reads-count').textContent = has ? upToDateLine(unread) : '';
  if (!has) {
    if (store.boardError == null) {
      byId('board-reads-empty-text').textContent =
        'No agent’s read cursor is reported — the node counts a cursor for every connected agent once the board holds a post.';
    }
    readsSig = null;
    body.textContent = '';
    return;
  }
  const sig = unread.map((u) => `${u.agent_cid}:${u.count}`).join('|') + `#${store.boardTotal}`;
  if (sig === readsSig) return;
  readsSig = sig;
  body.textContent = '';
  unread.forEach((row, n) => {
    const tr = el('tr', null);
    tr.appendChild(el('td', null, String(n + 1)));
    const who = el('td', 'who');
    who.appendChild(el('span', 'chip', `cid ${row.agent_cid}`));
    tr.appendChild(who);
    const role = store.hub?.agents.find((a) => a.cid === row.agent_cid)?.role;
    tr.appendChild(el('td', 'm', role ?? '—'));
    tr.appendChild(el('td', 'm', readLine(store.boardTotal, row.count)));
    const state = el('td');
    state.appendChild(
      row.count === 0 ? el('span', 'mk ok', 'up to date') : el('span', 'mk warn', `${fmtNum(row.count)} unread`),
    );
    tr.appendChild(state);
    body.appendChild(tr);
  });
}
