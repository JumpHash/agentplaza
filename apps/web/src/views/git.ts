// Git: the plaza's repositories on its git host — code, commits, branches and pull
// requests with their reviews; the owner reviews and merges here.

import '../styles/git.css';
import { api, type GitBranch, type GitComment, type GitCommit, type GitEntry, type GitFile, type GitPull, type GitPullDetail, type GitRepo } from '../api';
import { href } from '../router';
import { openIde } from '../ide/open';
import { ago, attempt, badge, button, card, empty, field, input, modal, select, tabs, textarea, toast } from '../ui/components';
import { embedContextFromStore } from '../ui/embeds';
import { append, h, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import type { Cleanup } from './index';

/** The rich-content context of a repository's pages: `PR #N` there is a pull request on the plaza's git host. The
 * organisation is the forge's (apid's ORG). */
const richOf = (c: Ctx) => embedContextFromStore({ project_repo: `agentplaza/${c.repo}` });

type PullState = 'open' | 'closed' | 'all';
type ReviewEvent = 'APPROVE' | 'REQUEST_CHANGES' | 'COMMENT';
type MergeStyle = 'merge' | 'squash' | 'rebase';

/** The pull request list's page size: the hub's maximum. */
const PULL_LIMIT = 50;

/** One mounted view: its route, the live flag, and the state that survives a redraw. */
interface Ctx {
  root: HTMLElement;
  repo: string;
  tab: string;
  num: string;
  live: () => boolean;
  redraw: () => void;
  code: { ref: string; path: string; file: string };
  commitRef: string;
  pulls: PullState;
}

const TABS = [
  { key: 'code', label: 'Code' },
  { key: 'commits', label: 'Commits' },
  { key: 'branches', label: 'Branches' },
  { key: 'pulls', label: 'Pull requests' },
];

// contract: W1b — params[0] = repo name (optional), params[1] = tab, params[2] = PR number
export function mount(root: HTMLElement, params: string[]): Cleanup {
  let live = true;
  const ctx: Ctx = {
    root,
    repo: params[0] ?? '',
    tab: TABS.find((t) => t.key === params[1])?.key ?? 'code',
    num: params[2] ?? '',
    live: () => live,
    redraw: () => void draw(ctx),
    code: { ref: '', path: '', file: '' },
    commitRef: '',
    pulls: 'open',
  };
  void draw(ctx);
  return () => {
    live = false;
  };
}

async function draw(c: Ctx): Promise<void> {
  replace(c.root, wrap(head('Git'), h('p.git-note', null, 'Loading…')));
  // Only the status call says the host is offline; any other failure is shown as itself.
  let git: { configured: boolean; org: string };
  try {
    git = await api.git();
  } catch (err) {
    if (c.live()) {
      const offline = empty('git', 'The git host is offline', 'Start the Forgejo container to browse repositories.');
      offline.title = errMsg(err);
      replace(c.root, wrap(head('Git'), offline));
    }
    return;
  }
  try {
    if (!c.live()) return;
    if (!git.configured) {
      replace(c.root, wrap(head('Git'), empty('git', 'The git host is not configured.')));
      return;
    }
    if (!c.repo) await reposView(c);
    else if (c.tab === 'pulls' && c.num) await pullView(c, Number(c.num));
    else await repoView(c, git.org);
  } catch (err) {
    if (c.live()) replace(c.root, wrap(head('Git'), empty('git', errMsg(err))));
  }
}

// ---- Repositories ----------------------------------------------------------

async function reposView(c: Ctx): Promise<void> {
  const repos = await api.gitRepos();
  if (!c.live()) return;
  const create = button('New repository', () => newRepoModal(), { kind: 'primary' });
  const body = repos.length
    ? h('div.git-list', null, repos.map(repoRow))
    : empty('git', 'No repositories yet', 'Create one for the agents to clone and push to.');
  replace(c.root, wrap(head('Git', create), body));
}

function newRepoModal(): void {
  const name = input({ placeholder: 'repo-name', autocomplete: 'off' });
  const desc = textarea({ rows: 3, placeholder: 'What this repository is for' });
  const create = button('Create', async () => {
    const value = name.value.trim();
    if (!value) {
      toast('Name the repository', 'error');
      return;
    }
    const made = await attempt(() => api.gitCreateRepo(value, desc.value.trim()), 'Repository created');
    if (made) {
      close();
      location.hash = href('git', made.name);
    }
  }, { kind: 'primary' });
  const cancel = button('Cancel', () => close(), { kind: 'ghost' });
  const close = modal('New repository', h('div.git-form', null, field('Name', name), field('Description', desc), h('div.git-actions', null, cancel, create)));
}

function repoRow(r: GitRepo): HTMLElement {
  return h('div.git-row', { 'data-key': r.name },
    h('a.git-row-main', { href: href('git', r.name) },
      h('strong', null, r.name),
      h('span.git-desc', null, r.description || 'No description'),
    ),
    h('div.git-row-meta', null,
      h('span.git-mono', null, r.default_branch),
      h('span', null, `${r.open_prs} open ${r.open_prs === 1 ? 'pull request' : 'pull requests'}`),
      h('span', null, `updated ${ago(Date.parse(r.updated_at))}`),
      r.project ? h('a.git-chip', { href: href('projects', r.project), title: r.project }, 'project') : null,
    ),
  );
}

// ---- One repository --------------------------------------------------------

async function repoView(c: Ctx, org: string): Promise<void> {
  const [repos, branches] = await Promise.all([api.gitRepos(), api.gitBranches(c.repo)]);
  if (!c.live()) return;
  const info = repos.find((r) => r.name === c.repo);
  if (!info) {
    replace(c.root, wrap(head(c.repo), empty('git', 'No such repository', 'It may have been removed.')));
    return;
  }
  const url = `http://plaza.gateway/git/${org}/${c.repo}.git`;
  const clone = h('div.git-clone', null,
    h('span.git-label', null, 'Clone'),
    h('code.git-url', null, url),
    button('Copy', () => void copyText(url), { kind: 'ghost', size: 'sm' }),
  );
  const nav = tabs(TABS, c.tab, (key) => {
    location.hash = href('git', c.repo, key);
  });
  const body = await tabBody(c, info, branches);
  if (!c.live()) return;
  replace(c.root, wrap(head(c.repo), clone, nav, body));
}

async function tabBody(c: Ctx, info: GitRepo, branches: GitBranch[]): Promise<Node> {
  if (c.tab === 'commits') return commitsView(c, info, branches);
  if (c.tab === 'branches') return branchesView(branches);
  if (c.tab === 'pulls') return pullsView(c, info, branches);
  return codeView(c, info, branches);
}

/** A branch picker over a list: its change redraws the view with the chosen branch. */
function branchPicker(names: string[], current: string, onPick: (name: string) => void): HTMLElement {
  const picker = select(names.map((n) => ({ value: n, label: n })), current);
  picker.addEventListener('change', () => onPick(picker.value));
  return h('div.git-toolbar', null, h('label.git-branch', null, 'Branch', picker));
}

async function commitsView(c: Ctx, info: GitRepo, branches: GitBranch[]): Promise<Node> {
  const names = branches.map((b) => b.name);
  if (!names.length) return empty('git', 'No commits yet');
  if (!names.includes(c.commitRef)) c.commitRef = names.includes(info.default_branch) ? info.default_branch : (names[0] ?? '');
  const toolbar = branchPicker(names, c.commitRef, (name) => {
    c.commitRef = name;
    c.redraw();
  });
  try {
    const commits = await api.gitCommits(c.repo, c.commitRef);
    return h('div', null, toolbar, commits.length ? h('div.git-list', null, commits.map(commitRow)) : empty('git', 'No commits yet'));
  } catch (err) {
    return h('div', null, toolbar, empty('git', errMsg(err)));
  }
}

function codeView(c: Ctx, info: GitRepo, branches: GitBranch[]): Node {
  const names = branches.map((b) => b.name);
  if (!names.length) return empty('git', 'Nothing pushed yet', 'This repository has no branches.');
  const st = c.code;
  if (!names.includes(st.ref)) st.ref = names.includes(info.default_branch) ? info.default_branch : (names[0] ?? '');

  const box = h('div.git-code', null, h('p.git-note', null, 'Loading…'));
  const go = (path: string): void => {
    st.path = path;
    st.file = '';
    void paint();
  };
  const paint = async (): Promise<void> => {
    // The picker is kept on a failed load, so another branch can still be chosen.
    const toolbar = branchPicker(names, st.ref, (name) => {
      st.ref = name;
      st.path = '';
      st.file = '';
      void paint();
    });
    try {
      const tree = await api.gitTree(c.repo, st.ref, st.path);
      const below: Node[] = [];
      if (st.file) {
        const f = await api.gitFile(c.repo, st.ref, st.file);
        const file = st.file;
        // The file at this branch, in the IDE: the repo workspace at the branch, opened on the file.
        below.push(fileBlock(file, f, () => openIde(`repo:${c.repo}@${st.ref}`, { path: file })));
      } else {
        const readme = tree.find((e) => e.kind === 'file' && e.name === 'README.md');
        if (readme) {
          const rf = await api.gitFile(c.repo, st.ref, readme.path);
          if (rf.text !== null) {
            below.push(h('section.git-filebox', null, h('div.git-filehead', null, h('code', null, 'README.md')), h('div.git-readme', null, rf.text)));
          }
        }
      }
      if (!c.live()) return;
      const openEntry = (e: GitEntry): void => {
        if (e.kind === 'dir') go(e.path);
        else {
          st.file = e.path;
          void paint();
        }
      };
      replace(box,
        toolbar,
        crumbs(c.repo, st.path, go),
        dirTable(tree, openEntry),
        ...below,
      );
    } catch (err) {
      if (c.live()) replace(box, toolbar, empty('git', errMsg(err)));
    }
  };
  void paint();
  return box;
}

function crumbs(repo: string, path: string, go: (path: string) => void): HTMLElement {
  const parts = path ? path.split('/') : [];
  const nodes: Node[] = [h('button.git-link', { type: 'button', onclick: () => go('') }, repo)];
  parts.forEach((part, i) => {
    const to = parts.slice(0, i + 1).join('/');
    nodes.push(h('span.git-sep', null, '/'), h('button.git-link', { type: 'button', onclick: () => go(to) }, part));
  });
  return h('nav.git-crumbs', { 'aria-label': 'Path' }, ...nodes);
}

function dirTable(tree: GitEntry[], open: (e: GitEntry) => void): HTMLElement {
  if (!tree.length) return empty('git', 'Empty directory');
  const rows = [...tree].sort((a, b) => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === 'dir' ? -1 : 1));
  return scroll(h('table.git-table', null,
    h('thead', null, h('tr', null, h('th', null, 'Name'), h('th.git-num', null, 'Size'))),
    h('tbody', null, rows.map((e) => h('tr', { 'data-key': e.path },
      h('td', null, h('button.git-link', { type: 'button', onclick: () => open(e) }, e.kind === 'dir' ? `${e.name}/` : e.name)),
      h('td.git-num', null, e.kind === 'dir' ? '—' : fmtSize(e.size)),
    ))),
  ));
}

