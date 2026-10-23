// The recipe graph editor's pure helpers: the TOML a save sends, the checks a save runs before the hub, the edits to a
// draft recipe, an edge's label and its loop, the input the layout takes, and a run's route and view model (the classes
// and words of its states). No DOM and no runtime imports, so the tests load this file on its own.

import type { EngineEdge, EngineRecipe, EngineStep, RecipeNodeState, RecipeRun, RecipeRunEdge, RecipeRunGraph, RecipeRunState } from '../api-review';
import type { Tone } from '../ui/components';

/** The conditions an edge's `when` offers. Any other condition is typed in (`findings == 2`, `score >= 4`). */
export const WHEN_OPTIONS = ['passed', 'failed', 'tests green', 'no findings', 'findings>0', 'owner'];
/** The words a step's `fan_out` takes besides a count. */
export const FAN_WORDS = ['per_file', 'per_target', 'per_finding'];
/** The models the editor offers. Any other model is shown as the file has it. */
export const MODEL_WORDS = ['any', 'differ'];

/** A field's value as typed, or the problem with it. */
export type Parsed<T> = { value: T } | { problem: string };

const BARE_KEY = /^[A-Za-z0-9_-]+$/;

function tomlKey(key: string): string {
  return BARE_KEY.test(key) ? key : JSON.stringify(key);
}

/** The `key = value` pairs of a table, in the order the object has them. Values with no TOML form, and empty arrays, are left out. */
function pairsOf(table: object): [string, string][] {
  const out: [string, string][] = [];
  for (const [key, value] of Object.entries(table)) {
    if (Array.isArray(value) && value.length === 0) continue;
    const text = tomlValue(value);
    if (text !== null) out.push([key, text]);
  }
  return out;
}

/** A value as TOML, or null when it has no TOML form (null, undefined, NaN). JSON's string escapes are TOML's too. */
export function tomlValue(value: unknown): string | null {
  if (typeof value === 'string') return JSON.stringify(value);
  if (typeof value === 'number') return Number.isFinite(value) ? String(value) : null;
  if (typeof value === 'boolean') return String(value);
  if (Array.isArray(value)) {
    const items = value.map(tomlValue).filter((item): item is string => item !== null);
    return `[${items.join(', ')}]`;
  }
  if (value !== null && typeof value === 'object') {
    const pairs = pairsOf(value).map(([key, text]) => `${tomlKey(key)} = ${text}`);
    return pairs.length ? `{ ${pairs.join(', ')} }` : '{}';
  }
  return null;
}

/** The recipe as TOML: its header keys, then a `[[steps]]` table per step and an `[[edges]]` table per edge. Comments the file had are not kept. */
export function recipeToml(recipe: EngineRecipe): string {
  const { steps, edges, ...head } = recipe;
  const lines = pairsOf(head).map(([key, text]) => `${tomlKey(key)} = ${text}`);
  for (const step of steps) lines.push('', '[[steps]]', ...pairsOf(step).map(([key, text]) => `${tomlKey(key)} = ${text}`));
  for (const edge of edges) lines.push('', '[[edges]]', ...pairsOf(edge).map(([key, text]) => `${tomlKey(key)} = ${text}`));
  return `${lines.join('\n')}\n`;
}

function isSet(value: unknown): boolean {
  return value !== undefined && value !== null;
}

/** The first problem the hub would refuse, found before a save: a step without a unique id, an edge that names no step, a bad count. Null when none. */
export function problemOf(recipe: EngineRecipe): string | null {
  const ids = new Set<string>();
  for (const step of recipe.steps) {
    if (step.id.trim() === '') return 'Every step needs an id.';
    if (ids.has(step.id)) return `Two steps are named "${step.id}". Step ids must be unique.`;
    ids.add(step.id);
    const fanOk = typeof step.fan_out === 'number' ? Number.isInteger(step.fan_out) && step.fan_out >= 1 : FAN_WORDS.includes(step.fan_out);
    if (!fanOk) return `Step "${step.id}": fan_out is a count of 1 or more, or ${FAN_WORDS.join(', ')}.`;
    const share = step.budget_share as number;
    if (isSet(step.budget_share) && !(Number.isFinite(share) && share > 0 && share <= 1)) {
      return `Step "${step.id}": budget_share is a number above 0 and at most 1.`;
    }
  }
  for (const edge of recipe.edges) {
    if (!ids.has(edge.from)) return `An edge starts at "${edge.from}", which is no step.`;
    if (!ids.has(edge.to)) return `An edge ends at "${edge.to}", which is no step.`;
    if (edge.when.trim() === '') return `The edge from "${edge.from}" to "${edge.to}" has no condition.`;
    if (isSet(edge.max) && !(Number.isInteger(edge.max) && (edge.max as number) >= 1)) {
      return `The edge from "${edge.from}" to "${edge.to}": max is a count of 1 or more.`;
    }
  }
  return null;
}

