// The review graph's API: the recipes, a task's reviews and their change trees, and the owner's call on a review
// card. Same origin and the same `call` as the rest of the console; the types follow the hub's real answers.

import { approvalPath, call } from './api';

/** One of the four plain readings of a review, with its level -5..5 (null: not measured). */
export interface ReviewReading { name: string; level: number | null }

/** What the gate decided for a round: `pass`, `rework` (back to the worker), `owner` (a card for the owner), `fail`, `replan`. */
export interface ReviewOutcome { decision: string; reasons: string[]; replan: string[]; at_ms: number; by: string }

/** A node as the run's list gives it (the flat form). */
export interface ReviewNodeBrief { id: string; kind: string; role: string | null; state: string; assignee: number | null; parent: string | null }

/** One run of a task's review (a round of a sealed revision), as `GET /v1/tasks/{id}/reviews` lists it, newest first. */
export interface ReviewRun {
  run: string; task: string; change: string; revision: string; recipe: string;
  round: number; rounds: number; kind: string; state: string; verdict: string;
  outcome: ReviewOutcome | null; readings: ReviewReading[];
  nodes: ReviewNodeBrief[]; created_ms: number; updated_ms: number;
}

export interface TaskReviews { task: string; runs: ReviewRun[] }

/** A step of a recipe as the recipe defines it. */
export interface NodeDef {
  id: string; kind: string; tool: string | null; role: string | null; axes: string[]; commands: string[];
  after: string[]; sub_reviews: number; constraints: boolean; ask: string;
}

/** A score a step gave a definition on one axis (-5 much worse .. +5 much better). `symbol` is a definition's id, or a file's path. */
export interface ReviewScore {
  symbol: string; lineage: string | null; axis: string; level: number; confidence: number;
  basis: string; evidence: string; node: string;
}

export interface ReviewFinding { symbol: string; lineage: string | null; text: string; severity: string; node: string; round: number }

/** What a definition touches: `how` is "uses" or "used by", `to` another definition's id. */
export interface Touch { changed: boolean; how: string; to: string }

/** A touch the code does not name outright (its shape is free text in the hub's answer). */
export type WordedTouch = string | { how?: string; to?: string; text?: string };

/** One step of a run, with what it did: its scores, findings and touches, and the state it ended in. */
export interface ReviewNode {
  def: NodeDef; parent: string | null; state: string; assignee: number | null; work: number | null; tries: number;
  scores: ReviewScore[]; findings: ReviewFinding[]; touches: Touch[]; notes: string; record: number;
  started_ms: number; ended_ms: number; spawned: number;
}

/** A run in full (`GET /v1/reviews/{run}`): its steps with their results, the gate's rules, and the lows by axis. */
export interface ReviewRunDetail extends Omit<ReviewRun, 'nodes'> {
  nodes: ReviewNode[]; lows: Record<string, [number, boolean]>; rules: { when: string; then: string }[];
}

export interface TreeSymbol {
  id: string; kind: string; qualified: string; lineage: string | null; op: string;
  start_line: number; end_line: number; add: number; del: number; from: string | null;
  provenance: unknown[]; touches: Touch[]; worded_touches: WordedTouch[]; scores: ReviewScore[]; findings: ReviewFinding[];
}

export interface TreeFile {
  path: string; op: string; pack: string; binary: boolean; add: number; del: number;
  before: string | null; after: string | null; symbols: TreeSymbol[]; scores: ReviewScore[];
}

/** An intent: what the change does, the definitions it covers (their ids). */
export interface TreeIntent { title: string; kind: string; summary: string; items: string[] }

/** The change tree of a revision, with each definition's scores, findings and touches merged in. */
export interface ChangeTree {
  change: string; revision: string; base: string; add: number; del: number;
  files: TreeFile[]; intents: TreeIntent[]; unsorted: string[]; divergence: unknown[]; languages: Record<string, number>;
}

/** `GET /v1/reviews/{run}/tree`. */
export interface ReviewTree { run: string; verdict: string; readings: ReviewReading[]; lows: Record<string, [number, boolean]>; tree: ChangeTree }

export interface RecipeSummary { id: string; version: number; title: string; applies: { kinds: string[]; roles: string[] }; axes: string[]; rounds: number; nodes: number }

/** A recipe as its TOML is parsed (`node` and `rule` are the TOML's `[[node]]` and `[[rule]]` tables). */
export interface RecipeDef {
  id: string; version: number; title: string; applies: { kinds: string[]; roles: string[] }; axes: string[]; rounds: number;
  first: { when: string; only: string[]; add: string[] }[]; node: NodeDef[]; rule: { when: string; then: string }[];
}

export interface RecipeDetail { recipe: RecipeDef; toml: string }

/** A recipe change waiting for the owner. The hub's proposal record is not fixed yet, so only what the page shows is typed. */
export interface RecipeProposal { id: string; recipe?: string; version?: number; title?: string; by?: string; reason?: string; created_ms?: number; state?: string }

/** A recipe the engine runs, as `GET /v1/recipes` lists it under `engine` (`steps` is the count of its steps). */
export interface EngineRecipeSummary { id: string; name: string; version: number; scope: string; steps: number; source: string }

/**
 * A step of an engine recipe. The fields the graph editor shows are typed; any other field the hub sends rides along
 * (the editor writes it back as it came).
 */
export interface EngineStep {
  id: string; seat?: string; role?: string; model: string | string[]; engine?: string; tools: string[];
  fan_out: number | string; gate: string[]; budget_share?: number; output?: string;
  [key: string]: unknown;
}

