// The New job form (on `#/lab/jobs`). It builds the job document the Lab's apid takes, field by field
// in the order the desktop shows them. "Preview plan" is the dry run; "Submit" unlocks only for the
// exact document the preview was made from, and asks for a confirm before it posts to the fleet.
// The document is built the same way on the desktop (`screens/lab_jobs.rs`): same keys, same order.

import {
  JobDispatchError,
  listDatasets,
  listDevices,
  listRecipes,
  planJob,
  submitJob,
  type Dataset,
  type DeviceMachine,
  type DeviceRequest,
  type FoundryMode,
  type JobDocument,
  type JobPlan,
  type Recipe,
} from '../api';
import { href } from '../router';
import { button, input, select, textarea, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import { errorText, fmtNumber, pollEvery } from './lab-common';
import { planDiagram } from './lab-jobs-plan';

/** The backends a device pin may name, in the planner's order. */
export const BACKENDS = ['cuda', 'rocm', 'xpu', 'mps', 'cpu'] as const;
const MODES: { value: string; label: string }[] = [
  { value: '', label: 'Auto (the planner chooses)' },
  { value: 'single', label: 'single' },
  { value: 'tight', label: 'tight' },
  { value: 'loose', label: 'loose' },
];
const POLL_MS = 4000;

/** What the form's fields hold. Everything is text until `buildDocument` reads it. */
export interface FormValues {
  recipe: string;
  datasets: string[];
  search: string;
  budget: string;
  minDevices: string;
  maxDevices: string;
  minMemory: string;
  backends: string[];
  mode: string;
  machines: string[];
  allowCpu: boolean;
  train: string;
}

export function emptyValues(): FormValues {
  return {
    recipe: '',
    datasets: [],
    search: '',
    budget: '',
    minDevices: '',
    maxDevices: '',
    minMemory: '',
    backends: [],
    mode: '',
    machines: [],
    allowCpu: false,
    train: '',
  };
}

type Parsed<T> = { ok: true; value: T } | { ok: false; error: string };

const blank = (s: string): boolean => s.trim() === '';
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

/** A JSON object from text. Empty text is no object (`undefined`), not an error. */
export function parseJsonObject(text: string, what: string): Parsed<Record<string, unknown> | undefined> {
  const t = text.trim();
  if (!t) return { ok: true, value: undefined };
  let parsed: unknown;
  try {
    parsed = JSON.parse(t);
  } catch (err) {
    return { ok: false, error: `${what} is not valid JSON: ${errorText(err)}` };
  }
  if (!isObject(parsed)) return { ok: false, error: `${what} must be a JSON object.` };
  return { ok: true, value: parsed };
}

/** A `key: value` value: JSON when it parses (numbers, lists, booleans), else the text itself. */
function parseValue(raw: string): unknown {
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    return raw;
  }
}

/** The search space: a JSON object, or one `key: value` per line. Empty text is no search. */
export function parseSearch(text: string): Parsed<Record<string, unknown> | undefined> {
  const t = text.trim();
  if (!t) return { ok: true, value: undefined };
  if (t.startsWith('{')) return parseJsonObject(t, 'Search space');
  const pairs = new Map<string, unknown>();
  const lines = t.split('\n');
  for (let i = 0; i < lines.length; i += 1) {
    const line = (lines[i] ?? '').trim();
    if (!line) continue;
    const at = line.indexOf(':');
    const key = at > 0 ? line.slice(0, at).trim() : '';
    if (!key) return { ok: false, error: `Search line ${i + 1} needs "key: value".` };
    if (pairs.has(key)) return { ok: false, error: `Search key "${key}" is set twice.` };
    pairs.set(key, parseValue(line.slice(at + 1).trim()));
  }
  return { ok: true, value: Object.fromEntries(pairs) };
}

/** A whole number from text, or NaN when the text is not one. */
const wholeOf = (s: string): number => (/^\d+$/.test(s.trim()) ? Number(s.trim()) : Number.NaN);