function fileBlock(path: string, f: GitFile, onIde?: () => void): HTMLElement {
  let body: Node;
  if (f.binary) body = h('p.git-note', null, `Binary file (${f.size} bytes)`);
  else if (f.text === null) body = h('p.git-note', null, 'No preview for this file.');
  else {
    const lines = f.text.split('\n');
    if (lines.length && lines[lines.length - 1] === '') lines.pop();
    body = h('pre.git-file', null, lines.map((line, i) =>
      h('span.git-line', null, h('span.git-ln', null, String(i + 1)), h('span.git-text', null, line)),
    ));
  }
  return h('section.git-filebox', null,
    h('div.git-filehead', null, h('code', null, path), h('span.git-when', null, fmtSize(f.size)), onIde ? button('Open in IDE', onIde, { size: 'sm', icon: 'square-code', title: 'Open this file in the IDE, at this branch' }) : null),
    body,
  );
}

function commitRow(k: GitCommit): HTMLElement {
  return h('div.git-commit', { 'data-key': k.id },
    h('code.git-sha', { title: k.id }, k.short),
    h('span.git-msg', null, firstLine(k.message)),
    h('span.git-by', null, k.author),
    h('span.git-when', null, ago(Date.parse(k.at))),
    h('span.git-add', null, `+${k.additions}`),
    h('span.git-del', null, `−${k.deletions}`),
  );
}

