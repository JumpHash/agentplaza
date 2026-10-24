// A project's Workflows tab: the CI/CD pipelines its repo declares under `.plaza/workflows`. The list, with the files that
// failed to parse under it; one workflow in full (its triggers, its jobs and their steps, its recent runs) and a manual
// run of it, confirmed first. Under them, the GitHub Actions workflows the repo has, converted for a preview: what could
// not be converted comes first, the TOML is one click away, and a save writes the file through a change the owner lands
// (nothing is saved on its own). A run opens in the recipe run view. The rules are in workflows-model.ts; the API is
// api-workflows.ts. The markup is the project page's (projects.css) and workflows.css. Every text from the hub is set as
// text, never as markup.

import '../styles/workflows.css';
import '../styles/recipe-graph.css';
import {
  workflowsApi,
  type ImportItem,
  type WorkflowDetail,
  type WorkflowError,
  type WorkflowJob,
  type WorkflowList,
  type WorkflowRun,
  type WorkflowSummary,
} from '../api-workflows';
import type { Project } from '../api';
import { hrefIn } from '../router';
import { ago, attempt, badge, button, card, empty, modal, toast } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import {
  errorLine,
  importCounts,
  importCountsLine,
  importLines,
  matrixLine,
  needsLine,
  plural,
  runStateLabel,
  runStateTone,
  savedText,
  startedText,
  stepLine,
  workflowSummaryLine,
} from './workflows-model';

/** How many of a workflow's runs the tab lists (the run pages keep the rest). */
const RUN_LIMIT = 10;