/** The `devices` block: only the keys the form sets, in the order the apid's struct lists them. */
export function buildDevices(v: FormValues): Parsed<DeviceRequest | undefined> {
  const minD = blank(v.minDevices) ? null : wholeOf(v.minDevices);
  const maxD = blank(v.maxDevices) ? null : wholeOf(v.maxDevices);
  const mem = blank(v.minMemory) ? null : wholeOf(v.minMemory);
  if (minD !== null && !(minD >= 1)) return { ok: false, error: 'Min devices is a whole number, 1 or more.' };
  if (maxD !== null && !(maxD >= 1)) return { ok: false, error: 'Max devices is a whole number, 1 or more.' };
  if (mem !== null && !(mem >= 1)) return { ok: false, error: 'Min memory is a whole number of MiB, 1 or more.' };
  if (minD !== null && maxD !== null && maxD < minD) return { ok: false, error: 'Max devices is below min devices.' };
  const req: DeviceRequest = {};
  if (minD !== null) req.min_devices = minD;
  if (maxD !== null) req.max_devices = maxD;
  if (mem !== null) req.min_memory_mib = mem;
  const backends = BACKENDS.filter((b) => v.backends.includes(b));
  if (backends.length) req.backends = [...backends];
  if (v.mode) req.mode = v.mode as FoundryMode;
  if (v.machines.length) req.machines = [...v.machines].sort();
  if (v.allowCpu) req.allow_cpu = true;
  return { ok: true, value: Object.keys(req).length ? req : undefined };
}

/**
 * The job document from the form, or the list of what is missing. Keys come in the order the desktop
 * sends them: recipe, datasets, search, budget_gpu_hours, devices, train.
 */
export function buildDocument(v: FormValues): { doc: JobDocument | null; errors: string[] } {
  const errors: string[] = [];
  if (!v.recipe) errors.push('Pick a vetted recipe.');
  if (v.datasets.length === 0) errors.push('Pick at least one dataset.');
  const budget = blank(v.budget) ? Number.NaN : Number(v.budget.trim());
  if (!Number.isFinite(budget) || !(budget > 0)) errors.push('Budget is a number of GPU-hours above 0.');
  const search = parseSearch(v.search);
  if (!search.ok) errors.push(search.error);
  const devices = buildDevices(v);
  if (!devices.ok) errors.push(devices.error);
  const train = parseJsonObject(v.train, 'Train overrides');
  if (!train.ok) errors.push(train.error);
  if (errors.length || !search.ok || !devices.ok || !train.ok) return { doc: null, errors };
  const doc: JobDocument = {
    recipe: v.recipe,
    datasets: [...v.datasets].sort(),
    ...(search.value ? { search: search.value } : {}),
    budget_gpu_hours: budget,
    ...(devices.value ? { devices: devices.value } : {}),
    ...(train.value ? { train: train.value } : {}),
  };
  return { doc, errors: [] };
}

const isVetted = (r: Recipe): boolean => r.vetted_by !== null && r.vetted_ms !== null;

function toggle(list: string[], id: string, on: boolean): void {
  const at = list.indexOf(id);
  if (on && at < 0) list.push(id);
  if (!on && at >= 0) list.splice(at, 1);
}

function check(label: string, checked: boolean, onChange: (on: boolean) => void, meta?: (Node | null)[]): HTMLLabelElement {
  const box = h('input', { type: 'checkbox' });
  box.checked = checked;
  box.addEventListener('change', () => onChange(box.checked));
  return h('label.lab-jobs-check', null, box, h('span.lab-jobs-check-name', null, label), meta ?? null);
}

function field(label: string, control: Node, hint?: string): HTMLLabelElement {
  return h('label.lab-jobs-field', null, h('span.lab-jobs-label', null, label), control, hint ? h('span.lab-muted', null, hint) : null);
}

function group(label: string, body: Node, hint?: string): HTMLElement {
  return h('div.lab-jobs-field', null, h('span.lab-jobs-label', null, label), body, hint ? h('span.lab-muted', null, hint) : null);
}

export interface JobForm {
  el: HTMLElement;
  destroy: () => void;
}

/**
 * The form, built once: its fields keep what the user typed while the options refresh on the poll.
 * `onSubmitted` gets the new job's id after a successful submit.
 */
