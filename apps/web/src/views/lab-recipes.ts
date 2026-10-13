// The Lab's recipes (`#/lab/recipes`): each registered recipe with its pin, its vet and its schema.
// Vetting asks for a commit (checked here the way apid checks it), confirms, then records the vet.

import '../styles/lab-data.css';
import { listRecipes, vetRecipe, type Recipe } from '../api';
import { ago, button, clip, field, input, modal, toast } from '../ui/components';
import { h, keepState, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { errorText, fmtTime, labEmpty, pollEvery } from './lab-common';

type Facts = Record<string, unknown>;

const HEX_COMMIT = /^[0-9a-fA-F]{7,64}$/;
const DEV_COMMIT = /^dev-\S+$/;

/** Why a commit cannot be vetted, or null when it can: the rule apid applies to the same field. */
function commitProblem(commit: string): string | null {
  const c = commit.trim();
  if (!c) return 'Enter the commit to vet.';
  if (HEX_COMMIT.test(c) || DEV_COMMIT.test(c)) return null;
  return 'A commit is 7 to 64 hex characters, or dev-... for a local-dev pin.';
}

/** The commit as the list shows it: its first twelve characters. The full value is the tooltip. */
const shortCommit = (commit: string): string => (commit.length > 12 ? commit.slice(0, 12) : commit);

/** A state badge in the Lab's mono skeleton (`lab-badge`), toned ok, warn, info, accent or muted. */
const tag = (text: string, tone: 'ok' | 'warn' | 'accent' | 'muted' = 'muted'): HTMLElement => h('span.lab-badge', { class: tone }, text);

const asFacts = (v: unknown): Facts | null => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Facts) : null);
const names = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []);

function kv(label: string, value: Node | string): HTMLElement {
  return h('div', null, h('dt', null, label), h('dd', null, value));
}

/** The vet state: a badge, then who vetted it and when. */
function vetLine(r: Recipe): HTMLElement {
  if (!r.vetted_ms) return h('span', null, tag('not vetted', 'warn'));
  return h(
    'span',
    null,
    tag('vetted', 'ok'),
    ' by ',
    r.vetted_by ?? 'unknown',
    ' ',
    h('span.lab-muted', { title: fmtTime(r.vetted_ms) }, ago(r.vetted_ms)),
  );
}

/** One side of the schema: its title, a count note, and its feature names as chips (categorical ones dashed). */
function facet(title: string, note: string, chips: { name: string; cat?: boolean }[]): HTMLElement {
  return h(
    'div.lab-schema',
    null,
    h('div.lab-schema-head', null, title, ' · ', h('b', null, note)),
    h(
      'div.lab-chips',
      null,
      chips.map((c) => h('span.lab-chip', { class: c.cat ? 'cat' : '' }, c.name)),
    ),
  );
}

/** The recipe's input and output schema, summarised: the features (numeric, categorical) and the classes. */
function schemaBlock(schema: unknown): HTMLElement {
  const s = asFacts(schema);
  const input = asFacts(s?.input);
  const output = asFacts(s?.output);
  if (!input && !output) {
    if (!s || Object.keys(s).length === 0) return h('div.lab-schema', null, h('div.lab-schema-head', null, 'Schema'), h('span.lab-muted', null, 'Not declared.'));
    return h('div.lab-schema', null, h('div.lab-schema-head', null, 'Schema'), h('p.lab-raw', null, clip(JSON.stringify(s), 240)));
  }
  const numeric = names(input?.numeric);
  const categorical = names(input?.categorical);
  const notes: string[] = [];
  if (numeric.length) notes.push(`${numeric.length} numeric`);
  if (categorical.length) notes.push(`${categorical.length} categorical`);
  if (typeof input?.embedding_dim === 'number') notes.push(`embedding ${input.embedding_dim}`);
  if (typeof input?.hash_buckets === 'number') notes.push(`${input.hash_buckets} hash buckets`);
  const classes = names(output?.classes);
  return h(
    'div',
    null,
    facet('Inputs', notes.join(', ') || 'none', [...numeric.map((name) => ({ name })), ...categorical.map((name) => ({ name, cat: true }))]),
    facet('Outputs', classes.length ? `${classes.length} classes` : 'none', classes.map((name) => ({ name }))),
  );
}

