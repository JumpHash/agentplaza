// Projects: every goal's flow from brief to delivery (brief → research → proposal →
// critique → approval → implementation → review → delivered), with its documents,
// critiques, approval and tasks; the owner approves or sends a project back.

import '../styles/projects.css';
import { api, PROJECT_STAGES, type Project, type ProjectCritique, type ProjectDoc, type ProjectStage } from '../api';
import { href } from '../router';
import { openIde } from '../ide/open';
import { on } from '../store';
import { ago, attempt, badge, button, card, empty, field, modal, select, textarea, toast, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import { renderMarkdown } from '../ui/markdown';
import type { Cleanup } from './index';

const DEBOUNCE_MS = 300;
const VERDICT_TONE: Record<ProjectCritique['verdict'], Tone> = { approve: 'ok', revise: 'danger', comment: 'muted' };
const APPROVABLE: ProjectStage[] = ['critique', 'approval'];

const label = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);
const stageIndex = (s: ProjectStage): number => PROJECT_STAGES.indexOf(s);
/** Orders documents by their stage; a kind that is not a stage name keeps its first-seen place at the end. */
const rank = (kind: string): number => {
  const i = PROJECT_STAGES.indexOf(kind as ProjectStage);
  return i < 0 ? PROJECT_STAGES.length : i;
};

/** The eight segments, filled up to the current stage, with the stage name beside them. */
function track(stage: ProjectStage): HTMLElement {
  const idx = stageIndex(stage);
  return h(
    'div.pj-track',
    null,
    h(
      'span.pj-segs',
      { 'aria-hidden': 'true' },
      PROJECT_STAGES.map((s, i) => h('span.pj-seg', { class: i === idx ? 'current' : i < idx ? 'filled' : null, title: label(s) })),
    ),
    h('span.pj-stage-name', null, label(stage)),
  );
}

function projectRow(p: Project): HTMLElement {
  return h(
    'a.pj-row',
    { href: href('projects', p.id), 'data-key': p.id },
    h(
      'div.pj-row-main',
      null,
      h('div.pj-row-title', null, h('span.pj-title', null, p.title), badge(p.kind, 'muted')),
      track(p.stage),
      h('div.pj-meta', null, `rev ${p.revision} · ${p.docs.length} docs · ${p.critiques.length} critiques · ${p.tasks.length} tasks`),
    ),
    h('div.pj-row-side', null, h('span.pj-ago', null, `updated ${ago(p.updated_ms)}`)),
  );
}

/** The big pipeline on the detail page: done steps are ticked, the current one is highlighted, later ones faint. */
function pipeline(stage: ProjectStage): HTMLElement {
  const idx = stageIndex(stage);
  return h(
    'ol.pj-pipeline',
    null,
    PROJECT_STAGES.map((s, i) =>
      h(
        'li',
        { class: i < idx ? 'done' : i === idx ? 'current' : 'future', 'aria-current': i === idx ? 'step' : null },
        h('span.pj-step-mark', null, i < idx ? '✓' : String(i + 1)),
        h('span.pj-step-name', null, label(s)),
      ),
    ),
  );
}

function docItem(d: ProjectDoc): HTMLElement {
  const commit = d.commit ? ` @ ${d.commit.slice(0, 7)}` : '';
  return h(
    'article.pj-doc',
    null,
    h('div.pj-doc-head', null, h('span.pj-doc-title', null, d.title), badge(d.kind, 'muted')),
    h('div.pj-meta', null, `by ${d.by} · ${ago(d.at_ms)}`),
    d.summary ? h('div.pj-doc-summary', null, renderMarkdown(d.summary)) : null,
    h(
      'div.pj-doc-links',
      null,
      d.file ? h('a.pj-link', { href: api.fileUrl(d.file), target: '_blank', rel: 'noopener' }, 'Open') : null,
      d.repo_path ? h('span.pj-path', null, `${d.repo_path}${commit}`) : null,
    ),
  );
}