export function jobForm(onSubmitted: (id: string) => void): JobForm {
  const v = emptyValues();
  let alive = true;
  let recipes: Recipe[] = [];
  let datasets: Dataset[] = [];
  let machines: DeviceMachine[] = [];
  let attempted = false;
  let previewing = false;
  let submitting = false;
  let confirming = false;
  let preview: { key: string; plan: JobPlan } | null = null;
  let previewError: { key: string; message: string } | null = null;
  let submitError: { message: string; jobId: string | null } | null = null;

  // Fixed controls: built once, so a poll never takes the focus away.
  const recipeSel = select([{ value: '', label: 'Choose a vetted recipe' }], '');
  const recipeHint = h('span.lab-muted');
  const datasetBox = h('div.lab-jobs-checks');
  const searchArea = textarea({ rows: 4, placeholder: '{"lr": [0.001, 0.01]}\nor one key: value per line', spellcheck: 'false' });
  const budgetIn = input({ type: 'number', min: '0', step: 'any', placeholder: 'e.g. 1.5', inputmode: 'decimal' });
  const minDevIn = input({ type: 'number', min: '1', step: '1', inputmode: 'numeric' });
  const maxDevIn = input({ type: 'number', min: '1', step: '1', inputmode: 'numeric' });
  const minMemIn = input({ type: 'number', min: '1', step: '1', inputmode: 'numeric', placeholder: 'MiB' });
  const backendBoxes = BACKENDS.map((name) => {
    const box = h('input', { type: 'checkbox' });
    box.addEventListener('change', () => {
      toggle(v.backends, name, box.checked);
      sync();
    });
    return { name, box, label: h('label.lab-jobs-check', null, box, h('span.lab-jobs-check-name', null, name)) };
  });
  const backendRow = h('div.lab-jobs-inline', null, backendBoxes.map((b) => b.label));
  const modeSel = select(MODES, '');
  const machineBox = h('div.lab-jobs-checks');
  const allowCpu = h('input', { type: 'checkbox' });
  const trainArea = textarea({ rows: 3, placeholder: '{"steps": 500}', spellcheck: 'false' });
  const optionsError = h('p.lab-error');
  const errorBox = h('div');
  const previewBox = h('div.lab-jobs-preview');
  const confirmBox = h('div');
  const submitBox = h('div');
  const previewBtn = button('Preview plan', () => void runPreview(), { kind: 'primary' });
  const submitBtn = button('Submit', () => askSubmit(), { title: 'Preview the plan first' });

  const bindText = (el: HTMLInputElement | HTMLTextAreaElement, write: (value: string) => void): void => {
    el.addEventListener('input', () => {
      write(el.value);
      sync();
    });
  };
  recipeSel.addEventListener('change', () => {
    v.recipe = recipeSel.value;
    sync();
  });
  bindText(searchArea, (s) => (v.search = s));
  bindText(budgetIn, (s) => (v.budget = s));
  bindText(minDevIn, (s) => (v.minDevices = s));
  bindText(maxDevIn, (s) => (v.maxDevices = s));
  bindText(minMemIn, (s) => (v.minMemory = s));
  bindText(trainArea, (s) => (v.train = s));
  modeSel.addEventListener('change', () => {
    v.mode = modeSel.value;
    sync();
  });
  allowCpu.addEventListener('change', () => {
    v.allowCpu = allowCpu.checked;
    sync();
  });

  const currentKey = (): string | null => {
    const { doc } = buildDocument(v);
    return doc ? JSON.stringify(doc) : null;
  };
  const isFresh = (): boolean => {
    const key = currentKey();
    return key !== null && preview !== null && preview.key === key;
  };

  function renderOptions(): void {
    const vetted = recipes.filter(isVetted);
    if (v.recipe && !vetted.some((r) => r.id === v.recipe)) v.recipe = '';
    const current = v.recipe;
    recipeSel.replaceChildren(
      ...[new Option('Choose a vetted recipe', ''), ...vetted.map((r) => new Option(`${r.id} · ${r.name}`, r.id))],
    );
    recipeSel.value = current;
    const unvetted = recipes.filter((r) => !isVetted(r));
    recipeHint.textContent = vetted.length
      ? unvetted.length
        ? `Not vetted, so not selectable: ${unvetted.map((r) => r.id).join(', ')}.`
        : ''
      : 'No vetted recipe yet. The owner vets one on the Recipes tab.';

    replace(
      datasetBox,
      datasets.length
        ? datasets.map((d) =>
            check(
              d.name,
              v.datasets.includes(d.id),
              (on) => {
                toggle(v.datasets, d.id, on);
                sync();
              },
              [
                h('span.lab-mono.lab-muted', null, d.id),
                h('span.lab-muted', null, `${d.kind} · ${fmtNumber(d.rows)} rows`),
                d.frozen_rows > 0 ? h('span.lab-badge.accent', null, `frozen ${fmtNumber(d.frozen_rows)}`) : null,
              ],
            ),
          )
        : h('p.lab-muted', null, 'No datasets yet. Make one on the Datasets tab.'),
    );
    replace(
      machineBox,
      machines.length
        ? machines.map((m) =>
            check(
              m.id,
              v.machines.includes(m.id),
              (on) => {
                toggle(v.machines, m.id, on);
                sync();
              },
              [
                h('span.lab-muted', null, `${m.devices.length} device${m.devices.length === 1 ? '' : 's'}`),
                h('span.lab-badge', { class: m.online ? 'ok' : 'muted' }, m.online ? 'online' : 'offline'),
              ],
            ),
          )
        : h('p.lab-muted', null, 'No machines report devices.'),
    );
    for (const { name, box } of backendBoxes) box.checked = v.backends.includes(name);
    allowCpu.checked = v.allowCpu;
  }

  function previewNode(doc: JobDocument | null, key: string | null, fresh: boolean): Node {
    if (previewing) return h('p.lab-muted', null, 'Planning the job…');
    if (previewError && key !== null && previewError.key === key) return h('p.lab-error', null, previewError.message);
    if (preview && fresh) return planDiagram(preview.plan);
    if (preview) return h('p.lab-muted', null, 'The inputs changed since the last preview. Preview again before you submit.');
    if (!doc) return h('p.lab-muted', null, 'Fill in the form, then preview the plan. The plan shows where the job would run.');
    return h('p.lab-muted', null, 'Preview the plan to see where the job would run. Submit unlocks after a preview of these inputs.');
  }

  function confirmNode(doc: JobDocument): Node {
    const datasetsCount = doc.datasets.length;
    return h(
      'div.lab-jobs-confirm',
      null,
      h(
        'p',
        null,
        `Submit ${doc.recipe} on ${datasetsCount} dataset${datasetsCount === 1 ? '' : 's'} with a budget of ${doc.budget_gpu_hours} GPU-hours? The job dispatches to the owner's GPU machines now.`,
      ),
      h(
        'div.lab-row',
        null,
        button('Confirm submit', () => void confirmSubmit(), { kind: 'primary' }),
        button('Cancel', () => {
          confirming = false;
          sync();
        }),
      ),
    );
  }

  function submitNode(): Node | null {
    if (!submitError) return null;
    if (submitError.jobId) {
      return h(
        'div.lab-error',
        null,
        h('p', null, `The job was recorded as failed: ${submitError.message}`),
        h('a.lab-mono', { href: href('lab', 'jobs', submitError.jobId) }, `Open job ${submitError.jobId}`),
      );
    }
    return h('p.lab-error', null, submitError.message);
  }

  function sync(): void {
    const { doc, errors } = buildDocument(v);
    const key = doc ? JSON.stringify(doc) : null;
    const fresh = key !== null && preview !== null && preview.key === key;
    if (confirming && !fresh) confirming = false;
    replace(errorBox, attempted && errors.length ? h('ul.lab-jobs-errors', null, errors.map((e) => h('li', null, e))) : null);
    replace(previewBox, previewNode(doc, key, fresh));
    previewBtn.disabled = previewing || submitting;
    previewBtn.textContent = previewing ? 'Previewing' : 'Preview plan';
    submitBtn.disabled = !fresh || previewing || submitting;
    submitBtn.title = fresh ? '' : 'Preview the plan for these inputs first';
    submitBtn.textContent = submitting ? 'Submitting' : 'Submit';
    replace(confirmBox, confirming && doc ? confirmNode(doc) : null);
    replace(submitBox, submitNode());
  }

  async function runPreview(): Promise<void> {
    attempted = true;
    const { doc } = buildDocument(v);
    if (!doc) {
      sync();
      return;
    }
    const key = JSON.stringify(doc);
    previewing = true;
    previewError = null;
    submitError = null;
    sync();
    try {
      const plan = await planJob(doc);
      if (!alive) return;
      if (currentKey() === key) {
        preview = { key, plan };
        previewError = null;
      }
    } catch (err) {
      if (!alive) return;
      if (currentKey() === key) {
        preview = null;
        previewError = { key, message: errorText(err) };
      }
    }
    previewing = false;
    if (alive) sync();
  }

  function askSubmit(): void {
    if (!isFresh()) return;
    confirming = true;
    submitError = null;
    sync();
  }

  async function confirmSubmit(): Promise<void> {
    const { doc } = buildDocument(v);
    if (!doc || !isFresh()) {
      confirming = false;
      sync();
      return;
    }
    submitting = true;
    sync();
    try {
      const job = await submitJob(doc);
      if (!alive) return;
      toast(`Job ${job.id} submitted.`);
      onSubmitted(job.id);
      return;
    } catch (err) {
      if (!alive) return;
      submitError =
        err instanceof JobDispatchError ? { message: err.message, jobId: err.job.id } : { message: errorText(err), jobId: null };
    }
    submitting = false;
    confirming = false;
    if (alive) sync();
  }

  async function loadOptions(): Promise<void> {
    const [r, d, dev] = await Promise.allSettled([listRecipes(), listDatasets(), listDevices()]);
    if (!alive) return;
    const failed: string[] = [];
    if (r.status === 'fulfilled') recipes = r.value;
    else failed.push(`Recipes: ${errorText(r.reason)}`);
    if (d.status === 'fulfilled') datasets = d.value;
    else failed.push(`Datasets: ${errorText(d.reason)}`);
    if (dev.status === 'fulfilled') machines = dev.value.machines;
    else failed.push(`Devices: ${errorText(dev.reason)}`);
    optionsError.textContent = failed.join(' ');
    renderOptions();
    sync();
  }

  // The layout: each field once, the two actions, then the three result areas.
  const el = h(
    'section.lab-card.lab-jobs-form-card',
    null,
    h('div.lab-card-head', null, h('h3', null, 'New job')),
    h(
      'div.lab-card-body',
      null,
      h(
        'div.lab-jobs-form',
        null,
        field('Recipe', recipeSel, undefined),
        h('div.lab-jobs-hint', null, recipeHint),
        group('Datasets', datasetBox, 'Pick one or more. A frozen count is the rows the dataset has frozen.'),
        group('Search space', searchArea, 'Optional. A JSON object, or one key: value per line.'),
        h(
          'div.lab-jobs-pair',
          null,
          field('Budget (GPU-hours)', budgetIn, 'The job stops once it has used this much GPU time.'),
          field('Min devices', minDevIn, 'Optional. Whole number, 1 or more.'),
          field('Max devices', maxDevIn, 'Optional. Defaults to the min.'),
          field('Min memory (MiB)', minMemIn, 'Optional. Each device must have at least this much.'),
        ),
        h(
          'fieldset.lab-jobs-pin',
          null,
          h('legend', null, 'Device pin (optional)'),
          group('Backends', backendRow, 'None picked means any backend the planner allows.'),
          group('Mode', modeSel, 'Auto lets the planner choose between single, tight and loose.'),
          group('Machines', machineBox, 'None picked means any machine.'),
          h('label.lab-jobs-check', null, allowCpu, h('span.lab-jobs-check-name', null, 'Allow CPU devices')),
        ),
        group('Train overrides', trainArea, 'Optional. A JSON object, such as {"steps": 500}. Empty keeps the recipe defaults.'),
        optionsError,
        errorBox,
        h('div.lab-row.lab-jobs-actions', null, previewBtn, submitBtn),
        previewBox,
        confirmBox,
        submitBox,
      ),
    ),
  );

  renderOptions();
  sync();
  const stop = pollEvery(POLL_MS, () => void loadOptions());
  return {
    el,
    destroy: () => {
      alive = false;
      stop();
    },
  };
}