/** The Workflows tab of project `p`. A read-only page (archived) lists and shows, but starts no run and saves nothing. */
export function workflowsTab(p: Project): HTMLElement {
  const root = h('div.pj-tab.wf');
  const writable = !p.archived;

  // Undefined until the first answer. A failed reload keeps what it had.
  let list: WorkflowList | undefined;
  let listFailed = false;
  let imports: ImportItem[] | undefined;
  let importErrors: WorkflowError[] = [];
  let importsFailed = false;
  let selected: string | null = null;
  let detail: WorkflowDetail | undefined;
  let detailFailed = false;
  let runs: WorkflowRun[] | undefined;
  let runsFailed = false;
  /** The last run started from this tab, shown under the workflow it started. */
  let started: { name: string; run: WorkflowRun } | null = null;
  let starting = false;
  /** The change each saved conversion went into, by its file; the files being saved now. */
  const saved = new Map<string, string>();
  const saving = new Set<string>();

  const loadList = async (): Promise<void> => {
    try {
      list = await workflowsApi.list(p.id);
      listFailed = false;
    } catch {
      listFailed = true;
    }
    // A workflow that is gone clears the selection.
    if (selected !== null && list && !list.workflows.some((w) => w.name === selected)) {
      selected = null;
      detail = undefined;
      runs = undefined;
    }
    draw();
  };

  const loadImports = async (): Promise<void> => {
    try {
      const got = await workflowsApi.imports(p.id);
      imports = got.imports;
      importErrors = got.errors;
      importsFailed = false;
    } catch {
      importsFailed = true;
    }
    draw();
  };

  /** The selected workflow's definition and runs, read together. A failed read of one keeps the other. */
  const loadSelected = async (name: string): Promise<void> => {
    const [d, r] = await Promise.allSettled([workflowsApi.get(p.id, name), workflowsApi.runs(p.id, name)]);
    if (selected !== name) return;
    if (d.status === 'fulfilled') {
      detail = d.value;
      detailFailed = false;
    } else detailFailed = true;
    if (r.status === 'fulfilled') {
      runs = r.value;
      runsFailed = false;
    } else runsFailed = true;
    draw();
  };

  const refresh = (): void => {
    void loadList();
    void loadImports();
    if (selected !== null) void loadSelected(selected);
  };

  /** Selects a workflow (a second click on it clears the selection) and reads it. */
  const select = (name: string): void => {
    if (selected === name) {
      selected = null;
      detail = undefined;
      runs = undefined;
      started = null;
      draw();
      return;
    }
    selected = name;
    detail = undefined;
    runs = undefined;
    detailFailed = false;
    runsFailed = false;
    started = null;
    draw();
    void loadSelected(name);
  };

  /** Starts a manual run after the owner has confirmed it; the new run's state is shown under the workflow. */
  const startRun = async (name: string): Promise<void> => {
    starting = true;
    draw();
    const run = await attempt(() => workflowsApi.startRun(p.id, name));
    starting = false;
    if (run) {
      started = { name, run };
      toast(startedText(run));
      try {
        runs = await workflowsApi.runs(p.id, name);
        runsFailed = false;
      } catch {
        runsFailed = true;
      }
    }
    draw();
  };

  const confirmRun = (name: string): void => {
    const close = modal(
      'Run now',
      h('p', null, `Start a manual run of ${name} now. Its run opens in the recipe run page.`),
      undefined,
      [
        button('Cancel', () => close(), { kind: 'ghost' }),
        button(
          'Start run',
          () => {
            close();
            void startRun(name);
          },
          { kind: 'primary', icon: 'play' },
        ),
      ],
    );
  };

  /** Saves one conversion to `.plaza/workflows/<name>.toml` through a change. Nothing is saved until this is pressed. */
  const save = async (item: ImportItem): Promise<void> => {
    if (saving.has(item.file)) return;
    saving.add(item.file);
    draw();
    const done = await attempt(() => workflowsApi.saveImport(p.id, item.file));
    saving.delete(item.file);
    if (done) {
      saved.set(item.file, done.change);
      toast(savedText(done.change));
    }
    draw();
  };

  const intro = (): HTMLElement =>
    h(
      'div.wf-intro',
      null,
      h('p', null, 'The CI/CD pipelines this project declares in its repo, under .plaza/workflows. Each run opens in the recipe run view.'),
      button('Refresh', () => refresh(), { kind: 'ghost', icon: 'refresh-cw' }),
    );

  const failNote = (text: string): HTMLElement => h('p.pj-faint', null, text);
  const errorNote = (text: string): HTMLElement => h('p.wf-warn', null, lu('triangle-alert', 15), h('span', null, text));

  const workflowRow = (w: WorkflowSummary): HTMLElement => {
    const row = h(
      'button.wf-row',
      { type: 'button', title: workflowSummaryLine(w), 'aria-pressed': selected === w.name ? 'true' : 'false' },
      h('span.wf-row-name', null, h('b.mono', null, w.name)),
      h('span.wf-chips', null, w.triggers.length ? w.triggers.map((t) => badge(t, 'info')) : badge('no triggers', 'muted')),
      h('span.pj-faint', null, `${plural(w.jobs, 'job')} · ${plural(w.steps, 'step')}`),
    );
    row.addEventListener('click', () => select(w.name));
    return row;
  };

  const listBody = (): Node[] => {
    if (list === undefined) return [failNote(listFailed ? 'The workflows could not be loaded.' : 'Loading the workflows…')];
    const { workflows, errors } = list;
    return [
      workflows.length
        ? h('div.wf-list', null, workflows.map(workflowRow))
        : empty('git-branch', 'No workflows yet.', 'Add a file under .plaza/workflows in the repo, or convert one from GitHub Actions below.'),
      ...errors.map((e) => errorNote(errorLine(e))),
    ];
  };

  const runRow = (run: WorkflowRun): HTMLElement =>
    h(
      'a.rr-row',
      { href: hrefIn(null, 'recipe-graph', 'run', run.run) },
      h('span.mono.rr-row-id', null, run.run),
      h('span.rr-row-what', null, run.outcome || run.trigger),
      badge(runStateLabel(run.state), runStateTone(run.state)),
      h('span.rr-row-when', null, ago(run.created_ms)),
    );

  const runsView = (): Node => {
    if (runsFailed) return failNote('The runs could not be loaded.');
    if (runs === undefined) return failNote('Loading the runs…');
    if (!runs.length) return failNote('No runs yet.');
    return h('div.rr-rows', null, runs.slice(0, RUN_LIMIT).map(runRow));
  };

  const jobView = (j: WorkflowJob): HTMLElement => {
    const matrix = matrixLine(j.matrix);
    return h(
      'div.wf-job',
      null,
      h('div.wf-job-h', null, h('b.mono', null, j.id), h('span.pj-faint', null, `runs on ${j.runs_on || 'no runner'}`)),
      h('p.wf-meta', null, needsLine(j.needs)),
      matrix ? h('p.wf-meta', null, `Matrix: ${matrix}`) : null,
      j.steps.length ? h('ol.wf-steps', null, j.steps.map((s) => h('li', null, stepLine(s)))) : failNote('No steps.'),
    );
  };

  const detailBody = (d: WorkflowDetail): (Node | null)[] => [
    h(
      'div.wf-head',
      null,
      h(
        'div',
        null,
        d.description ? h('p', null, d.description) : null,
        h('div.wf-chips', null, d.triggers.length ? d.triggers.map((t) => badge(t, 'info')) : badge('no triggers', 'muted')),
        d.cron ? h('p.wf-cron', null, h('span.pj-faint', null, 'Schedule'), h('code', null, d.cron)) : null,
      ),
      writable ? button('Run now', () => confirmRun(d.name), { kind: 'primary', icon: 'play', disabled: starting }) : null,
    ),
    started && started.name === d.name
      ? h('p.wf-started', { role: 'status' }, startedText(started.run), h('a', { href: hrefIn(null, 'recipe-graph', 'run', started.run.run) }, 'Open the run'))
      : null,
    h('h4', null, 'Jobs'),
    ...(d.jobs.length ? d.jobs.map(jobView) : [failNote('No jobs.')]),
    h('h4', null, 'Recent runs'),
    runsView(),
  ];

  const detailNode = (): HTMLElement | null => {
    if (selected === null) return null;
    const title = h('span.mono', null, selected);
    if (detailFailed) return card(title, failNote('The workflow could not be loaded.'));
    if (!detail) return card(title, failNote('Loading the workflow…'));
    return card(title, detailBody(detail).filter((n): n is Node => n !== null));
  };

  /** One conversion: its name and counts in the summary; open, what could not be converted, the notes and the TOML. */
  const importView = (item: ImportItem): HTMLElement => {
    const counts = importCounts(item.report);
    const lines = importLines(item);
    const change = saved.get(item.file);
    return h(
      'details.wf-imp',
      null,
      h(
        'summary',
        null,
        h('b.mono', null, item.name),
        h('span.pj-faint.mono', null, item.file),
        badge(importCountsLine(counts), counts.unsupported ? 'warn' : 'ok'),
      ),
      h(
        'div.wf-imp-b',
        null,
        lines.unsupported.length
          ? h(
              'div.wf-unsup',
              null,
              h('b', null, `Not converted (${lines.unsupported.length})`),
              h('ul', null, lines.unsupported.map((t) => h('li', null, t))),
            )
          : null,
        lines.notes.length ? h('div.wf-notes', null, h('b', null, 'Notes'), h('ul', null, lines.notes.map((t) => h('li', null, t)))) : null,
        lines.mapped.length ? h('p.pj-faint', null, `Carried over: ${lines.mapped.join(', ')}`) : null,
        h('pre.wf-pre', null, item.toml),
        writable
          ? h(
              'div.wf-save',
              null,
              button('Save to .plaza/workflows', () => void save(item), { kind: 'ghost', icon: 'save', disabled: saving.has(item.file) }),
              change ? h('p.wf-saved', { role: 'status' }, savedText(change)) : null,
            )
          : null,
      ),
    );
  };

  const importsBody = (): Node[] => {
    const nodes: Node[] = [h('p.pj-faint', null, 'The workflows under .github/workflows, converted to plaza’s format for a preview. Nothing is saved until you save one.')];
    if (imports === undefined) {
      nodes.push(failNote(importsFailed ? 'The imports could not be loaded.' : 'Loading the imports…'));
      return nodes;
    }
    if (!imports.length && !importErrors.length) nodes.push(failNote('No GitHub Actions workflows found.'));
    nodes.push(...imports.map(importView), ...importErrors.map((e) => errorNote(errorLine(e))));
    return nodes;
  };

  const draw = (): void => {
    replace(root, intro(), card('Workflows', listBody()), detailNode(), card('Import from GitHub Actions', importsBody()));
  };

  draw();
  refresh();
  return root;
}
