// The Lab's model registry (`#/lab/models`): every registered model as a table or as cards, with
// its state, size, parameters, source job and (once in shadow) its agreement rate. Shadow, promote
// and retire act on a model in place; import registers a directory a training job wrote. The
// detail page (`#/lab/models/<id>`, lab-model-detail.ts) reuses this file's badge, actions and
// import dialog, so both pages agree on what a state means and what each action asks.

import '../styles/lab-models.css';
import { importModel, listModels, promoteModel, retireModel, shadowModel, type ModelState, type ModelView } from '../api';
import { href } from '../router';
import { hfPanel } from './lab-hf';
import { button, field, input, modal, tabs, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { errorText, fmtNumber, fmtPercent, fmtSize, fmtTime, labEmpty, labRequest, pollEvery } from './lab-common';

type View = 'table' | 'cards';

const POLL_MS = 5_000;
/** The states a model record can hold: the four registry states, and `imported` (registered, not servable yet). */
export const STATE_LABEL: Record<ModelState | 'imported', string> = { trained: 'Trained', shadow: 'Shadow', promoted: 'Promoted', retired: 'Retired', imported: 'Imported' };
/** Each state has its own colour: trained blue, shadow amber-orange, promoted green, retired grey. */
const STATE_TONE: Record<ModelState | 'imported', string> = { trained: 'info', shadow: 'accent', promoted: 'ok', retired: 'muted', imported: 'warn' };

/** The state badge: a square mark and the state's name, in the state's colour. */
export function stateBadge(state: string): HTMLElement {
  return h('span.lab-badge', { class: STATE_TONE[state as ModelState] ?? 'muted' }, STATE_LABEL[state as ModelState] ?? state);
}

/** Why an imported model is not servable yet: the reason its source record keeps, or null. */
export function notServableReason(m: ModelView): string | null {
  const source = (m as unknown as { source?: { servable_reason?: string | null } | null }).source;
  return source?.servable_reason ?? null;
}

/** The reason line under an imported model's state, or nothing for a servable model. */
export function reasonLine(m: ModelView): HTMLElement | null {
  if ((m.state as string) !== 'imported') return null;
  return h('div.lab-muted', null, notServableReason(m) ?? 'Not servable yet.');
}

/** A definition list: each pair is a mono label over its value. */
export function kv(pairs: [string, Node | string][]): HTMLElement {
  return h(
    'dl.lab-kv',
    null,
    pairs.flatMap(([label, value]) => [h('dt', null, label), h('dd', null, value)]),
  );
}

/** A link to a job's detail page. */
export function jobLink(id: string | null): Node | string {
  return id ? h('a.lab-mono', { href: href('lab', 'jobs', id) }, id) : '—';
}

/** The shadow agreement as a percent with its counts: `92% · 46 of 50`, or a dash before any run. */
export function agreementText(m: ModelView): string {
  if (m.agreement_rate === null || m.shadow.runs === 0) return '—';
  return `${fmtPercent(m.agreement_rate, 1)} · ${fmtNumber(m.shadow.agreed)} of ${fmtNumber(m.shadow.runs)}`;
}

/**
 * The actions a model offers, as the registry allows them: shadow a trained model, promote a model
 * in shadow, retire any model that is not retired yet. `changed` gets the model the server answered
 * with; `failed` gets the message of a refusal.
 */
export function modelActions(model: ModelView, changed: (next: ModelView) => void, failed: (message: string) => void): HTMLElement {
  const row = h('div.lab-row.lab-model-actions');
  if (model.state === 'trained') {
    row.append(button('Shadow', () => void run(() => shadowModel(model.id), `${model.id} is in shadow`, changed, failed), { size: 'sm', icon: 'play' }));
  }
  if (model.state === 'shadow') {
    row.append(button('Promote', () => confirmPromote(model, changed, failed), { kind: 'primary', size: 'sm', icon: 'check' }));
  }
  if (model.state !== 'retired') {
    row.append(button('Retire', () => confirmRetire(model, changed, failed), { kind: 'danger', size: 'sm', icon: 'ban' }));
  }
  return row;
}

async function run(act: () => Promise<ModelView>, done: string, changed: (next: ModelView) => void, failed: (message: string) => void): Promise<void> {
  try {
    const next = await act();
    toast(done);
    changed(next);
  } catch (err) {
    failed(errorText(err));
  }
}

/** Opens the promote dialog: what promotion does, then the owner's confirm. */
function confirmPromote(model: ModelView, changed: (next: ModelView) => void, failed: (message: string) => void): void {
  const problem = h('div.lab-error', { role: 'alert' });
  let close: () => void = () => undefined;
  const confirm = button('Promote', () => {
    confirm.disabled = true;
    problem.textContent = '';
    promoteModel(model.id)
      .then((next) => {
        close();
        toast(`${model.id} is promoted to a model agent`);
        changed(next);
      })
      .catch((err: unknown) => {
        confirm.disabled = false;
        problem.textContent = errorText(err);
        failed(errorText(err));
      });
  }, { kind: 'primary' });
  const body = h(
    'div.lab-stack',
    null,
    h('p', null, `Promote ${model.card.name} (${model.id}) to a model agent.`),
    h(
      'p.lab-muted',
      null,
      'Promotion gives the model its own agent on the roster: a CID, no VM, no wallet, no tools and no chat. Other agents can invoke it with invoke_model where its card allows their role. Nobody can assign it tasks or message it.',
    ),
    problem,
    h('div.lab-row', null, button('Cancel', () => close(), { kind: 'ghost' }), confirm),
  );
  close = modal('Promote model', body);
}

/** Opens the retire dialog; retiring a promoted model removes its model agent from the roster. */
function confirmRetire(model: ModelView, changed: (next: ModelView) => void, failed: (message: string) => void): void {
  const problem = h('div.lab-error', { role: 'alert' });
  let close: () => void = () => undefined;
  const confirm = button('Retire', () => {
    confirm.disabled = true;
    problem.textContent = '';
    retireModel(model.id)
      .then((next) => {
        close();
        toast(`${model.id} is retired`);
        changed(next);
      })
      .catch((err: unknown) => {
        confirm.disabled = false;
        problem.textContent = errorText(err);
        failed(errorText(err));
      });
  }, { kind: 'danger' });
  const agent = model.agent_cid !== null ? ` Its model agent (CID ${model.agent_cid}) leaves the roster.` : '';
  const body = h(
    'div.lab-stack',
    null,
    h('p', null, `Retire ${model.card.name} (${model.id})?`),
    h('p.lab-muted', null, `No agent can invoke it after this.${agent} The record and its shadow totals stay.`),
    problem,
    h('div.lab-row', null, button('Cancel', () => close(), { kind: 'ghost' }), confirm),
  );
  close = modal('Retire model', body);
}

/**
 * The import dialog: a directory's absolute path, registered by apid (it reads model.onnx and
 * model_card.json there). `done` runs once the model is registered.
 */
export function importDialog(done: (model: ModelView) => void): void {
  const path = input({ placeholder: '/absolute/path/to/a/job-directory', spellcheck: false, autocomplete: 'off' });
  const problem = h('div.lab-error', { role: 'alert' });
  let close: () => void = () => undefined;
  const submit = (): void => {
    const value = path.value.trim();
    problem.textContent = '';
    if (!value) {
      problem.textContent = 'Give the directory path.';
      return;
    }
    importButton.disabled = true;
    importModel(value)
      .then((model) => {
        close();
        toast(`Imported ${model.id}`);
        done(model);
      })
      .catch((err: unknown) => {
        importButton.disabled = false;
        problem.textContent = errorText(err);
      });
  };
  const importButton = button('Import', submit, { kind: 'primary', icon: 'plus' });
  path.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') submit();
  });
  const body = h(
    'div.lab-stack',
    null,
    field('Directory', path, 'An absolute path to a directory holding model.onnx and model_card.json.'),
    problem,
    h('div.lab-row', null, button('Cancel', () => close(), { kind: 'ghost' }), importButton),
  );
  close = modal('Import a model', body);
}

