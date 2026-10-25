// Projects: the list (#/projects) and a project's page (#/projects/<id>/<tab>): its header and its pipeline from brief to
// delivery, its Plan (documents, critiques, tasks, files, history), and its Settings, Maintenance and Environments
// tabs. The owner approves, sends back, pauses, archives and changes the mode from here. The markup is the prototype's
// (styles/proto/projects.css); the data and the actions are the console's.

import '../styles/proto/projects.css';
import { api, PROJECT_STAGES, type Agent, type Project, type ProjectCritique, type ProjectDoc, type ProjectStage, type Task } from '../api';
import { href, hrefIn } from '../router';
import { openIde } from '../ide/open';
import { on, RESYNC } from '../store';
import { ago, attempt, badge, button, card, field, modal, select, textarea, toast, type Tone } from '../ui/components';
import { openNewProject } from '../ui/new-project';
import { isLive, refreshProjects } from '../scope';
import { settingsTab } from './project-settings';
import { maintenanceTab } from './project-maintenance';
import { environmentsTab } from './project-environments';
import { previewsTab } from './project-previews';
import { recipeRunsOf } from './recipe-graph';
import { projectsApi, type ModeDef, type ProjectMode } from '../api-projects';
import { dropZone, pickFiles, uploadToProject, type PickedFile } from '../ui/file-drop';
import { embedContextFromStore } from '../ui/embeds';
import { h, icon, lu, replace } from '../ui/dom';
import type { LucideName } from '../ui/icons.gen';
import { renderMarkdown } from '../ui/markdown';
import { popover, pmono } from '../ui/shell';
import { agentName } from '../roles';
import { STATE_LABEL, titleOf, workState, type WorkState } from './work-model';
import type { Cleanup } from './index';

const DEBOUNCE_MS = 300;
const FILES_TEXT = 'Drop files or a folder here, or choose them. They land on main as one change of yours; .git, node_modules and .DS_Store are left out.';
const MODE_HINT = 'Build: agents make it. Maintenance: agents watch it and propose; you pick what is done';

/** The rich-content context of one project's documents and critiques: its id (`task #N` is a task). */
const richOf = (p: Project) => embedContextFromStore({ project: p.id });
const VERDICT_TONE: Record<ProjectCritique['verdict'], Tone> = { approve: 'ok', revise: 'danger', comment: 'muted' };
const TASK_TONE: Record<WorkState, Tone> = {
  ready: 'accent',
  waiting: 'muted',
  blocked: 'danger',
  working: 'info',
  'needs-you': 'warn',
  review: 'info',
  done: 'ok',
  failed: 'danger',
  withdrawn: 'muted',
};
const APPROVABLE: ProjectStage[] = ['critique', 'approval'];
/** The tabs of a project's page that are drawn here; the others (Work, Tasks, Weft) are the console's own screens. */
const OWN_TABS = ['plan', 'maintenance', 'environments', 'previews', 'settings'];
const TAB_LINKS: [string, string][] = [
  ['plan', 'Plan'],
  ['work', 'Work'],
  ['tasks', 'Tasks'],
  ['history', 'Weft'],
  ['maintenance', 'Maintenance'],
  ['environments', 'Environments'],
  ['previews', 'Previews'],
  ['settings', 'Settings'],
];

const cap = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);
const stageIndex = (s: ProjectStage): number => PROJECT_STAGES.indexOf(s);
const modeOf = (p: Project): ProjectMode => p.mode ?? 'build';
/** Orders documents by their stage; a kind that is not a stage name keeps its first-seen place at the end. */
const rank = (kind: string): number => {
  const i = PROJECT_STAGES.indexOf(kind as ProjectStage);
  return i < 0 ? PROJECT_STAGES.length : i;
};

/** The modes, read once from the hub (their words are the hub's). */
let modes: Promise<ModeDef[]> | null = null;
const modesOnce = (): Promise<ModeDef[]> =>
  (modes ??= projectsApi.modes().catch(() => {
    modes = null;
    return [];
  }));

