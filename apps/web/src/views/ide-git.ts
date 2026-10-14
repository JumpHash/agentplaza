// The IDE's git actions in the top bar. For a repository workspace: "Open PR", a pull request from one branch into another,
// opened the way the Git view opens one (the same API). Agent workspaces have no git actions, so the slot is empty for them.
// The form's classes (git-form, git-actions) are in styles/git.css, which the Git view loads with the start bundle.
import { api, type GitBranch, type GitRepo } from '../api';
import { attempt, button, field, input, modal, select, textarea, toast } from '../ui/components';
import { h } from '../ui/dom';
import { message, refOf, repoNameOf, type IdeCtx } from './ide-ctx';

/** Paints the top bar's git slot for the selection. It is repainted only when the workspace changes. */
export function renderGit(c: IdeCtx): void {
  const host = c.dom.frame.git;
  const ws = c.ws;
  const key = ws?.kind === 'repo' ? ws.id : '';
  if (host.dataset.ws === key) return;
  host.dataset.ws = key;
  if (!ws || ws.kind !== 'repo') {
    host.replaceChildren();
    return;
  }
  const repo = repoNameOf(ws);
  const head = refOf(ws);
  host.replaceChildren(
    button('Open PR', () => void openPullModal(repo, head), { kind: 'ghost', size: 'sm', icon: 'git', title: `Open a pull request on ${repo}` }),
  );
}

/** The form: a title, the head and base branches (the workspace's branch is the head by default), and a description. */
async function openPullModal(repo: string, head: string): Promise<void> {
  let branches: GitBranch[];
  let defaultBranch = '';
  try {
    branches = await api.gitBranches(repo);
    const repos: GitRepo[] = await api.gitRepos();
    defaultBranch = repos.find((r) => r.name === repo)?.default_branch ?? '';
  } catch (err) {
    toast(message(err), 'error');
    return;
  }
  const names = branches.map((b) => b.name);
  if (names.length < 2) {
    toast('A pull request needs two branches', 'error');
    return;
  }
  const baseName = names.includes(defaultBranch) ? defaultBranch : (names[0] ?? '');
  const headName = names.includes(head) && head !== baseName ? head : (names.find((n) => n !== baseName) ?? baseName);
  const title = input({ placeholder: 'What this change does', autocomplete: 'off' });
  const from = select(names.map((n) => ({ value: n, label: n })), headName);
  const into = select(names.map((n) => ({ value: n, label: n })), baseName);
  const desc = textarea({ rows: 6, placeholder: 'Describe the change (optional)' });
  const create = button(
    'Open pull request',
    async () => {
      const value = title.value.trim();
      if (!value) {
        toast('Give the pull request a title', 'error');
        return;
      }
      if (!from.value || !into.value || from.value === into.value) {
        toast('Choose two different branches', 'error');
        return;
      }
      const made = await attempt(() => api.gitOpenPull(repo, value, from.value, into.value, desc.value.trim()), 'Pull request opened');
      if (made) close();
    },
    { kind: 'primary' },
  );
  const cancel = button('Cancel', () => close(), { kind: 'ghost' });
  const close = modal(
    'New pull request',
    h(
      'div.git-form',
      null,
      field('Title', title),
      field('Head branch', from, 'The branch with the changes'),
      field('Base branch', into, 'The branch they merge into'),
      field('Description', desc),
      h('div.git-actions', null, cancel, create),
    ),
  );
}