function branchesView(branches: GitBranch[]): Node {
  if (!branches.length) return empty('git', 'No branches yet');
  return scroll(h('table.git-table', null,
    h('thead', null, h('tr', null, h('th', null, 'Branch'), h('th', null, 'Last commit'), h('th', null, 'Author'), h('th', null, 'Committed'))),
    h('tbody', null, branches.map((b) => h('tr', { 'data-key': b.name },
      h('td', null, h('code', null, b.name)),
      h('td', null, firstLine(b.commit.message)),
      h('td', null, b.commit.author),
      h('td', null, ago(Date.parse(b.commit.at))),
    ))),
  ));
}

function pullsView(c: Ctx, info: GitRepo, branches: GitBranch[]): HTMLElement {
  const list = h('div.git-list', null, h('p.git-note', null, 'Loading…'));
  const pager = h('div.git-pager', null);
  const filter = tabs(
    [{ key: 'open', label: 'Open' }, { key: 'closed', label: 'Closed' }, { key: 'all', label: 'All' }],
    c.pulls,
    (key) => {
      c.pulls = key as PullState;
      c.redraw();
    },
  );
  // Pages are appended to the list; a change of filter redraws the view from page 1.
  let page = 0;
  const more = (busy: boolean): HTMLButtonElement => button(busy ? 'Loading…' : 'Load more', () => void loadPage(), { kind: 'ghost', disabled: busy });
  const loadPage = async (): Promise<void> => {
    const next = page + 1;
    replace(pager, more(true));
    try {
      const pulls = await api.gitPulls(c.repo, c.pulls, next, PULL_LIMIT);
      if (!c.live()) return;
      page = next;
      const rows = pulls.map((p) => pullRow(c.repo, p));
      if (next === 1) replace(list, rows.length ? rows : empty('git', 'No pull requests here'));
      else append(list, rows);
      replace(pager, pulls.length === PULL_LIMIT ? more(false) : null);
    } catch (err) {
      if (!c.live()) return;
      if (next === 1) replace(list, empty('git', errMsg(err)));
      else toast(errMsg(err), 'error');
      replace(pager, page ? more(false) : null);
    }
  };
  void loadPage();
  const create = button('New pull request', () => newPullModal(c, info, branches), { kind: 'primary' });
  return h('div.git-pulls', null, filter, h('div.git-actions', null, create), list, pager);
}

