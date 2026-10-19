// The Lab's recipes (`#/lab/recipes`), in the prototype's layout: one card per registered recipe with
// its facts (id, repo, commit, vet) and its schema as chips. Vet opens a dialog: a commit (checked
// here the way apid checks it), a review line, then a confirm that records the vet.

import { listRecipes, vetRecipe, type Recipe } from '../api';
import { badge, card, field, input, modal, toast } from '../ui/components';
import { h, keepState, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { agoText, errorText, labButton, labEmpty, pollEvery } from './lab-common';

type Facts = Record<string, unknown>;

const HEX_COMMIT = /^[0-9a-fA-F]{7,64}$/;
const DEV_COMMIT = /^dev-\S+$/;

/** Why a commit cannot be vetted, or null when it can: the rule apid applies to the same field. */
function commitProblem(commit: string): string | null {
  const c = commit.trim();
  if (!c) return 'Enter the commit to vet.';
  if (HEX_COMMIT.test(c) || DEV_COMMIT.test(c)) return null;
  return 'Seven to 64 hex characters of the commit, or dev-... for a local-dev pin.';
}

/** The commit as the card shows it: its first twelve characters. The full value is the tooltip. */
const shortCommit = (commit: string): string => (commit.length > 12 ? commit.slice(0, 12) : commit);

const asFacts = (v: unknown): Facts | null => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Facts) : null);
const names = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []);

/** A fact cell: a mono-label over its value (the prototype's `lab2-fact`). */
function fact(label: string, value: Node | string, wide = false): HTMLElement {
  return h('div.lab2-fact', { class: wide ? 'lab2-wide' : null }, h('span.lab2-k', null, label), h('span.lab2-v', null, value));
}

/** The vet state: a warning badge when not vetted; else who vetted it and how long ago. */
function vetLine(r: Recipe): Node {
  if (!r.vetted_ms) return badge('not vetted', 'warn');
  return h('span', null, `vetted by ${r.vetted_by ?? 'unknown'} `, h('span.lab2-note', null, agoText(r.vetted_ms)));
}

/** The recipe's schema as chips: numeric and embedding and bucket counts, categorical features dashed, the classes in accent. */
function schemaChips(schema: unknown): Node[] {
  const s = asFacts(schema);
  const input = asFacts(s?.input);
  const output = asFacts(s?.output);
  const chips: Node[] = [];
  const numeric = typeof input?.numeric === 'number' ? input.numeric : names(input?.numeric).length;
  if (numeric) chips.push(badge(`${numeric} numeric`, 'muted'));
  for (const c of names(input?.categorical)) chips.push(h('span.chip.lab2-dash', null, c));
  if (typeof input?.embedding_dim === 'number') chips.push(badge(`embedding ${input.embedding_dim}`, 'info'));
  if (typeof input?.hash_buckets === 'number') chips.push(badge(`${input.hash_buckets.toLocaleString('en-US')} hash buckets`, 'muted'));
  for (const c of names(output?.classes)) chips.push(badge(c, 'accent'));
  return chips;
}

/** One recipe: a local-dev badge when it has one, its facts, its schema chips, and the Vet action. */
function recipeCard(r: Recipe, onVet: (r: Recipe) => void): HTMLElement {
  const chips = schemaChips(r.schema);
  const body = h(
    'div',
    null,
    r.local_dev ? h('div.lab2-row', { style: 'justify-content:flex-start;margin-bottom:8px' }, badge('local dev', 'info')) : null,
    h(
      'div.lab2-facts',
      null,
      fact('Id', h('span.mono.lab2-clip', { title: r.id }, r.id)),
      fact('Repo', h('span.mono.lab2-clip', { title: r.repo }, r.repo || '—')),
      fact('Commit', h('span.mono.lab2-clip', { title: r.commit }, shortCommit(r.commit))),
      fact('Vet', vetLine(r)),
      fact('Schema', chips.length ? h('div.lab2-row', { style: 'justify-content:flex-start' }, chips) : 'none', true),
    ),
  );
  return card(r.name || r.id, body, {
    actions: [labButton('Vet', () => onVet(r), { kind: 'ghost', sm: true, icon: 'check', title: `Vet ${r.id}` })],
  });
}

/**
 * The vet dialog: a commit (prefilled with the recipe's pin), reviewed, then confirmed. The server's
 * refusal stays on the dialog; a recorded vet closes it and reloads the list.
 */
function vetDialog(r: Recipe, reload: () => void): void {
  const box = h('div.lab2-form');
  const commit = input({ autocomplete: 'off', spellcheck: 'false', placeholder: '7 to 64 hex characters, or dev-...', 'aria-label': 'Commit' });
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
      toast(`Vetted ${r.id} at ${shortCommit(done.commit)}`);
      close();
      reload();
    } catch (err) {
      busy = false;
      error = errorText(err);
      show('confirm', target);
    }
  };

  const show = (step: 'edit' | 'confirm', target = commit.value.trim()): void => {
    const note = h('p.lab2-err', { role: 'alert' }, error ?? '');
    if (step === 'edit') {
      const hint = r.local_dev
        ? `A local-dev recipe vets only at its own pin, ${r.commit}.`
        : 'Seven to 64 hex characters of the commit, or dev-... for a local-dev pin.';
      replace(
        box,
        field('Commit', commit, hint),
        note,
        h(
          'div.lab-actions',
          null,
          labButton('Cancel', () => close(), { kind: 'ghost' }),
          labButton(
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
      h('p', null, 'Vet ', h('span.mono', null, r.id), ' at commit ', h('span.mono', null, target), '? The vet records you as the vetter at this commit, so a job can use the recipe.'),
      note,
      h(
        'div.lab-actions',
        null,
        labButton('Back', () => show('edit'), { kind: 'ghost', disabled: busy }),
        labButton('Confirm vet', () => void doVet(target), { kind: 'primary', disabled: busy }),
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
    const nodes: Node[] = [];
    if (failure) nodes.push(h('p.lab-err', { role: 'alert' }, failure));
    if (recipes && recipes.length === 0) {
      nodes.push(labEmpty('flask-conical', 'No recipes registered'));
    } else if (recipes) {
      nodes.push(h('div.lab2-grid', null, recipes.map((r) => recipeCard(r, (picked) => vetDialog(picked, load)))));
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
