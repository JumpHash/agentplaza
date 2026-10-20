// One model (`#/lab/models/<id>`), in the prototype's layout: the page head (name, id and version, the
// state and the actions), the identity and the shadow cards, the Try it box for a text model in shadow
// or promoted, then the metrics, calibration, input and output schemas, features and data provenance.
// Its actions are the registry's (shadow, promote, retire), shared with the list, and an imported
// model's rederive. A promoted model links to its model agent's page.

import { ApiError, getModel, type ModelView } from '../api';
import { href } from '../router';
import { badge, card, modal, textarea, toast } from '../ui/components';
import { h, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { agoText, errorText, fmtNumber, fmtSize, fmtTime, labButton, labEmpty, labRequest, pollEvery } from './lab-common';
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

/** A card with its title and body (the prototype's `h.card`). */
function panel(title: string, body: Node | Node[]): HTMLElement {
  return card(title, body);
}

/** Chips for a list of names, or a dash for none. */
function chips(names: string[]): Node | string {
  if (names.length === 0) return '—';
  return h('div.lab2-row', { style: 'justify-content:flex-start' }, names.map((n) => h('span.chip.lab2-dash', null, n)));
}

/** A dollar figure: cents, or four decimals under a cent. */
function usd(v: number): string {
  if (v === 0) return '$0.00';
  if (Math.abs(v) < 0.01) return `$${v.toFixed(4)}`;
  return `$${v.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

/** The model's identity and sources. A promoted or retired model keeps the agent it had. */
function identityCard(m: ModelView): HTMLElement {
  const c = m.card;
  const agent: Node | string = m.agent_cid !== null ? h('a', { href: href('agents', m.agent_cid) }, `CID ${m.agent_cid}`) : 'none';
  return panel(
    'Model',
    kv([
      ['ID', h('span.mono', null, m.id)],
      ['Version', c.version],
      ['State', h('span', null, stateBadge(m.state), ' ', h('span.lab2-note', null, fmtTime(m.state_changed_ms)))],
      ['Created', fmtTime(m.created_ms)],
      ['Source job', jobLink(m.source_job)],
      ['Model agent', agent],
      ['Servable', (m.state as string) === 'imported' ? (notServableReason(m) ?? 'No') : 'Yes'],
      ['Allowed roles', chips(c.allowed_roles)],
      ['Labels', c.labels && c.labels.length ? c.labels.join(', ') : 'none'],
      ['Size', fmtSize(m.size_bytes)],
      ['Params', m.parameters ? `${(m.parameters / 1e9).toFixed(1)} B` : '—'],
      ['ONNX sha256', h('span.mono', null, c.onnx_sha256 || '—')],
    ]),
  );
}

/** The shadow totals and the agreement bar. The registry keeps totals, not a history, so there is no chart over time. */
function shadowCard(m: ModelView): HTMLElement {
  const s = m.shadow;
  const rate = m.agreement_rate;
  const bar =
    rate === null
      ? null
      : h('div.lab2-bars', null, h('em', { style: 'display:block;height:6px;border-radius:3px;background:var(--sheet-2);overflow:hidden' }, h('i', { style: `display:block;height:100%;width:${(Math.max(0, Math.min(1, rate)) * 100).toFixed(1)}%;background:var(--signal)` })));
  return panel(
    'Shadow',
    h(
      'div',
      null,
      kv([
        ['Runs', fmtNumber(s.runs)],
        ['Agreed', fmtNumber(s.agreed)],
        ['Agreement', rate === null ? '—' : `${Math.round(rate * 100)}%`],
        ['Cost saved', usd(s.cost_saved_usd)],
        ['Updated', s.updated_ms ? agoText(s.updated_ms) : '—'],
      ]),
      bar,
    ),
  );
}

/** A text model's answer: labels with scores as bars (classification), or an embedding's size and first values. */
function renderOutput(output: unknown, ms: number): HTMLElement {
  const o = (output ?? {}) as { label?: unknown; scores?: unknown; embedding?: unknown };
  if (Array.isArray(o.embedding)) {
    const values = o.embedding as number[];
    const first = values.slice(0, 8).map((v) => v.toFixed(4)).join(', ');
    return h('div.lab2-out', null, `Embedding · ${fmtNumber(values.length)} dimensions · ${fmtNumber(ms)} ms\n${values.length > 8 ? `${first}, …` : first}`);
  }
  if (Array.isArray(o.scores)) {
    const scores = o.scores as { label: string; score: number }[];
    const label = String(o.label ?? scores[0]?.label ?? '—');
    return h(
      'div',
      null,
      h('div.lab2-row', { style: 'justify-content:flex-start' }, badge(label, 'accent')),
      h(
        'div.lab2-bars',
        null,
        scores.map((s) =>
          h(
            'div.lab2-bar',
            null,
            h('span', null, s.label),
            h('em', null, h('i', { style: `width:${(Math.max(0, Math.min(1, s.score)) * 100).toFixed(1)}%` })),
            h('span.num', null, `${(s.score * 100).toFixed(0)}%`),
          ),
        ),
      ),
      h('div.lab2-note', null, `${fmtNumber(ms)} ms`),
    );
  }
  return h('div.lab2-out', null, JSON.stringify(output));
}

/** Opens the rederive confirm: apid reads the model's Hugging Face download again and registers it afresh. */
function confirmRederive(model: ModelView, changed: (next: ModelView) => void, failed: (message: string) => void): void {
  const problem = h('p.lab2-err', { role: 'alert' });
  let close: () => void = () => undefined;
  const confirm = labButton('Rederive', () => {
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
    'div.lab2-form',
    null,
    h('p', null, `Rederive ${model.card.name} (${model.id}) from its Hugging Face download?`),
    h(
      'p.lab2-note',
      null,
      'The download is registered again: a model that now checks as servable is registered as one, otherwise it stays imported. If that fails, the record stays as it was.',
    ),
    problem,
    h('div.lab-actions', null, labButton('Cancel', () => close(), { kind: 'ghost' }), confirm),
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
      return panel('Try it', h('p.lab2-note', null, 'Shadow or promote this model to try it here.'));
    }
    if (tryEl) return tryEl;
    const box = textarea({ rows: 4, maxlength: 4000, 'aria-label': 'Text to try', placeholder: 'Text to run through the model' });
    const note = h('span.lab2-err', { role: 'alert' });
    const out = h('div');
    const run = labButton('Run', () => {
      const text = box.value.trim();
      if (!text) {
        note.textContent = 'Write some text to run.';
        return;
      }
      note.textContent = '';
      run.disabled = true;
      replace(out, h('div.lab2-out', null, 'Running'));
      labRequest<{ output: unknown; ms: number }>('POST', `/v1/foundry/models/${encodeURIComponent(m.id)}/invoke`, { text })
        .then((r) => replace(out, renderOutput(r.output, r.ms)))
        .catch((err: unknown) => {
          note.textContent = errorText(err);
          replace(out);
        })
        .finally(() => {
          run.disabled = false;
        });
    }, { kind: 'primary', icon: 'play' });
    tryEl = panel(
      `Try it · ${serving.task ?? 'text'}`,
      h(
        'div.lab2-form',
        null,
        box,
        h('div.lab2-row', null, note, h('span.sp'), run),
        out,
      ),
    );
    return tryEl;
  };

  const draw = (): void => {
    if (missing) {
      replace(root, labEmpty('triangle-alert', 'No such model'));
      return;
    }
    if (model === null) {
      replace(root, failure ? h('p.lab-err', { role: 'alert' }, failure) : h('p.lab-note', null, 'Loading model'));
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
    const actions = h('div.page-actions', null, modelActions(m, changed, failed));
    if ((m.state as string) === 'imported') {
      actions.append(labButton('Rederive', () => confirmRederive(m, changed, failed), { kind: 'ghost', sm: true, icon: 'refresh-cw' }));
    }
    const head = h(
      'header.page-head',
      null,
      h('div.page-head-row', null, h('h1', null, m.card.name), stateBadge(m.state), h('span.sp'), actions),
      h('p.lede', null, `${m.id} · version ${m.card.version}`),
    );
    const metrics = leaves(m.card.metrics).map((l) => {
      const split = l.path.length > 1 ? (l.path[0] ?? '—') : '—';
      const metric = (l.path.length > 1 ? l.path.slice(1) : l.path).join(' ') || '—';
      return [split, metric, h('span.num', null, valueText(l.value))] as (string | Node)[];
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
      return [h('span.mono', null, String(o.name ?? i)), String(o.kind ?? '—'), rest || '—'] as (string | Node)[];
    });
    const trial = tryBox(m);
    replace(
      root,
      head,
      problem ? h('p.lab-err', { role: 'alert' }, problem) : null,
      failure ? h('p.lab-err', { role: 'alert' }, failure) : null,
      h(
        'div.lab2-form',
        null,
        h('div.lab2-grid', null, identityCard(m), shadowCard(m)),
        trial,
        h('div.lab2-grid', null, panel('Metrics', dataTable(['Split', 'Metric', 'Value'], metrics, 'No metrics yet.')), panel('Calibration', dataTable(['Measure', 'Value'], calibration, 'No calibration yet.'))),
        h('div.lab2-grid', null, panel('Input schema', dataTable(['Field', 'Type', 'Required'], schemaRows(m.card.input), 'No input schema.')), panel('Output schema', dataTable(['Field', 'Type', 'Required'], schemaRows(m.card.output), 'No output schema.'))),
        h('div.lab2-grid', null, panel('Features', dataTable(['Feature', 'Kind', 'Details'], features, 'No features.')), panel('Data provenance', dataTable(['Split', 'Field', 'Value'], provenance, 'No provenance recorded.'))),
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