/** One recipe: its name and local-dev badge, its facts, its schema, and the Vet action. */
function recipeCard(r: Recipe, onVet: (r: Recipe) => void): HTMLElement {
  return h(
    'article.lab-card.lab-recipe',
    null,
    h('header.lab-card-head', null, h('h3', null, r.name), r.local_dev ? tag('local dev', 'accent') : null),
    h(
      'div.lab-card-body',
      null,
      h(
        'dl.lab-kv',
        null,
        kv('Id', h('span.lab-mono', null, r.id)),
        kv('Repo', h('span.lab-mono', null, r.repo)),
        kv('Commit', h('span.lab-mono', { title: r.commit }, shortCommit(r.commit))),
        kv('Vetted', vetLine(r)),
      ),
      schemaBlock(r.schema),
      h('div.lab-recipe-foot', null, button('Vet', () => onVet(r), { icon: 'check', title: `Vet ${r.id}` })),
    ),
  );
}

/**
 * The vet dialog: a commit (prefilled with the recipe's pin), reviewed, then confirmed. The server's
 * refusal stays on the dialog; a recorded vet closes it and reloads the list.
 */
function vetDialog(r: Recipe, reload: () => void): void {
  const box = h('div.lab-form');
  const commit = input({ autocomplete: 'off', spellcheck: 'false', placeholder: '7 to 64 hex characters, or dev-...' });
  commit.value = r.commit;
  let close: () => void = () => undefined;
  let error: string | null = null;
  let busy = false;

  const doVet = async (target: string): Promise<void> => {
    if (busy) return;
    busy = true;
    error = null;
    show('confirm', target);
    try {
      const done = await vetRecipe(r.id, target);
      toast(`Vetted ${done.id} at ${shortCommit(done.commit)}`);
      close();
      reload();
    } catch (err) {
      busy = false;
      error = errorText(err);
      show('confirm', target);
    }
  };

  const show = (step: 'edit' | 'confirm', target = commit.value.trim()): void => {
    const note = error ? h('div.lab-error', null, error) : null;
    if (step === 'edit') {
      const hint = r.local_dev
        ? `A local-dev recipe vets only at its own pin, ${r.commit}.`
        : 'Seven to 64 hex characters of the commit, or dev-... for a local-dev pin.';
      replace(
        box,
        field('Commit', commit, hint),
        note,
        h(
          'div.lab-form-actions',
          null,
          button('Cancel', () => close(), { kind: 'ghost' }),
          button(
            'Review',
            () => {
              error = commitProblem(commit.value);
              show(error ? 'edit' : 'confirm');
            },
            { kind: 'primary' },
          ),
        ),
      );
      return;
    }
    replace(
      box,
      h('p', null, 'Vet ', h('span.lab-mono', null, r.id), ' at commit ', h('span.lab-mono', null, target), '? The vet records you as the vetter at this commit, so a job can use the recipe.'),
      note,
      h(
        'div.lab-form-actions',
        null,
        button('Back', () => show('edit'), { kind: 'ghost', disabled: busy }),
        button('Confirm vet', () => void doVet(target), { kind: 'primary', disabled: busy }),
      ),
    );
  };

  show('edit');
  close = modal(`Vet ${r.id}`, box);
}

export function render(root: HTMLElement, _params: string[]): Cleanup {
  let alive = true;
  let recipes: Recipe[] | null = null;
  let failure: string | null = null;
  const body = h('div.lab-stack');
  replace(root, body);

  const paint = (): void => {
    const restore = keepState(body);
    const count = recipes ? `${recipes.length} registered` : '';
    const nodes: Node[] = [h('div.lab-page-head', null, h('h2', null, 'Recipes'), h('div.lab-actions', null, h('span.lab-muted', null, count)))];
    if (failure) nodes.push(h('div.lab-error', null, failure));
    if (recipes && recipes.length === 0) {
      nodes.push(labEmpty('file', 'No recipes registered', 'Recipes come from trainer/recipes; a recipe must be vetted at a commit before a job can use it.'));
    } else if (recipes) {
      nodes.push(h('div.lab-grid.lab-recipes', null, recipes.map((r) => recipeCard(r, (picked) => vetDialog(picked, load)))));
    }
    replace(body, nodes);
    restore();
  };

  const load = (): void => {
    listRecipes().then(
      (list) => {
        if (!alive) return;
        recipes = list;
        failure = null;
        paint();
      },
      (err: unknown) => {
        if (!alive) return;
        failure = errorText(err);
        paint();
      },
    );
  };

  const stop = pollEvery(5000, load);
  return () => {
    alive = false;
    stop();
  };
}