/** A button with a lucide icon (button() takes the console's own icon names only). */
function lbtn(label: string, onClick: (ev: MouseEvent) => void, o: { kind?: 'primary' | 'danger' | 'ghost'; size?: 'sm'; icon?: LucideName; title?: string } = {}): HTMLButtonElement {
  const b = button(label, onClick, { kind: o.kind, size: o.size, title: o.title });
  if (o.icon) b.insertBefore(lu(o.icon, o.size === 'sm' ? 14 : 15), b.firstChild);
  return b;
}

/** A dialog as the prototype draws it: the body, and the actions in the sheet's own footer. */
function dialog(title: string, body: (Node | null)[], foot: (close: () => void) => Node[]): () => void {
  const close = modal(title, h('div.pj-dlg', null, body));
  const scrims = document.querySelectorAll<HTMLElement>('.scrim');
  scrims[scrims.length - 1]?.querySelector('.sheet')?.append(h('footer.sheet-f', null, foot(close)));
  return close;
}

/** components' empty(), with its icon given as a drawn icon (empty() takes the console's own icon names only). */
function emptyState(drawn: SVGSVGElement, title: string, text?: string, action?: Node): HTMLElement {
  return h('div.empty', null, drawn, h('b', null, title), text ? h('span', null, text) : null, action ?? null);
}

/** The eight segments of a project's stage on its list row: the done ones filled, the current one marked. */
function track(stage: ProjectStage): HTMLElement {
  const at = stageIndex(stage);
  return h(
    'div.pj-track',
    { 'aria-hidden': 'true' },
    PROJECT_STAGES.map((s, i) => h('i', { class: i === at ? 'cur' : i < at ? 'done' : null, title: cap(s) })),
  );
}

/** A project's life as a badge: paused, archived or in maintenance (a delivered one says so in its stage). */
function lifeBadge(p: Project): HTMLElement | null {
  if (p.archived) return badge('Archived', 'muted');
  if (p.paused) return badge('Paused', 'warn');
  if (modeOf(p) === 'maintenance') return badge('Maintenance', 'info');
  return null;
}

/** The big pipeline on the page: done steps are ticked, the current one is highlighted, later ones faint. */
function pipeline(stage: ProjectStage): HTMLElement {
  const at = stageIndex(stage);
  return h(
    'ol.pj-pipe',
    null,
    PROJECT_STAGES.map((s, i) =>
      h(
        'li',
        { class: `pj-step ${i < at ? 'done' : i === at ? 'current' : 'future'}`, title: cap(s), 'aria-current': i === at ? 'step' : null },
        h('span', null, i < at ? lu('check', 11) : null, cap(s)),
      ),
    ),
  );
}

function projectRow(p: Project): HTMLElement {
  return h(
    'a.pj-row',
    { href: href('projects', p.id), 'data-key': p.id, class: isLive(p) ? null : 'pj-over' },
    h(
      'div.pj-row-main',
      null,
      h('div.pj-row-t', null, h('span.pj-title', { title: p.title }, p.title), badge(p.kind, 'muted'), lifeBadge(p)),
      track(p.stage),
      h('div.pj-stage-n', null, cap(p.stage)),
      h('div.pj-meta', null, `rev ${p.revision} · ${p.docs.length} docs · ${p.critiques.length} critiques · ${p.tasks.length} tasks`),
    ),
    h('div.pj-row-r', null, `updated ${ago(p.updated_ms)}`),
  );
}

// ---- The Plan tab --------------------------------------------------------------------------------------------------

/** The documents, grouped by their kind in stage order: the kinds that are not stages come last, in first-seen order. */
function docsInOrder(docs: ProjectDoc[]): ProjectDoc[] {
  const groups = new Map<string, ProjectDoc[]>();
  for (const d of docs) groups.set(d.kind, [...(groups.get(d.kind) ?? []), d]);
  return [...groups.keys()].sort((a, b) => rank(a) - rank(b)).flatMap((k) => groups.get(k) ?? []);
}

