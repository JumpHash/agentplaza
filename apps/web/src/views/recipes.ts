// Recipes: the review recipes the hub knows (their steps, axes and rounds), the recipe changes waiting for the owner, and
// one recipe in full: its graph of steps, its TOML, and an edit that saves the next version. The hub checks the TOML and
// refuses a bad one; its reason shows under the editor. The Loom prototype's layout (the recipes part of m-review.js).

import '../styles/review.css';
import '../styles/proto/library.css';
import { ApiError } from '../api';
import { review, type NodeDef, type RecipeDetail, type RecipeList, type RecipeProposal, type RecipeSummary } from '../api-review';
import { go, href } from '../router';
import { attempt, badge, button, card, empty, table, textarea, toast, type Column } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import type { Cleanup } from './index';
import { axisName, bumpVersion, layerNodes, versionOf } from './review-model';

const POLL_MS = 8_000;
const SVG_NS = 'http://www.w3.org/2000/svg';

function svgEl(tag: string, attrs: Record<string, string | number>, cls?: string): SVGElement {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, String(v));
  if (cls) el.setAttribute('class', cls);
  return el;
}

/**
 * The steps of a recipe as a graph: each step in the column of its layer (its depth through `after`), a line from every
 * step it waits on, a dot per step with its name and kind under it.
 */
