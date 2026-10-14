// One model (`#/lab/models/<id>`): its card (identity, source job, state, size, parameters), the
// metrics per split, the calibration, the shadow totals and agreement, the input and output schemas,
// the features, and the data provenance per split. Its actions are the registry's (shadow, promote,
// retire), shared with the list, and an imported model's rederive. A promoted model links to its model
// agent's page.

import '../styles/lab-models.css';
import { ApiError, getModel, type ModelView } from '../api';
import { href } from '../router';
import { ago, button, modal, textarea, toast } from '../ui/components';
import { h, icon, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { errorText, fmtNumber, fmtPercent, fmtSize, fmtTime, labRequest, pollEvery } from './lab-common';
import { dataTable, jobLink, kv, modelActions, notServableReason, stateBadge } from './lab-models';

const POLL_MS = 5_000;

/** One leaf of a nested record: its path of keys and its value. */
interface Leaf { path: string[]; value: unknown }

/** The leaves of a nested record (objects recurse; arrays and scalars are leaves). */
export function leaves(value: unknown, path: string[] = []): Leaf[] {
  if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
    return Object.entries(value as Record<string, unknown>).flatMap(([key, v]) => leaves(v, [...path, key]));
  }
  return [{ path, value }];
}

/** A value as text: integers grouped, fractions to four significant digits, lists joined, objects as JSON. */
export function valueText(value: unknown): string {
  if (value === null || value === undefined) return '—';
  if (typeof value === 'number') return Number.isInteger(value) ? fmtNumber(value) : String(Number(value.toPrecision(4)));
  if (typeof value === 'boolean') return value ? 'yes' : 'no';
  if (typeof value === 'string') return value;
  if (Array.isArray(value)) return value.map(valueText).join(', ');
  return JSON.stringify(value);
}

/** A JSON schema property's type: its enum when it has one, else its `type`. */
function typeOf(prop: unknown): string {
  const p = (prop ?? {}) as { type?: unknown; enum?: unknown[] };
  if (Array.isArray(p.enum)) return `one of ${p.enum.map(valueText).join(', ')}`;
  if (typeof p.type === 'string') return p.type;
  if (Array.isArray(p.type)) return p.type.join(' | ');
  return 'any';
}

/** A JSON schema's properties as rows: name, type, required. A schema without properties shows its leaves. */
function schemaRows(schema: unknown): string[][] {
  const s = (schema ?? {}) as { properties?: Record<string, unknown>; required?: string[] };
  if (!s.properties) return leaves(schema).map((l) => [l.path.join(' ') || '—', valueText(l.value), '—']);
  const required = new Set(s.required ?? []);
  return Object.entries(s.properties).map(([name, prop]) => [name, typeOf(prop), required.has(name) ? 'yes' : 'no']);
}

/** A card: a mono head over its body, as the Lab's cards. */
function panel(title: string, body: Node | Node[]): HTMLElement {
  return h('section.lab-card', null, h('header.lab-card-head', null, h('h3', null, title)), h('div.lab-card-body', null, body));
}

/** Chips for a list of names, or a dash for none. */
function chips(names: string[]): Node | string {
  if (names.length === 0) return '—';
  return h('div.lab-chips', null, names.map((n) => h('span.lab-chip', null, n)));
}