/** An edge between two steps. `when` is a condition (`passed`, `findings>0`, ...); `max` makes it a loop taken at most that often. */
export interface EngineEdge { from: string; to: string; when: string; max?: number; [key: string]: unknown }

/** An engine recipe in full: the header, its steps and its edges. */
export interface EngineRecipe {
  id: string; name: string; version: number; scope: string;
  inputs: unknown[]; outputs: unknown[]; triggers: unknown[];
  steps: EngineStep[]; edges: EngineEdge[];
  [key: string]: unknown;
}

export interface EngineRecipeDetail { recipe: EngineRecipe; toml: string; engine: true }

export interface RecipeList { recipes: RecipeSummary[]; proposals: RecipeProposal[]; engine?: EngineRecipeSummary[] }

/** A run's state: `running`, `owner` (a card waits for the owner), or how it ended (`passed`, `failed`, `cancelled`). */
export type RecipeRunState = 'running' | 'owner' | 'passed' | 'failed' | 'cancelled';

/** A check a run's step made: whether it passed, the commands it ran, and the log it wrote (`summary`). */
export interface RecipeRunCheck { step: string; passed: boolean; commands: string; summary: string }

/** A run of an engine recipe, as `GET /v1/recipe-runs` lists it (newest first) and `GET /v1/recipe-runs/{id}` gives it. `checks` is set on the detail (a workflow run's checks). */
export interface RecipeRun {
  run: string; recipe: string; version: number; source: string; trigger: string;
  task: string | null; project: string | null; subject: string; state: RecipeRunState; outcome: string | null;
  created_ms: number; updated_ms: number; checks?: RecipeRunCheck[];
}

export interface RecipeRunList { runs: RecipeRun[] }

/** A step's state in a run; null when the run has no walk for the step. */
export type RecipeNodeState = 'waiting' | 'ready' | 'running' | 'passed' | 'failed' | 'skipped';

/** An edge's state in a run: `open` (not decided yet), `taken`, or `dead` (the walk went another way). */
export type RecipeEdgeState = 'open' | 'taken' | 'dead';

/** A step of a run's graph: its definition, how far the walk got, and what it reported. `gate_read` is shown as the hub sends it. */
export interface RecipeRunNode {
  id: string; seat: string | null; role: string | null; engine: string | null; model: string | string[] | null;
  fan_out: number | string; gate: string[]; state: RecipeNodeState | null; visits: number; items: number; reported: number;
  gate_read: unknown;
}

export interface RecipeRunEdge { index: number; from: string; to: string; when: string; max?: number | null; state: RecipeEdgeState; taken: boolean }

/** A run's graph (`GET /v1/recipe-runs/{id}/graph`): its steps and edges, each with its state. */
export interface RecipeRunGraph { run: string; recipe: string; state: RecipeRunState; nodes: RecipeRunNode[]; edges: RecipeRunEdge[] }

/** The owner's three calls on a review card. */
export type ReviewChoice = 'pass' | 'rework' | 'fail';

const seg = (text: string): string => encodeURIComponent(text);

export const review = {
  recipes: () => call<RecipeList>('GET', '/v1/recipes'),
  recipe: (id: string, version?: number) => call<RecipeDetail>('GET', `/v1/recipes/${seg(id)}${version === undefined ? '' : `?version=${version}`}`),
  /** Saves `toml` as a new version of the recipe (the hub refuses a bad one with its reason in `error`). */
  putRecipe: (id: string, toml: string) => call<RecipeDetail>('PUT', `/v1/recipes/${seg(id)}`, { toml }),
  /** The recipes the engine can open (the graph editor's list). */
  engineRecipes: async (): Promise<EngineRecipeSummary[]> => (await call<RecipeList>('GET', '/v1/recipes')).engine ?? [],
  engineRecipe: (id: string) => call<EngineRecipeDetail>('GET', `/v1/recipes/${seg(id)}`),
  decideProposal: (id: string, approve: boolean) => call<unknown>('POST', `/v1/recipes/proposals/${seg(id)}`, { approve }),
  /** The runs of a task, of a project, or of both, newest first. */
  recipeRuns: (scope: { task?: string; project?: string }) => {
    const query = new URLSearchParams();
    if (scope.task) query.set('task', scope.task);
    if (scope.project) query.set('project', scope.project);
    return call<RecipeRunList>('GET', `/v1/recipe-runs?${query}`);
  },
  recipeRun: (runId: string) => call<RecipeRun>('GET', `/v1/recipe-runs/${seg(runId)}`),
  recipeRunGraph: (runId: string) => call<RecipeRunGraph>('GET', `/v1/recipe-runs/${seg(runId)}/graph`),
  runs: (taskId: string) => call<TaskReviews>('GET', `/v1/tasks/${seg(taskId)}/reviews`),
  run: (runId: string) => call<ReviewRunDetail>('GET', `/v1/reviews/${seg(runId)}`),
  tree: (runId: string) => call<ReviewTree>('GET', `/v1/reviews/${seg(runId)}/tree`),
  /** Pass: approve. Send back: refuse with the `rework` action. Fail: refuse. */
  decide: (approvalId: string, choice: ReviewChoice, reason?: string) =>
    call<unknown>('POST', approvalPath(approvalId), {
      approve: choice === 'pass',
      reason: reason?.trim() || undefined,
      action: choice === 'rework' ? 'rework' : undefined,
    }),
};