/** A plain table in the Lab style: a head over cells of text or nodes; `none` stands in for no rows. */
export function dataTable(heads: string[], rows: (string | Node)[][], none: string): HTMLElement {
  if (rows.length === 0) return h('p.lab-muted', null, none);
  return h(
    'div.lab-table-wrap',
    null,
    h(
      'table.lab-table.lab-model-table',
      null,
      h('thead', null, h('tr', null, heads.map((label) => h('th', null, label)))),
      h('tbody', null, rows.map((cells) => h('tr', null, cells.map((c) => h('td.lab-wrap', null, c))))),
    ),
  );
}

/** One row per model: name and id (linked), version, state, size, parameters, created, source job, agreement, actions. */
function modelTable(list: ModelView[], changed: () => void, failed: (message: string) => void): HTMLElement {
  const heads: [string, boolean][] = [['Model', false], ['Version', false], ['State', false], ['Size', true], ['Params', true], ['Created', false], ['Source job', false], ['Agreement', true], ['', false]];
  return h(
    'div.lab-table-wrap',
    null,
    h(
      'table.lab-table',
      null,
      h('thead', null, h('tr', null, heads.map(([label, num]) => h('th', { class: num ? 'num' : '' }, label)))),
      h(
        'tbody',
        null,
        list.map((m) => {
          const tr = h(
            'tr',
            { 'data-link': m.id },
            h('td.lab-id-cell', null, h('a.lab-nowrap', { href: href('lab', 'models', m.id) }, m.card.name), h('div.lab-mono.lab-muted.lab-ellipsis', { title: m.id }, m.id)),
            h('td', null, m.card.version),
            h('td', null, stateBadge(m.state), reasonLine(m)),
            h('td.num.lab-nowrap', null, fmtSize(m.size_bytes)),
            h('td.num', null, fmtNumber(m.parameters)),
            h('td', null, fmtTime(m.created_ms)),
            h('td', null, jobLink(m.source_job)),
            h('td.num', null, agreementText(m)),
            h('td.lab-actions-cell', null, modelActions(m, changed, failed)),
          );
          tr.addEventListener('click', (e) => {
            if ((e.target as Element).closest('button, a')) return;
            location.hash = href('lab', 'models', m.id);
          });
          return tr;
        }),
      ),
    ),
  );
}