function docsBody(p: Project): Node {
  if (!p.docs.length) return h('p.pj-none', null, 'No documents yet.');
  const groups = new Map<string, ProjectDoc[]>();
  for (const d of p.docs) groups.set(d.kind, [...(groups.get(d.kind) ?? []), d]);
  const kinds = Array.from(groups.keys()).sort((a, b) => rank(a) - rank(b));
  return h(
    'div.pj-docs',
    null,
    kinds.map((k) => h('section.pj-doc-group', null, h('h3.pj-group', null, label(k)), (groups.get(k) ?? []).map(docItem))),
  );
}

function critiquesBody(p: Project): Node {
  if (!p.critiques.length) return h('p.pj-none', null, 'No critiques yet.');
  return h(
    'div.pj-crits',
    null,
    p.critiques.map((c) =>
      h(
        'article.pj-crit',
        null,
        h(
          'div.pj-crit-head',
          null,
          badge(c.verdict, VERDICT_TONE[c.verdict]),
          h('span', null, `by ${c.by}`),
          h('span.pj-muted', null, `revision ${c.revision}`),
          h('span.pj-muted', null, ago(c.at_ms)),
        ),
        c.text ? h('div.pj-crit-text', null, renderMarkdown(c.text)) : null,
      ),
    ),
  );
}

function tasksBody(p: Project): Node {
  if (!p.tasks.length) return h('p.pj-none', null, 'No tasks yet.');
  return h('ul.pj-tasks', null, p.tasks.map((t) => h('li', null, h('a.pj-link', { href: href('tasks', t) }, t))));
}

function historyBody(p: Project): Node {
  if (!p.history.length) return h('p.pj-none', null, 'Nothing recorded yet.');
  const events = [...p.history].sort((a, b) => a.at_ms - b.at_ms);
  return h(
    'ol.pj-history',
    null,
    events.map((e) =>
      h(
        'li',
        null,
        h('span.pj-ago', null, ago(e.at_ms)),
        h('strong', null, e.by),
        h('span', null, e.what),
        e.note ? h('span.pj-muted', null, `— ${e.note}`) : null,
      ),
    ),
  );
}

/** The owner's approval of the plan, with an optional note. */
function openApprove(p: Project, done: () => void): void {
  const note = textarea({ rows: 3, placeholder: 'A note for the team (optional)' });
  const submit = button(
    'Approve plan',
    () => {
      void (async () => {
        const ok = await attempt(() => api.approveProject(p.id, note.value.trim()), 'Plan approved');
        if (ok) {
          close();
          done();
        }
      })();
    },
    { kind: 'primary' },
  );
  const close = modal('Approve plan', h('div.pj-form', null, field('Note', note), h('div.pj-form-actions', null, submit)));
}

/** Sends the project back to an earlier stage, with the reason the team must act on. */
function openSendBack(p: Project, done: () => void): void {
  const earlier = PROJECT_STAGES.slice(0, stageIndex(p.stage));
  const stage = select(
    earlier.map((s) => ({ value: s, label: label(s) })),
    earlier[earlier.length - 1],
  );
  const reason = textarea({ rows: 4, placeholder: 'What must change, and why' });
  const submit = button(
    'Send back',
    () => {
      const why = reason.value.trim();
      if (!why) {
        toast('Give a reason so the team knows what to change', 'error');
        return;
      }
      void (async () => {
        const ok = await attempt(() => api.vetoProject(p.id, stage.value as ProjectStage, why), 'Sent back');
        if (ok) {
          close();
          done();
        }
      })();
    },
    { kind: 'danger' },
  );
  const close = modal(
    'Send back',
    h(
      'div.pj-form',
      null,
      field('Back to', stage, 'The stage the project returns to'),
      field('Reason', reason),
      h('div.pj-form-actions', null, submit),
    ),
  );
}