function docRow(d: ProjectDoc): HTMLElement {
  const where = d.repo_path ? `${d.repo_path}${d.commit ? ` @ ${d.commit.slice(0, 7)}` : ''}` : '';
  const open = d.file
    ? h('a.btn.ghost.sm.pj-doc-open', { href: api.fileUrl(d.file), target: '_blank', rel: 'noopener' }, 'Open', where ? [' ', h('span.mono', null, where)] : null)
    : where
      ? h('span.pj-path.mono.pj-doc-open', null, where)
      : null;
  return h(
    'div.pj-docrow',
    null,
    h('span.pj-doc-ico', { 'aria-hidden': 'true' }, lu('file-text', 16)),
    h(
      'div.pj-doc-main',
      null,
      h('div.pj-doc-t', null, h('span.pj-doc-name', null, d.title), badge(d.kind, 'muted')),
      h('div.pj-meta', null, `by ${d.by} · ${ago(d.at_ms)}`),
      d.summary ? h('div.pj-doc-sum', null, d.summary) : null,
    ),
    open,
  );
}

function critiqueRow(p: Project, c: ProjectCritique): HTMLElement {
  return h(
    'div.pj-doc',
    null,
    h(
      'div.pj-doc-h',
      null,
      badge(c.verdict, VERDICT_TONE[c.verdict]),
      h('strong', null, `by ${c.by}`),
      h('span.pj-faint', null, `revision ${c.revision}`),
      h('span.pj-faint', null, ago(c.at_ms)),
    ),
    c.text ? h('div.pj-prose', null, renderMarkdown(c.text, { embeds: richOf(p) })) : null,
  );
}

/** A task row: its id, the first line of its goal, its agent's name, and its state. The rows fill in once read. */
function taskRow(id: string, t: Task | undefined, names: Map<number, string>): HTMLElement {
  const link = href('tasks', id);
  if (!t) return h('a.pj-trow', { href: link }, h('span.mono.pj-trow-id', null, id));
  const state = workState(t);
  const who = t.agent_cid != null ? names.get(t.agent_cid) : undefined;
  return h(
    'a.pj-trow',
    { href: link },
    h('span.mono.pj-trow-id', null, id),
    h('span.pj-trow-goal', null, titleOf(t.goal)),
    who ? h('span.pj-trow-who', null, who) : null,
    badge(STATE_LABEL[state], TASK_TONE[state]),
  );
}

function tasksCard(p: Project): HTMLElement {
  const box = h('div', null, p.tasks.length ? null : h('p.muted', null, 'No tasks yet.'));
  if (p.tasks.length) {
    void Promise.all([attempt(() => api.tasksWhere({ project: p.id })), attempt(() => api.agents())]).then(([tasks, agents]) => {
      const byId = new Map<string, Task>((tasks ?? []).map((t) => [t.id, t]));
      const names = new Map<number, string>((agents ?? []).map((a: Agent) => [a.cid, agentName(a)]));
      box.replaceChildren(h('div.pj-tlist', null, p.tasks.map((id) => taskRow(id, byId.get(id), names))));
    });
  }
  return card('Tasks', [box, h('p.pj-faint', null, h('a', { href: href('work', p.id) }, 'See them in the order they run, in Work'))]);
}

/** The project's recipe runs: the recipe graphs, and the latest runs of the project. */
function recipesCard(p: Project): HTMLElement {
  return card('Recipes', recipeRunsOf({ project: p.id }));
}

function historyCard(p: Project): HTMLElement {
  if (!p.history.length) return card('History', h('p.muted', null, 'Nothing recorded yet.'));
  const events = [...p.history].sort((a, b) => a.at_ms - b.at_ms);
  return card(
    'History',
    h(
      'div.pj-events',
      null,
      events.map((e) =>
        h('div.pj-ev', null, h('span.pj-faint', null, ago(e.at_ms)), ' ', h('strong', null, e.by), ' ', e.what, e.note ? [' ', h('span.muted', null, `— ${e.note}`)] : null),
      ),
    ),
  );
}