/** A count as typed: a whole number of 1 or more, or empty (not set). */
export function parseCount(text: string): Parsed<number | undefined> {
  const t = text.trim();
  if (t === '') return { value: undefined };
  const n = Number(t);
  return Number.isInteger(n) && n >= 1 ? { value: n } : { problem: 'Enter a whole number, 1 or more.' };
}

/** A step's fan_out as typed: a count (empty is 1), or one of the words the engine knows. */
export function parseFanOut(text: string): Parsed<number | string> {
  const t = text.trim();
  if (t === '') return { value: 1 };
  if (FAN_WORDS.includes(t)) return { value: t };
  const n = parseCount(t);
  if ('problem' in n || n.value === undefined) return { problem: `Enter a count of 1 or more, or ${FAN_WORDS.join(', ')}.` };
  return { value: n.value };
}

/** A budget share as typed: a number, 0 or more, or empty (not set). */
export function parseShare(text: string): Parsed<number | undefined> {
  const t = text.trim();
  if (t === '') return { value: undefined };
  const n = Number(t);
  return Number.isFinite(n) && n >= 0 ? { value: n } : { problem: 'Enter a number, 0 or more.' };
}

/** A comma list as typed: its items, trimmed, without the empty ones. */
export function listOf(text: string): string[] {
  return text.split(',').map((item) => item.trim()).filter((item) => item !== '');
}

/** The model as the card and the form show it: a word, or the list of models joined. */
export function modelText(model: string | string[]): string {
  return Array.isArray(model) ? model.join(', ') : model;
}

/** A step's card line: its model and its gate (an empty gate is the default, passed). */
export function stepLine(step: EngineStep): string {
  return `${modelText(step.model)} · gate ${step.gate.length ? step.gate.join(', ') : 'passed'}`;
}

/** A loop edge: one with a bound (`max`) that the engine takes at most that many times. */
export function isLoop(edge: EngineEdge): boolean {
  return isSet(edge.max);
}

/** An edge's label on the graph: its condition, and its bound when it is a loop. */
export function edgeLabel(edge: EngineEdge): string {
  return isLoop(edge) ? `${edge.when} (max ${edge.max})` : edge.when;
}

/** The key two steps' link is kept under: the pair of ids. */
export function pairKey(from: string, to: string): string {
  return `${from}\u0000${to}`;
}

/** What the layout takes: the steps by id, and each linked pair once (a repeated pair is one link to the layout). */
export function layoutInput(recipe: EngineRecipe): { tasks: { id: string }[]; edges: [string, string][] } {
  const ids = new Set(recipe.steps.map((step) => step.id));
  const seen = new Set<string>();
  const edges: [string, string][] = [];
  for (const edge of recipe.edges) {
    if (!ids.has(edge.from) || !ids.has(edge.to)) continue;
    const key = pairKey(edge.from, edge.to);
    if (seen.has(key)) continue;
    seen.add(key);
    edges.push([edge.from, edge.to]);
  }
  return { tasks: recipe.steps.map((step) => ({ id: step.id })), edges };
}

/** A copy of the recipe the editor can change without touching the one the hub sent. */
export function cloneRecipe(recipe: EngineRecipe): EngineRecipe {
  return JSON.parse(JSON.stringify(recipe)) as EngineRecipe;
}

/** The step with its fields patched: a field given `undefined` is taken out, so the hub's default applies. */
export function patchStep(recipe: EngineRecipe, id: string, patch: Partial<EngineStep>): EngineRecipe {
  return {
    ...recipe,
    steps: recipe.steps.map((step) => {
      if (step.id !== id) return step;
      const next = { ...step, ...patch };
      for (const key of Object.keys(next)) {
        if (next[key] === undefined) delete next[key];
      }
      return next;
    }),
  };
}

/** The recipe with a step renamed; the edges that name it follow. */
export function renameStep(recipe: EngineRecipe, from: string, to: string): EngineRecipe {
  const follow = (id: string): string => (id === from ? to : id);
  return {
    ...recipe,
    steps: recipe.steps.map((step) => (step.id === from ? { ...step, id: to } : step)),
    edges: recipe.edges.map((edge) => ({ ...edge, from: follow(edge.from), to: follow(edge.to) })),
  };
}

/** A first id no step has (`step-N`). */
export function nextStepId(recipe: EngineRecipe): string {
  const ids = new Set(recipe.steps.map((step) => step.id));
  for (let n = recipe.steps.length + 1; ; n++) {
    const id = `step-${n}`;
    if (!ids.has(id)) return id;
  }
}

/** The recipe with a step added at the end: a model of any, one fan-out, no gate. */
export function addStep(recipe: EngineRecipe, id: string): EngineRecipe {
  return { ...recipe, steps: [...recipe.steps, { id, model: 'any', tools: [], fan_out: 1, gate: [] }] };
}

