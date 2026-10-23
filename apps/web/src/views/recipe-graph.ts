// The recipe graph editor: the engine's recipes (`#/recipe-graph`), and one of them as its steps and edges
// (`#/recipe-graph/<id>`). The graph is drawn the way the Work screen draws a task graph: the wasm layout places the
// cards, each link is a curve with an arrowhead, and a loop curls under its cards, dashed, with its bound as its label.
// A step's fields and the edges are edited in the side panel. Nothing is saved until Save: the draft goes out as TOML
// with the next version, and the hub's refusal shows under the editor.

import '../styles/proto/work.css';
import '../styles/recipe-graph.css';
import { ApiError } from '../api';
import { review, type EngineEdge, type EngineRecipe, type EngineRecipeSummary, type EngineStep } from '../api-review';
import { loadGraph, type GraphLayout, type Layout } from '../graph/wasm';
import { go, href } from '../router';
import { badge, button, empty, field, input, select, table, toast, type Column } from '../ui/components';
import { h, lu, replace } from '../ui/dom';
import type { Cleanup } from './index';
import {
  addEdge,
  addStep,
  cloneRecipe,
  edgeLabel,
  isLoop,
  layoutInput,
  listOf,
  MODEL_WORDS,
  modelText,
  nextStepId,
  pairKey,
  parseCount,
  parseFanOut,
  parseShare,
  patchStep,
  problemOf,
  recipeToml,
  removeEdge,
  removeStep,
  renameStep,
  stepLine,
  WHEN_OPTIONS,
} from './recipe-graph-model';

const SVG_NS = 'http://www.w3.org/2000/svg';
/** A step's card, and the gaps and padding the layout keeps, in CSS pixels (the Work screen's sizes, a little smaller). */
const CW = 200;
const CH = 96;
const CARD = { card_width: CW, card_height: CH, gap_x: 56, gap_y: 18, pad: 16 };
/** How far a loop curls below the cards it joins; a second loop between the same two steps curls a step deeper. */
const LOOP_DIP = 44;
const LOOP_STEP = 14;

/** The recipe as the hub sent it, and the copy the editor changes. */
interface Doc { original: EngineRecipe; draft: EngineRecipe }

function svgEl<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number> = {}, cls?: string): SVGElementTagNameMap[K] {
  const node = document.createElementNS(SVG_NS, tag) as SVGElementTagNameMap[K];
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, String(v));
  if (cls) node.setAttribute('class', cls);
  return node;
}

/** The links of a recipe over its layout: a curve per edge from the card it leaves to the card it enters. */
function linksOf(recipe: EngineRecipe, lay: Layout): SVGGElement {
  const at = new Map(lay.cards.map((c) => [c.id, c]));
  const layer = svgEl('g');
  const seen = new Map<string, number>();
  for (const edge of recipe.edges) {
    const a = at.get(edge.from);
    const b = at.get(edge.to);
    if (!a || !b) continue;
    const key = pairKey(edge.from, edge.to);
    const n = seen.get(key) ?? 0;
    seen.set(key, n + 1);
    let d: string;
    let lx: number;
    let ly: number;
    if (b.x <= a.x) {
      // A loop: from the bottom of the card it leaves to the bottom of the one it returns to, dipping below both.
      const x1 = a.x + CW / 2;
      const y1 = a.y + CH;
      const x2 = b.x + CW / 2;
      const y2 = b.y + CH;
      const dip = Math.max(y1, y2) + LOOP_DIP + n * LOOP_STEP;
      d = `M${x1},${y1} C${x1},${dip} ${x2},${dip} ${x2},${y2}`;
      lx = (x1 + x2) / 2;
      ly = (y1 + y2) / 8 + (dip * 3) / 4;
    } else {
      const x1 = a.x + CW;
      const y1 = a.y + CH / 2;
      const x2 = b.x;
      const y2 = b.y + CH / 2;
      const mid = (x1 + x2) / 2;
      d = `M${x1},${y1} C${mid},${y1} ${mid},${y2} ${x2},${y2}`;
      lx = mid;
      ly = (y1 + y2) / 2;
    }
    const label = svgEl('text', { x: lx, y: ly + n * 12 - 6, 'text-anchor': 'middle' }, 'rg-edge-label');
    label.textContent = edgeLabel(edge);
    layer.append(svgEl('path', { d, 'marker-end': 'url(#rg-arrow)' }, `wk-route${isLoop(edge) ? ' dash' : ''}`), label);
  }
  return layer;
}

