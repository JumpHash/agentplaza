// The task graph's layout (crates/apz-graph-web), loaded on first use. The console never fetches it at start: nothing
// imports this module then, and `loadGraph()` imports the glue (and so the wasm) only when the Work screen first draws a
// graph. The wasm's one export takes and returns JSON text; the wrapper here gives callers the shapes below.

/** The card size and gaps, in CSS pixels; any left out take the desk sizes (232 x 104 cards, 46 and 14 gaps, 24 pad). */
export interface LayoutOptions {
  card_width?: number;
  card_height?: number;
  gap_x?: number;
  gap_y?: number;
  pad?: number;
}

export interface LayoutCard {
  id: string;
  column: number;
  row: number;
  x: number;
  y: number;
  width: number;
  height: number;
}

/** One link's corners, from the middle of its source card's right side to the middle of its target card's left side. */
export interface LayoutRoute {
  from: string;
  to: string;
  points: [number, number][];
}

export interface Layout {
  width: number;
  height: number;
  /** Each column's task ids, top to bottom. */
  columns: string[][];
  cards: LayoutCard[];
  routes: LayoutRoute[];
}

/** What the layout needs of a graph: its task ids and its links as `[from, to]` (`to` runs after `from`). */
export interface GraphInput {
  tasks: { id: string }[];
  edges: [string, string][];
}

/** The glue's exports, as wasm-bindgen generates them (`--target web`). */
export interface GraphGlue {
  default(): Promise<unknown>;
  layout(graph: string, options: string): string;
}

/** The layout, typed. A refusal (sizes it cannot lay out) throws an Error with apz-graph's message. */
export interface GraphLayout {
  layout(graph: GraphInput, options?: LayoutOptions): Layout;
}

/** The typed wrapper over a glue. Exported so the wrapping is testable without the wasm. Only the ids and the links
 * cross: the layout reads nothing else. */
export function wrapGraph(glue: GraphGlue): GraphLayout {
  return {
    layout: (graph, options) =>
      JSON.parse(
        glue.layout(
          JSON.stringify({ tasks: graph.tasks.map((t) => ({ id: t.id })), edges: graph.edges }),
          options ? JSON.stringify(options) : '',
        ),
      ) as Layout,
  };
}

/** The glue's file name at the dist root: esbuild replaces this at build time (null when the graph crate is missing). */
declare const __APZ_GRAPH_JS__: string | null;

let loading: Promise<GraphLayout> | null = null;

/**
 * The layout, loaded and initialised on the first call; later calls share it. Rejects when this build has no layout
 * or the wasm does not load; a failed load is retried by the next call.
 */
export function loadGraph(): Promise<GraphLayout> {
  if (!loading) {
    loading = (async () => {
      const name = typeof __APZ_GRAPH_JS__ === 'string' ? __APZ_GRAPH_JS__ : '';
      if (name === '') throw new Error('this build has no graph layout');
      const glueUrl = new URL('./' + name, document.baseURI).href;
      const glue = (await import(/* @vite-ignore */ glueUrl)) as GraphGlue;
      await glue.default();
      return wrapGraph(glue);
    })();
    loading.catch(() => {
      loading = null;
    });
  }
  return loading;
}