/** A small form that opens a pull request from one branch into another, then opens it. */
function newPullModal(c: Ctx, info: GitRepo, branches: GitBranch[]): void {
  const names = branches.map((b) => b.name);
  const baseName = names.includes(info.default_branch) ? info.default_branch : (names[0] ?? '');
  const headName = names.find((n) => n !== baseName) ?? baseName;
  const title = input({ placeholder: 'What this change does', autocomplete: 'off' });
  const from = select(names.map((n) => ({ value: n, label: n })), headName);
  const into = select(names.map((n) => ({ value: n, label: n })), baseName);
  const desc = textarea({ rows: 6, placeholder: 'Describe the change (optional)' });
  const create = button('Open pull request', async () => {
    const value = title.value.trim();
    if (!value) {
      toast('Give the pull request a title', 'error');
      return;
    }
    if (!from.value || !into.value || from.value === into.value) {
      toast('Choose two different branches', 'error');
      return;
    }
    const made = await attempt(() => api.gitOpenPull(c.repo, value, from.value, into.value, desc.value.trim()), 'Pull request opened');
    if (made) {
      close();
      location.hash = href('git', c.repo, 'pulls', made.number);
    }
  }, { kind: 'primary' });
  const cancel = button('Cancel', () => close(), { kind: 'ghost' });
  const close = modal('New pull request', h('div.git-form', null,
    field('Title', title),
    field('Head branch', from, 'The branch with the changes'),
    field('Base branch', into, 'The branch they merge into'),
    field('Description', desc),
    h('div.git-actions', null, cancel, create),
  ));
}

function pullRow(repo: string, p: GitPull): HTMLElement {
  return h('a.git-pr', { href: href('git', repo, 'pulls', p.number), 'data-key': String(p.number) },
    h('span.git-num', null, `#${p.number}`),
    h('span.git-pr-title', null, p.title),
    h('span.git-flow', null, `${p.head} → ${p.base}`),
    h('span.git-by', null, p.author),
    reviewBadge(p.review),
    p.merged === true ? badge('merged', 'ok') : null,
    h('span.git-when', null, `${p.comments} comments`),
    h('span.git-when', null, `updated ${ago(Date.parse(p.updated_at))}`),
  );
}

// ---- One pull request ------------------------------------------------------