/** One card per model: its name, state, the headline figures and its actions. */
function modelCards(list: ModelView[], changed: () => void, failed: (message: string) => void): HTMLElement {
  return h(
    'div.lab-grid.auto',
    null,
    list.map((m) =>
      h(
        'section.lab-card',
        null,
        h('header.lab-card-head', null, h('h3.lab-ellipsis', { title: m.card.name }, h('a', { href: href('lab', 'models', m.id) }, m.card.name)), stateBadge(m.state)),
        h(
          'div.lab-card-body',
          null,
          h('div.lab-mono.lab-muted.lab-ellipsis', { title: m.id }, `${m.id} · v${m.card.version}`),
          reasonLine(m),
          kv([
            ['Size', fmtSize(m.size_bytes)],
            ['Params', fmtNumber(m.parameters)],
            ['Created', fmtTime(m.created_ms)],
            ['Source job', jobLink(m.source_job)],
            ['Agreement', agreementText(m)],
          ]),
          modelActions(m, changed, failed),
        ),
      ),
    ),
  );
}

export function render(root: HTMLElement, _params: string[]): Cleanup {
  // A phone gets the cards (a nine-column table does not fit 390 px); the toggle still offers the table.
  let view: View = window.matchMedia?.('(max-width: 640px)').matches ? 'cards' : 'table';
  /** All records, or only the base models (`?kind=base`: the records imported as a fine-tune base). */
  let kind: 'all' | 'base' = 'all';
  let models: ModelView[] | null = null;
  let failure = '';
  let problem = '';
  const content = h('div.lab-stack');

  const draw = (): void => {
    if (models === null) {
      replace(content, failure ? h('p.lab-error', null, failure) : h('p.lab-muted', null, 'Loading models'));
      return;
    }
    if (models.length === 0 && kind === 'all') {
      const none = labEmpty('models', 'No models yet.', "A training job that finishes writes model.onnx and model_card.json; import that directory here, or wait for a breeder's job to finish.");
      none.append(button('Import a model', () => importDialog(() => void reload()), { kind: 'primary', icon: 'plus' }));
      replace(content, none);
      return;
    }
    const counts = {
      shadow: models.filter((m) => m.state === 'shadow').length,
      promoted: models.filter((m) => m.state === 'promoted').length,
    };
    const summary = h('p.lab-muted', null, `${fmtNumber(models.length)} ${models.length === 1 ? 'model' : 'models'} · ${fmtNumber(counts.shadow)} in shadow · ${fmtNumber(counts.promoted)} promoted`);
    const toolbar = h(
      'div.lab-row',
      null,
      tabs(
        [
          { key: 'table', label: 'Table' },
          { key: 'cards', label: 'Cards' },
        ],
        view,
        (key) => {
          view = key as View;
          draw();
        },
      ),
      tabs(
        [
          { key: 'all', label: 'All models' },
          { key: 'base', label: 'Base models' },
        ],
        kind,
        (key) => {
          kind = key as 'all' | 'base';
          models = null;
          draw();
          void reload();
        },
      ),
      summary,
    );
    const failed = (message: string): void => {
      problem = message;
      draw();
    };
    const changed = (): void => {
      problem = '';
      void reload();
    };
    replace(
      content,
      problem ? h('p.lab-error', { role: 'alert' }, problem) : null,
      toolbar,
      models.length === 0
        ? h('p.lab-muted', null, 'No base models yet. Import one from Hugging Face as base.')
        : view === 'table'
          ? modelTable(models, changed, failed)
          : modelCards(models, changed, failed),
    );
  };

  const reload = async (): Promise<void> => {
    try {
      models = kind === 'base' ? (await labRequest<{ models: ModelView[] }>('GET', '/v1/foundry/models?kind=base')).models : await listModels();
      failure = '';
    } catch (err) {
      failure = errorText(err);
    }
    draw();
  };

  const head = h(
    'div.lab-page-head',
    null,
    h('h2', null, 'Models'),
    h('div.lab-actions', null, button('Import', () => importDialog(() => void reload()), { icon: 'plus' })),
  );
  const hf = hfPanel(() => void reload());
  replace(root, head, content, hf.el);
  const stop = pollEvery(POLL_MS, () => void reload());
  return () => {
    stop();
    hf.stop();
  };
}