/** A dollar figure: cents, or four decimals under a cent. */
function usd(v: number): string {
  if (v === 0) return '$0.00';
  if (Math.abs(v) < 0.01) return `$${v.toFixed(4)}`;
  return `$${v.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

function stat(label: string, value: string): HTMLElement {
  return h('div.lab-stat', null, h('span.lab-stat-label', null, label), h('span.lab-stat-value', null, value));
}

/** The model's identity and sources. A promoted or retired model keeps the agent it had. */
function identityCard(m: ModelView): HTMLElement {
  const c = m.card;
  const agent: Node | string =
    m.agent_cid !== null ? h('a', { href: href('agents', m.agent_cid) }, `Open the model agent (CID ${m.agent_cid})`) : '—';
  return panel(
    'Model',
    kv([
      ['ID', h('span.lab-mono', null, m.id)],
      ['Version', c.version],
      ['State', stateBadge(m.state)],
      ['State changed', fmtTime(m.state_changed_ms)],
      ['Created', fmtTime(m.created_ms)],
      ['Source job', jobLink(m.source_job)],
      ['Model agent', agent],
      ['Servable', (m.state as string) === 'imported' ? (notServableReason(m) ?? 'no') : 'yes'],
      ['Allowed roles', chips(c.allowed_roles)],
      ['Labels', chips(c.labels ?? [])],
      ['Size', fmtSize(m.size_bytes)],
      ['Parameters', fmtNumber(m.parameters)],
      ['ONNX sha256', h('span.lab-mono', null, c.onnx_sha256 || '—')],
    ]),
  );
}

/** The shadow totals and the agreement meter. The registry keeps totals, not a history, so there is no chart over time. */
function shadowCard(m: ModelView): HTMLElement {
  const s = m.shadow;
  const rate = m.agreement_rate;
  const meter =
    rate === null
      ? h('p.lab-muted', null, 'No shadow runs yet.')
      : h('div.lab-meter', { role: 'meter', 'aria-valuemin': 0, 'aria-valuemax': 1, 'aria-valuenow': rate }, h('span', { style: `width:${Math.max(0, Math.min(1, rate)) * 100}%` }));
  return panel('Shadow', [
    h(
      'div.lab-stats',
      null,
      stat('Runs', fmtNumber(s.runs)),
      stat('Agreed', fmtNumber(s.agreed)),
      stat('Agreement', rate === null ? '—' : fmtPercent(rate, 1)),
      stat('Cost saved', usd(s.cost_saved_usd)),
      stat('Updated', s.updated_ms ? ago(s.updated_ms) : 'never'),
    ),
    h('div.lab-stack-tight', null, meter),
    h('p.lab-note', null, 'Totals since the model entered shadow. The registry keeps no per-run history, so there is no chart over time.'),
  ]);
}

/** A text model's answer: labels with scores (classification), or an embedding's size and first values. */
function renderOutput(output: unknown, ms: number): HTMLElement {
  const o = (output ?? {}) as { label?: unknown; scores?: unknown; embedding?: unknown };
  if (Array.isArray(o.embedding)) {
    const values = o.embedding as number[];
    const first = values.slice(0, 8).map((v) => v.toFixed(4)).join(', ');
    return h(
      'div.lab-stack-tight',
      null,
      h('p.lab-mono', null, `Embedding · ${fmtNumber(values.length)} dimensions · ${fmtNumber(ms)} ms`),
      h('p.lab-mono.lab-muted', null, values.length > 8 ? `${first}, …` : first),
    );
  }
  if (Array.isArray(o.scores)) {
    const scores = o.scores as { label: string; score: number }[];
    return h(
      'div.lab-stack-tight',
      null,
      h('p.lab-mono', null, `${String(o.label ?? scores[0]?.label ?? '—')} · ${fmtNumber(ms)} ms`),
      scores.map((s) =>
        h(
          'div.lab-try-score',
          null,
          h('span.lab-mono', null, s.label),
          h('div.lab-meter', null, h('span', { style: `width:${Math.max(0, Math.min(1, s.score)) * 100}%` })),
          h('span.lab-mono', null, fmtPercent(s.score, 2)),
        ),
      ),
    );
  }
  return h('pre.lab-mono', null, JSON.stringify(output));
}

/** The Rederive button of an imported model: it opens a confirm, then re-registers the model from its download. */
function rederiveButton(model: ModelView, changed: (next: ModelView) => void, failed: (message: string) => void): HTMLElement {
  return button('Rederive', () => confirmRederive(model, changed, failed), { size: 'sm', icon: 'refresh' });
}

/** Opens the rederive confirm: apid reads the model's Hugging Face download again and registers it afresh. */
function confirmRederive(model: ModelView, changed: (next: ModelView) => void, failed: (message: string) => void): void {
  const problem = h('div.lab-error', { role: 'alert' });
  let close: () => void = () => undefined;
  const confirm = button('Rederive', () => {
    confirm.disabled = true;
    problem.textContent = '';
    labRequest<{ model: ModelView }>('POST', `/v1/foundry/models/${encodeURIComponent(model.id)}/rederive`)
      .then((answer) => {
        close();
        toast(`${model.id} is rederived`);
        changed(answer.model);
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
    h('p', null, `Rederive ${model.card.name} (${model.id}) from its Hugging Face download?`),
    h(
      'p.lab-muted',
      null,
      'The download is registered again: a model that now checks as servable is registered as one, otherwise it stays imported. If that fails, the record stays as it was.',
    ),
    problem,
    h('div.lab-row', null, button('Cancel', () => close(), { kind: 'ghost' }), confirm),
  );
  close = modal('Rederive model', body);
}

export function render(root: HTMLElement, params: string[]): Cleanup {
  const id = params[0] ?? '';
  let model: ModelView | null = null;
  let missing = false;
  let failure = '';
  let problem = '';
  /** The model as last drawn, so a poll that changes nothing does not redraw (and keeps the Try it box as it is). */
  let lastJson = '';
  /** The Try it box, built once per visit and kept across redraws so its text and answer survive. */
  let tryEl: HTMLElement | null = null;

  /** A "Try it" box for a text model in shadow or promoted: its text goes to invoke, its answer shows below. */
  const tryBox = (m: ModelView): HTMLElement | null => {
    const serving = (m.card as unknown as { serving?: { kind?: string; task?: string; max_len?: number } | null }).serving;
    if (serving?.kind !== 'text') return null;
    if (m.state !== 'shadow' && m.state !== 'promoted') {
      return panel('Try it', h('p.lab-muted', null, 'Shadow or promote this model to try it here.'));
    }
    if (tryEl) return tryEl;
    const box = textarea({ rows: 3, placeholder: 'Text to run through the model', maxlength: 4000 });
    const out = h('div.lab-stack-tight');
    const run = button('Run', () => {
      const text = box.value.trim();
      if (!text) {
        replace(out, h('p.lab-error', { role: 'alert' }, 'Type some text first.'));
        return;
      }
      run.disabled = true;
      replace(out, h('p.lab-muted', null, 'Running'));
      labRequest<{ output: unknown; ms: number }>('POST', `/v1/foundry/models/${encodeURIComponent(m.id)}/invoke`, { text })
        .then((r) => replace(out, renderOutput(r.output, r.ms)))
        .catch((err: unknown) => replace(out, h('p.lab-error', { role: 'alert' }, errorText(err))))
        .finally(() => {
          run.disabled = false;
        });
    }, { kind: 'primary', size: 'sm', icon: 'play' });
    tryEl = panel(`Try it · ${serving.task ?? 'text'}`, [
      h('p.lab-muted', null, `The text is cut to ${serving.max_len ?? '—'} tokens. Each run is a real call to the model.`),
      box,
      h('div.lab-row', null, run),
      out,
    ]);
    return tryEl;
  };

  const notFound = (): HTMLElement =>
    h(
      'div.lab-empty',
      null,
      icon('models'),
      h('div.lab-empty-title', null, 'No such model'),
      h('div.lab-empty-text', null, 'No model with this id. ', h('a', { href: href('lab', 'models') }, 'Go back to Models.')),
    );

  const draw = (): void => {
    if (missing) {
      replace(root, notFound());
      return;
    }
    if (model === null) {
      replace(root, failure ? h('p.lab-error', null, failure) : h('p.lab-muted', null, 'Loading model'));
      return;
    }
    const m = model;
    const failed = (message: string): void => {
      problem = message;
      draw();
    };
    const changed = (next: ModelView): void => {
      model = next;
      problem = '';
      draw();
      void load();
    };
    const actions = modelActions(m, changed, failed);
    if ((m.state as string) === 'imported') actions.append(rederiveButton(m, changed, failed));
    const head = h('div.lab-page-head', null, h('h2', null, m.card.name), h('div.lab-actions', null, stateBadge(m.state), actions));
    const metrics = leaves(m.card.metrics).map((l) => {
      const split = l.path.length > 1 ? (l.path[0] ?? '—') : '—';
      const metric = (l.path.length > 1 ? l.path.slice(1) : l.path).join(' ') || '—';
      return [split, metric, valueText(l.value)];
    });
    const calibration = leaves(m.calibration).map((l) => [l.path.join(' ') || '—', valueText(l.value)]);
    const provenance = leaves(m.provenance).map((l) => {
      const split = l.path.length > 1 ? (l.path[0] ?? '—') : '—';
      const field = (l.path.length > 1 ? l.path.slice(1) : l.path).join(' ') || '—';
      return [split, field, valueText(l.value)];
    });
    const features = m.card.features.map((f, i) => {
      const o = (f ?? {}) as Record<string, unknown>;
      const rest = Object.entries(o)
        .filter(([k]) => k !== 'name' && k !== 'kind')
        .map(([k, v]) => `${k}: ${valueText(v)}`)
        .join(', ');
      return [String(o.name ?? i), String(o.kind ?? '—'), rest || '—'];
    });
    const back = h('a.lab-back', { href: href('lab', 'models') }, 'Back to Models');
    replace(
      root,
      back,
      head,
      h('p.lab-mono.lab-muted', null, m.id),
      problem ? h('p.lab-error', { role: 'alert' }, problem) : null,
      failure ? h('p.lab-error', { role: 'alert' }, failure) : null,
      tryBox(m),
      h(
        'div.lab-grid.cols-2',
        null,
        identityCard(m),
        shadowCard(m),
        panel('Metrics', dataTable(['Split', 'Metric', 'Value'], metrics, 'No metrics recorded.')),
        panel('Calibration', dataTable(['Measure', 'Value'], calibration, 'No calibration recorded.')),
        panel('Input schema', dataTable(['Field', 'Type', 'Required'], schemaRows(m.card.input), 'No input schema.')),
        panel('Output schema', dataTable(['Field', 'Type', 'Required'], schemaRows(m.card.output), 'No output schema.')),
        panel('Features', dataTable(['Name', 'Kind', 'Details'], features, 'No features.')),
        panel('Data provenance', dataTable(['Split', 'Field', 'Value'], provenance, 'No provenance recorded.')),
      ),
    );
  };

  const load = async (): Promise<void> => {
    try {
      const next = await getModel(id);
      const json = JSON.stringify(next);
      const same = json === lastJson && !missing && !failure;
      lastJson = json;
      model = next;
      missing = false;
      failure = '';
      if (same) return;
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) {
        missing = true;
        model = null;
      } else {
        failure = errorText(err);
      }
    }
    draw();
  };

  const stop = pollEvery(POLL_MS, () => void load());
  return () => stop();
}