/** The recipe without a step, and without the edges that touch it. */
export function removeStep(recipe: EngineRecipe, id: string): EngineRecipe {
  return {
    ...recipe,
    steps: recipe.steps.filter((step) => step.id !== id),
    edges: recipe.edges.filter((edge) => edge.from !== id && edge.to !== id),
  };
}

/** The recipe with an edge added at the end. */
export function addEdge(recipe: EngineRecipe, edge: EngineEdge): EngineRecipe {
  return { ...recipe, edges: [...recipe.edges, edge] };
}

/** The recipe without the edge at `index`. */
export function removeEdge(recipe: EngineRecipe, index: number): EngineRecipe {
  return { ...recipe, edges: recipe.edges.filter((_, i) => i !== index) };
}

// ---- The run view: a recipe run's graph, read-only -----------------------------------------------------------------------

const NODE_LABELS: Record<RecipeNodeState, string> = { waiting: 'Waiting', ready: 'Ready', running: 'Running', passed: 'Passed', failed: 'Failed', skipped: 'Skipped' };
const RUN_LABELS: Record<RecipeRunState, string> = { running: 'Running', owner: 'Waiting for you', passed: 'Passed', failed: 'Failed', cancelled: 'Cancelled' };
const RUN_TONES: Record<RecipeRunState, Tone> = { running: 'info', owner: 'warn', passed: 'ok', failed: 'danger', cancelled: 'muted' };

/** The run a route names: `#/recipe-graph/run/<id>` gives the params `run` and the id. Null for any other route. */
export function runIdOf(params: string[]): string | null {
  return params[0] === 'run' && (params[1] ?? '') !== '' ? params[1]! : null;
}

/** The class a step takes for its state; `rr-none` when the run has no walk for the step. */
export function nodeClassOf(state: RecipeNodeState | null): string {
  return state ? `rr-${state}` : 'rr-none';
}

/** A step's state as the page words it; `No walk` when the run has none for the step. */
export function nodeLabelOf(state: RecipeNodeState | null): string {
  return state ? NODE_LABELS[state] : 'No walk';
}

/** A link's class: a taken link is solid; an open or dead one is dashed and faded (dead is the faintest). */
export function edgeClassOf(edge: Pick<RecipeRunEdge, 'state' | 'taken'>): string {
  if (edge.taken) return 'rr-taken';
  return edge.state === 'dead' ? 'rr-dead' : 'rr-open';
}

/** A run's state as the page words it. */
export function runLabelOf(state: RecipeRunState): string {
  return RUN_LABELS[state];
}

/** The badge tone of a run's state. */
export function runToneOf(state: RecipeRunState): Tone {
  return RUN_TONES[state];
}

/** A value the hub sent, as text: a list joined, a string or number as it is, nothing for null. */
export function plainOf(value: unknown): string {
  if (value === null || value === undefined) return '';
  if (Array.isArray(value)) return value.map(plainOf).join(', ');
  if (typeof value === 'object') return JSON.stringify(value);
  return String(value);
}

/** A run's graph as the page draws it: the steps and links as a recipe (for the layout and the cards), and each step and link with its classes and words. */
export interface RunView {
  recipe: EngineRecipe;
  nodes: { id: string; cls: string; label: string; visits: number; items: number; reported: number; gateRead: string }[];
  edges: { index: number; from: string; to: string; cls: string; taken: boolean; dashed: boolean }[];
}

/** The view model of a run's graph: the steps as the editor's recipe shape, and the classes the cards and links take. */
export function runViewOf(graph: RecipeRunGraph): RunView {
  const steps: EngineStep[] = graph.nodes.map((node) => ({
    id: node.id,
    seat: node.seat ?? undefined,
    role: node.role ?? undefined,
    model: node.model ?? '',
    engine: node.engine ?? undefined,
    tools: [],
    fan_out: node.fan_out,
    gate: node.gate,
  }));
  const edges: EngineEdge[] = graph.edges.map((edge) => ({ from: edge.from, to: edge.to, when: edge.when, ...(isSet(edge.max) ? { max: edge.max as number } : {}) }));
  return {
    recipe: { id: graph.recipe, name: graph.recipe, version: 0, scope: '', inputs: [], outputs: [], triggers: [], steps, edges },
    nodes: graph.nodes.map((node) => ({
      id: node.id,
      cls: nodeClassOf(node.state),
      label: nodeLabelOf(node.state),
      visits: node.visits,
      items: node.items,
      reported: node.reported,
      gateRead: plainOf(node.gate_read),
    })),
    edges: graph.edges.map((edge) => ({
      index: edge.index,
      from: edge.from,
      to: edge.to,
      cls: edgeClassOf(edge),
      taken: edge.taken,
      dashed: !edge.taken,
    })),
  };
}

/** A run's words in a list: its recipe, its version and its trigger. */
export function runSummaryOf(run: RecipeRun): string {
  return `${run.recipe} v${run.version} · ${run.trigger}`;
}