function recipeGraph(nodes: NodeDef[]): SVGSVGElement {
  const layers = layerNodes(nodes);
  const columns = new Map<number, NodeDef[]>();
  for (const n of nodes) {
    const l = layers.get(n.id) ?? 0;
    columns.set(l, [...(columns.get(l) ?? []), n]);
  }
  const colCount = Math.max(0, ...layers.values()) + 1;
  const tallest = Math.max(1, ...[...columns.values()].map((c) => c.length));
  const COL = 170;
  const ROW = 66;
  const LEFT = 84;
  const height = 40 + tallest * ROW;
  const width = LEFT * 2 + (colCount - 1) * COL;
  const at = new Map<string, [number, number]>();
  for (const [layer, column] of columns) {
    column.forEach((n, i) => {
      const y = height / 2 + (i - (column.length - 1) / 2) * ROW;
      at.set(n.id, [LEFT + layer * COL, y]);
    });
  }
  const svg = svgEl('svg', { viewBox: `0 0 ${width} ${height}`, role: 'img', 'aria-label': 'The recipe, step by step' }, 'rc-graph') as SVGSVGElement;
  for (const n of nodes) {
    const [x1, y1] = at.get(n.id) ?? [0, 0];
    for (const p of n.after) {
      const [x0, y0] = at.get(p) ?? [x1, y1];
      svg.append(svgEl('line', { x1: x0, y1: y0, x2: x1, y2: y1 }, 'rc-edge'));
    }
  }
  for (const n of nodes) {
    const [x, y] = at.get(n.id) ?? [0, 0];
    const kind = n.kind === 'gate' ? 'gate' : n.tool === 'tree' || n.kind === 'tool' ? 'tool' : '';
    const g = svgEl('g', {}, 'rc-node');
    const title = svgEl('title', {});
    title.textContent = [n.id, n.kind, n.role ? `reviewer: ${n.role}` : null, n.axes.length ? `scores ${n.axes.map((a) => axisName(a)).join(', ')}` : null]
      .filter(Boolean)
      .join(' · ');
    g.append(title, svgEl('circle', { cx: x, cy: y, r: kind === 'gate' ? 11 : 9 }, `rc-dot ${kind}`));
    const label = svgEl('text', { x, y: y - 16, 'text-anchor': 'middle' }, 'rc-label');
    label.textContent = n.id;
    const sub = svgEl('text', { x, y: y + 23, 'text-anchor': 'middle' }, 'rc-sub');
    sub.textContent = n.role ?? n.kind;
    g.append(label, sub);
    svg.append(g);
  }
  return svg;
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const id = params[0] ?? '';
  const view = h('div.rc');
  root.append(view);

  let alive = true;
  let loaded = false;
  let failed: string | null = null;
  let list: RecipeList | null = null;
  let detail: RecipeDetail | null = null;
  let editing = false;
  let saving = false;
  let saveError: string | null = null;
  let timer: number | undefined;
  // The editor survives the redraws of the page while the owner types into it.
  const editor = textarea({ rows: 26, spellcheck: 'false', 'aria-label': 'The recipe, as TOML', class: 'rc-editor' });

  const decide = async (p: RecipeProposal, approve: boolean): Promise<void> => {
    await attempt(() => review.decideProposal(p.id, approve), approve ? 'Approved' : 'Declined');
    void load();
  };

  const proposalCard = (p: RecipeProposal): HTMLElement => {
    const decline = button('Decline', () => void decide(p, false), { kind: 'ghost' });
    const approve = button('Approve', () => void decide(p, true), { kind: 'primary' });
    const meta = [p.recipe ? `recipe ${p.recipe}${p.version ? ` v${p.version}` : ''}` : null, p.by ? `by agent ${p.by}` : null].filter(Boolean).join(' · ');
    const el = card(
      h('strong', null, p.title || p.recipe || p.id),
      h('div', null, p.reason ? h('p', null, p.reason) : null, h('p.muted', null, meta), h('div.rc-actions', null, decline, approve)),
    );
    el.classList.add('rc-prop');
    return el;
  };

  const listPage = (data: RecipeList): HTMLElement => {
    const cols: Column<RecipeSummary>[] = [
      { label: 'Recipe', cell: (r) => h('div', null, h('strong', null, r.title || r.id), h('div.mono.pr-note', null, r.id)) },
      { label: 'Version', num: true, cell: (r) => `v${r.version}` },
      { label: 'Axes', num: true, cell: (r) => r.axes.length },
      { label: 'Steps', num: true, cell: (r) => r.nodes },
    ];
    return h(
      'div.rc',
      null,
      h('header.page-head', null, h('div.page-head-row', null, h('h1', null, 'Recipes'), h('span.sp'), h('div.page-actions'))),
      data.proposals.length
        ? h('section.rc-block', null, h('h2', null, 'Waiting for you'), data.proposals.map((p) => proposalCard(p)))
        : null,
      h(
        'section.rc-block',
        null,
        h('h2', null, 'Recipes'),
        data.recipes.length
          ? h('div.tbl-wrap', null, table(cols, data.recipes, (r) => go('recipes', r.id), (r) => r.id))
          : empty('rules', 'No recipes yet', 'A recipe is the set of steps a change is reviewed with.'),
      ),
    );
  };

  const save = async (current: number): Promise<void> => {
    if (saving) return;
    saving = true;
    saveError = null;
    draw();
    const next = bumpVersion(editor.value, current);
    try {
      await review.putRecipe(id, next);
      editing = false;
      toast(`Saved as version ${versionOf(next) ?? current + 1}`);
    } catch (err) {
      saveError = err instanceof Error ? err.message : String(err);
    } finally {
      saving = false;
    }
    await load();
  };

  const recipePage = (d: RecipeDetail): HTMLElement => {
    const r = d.recipe;
    const current = r.version;
    const startEdit = (): void => {
      editing = true;
      saveError = null;
      editor.value = d.toml;
      draw();
    };
    const stopEdit = (): void => {
      editing = false;
      saveError = null;
      draw();
    };
    const body = editing
      ? h(
          'section.rc-block',
          null,
          h('h2', null, 'Edit'),
          editor,
          saveError ? h('p.rc-error', { role: 'alert' }, saveError) : null,
          h(
            'div.rc-actions',
            null,
            h('span.muted', null, 'The hub checks the TOML before it keeps a new version.'),
            button('Cancel', stopEdit, { kind: 'ghost', disabled: saving }),
            button(`Save as version ${current + 1}`, () => void save(current), { kind: 'primary', disabled: saving }),
          ),
        )
      : h('section.rc-block', null, h('h2', null, 'Recipe, as written'), h('pre.rc-toml', null, d.toml));
    return h(
      'div.rc',
      null,
      h('p', null, h('a.btn.ghost.sm', { href: href('recipes') }, lu('arrow-left', 14), h('span', null, 'Recipes'))),
      h(
        'div.rc-head',
        null,
        h(
          'div',
          null,
          h('h1', null, r.title || r.id),
          h(
            'div.rc-meta',
            null,
            h('code', null, r.id),
            badge(`version ${r.version}`, 'info'),
            h('span.muted', null, r.applies.kinds.join(', ') || 'any kind'),
            h('span.muted', null, `${r.rounds} ${r.rounds === 1 ? 'round' : 'rounds'}`),
          ),
        ),
        editing ? null : button('Edit', startEdit),
      ),
      h('section.rc-block', null, h('h2', null, 'Steps'), recipeGraph(r.node)),
      h('section.rc-block', null, h('h2', null, 'Axes'), h('div.rc-chips', null, r.axes.map((a) => h('span.rc-chip', { title: axisName(a) }, a)))),
      body,
    );
  };

  const draw = (): void => {
    if (!alive) return;
    if (id) {
      if (detail) return replace(view, recipePage(detail));
      if (!loaded) return replace(view, h('p.muted', null, 'Loading the recipe…'));
      return replace(view, empty('alert', failed ? failed : 'Recipe not found.'));
    }
    if (list) return replace(view, listPage(list));
    replace(view, failed ? empty('alert', failed) : h('p.muted', null, 'Loading the recipes…'));
  };

  // A failed read keeps what is on screen; it only shows its own message when there is nothing to show.
  const load = async (): Promise<void> => {
    try {
      if (id) detail = await review.recipe(id);
      else list = await review.recipes();
      failed = null;
    } catch (err) {
      const notFound = err instanceof ApiError && err.status === 404;
      if (id) {
        if (notFound || !detail) {
          detail = null;
          failed = notFound ? 'Recipe not found.' : 'Could not load the recipe.';
        }
      } else if (!list) failed = 'Could not load the recipes.';
    }
    loaded = true;
    if (alive) draw();
  };

  void load();
  timer = window.setInterval(() => {
    if (!editing && !saving) void load();
  }, POLL_MS);

  return () => {
    alive = false;
    window.clearInterval(timer);
  };
}