/** The Add files card: drop files or a folder (or choose them) and they land on main as one change. */
function filesCard(p: Project): HTMLElement {
  const status = h('p.pj-intro', null, FILES_TEXT);
  const send = (files: PickedFile[]): void => {
    if (!files.length) return;
    status.textContent = `Sending ${files.length} files…`;
    void uploadToProject(p.id, files, {
      onProgress: (x) => (status.textContent = `Sending ${x.done} of ${x.total} files…`),
    }).then(
      ({ added, skipped }) => {
        const left = skipped ? `, ${skipped} left out` : '';
        status.textContent = added ? `Added ${added.files} files as change ${added.landed.change}${left}.` : `Nothing to add${left}.`;
        if (added) toast(`Added ${added.files} files to ${added.repo}`);
      },
      (err: unknown) => {
        status.textContent = `Not sent: ${err instanceof Error ? err.message : String(err)}`;
      },
    );
  };
  const zone = h('div.pj-drop', null, card('Add files', [status, h('div.pj-acts', null, lbtn('Choose files', () => void pickFiles().then(send), { icon: 'upload' }), lbtn('Choose a folder', () => void pickFiles({ folder: true }).then(send), { icon: 'folder' }))]));
  dropZone(zone, send);
  return zone;
}

function planTab(p: Project): (Node | null)[] {
  const docs = p.docs.length
    ? docsInOrder(p.docs).map(docRow)
    : emptyState(lu('file-text', 22), 'No documents yet', 'The brief and the proposal appear here once the orchestrator drafts them.');
  const critiques = p.critiques.length
    ? p.critiques.map((c) => critiqueRow(p, c))
    : emptyState(lu('messages-square', 22), 'No critiques yet', 'The critiques appear here once agents review the proposal.');
  const repo = p.repo ? repoCard(p.repo) : null;
  const files = p.kind === 'code' && !p.archived ? filesCard(p) : null;
  return [card('Documents', docs), card('Critiques', critiques), tasksCard(p), recipesCard(p), repo, files, historyCard(p)];
}

/** The repository's name: the last part of its path. */
const repoName = (repo: string): string => repo.slice(repo.lastIndexOf('/') + 1);

function repoCard(repo: string): HTMLElement {
  const name = repoName(repo);
  return card(
    'Repository',
    h(
      'div.pj-acts',
      null,
      h('a.mono', { href: href('history', name) }, repo),
      lbtn('Open repo', () => openRepo(name), { title: 'Open the repository in the IDE, on main' }),
    ),
  );
}

// ---- Dialogs and actions -------------------------------------------------------------------------------------------

/** Choose how the project is worked: each mode with what it means; the current one marked. */
function openMode(p: Project, done: () => void): void {
  void modesOnce().then((list) => {
    const current = modeOf(p);
    let chosen: ProjectMode = current;
    dialog(
      'How this project is worked',
      [
        ...list.map((m) =>
          h(
            'label.pj-radio',
            null,
            h('input', { type: 'radio', name: 'pj-mode', value: m.id, checked: m.id === current, onchange: () => (chosen = m.id) }),
            ' ',
            h('strong', null, m.label),
            m.id === current ? ' (now)' : null,
            ' ',
            h('span.muted', null, cap(m.summary)),
          ),
        ),
        p.stage === 'delivered' ? h('p.hint', null, 'It is delivered: back in build it opens again at implementation.') : null,
      ],
      (close) => [
        lbtn('Cancel', () => close(), { kind: 'ghost' }),
        lbtn(
          'Switch',
          () => {
            if (chosen === current) {
              close();
              return;
            }
            void attempt(() => projectsApi.setMode(p.id, chosen), `${p.title} is in ${chosen} now`).then((r) => {
              if (!r) return;
              close();
              done();
            });
          },
          { kind: 'primary' },
        ),
      ],
    );
  });
}

function openApprove(p: Project, done: () => void): void {
  const note = textarea({ rows: 3, placeholder: 'A note for its agents (optional)' });
  dialog('Approve plan', [field('Note', note)], (close) => [
    lbtn(
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
    ),
  ]);
}

