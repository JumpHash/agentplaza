// The Lab's model registry (`#/lab/models`), in the prototype's layout: All models and Base models
// tabs, the summary and the Table or Cards view with Import, then the Hugging Face panel. Each model
// has Shadow, Promote and Retire in two fixed slots, so the same action sits in one column. The detail
// page (`#/lab/models/<id>`, lab-model-detail.ts) reuses this file's badge, actions and import dialog,
// so both pages agree on what a state means and what each action asks.

import { importModel, listModels, promoteModel, retireModel, shadowModel, type ModelState, type ModelView } from '../api';
import { href } from '../router';
import { hfPanel } from './lab-hf';
import { badge, card, field, input, modal, table as componentTable, toast, type Column, type Tone } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { errorText, fmtNumber, fmtPercent, fmtSize, fmtTime, labButton, labEmpty, labRequest, pollEvery, seg } from './lab-common';

type View = 'table' | 'cards';

const POLL_MS = 5_000;
/** The states a model record can hold: the four registry states, and `imported` (registered, not servable yet). */
export const STATE_LABEL: Record<ModelState | 'imported', string> = { trained: 'Trained', shadow: 'Shadow', promoted: 'Promoted', retired: 'Retired', imported: 'Imported' };
/** Each state has its own tone: trained blue, shadow accent, promoted green, retired grey, imported amber. */
const STATE_TONE: Record<ModelState | 'imported', Tone> = { trained: 'info', shadow: 'accent', promoted: 'ok', retired: 'muted', imported: 'warn' };

/** The state badge: the state's name, in the state's tone. */
export function stateBadge(state: string): HTMLElement {
  return badge(STATE_LABEL[state as ModelState] ?? state, STATE_TONE[state as ModelState] ?? 'muted');
}

/** Why an imported model is not servable yet: the reason its source record keeps, or null. */
export function notServableReason(m: ModelView): string | null {
  const source = (m as unknown as { source?: { servable_reason?: string | null } | null }).source;
  return source?.servable_reason ?? null;
}

/** The reason line under an imported model's state, or nothing for a servable model. */
export function reasonLine(m: ModelView): HTMLElement | null {
  if ((m.state as string) !== 'imported') return null;
  return h('div.lab2-note', null, notServableReason(m) ?? 'Not servable yet.');
}

/** A definition list: each pair is a label over its value (the prototype's `h.kv`). */
export function kv(pairs: [string, Node | string][]): HTMLElement {
  return h(
    'dl.kv',
    null,
    pairs.map(([label, value]) => h('div', null, h('dt', null, label), h('dd', null, value))),
  );
}

/** A link to a job's detail page. */
export function jobLink(id: string | null): Node | string {
  return id ? h('a.mono', { href: href('lab', 'jobs', id) }, id) : '—';
}

/** The shadow agreement as a percent with its counts: `92% · 46 of 50`, or a dash before any run. */
export function agreementText(m: ModelView): string {
  if (m.agreement_rate === null || m.shadow.runs === 0) return '—';
  return `${fmtPercent(m.agreement_rate, 1)} · ${fmtNumber(m.shadow.agreed)} of ${fmtNumber(m.shadow.runs)}`;
}

/**
 * The actions a model offers, as the registry allows them: shadow a trained model, promote a model
 * in shadow, retire any model that is not retired yet. They sit in two fixed slots (the stage action,
 * then Retire). `changed` gets the model the server answered with; `failed` gets the message of a refusal.
 */
export function modelActions(model: ModelView, changed: (next: ModelView) => void, failed: (message: string) => void): HTMLElement {
  const stage =
    model.state === 'trained'
      ? labButton('Shadow', () => void run(() => shadowModel(model.id), `${model.id} is in shadow`, changed, failed), { kind: 'ghost', sm: true })
      : model.state === 'shadow'
        ? labButton('Promote', () => confirmPromote(model, changed, failed), { kind: 'primary', sm: true })
        : null;
  const retire = model.state !== 'retired' ? labButton('Retire', () => confirmRetire(model, changed, failed), { kind: 'danger', sm: true }) : null;
  return h('div.lab2-acts', null, h('span', null, stage), h('span', null, retire));
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
  const problem = h('p.lab2-err', { role: 'alert' });
  let close: () => void = () => undefined;
  const confirm = labButton('Promote', () => {
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
    'div.lab2-form',
    null,
    h('p', null, `Promote ${model.card.name} (${model.id}) to a model agent.`),
    h(
      'p.lab2-note',
      null,
      'Promotion gives the model its own agent on the roster: a CID, no VM, no tools and no chat. Other agents can invoke it with invoke_model where its card allows their role. Nobody can assign it tasks or message it.',
    ),
    problem,
    h('div.lab-actions', null, labButton('Cancel', () => close(), { kind: 'ghost' }), confirm),
  );
  close = modal('Promote model', body);
}

/** Opens the retire dialog; retiring a promoted model removes its model agent from the roster. */
function confirmRetire(model: ModelView, changed: (next: ModelView) => void, failed: (message: string) => void): void {
  const problem = h('p.lab2-err', { role: 'alert' });
  let close: () => void = () => undefined;
  const confirm = labButton('Retire', () => {
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
    'div.lab2-form',
    null,
    h('p', null, `Retire ${model.card.name} (${model.id})?`),
    h('p.lab2-note', null, `No agent can invoke it after this.${agent} The record and its shadow totals stay.`),
    problem,
    h('div.lab-actions', null, labButton('Cancel', () => close(), { kind: 'ghost' }), confirm),
  );
  close = modal('Retire model', body);
}

/**
 * The import dialog: a directory's absolute path, registered by apid (it reads model.onnx and
 * model_card.json there). `done` runs once the model is registered.
 */
export function importDialog(done: (model: ModelView) => void): void {
  const path = input({ placeholder: '/absolute/path/to/a/job-directory', spellcheck: 'false', autocomplete: 'off', 'aria-label': 'Directory' });
  const problem = h('p.lab2-err', { role: 'alert' });
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
  const importButton = labButton('Import', submit, { kind: 'primary', icon: 'plus' });
  path.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') submit();
  });
  const body = h(
    'div.lab2-form',
    null,
    field('Directory', path, 'An absolute path to a directory holding model.onnx and model_card.json.'),
    problem,
    h('div.lab-actions', null, labButton('Cancel', () => close(), { kind: 'ghost' }), importButton),
  );
  close = modal('Import a model', body);
}