/** Opens a project's repository in the IDE at its default branch (the branch the repo list reports). */
async function openRepo(name: string): Promise<void> {
  const repos = await attempt(() => api.gitRepos());
  if (!repos) return;
  const repo = repos.find((r) => r.name === name);
  if (!repo) {
    toast(`The repository ${name} was not found`, 'error');
    return;
  }
  openIde(`repo:${name}@${repo.default_branch}`);
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const id = params[0] || '';
  const screen = h('div.projects', null);
  root.append(screen);

  let alive = true;
  let loaded = false;
  let timer = 0;
  let list: Project[] = [];
  let current: Project | null = null;

  const listNodes = (): Node[] => [
    h(
      'div.view-head',
      null,
      h('h1', null, 'Projects'),
      h('p.pj-intro', null, 'Every goal goes through brief, research, proposal, critique and approval before anything is built.'),
    ),
    list.length
      ? h('div.pj-list', null, list.map(projectRow))
      : empty('projects','No projects yet.', 'The orchestrator opens one for each goal you give it.'),
  ];

  const detailNodes = (p: Project): Node[] => {
    const approvable = APPROVABLE.includes(p.stage);
    const canSendBack = stageIndex(p.stage) > 0;
    const refresh = (): void => void reload();
    const actions: Node[] =
      approvable || canSendBack
        ? [
            h(
              'div.pj-actions',
              null,
              approvable ? button('Approve plan', () => openApprove(p, refresh), { kind: 'primary' }) : null,
              canSendBack ? button('Send back…', () => openSendBack(p, refresh)) : null,
            ),
          ]
        : [];
    const sections: Node[] = [card('Documents', docsBody(p)), card('Critiques', critiquesBody(p)), card('Tasks', tasksBody(p))];
    if (p.repo) {
      const name = p.repo.slice(p.repo.lastIndexOf('/') + 1);
      sections.push(
        card(
          'Repository',
          h('div', { style: 'display: flex; gap: 12px; align-items: center; flex-wrap: wrap' }, h('a.pj-link', { href: href('git', name) }, p.repo), button('Open repo', () => void openRepo(name), { size: 'sm', icon: 'square-code', title: 'Open the repository in the IDE, at its default branch' })),
        ),
      );
    }
    sections.push(card('History', historyBody(p)));
    return [
      h('div.pj-back-row', null, h('a.pj-back', { href: href('projects') }, '← All projects')),
      h(
        'div.view-head',
        null,
        h('h1', null, p.title),
        h(
          'div.pj-view-badges',
          null,
          badge(p.kind, 'muted'),
          badge(label(p.stage), 'accent'),
          p.approved_by ? h('span.pj-muted', null, `approved by ${p.approved_by}`) : null,
        ),
      ),
      h('p.pj-goal', null, p.goal),
      pipeline(p.stage),
      ...actions,
      h('div.pj-sections', null, sections),
    ];
  };

  const draw = (): void => {
    if (!loaded) return;
    if (!id) {
      replace(screen, listNodes());
      return;
    }
    if (!current) {
      replace(
        screen,
        h('div.pj-back-row', null, h('a.pj-back', { href: href('projects') }, '← All projects')),
        empty('projects','Project not found', 'It may have been removed.'),
      );
      return;
    }
    replace(screen, detailNodes(current));
  };

  const reload = async (): Promise<void> => {
    if (id) {
      const p = await attempt(() => api.project(id));
      if (!alive) return;
      if (p) current = p;
    } else {
      const ps = await attempt(() => api.projects());
      if (!alive) return;
      if (ps) list = ps;
    }
    loaded = true;
    draw();
  };

  const schedule = (): void => {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => void reload(), DEBOUNCE_MS);
  };

  // Project and task events bring new documents, critiques and stages; the redraw is a fresh read.
  const offs: (() => void)[] = [on('project', schedule), on('task', schedule)];
  void reload();

  return () => {
    alive = false;
    window.clearTimeout(timer);
    offs.forEach((off) => off());
    screen.remove();
  };
}