/** The map of a recipe: its links under its step cards, in the Work screen's map. A card is a button; a click picks it. */
function graphBody(recipe: EngineRecipe, lay: Layout, active: string | null, pick: (id: string) => void): HTMLElement {
  const height = lay.height + LOOP_DIP * 2;
  const svg = svgEl('svg', { width: lay.width, height, 'aria-hidden': 'true' });
  const defs = svgEl('defs');
  const marker = svgEl('marker', { id: 'rg-arrow', viewBox: '0 0 10 10', refX: 9, refY: 5, markerWidth: 7, markerHeight: 7, orient: 'auto-start-reverse' });
  marker.append(svgEl('path', { d: 'M0,0 L10,5 L0,10 z' }, 'wk-arrow'));
  defs.append(marker);
  svg.append(defs, linksOf(recipe, lay));
  const map = h('div.wk-map', { style: `width:${lay.width}px;height:${height}px`, role: 'group', 'aria-label': 'The steps and edges of the recipe' }, svg);
  const at = new Map(lay.cards.map((c) => [c.id, c]));
  for (const step of recipe.steps) {
    const c = at.get(step.id);
    if (!c) continue;
    const card = h(
      'button.wk-card',
      { type: 'button', style: `left:${c.x}px;top:${c.y}px;width:${CW}px;height:${CH}px`, 'aria-pressed': step.id === active ? 'true' : 'false', 'data-id': step.id },
      h('span.wk-id', null, h('span', null, step.id), h('span', null, step.seat ?? '')),
      h('span.wk-card-title', null, step.role || 'No role'),
      h('span.wk-line', null, stepLine(step)),
    );
    card.addEventListener('click', () => pick(step.id));
    map.append(card);
  }
  return h('div.wk-scroll', null, map);
}