/** Sends the project back to an earlier stage, with the reason its agents must act on. */
function openSendBack(p: Project, done: () => void): void {
  const earlier = PROJECT_STAGES.slice(0, stageIndex(p.stage));
  const stage = select(
    earlier.map((s) => ({ value: s, label: cap(s) })),
    earlier[earlier.length - 1],
  );
  const reason = textarea({ rows: 4, placeholder: 'What must change, and why' });
  dialog('Send back', [field('Back to', stage, 'The stage the project returns to'), field('Reason', reason)], (close) => [
    lbtn(
      'Send back',
      () => {
        const why = reason.value.trim();
        if (!why) {
          toast('Give a reason so the agents know what to change', 'error');
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
    ),
  ]);
}

/** Archives the project once the owner confirms: nothing more is done for it. */
function openArchive(p: Project, done: () => void): void {
  dialog(
    'Archive project?',
    [h('p.muted', null, `Nothing more is done for ${p.title}: its open tasks are withdrawn and its agents stop working on it. Its history stays.`)],
    (close) => [
      lbtn('Cancel', () => close(), { kind: 'ghost' }),
      lbtn(
        'Archive',
        () => {
          void (async () => {
            const ok = await attempt(() => api.archiveProject(p.id), `${p.title} archived`);
            if (ok) {
              close();
              done();
            }
          })();
        },
        { kind: 'danger' },
      ),
    ],
  );
}

/** Opens a project's repository in the IDE at `main`, which every Weft repository has. */
function openRepo(name: string): void {
  openIde(`repo:${name}@main`);
}

/** The page's More menu: send back (once work has started) and archive. */
function openMore(p: Project, anchor: HTMLElement, refresh: () => void): void {
  const item = (icon: LucideName, label: string, go: () => void): HTMLElement =>
    h('button', { type: 'button', role: 'menuitem', onclick: go }, lu(icon, 15), h('span', null, label));
  const items: HTMLElement[] = [];
  if (stageIndex(p.stage) > 0) items.push(item('undo-2', 'Send back…', () => openSendBack(p, refresh)));
  items.push(item('archive', 'Archive…', () => openArchive(p, refresh)));
  popover(anchor, h('div.menu', { role: 'menu' }, items));
}

/** The page's header: the project, its goal, its pipeline, and the actions the owner can take now. */
function header(p: Project, refresh: () => void): Node[] {
  const live = isLive(p);
  const approvable = APPROVABLE.includes(p.stage);
  const out: Node[] = [];
  const end: Node[] = [];
  const prim: Node[] = [];
  if (approvable) prim.push(lbtn('Approve plan', () => openApprove(p, refresh), { kind: 'primary', size: 'sm' }));
  out.push(h('a.btn.sm', { href: href('work', p.id), title: 'The project as one flow: its plan, its tasks in order, delivery' }, icon('commit', '', 14), h('span', null, 'Open in Work')));
  if (p.repo) {
    const name = p.repo.slice(p.repo.lastIndexOf('/') + 1);
    out.push(lbtn('Open repo', () => openRepo(name), { size: 'sm' }));
  }
  if (live && !p.paused) out.push(lbtn('Pause', () => void attempt(() => api.pauseProject(p.id), `${p.title} paused: its agents wait`).then(refresh), { size: 'sm', icon: 'pause', title: 'Its agents stop taking work until you resume it' }));
  if (live && p.paused) prim.push(lbtn('Resume', () => void attempt(() => api.resumeProject(p.id), `${p.title} resumed`).then(refresh), { kind: 'primary', size: 'sm', icon: 'play' }));
  // Mode is a choice, so it reads as a select-like pill: a label, the value, a chevron.
  if (!p.archived) {
    out.push(
      h(
        'button.btn.sm.pj-mode',
        { type: 'button', title: MODE_HINT, onclick: () => openMode(p, refresh) },
        h('span.pj-k', null, 'Mode'),
        h('span.pj-v', null, cap(modeOf(p))),
        lu('chevron-down', 14),
      ),
    );
  }
  // The rarer, heavier actions (send back, archive) sit behind More, at the right end.
  if (live) end.push(lbtn('More', (ev) => openMore(p, ev.currentTarget as HTMLElement, refresh), { size: 'sm', icon: 'ellipsis' }));
  if (end.length || prim.length) out.push(h('div.pj-end', null, end, prim));
  return [
    h('a.pj-back', { href: href('projects') }, '← All projects'),
    h(
      'div.pj-top',
      null,
      pmono(p, 40),
      h(
        'div.pj-idt',
        null,
        h('h1.pj-h1', null, p.title),
        h(
          'div.pj-badges',
          null,
          badge(p.kind, 'muted'),
          p.repo ? h('span.pj-faint.mono', null, p.repo) : null,
          badge(cap(p.stage), 'accent'),
          lifeBadge(p),
          p.approved_by ? h('span.pj-faint', null, `approved by ${p.approved_by}`) : null,
        ),
      ),
    ),
    h('p.pj-goal', null, p.goal),
    pipeline(p.stage),
    h('div.pj-actions', null, out),
  ];
}

/** The page's tabs: the ones drawn here, and the console's own Work, Tasks and Weft screens for the project. */
function tabsBar(p: Project, tab: string): HTMLElement {
  return h(
    'nav.pj-tabs',
    null,
    TAB_LINKS.map(([key, label]) => {
      const to = key === 'work' ? href('work', p.id) : key === 'tasks' ? hrefIn(p.id, 'tasks') : key === 'history' ? hrefIn(p.id, 'history') : href('projects', p.id, ...(key === 'plan' ? [] : [key]));
      return h('a.tab', { href: to, 'aria-current': OWN_TABS.includes(key) && key === tab ? 'page' : null }, label);
    }),
  );
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const id = params[0] || '';
  const tab = OWN_TABS.includes(params[1] ?? '') ? params[1]! : 'plan';
  const screen = h('div.view', null);
  root.append(screen);

  let alive = true;
  let loaded = false;
  let timer = 0;
  let list: Project[] = [];
  let current: Project | null = null;
  // The Maintenance and Environments tabs follow their own events: one kept across this page's redraws (for the same
  // project and mode) keeps its scroll, its selection and its data.
  const kept = new Map<string, HTMLElement>();
  const keep = (key: string, make: () => HTMLElement): HTMLElement => {
    let el = kept.get(key);
    if (!el) kept.set(key, (el = make()));
    return el;
  };

  const listNodes = (): Node[] => {
    // The live projects first, newest first; the ones that are over after them.
    const ordered = [...list].sort((a, b) => Number(isLive(b)) - Number(isLive(a)) || b.created_ms - a.created_ms);
    const newBtn = lbtn('New project', () => openNewProject(), { kind: 'primary', icon: 'plus' });
    return [
      h('div.view-head.pj-head', null, h('div', null, h('h1.pj-h1', null, 'Projects')), h('div.pj-actions', null, newBtn)),
      h('p.pj-intro', null, 'Agents work on one project at a time. Talk a new one through with the orchestrator, or open one yourself.'),
      ordered.length
        ? h('div.pj-list', null, ordered.map(projectRow))
        : emptyState(icon('commit', '', 22), 'No projects yet.', 'Start one in a chat with the orchestrator: it drafts the tasks and the budget, and you start it.', newBtn),
    ];
  };

  const detailNodes = (p: Project): Node[] => {
    const refresh = (): void => {
      void reload();
      void refreshProjects();
    };
    const body: (Node | null)[] =
      tab === 'settings'
        ? [settingsTab(p)]
        : tab === 'maintenance'
          ? [keep(`maintenance:${p.id}:${modeOf(p)}`, () => maintenanceTab(p, refresh))]
          : tab === 'environments'
            ? [keep(`environments:${p.id}:${p.archived ? 'archived' : 'live'}`, () => environmentsTab(p))]
            : tab === 'previews'
              ? [keep(`previews:${p.id}:${p.archived ? 'archived' : 'live'}`, () => previewsTab(p))]
              : planTab(p);
    return [...header(p, refresh), tabsBar(p, tab), h('div.pj-body', null, body)];
  };

  const draw = (): void => {
    if (!loaded) return;
    if (!id) {
      replace(screen, listNodes());
      return;
    }
    if (!current) {
      replace(screen, h('a.pj-back', { href: href('projects') }, '← All projects'), emptyState(icon('commit', '', 22), 'Project not found', 'It may have been removed.'));
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
  const offs: (() => void)[] = [on('project', schedule), on('task', schedule), on(RESYNC, schedule)];
  // The last read paints at once; the fresh one (asked just below) replaces it.
  if (id) {
    const kept = api.project.cached(id);
    if (kept) {
      current = kept;
      loaded = true;
      draw();
    }
  } else {
    const kept = api.projects.cached();
    if (kept) {
      list = kept;
      loaded = true;
      draw();
    }
  }
  void reload();

  return () => {
    alive = false;
    window.clearTimeout(timer);
    offs.forEach((off) => off());
    screen.remove();
  };
}