async function pullView(c: Ctx, n: number): Promise<void> {
  const d = await api.gitPull(c.repo, n);
  if (!c.live()) return;
  const trail = h('nav.git-crumbs', { 'aria-label': 'Path' },
    h('a', { href: href('git', c.repo) }, c.repo),
    h('span.git-sep', null, '/'),
    h('a', { href: href('git', c.repo, 'pulls') }, 'Pull requests'),
    h('span.git-sep', null, '/'),
    `#${d.number}`,
  );
  // A merged pull request is closed in Forgejo: it has no Merge, Close or Reopen.
  const merged = d.merged === true;
  const mergeable = d.state === 'open' && d.review === 'approved' && d.mergeable;
  const actions = h('div.git-actions', null,
    button('Open in IDE', () => openIde(`repo:${c.repo}@${d.head}`, { review: true }), { icon: 'square-code', title: 'Open the pull request head in the IDE, in review mode' }),
    button('Approve', () => reviewModal(c, d, 'APPROVE', 'Approve'), { kind: 'primary' }),
    button('Request changes', () => reviewModal(c, d, 'REQUEST_CHANGES', 'Request changes'), { kind: 'danger' }),
    button('Comment', () => reviewModal(c, d, 'COMMENT', 'Comment'), { kind: 'ghost' }),
    merged ? null : button('Merge', () => mergeModal(c, d), {
      kind: 'primary',
      disabled: !mergeable,
      title: mergeable ? 'Merge this pull request' : 'Needs an approving review and no conflicts',
    }),
    merged ? null : d.state === 'open'
      ? button('Close', () => void togglePull(c, d), { kind: 'danger', title: 'Close without merging' })
      : button('Reopen', () => void togglePull(c, d), { kind: 'ghost', title: 'Reopen this pull request' }),
  );
  replace(c.root, wrap(
    trail,
    head(d.title),
    h('div.git-pr-meta', null,
      h('span.git-num', null, `#${d.number}`),
      merged ? badge('merged', 'ok') : badge(d.state, d.state === 'open' ? 'ok' : 'muted'),
      reviewBadge(d.review),
      h('span.git-by', null, d.author),
      h('span.git-flow', null, `${d.head} → ${d.base}`),
      h('span.git-when', null, `updated ${ago(Date.parse(d.updated_at))}`),
      merged && d.merged_at ? h('span.git-when', null, `merged ${ago(Date.parse(d.merged_at))}`) : null,
    ),
    actions,
    card('Description', h('div.git-prose', null, d.body ? renderMarkdown(d.body, { embeds: richOf(c) }) : 'No description provided.')),
    card(`Files changed (${d.files.length})`, d.files.length ? scroll(filesTable(d.files)) : h('p.git-note', null, 'No files.')),
    card('Diff', h('pre.git-diff', null, diffLines(d.diff))),
    card(`Reviews (${d.reviews.length})`, d.reviews.length ? d.reviews.map((rv) => reviewRow(c, rv)) : h('p.git-note', null, 'No reviews yet.')),
    card(`Discussion (${d.discussion.length})`, [
      ...(d.discussion.length ? d.discussion.map((cm) => commentRow(c, cm)) : [h('p.git-note', null, 'No comments yet.')]),
      commentBox(c, d),
    ]),
  ));
}

/** Closes an open pull request, or reopens a closed one, after a confirmation for closing. */
async function togglePull(c: Ctx, d: GitPullDetail): Promise<void> {
  const closing = d.state === 'open';
  if (closing && !window.confirm(`Close #${d.number}? It can be reopened later.`)) return;
  const changed = await attempt(
    () => api.gitSetPullState(c.repo, d.number, closing ? 'closed' : 'open'),
    closing ? `#${d.number} closed` : `#${d.number} reopened`,
  );
  if (changed) c.redraw();
}

function commentBox(c: Ctx, d: GitPullDetail): HTMLElement {
  const text = textarea({ rows: 4, placeholder: 'Write a comment (Ctrl+Enter or Cmd+Enter posts it)' });
  const post = button('Post comment', () => void send(), { kind: 'primary' });
  const send = async (): Promise<void> => {
    const body = text.value.trim();
    if (!body) {
      toast('Write something first', 'error');
      return;
    }
    post.disabled = true;
    const sent = await attempt(() => api.gitComment(c.repo, d.number, body), 'Comment posted');
    if (sent) c.redraw();
    else post.disabled = false;
  };
  text.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      void send();
    }
  });
  return h('div.git-form', null, field('Comment', text), h('div.git-actions', null, post));
}