function reason(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export function mount(root: HTMLElement, params: string[]): Cleanup {
  const id = params[0] ?? '';
  const view = h('div.rg');
  root.append(view);

  let alive = true;
  let loaded = false;
  let failed: string | null = null;
  let list: EngineRecipeSummary[] | null = null;
  let doc: Doc | null = null;
  let selected: string | null = null;
  let saving = false;
  let saveError: string | null = null;
  let parts: { graph: HTMLElement; side: HTMLElement; foot: HTMLElement } | null = null;
  let layouter: GraphLayout | null = null;
  let layoutError = '';
  let layoutKey = '';
  let layout: Layout | null = null;

  const setDraft = (draft: EngineRecipe): void => {
    if (doc) doc = { original: doc.original, draft };
  };

  /** The layout of the draft, kept while its steps and links are the same: cards never move under the editor. */
  const layoutOf = (recipe: EngineRecipe): Layout | null => {
    if (!layouter) return null;
    const graphInput = layoutInput(recipe);
    const key = JSON.stringify(graphInput);
    if (key !== layoutKey || !layout) {
      try {
        layout = layouter.layout(graphInput, CARD);
        layoutKey = key;
        layoutError = '';
      } catch (err) {
        layoutError = `Could not lay out the graph: ${reason(err)}`;
        layout = null;
      }
    }
    return layout;
  };

  const pick = (stepId: string): void => {
    selected = stepId;
    drawGraph();
    drawSide();
  };

  const drawGraph = (): void => {
    if (!doc || !parts) return;
    const recipe = doc.draft;
    const lay = layoutOf(recipe);
    let body: HTMLElement;
    if (!lay) body = h('p.rg-note', null, layoutError || 'Laying out the graph…');
    else if (recipe.steps.length === 0) body = h('p.rg-note', null, 'This recipe has no steps yet. Add one in the panel.');
    else body = graphBody(recipe, lay, selected, pick);
    replace(parts.graph, h('div.rg-row', null, h('h2', null, 'Steps and edges')), body);
  };

  /** Changes one step's fields. The graph and the save line follow; the panel keeps its inputs (and the reader's place in them). */
  const edit = (patch: Partial<EngineStep>): void => {
    if (!doc || selected === null) return;
    setDraft(patchStep(doc.draft, selected, patch));
    drawGraph();
    drawFoot();
  };

  const rename = (to: string): void => {
    if (!doc || selected === null) return;
    const next = to.trim();
    if (next !== '' && next !== selected) {
      setDraft(renameStep(doc.draft, selected, next));
      selected = next;
    }
    draw();
  };

  const deleteStep = (stepId: string): void => {
    if (!doc) return;
    setDraft(removeStep(doc.draft, stepId));
    selected = null;
    draw();
  };

  const deleteEdge = (index: number): void => {
    if (!doc) return;
    setDraft(removeEdge(doc.draft, index));
    draw();
  };

  const addNew = (): void => {
    if (!doc) return;
    const next = nextStepId(doc.draft);
    setDraft(addStep(doc.draft, next));
    selected = next;
    draw();
  };

  /** A text field for one of a step's string fields: empty takes the field out. */
  const textField = (label: string, value: string | undefined, key: 'seat' | 'role' | 'engine' | 'output'): HTMLElement => {
    const el = input({ value: value ?? '' });
    el.addEventListener('input', () => edit({ [key]: el.value || undefined } as Partial<EngineStep>));
    return field(label, el);
  };

  const stepForm = (step: EngineStep): HTMLElement => {
    const idInput = input({ value: step.id });
    idInput.addEventListener('change', () => rename(idInput.value));

    let model: HTMLElement;
    if (typeof step.model === 'string' && MODEL_WORDS.includes(step.model)) {
      const sel = select(MODEL_WORDS.map((w) => ({ value: w, label: w })), step.model);
      sel.addEventListener('change', () => edit({ model: sel.value }));
      model = field('Model', sel);
    } else {
      model = field('Model', h('span.rg-fixed', null, modelText(step.model)), 'Set in the file; shown as it is.');
    }

    const fanHint = h('small.hint', null, '');
    const fan = input({ value: String(step.fan_out), placeholder: '1' });
    fan.addEventListener('input', () => {
      const parsed = parseFanOut(fan.value);
      if ('value' in parsed) {
        fanHint.textContent = '';
        edit({ fan_out: parsed.value });
      } else {
        fanHint.textContent = parsed.problem;
      }
    });

    const shareHint = h('small.hint', null, '');
    const share = input({ value: step.budget_share === undefined ? '' : String(step.budget_share), type: 'number', min: '0', step: 'any' });
    share.addEventListener('input', () => {
      const parsed = parseShare(share.value);
      if ('value' in parsed) {
        shareHint.textContent = '';
        edit({ budget_share: parsed.value });
      } else {
        shareHint.textContent = parsed.problem;
      }
    });

    const gate = input({ value: step.gate.join(', ') });
    gate.addEventListener('input', () => edit({ gate: listOf(gate.value) }));
    const tools = input({ value: step.tools.join(', ') });
    tools.addEventListener('input', () => edit({ tools: listOf(tools.value) }));

    return h(
      'div.rg-form',
      null,
      field('Id', idInput),
      textField('Seat', step.seat, 'seat'),
      textField('Role', step.role, 'role'),
      model,
      textField('Engine', step.engine, 'engine'),
      h('div', null, field('Fan-out', fan, 'A count, or per_file, per_target, per_finding.'), fanHint),
      field('Gate', gate, 'Conditions the step must meet. Empty means passed.'),
      field('Tools', tools, 'Comma separated.'),
      h('div', null, field('Budget share', share), shareHint),
      textField('Output', step.output, 'output'),
    );
  };

  const edgeList = (recipe: EngineRecipe): HTMLElement => {
    if (recipe.edges.length === 0) return h('p.rg-note', null, 'No edges yet.');
    return h(
      'ul.rg-edges',
      null,
      recipe.edges.map((edge, i) =>
        h(
          'li',
          null,
          h(
            'span',
            null,
            h('span.mono', null, `${edge.from} → ${edge.to}`),
            h('span.rg-when', null, isLoop(edge) ? `${edge.when}, at most ${edge.max} times` : edge.when),
          ),
          button('Delete', () => deleteEdge(i), { kind: 'ghost' }),
        ),
      ),
    );
  };

  const edgeAdder = (recipe: EngineRecipe): HTMLElement => {
    const ids = recipe.steps.map((step) => step.id);
    if (ids.length === 0) return h('p.rg-note', null, 'Add a step before an edge.');
    const options = ids.map((v) => ({ value: v, label: v }));
    const from = select(options, selected ?? ids[0]);
    const to = select(options, ids[1] ?? ids[0]);
    const when = input({ list: 'rg-when-options', placeholder: 'passed, findings>0, ...' });
    const bound = input({ type: 'number', min: '1', placeholder: 'none' });
    const hint = h('small.hint', null, '');
    const add = (): void => {
      const condition = when.value.trim();
      const count = parseCount(bound.value);
      if (condition === '') {
        hint.textContent = 'Give the edge a condition.';
        return;
      }
      if ('problem' in count) {
        hint.textContent = count.problem;
        return;
      }
      if (!doc) return;
      const edge: EngineEdge = { from: from.value, to: to.value, when: condition };
      if (count.value !== undefined) edge.max = count.value;
      setDraft(addEdge(doc.draft, edge));
      draw();
    };
    return h(
      'div.rg-form',
      null,
      field('From', from),
      field('To', to),
      field('When', when, 'A condition from the list, or one typed in.'),
      h('datalist', { id: 'rg-when-options' }, WHEN_OPTIONS.map((w) => h('option', { value: w }))),
      field('At most', bound, 'Set for a loop: the edge is taken at most this often.'),
      hint,
      button('Add edge', add, { kind: 'ghost' }),
    );
  };

  const drawSide = (): void => {
    if (!doc || !parts) return;
    const recipe = doc.draft;
    const step = recipe.steps.find((s) => s.id === selected);
    replace(
      parts.side,
      h(
        'section.rg-block',
        null,
        h('div.rg-row', null, h('h2', null, 'Step'), h('span.rg-sp'), button('Add step', addNew, { kind: 'ghost' })),
        step ? stepForm(step) : h('p.rg-note', null, 'Choose a step on the graph to edit it.'),
        step ? button('Delete step', () => deleteStep(step.id), { kind: 'ghost' }) : null,
      ),
      h('section.rg-block', null, h('h2', null, 'Edges'), edgeList(recipe), edgeAdder(recipe)),
    );
  };

  const discard = (): void => {
    if (!doc) return;
    setDraft(cloneRecipe(doc.original));
    saveError = null;
    draw();
  };

  const save = async (): Promise<void> => {
    if (!doc || saving) return;
    const next: EngineRecipe = { ...doc.draft, version: doc.original.version + 1 };
    saveError = problemOf(next);
    if (saveError) return drawFoot();
    saving = true;
    drawFoot();
    let saved = false;
    try {
      await review.putRecipe(id, recipeToml(next));
      saved = true;
    } catch (err) {
      saveError = reason(err);
    } finally {
      saving = false;
    }
    if (saved) {
      toast(`Saved as version ${next.version}`);
      await load();
    } else {
      drawFoot();
    }
  };

  const drawFoot = (): void => {
    if (!doc || !parts) return;
    const { original, draft } = doc;
    const dirty = JSON.stringify(draft) !== JSON.stringify(original);
    const problem = problemOf(draft);
    const version = original.version + 1;
    replace(
      parts.foot,
      saveError ? h('p.rg-error', { role: 'alert' }, saveError) : null,
      h(
        'div.rg-row',
        null,
        h('span.rg-hint', null, dirty ? (problem ?? 'Not saved yet.') : 'No changes to save.'),
        button('Discard', discard, { kind: 'ghost', disabled: !dirty || saving }),
        button(`Save as version ${version}`, () => void save(), { kind: 'primary', disabled: !dirty || saving }),
      ),
      h('details.rg-toml-box', null, h('summary', null, 'The TOML this saves'), h('pre.rg-toml', null, recipeToml({ ...draft, version }))),
    );
  };

  const detailPage = (d: Doc): HTMLElement => {
    const r = d.draft;
    parts = { graph: h('section.rg-block'), side: h('div.rg-side'), foot: h('div.rg-foot') };
    const page = h(
      'div.rg',
      null,
      h('p', null, h('a.btn.ghost.sm', { href: href('recipe-graph') }, lu('arrow-left', 14), h('span', null, 'Recipe graphs'))),
      h(
        'div.rg-head',
        null,
        h(
          'div',
          null,
          h('h1', null, r.name || r.id),
          h('div.rg-meta', null, h('code', null, r.id), badge(`version ${r.version}`, 'info'), h('span.muted', null, r.scope)),
        ),
      ),
      h('div.rg-main', null, parts.graph, parts.side),
      parts.foot,
    );
    drawGraph();
    drawSide();
    drawFoot();
    return page;
  };

  const listPage = (rows: EngineRecipeSummary[]): HTMLElement => {
    const cols: Column<EngineRecipeSummary>[] = [
      { label: 'Recipe', cell: (r) => h('div', null, h('strong', null, r.name || r.id), h('div.mono.pr-note', null, r.id)) },
      { label: 'Version', num: true, cell: (r) => `v${r.version}` },
      { label: 'Scope', cell: (r) => r.scope },
      { label: 'Steps', num: true, cell: (r) => r.steps },
      { label: 'Source', cell: (r) => r.source },
    ];
    return h(
      'div.rg',
      null,
      h('h1', null, 'Recipe graphs'),
      h('p.muted', null, 'The recipes the engine runs. Open one to see its steps and edges, and to edit them.'),
      rows.length
        ? h('div.tbl-wrap', null, table(cols, rows, (r) => go('recipe-graph', r.id), (r) => r.id))
        : empty('rules', 'No recipes yet', 'The engine has no recipes to open.'),
    );
  };

  const draw = (): void => {
    if (!alive) return;
    if (id) {
      if (doc) return replace(view, detailPage(doc));
      if (!loaded) return replace(view, h('p.muted', null, 'Loading the recipe…'));
      return replace(view, empty('alert', failed ?? 'Recipe not found.'));
    }
    if (list) return replace(view, listPage(list));
    replace(view, failed ? empty('alert', failed) : h('p.muted', null, 'Loading the recipes…'));
  };

  // A failed read keeps what is on screen; it only shows its own message when there is nothing to show.
  const load = async (): Promise<void> => {
    try {
      if (id) {
        const d = await review.engineRecipe(id);
        doc = { original: d.recipe, draft: cloneRecipe(d.recipe) };
        if (!d.recipe.steps.some((s) => s.id === selected)) selected = null;
      } else {
        list = await review.engineRecipes();
      }
      failed = null;
    } catch (err) {
      const notFound = err instanceof ApiError && err.status === 404;
      if (id) {
        if (notFound || !doc) {
          doc = null;
          failed = notFound ? 'Recipe not found.' : 'Could not load the recipe.';
        }
      } else if (!list) {
        failed = 'Could not load the recipes.';
      }
    }
    loaded = true;
    draw();
  };

  void load();
  if (id) {
    loadGraph().then(
      (graph) => {
        layouter = graph;
        if (alive) drawGraph();
      },
      (err: unknown) => {
        layoutError = `Could not lay out the graph: ${reason(err)}`;
        if (alive) drawGraph();
      },
    );
  }

  return () => {
    alive = false;
  };
}
