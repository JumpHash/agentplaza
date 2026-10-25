// The Workflows tab: the pure rules (the lines of a workflow, a job and a step, the run states, the counts and lines of a
// GitHub Actions conversion) and the tab itself drawn over fixtures. The runner has no DOM, so the tab's markup is built
// against a small stand-in (only what the tab's builders touch), and the answers come from a stubbed fetch with the hub's
// shapes. Bundled with the other suites by test/tests.ts; it prints "workflows tests: ok".

import type { Project } from '../src/api';
import type { ImportItem, WorkflowList, WorkflowStep } from '../src/api-workflows';
import { workflowsTab } from '../src/views/project-workflows';
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
  unsupportedLine,
  workflowSummaryLine,
} from '../src/views/workflows-model';

let checks = 0;

function eq<T>(actual: T, expected: T, what: string): void {
  checks++;
  const a = JSON.stringify(actual);
  const b = JSON.stringify(expected);
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`);
}

// ---- The hub's shapes, as fixtures --------------------------------------------------------------------------------------

const LIST: WorkflowList = {
  workflows: [
    { name: 'build', file: '.plaza/workflows/build.toml', triggers: ['push', 'manual'], cron: '', jobs: 2, steps: 5, error: null },
    { name: 'nightly', file: '.plaza/workflows/nightly.toml', triggers: [], cron: '0 3 * * *', jobs: 1, steps: 1, error: null },
  ],
  errors: [{ file: '.plaza/workflows/broken.toml', error: 'line 3: expected a table' }],
};

const IMPORT: ImportItem = {
  file: '.github/workflows/ci.yml',
  name: 'ci',
  toml: 'name = "ci"\n\n[[job]]\nid = "test"\n',
  report: {
    mapped: ['on: push', 'jobs.test'],
    notes: ['runs-on ubuntu-latest is kept as the runner label'],
    unsupported: [{ at: 'jobs.test.steps[2]', what: 'uses: actions/cache@v4', reason: 'no plaza step caches dependencies' }],
  },
};

// ---- The pure rules ------------------------------------------------------------------------------------------------------

eq(plural(1, 'job'), '1 job', 'one is singular');
eq(plural(2, 'step'), '2 steps', 'more is plural');
eq(plural(0, 'note'), '0 notes', 'zero is plural');
eq(workflowSummaryLine(LIST.workflows[0]!), 'build: push, manual · 2 jobs · 5 steps', 'a workflow line: name, triggers, jobs and steps');
eq(workflowSummaryLine(LIST.workflows[1]!), 'nightly: no triggers · 1 job · 1 step', 'a workflow with no trigger says so');
eq(errorLine(LIST.errors[0]!), '.plaza/workflows/broken.toml: line 3: expected a table', 'a parse error names its file');

eq([runStateLabel('running'), runStateLabel('owner'), runStateLabel('passed'), runStateLabel('failed'), runStateLabel('cancelled')], ['Running', 'Waiting for you', 'Passed', 'Failed', 'Cancelled'], 'run states in words');
eq(runStateLabel('paused'), 'paused', 'an unknown state reads as it is');
eq([runStateTone('running'), runStateTone('owner'), runStateTone('passed'), runStateTone('failed')], ['info', 'warn', 'ok', 'danger'], 'run state tones');
eq(runStateTone('cancelled'), 'muted', 'a cancelled run is muted');
eq(startedText({ run: 'r9', state: 'running' } as never), 'Run r9 is running.', 'a started run says its id and state');

const step = (over: Partial<WorkflowStep> & { kind: WorkflowStep['kind'] }): WorkflowStep => over as WorkflowStep;
eq(stepLine(step({ kind: 'run', run: 'npm   test\n --workspace web' })), 'run: npm test --workspace web', 'a run step shows its command on one line');
eq(stepLine(step({ kind: 'run' })), 'run', 'a run step with no command');
eq(stepLine(step({ kind: 'test', select: 'changed' })), 'test (changed)', 'a test step names its selection');
eq(stepLine(step({ kind: 'test' })), 'test', 'a test step with every test');
eq(stepLine(step({ kind: 'build' })), 'build', 'a build step');
eq(stepLine(step({ kind: 'deploy', environment: 'staging' })), 'deploy staging', 'a deploy step names its environment');
eq(stepLine(step({ kind: 'deploy', environment: 'preview', ephemeral: true, ttl_minutes: 30 })), 'deploy preview (ephemeral, ttl 30 min)', 'an ephemeral deploy with a time to live');
eq(stepLine(step({ kind: 'deploy', environment: 'preview', ephemeral: true })), 'deploy preview (ephemeral)', 'an ephemeral deploy with no time to live');
eq(stepLine(step({ kind: 'against', environment: 'staging' })), 'against staging', 'an against step');
eq(stepLine(step({ kind: 'promote', environment: 'production' })), 'promote to production', 'a promote step');
eq(stepLine(step({ kind: 'teardown', environment: 'preview' })), 'teardown preview', 'a teardown step');
eq(stepLine(step({ kind: 'approval', message: 'Ship it?' })), 'approval: Ship it?', 'an approval step carries its message');
eq(stepLine(step({ kind: 'approval' })), 'approval', 'an approval step with no message');

eq(needsLine(['build', 'lint']), 'Needs build, lint', 'a job with needs');
eq(needsLine([]), 'Needs nothing', 'a job with no needs');
eq(matrixLine({ os: ['linux', 'macos'], node: ['20', '22'] }), 'os: linux, macos · node: 20, 22', 'a matrix in one line');
eq(matrixLine({}), '', 'no matrix is an empty line');

eq(importCounts(IMPORT.report), { mapped: 2, notes: 1, unsupported: 1 }, 'the counts of a conversion');
eq(importCountsLine(importCounts(IMPORT.report)), '2 mapped · 1 note · 1 unsupported', 'the counts in one line');
eq(unsupportedLine(IMPORT.report.unsupported[0]!), 'jobs.test.steps[2]: uses: actions/cache@v4. no plaza step caches dependencies', 'an unsupported item says where, what and why');
eq(importLines(IMPORT).unsupported, [unsupportedLine(IMPORT.report.unsupported[0]!)], 'the preview lists what could not be converted');
eq(savedText('ch42'), 'Saved in change ch42. Land it to add the workflow.', 'a save names its change');

// ---- The tab, drawn over fixtures -------------------------------------------------------------------------------------

/** The part of the DOM the tab's builders touch: elements with attributes and children, and text nodes. */
class FakeText {
  constructor(public data: string) {}
  get textContent(): string {
    return this.data;
  }
}
class FakeElement {
  className = '';
  attrs: Record<string, string> = {};
  children: (FakeElement | FakeText)[] = [];
  constructor(public tagName: string) {}
  /** keepState reads the children of a drawn element; a fresh draw has none to keep, so the stand-in reports none. */
  get firstElementChild(): null {
    return null;
  }
  get firstChild(): FakeElement | FakeText | null {
    return this.children[0] ?? null;
  }
  get textContent(): string {
    return this.children.map((c) => c.textContent).join('');
  }
  setAttribute(name: string, value: string): void {
    this.attrs[name] = value;
  }
  appendChild(child: FakeElement | FakeText): FakeElement | FakeText {
    this.children.push(child);
    return child;
  }
  removeChild(child: FakeElement | FakeText): FakeElement | FakeText {
    this.children = this.children.filter((c) => c !== child);
    return child;
  }
  addEventListener(): void {}
}

const g = globalThis as unknown as Record<string, unknown>;
g.Element = FakeElement;
g.document = {
  createElement: (tag: string) => new FakeElement(tag),
  createElementNS: (_ns: string, tag: string) => new FakeElement(tag),
  createTextNode: (text: string) => new FakeText(text),
};
g.localStorage = { getItem: () => null, setItem: () => {}, removeItem: () => {}, key: () => null, length: 0 };

/** The hub's answers for the tab's reads; anything else is a 404. */
const answers: Record<string, unknown> = {
  '/v1/projects/pr1/workflows': LIST,
  '/v1/projects/pr1/workflows/import': { imports: [IMPORT], errors: [] },
};
g.fetch = async (input: string | URL) => {
  const body = answers[String(input)];
  return body === undefined
    ? new Response('{"error":"not found"}', { status: 404, headers: { 'content-type': 'application/json' } })
    : new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } });
};

/** Waits until the tab shows `text` (its reads resolve on a later tick), or gives up after a while. */
async function settle(root: { textContent: string }, text: string): Promise<string> {
  for (let i = 0; i < 200 && !root.textContent.includes(text); i++) await new Promise((resolve) => setTimeout(resolve, 0));
  return root.textContent;
}

async function render(): Promise<void> {
  const project = { id: 'pr1', archived: false } as unknown as Project;
  const root = workflowsTab(project) as unknown as FakeElement;
  const shown = await settle(root, 'Not converted (1)');

  // The list: each workflow with its name, its triggers (or none) and its counts; the file that did not parse under it.
  for (const part of ['build', 'push', 'manual', '2 jobs · 5 steps', 'nightly', 'no triggers', 'broken.toml']) {
    eq(shown.includes(part), true, `the list shows ${part}`);
  }
  eq(shown.includes('.plaza/workflows/broken.toml: line 3: expected a table'), true, 'a parse error is listed under the workflows');

  // The import: its counts, the unsupported item (where, what and why) first, and its TOML.
  eq(shown.includes('2 mapped · 1 note · 1 unsupported'), true, 'the import shows its counts');
  eq(shown.includes('Not converted (1)'), true, 'the unsupported items are headed');
  eq(shown.includes('jobs.test.steps[2]: uses: actions/cache@v4. no plaza step caches dependencies'), true, 'the unsupported text is rendered');
  eq(shown.includes('name = "ci"'), true, 'the converted TOML is rendered');
  eq(shown.includes('Save to .plaza/workflows'), true, 'a conversion offers its save, which writes only when pressed');
}

// The render check runs after the synchronous rules and reports on its own.
render().then(
  () => console.log(`workflows tests: ok (${checks} checks, the tab drawn over fixtures)`),
  (err: unknown) => {
    console.error(err);
    process.exitCode = 1;
  },
);