/** A plain table: a head over cells of text or nodes; `none` stands in for no rows. */
export function dataTable(heads: string[], rows: (string | Node)[][], none: string): HTMLElement {
  if (rows.length === 0) return h('p.lab2-note', null, none);
  const cols: Column<(string | Node)[]>[] = heads.map((label, i) => ({ label, cell: (row) => row[i] ?? null }));
  return h('div.tbl-wrap', null, componentTable(cols, rows));
}

/** One row per model: name and id (linked), version, state, size, parameters, created, source job, agreement, actions. */
function modelTable(list: ModelView[], changed: () => void, failed: (message: string) => void): HTMLElement {
  const cols: Column<ModelView>[] = [
    { label: 'Model', cell: (m) => h('div', null, h('a', { href: href('lab', 'models', m.id) }, h('strong', null, m.card.name)), h('div.mono.lab2-note', null, m.id)) },
    { label: 'Version', cell: (m) => m.card.version },
    { label: 'State', cell: (m) => h('div', null, stateBadge(m.state), reasonLine(m)) },
    { label: 'Size', num: true, cell: (m) => h('span.lab2-nw', null, fmtSize(m.size_bytes)) },
    { label: 'Params', num: true, cell: (m) => (m.parameters ? `${(m.parameters / 1e9).toFixed(1)} B` : '—') },
    { label: 'Created', cell: (m) => fmtTime(m.created_ms) },
    { label: 'Source job', cell: (m) => jobLink(m.source_job) },
    { label: 'Agreement', num: true, cell: (m) => agreementText(m) },
    { label: '', num: true, cell: (m) => modelActions(m, changed, failed) },
  ];
  return h('div.tbl-wrap', null, componentTable(cols, list));
}

/** One card per model: its name, state, id and version, the headline figures, and its actions. */
function modelCards(list: ModelView[], changed: () => void, failed: (message: string) => void): HTMLElement {
  return h(
    'div.lab2-grid',
    null,
    list.map((m) =>
      card(
        m.card.name,
        h(
          'div',
          null,
          h('div.lab2-row', { style: 'justify-content:flex-start' }, stateBadge(m.state), h('span.lab2-note.mono', null, `${m.id} · v${m.card.version}`)),
          reasonLine(m),
          kv([
            ['Size', fmtSize(m.size_bytes)],
            ['Params', m.parameters ? `${(m.parameters / 1e9).toFixed(1)} B` : '—'],
            ['Agreement', agreementText(m)],
          ]),
        ),
        { actions: [h('a.btn.ghost.sm', { href: href('lab', 'models', m.id) }, h('span', null, 'Open')), modelActions(m, changed, failed)] },
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
  const content = h('div.lab2-form');

  const draw = (): void => {
    if (models === null) {
      replace(content, failure ? h('p.lab-err', { role: 'alert' }, failure) : h('p.lab-note', null, 'Loading models'));
      return;
    }
    const importBtn = (): HTMLButtonElement => labButton('Import a model', () => importDialog(() => void reload()), { kind: 'primary', sm: true, icon: 'plus' });
    const counts = {
      shadow: models.filter((m) => m.state === 'shadow').length,
      promoted: models.filter((m) => m.state === 'promoted').length,
    };
    const tab = (key: string, label: string, n: number): HTMLElement => {
      const b = h('button.tab', { type: 'button', role: 'tab', 'aria-selected': key === kind ? 'true' : 'false' }, label, n ? ' ' : null, n ? h('span.count.num', null, String(n)) : null);
      b.addEventListener('click', () => {
        kind = key as 'all' | 'base';
        models = null;
        draw();
        void reload();
      });
      return b;
    };
    const summary = `${fmtNumber(models.length)} models · ${fmtNumber(counts.shadow)} in shadow · ${fmtNumber(counts.promoted)} promoted`;
    // The server answers the base list on its own (`?kind=base`), so only the open tab knows its count.
    const tabs = h('div.tabs', { role: 'tablist' }, tab('all', 'All models', kind === 'all' ? models.length : 0), tab('base', 'Base models', kind === 'base' ? models.length : 0));
    const bar = h(
      'div.lab2-row',
      null,
      h('span.lab2-note', null, summary),
      h('span.sp'),
      seg(
        [
          ['table', 'Table'],
          ['cards', 'Cards'],
        ],
        view,
        (key) => {
          view = key as View;
          draw();
        },
      ),
      labButton('Import', () => importDialog(() => void reload()), { kind: 'ghost', icon: 'plus', title: 'Import a model from a directory' }),
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
      problem ? h('p.lab-err', { role: 'alert' }, problem) : null,
      tabs,
      bar,
      models.length === 0
        ? labEmpty('cpu', kind === 'base' ? 'No base models yet' : 'No models yet', undefined, importBtn())
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

  const hf = hfPanel(() => void reload());
  replace(root, content, hf.el);
  const stop = pollEvery(POLL_MS, () => void reload());
  return () => {
    stop();
    hf.stop();
  };
}