function commentRow(c: Ctx, cm: GitComment): HTMLElement {
  return h('div.git-review', null,
    h('div.git-review-head', null, h('strong', null, cm.author), h('span.git-when', null, ago(Date.parse(cm.at)))),
    h('div.git-prose', null, renderMarkdown(cm.body, { embeds: richOf(c) })),
  );
}

function reviewModal(c: Ctx, d: GitPullDetail, event: ReviewEvent, label: string): void {
  const text = textarea({ rows: 5, placeholder: event === 'APPROVE' ? 'Optional note' : 'Write the review' });
  const send = button(label, async () => {
    const body = text.value.trim();
    if (event !== 'APPROVE' && !body) {
      toast('Write something first', 'error');
      return;
    }
    const sent = await attempt(() => api.gitReview(c.repo, d.number, event, body).then(() => true), `${label} sent`);
    if (sent) {
      close();
      c.redraw();
    }
  }, { kind: event === 'REQUEST_CHANGES' ? 'danger' : 'primary' });
  const cancel = button('Cancel', () => close(), { kind: 'ghost' });
  const close = modal(`${label} on #${d.number}`, h('div.git-form', null, field('Review', text), h('div.git-actions', null, cancel, send)));
}

function mergeModal(c: Ctx, d: GitPullDetail): void {
  const style = select([
    { value: 'merge', label: 'Merge commit' },
    { value: 'squash', label: 'Squash' },
    { value: 'rebase', label: 'Rebase' },
  ], 'merge');
  const confirm = button('Merge', async () => {
    const merged = await attempt(
      () => api.gitMerge(c.repo, d.number, style.value as MergeStyle).then(() => true),
      `#${d.number} merged`,
    );
    if (merged) {
      close();
      c.redraw();
    }
  }, { kind: 'primary' });
  const cancel = button('Cancel', () => close(), { kind: 'ghost' });
  const close = modal(`Merge #${d.number}`, h('div.git-form', null,
    h('p', null, `Merge ${d.head} into ${d.base}?`),
    field('Style', style),
    h('div.git-actions', null, cancel, confirm),
  ));
}

function reviewRow(c: Ctx, rv: GitPullDetail['reviews'][number]): HTMLElement {
  const s = rv.state.toLowerCase();
  const tone = s.includes('approv') ? 'ok' : s.includes('change') ? 'danger' : 'muted';
  return h('div.git-review', null,
    h('div.git-review-head', null, h('strong', null, rv.author), badge(s.replace(/_/g, ' '), tone), h('span.git-when', null, ago(Date.parse(rv.at)))),
    rv.body ? h('div.git-prose', null, renderMarkdown(rv.body, { embeds: richOf(c) })) : null,
  );
}

function filesTable(files: GitPullDetail['files']): HTMLElement {
  return h('table.git-table', null, h('tbody', null, files.map((f) => h('tr', null,
    h('td', null, h('code', null, f.path)),
    h('td.git-add', null, `+${f.additions}`),
    h('td.git-del', null, `−${f.deletions}`),
  ))));
}

function diffLines(diff: string): HTMLElement[] {
  return diff.split('\n').map((line) => {
    const cls = line.startsWith('@@') ? 'hunk'
      : line.startsWith('+++') || line.startsWith('---') ? 'meta'
        : line.startsWith('+') ? 'add'
          : line.startsWith('-') ? 'del' : null;
    return h('span', { class: cls }, line);
  });
}

function reviewBadge(r: GitPull['review']): HTMLElement {
  if (r === 'approved') return badge('approved', 'ok');
  if (r === 'changes_requested') return badge('changes requested', 'danger');
  return badge('pending', 'muted');
}

// ---- Helpers ---------------------------------------------------------------

function head(title: string, ...actions: Node[]): HTMLElement {
  return h('div.view-head', null, h('h1', null, title), actions.length ? h('div.git-head-actions', null, ...actions) : null);
}

function wrap(...children: Node[]): HTMLElement {
  return h('div.git', null, ...children);
}

function scroll(node: Node): HTMLElement {
  return h('div.git-scroll', null, node);
}

async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    toast('Clone URL copied');
  } catch {
    toast('Could not copy the clone URL', 'error');
  }
}

function firstLine(message: string): string {
  return message.split('\n')[0] ?? '';
}

function fmtSize(bytes: number): string {
  return bytes < 1024 ? `${bytes} B` : `${(bytes / 1024).toFixed(1)} KB`;
}

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
